//! 색 팔레트 — 소스별 듀얼톤. Claude=주황(테라코타) / Codex=그린(에메랄드). [D14 후속, dogfooding 2차]
//!
//! claude-history 의 teal 단일 지배에서 벗어나 **소스를 색으로** 표현한다. 크롬(프롬프트·선택바)은
//! 호버 세션의 소스색을 따라가고(`tui::App::accent`), 디렉토리/소스없는 행은 [`NEUTRAL`].

use crate::model::Source;
use ratatui::style::{Color, Modifier, Style};

/// Claude = Anthropic 테라코타 주황.
pub const CLAUDE: Color = Color::Rgb(217, 119, 87);
/// Codex = 에메랄드 그린 (구 teal 와 구분되게 더 초록 쪽).
pub const CODEX: Color = Color::Rgb(52, 186, 124);
/// 디렉토리/소스없는 크롬(트리 노드, UP, 빈 선택).
pub const NEUTRAL: Color = Color::Rgb(170, 170, 170);

/// 소스 문자열("codex"/"claude") → 색.
pub fn source_color(source: &str) -> Color {
    if source == "codex" {
        CODEX
    } else {
        CLAUDE
    }
}

/// `Source` enum → 색.
pub fn source_color_e(source: Source) -> Color {
    match source {
        Source::Codex => CODEX,
        Source::Claude => CLAUDE,
    }
}

/// 검색 매치 하이라이트 — 노랑 배경 + 검정 글씨 + 볼드. 듀얼톤(라이트/다크) 무관하게 가독.
/// base 스타일에 `patch` 로 얹어 매치 substring 만 이 색으로 덮는다(주변 스팬은 base 유지). [search-hit-preview]
pub fn match_hl() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Rgb(240, 200, 80))
        .add_modifier(Modifier::BOLD)
}
