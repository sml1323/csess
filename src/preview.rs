//! 인프로세스 프리뷰 렌더 — JSONL → ratatui `Line` (외부 jq+bat 서브프로세스 제거). [D14]
//!
//! Phase A 는 선택마다 jq+bat 서브프로세스를 띄워 스크롤 CPU 스파이크(5%→20%)가 있었다.
//! Phase B 는 인프로세스로 렌더해 세션별로 **1회 렌더 후 캐시**(TUI 가 보관) → 스크롤 idle.
//!
//! 사용자 결정(2026-06-01): user/assistant **텍스트**는 보여주고, **tool_use/thinking 은
//! 접기 요약**(한 줄, dim)으로. tool_result 는 생략. (claude-history 식.)
//! 마크다운은 라인 기반 경량 스타일(heading/코드펜스/불릿). 풀 pulldown-cmark+syntect 는 후속 polish.

use crate::model::Source;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use serde_json::Value;

/// 프리뷰가 파싱할 입력 바이트 상한(거대 세션 글랜스). Phase A `CSESS_PREVIEW_BYTES`(기본 500KB). [D14]
const PREVIEW_INPUT_BYTES: usize = 500_000;
/// 렌더 출력 줄 상한(거대 세션 지연 방지). Phase A `PREVIEW_MAX_LINES`.
const PREVIEW_MAX_LINES: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// 한 턴의 블록: 텍스트(마크다운) 또는 접힌 요약(tool/thinking, 마커 포함).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(String),
    Collapsed(String),
}

// ---- 색 (claude-history 팔레트, Phase A 와 동일) ----
fn user_hdr_style() -> Style {
    Style::default()
        .fg(Color::Rgb(235, 235, 235))
        .add_modifier(Modifier::BOLD)
}
fn asst_hdr_style() -> Style {
    Style::default()
        .fg(Color::Rgb(78, 201, 176))
        .add_modifier(Modifier::BOLD)
}
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
fn code_style() -> Style {
    Style::default().fg(Color::Rgb(206, 145, 120))
}
fn heading_style() -> Style {
    Style::default()
        .fg(Color::Rgb(86, 156, 214))
        .add_modifier(Modifier::BOLD)
}

/// tool_use 블록 한 줄 요약: `name hint` (hint = file_path/command/pattern 등).
fn tool_summary(block: &Value) -> String {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let input = block.get("input");
    let hint = input
        .and_then(|i| {
            i.get("file_path")
                .or_else(|| i.get("path"))
                .or_else(|| i.get("command"))
                .or_else(|| i.get("pattern"))
                .or_else(|| i.get("query"))
                .or_else(|| i.get("description"))
        })
        .and_then(Value::as_str)
        .unwrap_or("");
    let hint = first_line_trunc(hint, 70);
    if hint.is_empty() {
        format!("⚒ {name}")
    } else {
        format!("⚒ {name}: {hint}")
    }
}

fn first_line_trunc(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    let t: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        format!("{t}…")
    } else {
        t
    }
}

/// Claude JSONL → 턴 목록. user 의 슬래시 래퍼 텍스트는 스킵, tool_use/thinking 은 접음.
pub fn extract_claude(bytes: &[u8]) -> Vec<(Role, Vec<Block>)> {
    let mut turns = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let role = match v.get("type").and_then(Value::as_str) {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };
        let content = v.get("message").and_then(|m| m.get("content"));
        let mut blocks = Vec::new();
        match content {
            Some(Value::String(s)) => {
                if !(role == Role::User && crate::parser::claude::is_slash_wrapper(s)) && !s.is_empty() {
                    blocks.push(Block::Text(s.clone()));
                }
            }
            Some(Value::Array(arr)) => {
                for b in arr {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                if !(role == Role::User && crate::parser::claude::is_slash_wrapper(t))
                                    && !t.is_empty()
                                {
                                    blocks.push(Block::Text(t.to_string()));
                                }
                            }
                        }
                        Some("thinking") => {
                            let th = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                            blocks.push(Block::Collapsed(format!(
                                "💭 {}",
                                first_line_trunc(th, 60)
                            )));
                        }
                        Some("tool_use") => blocks.push(Block::Collapsed(tool_summary(b))),
                        // tool_result/image 등은 생략(요약 가치 낮음).
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if !blocks.is_empty() {
            turns.push((role, blocks));
        }
    }
    turns
}

/// Codex JSONL → 턴 목록. event_msg 의 user_message/agent_message 텍스트(클린 transcript).
pub fn extract_codex(bytes: &[u8]) -> Vec<(Role, Vec<Block>)> {
    let mut turns = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let p = v.get("payload");
        let ptype = p
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let msg = p.and_then(|p| p.get("message")).and_then(Value::as_str);
        match ptype {
            "user_message" => {
                if let Some(m) = msg {
                    if !m.is_empty() {
                        turns.push((Role::User, vec![Block::Text(m.to_string())]));
                    }
                }
            }
            "agent_message" => {
                if let Some(m) = msg {
                    if !m.is_empty() {
                        turns.push((Role::Assistant, vec![Block::Text(m.to_string())]));
                    }
                }
            }
            "agent_reasoning" | "reasoning" => {
                let m = msg.unwrap_or("");
                turns.push((Role::Assistant, vec![Block::Collapsed(format!("💭 {}", first_line_trunc(m, 60)))]));
            }
            _ => {}
        }
    }
    turns
}

/// 마크다운 텍스트 → 스타일된 라인(라인 기반 경량: heading/코드펜스/불릿).
fn md_lines(text: &str, out: &mut Vec<Line<'static>>) {
    let mut in_code = false;
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue; // 펜스 마커 줄은 생략
        }
        if in_code {
            out.push(Line::styled(format!("  {raw}"), code_style()));
            continue;
        }
        if trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("#### ")
        {
            out.push(Line::styled(raw.to_string(), heading_style()));
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            out.push(Line::from(format!("  • {rest}")));
        } else {
            out.push(Line::from(raw.to_string()));
        }
    }
}

/// 턴 목록 → ratatui 라인 (역할 헤더 + 텍스트 + 접힌 요약). 출력 줄 상한 적용.
fn render_turns(turns: &[(Role, Vec<Block>)], source: Source) -> Vec<Line<'static>> {
    let asst_label = match source {
        Source::Codex => "▌ Codex",
        Source::Claude => "▌ Claude",
    };
    let mut out: Vec<Line<'static>> = Vec::new();
    for (i, (role, blocks)) in turns.iter().enumerate() {
        if i > 0 {
            out.push(Line::from(""));
        }
        match role {
            Role::User => out.push(Line::styled("▌ 나".to_string(), user_hdr_style())),
            Role::Assistant => out.push(Line::styled(asst_label.to_string(), asst_hdr_style())),
        }
        out.push(Line::from(""));
        for b in blocks {
            match b {
                Block::Text(t) => md_lines(t, &mut out),
                Block::Collapsed(s) => out.push(Line::styled(format!("  {s}"), dim())),
            }
            if out.len() >= PREVIEW_MAX_LINES {
                out.truncate(PREVIEW_MAX_LINES);
                return out;
            }
        }
    }
    out
}

/// 세션 파일 → 프리뷰 라인. 입력 바이트 상한 적용(거대 세션 글랜스). 실패 시 빈 벡터.
pub fn render_session(path: &str, source: Source) -> Vec<Line<'static>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return vec![Line::styled("(파일 읽기 실패)".to_string(), dim())],
    };
    let slice = if bytes.len() > PREVIEW_INPUT_BYTES {
        &bytes[..PREVIEW_INPUT_BYTES]
    } else {
        &bytes[..]
    };
    let turns = match source {
        Source::Claude => extract_claude(slice),
        Source::Codex => extract_codex(slice),
    };
    if turns.is_empty() {
        return vec![Line::styled("(렌더할 텍스트 턴 없음)".to_string(), dim())];
    }
    render_turns(&turns, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_collapses_tool_and_thinking() {
        let jsonl = r#"{"type":"user","message":{"content":"이거 어떻게 고쳐?"}}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"먼저 파일을 읽자\n그다음..."},{"type":"tool_use","name":"Read","input":{"file_path":"/x/main.rs"}},{"type":"text","text":"고치는 방법은 **이거**:\n```rust\nfn main(){}\n```"}]}}
{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}
"#;
        let turns = extract_claude(jsonl.as_bytes());
        // user 질문 1 + assistant 1 (slash-wrapper user 는 스킵)
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].0, Role::User);
        assert_eq!(turns[0].1, vec![Block::Text("이거 어떻게 고쳐?".into())]);
        // assistant: thinking(접힘) + tool_use(접힘) + text
        let a = &turns[1].1;
        assert!(matches!(&a[0], Block::Collapsed(s) if s.starts_with("💭")));
        assert!(matches!(&a[1], Block::Collapsed(s) if s == "⚒ Read: /x/main.rs"));
        assert!(matches!(&a[2], Block::Text(t) if t.contains("**이거**")));

        // 렌더: 역할 헤더 + 접힘 마커 존재, raw tool json 없음
        let lines = render_turns(&turns, Source::Claude);
        let flat: String = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(flat.contains("▌ 나"));
        assert!(flat.contains("▌ Claude"));
        assert!(flat.contains("⚒ Read: /x/main.rs"));
        assert!(flat.contains("💭"));
        assert!(!flat.contains("tool_use")); // raw 노이즈 없음
        assert!(!flat.contains("```")); // 펜스 마커 제거됨
        assert!(flat.contains("fn main(){}")); // 코드 내용은 보존
    }

    #[test]
    fn codex_clean_transcript() {
        let jsonl = r#"{"type":"session_meta","payload":{"id":"x","cwd":"/w"}}
{"type":"event_msg","payload":{"type":"user_message","message":"코덱스 질문"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"코덱스 답변"}}
"#;
        let turns = extract_codex(jsonl.as_bytes());
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].0, Role::User);
        assert_eq!(turns[1].0, Role::Assistant);
        let lines = render_turns(&turns, Source::Codex);
        let flat: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(flat.contains("▌ Codex"));
        assert!(flat.contains("코덱스 답변"));
    }
}
