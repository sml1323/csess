//! SQLite 인덱스 — mtime/size 증분 무효화 캐시. [DESIGN Approach C → Phase B 흡수]
//!
//! Phase A 는 매 실행 fresh 빌드(캐시 없음)였다. Phase B 는 SQLite 로 흡수: 디스크를 열거해
//! (path, mtime, size)가 그대로면 재파싱을 건너뛰고(warm), 바뀐/새 파일만 파싱, 사라진 파일은
//! 삭제(유령 제거)한다. 단일 트랜잭션. TUI/검색(step 5)은 이 캐시의 `load_rows` 를 소비한다.

use crate::model::Source;
use crate::parser;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

/// 디스크 열거 후보 한 건.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: String,
    pub source: Source,
    pub mtime: i64,
    pub size: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshStats {
    /// 새로/변경되어 재파싱한 수.
    pub parsed: usize,
    /// (mtime,size) 동일 → 스킵(캐시 적중).
    pub cached: usize,
    /// 디스크에서 사라져 삭제된 수.
    pub deleted: usize,
    /// 갱신 후 전체 행 수.
    pub total: usize,
}

/// TUI/검색이 소비하는 한 행 (캐시에서 로드).
#[derive(Debug, Clone)]
#[allow(dead_code)] // 필드는 step 5(TUI)/step 6(resume)에서 읽힘
pub struct SessionRow {
    pub source: String,
    pub id: String,
    pub cwd: Option<String>,
    pub cwd_lossy: bool,
    pub path: String,
    pub mtime: i64,
    pub title: String,
    pub proj: String,
    pub body: String,
}

pub struct IndexStore {
    conn: Connection,
}

impl IndexStore {
    pub fn open(db_path: &str) -> rusqlite::Result<Self> {
        if let Some(parent) = Path::new(db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let mut store = IndexStore { conn };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut s = IndexStore { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&mut self) -> rusqlite::Result<()> {
        let v: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v < SCHEMA_VERSION {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS sessions (
                     path      TEXT PRIMARY KEY,
                     source    TEXT NOT NULL,
                     id        TEXT NOT NULL,
                     cwd       TEXT,
                     cwd_lossy INTEGER NOT NULL DEFAULT 0,
                     mtime     INTEGER NOT NULL,
                     size      INTEGER NOT NULL,
                     title     TEXT NOT NULL,
                     proj      TEXT NOT NULL,
                     body      TEXT NOT NULL DEFAULT ''
                 );
                 CREATE INDEX IF NOT EXISTS sessions_mtime ON sessions(mtime DESC);
                 CREATE INDEX IF NOT EXISTS sessions_cwd ON sessions(cwd);
                 CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )?;
            self.conn
                .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(())
    }

    /// 디스크 후보 목록으로 증분 갱신. (path,mtime,size) 가 그대로면 스킵.
    pub fn refresh(&mut self, candidates: &[Candidate]) -> rusqlite::Result<RefreshStats> {
        // 기존 (path -> mtime,size)
        let mut existing: HashMap<String, (i64, i64)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare("SELECT path, mtime, size FROM sessions")?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
            })?;
            for r in rows {
                let (p, ms) = r?;
                existing.insert(p, ms);
            }
        }
        let disk: HashSet<&str> = candidates.iter().map(|c| c.path.as_str()).collect();

        let mut stats = RefreshStats::default();
        let tx = self.conn.transaction()?;
        for c in candidates {
            match existing.get(&c.path) {
                Some(&(mt, sz)) if mt == c.mtime && sz == c.size as i64 => {
                    stats.cached += 1;
                }
                _ => {
                    if let Some(row) = parse_candidate(c) {
                        tx.execute(
                            "INSERT INTO sessions(path,source,id,cwd,cwd_lossy,mtime,size,title,proj,body)
                             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                             ON CONFLICT(path) DO UPDATE SET
                               source=?2,id=?3,cwd=?4,cwd_lossy=?5,mtime=?6,size=?7,title=?8,proj=?9,body=?10",
                            params![
                                row.path, row.source, row.id, row.cwd, row.cwd_lossy as i64,
                                row.mtime, c.size as i64, row.title, row.proj, row.body
                            ],
                        )?;
                        stats.parsed += 1;
                    }
                }
            }
        }
        // 유령 삭제: 캐시엔 있는데 디스크에 없음
        for path in existing.keys() {
            if !disk.contains(path.as_str()) {
                tx.execute("DELETE FROM sessions WHERE path=?1", params![path])?;
                stats.deleted += 1;
            }
        }
        tx.commit()?;
        stats.total = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
        Ok(stats)
    }

    /// mtime 역순 전체 행 (TUI/검색 소스). step 5(TUI)에서 호출.
    #[allow(dead_code)]
    pub fn load_rows(&self) -> rusqlite::Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source,id,cwd,cwd_lossy,path,mtime,title,proj,body
             FROM sessions ORDER BY mtime DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SessionRow {
                source: r.get(0)?,
                id: r.get(1)?,
                cwd: r.get(2)?,
                cwd_lossy: r.get::<_, i64>(3)? != 0,
                path: r.get(4)?,
                mtime: r.get(5)?,
                title: r.get(6)?,
                proj: r.get(7)?,
                body: r.get(8)?,
            })
        })?;
        rows.collect()
    }
}

/// 소스별 파서 디스패치 → SessionRow. (Codex 는 step 4)
fn parse_candidate(c: &Candidate) -> Option<SessionRow> {
    match c.source {
        Source::Claude => parser::claude::parse_claude_file(&c.path, true).map(|r| SessionRow {
            source: r.source.as_str().to_string(),
            id: r.id,
            cwd: r.cwd_real.clone(),
            cwd_lossy: r.cwd_real.is_none(),
            path: r.path,
            mtime: r.mtime,
            title: r.title,
            proj: r.proj,
            body: r.body,
        }),
        Source::Codex => parser::codex::parse_codex_file(&c.path, true).map(|r| SessionRow {
            source: r.source.as_str().to_string(),
            id: r.id,
            cwd: r.cwd_real.clone(),
            cwd_lossy: r.cwd_real.is_none(),
            path: r.path,
            mtime: r.mtime,
            title: r.title,
            proj: r.proj,
            body: r.body,
        }),
    }
}

/// Claude 루트의 depth-2 JSONL 을 (path,source,mtime,size) 후보로 열거. [D2]
/// `read_dir(root)`(depth-1) → `read_dir(dir)`(depth-2) = `-mindepth 2 -maxdepth 2`.
pub fn enumerate_claude(root: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let dirs = match std::fs::read_dir(root) {
        Ok(d) => d,
        Err(_) => return out,
    };
    for d in dirs.flatten() {
        let dp = d.path();
        if !dp.is_dir() {
            continue;
        }
        let files = match std::fs::read_dir(&dp) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for f in files.flatten() {
            let fp = f.path();
            if !fp.is_file() || fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = match fp.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if let Some(s) = fp.to_str() {
                out.push(Candidate {
                    path: s.to_string(),
                    source: Source::Claude,
                    mtime,
                    size: meta.len(),
                });
            }
        }
    }
    out
}

/// Codex 루트(`~/.codex/sessions`)를 재귀 walk 하며 `rollout-*.jsonl` 을 후보로 열거.
/// (YYYY/MM/DD 중첩이라 depth 고정 불가 → 스택 기반 재귀.)
pub fn enumerate_codex(root: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let meta = match p.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if let Some(s) = p.to_str() {
                out.push(Candidate {
                    path: s.to_string(),
                    source: Source::Codex,
                    mtime,
                    size: meta.len(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_load_roundtrip() {
        // tests/fixtures 를 루트로 보면 fixtures/claude/*.jsonl 이 depth-2.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let cands = enumerate_claude(root);
        assert!(cands.len() >= 14, "fixtures enumerated: {}", cands.len());

        let mut store = IndexStore::open_in_memory().unwrap();

        // cold: 전부 파싱
        let s1 = store.refresh(&cands).unwrap();
        assert_eq!(s1.parsed, cands.len());
        assert_eq!(s1.cached, 0);
        assert_eq!(s1.total, cands.len());

        // warm: 변경 없음 → 전부 캐시 적중
        let s2 = store.refresh(&cands).unwrap();
        assert_eq!(s2.parsed, 0);
        assert_eq!(s2.cached, cands.len());
        assert_eq!(s2.deleted, 0);

        // load: mtime 역순 정렬 + 알려진 제목/폴백 존재
        let rows = store.load_rows().unwrap();
        assert_eq!(rows.len(), cands.len());
        assert!(rows.windows(2).all(|w| w[0].mtime >= w[1].mtime), "mtime desc");
        assert!(rows.iter().any(|r| r.title == "안녕하세요 이거 어떻게 해요?"));
        assert!(rows.iter().any(|r| r.title == "(no prompt)"));
        // 09-cwd-missing → cwd 불명 → cwd_lossy
        assert!(rows.iter().any(|r| r.cwd_lossy && r.cwd.is_none()));
    }
}
