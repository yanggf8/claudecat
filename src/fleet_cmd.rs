//! The `claudecat fleet` subcommands.
use crate::fleet_store::{resolve_config, Config, Store, UreqHttp};
use crate::fleet_view::{pick, render_table, summarize, Pool, Summary};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// Gate and status push at most this often (seconds).
pub const AUTOPUSH_INTERVAL_SECS: u64 = 60;
/// HTTP timeout for the automatic background push.
pub const AUTOPUSH_TIMEOUT_SECS: u64 = 2;

/// Default HTTP timeout for an explicit `fleet` command.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

fn config() -> Result<Config, String> {
    resolve_config(&|k| std::env::var(k).ok())
}

fn fail(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    1
}

/// Collect this machine's snapshot and upsert its row.
pub fn push(timeout_secs: u64) -> i32 {
    let config = match config() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let snap = crate::fleet::collect();
    let store = Store {
        config,
        http: &UreqHttp,
        timeout: Duration::from_secs(timeout_secs.max(1)),
    };
    match store.push(&snap) {
        Ok(()) => 0,
        Err(e) => fail(&format!("fleet push: {e}")),
    }
}

fn summaries() -> Result<Vec<Summary>, String> {
    let config = config()?;
    let store = Store {
        config,
        http: &UreqHttp,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    };
    let now = crate::gate::unix_now();
    let rows = store.list().map_err(|e| format!("fleet: {e}"))?;
    Ok(rows.iter().map(|r| summarize(r, now)).collect())
}

/// Every machine's latest snapshot: a table, or JSON.
pub fn show(json: bool) -> i32 {
    let rows = match summaries() {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
        );
    } else if rows.is_empty() {
        return fail(
            "fleet: no machine has pushed yet; run `claudecat fleet push` on each machine",
        );
    } else {
        print!("{}", render_table(&rows));
    }
    0
}

/// Hosts ordered by fitness for a new heavy job; exit 1 when none is fit.
pub fn pick_cmd(pool: Pool, json: bool) -> i32 {
    let rows = match summaries() {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let fit = pick(&rows, pool);
    if fit.is_empty() {
        return fail(
            "fleet pick: no eligible host (every host is unknown, blocked or has no free slot)",
        );
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&fit).unwrap_or_else(|_| "[]".into())
        );
    } else {
        for p in &fit {
            println!(
                "{} free_slots={} mem_available_mb={} load={:.2}/{:.2}",
                p.host,
                p.free_slots.map_or("-".to_owned(), |free| free.to_string()),
                p.mem_available_mb,
                p.load_1m,
                p.max_load
            );
        }
    }
    0
}

/// Whether the last push (unix seconds, from the stamp file) is old enough to push again. A stamp
/// in the future (clock change) counts as due, so it can never block pushing forever.
pub fn due(last: Option<u64>, now: u64) -> bool {
    last.is_none_or(|t| t > now || now - t >= AUTOPUSH_INTERVAL_SECS)
}

/// The stamp file recording when this machine last started an automatic push.
pub fn stamp_path() -> PathBuf {
    crate::data_dir::data_dir().join("fleet-last-push")
}

/// Best-effort refresh of this machine's fleet row from `gate` and `status`: at most once per
/// `AUTOPUSH_INTERVAL_SECS`, in a detached child so the caller never waits and never fails.
pub fn maybe_push_background() {
    // A fake /proc means a test fixture: the snapshot would describe the fixture, not the machine.
    if std::env::var_os("CLAUDECAT_PROC_ROOT").is_some()
        || std::env::var("CLAUDECAT_FLEET_AUTOPUSH").is_ok_and(|v| v == "0")
    {
        return;
    }
    let stamp = stamp_path();
    let now = crate::gate::unix_now();
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    // No config means this machine is not part of the fleet: do not even fork.
    if !due(last, now) || config().is_err() {
        return;
    }
    // Stamp before spawning, so a failing push is retried a minute later, not on every command.
    let _ = crate::data_dir::write_atomic(&stamp, &format!("{now}\n"));
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let spawned = std::process::Command::new(exe)
        .args([
            "fleet",
            "push",
            "--timeout-secs",
            &AUTOPUSH_TIMEOUT_SECS.to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        // Reap it without making the caller wait.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_when_never_pushed_or_old_enough() {
        assert!(due(None, 100));
        assert!(due(Some(40), 100)); // exactly 60 s ago
        assert!(!due(Some(41), 100)); // 59 s ago
    }

    #[test]
    fn a_stamp_in_the_future_is_due() {
        assert!(due(Some(200), 100));
    }
}
