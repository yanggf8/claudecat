//! Navigation and memory recall usage is written locally only.
//! Raw queries are deliberately never stored; recall counts are exposure, not correctness.
use crate::data_dir;
use crate::navigate::NavigateResult;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageReport {
    pub window_days: u32,
    pub events: i64,
    pub cort_events: i64,
    pub symbol_hit_events: i64,
    pub document_hit_events: i64,
    pub no_hit_events: i64,
    pub top_documents: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MemoryUsageReport {
    pub window_days: u32,
    pub recall_calls: i64,
    pub errors: i64,
    pub absent_ledgers: i64,
    pub no_match_calls: i64,
    pub unchanged_hits: i64,
    pub changed_hits: i64,
    pub missing_hits: i64,
    pub conflict_hits: i64,
    pub synced_calls: i64,
    pub offline_or_pending_calls: i64,
    pub total_recall_elapsed_ms: i64,
    pub max_recall_elapsed_ms: i64,
    /// The caller supplies this label. It is not independently verified from a harness payload.
    pub by_harness_declared: BTreeMap<String, i64>,
    /// Machine labels are stored per event, so copying a database cannot silently relabel rows.
    pub by_machine: BTreeMap<String, i64>,
    pub by_project_id: BTreeMap<String, i64>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn db_path() -> PathBuf {
    data_dir::data_dir().join("usage.db")
}

fn open() -> rusqlite::Result<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    }
    let mut conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS navigation_log (
            ts INTEGER NOT NULL,
            project_id TEXT NOT NULL,
            machine_id TEXT NOT NULL DEFAULT 'legacy_unknown',
            machine_source TEXT NOT NULL DEFAULT 'legacy_no_stamp',
            mode TEXT NOT NULL,
            query_shape TEXT NOT NULL,
            query_hash TEXT NOT NULL,
            symbol_hits INTEGER NOT NULL,
            document_hits INTEGER NOT NULL,
            file_hits INTEGER NOT NULL,
            cort_hits INTEGER NOT NULL,
            route_kind TEXT NOT NULL,
            primary_document TEXT,
            primary_heading TEXT
        );
        CREATE INDEX IF NOT EXISTS navigation_log_ts ON navigation_log(ts);
        CREATE INDEX IF NOT EXISTS navigation_log_document ON navigation_log(primary_document);
        CREATE TABLE IF NOT EXISTS memory_recall_log (
            ts INTEGER NOT NULL,
            project_id TEXT NOT NULL,
            machine_id TEXT NOT NULL,
            machine_source TEXT NOT NULL,
            harness_declared TEXT NOT NULL,
            outcome TEXT NOT NULL,
            ledger_status TEXT NOT NULL,
            sync_status TEXT NOT NULL DEFAULT 'legacy_unknown',
            hit_count INTEGER NOT NULL,
            unchanged_hits INTEGER NOT NULL,
            changed_hits INTEGER NOT NULL,
            missing_hits INTEGER NOT NULL,
            conflict_hits INTEGER NOT NULL,
            elapsed_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS memory_recall_log_ts ON memory_recall_log(ts);",
    )?;
    ensure_navigation_machine_columns(&mut conn)?;
    Ok(conn)
}

fn ensure_navigation_machine_columns(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (column, ddl) in [
        (
            "machine_id",
            "ALTER TABLE navigation_log ADD COLUMN machine_id TEXT NOT NULL DEFAULT 'legacy_unknown'",
        ),
        (
            "machine_source",
            "ALTER TABLE navigation_log ADD COLUMN machine_source TEXT NOT NULL DEFAULT 'legacy_no_stamp'",
        ),
    ] {
        let present = tx
            .query_row(
                "SELECT 1 FROM pragma_table_info('navigation_log') WHERE name = ?1 LIMIT 1",
                [column],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !present {
            tx.execute(ddl, [])?;
        }
    }
    tx.commit()
}

pub(crate) fn machine_stamp() -> (String, &'static str) {
    if let Ok(raw) = std::fs::read_to_string("/etc/machine-id") {
        let id = raw.trim();
        if !id.is_empty() {
            return (query_hash(id), "etc-machine-id-sha256");
        }
    }
    if let Some(host) = crate::host::resolve_host() {
        return (query_hash(&host), "hostname-sha256");
    }
    ("unknown".into(), "unavailable")
}

fn ensure_memory_machine_columns(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (column, ddl) in [
        (
            "machine_id",
            "ALTER TABLE memory_recall_log ADD COLUMN machine_id TEXT NOT NULL DEFAULT 'legacy_unknown'",
        ),
        (
            "machine_source",
            "ALTER TABLE memory_recall_log ADD COLUMN machine_source TEXT NOT NULL DEFAULT 'legacy_no_stamp'",
        ),
        (
            "sync_status",
            "ALTER TABLE memory_recall_log ADD COLUMN sync_status TEXT NOT NULL DEFAULT 'legacy_unknown'",
        ),
    ] {
        let present = tx
            .query_row(
                "SELECT 1 FROM pragma_table_info('memory_recall_log') WHERE name = ?1 LIMIT 1",
                [column],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !present {
            tx.execute(ddl, [])?;
        }
    }
    tx.commit()
}

// `memory_recall_log` gets no new rows from ClaudeCat: `memory recall` forwards to cort, which
// records its own usage. The table and this report stay so historical recall rows remain readable.
pub fn memory_report(days: u32) -> Result<MemoryUsageReport, String> {
    let mut conn = open().map_err(|e| e.to_string())?;
    ensure_memory_machine_columns(&mut conn).map_err(|e| e.to_string())?;
    let since = now_ms().saturating_sub(days as i64 * 24 * 60 * 60 * 1000);
    let mut out = conn
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(outcome = 'error'), 0),
                    COALESCE(SUM(ledger_status = 'absent'), 0),
                    COALESCE(SUM(ledger_status = 'present' AND hit_count = 0), 0),
                    COALESCE(SUM(unchanged_hits), 0),
                    COALESCE(SUM(changed_hits), 0),
                    COALESCE(SUM(missing_hits), 0),
                    COALESCE(SUM(conflict_hits), 0),
                    COALESCE(SUM(sync_status = 'synced'), 0),
                    COALESCE(SUM(sync_status = 'offline_or_pending'), 0),
                    COALESCE(SUM(elapsed_ms), 0),
                    COALESCE(MAX(elapsed_ms), 0)
             FROM memory_recall_log WHERE ts >= ?1",
            [since],
            |row| {
                Ok(MemoryUsageReport {
                    window_days: days,
                    recall_calls: row.get(0)?,
                    errors: row.get(1)?,
                    absent_ledgers: row.get(2)?,
                    no_match_calls: row.get(3)?,
                    unchanged_hits: row.get(4)?,
                    changed_hits: row.get(5)?,
                    missing_hits: row.get(6)?,
                    conflict_hits: row.get(7)?,
                    synced_calls: row.get(8)?,
                    offline_or_pending_calls: row.get(9)?,
                    total_recall_elapsed_ms: row.get(10)?,
                    max_recall_elapsed_ms: row.get(11)?,
                    ..Default::default()
                })
            },
        )
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT harness_declared, COUNT(*) FROM memory_recall_log WHERE ts >= ?1
             GROUP BY harness_declared ORDER BY harness_declared",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (harness, count) = row.map_err(|e| e.to_string())?;
        out.by_harness_declared.insert(harness, count);
    }
    let mut stmt = conn
        .prepare(
            "SELECT machine_source, machine_id, COUNT(*) FROM memory_recall_log WHERE ts >= ?1
             GROUP BY machine_source, machine_id ORDER BY machine_source, machine_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (source, id, count) = row.map_err(|e| e.to_string())?;
        out.by_machine.insert(format!("{source}:{id}"), count);
    }
    let mut stmt = conn
        .prepare(
            "SELECT project_id, COUNT(*) FROM memory_recall_log WHERE ts >= ?1
             GROUP BY project_id ORDER BY project_id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (project, count) = row.map_err(|e| e.to_string())?;
        out.by_project_id.insert(project, count);
    }
    Ok(out)
}

fn query_shape(query: &str) -> &'static str {
    let tokens = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .count();
    if query.contains('/') || query.contains('.') {
        "path_like"
    } else if query.contains('?') || query.split_whitespace().count() >= 5 {
        "question_like"
    } else if tokens <= 1 {
        "one_token"
    } else {
        "multi_token"
    }
}

fn query_hash(query: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(query.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn route_kind(result: &NavigateResult) -> &'static str {
    match (result.symbols.is_empty(), result.documents.is_empty()) {
        (false, false) => "mixed",
        (false, true) => "symbol",
        (true, false) => "document",
        (true, true) => "none",
    }
}

pub fn record_navigation(
    root: &Path,
    query: &str,
    used_cort: bool,
    cort_hits: usize,
    result: &NavigateResult,
) {
    if std::env::var_os("CLAUDECAT_NO_USAGE").is_some() {
        return;
    }
    let Ok(conn) = open() else { return };
    let primary = result.documents.first();
    let (machine_id, machine_source) = machine_stamp();
    let _ = conn.execute(
        "INSERT INTO navigation_log
         (ts, project_id, machine_id, machine_source, mode, query_shape, query_hash, symbol_hits, document_hits,
          file_hits, cort_hits, route_kind, primary_document, primary_heading)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            now_ms(),
            crate::cort::project_id(
                &std::fs::canonicalize(root)
                    .unwrap_or_else(|_| root.to_path_buf())
                    .to_string_lossy()
            ),
            machine_id,
            machine_source,
            if used_cort { "cort" } else { "tree" },
            query_shape(query),
            query_hash(query),
            result.symbols.len() as i64,
            result.documents.len() as i64,
            result.files.len() as i64,
            cort_hits as i64,
            route_kind(result),
            primary.map(|d| d.path.as_str()),
            primary.map(|d| d.heading_path.join(" > ")),
        ],
    );
}

pub fn report(days: u32) -> Option<UsageReport> {
    let conn = open().ok()?;
    let since = now_ms().saturating_sub(days as i64 * 24 * 60 * 60 * 1000);
    let mut out = UsageReport {
        window_days: days,
        ..Default::default()
    };
    out.events = conn
        .query_row(
            "SELECT COUNT(*) FROM navigation_log WHERE ts >= ?1",
            [since],
            |row| row.get(0),
        )
        .ok()?;
    out.cort_events = count_where(&conn, since, "mode = 'cort'");
    out.symbol_hit_events = count_where(&conn, since, "symbol_hits > 0");
    out.document_hit_events = count_where(&conn, since, "document_hits > 0");
    out.no_hit_events = count_where(&conn, since, "route_kind = 'none'");
    if let Ok(mut stmt) = conn.prepare(
        "SELECT primary_document, COUNT(*) FROM navigation_log
         WHERE ts >= ?1 AND primary_document IS NOT NULL
         GROUP BY primary_document ORDER BY COUNT(*) DESC, primary_document LIMIT 20",
    ) {
        if let Ok(rows) = stmt.query_map([since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            for row in rows.flatten() {
                out.top_documents.insert(row.0, row.1);
            }
        }
    }
    Some(out)
}

fn count_where(conn: &Connection, since: i64, predicate: &str) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM navigation_log WHERE ts >= ?1 AND {predicate}"),
        [since],
        |row| row.get(0),
    )
    .unwrap_or(0)
}
