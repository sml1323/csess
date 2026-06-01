//! resume — cd-guard 후 exec(프로세스 교체). [D7]
//!
//! `claude --resume <id>` 는 cwd-스코프라 `cd <cwd>` 는 안전이 아니라 **정확성** 필수.
//! Codex 는 `codex resume <uuid>`(cwd 가드는 일관성 + 올바른 작업 디렉토리 위해 동일 적용).
//! 거부 조건: id 빈값 · cwd 불명/lossy · cwd 가 디렉토리 아님. (Phase B 는 lossy cwd resume 거부.)
//!
//! TUI 가 터미널을 복원(raw mode 해제)한 **뒤** main 이 호출한다 — exec 는 현재 프로세스를
//! 교체하므로 그 터미널에 claude/codex 가 그대로 뜬다.

use crate::index::SessionRow;
use std::os::unix::process::CommandExt;
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub enum ResumeError {
    NoId,
    BadCwd(String),
}

/// 작은따옴표 셸 인용 (Phase A `csess_shquote`). bash/zsh 모두 안전, 멀티바이트 보존.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// resume CLI 프로그램 + 서브커맨드/플래그. (source 로 분기)
/// claude: `claude --resume <id>` · codex: `codex resume <uuid>`.
fn invocation(row: &SessionRow) -> (&'static str, &'static str) {
    match row.source.as_str() {
        "codex" => ("codex", "resume"),
        _ => ("claude", "--resume"),
    }
}

/// cd-guard: 통과 시 신뢰 가능한 cwd 반환. (id 빈값/cwd 불명·lossy/디렉토리 아님 → 거부)
pub fn guard(row: &SessionRow) -> Result<String, ResumeError> {
    if row.id.is_empty() {
        return Err(ResumeError::NoId);
    }
    match &row.cwd {
        Some(c) if !c.is_empty() && !row.cwd_lossy && std::path::Path::new(c).is_dir() => {
            Ok(c.clone())
        }
        other => Err(ResumeError::BadCwd(other.clone().unwrap_or_default())),
    }
}

/// 복사/표시용 셸 명령 문자열: `cd '<cwd>' && <prog> <flag> '<id>'`.
/// 플래그/서브커맨드는 평문, id 만 인용(Phase A `csess_resume` 와 동일).
pub fn command_string(row: &SessionRow, cwd: &str) -> String {
    let (prog, flag) = invocation(row);
    format!("cd {} && {} {} {}", shq(cwd), prog, flag, shq(&row.id))
}

/// pbcopy 로 클립보드 복사 (macOS). 실패 시 false.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(si) = child.stdin.as_mut() {
            let _ = si.write_all(text.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    } else {
        false
    }
}

/// 선택 세션 resume 실행. copy=true 면 명령만 클립보드로(exec 안 함).
/// CSESS_DRY_RUN 이면 명령을 stdout 으로 출력하고 반환. 성공 exec 는 반환하지 않음.
pub fn resume(row: &SessionRow, copy: bool) -> Result<(), ResumeError> {
    let cwd = guard(row)?;
    let cmd = command_string(row, &cwd);

    if copy {
        if copy_to_clipboard(&cmd) {
            eprintln!("csess: 클립보드에 복사됨: {cmd}");
        } else {
            eprintln!("csess: pbcopy 없음. 명령:\n{cmd}");
        }
        return Ok(());
    }
    if std::env::var("CSESS_DRY_RUN").is_ok() {
        println!("{cmd}");
        return Ok(());
    }

    std::env::set_current_dir(&cwd).map_err(|_| ResumeError::BadCwd(cwd.clone()))?;
    let (prog, flag) = invocation(row);
    // exec: 성공하면 현재 프로세스를 교체(반환 안 함). 반환되면 실패.
    let err = Command::new(prog).arg(flag).arg(&row.id).exec();
    eprintln!("csess: exec 실패 ({prog}): {err}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: &str, id: &str, cwd: Option<&str>, lossy: bool) -> SessionRow {
        SessionRow {
            source: source.into(),
            id: id.into(),
            cwd: cwd.map(String::from),
            cwd_lossy: lossy,
            path: "p".into(),
            mtime: 0,
            title: "t".into(),
            proj: "p".into(),
            body: String::new(),
        }
    }

    #[test]
    fn guard_rejects() {
        // id 빈값
        assert_eq!(guard(&row("claude", "", Some("/tmp"), false)), Err(ResumeError::NoId));
        // cwd 없음
        assert!(matches!(guard(&row("claude", "x", None, false)), Err(ResumeError::BadCwd(_))));
        // lossy cwd 거부
        assert!(matches!(guard(&row("claude", "x", Some("/tmp"), true)), Err(ResumeError::BadCwd(_))));
        // 존재하지 않는 디렉토리
        assert!(matches!(guard(&row("claude", "x", Some("/no/such/dir/xyz"), false)), Err(ResumeError::BadCwd(_))));
        // 정상 (/tmp 는 존재)
        assert_eq!(guard(&row("claude", "x", Some("/tmp"), false)), Ok("/tmp".to_string()));
    }

    #[test]
    fn command_string_per_source() {
        let c = row("claude", "abc-123", Some("/Users/me/work"), false);
        assert_eq!(command_string(&c, "/Users/me/work"), "cd '/Users/me/work' && claude --resume 'abc-123'");
        let x = row("codex", "019c-uuid", Some("/w/proj"), false);
        assert_eq!(command_string(&x, "/w/proj"), "cd '/w/proj' && codex resume '019c-uuid'");
    }

    #[test]
    fn shq_escapes_quotes() {
        assert_eq!(shq("a'b"), "'a'\\''b'");
        assert_eq!(shq("/Users/회사/test"), "'/Users/회사/test'");
    }
}
