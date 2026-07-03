# Phase 0 Research: 검색 매치 지점에 앵커되는 프리뷰

기존 코드(`src/tui.rs`, `src/preview.rs`, `src/search.rs`, `src/index/mod.rs`, `src/parser/claude.rs`)를 읽고 확정한 결정들. spec에 남았던 미지정 항목을 여기서 해소한다.

---

## D1. 앵커 메커니즘: 스크롤 오프셋 = 논리 라인 인덱스

- **Decision**: 프리뷰는 `Paragraph::new(lines).scroll((self.preview_scroll, 0))`로 렌더되고 **`.wrap()`를 쓰지 않는다**(`tui.rs:454-456`). 따라서 스크롤 오프셋(u16)은 **렌더된 `Vec<Line>`의 논리 라인 인덱스와 1:1**이다. 앵커 = `첫_매치_라인_인덱스 - LEAD_IN`을 `saturating_sub`로 클램프해 `preview_scroll`에 대입.
- **Rationale**: 랩이 없으니 "표시 행 ≠ 논리 라인" 문제가 없어 인덱스→오프셋 매핑이 정확. 라인 수는 최대 4000(`PREVIEW_MAX_LINES`) < `u16::MAX` → 오버플로 없음.
- **Alternatives**: 랩을 켜고 표시 행을 재계산 → 폭 의존·복잡·성능↓. 채택 안 함. 현재 no-wrap 정책 유지가 이 기능을 가장 단순하게 만든다.

## D2. 매치는 "검색 body"가 아니라 "렌더된 프리뷰 라인 텍스트"에서 찾는다

- **Decision**: 앵커/하이라이트의 매치 스캔 대상은 **프리뷰에 실제로 렌더된 라인들의 평문 텍스트**다. 검색 필터가 쓰는 인덱스 `body`(`SessionRow.body`)를 재사용하지 않는다.
- **Rationale**: 두 코퍼스가 다르다.
  - 검색 `body`(`parser/claude.rs::claude_body_text`) = 모든 `type=="text"` 블록 이어붙임(슬래시 래퍼 **포함**, thinking/tool_use/tool_result **제외**).
  - 프리뷰(`preview.rs::extract_claude`) = user/assistant text(슬래시 래퍼 **제외**) + thinking/tool_use **한 줄 접힘 요약** + tool_result 생략, 그리고 입력 500KB·출력 4000줄 **상한**.
  - 스크롤은 "렌더된 것"에만 가능하므로, 매치도 렌더된 라인에서 찾아야 앵커가 유효하다. 렌더 라인에 매치가 없으면(예: 슬래시 래퍼에만 있던 매치, 상한 밖 매치) 자연히 **top 폴백** → FR-006/FR-007 자동 충족.
- **Alternatives**: 인덱스 body 오프셋을 프리뷰 라인에 매핑 → 두 파서가 텍스트를 다르게 조립해 신뢰 불가. 기각.

## D3. 앵커 계산을 끼우는 지점: `recompute()`와 `move_sel()`

- **Decision**: 현재 `preview_scroll = 0`으로 리셋하는 **딱 두 곳**을 `preview_scroll = anchor_scroll(...)`로 바꾼다.
  - `App::recompute()` (`tui.rs:102`) — 쿼리 입력/백스페이스/소스순환/트리·스코프 전환 등 모든 리스트 갱신이 여기로 수렴. → **FR-009(쿼리 변경 시 재앵커)** 자동 충족.
  - `App::move_sel()` (`tui.rs:282`) — 위/아래 선택 이동.
- **Rationale**: Ctrl+D/U(`tui.rs:163-164`)는 이 두 함수를 호출하지 않고 `preview_scroll`을 직접 ±10 한다. 따라서 **한 번 수동 스크롤하면 선택/쿼리가 바뀌기 전까지 재앵커되지 않는다** → **FR-008(수동 스크롤 우선)**이 별도 플래그 없이 성립.
- **Note**: 앵커 계산은 현재 선택이 세션일 때만(그리고 쿼리가 비어있지 않을 때만) 수행. 비세션(Dir/Up)·빈 쿼리는 기존대로 0 → FR-010.

## D4. 앵커 대상 = 어느 검색어든 "가장 먼저" 나오는 렌더 라인

- **Decision**: 공백 분리 AND 쿼리에서, 각 term의 첫 등장 라인 인덱스 중 **최솟값**을 앵커로 삼는다(= 어느 term이든 최초 매치). proximity(여러 term 근접) 고려 안 함.
- **Rationale**: spec Assumptions와 일치. 구현 단순, 사용자 기대("맨 처음 관련 부분")와 부합. 대소문자 무시(`term.to_lowercase()` vs `line_text.to_lowercase()`) — 검색 규칙과 동일(FR-004).

## D5. 하이라이트: 스팬 분할(대소문자 무시), base 스타일 보존 + 오버레이

- **Decision**: 렌더된 각 `Line`의 스팬들을 순회하며, 스팬 content에서 term(대소문자 무시) 구간을 찾아 `[앞(원스타일), 매치(원스타일+하이라이트), 뒤(원스타일)…]`로 분할. 하이라이트 스타일은 `theme.rs`에 추가한 색(예: 배경 강조 또는 `REVERSED`).
- **Rationale**: 라인이 이미 heading/code/dim/bullet 스타일을 가질 수 있어(`preview.rs::md_lines`), 통째 교체가 아니라 **스팬별 부분 분할**로 base 스타일을 보존해야 한다. `Line`은 `Vec<Span>`이라 분할이 자연스럽다.
- **Alternatives**: 라인 전체를 한 스타일로 강조 → 코드/heading 스타일 소실, 과함. 기각.

## D6. 하이라이트 캐시 전략: base(path) 캐시 유지 + 하이라이트(path,query) 캐시

- **Decision**: 기존 `preview_cache: HashMap<path, Vec<Line>>`(하이라이트 없는 base)는 그대로. 하이라이트된 라인은 **(path, 정규화된 query) 키로 별도 캐시**하거나, `preview_content()`에서 쿼리 비어있지 않을 때만 base→하이라이트 변환을 수행하되 결과를 (path,query)로 메모.
- **Rationale**: Ctrl+D/U 스크롤 중에는 query가 안 바뀌므로 (path,query) 캐시가 **히트 → 스크롤 idle 유지**(D14 성능 게이트). query가 바뀔 때만(=이미 검색 재계산이 도는 키 입력 경로) 재변환. base 캐시를 query로 오염시키지 않아 하이라이트 off(빈 쿼리) 시 그대로 재사용.
- **Alternatives**: base 캐시 자체를 (path,query)로 → 빈 쿼리/쿼리 변할 때 JSONL 재렌더 유발 가능. 기각(파싱 재실행은 비쌈). 매 프레임 하이라이트 재계산 → 스크롤 idle 위반. 기각.

## D7. LEAD_IN(위쪽 맥락 줄 수)

- **Decision**: 매치 라인 위로 **2줄**의 맥락을 함께 노출(`anchor = first_match.saturating_sub(2)`). 상수로 두고 조정 가능.
- **Rationale**: FR-002. 매치가 최상단에 딱 붙지 않게 하되, 화면을 낭비하지 않는 최소값. 역할 헤더(`▌ 나`/`▌ Claude`)가 매치 바로 위에 오는 경우가 많아 2줄이면 맥락이 붙는다.

## D8. P3 다음/이전 매치 키바인딩

- **Decision**: **`Alt+n` = 다음 매치, `Alt+p` = 이전 매치**. `preview.rs`가 계산한 매치 라인 인덱스 목록을 순회하며 각 인덱스로 앵커.
- **Rationale/제약**: 평문 글자는 전부 쿼리로 들어간다(`Char(c) if !ctrl` catch-all, `tui.rs:169`). 그래서 매치 순회 키는 **모디파이어 필수**. Ctrl letter 중 `Ctrl+I`(Tab)/`Ctrl+J`(LF)/`Ctrl+M`(CR)/`Ctrl+H`(BS)는 터미널이 특수키로 바꿔 위험, `Ctrl+N/P`는 이미 리스트 이동. → **Alt+n/p**가 안전·mnemonic. crossterm이 `KeyModifiers::ALT`로 전달.
- **구현 주의**: 현재 catch-all이 `Char(c) if !ctrl`이라 **Alt+n이 쿼리 'n'으로 새는 버그**가 생긴다. Alt 조합 arm을 catch-all **앞**에 두고, catch-all 가드를 `Char(c) if !ctrl && !alt`로 강화해야 함(`tui.rs`의 "새 ctrl 암은 catch-all 앞" 주석과 동일 원칙). P3라 MVP에선 생략 가능.

## 미해결/후속(비차단)

- 접힌 thinking/tool 블록 안에만 있는 매치를 **펼쳐서** 보여줄지 → v1 범위 밖(spec Assumptions). 후속에서 `/speckit-clarify` 대상.
- 퍼지 검색 모드(후속 nucleo 토글) 도입 시 하이라이트/앵커의 "매치 구간" 정의 재검토 필요(현재는 exact substring 전제).
