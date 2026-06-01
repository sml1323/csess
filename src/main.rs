//! csess (Phase B) — Claude/Codex 세션 본문 검색 + resume 런처.
//!
//! 현재 구현: 데이터 레이어(step 1–2). Phase A `bin/csess` 와의 골든 패리티 시임:
//!   csess --index-file F   파일 F 의 8필드 TSV 한 줄(순수 파서, fzf/exec 없음)
//!   csess --index          Claude depth-2 세션 전체를 mtime 역순 TSV 로
//! TUI/검색/트리/resume(step 3–6)은 후속.

mod index;
mod model;
mod parser;
mod preview;
mod resume;
mod search;
mod theme;
mod tree;
mod tui;

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// 상대시각 기준 now. `CSESS_NOW`(테스트/파리티 시임) 우선, 없으면 현재 시각. [D10/D12]
fn now_epoch() -> i64 {
    if let Ok(s) = std::env::var("CSESS_NOW") {
        if let Ok(n) = s.trim().parse::<i64>() {
            return n;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Claude projects 루트. `CSESS_CLAUDE_ROOT` 오버라이드 우선. [D10]
fn claude_root() -> String {
    std::env::var("CSESS_CLAUDE_ROOT").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.claude/projects", home)
    })
}

/// Codex sessions 루트. `CSESS_CODEX_ROOT` 오버라이드 우선.
fn codex_root() -> String {
    std::env::var("CSESS_CODEX_ROOT").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.codex/sessions", home)
    })
}

/// 경로가 Codex rollout 인지(파일명 `rollout-*.jsonl`).
fn is_codex_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        .unwrap_or(false)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--index-file") => {
            let path = match args.get(2) {
                Some(p) => p,
                None => {
                    eprintln!("csess: --index-file 는 경로 인자가 필요합니다");
                    std::process::exit(2);
                }
            };
            // 소스 자동 판별: rollout-*.jsonl 이면 Codex, 아니면 Claude.
            // (골든 패리티는 ~/.claude/projects 만 대상이라 영향 없음.)
            let row = if is_codex_path(path) {
                parser::codex::parse_codex_file(path, false)
            } else {
                parser::claude::parse_claude_file(path, false)
            };
            // 무출력 = stat 실패/무내용 (Phase A `return 0`).
            if let Some(row) = row {
                let _ = std::io::stdout().write_all(row.to_tsv(now_epoch()).as_bytes());
            }
        }
        Some("--index") => cmd_index(),
        Some("--refresh") => cmd_refresh(),
        Some("-h") | Some("--help") => print_help(),
        None => cmd_tui(),
        Some(other) => {
            eprintln!("csess: 알 수 없는 인자 '{other}'. `-h` 로 도움말.");
            std::process::exit(2);
        }
    }
}

/// 기본 동작: 캐시 갱신 → 로드 → TUI → (복원 후) resume/copy. [step 5/6]
fn cmd_tui() {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        eprintln!("csess: TUI 는 터미널에서 실행하세요 (비-tty). 데이터 확인은 `csess --index` / `--refresh`.");
        std::process::exit(1);
    }
    let mut store = match index::IndexStore::open(&db_path()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("csess: 인덱스 DB 열기 실패: {e}");
            std::process::exit(1);
        }
    };
    let mut cands = index::enumerate_claude(&claude_root());
    cands.extend(index::enumerate_codex(&codex_root()));
    if let Err(e) = store.refresh(&cands) {
        eprintln!("csess: 인덱스 갱신 실패: {e}");
        std::process::exit(1);
    }
    let rows = match store.load_rows() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("csess: 인덱스 로드 실패: {e}");
            std::process::exit(1);
        }
    };
    if rows.is_empty() {
        eprintln!("csess: 세션이 없습니다 (Claude: {} / Codex: {})", claude_root(), codex_root());
        std::process::exit(1);
    }

    let home = std::env::var("HOME").unwrap_or_default();
    match tui::run(rows, now_epoch(), home) {
        tui::Outcome::Quit => {}
        tui::Outcome::Resume(row) => report_resume(resume::resume(&row, false)),
        tui::Outcome::Copy(row) => report_resume(resume::resume(&row, true)),
    }
}

/// resume 가드 실패를 사용자에게 알린다. (성공 exec 는 반환하지 않음)
fn report_resume(res: Result<(), resume::ResumeError>) {
    if let Err(e) = res {
        match e {
            resume::ResumeError::NoId => eprintln!("csess: 세션 id 가 없어 resume 거부"),
            resume::ResumeError::BadCwd(c) => eprintln!(
                "csess: cwd 불명/비유효라 resume 거부: {}",
                if c.is_empty() { "<empty>" } else { &c }
            ),
        }
        std::process::exit(1);
    }
}

/// Claude 루트의 depth-2 JSONL 전체를 파싱해 mtime 역순 TSV 로 방출. [D1/D2]
/// `read_dir(root)`(depth-1 dirs) → `read_dir(dir)`(depth-2 files) = `-mindepth 2 -maxdepth 2`,
/// depth-3 `*/subagents/*` 서브에이전트 저널을 자연히 배제.
fn cmd_index() {
    let root = claude_root();
    let mut rows: Vec<model::IndexRow> = Vec::new();
    if let Ok(dirs) = std::fs::read_dir(&root) {
        for d in dirs.flatten() {
            let dp = d.path();
            if !dp.is_dir() {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(&dp) {
                for f in files.flatten() {
                    let fp = f.path();
                    if fp.is_file() && fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        if let Some(s) = fp.to_str() {
                            if let Some(r) = parser::claude::parse_claude_file(s, false) {
                                rows.push(r);
                            }
                        }
                    }
                }
            }
        }
    }
    rows.sort_by(|a, b| b.mtime.cmp(&a.mtime)); // mtime desc
    let now = now_epoch();
    let mut out = std::io::stdout().lock();
    for r in &rows {
        let _ = out.write_all(r.to_tsv(now).as_bytes());
    }
}

/// SQLite 인덱스 DB 경로. `CSESS_DB` 오버라이드(테스트) 우선.
fn db_path() -> String {
    std::env::var("CSESS_DB").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}/.cache/csess/index.db", home)
    })
}

/// SQLite 캐시를 증분 갱신하고 통계를 stderr 로. [step 3]
fn cmd_refresh() {
    let mut store = match index::IndexStore::open(&db_path()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("csess: 인덱스 DB 열기 실패: {e}");
            std::process::exit(1);
        }
    };
    let mut candidates = index::enumerate_claude(&claude_root());
    candidates.extend(index::enumerate_codex(&codex_root()));
    match store.refresh(&candidates) {
        Ok(s) => eprintln!(
            "csess: refresh — parsed={} cached={} deleted={} total={}",
            s.parsed, s.cached, s.deleted, s.total
        ),
        Err(e) => {
            eprintln!("csess: refresh 실패: {e}");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!(
        "csess (Phase B) — Claude/Codex 세션 본문 검색 + resume 런처\n\n\
         csess                  TUI 실행: 본문검색 → enter 로 resume (기본)\n  \
         csess --index-file F   파일 F 의 8필드 TSV 한 줄 (Phase A 파리티 시임)\n  \
         csess --index          Claude depth-2 세션 전체를 mtime 역순 TSV 로 (fresh)\n  \
         csess --refresh        SQLite 캐시 증분 갱신 (Claude+Codex, parsed/cached/deleted)\n\n\
         TUI 키: 타이핑=검색 · ↑↓/ctrl-p,n=이동 · enter=resume · ctrl-y=복사 · ctrl-d,u=프리뷰 스크롤 · esc=종료\n\n\
         env:\n  \
         CSESS_CLAUDE_ROOT   Claude projects 루트 (기본 ~/.claude/projects)\n  \
         CSESS_CODEX_ROOT    Codex sessions 루트 (기본 ~/.codex/sessions)\n  \
         CSESS_DB            인덱스 DB 경로 (기본 ~/.cache/csess/index.db)\n  \
         CSESS_NOW           상대시각 now 고정 (테스트/파리티 시임)"
    );
}
