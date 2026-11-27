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

/// 4.6: `gate usage` reports the average cores of a run, `cpu_secs / wall_secs`, per group; for
/// Gradle also those of the whole family, from `gradle_family_cpu_secs`. A run that reports no CPU
/// time, or no wall time to divide by, is left out of the ratios and still counted.
#[test]
fn gate_usage_reports_average_cores() {
    let f = Fixture::new();
    let t = now();
    let line = |tool: &str, class: &str, wall: f64, cpu: Option<f64>, family_cpu: Option<f64>| {
        let mut row = serde_json::json!({
            "ts": t - 10, "session": null, "toolchain": tool, "class": class, "kind": "test",
            "wall_secs": wall, "peak_rss_mb": 100.0, "cpu_secs": cpu, "exit": 0,
            "start": {}, "end": {}
        });
        if let Some(cpu) = family_cpu {
            row["gradle_family_cpu_secs"] = cpu.into();
        }
        row.to_string()
    };
    // Cores 2, 2 and 5; the family's 10, 3 and 7.5. A wall of 0 and a missing cpu_secs divide
    // nothing.
    let lines = [
        line("gradle", "test", 10.0, Some(20.0), Some(100.0)),
        line("gradle", "test", 20.0, Some(40.0), Some(60.0)),
        line("gradle", "test", 40.0, Some(200.0), Some(300.0)),
        line("gradle", "test", 0.0, Some(5.0), Some(5.0)),
        line("gradle", "build", 5.0, None, None),
        line("cargo", "build", 4.0, Some(6.0), None),
    ];
    fs::create_dir_all(f.temp.path().join("state")).expect("state");
    fs::write(f.usage_log(), lines.join("\n")).expect("write log");
    let out = f.run(&["gate", "usage", "--json", "--days", "7"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("usage json");
    let groups = value["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 3, "{value}");
    let group = |tool: &str, class: &str| {
        groups
            .iter()
            .find(|g| g["toolchain"] == tool && g["class"] == class)
            .cloned()
            .unwrap_or_else(|| panic!("{tool} {class}: {value}"))
    };
    let test = group("gradle", "test");
    assert_eq!(
        test["avg_cores"],
        serde_json::json!({"mean": 3.0, "p50": 2.0, "p90": 5.0, "max": 5.0}),
        "{test}"
    );
    assert_eq!(
        test["gradle_family_avg_cores"],
        serde_json::json!({"mean": 6.83, "p50": 7.5, "p90": 10.0, "max": 10.0}),
        "{test}"
    );
    assert_eq!(
        test["count"], 4,
        "the run without wall time is still counted"
    );
    assert_eq!(
        test["cpu_secs_mean"], 66.25,
        "unchanged: CPU seconds per run"
    );
    let cargo = group("cargo", "build");
    assert_eq!(
        cargo["avg_cores"],
        serde_json::json!({"mean": 1.5, "p50": 1.5, "p90": 1.5, "max": 1.5}),
        "{cargo}"
    );
    // Not Gradle: the family is not measured, as for `gradle_family_rss_mb`.
    assert_eq!(
        cargo["gradle_family_avg_cores"],
        serde_json::json!({"mean": null, "p50": null, "p90": null, "max": null}),
        "{cargo}"
    );
    let gradle_build = group("gradle", "build");
    assert_eq!(gradle_build["avg_cores"]["mean"], serde_json::Value::Null);
    assert_eq!(gradle_build["count"], 1);

    let text = String::from_utf8_lossy(&f.run(&["gate", "usage"]).stdout).into_owned();
    let header = text.lines().next().unwrap();
    assert!(header.contains("cores_mean"), "{text}");
    let line = |tool: &str, class: &str| {
        text.lines()
            .skip(1)
            .find(|l| {
                let words: Vec<&str> = l.split_whitespace().collect();
                words.len() >= 2 && words[0] == tool && words[1] == class
            })
            .unwrap_or_else(|| panic!("{tool} {class}: {text}"))
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(line("gradle", "test").last().unwrap(), "3.00");
    assert_eq!(line("cargo", "build").last().unwrap(), "1.50");
    assert_eq!(line("gradle", "build").last().unwrap(), "-");
}

/// 4.6: a Gradle run's usage row measures the CPU of the whole Gradle family; no other run's does.
#[test]
fn gradle_runs_measure_the_family_cpu_and_no_other_run_does() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let gradlew = f.temp.path().join("gradlew");
    fs::write(&gradlew, "#!/bin/sh\nsleep 0.3\n").expect("gradlew");
    fs::set_permissions(&gradlew, fs::Permissions::from_mode(0o755)).expect("mode");
    let run = |args: &[&str]| {
        f.cmd(args)
            // Sample the real /proc; the gate's own figures still come from the fixture.
            .env("CLAUDECAT_USAGE_PID_ROOT", "/proc")
            .output()
            .expect("claudecat")
    };
    let out = run(&[
        "gate",
        "run",
        "--kind",
        "test",
        "--",
        gradlew.to_str().expect("path"),
        "test",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let logged = rows(&f.usage_log());
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["toolchain"], "gradle");
    assert!(logged[0]["gradle_family_rss_mb"].is_number(), "{logged:?}");
    assert!(
        logged[0]["gradle_family_cpu_secs"].is_number(),
        "{}",
        logged[0]
    );
    let out = run(&[
        "gate",
        "run",
        "--kind",
        "test",
        "--",
        "sh",
        "-c",
        "sleep 0.1",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let logged = rows(&f.usage_log());
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[1]["toolchain"], "unknown");
    assert!(
        logged[1].get("gradle_family_cpu_secs").is_none(),
        "{}",
        logged[1]
    );
}
