# Design: 세션 브라우저 TUI (Claude/Codex resume launcher)

## Problem Statement

Claude Code와 Codex 대화 세션이 디렉토리별로 흩어져 로컬에 쌓인다. `claude --resume`은
**현재 디렉토리의 세션만**, 그것도 내용 검색 없이 보여준다. 다른 프로젝트 세션을 이어가려면
원래 작업경로를 찾아 `cd` 하고 세션 ID를 복사하는 수동 작업이 필요하다.

실측(2026-06-01): 이 머신에 resume 가능 Claude 세션 **~255개 / 45개 프로젝트** (전체 ~496 JSONL 중 ~241개는
서브에이전트 저널이라 제외). Codex는 `~/.codex/sessions/`에 별도로 **411개** 더 쌓임. 손으로 뒤지는 건 사실상 불가능.

핵심 페인: **"그때 그 에러/주제 어느 세션에서 물어봤더라?"** — 파일명이 아니라 대화 내용으로
세션을 찾고, 골라서 바로 그 세션으로 점프하고 싶다.

## What Makes This Cool

- **내용으로 세션을 찾는다.** 파일명/경로가 아니라 대화 안의 말로. fzf 퍼지 매칭이라
  `auth jwt`처럼 대충 쳐도 근접하게 뜬다.
- **크로스 프로젝트.** 45개 디렉토리에 흩어진 ~255개를 한 화면에. `claude --resume`이 못 하는 것.
- **엔터 한 번에 점프.** 고르면 그 자리에서 `chdir` 후 `claude --resume`로 프로세스 교체(exec) —
  복사·붙여넣기·cd 없이 바로 그 세션이 터미널에 뜬다.

## Constraints

- 빌더 모드 / 개인 도구. "내가 쓰려고" — 일단 쓸 수 있는 게 최우선, 공유는 보너스.
- 1단계는 주말 + α 안에 실제로 매일 쓸 만한 결과물.
- 검색 엔진을 직접 만들지 않는다 (fzf에 위임).
- 1단계는 Claude만. Codex는 포맷 확인 후 합류.

## Premises (모두 합의됨)

1. **세션 파일에서 필요한 게 다 나온다.** Claude `~/.claude/projects/<dir>/<id>.jsonl`,
   Codex `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`. 둘 다 로컬 JSONL이지만
   **스키마가 다르다** (파서를 분리해야 함):
   - Claude: `cwd`·`sessionId`가 이벤트 라인 최상위. 첫 줄은 `last-prompt` 같은 메타라 cwd가
     없을 수 있음 → 여러 라인 스캔해 첫 유효 cwd 채택.
   - Codex: `cwd`·`id`가 `session_meta` 라인의 `.payload` 아래 중첩 (`.payload.cwd`, `.payload.id`).
   - 공통: **디렉토리명/파일명 디코딩은 신뢰 불가** (`/`·`.`·`-`가 뭉개짐) → cwd는 항상 JSONL
     내용에서 읽고, 디코딩은 최후 폴백(부정확 표시).
2. **"바로 이동"의 정체 = exec.** 자식 프로세스는 부모 셸 cwd를 못 바꾼다. 도구가 직접
   `chdir` 후 `claude --resume <id>`로 exec(프로세스 교체)하면 그 터미널에 그대로 뜬다.
   클립보드 복사(`cd ... && claude --resume ...`)는 폴백.
3. **fzf가 검색 엔진.** 우리는 "세션당 한 줄(경로+시각+첫 질문)"을 잘 뽑아 fzf에 먹이고
   `--preview`로 마지막 대화를 보여줄 뿐. 퍼지 매칭은 fzf가 한다.
4. **만들 가치가 있다.** `claude --resume`은 현재 디렉토리·검색 없음. 크로스 프로젝트 +
   내용 검색은 빈 자리. 안 만들면 계속 손으로 cd.

## Approaches Considered

### Approach A: 셸 원샷 (Minimal Viable) — 선택됨, 1단계
`bash/zsh + fzf + jq` 한 스크립트. 세션 JSONL 긁어 한 줄씩 → fzf `--preview` → 선택 시
chdir + exec `claude --resume`. 노력 S / 리스크 Low. 버릴 코드가 아니라 B의 스펙 문서가 됨.

### Approach B: Rust TUI (Ideal / 학습 본진) — 선택됨, 2단계
`ratatui` + `nucleo`(fzf 제작자의 Rust 퍼지 크레이트). 리스트/검색창/미리보기 패널 직접.
단일 바이너리. Codex 합류. C의 SQLite 인덱스를 흡수 → 타임라인/분석까지 확장 가능.
노력 L~XL / 리스크 Med (학습 목적이라 일부러 손으로).

### Approach C: 인덱스 먼저 (Creative / lateral) — B에 흡수
모든 세션을 SQLite로 인덱싱(프로젝트, 시각, 메시지 수, 토큰, 첫/끝 메시지). TUI는 그 뷰.
검색 즉시 + 접어둔 "디렉토리 타임라인/분석"이 거의 공짜로 열림. B의 부품으로 채택.

## Recommended Approach

**A → B.** 주말에 셸 버전(A)으로 실물을 만들어 매일 쓰면서 데이터 스키마·미리보기 포맷·
exec 로직을 굳힌다. 그 스펙 그대로 Rust(B)로 다시 지으며 언어/TUI를 깊게 판다. 미지수(도메인 +
Rust)를 동시에 풀지 않고 하나씩. C의 인덱스는 B에서 SQLite로 흡수.

## Open Questions (2026-06-01 리뷰로 대부분 해소)

- **Codex 스키마/resume** — ✅ 확인됨. `cwd`·`id`는 `session_meta` 라인의 `.payload` 아래.
  `codex resume <uuid>`(명시 UUID)는 picker 건너뜀. 코퍼스 411개. **Phase A는 Claude만, Codex는 합류 보류**
  (남은 미지수: Codex의 "첫 사람 질문" 라인 모양 — Claude와 다름, 합류 시 ~15분 probing).
- **미리보기** — ✅ 결정: **첫 질문(내가 시작한 것) + 마지막 출력** 두 섹션. 첫 질문은 head 에서 `first()` 단락,
  마지막 출력은 `tail -n N | jq` (slurp 금지). preview 안은 ctrl-d/u 스크롤로 탐색, 전체는 enter 로 resume.
- **검색 방식** — ✅ 결정: fzf `--exact` (fuzzy 아닌 **문자열 substring** 매치, 공백 구분 = AND). 검색 범위는 제목 + 프로젝트경로.
- **세션 정렬** — ✅ 결정: mtime 최근순. 프로젝트별 그룹은 보너스(보류).

## Success Criteria

- 한 명령(`csess` 같은 alias)으로 ~255개 세션이 최근순으로 뜬다.
- 대화 내용 단어로 퍼지 검색해서 원하는 세션을 5초 안에 찾는다.
- 엔터 → 그 세션이 올바른 디렉토리에서 바로 resume 된다 (복사/cd 수동 작업 0).
- 일주일간 실제로 손이 가서 쓴다. (안 쓰면 스펙이 틀린 것 → B 가기 전에 고친다)

## Distribution Plan

- **A**: 단일 셸 스크립트. `~/bin/csess` 또는 dotfiles에 넣고 alias. 배포 불필요(개인용).
- **B**: `cargo build --release` → 단일 바이너리. 나중에 오픈소스로 가면 GitHub Releases +
  `cargo install` / Homebrew tap. 1단계에선 보류.

> ✅ **성능 (2026-06-01 실측, 리뷰로 정정):** 원래 추정 "443개 × 파일당 jq = ~7.5초"는 **재현 안 됨.**
> 실측: 순진한 파일당 jq **~3.0초**, 병렬 파일당(`find -print0 | xargs -0 -P8`) **~0.4–1.9초**, 단일 패스 **~0.5초**.
> resume 가능 세션 **255–259개** (전체 ~496 JSONL 중 **~241개는 depth-3 서브에이전트 저널 → 제외**), 코퍼스 ~375MB.
> 성공 기준("5초 안에")은 **캐시 없이도 충족.** → **1단계는 캐시 없이 매 실행 fresh 빌드.**
> (mtime 캐시는 ~2000+ 세션 넘을 때의 최적화로 보류 — Phase B SQLite가 흡수. 캐시를 빼면 무효화·원자적쓰기·삭제유령·bash3.2 연관배열 버그 클래스가 통째로 사라진다.)

1. **매 실행 fresh 빌드 (캐시 없음).** `find ~/.claude/projects -mindepth 2 -maxdepth 2 -name '*.jsonl' -print0`
   → `xargs -0 -P8`로 파일당 병렬 파싱 → mtime 역순 정렬. **`-mindepth 2 -maxdepth 2`가 곧 서브에이전트 필터**
   (`**` 재귀 글롭 금지 — depth-3 `<session>/subagents/*`는 resume 불가). 파싱은 **line-tolerant** `jq -R 'fromjson? // empty'` —
   단일 `jq -n inputs` 패스 금지 (코퍼스의 invalid-surrogate 파일 1개(17.7MB)가 strict 패스를 abort → 뒤 세션 silent 누락).
2. 각 세션에서 추출: sessionId + 첫 유효 cwd + mtime + 첫 질문 제목(80자).
   - **cwd:** 모든 라인 타입 스캔, 첫 non-null `.cwd` (user/assistant 라인엔 없을 때 많음 — 라인 타입 가정 금지).
   - **첫 질문:** `type==user`에서 content 문자열 → 사용 / 배열이면 첫 text 블록 / tool_result·image-only 배열이면 그 라인 스킵.
     **추가로** `<command-name>`·`<command-message>`·`<local-command-caveat>`·`<task-notification>` 등 슬래시-커맨드/시스템 래퍼 라인도 스킵하고 첫 **실제 사람 질문**을 (안 그러면 ~51%가 boilerplate 제목).
   - **정제:** `gsub("[\t\n\r]+";" ")` 후 80자 컷 (첫 질문 ~50–60%가 멀티라인이라 정제 없으면 한 세션이 fzf 여러 줄로 쪼개짐).
3. 한 줄 TSV: 숨김 필드(`mtime`·`id`·`cwd`·`path`)를 **먼저**, free-text 제목을 **뒤**에 (제목 잔여 문자가 exec가 읽는 id/cwd를 밀어낼 수 없게). 화면엔 `--with-nth`로 제목+프로젝트만 (검색 범위도 그 컬럼들로 한정됨 — 의도대로).
4. fzf `--preview`: 해당 JSONL을 **`tail -n N | jq` line-tolerant 렌더** (slurp 금지 — 멀티-MB 파일 RAM 폭발 + corrupt 파일 abort 회피; O(1) 메모리 ~5ms).
5. **선택 후 실행:** 부모 셸 cwd는 자식이 못 바꾸므로 "stranded" 걱정 없음(**Premise 2와 일치 — 원래 step5의 "바뀐 디렉토리에 갇힘" 근거는 틀렸다**). 핵심은 **cd 가드**:
   `[[ -n $id ]]` + cwd 비었거나 `! -d $cwd`면 **거부**(절대 `cd "" && resume ""` 안 함) → `cd "$cwd" || { echo …; exit 1; }; exec claude --resume "$id"`.
   `claude --resume <id>`는 **cwd-스코프**라(맞는 프로젝트 디렉토리에서 호출해야 세션을 찾음) cd는 안전이 아니라 **정확성** 필수. 빈 fzf 선택 → `exit 0`. (`--copy`면 명령만 `pbcopy` — macOS.)
6. 일주일 dogfooding → Open Questions 실측 확정 → Codex 합류 → B 착수.

## What I noticed about how you think

- "음 다 좋은데? ... 이거는 좀 어려워 보이고" — 타임라인/분석을 멋지다고 보면서도 **지금 난이도가
  높다고 스스로 접었다.** 욕심과 현실감을 동시에 쥐는 건 흔치 않다. 그래서 10x로 빼두는 결정이 깔끔했다.
- "2번 상황을 자주 겪어서" — 멋져 보이는 기능이 아니라 **본인이 실제로 겪는 마찰**을 핵심으로
  골랐다. 좋은 도구는 거의 항상 자기 문제에서 나온다.
- "1 + 4?" — 물음표까지 붙여서 던졌지만, 이건 사실 가장 성숙한 답이었다. 쉬운 것(셸)으로 빨리
  배우고 어려운 것(Rust)으로 깊게 가는 걸 직관적으로 묶었다. 단계적 학습을 본능으로 안다는 신호.

---

## 리뷰 확정 (2026-06-01)

실측 기반 멀티에이전트 리뷰 + 독립 outside-voice 검증. `bin/csess`(Claude 전용 Phase A) 구현·검증 완료.
실측은 모두 이 머신 코퍼스(~255 resume 가능 세션, ~496 JSONL, 17.7MB corrupt 파일 1개 포함)에 직접 돌려 확인.

**잠긴 결정 (스크립트에 반영됨):**
1. 캐시 없음 — 매 실행 fresh 빌드(`find -mindepth 2 -maxdepth 2 -print0 | xargs -0 -P8`), ~1.8초. (7.5초는 재현 안 됨.)
2. depth-2 글롭 = 서브에이전트 필터 (depth-3 `*/subagents/*` ~241개 제외). **0 누수 확인.**
3. 파일당 line-tolerant `jq -R 'fromjson? // empty'` — invalid-surrogate 파일에서도 abort 안 함(단일 패스는 abort 확인).
4. cwd = 모든 라인 타입에서 첫 non-null `.cwd`. 디코딩 폴백은 표시만, resume은 cwd 검증 후 거부.
5. 첫 질문 = 슬래시-커맨드/시스템 래퍼 스킵 후 첫 사람 질문 (denylist로 boilerplate 제목 0/259).
6. TSV 정제(`gsub` 후 80자) + 숨김 필드 앞·제목 뒤 (멀티라인 ~50–60% → 행 쪼개짐 방지).
7. exec: `cd "$cwd" || exit 1` 가드(= **정확성** 필수, `--resume`는 cwd-스코프) 후 `exec`. 부모 셸 stranded는 불가능(Premise 2).
8. preview = 첫 질문 + 마지막 출력 (head `first()` + `tail -n N | jq`, slurp 금지, ctrl-d/u 스크롤). 검색 = fzf `--exact` 문자열 매치.
9. 셰뱅 `#!/usr/bin/env bash` (macOS bash 3.2-safe, 연관배열 미사용).
10. 테스트 시임: `--index-file`, `CSESS_CLAUDE_ROOT`, `CSESS_DRY_RUN`. (bats/golden은 Phase B.)

**보류 (→ `TODOS.md`):** Codex 합류(포맷 확인 완료), 순수-슬래시 세션 제목에 `/command` 표시, 디코딩 폴백 완전 제거, bats 하니스(Phase B).

---

## Phase A 개선 + prior art 재프레이밍 (2026-06-01)

리서치 요약(21 소스 → 102 주장 → 25 적대적 검증, 23 confirmed). 아래는 그 결론과 그로 인한 Phase A 변경.

### prior art 재프레이밍 (B의 명분 변경)

- **거의 동일한 도구가 이미 성숙:** `raine/claude-history`(302★, ratatui, Ctrl+R resume/Ctrl+F fork, 활발 유지),
  `chronologos/cc-sessions`(Rust TUI, depth-2 스캔·JSONL에서 cwd 읽기까지 본 DESIGN과 글자 그대로 동일) 외 ~4개.
- **전부 Claude 전용** → csess의 **Codex 합류(`codex resume`)가 유일하게 빈 차별점.**
- **결론:** Phase B의 "도구가 없어서 짓는다" 명분은 죽었다. B는 오직 **(a) Codex 합류 + (b) Rust/TUI 학습**으로만 정당화.
  학습 동기가 약해지면 `claude-history` fork(Codex 파서만 추가)가 더 빠른 길. **Phase B 착수 직전 경쟁 도구 재확인 필수**(빠르게 움직임).
- **제3 선택지(보류):** `television` 커스텀 TOML 채널로 거의 코드 없이 구현 가능 — 단 execute 모드가 cwd 가드를 통과시키는지 미검증.

#### Phase B 방향 확정 (2026-06-01, dogfooding 후 결정)

사용자가 **손코딩을 안 함**(에이전트가 구현) → "Rust/TUI 학습(일부러 손으로)" 명분은 빠진다.
그럼에도 **fork 가 아니라 from-scratch build-own** 으로 결정. 재정의된 명분:
1. **Codex 합류** — claude-history 등 성숙 도구가 전부 Claude 전용이라 유일하게 빈 차별점.
2. **계층 디렉토리 트리(yazi식)** — claude-history 는 "No directory drill-down". csess 의 [D14] 트리(경로압축+rollup)는
   사용자가 명시적으로 선호하는 뷰라 차별점이자 build-own 의 근거.
3. **인프로세스 렌더** — Phase A 셸의 스크롤 CPU(선택마다 jq+bat 서브프로세스)를 근본 해결.
4. **자체 코드베이스 제어** — DESIGN 스펙대로 깨끗하게. 에이전트가 코딩하므로 손시간 부담 없음.

→ `claude-history` 는 **fork 대상이 아니라 렌더링 참고용**(레저 레이아웃/툴 휴먼화/syntect 해부는 D14 리서치에 있음).
**안전망:** 사용자가 Rust 를 줄단위로 검수하지 않으므로, 데이터 레이어는 Phase A(`bin/csess --index-file`) 출력과의
**골든 패리티 테스트**를 1순위로 깐다(동일 코퍼스 동치 검증). 구현 전 아키텍처/스키마 합의 필수.

### 잠긴 결정 11 — 디렉토리는 네비게이션 단계가 아니라 **표시 컬럼** [D11→재확인]

리서치 Q3: yazi식 디렉토리→세션 드릴다운(처음 아이디어)은 csess 페인("그때 그 세션")엔 **마찰만 추가**.
claude-history도 flat 퍼지 + "No directory drill-down". → **flat 단일 검색 유지가 옳음.** 디렉토리는 컬럼/필터로만 노출.
(yazi식 miller column / 다중 모드는 의도적으로 **채택 안 함**. fzf로 가능은 하나 불필요.)

### 잠긴 결정 12 — Phase A UX 보강 (`bin/csess`에 반영)

리서치 Q4/Q5 컨벤션 + Q2 fzf 능력 확인에 근거. TSV를 5필드 → **8필드**로 확장(`mtime id cwd path title  reltime projDisplay projPlain`).
파생 3필드는 title **뒤**에 — title은 이미 sanitize되어 tab/newline이 없으므로 [D6] 불변식(주입 문자가 id/cwd를 못 밀어냄)을 깨지 않는다.

1. **상대시각 컬럼** (`reltime`, dim, 4칸 우정렬): `now/6m/19m/4d/15mo`. 페인의 절반이 "언제"라 리스트 스캔성↑. (캐시 없는 매-실행 빌드라 stale 없음.)
2. **프로젝트 컬럼** (`projDisplay`, dim): 긴 full cwd 대신 마지막 2 path 컴포넌트(`work/csess`, `회사/test_erp`). 짧고 충분히 고유.
   `--with-nth '6,5,7'`로 화면=검색을 `reltime + title + project`로. dim ANSI는 `--ansi`가 표시엔 쓰고 매칭엔 strip(검색은 plain).
3. **enter/tab 듀얼 액션** (atuin accept vs return-selection): `--expect=tab,ctrl-y`. `enter`=즉시 resume(exec) / `tab`(=`ctrl-y`)=`cd && resume` 명령만 복사.
   `--copy` CLI 플래그를 **라이브 키로** 승격(실행 전 미리 결정할 필요 없음). 출력 첫 줄=눌린 키(enter면 빈 줄), 둘째 줄=선택.
4. **"이 프로젝트만" 필터** (`ctrl-g`): 호버 세션의 `projPlain`을 `{8}`로 받아 쿼리 세팅(`transform-query(printf %s {8})`), `ctrl-x`로 해제.
   `{n}` placeholder는 `--with-nth`와 무관하게 원본 필드를 가리킴(`--preview`의 `{4}`=path와 동일 원리). yazi 드릴다운 대신 flat 필터.
   - **footgun 회피(리서치 Q2):** fzf는 `{}`를 셸 실행 *전에* 평가 → 바인드 안에서 `{}` 쓰는 외부 도구 임베드 금지. `printf %s {8}`은 단일 placeholder라 안전.
5. **정렬은 mtime 유지** (Q5 표준). frecency(zoxide)는 Phase B SQLite의 2차 정렬 보너스로 보류.

**실측 검증(이 머신, 2026-06-01):** 인덱스 263 세션 / **~1.8s**, 전 라인 8필드 0오류, 빈 id/cwd 0개,
reltime 8경계값 width=4 정확, `--expect` 4케이스(enter/tab/ctrl-y/취소) 정확, fzf 0.70 바인드 전부 파싱 통과.
bash 3.2.57 + `bash -n` OK (연관배열 미사용 유지).

---

## 잠긴 결정 13 — 본문 전체 검색 + 전체-대화 프리뷰 + 디렉토리 드릴다운 (2026-06-01, dogfooding 피드백)

D12 직후 실사용에서 나온 3가지 피드백으로 검색·프리뷰 모델을 재설계. **D11(flat 기본)은 유지** —
드릴다운은 기본이 아니라 opt-in 모드다(기본은 여전히 flat 본문 검색).

1. **검색 = 전체 대화 본문** (이전: 제목+프로젝트만). 페인이 "내용으로 찾기"인데 제목만 검색하면 본문으로 못 찾음.
   - fzf 자체 매칭을 끄고(`--disabled`) `change:reload($self --browse {q})` 로 **키 입력마다** content 검색.
   - 백엔드: **rg 우선, grep 폴백**(`-l -i -F`). 인덱스 경로만 검색 → 매칭 경로를 awk join 으로 인덱스 라인에 매핑(mtime 순서 보존).
   - **공백 = AND**(기존 `--exact` 의미 유지): term 별로 파일 집합을 좁힌다. 단어 1개는 노이즈 많지만(raw JSONL) 2~3어면 정밀.
   - **실측:** 264파일/297MB 전체 검색 **0.02–0.04s**(rg). AND 동작 확인(`박스히어로 관리자`=2 ⊂ `박스히어로`=3). grep 폴백 동일 결과.
   - 트레이드오프: raw JSONL 검색이라 메타/툴 노이즈 매칭 가능(흔한 단어는 광범위). 정밀도는 다중어로.
2. **프리뷰 = 전체 대화 스트리밍 렌더** (이전: 첫질문+마지막출력 2섹션 → 스크롤할 게 없어 ctrl-d/u 가 "안 먹는" 것처럼 보임).
   - `jq -R 'fromjson?'` 라인-tolerant 스트리밍(slurp 금지 — corrupt surrogate/멀티-MB 생존), user/assistant 텍스트를 역할 헤더(`▌ 나`/`▌ Claude`, ANSI)와 함께.
   - `head -n 4000` 으로 출력 상한(거대 세션 프리뷰 지연 방지; jq 는 SIGPIPE 로 조기 종료). 17MB corrupt 파일도 안전.
   - **실측:** 한 세션 1146줄 렌더(user 7 + assistant 41턴) → ctrl-d/u 스크롤 의미 생김.
3. **디렉토리 드릴다운 + 프로젝트 스코프 토글** (사용자 요청 — yazi식 디렉토리→세션. D11 권고를 사용자가 override, 단 opt-in).
   - 상태는 매-실행 temp dir 의 두 파일(`scope`/`mode`) — bash3.2 연관배열 회피, 연구가 지적한 "fzf 는 상태에 /tmp I/O 필요"를 그대로 수용.
   - `ctrl-o` → 디렉토리 뷰(프로젝트 목록 + 세션 수 + 최근시각), `enter` → 그 프로젝트로 드릴(스코프 설정 후 세션 뷰), `ctrl-a` → 전체.
   - `ctrl-g` → 호버 세션의 프로젝트 스코프 **토글**(껐다 켰다). 스코프 켜지면 검색·목록이 그 프로젝트로 한정.
   - 디렉토리 라인도 **8필드 동일 구조**(세션과 같은 `--with-nth`/`{3}`/`{4}` 동작): `mtime '' cwd DIR:cwd projlabel reltime count projplain`. id 없음 → resume 가드가 자연히 거부.
   - fzf 바인드는 모두 `key:transform($self --xxx {n})` 로 단순화 — 동적 로직을 self 서브커맨드로 빼서 중첩 따옴표·footgun 회피. transform stdout 을 fzf 액션열(`reload`/`change-prompt`/`accept`)로 해석.

**구조 변경:** 5필드 인덱스 변수 → 8필드 인덱스 **temp 파일**(`$CSESS_DIR/idx`). 새 서브커맨드(테스트 시임 겸 fzf reload 소스):
`--browse {q}` · `--on-enter {3}` · `--toggle-scope {3}` · `--enter-dirs` · `--all`. 새 env: `CSESS_DIR`(상태 dir, main 이 export) · `CSESS_GREP`(검색 백엔드).
`exec` 는 EXIT trap 을 안 태우므로 `csess_resume` 이 exec 직전 temp dir 을 직접 rm.

**실측 검증(2026-06-01):** `bash -n` OK · 본문검색 0.02–0.04s/AND 동작 · 프리뷰 1146줄/색 정상 · dirs 38프로젝트 전부 8필드 ·
transform 6종 액션 문자열 정확(토글 on/off, 드릴, accept) · fzf 0.70 바인드 파싱 0에러. **인터랙티브 라이브 플로우는 dogfooding 으로 확인 예정**(헤드리스로 미검증).

**보류/리스크:** 흔한 단어 검색 노이즈(raw JSONL) → 필요시 본문 텍스트만 추출 검색으로 정밀화(캐시 필요, Phase B). 거대 세션 프리뷰 지연(rare, head -n 으로 완화).

---

## 잠긴 결정 14 — 계층 디렉토리 트리(yazi식) + 프리뷰 미려화 (2026-06-01, dogfooding 피드백 + prior-art 프로빙)

D13 의 평면 디렉토리 뷰(`ctrl-o`)와 텍스트-only 프리뷰를 사용자 요청으로 재설계. **D11(flat 기본)은 유지** —
트리/예쁜 프리뷰는 opt-in 이고 기본은 여전히 flat 본문 검색. 결정 전 멀티에이전트 프로빙으로 실측 근거 확보:
상위 디렉토리에 가져온 `raine/claude-history`(Rust TUI) 렌더링 해부 + 코퍼스 직접 측정 + fzf/도구 가용성 점검.

**측정으로 굳힌 근거 (이 머신, 2026-06-01):**
- **프리뷰가 대화의 ~75%를 버림:** assistant 턴 11,316개 중 51.1% 순수 tool_use·24.0% 순수 thinking·24.9% 순수 text.
  텍스트-only 렌더는 ~25%만 보이고 나머지는 빈 줄. → 사용자 결정: **tool/thinking 은 생략 OK**, 남는 텍스트만 예쁘게.
- **마크다운 빈도:** assistant 텍스트 블록의 48.2%가 마크다운(bold 44%·bullet 27%·`##` 23%·table 15%·fence 13%). → 색칠 가치 있음.
- **디렉토리 중첩:** 39 unique cwd / 270 세션, **38/39 가 다른 cwd 밑에 중첩.** `test_erp` 가 평면 목록에선 8행으로
  흩어지고, `work` 하나가 세션 65%·프로젝트 28개. → 홈 디스크 스캔(빈 폴더 노이즈)도 평면 목록(중첩 깨짐)도 아닌
  **세션 cwd 로만 친 계층 트리**가 정답(모든 노드가 정의상 resume 가능 = 디스크 스캔 불필요).

**1. 계층 디렉토리 트리 (yazi식, `tab`).** 세션 cwd 만으로 트리를 친다(`csess_list_tree`, awk).
   - **경로 압축:** own-session 없는 단일 체인은 한 노드로(`specs/001-vendor-migration`, `dsaf/my_background/upbit_websocket`).
     다중 자식 노드는 펼친다(`my_background` 7, `app` 2). → 깊이 체감 감소.
   - **rollup 카운트:** 부모 노드는 서브트리 전체 세션 수(`work` 176, `work/회사` 128, `test_erp` 96). count desc, mtime desc 정렬.
   - **혼합 행:** 한 레벨에 하위 디렉토리(`📁`, bold) + 그 자리 직속 세션(원본 idx 라인 → resume)을 yazi처럼 함께. UP 행(`⬅ ..`)도.
   - **8필드 동일 구조** 유지(세션과 같은 `--with-nth '6,5,7'`/`{3}`/`{4}`): `mtime '' node TREE:node 라벨 reltime 카운트 라벨`.
     id 없는 트리 행은 resume 가드가 자연히 거부.
   - 상태 파일 `dirpath`(현재 prefix, 빈=ROOT=$HOME) 추가. `enter`=UP/드릴/resume(`{4}` 로 행 종류 판별), `ctrl-h`/UP행=상위, `ctrl-a`=전체.

**2. 프리뷰 미려화 (`bat`, 속도 우선).** user/assistant **텍스트 턴만** 추출(tool_use/tool_result/thinking 생략 — 사용자 결정).
   - jq 로 마크다운 본문 + 역할 마커(`@@CSESS-ROLE:*@@`) 방출 → **`bat` 한 패스**(`-l md --color=always --style=plain --paging=never --wrap=never`)
     로 마크다운/펜스코드 구문 하이라이트 → awk 로 마커를 truecolor 역할 헤더(`▌ 나` 흰 굵게 / `▌ Claude` teal 굵게, claude-history 팔레트)로 치환 + 빈 줄 dedup.
   - bat 가 마커에 색을 덧칠해도 **strip 후 매칭**하므로 견고. `command -v bat` 가드 + `CSESS_NO_BAT` 로 plain 폴백(역할 헤더는 유지).
   - 줄바꿈은 `--preview-window 'wrap'`(CJK 정확)에 맡기고 bat 은 `--wrap=never`. 한 번의 bat 콜이라 빠름.

**3. 키 재배치 (D13→변경).** `tab`=계층 트리 진입(was 복사), `ctrl-y`=복사(was tab/ctrl-y), `ctrl-h`=트리 상위(신규), `ctrl-o`=트리 진입(alias).
   **footgun 회피:** 한 키는 `--expect` 와 `--bind` 중 **하나에만**. `tab` 을 `--expect` 에서 빼 `--bind` 로만, `ctrl-y` 는 `--expect` 로만.
   `enter` 는 `{3}`→`{4}` 로 변경(경로 prefix `UP:`/`TREE:` 로 드릴 vs accept 판별).

**구조 변경:** `csess_list_dirs`→`csess_list_tree`(경로압축/rollup, awk 자체 배열 — bash3.2 연관배열 회피). `csess_preview_dir`→`csess_preview_tree`.
새 함수 `csess_render_session`·`csess_md_color`·`csess_role_headers`·`csess_homerel`·`csess_tree_up`·`csess_enter_tree`. 새 상태 파일 `dirpath`. 새 env `CSESS_NO_BAT`.
서브커맨드: `--enter-dirs`→`--enter-tree`, `--tree-up` 신규, `--on-enter {3}`→`{4}`.

**실측 검증(2026-06-01, 헤드리스):** `bash -n`/`zsh -n` OK · `--index-file` 8필드 회귀 OK · 트리 ROOT/드릴/경로압축/rollup 정확
(work 176·회사 128·test_erp 96, `specs/001-vendor-migration`·`demo/esl`·`company37/target_company` 압축 확인) · transform 6종 액션열 정확(드릴 `~/work/회사/test_erp>`, UP, accept, no-op) ·
프리뷰: 마커 0 누출·thinking/tool 0 누출·역할 헤더·`##`/`**`/리스트/**python 펜스 구문색** 정상 · 속도 일반 세션 ~0.16s/17.7MB corrupt 파일 ~1.7s(기존 동일, bat 은 head 4000줄만) ·
flat 회귀(270줄·AND 검색) OK. **인터랙티브 라이브 플로우는 dogfooding 으로 확인 예정.**

**보류/리스크:** bat 기본 테마(사용자 `BAT_THEME` 존중, 미설정 시 vivid). 트리 직속 세션이 많은 노드(test_erp 88행)는 길지만 fzf 스크롤·타이핑 필터로 완화.

### D14 후속 (dogfooding 1차 피드백, 2026-06-01)

1. **헤더 잘림 → 짧은 헤더 + `?` 도움말.** 좁은 터미널에서 한 줄 헤더가 잘렸다. 헤더는 핵심만
   (`타이핑=검색 · enter 진입 · tab 트리 · ? 도움말`)으로 줄이고, 전체 키 레퍼런스는 `?` 키 →
   `preview($self --help-keys)` 로 프리뷰 패인에 띄운다(아무 키로 이동하면 기본 프리뷰로 복귀 —
   `man fzf` 가 명시한 "default preview + extra preview binding" 표준 패턴). 새 서브커맨드 `--help-keys`.
2. **스크롤 CPU 스파이크(5%→20%) → 프리뷰 입력 바이트 상한.** 원인은 선택이 바뀔 때마다 프리뷰가
   세션 JSONL 을 jq 로 재파싱(+bat)하는 비용. 실측: 일반 1.19MB 세션 0.15s(jq 0.10 + bat 0.05),
   17.7MB 파일 1.74s. 거대 파일이 스파이크의 주범. 입력은 **줄 수가 아니라 바이트**가 비용을 결정
   (17.7MB가 416줄뿐 — 거대 라인) → `head -c $PREVIEW_INPUT_BYTES`(기본 1MB, `CSESS_PREVIEW_BYTES`
   로 튜닝)로 jq 파싱을 묶는다. 결과: 17.7MB **1.74s→0.45s(콜드)/0.09s(웜)**, `=300000` 이면 0.06s.
   프리뷰는 글랜스라 앞부분이면 충분(head -c 가 마지막 라인을 잘라도 `fromjson?` 가 스킵). 캐시 없는
   매-실행 모델의 본질적 비용이라 완전 제거는 Phase B(SQLite/사전렌더)의 몫.
