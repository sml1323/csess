//! 세션 파서 — 라인-tolerant JSONL 파싱 공용 헬퍼 + 소스별 파서.
//!
//! Phase A 의 순수-함수 분리(`csess_index_file` ↔ fzf/exec)를 계승: 파서는 TUI 없이
//! 단독 호출(`--index-file`)·테스트 가능하다. 라인 단위 tolerant 파싱이 핵심 — 코퍼스의
//! 17.7MB invalid-surrogate 파일에서 strict 단일 패스는 abort 하지만(뒤 라인 silent 누락)
//! 줄별 `serde_json::from_slice` + Err 스킵은 생존한다. [D3]

pub mod claude;
pub mod codex;

use regex::Regex;
use std::sync::OnceLock;

/// 제목 정제: `[\t\n\r]+` 런을 단일 공백으로(Phase A `gsub("[\t\n\r]+";" ")`), 그 뒤
/// 80 codepoint 컷 (jq `.[0:80]` == Rust `chars().take(80)`). 정제→컷 **순서** 중요. [D6]
pub fn sanitize_title(t: &str) -> String {
    let collapsed = ws_re().replace_all(t, " ");
    collapsed.chars().take(crate::model::TITLE_MAX).collect()
}

fn ws_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\t\n\r]+").unwrap())
}
