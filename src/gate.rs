use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LOAD: f64 = 6.0;
const DEFAULT_SWAP_MB: u64 = 1024;
const DEFAULT_SLOTS: usize = 3;

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Kind {
    Build,
    Test,
    E2e,
    Other,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Test => "test",
            Self::E2e => "e2e",
            Self::Other => "other",
        }
    }

    fn checks_compilers(self) -> bool {
        matches!(self, Self::Build | Self::Test)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Holder {
    pub pid: u32,
    pub kind: String,
    pub command: Vec<String>,
    pub started_at: u64,
}

#[derive(Serialize)]
pub struct Slot {
    pub index: usize,
    pub holder: Option<Holder>,
    pub occupied: bool,
}

#[derive(Serialize)]
pub struct CompilerLock {
    pub holder: Option<Holder>,
    pub occupied: bool,
}

#[derive(Serialize, Clone)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cwd: String,
}

#[derive(Serialize)]
pub struct Broker {
    pub pid: u32,
    pub cwd_arg: Option<String>,
    pub cwd_exists: bool,
    pub children: Vec<Process>,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub load_1m: f64,
    pub swap_used_mb: u64,
    pub mem_available_mb: u64,
    pub slots: Vec<Slot>,
    pub compiler_lock: CompilerLock,
    pub compiling: Vec<Process>,
    pub orphan_brokers: Vec<Broker>,
}

struct Config {
    proc_root: PathBuf,
    gate_dir: PathBuf,
    max_load: f64,
    max_swap_mb: u64,
    slots: usize,
}

fn positive_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(default)
}

fn positive_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

fn config() -> io::Result<Config> {
    let slots = env::var("CLAUDECAT_SLOTS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SLOTS);
    let gate_dir = if let Some(dir) = env::var_os("CLAUDECAT_GATE_DIR") {
        PathBuf::from(dir)
    } else if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("claudecat/gate")
    } else {
        let output = Command::new("id").arg("-u").output()?;
        if !output.status.success() {
            return Err(io::Error::other("id -u failed"));
        }
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if uid.parse::<u32>().is_err() {
            return Err(io::Error::other("invalid uid"));
        }
        PathBuf::from(format!("/tmp/claudecat-{uid}/gate"))
    };
    Ok(Config {
        proc_root: env::var_os("CLAUDECAT_PROC_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/proc")),
        gate_dir,
        max_load: positive_f64("CLAUDECAT_MAX_LOAD", DEFAULT_LOAD),
        max_swap_mb: positive_u64("CLAUDECAT_MAX_SWAP_MB", DEFAULT_SWAP_MB),
        slots,
    })
}

fn metrics(root: &Path) -> io::Result<(f64, u64, u64)> {
    let load = fs::read_to_string(root.join("loadavg"))?
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::other("missing load average"))?
        .parse::<f64>()
        .map_err(|_| io::Error::other("invalid load average"))?;
    let mem = fs::read_to_string(root.join("meminfo"))?;
    let field = |name: &str| -> io::Result<u64> {
        mem.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other(format!("missing {name}")))
    };
    let swap = field("SwapTotal:")?.saturating_sub(field("SwapFree:")?) / 1024;
    let available = field("MemAvailable:")? / 1024;
    Ok((load, swap, available))
}

fn lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Try the lock without waiting; release it immediately when we got it. A would-block lock is
/// held by someone else, and the holder JSON they wrote is readable while it is locked.
fn probe_lock(path: &Path) -> io::Result<(bool, Option<Holder>)> {
    let mut file = lock_file(path)?;
    match file.try_lock() {
        Ok(()) => {
            file.unlock()?;
            Ok((false, None))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut data = String::new();
            file.read_to_string(&mut data)?;
            Ok((true, serde_json::from_str(&data).ok()))
        }
        Err(e) => Err(io::Error::other(e)),
    }
}

fn slots(cfg: &Config) -> io::Result<Vec<Slot>> {
    fs::create_dir_all(&cfg.gate_dir)?;
    (0..cfg.slots)
        .map(|index| {
            let (occupied, holder) = probe_lock(&cfg.gate_dir.join(format!("slot-{index}.lock")))?;
            Ok(Slot {
                index,
                holder,
                occupied,
            })
        })
        .collect()
}

fn compiler_lock(cfg: &Config) -> io::Result<CompilerLock> {
    let (occupied, holder) = probe_lock(&cfg.gate_dir.join("compiler.lock"))?;
    Ok(CompilerLock { holder, occupied })
}

struct ProcEntry {
    uid: u32,
    pid: u32,
    name: String,
    cmdline: Vec<String>,
    cwd: String,
    ppid: Option<u32>,
}

fn processes(root: &Path) -> io::Result<Vec<ProcEntry>> {
    let mut found = Vec::new();
    for item in fs::read_dir(root)? {
        let item = item?;
        let Some(pid) = item
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = item.path();
        let Ok(uid) = fs::metadata(&dir).map(|m| m.uid()) else {
            continue;
        };
        let Ok(name) = fs::read_to_string(dir.join("comm")) else {
            continue;
        };
        let cmd = fs::read(dir.join("cmdline")).unwrap_or_default();
        let cmdline = cmd
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let cwd = fs::read_link(dir.join("cwd"))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "?".into());
        let ppid = fs::read_to_string(dir.join("stat")).ok().and_then(|s| {
            s.rsplit_once(") ")
                .and_then(|(_, tail)| tail.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
        });
        found.push(ProcEntry {
            uid,
            pid,
            name: name.trim().into(),
            cmdline,
            cwd,
            ppid,
        });
    }
    found.sort_by_key(|p| p.pid);
    Ok(found)
}

/// Gradle only counts while a test or compile worker JVM is running: its main class is an argument
/// ending in `.GradleWorkerMain`. The daemon is deliberately not counted -- it stays resident for
/// hours after a build (three by default) and is idle most of that time, so counting it refused
/// every build/test until it timed out. A gated Gradle build is covered by `compiler.lock` instead.
/// The JVM requirement and the dotted suffix keep `rg …GradleWorkerMain` and `tail -f` on a log --
/// mentions, not compilers -- from blocking the gate.
fn is_gradle(p: &ProcEntry) -> bool {
    p.name == "java" && p.cmdline.iter().any(|a| a.ends_with(".GradleWorkerMain"))
}

fn compiling(processes: &[ProcEntry], uid: u32) -> Vec<Process> {
    processes
        .iter()
        .filter(|p| p.uid == uid && (p.name == "rustc" || is_gradle(p)))
        .map(|p| Process {
            pid: p.pid,
            name: p.name.clone(),
            cwd: p.cwd.clone(),
        })
        .collect()
}

/// A node process runs the script when its argv[0] is node and a later argument is that file.
/// Not `comm`: node 24 renames its main thread, so every node process on the machine reads
/// `MainThread` there. Comparing file names, not substrings, keeps `vim app-server-broker.mjs`
/// from counting as a broker.
fn runs_script(p: &ProcEntry, script: &str) -> bool {
    let file_name = |a: &String| {
        Path::new(a)
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
    };
    let mut args = p.cmdline.iter();
    args.next()
        .and_then(file_name)
        .is_some_and(|argv0| argv0 == "node" || argv0 == "nodejs")
        && args.any(|a| file_name(a).as_deref() == Some(script))
}

/// A broker is orphaned once the codex-companion that started it is gone. Its new parent is not
/// always pid 1: on WSL a detached process is reparented to the session's `/init`, which has its
/// own pid, so the test is "the parent is not a companion", never "the parent is pid 1".
fn is_companion(processes: &[ProcEntry], pid: Option<u32>) -> bool {
    pid.and_then(|pid| processes.iter().find(|p| p.pid == pid))
        .is_some_and(|p| runs_script(p, "codex-companion.mjs"))
}

fn brokers(processes: &[ProcEntry]) -> Vec<Broker> {
    processes
        .iter()
        .filter(|p| runs_script(p, "app-server-broker.mjs") && !is_companion(processes, p.ppid))
        .map(|p| {
            let cwd_arg = p
                .cmdline
                .windows(2)
                .find(|w| w[0] == "--cwd")
                .map(|w| w[1].clone())
                .or_else(|| {
                    p.cmdline
                        .iter()
                        .find_map(|a| a.strip_prefix("--cwd=").map(str::to_owned))
                });
            let cwd_exists = cwd_arg.as_ref().is_some_and(|s| Path::new(s).exists());
            let children = processes
                .iter()
                .filter(|child| child.ppid == Some(p.pid))
                .map(|child| Process {
                    pid: child.pid,
                    name: child.name.clone(),
                    cwd: child.cwd.clone(),
                })
                .collect();
            Broker {
                pid: p.pid,
                cwd_arg,
                cwd_exists,
                children,
            }
        })
        .collect()
}

fn snapshot(cfg: &Config) -> io::Result<Snapshot> {
    let (load_1m, swap_used_mb, mem_available_mb) = metrics(&cfg.proc_root)?;
    let procs = processes(&cfg.proc_root)?;
    fs::create_dir_all(&cfg.gate_dir)?;
    // Our own uid, not the gate directory's owner: the directory can be shared with a group and
    // must not decide whose compilers count. CLAUDECAT_PROC_ROOT is the fixtures' fake proc, so
    // read the real /proc/self; fall back to the directory owner only if it is unreadable.
    let uid = fs::metadata("/proc/self")
        .map(|m| m.uid())
        .or_else(|_| fs::metadata(&cfg.gate_dir).map(|m| m.uid()))?;
    Ok(Snapshot {
        load_1m,
        swap_used_mb,
        mem_available_mb,
        slots: slots(cfg)?,
        compiler_lock: compiler_lock(cfg)?,
        compiling: compiling(&procs, uid),
        orphan_brokers: brokers(&procs),
    })
}

fn deny_reason(s: &Snapshot, cfg: &Config, kind: Kind) -> Option<String> {
    let reasons = [
        (s.load_1m > cfg.max_load, "load"),
        (s.swap_used_mb >= cfg.max_swap_mb, "swap"),
        (s.slots.iter().all(|slot| slot.occupied), "slots"),
        (
            kind.checks_compilers() && !s.compiling.is_empty(),
            "compiler",
        ),
        (
            kind.checks_compilers() && s.compiler_lock.occupied,
            "compiler-lock",
        ),
    ];
    let failed: Vec<_> = reasons
        .iter()
        .filter(|(yes, _)| *yes)
        .map(|(_, why)| *why)
        .collect();
    if failed.is_empty() {
        return None;
    }
    let holders: Vec<_> = s
        .slots
        .iter()
        .filter(|slot| slot.occupied)
        .map(|slot| match &slot.holder {
            Some(h) => format!(
                "slot-{} pid={} kind={} cmd={} since={}",
                slot.index,
                h.pid,
                h.kind,
                h.command.join(" "),
                h.started_at
            ),
            None => format!("slot-{} unknown", slot.index),
        })
        .collect();
    let compilers: Vec<_> = s
        .compiling
        .iter()
        .map(|p| format!("pid={} name={} cwd={}", p.pid, p.name, p.cwd))
        .collect();
    let compiler_lock = match &s.compiler_lock.holder {
        Some(h) => format!(
            "pid={} kind={} cmd={} since={}",
            h.pid,
            h.kind,
            h.command.join(" "),
            h.started_at
        ),
        None if s.compiler_lock.occupied => "unknown".into(),
        None => "none".into(),
    };
    Some(format!(
        "gate denied ({}) load={:.2}/{:.2} swap={}MB/{}MB slots={}/{} holders=[{}] compiler_lock=[{}] compiling=[{}]",
        failed.join(","),
        s.load_1m,
        cfg.max_load,
        s.swap_used_mb,
        cfg.max_swap_mb,
        holders.len(),
        cfg.slots,
        holders.join("; "),
        compiler_lock,
        compilers.join("; ")
    ))
}

fn warning(error: &io::Error) {
    let _ = writeln!(io::stderr(), "gate warning: {error}; allowing work");
}

fn write_holder(file: &mut File, holder: &Holder) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    serde_json::to_writer(&mut *file, holder)?;
    file.write_all(b"\n")?;
    file.flush()
}

pub fn check(kind: Kind) -> i32 {
    match config().and_then(|cfg| snapshot(&cfg).map(|s| (cfg, s))) {
        Ok((cfg, s)) => match deny_reason(&s, &cfg, kind) {
            Some(reason) => {
                let _ = writeln!(io::stderr(), "{reason}");
                75
            }
            None => 0,
        },
        Err(e) => {
            warning(&e);
            0
        }
    }
}

pub fn run(kind: Kind, command: &[String]) -> i32 {
    let result = config().and_then(|cfg| snapshot(&cfg).map(|s| (cfg, s)));
    let (cfg, snap) = match result {
        Ok(pair) => pair,
        Err(e) => {
            warning(&e);
            return execute(command);
        }
    };
    if let Some(reason) = deny_reason(&snap, &cfg, kind) {
        let _ = writeln!(io::stderr(), "{reason}");
        return 75;
    }
    for index in 0..cfg.slots {
        let path = cfg.gate_dir.join(format!("slot-{index}.lock"));
        let mut file = match lock_file(&path) {
            Ok(f) => f,
            Err(e) => {
                warning(&e);
                return execute(command);
            }
        };
        match file.try_lock() {
            Ok(()) => {
                let holder = Holder {
                    pid: std::process::id(),
                    kind: kind.as_str().into(),
                    command: command.to_vec(),
                    started_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                };
                if let Err(e) = write_holder(&mut file, &holder) {
                    warning(&e);
                    return execute(command);
                }
                // The slot alone cannot see the startup window before cargo spawns its first
                // rustc, so build/test also holds a second lock for the whole child lifetime.
                let mut compiler = None;
                if kind.checks_compilers() {
                    let mut lock = match lock_file(&cfg.gate_dir.join("compiler.lock")) {
                        Ok(f) => f,
                        Err(e) => {
                            warning(&e);
                            return execute(command);
                        }
                    };
                    match lock.try_lock() {
                        Ok(()) => {
                            if let Err(e) = write_holder(&mut lock, &holder) {
                                warning(&e);
                            }
                            compiler = Some(lock);
                        }
                        Err(std::fs::TryLockError::WouldBlock) => {
                            // Drop the slot before answering 75: the caller queued, so it must
                            // not keep a place it will not use. Only a race reaches this arm --
                            // someone took compiler.lock after the snapshot's pre-check -- so no
                            // test drives it; keep the drop when touching this.
                            drop(file);
                            let mut data = String::new();
                            let _ = lock.read_to_string(&mut data);
                            let _ = writeln!(
                                io::stderr(),
                                "gate denied (compiler-lock) holder=[{}] load={:.2} swap={}MB",
                                data.trim(),
                                snap.load_1m,
                                snap.swap_used_mb
                            );
                            return 75;
                        }
                        Err(e) => {
                            warning(&io::Error::other(e));
                            return execute(command);
                        }
                    }
                }
                let exit = execute(command);
                drop(compiler);
                drop(file);
                return exit;
            }
            Err(std::fs::TryLockError::WouldBlock) => continue,
            Err(e) => {
                warning(&io::Error::other(e));
                return execute(command);
            }
        }
    }
    let _ = writeln!(
        io::stderr(),
        "gate denied (slots) load={:.2} swap={}MB slots={}/{} holders=[{}] compiling=[{}]",
        snap.load_1m,
        snap.swap_used_mb,
        cfg.slots,
        cfg.slots,
        snap.slots
            .iter()
            .filter_map(|s| s.holder.as_ref().map(|h| format!(
                "pid={} kind={} cmd={}",
                h.pid,
                h.kind,
                h.command.join(" ")
            )))
            .collect::<Vec<_>>()
            .join("; "),
        snap.compiling
            .iter()
            .map(|p| format!("pid={} name={} cwd={}", p.pid, p.name, p.cwd))
            .collect::<Vec<_>>()
            .join("; ")
    );
    75
}

fn execute(command: &[String]) -> i32 {
    let status = Command::new("nice")
        .arg("-n")
        .arg("15")
        // Without this, a child whose name starts with `-` is parsed by nice as its own option
        // and never runs (`gate run -- --version` printed nice's version instead).
        .arg("--")
        .args(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) => s
            .code()
            .or_else(|| s.signal().map(|sig| 128 + sig))
            .unwrap_or(1),
        Err(e) => {
            let _ = writeln!(io::stderr(), "gate run: {e}");
            1
        }
    }
}

pub fn status(json: bool) -> i32 {
    let snap = match config().and_then(|cfg| snapshot(&cfg)) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(io::stderr(), "status warning: {e}");
            return 0;
        }
    };
    if json {
        match serde_json::to_string_pretty(&snap) {
            Ok(s) => {
                let _ = writeln!(io::stdout(), "{s}");
            }
            Err(e) => {
                let _ = writeln!(io::stderr(), "status warning: {e}");
            }
        }
    } else {
        let _ = writeln!(
            io::stdout(),
            "load 1m: {:.2}; swap used: {} MB; MemAvailable: {} MB",
            snap.load_1m,
            snap.swap_used_mb,
            snap.mem_available_mb
        );
        for slot in &snap.slots {
            match &slot.holder {
                Some(h) => {
                    let _ = writeln!(
                        io::stdout(),
                        "slot-{}: pid={} kind={} cmd={} since={}",
                        slot.index,
                        h.pid,
                        h.kind,
                        h.command.join(" "),
                        h.started_at
                    );
                }
                None if slot.occupied => {
                    let _ = writeln!(
                        io::stdout(),
                        "slot-{}: occupied (unknown holder)",
                        slot.index
                    );
                }
                None => {
                    let _ = writeln!(io::stdout(), "slot-{}: empty", slot.index);
                }
            }
        }
        match &snap.compiler_lock.holder {
            Some(h) => {
                let _ = writeln!(
                    io::stdout(),
                    "compiler lock: pid={} kind={} cmd={} since={}",
                    h.pid,
                    h.kind,
                    h.command.join(" "),
                    h.started_at
                );
            }
            None if snap.compiler_lock.occupied => {
                let _ = writeln!(io::stdout(), "compiler lock: occupied (unknown holder)");
            }
            None => {
                let _ = writeln!(io::stdout(), "compiler lock: empty");
            }
        }
        for p in &snap.compiling {
            let _ = writeln!(
                io::stdout(),
                "compiler: pid={} name={} cwd={}",
                p.pid,
                p.name,
                p.cwd
            );
        }
        for b in &snap.orphan_brokers {
            let _ = writeln!(
                io::stdout(),
                "orphan broker: pid={} --cwd={} exists={}",
                b.pid,
                b.cwd_arg.as_deref().unwrap_or("?"),
                b.cwd_exists
            );
            for child in &b.children {
                let _ = writeln!(
                    io::stdout(),
                    "  child: pid={} name={} cwd={}",
                    child.pid,
                    child.name,
                    child.cwd
                );
            }
        }
    }
    0
}
