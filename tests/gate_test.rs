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
        let f = Self { temp };
        f.meminfo(4096000, swap_used_kb);
        f
    }

    fn meminfo(&self, available_kb: u64, swap_used_kb: u64) {
        fs::write(
            self.proc().join("meminfo"),
            format!(
                "MemAvailable: {available_kb} kB\nSwapTotal: 2097152 kB\nSwapFree: {} kB\n",
                2097152 - swap_used_kb
            ),
        )
        .expect("meminfo");
    }

    fn pressure(&self, some_avg10: &str) {
        let dir = self.proc().join("pressure");
        fs::create_dir_all(&dir).expect("pressure dir");
        fs::write(
            dir.join("memory"),
            format!(
                "some avg10={some_avg10} avg60=0.00 avg300=0.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1\n"
            ),
        )
        .expect("pressure");
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
            .env_remove("CLAUDECAT_MAX_MEM_PRESSURE")
            .env_remove("CLAUDECAT_MIN_AVAILABLE_MB")
            .env_remove("CLAUDECAT_SLOTS")
            .env_remove("CLAUDECAT_OTHER_SLOTS")
            .env_remove("CLAUDECAT_RESERVE_GRADLE_MB")
            .env_remove("CLAUDECAT_RESERVE_CARGO_MB")
            .env_remove("CLAUDECAT_RESERVE_WARMUP_SECS")
            .env_remove("CLAUDECAT_RESERVE_CARGO_LOAD")
            .env_remove("CLAUDECAT_RESERVE_GRADLE_LOAD")
            .env_remove("CLAUDECAT_RESERVE_OTHER_MB")
            .env_remove("CLAUDECAT_GRADLE_SAMPLE_MS")
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
        self.stat(pid, name, ppid, 0, 0, 1);
        symlink(self.temp.path(), dir.join("cwd")).expect("cwd");
    }
    fn stat(&self, pid: u32, name: &str, ppid: u32, utime: u64, stime: u64, start: u64) {
        let dir = self.proc().join(pid.to_string());
        let contents = format!(
            "{pid} ({name}) S {ppid} 0 0 0 0 0 0 0 0 0 {utime} {stime} 0 0 0 0 0 0 {start}\n"
        );
        let temporary = dir.join("stat.new");
        fs::write(&temporary, contents).expect("stat");
        fs::rename(temporary, dir.join("stat")).expect("replace stat");
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
        ("cargo install --locked --path .", NotHeavy),
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

/// Now in seconds, early in the second. The gate reads the clock in a process of its own, some
/// milliseconds after this -- more on a loaded machine -- so a holder stamped late in its second
/// is a second old by then, and its decaying reservation a sixtieth smaller than the test expects.
fn now_secs() -> u64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    if now.subsec_millis() > 400 {
        std::thread::sleep(std::time::Duration::from_millis(
            u64::from(1000 - now.subsec_millis()) + 20,
        ));
    }
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
    // Best of three refilled runs, judged against the same hook on a one-row log measured right
    // beside it: process start-up and a busy runner inflate both alike, so only the extra cost of
    // reading the whole 1 MB file (what compaction must not do) can push the difference over.
    let fastest_of_three = |rows: usize, load: &str, expected: i32| {
        fs::write(f.proc().join("loadavg"), format!("{load} 0 0 1/1 1\n")).unwrap();
        (0..3)
            .map(|_| {
                fs::write(log_path(&f), row.repeat(rows)).unwrap();
                // Each allowed hook admits cargo; start every run from an empty reservation.
                let _ = fs::remove_file(f.gate().join("admitted.jsonl"));
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
            .unwrap()
    };
    for (load, expected) in [("7", 75), ("0.5", 0)] {
        let baseline = fastest_of_three(1, load, expected);
        let fastest = fastest_of_three(1024 * 1024 / row.len() + 1, load, expected);
        assert!(
            fastest <= baseline + 40,
            "1 MB log took {fastest} ms vs {baseline} ms for a one-row log"
        );
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
    assert!(line.ends_with("Denied, not queued: nothing will run this command later. Retry it after load and memory pressure drop (figures above); do not poll with sleep in the foreground.\n"));
    assert_eq!(line.lines().count(), 1);
    assert!(denied.stderr.is_empty());
    busy.process(123, "rustc", &["rustc", "main.rs"], 99);
    let compiling = hook(&busy, r#"{"tool_input":{"command":"cargo test"}}"#);
    assert_eq!(code(&compiling), 75);
    let line = String::from_utf8(compiling.stdout).unwrap();
    assert!(line.starts_with("gate denied (load) "), "{line}");
    // A running rustc is no longer a reason to wait for holders: the load alone denies.
    assert!(line.ends_with("Denied, not queued: nothing will run this command later. Retry it after load and memory pressure drop (figures above); do not poll with sleep in the foreground.\n"), "{line}");
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
fn high_swap_without_pressure_allows() {
    // WSL2 keeps idle pages in swap forever: swap used alone must not deny.
    let f = Fixture::new("0.5", 1141 * 1024);
    f.pressure("0.02");
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty());
}

#[test]
fn memory_pressure_over_limit_denies() {
    let f = Fixture::new("0.5", 0);
    f.pressure("10.01");
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.starts_with("gate denied (memory) "), "{stderr}");
    assert!(
        stderr.contains("psi_some_avg10=10.01/10.00 avail=4000MB/1536MB swap=0MB"),
        "{stderr}"
    );
    f.pressure("10.00");
    assert_eq!(code(&f.run(&["gate", "check"])), 0);
}

#[test]
fn low_mem_available_denies() {
    let f = Fixture::new("0.5", 0);
    f.pressure("0.00");
    f.meminfo(1535 * 1024, 0);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.starts_with("gate denied (memory) "), "{stderr}");
    assert!(stderr.contains("avail=1535MB/1536MB"), "{stderr}");
    f.meminfo(1536 * 1024, 0);
    assert_eq!(code(&f.run(&["gate", "check"])), 0);
}

#[test]
fn missing_pressure_file_checks_mem_available_only() {
    let f = Fixture::new("0.5", 0);
    assert!(!f.proc().join("pressure/memory").exists());
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0);
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    f.meminfo(512 * 1024, 0);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!stderr.contains("warning"), "{stderr}");
    // 512MB is also below the 1024MB an `other` needs beyond the reservations.
    assert!(
        stderr.starts_with("gate denied (memory,reserved) "),
        "{stderr}"
    );
    assert!(
        stderr.contains("psi_some_avg10=n/a avail=512MB/1536MB"),
        "{stderr}"
    );
    let status = f.run(&["status", "--json"]);
    assert_eq!(code(&status), 0);
    let v: serde_json::Value = serde_json::from_slice(&status.stdout).expect("json");
    assert!(v["mem_pressure_some_avg10"].is_null());
    assert_eq!(v["mem_available_mb"], 512);
}

#[test]
fn unparsable_pressure_file_checks_mem_available_only() {
    let f = Fixture::new("0.5", 0);
    f.pressure("garbage");
    assert_mem_available_only(&f);
}

#[test]
fn unreadable_pressure_file_checks_mem_available_only() {
    // A directory where the file should be makes read_to_string fail, like a psi=0 kernel.
    let f = Fixture::new("0.5", 0);
    fs::create_dir_all(f.proc().join("pressure/memory")).expect("pressure dir");
    assert_mem_available_only(&f);
}

/// PSI is unusable: allow on plenty of MemAvailable, deny on low, never fall back to a warning.
fn assert_mem_available_only(f: &Fixture) {
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty());
    f.meminfo(512 * 1024, 0);
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8_lossy(&o.stderr);
    // 512MB is also below the 1024MB an `other` needs beyond the reservations.
    assert!(
        stderr.starts_with("gate denied (memory,reserved) "),
        "{stderr}"
    );
    assert!(stderr.contains("psi_some_avg10=n/a"), "{stderr}");
    assert!(!stderr.contains("gate warning"), "{stderr}");
}

#[test]
fn hook_memory_only_deny_does_not_wait_for_holders() {
    let f = Fixture::new("0.5", 0);
    f.pressure("0.00");
    f.meminfo(512 * 1024, 0);
    let denied = hook(&f, r#"{"tool_input":{"command":"cargo test"}}"#);
    assert_eq!(code(&denied), 75);
    let line = String::from_utf8(denied.stdout).unwrap();
    // 512MB is also below cargo's reservation estimate, so `reserved` fires beside `memory`.
    assert!(line.starts_with("gate denied (memory,reserved) "), "{line}");
    assert!(line.contains("holders=[]"), "{line}");
    assert!(line.ends_with("Denied, not queued: nothing will run this command later. Retry it after load and memory pressure drop (figures above); do not poll with sleep in the foreground.\n"), "{line}");
    assert!(!line.contains("holders above"), "{line}");
    assert_eq!(line.lines().count(), 1);
}

#[test]
fn memory_thresholds_follow_env() {
    let f = Fixture::new("0.5", 0);
    f.pressure("5.00");
    f.meminfo(2048 * 1024, 0);
    assert_eq!(code(&f.run(&["gate", "check"])), 0);
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_MEM_PRESSURE", "4.5")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("psi_some_avg10=5.00/4.50"));
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MIN_AVAILABLE_MB", "4096")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("avail=2048MB/4096MB"));
    f.pressure("20.00");
    f.meminfo(512 * 1024, 0);
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_MEM_PRESSURE", "25")
        .env("CLAUDECAT_MIN_AVAILABLE_MB", "256")
        // 512MB is below the 1024MB an `other` needs beyond the reservations unless that follows.
        .env("CLAUDECAT_RESERVE_OTHER_MB", "256")
        .output()
        .expect("check");
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn status_reports_memory_pressure() {
    let f = Fixture::new("0.5", 1141 * 1024);
    f.pressure("0.02");
    let o = f.run(&["status"]);
    assert_eq!(code(&o), 0);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        stdout.starts_with(
            "load 1m: 0.50; PSI memory some avg10: 0.02; MemAvailable: 4000 MB; swap used: 1141 MB\n"
        ),
        "{stdout}"
    );
    let o = f.run(&["status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["mem_pressure_some_avg10"], 0.02);
    assert_eq!(v["swap_used_mb"], 1141);
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
fn invalid_thresholds_use_defaults() {
    let f = Fixture::new("6.01", 0);
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_LOAD", "0")
        .env("CLAUDECAT_MAX_MEM_PRESSURE", "garbage")
        .env("CLAUDECAT_MIN_AVAILABLE_MB", "-1")
        .env("CLAUDECAT_SLOTS", "0")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("6.00"), "{stderr}");
    assert!(stderr.contains("avail=4000MB/1536MB"), "{stderr}");
    let f = Fixture::new("0.5", 0);
    f.pressure("10.01");
    let o = f
        .cmd(&["gate", "check"])
        .env("CLAUDECAT_MAX_LOAD", "garbage")
        .env("CLAUDECAT_MAX_MEM_PRESSURE", "0")
        .env("CLAUDECAT_MIN_AVAILABLE_MB", "0")
        .output()
        .expect("check");
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8_lossy(&o.stderr).contains("psi_some_avg10=10.01/10.00"));
}

/// Without load or memory figures the gate cannot tell an idle machine from a swapping one, so it
/// refuses (NUC #4380: a gate that allows on unreadable resources is no gate -- macOS had none).
#[test]
fn unreadable_resources_fail_closed() {
    let f = Fixture::new("0.5", 0);
    fs::remove_file(f.proc().join("loadavg")).expect("remove loadavg");
    let o = f.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("gate denied (resources)") && err.contains("machine resources unreadable"),
        "{err}"
    );
    let o = f.run(&["gate", "run", "--", "true"]);
    assert_eq!(code(&o), 75, "run must not execute either");
}

/// A gate run started before the upgrade still holds `slot-N.lock`: a holder to count, never a
/// slot to wait for.
#[test]
fn old_slot_locks_do_not_stop_a_run() {
    let f = Fixture::new("0.5", 0);
    let _locks: Vec<_> = (0..3u32)
        .map(|i| {
            hold_holder(
                &f,
                &format!("slot-{i}.lock"),
                90 + i,
                &["cargo", "build"],
                1,
            )
        })
        .collect();
    let marker = f.temp.path().join("marker");
    let o = f
        .cmd(&[
            "gate",
            "run",
            "--kind",
            "e2e",
            "--",
            "touch",
            marker.to_str().expect("marker path"),
        ])
        .output()
        .expect("run");
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(marker.exists());
    assert_eq!(holders(&f).len(), 3);
}
#[test]
fn run_returns_child_codes_and_leaves_no_lock_file() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "true"])), 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "false"])), 1);
    // No slot file, no compiler lock file: the registrations are gone with their runs.
    assert_eq!(lock_files(&f), ["admission.lock"]);
    assert!(holders(&f).is_empty());
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

/// Ticks keep growing for 800 ms, twice the 400 ms sample the busy tests set, so the gate sees growth
/// whenever its first sample lands -- a single rewrite after a fixed delay raced the child's startup.
fn keep_busy(f: &Fixture) {
    for ticks in 1..=16 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        f.stat(123, "java", 99, ticks, 3, 1);
    }
}

fn gradle_worker(f: &Fixture) {
    f.process(
        123,
        "java",
        &[
            "java",
            "org.gradle.process.internal.worker.GradleWorkerMain",
        ],
        99,
    );
}

#[test]
/// Gradle is gated on resources alone (user 2026-11-20): a busy worker of another build does
/// not deny build or test while load and memory allow it.
fn gradle_worker_does_not_deny_build_or_test() {
    let f = Fixture::new("0.5", 0);
    gradle_worker(&f);
    std::thread::scope(|scope| {
        scope.spawn(|| keep_busy(&f));
        for kind in ["test", "build"] {
            let output = f
                .cmd(&["gate", "check", "--kind", kind])
                .env("CLAUDECAT_GRADLE_SAMPLE_MS", "400")
                .output()
                .expect("check");
            assert_eq!(
                code(&output),
                0,
                "{kind}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    });
}

#[test]
fn idle_gradle_worker_is_allowed() {
    let f = Fixture::new("0.5", 0);
    gradle_worker(&f);
    let output = f
        .cmd(&["gate", "check", "--kind", "test"])
        .env("CLAUDECAT_GRADLE_SAMPLE_MS", "1")
        .output()
        .expect("check");
    assert_eq!(code(&output), 0);
    let output = f
        .cmd(&["status", "--json"])
        .env("CLAUDECAT_GRADLE_SAMPLE_MS", "1")
        .output()
        .expect("status");
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert!(status["compiling"].as_array().expect("array").is_empty());
}

#[test]
fn unreadable_gradle_stat_is_allowed() {
    let f = Fixture::new("0.5", 0);
    gradle_worker(&f);
    fs::remove_file(f.proc().join("123/stat")).expect("remove stat");
    let output = f
        .cmd(&["gate", "check", "--kind", "test"])
        .env("CLAUDECAT_GRADLE_SAMPLE_MS", "0")
        .output()
        .expect("check");
    assert_eq!(code(&output), 0);
}

#[test]
fn status_reports_busy_gradle_worker() {
    let f = Fixture::new("0.5", 0);
    gradle_worker(&f);
    std::thread::scope(|scope| {
        scope.spawn(|| keep_busy(&f));
        let output = f
            .cmd(&["status", "--json"])
            .env("CLAUDECAT_GRADLE_SAMPLE_MS", "400")
            .output()
            .expect("status");
        let status: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        assert_eq!(status["compiling"][0]["pid"], 123);
    });
}

#[test]
fn vanished_or_reused_gradle_worker_is_allowed() {
    for reused in [false, true] {
        let f = Fixture::new("0.5", 0);
        gradle_worker(&f);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if reused {
                    f.stat(123, "java", 99, 10, 0, 2);
                } else {
                    fs::remove_dir_all(f.proc().join("123")).expect("remove worker");
                }
            });
            let output = f
                .cmd(&["gate", "check", "--kind", "test"])
                .env("CLAUDECAT_GRADLE_SAMPLE_MS", "400")
                .output()
                .expect("check");
            assert_eq!(code(&output), 0);
        });
    }
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

/// `holders` lists every `gate run`: the new registrations and the lock files of a run that
/// started before the upgrade. Status prints neither the cwd nor the target they carry.
#[test]
fn status_reports_holders_of_old_and_new_lock_files() {
    let f = Fixture::new("0.5", 0);
    let _slot = hold_holder(&f, "slot-0.lock", 42, &["cargo", "build"], 10);
    let _run = hold_run(&f, 88, "test", &["cargo", "test"], 12);
    let listed = holders(&f);
    let by_pid = |pid: i64| {
        listed
            .iter()
            .find(|h| h["pid"].as_i64() == Some(pid))
            .unwrap_or_else(|| panic!("{listed:?}"))
    };
    assert_eq!(by_pid(42)["kind"], "build");
    assert_eq!(by_pid(42)["command"], serde_json::json!(["cargo", "build"]));
    assert_eq!(by_pid(42)["started_at"], 10);
    assert_eq!(by_pid(88)["kind"], "test");
    assert_eq!(by_pid(88)["started_at"], 12);
    assert_eq!(listed.len(), 2, "{listed:?}");
    for h in &listed {
        assert!(h.get("cwd").is_none() && h.get("target").is_none(), "{h}");
    }
    let text = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    for pid in [42, 88] {
        assert!(text.contains(&format!("pid={pid} ")), "{text}");
    }
    assert!(!text.contains("slots="), "{text}");
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

/// A pre-upgrade gate run's compiler lock file is a holder whose reservation counts like any
/// other; it excludes nobody.
#[test]
fn old_compiler_lock_reserves_but_excludes_nobody() {
    // 4000MB available: Gradle's 3072MB leaves 928MB, below cargo's 1536MB.
    let f = Fixture::new("0.5", 0);
    let _gradle = hold_holder(
        &f,
        "compiler-gradle.lock",
        77,
        &["./gradlew", "test"],
        now_secs(),
    );
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    // Once past the warm-up window the same lock denies nothing.
    let g = Fixture::new("0.5", 0);
    let _cargo = hold_holder(
        &g,
        "compiler-cargo.lock",
        78,
        &["cargo", "test"],
        now_secs() - 120,
    );
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "build"])), 0);
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "test"])), 0);
}
#[test]
fn run_executes_while_an_old_compiler_lock_is_held() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold_holder(&f, "compiler-cargo.lock", 77, &["cargo", "test"], 1);
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
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(marker.exists());
}
/// A run registers `run-<pid>-<random>.lock`; the new gate run creates no compiler lock file.
#[test]
fn run_creates_no_compiler_lock_file() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(
        code(&f.run(&["gate", "run", "--kind", "test", "--", "true"])),
        0
    );
    assert_eq!(lock_files(&f), ["admission.lock"]);
}
#[test]
fn status_reports_the_holder_of_an_old_compiler_lock() {
    let f = Fixture::new("0.5", 0);
    let _lock = hold_holder(&f, "compiler-cargo.lock", 77, &["cargo", "test"], 11);
    let listed = holders(&f);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["pid"], 77);
    assert_eq!(listed[0]["started_at"], 11);
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
    // until it timed out. A daemon is not a worker candidate.
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
    // Unreadable resources deny (75); the point here is that a closed stderr does not crash it.
    assert_eq!(child.wait().expect("wait").code(), Some(75));
}

/// 7-3: one holder while the child lives, none once `gate run` returns, even with a grandchild
/// still alive: the lock is never inherited and the file is unlinked. (Replaces the loop over
/// the old slot and compiler lock file names, which a name pattern would have emptied.)
#[test]
fn locks_are_not_inherited_by_grandchildren() {
    use std::io::BufRead;
    let f = Fixture::new("0.5", 0);
    let pidfile = f.temp.path().join("grandchild.pid");
    let script = format!(
        "sleep 5 >/dev/null 2>&1 & echo $! > {}; echo ready; read line; exit 0",
        pidfile.display()
    );
    let mut child = f
        .cmd(&["gate", "run", "--kind", "build", "--", "sh", "-c", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready");
    assert_eq!(holders(&f).len(), 1, "registered while its child lives");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(holders(&f).is_empty());
    assert!(run_files(&f).is_empty());
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
/// The registered holder carries the cwd and target the duplicate check reads.
#[test]
fn registered_holder_carries_cwd_and_target() {
    use std::io::BufRead;
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
        .env("CARGO_TARGET_DIR", "/tmp/gate-run-target")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready");
    let files = run_files(&f);
    assert_eq!(files.len(), 1, "{files:?}");
    let holder: serde_json::Value =
        serde_json::from_slice(&fs::read(f.gate().join(&files[0])).unwrap()).unwrap();
    assert_eq!(
        holder["cwd"],
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(holder["target"], "/tmp/gate-run-target");
    drop(child.stdin.take());
    assert!(child.wait().expect("wait").success());
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
    assert!(status["holders"][0].get("cwd").is_none());
    assert!(status["holders"][0].get("target").is_none());
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

/// Hold a lock file the way `gate run` does: locked, with the holder JSON inside.
fn hold_holder(f: &Fixture, name: &str, pid: u32, command: &[&str], started_at: u64) -> File {
    let file = hold(f, name);
    fs::write(
        f.gate().join(name),
        serde_json::json!({"pid":pid,"kind":"build","command":command,"started_at":started_at})
            .to_string(),
    )
    .expect("holder");
    file
}

fn deny_line(o: &Output) -> String {
    assert_eq!(code(o), 75, "{}", String::from_utf8_lossy(&o.stdout));
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn cross_toolchain_does_not_deny_when_memory_is_ample() {
    let f = Fixture::new("0.5", 0);
    let _gradle = hold_run(&f, 77, "build", &["./gradlew", "test"], 1);
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 0);
    // The allowed cargo hook admitted a reservation; drop it so only the holder is under test.
    fs::remove_file(f.gate().join("admitted.jsonl")).unwrap();
    assert_eq!(code(&hook_command(&f, "./gradlew test", f.temp.path())), 0);
    // A running rustc counts against nothing: it is information for `status` only.
    let g = Fixture::new("0.5", 0);
    g.process(123, "rustc", &["rustc", "main.rs"], 99);
    assert_eq!(code(&hook_command(&g, "./gradlew test", g.temp.path())), 0);
    fs::remove_file(g.gate().join("admitted.jsonl")).unwrap();
    assert_eq!(code(&hook_command(&g, "cargo build", g.temp.path())), 0);
}
/// Neither old toolchain lock file queues a cargo run.
#[test]
fn run_ignores_old_toolchain_locks() {
    let f = Fixture::new("0.5", 0);
    let _gradle = hold_holder(&f, "compiler-gradle.lock", 77, &["./gradlew", "test"], 1);
    let _cargo = hold_holder(&f, "compiler-cargo.lock", 78, &["cargo", "test"], 1);
    let o = f.run(&[
        "gate", "run", "--kind", "test", "--", "cargo", "test", "--help",
    ]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
}
#[test]
fn reservation_denies_while_holder_warms_up_and_not_after() {
    let f = Fixture::new("0.5", 0);
    let now = now_secs();
    // 4000MB available, gradle reserves 3072MB: 928MB left, cargo needs 1536MB.
    let _slot = hold_holder(&f, "slot-0.lock", 77, &["./gradlew", "test"], now);
    let _gradle = hold_holder(&f, "compiler-gradle.lock", 77, &["./gradlew", "test"], now);
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    // One holder in two locks reserves once.
    assert!(line.contains("reserved=gradle:3072MB "), "{line}");
    assert!(line.contains("warm-up passes (at most 60s)"), "{line}");
    assert_eq!(line.lines().count(), 1);

    let old = Fixture::new("0.5", 0);
    let _slot = hold_holder(&old, "slot-0.lock", 77, &["./gradlew", "test"], now - 120);
    let _gradle = hold_holder(
        &old,
        "compiler-gradle.lock",
        77,
        &["./gradlew", "test"],
        now - 120,
    );
    let status = String::from_utf8(old.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=none"), "{status}");
    assert_eq!(code(&hook_command(&old, "cargo test", old.temp.path())), 0);
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=gradle:3072MB"), "{status}");
}

/// `gate check` names no command, so no reservation is checked against it; a holder reserves
/// memory only for a toolchain its command names.
#[test]
fn a_command_that_names_no_toolchain_needs_no_reservation() {
    let f = Fixture::new("0.5", 0);
    f.meminfo(2000 * 1024, 0);
    let _cargo = hold_run(&f, 77, "build", &["cargo", "test"], now_secs());
    let o = f.run(&["gate", "check", "--kind", "build"]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8(f.run(&["status"]).stdout)
        .unwrap()
        .contains("reserved=cargo:1536MB"));
    let g = Fixture::new("0.5", 0);
    let _make = hold_run(&g, 78, "build", &["make", "all"], now_secs());
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "build"])), 0);
    assert!(String::from_utf8(g.run(&["status"]).stdout)
        .unwrap()
        .contains("reserved=none"));
}
#[test]
fn compound_command_checks_both_toolchains() {
    let compound = "cargo build && ./gradlew test";
    let with = |command: &[&str], available_mb: u64| {
        let f = Fixture::new("0.5", 0);
        f.meminfo(available_mb * 1024, 0);
        let _holder = hold_run(&f, 77, "build", command, now_secs());
        f
    };
    // 8000MB available: either reservation leaves room for both estimates (1536 + 3072).
    for holder in [["./gradlew", "test"], ["cargo", "test"]] {
        let f = with(&holder, 8000);
        assert_eq!(code(&hook_command(&f, compound, f.temp.path())), 0);
    }
    // 4000MB available: they do not fit, whether a Gradle or a cargo run reserves first...
    for holder in [["./gradlew", "test"], ["cargo", "test"]] {
        let f = with(&holder, 4000);
        let line = deny_line(&hook_command(&f, compound, f.temp.path()));
        assert!(
            line.starts_with("gate denied (reserved) "),
            "{holder:?}: {line}"
        );
    }
    // ...nor with nothing held, while cargo alone fits.
    let h = Fixture::new("0.5", 0);
    let line = deny_line(&hook_command(&h, compound, h.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=none"), "{line}");
    assert_eq!(code(&hook_command(&h, "cargo build", h.temp.path())), 0);
}
#[test]
fn huge_reservation_estimates_saturate_instead_of_wrapping() {
    let f = Fixture::new("0.5", 0);
    f.meminfo(8000 * 1024, 0);
    let payload = serde_json::json!({
        "cwd": f.temp.path(),
        "tool_input": {"command": "cargo build && ./gradlew test"},
    })
    .to_string();
    // u64::MAX + 2 wraps to 1, which would let the command through.
    let mut child = f
        .cmd(&["gate", "hook"])
        .env("CLAUDECAT_RESERVE_CARGO_MB", u64::MAX.to_string())
        .env("CLAUDECAT_RESERVE_GRADLE_MB", "2")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("payload");
    let out = child.wait_with_output().expect("hook output");
    assert_eq!(code(&out), 75, "{}", String::from_utf8_lossy(&out.stdout));
    let line = String::from_utf8(out.stdout).unwrap();
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
}

/// Neither a test runner nor an unrecognised command queues behind the lock files a pre-upgrade
/// gate run holds.
#[test]
fn old_lock_files_queue_neither_test_runners_nor_unknown_commands() {
    let f = Fixture::new("0.5", 0);
    let _cargo = hold_holder(&f, "compiler-cargo.lock", 77, &["cargo", "test"], 1);
    let _gradle = hold_holder(&f, "compiler-gradle.lock", 78, &["./gradlew", "test"], 1);
    for command in [vec!["python3", "-c", "pass"], vec!["true"]] {
        let o = f
            .cmd(&["gate", "run", "--kind", "test", "--"])
            .args(&command)
            .output()
            .expect("run");
        assert_eq!(
            code(&o),
            0,
            "{command:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}
fn admissions_path(f: &Fixture) -> std::path::PathBuf {
    f.gate().join("admitted.jsonl")
}

fn admission_rows(f: &Fixture) -> Vec<serde_json::Value> {
    fs::read_to_string(admissions_path(f))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("admission row"))
        .collect()
}

#[test]
fn hook_admission_reserves_memory_until_warmup_passes() {
    let f = Fixture::new("0.5", 0);
    // 4000MB available: gradle's 3072MB leaves 928MB, below cargo's 1536MB.
    assert_eq!(code(&hook_command(&f, "./gradlew test", f.temp.path())), 0);
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["toolchains"], serde_json::json!(["gradle"]));
    assert!(rows[0]["session"].is_null());
    assert_eq!(rows[0].as_object().unwrap().len(), 3, "no command text");
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=gradle:3072MB "), "{line}");
    // No slot or lock holder exists, only the admission: wait for the warm-up, not for holders.
    assert!(!line.contains("holders above"), "{line}");
    assert!(line.contains("warm-up passes (at most 60s)"), "{line}");
    assert_eq!(line.lines().count(), 1);
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=gradle:3072MB"), "{status}");
    // The denied cargo hook wrote nothing.
    assert_eq!(admission_rows(&f).len(), 1);

    // Backdate the row beyond the 60s warm-up: it no longer reserves.
    let mut row = rows[0].clone();
    row["ts"] = serde_json::json!(now_secs() - 120);
    fs::write(admissions_path(&f), format!("{row}\n")).unwrap();
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=none"), "{status}");
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 0);
    // The stale row was pruned when the new one was written.
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["toolchains"], serde_json::json!(["cargo"]));
}

#[test]
fn concurrent_hooks_admit_only_what_fits() {
    let f = Fixture::new("0.5", 0);
    let payload =
        serde_json::json!({"cwd": f.temp.path(), "tool_input": {"command": "./gradlew test"}})
            .to_string();
    let children: Vec<_> = (0..6)
        .map(|_| {
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
                .write_all(payload.as_bytes())
                .unwrap();
            child
        })
        .collect();
    let outs: Vec<_> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();
    let allowed = outs.iter().filter(|o| code(o) == 0).count();
    assert_eq!(
        allowed,
        1,
        "{:?}",
        outs.iter().map(code).collect::<Vec<_>>()
    );
    for o in outs.iter().filter(|o| code(o) != 0) {
        assert_eq!(code(o), 75);
        assert!(String::from_utf8_lossy(&o.stdout).starts_with("gate denied (reserved) "));
    }
    assert_eq!(admission_rows(&f).len(), 1);
}

#[test]
fn admission_cache_failures_fail_open() {
    // admitted.jsonl is a directory: it cannot be read or replaced.
    let f = Fixture::new("0.5", 0);
    fs::create_dir_all(admissions_path(&f)).unwrap();
    let o = hook_command(&f, "./gradlew test", f.temp.path());
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 0);
    // A denial still comes from the snapshot alone.
    f.meminfo(512 * 1024, 0);
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (memory,reserved) "), "{line}");

    // admission.lock is unusable too.
    let g = Fixture::new("0.5", 0);
    fs::create_dir_all(g.gate().join("admission.lock")).unwrap();
    let o = hook_command(&g, "./gradlew test", g.temp.path());
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    assert!(!admissions_path(&g).exists());
}

#[test]
fn only_allowed_compiler_hooks_are_admitted() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "build"])), 0);
    assert_eq!(code(&hook_command(&f, "echo hi", f.temp.path())), 0);
    assert_eq!(
        code(&hook_command(
            &f,
            "claudecat gate run --kind test -- cargo test",
            f.temp.path()
        )),
        0
    );
    assert!(!admissions_path(&f).exists());
    // A denied hook records nothing.
    f.meminfo(512 * 1024, 0);
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 75);
    assert!(!admissions_path(&f).exists());
}

#[test]
fn admission_rows_are_capped() {
    let f = Fixture::new("0.5", 0);
    f.meminfo(1_000_000 * 1024, 0);
    fs::create_dir_all(f.gate()).unwrap();
    let ts = now_secs();
    let rows: String = (0..70)
        .map(|_| format!("{{\"ts\":{ts},\"session\":null,\"toolchains\":[\"cargo\"]}}\n"))
        .collect();
    fs::write(admissions_path(&f), rows).unwrap();
    // Seventy fresh cargo rows also reserve 140 cores of load, so lift the load limit here.
    let mut hook = f.cmd(&["gate", "hook"]);
    hook.env("CLAUDECAT_MAX_LOAD", "1000");
    hook.stdin(Stdio::piped());
    hook.stdout(Stdio::piped());
    let mut child = hook.spawn().unwrap();
    let payload =
        serde_json::json!({"cwd": f.temp.path(), "tool_input": {"command": "./gradlew test"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stdout));
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 64);
    assert_eq!(rows[63]["toolchains"], serde_json::json!(["gradle"]));
}
#[test]
fn sequential_commands_of_one_session_do_not_accumulate_reservations() {
    let f = Fixture::new("0.5", 0);
    f.meminfo(5000 * 1024, 0);
    // Four quick cargo runs of one session within the warm-up window: 4 x 1536MB would deny the
    // last on an idle 5000MB machine if every admission piled up.
    for _ in 0..4 {
        let o = logged_hook(&f, Some("sess-a"), "cargo test");
        assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stdout));
    }
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    // Another session's cargo run is a different command and keeps its own reservation.
    f.meminfo(9000 * 1024, 0);
    assert_eq!(code(&logged_hook(&f, Some("sess-b"), "cargo test")), 0);
    assert_eq!(admission_rows(&f).len(), 2);
    // A different toolchain of the same session is kept too.
    assert_eq!(code(&logged_hook(&f, Some("sess-a"), "./gradlew test")), 0);
    assert_eq!(admission_rows(&f).len(), 3);
}

#[test]
fn future_or_duplicated_admission_rows_cannot_reserve_forever() {
    let f = Fixture::new("0.5", 0);
    f.meminfo(4000 * 1024, 0);
    let future = serde_json::json!({
        "ts": now_secs() + 3600,
        "session": null,
        "toolchains": ["gradle"],
    });
    fs::create_dir_all(f.gate()).unwrap();
    fs::write(admissions_path(&f), format!("{future}\n")).unwrap();
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=none"), "{status}");
    assert_eq!(code(&hook_command(&f, "cargo test", f.temp.path())), 0);
    // A duplicated toolchain in one row counts once.
    let dup = serde_json::json!({
        "ts": now_secs(),
        "session": null,
        "toolchains": ["cargo", "cargo"],
    });
    fs::write(admissions_path(&f), format!("{dup}\n")).unwrap();
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=cargo:1536MB"), "{status}");
}

impl Fixture {
    fn machine(&self, cpus: usize, mem_total_kb: u64) {
        let cpuinfo: String = (0..cpus)
            .map(|i| format!("processor\t: {i}\nmodel name\t: test\n\n"))
            .collect();
        fs::write(self.proc().join("cpuinfo"), cpuinfo).expect("cpuinfo");
        let meminfo = fs::read_to_string(self.proc().join("meminfo")).expect("meminfo");
        fs::write(
            self.proc().join("meminfo"),
            format!("MemTotal: {mem_total_kb} kB\n{meminfo}"),
        )
        .expect("meminfo");
    }
}

/// Load follows the CPUs, and so does the status document: the keys `fleet_view` and the old
/// fleet manager read stay; the slot and compiler lock keys are gone, `holders` is new.
#[test]
fn limits_follow_the_machine() {
    let document = |f: &Fixture| {
        let out = f.run(&["status", "--json"]);
        serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("json")
    };
    // 16 CPUs, 32 GB: load = 16.
    let f = Fixture::new("0.5", 0);
    f.machine(16, 32_768_000);
    let v = document(&f);
    assert_eq!(v["limits"]["max_load"], 16.0);
    assert_eq!(v["limits"]["cpus"], 16);
    assert_eq!(v["limits"]["mem_total_mb"], 32000);
    for key in [
        "load_1m",
        "mem_available_mb",
        "swap_used_mb",
        "mem_pressure_some_avg10",
    ] {
        assert!(v.get(key).is_some(), "{key}");
    }
    for key in ["slots", "other_slots", "compiler_lock", "compiler_locks"] {
        assert!(v.get(key).is_none(), "{key}");
    }
    assert!(v["limits"].get("slots").is_none());
    assert!(v["limits"].get("other_slots").is_none());
    assert!(v["holders"].is_array());
    // Memory no longer caps a count; load 6 on 6 CPUs, as before.
    let f = Fixture::new("6.5", 0);
    f.machine(6, 8_192_000);
    assert_eq!(document(&f)["limits"]["max_load"], 6.0);
    let text = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(text.contains("max_load=6.00 [cpus 6x1]"), "{text}");
    assert!(!text.contains("slots="), "{text}");
    assert_eq!(code(&f.run(&["gate", "check"])), 75);
    // Load scales with CPUs: 6.5 is under 16 on a 16-CPU machine.
    let f = Fixture::new("6.5", 0);
    f.machine(16, 32_768_000);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "build"])), 0);
}
#[test]
fn environment_overrides_derived_limits() {
    let f = Fixture::new("0.5", 0);
    f.machine(16, 32_768_000);
    let o = f
        .cmd(&["status", "--json"])
        .env("CLAUDECAT_MAX_LOAD", "3")
        .output()
        .expect("status");
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["limits"]["max_load"], 3.0);
    assert_eq!(v["limits"]["max_load_basis"], "CLAUDECAT_MAX_LOAD");
}

/// An executable stand-in for `./gradlew`, so an allowed `gate run -- ./gradlew ...` exits 0.
fn fake_gradlew(f: &Fixture) {
    use std::os::unix::fs::PermissionsExt;
    let path = f.temp.path().join("gradlew");
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("gradlew");
    let mut permissions = fs::metadata(&path).expect("gradlew").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("gradlew mode");
}

fn reserved_status(f: &Fixture) -> String {
    String::from_utf8(f.run(&["status"]).stdout).unwrap()
}

#[test]
fn warming_up_gradle_test_reserves_one_estimate_per_fork() {
    let now = now_secs();
    // 7000MB available: two forks reserve 6144MB and leave 856MB, below cargo's 1536MB.
    let f = Fixture::new("0.5", 0);
    f.meminfo(7000 * 1024, 0);
    let _slot = hold_holder(
        &f,
        "slot-0.lock",
        77,
        &["./gradlew", "test", "-PtestForks=2"],
        now,
    );
    let _gradle = hold_holder(
        &f,
        "compiler-gradle.lock",
        77,
        &["./gradlew", "test", "-PtestForks=2"],
        now,
    );
    // One holder in two locks reserves once.
    assert!(reserved_status(&f).contains("reserved=gradle:6144MB"));
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=gradle:6144MB "), "{line}");
    assert!(line.contains("warm-up passes (at most 60s)"), "{line}");
    assert_eq!(line.lines().count(), 1);

    // A holder may run its Gradle command through a shell.
    let g = Fixture::new("0.5", 0);
    g.meminfo(7000 * 1024, 0);
    let _slot = hold_holder(
        &g,
        "slot-0.lock",
        78,
        &["sh", "-c", "./gradlew test -PtestForks=2"],
        now,
    );
    assert!(reserved_status(&g).contains("reserved=gradle:6144MB"));

    // The same holder asking for one fork leaves enough for cargo.
    let one = Fixture::new("0.5", 0);
    one.meminfo(7000 * 1024, 0);
    let _slot = hold_holder(
        &one,
        "slot-0.lock",
        77,
        &["./gradlew", "test", "-PtestForks=1"],
        now,
    );
    assert!(reserved_status(&one).contains("reserved=gradle:3072MB"));
    assert_eq!(code(&hook_command(&one, "cargo test", one.temp.path())), 0);
}

#[test]
fn gate_run_reserves_gradle_test_forks_also_through_a_shell() {
    let f = Fixture::new("0.5", 0);
    fake_gradlew(&f);
    // 4000MB available: two forks need 6144MB, one fork's 3072MB fits.
    let run = |command: &[&str]| {
        f.cmd(&["gate", "run", "--kind", "test", "--"])
            .args(command)
            .current_dir(f.temp.path())
            .output()
            .expect("gate run")
    };
    let denied = [
        vec!["./gradlew", "test", "-PtestForks=2"],
        vec!["sh", "-c", "./gradlew test -PtestForks=2"],
        vec!["bash", "-l", "-c", "cd x && ./gradlew test -PtestForks=2"],
    ];
    for command in &denied {
        let o = run(command);
        assert_eq!(
            code(&o),
            75,
            "{command:?} {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let line = String::from_utf8(o.stderr).unwrap();
        assert!(line.starts_with("gate denied (reserved) "), "{line}");
        assert!(line.contains("reserved=none"), "{line}");
    }
    let allowed = [
        vec!["./gradlew", "test", "-PtestForks=1"],
        // An invalid count is no statement, and the gate knows no machine here: one fork.
        vec!["./gradlew", "test", "-PtestForks=x"],
        // Not a test run.
        vec!["sh", "-c", "./gradlew build"],
    ];
    for command in &allowed {
        let o = run(command);
        assert_eq!(
            code(&o),
            0,
            "{command:?} {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn hook_denies_a_gradle_test_whose_forks_do_not_fit() {
    // The hook checks the new command's own need, not only what is already reserved: with
    // 4000MB available and nothing reserved, two forks (6144MB) are denied and one (3072MB) fits.
    let f = Fixture::new("0.5", 0);
    f.meminfo(4000 * 1024, 0);
    let line = deny_line(&hook_command(
        &f,
        "./gradlew test -PtestForks=2",
        f.temp.path(),
    ));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=none"), "{line}");
    assert_eq!(
        code(&hook_command(
            &f,
            "./gradlew test -PtestForks=1",
            f.temp.path()
        )),
        0
    );
    // Excluding the tests is no test run, even where the default would be 2 forks.
    let g = Fixture::new("0.5", 0);
    g.meminfo(4000 * 1024, 0);
    g.machine(16, 32_768_000);
    assert_eq!(
        code(&hook_command(&g, "./gradlew build -x test", g.temp.path())),
        0
    );
}

#[test]
fn gradle_test_admission_reserves_one_estimate_per_fork() {
    let f = Fixture::new("0.5", 0);
    // 20000MB available: 2 x 3072MB fits.
    f.meminfo(20_000 * 1024, 0);
    assert_eq!(
        code(&logged_hook(
            &f,
            Some("sess-a"),
            "./gradlew test -PtestForks=2"
        )),
        0
    );
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["toolchains"], serde_json::json!(["gradle"]));
    assert_eq!(rows[0]["gradle_forks"], 2);
    // Still no command text: only the fork count was added.
    assert_eq!(rows[0].as_object().unwrap().len(), 4, "{rows:?}");
    assert!(reserved_status(&f).contains("reserved=gradle:6144MB"));
    // 7000MB available leave 856MB after the 6144MB: below cargo's 1536MB.
    f.meminfo(7000 * 1024, 0);
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=gradle:6144MB "), "{line}");
    // No slot or lock holder exists, only the admission.
    assert!(line.contains("warm-up passes (at most 60s)"), "{line}");
    assert!(!line.contains("holders above"), "{line}");

    // The same session's next Gradle run replaces the row, forks included.
    f.meminfo(20_000 * 1024, 0);
    assert_eq!(code(&logged_hook(&f, Some("sess-a"), "./gradlew build")), 0);
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].get("gradle_forks").is_none(),
        "one fork is not written"
    );
    let status = reserved_status(&f);
    assert!(status.contains("reserved=gradle:3072MB"), "{status}");
    assert!(!status.contains("6144"), "{status}");
}

#[test]
fn gradle_test_forks_default_to_the_machine() {
    // 16 CPUs and 32 GB: the build's default is 2 forks. (`machine` adds MemTotal to the meminfo
    // that `meminfo` writes, so it goes last.)
    let f = Fixture::new("0.5", 0);
    f.meminfo(20_000 * 1024, 0);
    f.machine(16, 32_768_000);
    assert_eq!(code(&hook_command(&f, "./gradlew test", f.temp.path())), 0);
    let rows = admission_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["gradle_forks"], 2);
    assert!(reserved_status(&f).contains("reserved=gradle:6144MB"));
    // A command that states its forks wins over the default.
    fs::remove_file(admissions_path(&f)).unwrap();
    assert_eq!(
        code(&hook_command(
            &f,
            "./gradlew :backend:test -P testForks=3",
            f.temp.path()
        )),
        0
    );
    assert!(reserved_status(&f).contains("reserved=gradle:9216MB"));
    // Not a test run: one estimate, whatever the machine.
    fs::remove_file(admissions_path(&f)).unwrap();
    assert_eq!(code(&hook_command(&f, "./gradlew build", f.temp.path())), 0);
    let rows = admission_rows(&f);
    assert_eq!(rows[0].as_object().unwrap().len(), 3, "{rows:?}");
    assert!(reserved_status(&f).contains("reserved=gradle:3072MB"));

    // 2 CPUs, or 6 GB of memory: one fork.
    for (cpus, mem_total_kb) in [(2, 32_768_000), (16, 6_144_000)] {
        let g = Fixture::new("0.5", 0);
        g.machine(cpus, mem_total_kb);
        assert_eq!(code(&hook_command(&g, "./gradlew test", g.temp.path())), 0);
        let status = reserved_status(&g);
        assert!(status.contains("reserved=gradle:3072MB"), "{status}");
        assert!(!status.contains("6144"), "{status}");
    }
    // A machine the gate knows nothing about (no cpuinfo, no MemTotal): one fork as well.
    let g = Fixture::new("0.5", 0);
    assert_eq!(code(&hook_command(&g, "./gradlew test", g.temp.path())), 0);
    let rows = admission_rows(&g);
    assert_eq!(rows[0].as_object().unwrap().len(), 3, "{rows:?}");
    assert!(reserved_status(&g).contains("reserved=gradle:3072MB"));
}

#[test]
fn admission_rows_from_before_forks_reserve_one_estimate() {
    // 16 CPUs and 32 GB, where a new test run defaults to 2 forks: a row written by an older
    // binary states none and counts as one.
    let f = Fixture::new("0.5", 0);
    f.machine(16, 32_768_000);
    fs::create_dir_all(f.gate()).unwrap();
    let old = serde_json::json!({"ts": now_secs(), "session": null, "toolchains": ["gradle"]});
    fs::write(admissions_path(&f), format!("{old}\n")).unwrap();
    // 4000MB available: one estimate leaves 928MB, below cargo's 1536MB.
    let status = reserved_status(&f);
    assert!(status.contains("reserved=gradle:3072MB"), "{status}");
    assert!(!status.contains("6144"), "{status}");
    let line = deny_line(&hook_command(&f, "cargo test", f.temp.path()));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!(line.contains("reserved=gradle:3072MB "), "{line}");
}

/// Hold a registration the way `gate run` writes it: `run-<pid>-<16 hex digits>.lock`, locked,
/// with the holder JSON inside.
fn hold_run(f: &Fixture, pid: u32, kind: &str, command: &[&str], started_at: u64) -> File {
    fs::create_dir_all(f.gate()).expect("gate");
    let name = format!("run-{pid}-{pid:016x}.lock");
    let file = hold(f, &name);
    fs::write(
        f.gate().join(name),
        serde_json::json!({"pid": pid, "kind": kind, "command": command, "started_at": started_at})
            .to_string(),
    )
    .expect("holder");
    file
}

/// `holders` of `status --json`; empty when the document has no such key.
fn holders(f: &Fixture) -> Vec<serde_json::Value> {
    let out = f.run(&["status", "--json"]);
    serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("json")["holders"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Names of the `*.lock` files in the gate directory, sorted.
fn lock_files(f: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(f.gate())
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".lock"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn run_files(f: &Fixture) -> Vec<String> {
    lock_files(f)
        .into_iter()
        .filter(|name| name.starts_with("run-") && name.ends_with(".lock"))
        .collect()
}

/// An executable `name` on PATH for the children of `gate run`.
fn fake_tool(f: &Fixture, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = f.temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let path = bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect(name);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
}

/// `cmd` with the fixture's fake tools first on PATH.
fn with_fake_tools<'a>(f: &'a Fixture, cmd: &'a mut Command) -> &'a mut Command {
    cmd.env(
        "PATH",
        format!(
            "{}:{}",
            f.temp.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
}

/// Start a `gate run` whose command names cargo: a fake `cargo` prints `ready` once the run is
/// registered, then blocks until its stdin closes.
fn blocked_run(f: &Fixture, kind: &str, max_load: &str) -> std::process::Child {
    use std::io::BufRead;
    fake_tool(f, "cargo", "echo ready\nread line\nexit 0");
    let mut child = with_fake_tools(
        f,
        f.cmd(&[
            "gate",
            "run",
            "--kind",
            kind,
            "--",
            "sh",
            "-c",
            "cargo build",
        ])
        .env("CLAUDECAT_MAX_LOAD", max_load)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()),
    )
    .spawn()
    .expect("gate run");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready", "gate run was not allowed");
    child
}

/// The `reserved_load=` figure of a denial.
fn reserved_load_of(text: &str) -> f64 {
    text.split("reserved_load=")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|figure| figure.parse().ok())
        .unwrap_or_else(|| panic!("no reserved_load figure: {text}"))
}

/// Seconds the wait hint names: `the warm-up passes (at most Ns)`. A run started a moment ago
/// has up to a second less than the window left.
fn warmup_hint(line: &str) -> u64 {
    line.split("warm-up passes (at most ")
        .nth(1)
        .and_then(|rest| rest.split('s').next())
        .and_then(|secs| secs.parse().ok())
        .unwrap_or_else(|| panic!("no warm-up hint: {line}"))
}

/// `gate run --kind build -- sh -c 'cargo build'` under a load limit; the fake cargo exits 0.
fn cargo_run(f: &Fixture, max_load: &str) -> Output {
    fake_tool(f, "cargo", "exit 0");
    let mut cmd = f.cmd(&[
        "gate",
        "run",
        "--kind",
        "build",
        "--",
        "sh",
        "-c",
        "cargo build",
    ]);
    cmd.env("CLAUDECAT_MAX_LOAD", max_load).stdin(Stdio::null());
    with_fake_tools(f, &mut cmd).output().expect("gate run")
}

fn hook_at(f: &Fixture, session: Option<&str>, command: &str, max_load: &str) -> Output {
    let payload = serde_json::json!({"session_id":session,"cwd":f.temp.path(),"tool_input":{"command":command}});
    let mut child = f
        .cmd(&["gate", "hook"])
        .env("CLAUDECAT_MAX_LOAD", max_load)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .expect("payload");
    child.wait_with_output().expect("hook output")
}

/// 7-1: no cap. `CLAUDECAT_SLOTS=1` and `CLAUDECAT_OTHER_SLOTS=1` limit nothing: more runs than
/// that start together, all are allowed, and all are listed as holders.
#[test]
fn runs_have_no_count_limit_for_build_or_other() {
    for kind in ["build", "other"] {
        let f = Fixture::new("0.5", 0);
        let mut children: Vec<_> = (0..3)
            .map(|_| {
                f.cmd(&[
                    "gate",
                    "run",
                    "--kind",
                    kind,
                    "--",
                    "sh",
                    "-c",
                    "read line; exit 0",
                ])
                .env("CLAUDECAT_SLOTS", "1")
                .env("CLAUDECAT_OTHER_SLOTS", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("gate run")
            })
            .collect();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while holders(&f).len() < 3 && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let listed = holders(&f);
        assert_eq!(listed.len(), 3, "{kind}: {listed:?}");
        assert!(
            listed.iter().all(|h| h["kind"] == kind),
            "{kind}: {listed:?}"
        );
        for child in &mut children {
            drop(child.stdin.take());
        }
        for child in children {
            let out = child.wait_with_output().expect("gate run output");
            assert_eq!(
                code(&out),
                0,
                "{kind}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(holders(&f).is_empty(), "{kind}");
    }
}

/// 7-2: with memory ample, a warming-up reservation denies for `load` and for nothing else, and
/// a total equal to the limit passes. Holder and admission sources; the command's own need.
#[test]
fn reserved_load_denies_with_exactly_load_at_the_boundary() {
    let check = |f: &Fixture, max_load: &str| {
        let mut cmd = f.cmd(&["gate", "check", "--kind", "build"]);
        cmd.env("CLAUDECAT_MAX_LOAD", max_load);
        code(&cmd.output().expect("check"))
    };
    // A holder: 0.5 measured + 2.0 reserved, 0 needed.
    let f = Fixture::new("0.5", 0);
    let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now_secs());
    assert_eq!(check(&f, "2.5"), 0, "equal to the limit passes");
    assert_eq!(check(&f, "2.4"), 75);
    let o = f
        .cmd(&["gate", "check", "--kind", "build"])
        .env("CLAUDECAT_MAX_LOAD", "2.4")
        .output()
        .expect("check");
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(stderr.starts_with("gate denied (load) "), "{stderr}");
    assert!(
        stderr.contains("load=0.50/2.40 reserved_load=2.00 need_load=0.00"),
        "{stderr}"
    );
    // An admission reserves the same load.
    let g = Fixture::new("0.5", 0);
    assert_eq!(code(&hook_at(&g, Some("sess-a"), "cargo test", "6")), 0);
    assert_eq!(check(&g, "2.5"), 0);
    assert_eq!(check(&g, "2.4"), 75);
}

/// 7-2: hook against hook. One hook's admission is the next hook's reserved load; a session's
/// allowed run replaces its earlier admission, so its runs do not pile up.
#[test]
fn a_hook_sees_the_reserved_load_of_an_earlier_hooks_admission() {
    let f = Fixture::new("0.5", 0);
    // 0.5 + 0 + 2.0 needed.
    assert_eq!(code(&hook_at(&f, Some("sess-a"), "cargo test", "4.4")), 0);
    // 0.5 + 2.0 reserved + 2.0 needed = 4.5.
    let line = deny_line(&hook_at(&f, Some("sess-b"), "cargo test", "4.4"));
    assert!(line.starts_with("gate denied (load) "), "{line}");
    assert!(
        line.contains("load=0.50/4.40 reserved_load=2.00 need_load=2.00"),
        "{line}"
    );
    assert_eq!(code(&hook_at(&f, Some("sess-b"), "cargo test", "4.5")), 0);
    // Session a runs the same command three times: each new admission replaces the old one, so
    // 0.5 + 2.0 + 2.0 = 4.5 is the most that is ever counted, never 0.5 + 3 x 2.0.
    let g = Fixture::new("0.5", 0);
    for _ in 0..3 {
        assert_eq!(code(&hook_at(&g, Some("sess-a"), "cargo test", "4.5")), 0);
    }
    assert_eq!(admission_rows(&g).len(), 1);
}

/// 7-2: hook against gate run and gate run against hook. What either admits, the other sees.
#[test]
fn gate_run_and_hook_see_each_others_reserved_load() {
    let f = Fixture::new("0.5", 0);
    let mut holder = blocked_run(&f, "build", "6");
    // 0.5 + 2.0 reserved + 2.0 needed = 4.5.
    let denied = cargo_run(&f, "4.4");
    assert_eq!(code(&denied), 75);
    let stderr = String::from_utf8(denied.stderr).unwrap();
    assert!(stderr.starts_with("gate denied (load) "), "{stderr}");
    assert!(
        stderr.contains("reserved_load=2.00 need_load=2.00"),
        "{stderr}"
    );
    let line = deny_line(&hook_at(&f, Some("sess-a"), "cargo test", "4.4"));
    assert!(line.starts_with("gate denied (load) "), "{line}");
    drop(holder.stdin.take());
    assert!(holder.wait().expect("wait").success());
    assert_eq!(code(&cargo_run(&f, "4.4")), 0, "nothing reserved any more");

    assert_eq!(code(&hook_at(&f, Some("sess-b"), "cargo test", "4.4")), 0);
    let denied = cargo_run(&f, "4.4");
    assert_eq!(code(&denied), 75, "gate run against hook");
    let stderr = String::from_utf8(denied.stderr).unwrap();
    assert!(stderr.starts_with("gate denied (load) "), "{stderr}");
    assert_eq!(code(&cargo_run(&f, "4.5")), 0, "equal to the limit passes");
}

/// 7-2: the reservation decays linearly: halfway through the window half is left, after it
/// nothing. Holder and admission sources; `gate check --kind build` needs 0, so 0.5 + the
/// reservation is the whole load.
#[test]
fn reserved_load_decays_linearly_across_the_warmup_window() {
    for source in ["holder", "admission"] {
        let mut results = Vec::new();
        for (secs_ago, max_load) in [
            (0, "2.4"),
            (0, "2.5"),
            (30, "1.4"),
            (30, "1.5"),
            (120, "0.5"),
        ] {
            let f = Fixture::new("0.5", 0);
            let now = now_secs();
            let holder = (source == "holder")
                .then(|| hold_run(&f, 77, "build", &["cargo", "test"], now - secs_ago));
            if source == "admission" {
                fs::create_dir_all(f.gate()).unwrap();
                let row = serde_json::json!({"ts": now - secs_ago, "session": null, "toolchains": ["cargo"]});
                fs::write(admissions_path(&f), format!("{row}\n")).unwrap();
            }
            let mut cmd = f.cmd(&["gate", "check", "--kind", "build"]);
            cmd.env("CLAUDECAT_MAX_LOAD", max_load);
            // A load limit below the measured load denies `other` on its own; its message carries
            // the figure.
            let mut other = f.cmd(&["gate", "check", "--kind", "other"]);
            other.env("CLAUDECAT_MAX_LOAD", "0.4");
            let denied = other.output().unwrap();
            assert_eq!(code(&denied), 75);
            let figure = String::from_utf8(denied.stderr)
                .unwrap()
                .split("reserved_load=")
                .nth(1)
                .and_then(|rest| rest.split(' ').next())
                .unwrap_or_else(|| panic!("no reserved_load figure"))
                .to_owned();
            results.push(format!("{} {figure}", code(&cmd.output().unwrap())));
            drop(holder);
        }
        assert_eq!(
            results,
            ["75 2.00", "0 2.00", "75 1.00", "0 1.00", "0 0.00"],
            "{source}"
        );
    }
}

/// 7-2: `other` takes no reserved load; its protection is memory (the next test).
#[test]
fn other_does_not_take_reserved_load() {
    let f = Fixture::new("0.5", 0);
    let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now_secs());
    let at = |args: &[&str]| {
        let mut cmd = f.cmd(args);
        cmd.env("CLAUDECAT_MAX_LOAD", "2");
        code(&cmd.output().expect("claudecat"))
    };
    // 0.5 + 2.0 reserved is over 2 for a build, and 0.5 for other.
    assert_eq!(at(&["gate", "check", "--kind", "build"]), 75);
    assert_eq!(at(&["gate", "check", "--kind", "other"]), 0);
    assert_eq!(at(&["gate", "run", "--kind", "other", "--", "true"]), 0);
}

/// 7-3: one holder while the child lives, none after; the file is 0600 and removed.
#[test]
fn run_registers_one_holder_and_removes_it_after_the_child() {
    use std::io::BufRead;
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("0.5", 0);
    let script = f.temp.path().join("wait.sh");
    fs::write(&script, "#!/bin/sh\necho ready\nread line\nexit 0\n").expect("script");
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready");
    let files = run_files(&f);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("run-"), "{files:?}");
    let mode = fs::metadata(f.gate().join(&files[0]))
        .expect("registration")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let listed = holders(&f);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["pid"], u64::from(child.id()));
    assert_eq!(listed[0]["kind"], "test");
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    assert!(out.status.success());
    assert!(run_files(&f).is_empty(), "unlinked on exit");
    assert!(holders(&f).is_empty());
}

/// 7-3: a registration nobody holds any more (a gate run killed by SIGTERM) is removed, and it
/// never reserves.
#[test]
fn stale_registration_is_removed_and_not_counted() {
    let f = Fixture::new("0.5", 0);
    fs::create_dir_all(f.gate()).unwrap();
    let stale = f.gate().join("run-999-0123456789abcdef.lock");
    fs::write(
        &stale,
        serde_json::json!({"pid":999,"kind":"build","command":["cargo","test"],"started_at":now_secs()})
            .to_string(),
    )
    .unwrap();
    let _ = f.run(&["status"]);
    assert!(!stale.exists(), "status removes a stale registration");
    let mut cmd = f.cmd(&["gate", "check", "--kind", "build"]);
    cmd.env("CLAUDECAT_MAX_LOAD", "2.5");
    assert_eq!(
        code(&cmd.output().unwrap()),
        0,
        "one live reservation fits, two would not"
    );
    // A live registration next to it stays.
    let _live = hold_run(&f, 77, "build", &["cargo", "test"], now_secs());
    let _ = f.run(&["status"]);
    assert_eq!(holders(&f).len(), 1);
}

/// 7-3: a holder stamped in the future (clock step, hand edit) is listed, for it is held, but
/// reserves nothing until the clock catches up; a few seconds ahead is tolerated.
#[test]
fn future_holder_timestamp_reserves_nothing() {
    let f = Fixture::new("0.5", 0);
    let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now_secs() + 3600);
    let listed = holders(&f);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=none"), "{status}");
    let g = Fixture::new("0.5", 0);
    let _soon = hold_run(&g, 78, "build", &["cargo", "test"], now_secs() + 3);
    let status = String::from_utf8(g.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=cargo:1536MB"), "{status}");
}

/// 7-3, last point: that a registration takes its lock and syncs its holder before it carries the
/// listed name cannot be seen from outside, so it is asserted on the source. A weaker assertion:
/// it pins the text of `register`, not its behaviour.
#[test]
fn registration_locks_and_syncs_before_it_carries_the_listed_name() {
    let source =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gate.rs")).expect("gate.rs");
    let start = source
        .find("fn register(")
        .expect("gate.rs has a register function");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("end of register");
    let body = &body[..end];
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("{needle} is not in register():\n{body}"))
    };
    let (create, lock, sync, rename) = (
        at(".create_new(true)"),
        at(".lock()"),
        at(".sync_all()"),
        at("fs::rename("),
    );
    assert!(
        create < lock && lock < sync && sync < rename,
        "create_new, lock, sync_all, rename in that order:\n{body}"
    );
    assert!(
        body.contains(".reg-"),
        "the temporary name must not match run-*.lock"
    );
}

/// 7-4: the lock files of a gate run started before the upgrade count as holders, whatever their
/// index, and a pid with several of them counts once.
#[test]
fn old_lock_files_count_as_holders_once() {
    let f = Fixture::new("0.5", 0);
    let _other = hold_holder(&f, "other-0.lock", 77, &["python3", "x.py"], 1);
    let _slot7 = hold_holder(&f, "slot-7.lock", 78, &["make", "all"], 1);
    let _slot0 = hold_holder(&f, "slot-0.lock", 79, &["cargo", "test"], 1);
    let _compiler = hold_holder(&f, "compiler-cargo.lock", 79, &["cargo", "test"], 1);
    let mut pids: Vec<_> = holders(&f)
        .iter()
        .map(|h| h["pid"].as_u64().expect("pid"))
        .collect();
    pids.sort();
    assert_eq!(pids, [77, 78, 79]);
}

/// 7-5: Gradle test forks scale the load reservation; cargo's default follows the CPUs; both
/// follow the environment. Read off a load denial, which prints the figure.
#[test]
fn reserved_load_follows_the_toolchain_the_machine_and_the_environment() {
    type Case = (
        Option<(usize, u64)>,
        &'static [&'static str],
        &'static [(&'static str, &'static str)],
        &'static str,
    );
    let cases: [Case; 10] = [
        (
            None,
            &["./gradlew", "test", "-PtestForks=3"],
            &[],
            "reserved_load=3.00",
        ),
        (None, &["./gradlew", "test"], &[], "reserved_load=1.00"),
        (None, &["./gradlew", "build"], &[], "reserved_load=1.00"),
        (None, &["cargo", "test"], &[], "reserved_load=2.00"),
        (None, &["python3", "x.py"], &[], "reserved_load=0.00"),
        // 16 CPUs and 32 GB: two forks by default, cargo max(2.0, 16 x 0.25).
        (
            Some((16, 32_768_000)),
            &["./gradlew", "test"],
            &[],
            "reserved_load=2.00",
        ),
        (
            Some((16, 32_768_000)),
            &["cargo", "test"],
            &[],
            "reserved_load=4.00",
        ),
        // 6 CPUs: max(2.0, 6 x 0.25) stays 2.0.
        (
            Some((6, 8_192_000)),
            &["cargo", "test"],
            &[],
            "reserved_load=2.00",
        ),
        (
            None,
            &["./gradlew", "test", "-PtestForks=3"],
            &[("CLAUDECAT_RESERVE_GRADLE_LOAD", "2.5")],
            "reserved_load=7.50",
        ),
        (
            None,
            &["cargo", "test"],
            &[("CLAUDECAT_RESERVE_CARGO_LOAD", "3.5")],
            "reserved_load=3.50",
        ),
    ];
    for (machine, holder, env, expected) in cases {
        let f = Fixture::new("7", 0);
        // Plenty of memory, so that only the load denies.
        f.meminfo(1_000_000 * 1024, 0);
        if let Some((cpus, mem_total_kb)) = machine {
            f.machine(cpus, mem_total_kb);
        }
        let _lock = hold_run(&f, 77, "build", holder, now_secs());
        // Whatever the machine, a limit of 6 is over the measured 7.
        let mut cmd = f.cmd(&["gate", "check", "--kind", "other"]);
        cmd.env("CLAUDECAT_MAX_LOAD", "6");
        for (key, value) in env {
            cmd.env(key, value);
        }
        let o = cmd.output().expect("check");
        let status = code(&o);
        let stderr = String::from_utf8(o.stderr).unwrap();
        assert_eq!(status, 75, "{holder:?}: {stderr}");
        assert!(
            stderr.starts_with("gate denied (load) "),
            "{holder:?}: {stderr}"
        );
        assert!(stderr.contains(expected), "{holder:?} {env:?}: {stderr}");
    }
}

/// 7-6: the stderr contract. The `gate denied (<reason>) ` prefix stays; `slots=` and
/// `compiler_lock=` are gone; every denial prints the load and what is reserved and needed.
#[test]
fn deny_messages_keep_the_gate_denied_prefix() {
    let f = Fixture::new("7", 0);
    for args in [
        vec!["gate", "check", "--kind", "other"],
        vec!["gate", "check", "--kind", "test"],
        vec!["gate", "run", "--kind", "other", "--", "true"],
        vec!["gate", "run", "--kind", "test", "--", "true"],
    ] {
        let o = f.run(&args);
        assert_eq!(code(&o), 75, "{args:?}");
        let stderr = String::from_utf8(o.stderr).unwrap();
        assert!(
            stderr.starts_with("gate denied (load) "),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains("load=7.00/6.00 reserved_load=0.00 need_load=0.00"),
            "{args:?}: {stderr}"
        );
        assert!(!stderr.contains("slots="), "{args:?}: {stderr}");
        assert!(!stderr.contains("compiler_lock="), "{args:?}: {stderr}");
    }
    let m = Fixture::new("0.5", 0);
    m.pressure("10.01");
    let o = m.run(&["gate", "check"]);
    assert_eq!(code(&o), 75);
    assert!(String::from_utf8(o.stderr)
        .unwrap()
        .starts_with("gate denied (memory) "));
    let clear = Fixture::new("0.5", 0);
    assert_eq!(code(&clear.run(&["gate", "check", "--kind", "test"])), 0);
    assert_eq!(
        code(&clear.run(&["gate", "run", "--kind", "other", "--", "true"])),
        0
    );
}

/// 7-6, new behaviour: the gate judges resources only. A running rustc decides nothing, and
/// neither does a compiler lock file a pre-upgrade gate run still holds; `check --kind test` used
/// to answer 75 for both.
#[test]
fn running_compilers_and_old_locks_decide_nothing() {
    let f = Fixture::new("0.5", 0);
    f.process(123, "rustc", &["rustc", "main.rs"], 99);
    let _cargo = hold_holder(
        &f,
        "compiler-cargo.lock",
        77,
        &["cargo", "test"],
        now_secs(),
    );
    for kind in ["build", "test", "e2e", "other"] {
        let o = f.run(&["gate", "check", "--kind", kind]);
        assert_eq!(
            code(&o),
            0,
            "{kind}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    // Status still shows the compiler, as information, and the lock as a holder.
    let v: serde_json::Value =
        serde_json::from_slice(&f.run(&["status", "--json"]).stdout).unwrap();
    assert_eq!(v["compiling"][0]["pid"], 123);
    assert_eq!(holders(&f).len(), 1);
}

/// 4.3, shared critical section: `gate run` checks and registers under `admission.lock`, so it
/// waits for whoever holds it.
#[test]
fn gate_run_decides_and_registers_inside_the_admission_lock() {
    let f = Fixture::new("0.5", 0);
    let busy = hold(&f, "admission.lock");
    let marker = f.temp.path().join("marker");
    let child = f
        .cmd(&[
            "gate",
            "run",
            "--",
            "touch",
            marker.to_str().expect("marker path"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("gate run");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!marker.exists(), "gate run must wait for admission.lock");
    drop(busy);
    let out = child.wait_with_output().expect("gate run output");
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(marker.exists());
}

/// 4.3: when the lock stays busy past the wait budget, `gate run` warns and runs, as it does for
/// any other gate failure.
#[test]
fn gate_run_warns_and_executes_when_the_admission_lock_stays_busy() {
    let f = Fixture::new("0.5", 0);
    let _busy = hold(&f, "admission.lock");
    let marker = f.temp.path().join("marker");
    let o = f.run(&[
        "gate",
        "run",
        "--",
        "touch",
        marker.to_str().expect("marker path"),
    ]);
    assert_eq!(code(&o), 0);
    assert!(marker.exists());
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("gate warning: "), "{stderr}");
}

/// 4.3: an `other` run reserves `CLAUDECAT_RESERVE_OTHER_MB` (1024) while it warms up, and a new
/// `other` must find that much beyond the reservations. Nothing counts runs.
#[test]
fn other_is_held_back_by_memory_reservations_not_by_count() {
    // A full pool of pre-upgrade `other-N.lock` files denies nothing.
    let f = Fixture::new("0.5", 0);
    let _other: Vec<_> = (0..3u32)
        .map(|i| {
            hold_holder(
                &f,
                &format!("other-{i}.lock"),
                70 + i,
                &["python3", "x.py"],
                1,
            )
        })
        .collect();
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "other"])), 0);
    assert_eq!(code(&f.run(&["gate", "check", "--kind", "test"])), 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "true"])), 0);

    // 4000MB available: three other runs still warming up reserve 3072MB and leave 928MB, below the 1024MB a new other needs.
    let g = Fixture::new("0.5", 0);
    let [a, b, c] =
        [100, 101, 102].map(|pid| hold_run(&g, pid, "other", &["python3", "x.py"], now_secs()));
    let o = g.run(&["gate", "check", "--kind", "other"]);
    assert_eq!(code(&o), 75);
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(stderr.starts_with("gate denied (reserved) "), "{stderr}");
    assert!(stderr.contains("reserved=other:3072MB "), "{stderr}");
    let marker = g.temp.path().join("marker");
    let o = g.run(&["gate", "run", "--", "touch", marker.to_str().unwrap()]);
    assert_eq!(code(&o), 75);
    assert!(!marker.exists());
    // A build that names no command needs nothing reserved.
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "build"])), 0);
    // Two reservations leave 1952MB.
    drop(c);
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "other"])), 0);
    drop(b);
    drop(a);

    // Past the warm-up window an other run reserves nothing.
    let h = Fixture::new("0.5", 0);
    let _old = hold_run(&h, 110, "other", &["python3", "x.py"], now_secs() - 120);
    assert_eq!(code(&h.run(&["gate", "check", "--kind", "other"])), 0);
    // The estimate is adjustable.
    let i = Fixture::new("0.5", 0);
    let _three: Vec<_> = (0..3u32)
        .map(|k| hold_run(&i, 120 + k, "other", &["python3", "x.py"], now_secs()))
        .collect();
    let o = i
        .cmd(&["gate", "check", "--kind", "other"])
        .env("CLAUDECAT_RESERVE_OTHER_MB", "512")
        .output()
        .unwrap();
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
}

/// 4.1: `gate run --kind other -- sh -c 'cargo test'` is checked on the `other` row, but what it
/// registers reserves as cargo, by its command.
#[test]
fn an_other_run_of_cargo_is_checked_as_other_and_reserves_as_cargo() {
    let f = Fixture::new("0.5", 0);
    // As a build, 0.5 + 2.0 would be over 2.
    let mut run = blocked_run(&f, "other", "2");
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=cargo:1536MB"), "{status}");
    drop(run.stdin.take());
    assert!(run.wait().expect("wait").success());
}

/// 4.2-8: the gate directory sits under a predictable `/tmp` path, so it is created 0700 (parents
/// too), and `status` points out one that is more open -- on stderr, never inside the JSON.
#[test]
fn gate_dir_is_private_and_status_flags_an_open_one() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("0.5", 0);
    assert_eq!(code(&f.run(&["gate", "run", "--", "true"])), 0);
    assert_eq!(
        fs::metadata(f.gate()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let o = f.run(&["status"]);
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    fs::set_permissions(f.gate(), fs::Permissions::from_mode(0o755)).unwrap();
    let o = f.run(&["status"]);
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(
        stderr.contains("gate directory") && stderr.contains("chmod"),
        "{stderr}"
    );
    let o = f.run(&["status", "--json"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("gate directory"));
    serde_json::from_slice::<serde_json::Value>(&o.stdout).expect("the notice stays off stdout");
    // Parent directories it has to create are private as well.
    let g = Fixture::new("0.5", 0);
    let nested = g.temp.path().join("a/b/gate");
    let mut cmd = g.cmd(&["gate", "run", "--", "true"]);
    cmd.env("CLAUDECAT_GATE_DIR", &nested);
    assert_eq!(code(&cmd.output().unwrap()), 0);
    for dir in ["a", "a/b", "a/b/gate"] {
        assert_eq!(
            fs::metadata(g.temp.path().join(dir))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "{dir}"
        );
    }
}

/// 4.1: `CLAUDECAT_SLOTS` and `CLAUDECAT_OTHER_SLOTS` have no effect any more; the text `status`
/// says so once, on stderr.
#[test]
fn slot_variables_no_longer_apply_and_status_says_so() {
    let f = Fixture::new("0.5", 0);
    let mut text = f.cmd(&["status"]);
    text.env("CLAUDECAT_SLOTS", "1")
        .env("CLAUDECAT_OTHER_SLOTS", "1");
    let o = text.output().unwrap();
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("CLAUDECAT_SLOTS"), "{stderr}");
    assert!(stderr.contains("CLAUDECAT_OTHER_SLOTS"), "{stderr}");
    let mut json = f.cmd(&["status", "--json"]);
    json.env("CLAUDECAT_SLOTS", "1")
        .env("CLAUDECAT_OTHER_SLOTS", "1");
    let o = json.output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert!(!String::from_utf8_lossy(&o.stdout).contains("CLAUDECAT_SLOTS"));
    for key in ["slots", "other_slots", "compiler_lock", "compiler_locks"] {
        assert!(v.get(key).is_none(), "{key}");
    }
    assert!(v["limits"].get("slots").is_none());
    assert!(v["limits"].get("other_slots").is_none());
    assert!(v["holders"].is_array());
}

/// 4.1: a denial the reservations cause tells the agent how long the warm-up can last instead of
/// pointing at holders; with nothing reserved the wait is for load and memory pressure.
#[test]
fn hook_wait_hint_names_the_warmup_when_a_reservation_denies() {
    let f = Fixture::new("0.5", 0);
    assert_eq!(code(&hook_at(&f, Some("sess-a"), "cargo test", "4.4")), 0);
    // Load: 0.5 + 2.0 + 2.0.
    let line = deny_line(&hook_at(&f, Some("sess-b"), "cargo test", "4.4"));
    assert!(
        line.ends_with("; do not poll with sleep in the foreground.\n"),
        "{line}"
    );
    assert!((59..=60).contains(&warmup_hint(&line)), "{line}");
    // Memory: 4000 - 1536 = 2464MB, below Gradle's 3072MB.
    let line = deny_line(&hook_at(&f, Some("sess-c"), "./gradlew test", "4.4"));
    assert!((59..=60).contains(&warmup_hint(&line)), "{line}");
    // Twenty seconds into the window: forty left, and 0.5 + 2.0 x 40/60 + 2.0 = 3.83.
    let mut row = admission_rows(&f)[0].clone();
    row["ts"] = serde_json::json!(now_secs() - 20);
    fs::write(admissions_path(&f), format!("{row}\n")).unwrap();
    let line = deny_line(&hook_at(&f, Some("sess-b"), "cargo test", "3.7"));
    assert!((39..=40).contains(&warmup_hint(&line)), "{line}");
    let busy = Fixture::new("7", 0);
    let line = deny_line(&logged_hook(&busy, Some("sess-a"), "cargo test"));
    assert!(
        line.ends_with("Retry it after load and memory pressure drop (figures above); do not poll with sleep in the foreground.\n"),
        "{line}"
    );
}

/// 4.3: a run checked on the `other` row reserves `CLAUDECAT_RESERVE_OTHER_MB` whatever kind it
/// declared. `gate run --kind test -- python3 …` names no toolchain: nothing else would hold
/// memory back for its holder, and a burst of such runs would all pass. The holder keeps the kind
/// it declared.
#[test]
fn other_row_runs_reserve_other_memory_whatever_kind_they_declare() {
    use std::io::BufRead;
    // 4000MB available: three such runs reserve 3072MB and leave 928MB, below the 1024MB the next needs.
    let f = Fixture::new("0.5", 0);
    let _runs: Vec<_> = [200, 201, 202]
        .into_iter()
        .map(|pid| hold_run(&f, pid, "test", &["python3", "x.py"], now_secs()))
        .collect();
    let o = f.run(&["gate", "check", "--kind", "other"]);
    assert_eq!(code(&o), 75, "{}", String::from_utf8_lossy(&o.stderr));
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(stderr.contains("reserved=other:3072MB "), "{stderr}");
    // Declared `test` like the ones above, but `make` may drive a compiler: it was checked on the
    // build row, where a command that names no toolchain reserves nothing.
    let g = Fixture::new("0.5", 0);
    let _make = hold_run(&g, 203, "test", &["make", "test"], now_secs());
    assert_eq!(code(&g.run(&["gate", "check", "--kind", "other"])), 0);

    // The same through a real `gate run --kind test -- python3 …`, listed as the kind it declared.
    let h = Fixture::new("0.5", 0);
    fake_tool(&h, "python3", "echo ready\nread line\nexit 0");
    let mut child = with_fake_tools(
        &h,
        h.cmd(&["gate", "run", "--kind", "test", "--", "python3", "x.py"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .spawn()
    .expect("gate run");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready", "gate run was not allowed");
    let listed = holders(&h);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["kind"], "test");
    let status = String::from_utf8(h.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=other:1024MB"), "{status}");
    drop(child.stdin.take());
    assert!(child.wait().expect("wait").success());
}

/// 4.1: a denial that only the `other` memory reservations cause also tells how long the warm-up
/// can last, instead of "at most 0s".
#[test]
fn hook_wait_hint_names_the_warmup_when_only_other_reservations_deny() {
    // 4000MB available; three other runs warming up reserve 3072MB, leaving 928MB for cargo's 1536MB.
    let f = Fixture::new("0.5", 0);
    let _runs: Vec<_> = [200, 201, 202]
        .into_iter()
        .map(|pid| hold_run(&f, pid, "other", &["python3", "x.py"], now_secs()))
        .collect();
    let line = deny_line(&hook_at(&f, Some("sess-a"), "cargo test", "4.4"));
    assert!(line.starts_with("gate denied (reserved) "), "{line}");
    assert!((59..=60).contains(&warmup_hint(&line)), "{line}");
}

/// 4.3: a child never runs under `admission.lock`. When its registration fails, `gate run` warns
/// and runs -- and must let go of the lock first, or every other run and hook waits out its budget
/// for as long as the child lives.
#[test]
fn gate_run_lets_go_of_the_admission_lock_when_its_registration_fails() {
    use std::io::BufRead;
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("0.5", 0);
    // The lock file is created first (opening an existing file needs no write permission on the
    // directory); read-only, the directory takes no registration.
    drop(hold(&f, "admission.lock"));
    fs::set_permissions(f.gate(), fs::Permissions::from_mode(0o500)).unwrap();
    if File::create(f.gate().join("probe")).is_ok() {
        eprintln!("skipped: the gate directory stays writable (running as root?)");
        return;
    }
    fake_tool(&f, "slow", "echo ready\nread line\nexit 0");
    let mut first = with_fake_tools(
        &f,
        f.cmd(&["gate", "run", "--kind", "build", "--", "slow"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .spawn()
    .expect("gate run");
    let mut line = String::new();
    std::io::BufReader::new(first.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("ready");
    assert_eq!(line.trim(), "ready", "gate run was not allowed");

    // The child is running. Writable again, so the next ones can register and record.
    fs::set_permissions(f.gate(), fs::Permissions::from_mode(0o700)).unwrap();
    let marker = f.temp.path().join("marker");
    let second = f.run(&[
        "gate",
        "run",
        "--",
        "touch",
        marker.to_str().expect("marker path"),
    ]);
    let stderr = String::from_utf8_lossy(&second.stderr).into_owned();
    assert_eq!(code(&second), 0, "{stderr}");
    assert!(marker.exists());
    assert!(
        !stderr.contains("gate warning"),
        "the second gate run had to wait for the lock: {stderr}"
    );
    assert_eq!(code(&hook_at(&f, Some("sess-a"), "cargo test", "4.4")), 0);
    assert_eq!(
        admission_rows(&f).len(),
        1,
        "the hook could not take the lock to record its admission"
    );

    drop(first.stdin.take());
    let out = first.wait_with_output().expect("gate run output");
    assert_eq!(code(&out), 0);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("gate warning: "),
        "the first run's registration was to fail"
    );
}

/// 4.3, shared critical section: a `gate run` takes its snapshot before it waits for
/// `admission.lock`. What is registered while it waits must still deny it: the registry is read
/// again once the lock is held. Without that second reading the run would decide on a snapshot as
/// old as its wait.
#[test]
fn gate_run_waiting_for_the_lock_sees_what_was_registered_meanwhile() {
    for _ in 0..20 {
        let f = Fixture::new("0.5", 0);
        let now = now_secs();
        let busy = hold(&f, "admission.lock");
        fake_tool(&f, "cargo", "exit 0");
        let mut command = f.cmd(&[
            "gate",
            "run",
            "--kind",
            "build",
            "--",
            "sh",
            "-c",
            "cargo build",
        ]);
        command
            .env("CLAUDECAT_MAX_LOAD", "4.4")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = with_fake_tools(&f, &mut command).spawn().expect("gate run");
        // Long enough for the run to have taken its lock-less snapshot and be waiting, far shorter
        // than the 2s it waits.
        std::thread::sleep(std::time::Duration::from_millis(300));
        // 0.5 + 2.0 reserved + 2.0 needed = 4.5.
        let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now);
        drop(busy);
        let out = child.wait_with_output().expect("gate run output");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        match code(&out) {
            75 => {
                assert!(stderr.starts_with("gate denied (load) "), "{stderr}");
                // One cargo run reserved: 2.0, or 2.0 x 59/60 should the clock have passed into
                // the next second since the holder was stamped.
                let reserved_load = reserved_load_of(&stderr);
                assert!((1.9..=2.0).contains(&reserved_load), "{stderr}");
                return;
            }
            // The run gave up waiting: too loaded to order the two. Again.
            0 if stderr.contains("gate warning") => continue,
            status => panic!(
                "gate run (status {status}) was allowed although a reserving holder was registered while it waited for the lock: {stderr}"
            ),
        }
    }
    panic!("gate run never waited for admission.lock in 20 attempts");
}

/// 4.3, shared critical section, the hook: its snapshot is taken without the lock as well, and it
/// waits for the lock for 50ms at most. An admission written while it waits must still deny it.
///
/// The hook cannot be seen waiting, so the attempt is ordered by time and repeated when it was
/// not: a hook that gave up waiting decides on its lock-less snapshot and records nothing, which
/// is told apart from one that took the lock and admitted its own row after ours.
#[test]
fn hook_waiting_for_the_lock_sees_what_was_admitted_meanwhile() {
    for _ in 0..40 {
        let f = Fixture::new("0.5", 0);
        let ts = now_secs();
        let busy = hold(&f, "admission.lock");
        let payload = serde_json::json!({"session_id":"sess-b","cwd":f.temp.path(),"tool_input":{"command":"cargo test"}});
        let mut hook = f
            .cmd(&["gate", "hook"])
            .env("CLAUDECAT_MAX_LOAD", "4.4")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("hook");
        {
            let mut stdin = hook.stdin.take().expect("stdin");
            stdin
                .write_all(payload.to_string().as_bytes())
                .expect("payload");
            // Closing stdin is when the hook's work starts: a sharp moment, without the start-up
            // of the process before it.
        }
        // A few milliseconds: the hook has its snapshot and is waiting for the lock.
        std::thread::sleep(std::time::Duration::from_millis(10));
        // 0.5 + 2.0 reserved + 2.0 needed = 4.5. Another session's, so a hook that admits keeps it.
        let row = serde_json::json!({"ts": ts, "session": null, "toolchains": ["cargo"]});
        fs::write(admissions_path(&f), format!("{row}\n")).unwrap();
        drop(busy);
        let out = hook.wait_with_output().expect("hook output");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        match code(&out) {
            75 => {
                assert!(stdout.starts_with("gate denied (load) "), "{stdout}");
                // One cargo run reserved: 2.0, or 2.0 x 59/60 should the clock have passed into
                // the next second since the row was stamped.
                let reserved_load = reserved_load_of(&stdout);
                assert!((1.9..=2.0).contains(&reserved_load), "{stdout}");
                return;
            }
            0 if admission_rows(&f).len() >= 2 => panic!(
                "the hook took the lock and was allowed although a reserving admission was written before that: {stdout}"
            ),
            // The hook gave up waiting: too loaded to order the two. Again.
            0 => continue,
            status => panic!("hook exited {status}: {stdout}"),
        }
    }
    panic!("the hook never waited for admission.lock in 40 attempts");
}

/// 4.2-4: a lock file reached through a symlink is nobody's registration. The behaviour shows for
/// a symlink that is there to stay; a link put in its place between the look at the path and the
/// open would need a second user racing the gate, and cannot be seen from a test.
#[test]
fn a_lock_file_reached_through_a_symlink_is_no_holder() {
    let f = Fixture::new("0.5", 0);
    let _real = hold_holder(&f, "target.lock", 77, &["cargo", "test"], now_secs());
    symlink(
        f.gate().join("target.lock"),
        f.gate().join("run-7-0000000000000001.lock"),
    )
    .unwrap();
    assert!(holders(&f).is_empty());
    let status = String::from_utf8(f.run(&["status"]).stdout).unwrap();
    assert!(status.contains("reserved=none"), "{status}");
    assert!(f.gate().join("run-7-0000000000000001.lock").exists());
}

/// 4.2-4: only the file the gate opened may say who owns a lock file, and the open goes through
/// no symlink. Only a second user racing the open could show the difference, so this asserts the
/// text of `holders`: a weaker assertion than one of the behaviour.
#[test]
fn holders_open_through_no_symlink_and_check_the_file_they_opened() {
    let source =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gate.rs")).expect("gate.rs");
    let start = source
        .find("fn holders(")
        .expect("gate.rs has a holders function");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("end of holders");
    let body = &body[..end];
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("{needle} is not in holders():\n{body}"))
    };
    let (open, opened, owner) = (
        at(".open(&path)"),
        at("file.metadata()"),
        at(".uid() != uid"),
    );
    assert!(
        open < opened && opened < owner,
        "open, then the metadata of the file opened, then the owner check:\n{body}"
    );
    let before = &body[..open];
    assert!(
        before.contains("libc::O_NOFOLLOW"),
        "the open must refuse to follow a symlink:\n{body}"
    );
    assert!(
        before.contains("fs::symlink_metadata(&path)") && before.contains(".is_file()"),
        "whatever is no regular file is left unopened (the open of a FIFO blocks):\n{body}"
    );
    assert!(
        body[opened..].contains("!metadata.is_file()"),
        "the file opened must be a regular file:\n{body}"
    );
    assert!(
        !body.contains("fs::metadata("),
        "nothing in holders() may look at the path through a symlink:\n{body}"
    );
}

/// 4.4: every figure of `capacity`, from a machine and a registry whose contents are known. The
/// warm-up window is 6000s and whatever is warming sits halfway through it, so each reservation
/// counts at half and a second more or less of clock changes nothing at two decimals.
#[test]
fn status_capacity_reports_every_figure() {
    let f = Fixture::new("1.2", 0);
    f.meminfo(10_000 * 1024, 0);
    f.machine(8, 16_384_000);
    f.pressure("0.50");
    let now = now_secs();
    let half = now - 3000;
    // Load 1.0 and 1536MB; Gradle two forks: load 1.0 and 6144MB; an `other` run: 1024MB and no
    // load; a run that names no toolchain: warming, but it reserves nothing.
    let _cargo = hold_run(&f, 77, "build", &["cargo", "test"], half);
    let _gradle = hold_run(
        &f,
        78,
        "test",
        &["./gradlew", "test", "-PtestForks=2"],
        half,
    );
    let _other = hold_run(&f, 79, "other", &["sh", "-c", "sleep 100"], half);
    let _plain = hold_run(&f, 80, "build", &["sleep", "100"], half);
    // Past the window on both sides: neither counts.
    let _old = hold_run(&f, 81, "build", &["cargo", "test"], now - 9000);
    fs::create_dir_all(f.gate()).unwrap();
    let rows = [
        serde_json::json!({"ts": half, "session": "s1", "toolchains": ["cargo"]}),
        serde_json::json!({"ts": now - 9000, "session": "s2", "toolchains": ["cargo"]}),
    ];
    fs::write(admissions_path(&f), format!("{}\n{}\n", rows[0], rows[1])).unwrap();
    let (text, v) = status_pair(&f, &[("CLAUDECAT_RESERVE_WARMUP_SECS", "6000")]);
    let cap = &v["capacity"];
    assert_eq!(
        *cap,
        serde_json::json!({
            "cpus": 8, "mem_total_mb": 16000, "max_load": 8.0, "load_1m": 1.2,
            "reserved_load": 3.0, "load_headroom": 3.8, "mem_available_mb": 10000,
            "reserved_mb": 10240, "mem_headroom_mb": 0, "mem_pressure": 0.5,
            "blocked": [], "warming": 5,
        }),
        "{v}"
    );
    let mut keys: Vec<&str> = v["capacity"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "blocked",
            "cpus",
            "load_1m",
            "load_headroom",
            "max_load",
            "mem_available_mb",
            "mem_headroom_mb",
            "mem_pressure",
            "mem_total_mb",
            "reserved_load",
            "reserved_mb",
            "warming",
        ]
    );
    // The keys that stayed at the top say the same.
    for (top, inside) in [
        ("load_1m", "load_1m"),
        ("mem_available_mb", "mem_available_mb"),
        ("mem_pressure_some_avg10", "mem_pressure"),
        ("limits.max_load", "max_load"),
        ("limits.cpus", "cpus"),
        ("limits.mem_total_mb", "mem_total_mb"),
    ] {
        let top = top.split('.').fold(&v, |at, key| &at[key]);
        assert_eq!(top, &cap[inside], "{top} vs capacity.{inside}");
    }
    // The text prints the same figures.
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("capacity: "))
        .collect();
    assert_eq!(
        lines,
        [
            "capacity: cpus=8 mem_total=16000MB max_load=8.00 load_1m=1.20 reserved_load=3.00 load_headroom=3.80",
            "capacity: mem_available=10000MB reserved=10240MB mem_headroom=0MB psi=0.50 blocked=none warming=5",
        ],
        "{text}"
    );
}

/// 4.4: a machine that tells nothing of its CPUs, its memory total or its memory pressure reports
/// null there, and the figures that need none of them are still reported.
#[test]
fn capacity_is_null_where_the_machine_tells_nothing() {
    let f = Fixture::new("0.5", 0);
    let _other = hold_run(&f, 77, "other", &["sleep", "100"], now_secs());
    let (text, v) = status_pair(&f, &[]);
    assert_eq!(
        v["capacity"],
        serde_json::json!({
            "cpus": null, "mem_total_mb": null, "max_load": 6.0, "load_1m": 0.5,
            "reserved_load": 0.0, "load_headroom": 5.5, "mem_available_mb": 4000,
            "reserved_mb": 1024, "mem_headroom_mb": 2976, "mem_pressure": null,
            "blocked": [], "warming": 1,
        }),
        "{v}"
    );
    assert!(
        text.contains("capacity: cpus=? mem_total=? max_load=6.00 load_1m=0.50 reserved_load=0.00 load_headroom=5.50\n"),
        "{text}"
    );
    assert!(
        text.contains("capacity: mem_available=4000MB reserved=1024MB mem_headroom=2976MB psi=n/a blocked=none warming=1\n"),
        "{text}"
    );
}

/// 4.4: `blocked` is what denies a build or test run that needs nothing of its own right now -- the
/// gate's own reasons, not a second set of thresholds -- and it names memory pressure even while
/// memory headroom is plentiful.
#[test]
fn blocked_names_what_denies_a_build_with_no_need_right_now() {
    let capacity = |f: &Fixture, env: &[(&str, &str)]| status_pair(f, env).1["capacity"].clone();
    // PSI over the limit while memory headroom is plentiful.
    let f = Fixture::new("0.5", 0);
    f.pressure("10.01");
    let cap = capacity(&f, &[]);
    assert_eq!(cap["blocked"], serde_json::json!(["memory"]), "{cap}");
    assert_eq!(cap["mem_headroom_mb"], 4000);
    assert_eq!(cap["load_headroom"], 5.5);
    // MemAvailable under the floor, with no PSI file at all.
    let f = Fixture::new("0.5", 0);
    f.meminfo(1500 * 1024, 0);
    assert_eq!(capacity(&f, &[])["blocked"], serde_json::json!(["memory"]));
    // The measured load over the limit.
    let f = Fixture::new("7", 0);
    let cap = capacity(&f, &[]);
    assert_eq!(cap["blocked"], serde_json::json!(["load"]));
    assert_eq!(cap["load_headroom"], -1.0);
    // Reserved load pushing it over: 0.5 + 1.0 (a cargo run, half decayed) > 1.4.
    let f = Fixture::new("0.5", 0);
    let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now_secs() - 3000);
    let cap = capacity(
        &f,
        &[
            ("CLAUDECAT_RESERVE_WARMUP_SECS", "6000"),
            ("CLAUDECAT_MAX_LOAD", "1.4"),
        ],
    );
    assert_eq!(cap["reserved_load"], 1.0);
    assert_eq!(cap["load_headroom"], -0.1);
    assert_eq!(cap["blocked"], serde_json::json!(["load"]));
    // Both, in the order of the gate's message.
    let f = Fixture::new("7", 0);
    f.pressure("10.01");
    assert_eq!(
        capacity(&f, &[])["blocked"],
        serde_json::json!(["load", "memory"])
    );
    // The gate agrees: `gate check --kind build` needs nothing, so it is denied exactly when
    // `blocked` is not empty. A boundary equal to the limit passes.
    for (load, psi, available_mb, denied) in [
        ("0.5", "0.02", 4000, false),
        ("6", "0.02", 4000, false),
        ("6.01", "0.02", 4000, true),
        ("0.5", "10.01", 4000, true),
        ("0.5", "0.02", 1536, false),
        ("0.5", "0.02", 1535, true),
    ] {
        let f = Fixture::new(load, 0);
        f.meminfo(available_mb * 1024, 0);
        f.pressure(psi);
        let blocked = capacity(&f, &[])["blocked"].as_array().cloned().unwrap();
        let mut cmd = f.cmd(&["gate", "check", "--kind", "build"]);
        let status = code(&cmd.output().expect("check"));
        assert_eq!(
            (!blocked.is_empty(), status),
            (denied, if denied { 75 } else { 0 }),
            "load {load} psi {psi} available {available_mb}MB: {blocked:?}"
        );
    }
}

/// The deny log's rows, as JSON.
fn deny_rows(f: &Fixture) -> Vec<serde_json::Value> {
    fs::read_to_string(log_path(f))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// 4.6, 7-8: a `gate check` or `gate run` denial reaches the deny log like a hook's, with the load
/// figures; an allowed one writes nothing, and a denied run does not run.
#[test]
fn gate_check_and_run_denials_are_logged_with_the_load_figures() {
    // A long warm-up window: the holder's reservation stays 2.00 at two decimals for seconds.
    let warm = ("CLAUDECAT_RESERVE_WARMUP_SECS", "6000");
    let f = Fixture::new("0.5", 0);
    let _holder = hold_run(&f, 77, "build", &["cargo", "test"], now_secs());
    // check, build row: 0.5 measured + 2.0 reserved, nothing needed.
    let check = |max_load: &str| {
        f.cmd(&["gate", "check", "--kind", "build"])
            .env(warm.0, warm.1)
            .env("CLAUDECAT_MAX_LOAD", max_load)
            .output()
            .expect("check")
    };
    assert_eq!(code(&check("2.4")), 75);
    let rows = deny_rows(&f);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0],
        serde_json::json!({
            "ts": rows[0]["ts"], "session": null, "id": null, "sub": "check:build",
            "reason": "load", "mem_available_mb": 4000, "load_1m": 0.5,
            "reserved_load": 2.0, "need_load": 0.0,
        }),
        "{}",
        rows[0]
    );
    assert_eq!(code(&check("2.5")), 0);
    assert_eq!(deny_rows(&f).len(), 1, "an allowed check writes nothing");

    // run: 0.5 + 2.0 reserved + 2.0 needed = 4.5 over 4.4. The command must not run.
    let marker = f.temp.path().join("ran");
    fake_tool(&f, "cargo", &format!("touch {}", marker.display()));
    let run = |max_load: &str| {
        let mut cmd = f.cmd(&[
            "gate",
            "run",
            "--kind",
            "build",
            "--",
            "sh",
            "-c",
            "cargo build",
        ]);
        cmd.env(warm.0, warm.1)
            .env("CLAUDECAT_MAX_LOAD", max_load)
            .stdin(Stdio::null());
        with_fake_tools(&f, &mut cmd).output().expect("gate run")
    };
    assert_eq!(code(&run("4.4")), 75);
    assert!(!marker.exists(), "a denied run ran its command");
    let rows = deny_rows(&f);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[1],
        serde_json::json!({
            "ts": rows[1]["ts"], "session": null, "id": null, "sub": "run:build",
            "reason": "load", "mem_available_mb": 4000, "load_1m": 0.5,
            "reserved_load": 2.0, "need_load": 2.0,
        }),
        "{}",
        rows[1]
    );
    assert_eq!(code(&run("4.5")), 0);
    assert!(marker.exists());
    assert_eq!(deny_rows(&f).len(), 2, "an allowed run writes nothing");

    // The hook's rows carry the same two figures.
    let line = deny_line(&hook_at(&f, Some("sess-a"), "cargo test", "4.4"));
    assert!(line.starts_with("gate denied (load) "), "{line}");
    let last = deny_rows(&f).pop().expect("hook row");
    // Under the default window the holder's reservation has decayed a little by now.
    let reserved_load = last["reserved_load"].as_f64().expect("reserved_load");
    assert!((1.9..=2.0).contains(&reserved_load), "{last}");
    assert_eq!(last["need_load"], 2.0);
    assert_eq!(last["sub"], "test");

    // The other row denies on memory pressure: nothing is reserved on this machine, and the other
    // row asks for no load.
    let g = Fixture::new("0.5", 0);
    g.pressure("10.01");
    let mut cmd = g.cmd(&["gate", "check"]);
    cmd.env(warm.0, warm.1);
    assert_eq!(code(&cmd.output().expect("check")), 75);
    let rows = deny_rows(&g);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["reason"], "memory");
    assert_eq!(rows[0]["sub"], "check:other");
    assert_eq!(rows[0]["reserved_load"], 0.0);
    assert_eq!(rows[0]["need_load"], 0.0);
    assert_eq!(rows[0]["psi_some_avg10"], 10.01);

    // Resources that cannot be read refuse the work as well: that is logged too, without figures.
    fs::remove_file(g.proc().join("loadavg")).unwrap();
    let out = g.cmd(&["gate", "check"]).output().expect("check");
    assert_eq!(code(&out), 75);
    let rows = deny_rows(&g);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["reason"], "resources");
    assert!(rows[1].get("load_1m").is_none(), "{}", rows[1]);
}

/// 4.6, 7-8: rows denied for `slots`, `compiler` or `compiler-lock` before the upgrade stay in the
/// log, and `gate log` still sums them, alone and beside the reasons that remain.
#[test]
fn gate_log_still_sums_old_rows_with_removed_reasons() {
    let f = Fixture::new("0.5", 0);
    let now = now_secs();
    let row = |reason: &str, session: &str| serde_json::json!({"ts": now - 10, "session": session, "id": "x", "sub": "test", "reason": reason});
    fs::create_dir_all(log_path(&f).parent().unwrap()).unwrap();
    let rows = [
        row("slots", "a"),
        row("compiler", "b"),
        row("compiler-lock", "c"),
        row("load,compiler-lock", "d"),
        row("memory,slots,compiler", "e"),
    ];
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::write(log_path(&f), text).unwrap();
    let out = f.run(&["gate", "log", "--json"]);
    assert_eq!(code(&out), 0);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value["reasons"],
        serde_json::json!({"slots": 2, "compiler": 2, "compiler-lock": 2, "load": 1, "memory": 1}),
        "{value}"
    );
    let text = String::from_utf8(f.run(&["gate", "log"]).stdout).unwrap();
    for line in ["slots: 2", "compiler: 2", "compiler-lock: 2"] {
        assert!(text.contains(&format!("\n  {line}\n")), "{text}");
    }
}

/// 7-7: the real `status --json` of this build, through `summarize` and `pick`. A machine that
/// reports no slots is judged on its capacity: one that fits is picked under either kind, one that
/// is blocked is not, however much memory it has, and neither is one whose memory the reservations
/// have used up, though it reports nothing blocked.
#[test]
fn fleet_pick_reads_the_status_document_of_this_build() {
    use claudecat::fleet::assemble;
    use claudecat::fleet_store::Row;
    use claudecat::fleet_view::{pick, summarize, Pool, State};
    let now = now_secs();
    let fresh = Fixture::new("0.5", 0);
    fresh.meminfo(12_000 * 1024, 0);
    // Warming cargo runs reserve 4.0: 0.5 + 4.0 is under 6.
    let _a = hold_run(&fresh, 77, "build", &["cargo", "test"], now);
    let _b = hold_run(&fresh, 78, "build", &["cargo", "test"], now);
    // The same two on a machine with 4000 MB: they reserve 3072 of them, too little is left for a
    // Gradle run, and nothing else is the matter with it.
    let booked = Fixture::new("0.5", 0);
    let _d = hold_run(&booked, 77, "build", &["cargo", "test"], now);
    let _e = hold_run(&booked, 78, "build", &["cargo", "test"], now);
    let pressed = Fixture::new("0.5", 0);
    pressed.meminfo(30_000 * 1024, 0);
    pressed.pressure("10.01");
    let loaded = Fixture::new("0.5", 0);
    loaded.meminfo(40_000 * 1024, 0);
    // 0.5 + 2.0 reserved is over its limit of 2.4.
    let _c = hold_run(&loaded, 77, "build", &["cargo", "test"], now);
    let row = |host: &str, f: &Fixture, max_load: &str| {
        let out = f
            .cmd(&["status", "--json"])
            .env("CLAUDECAT_MAX_LOAD", max_load)
            .output()
            .expect("status");
        let status = serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("document");
        assert!(status.get("slots").is_none(), "this build reports no slots");
        let snap = assemble(host, "linux", now, Ok(status), Some(vec![]));
        Row {
            host: host.into(),
            updated_at: snap.collected_at.clone(),
            snapshot: snap,
        }
    };
    let rows = [
        row("small", &fresh, "6"),
        row("booked", &booked, "6"),
        row("pressed", &pressed, "6"),
        row("big", &loaded, "2.4"),
    ];
    let summaries: Vec<_> = rows.iter().map(|r| summarize(r, now)).collect();
    assert!(summaries.iter().all(|s| s.state == State::Ok));
    for pool in [Pool::Gradle, Pool::Other] {
        let picked: Vec<_> = pick(&summaries, pool)
            .into_iter()
            .map(|p| serde_json::to_value(p).unwrap())
            .collect();
        assert_eq!(picked.len(), 1, "{pool:?}: {picked:?}");
        assert_eq!(picked[0]["host"], "small");
        assert_eq!(picked[0]["free_slots"], serde_json::Value::Null);
        assert_eq!(picked[0]["mem_available_mb"], 12_000);
        assert_eq!(picked[0]["mem_headroom_mb"], 12_000 - 3072);
    }
    let summary = |host: &str, key: &str| {
        summaries
            .iter()
            .find(|s| s.host == host)
            .map(|s| serde_json::to_value(s).unwrap()[key].clone())
            .unwrap()
    };
    for host in ["small", "booked"] {
        assert_eq!(summary(host, "blocked"), serde_json::json!([]), "{host}");
    }
    assert_eq!(summary("booked", "mem_available_mb"), 4000);
    assert_eq!(summary("booked", "mem_headroom_mb"), 4000 - 3072);
    assert_eq!(summary("pressed", "blocked"), serde_json::json!(["memory"]));
    assert_eq!(summary("big", "blocked"), serde_json::json!(["load"]));
}

/// `status` as text and as the JSON document, with `env` set for both.
fn status_pair(f: &Fixture, env: &[(&str, &str)]) -> (String, serde_json::Value) {
    let (mut text, mut json) = (f.cmd(&["status"]), f.cmd(&["status", "--json"]));
    for (key, value) in env {
        text.env(key, value);
        json.env(key, value);
    }
    let text = String::from_utf8(text.output().expect("status").stdout).unwrap();
    let out = json.output().expect("status");
    assert!(
        out.stdout.starts_with(b"{"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        text,
        serde_json::from_slice(&out.stdout).expect("json document"),
    )
}
