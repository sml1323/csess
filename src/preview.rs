//! 인프로세스 프리뷰 렌더 — JSONL → ratatui `Line` (외부 jq+bat 서브프로세스 제거). [D14]
//!
//! Phase A 는 선택마다 jq+bat 서브프로세스를 띄워 스크롤 CPU 스파이크(5%→20%)가 있었다.
//! Phase B 는 인프로세스로 렌더해 세션별로 **1회 렌더 후 캐시**(TUI 가 보관) → 스크롤 idle.
//!
//! 사용자 결정(2026-06-01): user/assistant **텍스트**는 보여주고, **tool_use/thinking 은
//! 접기 요약**(한 줄, dim)으로. tool_result 는 생략. (claude-history 식.)
//! 마크다운은 라인 기반(heading/펜스/불릿/번호/인용) + **인라인 스팬**(`**bold**`/`` `code` ``/
//! `*italic*`) — 1 논리라인 = 1 `Line` 불변식 유지(앵커가 렌더 Line 인덱스 기준이라 필수).
//! 긴 줄은 [`wrap_lines`] 가 표시폭 기준 word-wrap — 랩 후 라인들이 앵커/하이라이트의 좌표계.

use crate::model::Source;
use crate::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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

// ---- 스타일 (팔레트는 theme.rs 집결) ----
fn dim() -> Style {
    theme::dim()
}
/// 코드펜스 내부 라인 — 스틸블루 fg + 어두운 bg 틴트(코드/프로즈 경계).
fn code_block_style() -> Style {
    Style::default().fg(theme::CODE).bg(theme::CODE_BG)
}
/// 인라인 `code` 스팬.
fn inline_code_style() -> Style {
    Style::default().fg(theme::CODE).bg(theme::CODE_BG)
}
/// 턴 헤더 칩 — 검정 글씨 + 역할색 배경(user=밝음, assistant=소스색).
fn chip_style(bg: Color) -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(bg)
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

/// thinking/reasoning 접기 요약 길이 — 60→120: 키워드 밀집 구간이라 검색 매치 확률도 올라간다.
const THINK_TRUNC: usize = 120;

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
                                first_line_trunc(th, THINK_TRUNC)
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
                turns.push((
                    Role::Assistant,
                    vec![Block::Collapsed(format!("💭 {}", first_line_trunc(m, THINK_TRUNC)))],
                ));
            }
            _ => {}
        }
    }
    turns
}

// ---- 마크다운 렌더 ----

/// 인라인 마크다운 → 스팬: `` `code` `` / `**bold**` / `*italic*`.
/// `_italic_` 은 미지원(snake_case 식별자 오탐이 잦음). 짝 없는 마커는 원문 그대로 폴백 —
/// 절대 텍스트를 잃지 않는다. 1 라인 안에서만 동작(멀티라인 마커 미지원).
fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(pos) = rest.find(['`', '*']) else {
            plain.push_str(rest);
            break;
        };
        plain.push_str(&rest[..pos]);
        rest = &rest[pos..];
        // 마커 시도 — 성공 시 (스팬, 소비 바이트), 실패 시 마커 첫 char 를 평문으로.
        let taken: Option<(Span<'static>, usize)> = if let Some(r) = rest.strip_prefix('`') {
            r.find('`')
                .filter(|&e| e > 0)
                .map(|e| (Span::styled(r[..e].to_string(), inline_code_style()), e + 2))
        } else if let Some(r) = rest.strip_prefix("**") {
            r.find("**").filter(|&e| e > 0).map(|e| {
                (
                    Span::styled(r[..e].to_string(), base.add_modifier(Modifier::BOLD)),
                    e + 4,
                )
            })
        } else if let Some(r) = rest.strip_prefix('*') {
            // *italic*: 내용이 공백으로 시작/끝나면 불성립(곱셈·글롭 오탐 방지).
            match r.find('*') {
                Some(e) if e > 0 && !r[..e].starts_with(' ') && !r[..e].ends_with(' ') => Some((
                    Span::styled(r[..e].to_string(), base.add_modifier(Modifier::ITALIC)),
                    e + 2,
                )),
                _ => None,
            }
        } else {
            None
        };
        match taken {
            Some((span, len)) => {
                if !plain.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut plain), base));
                }
                spans.push(span);
                rest = &rest[len..];
            }
            None => {
                let n = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                plain.push_str(&rest[..n]);
                rest = &rest[n..];
            }
        }
    }
    if !plain.is_empty() {
        spans.push(Span::styled(plain, base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

/// `1. ` / `12) ` 번호 리스트 마커 분리 → (마커, 본문). 아니면 None.
fn split_ordered(s: &str) -> Option<(&str, &str)> {
    let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = &s[digits..];
    let body = rest
        .strip_prefix('.')
        .or_else(|| rest.strip_prefix(')'))?
        .strip_prefix(' ')?;
    Some((&s[..digits + 1], body))
}

/// 마크다운 텍스트 → 스타일된 라인. heading 은 소스 accent(듀얼톤 안으로 — 제3 강조색 제거),
/// 불릿은 들여쓰기 보존, 번호/인용 지원, 인라인 마커는 스팬으로.
fn md_lines(text: &str, accent: Color, out: &mut Vec<Line<'static>>) {
    let mut in_code = false;
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if let Some(fence) = trimmed.strip_prefix("```") {
            in_code = !in_code;
            if in_code {
                // 여는 펜스 → 언어 라벨 1:1 치환. 닫는 펜스는 생략(bg 틴트가 경계 담당).
                out.push(Line::styled(format!("  ⌜{}", fence.trim()), dim()));
            }
            continue;
        }
        if in_code {
            out.push(Line::styled(format!("  {raw}"), code_block_style()));
            continue;
        }
        let indent = raw.len() - trimmed.len();
        if trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || trimmed.starts_with("#### ")
        {
            out.push(Line::styled(
                raw.to_string(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ));
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let mut spans = vec![Span::raw(format!("{:w$}• ", "", w = indent + 2))];
            spans.extend(inline_spans(rest, Style::default()));
            out.push(Line::from(spans));
        } else if let Some((num, rest)) = split_ordered(trimmed) {
            let mut spans = vec![Span::styled(format!("{:w$}{num} ", "", w = indent + 2), dim())];
            spans.extend(inline_spans(rest, Style::default()));
            out.push(Line::from(spans));
        } else if let Some(rest) = trimmed
            .strip_prefix("> ")
            .or_else(|| (trimmed == ">").then_some(""))
        {
            let mut spans = vec![Span::styled(
                format!("{:w$}▏ ", "", w = indent),
                Style::default().fg(theme::SEPARATOR),
            )];
            spans.extend(inline_spans(rest, dim()));
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(inline_spans(raw, Style::default())));
        }
    }
}

/// 프리뷰 상한 도달 시 끝에 붙는 마커 라인.
fn trunc_marker() -> Line<'static> {
    Line::styled(
        "── 이후 생략 (거대 세션 — 500KB/4000줄 프리뷰 상한) ──".to_string(),
        dim(),
    )
}

/// 턴 목록 → ratatui 라인 (역할 칩 + 텍스트 + 접힌 요약). 출력 줄 상한 적용.
/// Collapsed↔Text 전환 시 빈 줄 1개 — 툴 체인 요약 밑에 본문이 붙는 답답함 제거
/// (연속 Collapsed 끼리는 붙임: 툴 체인은 묶음이 맞음).
fn render_turns(turns: &[(Role, Vec<Block>)], source: Source) -> Vec<Line<'static>> {
    let accent = theme::source_color_e(source);
    let asst_label = match source {
        Source::Codex => " Codex ",
        Source::Claude => " Claude ",
    };
    let mut out: Vec<Line<'static>> = Vec::new();
    for (i, (role, blocks)) in turns.iter().enumerate() {
        if i > 0 {
            out.push(Line::from(""));
        }
        let chip = match role {
            Role::User => Span::styled(" 나 ".to_string(), chip_style(theme::TEXT_BRIGHT)),
            Role::Assistant => Span::styled(asst_label.to_string(), chip_style(accent)),
        };
        out.push(Line::from(chip));
        out.push(Line::from(""));
        let mut prev_collapsed: Option<bool> = None;
        for b in blocks {
            let collapsed = matches!(b, Block::Collapsed(_));
            if prev_collapsed.is_some_and(|p| p != collapsed) {
                out.push(Line::from(""));
            }
            prev_collapsed = Some(collapsed);
            match b {
                Block::Text(t) => md_lines(t, accent, &mut out),
                Block::Collapsed(s) => out.push(Line::styled(format!("  {s}"), dim())),
            }
            if out.len() >= PREVIEW_MAX_LINES {
                out.truncate(PREVIEW_MAX_LINES);
                out.push(trunc_marker());
                return out;
            }
        }
    }
    out
}

/// 세션 파일 → 프리뷰 라인. 입력 바이트 상한 적용(거대 세션 글랜스).
/// 실패/빈 세션은 원인이 보이는 메시지로.
pub fn render_session(path: &str, source: Source) -> Vec<Line<'static>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return vec![
                Line::styled(
                    format!("⚠ 파일 읽기 실패: {e}"),
                    Style::default().fg(theme::WARN),
                ),
                Line::styled(format!("  {path}"), dim()),
                Line::styled("  (ctrl-y 로 resume 명령 복사는 가능)".to_string(), dim()),
            ]
        }
    };
    if bytes.is_empty() {
        return vec![Line::styled("(빈 세션 파일)".to_string(), dim())];
    }
    let capped = bytes.len() > PREVIEW_INPUT_BYTES;
    let slice = if capped {
        &bytes[..PREVIEW_INPUT_BYTES]
    } else {
        &bytes[..]
    };
    let turns = match source {
        Source::Claude => extract_claude(slice),
        Source::Codex => extract_codex(slice),
    };
    if turns.is_empty() {
        return vec![Line::styled(
            "(텍스트 턴 없음 — tool/thinking 호출만 있는 세션)".to_string(),
            dim(),
        )];
    }
    let mut lines = render_turns(&turns, source);
    if capped && lines.len() <= PREVIEW_MAX_LINES {
        lines.push(trunc_marker());
    }
    lines
}

// ---- 표시폭 word-wrap (search-hit-preview 후속) ----

/// 스타일 보존 word-wrap: 각 논리 Line 을 표시폭 ≤ `width` 의 시각 Line 들로 분할.
///
/// 분할점은 **공백 경계 우선**(경계의 공백 런은 드랍) — `search::terms` 는 공백 미포함이라
/// term 은 항상 한 시각 라인 안에 남는다(매치/앵커/하이라이트 보존, [D2] 좌표계 유지).
/// 단어 자체가 폭 초과일 때만 grapheme 경계 하드브레이크(걸친 term 은 top 폴백 — 희박, 수용).
/// **grapheme 단위**로 다뤄 ZWJ/VS16 이모지 클러스터를 절대 쪼개지 않고, 폭도 str-폭
/// (`Line::width`·ratatui 렌더와 동일 좌표계)으로 합산한다.
/// 연속줄은 원 라인의 선행 공백(들여쓰기, 폭 절반 상한)을 상속해 코드/불릿 좌변 유지.
/// `width == 0` → 입력 그대로(랩 항등 — 단위테스트/첫 프레임 전 경로).
pub fn wrap_lines(lines: &[Line<'static>], width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    if w == 0 {
        return lines.to_vec();
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in lines {
        if line.width() <= w {
            out.push(line.clone());
            continue;
        }
        // grapheme 셀: (클러스터 문자열, 셀 폭, 스타일)
        let cells: Vec<(String, usize, Style)> = line
            .spans
            .iter()
            .flat_map(|s| {
                s.content
                    .as_ref()
                    .graphemes(true)
                    .map(move |g| (g.to_string(), g.width(), s.style))
            })
            .collect();
        let lead_n = cells.iter().take_while(|(g, _, _)| g == " ").count().min(w / 2);
        let mut start = 0usize;
        let mut first = true;
        while start < cells.len() {
            let avail = if first { w } else { w - lead_n };
            let mut used = 0usize;
            let mut end = start;
            let mut last_space: Option<usize> = None;
            while end < cells.len() {
                let cw = cells[end].1;
                if used + cw > avail && end > start {
                    break;
                }
                used += cw;
                if cells[end].0 == " " {
                    last_space = Some(end);
                }
                end += 1;
            }
            let mut seg: Vec<&(String, usize, Style)> = Vec::new();
            if !first {
                seg.extend(cells[..lead_n].iter());
            }
            if end >= cells.len() {
                seg.extend(cells[start..].iter());
                out.push(rebuild_line(&seg, line));
                break;
            }
            let (cut, next) = match last_space {
                // 공백 경계: 공백 런 앞에서 자르고 런 뒤에서 재개.
                Some(sp) if sp > start => {
                    let mut cs = sp;
                    while cs > start && cells[cs - 1].0 == " " {
                        cs -= 1;
                    }
                    let mut nx = sp;
                    while nx < cells.len() && cells[nx].0 == " " {
                        nx += 1;
                    }
                    (cs, nx)
                }
                _ => (end, end), // 하드브레이크
            };
            if cut <= start || next <= start {
                // 방어(공백-only 초장라인 등): 하드 세그먼트로 진행 보장.
                seg.extend(cells[start..end].iter());
                out.push(rebuild_line(&seg, line));
                start = end;
            } else {
                seg.extend(cells[start..cut].iter());
                out.push(rebuild_line(&seg, line));
                start = next;
            }
            first = false;
        }
    }
    out
}

/// grapheme 셀 나열 → 동일 스타일 연속 구간을 스팬으로 묶어 Line 재조립.
fn rebuild_line(cells: &[&(String, usize, Style)], orig: &Line<'static>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cur = String::new();
    let mut cur_style: Option<Style> = None;
    for (g, _, st) in cells {
        match cur_style {
            Some(s) if s == *st => cur.push_str(g),
            Some(s) => {
                spans.push(Span::styled(std::mem::take(&mut cur), s));
                cur.push_str(g);
                cur_style = Some(*st);
            }
            None => {
                cur.push_str(g);
                cur_style = Some(*st);
            }
        }
    }
    if let Some(s) = cur_style {
        spans.push(Span::styled(cur, s));
    }
    let mut l = Line::from(spans);
    l.alignment = orig.alignment;
    l.style = orig.style;
    l
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

/// 현재 앵커(=논리 scroll+LEAD_IN) 기준 1-based "현재 매치" 순번 — anchor **이하**인 마지막 매치.
/// 앵커가 첫 매치보다 위(saturating 0 포함)면 1. `jump_match` 의 순회와 같은 좌표계.
/// 매치 없으면 0.
pub fn current_match(matches: &[usize], anchor: usize) -> usize {
    let n = matches.len();
    if n == 0 {
        return 0;
    }
    matches.partition_point(|&m| m <= anchor).clamp(1, n)
}

// ---- 검색 매치 하이라이트 (search-hit-preview) ----

/// `content` 안에서 terms(소문자) 중 하나라도 대소문자 무시 매치하는 **원본 바이트 구간**들(병합·정렬).
/// `to_lowercase()` 가 바이트 길이를 바꿀 수 있어(예: ß→ss, İ), 소문자 문자열의 각 바이트를
/// 원본 오프셋으로 되매핑해 슬라이스가 항상 원본의 char 경계에 떨어지게 한다(패닉 방지). [D5]
/// tui 의 리스트 제목 하이라이트도 재사용(pub(crate)).
pub(crate) fn match_ranges(content: &str, terms: &[String]) -> Vec<(usize, usize)> {
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
    let hl = theme::match_hl();
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

    fn plain_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn flat_of(lines: &[Line]) -> String {
        lines.iter().map(plain_of).collect::<Vec<_>>().join("\n")
    }

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

        // 렌더: 역할 칩 + 접힘 마커 존재, raw tool json 없음
        let lines = render_turns(&turns, Source::Claude);
        let flat = flat_of(&lines);
        assert!(flat.contains(" 나 "));
        assert!(flat.contains(" Claude "));
        assert!(flat.contains("⚒ Read: /x/main.rs"));
        assert!(flat.contains("💭"));
        assert!(!flat.contains("tool_use")); // raw 노이즈 없음
        assert!(!flat.contains("```")); // 펜스 마커 제거됨
        assert!(flat.contains("⌜rust")); // 언어 라벨로 치환
        assert!(flat.contains("fn main(){}")); // 코드 내용은 보존
        assert!(!flat.contains("**")); // 인라인 볼드 마커 제거
        assert!(flat.contains("이거")); // 볼드 내용은 보존
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
        let flat = flat_of(&lines);
        assert!(flat.contains(" Codex "));
        assert!(flat.contains("코덱스 답변"));
    }

    #[test]
    fn inline_bold_code_italic_and_fallback() {
        // bold
        let spans = inline_spans("이건 **중요** 함", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(joined, "이건 중요 함"); // 마커 제거
        let bold = spans.iter().find(|s| s.content.as_ref() == "중요").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        // inline code
        let spans = inline_spans("run `cargo test` now", Style::default());
        let code = spans.iter().find(|s| s.content.as_ref() == "cargo test").unwrap();
        assert_eq!(code.style.fg, Some(crate::theme::CODE));
        // italic (공백 규칙)
        let spans = inline_spans("*기울임* 그리고 3 * 4 = 12", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "기울임 그리고 3 * 4 = 12"); // 곱셈 * 는 보존
        // 짝 없는 마커 폴백
        let spans = inline_spans("미완 **볼드 와 `백틱", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "미완 **볼드 와 `백틱"); // 원문 무손실
        // snake_case 는 건드리지 않음
        let spans = inline_spans("foo_bar_baz", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "foo_bar_baz");
    }

    #[test]
    fn md_ordered_quote_nested_bullets() {
        let mut out = Vec::new();
        md_lines(
            "1. 첫째\n2) 둘째\n> 인용문\n- 상위\n  - 중첩",
            crate::theme::CLAUDE,
            &mut out,
        );
        let flat = flat_of(&out);
        assert!(flat.contains("1. 첫째"));
        assert!(flat.contains("2) 둘째"));
        assert!(flat.contains("▏ 인용문"));
        assert!(flat.contains("  • 상위"));
        assert!(flat.contains("    • 중첩")); // 들여쓰기 보존(2+indent)
    }

    #[test]
    fn heading_uses_accent_color() {
        let mut out = Vec::new();
        md_lines("## Plan", crate::theme::CODEX, &mut out);
        // Line::styled 는 라인 레벨 style 에 fg 를 싣는다(스팬은 default)
        assert_eq!(out[0].style.fg, Some(crate::theme::CODEX));
    }

    #[test]
    fn render_session_error_and_empty_messages() {
        let lines = render_session("/no/such/file.jsonl", Source::Claude);
        let flat = flat_of(&lines);
        assert!(flat.contains("⚠ 파일 읽기 실패"));
        assert!(flat.contains("/no/such/file.jsonl"));
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

    #[test]
    fn current_match_is_last_at_or_before_anchor() {
        let m = vec![3usize, 8, 14];
        assert_eq!(current_match(&m, 0), 1); // 첫 매치 전 → 1 (다음이 곧 현재)
        assert_eq!(current_match(&m, 3), 1); // 정확히 첫 매치
        assert_eq!(current_match(&m, 8), 2);
        assert_eq!(current_match(&m, 9), 2); // 매치 8 을 막 지남 → 여전히 2 가 현재
        assert_eq!(current_match(&m, 99), 3); // 다 지나침 → N
        assert_eq!(current_match(&[], 5), 0);
        // 상단 경계: 첫 매치가 0/1 이라 anchor(=scroll0+LEAD_IN=2)가 매치보다 커도 1
        assert_eq!(current_match(&[1, 9], 2), 1);
    }

    #[test]
    fn wrap_identity_when_fits_or_zero_width() {
        let lines = vec![Line::from("짧은 줄"), Line::from("second")];
        assert_eq!(wrap_lines(&lines, 40).len(), 2);
        assert_eq!(wrap_lines(&lines, 0).len(), 2); // width 0 → 항등
    }

    #[test]
    fn wrap_breaks_at_word_boundary_and_preserves_terms() {
        let lines = vec![Line::from("alpha beta gamma delta")];
        let out = wrap_lines(&lines, 12);
        // 각 시각 라인 폭 ≤ 12, 단어는 안 쪼개짐
        for l in &out {
            assert!(l.width() <= 12, "line too wide: {:?}", plain_of(l));
        }
        let flat = flat_of(&out);
        for term in ["alpha", "beta", "gamma", "delta"] {
            // term 이 한 라인 안에 통째로 존재(줄 걸침 없음)
            assert!(out.iter().any(|l| plain_of(l).contains(term)), "{term} split: {flat}");
        }
    }

    #[test]
    fn wrap_hard_breaks_overlong_word_and_cjk() {
        // 폭 초과 단어(공백 없음) → 하드브레이크로라도 전부 보존
        let lines = vec![Line::from("aaaaaaaaaaaaaaaaaaaa")]; // 20자, 폭 8
        let out = wrap_lines(&lines, 8);
        assert!(out.len() >= 3);
        let total: String = out.iter().map(|l| plain_of(l)).collect();
        assert_eq!(total, "aaaaaaaaaaaaaaaaaaaa");
        // CJK(2셀) 폭 계산
        let lines = vec![Line::from("가나다라마바사")]; // 14셀, 폭 6
        let out = wrap_lines(&lines, 6);
        for l in &out {
            assert!(l.width() <= 6);
        }
        let total: String = out.iter().map(|l| plain_of(l)).collect();
        assert_eq!(total, "가나다라마바사");
    }

    #[test]
    fn wrap_keeps_zwj_emoji_cluster_atomic() {
        // 가족 이모지(ZWJ 시퀀스)는 grapheme 1개 — 하드브레이크에서도 절대 쪼개지지 않는다
        let fam = "👨\u{200D}👩\u{200D}👧"; // 👨‍👩‍👧
        let long = format!("{}{}{}{}", fam, "x".repeat(6), fam, "y".repeat(6));
        let out = wrap_lines(&[Line::from(long.clone())], 6);
        let total: String = out.iter().map(|l| plain_of(l)).collect();
        assert_eq!(total, long); // 무손실
        for l in &out {
            let p = plain_of(l);
            // 조각난 ZWJ(끝이 ZWJ 로 끝나거나 시작) 없음 = 클러스터가 항상 통째
            assert!(!p.ends_with('\u{200D}') && !p.starts_with('\u{200D}'), "ZWJ split: {p:?}");
        }
        // VS16 이모지: str-폭 기준 합산이라 라인 폭이 Line::width 와 일관
        let vs = "⚠\u{FE0F} 주의 ".repeat(8);
        let out = wrap_lines(&[Line::from(vs)], 10);
        for l in &out {
            assert!(l.width() <= 10, "VS16 overflow: {:?}", plain_of(l));
        }
    }

    #[test]
    fn wrap_preserves_styles_and_inherits_indent() {
        let styled = Line::from(vec![
            Span::styled("  코드라인 ".to_string(), code_block_style()),
            Span::styled("qwertyuiopasdfghjkl".to_string(), Style::default()),
        ]);
        let out = wrap_lines(&[styled], 14);
        assert!(out.len() >= 2);
        // 첫 라인 첫 스팬은 코드 스타일 유지
        assert_eq!(out[0].spans[0].style.fg, Some(crate::theme::CODE));
        // 연속줄은 들여쓰기(2칸) 상속
        assert!(plain_of(&out[1]).starts_with("  "));
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
        // 스타일 라인 → 매치에 하이라이트 오버레이, base 유지(patch)
        let base = Style::default().fg(crate::theme::CODE);
        let out = highlight_lines(
            &[Line::from(Span::styled("## auth 설정", base))],
            &["auth".to_string()],
        );
        assert_eq!(plain_of(&out[0]), "## auth 설정");
        let m = out[0].spans.iter().find(|s| s.content.as_ref() == "auth").unwrap();
        assert_eq!(m.style.bg, match_hl().bg); // 하이라이트 배경
        assert_eq!(m.style.fg, match_hl().fg); // patch 로 fg 도 덮임(가독)
        let b = out[0].spans.iter().find(|s| s.content.as_ref() == "## ").unwrap();
        assert_eq!(b.style.fg, base.fg); // 주변은 base 유지

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

    #[test]
    fn dim_match_stays_bright() {
        // 접힌(DIM) 라인 위 매치 — match_hl 의 remove_modifier(DIM) 가 patch 로 전파
        let out = highlight_lines(
            &[Line::from(Span::styled("⚒ Read: auth.rs", dim()))],
            &["auth".to_string()],
        );
        let m = out[0].spans.iter().find(|s| s.content.as_ref() == "auth").unwrap();
        assert!(m.style.sub_modifier.contains(Modifier::DIM)); // DIM 제거 지시 포함
        assert!(m.style.add_modifier.contains(Modifier::BOLD));
    }
}
