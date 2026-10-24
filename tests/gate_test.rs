use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::symlink;
use std::process::{Command, Output, Stdio};
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
            .env_remove("CLAUDECAT_MAX_LOAD")
            .env_remove("CLAUDECAT_MAX_SWAP_MB")
            .env_remove("CLAUDECAT_SLOTS");
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
    // A test run holds compiler.lock for the child's whole life, not just at the start.
    let compiler = File::open(f.gate().join("compiler.lock")).expect("compiler lock");
    assert!(compiler.try_lock().is_err());
    drop(child.stdin.take());
    assert!(child.wait().expect("wait").success());
    file.try_lock().expect("released");
    compiler.try_lock().expect("compiler lock released");
}
