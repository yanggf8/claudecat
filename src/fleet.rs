//! Fleet snapshot: what one machine reports about itself so a coordinator can pick where to
//! dispatch work. Holds the same data as `status --json` plus the running Claude Code sessions.
//! Nothing secret is collected: no environment, no command-line arguments, no file contents.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

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

/// UTC RFC 3339, whole seconds.
pub fn rfc3339_utc(_secs: u64) -> String {
    String::new()
}

/// Average CPU use over a sample: tick delta at `clk_tck` ticks per second over `secs`.
pub fn cpu_percent(_delta_ticks: u64, _secs: f64) -> f64 {
    -1.0
}

/// The tmux session whose pane process is `pid` or one of its ancestors.
pub fn tmux_session_for(
    _pid: u32,
    _ppids: &BTreeMap<u32, u32>,
    _panes: &[(u32, String)],
) -> Option<String> {
    None
}

/// Scan a /proc-shaped directory for this uid's `claude` processes, sampling CPU for
/// `sample_ms` milliseconds.
pub fn scan_sessions(_root: &Path, _uid: u32, _sample_ms: u64) -> Vec<Session> {
    Vec::new()
}

/// Put a snapshot together from its parts; `sessions` None means scanning is unsupported here.
pub fn assemble(
    _host: &str,
    _os: &str,
    _now_secs: u64,
    _status: Result<serde_json::Value, String>,
    _sessions: Option<Vec<Session>>,
) -> FleetSnapshot {
    FleetSnapshot {
        host: String::new(),
        os: String::new(),
        collected_at: String::new(),
        claudecat_version: String::new(),
        status: None,
        status_error: None,
        sessions: Vec::new(),
        sessions_unsupported: false,
    }
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
            fs::metadata(&root.join("100")).unwrap().uid()
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
