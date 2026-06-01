# csess — Claude/Codex 세션 브라우저

흩어진 Claude Code / Codex 대화 세션을 **내용으로 퍼지 검색**해서 한 화면에 띄우고,
엔터 한 번에 **올바른 디렉토리에서 바로 resume** 하는 TUI.

`claude --resume`은 현재 디렉토리 세션만, 검색 없이 보여준다. csess는 크로스 프로젝트 +
내용 검색을 채운다. (이 머신 기준 442개 세션 / 44개 프로젝트)

## 핵심

- **내용으로 찾기** — 파일명이 아니라 대화 안의 말로. fzf 퍼지 매칭.
- **크로스 프로젝트** — 흩어진 세션을 한 화면에.
- **엔터 → 점프** — `chdir` 후 `claude --resume` exec. 복사·cd 수동 작업 0.

## 로드맵 (A → B)

- **A (셸, 주말):** `bash + fzf + jq`. Claude만. mtime 캐시 필수.
- **B (Rust 학습 본진):** `ratatui` + `nucleo`. Codex 합류. SQLite 인덱스 → 타임라인/분석 확장.

전체 설계: [`docs/DESIGN.md`](docs/DESIGN.md)

## 상태

설계 승인됨 (2026-06-01). 구현 미착수.
