# TODOS — csess

2026-06-01 설계 리뷰에서 의도적으로 보류한 항목. Phase A는 `bin/csess`로 구현·검증 완료.

## Phase A.5 (dogfooding 1주 후)

### Codex 세션 합류
- **What:** `~/.codex/sessions/**/rollout-*.jsonl` 파싱 + `codex resume <uuid>` 런처 추가.
- **Why:** Codex 세션이 411개 — Claude(255)보다 많다. Claude만이면 전체의 **38%만** 보임.
  "그 질문 어느 세션에서 했더라"의 절반 이상이 Codex라 매번 "여기 없네 → Codex겠지"로 신뢰 깎임.
- **Pros:** 667개 전부 한 화면. 같은 아키텍처(find→xargs→fzf→cd+exec), 분기만 추가.
- **Cons:** 주말 스코프 살짝 확장. Codex의 "첫 사람 질문" 라인 모양 확인 필요(Claude와 다름).
- **Context:** 스키마 확인 완료 — `cwd`·`id`는 `session_meta` 라인 `.payload` 아래. `codex resume <uuid>`는 picker 건너뜀.
  파서를 `case "$file" in */.codex/*) parse_codex ;; *) parse_claude ;;` 소스-디스패치로 두면 순수 추가.
  남은 미지수: Codex user 메시지에서 첫 사람 질문 추출 규칙(~15분 probing). cwd 다수 디렉토리 확인됨(크로스 프로젝트 가치 있음).
- **Depends on:** 없음. (사용자가 일주일 dogfooding 후 합류 결정 — 2026-06-01 리뷰에서 "Claude-only, defer" 선택.)

### 순수-슬래시 세션 제목 개선
- **What:** 첫 사람 질문이 없는 세션(전부 `/command` 호출, 실측 19/259 ≈ 7%)은 `(no prompt)` 대신 `/command` 이름 표시.
- **Why:** `(no prompt)` 행이 똑같이 여러 개 — 어느 게 어느 슬래시였는지 구분 안 됨.
- **Pros:** 7% 행이 유용해짐. `<command-name>` 값 추출로 ~3줄.
- **Cons:** 파서에 분기 추가 (검증된 추출 로직 건드림). cosmetic.
- **Context:** denylist로 스킵한 `<command-name>/xxx</command-name>`에서 슬래시명을 살려 폴백 제목으로.

## 정리 (낮은 우선순위)
- **디코딩 폴백 제거:** cwd 없을 때의 lossy 디렉토리명 디코딩은 실측 0/259에서 트리거 안 됨(데드코드). 한글 경로를 뭉갬.
  `! -d` 가드가 이미 잘못된 resume을 막지만, 그냥 "(unknown cwd) → 거부"로 단순화해도 됨.
- **stream-never-slurp 전역화:** 인덱싱도(preview만이 아니라) 절대 `jq -Rrs` slurp 안 함을 코드/주석에 명시. 실측 slurp이 2x 느림 + RAM.

## Phase B (Rust)
- bats/golden-fixture 테스트 하니스 → Rust `ratatui` + `nucleo` 재작성 시 정식 테스트. **(구현됨: 골든 패리티 273/0-diff + 23 적대적 fixture + SQLite 증분 캐시 — step 1~3 완료)**
- SQLite 인덱스(타임라인/분석). 이때 mtime 캐시 의미 생김(~2000+ 세션). **(구현됨: rusqlite 증분 무효화, warm 0.00s)**
- 코퍼스 ~2000+ 세션 도달 시 fresh 빌드가 5초 넘김 → 인덱스 트리거.

### 제목 품질 정밀화 (Claude + Codex, "파리티 먼저, 개선은 후속" 배치 건)
실측(2026-06-01, step 2~4)에서 제목이 boilerplate인 케이스. **현재는 Phase A 파리티 유지 위해 미적용** — Claude·Codex 동시 적용 + 골든 베이스라인 갱신으로 나중에 일괄 처리.
- **Claude denylist 신규 패턴:** `[Request interrupted by user]`(16/60), `Base directory for this skill:`(스킬 주입), image-only `[Image #N]`.
- **Codex 주입 프리픽스:** **57/411 세션**의 첫 user_message 가 사용자 설정 가드(`IMPORTANT: Do NOT read or execute any files under ~/.claude/...`)로 시작하고 그 **뒤에 실제 질문**이 붙음 → 제목이 가드로 뜸. 파서는 spec(첫 user_message)을 정확히 추출하므로 버그 아님; 정밀화는 **알려진 주입 프리픽스 strip**(라인 스킵 아님 — 메시지 내부 프리픽스라 본문 검색엔 영향 없음). 사용자-환경 특수값이라 향후 설정 가능한 strip 패턴으로.
- **Codex 단문 제목:** 제목 `.`(20), 슬래시명 그대로(`code-review` 4 — Codex는 슬래시를 평문 저장) 등. 표시 보강 여지(낮은 우선순위).

### Phase B 착수 전 확인 (2026-06-01 prior-art 리서치)
- **prior art 재확인 필수:** `raine/claude-history`·`chronologos/cc-sessions`가 Phase B와 거의 동일(둘 다 Claude 전용).
  경쟁 도구가 빠르게 움직이므로 착수 직전 재확인 — 차별점은 **Codex 합류**뿐. B는 "도구 필요"가 아니라 **학습 + Codex**로만 정당화.
- **fork vs scratch 결정:** claude-history fork(Codex 파서만 추가) = 빠름 / 처음부터 = 학습 가치. 학습 동기 확인 후 택1.
- **television 채널 평가(선택):** TOML cable 채널로 거의 코드 없이 구현 가능한지 — 단 `mode='execute'`가
  cwd 가드(`cd "$cwd" && claude --resume`, 정확성 필수)를 통과시키는지 먼저 검증. 안 되면 폐기.
- **frecency 2차 정렬(보너스):** zoxide DB(`~/.local/share/zoxide`) 읽어 프로젝트 정렬 신호 추가 — cwd↔zoxide 키 매칭(심링크/worktree/대소문자) 검증 필요.
