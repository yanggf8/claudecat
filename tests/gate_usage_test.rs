use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let proc = temp.path().join("proc");
        fs::create_dir_all(proc.join("pressure")).expect("proc");
        fs::write(proc.join("loadavg"), "0.50 0 0 1/1 1\n").expect("loadavg");
        fs::write(
            proc.join("meminfo"),
            "MemAvailable: 4096000 kB\nSwapTotal: 2097152 kB\nSwapFree: 2097152 kB\n",
        )
        .expect("meminfo");
        fs::write(
            proc.join("pressure/memory"),
            "some avg10=0.25 avg60=0.00 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1\n",
        )
        .expect("pressure");
        Self { temp }
    }

    fn usage_log(&self) -> PathBuf {
        self.temp.path().join("state/gate-usage.jsonl")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_claudecat"));
        cmd.args(args)
            .env("CLAUDECAT_PROC_ROOT", self.temp.path().join("proc"))
            .env("CLAUDECAT_GATE_DIR", self.temp.path().join("gate"))
            .env(
                "CLAUDECAT_GATE_LOG",
                self.temp.path().join("state/gate-denies.jsonl"),
            )
            .env("CLAUDECAT_GATE_USAGE_LOG", self.usage_log())
            .env_remove("CLAUDECAT_MAX_LOAD")
            .env_remove("CLAUDECAT_MAX_MEM_PRESSURE")
            .env_remove("CLAUDECAT_MIN_AVAILABLE_MB")
            .env_remove("CLAUDECAT_SLOTS")
            .env_remove("CLAUDECAT_OTHER_SLOTS");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("claudecat")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn rows(path: &PathBuf) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("jsonl row"))
        .collect()
}

#[test]
fn gate_run_writes_one_usage_row_with_peak_rss() {
    let f = Fixture::new();
    let out = f
        .cmd(&[
            "gate",
            "run",
            "--kind",
            "test",
            "--",
            "sh",
            "-c",
            "sleep 0.3",
        ])
        // Sample the real /proc; the gate's own figures still come from the fixture.
        .env("CLAUDECAT_USAGE_PID_ROOT", "/proc")
        .output()
        .expect("claudecat");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let rows = rows(&f.usage_log());
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["kind"], "test");
    assert_eq!(row["toolchain"], "unknown");
    assert_eq!(row["class"], "other");
    assert_eq!(row["exit"], 0);
    assert!(row["session"].is_null());
    assert!(row["wall_secs"].as_f64().expect("wall") >= 0.25);
    assert!(row["peak_rss_mb"].as_f64().expect("peak rss present") > 0.0);
    assert!(row["cpu_secs"].as_f64().is_some());
    for side in ["start", "end"] {
        assert_eq!(row[side]["mem_available_mb"], 4000);
        assert_eq!(row[side]["psi_some_avg10"], 0.25);
        assert_eq!(row[side]["load_1m"], 0.5);
        assert_eq!(row[side]["swap_used_mb"], 0);
    }
}

#[test]
fn gate_run_keeps_exit_code() {
    let f = Fixture::new();
    let out = f.run(&["gate", "run", "--", "sh", "-c", "exit 7"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let rows = rows(&f.usage_log());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["exit"], 7);
    assert_eq!(rows[0]["kind"], "other");
}

#[test]
fn gate_run_survives_missing_proc_for_sampling() {
    let f = Fixture::new();
    // The fixture proc has no pid directories: sampling finds nothing, the run is unaffected.
    let out = f.run(&["gate", "run", "--", "sh", "-c", "sleep 0.2"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let rows = rows(&f.usage_log());
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["peak_rss_mb"].is_null());
    assert!(rows[0]["cpu_secs"].as_f64().is_some());
}

#[test]
fn gate_usage_json_summarizes_fixture_log() {
    let f = Fixture::new();
    let t = now();
    let mut lines = Vec::new();
    let mut add = |ts: u64, tool: &str, class: &str, rss: f64, wall: f64, cpu: f64| {
        lines.push(
            serde_json::json!({
                "ts": ts, "session": null, "toolchain": tool, "class": class, "kind": "test",
                "wall_secs": wall, "peak_rss_mb": rss, "cpu_secs": cpu, "exit": 0,
                "start": {}, "end": {}
            })
            .to_string(),
        );
    };
    for (i, rss) in [100.0, 200.0, 300.0, 400.0, 500.0].iter().enumerate() {
        add(
            t - 10,
            "gradle",
            "test",
            *rss,
            10.0 * (i as f64 + 1.0),
            20.0,
        );
    }
    add(t - 10, "cargo", "build", 50.0, 3.0, 4.0);
    // Outside the window: must be ignored.
    add(t - 30 * 86400, "cargo", "build", 9999.0, 1.0, 1.0);
    fs::create_dir_all(f.temp.path().join("state")).expect("state");
    fs::write(f.usage_log(), lines.join("\n") + "\nnot json\n").expect("write log");

    let out = f.run(&["gate", "usage", "--json", "--days", "7"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("usage json");
    let groups = value["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2, "{value}");
    let cargo = &groups[0];
    assert_eq!(
        (cargo["toolchain"].as_str(), cargo["class"].as_str()),
        (Some("cargo"), Some("build"))
    );
    assert_eq!(cargo["count"], 1);
    assert_eq!(cargo["peak_rss_mb"]["max"], 50.0);
    let gradle = &groups[1];
    assert_eq!(gradle["count"], 5);
    assert_eq!(gradle["peak_rss_mb"]["p50"], 300.0);
    assert_eq!(gradle["peak_rss_mb"]["p90"], 500.0);
    assert_eq!(gradle["peak_rss_mb"]["max"], 500.0);
    assert_eq!(gradle["wall_secs"]["p50"], 30.0);
    assert_eq!(gradle["wall_secs"]["p90"], 50.0);
    assert_eq!(gradle["cpu_secs_mean"], 20.0);

    let text = f.run(&["gate", "usage"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&text.stdout).contains("gradle"));
}

#[test]
fn gate_usage_with_no_log_is_empty_not_an_error() {
    let f = Fixture::new();
    let out = f.run(&["gate", "usage", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["groups"].as_array().map(Vec::len), Some(0));
}

#[test]
fn deny_rows_carry_system_context() {
    let f = Fixture::new();
    // Force a load deny through the hook, then read the context off the logged row.
    let payload = serde_json::json!({
        "session_id": "s1",
        "cwd": f.temp.path(),
        "tool_input": {"command": "cargo build"}
    })
    .to_string();
    let mut child = f
        .cmd(&["gate", "hook"])
        .env("CLAUDECAT_MAX_LOAD", "0.1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    use std::io::Write;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("payload");
    let out = child.wait_with_output().expect("hook");
    assert_eq!(out.status.code(), Some(75), "{out:?}");
    let denies = rows(&f.temp.path().join("state/gate-denies.jsonl"));
    assert_eq!(denies.len(), 1);
    assert_eq!(denies[0]["mem_available_mb"], 4000);
    assert_eq!(denies[0]["psi_some_avg10"], 0.25);
    assert_eq!(denies[0]["load_1m"], 0.5);
}
