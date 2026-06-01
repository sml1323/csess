//! Codex 세션 파서 (`~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`).
//!
//! Phase A 베이스라인 없음 — 실파일 프로빙(2026-06-01)으로 확정한 스펙. 골든 패리티 대신
//! 단위 테스트(fixtures)로 검증한다.
//!   - id  = session_meta `.payload.id` (== 파일명 uuid). cwd = `.payload.cwd` (lossless 절대경로,
//!           Claude 와 달리 디코딩 불필요).
//!   - title PRIMARY = 첫 `event_msg`/`user_message` 의 `.payload.message` (genuine 입력에만 방출,
//!           래퍼 0 → denylist 불필요). FALLBACK = 첫 `response_item`/message/role==user 의
//!           `content[].text` 중 래퍼(`# AGENTS.md`/`<environment_context>` 등) 아님. 둘 다 없으면 `(no prompt)`.
//!   - body = `user_message` + `agent_message` 의 `.payload.message` (검색 코퍼스).
//! resume = `codex resume <uuid>` (cd 가드는 Claude 와 동일, lossless cwd 사용).

use crate::model::{proj_label, IndexRow, Source};
use crate::parser::sanitize_title;
use serde_json::Value;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// FALLBACK(response_item user) 경로에서 스킵할 래퍼 prefix. PRIMARY(user_message)엔 불필요.
fn is_codex_wrapper(text: &str) -> bool {
    let t = text.trim_start();
    const MARKERS: &[&str] = &[
        "# AGENTS.md",
        "<environment_context",
        "<turn_aborted",
        "<skill",
        "<user_shell_command",
        "<permissions",
    ];
    MARKERS.iter().any(|m| t.starts_with(m))
}

/// session_meta 에 id 가 없을 때 파일명에서 UUID(끝 36자) 폴백.
fn uuid_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    if stem.len() >= 36 {
        let tail = &stem[stem.len() - 36..];
        if tail.as_bytes().get(8) == Some(&b'-') {
            return Some(tail.to_string());
        }
    }
    None
}

/// response_item/message 의 content 배열에서 첫 문자열 `.text`.
fn first_content_text(payload: &Value) -> Option<&str> {
    payload
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
}

/// 한 Codex rollout → IndexRow. stat 실패면 None.
pub fn parse_codex_file(path_arg: &str, want_body: bool) -> Option<IndexRow> {
    let path = Path::new(path_arg);
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len();

    let bytes = std::fs::read(path).ok()?;
    let mut id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut primary: Option<String> = None; // 첫 user_message
    let mut fallback: Option<String> = None; // 첫 비-래퍼 response_item user
    let mut body = String::new();

    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => continue, // 라인-tolerant (Claude 와 동일 규칙)
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_meta" => {
                if id.is_none() {
                    if let Some(p) = v.get("payload") {
                        id = p.get("id").and_then(Value::as_str).map(String::from);
                        cwd = p.get("cwd").and_then(Value::as_str).map(String::from);
                    }
                }
            }
            "event_msg" => {
                let p = v.get("payload");
                let ptype = p
                    .and_then(|p| p.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match ptype {
                    "user_message" => {
                        if let Some(m) = p.and_then(|p| p.get("message")).and_then(Value::as_str) {
                            if !m.is_empty() {
                                if primary.is_none() {
                                    primary = Some(m.to_string());
                                }
                                if want_body {
                                    body.push_str(m);
                                    body.push('\n');
                                }
                            }
                        }
                    }
                    "agent_message" if want_body => {
                        if let Some(m) = p.and_then(|p| p.get("message")).and_then(Value::as_str) {
                            if !m.is_empty() {
                                body.push_str(m);
                                body.push('\n');
                            }
                        }
                    }
                    _ => {}
                }
            }
            "response_item" if fallback.is_none() => {
                if let Some(p) = v.get("payload") {
                    let is_msg = p.get("type").and_then(Value::as_str) == Some("message");
                    let role = p.get("role").and_then(Value::as_str).unwrap_or("");
                    if is_msg && role == "user" {
                        if let Some(text) = first_content_text(p) {
                            if !text.is_empty() && !is_codex_wrapper(text) {
                                fallback = Some(text.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        // 파리티 경로(want_body=false): id+cwd+primary 확보 시 조기 종료.
        if !want_body && id.is_some() && cwd.is_some() && primary.is_some() {
            break;
        }
    }

    let id = id.or_else(|| uuid_from_filename(path)).unwrap_or_default();
    let title = primary
        .or(fallback)
        .map(|t| sanitize_title(&t))
        .unwrap_or_else(|| "(no prompt)".to_string());
    let cwd_real = cwd.filter(|s| !s.is_empty());
    let cwd_field = cwd_real.clone().unwrap_or_default();
    let proj = proj_label(&cwd_field);

    Some(IndexRow {
        source: Source::Codex,
        mtime,
        id,
        cwd_field,
        cwd_real,
        path: path_arg.to_string(),
        title,
        proj,
        size,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        format!(
            "{}/tests/fixtures/codex/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        )
    }

    #[test]
    fn primary_user_message_wins() {
        let r = parse_codex_file(&fixture("c01-user-message.jsonl"), true).unwrap();
        assert_eq!(r.title, "실제 코덱스 질문입니다");
        assert_eq!(r.id, "019c0000-0000-7000-8000-000000000001");
        assert_eq!(r.cwd_field, "/Users/me/work/codeproj");
        assert_eq!(r.proj, "work/codeproj");
        assert_eq!(r.cwd_real.as_deref(), Some("/Users/me/work/codeproj"));
        // body 에 user_message + agent_message 둘 다.
        assert!(r.body.contains("실제 코덱스 질문입니다"));
        assert!(r.body.contains("네 도와드릴게요"));
    }

    #[test]
    fn fallback_when_no_user_message() {
        let r = parse_codex_file(&fixture("c02-fallback-no-usermsg.jsonl"), false).unwrap();
        assert_eq!(r.title, "폴백 경로 실제 질문"); // 래퍼 2개 스킵 후
        assert_eq!(r.proj, "work/fb");
    }

    #[test]
    fn no_prompt_when_only_wrappers() {
        let r = parse_codex_file(&fixture("c03-no-prompt.jsonl"), false).unwrap();
        assert_eq!(r.title, "(no prompt)");
    }

    #[test]
    fn primary_sanitized() {
        let r = parse_codex_file(&fixture("c04-image-multiline.jsonl"), false).unwrap();
        assert_eq!(r.title, "[Image #1] 이미지 보고 질문"); // \n\n, \t → 공백
    }

    #[test]
    fn wrapper_detection() {
        assert!(is_codex_wrapper("# AGENTS.md instructions for /x"));
        assert!(is_codex_wrapper("<environment_context> ..."));
        assert!(is_codex_wrapper("  <permissions instructions>"));
        assert!(!is_codex_wrapper("실제 질문"));
        assert!(!is_codex_wrapper("<div> 이건 진짜 질문 아닌가"));
    }

    #[test]
    fn uuid_filename_fallback() {
        let p = Path::new("/x/2026/03/17/rollout-2026-03-17T16-54-02-019cfac9-9c15-7bb3-9b7a-b3c3d31f5627.jsonl");
        assert_eq!(
            uuid_from_filename(p).as_deref(),
            Some("019cfac9-9c15-7bb3-9b7a-b3c3d31f5627")
        );
    }
}
