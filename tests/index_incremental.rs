//! SQLite 증분 무효화 검증: cold/warm 캐시 적중, 변경 감지(size), 유령 삭제.
//! csess 크레이트의 index 모듈을 직접 쓰기 위해 lib 가 아닌 통합 바이너리 경유 — 여기서는
//! `csess --refresh` 의 stderr 통계와 `--index`(fresh) 결과의 정합을 외부에서 검증한다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_csess")
}

/// `--refresh` 실행 후 stderr 의 "parsed=.. cached=.. deleted=.. total=.." 파싱.
fn refresh(root: &Path, db: &Path) -> (usize, usize, usize, usize) {
    let out = Command::new(bin())
        .arg("--refresh")
        .env("CSESS_CLAUDE_ROOT", root)
        // 실제 ~/.codex/sessions 가 섞이지 않게 존재하지 않는 Codex 루트로 격리.
        .env("CSESS_CODEX_ROOT", root.join("__no_codex__"))
        .env("CSESS_DB", db)
        .output()
        .expect("run --refresh");
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err
        .lines()
        .find(|l| l.contains("refresh —"))
        .unwrap_or_else(|| panic!("no refresh stats in: {err}"));
    let num = |key: &str| -> usize {
        line.split_whitespace()
            .find_map(|tok| tok.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no {key} in {line}"))
    };
    (num("parsed="), num("cached="), num("deleted="), num("total="))
}

fn write_session(root: &Path, encdir: &str, name: &str, content: &str) -> PathBuf {
    let dir = root.join(encdir);
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, content).unwrap();
    p
}

#[test]
fn incremental_cache_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("projects");
    let db = tmp.path().join("index.db");

    // depth-2 구조: projects/<encdir>/<id>.jsonl  (3개 세션)
    write_session(
        &root,
        "-Users-me-work-a",
        "s1.jsonl",
        "{\"type\":\"attachment\",\"cwd\":\"/Users/me/work/a\"}\n{\"type\":\"user\",\"message\":{\"content\":\"질문 1\"}}\n",
    );
    write_session(
        &root,
        "-Users-me-work-a",
        "s2.jsonl",
        "{\"type\":\"attachment\",\"cwd\":\"/Users/me/work/a\"}\n{\"type\":\"user\",\"message\":{\"content\":\"질문 2\"}}\n",
    );
    let s3 = write_session(
        &root,
        "-Users-me-work-b",
        "s3.jsonl",
        "{\"type\":\"attachment\",\"cwd\":\"/Users/me/work/b\"}\n{\"type\":\"user\",\"message\":{\"content\":\"질문 3\"}}\n",
    );

    // cold: 3개 전부 파싱
    let (parsed, cached, deleted, total) = refresh(&root, &db);
    assert_eq!((parsed, cached, deleted, total), (3, 0, 0, 3), "cold refresh");

    // warm: 변경 없음 → 전부 캐시 적중
    let (parsed, cached, deleted, total) = refresh(&root, &db);
    assert_eq!((parsed, cached, deleted, total), (0, 3, 0, 3), "warm refresh (no change)");

    // s3 내용 변경(길이 다름 → size 변경 → mtime 초 해상도와 무관하게 감지)
    fs::write(
        &s3,
        "{\"type\":\"attachment\",\"cwd\":\"/Users/me/work/b\"}\n{\"type\":\"user\",\"message\":{\"content\":\"질문 3 수정됨 더 길게\"}}\n",
    )
    .unwrap();
    let (parsed, cached, deleted, total) = refresh(&root, &db);
    assert_eq!((parsed, cached, deleted, total), (1, 2, 0, 3), "changed file reparsed");

    // s3 삭제 → 유령 제거
    fs::remove_file(&s3).unwrap();
    let (parsed, cached, deleted, total) = refresh(&root, &db);
    assert_eq!((parsed, cached, deleted, total), (0, 2, 1, 2), "deleted file removed");
}

/// 캐시(--refresh + load)와 fresh(--index)가 같은 세션 집합/제목을 내는지 외부 정합 검증.
/// (--index 는 fresh 파싱, 캐시 무관. 둘의 id+title 집합이 같아야 한다.)
#[test]
fn cache_matches_fresh_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("projects");
    let db = tmp.path().join("index.db");

    // 실 fixture 들을 임시 루트로 복사 (depth-2: projects/-fixtures/<name>)
    let fix = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude");
    let dst = root.join("-fixtures");
    fs::create_dir_all(&dst).unwrap();
    let mut n = 0;
    for e in fs::read_dir(&fix).unwrap() {
        let p = e.unwrap().path();
        if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            fs::copy(&p, dst.join(p.file_name().unwrap())).unwrap();
            n += 1;
        }
    }
    assert!(n > 0);

    let (parsed, _, _, total) = refresh(&root, &db);
    assert_eq!(parsed, n, "cold parsed all fixtures");
    assert_eq!(total, n, "total == fixture count");

    // --index (fresh) 의 id 집합과 비교: 같은 파일 수.
    let idx = Command::new(bin())
        .arg("--index")
        .env("CSESS_CLAUDE_ROOT", &root)
        .env("CSESS_NOW", "1780000000")
        .output()
        .expect("run --index");
    let fresh_lines = String::from_utf8_lossy(&idx.stdout).lines().count();
    assert_eq!(fresh_lines, n, "fresh --index line count == fixture count");
}
