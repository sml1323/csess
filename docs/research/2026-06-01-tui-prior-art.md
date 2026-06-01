# 리서치: 세션/디렉토리 브라우저 TUI — prior art & UX 패턴

생성: 2026-06-01 (`/deep-research`, 멀티에이전트 web research)
범위: 21개 소스 fetch → 102개 주장 추출 → 25개 적대적 검증(2/3 반박이면 폐기) → 23 confirmed / 2 killed
목적: csess 다음 설계 결정(자체 구현 vs 기존 도구, fzf-A vs ratatui-B, 네비게이션 모델)에 답하기.

> 이 리포트는 [`DESIGN.md`](../DESIGN.md)의 결정을 **검증·강화하는 외부 근거**다. 재설계 제안이 아니라,
> 이미 잠긴 결정이 외부 prior art와 어떻게 맞물리는지를 기록한다. Phase A 개선(D12)의 근거.

---

## TL;DR

1. **바퀴는 부분적으로 이미 발명됐다.** csess Phase B(크로스 프로젝트 Claude 세션 퍼지 검색 + resume Rust TUI)와
   거의 동일한 도구가 최소 2개 성숙한 상태로 존재 — `raine/claude-history`, `chronologos/cc-sessions`.
   둘 다 **Claude 전용** → csess의 **Codex 합류가 유일하게 빈 차별점.**
2. **Phase B의 명분이 바뀐다.** "도구가 없어서 짓는다"는 죽었다. B는 오직 **(a) Codex 합류 + (b) Rust/TUI 학습**으로만 정당화.
3. **fzf로 거의 다 된다.** yazi식 디렉토리→세션 드릴다운 + 프리뷰 패널조차 순수 bash+fzf로 구현 가능(공식 문서 확인).
   ratatui로 넘어가야만 얻는 것: 상태 영속·다중 패널 동시 표시·복잡한 분기.
4. **디렉토리는 네비게이션 단계가 아니라 표시 컬럼이어야 한다.** csess 페인("그때 그 세션")엔 flat 퍼지가 맞다.
   yazi식 2단계 드릴다운은 마찰만 추가. csess의 flat 선택은 옳았다.
5. **정렬은 mtime이 표준**(세션 리스트). frecency(zoxide)는 디렉토리 *재방문* 신호라 Phase B SQLite의 보너스.

---

## Q1 — 바퀴 재발명 체크 (confidence: high, 3-0)

| 도구 | 스택 | 상태(2026-06-01) | csess와 겹침 |
|------|------|------------------|--------------|
| **raine/claude-history** | 99% Rust + ratatui 0.30 + crossterm 0.29 (fzf/skim 의존 0) | 302★, v0.1.64 (2026-05-30) 활발 | `~/.claude/projects/*.jsonl` 퍼지 검색 → `claude --resume <id>`. **Ctrl+R resume, Ctrl+F fork, cross-project fork** 이미 구현. = csess Phase B가 제안한 정확한 스택·기능 |
| **chronologos/cc-sessions** | Rust TUI (98.6% Rust) | v1.8.1 (2026-04-20) | "list and resume Claude Code sessions across all projects". `WalkDir min/max_depth(2)`로 서브에이전트 제외(=csess depth-2), JSONL 라인에서 `entry.get('cwd')` 첫 non-empty(=csess "cwd는 내용에서, 디코딩은 폴백") — **DESIGN 결정과 글자 그대로 동일** |
| 추가 발견 | — | — | ccrider, tradchenko/claude-sessions, davidpp/claude-session-browser, borball/claude-session-manager-tui |

- **빈 칸:** 위 도구 전부 Claude 전용. csess의 Codex 파서 + `codex resume` 합류는 **prior art 없음**.
- **소스:** github.com/chronologos/cc-sessions (+/blob/main/src/claude_code.rs), github.com/raine/claude-history.

### "그냥 기존 도구 쓰면 되는가" (high, 3-0)
- 순수히 **쓸 도구**가 목표면 → `raine/claude-history` 설치가 합리적(resume·fork·cross-project 다 됨).
- 그러나 csess 목표는 (a) Codex 합류 + (b) Rust/TUI 학습 본진(DESIGN.md L60-63: "학습 목적이라 일부러 손으로").
- **권고:** Phase A는 그대로(셸 도구 = 즉각 매일 쓰는 가치 + B 스펙 굳히기). Phase B는 "경쟁 도구를 이긴다"가 아니라
  "내가 짠다(학습)"로 프레이밍하되 **Codex 합류를 차별화 1순위**로. 학습 동기가 약해지면 claude-history fork가 빠른 길.

### 제3 선택지: television 커스텀 채널 (high, 3-0)
- television(`tv`)은 csess Phase B가 제안한 **ratatui 0.30 + nucleo 스택의 작동하는 프로덕션 레퍼런스**(63k crates.io DL).
- TOML cable 채널: `[source]`(임의 셸 = csess의 `find|jq`), `[preview]`(`{}` placeholder), `[actions]`(`mode='execute'`로 앱 대체).
- → **거의 코드 없이** csess 구현 가능. 단 **미검증**: execute 모드가 csess 핵심 안전장치 `cd "$cwd" && claude --resume`(cwd-스코프 가드)를 통과시키는가?
- **소스:** github.com/alexpasmantier/television, alexpasmantier.github.io/television/user-guide/channels/.

## Q2 — fzf vs ratatui (high, 3-0)

- **두 진영:** claude-history·cc-sessions·television = 자체 Rust TUI(fzf 미사용) / sesh = fzf 위임.
- **fzf 한계:** "stateless selector" — 네이티브 세션/상태 API 없음. 모드 전환 시 모드별 쿼리 복원은 `/tmp` 파일 I/O 필요
  (공식 ADVANCED.md: `/tmp/rg-fzf-{r,f}` + `transform-query`). `$FZF_PROMPT`은 모드(바이너리)만, 쿼리 텍스트는 저장 못 함.
  → **단 csess Phase A는 flat 단일 검색이라 이 한계에 아직 안 닿음.** 모드별 쿼리 복원 footgun은 yazi식 다중 모드로 갈 때만 발생.
- **fzf 능력(yazi식 드릴다운+프리뷰는 순수 셸로 가능, high 3-0):**
  - `reload`(런타임 후보 리스트 동적 교체, v0.19.0+), `change-prompt`, `transform`+`$FZF_PROMPT`(모드 인코딩/디스패치).
  - 공식 드릴다운 예: `fzf --bind 'ctrl-d:change-prompt(Dirs> )+reload(find * -type d)'`.
  - `--preview-window`: position/size/border/scroll/`~HEADER_LINES` 선언적 (예 `up,60%,border-bottom,+{2}+3/3,~3`), `{1}{2}` 필드 placeholder.
  - **ratatui로만 얻는 것:** 상태 영속 · 다중 패널 동시 표시 · 복잡한 분기.
- **footgun (high, 3-0):** fzf는 `{}` placeholder를 셸 실행 *전에* 평가. `xargs -I {}`처럼 `{}` 쓰는 도구를 임베드하면 깨짐.
  → `xargs -I {x}` 또는 `\{}` escape. (메인테이너 junegunn issue #4246.)
- **소스:** github.com/junegunn/fzf/blob/master/ADVANCED.md, issues/4246, github.com/joshmedeski/sesh.

## Q3 — 네비게이션 모델 (high, 3-0)

| 모델 | 도구 | csess 적합도 |
|------|------|-------------|
| 계층 드릴다운(enter=자식/leave=부모) + 8 모달 레이어 | **yazi** | ✗ 마찰. 디렉토리 거치면 "한 화면에서 내용으로 찾기"가 느려짐 |
| **flat 퍼지 + 프리뷰** (드릴다운 없음, 프로젝트 자동 발견) | **claude-history** | ✓ csess 페인에 직접 맞음 |
| fzf reload 드릴다운 | (fzf 예제) | △ 가능하나 불필요 |

- claude-history: "all conversations, sorted by recency. Type to search across all transcripts", "single-panel list, **No directory drill-down**".
- **결론:** csess는 45 디렉토리 × 263 세션을 "한 화면에"(DESIGN L25) 원하므로 디렉토리는 **표시 컬럼**이지 네비게이션 단계가 아님.
  yazi식 드릴다운은 prior art로만 인용, 권고 아님. csess flat 선택 = 옳음.
- **소스:** yazi-rs.github.io/docs/configuration/keymap/, github.com/raine/claude-history.

## Q4 — 레이아웃/키바인딩 컨벤션 (high, 3-0)

- **듀얼 네비:** emacs(기본)+vim 양쪽이 관례. atuin 5 keymap(emacs/vim-normal/vim-insert/inspector/prefix).
  claude-history도 `↑↓` + `Ctrl+P/N`(vi-style) 둘 다. → fzf 기본 `ctrl-n/p`가 vi-ish 이동 공짜 제공.
- **프리뷰 배치:** fzf `--preview-window 'right:50%'` 또는 `'up,60%'` 선언적 = 표준.
- **선택 결과 듀얼 액션:** atuin은 **accept**(즉시 실행, enter 기본) vs **return-selection**(실행 없이 명령줄에 배치, tab 기본)을 구분.
  → csess 매핑: `enter`=즉시 resume(exec), `tab`=`cd && resume` 명령 복사(실행 안 함). 부모 셸 cwd 바꾸고 싶을 때의 폴백.
- **소스:** blog.atuin.sh/custom-keybindings-for-the-atuin-tui/, github.com/raine/claude-history.

## Q5 — 정렬/순서 (high, 3-0)

- **세션 리스트:** mtime(최근순)이 표준 — claude-history "sorted by recency". csess의 mtime 결정(DESIGN L83) = 합리적 기본값.
- **디렉토리/프로젝트 발견:** frecency(zoxide)가 대표 — sesh가 zoxide로 프로젝트 발견 + `sort_order`로 타입 precedence.
- **csess 적용:** frecency는 디렉토리 *재방문* 신호에서 빛나는데 csess 페인은 "내용으로 옛 세션 찾기"라 최근성+퍼지가 더 맞다.
  Phase B SQLite에서 zoxide DB(`~/.local/share/zoxide`)를 2차 정렬 신호로 옵션 추가 가능(보너스).
- **소스:** github.com/joshmedeski/sesh, github.com/ajeetdsouza/zoxide/wiki/Algorithm.

---

## 폐기된 주장 (검증으로 반박됨)

- ❌ "cc-sessions의 풍부한 기능이 ratatui가 fzf 대비 무엇을 사주는지 입증한다" → **0-3 기각.** (fzf로도 대부분 가능.)
- ❌ "sesh는 자체 TUI 없이 외부 picker(fzf/tv/gum)에만 위임한다" → **0-3 기각.** (sesh는 fzf preview를 직접 ship.)

## Caveats

- **시간 민감성:** 경쟁 도구가 매우 빠르게 움직임(claude-history 이틀 전 릴리스, cc-sessions 6주 전, tv ~18일 전).
  "mature"는 기능·릴리스 케이던스 기준이며 인기는 아직 초기(claude-history 302★, cc-sessions 25★). **Phase B 착수 직전 재확인.**
- **소스 품질:** 핵심 주장 거의 전부 1차 소스(GitHub 레포·공식 docs·Cargo.toml·메인테이너 발언). fzf 드릴다운은 issue #4246(미해결 버그 스레드)이 아니라 **0.45.0 릴리스 노트/man page**를 권위 소스로.
- **television-nucleo는 저자 포크**(업스트림 helix-editor/nucleo 아님)이며 tokio async 사용 — csess B가 tokio 채택할지는 미정.

## Open Questions (Phase B 전 해소)

1. **Codex "첫 사람 질문" 라인 모양** — Claude와 다름(DESIGN L79). claude-history/cc-sessions 둘 다 Codex 미지원 → prior art 없음. 합류 시 ~15분 probing 필요.
2. **television execute 모드 + cwd 가드** — csess의 cwd-스코프 resume 정확성을 채널로 맞출 수 있는지 검증 필요.
3. **fork vs scratch** — claude-history fork(Codex 파서만 추가) vs 처음부터(학습). 시간 절약 vs 학습 가치 = 사용자 결정.
4. **zoxide DB 키 매칭** — csess의 cwd(JSONL에서 읽음)와 zoxide 추적 디렉토리가 심링크/worktree/대소문자에서 일치하는지.
