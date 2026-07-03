# Phase 1 Contracts: 내부 인터페이스

csess는 외부 API를 노출하지 않는 개인 TUI 도구다. "계약"은 (1) 사용자 대면 **키바인딩**과 (2) 테스트 가능한 **내부 순수 함수 시그니처**다.

## 1. 키바인딩 계약 (사용자 대면)

| 키 | 동작 | 상태 |
|---|---|---|
| 타이핑 | 검색 쿼리에 추가 → 리스트 갱신 + **첫 매치로 앵커** | 앵커 신규(P1) |
| ↑/↓, Ctrl+P/Ctrl+N | 선택 이동 → 선택 세션 **첫 매치로 앵커** | 앵커 신규(P1) |
| Ctrl+D / Ctrl+U | 프리뷰 ±10줄 스크롤(재앵커 안 함) | 기존, 의미 불변 |
| Alt+n / Alt+p | **다음 / 이전 매치**로 이동 | 신규(P3) |

- 앵커·하이라이트는 **쿼리가 비어있지 않고 선택이 세션일 때만** 활성. 그 외엔 기존 동작(top, 하이라이트 off).
- 기존 키(Tab/Ctrl-h/g/s/a/o/y, Enter, Esc, Ctrl-c)의 의미는 **불변**. Alt+n/p 추가 시 catch-all 가드를 `Char(c) if !ctrl && !alt`로 강화(D8).

## 2. 내부 순수 함수 계약 (`src/preview.rs`, `src/search.rs`)

```rust
// search.rs — 공유 term 추출(검색·앵커·하이라이트 단일 규칙)
/// 공백 분리 + 소문자화. 빈/공백만 → 빈 Vec.
pub fn terms(query: &str) -> Vec<String>;

// preview.rs — 매치 계산/하이라이트(모두 순수, 파일 I/O 없음)
/// 각 라인 평문에 term(이미 소문자) 하나 이상 포함 → 그 라인 인덱스(오름차순).
pub fn match_lines(lines: &[Line], terms: &[String]) -> Vec<usize>;

/// 첫 매치 라인 - lead_in(saturating). 매치 없음/terms 빈 경우 0.
pub fn anchor_scroll(lines: &[Line], terms: &[String], lead_in: u16) -> u16;

/// base 라인 → 매치 substring을 하이라이트 스팬으로 분할한 라인.
/// terms 비면 입력을 그대로 clone. 평문은 보존(문자 손실 없음).
pub fn highlight_lines(lines: &[Line<'static>], terms: &[String]) -> Vec<Line<'static>>;
```

### 계약 테스트(행위 명세 — `cargo test`로 검증)

**`terms`**
- `terms("Auth JWT")` == `["auth","jwt"]`; `terms("  ")` == `[]`.

**`match_lines`** (대소문자 무시, FR-004)
- "jwt"가 라인 5·12에 있는 입력, `terms(["jwt"])` → `[5,12]`.
- 본문 "JWT" + term "jwt" → 매치(대소문자 무시).
- 매치 없음 → `[]`.

**`anchor_scroll`** (FR-001/FR-002/FR-006/FR-007)
- 첫 매치 5, lead_in 2 → `3`.
- 첫 매치 1, lead_in 2 → `0`(saturating).
- 매치 없음 → `0`(폴백). terms 빈 경우 → `0`.

**`highlight_lines`** (FR-003/FR-005, US2)
- 라인 "토큰 JWT 검증", terms `["jwt"]` → 스팬 3개, 가운데 "JWT"에 하이라이트 스타일; 평문 이어붙이면 원본과 동일.
- heading 스타일 라인의 매치 → 매치 구간에 하이라이트가 **오버레이**되고 heading base 스타일 유지(D5).
- 다중 term `["토큰","검증"]` → 두 구간 모두 하이라이트.
- terms 빈 경우 → 입력과 동일(clone), 스팬 분할 없음.

## 3. tui.rs 통합 계약 (상태 로직 — 기존 `handle_key` 테스트 패턴)

- `recompute()`/`move_sel()`가 `preview_scroll`을 `anchor_scroll(...)` 결과로 설정(세션+비빈쿼리), 그 외 0.
- Ctrl+D/U는 `preview_scroll`만 ±10 — `recompute`/`move_sel` 호출 안 함(수동 우선, FR-008).
- (P3) Alt+n/p는 현재 세션 `match_lines`에서 현 스크롤 기준 다음/이전 인덱스로 앵커; 끝에서 순환 또는 정지(일관되게).
