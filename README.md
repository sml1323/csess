# csess

> Claude Code / Codex 세션을 **대화 본문으로 검색**하고 **엔터 한 번에 그 자리에서 resume** 하는 터미널 TUI.

![csess — 세션 본문검색 + 미리보기](docs/img/list-preview.png)

`claude --resume` 은 **현재 디렉토리의 세션만**, 그것도 내용 검색 없이 보여준다. `csess` 는
`~/.claude/projects` 와 `~/.codex/sessions` 에 흩어진 **모든 세션을 한 화면에** 모아 — 파일명이 아니라
**대화 내용**으로 찾고, 고르면 올바른 작업 디렉토리로 `cd` 한 뒤 바로 그 세션을 잇는다.

> _스크린샷은 데모 데이터이며 셸 프로토타입(Phase A) 기준입니다. Rust 버전(Phase B)은 소스별 듀얼톤 색(Claude 주황 / Codex 그린)을 씁니다._

## 왜

- **내용으로 찾는다** — 경로/파일명이 아니라 대화 안의 말로. _"그때 그 에러 어느 세션에서 물어봤더라?"_
- **Claude + Codex 한 곳에** — 686개(예시) 세션이 두 도구에 흩어져 있어도 한 목록에서. 소스는 색으로 구분.
- **크로스 프로젝트** — 수십 개 디렉토리에 흩어진 세션을 한 곳에서. `claude --resume` 이 못 하는 것.
- **엔터 한 번에 점프** — cd·복사·붙여넣기 없이 그 터미널에 바로 뜬다 (`exec claude/codex resume`).

## 기능

### 본문 전체 검색 (공백 = AND)
타이핑하면 추출된 대화 본문(제목+내용)을 **인프로세스 substring 검색**한다. 공백으로 나누면 AND —
`auth jwt` 처럼 2~3 단어면 정밀하게 좁혀진다.

### 소스별 듀얼톤 + 필터 — `ctrl-s`
Claude 는 **주황 `c`**, Codex 는 **그린 `ⓒ`** 마커로 한눈에 구분. 프롬프트·선택바 색도 호버한
세션의 소스를 따라간다. `ctrl-s` 로 **전체 → Claude만 → Codex만** 순환(프롬프트에 `claude>`/`codex>` 표시).

### 계층 디렉토리 트리 — `tab` (yazi식)
`tab` 으로 프로젝트를 트리처럼 탐색한다. **세션 cwd 로만 트리를 치고**(디스크 스캔 없음 — 모든 노드가
정의상 resume 가능), 단일 체인은 **경로 압축**(`specs/001-vendor-migration`), 부모 노드엔 **rollup 카운트**가 붙는다.
`enter` 로 드릴다운, `ctrl-h` 또는 `⬅ ..` 행으로 상위. 한 레벨에 하위 폴더와 그 자리 세션이 함께 뜬다.

![디렉토리 트리](docs/img/tree.png)

### 프로젝트 스코프 — `ctrl-g`
호버한 세션의 프로젝트(cwd)로 목록·검색을 한정 토글. 다시 누르면 해제. `ctrl-a` 로 전체(스코프·필터·트리) 리셋.

### 인프로세스 미리보기
선택한 세션의 대화를 **마크다운 스타일 + 역할 헤더**(`▌ 나` / `▌ Claude` 주황 / `▌ Codex` 그린)로
인프로세스 렌더한다. 툴 호출·thinking 블록은 접고 사람·모델의 글만. 디렉토리 노드를 호버하면 하위 폴더 +
최근 세션 요약. `ctrl-d`/`ctrl-u` 로 스크롤. (셸의 jq+bat 서브프로세스 비용이 없어 스크롤이 가볍다.)

## 요구사항 (Rust 버전 / Phase B)

| 도구 | 용도 | 필수 |
|------|------|:---:|
| [Rust](https://rustup.rs) (cargo) | 빌드 | ✅ |
| `claude` CLI | Claude 세션 `--resume` | ✅ (Claude 쓰면) |
| `codex` CLI | Codex 세션 `resume` | ✅ (Codex 쓰면) |
| `pbcopy` | `ctrl-y` 명령 복사 | 선택 (macOS) |

> 외부 런타임 의존(fzf·jq·bat·rg) 없이 **단일 정적 바이너리**. resume 의 cwd 가드 등은 **macOS** 기준으로 검증했다.

## 설치

```sh
git clone https://github.com/sml1323/csess.git && cd csess
cargo build --release
ln -s "$PWD/target/release/csess" ~/bin/csess   # ~/bin 이 PATH 에 있으면
```

> 셸 프로토타입(Phase A)도 `bin/csess` 에 그대로 있다 (`fzf`+`jq`+`bat`, Claude 전용). Rust 버전이 기본.

## 사용

```sh
csess              # TUI: 타이핑으로 본문 검색 → enter 로 resume (기본)
csess --refresh    # SQLite 캐시 증분 갱신 (parsed/cached/deleted 통계)
csess --index      # Claude 세션 전체를 TSV 로 (디버그/파리티)
csess -h           # 도움말
```

### 키

| 키 | 동작 |
|------|------|
| _(타이핑)_ | 본문 검색 (공백 = AND) · 트리에선 라벨/제목 필터 |
| `enter` | 세션 resume · 트리에선 디렉토리 드릴 / 세션 resume |
| `↑`/`↓` · `ctrl-p`/`ctrl-n` | 이동 |
| `tab` · `ctrl-o` | 계층 디렉토리 트리 진입 |
| `ctrl-h` | 트리에서 상위로 (목록의 `⬅ ..` 행과 동일) |
| `ctrl-g` | 호버 세션의 프로젝트로 스코프 토글 |
| `ctrl-s` | 소스 필터 순환 (전체 → Claude만 → Codex만) |
| `ctrl-a` | 전체로 리셋 (스코프·필터·트리 해제) |
| `ctrl-y` | resume 대신 `cd && claude/codex resume` 명령 복사 |
| `ctrl-d` / `ctrl-u` | 미리보기 ↓/↑ 스크롤 |
| `esc` / `ctrl-c` | 종료 |

### 환경변수

| 변수 | 설명 |
|------|------|
| `CSESS_CLAUDE_ROOT` | Claude projects 루트 (기본 `~/.claude/projects`) |
| `CSESS_CODEX_ROOT` | Codex sessions 루트 (기본 `~/.codex/sessions`) |
| `CSESS_DB` | SQLite 인덱스 경로 (기본 `~/.cache/csess/index.db`) |
| `CSESS_NOW` | 상대시각 now 고정 (테스트/파리티) |
| `CSESS_DRY_RUN` | resume 를 exec 하지 않고 명령만 출력 |

## 동작 방식 (간단히)

- **SQLite 증분 캐시.** 디스크를 열거해 `(path, mtime, size)` 가 그대로면 재파싱 스킵(warm 0.00s), 바뀐/새
  파일만 파싱, 사라진 파일은 삭제. 첫 빌드만 전체 파싱.
- **Claude 는 depth-2 만.** `~/.claude/projects/<cwd>/<id>.jsonl` — `*/subagents/*`(resume 불가 저널)를 자연 배제.
  Codex 는 `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` 재귀 열거.
- **cwd 는 JSONL 내용에서 읽는다.** 모든 라인을 스캔해 첫 유효 cwd 채택. 불명이면 표시만, **resume 은 거부**(안전).
- **트리는 세션 cwd 로만** 친다(디스크 스캔 X) — 경로압축 + rollup 은 인메모리 순수 함수.
- **resume = `cd "$cwd" && exec claude --resume <id>` / `codex resume <uuid>`.** `--resume` 은 cwd-스코프라
  `cd` 는 안전이 아니라 **정확성** 필수.

전체 설계와 결정 근거는 **[docs/DESIGN.md](docs/DESIGN.md)** 에 있다.

## 로드맵

- **Phase A — 완료.** 셸 + `fzf` + `jq` 프로토타입(`bin/csess`). Claude 전용. 데이터 스키마·미리보기 포맷·exec 로직을 굳혔다.
- **Phase B — 진행 중 (현재).** Rust(`ratatui`) 단일 바이너리. **인프로세스 렌더로 스크롤 CPU 제거** · SQLite 인덱스 ·
  **Codex 합류** · 계층 트리/스코프/소스 필터. 데이터 레이어는 Phase A 출력과 골든 패리티로 검증.
- **다음.** syntect 코드 구문 하이라이트 · 제목 품질 정밀화.

## 한계

- 개인용 빌더 도구. resume 의 cwd 가드·복사는 **macOS** 기준으로 검증.
- substring 검색이라 흔한 단어는 노이즈가 많다 — 다중어로 좁히면 정밀.
- 미리보기 코드 구문 하이라이트(syntect)는 아직 라인 기반 경량 스타일.
