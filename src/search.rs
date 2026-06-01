//! 본문 검색 — 공백=AND substring, 대소문자 무시 (Phase A [D13] 의미 계승).
//!
//! D13 은 fzf `--exact`(fuzzy 아님) + rg `-l -i -F` AND 였다: 각 term 으로 후보를 좁힌다.
//! Phase B 는 추출 텍스트(title+body)를 대상으로 같은 의미를 인프로세스로 — 키 입력마다
//! 미리 소문자화한 haystack 에 substring AND. (퍼지 모드는 후속에서 nucleo 토글로.)

use crate::index::SessionRow;

/// row 별 검색 haystack(소문자 title+body)을 1회 생성. 키 입력마다 재계산 회피.
pub fn build_haystacks(rows: &[SessionRow]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            let mut s = String::with_capacity(r.title.len() + r.body.len() + 1);
            s.push_str(&r.title.to_lowercase());
            s.push('\n');
            s.push_str(&r.body.to_lowercase());
            s
        })
        .collect()
}

/// 쿼리(공백 분리=AND)로 통과하는 row 인덱스. 입력 순서(mtime desc) 보존. 빈 쿼리=전체.
pub fn filter(haystacks: &[String], query: &str) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    (0..haystacks.len())
        .filter(|&i| terms.iter().all(|t| haystacks[i].contains(t.as_str())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, body: &str) -> SessionRow {
        SessionRow {
            source: "claude".into(),
            id: "x".into(),
            cwd: None,
            cwd_lossy: false,
            path: "p".into(),
            mtime: 0,
            title: title.into(),
            proj: "p".into(),
            body: body.into(),
        }
    }

    #[test]
    fn and_substring_case_insensitive() {
        let rows = vec![
            row("Auth JWT 질문", "토큰 검증 로직"),
            row("그냥 잡담", "auth 관련 없음"),
            row("배포 이슈", "JWT 토큰 만료"),
        ];
        let h = build_haystacks(&rows);
        // 빈 쿼리 → 전체
        assert_eq!(filter(&h, ""), vec![0, 1, 2]);
        // 단일어(대소문자 무시): title 또는 body 에 있으면
        assert_eq!(filter(&h, "jwt"), vec![0, 2]);
        // AND: 두 term 모두 (어느 필드든)
        assert_eq!(filter(&h, "auth jwt"), vec![0]); // row0 만 둘 다
        // 순서 보존
        assert_eq!(filter(&h, "토큰"), vec![0, 2]);
        // 없는 단어
        assert!(filter(&h, "존재하지않는단어").is_empty());
    }
}
