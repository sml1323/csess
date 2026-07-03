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
use ratatui::text::{Line, Span};
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
/// assistant 헤더 색 = 소스별(Claude 주황 / Codex 그린). [theme]
fn asst_hdr_style(source: Source) -> Style {
    Style::default()
        .fg(crate::theme::source_color_e(source))
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
            Role::Assistant => out.push(Line::styled(asst_label.to_string(), asst_hdr_style(source))),
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

// ---- 검색 매치 앵커 (search-hit-preview) ----

/// `Line` 의 평문(스팬 content 이어붙임). 매치 스캔·앵커 계산의 대상 텍스트.
fn line_plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// term(이미 소문자, `search::terms`) 하나 이상 포함하는 렌더 라인 인덱스(오름차순, 대소문자 무시).
/// 매치를 **렌더된 라인**에서 찾으므로, 검색 코퍼스에만 있고 프리뷰엔 안 보이는 매치는 자연히 제외(top 폴백). [D2]
pub fn match_lines(lines: &[Line<'_>], terms: &[String]) -> Vec<usize> {
    if terms.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let plain = line_plain(l).to_lowercase();
            terms.iter().any(|t| plain.contains(t.as_str())).then_some(i)
        })
        .collect()
}

/// 첫 매치 라인 - `lead_in`(위쪽 맥락 줄 수, saturating). 무매치/빈 terms → 0(top 폴백). [D1/D7]
pub fn anchor_scroll(lines: &[Line<'_>], terms: &[String], lead_in: u16) -> u16 {
    match match_lines(lines, terms).first() {
        Some(&i) => (i.min(u16::MAX as usize) as u16).saturating_sub(lead_in),
        None => 0,
    }
}

// ---- 검색 매치 하이라이트 (search-hit-preview) ----

/// `content` 안에서 terms(소문자) 중 하나라도 대소문자 무시 매치하는 **원본 바이트 구간**들(병합·정렬).
/// `to_lowercase()` 가 바이트 길이를 바꿀 수 있어(예: ß→ss, İ), 소문자 문자열의 각 바이트를
/// 원본 오프셋으로 되매핑해 슬라이스가 항상 원본의 char 경계에 떨어지게 한다(패닉 방지). [D5]
fn match_ranges(content: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let mut low = String::with_capacity(content.len());
    let mut map: Vec<usize> = Vec::with_capacity(content.len() + 1); // low 의 각 바이트 → 원본 바이트 offset
    for (off, ch) in content.char_indices() {
        for lc in ch.to_lowercase() {
            let mut buf = [0u8; 4];
            let enc = lc.encode_utf8(&mut buf);
            for _ in 0..enc.len() {
                map.push(off);
            }
            low.push_str(enc);
        }
    }
    let orig_end = |low_off: usize| if low_off < map.len() { map[low_off] } else { content.len() };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        if term.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = low[from..].find(term.as_str()) {
            let ls = from + rel;
            let le = ls + term.len();
            ranges.push((map[ls], orig_end(le)));
            from = le;
        }
    }
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

/// 한 라인의 각 스팬을 매치 경계로 분할 — 매치 구간은 `base.patch(match_hl)`, 주변은 base 유지.
fn highlight_line(line: &Line<'static>, terms: &[String]) -> Line<'static> {
    let hl = crate::theme::match_hl();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for span in &line.spans {
        let content = span.content.as_ref();
        let ranges = match_ranges(content, terms);
        if ranges.is_empty() {
            spans.push(span.clone());
            continue;
        }
        let base = span.style;
        let mut cur = 0usize;
        for (s, e) in ranges {
            if s > cur {
                spans.push(Span::styled(content[cur..s].to_string(), base));
            }
            spans.push(Span::styled(content[s..e].to_string(), base.patch(hl)));
            cur = e;
        }
        if cur < content.len() {
            spans.push(Span::styled(content[cur..].to_string(), base));
        }
    }
    let mut out = Line::from(spans);
    out.alignment = line.alignment;
    out.style = line.style;
    out
}

/// base 라인들 → 매치 substring 을 하이라이트 스팬으로 분할한 라인들.
/// terms 비면 입력 그대로 clone. 평문(스팬 content 이어붙임)은 항상 보존. [D5]
pub fn highlight_lines(lines: &[Line<'static>], terms: &[String]) -> Vec<Line<'static>> {
    if terms.is_empty() {
        return lines.to_vec();
    }
    lines.iter().map(|l| highlight_line(l, terms)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::match_hl;

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

    #[test]
    fn match_lines_indices_case_insensitive_multiterm() {
        let lines = vec![
            Line::from("첫 줄"),         // 0
            Line::from("JWT 토큰 검증"), // 1
            Line::from("관련 없음"),     // 2
            Line::from("다시 jwt 나옴"), // 3
        ];
        // 단일어, 대소문자 무시
        assert_eq!(match_lines(&lines, &["jwt".to_string()]), vec![1, 3]);
        // 다중어 → 어느 term이든 포함 라인, 오름차순(첫=가장 이른 인덱스)
        assert_eq!(
            match_lines(&lines, &["첫".to_string(), "jwt".to_string()]),
            vec![0, 1, 3]
        );
        // 무매치 / 빈 terms
        assert!(match_lines(&lines, &["없는단어".to_string()]).is_empty());
        assert!(match_lines(&lines, &[]).is_empty());
    }

    #[test]
    fn anchor_scroll_first_match_with_lead_in() {
        let mut lines: Vec<Line> = (0..20).map(|i| Line::from(format!("filler {i}"))).collect();
        lines[7] = Line::from("여기 auth 매치");
        // 첫 매치 7, lead_in 2 → 5
        assert_eq!(anchor_scroll(&lines, &["auth".to_string()], 2), 5);
        // 첫 매치가 0 이면 saturating → 0
        lines[0] = Line::from("auth 맨 위");
        assert_eq!(anchor_scroll(&lines, &["auth".to_string()], 2), 0);
        // 무매치 / 빈 terms → 0
        assert_eq!(anchor_scroll(&lines, &["없음".to_string()], 2), 0);
        assert_eq!(anchor_scroll(&lines, &[], 2), 0);
    }

    fn plain_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn highlight_splits_match_and_preserves_plaintext() {
        let out = highlight_lines(&[Line::from("토큰 JWT 검증")], &["jwt".to_string()]);
        assert_eq!(out.len(), 1);
        let spans = &out[0].spans;
        // "토큰 " / "JWT"(하이라이트) / " 검증"
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[1].content.as_ref(), "JWT"); // 원본 대소문자 보존
        assert_eq!(plain_of(&out[0]), "토큰 JWT 검증"); // 평문 보존
        assert_eq!(spans[1].style.bg, match_hl().bg); // 매치에 하이라이트 배경
    }

    #[test]
    fn highlight_preserves_base_style_multiterm_and_empty() {
        // heading 스타일 라인 → 매치에 하이라이트 오버레이, base(heading) 유지(patch)
        let out = highlight_lines(
            &[Line::from(Span::styled("## auth 설정", heading_style()))],
            &["auth".to_string()],
        );
        assert_eq!(plain_of(&out[0]), "## auth 설정");
        let m = out[0].spans.iter().find(|s| s.content.as_ref() == "auth").unwrap();
        assert_eq!(m.style.bg, match_hl().bg); // 하이라이트 배경
        assert_eq!(m.style.fg, match_hl().fg); // patch 로 fg 도 덮임(가독)
        let base = out[0].spans.iter().find(|s| s.content.as_ref() == "## ").unwrap();
        assert_eq!(base.style.fg, heading_style().fg); // 주변은 heading base 유지

        // 다중어 → 둘 다 강조
        let out2 = highlight_lines(
            &[Line::from("토큰 검증 완료")],
            &["토큰".to_string(), "검증".to_string()],
        );
        let hl_count = out2[0].spans.iter().filter(|s| s.style.bg == match_hl().bg).count();
        assert_eq!(hl_count, 2);

        // 빈 terms → 입력 그대로(분할 없음)
        let out3 = highlight_lines(&[Line::from("변화 없음")], &[]);
        assert_eq!(out3[0].spans.len(), 1);
        assert_eq!(plain_of(&out3[0]), "변화 없음");
    }
}
