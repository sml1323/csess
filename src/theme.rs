//! 색 팔레트 — 소스별 듀얼톤. Claude=주황(테라코타) / Codex=그린(에메랄드). [D14 후속, dogfooding 2차]
//!
//! claude-history 의 teal 단일 지배에서 벗어나 **소스를 색으로** 표현한다. 크롬(프롬프트·선택바)은
//! 호버 세션의 소스색을 따라가고(`tui::App::accent`), 디렉토리/소스없는 행은 [`NEUTRAL`].
//!
//! **다크 터미널 전제.** 고정 bg([`SEL_BG`])와 밝은 fg 상수들은 다크 배경 기준으로 튜닝됐다
//! (개인 도구 결정 — 라이트 지원은 비목표). TUI 렌더 전용 팔레트이며, `model::DIM/RESET`
//! (fzf ANSI, 골든 파리티 소속)과는 별개 세계다 — 절대 여기로 옮기지 말 것.

use crate::model::Source;
use ratatui::style::{Color, Modifier, Style};

/// Claude = Anthropic 테라코타 주황.
pub const CLAUDE: Color = Color::Rgb(217, 119, 87);
/// Codex = 에메랄드 그린 (구 teal 와 구분되게 더 초록 쪽).
pub const CODEX: Color = Color::Rgb(52, 186, 124);
/// 디렉토리/소스없는 크롬(트리 노드, UP, 빈 선택).
pub const NEUTRAL: Color = Color::Rgb(170, 170, 170);
/// 선택바 배경(다크 전제). 구 tui.rs 하드코딩 값 그대로.
pub const SEL_BG: Color = Color::Rgb(40, 44, 52);
/// 밝은 본문 강조(user 턴 헤더 등). 구 preview.rs 하드코딩 값 그대로.
pub const TEXT_BRIGHT: Color = Color::Rgb(235, 235, 235);
/// 코드블록 — 듀얼톤(테라코타/에메랄드) 어느 쪽도 아닌 등거리 스틸블루.
/// (구 살몬 Rgb(206,145,120)은 CLAUDE 테라코타와 hue 가 겹쳐 '코드'가 'Claude 강조'로 읽혔다.)
pub const CODE: Color = Color::Rgb(130, 160, 190);
/// 코드블록 배경 틴트 — SEL_BG 보다 어둡게, 코드/프로즈 경계를 색 하나에 안 맡김.
pub const CODE_BG: Color = Color::Rgb(30, 32, 38);
/// 리스트/프리뷰 구분선·스크롤바 등 뒤로 물러날 크롬 — SEL_BG 보다 살짝 밝게.
pub const SEPARATOR: Color = Color::Rgb(70, 75, 85);
/// 경고/매치 노랑 — resume 거부 status, 리스트 매치 fg, 매치 하이라이트 bg 공용.
pub const WARN: Color = Color::Rgb(240, 200, 80);

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

/// 공용 dim 스타일 (헬퍼 — tui/preview 가 각자 정의하던 것 통합).
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// 검색 매치 하이라이트 — 노랑 배경 + 검정 글씨 + 볼드. 듀얼톤(라이트/다크) 무관하게 가독.
/// base 스타일에 `patch` 로 얹어 매치 substring 만 이 색으로 덮는다(주변 스팬은 base 유지).
/// `remove_modifier(DIM)`: patch 의 sub_modifier 로 전파돼 DIM 라인(접힌 ⚒/💭) 위 매치도
/// 감광 없이 또렷하다 — 매치는 라인 종류와 무관하게 같은 밝기여야 한다. [search-hit-preview]
pub fn match_hl() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(WARN)
        .add_modifier(Modifier::BOLD)
        .remove_modifier(Modifier::DIM)
}
