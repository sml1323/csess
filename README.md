# csess

Claude Code / Codex 세션을 **대화 내용으로 검색**하고 엔터 한 번에 그 자리에서 이어가는 터미널 TUI. Rust 단일 바이너리.

![csess — 본문 검색 + 미리보기](docs/img/list-preview.png)

`claude --resume` 은 현재 디렉토리의 세션만, 그것도 내용 검색 없이 보여준다. `csess` 는 `~/.claude/projects` 와 `~/.codex/sessions` 에 흩어진 **모든 세션을 한 화면에** 모아 — 파일명이 아니라 **대화 안의 말**로 찾고, 고르면 올바른 작업 디렉토리로 `cd` 한 뒤 그 세션을 resume 한다. Claude 는 주황 `c`, Codex 는 그린 `ⓒ` 로 구분.

## 설치

```sh
git clone https://github.com/sml1323/csess.git && cd csess
cargo build --release
ln -s "$PWD/target/release/csess" ~/bin/csess   # ~/bin 이 PATH 에 있으면
```

resume 하려면 해당 `claude` / `codex` CLI 가 있어야 한다. macOS 기준으로 검증했다.

## 사용

```sh
csess              # TUI (기본): 타이핑으로 본문 검색 → enter 로 resume
csess --refresh    # SQLite 캐시 증분 갱신
csess --index      # Claude 세션 전체를 TSV 로 (디버그)
csess -h           # 도움말
```

타이핑하면 추출된 대화 본문(제목+내용)을 substring 검색한다 — 공백으로 나누면 AND(`auth jwt`). `tab` 으로 디렉토리 트리, `ctrl-s` 로 소스 필터, `ctrl-g` 로 프로젝트 스코프.

| 본문 검색 (공백 = AND) | 계층 트리 (`tab`) | 소스 필터 (`ctrl-s`) |
|:---:|:---:|:---:|
| ![검색](docs/img/search.png) | ![트리](docs/img/tree.png) | ![소스 필터](docs/img/source-filter.png) |

### 키

| 키 | 동작 |
|------|------|
| _(타이핑)_ | 본문 검색 (공백 = AND) · 트리에선 라벨/제목 필터 |
| `enter` | 세션 resume · 트리에선 디렉토리 드릴 / 세션 resume |
| `↑`/`↓` · `ctrl-p`/`ctrl-n` | 이동 |
| `tab` · `ctrl-o` | 계층 디렉토리 트리 진입 |
| `ctrl-h` | 트리에서 상위로 |
| `ctrl-g` | 호버 세션의 프로젝트로 스코프 토글 |
| `ctrl-s` | 소스 필터 순환 (전체 → Claude → Codex) |
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
| `CSESS_DRY_RUN` | resume 를 exec 하지 않고 명령만 출력 |

## 동작 방식

- **SQLite 증분 캐시** — `(path, mtime, size)` 가 그대로면 재파싱 스킵, 바뀐/새 파일만 파싱, 사라진 파일은 삭제.
- **인프로세스 렌더** — 미리보기·검색을 외부 프로세스 없이 처리해 스크롤이 가볍다. 툴 호출·thinking 은 접고 사람·모델의 글만.
- **cwd 는 JSONL 내용에서** 읽는다 — 디렉토리명 디코딩은 lossy. cwd 불명이면 표시만 하고 **resume 은 거부**.
- **resume = `cd "$cwd" && exec claude --resume <id>`** (Codex 는 `codex resume <uuid>`). `--resume` 은 cwd-스코프라 `cd` 는 정확성에 필수.

전체 설계와 결정 근거는 **[docs/DESIGN.md](docs/DESIGN.md)** 에 있다. 스크린샷은 `scripts/gen-demo.py` + `scripts/demo.tape`([VHS](https://github.com/charmbracelet/vhs))로 재현 가능한 데모 데이터에서 생성한다. (초기 셸 프로토타입은 `bin/csess` 에 남아 있다.)
