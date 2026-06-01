//! 골든 패리티: `csess --index-file`(Rust) == `bin/csess --index-file`(Phase A 셸)이
//! **byte-exact** 임을 동일 코퍼스에서 검증한다. 이것이 데이터 레이어의 1순위 안전망 —
//! 0 diff 게이트를 통과해야 SQLite/Codex/TUI(step 3+)로 진행한다.
//!
//! reltime(필드6)은 시간 의존이라 양쪽에 **고정 CSESS_NOW** 를 주입해 결정성을 만든다.
//! mtime(필드1)은 두 도구가 같은 파일을 stat 하므로 동일.
//!
//!   cargo test --test golden_parity                 # fixture 코퍼스(항상)
//!   CSESS_PARITY_LIVE=1 cargo test --test golden_parity   # 실제 ~/.claude/projects

use std::path::{Path, PathBuf};
use std::process::Command;

/// 고정 now (epoch). 값 자체는 무관 — 두 도구가 같은 값을 쓰면 reltime 이 일치한다.
const NOW: &str = "1780000000";

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn shell_bin() -> PathBuf {
    manifest().join("bin/csess")
}

fn rust_stdout(file: &Path) -> Vec<u8> {
    Command::new(env!("CARGO_BIN_EXE_csess"))
        .arg("--index-file")
        .arg(file)
        .env("CSESS_NOW", NOW)
        .output()
        .expect("run rust csess")
        .stdout
}

fn shell_stdout(file: &Path) -> Vec<u8> {
    Command::new("bash")
        .arg(shell_bin())
        .arg("--index-file")
        .arg(file)
        .env("CSESS_NOW", NOW)
        .output()
        .expect("run shell bin/csess")
        .stdout
}

fn diff(file: &Path) -> Option<String> {
    let rust = rust_stdout(file);
    let shell = shell_stdout(file);
    if rust == shell {
        return None;
    }
    Some(format!(
        "DIFF {}\n  shell: {:?}\n  rust : {:?}",
        file.display(),
        String::from_utf8_lossy(&shell),
        String::from_utf8_lossy(&rust),
    ))
}

#[test]
fn golden_fixtures() {
    let dir = manifest().join("tests/fixtures/claude");
    let mut n = 0usize;
    let mut diffs = Vec::new();
    for e in std::fs::read_dir(&dir).expect("fixtures dir") {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        n += 1;
        if let Some(d) = diff(&p) {
            diffs.push(d);
        }
    }
    assert!(n > 0, "no fixtures in {}", dir.display());
    assert!(
        diffs.is_empty(),
        "{}/{} fixture diffs:\n{}",
        diffs.len(),
        n,
        diffs.join("\n---\n")
    );
    eprintln!("golden_fixtures OK: {} files, 0 diff", n);
}

#[test]
fn golden_live() {
    if std::env::var("CSESS_PARITY_LIVE").is_err() {
        eprintln!("skip golden_live (set CSESS_PARITY_LIVE=1 to run against ~/.claude/projects)");
        return;
    }
    let root = std::env::var("CSESS_CLAUDE_ROOT")
        .unwrap_or_else(|_| format!("{}/.claude/projects", std::env::var("HOME").unwrap()));

    // depth-2 열거 (Phase A find -mindepth 2 -maxdepth 2 와 동일).
    let mut files = Vec::new();
    for d in std::fs::read_dir(&root).expect("claude root").flatten() {
        if !d.path().is_dir() {
            continue;
        }
        if let Ok(fs) = std::fs::read_dir(d.path()) {
            for f in fs.flatten() {
                let fp = f.path();
                if fp.is_file() && fp.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                    files.push(fp);
                }
            }
        }
    }
    let total = files.len();
    assert!(total > 0, "no live sessions under {}", root);

    let mut diffs = Vec::new();
    for f in &files {
        if let Some(d) = diff(f) {
            diffs.push(d);
        }
    }
    assert!(
        diffs.is_empty(),
        "{}/{} live diffs (first 20):\n{}",
        diffs.len(),
        total,
        diffs.iter().take(20).cloned().collect::<Vec<_>>().join("\n---\n")
    );
    eprintln!("golden_live OK: {} files, 0 diff", total);
}
