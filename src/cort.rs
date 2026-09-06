//! cortexyoung/cort 索引整合：唯讀存取 cort 的 SQLite（~/.cache/cortex-ng/<sha256>.db）
//! 相容 cort schema v4（chunks / projects / relationships）。
//! 不做任何寫入；DB 不存在或 schema 不符時回退到 None。
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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

// ---------------------------------------------------------------------------
// cort-audit：收集「整合是否達成」的驗證數據（唯讀）
// 索引健康 + 覆蓋缺口 + FTS 同步 + 用量（usage.db），全部不寫入
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CortAuditIndex {
    /// 索引健康（index_info 已有欄位）
    pub name: String,
    pub path: String,
    pub fresh: bool,
    pub git_head_matches: bool,
    /// 索引距今幾天；無 last_indexed_at 時為 None
    pub index_age_days: Option<i64>,
    pub chunk_count: i64,
    pub relationships_count: i64,
    /// 覆蓋：file_state 有、chunks 沒有的檔案（completeness 缺口）。
    /// 查詢失敗（schema 差異等）一律 None＝「無法判讀」，絕不用 0 假裝「無缺口」
    /// ——當初 ?1 參數 bug 就是被 unwrap_or(0) 吞成「無缺口」。
    pub file_state_files: Option<i64>,
    pub chunked_files: Option<i64>,
    /// 未 chunk 檔案的「總數」（清單只保留前 20 筆，避免輸出過長）
    pub not_chunked_total: Option<i64>,
    pub not_chunked_files: Vec<String>,
    /// 含至少一個 unparsed chunk 的檔案數（純資訊欄）
    pub files_with_unparsed_chunks: i64,
    /// chunks_fts 列數（表不存在 → None）
    pub fts_docs: Option<i64>,
    /// FTS 與 chunks 的 rowid 雙向差異數（None = 無法判讀）。
    /// 0 才是同步——「數量相等」會在一多一少時偽稱 synced。
    pub fts_drift: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageWindow {
    pub window_days: u32,
    pub total_commands: i64,
    pub by_command: BTreeMap<String, i64>,
    pub suggest_outcomes: BTreeMap<String, i64>,
    /// hook-suggest 的 decline 歸因（鍵如 "no_shape/context_flag"）——
    /// cortexyoung c290c383（2026-09-06）起的新列才帶 decline，舊列自然缺席
    pub declines: BTreeMap<String, i64>,
    pub refresh_outcomes: BTreeMap<String, i64>,
    pub errors: i64,
    pub index_stale_queries: i64,
    pub saved_bytes: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CortAudit {
    pub root: String,
    /// 主機名（/etc/hostname）——usage.db 是每台機器各自的，多機的列靠 host 区分
    pub host: String,
    pub window_days: u32,
    pub index: Option<CortAuditIndex>,
    /// DB 檔案存在但 index=None → 「讀取失敗」，不是「尚未索引」
    pub db_exists: bool,
    pub usage: Option<UsageWindow>,
    /// 固定 7 天窗口（早期訊號；與 `--window` 的長期趨勢互補）
    pub usage_7d: Option<UsageWindow>,
}

/// 主機名：讀 /etc/hostname（WSL/Linux），讀不到則 "unknown"
fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 索引健康 + 覆蓋缺口 + FTS 同步（單一唯讀連線內完成）
pub fn audit_index(root: &Path) -> Option<CortAuditIndex> {
    let real = std::fs::canonicalize(root).ok()?;
    let real_str = real.to_str()?;
    let pid = project_id(real_str);
    let head_now = git_head(real_str);

    with_readonly(real_str, |conn| {
        let row = conn
            .query_row(
                "SELECT name, path, git_head, last_indexed_at FROM projects WHERE project_id = ?1",
                [&pid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()?;

        let (name, path, indexed_head, last_indexed_at) = match row {
            Some(r) => r,
            None => return Ok(None), // 專案尚未被 cort 索引
        };

        let chunk_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE project_id = ?1",
            [&pid],
            |r| r.get(0),
        )?;
        let relationships_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks c JOIN relationships r \
                 ON r.source_chunk_id = c.chunk_id WHERE c.project_id = ?1",
                [&pid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        // 以下健康聲明欄位：查詢失敗 → None（無法判讀），絕不 unwrap_or(0) 假裝健康
        let file_state_files: Option<i64> = conn
            .query_row(
                "SELECT COUNT(*) FROM file_state WHERE project_id = ?1",
                [&pid],
                |r| r.get(0),
            )
            .ok();
        let chunked_files: Option<i64> = conn
            .query_row(
                "SELECT COUNT(DISTINCT file_path) FROM chunks WHERE project_id = ?1",
                [&pid],
                |r| r.get(0),
            )
            .ok();
        let not_chunked_sql = "SELECT file_path FROM file_state WHERE project_id = ? \
             AND file_path NOT IN (SELECT DISTINCT file_path FROM chunks WHERE project_id = ?) \
             ORDER BY file_path";
        let not_chunked_total: Option<i64> = conn
            .query_row(
                "SELECT COUNT(*) FROM file_state WHERE project_id = ? \
                 AND file_path NOT IN (SELECT DISTINCT file_path FROM chunks WHERE project_id = ?)",
                [&pid, &pid],
                |r| r.get(0),
            )
            .ok();
        let mut not_chunked: Vec<String> = Vec::new();
        if not_chunked_total.is_some() {
            if let Ok(mut stmt) = conn.prepare(not_chunked_sql) {
                let rows = stmt.query_map([&pid, &pid], |r| r.get::<_, String>(0));
                if let Ok(rows) = rows {
                    for r in rows.flatten() {
                        if not_chunked.len() < 20 {
                            not_chunked.push(r);
                        }
                    }
                }
            }
        }
        let files_with_unparsed_chunks: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT file_path) FROM chunks \
                 WHERE project_id = ?1 AND chunk_source = 'unparsed'",
                [&pid],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let fts_docs: Option<i64> = conn
            .query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
            .ok();
        // drift 用 rowid 雙向差異，不用數量相等（一多一少會偽稱 synced）
        let fts_drift: Option<i64> = conn
            .query_row(
                "SELECT \
                   (SELECT COUNT(*) FROM chunks c LEFT JOIN chunks_fts f ON f.rowid = c.rowid \
                     WHERE f.rowid IS NULL AND c.project_id = ?1) + \
                   (SELECT COUNT(*) FROM chunks_fts f LEFT JOIN chunks c ON c.rowid = f.rowid \
                     WHERE c.rowid IS NULL)",
                [&pid],
                |r| r.get(0),
            )
            .ok();

        // 新鮮度：git head 相符 + ≤7 天
        let git_head_matches = match (&head_now, &indexed_head) {
            (Some(now), Some(idx)) => now == idx,
            _ => true, // 非 git repo 時不追究
        };
        let age_days = last_indexed_at.map(|t| (now_ms() - t).max(0) / (24 * 3600 * 1000));
        let fresh = is_fresh(git_head_matches, last_indexed_at);

        Ok(Some(CortAuditIndex {
            name,
            path,
            fresh,
            git_head_matches,
            index_age_days: age_days,
            chunk_count,
            relationships_count,
            file_state_files,
            chunked_files,
            not_chunked_total,
            not_chunked_files: not_chunked,
            files_with_unparsed_chunks,
            fts_docs,
            fts_drift,
        }))
    })?
}

fn open_usage_readonly() -> Option<Connection> {
    let db = cache_dir().join("usage.db");
    if !db.is_file() {
        return None;
    }
    let uri = sqlite_uri(&db);
    Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .or_else(|_| {
            Connection::open_with_flags(
                &uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
        })
        .ok()
}

/// 用量統計（cort 自己的 usage.db command_log，唯讀）：window 天內
pub fn audit_usage(window_days: u32) -> Option<UsageWindow> {
    let conn = open_usage_readonly()?;
    let since = now_ms() - (window_days as i64) * 24 * 3600 * 1000;

    let mut u = UsageWindow {
        window_days,
        ..Default::default()
    };
    if let Ok(mut stmt) =
        conn.prepare("SELECT command, COUNT(*) FROM command_log WHERE ts >= ?1 GROUP BY command")
    {
        if let Ok(rows) = stmt.query_map([&since], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        }) {
            for r in rows.flatten() {
                u.by_command.insert(r.0, r.1);
                u.total_commands += r.1;
            }
        }
    }
    // hook 結果分佈（args_summary 是 JSON）
    for (cmd, target) in [
        ("hook-suggest", &mut u.suggest_outcomes),
        ("hook-refresh", &mut u.refresh_outcomes),
    ] {
        if let Ok(mut stmt) =
            conn.prepare("SELECT args_summary FROM command_log WHERE command = ?1 AND ts >= ?2")
        {
            if let Ok(rows) =
                stmt.query_map(rusqlite::params![cmd, since], |r| r.get::<_, String>(0))
            {
                for r in rows.flatten() {
                    let parsed = serde_json::from_str::<serde_json::Value>(&r).ok();
                    let hook = parsed
                        .as_ref()
                        .and_then(|v| v.get("hook").and_then(|h| h.as_str()).map(String::from))
                        .unwrap_or_else(|| "unparsed".to_string());
                    *target.entry(hook.clone()).or_insert(0) += 1;
                    if cmd == "hook-suggest" {
                        if let Some(d) = parsed
                            .as_ref()
                            .and_then(|v| v.get("decline").and_then(|d| d.as_str()))
                        {
                            *u.declines.entry(format!("{hook}/{d}")).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }
    u.errors = conn
        .query_row(
            "SELECT COUNT(*) FROM command_log WHERE status = 'error' AND ts >= ?1",
            [&since],
            |r| r.get(0),
        )
        .unwrap_or(0);
    u.index_stale_queries = conn
        .query_row(
            "SELECT COUNT(*) FROM command_log WHERE index_stale = 1 AND ts >= ?1",
            [&since],
            |r| r.get(0),
        )
        .unwrap_or(0);
    u.saved_bytes = conn
        .query_row(
            "SELECT COALESCE(SUM(saved_bytes), 0) FROM command_log WHERE ts >= ?1",
            [&since],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Some(u)
}

/// 完整審計：索引健康/覆蓋 + 用量（window 天）+ 固定 7 天早期訊號
pub fn audit(root: &Path, window_days: u32) -> CortAudit {
    let real = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let usage = audit_usage(window_days);
    let usage_7d = if window_days == 7 {
        usage.clone()
    } else {
        audit_usage(7)
    };
    CortAudit {
        root: real.to_string_lossy().into_owned(),
        host: host_name(),
        window_days,
        index: audit_index(&real),
        db_exists: db_exists(&real),
        usage,
        usage_7d,
    }
}
