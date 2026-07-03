---
description: "Task list: 검색 매치 지점에 앵커되는 프리뷰"
---

# Tasks: 검색 매치 지점에 앵커되는 프리뷰 (Search-hit-anchored preview)

**Input**: Design documents from `specs/001-search-hit-preview/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/internal.md, quickstart.md (모두 존재)

**Tests**: 포함 — `contracts/internal.md`가 계약 테스트를 명시하고, 프로젝트가 순수 함수를 `cargo test`로 단위 검증하는 관례(`search.rs`/`preview.rs`/`tui.rs`).

**Organization**: 유저스토리별 그룹(P1 → P2 → P3). 단일 Rust 크레이트라 같은 파일을 만지는 태스크는 [P] 아님.

> **개정(analyze 반영)**: F1 — `match_lines`를 Foundational로 이동해 US3를 US1과 독립화. F2 — 다중어 앵커 테스트 추가. F3 — "제목만 매치→top" App 테스트 추가.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 다른 파일 · 의존 없음 → 병렬 가능
- **[Story]**: US1/US2/US3 (Setup·Foundational·Polish은 라벨 없음)
- 파일 경로 명시

## Path Conventions

단일 Rust 크레이트, 소스는 저장소 루트 `src/`. 테스트는 각 소스 파일의 `#[cfg(test)] mod tests`(기존 관례).

---

## Phase 1: Setup

**Purpose**: 변경 기준선 확인 (기존 프로젝트라 신규 스캐폴딩 없음)

- [X] T001 기준선 확인: `cargo build && cargo test` 통과, 변경 지점 특정 — `src/tui.rs:102`(`recompute` 스크롤 리셋), `src/tui.rs:282`(`move_sel` 스크롤 리셋), `src/tui.rs:163-164`(Ctrl+D/U), 프리뷰 Paragraph `.wrap()` 없음(`src/tui.rs:454`) 재확인

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: 세 스토리가 공유하는 프리미티브 — 검색어 추출, 하이라이트 스타일, **매치 라인 계산**(앵커=US1·순회=US3 공용)

**⚠️ CRITICAL**: 이 단계 완료 전 유저스토리 시작 불가

- [X] T002 [P] 매치 하이라이트 스타일 추가 in `src/theme.rs` (예: `match_hl() -> Style`, 배경 강조 또는 `REVERSED`; 듀얼톤 테마와 충돌 없게)
- [X] T003 [P] 공유 `terms(query: &str) -> Vec<String>`(공백 분리 + 소문자, 빈/공백만 → 빈 Vec) 추가 in `src/search.rs`, 기존 `filter()`가 이를 호출하도록 리팩터(동작 불변)
- [X] T004 `terms()` 단위 테스트 추가 in `src/search.rs` (`terms("Auth JWT")==["auth","jwt"]`, `terms("  ")==[]`), 기존 `filter` 테스트 회귀 없음
- [X] T005 [P] `line_plain(&Line) -> String`(스팬 content 이어붙임) + `match_lines(lines: &[Line], terms: &[String]) -> Vec<usize>`(term 하나 이상 포함 라인, 오름차순, 대소문자 무시) 추가 in `src/preview.rs`
- [X] T006 `match_lines` 단위 테스트 in `src/preview.rs` — "jwt"가 라인 5·12 → `[5,12]`; 대소문자 무시("JWT" vs "jwt"); **다중어 `["토큰","검증"]` → 두 라인 모두, 오름차순(F2: 최초=어느 term이든 가장 이른 인덱스)**; 무매치 → `[]`; 빈 terms → `[]`

**Checkpoint**: 공유 프리미티브(terms·match_lines·하이라이트색) 준비 — US1/US2/US3 각각 독립 시작 가능

---

## Phase 3: User Story 1 - 매치 지점으로 프리뷰 자동 이동 (Priority: P1) 🎯 MVP

**Goal**: 검색어 활성 상태에서 세션을 선택/이동하면 프리뷰가 맨 위가 아니라 **첫 매치가 보이는 위치**(위로 ~2줄 맥락)로 앵커된다.

**Independent Test**: 후반부에만 특정 단어가 있는 긴 세션을 그 단어로 검색 → 선택 시 수동 스크롤 없이 매치 구간이 보임. Ctrl+D/U 후 재앵커 안 함. 빈 쿼리·비세션·제목만 매치 → top.

### Tests for User Story 1 ⚠️

- [X] T007 [US1] `anchor_scroll` 단위 테스트 in `src/preview.rs` — 첫매치5+lead_in2 → 3; 첫매치1+lead_in2 → 0(saturating); 무매치 → 0; 빈 terms → 0 (구현 전 작성 → FAIL 확인)

### Implementation for User Story 1

- [X] T008 [US1] `anchor_scroll(lines: &[Line], terms: &[String], lead_in: u16) -> u16`(`match_lines`의 첫 인덱스 `saturating_sub(lead_in)`, 무매치/빈terms → 0) 추가 in `src/preview.rs`
- [X] T009 [US1] `LEAD_IN` 상수(=2) + `App::anchor_for_selection(&mut self) -> u16`(현재 항목이 세션이고 쿼리 비어있지 않으면 캐시된 base 프리뷰 라인으로 `preview::anchor_scroll` 계산, 아니면 0) 추가 in `src/tui.rs`
- [X] T010 [US1] `recompute()`(`src/tui.rs:102`)·`move_sel()`(`src/tui.rs:282`)의 `self.preview_scroll = 0`을 `self.preview_scroll = self.anchor_for_selection()`로 교체 in `src/tui.rs`
- [X] T011 [US1] App 앵커 동작 테스트 in `src/tui.rs` (`handle_key` 시임): 후반 매치 픽스처로 쿼리 입력 시 `preview_scroll>0`; `move_sel` 재앵커; Ctrl+D/U는 재앵커 안 함; 빈 쿼리·비세션 → 0; **쿼리가 제목에만 매치(본문 렌더에 없음) → `preview_scroll==0`(F3: top 폴백, FR-006/FR-007)**

**Checkpoint**: US1 단독 완전 동작 — "검색→선택→매치 구간 바로 보임" MVP 완성

---

## Phase 4: User Story 2 - 프리뷰 안 매치 단어 강조 (Priority: P2)

**Goal**: 프리뷰에 보이는 검색어를 시각적으로 강조. 스크롤해도 재계산 없이 idle 유지.

**Independent Test**: `JWT`로 검색 시 본문 `jwt`가 주변과 구분되게 강조(대소문자 무시), 공백 다중어 모두 강조. (US1 없이도 검증 가능)

### Tests for User Story 2 ⚠️

- [X] T012 [US2] `highlight_lines` 단위 테스트 in `src/preview.rs` — "토큰 JWT 검증"+`["jwt"]` → 스팬 3개(가운데 하이라이트), 평문 이어붙이면 원본 동일; heading 스타일 라인 → 매치에 하이라이트 오버레이 + base 유지; 다중어 둘 다; 빈 terms → 입력과 동일 clone (구현 전 작성 → FAIL 확인)

### Implementation for User Story 2

- [X] T013 [US2] `highlight_lines(lines: &[Line<'static>], terms: &[String]) -> Vec<Line<'static>>`(스팬별 term 경계 분할, base 스타일 보존 + `theme::match_hl()` 오버레이, 평문 보존, 빈 terms → clone) 추가 in `src/preview.rs`
- [X] T014 [US2] `App::preview_content()`에서 현재 항목이 세션이고 쿼리 비어있지 않으면 base 라인에 `highlight_lines` 적용, 결과를 (path, query) 키 캐시에 저장(스크롤 중 쿼리 불변 → 히트) in `src/tui.rs`
- [X] T015 [US2] 스크롤 idle 회귀 확인: Ctrl+D/U 연타 시 (path,query) 캐시 히트로 재하이라이트 없음(quickstart.md 회귀 체크 절차)

**Checkpoint**: US1+US2 각각 독립 동작 — 매치 지점 이동 + 강조

---

## Phase 5: User Story 3 - 매치 순회 (Priority: P3)

**Goal**: `Alt+n`/`Alt+p`로 현재 세션 안 다음/이전 매치로 이동.

**Independent Test**: 같은 단어가 3곳인 세션에서 Alt+n 누를 때마다 다음 매치로 이동. Alt+n이 검색 쿼리에 `n`으로 새지 않음. (Foundational의 `match_lines` 사용 — US1과 독립)

### Tests for User Story 3 ⚠️

- [X] T016 [US3] Alt 순회 테스트 in `src/tui.rs` — 다중 매치 픽스처에서 Alt+n → 다음 `match_lines` 항목으로 `preview_scroll` 이동, Alt+p → 이전, Alt+n이 `query`에 안 들어감(기존 `ctrl_keys_dont_leak_into_query` 패턴 확장) (구현 전 작성 → FAIL 확인)

### Implementation for User Story 3

- [X] T017 [US3] `handle_key`에 `KeyCode::Char('n') if alt`·`Char('p') if alt` arm을 catch-all **앞**에 추가하고, catch-all 가드를 `Char(c) if !ctrl && !alt`로 강화(현재 `Char(c) if !ctrl`) in `src/tui.rs`
- [X] T018 [US3] 현재 세션 `match_lines`에서 현 `preview_scroll` 기준 다음/이전 매치 인덱스로 앵커(끝에서 순환 또는 정지 — 하나로 고정하고 T016에 명시); 매치 0개면 no-op in `src/tui.rs`

**Checkpoint**: 세 스토리 모두 독립 동작

---

## Phase 6: Polish & Cross-Cutting Concerns

- [X] T019 [P] 키바인딩 문서화(앵커 동작 + 하이라이트 + `Alt+n`/`Alt+p`) in `README.md` 및 `docs/DESIGN.md`
- [~] T020 [P] fmt/clippy — 내 추가 코드는 rustfmt 스타일로 손수 정렬(신규 코드 clippy lint 0건). **크레이트 전체 `cargo fmt` 는 미실행**: 저장소가 애초에 fmt-clean 이 아니라 전체 재포맷 시 무관 파일(tree/parser/resume/main/index)까지 churn → 되돌리고 기능 4개 파일만 남김. `-D warnings` 는 **기존 코드 7건**(rustc 1.92 신규 lint: `extract_claude` boolean 2 + parser 문서주석 정렬 5) 잔존 — 이 기능 무관 + 저자 정렬 문서 뭉개짐 우려로 미수정, 사용자 판단에 맡김
- [X] T021 `quickstart.md` 수동 검증: US1/US2/US3 + 엣지(제목만 매치→top, 상한 밖/접힘 매치→top 폴백, 빈 쿼리→기존 동작) — 크래시 0, 스크롤 idle 확인

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup(P1)**: 의존 없음
- **Foundational(P2)**: Setup 후 — 모든 스토리 차단(공유 `terms()`·`match_lines`·하이라이트 스타일)
- **US1(P3)**: Foundational 후 — 다른 스토리 의존 없음(MVP)
- **US2(P4)**: Foundational 후 — US1과 논리적 독립(단 `src/preview.rs`·`src/tui.rs` 공유 → 순차 작업 권장)
- **US3(P5)**: Foundational 후 — **이제 US1과 독립**(match_lines가 Foundational에 있음)
- **Polish(P6)**: 원하는 스토리 완료 후

### Within Each User Story

- 테스트 먼저 작성 → FAIL 확인 → 구현(순수 함수 → App 통합)
- `src/preview.rs` 순수 함수(T008/T013) → `src/tui.rs` 통합(T009/T010/T014/T017/T018)

### Parallel Opportunities

- **Foundational**: T002(`theme.rs`) · T003(`search.rs`) · T005(`preview.rs`)는 다른 파일 → **병렬 가능** [P]
- **Polish**: T019(docs) · T020(fmt/clippy) 병렬 가능 [P]
- 그 외는 `src/preview.rs`·`src/tui.rs` 공유 → **직렬**(같은 파일 충돌 회피)

---

## Parallel Example: Foundational

```bash
# 다른 파일이라 동시 진행 가능:
Task: "매치 하이라이트 스타일 추가 in src/theme.rs"           # T002
Task: "terms() 추가 + filter 리팩터 in src/search.rs"          # T003
Task: "line_plain + match_lines 추가 in src/preview.rs"        # T005
```

---

## Implementation Strategy

### MVP First (User Story 1)

1. Phase 1 Setup(T001) → 2. Phase 2 Foundational(T002-T006) → 3. Phase 3 US1(T007-T011)
4. **STOP & VALIDATE**: 후반 매치 단어로 검색 → 선택 시 앵커되는지 독립 검증
5. 여기까지가 사용자 핵심 요청("grep 한 자리 대화내역 보이게")

### Incremental Delivery

1. Setup + Foundational → 기반 완성
2. US1(앵커) → 독립 검증 → **MVP**
3. US2(하이라이트) → 독립 검증
4. US3(매치 순회) → 독립 검증

---

## Notes

- [P] = 다른 파일 · 무의존. `preview.rs`·`tui.rs` 공유가 많아 [P] 기회 제한적.
- 테스트는 구현 전 작성해 FAIL 확인(순수 함수는 인라인 `Line` 픽스처, 파일 I/O 없음).
- 성능 게이트: US2 캐시는 반드시 (path,query) 키 — 스크롤 idle(D14) 회귀 금지.
- 각 태스크/논리 그룹 후 커밋 권장(공개 저장소 커밋 여부는 사용자 결정 대기).
