# Phase 1 Data Model: 검색 매치 지점에 앵커되는 프리뷰

이 기능은 영속 데이터(SQLite 스키마) 변경이 없다. 다루는 것은 **인메모리 값 객체와 UI 상태**다. 아래는 그 값들과 파생 규칙.

## 값 객체 / 파생값

### QueryTerms
- **정의**: `App.query`를 공백 분리 + 소문자화한 substring 항목 목록(AND).
- **출처**: 현재 `search::filter`가 내부에서 만드는 것을 **공유 헬퍼 `search::terms(&str) -> Vec<String>`로 추출**해 검색·앵커·하이라이트가 같은 규칙을 쓰게 한다.
- **규칙**: 빈 문자열/공백만 → 빈 목록 → 앵커·하이라이트 비활성(FR-010). 대소문자 무시(FR-004).

### RenderedPreview
- **정의**: `Vec<Line<'static>>` — `preview::render_session(path, source)`의 출력(역할 헤더 + 텍스트 + 접힘 요약). 하이라이트 없는 **base**.
- **캐시**: `App.preview_cache: HashMap<path, Vec<Line>>` (기존 유지).
- **제약**: 입력 500KB / 출력 4000줄 상한(`preview.rs`). 상한 밖 본문은 여기에 없다.

### MatchSet (파생)
- **정의**: (RenderedPreview, QueryTerms)에서 계산.
  - `match_lines: Vec<usize>` — term을 하나 이상 포함하는 라인 인덱스(오름차순). P3 순회용.
  - `first: Option<usize>` = `match_lines.first().copied()` — 어느 term이든 최초 매치(D4).
- **규칙**: terms 비거나 매치 없음 → `match_lines` 빈 목록, `first = None`.

### AnchorScroll (파생)
- **정의**: `u16` 스크롤 오프셋 = `first.map(|i| i.saturating_sub(LEAD_IN)).unwrap_or(0)`, `LEAD_IN = 2`(D7). u16 캐스팅(라인 수 ≤ 4000 안전).
- **적용 지점**: `App::recompute()`·`App::move_sel()`의 기존 `preview_scroll = 0`을 대체(D3). 비세션/빈 쿼리 → 0.

### HighlightedPreview (파생)
- **정의**: `Vec<Line<'static>>` — base 라인의 각 스팬을 term 경계로 분할, 매치 구간에 하이라이트 스타일 오버레이(base 스타일 보존, D5).
- **캐시**: (path, 정규화 query) 키. 스크롤 중 query 불변 → 캐시 히트로 재계산 0(D6, 성능 게이트). terms 비면 base를 그대로 clone(하이라이트 off).

## 상태 전이 (App)

| 트리거 | preview_scroll | 하이라이트 | 근거 |
|---|---|---|---|
| 쿼리 입력/삭제(`recompute`) | AnchorScroll 재계산 | (path,query) 재계산 | FR-009 |
| 위/아래 선택 이동(`move_sel`) | 새 세션의 AnchorScroll | 새 세션 (path,query) | US1 |
| Ctrl+D / Ctrl+U | ±10 (재앵커 안 함) | 불변 | FR-008 |
| Alt+n / Alt+p (P3) | 다음/이전 `match_lines` 항목으로 | 불변 | US3 |
| 빈 쿼리 / 비세션 선택 | 0 | off | FR-010 |
| 매치가 렌더에 없음 | 0 (top 폴백) | (보이는 매치 없음) | FR-006/FR-007 |

## 불변식

- `match_lines`의 모든 인덱스 < `rendered.len()`.
- `AnchorScroll` ≤ 마지막 라인 인덱스(스크롤 오프셋이 내용 밖으로 나가도 ratatui가 빈 화면 → 크래시 없음, 단 앵커는 항상 유효 인덱스 기준이라 실무상 안전).
- 하이라이트 스팬 분할 후 라인의 평문(content 이어붙임)은 원본과 **동일**(문자 손실/삽입 없음).
