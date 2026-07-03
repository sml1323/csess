# Implementation Plan: 검색 매치 지점에 앵커되는 프리뷰 (Search-hit-anchored preview)

**Branch**: `001-search-hit-preview` | **Date**: 2026-07-03 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/001-search-hit-preview/spec.md`

## Summary

검색으로 본문이 매치된 세션을 선택하면, 프리뷰가 **맨 위가 아니라 첫 매치가 보이는 위치**로 열리고(P1), 프리뷰 안 매치 단어가 **강조**되며(P2), 키로 **다음/이전 매치**를 순회할 수 있게(P3) 한다.

**기술 접근**: 프리뷰 Paragraph는 `.wrap()`을 쓰지 않으므로 **스크롤 오프셋 = 논리 라인 인덱스(1:1)**. 이미 캐시되는 base 렌더 라인(`Vec<Line>`)의 **평문 텍스트를 스캔**해 (a) 첫 매치 라인 인덱스를 구해 앵커 스크롤로 쓰고, (b) 매치 substring을 하이라이트 스팬으로 분할한다. 앵커 계산은 스크롤 리셋이 일어나는 딱 두 지점(`recompute()`, `move_sel()`)에만 끼운다 — Ctrl+D/U는 리셋을 안 하므로 수동 스크롤 우선(FR-008)이 자동 충족된다. 앵커를 렌더된 라인 텍스트 기준으로 계산하므로, 검색 코퍼스에만 있고 프리뷰엔 안 보이는 매치(슬래시 래퍼·상한 초과)는 자동으로 top 폴백(FR-006/FR-007)된다.

## Technical Context

**Language/Version**: Rust 2021 (rustc 1.92; rusqlite 0.32 핀 — [[rusqlite-version-pin]])

**Primary Dependencies**: `ratatui`(TUI 렌더/`Paragraph`/`Line`/`Span`), `crossterm`(키), `serde_json`(JSONL), `rusqlite`(인덱스). 신규 크레이트 도입 없음.

**Storage**: SQLite 인덱스(`src/index/mod.rs`) — 이 기능은 스키마 변경 없음. 프리뷰는 원본 JSONL 파일을 직접 읽어 인메모리 렌더.

**Testing**: `cargo test` — 순수 함수 단위 테스트(현행 `search.rs`/`preview.rs`/`tui.rs` 패턴 계승).

**Target Platform**: macOS/Linux 터미널(TUI).

**Project Type**: 단일 Rust 바이너리 크레이트(CLI/TUI). `src/` 플랫 모듈.

**Performance Goals**: 프리뷰는 "세션당 1회 렌더 후 캐시 → 스크롤 idle"(DESIGN [D14]). 앵커·하이라이트가 이 특성을 **깨지 않아야** 함: 앵커 계산은 선택/쿼리 변경당 O(렌더 라인 수) 1회; 하이라이트는 (path,query) 캐시로 스크롤 중 재계산 0.

**Constraints**: 키 입력마다의 검색 경로에 체감 지연 추가 금지(현재 인프로세스 substring AND). 하이라이트/앵커는 이미 추출된 라인 텍스트 대상이라 JSONL 재파싱 없음.

**Scale/Scope**: 코퍼스 ~255–259 resume 세션. 프리뷰 상한 = 입력 500KB / 출력 4000줄(`preview.rs`). 단일 세션 프리뷰 내에서만 동작.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

프로젝트 헌법(`.specify/memory/constitution.md`)은 **미작성 템플릿(placeholder)**이라 비준된 게이트가 없다. 대신 프로젝트 실제 원칙(`CLAUDE.md`, `docs/DESIGN.md`)을 게이트로 적용한다:

| 게이트(프로젝트 원칙) | 상태 | 근거 |
|---|---|---|
| **성능: 스크롤 idle 유지** [D14] | ✅ PASS | 앵커=선택/쿼리 변경당 1회 스캔, 하이라이트=(path,query) 캐시 → 스크롤 중 재계산 없음 |
| **테스트 시임(순수 함수)** | ✅ PASS | 매치 인덱스 계산·하이라이트 스팬 분할을 `preview.rs` 순수 함수로 → 단위 테스트 가능 |
| **"일단 쓸 수 있는 것" / MVP 우선** | ✅ PASS | P1(앵커) 단독으로 사용 가능한 슬라이스, P2/P3는 증분 |
| **신규 의존성 최소** | ✅ PASS | ratatui `Span` 스타일만 사용, 크레이트 추가 없음 |
| **lossy/폴백 안전** | ✅ PASS | 렌더 라인 기준 앵커 → 코퍼스 미스매치는 top 폴백, 크래시 없음(FR-007) |

위반 없음 → Complexity Tracking 불필요.

## Project Structure

### Documentation (this feature)

```text
specs/001-search-hit-preview/
├── plan.md              # This file
├── spec.md              # Feature spec
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── internal.md      # 키바인딩 + 내부 함수 계약
├── checklists/
│   └── requirements.md  # spec 품질 체크리스트
└── tasks.md             # Phase 2 output (/speckit-tasks — 아직 없음)
```

### Source Code (repository root)

```text
src/
├── search.rs      # [변경] 매치 판정 규칙 재사용 헬퍼 노출(term 분리/lowercase) — 하이라이트·앵커가 공유
├── preview.rs     # [변경] 렌더 라인의 평문 추출 + 첫/다음/이전 매치 라인 인덱스 계산 + 하이라이트 스팬 분할(순수 함수)
├── tui.rs         # [변경] App: 앵커 계산 훅(recompute/move_sel), 하이라이트 적용(preview_content),
│                  #        쿼리 변경 시 재앵커(FR-009), P3 다음/이전 매치 키
├── theme.rs       # [변경] 하이라이트 스타일(match highlight) 색 1개 추가
├── model.rs       # 변경 없음
├── index/mod.rs   # 변경 없음(검색 body 스키마 그대로)
└── parser/        # 변경 없음
```

**Structure Decision**: 기존 단일 크레이트 플랫 모듈 유지. 신규 파일/모듈 없이 4개 파일(`search.rs`/`preview.rs`/`tui.rs`/`theme.rs`)에 최소 침습으로 얹는다. 매치 계산·하이라이트 분할은 `preview.rs`의 **순수 함수**로 넣어 `tui.rs`의 상태 로직과 분리(테스트 시임 확보).

## Complexity Tracking

> Constitution Check 위반 없음 — 작성 불필요.
