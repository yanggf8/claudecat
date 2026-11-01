//! Resource measurements around `gate run`, so reserved-memory estimates and PSI/MemAvailable
//! thresholds can be calibrated from data. Everything here is best effort: a sampling or logging
//! failure is dropped silently and never changes the wrapped command's behaviour or exit code.

use crate::gate::{append_log, compact_log, gate_log_path, mem_pressure, metrics, unix_now};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_SAMPLE_MS: u64 = 2000;
/// Longest the gate waits for the sampler thread after the child exits.
const JOIN_CAP: Duration = Duration::from_millis(50);
/// USER_HZ: /proc reports CPU time in these ticks on every Linux configuration in practice.
const CLK_TCK: f64 = 100.0;

/// Minimal view of a JSONL row used by age-based compaction (any log with a `ts` field).
#[derive(Deserialize)]
pub(crate) struct LogTs {
    pub ts: u64,
}

/// System figures attached to a deny row. All optional so old rows still parse.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct DenyContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mem_available_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    psi_some_avg10: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    load_1m: Option<f64>,
}

impl DenyContext {
    pub fn new(mem_available_mb: u64, psi_some_avg10: Option<f64>, load_1m: f64) -> Self {
        Self {
            mem_available_mb: Some(mem_available_mb),
            psi_some_avg10,
            load_1m: Some(load_1m),
        }
    }
}

/// System context at one instant (start or end of a run).
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct SysCtx {
    mem_available_mb: Option<u64>,
    psi_some_avg10: Option<f64>,
    load_1m: Option<f64>,
    swap_used_mb: Option<u64>,
}

fn sys_ctx(root: &Path) -> SysCtx {
    let mut ctx = SysCtx {
        psi_some_avg10: mem_pressure(root),
        ..SysCtx::default()
    };
    if let Ok((load, swap, available)) = metrics(root) {
        ctx.load_1m = Some(load);
        ctx.swap_used_mb = Some(swap);
        ctx.mem_available_mb = Some(available);
    }
    ctx
}

#[derive(Serialize, Deserialize)]
pub struct UsageRow {
    ts: u64,
    session: Option<String>,
    toolchain: String,
    class: String,
    kind: String,
    wall_secs: f64,
    peak_rss_mb: Option<f64>,
    /// Gradle only: peak RSS of the run's own tree plus every other Gradle JVM of this user. The
    /// daemon detaches from the client, so `peak_rss_mb` alone misses most of a Gradle build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gradle_family_rss_mb: Option<f64>,
    cpu_secs: Option<f64>,
    exit: i32,
    start: SysCtx,
    end: SysCtx,
}

// ---------------------------------------------------------------------------------------------
// Task class
// ---------------------------------------------------------------------------------------------

/// Toolchain and task class of a command line (`cargo`|`gradle`|`unknown`, and
/// `test`|`build`|`compile`|`other`). Looks through `sh -c "..."` and wrappers such as `nice`,
/// `env` or `timeout` by locating the first cargo/gradle word of each shell segment; a segment
/// quoting those words as data (`echo cargo test`) is therefore misread, which is acceptable for
/// statistics. When several segments differ, test beats build beats compile beats other.
pub fn task_class(argv: &[String]) -> (&'static str, &'static str) {
    let mut best: Option<(&'static str, &'static str)> = None;
    for line in shell_lines(argv) {
        for segment in split_segments(&line) {
            let Some(found) = segment_class(&segment) else {
                continue;
            };
            if best.is_none_or(|b| rank(found.1) > rank(b.1)) {
                best = Some(found);
            }
        }
    }
    best.unwrap_or(("unknown", "other"))
}

fn rank(class: &str) -> u8 {
    match class {
        "test" => 3,
        "build" => 2,
        "compile" => 1,
        _ => 0,
    }
}

/// The command lines to inspect: argv itself, plus the script of any `sh|bash|zsh -c script`.
fn shell_lines(argv: &[String]) -> Vec<String> {
    let mut lines = vec![argv.join(" ")];
    for (i, word) in argv.iter().enumerate() {
        let name = word.rsplit('/').next().unwrap_or(word);
        if matches!(name, "sh" | "bash" | "zsh" | "dash") {
            if let Some(script) = argv[i + 1..]
                .iter()
                .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('c'))
                .and_then(|p| argv.get(i + 2 + p))
            {
                lines.push(script.clone());
            }
        }
    }
    // argv itself already contains the script text (joined), but quoting is lost; scanning the
    // script alone keeps `-c "a && b"` segments intact, so drop the joined form when we have one.
    if lines.len() > 1 {
        lines.remove(0);
    }
    lines
}

fn split_segments(line: &str) -> Vec<Vec<String>> {
    let mut segments = vec![Vec::new()];
    for raw in line.replace(['\n', ';'], " ; ").split_whitespace() {
        let word = raw.trim_matches(|c| c == '"' || c == '\'');
        if matches!(word, ";" | "&&" | "||" | "|" | "&") {
            segments.push(Vec::new());
        } else if !word.is_empty() {
            if let Some(last) = segments.last_mut() {
                last.push(word.to_owned());
            }
        }
    }
    segments
}

fn segment_class(words: &[String]) -> Option<(&'static str, &'static str)> {
    let at = words.iter().position(|w| {
        matches!(
            w.rsplit('/').next().unwrap_or(w),
            "cargo" | "gradle" | "gradlew"
        )
    })?;
    let tool = words[at].rsplit('/').next().unwrap_or(&words[at]);
    let rest = &words[at + 1..];
    Some(if tool == "cargo" {
        ("cargo", cargo_class(rest))
    } else {
        ("gradle", gradle_class(rest))
    })
}

fn cargo_class(rest: &[String]) -> &'static str {
    let mut i = 0;
    while let Some(arg) = rest.get(i) {
        if arg.starts_with('+') {
            i += 1;
        } else if arg.starts_with('-') {
            let takes_value = matches!(
                arg.as_str(),
                "-C" | "--config" | "--manifest-path" | "--target-dir" | "-Z" | "--color"
            );
            i += 1 + usize::from(takes_value);
        } else {
            return match arg.as_str() {
                "test" | "nextest" | "bench" => "test",
                "build" | "check" | "clippy" | "doc" | "run" => "build",
                _ => "other",
            };
        }
    }
    "other"
}

fn gradle_class(rest: &[String]) -> &'static str {
    let mut best = "other";
    let mut i = 0;
    while let Some(arg) = rest.get(i) {
        i += 1;
        if arg.starts_with('-') {
            if arg == "--tests" || arg.starts_with("--tests=") {
                return "test";
            }
            // `-Dx=y`, `-Pfoo=bar` and `--opt=value` carry their value; a bare flag below takes the next word.
            if matches!(
                arg.as_str(),
                "-p" | "--project-dir"
                    | "-g"
                    | "--gradle-user-home"
                    | "-c"
                    | "--settings-file"
                    | "-b"
                    | "--build-file"
                    | "-I"
                    | "--init-script"
                    | "--include-build"
                    | "--project-cache-dir"
                    | "-x"
                    | "--exclude-task"
                    | "--max-workers"
                    | "--console"
                    | "--warning-mode"
                    | "--priority"
                    | "-D"
                    | "-P"
            ) {
                i += 1;
            }
            continue;
        }
        let task = arg.rsplit(':').next().unwrap_or(arg);
        let class = gradle_task_class(task);
        if rank(class) > rank(best) {
            best = class;
        }
    }
    best
}

fn gradle_task_class(task: &str) -> &'static str {
    let is_test = matches!(task, "test" | "check" | "connectedCheck")
        || task.ends_with("Test")
        || task.ends_with("Tests")
        || (task.starts_with("test")
            && task[4..].chars().next().is_some_and(char::is_uppercase)
            && !task.ends_with("Classes"));
    if is_test {
        "test"
    } else if matches!(
        task,
        "build" | "bootJar" | "bootWar" | "shadowJar" | "installDist" | "war" | "buildNeeded"
    ) || task.starts_with("publish")
        || task.starts_with("bundle")
    {
        "build"
    } else if task.starts_with("compile")
        || task.starts_with("assemble")
        || matches!(task, "classes" | "testClasses" | "jar")
    {
        "compile"
    } else {
        "other"
    }
}

// ---------------------------------------------------------------------------------------------
// Process-tree sampling
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Acc {
    peak_rss_kb: Option<u64>,
    peak_family_kb: Option<u64>,
    /// Highest utime+stime ticks seen per (pid, start time), summed for the CPU fallback.
    cpu_ticks: HashMap<(u32, u64), u64>,
}

/// (pid, ppid, start ticks, cpu ticks) from a stat line; the comm field may contain spaces and
/// parentheses, so split after the last `)`.
fn parse_stat(text: &str) -> Option<(u32, u32, u64, u64)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let pid = text.get(..open)?.trim().parse().ok()?;
    let f: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // f[0]=state f[1]=ppid ... f[11]=utime f[12]=stime ... f[19]=starttime
    let ppid = f.get(1)?.parse().ok()?;
    let cpu = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let start = f.get(19)?.parse().ok()?;
    Some((pid, ppid, start, cpu))
}

fn vm_rss_kb(root: &Path, pid: u32) -> Option<u64> {
    fs::read_to_string(root.join(pid.to_string()).join("status"))
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

struct TreeSample {
    rss_kb: u64,
    cpu: Vec<((u32, u64), u64)>,
    /// Tree RSS plus every other Gradle JVM of `family_uid`; 0 when no uid was given.
    family_kb: u64,
}

/// A Gradle daemon, worker or Kotlin daemon: a `java` process naming one of their main classes.
fn is_gradle_jvm(root: &Path, pid: u32) -> bool {
    let Ok(bytes) = fs::read(root.join(pid.to_string()).join("cmdline")) else {
        return false;
    };
    let mut args = bytes.split(|b| *b == 0);
    let java = args
        .next()
        .is_some_and(|a| a.rsplit(|b| *b == b'/').next() == Some(b"java"));
    java && args.any(|a| {
        [
            &b"GradleDaemon"[..],
            b"GradleWorkerMain",
            b"KotlinCompileDaemon",
        ]
        .iter()
        .any(|m| a.windows(m.len()).any(|w| w == *m))
    })
}

/// One pass over `root`: total RSS (kB) of `leader` and its descendants, and their CPU ticks.
/// None if the leader itself is not visible.
fn sample_tree(root: &Path, leader: u32, family_uid: Option<u32>) -> Option<TreeSample> {
    let mut stats = HashMap::new();
    let mut gradle_jvms = Vec::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for entry in fs::read_dir(root).ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(text) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if family_uid.is_some_and(|uid| {
            entry.metadata().is_ok_and(|m| m.uid() == uid) && is_gradle_jvm(root, pid)
        }) {
            gradle_jvms.push(pid);
        }
        if let Some((_, ppid, start, cpu)) = parse_stat(&text) {
            children.entry(ppid).or_default().push(pid);
            stats.insert(pid, (start, cpu));
        }
    }
    stats.get(&leader)?;
    let mut seen = HashSet::from([leader]);
    let mut stack = vec![leader];
    let (mut rss, mut cpu) = (0, Vec::new());
    while let Some(pid) = stack.pop() {
        rss += vm_rss_kb(root, pid).unwrap_or(0);
        if let Some(&(start, ticks)) = stats.get(&pid) {
            cpu.push(((pid, start), ticks));
        }
        for &child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child) {
                stack.push(child);
            }
        }
    }
    let family_kb = if family_uid.is_some() {
        rss + gradle_jvms
            .iter()
            .filter(|pid| !seen.contains(pid))
            .map(|pid| vm_rss_kb(root, *pid).unwrap_or(0))
            .sum::<u64>()
    } else {
        0
    };
    Some(TreeSample {
        rss_kb: rss,
        cpu,
        family_kb,
    })
}

fn sampler(
    root: PathBuf,
    leader: u32,
    interval: Duration,
    acc: Arc<Mutex<Acc>>,
    family_uid: Option<u32>,
) -> Stop {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    // A failed spawn drops the closure and with it `done_tx`, so `finish` returns at once and the
    // run simply goes unmeasured instead of panicking beside a live child.
    let _ = thread::Builder::new().spawn(move || {
        let mut delay = Duration::from_millis(50).min(interval);
        loop {
            if let Some(TreeSample {
                rss_kb: rss,
                cpu,
                family_kb,
            }) = sample_tree(&root, leader, family_uid)
            {
                if let Ok(mut acc) = acc.lock() {
                    acc.peak_rss_kb = Some(acc.peak_rss_kb.map_or(rss, |p| p.max(rss)));
                    if family_uid.is_some() {
                        acc.peak_family_kb =
                            Some(acc.peak_family_kb.map_or(family_kb, |p| p.max(family_kb)));
                    }
                    for (key, ticks) in cpu {
                        let slot = acc.cpu_ticks.entry(key).or_default();
                        *slot = (*slot).max(ticks);
                    }
                }
            }
            // Early samples come quickly so short jobs still get a reading, then back off.
            if stop_rx.recv_timeout(delay) != Err(mpsc::RecvTimeoutError::Timeout) {
                break;
            }
            delay = (delay * 2).min(interval);
        }
        let _ = done_tx.send(());
    });
    Stop { stop_tx, done_rx }
}

struct Stop {
    stop_tx: mpsc::Sender<()>,
    done_rx: mpsc::Receiver<()>,
}

impl Stop {
    /// Ask the sampler to stop and wait at most JOIN_CAP; a stuck /proc read is abandoned.
    fn finish(self) {
        let _ = self.stop_tx.send(());
        let _ = self.done_rx.recv_timeout(JOIN_CAP);
    }
}

/// CPU seconds of every child this process has waited for (and their waited descendants).
fn reaped_cpu_secs() -> Option<f64> {
    let text = fs::read_to_string("/proc/self/stat").ok()?;
    let rest = text.get(text.rfind(')')? + 1..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[13]=cutime f[14]=cstime
    let ticks = f.get(13)?.parse::<u64>().ok()? + f.get(14)?.parse::<u64>().ok()?;
    Some(ticks as f64 / CLK_TCK)
}

fn round(value: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (value * m).round() / m
}

// ---------------------------------------------------------------------------------------------
// gate run integration
// ---------------------------------------------------------------------------------------------

fn usage_log_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("CLAUDECAT_GATE_USAGE_LOG") {
        return Ok(PathBuf::from(path));
    }
    Ok(gate_log_path()?.with_file_name("gate-usage.jsonl"))
}

fn sample_interval() -> Duration {
    let ms = env::var("CLAUDECAT_USAGE_SAMPLE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SAMPLE_MS);
    Duration::from_millis(ms)
}

fn spawn(command: &[String]) -> io::Result<Child> {
    // Mirrors gate::execute: `--` keeps nice from parsing a child name that starts with `-`.
    Command::new("nice")
        .args(["-n", "15", "--"])
        .args(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

/// Run `command` under nice like `gate::execute`, sample its process tree, and append one row to
/// the usage log when it exits. Returns the child's exit code.
pub fn run_measured(kind: &str, command: &[String], proc_root: &Path) -> i32 {
    // CLAUDECAT_USAGE_PID_ROOT lets tests sample the real /proc while the gate's own figures
    // come from a fixture CLAUDECAT_PROC_ROOT; by default both are the same tree.
    let pid_root = env::var_os("CLAUDECAT_USAGE_PID_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| proc_root.to_path_buf());
    let start_ctx = sys_ctx(proc_root);
    let began = Instant::now();
    let mut child = match spawn(command) {
        Ok(child) => child,
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(io::stderr(), "gate run: {e}");
            return 1;
        }
    };
    let (toolchain, class) = task_class(command);
    let family_uid = (toolchain == "gradle")
        .then(|| fs::metadata("/proc/self").map(|m| m.uid()).ok())
        .flatten();
    let acc = Arc::new(Mutex::new(Acc::default()));
    let stop = sampler(
        pid_root,
        child.id(),
        sample_interval(),
        Arc::clone(&acc),
        family_uid,
    );
    let status = child.wait();
    let wall = began.elapsed();
    stop.finish();
    let exit = match &status {
        Ok(s) => s
            .code()
            .or_else(|| s.signal().map(|sig| 128 + sig))
            .unwrap_or(1),
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(io::stderr(), "gate run: {e}");
            1
        }
    };
    if status.is_ok() {
        let (peak_kb, family_kb, sampled_ticks) = acc
            .try_lock()
            .map(|a| {
                (
                    a.peak_rss_kb,
                    a.peak_family_kb,
                    a.cpu_ticks.values().sum::<u64>(),
                )
            })
            .unwrap_or((None, None, 0));
        let cpu_secs =
            reaped_cpu_secs().or((sampled_ticks > 0).then_some(sampled_ticks as f64 / CLK_TCK));
        let row = UsageRow {
            ts: unix_now(),
            session: None,
            toolchain: toolchain.into(),
            class: class.into(),
            kind: kind.into(),
            wall_secs: round(wall.as_secs_f64(), 2),
            peak_rss_mb: peak_kb.map(|kb| round(kb as f64 / 1024.0, 1)),
            gradle_family_rss_mb: family_kb.map(|kb| round(kb as f64 / 1024.0, 1)),
            cpu_secs: cpu_secs.map(|c| round(c, 2)),
            exit,
            start: start_ctx,
            end: sys_ctx(proc_root),
        };
        let _ = record_usage(&row);
    }
    exit
}

fn record_usage(row: &UsageRow) -> io::Result<()> {
    let path = usage_log_path()?;
    // Compaction replaces the file by rename, so two runs finishing together could drop each
    // other's row. A sidecar lock serialises them; if it cannot be taken, log without it.
    let _guard = path.parent().and_then(|dir| {
        fs::create_dir_all(dir).ok()?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("jsonl.lock"))
            .ok()?;
        lock.lock().ok()?;
        Some(lock)
    });
    compact_log(&path, row.ts)?;
    append_log(&path, row)
}

// ---------------------------------------------------------------------------------------------
// gate usage report
// ---------------------------------------------------------------------------------------------

/// Nearest-rank percentile of an ascending slice, matching `gate log`.
pub fn percentile(sorted: &[f64], p: usize) -> Option<f64> {
    sorted
        .get((sorted.len() * p).div_ceil(100).saturating_sub(1))
        .copied()
}

fn sorted(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values
}

fn read_rows(path: &Path) -> io::Result<Vec<UsageRow>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<UsageRow>(line).ok())
        .collect())
}

/// Per (toolchain, class) summary of rows no older than `days`.
pub fn summarize(rows: &[UsageRow], now: u64, days: u32) -> serde_json::Value {
    let floor = now.saturating_sub(u64::from(days) * 86400);
    let mut groups: BTreeMap<(&str, &str), Vec<&UsageRow>> = BTreeMap::new();
    for row in rows.iter().filter(|r| r.ts >= floor && r.ts <= now) {
        groups
            .entry((&row.toolchain, &row.class))
            .or_default()
            .push(row);
    }
    let list: Vec<_> = groups
        .into_iter()
        .map(|((toolchain, class), rows)| {
            let rss = sorted(rows.iter().filter_map(|r| r.peak_rss_mb).collect());
            let wall = sorted(rows.iter().map(|r| r.wall_secs).collect());
            let family = sorted(rows.iter().filter_map(|r| r.gradle_family_rss_mb).collect());
            let cpu: Vec<f64> = rows.iter().filter_map(|r| r.cpu_secs).collect();
            let cpu_mean = (!cpu.is_empty()).then(|| round(cpu.iter().sum::<f64>() / cpu.len() as f64, 2));
            serde_json::json!({
                "toolchain": toolchain,
                "class": class,
                "count": rows.len(),
                "peak_rss_mb": {"p50": percentile(&rss, 50), "p90": percentile(&rss, 90), "max": rss.last()},
                "gradle_family_rss_mb": {"p50": percentile(&family, 50), "p90": percentile(&family, 90), "max": family.last()},
                "wall_secs": {"p50": percentile(&wall, 50), "p90": percentile(&wall, 90)},
                "cpu_secs_mean": cpu_mean,
            })
        })
        .collect();
    serde_json::json!({"days": days, "groups": list})
}

fn cell(value: &serde_json::Value) -> String {
    value.as_f64().map_or("-".into(), |n| format!("{n:.1}"))
}

pub fn usage(json: bool, days: u32) -> i32 {
    let result = (|| -> io::Result<serde_json::Value> {
        let path = usage_log_path()?;
        let now = unix_now();
        compact_log(&path, now)?;
        Ok(summarize(&read_rows(&path)?, now, days))
    })();
    match result {
        Ok(value) if json => {
            println!("{value}");
            0
        }
        Ok(value) => {
            println!("toolchain class     count  rss_p50  rss_p90  rss_max  wall_p50  wall_p90  cpu_mean");
            for g in value["groups"].as_array().into_iter().flatten() {
                println!(
                    "{:<9} {:<9} {:>5} {:>8} {:>8} {:>8} {:>9} {:>9} {:>9}",
                    g["toolchain"].as_str().unwrap_or("?"),
                    g["class"].as_str().unwrap_or("?"),
                    g["count"],
                    cell(&g["peak_rss_mb"]["p50"]),
                    cell(&g["peak_rss_mb"]["p90"]),
                    cell(&g["peak_rss_mb"]["max"]),
                    cell(&g["wall_secs"]["p50"]),
                    cell(&g["wall_secs"]["p90"]),
                    cell(&g["cpu_secs_mean"]),
                );
            }
            0
        }
        Err(e) => {
            eprintln!("gate usage: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn classifies_gradle() {
        let cases = [
            ("./gradlew test", "test"),
            ("gradle :app:integrationTest", "test"),
            ("gradle reproTest", "test"),
            ("gradle check", "test"),
            ("gradle testDebugUnitTest", "test"),
            ("gradle compileJava", "compile"),
            ("gradle :core:compileKotlin", "compile"),
            ("gradle classes", "compile"),
            ("gradle assemble", "compile"),
            ("gradle jar", "compile"),
            ("gradle testClasses", "compile"),
            ("gradle build", "build"),
            ("gradle bootJar", "build"),
            ("gradle clean build", "build"),
            ("gradle compileJava test", "test"),
            ("gradle compileJava build", "build"),
            ("gradle build -x test", "build"),
            ("gradle build --tests Foo", "test"),
            ("gradle compileJava --tests=Foo", "test"),
            ("gradle -p sub/test --console plain compileJava", "compile"),
            (
                "gradle -Dorg.gradle.test=1 -Pcheck=1 compileJava",
                "compile",
            ),
            ("gradle -p test", "other"),
            ("gradle --version", "other"),
            ("gradle clean", "other"),
            ("/opt/x/gradlew --no-daemon :a:test", "test"),
        ];
        for (cmd, class) in cases {
            assert_eq!(task_class(&v(cmd)), ("gradle", class), "{cmd}");
        }
    }

    #[test]
    fn classifies_cargo_and_wrappers() {
        let cases = [
            ("cargo test", "test"),
            ("cargo nextest run", "test"),
            ("cargo +nightly bench", "test"),
            ("cargo -C dir build --release", "build"),
            ("cargo clippy --all-targets", "build"),
            ("cargo fmt", "other"),
            ("nice -n 5 cargo check", "build"),
            ("env A=b cargo doc", "build"),
        ];
        for (cmd, class) in cases {
            assert_eq!(task_class(&v(cmd)), ("cargo", class), "{cmd}");
        }
        assert_eq!(task_class(&v("sleep 1")), ("unknown", "other"));
        assert_eq!(task_class(&[]), ("unknown", "other"));
    }

    #[test]
    fn looks_through_sh_c() {
        let argv = vec!["sh".into(), "-c".into(), "cd x && cargo test -p a".into()];
        assert_eq!(task_class(&argv), ("cargo", "test"));
        let argv = vec![
            "bash".into(),
            "-lc".into(),
            "echo hi; ./gradlew compileJava | tee log".into(),
        ];
        assert_eq!(task_class(&argv), ("gradle", "compile"));
        let argv = vec!["sh".into(), "-c".into(), "sleep 1".into()];
        assert_eq!(task_class(&argv), ("unknown", "other"));
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let s = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&s, 50), Some(5.0));
        assert_eq!(percentile(&s, 90), Some(9.0));
        assert_eq!(percentile(&s, 100), Some(10.0));
        assert_eq!(percentile(&[7.0], 90), Some(7.0));
        assert_eq!(percentile(&[], 50), None);
    }

    #[test]
    fn parse_stat_handles_odd_comm() {
        let line = "42 (a) b) S 7 0 0 0 0 0 0 0 0 0 11 22 0 0 0 0 0 0 999 0 0\n";
        assert_eq!(parse_stat(line), Some((42, 7, 999, 33)));
    }

    #[test]
    fn sampler_tolerates_missing_proc() {
        assert!(sample_tree(Path::new("/nonexistent/proc/root"), 1, None).is_none());
        let dir = tempfile::tempdir().unwrap();
        assert!(sample_tree(dir.path(), 1, None).is_none());
        let acc = Arc::new(Mutex::new(Acc::default()));
        let started = Instant::now();
        sampler(
            dir.path().to_path_buf(),
            1,
            Duration::from_secs(60),
            acc,
            None,
        )
        .finish();
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn deny_context_is_backward_compatible() {
        let ctx: DenyContext = serde_json::from_str("{}").unwrap();
        assert!(ctx.mem_available_mb.is_none());
        assert_eq!(
            serde_json::to_string(&DenyContext::default()).unwrap(),
            "{}"
        );
    }
    fn fake_proc(root: &Path, pid: u32, ppid: u32, rss_kb: u64, cmdline: &[&str]) {
        let dir = root.join(pid.to_string());
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("stat"),
            format!("{pid} (x) S {ppid} 0 0 0 0 0 0 0 0 0 1 1 0 0 0 0 1 0 5 0 0\n"),
        )
        .unwrap();
        fs::write(dir.join("status"), format!("VmRSS:\t{rss_kb} kB\n")).unwrap();
        fs::write(dir.join("cmdline"), cmdline.join("\0") + "\0").unwrap();
    }

    #[test]
    fn gradle_family_counts_detached_gradle_jvms_only_for_gradle() {
        let dir = tempfile::tempdir().unwrap();
        // The client (leader) is small; the daemon detached from it and a worker hangs off the
        // daemon, so neither is in the leader's tree. An unrelated java and an idle shell are not Gradle.
        fake_proc(dir.path(), 10, 1, 100, &["java", "GradleWrapperMain"]);
        fake_proc(
            dir.path(),
            20,
            1,
            2000,
            &[
                "/usr/bin/java",
                "org.gradle.launcher.daemon.bootstrap.GradleDaemon",
            ],
        );
        fake_proc(
            dir.path(),
            21,
            20,
            1000,
            &[
                "java",
                "worker.org.gradle.process.internal.worker.GradleWorkerMain",
            ],
        );
        fake_proc(dir.path(), 30, 1, 500, &["java", "-jar", "app.jar"]);
        fake_proc(dir.path(), 31, 1, 400, &["rg", "GradleDaemon"]);
        let uid = fs::metadata(dir.path()).unwrap().uid();
        let with = sample_tree(dir.path(), 10, Some(uid)).unwrap();
        assert_eq!(with.rss_kb, 100);
        assert_eq!(with.family_kb, 100 + 2000 + 1000);
        let without = sample_tree(dir.path(), 10, None).unwrap();
        assert_eq!(without.family_kb, 0);
    }
}
