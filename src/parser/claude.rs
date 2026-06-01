//! Claude 세션 파서 (`~/.claude/projects/<encoded-cwd>/<id>.jsonl`, depth-2). [D2]
//!
//! Phase A `csess_index_file` 의 8필드 추출을 byte-exact 재현 — 골든 패리티 대상.
//!   - id  = 파일 스템(`sessionId` 와 일치, 실측 5/5). 스캔 불필요.
//!   - cwd = **모든 라인 타입** 스캔, 첫 non-null `.cwd`(라인 타입 고정 아님). 불명이면
//!           디렉토리명 디코딩 폴백 + '?'(부정확 표시, resume 거부). [D4]
//!   - title = type==user·비-meta·denylist 통과한 첫 사람질문, 정제 후 80자. [D5/D6]

use crate::model::{proj_label, IndexRow, Source};
use crate::parser::sanitize_title;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

/// 슬래시-커맨드/시스템 래퍼 user 라인 denylist (제목 부적합). [D5]
/// jq(oniguruma)의 `\s`·`\b` 는 **유니코드** 의미다(실측: `\s` 가 U+00A0 매치 →
/// fixture 23 으로 확인). Rust regex 기본도 유니코드라 그대로 두면 일치한다.
fn denylist() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*<(command-name|command-message|command-args|local-command-stdout|local-command-stderr|local-command-caveat|task-notification|bash-input|bash-stdout|bash-stderr|system-reminder)\b",
        )
        .unwrap()
    })
}

/// jq `-r` 로 본 `.cwd` 값. 거의 항상 문자열, 비문자는 compact JSON 텍스트.
fn render_raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// cwd 불명 시 부모 디렉토리명 디코딩 폴백 + '?'. [D4 최후 폴백]
/// Phase A `sed -e 's#^-#/#' -e 's#-#/#g'` 의 net 효과 = 모든 '-' → '/'.
fn decode_dir_fallback(enc: &str) -> String {
    format!("{}?", enc.replace('-', "/"))
}

/// 한 라인에서 첫 질문 후보 $t 추출 + 모든 select 통과 여부. 통과 시 정제 전 raw $t. [D5]
/// jq 파이프라인 그대로:
///   select(type=="user" and ((isMeta // false) | not))
///   | content: string→그대로 / array→첫 {type:text}.text / else null
///   | select(non-null string && != "")
///   | select(denylist | not)
fn claude_user_text(v: &Value) -> Option<String> {
    if v.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    // (.isMeta // false) | not → isMeta 가 없음/null/false 일 때만 통과(0/객체 등 truthy 면 drop)
    let meta_truthy = match v.get("isMeta") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(_) => true,
    };
    if meta_truthy {
        return None;
    }
    let content = v.get("message").and_then(|m| m.get("content"));
    let t = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            // [.content[] | select(.type=="text") | .text] | first
            let first_text = arr
                .iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("text"));
            match first_text.and_then(|b| b.get("text")) {
                Some(Value::String(s)) => s.clone(),
                // 첫 text 블록의 .text 가 문자열 아님/없음 → $t=null → 이 라인 스킵
                _ => return None,
            }
        }
        _ => return None,
    };
    if t.is_empty() || denylist().is_match(&t) {
        return None;
    }
    Some(t)
}

/// 검색 코퍼스용 텍스트(추출, step 3+). user/assistant 의 텍스트만. 파리티 경로 미사용.
fn claude_body_text(v: &Value) -> Option<String> {
    let role = v.get("type").and_then(Value::as_str)?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let content = v.get("message").and_then(|m| m.get("content"))?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 한 Claude JSONL 파일 → `IndexRow`. stat 실패면 None(Phase A `return 0` = 무출력).
/// `want_body` 가 false 면 cwd·title 둘 다 찾은 즉시 조기 종료(거대 라인 파싱 회피).
pub fn parse_claude_file(path_arg: &str, want_body: bool) -> Option<IndexRow> {
    let path = Path::new(path_arg);
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len();
    let id = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.strip_suffix(".jsonl").unwrap_or(n).to_string())
        .unwrap_or_default();

    let bytes = std::fs::read(path).ok()?;
    let mut cwd_raw: Option<String> = None;
    let mut title: Option<String> = None;
    let mut body = String::new();

    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // 라인-tolerant: 깨진 줄/invalid surrogate 는 스킵(절대 abort 안 함). [D3]
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if cwd_raw.is_none() {
            // jq `.cwd // empty`: null/false 는 제외, 그 외(빈 문자열 포함) 첫 값 채택.
            if let Some(cv) = v.get("cwd") {
                if !cv.is_null() && !matches!(cv, Value::Bool(false)) {
                    cwd_raw = Some(render_raw(cv));
                }
            }
        }
        if title.is_none() {
            if let Some(t) = claude_user_text(&v) {
                title = Some(sanitize_title(&t));
            }
        }
        if want_body {
            if let Some(t) = claude_body_text(&v) {
                body.push_str(&t);
                body.push('\n');
            }
        } else if cwd_raw.is_some() && title.is_some() {
            break;
        }
    }

    // cwd: 첫 값이 빈 문자열이면 불명으로 간주(Phase A `[ -z "$cwd" ]` 폴백 트리거).
    let cwd_real = match cwd_raw {
        Some(ref s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    let cwd_field = match &cwd_real {
        Some(s) => s.clone(),
        None => {
            let enc = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            decode_dir_fallback(enc)
        }
    };
    let title = title.unwrap_or_else(|| "(no prompt)".to_string());
    let proj = proj_label(&cwd_field);

    Some(IndexRow {
        source: Source::Claude,
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

    #[test]
    fn decode_fallback_matches_sed() {
        assert_eq!(
            decode_dir_fallback("-Users-me-work-csess"),
            "/Users/me/work/csess?"
        );
        assert_eq!(decode_dir_fallback("claude"), "claude?");
        assert_eq!(decode_dir_fallback("-a"), "/a?");
    }

    #[test]
    fn user_text_filters() {
        let real: Value = serde_json::from_str(r#"{"type":"user","message":{"content":"질문"}}"#).unwrap();
        assert_eq!(claude_user_text(&real).as_deref(), Some("질문"));

        let meta: Value = serde_json::from_str(r#"{"type":"user","isMeta":true,"message":{"content":"메타"}}"#).unwrap();
        assert_eq!(claude_user_text(&meta), None);

        let cmd: Value = serde_json::from_str(r#"{"type":"user","message":{"content":"<command-name>/x</command-name>"}}"#).unwrap();
        assert_eq!(claude_user_text(&cmd), None);

        let toolresult: Value = serde_json::from_str(r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}"#).unwrap();
        assert_eq!(claude_user_text(&toolresult), None);

        let arr_text: Value = serde_json::from_str(r#"{"type":"user","message":{"content":[{"type":"text","text":"배열 질문"}]}}"#).unwrap();
        assert_eq!(claude_user_text(&arr_text).as_deref(), Some("배열 질문"));
    }
}
