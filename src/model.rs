//! 핵심 데이터 타입 + 8필드 TSV 방출.
//!
//! `IndexRow::to_tsv` 는 Phase A `csess_index_file`(bin/csess)의 출력과 **byte-exact**
//! 파리티를 목표로 한다 — `tests/golden_parity.rs` 가 동일 코퍼스에서 0 diff 를 검증한다.
//! 필드 순서/정제/상대시각/프로젝트 라벨은 모두 Phase A 결정(D2~D6, D12)을 그대로 옮긴 것.

/// 리스트의 시각/프로젝트 컬럼 de-emphasis (fzf `--ansi`). Phase A `DIM`/`RESET`. [D12]
pub const DIM: &str = "\x1b[2m";
pub const RESET: &str = "\x1b[0m";

/// 제목 자르는 길이(코드포인트). Phase A `TITLE_MAX`. [D6]
pub const TITLE_MAX: usize = 80;

/// 세션 출처. Phase A 는 Claude 전용, Phase B 에서 Codex 합류(step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Codex variant 는 step 4 에서 사용
pub enum Source {
    Claude,
    Codex,
}

#[allow(dead_code)]
impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Claude => "claude",
            Source::Codex => "codex",
        }
    }
}

/// 파싱된 한 세션 행. 파리티 출력(`--index-file`)과 SQLite 저장(step 3) 양쪽에 쓰인다.
/// 일부 필드(cwd_real/size/body/source)는 step 3+ 에서 읽힌다.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct IndexRow {
    pub source: Source,
    pub mtime: i64,
    pub id: String,
    /// 필드3(파리티): 실제 cwd, 또는 cwd 불명 시 'decoded?' 폴백. [D4]
    pub cwd_field: String,
    /// 신뢰 가능한 실제 cwd. 불명/lossy면 None → resume 거부(step 6).
    pub cwd_real: Option<String>,
    /// 필드4: `--index-file` 인자로 받은 경로 verbatim.
    pub path: String,
    /// 필드5: 첫 사람질문(정제, ≤80 codepoint). 없으면 "(no prompt)". [D5/D6]
    pub title: String,
    /// 마지막 2 path 컴포넌트. 필드7/8 + 스코프/트리 키. [D12/D14]
    pub proj: String,
    pub size: u64,
    /// 추출 대화 텍스트(검색 코퍼스, step 3+). 파리티 경로에선 비어 있음.
    pub body: String,
}

impl IndexRow {
    /// Phase A `csess_index_file` 의 8필드 TSV 한 줄(개행 포함)을 재현.
    ///
    /// `mtime \t id \t cwd \t path \t title \t DIM reltime RESET \t DIM proj RESET \t proj`
    /// 숨김 필드(mtime/id/cwd/path)를 free-text title **앞**에 둔다(주입 문자가 id/cwd 를
    /// 밀어낼 수 없게). 파생 3필드(reltime/proj)는 title **뒤** — 이미 정제된 값이라 불변식 유지. [D6]
    pub fn to_tsv(&self, now: i64) -> String {
        let rt = reltime(self.mtime, now);
        format!(
            "{mtime}\t{id}\t{cwd}\t{path}\t{title}\t{dim}{rt:>4}{reset}\t{dim}{proj}{reset}\t{proj}\n",
            mtime = self.mtime,
            id = self.id,
            cwd = self.cwd_field,
            path = self.path,
            title = self.title,
            dim = DIM,
            rt = rt,
            reset = RESET,
            proj = self.proj,
        )
    }
}

/// 상대시각 버킷 (Phase A `csess_reltime`). 우정렬 패딩(`{:>4}`)은 호출부(to_tsv)에서. [D12]
/// `now` 는 호출자가 주입(CSESS_NOW 또는 현재 시각) → 파리티 결정성 확보.
pub fn reltime(then: i64, now: i64) -> String {
    let diff = (now - then).max(0);
    if diff < 60 {
        "now".to_string()
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else if diff < 2_592_000 {
        format!("{}d", diff / 86400)
    } else {
        format!("{}mo", diff / 2_592_000)
    }
}

/// 프로젝트 라벨 = cwd 의 마지막 2 path 컴포넌트 (Phase A `csess_proj`). [D12]
/// `parent/base`, 단 parent 가 없거나 base 와 같으면 `base` 만.
pub fn proj_label(cwd: &str) -> String {
    let cwd = cwd.strip_suffix('/').unwrap_or(cwd); // ${cwd%/}
    let base = cwd.rsplit_once('/').map(|(_, b)| b).unwrap_or(cwd); // ${cwd##*/}
    let parent_full = cwd.rsplit_once('/').map(|(p, _)| p).unwrap_or(cwd); // ${cwd%/*}
    let parent = parent_full
        .rsplit_once('/')
        .map(|(_, b)| b)
        .unwrap_or(parent_full); // ${parent##*/}
    if !parent.is_empty() && parent != base {
        format!("{}/{}", parent, base)
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proj_matches_phase_a() {
        assert_eq!(proj_label("/Users/me/work/csess"), "work/csess");
        assert_eq!(proj_label("/Users/me/회사/test_erp"), "회사/test_erp");
        assert_eq!(proj_label("/foo"), "foo");
        assert_eq!(proj_label("foo"), "foo");
        assert_eq!(proj_label("/Users/me/proj/"), "me/proj"); // 끝 슬래시 제거
        assert_eq!(proj_label("/solo"), "solo");
        assert_eq!(proj_label("/a/b/c?"), "b/c?"); // lossy 폴백도 proj 계산
    }

    #[test]
    fn reltime_bucket_boundaries() {
        assert_eq!(reltime(100, 100), "now");
        assert_eq!(reltime(0, 59), "now");
        assert_eq!(reltime(0, 60), "1m");
        assert_eq!(reltime(0, 3599), "59m");
        assert_eq!(reltime(0, 3600), "1h");
        assert_eq!(reltime(0, 86399), "23h");
        assert_eq!(reltime(0, 86400), "1d");
        assert_eq!(reltime(0, 2_591_999), "29d");
        assert_eq!(reltime(0, 2_592_000), "1mo");
        assert_eq!(reltime(200, 100), "now"); // diff<0 → 0
    }

    #[test]
    fn tsv_layout_and_ansi() {
        let row = IndexRow {
            source: Source::Claude,
            mtime: 1000,
            id: "abc".into(),
            cwd_field: "/Users/me/work/csess".into(),
            cwd_real: Some("/Users/me/work/csess".into()),
            path: "/x/abc.jsonl".into(),
            title: "안녕".into(),
            proj: "work/csess".into(),
            size: 0,
            body: String::new(),
        };
        // now = 1000 → diff 0 → "now" → "%4s" = " now"
        let line = row.to_tsv(1000);
        assert_eq!(
            line,
            "1000\tabc\t/Users/me/work/csess\t/x/abc.jsonl\t안녕\t\x1b[2m now\x1b[0m\t\x1b[2mwork/csess\x1b[0m\twork/csess\n"
        );
    }
}
