# CLAUDE.md — csess

## 이 프로젝트가 뭔가

`csess` — Claude Code / Codex 대화 세션을 fzf 퍼지 검색으로 띄우고 엔터로 바로 resume 하는 TUI.
**전체 설계와 결정 근거는 [`docs/DESIGN.md`](docs/DESIGN.md)에 있다. 작업 전 반드시 읽을 것.**

빌더 모드 개인 도구. "일단 쓸 수 있는 것" 우선. 검색 엔진은 직접 안 짜고 fzf에 위임.

## 구현 순서 (A → B)

1. **A — 셸 프로토타입 (구현됨: `bin/csess`):** 한 스크립트. `bash/zsh + fzf + jq`. 모든 결정은 2026-06-01 실측 리뷰로 확정.
   - Claude만. 열거: **`find ~/.claude/projects -mindepth 2 -maxdepth 2 -name '*.jsonl'`** — `**` 재귀 글롭 금지.
     depth-2가 곧 서브에이전트 필터다 (depth-3 `<session>/subagents/*`는 resume 불가 저널, 실측 ~257개 = 코퍼스 47%). Codex는 B에서.
   - **캐시 없음.** 실측: 순진한 파일당 jq ~3.0s, 병렬 파일당(`xargs -0 -P8`) ~0.4–2.0s → 1초 목표 안. (`mtime 캐시`는 ~2000+ 세션의 최적화로 보류, Phase B SQLite가 흡수.)
   - 파싱은 **파일당 line-tolerant** `jq -R 'fromjson? // empty'`. 단일 `jq -n inputs` 패스 금지 — 코퍼스에 invalid surrogate pair 파일 1개(17.7MB)가 있어 strict 패스는 거기서 abort → 뒤 세션 silent 누락.
   - cwd는 디렉토리명 디코딩 말고 **JSONL 내용에서** 읽는다. **모든 라인 타입** 스캔해 첫 non-null `.cwd` 채택 (user/assistant 라인엔 없을 때 많음). 디코딩은 lossy → cwd 비면 표시만, resume은 **거부**.
   - 첫 질문(제목): `<command-name>`·`<local-command-caveat>` 등 슬래시-커맨드/시스템 래퍼 user 라인은 스킵하고 첫 **실제 사람 질문**을 (안 그러면 ~51%가 boilerplate 제목). TSV 방출 전 `gsub("[\t\n\r]+";" ")` 정제 후 80자 컷 (첫 질문 ~50–60%가 멀티라인 → 정제 안 하면 한 세션이 fzf 여러 줄로 쪼개져 빈 id → resume 깨짐). 숨김 필드(mtime/id/cwd/path)는 제목 **앞**에.
   - 선택 후: **`cd "$cwd" || exit 1` 가드가 핵심 안전장치** 후 `exec claude --resume "$id"`. `claude --resume <id>`는 **cwd-스코프**(맞는 프로젝트 디렉토리에서 호출해야 세션을 찾음). 자식은 부모 셸 cwd를 못 바꾸므로 "stranded" 걱정 없음 = 원래 step5 근거는 틀림.
   - 테스트 시임: `csess --index-file <path>`(순수 파서, fzf/exec 없음), `CSESS_CLAUDE_ROOT`·`CSESS_DRY_RUN` 환경변수.
2. **B — Rust TUI:** A로 스펙 굳힌 뒤. `ratatui` + `nucleo`. SQLite 인덱스 흡수. Codex 파서·`codex resume` 합류(포맷 확인 완료).

## 데이터 스키마 (실측)

- **Claude:** `~/.claude/projects/<encoded-cwd>/<id>.jsonl` (**depth-2만** — depth-3 `*/subagents/*`는 resume 불가 저널, 제외).
  `cwd`·`sessionId`가 이벤트 라인 최상위지만 **첫 유효 cwd의 라인 타입은 고정 아님** (last-prompt/permission-mode/file-history-snapshot 메타 라인엔 없음) → 모든 라인 스캔해 첫 non-null `.cwd`.
  실측(2026-06-01): resume 가능 세션 ~255–259개 / 45개 프로젝트 (전체 ~496 JSONL 중 ~241개가 서브에이전트 저널).
- **Codex:** `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`. `cwd`·`id`가 `session_meta`
  라인의 `.payload` 아래. resume은 `codex resume <uuid>` (명시 UUID면 picker 건너뜀).

## Skill routing

요청이 스킬과 맞으면 Skill 도구로 먼저 호출한다.
- 버그/에러/"왜 안 되지" → `/investigate`
- ship/PR/배포 → `/ship`
- QA/테스트 → `/qa`
- 코드 리뷰/diff 확인 → `/review`
- 아키텍처 리뷰 → `/plan-eng-review`
