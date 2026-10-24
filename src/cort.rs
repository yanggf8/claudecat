//! cortexyoung/cort 索引整合：唯讀存取 cort 的 SQLite（~/.cache/cortex-ng/<sha256>.db）
//! 相容 cort schema v5–v7（chunks / projects / relationships / _cortex_meta / file_state；
//! v6 的 `file_state.indexed_uncommitted`、v7 的 `file_state.chunk_count`）。
//! 新欄位在舊 DB 上整句查詢會失敗 → 對應欄位回 `None`＝「無法判讀」，
//! 而不是拿 0 假裝健康；claudecat 是消費者，靠欄位查不查得到判斷，不靠版本號。
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
    /// cort 的 `_cortex_meta.SCHEMA_VERSION`（純呈現；claudecat 是消費者，
    /// 不硬編碼「期望版本」——那個常數只有 cort 自己知道，寫死必然像文件一樣爛掉）
    pub schema_version: Option<String>,
    /// derived graph 是否落後 chunks（None = 無法判讀，見 [`read_meta`]）
    pub graph_pending: Option<bool>,
    /// HEAD 相符 + 索引 ≤7 天 **且** 圖不是已知落後（`graph_pending != Some(true)`）。
    /// `None`（讀不到 `_cortex_meta`）不翻布林——沒讀到不等於壞掉。
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
    pub content: Option<String>,
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

/// SQLite URI：路徑裡的 `%` `?` `#` 空白 會被 URI 語法吃掉（query/fragment 邊界、
/// percent-decode），必須 percent-encode；其餘位元組（含 UTF-8）原樣。
pub fn sqlite_uri(path: &Path) -> String {
    let mut out = String::from("file:");
    for c in path.to_string_lossy().chars() {
        match c {
            '%' | '?' | '#' | ' ' => out.push_str(&format!("%{:02X}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push_str("?immutable=1");
    out
}

/// cort 的 `_cortex_meta`（key/value）：升級與圖重建狀態的唯一權威。
/// 回傳 `(schema_version, graph_pending)`，兩者的 `None` 一律是「無法判讀」。
///
/// `graph_pending` 是 cort 的 derived-graph 旗標（cort `db.rs` 判準為 `== Some("1")`，
/// 在 `rebuild_reasons_of` 轉成 `graph_incomplete`）：
/// - 每次 per-file 增量都會先設 1，但**與 `git_head`/`last_indexed_at` 在同一個
///   transaction 裡清 0**（cort `incremental.rs`），所以外部讀到持續為 1 只有兩種情況：
///   ①schema 遷移後還沒跑過任何索引（此時時戳與 HEAD 仍是「新的」——這正是會被誤報
///   fresh 的洞）②增量跑到一半死掉（此時時戳沒前進，本來就會 STALE）。
/// - 表讀得到但沒這個鍵 → `Some(false)`（對齊 cort 的判準；「沒這個鍵」是讀到的事實）
/// - 表不存在／查詢失敗 → `None`；**絕不寫成 `Some(false)` 假裝圖是新的**
fn read_meta(conn: &Connection) -> (Option<String>, Option<bool>) {
    // 外層 Option：Err（表不存在／查詢失敗）→ None＝無法判讀
    // 內層 Option：Ok(None)＝表在、鍵不存在
    let get = |key: &str| -> Option<Option<String>> {
        conn.query_row(
            "SELECT value FROM _cortex_meta WHERE key = ?1",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
    };
    let schema_version = get("SCHEMA_VERSION").flatten();
    let graph_pending = get("graph_pending").map(|v| v.as_deref() == Some("1"));
    (schema_version, graph_pending)
}

/// cort 的 DB 檔案是否存在（區分「尚未索引」與「存在但讀取失敗」）
pub fn db_exists(root: &Path) -> bool {
    std::fs::canonicalize(root)
        .ok()
        .and_then(|r| r.to_str().map(db_path_for))
        .is_some_and(|p| p.is_file())
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
    let uri = sqlite_uri(&db);
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

    let (schema_version, graph_pending) =
        with_readonly(real_str, |conn| Ok(read_meta(conn))).unwrap_or((None, None));

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

    // 新鮮度：git head 相符 + 索引在 7 天內 + 圖不是已知落後
    let fresh = freshness(real_str, row.2.as_deref(), row.3) && graph_pending != Some(true);
    Some(CortIndexInfo {
        project_id: pid,
        name: row.0,
        path: row.1,
        git_head: row.2,
        last_indexed_at: row.3,
        extractor_version: row.4,
        chunk_count,
        relationships_count,
        schema_version,
        graph_pending,
        fresh,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 索引新鮮度：git head 相符 + 索引時間在 7 天內。
/// `last_indexed_at` 與 cort 的寫入一致，是**毫秒**（epoch ms）——
/// 曾因誤當秒數比較，7 天關卡永不觸發（cort-status 對 40 天前的索引仍報 fresh）。
const FRESH_WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;

fn is_fresh(head_matches: bool, last_indexed_at: Option<i64>) -> bool {
    head_matches && last_indexed_at.is_some_and(|t| now_ms() - t <= FRESH_WINDOW_MS)
}

fn freshness(real_str: &str, indexed_head: Option<&str>, last_indexed_at: Option<i64>) -> bool {
    // git head 相符（非 git repo 或索引未記錄 head 時不追究）
    let head_matches = match (&git_head(real_str), indexed_head) {
        (Some(now), Some(ih)) => now == ih,
        _ => true,
    };
    is_fresh(head_matches, last_indexed_at)
}

fn git_head(real_str: &str) -> Option<String> {
    let head = std::path::Path::new(real_str).join(".git").join("HEAD");
    let content = std::fs::read_to_string(head).ok()?;
    let content = content.trim();
    if let Some(ref_path) = content.strip_prefix("ref: ") {
        let full = std::path::Path::new(real_str)
            .join(".git")
            .join(ref_path.trim());
        std::fs::read_to_string(full)
            .ok()
            .map(|s| s.trim().to_string())
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
            "SELECT symbol_name, chunk_type, file_path, start_line, end_line, language, content \
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
                content: r.get(6)?,
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

/// 把使用者查詢轉成 FTS5 MATCH 字串：拆成 alnum token、逐個加雙引號（字面 token，
/// 不讓 FTS 運算子影響）、以 AND 連接。例：`user creation` → `"user" AND "creation"`。
fn fts_match_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" AND "))
    }
}

/// FTS 全文 fallback（`cort recall` 的資料源 `chunks_fts`）：symbol_name 未命中時，
/// 搜 content / symbol / file 全文（external-content FTS5，唯讀 join 回 chunks）。
pub fn search_fts(root: &Path, query: &str) -> Option<Vec<CortHit>> {
    let real = std::fs::canonicalize(root).ok()?;
    let real_str = real.to_str()?;
    let pid = project_id(real_str);
    let match_q = fts_match_query(query)?;

    let hits = with_readonly(real_str, |conn| {
        let mut stmt = conn.prepare(
            "SELECT c.symbol_name, c.chunk_type, c.file_path, c.start_line, c.end_line, c.language, c.content \
             FROM chunks_fts f JOIN chunks c ON c.rowid = f.rowid \
             WHERE chunks_fts MATCH ?1 AND c.project_id = ?2 \
             ORDER BY f.rank LIMIT 20",
        )?;
        let rows = stmt.query_map([&match_q, &pid], |r| {
            Ok(CortHit {
                symbol: r.get(0)?,
                chunk_type: r.get(1)?,
                file: r.get(2)?,
                start_line: r.get(3)?,
                end_line: r.get(4)?,
                language: r.get(5)?,
                content: r.get(6)?,
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

/// 摘要 cort content：壓縮空白、取前 max_chars 字元（省 read 用，單行可讀）。
pub fn content_summary(content: &str, max_chars: usize) -> String {
    let collapsed: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.trim();
    let mut out: String = collapsed.chars().take(max_chars).collect();
    if collapsed.chars().count() > max_chars {
        out.push('…');
    }
    out
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
