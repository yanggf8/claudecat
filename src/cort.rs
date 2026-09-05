//! cortexyoung/cort 索引整合：唯讀存取 cort 的 SQLite（~/.cache/cortex-ng/<sha256>.db）
//! 相容 cort schema v4（chunks / projects / relationships）。
//! 不做任何寫入；DB 不存在或 schema 不符時回退到 None。
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 與 cort 完全一致：project_id = sha256(real_path) hex
pub fn project_id(real_path: &str) -> String {
    let mut h = Sha256::new();
    h.update(real_path.as_bytes());
    let out = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in out.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

pub fn cache_dir() -> PathBuf {
    match std::env::var("CORT_CACHE_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cache").join("cortex-ng"))
            .unwrap_or_else(|| PathBuf::from(".cortex-ng")),
    }
}

pub fn db_path_for(real_path: &str) -> PathBuf {
    cache_dir().join(format!("{}.db", project_id(real_path)))
}

#[derive(Debug, Clone, Serialize)]
pub struct CortIndexInfo {
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub git_head: Option<String>,
    pub last_indexed_at: Option<i64>,
    pub extractor_version: String,
    pub chunk_count: i64,
    pub relationships_count: i64,
    pub fresh: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CortHit {
    pub symbol: Option<String>,
    pub chunk_type: String,
    pub file: String,
    pub start_line: i64,
    pub end_line: i64,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CortDependent {
    pub source_file: String,
    pub source_symbol: Option<String>,
    pub source_start_line: i64,
    pub rel_type: String,
    pub call_site_line: Option<i64>,
    pub confidence_score: f64,
}

/// 對 cort DB 跑唯讀查詢：先試一般唯讀（sidecar 齊全時最準）；若開檔失敗
/// （唯讀檔案系統缺 -shm/-wal、sandbox 擋 lock 等）或查詢時 BUSY（cort 正持有寫鎖），
/// 自動退回 `immutable=1`（SQLite 完全不碰 sidecar/lock，直接讀主檔；代價是 cort 若有
/// 未 checkpoint 的 WAL 內容會讀不到——這是可接受的誠實取捨）。claudecat 全程不寫入。
fn with_readonly<T>(real_path: &str, f: impl Fn(&Connection) -> rusqlite::Result<T>) -> Option<T> {
    let db = db_path_for(real_path);
    if !db.is_file() {
        return None;
    }
    let uri = format!("file:{}?immutable=1", db.display());
    let candidates = [
        Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY),
        Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        ),
    ];
    for conn in candidates.into_iter().flatten() {
        if let Ok(v) = f(&conn) {
            return Some(v);
        }
    }
    None
}

/// 取得 cort 索引狀態（DB 不存在或 projects 表無此專案 → None）
pub fn index_info(root: &Path) -> Option<CortIndexInfo> {
    let real = std::fs::canonicalize(root).ok()?;
    let real_str = real.to_str()?;
    let pid = project_id(real_str);

    let row = with_readonly(real_str, |conn| {
        conn.query_row(
            "SELECT name, path, git_head, last_indexed_at, extractor_version \
             FROM projects WHERE project_id = ?1",
            [&pid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
    })??;

    let (chunk_count, relationships_count) = with_readonly(real_str, |conn| {
        let chunks: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE project_id = ?1",
            [&pid],
            |r| r.get(0),
        )?;
        let rels: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks c JOIN relationships r \
             ON r.source_chunk_id = c.chunk_id WHERE c.project_id = ?1",
            [&pid],
            |r| r.get(0),
        )?;
        Ok((chunks, rels))
    })
    .unwrap_or((0, 0));

    // 新鮮度：git head 相符 + 索引在 7 天內
    let fresh = freshness(real_str, row.2.as_deref(), row.3);
    Some(CortIndexInfo {
        project_id: pid,
        name: row.0,
        path: row.1,
        git_head: row.2,
        last_indexed_at: row.3,
        extractor_version: row.4,
        chunk_count,
        relationships_count,
        fresh,
    })
}

fn freshness(real_str: &str, indexed_head: Option<&str>, last_indexed_at: Option<i64>) -> bool {
    // 1) git head 相符（若專案是 git repo）
    if let Some(head) = git_head(real_str) {
        if indexed_head.is_some_and(|ih| ih != head.as_str()) {
            return false;
        }
    }
    // 2) 索引不超過 7 天
    match last_indexed_at {
        Some(t) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            now - t <= 7 * 24 * 3600
        }
        None => false,
    }
}

fn git_head(real_str: &str) -> Option<String> {
    let head = std::path::Path::new(real_str)
        .join(".git")
        .join("HEAD");
    let content = std::fs::read_to_string(head).ok()?;
    let content = content.trim();
    if let Some(ref_path) = content.strip_prefix("ref: ") {
        let full = std::path::Path::new(real_str)
            .join(".git")
            .join(ref_path.trim());
        std::fs::read_to_string(full).ok().map(|s| s.trim().to_string())
    } else {
        Some(content.to_string())
    }
}

/// 搜尋 cort 索引的符號（比 tree-sitter 的 top-N 更完整：全 project）
pub fn search_symbols(root: &Path, query: &str) -> Option<Vec<CortHit>> {
    let real = std::fs::canonicalize(root).ok()?;
    let real_str = real.to_str()?;
    let pid = project_id(real_str);
    let pat = format!("%{}%", query.to_lowercase());

    let hits = with_readonly(real_str, |conn| {
        let mut stmt = conn.prepare(
            "SELECT symbol_name, chunk_type, file_path, start_line, end_line, language \
             FROM chunks WHERE project_id = ?1 AND lower(symbol_name) LIKE ?2 \
             ORDER BY start_line LIMIT 50",
        )?;
        let rows = stmt.query_map([&pid, &pat], |r| {
            Ok(CortHit {
                symbol: r.get(0)?,
                chunk_type: r.get(1)?,
                file: r.get(2)?,
                start_line: r.get(3)?,
                end_line: r.get(4)?,
                language: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<CortHit>>>()
    })?;
    if hits.is_empty() {
        None
    } else {
        Some(hits)
    }
}

/// 「誰呼叫/import 這個符號」——反向依賴（cort impact 的資料來源）
pub fn dependents(root: &Path, symbol: &str) -> Option<Vec<CortDependent>> {
    let real = std::fs::canonicalize(root).ok()?;
    let real_str = real.to_str()?;
    let pid = project_id(real_str);

    let deps = with_readonly(real_str, |conn| {
        let mut stmt = conn.prepare(
            "SELECT sc.file_path, sc.symbol_name, sc.start_line, r.rel_type, r.call_site_line, r.confidence_score \
             FROM relationships r \
             JOIN chunks tc ON r.target_chunk_id = tc.chunk_id AND tc.project_id = ?1 \
             JOIN chunks sc ON r.source_chunk_id = sc.chunk_id AND sc.project_id = ?1 \
             WHERE tc.symbol_name = ?2 \
             ORDER BY sc.file_path LIMIT 50",
        )?;
        let rows = stmt.query_map([&pid, symbol], |r| {
            Ok(CortDependent {
                source_file: r.get(0)?,
                source_symbol: r.get(1)?,
                source_start_line: r.get(2)?,
                rel_type: r.get(3)?,
                call_site_line: r.get(4)?,
                confidence_score: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<CortDependent>>>()
    })?;
    if deps.is_empty() {
        None
    } else {
        Some(deps)
    }
}
