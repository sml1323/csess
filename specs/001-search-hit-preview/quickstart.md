# Quickstart: 검색 매치 지점에 앵커되는 프리뷰

## 빌드

```bash
cargo build --release   # 또는 개발 중 cargo run
```

## 수동 검증 (구현 후)

TUI를 띄우고 실제 코퍼스로 확인한다.

```bash
cargo run --release
# 또는 설치본: csess
```

### US1 — 첫 매치 자동 앵커 (P1)
1. 대화 **후반부에만** 등장하는 단어(예: 특정 함수명/오류 메시지)를 입력해 검색한다.
2. 매치된 세션을 선택한다.
3. **기대**: 프리뷰가 맨 위가 아니라 **그 단어가 보이는 구간**에서 열린다(위로 ~2줄 맥락). 수동 스크롤 불필요.
4. Ctrl+D/U로 스크롤 → 그 뒤로는 재앵커 안 됨(수동 우선). ↑/↓로 다른 세션 이동 → 그 세션의 첫 매치로 다시 앵커.
5. 검색어를 바꾸면 같은 세션에서도 새 매치로 재앵커.

### US2 — 매치 하이라이트 (P2)
- 화면에 보이는 검색어가 주변 텍스트와 구분되게 **강조**되는지. `JWT`로 검색해 본문 `jwt`가 강조되면 대소문자 무시 OK. 공백 다중어(`토큰 검증`)는 둘 다 강조.

### US3 — 매치 순회 (P3)
- 같은 단어가 여러 곳인 세션에서 `Alt+n`(다음)/`Alt+p`(이전)로 매치 간 이동.

### 엣지(오류 없이 폴백되는지)
- **제목만 매치**되는 검색어 → 프리뷰 top에서 시작(오탐 앵커 없음).
- 매치가 **접힌 tool/thinking·거대 세션 상한 밖**에만 있음 → top 폴백, 크래시 없음.
- **빈 검색어** → 기존 동작(top, 하이라이트 off).

## 자동 테스트

```bash
cargo test                    # 전체
cargo test -p csess preview   # 매치/하이라이트 순수 함수
cargo test anchor_scroll match_lines highlight_lines terms
```

계약 테스트 목록은 [`contracts/internal.md`](./contracts/internal.md) 참조. 순수 함수(`match_lines`/`anchor_scroll`/`highlight_lines`/`terms`)는 파일 I/O 없이 인라인 `Line` 픽스처로 검증한다.

## 회귀 체크(성능 게이트)
- 긴 세션에서 Ctrl+D/U 연타 시 프리뷰가 **idle**(재파싱/재하이라이트 없이) 스크롤되는지 — (path,query) 하이라이트 캐시가 히트해야 함(D6).
