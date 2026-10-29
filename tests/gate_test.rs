use claudecat::gate::{classify, Classification, Kind};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::symlink;
use std::process::{Command, Output, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new(load: &str, swap_used_kb: u64) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let proc = temp.path().join("proc");
        fs::create_dir(&proc).expect("proc");
        fs::write(proc.join("loadavg"), format!("{load} 0 0 1/1 1\n")).expect("loadavg");
        fs::write(
            proc.join("meminfo"),
            format!(
                "MemAvailable: 4096000 kB\nSwapTotal: 2097152 kB\nSwapFree: {} kB\n",
                2097152 - swap_used_kb
            ),
        )
        .expect("meminfo");
        Self { temp }
    }

    fn proc(&self) -> std::path::PathBuf {
        self.temp.path().join("proc")
    }
    fn gate(&self) -> std::path::PathBuf {
        self.temp.path().join("gate")
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_claudecat"));
        cmd.args(args)
            .env("CLAUDECAT_PROC_ROOT", self.proc())
            .env("CLAUDECAT_GATE_DIR", self.gate())
            .env(
                "CLAUDECAT_GATE_LOG",
                self.temp.path().join("state/gate-denies.jsonl"),
            )
            .env_remove("CLAUDECAT_MAX_LOAD")
            .env_remove("CLAUDECAT_MAX_SWAP_MB")
            .env_remove("CLAUDECAT_SLOTS")
            .env_remove("CARGO_TARGET_DIR");
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().expect("claudecat")
    }
    fn process(&self, pid: u32, name: &str, args: &[&str], ppid: u32) {
        let dir = self.proc().join(pid.to_string());
        fs::create_dir(&dir).expect("pid dir");
        fs::write(dir.join("comm"), format!("{name}\n")).expect("comm");
        fs::write(dir.join("cmdline"), args.join("\0") + "\0").expect("cmdline");
        fs::write(dir.join("stat"), format!("{pid} ({name}) S {ppid} 0 0\n")).expect("stat");
        symlink(self.temp.path(), dir.join("cwd")).expect("cwd");
    }
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code")
}

#[test]
fn classify_shell_commands() {
    use Classification::{Heavy, NotHeavy, Unparsed};
    let cases = [
        ("cd x && cargo test", Heavy(Kind::Test)),
        (
            "RUST_TEST_THREADS=2 nice -n 15 cargo test",
            Heavy(Kind::Test),
        ),
        ("cargo +stable build", Heavy(Kind::Build)),
        ("/usr/bin/cargo build", Heavy(Kind::Build)),
        ("command cargo build", Heavy(Kind::Build)),
        ("claudecat gate run -- cargo build", NotHeavy),
        (
            "claudecat gate run -- cargo build && cargo test",
            Heavy(Kind::Test),
        ),
        ("echo cargo build", NotHeavy),
        ("git commit -m \"cargo test\"", NotHeavy),
        ("git commit -m \"docs; cargo build; why\"", NotHeavy),
        ("echo \"unterminated", Unparsed),
        ("cargo fmt --all", NotHeavy),
        (
            "timeout -s TERM 30s env A=b ./gradlew test",
            Heavy(Kind::Build),
        ),
        ("exec cargo -C dir --config foo test", Heavy(Kind::Test)),
        ("echo 'x && cargo test'; cargo build", Heavy(Kind::Build)),
        ("echo cargo\\;test", NotHeavy),
        ("cargo build | cargo bench", Heavy(Kind::Test)),
        ("cargo build\ncargo test", Heavy(Kind::Test)),
        ("~/.cargo/bin/cargo check", Heavy(Kind::Build)),
        (
            "cat > p.md <<'EOF'\nnice -n 15 cargo test\nEOF\necho done",
            NotHeavy,
        ),
        ("cat <<EOF\ncargo build\nEOF\ncargo test", Heavy(Kind::Test)),
        ("cat <<-\"END\"\n\tcargo build\n\tEND", NotHeavy),
        ("cat <<EOF\ncargo build", NotHeavy),
        ("grep x <<< \"cargo build\"", NotHeavy),
        ("cargo test <<EOF\ninput\nEOF", Heavy(Kind::Test)),
        ("echo ok # && cargo test", NotHeavy),
        ("# cargo build\necho ok", NotHeavy),
        ("echo a#b && cargo test", Heavy(Kind::Test)),
        ("cat <<EOF\n\tEOF\ncargo test\nEOF", NotHeavy),
        ("cat <<EOF\nEOF \ncargo test\nEOF", NotHeavy),
        ("cargo --color always test", Heavy(Kind::Test)),
        ("cargo -Z build-std build", Heavy(Kind::Build)),
        ("2>/tmp/cargo.log cargo test", Heavy(Kind::Test)),
        ("> log cargo build", Heavy(Kind::Build)),
        ("cargo test 2>&1 | tail -5", Heavy(Kind::Test)),
        ("echo hi >&2; echo done", NotHeavy),
    ];
    for (command, expected) in cases {
        assert_eq!(classify(command), expected, "{command}");
    }
}

fn hook(f: &Fixture, stdin: &str) -> Output {
    let mut child = f
        .cmd(&["gate", "hook"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .expect("payload");
    child.wait_with_output().expect("hook output")
}

fn logged_hook(f: &Fixture, session: Option<&str>, command: &str) -> Output {
    let payload = serde_json::json!({"session_id":session,"cwd":f.temp.path(),"tool_input":{"command":command}});
    hook(f, &payload.to_string())
}

fn log_path(f: &Fixture) -> std::path::PathBuf {
    f.temp.path().join("state/gate-denies.jsonl")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn deny_log_retries_sessions_reset_and_privacy() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("7", 0);
    for (session, expected) in [
        (Some("secret-session"), None),
        (Some("secret-session"), Some("retry 2")),
        (Some("secret-session"), Some("retry 3")),
        (Some("other-session"), None),
        (None, None),
    ] {
        let out = logged_hook(&f, session, "cargo test --secret-token=raw-secret");
        assert_eq!(code(&out), 75);
        let line = String::from_utf8(out.stdout).unwrap();
        assert_eq!(line.lines().count(), 1);
        assert_eq!(expected.map(|s| line.contains(s)), expected.map(|_| true));
        if expected.is_none() {
            assert!(!line.contains("retry 2"));
        }
        if expected == Some("retry 3") {
            assert!(line.contains("Stop retrying"));
        }
    }
    let path = log_path(&f);
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let data = fs::read_to_string(&path).unwrap();
    assert!(!data.contains("raw-secret"));
    assert!(!data.contains("secret-session"));
    assert!(!data.contains(f.temp.path().to_str().unwrap()));
    assert_eq!(data.lines().count(), 5);
    fs::write(f.proc().join("loadavg"), "0.5 0 0 1/1 1\n").unwrap();
    assert_eq!(
        code(&logged_hook(
            &f,
            Some("secret-session"),
            "cargo test --secret-token=raw-secret"
        )),
        0
    );
    fs::write(f.proc().join("loadavg"), "7 0 0 1/1 1\n").unwrap();
    let reset = logged_hook(
        &f,
        Some("secret-session"),
        "cargo test --secret-token=raw-secret",
    );
    assert!(!String::from_utf8_lossy(&reset.stdout).contains("retry 2"));
}

#[test]
fn deny_log_old_garbage_unwritable_and_gradle() {
    let f = Fixture::new("7", 0);
    assert_eq!(
        code(&logged_hook(&f, Some("secret-session"), "cargo test")),
        75
    );
    let first = fs::read_to_string(log_path(&f)).unwrap();
    fs::write(log_path(&f), format!("{first}not json\n")).unwrap();
    let counted = logged_hook(&f, Some("secret-session"), "cargo test");
    assert!(String::from_utf8_lossy(&counted.stdout).contains("retry 2"));
    let mut old: serde_json::Value = serde_json::from_str(first.trim()).unwrap();
    old["ts"] = (now_secs() - 1801).into();
    fs::write(log_path(&f), format!("{old}\nnot json\n")).unwrap();
    let out = logged_hook(&f, Some("secret-session"), "cargo test");
    assert_eq!(code(&out), 75);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("retry 2"));
    let flagged = logged_hook(
        &f,
        Some("secret-session"),
        "cargo --manifest-path=/secret/dir/Cargo.toml --color always test",
    );
    assert_eq!(code(&flagged), 75);
    let data = fs::read_to_string(log_path(&f)).unwrap();
    assert!(!data.contains("/secret/dir"));
    let row: serde_json::Value = serde_json::from_str(data.lines().last().unwrap()).unwrap();
    assert_eq!(row["sub"], "test");
    let gradle = logged_hook(&f, Some("secret-session"), "./gradlew test");
    assert_eq!(code(&gradle), 75);
    assert!(!String::from_utf8_lossy(&gradle.stdout).contains("retry"));
    let rows: Vec<serde_json::Value> = fs::read_to_string(log_path(&f))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(rows.last().unwrap()["id"], serde_json::Value::Null);
    assert_eq!(rows.last().unwrap()["sub"], "gradle");
    let gradle_again = logged_hook(&f, Some("secret-session"), "./gradlew test");
    assert!(!String::from_utf8_lossy(&gradle_again.stdout).contains("retry"));
    fs::remove_file(log_path(&f)).unwrap();
    fs::create_dir(log_path(&f)).unwrap();
    let denied = logged_hook(&f, Some("secret-session"), "cargo test");
    assert_eq!(code(&denied), 75);
    assert!(String::from_utf8_lossy(&denied.stdout).starts_with("gate denied"));
}

#[test]
fn gate_log_aggregation_and_retention() {
    let f = Fixture::new("7", 0);
    let now = now_secs();
    fs::create_dir_all(log_path(&f).parent().unwrap()).unwrap();
    let old =
        serde_json::json!({"ts":now-8*86400,"session":"a","id":"b","sub":"test","reason":"load"});
    let deny =
        serde_json::json!({"ts":now-10,"session":"a","id":"b","sub":"test","reason":"load,slots"});
    let allow = serde_json::json!({"ts":now,"session":"a","id":"b","sub":"test","allowed":true});
    fs::write(log_path(&f), format!("{old}\n{deny}\n{deny}\n{allow}\n")).unwrap();
    let out = f.run(&["gate", "log", "--json"]);
    assert_eq!(code(&out), 0);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["reasons"]["load"], 2);
    assert_eq!(value["subs"]["test"], 2);
    assert_eq!(value["retries"]["2"], 1);
    assert_eq!(value["seconds_to_allow"]["p50"], 10);
    assert!(!fs::read_to_string(log_path(&f))
        .unwrap()
        .contains(&old.to_string()));
    let text = String::from_utf8(f.run(&["gate", "log"]).stdout).unwrap();
    assert!(text.contains("load: 2"));
    assert!(text.contains("p95=10"));
}

#[test]
fn large_log_compacts_on_deny_and_allow() {
    let f = Fixture::new("7", 0);
    assert_eq!(
        code(&logged_hook(&f, Some("large-session"), "cargo test")),
        75
    );
    let row = fs::read_to_string(log_path(&f)).unwrap();
    fs::write(log_path(&f), row.repeat(140 * 1024 / row.len() + 1)).unwrap();
    let within_window = logged_hook(&f, Some("large-session"), "cargo test");
    assert_eq!(code(&within_window), 75);
    assert!(String::from_utf8_lossy(&within_window.stdout).contains("retry"));
    // Best of three refilled runs: one slow process start on a loaded runner is noise,
    // three in a row is the full-file read this guards against.
    for (load, expected) in [("7", 75), ("0.5", 0)] {
        fs::write(f.proc().join("loadavg"), format!("{load} 0 0 1/1 1\n")).unwrap();
        let fastest = (0..3)
            .map(|_| {
                fs::write(log_path(&f), row.repeat(1024 * 1024 / row.len() + 1)).unwrap();
                let start = Instant::now();
                assert_eq!(
                    code(&logged_hook(&f, Some("large-session"), "cargo test")),
                    expected
                );
                let elapsed = start.elapsed().as_millis();
                assert!(fs::metadata(log_path(&f)).unwrap().len() <= 256 * 1024);
                elapsed
            })
            .min()
            .unwrap();
        assert!(fastest < 50, "fastest of three took {fastest} ms");
    }
}

#[test]
fn hook_allows_and_denies_with_one_stdout_line() {
    let clear = Fixture::new("0.5", 0);
    let allowed = hook(&clear, r#"{"tool_input":{"command":"cargo test"}}"#);
    assert_eq!(code(&allowed), 0);
    assert!(allowed.stdout.is_empty());
    let busy = Fixture::new("7", 0);
    let denied = hook(&busy, r#"{"tool_input":{"command":"cargo test"}}"#);
    assert_eq!(code(&denied), 75);
    let line = String::from_utf8(denied.stdout).unwrap();
    assert!(line.starts_with("gate denied"));
    assert!(line.ends_with("Denied, not queued: nothing will run this command later. Retry it after the holders above finish; do not poll with sleep in the foreground.\n"));
    assert_eq!(line.lines().count(), 1);
    assert!(denied.stderr.is_empty());
    let light = hook(&busy, r#"{"tool_input":{"command":"cargo fmt --all"}}"#);
    assert_eq!(code(&light), 0);
    assert!(light.stdout.is_empty());
    let garbage = hook(&busy, "not json");
    assert_eq!(code(&garbage), 0);
    assert!(garbage.stdout.is_empty());
}

#[test]
fn hook_without_path_or_gate_env_still_uses_uid() {
    let f = Fixture::new("7", 0);
    let mut child = f
        .cmd(&["gate", "hook"])
        .env("PATH", "")
        .env_remove("CLAUDECAT_GATE_DIR")
        .env_remove("XDG_RUNTIME_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"tool_input":{"command":"cargo build"}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(code(&output), 75);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("gate denied"));
}

#[test]
fn install_and_uninstall_forward_to_cort() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("0.5", 0);
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let script = bin.join("cort");
    fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let install = f
        .cmd(&["gate", "install"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert_eq!(code(&install), 0);
    let lines: Vec<_> = std::str::from_utf8(&install.stdout)
        .unwrap()
        .lines()
        .collect();
    assert_eq!(
        lines,
        [
            "hook-gate",
            "set",
            "--",
            env!("CARGO_BIN_EXE_claudecat"),
            "gate",
            "hook"
        ]
    );
    let uninstall = f
        .cmd(&["gate", "uninstall"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert_eq!(code(&uninstall), 0);
    assert_eq!(uninstall.stdout, b"hook-gate\nclear\n");
}

#[test]
fn load_over_limit_denies() {
    let f = Fixture::new("6.01", 0);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("load"));
}

#[test]
fn swap_at_limit_denies() {
    let f = Fixture::new("0.5", 1024 * 1024);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("swap"));
}

#[test]
fn clear_resources_allow_silently() {
    let f = Fixture::new("0.5", 0);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0);
    assert!(o.stderr.is_empty());
    assert!(o.stdout.is_empty());
}

#[test]
fn build_denies_rustc_but_e2e_allows() {
    let f = Fixture::new("0.5", 0);
    f.process(123, "rustc", &["rustc", "main.rs"], 99);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "build"])), 75);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "e2e"])), 0);
}

#[test]
fn invalid_thresholds_use_defaults() {
    let f = Fixture::new("6.01", 0);
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_LOAD", "0")
        .env("CLAUDECAT_MAX_SWAP_MB", "garbage")
        .env("CLAUDECAT_SLOTS", "0")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("6.00"));
    let f = Fixture::new("0.5", 1024 * 1024);
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_LOAD", "garbage")
        .env("CLAUDECAT_MAX_SWAP_MB", "0")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
}

#[test]
fn missing_proc_fails_open() {
    let f = Fixture::new("0.5", 0);
    fs::remove_file(f.proc().join("loadavg")).expect("remove loadavg");
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0);
    assert!(String::from_utf8_lossy(&o.stderr).contains("warning"));
}

#[test]
fn occupied_slots_deny_run_without_executing() {
    let f = Fixture::new("0.5", 0);
    fs::create_dir(f.gate()).expect("gate");
    let mut locks = Vec::new();
    for i in 0..3 {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(f.gate().join(format!("slot-{i}.lock")))
            .expect("lock file");
        file.try_lock().expect("lock");
        locks.push(file);
    }
    let marker = f.temp.path().join("marker");
    let o = f
        .cmd(&[
            "gate",
            "run",
            "--",
            "touch",
            marker.to_str().expect("marker path"),
        ])
        .output()
        .expect("run");
    assert_eq!(code(&o), 75);
    assert!(!marker.exists());
    drop(locks);
}

#[test]
fn run_returns_child_codes_and_releases_slot() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "true"])), 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "false"])), 1);
    let file = File::open(f.gate().join("slot-0.lock")).expect("slot file");
    file.try_lock().expect("released lock");
}

#[test]
fn status_json_reports_orphan_broker_and_children() {
    let f = Fixture::new("0.5", 0);
    let cwd = f.temp.path().join("project");
    fs::create_dir(&cwd).expect("project");
    // WSL reparents a detached orphan to the session's `/init`, not to pid 1.
    f.process(702, "init", &["/init"], 1);
    // Node 24 renames its main thread, so comm reads `MainThread`; argv[0] is the node binary.
    f.process(
        123,
        "MainThread",
        &[
            "/usr/local/bin/node",
            "/opt/codex/scripts/app-server-broker.mjs",
            "--cwd",
            cwd.to_str().expect("cwd"),
        ],
        702,
    );
    f.process(124, "codex", &["codex", "app-server"], 123);
    let o = f.run(&["status", "--json"]);
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["orphan_brokers"][0]["pid"], 123);
    assert_eq!(v["orphan_brokers"][0]["cwd_arg"].as_str(), cwd.to_str());
    assert_eq!(v["orphan_brokers"][0]["cwd_exists"], true);
    assert_eq!(v["orphan_brokers"][0]["children"][0]["pid"], 124);
}

#[test]
fn status_ignores_broker_with_normal_parent() {
    let f = Fixture::new("0.5", 0);
    // A `nodejs` alias is still node; comm is `MainThread` either way.
    f.process(
        55,
        "MainThread",
        &[
            "/usr/bin/nodejs",
            "/opt/codex/scripts/codex-companion.mjs",
            "task-worker",
        ],
        702,
    );
    f.process(
        123,
        "MainThread",
        &[
            "/usr/local/bin/node",
            "/opt/codex/scripts/app-server-broker.mjs",
            "--cwd",
            "/nonexistent",
        ],
        55,
    );
    let o = f.run(&["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["orphan_brokers"].as_array().expect("array").len(), 0);
}

#[test]
fn gradle_worker_denies_test() {
    let f = Fixture::new("0.5", 0);
    f.process(
        123,
        "java",
        &[
            "java",
            "org.gradle.process.internal.worker.GradleWorkerMain",
        ],
        99,
    );
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "test"])), 75);
}

#[test]
fn gate_dir_error_fails_open() {
    let f = Fixture::new("0.5", 0);
    fs::write(f.gate(), "not a directory").expect("block dir");
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0);
    assert!(String::from_utf8_lossy(&o.stderr).contains("warning"));
}

#[test]
fn run_gate_dir_error_executes_command() {
    let f = Fixture::new("0.5", 0);
    fs::write(f.gate(), "not a directory").expect("block dir");
    let o = f.run(&["gate", "run", "--", "true"]);
    assert_eq!(code(&o), 0);
    assert!(String::from_utf8_lossy(&o.stderr).contains("warning"));
}

#[test]
fn run_returns_signal_code() {
    let f = Fixture::new("0.5", 0);
    let o = f.run(&["gate", "run", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(code(&o), 143);
}

#[test]
fn status_reports_locked_slot_holder() {
    let f = Fixture::new("0.5", 0);
    fs::create_dir(f.gate()).expect("gate");
    let slot = f.gate().join("slot-0.lock");
    fs::write(
        &slot,
        "{\"pid\":42,\"kind\":\"build\",\"command\":[\"cargo\",\"build\"],\"started_at\":10}\n",
    )
    .expect("holder");
    let file = File::open(&slot).expect("slot");
    file.try_lock().expect("lock");
    let o = f.run(&["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["slots"][0]["occupied"], true);
    assert_eq!(v["slots"][0]["holder"]["pid"], 42);
    assert_eq!(v["slots"][1]["occupied"], false);
}

#[test]
fn run_passes_leading_dash_arguments_to_child() {
    let f = Fixture::new("0.5", 0);
    // `nice --version` prints nice's own version and exits 0; the child never runs.
    let o = f.run(&["gate", "run", "--", "--version"]);
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains("nice (GNU coreutils)"),
        "stdout was nice's version: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o = f.run(&["gate", "run", "--", "-claudecat-missing-command"]);
    assert_ne!(code(&o), 0, "a missing command must fail");
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        !stderr.contains("Usage:") && !stderr.contains("Try 'nice --help'"),
        "stderr was nice's usage: {stderr}"
    );
}

fn hold(f: &Fixture, name: &str) -> File {
    fs::create_dir_all(f.gate()).expect("gate");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.gate().join(name))
        .expect("lock file");
    file.try_lock().expect("lock");
    file
}

#[test]
fn compiler_lock_denies_build_and_test_but_not_e2e() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold(&f, "compiler.lock");
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "build"])), 75);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "test"])), 75);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "e2e"])), 0);
}

#[test]
fn run_denies_when_compiler_lock_held_and_releases_slot() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold(&f, "compiler.lock");
    let marker = f.temp.path().join("marker");
    let o = f
        .cmd(&[
            "gate",
            "run",
            "--kind",
            "test",
            "--",
            "touch",
            marker.to_str().expect("marker path"),
        ])
        .output()
        .expect("run");
    assert_eq!(code(&o), 75);
    assert!(!marker.exists(), "child must not run");
    let slot = File::open(f.gate().join("slot-0.lock")).expect("slot file");
    slot.try_lock().expect("slot released on denial");
}

#[test]
fn run_releases_compiler_lock_after_child() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(
        code(&f.run(&["gate", "run", "--kind", "test", "--", "true"])),
        0
    );
    let file = File::open(f.gate().join("compiler.lock")).expect("compiler.lock exists");
    file.try_lock().expect("compiler lock released");
}

#[test]
fn status_reports_locked_compiler_lock_holder() {
    let f = Fixture::new("0.5", 0);
    fs::create_dir(f.gate()).expect("gate");
    let path = f.gate().join("compiler.lock");
    fs::write(
        &path,
        "{\"pid\":77,\"kind\":\"build\",\"command\":[\"cargo\",\"build\"],\"started_at\":11}\n",
    )
    .expect("holder");
    let file = File::open(&path).expect("compiler lock");
    file.try_lock().expect("lock");
    let o = f.run(&["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["compiler_lock"]["occupied"], true);
    assert_eq!(v["compiler_lock"]["holder"]["pid"], 77);
}

#[test]
fn gradle_mentions_in_arguments_do_not_count_as_compiling() {
    let f = Fixture::new("0.5", 0);
    f.process(123, "rg", &["rg", "GradleDaemon"], 99);
    f.process(124, "tail", &["tail", "-f", "GradleDaemon.log"], 99);
    // A full class name ends with the dotted suffix too; only the JVM requirement rules it out.
    f.process(
        125,
        "rg",
        &[
            "rg",
            "worker.org.gradle.process.internal.worker.GradleWorkerMain",
        ],
        99,
    );
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "build"])), 0);
}

#[test]
fn idle_gradle_daemon_does_not_block_builds() {
    // The daemon stays resident for hours after a build; counting it refused every build/test
    // until it timed out. Only a worker JVM means Gradle is compiling or testing right now.
    let f = Fixture::new("0.5", 0);
    f.process(
        123,
        "java",
        &["java", "org.gradle.launcher.daemon.bootstrap.GradleDaemon"],
        99,
    );
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "test"])), 0);
}

#[test]
fn vim_on_broker_script_is_not_a_broker() {
    let f = Fixture::new("0.5", 0);
    f.process(702, "init", &["/init"], 1);
    f.process(123, "vim", &["vim", "app-server-broker.mjs"], 702);
    let o = f.run(&["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["orphan_brokers"].as_array().expect("array").len(), 0);
}

#[test]
fn status_survives_closed_stdout() {
    let f = Fixture::new("0.5", 0);
    let mut child = f
        .cmd(&["status"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stdout.take());
    assert_eq!(child.wait().expect("wait").code(), Some(0));
}

#[test]
fn check_survives_closed_stderr_when_proc_unreadable() {
    let f = Fixture::new("0.5", 0);
    fs::remove_file(f.proc().join("loadavg")).expect("remove loadavg");
    let mut child = f
        .cmd(&["gate", "check"])
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    drop(child.stderr.take());
    assert_eq!(child.wait().expect("wait").code(), Some(0));
}

#[test]
fn locks_are_not_inherited_by_grandchildren() {
    let f = Fixture::new("0.5", 0);
    let pidfile = f.temp.path().join("grandchild.pid");
    let script = format!("sleep 5 & echo $! > {}; exit 0", pidfile.display());
    // Not `output()`: it reads the captured pipes to EOF, and the grandchild would keep them
    // open, so the test would wait out the sleep before it could observe it.
    let status = f
        .cmd(&["gate", "run", "--kind", "build", "--", "sh", "-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run");
    assert_eq!(status.code(), Some(0));
    for name in ["slot-0.lock", "compiler.lock"] {
        let file = File::open(f.gate().join(name)).expect(name);
        file.try_lock()
            .expect("lock must be free when gate run returns");
    }
    let pid = fs::read_to_string(&pidfile).expect("pid file");
    let pid = pid.trim();
    let alive = Command::new("kill")
        .args(["-0", pid])
        .status()
        .expect("kill -0");
    assert!(alive.success(), "grandchild {pid} should still be running");
    Command::new("kill")
        .args(["-TERM", pid])
        .status()
        .expect("kill");
}

#[test]
fn run_holds_slot_during_child() {
    let f = Fixture::new("0.5", 0);
    let script = f.temp.path().join("wait.sh");
    fs::write(&script, "#!/bin/sh\necho ready\nread line\nexit 0\n").expect("script");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).expect("chmod");
    let mut child = f
        .cmd(&[
            "gate",
            "run",
            "--kind",
            "test",
            "--",
            script.to_str().expect("script path"),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut line = String::new();
    use std::io::BufRead;
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready");
    let file = File::open(f.gate().join("slot-0.lock")).expect("slot");
    assert!(file.try_lock().is_err());
    let holder: serde_json::Value =
        serde_json::from_slice(&fs::read(f.gate().join("slot-0.lock")).unwrap()).unwrap();
    assert_eq!(
        holder["cwd"],
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert!(holder.get("target").is_some());
    // A test run holds compiler.lock for the child's whole life, not just at the start.
    let compiler = File::open(f.gate().join("compiler.lock")).expect("compiler lock");
    assert!(compiler.try_lock().is_err());
    drop(child.stdin.take());
    assert!(child.wait().expect("wait").success());
    file.try_lock().expect("released");
    compiler.try_lock().expect("compiler lock released");
}

#[test]
fn cargo_identity_paths_options_and_targets() {
    use claudecat::gate::classify_with_cwd;
    let f = Fixture::new("0.5", 0);
    let root = f.temp.path();
    let project = root.join("x");
    fs::create_dir(&project).unwrap();
    let link = root.join("link");
    symlink(&project, &link).unwrap();
    let cases = [
        "cd x && cargo test",
        "cargo test --manifest-path x/Cargo.toml",
        "cargo test --manifest-path=x/Cargo.toml",
        "cargo -C x test",
        "env -C x cargo test",
        "env --chdir=x cargo test",
        "RUST_TEST_THREADS=2 nice -n 15 env -C x cargo test 2>&1 | tail",
        "cd x/../x && cargo +stable test",
        "cd link && cargo test",
        "cargo --config=net.retry=2 -C x test",
        "cargo --config net.retry=2 -C x test",
    ];
    for command in cases {
        let (kind, id) = classify_with_cwd(command, root);
        assert_eq!(kind, Classification::Heavy(Kind::Test), "{command}");
        let id = id.unwrap();
        assert!(id.argv.iter().any(|arg| arg == "test"), "{command}");
        assert_eq!(fs::canonicalize(&id.dir).unwrap(), project, "{command}");
    }
    let (_, tilde) = classify_with_cwd("cd ~/x/../x && cargo test", root);
    assert_eq!(
        tilde.unwrap().dir,
        std::path::Path::new(&std::env::var("HOME").unwrap()).join("x")
    );
    let (_, a) = classify_with_cwd("CARGO_TARGET_DIR=/tmp/a cargo test", root);
    let (_, b) = classify_with_cwd("CARGO_TARGET_DIR=/tmp/b cargo test", root);
    assert_ne!(a.unwrap().target, b.unwrap().target);
    let (_, override_target) = classify_with_cwd(
        "CARGO_TARGET_DIR=/tmp/a cargo test --target-dir=/tmp/b",
        root,
    );
    assert_eq!(
        override_target.unwrap().target.unwrap(),
        std::path::Path::new("/tmp/b")
    );
    let (_, separated_target) = classify_with_cwd("cargo test --target-dir /tmp/b", root);
    assert_eq!(
        separated_target.unwrap().target.unwrap(),
        std::path::Path::new("/tmp/b")
    );
    let (_, manifest) = classify_with_cwd("cargo test --manifest-path=x/Cargo.toml", root);
    assert_eq!(
        manifest.unwrap().argv,
        vec!["test", "--manifest-path=x/Cargo.toml"]
    );
    let (_, gradle) = classify_with_cwd("./gradlew test", root);
    assert!(gradle.is_none());
}

fn hook_command(f: &Fixture, command: &str, cwd: &std::path::Path) -> Output {
    let payload = serde_json::json!({"cwd":cwd,"tool_input":{"command":command}});
    hook(f, &payload.to_string())
}

#[test]
fn duplicate_cargo_process_cases() {
    let cases: &[(&[&str], &str, Option<&str>, bool)] = &[
        (&["cargo", "test"], "cargo test", Some(""), true),
        (
            &[
                "/x/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo",
                "test",
            ],
            "cargo +stable test",
            Some(""),
            true,
        ),
        (&["cargo", "test"], "env -C /b cargo test", Some(""), false),
        (
            &["cargo", "test"],
            "cargo test -- --nocapture",
            Some(""),
            false,
        ),
        (
            &["cargo", "test"],
            "cargo test",
            Some("CARGO_TARGET_DIR=/tmp/other\0"),
            false,
        ),
        (&["cargo", "test"], "cargo test", None, false),
        (&["cargo-clippy", "test"], "cargo test", Some(""), false),
        (&["rustc", "test"], "cargo test", Some(""), false),
        (&["clippy-driver", "test"], "cargo test", Some(""), false),
    ];
    for (argv, command, environ, expected) in cases {
        let f = Fixture::new("0.5", 0);
        f.process(1001, argv[0].rsplit('/').next().unwrap(), argv, 1);
        if let Some(value) = environ {
            fs::write(f.proc().join("1001/environ"), value).unwrap();
        }
        let cwd = if command.contains("/b") {
            std::path::Path::new("/a")
        } else {
            f.temp.path()
        };
        let out = hook_command(&f, command, cwd);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).starts_with("gate denied (duplicate)"),
            *expected,
            "{command} {argv:?} {environ:?}"
        );
        if *expected {
            let line = String::from_utf8(out.stdout).unwrap();
            assert!(line.starts_with(
                "gate denied (duplicate) the same command is already running: pid=1001 "
            ));
            assert!(!line.contains("since="));
            assert_eq!(line.lines().count(), 1);
        }
    }
}

#[test]
fn duplicate_target_and_symlink_and_busy_precedence() {
    let f = Fixture::new("7", 0);
    let real = f.temp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = f.temp.path().join("link");
    symlink(&real, &link).unwrap();
    f.process(1002, "cargo", &["cargo", "test"], 1);
    fs::remove_file(f.proc().join("1002/cwd")).unwrap();
    symlink(&real, f.proc().join("1002/cwd")).unwrap();
    fs::write(f.proc().join("1002/environ"), "").unwrap();
    let out = hook_command(&f, "cargo test", &link);
    assert_eq!(code(&out), 75);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("gate denied (duplicate)"));
}

#[test]
fn duplicate_explicit_target_without_environ_and_different_cwd() {
    let f = Fixture::new("0.5", 0);
    f.process(
        1003,
        "cargo",
        &["cargo", "test", "--target-dir=/tmp/g4-target"],
        1,
    );
    let same = hook_command(&f, "cargo test --target-dir=/tmp/g4-target", f.temp.path());
    assert_eq!(code(&same), 75);
    assert!(String::from_utf8_lossy(&same.stdout).starts_with("gate denied (duplicate)"));
    let different = hook_command(
        &f,
        "cargo test --target-dir=/tmp/g4-target",
        &f.temp.path().join("elsewhere"),
    );
    assert_eq!(code(&different), 0);
}

#[test]
fn duplicate_holder_and_old_holder() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold(&f, "slot-0.lock");
    let path = f.gate().join("slot-0.lock");
    fs::write(
        &path,
        r#"{"pid":77,"kind":"test","command":["cargo","test"],"started_at":1}"#,
    )
    .unwrap();
    let old = hook_command(&f, "cargo test", f.temp.path());
    assert_eq!(code(&old), 0);
    fs::write(&path, serde_json::json!({"pid":77,"kind":"test","command":["cargo","test"],"started_at":1,"cwd":f.temp.path(),"target":null}).to_string()).unwrap();
    let new = hook_command(&f, "cargo test", f.temp.path());
    assert_eq!(code(&new), 75);
    let line = String::from_utf8(new.stdout).unwrap();
    assert!(line.contains("pid=77 since="));
    assert_eq!(line.lines().count(), 1);
    let status: serde_json::Value =
        serde_json::from_slice(&f.run(&["status", "--json"]).stdout).unwrap();
    assert!(status["slots"][0]["holder"].get("cwd").is_none());
    assert!(status["slots"][0]["holder"].get("target").is_none());
}

#[test]
fn duplicate_inherited_target_environment() {
    let f = Fixture::new("0.5", 0);
    f.process(1004, "cargo", &["cargo", "test"], 1);
    fs::write(
        f.proc().join("1004/environ"),
        b"CARGO_TARGET_DIR=/tmp/g4-shared\0",
    )
    .unwrap();
    let payload = serde_json::json!({"cwd":f.temp.path(),"tool_input":{"command":"cargo test"}});
    let mut child = f
        .cmd(&["gate", "hook"])
        .env("CARGO_TARGET_DIR", "/tmp/g4-shared")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 75);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("gate denied (duplicate)"));
}

#[test]
fn duplicate_relative_target_follows_symlinked_dir() {
    let f = Fixture::new("0.5", 0);
    let real = f.temp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = f.temp.path().join("link");
    symlink(&real, &link).unwrap();
    f.process(1005, "cargo", &["cargo", "test", "--target-dir=out"], 1);
    fs::remove_file(f.proc().join("1005/cwd")).unwrap();
    symlink(&real, f.proc().join("1005/cwd")).unwrap();
    let out = hook_command(&f, "cargo test --target-dir=out", &link);
    assert_eq!(code(&out), 75);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("gate denied (duplicate)"));
}

#[test]
fn duplicate_identity_uses_the_executed_word() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold(&f, "slot-0.lock");
    fs::write(
        f.gate().join("slot-0.lock"),
        serde_json::json!({"pid":78,"kind":"test","command":["node","server.js","cargo","test"],"started_at":1,"cwd":f.temp.path(),"target":null}).to_string(),
    )
    .unwrap();
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 0);
    f.process(1006, "cargo", &["cargo", "test"], 1);
    fs::write(f.proc().join("1006/environ"), "").unwrap();
    let wrapped = hook_command(&f, "env -u cargo cargo test", f.temp.path());
    assert_eq!(code(&wrapped), 75);
    assert!(String::from_utf8_lossy(&wrapped.stdout).contains("pid=1006"));
}
