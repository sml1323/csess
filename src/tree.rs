//! 계층 디렉토리 트리 — 순수 함수 모듈 (App/ratatui/I-O 무의존). [D14]
//!
//! Phase A `bin/csess` 의 awk `csess_list_tree` 를 Rust 로 옮긴다. 핵심: **디스크 스캔이 아니라
//! 세션 cwd 만으로 트리를 친다** — 모든 노드가 정의상 resume 가능한 디렉토리라 빈 폴더 노이즈 0.
//!
//! 한 레벨(prefix 아래)을 [`ListItem`] 목록으로 방출:
//!   1. UP 행 (prefix != HOME 일 때)  — 상위로
//!   2. DIR 행 (자식 첫 세그먼트, **경로압축** + **rollup 카운트**, count desc·mtime desc)
//!   3. 세션 행 (cwd 가 정확히 prefix 인 원본 세션 → resume)
//!
//! 검색은 여기서 안 한다 — 호출부(`recompute_tree`)가 [`apply_tree_filter`] 로 라벨/제목 부분일치.

use crate::index::SessionRow;
use std::collections::{HashMap, HashSet};

/// 리스트 패인의 한 행. 기존 `filtered: Vec<usize>` 를 대체한다.
/// UP/Dir 은 **합성 행**(backing SessionRow 없음), Session 은 `rows` 인덱스를 들고 O(1) resume.
#[derive(Debug, Clone)]
pub enum ListItem {
    /// `⬅  ..` — `parent` 로 드릴.
    Up { parent: String },
    /// 압축된 디렉토리 노드.
    Dir {
        /// 압축 노드의 절대경로 — 드릴 시 다음 prefix.
        node: String,
        /// 표시 라벨 = `prefix/` 를 벗긴 것 (압축 체인이면 '/' 포함 가능).
        label: String,
        /// 서브트리 전체 세션 수(rollup).
        count: usize,
        /// 서브트리 최대 mtime(epoch).
        max_mtime: i64,
    },
    /// 실제 세션 — `App.rows` 인덱스.
    Session { row_idx: usize },
}

impl ListItem {
    pub fn is_session(&self) -> bool {
        matches!(self, ListItem::Session { .. })
    }
}

/// 뷰 모드. Flat = 본문검색 평면 리스트(기본), Tree = 계층 트리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Flat,
    Tree,
}

/// 세그먼트별 누적(rollup). build_tree_level 내부 전용.
struct SegStats {
    count: usize,
    max_mtime: i64,
}

/// `cwd` 가 `base` 와 같거나 `base/` 로 시작하는가. base 다음 바이트가 '/' 인지 확인해
/// `/home/u-other` vs `/home/u` 오탐 방지. format! 할당 없이 바이트 비교.
pub fn under(cwd: &str, base: &str) -> bool {
    cwd == base
        || (cwd.len() > base.len() && cwd.as_bytes()[base.len()] == b'/' && cwd.starts_with(base))
}

/// `base/` 다음 첫 path 세그먼트. `under(cwd, base) && cwd != base` 가정.
pub fn relseg<'a>(cwd: &'a str, base: &str) -> &'a str {
    let rest = &cwd[base.len() + 1..];
    match rest.find('/') {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// HOME 상대 표시. `~`, `~/rest`, 또는 (HOME 밖이면) 원본 그대로.
pub fn homerel(path: &str, home: &str) -> String {
    if path == home {
        "~".to_string()
    } else if path.len() > home.len()
        && path.as_bytes()[home.len()] == b'/'
        && path.starts_with(home)
    {
        format!("~/{}", &path[home.len() + 1..])
    } else {
        path.to_string()
    }
}

/// 경로압축: 자기 직속 세션이 없고(=all_cwds 에 없음) 자식 세그먼트가 **정확히 하나**인
/// 단일 체인을 한 노드로 접는다(`specs` → `specs/001-vendor`). all_cwds 는 **전체 코퍼스**
/// 기준(awk 가 END 전에 cset 을 전체 라인에서 구축하는 것과 동일) — 서브트리별로 다른 압축
/// 결과가 나오는 버그 방지.
fn compress_node(start: &str, all_cwds: &HashSet<&str>) -> String {
    let mut node = start.to_string();
    loop {
        if all_cwds.contains(node.as_str()) {
            break; // 직속 세션 있음 → 더 못 접음
        }
        let mut children: HashSet<&str> = HashSet::new();
        for &cwd in all_cwds {
            if cwd != node.as_str() && under(cwd, &node) {
                children.insert(relseg(cwd, &node));
            }
        }
        if children.len() == 1 {
            let only = (*children.iter().next().unwrap()).to_string();
            node.push('/');
            node.push_str(&only);
        } else {
            break; // 자식 0개 또는 2개+ → 정지
        }
    }
    node
}

/// `p` 의 부모. HOME 밖으로 내려가지 않게 클램프(HOME 미만이면 HOME).
pub fn parent_clamped(p: &str, home: &str) -> String {
    let par = p.rsplit_once('/').map(|(l, _)| l).unwrap_or(home);
    if under(par, home) {
        par.to_string()
    } else {
        home.to_string()
    }
}

/// `prefix`(빈=ROOT=home) 아래 한 레벨을 방출. 순서: [UP?] · [DIR* count·mtime desc] · [Session* mtime desc].
/// `source`=Some("claude"|"codex")면 그 소스만(트리 모아보기), None=전체. 쿼리 필터는 안 함(호출부가
/// [`apply_tree_filter`]). compress 정확성 위해 (소스필터 반영한) 전체 rows 로 cwd_set 구축.
/// **주의:** HOME 밖 cwd(`/tmp/x` 등)는 트리에서 제외 — flat 검색으로만 도달(Phase A 도 HOME 루트).
pub fn build_tree_level(
    rows: &[SessionRow],
    prefix: &str,
    home: &str,
    source: Option<&str>,
) -> Vec<ListItem> {
    let p: &str = if prefix.is_empty() { home } else { prefix };
    let keep = |r: &SessionRow| source.is_none_or(|s| r.source == s);

    // 전체 cwd 집합(compress 용, 소스필터 반영). cwd=None(=lossy) 은 제외.
    let mut all_cwds: HashSet<&str> = HashSet::new();
    for r in rows {
        if keep(r) {
            if let Some(c) = &r.cwd {
                all_cwds.insert(c.as_str());
            }
        }
    }

    // 한 패스: 직속 세션 + 첫 세그먼트별 rollup.
    let mut own_idxs: Vec<usize> = Vec::new();
    let mut seg_stats: HashMap<String, SegStats> = HashMap::new();
    let mut seg_order: Vec<String> = Vec::new(); // 첫 등장 순서 보존(타이브레이크 안정성)

    for (idx, r) in rows.iter().enumerate() {
        if !keep(r) {
            continue;
        }
        let cwd = match &r.cwd {
            Some(c) => c.as_str(),
            None => continue,
        };
        if !under(cwd, p) {
            continue;
        }
        if cwd == p {
            own_idxs.push(idx);
            continue;
        }
        let seg = relseg(cwd, p).to_string();
        match seg_stats.get_mut(&seg) {
            Some(s) => {
                s.count += 1;
                if r.mtime > s.max_mtime {
                    s.max_mtime = r.mtime;
                }
            }
            None => {
                seg_stats.insert(seg.clone(), SegStats { count: 1, max_mtime: r.mtime });
                seg_order.push(seg);
            }
        }
    }

    // count desc, mtime desc (stable → 완전 동률은 첫 등장 순서). awk insertion sort 와 동치.
    seg_order.sort_by(|a, b| {
        let sa = &seg_stats[a];
        let sb = &seg_stats[b];
        sb.count.cmp(&sa.count).then(sb.max_mtime.cmp(&sa.max_mtime))
    });

    let mut out: Vec<ListItem> = Vec::new();
    if p != home {
        out.push(ListItem::Up { parent: parent_clamped(p, home) });
    }
    for seg in &seg_order {
        let start = format!("{}/{}", p, seg);
        let node = compress_node(&start, &all_cwds);
        let label = node[p.len() + 1..].to_string(); // "p/" 벗기기
        let st = &seg_stats[seg]; // rollup 은 압축 전 원본 세그먼트 기준(동일 — 압축은 세션 없는 단일체인만 통과)
        out.push(ListItem::Dir {
            node,
            label,
            count: st.count,
            max_mtime: st.max_mtime,
        });
    }
    for idx in own_idxs {
        out.push(ListItem::Session { row_idx: idx });
    }
    out
}

/// 트리 모드 쿼리 필터: 대소문자 무시 **부분일치, 전체 쿼리 한 패턴**(flat 의 공백=AND 와 다름 — Phase A 파리티).
/// UP 항상 통과 · Dir 은 라벨 · Session 은 제목.
pub fn apply_tree_filter(items: Vec<ListItem>, rows: &[SessionRow], query: &str) -> Vec<ListItem> {
    if query.is_empty() {
        return items;
    }
    let ql = query.to_lowercase();
    items
        .into_iter()
        .filter(|it| match it {
            ListItem::Up { .. } => true,
            ListItem::Dir { label, .. } => label.to_lowercase().contains(&ql),
            ListItem::Session { row_idx } => rows[*row_idx].title.to_lowercase().contains(&ql),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cwd: Option<&str>, mtime: i64, title: &str) -> SessionRow {
        SessionRow {
            source: "claude".into(),
            id: "id".into(),
            cwd: cwd.map(|s| s.to_string()),
            cwd_lossy: cwd.is_none(),
            path: "/no/path.jsonl".into(),
            mtime,
            title: title.into(),
            proj: "p".into(),
            body: String::new(),
        }
    }

    /// 동일 cwd 의 행 n개를 mtime 내림차순으로 생성.
    fn rows_n(cwd: &str, n: usize, base_mtime: i64) -> Vec<SessionRow> {
        (0..n).map(|i| row(Some(cwd), base_mtime - i as i64, "t")).collect()
    }

    #[test]
    fn under_basic() {
        assert!(under("/h/u/work", "/h/u"));
        assert!(under("/h/u", "/h/u"));
        assert!(!under("/h/u2", "/h/u"));
        assert!(!under("/h/u-other", "/h/u")); // 바이트 경계 오탐 방지
        assert!(!under("/x", "/h/u"));
    }

    #[test]
    fn relseg_basic() {
        assert_eq!(relseg("/h/u/work", "/h/u"), "work");
        assert_eq!(relseg("/h/u/work/csess", "/h/u"), "work");
        assert_eq!(relseg("/h/u/a/b/c", "/h/u/a"), "b");
    }

    #[test]
    fn homerel_basic() {
        assert_eq!(homerel("/h/u", "/h/u"), "~");
        assert_eq!(homerel("/h/u/work", "/h/u"), "~/work");
        assert_eq!(homerel("/h/u/work/csess", "/h/u"), "~/work/csess");
        assert_eq!(homerel("/tmp/other", "/h/u"), "/tmp/other"); // HOME 밖 → 원본
    }

    #[test]
    fn parent_clamped_basic() {
        assert_eq!(parent_clamped("/h/u/work/csess", "/h/u"), "/h/u/work");
        assert_eq!(parent_clamped("/h/u/work", "/h/u"), "/h/u"); // 부모가 HOME
        assert_eq!(parent_clamped("/h/u", "/h/u"), "/h/u"); // 이미 HOME → 클램프
    }

    #[test]
    fn compress_single_chain() {
        let mut s = HashSet::new();
        s.insert("/h/u/specs/001-vendor");
        assert_eq!(compress_node("/h/u/specs", &s), "/h/u/specs/001-vendor");
    }

    #[test]
    fn compress_multi_child_no_compression() {
        let mut s = HashSet::new();
        s.insert("/h/u/work/a");
        s.insert("/h/u/work/b");
        assert_eq!(compress_node("/h/u/work", &s), "/h/u/work"); // 자식 2개 → 정지
    }

    #[test]
    fn compress_own_session_blocks() {
        let mut s = HashSet::new();
        s.insert("/h/u/work");
        s.insert("/h/u/work/sub");
        assert_eq!(compress_node("/h/u/work", &s), "/h/u/work"); // 직속 세션 → 정지
    }

    #[test]
    fn compress_chain_of_3() {
        let mut s = HashSet::new();
        s.insert("/h/a/b/c");
        assert_eq!(compress_node("/h/a", &s), "/h/a/b/c");
    }

    #[test]
    fn build_root_fixture_full() {
        // cwd:count(maxmtime): /h/u:1(900) work:3(800) work/csess:5(700)
        //   work/회사:2(600) work/회사/test_erp:4(500) specs/001-vendor:2(400) /tmp/external:1(300)
        let mut rows = Vec::new();
        rows.extend(rows_n("/h/u", 1, 900));
        rows.extend(rows_n("/h/u/work", 3, 800));
        rows.extend(rows_n("/h/u/work/csess", 5, 700));
        rows.extend(rows_n("/h/u/work/회사", 2, 600));
        rows.extend(rows_n("/h/u/work/회사/test_erp", 4, 500));
        rows.extend(rows_n("/h/u/specs/001-vendor", 2, 400));
        rows.extend(rows_n("/tmp/external", 1, 300));
        // load_rows 는 mtime desc → 그 순서로 정렬해 재현
        rows.sort_by(|a, b| b.mtime.cmp(&a.mtime));

        let items = build_tree_level(&rows, "", "/h/u", None);
        // ROOT → UP 없음. Dir work(14, mt800), Dir specs/001-vendor(2, mt400), Session(/h/u) 1개. /tmp 제외.
        assert!(matches!(items[0], ListItem::Dir { ref label, count, max_mtime, .. }
            if label == "work" && count == 14 && max_mtime == 800));
        assert!(matches!(items[1], ListItem::Dir { ref label, count, max_mtime, .. }
            if label == "specs/001-vendor" && count == 2 && max_mtime == 400));
        assert!(matches!(items[2], ListItem::Session { .. }));
        assert_eq!(items.len(), 3); // /tmp/external 제외 확인
    }

    #[test]
    fn build_drill_work() {
        let mut rows = Vec::new();
        rows.extend(rows_n("/h/u/work", 3, 800));
        rows.extend(rows_n("/h/u/work/csess", 5, 700));
        rows.extend(rows_n("/h/u/work/회사", 2, 600));
        rows.extend(rows_n("/h/u/work/회사/test_erp", 4, 500));
        rows.sort_by(|a, b| b.mtime.cmp(&a.mtime));

        let items = build_tree_level(&rows, "/h/u/work", "/h/u", None);
        // UP(parent=/h/u) · Dir 회사(6, mt600) · Dir csess(5, mt700) · Session*3
        assert!(matches!(items[0], ListItem::Up { ref parent } if parent == "/h/u"));
        assert!(matches!(items[1], ListItem::Dir { ref label, count, max_mtime, .. }
            if label == "회사" && count == 6 && max_mtime == 600)); // 회사(2)+test_erp(4)=6
        assert!(matches!(items[2], ListItem::Dir { ref label, count, .. }
            if label == "csess" && count == 5));
        let sess = items.iter().filter(|i| i.is_session()).count();
        assert_eq!(sess, 3);
    }

    #[test]
    fn rollup_count_correctness() {
        let rows = vec![
            row(Some("/h/u/work/a"), 3, "t"),
            row(Some("/h/u/work/a/b"), 2, "t"),
            row(Some("/h/u/work/c"), 1, "t"),
        ];
        let items = build_tree_level(&rows, "", "/h/u", None);
        // 모두 첫 세그먼트 work → count 3
        assert!(matches!(items[0], ListItem::Dir { ref label, count, .. } if label == "work" && count == 3));
    }

    #[test]
    fn lossy_and_external_excluded() {
        let rows = vec![
            row(None, 5, "lossy"),          // cwd=None → 제외
            row(Some("/tmp/x"), 4, "ext"),  // HOME 밖 → 제외
            row(Some("/h/u/work"), 3, "ok"),
        ];
        let items = build_tree_level(&rows, "", "/h/u", None);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], ListItem::Dir { ref label, count, .. } if label == "work" && count == 1));
    }

    #[test]
    fn home_itself_is_own_session() {
        let rows = vec![row(Some("/h/u"), 5, "home sess"), row(Some("/h/u/work"), 4, "w")];
        let items = build_tree_level(&rows, "", "/h/u", None);
        // Dir work + Session(/h/u). HOME 용 Dir 행은 안 생김.
        assert!(matches!(items[0], ListItem::Dir { ref label, .. } if label == "work"));
        assert!(items.iter().any(|i| i.is_session()));
        assert_eq!(items.iter().filter(|i| i.is_session()).count(), 1);
    }

    #[test]
    fn empty_level_only_up() {
        let rows = vec![row(Some("/h/u/work"), 5, "w")];
        // prefix 아래 아무것도 없지만 HOME 아님 → UP 한 줄.
        let items = build_tree_level(&rows, "/h/u/empty", "/h/u", None);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], ListItem::Up { .. }));
    }

    #[test]
    fn source_filter_in_tree() {
        let mut rows = vec![
            row(Some("/h/u/work/a"), 5, "claude one"),
            row(Some("/h/u/work/b"), 4, "codex one"),
        ];
        rows[1].source = "codex".into();
        // 전체: work count=2
        let all = build_tree_level(&rows, "", "/h/u", None);
        assert!(matches!(all[0], ListItem::Dir { count, .. } if count == 2));
        // codex 만: work count=1 (work/b)
        let codex = build_tree_level(&rows, "", "/h/u", Some("codex"));
        assert!(matches!(codex[0], ListItem::Dir { count, .. } if count == 1));
        // claude 만: work count=1 (work/a)
        let claude = build_tree_level(&rows, "", "/h/u", Some("claude"));
        assert!(matches!(claude[0], ListItem::Dir { count, .. } if count == 1));
    }

    #[test]
    fn tree_filter_dir_label_and_title() {
        let rows = vec![row(Some("/h/u/work"), 1, "fix auth bug")];
        let items = vec![
            ListItem::Up { parent: "/h/u".into() },
            ListItem::Dir { node: "/h/u/work".into(), label: "work".into(), count: 1, max_mtime: 1 },
            ListItem::Dir { node: "/h/u/회사".into(), label: "회사/test_erp".into(), count: 1, max_mtime: 1 },
            ListItem::Session { row_idx: 0 },
        ];
        let out = apply_tree_filter(items, &rows, "auth");
        // UP 통과 + 'fix auth bug' 통과. work/회사 라벨 탈락.
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], ListItem::Up { .. }));
        assert!(out[1].is_session());
    }

    #[test]
    fn tree_filter_no_and_split() {
        let rows: Vec<SessionRow> = vec![];
        let items = vec![ListItem::Dir { node: "/x".into(), label: "회사/test_erp".into(), count: 1, max_mtime: 1 }];
        // 공백 포함 전체가 한 패턴 → '회사 test' 는 라벨에 없음 → 탈락 (AND 분할 아님)
        let out = apply_tree_filter(items, &rows, "회사 test");
        assert!(out.is_empty());
    }

    #[test]
    fn tree_filter_empty_passes_all() {
        let rows: Vec<SessionRow> = vec![];
        let items = vec![
            ListItem::Up { parent: "/h/u".into() },
            ListItem::Dir { node: "/x".into(), label: "work".into(), count: 1, max_mtime: 1 },
        ];
        let out = apply_tree_filter(items, &rows, "");
        assert_eq!(out.len(), 2);
    }
}
