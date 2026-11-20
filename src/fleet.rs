//! Fleet snapshot: what one machine reports about itself so a coordinator can pick where to
//! dispatch work. Holds the same data as `status --json` plus the running Claude Code sessions.
//! Nothing secret is collected: no environment, no command-line arguments, no file contents.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One running `claude` process. Only these fields ever leave the machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Session {
    pub pid: u32,
    pub cwd: String,
    pub cpu_percent: f64,
    pub elapsed_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FleetSnapshot {
    pub host: String,
    pub os: String,
    pub collected_at: String,
    pub claudecat_version: String,
    /// The `status --json` document; None when the machine's resources could not be read.
    pub status: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    pub sessions: Vec<Session>,
    /// True where session scanning is not implemented (anything but Linux).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sessions_unsupported: bool,
}

/// Kernel clock ticks per second; 100 on every Linux target we run (USER_HZ is fixed by the ABI).
const CLK_TCK: f64 = 100.0;

/// UTC RFC 3339, whole seconds.
pub fn rfc3339_utc(secs: u64) -> String {
    let (y, m, d) = crate::dates::civil_from_days(i64::try_from(secs / 86400).unwrap_or(0));
    let (hh, mm, ss) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Average CPU use over a sample: tick delta at `CLK_TCK` ticks per second over `secs`.
#[allow(clippy::cast_precision_loss)]
pub fn cpu_percent(delta_ticks: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    delta_ticks as f64 / CLK_TCK / secs * 100.0
}

/// The tmux session whose pane process is `pid` or one of its ancestors.
pub fn tmux_session_for(
    pid: u32,
    ppids: &BTreeMap<u32, u32>,
    panes: &[(u32, String)],
) -> Option<String> {
    let mut cur = pid;
    // The depth cap also ends a ppid cycle in a corrupt table.
    for _ in 0..64 {
        if let Some((_, name)) = panes.iter().find(|(p, _)| *p == cur) {
            return Some(name.clone());
        }
        cur = *ppids.get(&cur)?;
        if cur == 0 {
            return None;
        }
    }
    None
}

/// Fields of /proc/<pid>/stat after the `(comm)` part: state, ppid, ... utime, stime, ... starttime.
fn stat_fields(dir: &Path) -> Option<Vec<String>> {
    let stat = fs::read_to_string(dir.join("stat")).ok()?;
    let (_, tail) = stat.rsplit_once(") ")?;
    Some(tail.split_whitespace().map(str::to_owned).collect())
}

fn cpu_ticks(fields: &[String]) -> Option<u64> {
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    utime.checked_add(stime)
}

/// pid -> parent pid for every process under `root`.
pub fn ppid_map(root: &Path) -> BTreeMap<u32, u32> {
    let mut map = BTreeMap::new();
    let Ok(entries) = fs::read_dir(root) else {
        return map;
    };
    for item in entries.flatten() {
        let Some(pid) = item
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(ppid) =
            stat_fields(&item.path()).and_then(|f| f.get(1).and_then(|s| s.parse::<u32>().ok()))
        {
            map.insert(pid, ppid);
        }
    }
    map
}

/// Scan a /proc-shaped directory for this uid's `claude` processes, sampling CPU for
/// `sample_ms` milliseconds. Reads only comm, cwd, stat and uptime: never cmdline or environ.
pub fn scan_sessions(root: &Path, uid: u32, sample_ms: u64) -> Vec<Session> {
    let mut found: Vec<(u32, String, u64, u64)> = Vec::new(); // pid, cwd, ticks, start
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    for item in entries.flatten() {
        let Some(pid) = item
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = item.path();
        if fs::metadata(&dir).map(|m| m.uid()).ok() != Some(uid) {
            continue;
        }
        if fs::read_to_string(dir.join("comm"))
            .map(|c| c.trim() != "claude")
            .unwrap_or(true)
        {
            continue;
        }
        let Some(fields) = stat_fields(&dir) else {
            continue;
        };
        let (Some(ticks), Some(start)) = (
            cpu_ticks(&fields),
            fields.get(19).and_then(|s| s.parse::<u64>().ok()),
        ) else {
            continue;
        };
        let cwd = fs::read_link(dir.join("cwd"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "?".into());
        found.push((pid, cwd, ticks, start));
    }
    let started = Instant::now();
    if sample_ms > 0 && !found.is_empty() {
        std::thread::sleep(Duration::from_millis(sample_ms));
    }
    let wall = started.elapsed().as_secs_f64();
    let uptime = fs::read_to_string(root.join("uptime"))
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    found.sort_by_key(|f| f.0);
    found
        .into_iter()
        .map(|(pid, cwd, ticks, start)| {
            // A pid that exited or was reused during the sample reports 0%.
            let later = fs::read_to_string(root.join(pid.to_string()).join("stat"))
                .ok()
                .and_then(|_| stat_fields(&root.join(pid.to_string())))
                .filter(|f| f.get(19).and_then(|s| s.parse::<u64>().ok()) == Some(start))
                .and_then(|f| cpu_ticks(&f));
            let cpu = later.map_or(0.0, |t| cpu_percent(t.saturating_sub(ticks), wall));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let elapsed = (uptime - start as f64 / CLK_TCK).max(0.0) as u64;
            Session {
                pid,
                cwd,
                cpu_percent: (cpu * 10.0).round() / 10.0,
                elapsed_secs: elapsed,
                tmux: None,
            }
        })
        .collect()
}

/// Put a snapshot together from its parts; `sessions` None means scanning is unsupported here.
pub fn assemble(
    host: &str,
    os: &str,
    now_secs: u64,
    status: Result<serde_json::Value, String>,
    sessions: Option<Vec<Session>>,
) -> FleetSnapshot {
    let (status, status_error) = match status {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    FleetSnapshot {
        host: host.to_owned(),
        os: os.to_owned(),
        collected_at: rfc3339_utc(now_secs),
        claudecat_version: env!("CARGO_PKG_VERSION").to_owned(),
        status,
        status_error,
        sessions_unsupported: sessions.is_none(),
        sessions: sessions.unwrap_or_default(),
    }
}

/// tmux's pane pid -> session name table; empty when tmux is absent or has no server.
fn tmux_panes() -> Vec<(u32, String)> {
    let Ok(out) = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_pid} #{session_name}"])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, name) = l.split_once(' ')?;
            Some((pid.parse().ok()?, name.to_owned()))
        })
        .collect()
}

/// Collect this machine's snapshot now.
pub fn collect() -> FleetSnapshot {
    let host = crate::host::resolve_host().unwrap_or_else(|| "unknown".into());
    let status = crate::gate::status_value().map_err(|e| e.to_string());
    let sessions = if cfg!(target_os = "linux") {
        let root = Path::new("/proc");
        let uid = crate::gate::own_uid().unwrap_or(u32::MAX);
        let mut sessions = scan_sessions(root, uid, 300);
        if !sessions.is_empty() {
            let (ppids, panes) = (ppid_map(root), tmux_panes());
            for s in &mut sessions {
                s.tmux = tmux_session_for(s.pid, &ppids, &panes);
            }
        }
        Some(sessions)
    } else {
        None
    };
    assemble(
        &host,
        std::env::consts::OS,
        crate::gate::unix_now(),
        status,
        sessions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rfc3339_formats_utc() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_795_169_472), "2026-11-20T10:11:12Z");
        assert_eq!(rfc3339_utc(1_709_251_199), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn cpu_percent_is_ticks_over_wall_time() {
        // 100 ticks/s: 30 ticks in 0.5 s is 60% of one CPU.
        assert!((cpu_percent(30, 0.5) - 60.0).abs() < 1e-9);
        assert_eq!(cpu_percent(5, 0.0), 0.0);
    }

    #[test]
    fn tmux_session_found_through_ancestors() {
        let ppids: BTreeMap<u32, u32> = [(30, 20), (20, 10), (10, 1)].into();
        let panes = vec![(10, "bsrnuc".to_owned()), (99, "other".to_owned())];
        assert_eq!(
            tmux_session_for(30, &ppids, &panes).as_deref(),
            Some("bsrnuc")
        );
        assert_eq!(
            tmux_session_for(10, &ppids, &panes).as_deref(),
            Some("bsrnuc")
        );
        assert_eq!(tmux_session_for(1, &ppids, &panes), None);
    }

    #[test]
    fn tmux_ancestor_loop_terminates() {
        let ppids: BTreeMap<u32, u32> = [(2, 3), (3, 2)].into();
        assert_eq!(tmux_session_for(2, &ppids, &[]), None);
    }

    fn proc_entry(
        root: &Path,
        pid: u32,
        comm: &str,
        args: &[&str],
        uid_dir_cwd: &Path,
        start: u64,
    ) {
        let dir = root.join(pid.to_string());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        fs::write(dir.join("cmdline"), args.join("\0")).unwrap();
        fs::write(dir.join("environ"), "ANTHROPIC_API_KEY=SECRET-ENV\0").unwrap();
        std::os::unix::fs::symlink(uid_dir_cwd, dir.join("cwd")).unwrap();
        // fields after "(comm) ": state ppid ... utime(14) stime(15) ... starttime(22)
        let tail: Vec<String> = (3..=22)
            .map(|n| match n {
                4 => "1".to_owned(),
                14 => "10".to_owned(),
                15 => "5".to_owned(),
                22 => start.to_string(),
                _ => "0".to_owned(),
            })
            .collect();
        fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) S {}\n", tail[1..].join(" ")),
        )
        .unwrap();
    }

    #[test]
    fn scan_finds_only_claude_and_leaks_nothing() {
        let t = tempfile::tempdir().unwrap();
        let work = t.path().join("work");
        fs::create_dir(&work).unwrap();
        let root = t.path().join("proc");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("uptime"), "1000.00 500.00\n").unwrap();
        proc_entry(
            &root,
            100,
            "claude",
            &["claude", "--token", "SECRET-ARG"],
            &work,
            50_000,
        );
        proc_entry(&root, 101, "node", &["node", "x.js"], &work, 50_000);
        let own_uid = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(root.join("100")).unwrap().uid()
        };
        let found = scan_sessions(&root, own_uid, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, 100);
        assert_eq!(found[0].cwd, work.to_string_lossy());
        assert_eq!(found[0].elapsed_secs, 500);
        assert_eq!(found[0].tmux, None);
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("SECRET"), "{json}");
        assert!(!json.contains("--token"), "{json}");
    }

    #[test]
    fn scan_skips_other_users_processes() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("proc");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("uptime"), "1000.00 500.00\n").unwrap();
        proc_entry(&root, 100, "claude", &["claude"], t.path(), 50_000);
        assert!(scan_sessions(&root, u32::MAX, 0).is_empty());
    }

    #[test]
    fn assemble_marks_unsupported_and_unreadable() {
        let s = assemble("h1", "macos", 0, Err("no /proc".into()), None);
        assert_eq!(s.host, "h1");
        assert_eq!(s.os, "macos");
        assert_eq!(s.collected_at, "1970-01-01T00:00:00Z");
        assert_eq!(s.claudecat_version, env!("CARGO_PKG_VERSION"));
        assert!(s.status.is_none());
        assert_eq!(s.status_error.as_deref(), Some("no /proc"));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["sessions_unsupported"], true);
        assert_eq!(v["sessions"], serde_json::json!([]));
    }

    #[test]
    fn assemble_linux_omits_unsupported_flag_and_keeps_status() {
        let status = serde_json::json!({"load_1m": 0.5});
        let s = assemble("h", "linux", 0, Ok(status.clone()), Some(vec![]));
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("sessions_unsupported").is_none());
        assert!(v.get("status_error").is_none());
        assert_eq!(v["status"], status);
    }
}
