//! Structured usage telemetry synced to the shared Turso database.
//! The local file is a sync replica and must never be opened with rusqlite or the Turso CLI.
//!
//! Project memory lives in cort (`cort memory`); `claudecat memory` forwards every verb except
//! `usage` and `eval-report` before argument parsing, so ClaudeCat holds no ledger code of its own.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_TELEMETRY: &str = "CREATE TABLE IF NOT EXISTS telemetry_events (source TEXT NOT NULL, event_id TEXT NOT NULL, machine_id TEXT NOT NULL, machine_source TEXT NOT NULL, project_id TEXT, time_ms INTEGER, payload TEXT NOT NULL, PRIMARY KEY (source, event_id))";
const CREATE_ALIASES: &str = "CREATE TABLE IF NOT EXISTS telemetry_project_aliases (machine_id TEXT NOT NULL, local_project_id TEXT NOT NULL, shared_project_id TEXT NOT NULL, PRIMARY KEY (machine_id, local_project_id, shared_project_id))";

#[derive(Debug, Serialize)]
pub struct TelemetrySyncReport {
    pub scanned: usize,
    pub inserted: usize,
    pub already_present: usize,
    pub by_source: std::collections::BTreeMap<String, usize>,
    pub unattributed_machine_rows: usize,
    pub legacy_cort_rows: usize,
    pub project_aliases: usize,
    pub unresolved_project_ids: Vec<String>,
    pub unmatched_project_rows: usize,
    pub missing_sources: Vec<String>,
    pub sync_status: &'static str,
}

#[derive(Deserialize, Serialize)]
struct Config {
    url: String,
    token: String,
}

fn digest(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn config_path() -> PathBuf {
    crate::data_dir::data_dir().join("turso-memory.json")
}

/// Write the credentials `telemetry sync` reads. `claudecat memory configure` forwards to cort
/// and writes cort's own config, so this is the only command that creates this file.
pub fn configure(url: &str, token: &str) -> Result<PathBuf, String> {
    let url = url.trim().trim_end_matches('/');
    if !(url.starts_with("https://") || url.starts_with("turso://") || url.starts_with("libsql://"))
        || token.trim().is_empty()
    {
        return Err("provide a Turso database URL and a nonempty auth token".into());
    }
    let path = config_path();
    let body = serde_json::to_string(&Config {
        url: url.to_string(),
        token: token.trim().to_string(),
    })
    .map_err(|e| e.to_string())?;
    // A credential file is local to this machine and private to its owner.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Turso memory config must not be a symlink".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), stamp));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    crate::data_dir::write_atomic(&path, &body).map_err(|e| e.to_string())?;
    Ok(path)
}

fn config() -> Result<Config, String> {
    let path = config_path();
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Turso memory config must not be a symlink".into());
    }
    let body = fs::read_to_string(&path).map_err(|_| {
        format!(
            "Turso telemetry is not configured: {} is missing; run `claudecat telemetry configure --url URL --token-stdin`",
            path.display()
        )
    })?;
    serde_json::from_str(&body).map_err(|e| format!("invalid Turso memory config: {e}"))
}

/// Resolve an indexed project's identity from its own origin, ignoring the process-wide
/// override that applies only to the current memory command's chosen project.
pub(crate) fn project_from_git_origin(root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot identify project Git remote: {e}"))?;
    if !output.status.success() {
        return Err("project needs an origin remote or CLAUDECAT_MEMORY_PROJECT".into());
    }
    let remote = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let remote = remote.trim();
    if remote.is_empty() {
        return Err("empty origin remote".into());
    }
    Ok(digest(&normalize_remote(remote)))
}

fn normalize_remote(remote: &str) -> String {
    // Match common Git SSH and HTTPS spellings. A caller can set an explicit project key
    // when its host has case-sensitive paths or uses another remote syntax.
    let remote = remote.trim().trim_end_matches(".git");
    let normalized = if let Some(s) = remote.strip_prefix("git@") {
        s.replace(':', "/")
    } else if let Some(s) = remote.strip_prefix("ssh://git@") {
        s.to_string()
    } else if let Some(s) = remote.strip_prefix("https://") {
        s.to_string()
    } else {
        remote.to_string()
    };
    normalized.to_lowercase()
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

async fn open_telemetry(config: &Config) -> Result<turso::sync::Database, String> {
    open_replica(config, "telemetry.db").await
}

async fn open_replica(config: &Config, file: &str) -> Result<turso::sync::Database, String> {
    let dir = crate::data_dir::data_dir()
        .join("turso-memory")
        .join(digest(&config.url));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let path = dir.join(file);
    turso::sync::Builder::new_remote(&path.to_string_lossy())
        .with_remote_url(&config.url)
        .with_auth_token(&config.token)
        .build()
        .await
        .map_err(|e| format!("cannot open Turso sync replica: {e}"))
}

pub fn sync_telemetry(
    cort_usage: Option<&Path>,
    claudecat_usage: Option<&Path>,
    cost_reports: Option<&Path>,
) -> Result<TelemetrySyncReport, String> {
    let scan = crate::telemetry::scan(cort_usage, claudecat_usage, cost_reports)?;
    with_telemetry_lock(|config| {
        runtime()?.block_on(async {
            let db = open_telemetry(&config).await?;
            let conn = db.connect().await.map_err(|e| e.to_string())?;
            conn.execute(CREATE_TELEMETRY, ())
                .await
                .map_err(|e| e.to_string())?;
            conn.execute(CREATE_ALIASES, ())
                .await
                .map_err(|e| e.to_string())?;
            db.push().await.map_err(|e| format!("pending telemetry could not sync: {e}"))?;
            db.pull().await.map_err(|e| format!("cannot pull telemetry before upload: {e}"))?;
            let mut known = std::collections::HashSet::new();
            let mut rows = conn
                .query("SELECT source, event_id FROM telemetry_events", ())
                .await
                .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
                let source: String = row.get(0).map_err(|e| e.to_string())?;
                let id: String = row.get(1).map_err(|e| e.to_string())?;
                known.insert((source, id));
            }
            drop(rows);
            let pending: Vec<_> = scan
                .events
                .iter()
                .filter(|event| !known.contains(&(event.source.to_string(), event.id.clone())))
                .collect();
            let has_pending = !pending.is_empty() || !scan.project_aliases.is_empty();
            if has_pending {
                conn.execute("BEGIN CONCURRENT", ())
                    .await
                    .map_err(|e| format!("cannot begin telemetry batch: {e}"))?;
            }
            let mut inserted = 0;
            for event in pending {
                match conn
                    .execute(
                        "INSERT OR IGNORE INTO telemetry_events (source, event_id, machine_id, machine_source, project_id, time_ms, payload) VALUES (?, ?, ?, ?, ?, ?, ?)",
                        (
                            event.source,
                            event.id.as_str(),
                            event.machine_id.as_str(),
                            event.machine_source.as_str(),
                            event.project_id.as_deref(),
                            event.time_ms,
                            event.payload.as_str(),
                        ),
                    )
                    .await
                {
                    Ok(n) => inserted += n as usize,
                    Err(e) => {
                        let _ = conn.execute("ROLLBACK", ()).await;
                        return Err(format!("telemetry batch insert failed: {e}"));
                    }
                }
            }
            for alias in &scan.project_aliases {
                if let Err(e) = conn
                    .execute(
                        "INSERT OR IGNORE INTO telemetry_project_aliases (machine_id, local_project_id, shared_project_id) VALUES (?, ?, ?)",
                        (
                            alias.machine_id.as_str(),
                            alias.local_project_id.as_str(),
                            alias.shared_project_id.as_str(),
                        ),
                    )
                    .await
                {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    return Err(format!("project alias insert failed: {e}"));
                }
            }
            if has_pending {
                conn.execute("COMMIT", ())
                    .await
                    .map_err(|e| format!("telemetry batch commit failed: {e}"))?;
            }
            db.push().await.map_err(|e| format!("telemetry saved locally but push failed: {e}"))?;
            db.pull().await.map_err(|e| format!("telemetry pushed but pull failed: {e}"))?;
            db.checkpoint()
                .await
                .map_err(|e| format!("telemetry synced but local checkpoint failed: {e}"))?;
            let mut by_source = std::collections::BTreeMap::new();
            for event in &scan.events {
                *by_source.entry(event.source.to_string()).or_insert(0) += 1;
            }
            Ok(TelemetrySyncReport {
                scanned: scan.events.len(),
                inserted,
                already_present: scan.events.len().saturating_sub(inserted),
                by_source,
                unattributed_machine_rows: scan
                    .events
                    .iter()
                    .filter(|event| event.machine_id == "legacy_unknown")
                    .count(),
                legacy_cort_rows: scan.legacy_cort_rows,
                project_aliases: scan.project_aliases.len(),
                unresolved_project_ids: scan.unresolved_project_ids,
                unmatched_project_rows: scan.unmatched_project_rows,
                missing_sources: scan.missing_sources,
                sync_status: "synced",
            })
        })
    })
}

fn with_telemetry_lock<T>(action: impl FnOnce(Config) -> Result<T, String>) -> Result<T, String> {
    with_named_lock("telemetry.lock", action)
}

fn with_named_lock<T>(
    name: &str,
    action: impl FnOnce(Config) -> Result<T, String>,
) -> Result<T, String> {
    let config = config()?;
    let lock_path = crate::data_dir::data_dir()
        .join("turso-memory")
        .join(digest(&config.url))
        .join(name);
    fs::create_dir_all(lock_path.parent().unwrap()).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    action(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_id_matches_common_git_remote_spellings() {
        let expected = normalize_remote("https://github.com/yanggf8/claudecat.git");
        assert_eq!(
            normalize_remote("git@github.com:yanggf8/claudecat.git"),
            expected
        );
        assert_eq!(
            normalize_remote("ssh://git@github.com/yanggf8/claudecat"),
            expected
        );
    }
}
