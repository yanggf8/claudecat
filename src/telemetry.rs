//! Read existing structured local collection files without changing their writers.
//! Hook latency stays independent of the network; the Turso sync is a separate CLI action.

use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug)]
pub struct Event {
    pub source: &'static str,
    pub id: String,
    pub machine_id: String,
    pub machine_source: String,
    pub project_id: Option<String>,
    pub time_ms: Option<i64>,
    pub payload: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ProjectAlias {
    pub machine_id: String,
    pub local_project_id: String,
    pub shared_project_id: String,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub events: Vec<Event>,
    pub missing_sources: Vec<String>,
    pub project_aliases: Vec<ProjectAlias>,
    pub unresolved_project_ids: Vec<String>,
    pub unmatched_project_rows: usize,
    pub legacy_cort_rows: usize,
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn json_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::from(n),
        ValueRef::Real(n) => Value::from(n),
        ValueRef::Text(bytes) => Value::from(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Value::from(format!(
            "blob:{}",
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )),
    }
}

fn open_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn cort_stamp(conn: &Connection) -> (String, String) {
    let get = |key| {
        conn.query_row(
            "SELECT value FROM _usage_meta WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| "legacy_unknown".into())
    };
    (get("MACHINE_ID"), get("MACHINE_ID_SOURCE"))
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info(?1) WHERE name = ?2")
        .map_err(|e| e.to_string())?;
    stmt.exists((table, column)).map_err(|e| e.to_string())
}

fn read_table(
    scan: &mut Scan,
    path: &Path,
    source: &'static str,
    table: &'static str,
    default_stamp: (&str, &str),
) -> Result<(), String> {
    if !path.exists() {
        scan.missing_sources.push(source.into());
        return Ok(());
    }
    let conn = open_readonly(path)?;
    let stamp = if source == "cort_command" {
        cort_stamp(&conn)
    } else {
        (default_stamp.0.to_string(), default_stamp.1.to_string())
    };
    let row_stamp = source == "claudecat_memory_recall"
        || (source == "claudecat_navigation"
            && has_column(&conn, table, "machine_id")?
            && has_column(&conn, table, "machine_source")?)
        || (source == "cort_command"
            && has_column(&conn, table, "machine_id")?
            && has_column(&conn, table, "machine_source")?);
    let mut stmt = conn
        .prepare(&format!("SELECT rowid, * FROM {table}"))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut object = Map::new();
        for (index, name) in names.iter().enumerate() {
            object.insert(
                name.clone(),
                json_value(row.get_ref(index).map_err(|e| e.to_string())?),
            );
        }
        let rowid = object.get("rowid").and_then(Value::as_i64).unwrap_or(0);
        let row_machine = object.get("machine_id").and_then(Value::as_str);
        let legacy_cort = source == "cort_command"
            && (!row_stamp || row_machine.is_none_or(|id| id == "legacy_unknown"));
        if legacy_cort {
            scan.legacy_cort_rows += 1;
        }
        let machine_id = if row_stamp && !legacy_cort {
            row_machine.unwrap_or("legacy_unknown").to_string()
        } else {
            stamp.0.clone()
        };
        let machine_source = if row_stamp && !legacy_cort {
            object
                .get("machine_source")
                .and_then(Value::as_str)
                .unwrap_or("legacy_no_stamp")
                .to_string()
        } else {
            stamp.1.clone()
        };
        let project_id = object
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let time_ms = object.get("ts").and_then(Value::as_i64);
        // The pre-v2 cort rows were uploaded before these columns existed. Keep their
        // original payload and event ID so migration cannot duplicate historical events.
        if legacy_cort && row_stamp {
            object.remove("machine_id");
            object.remove("machine_source");
        }
        let payload = Value::Object(object).to_string();
        let id = digest(&format!("{source}\0{machine_id}\0{rowid}\0{payload}"));
        scan.events.push(Event {
            source,
            id,
            machine_id,
            machine_source,
            project_id,
            time_ms,
            payload,
        });
    }
    Ok(())
}

fn cost_dir() -> PathBuf {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(data).join("cort-evals/claude-cost")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share/cort-evals/claude-cost")
    } else {
        PathBuf::from(".local/share/cort-evals/claude-cost")
    }
}

fn read_cost(scan: &mut Scan, dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        scan.missing_sources.push("cost_daily".into());
        return Ok(());
    }
    for item in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let body = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let report: Value =
            serde_json::from_str(&body).map_err(|e| format!("{}: {e}", path.display()))?;
        let machine = report
            .get("machine")
            .ok_or_else(|| format!("{}: cost report has no machine stamp", path.display()))?;
        let machine_id = machine
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{}: cost report machine id missing", path.display()))?;
        let machine_source = machine
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let payload = report.to_string();
        let id = digest(&format!(
            "cost_daily\0{machine_id}\0{}\0{payload}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        scan.events.push(Event {
            source: "cost_daily",
            id,
            machine_id: machine_id.into(),
            machine_source: machine_source.into(),
            project_id: None,
            time_ms: None,
            payload,
        });
    }
    Ok(())
}

fn read_project_aliases(scan: &mut Scan, cache_dir: &Path) -> Result<(), String> {
    if !cache_dir.is_dir() {
        return Ok(());
    }
    let mut observed = std::collections::HashMap::<&str, std::collections::HashSet<&str>>::new();
    for event in &scan.events {
        if let Some(project) = event.project_id.as_deref() {
            if event.machine_id != "legacy_unknown" && event.machine_id != "unknown" {
                observed
                    .entry(project)
                    .or_default()
                    .insert(&event.machine_id);
            }
        }
    }
    let mut aliases = std::collections::HashSet::new();
    for item in fs::read_dir(cache_dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|ext| ext != "db")
            || path.file_name().is_some_and(|name| name == "usage.db")
        {
            continue;
        }
        let Ok(conn) = open_readonly(&path) else {
            continue;
        };
        let Ok(mut stmt) = conn.prepare("SELECT project_id, path FROM projects") else {
            continue;
        };
        let Ok(mut rows) = stmt.query([]) else {
            continue;
        };
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let local: String = row.get(0).map_err(|e| e.to_string())?;
            let project_path: String = row.get(1).map_err(|e| e.to_string())?;
            let Some(machines) = observed.get(local.as_str()) else {
                continue;
            };
            let canonical = match fs::canonicalize(&project_path) {
                Ok(path) => path,
                Err(_) => {
                    scan.unresolved_project_ids.push(local);
                    continue;
                }
            };
            if crate::cort::project_id(&canonical.to_string_lossy()) != local {
                scan.unresolved_project_ids.push(local);
                continue;
            }
            match crate::turso_memory::project_from_git_origin(&canonical) {
                Ok(shared) => {
                    for machine_id in machines {
                        aliases.insert(ProjectAlias {
                            machine_id: (*machine_id).to_string(),
                            local_project_id: local.clone(),
                            shared_project_id: shared.clone(),
                        });
                    }
                }
                Err(_) => scan.unresolved_project_ids.push(local),
            }
        }
    }
    scan.project_aliases = aliases.into_iter().collect();
    scan.project_aliases.sort_by(|a, b| {
        a.machine_id
            .cmp(&b.machine_id)
            .then(a.local_project_id.cmp(&b.local_project_id))
            .then(a.shared_project_id.cmp(&b.shared_project_id))
    });
    scan.unresolved_project_ids.sort();
    scan.unresolved_project_ids.dedup();
    Ok(())
}

pub fn scan(
    cort_usage: Option<&Path>,
    claudecat_usage: Option<&Path>,
    cost_reports: Option<&Path>,
) -> Result<Scan, String> {
    let mut result = Scan::default();
    let cort_path = cort_usage
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::cort::cache_dir().join("usage.db"));
    let cat_path = claudecat_usage
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::data_dir::data_dir().join("usage.db"));
    let cost_path = cost_reports.map(Path::to_path_buf).unwrap_or_else(cost_dir);
    let cache_dir = cort_path.parent().unwrap_or_else(|| Path::new("."));
    read_table(
        &mut result,
        &cort_path,
        "cort_command",
        "command_log",
        ("legacy_unknown", "legacy_no_stamp"),
    )?;
    read_table(
        &mut result,
        &cat_path,
        "claudecat_navigation",
        "navigation_log",
        ("legacy_unknown", "legacy_no_stamp"),
    )?;
    read_table(
        &mut result,
        &cat_path,
        "claudecat_memory_recall",
        "memory_recall_log",
        ("legacy_unknown", "legacy_no_stamp"),
    )?;
    read_cost(&mut result, &cost_path)?;
    read_project_aliases(&mut result, cache_dir)?;
    let mapped: std::collections::HashSet<_> = result
        .project_aliases
        .iter()
        .map(|alias| (alias.machine_id.as_str(), alias.local_project_id.as_str()))
        .collect();
    result.unmatched_project_rows = result
        .events
        .iter()
        .filter(|event| {
            event
                .project_id
                .as_deref()
                .is_some_and(|project| !mapped.contains(&(event.machine_id.as_str(), project)))
        })
        .count();
    Ok(result)
}
