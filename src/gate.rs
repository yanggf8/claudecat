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

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Build,
    Test,
    E2e,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Heavy(Kind),
    NotHeavy,
    Unparsed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub argv: Vec<String>,
    pub dir: PathBuf,
    pub target: Option<PathBuf>,
}

#[derive(Debug)]
enum ShellToken {
    Word(String),
    Separator,
}

fn lex(command: &str) -> Option<Vec<ShellToken>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    // Pending heredoc delimiters, and whether each was `<<-` (which strips leading tabs).
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    word.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => word.push(chars.next()?),
                _ => word.push(ch),
            },
            _ => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    started = true;
                }
                '\\' => {
                    word.push(chars.next()?);
                    started = true;
                }
                ' ' | '\t' | '\r' => {
                    if started {
                        tokens.push(ShellToken::Word(std::mem::take(&mut word)));
                        started = false;
                    }
                }
                // A heredoc body is data, not commands: `cat > f <<'EOF'` followed by a line that
                // starts with `cargo test` must not read as a build. Record the delimiter here and
                // skip the body at the end of the line. `<<<` is a here-string, not a heredoc.
                '<' if chars.peek() == Some(&'<') => {
                    chars.next();
                    if chars.peek() == Some(&'<') {
                        chars.next();
                        word.push_str("<<<");
                        started = true;
                        continue;
                    }
                    if started {
                        tokens.push(ShellToken::Word(std::mem::take(&mut word)));
                        started = false;
                    }
                    let strip_tabs = chars.peek() == Some(&'-');
                    if strip_tabs {
                        chars.next();
                    }
                    while matches!(chars.peek(), Some(' ' | '\t')) {
                        chars.next();
                    }
                    let mut delimiter = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>') {
                            break;
                        }
                        chars.next();
                        if !matches!(c, '\'' | '"' | '\\') {
                            delimiter.push(c);
                        }
                    }
                    if delimiter.is_empty() {
                        return None;
                    }
                    heredocs.push((delimiter, strip_tabs));
                }
                '\n' if !heredocs.is_empty() => {
                    if started {
                        tokens.push(ShellToken::Word(std::mem::take(&mut word)));
                        started = false;
                    }
                    tokens.push(ShellToken::Separator);
                    for (delimiter, strip_tabs) in std::mem::take(&mut heredocs) {
                        let mut closed = false;
                        while chars.peek().is_some() {
                            let line: String = chars.by_ref().take_while(|&c| c != '\n').collect();
                            let line = if strip_tabs {
                                line.trim_start_matches('\t')
                            } else {
                                line.as_str()
                            };
                            if line == delimiter {
                                closed = true;
                                break;
                            }
                        }
                        if !closed {
                            // An unterminated body is still a body; nothing after it is a command.
                            return Some(tokens);
                        }
                    }
                }
                // A comment runs to the end of the line; `echo ok # && cargo test` runs no cargo.
                // `#` inside a word (`a#b`) is an ordinary character, as in the shell.
                '#' if !started => {
                    while chars.peek().is_some_and(|&c| c != '\n') {
                        chars.next();
                    }
                }
                // `2>&1` and `>&2` duplicate a descriptor; that `&` does not end a command.
                '&' if started && (word.ends_with('>') || word.ends_with('<')) => word.push(ch),
                '\n' | ';' | '|' | '&' => {
                    if started {
                        tokens.push(ShellToken::Word(std::mem::take(&mut word)));
                        started = false;
                    }
                    // A doubled operator still separates the same two segments.
                    if (ch == '|' || ch == '&') && chars.peek() == Some(&ch) {
                        chars.next();
                    }
                    tokens.push(ShellToken::Separator);
                }
                _ => {
                    word.push(ch);
                    started = true;
                }
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        tokens.push(ShellToken::Word(word));
    }
    Some(tokens)
}

fn assignment(s: &str) -> bool {
    s.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().enumerate().all(|(i, c)| {
                c == '_' || (c.is_ascii_alphabetic()) || (i > 0 && c.is_ascii_digit())
            })
    })
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// How many words a redirection at the front of a command occupies: `2>/tmp/x` is one, `2> /tmp/x`
/// and `<<< text` are two. `None` when the word is not a redirection.
fn redirection(word: &str) -> Option<usize> {
    let rest = word.trim_start_matches(|c: char| c.is_ascii_digit());
    if !rest.starts_with(['<', '>']) {
        return None;
    }
    let target = rest.trim_start_matches(['<', '>', '&', '|']);
    Some(if target.is_empty() { 2 } else { 1 })
}

fn segment_kind(words: &[String]) -> Option<Kind> {
    let mut i = 0;
    while i < words.len() {
        let name = basename(&words[i]);
        if let Some(n) = redirection(&words[i]) {
            i += n;
        } else if assignment(&words[i]) || name == "exec" || name == "command" {
            i += 1;
        } else if name == "nice" {
            i += 1;
            if i < words.len() && words[i] == "-n" {
                i += 2;
            } else if i < words.len()
                && (words[i].starts_with("-n")
                    || (words[i].starts_with('-')
                        && words[i][1..].chars().all(|c| c.is_ascii_digit())))
            {
                i += 1;
            }
        } else if name == "timeout" {
            i += 1;
            while i < words.len() && words[i].starts_with('-') {
                if matches!(words[i].as_str(), "-s" | "--signal" | "-k" | "--kill-after") {
                    i += 1;
                }
                i += 1;
            }
            i += 1; // duration
        } else if name == "env" {
            i += 1;
            while i < words.len() {
                if assignment(&words[i]) {
                    i += 1;
                } else if matches!(words[i].as_str(), "-u" | "--unset" | "-C" | "--chdir") {
                    i += 2;
                } else if words[i].starts_with("--chdir=") || words[i].starts_with('-') {
                    i += 1;
                } else {
                    break;
                }
            }
        } else {
            break;
        }
    }
    let cmd = words.get(i)?;
    match basename(cmd) {
        "cargo" => {
            i += 1;
            while let Some(arg) = words.get(i) {
                if arg.starts_with('+') || arg.starts_with('-') {
                    let takes_value = matches!(
                        arg.as_str(),
                        "-C" | "--config" | "--manifest-path" | "--target-dir" | "-Z" | "--color"
                    );
                    i += 1 + usize::from(takes_value);
                } else {
                    break;
                }
            }
            match words.get(i).map(String::as_str) {
                Some("test" | "bench" | "nextest") => Some(Kind::Test),
                Some("build" | "clippy" | "check" | "run" | "install" | "doc") => Some(Kind::Build),
                _ => None,
            }
        }
        "gradle" | "gradlew" => Some(Kind::Build),
        _ => None,
    }
}

/// Classify a shell command conservatively; malformed quotes allow the command through.
pub fn classify(command: &str) -> Classification {
    let Some(tokens) = lex(command) else {
        return Classification::Unparsed;
    };
    let mut segment = Vec::new();
    let mut result = Classification::NotHeavy;
    for token in tokens.iter().chain(std::iter::once(&ShellToken::Separator)) {
        match token {
            ShellToken::Word(word) => segment.push(word.clone()),
            ShellToken::Separator => {
                if let Some(kind) = segment_kind(&segment) {
                    if kind == Kind::Test {
                        result = Classification::Heavy(Kind::Test);
                    } else if result == Classification::NotHeavy {
                        result = Classification::Heavy(kind);
                    }
                }
                segment.clear();
            }
        }
    }
    result
}

fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolved(base: &Path, value: &str) -> PathBuf {
    let expanded = if value == "~" || value.starts_with("~/") {
        env::var("HOME")
            .map(|home| format!("{home}{}", &value[1..]))
            .unwrap_or_else(|_| value.to_owned())
    } else {
        value.to_owned()
    };
    let path = Path::new(&expanded);
    normalized(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}

fn option_value<'a>(words: &'a [String], key: &str) -> Option<&'a str> {
    words.iter().enumerate().find_map(|(i, word)| {
        if word == key {
            words.get(i + 1).map(String::as_str)
        } else {
            word.strip_prefix(key).and_then(|v| v.strip_prefix('='))
        }
    })
}

fn identity(words: &[String], base: &Path, inherited_target: Option<&str>) -> Option<Identity> {
    let cargo = words.iter().position(|word| basename(word) == "cargo")?;
    let prefix = &words[..cargo];
    let env_dir = option_value(prefix, "-C").or_else(|| option_value(prefix, "--chdir"));
    let cargo_words = &words[cargo + 1..];
    let dir = normalized(base);
    let dir = env_dir.map(|v| resolved(&dir, v)).unwrap_or(dir);
    let dir = option_value(cargo_words, "-C")
        .map(|v| resolved(&dir, v))
        .unwrap_or(dir);
    let dir = option_value(cargo_words, "--manifest-path")
        .map(|v| resolved(&dir, v).parent().unwrap_or(&dir).to_path_buf())
        .unwrap_or(dir);
    let mut argv = Vec::new();
    let mut i = 0;
    while i < cargo_words.len() {
        let word = &cargo_words[i];
        if let Some(n) = redirection(word) {
            i += n;
            continue;
        }
        if i == 0 && word.starts_with('+') {
            i += 1;
            continue;
        }
        argv.push(word.clone());
        i += 1;
    }
    let prefix_target = prefix
        .iter()
        .find_map(|word| word.strip_prefix("CARGO_TARGET_DIR="));
    let target_value = option_value(cargo_words, "--target-dir")
        .or(prefix_target)
        .or(inherited_target);
    let target = Some(
        target_value
            .map(|v| resolved(&dir, v))
            .unwrap_or_else(|| dir.join("target")),
    );
    Some(Identity { argv, dir, target })
}

/// Classify a command and identify its selected heavy Cargo segment using the payload cwd.
pub fn classify_with_cwd(command: &str, cwd: &Path) -> (Classification, Option<Identity>) {
    let Some(tokens) = lex(command) else {
        return (Classification::Unparsed, None);
    };
    let mut segment = Vec::new();
    let mut selected = None;
    let mut current_dir = if cwd.is_absolute() {
        normalized(cwd)
    } else {
        env::current_dir()
            .map(|base| normalized(&base.join(cwd)))
            .unwrap_or_else(|_| normalized(cwd))
    };
    for token in tokens.iter().chain(std::iter::once(&ShellToken::Separator)) {
        match token {
            ShellToken::Word(word) => segment.push(word.clone()),
            ShellToken::Separator => {
                if let Some(kind) = segment_kind(&segment) {
                    if selected
                        .as_ref()
                        .is_none_or(|(old, _)| *old != Kind::Test && kind == Kind::Test)
                    {
                        selected = Some((
                            kind,
                            identity(
                                &segment,
                                &current_dir,
                                env::var("CARGO_TARGET_DIR").ok().as_deref(),
                            ),
                        ));
                    }
                }
                if segment.first().is_some_and(|w| w == "cd") {
                    if let Some(path) = segment.get(1) {
                        current_dir = resolved(&current_dir, path);
                    }
                }
                segment.clear();
            }
        }
    }
    selected.map_or((Classification::NotHeavy, None), |(kind, id)| {
        (Classification::Heavy(kind), id)
    })
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
    #[serde(default, skip_serializing)]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing)]
    pub target: Option<String>,
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
        // Use the real proc mount, as snapshot() does: CLAUDECAT_PROC_ROOT may be a fixture.
        let uid = fs::metadata("/proc/self")?.uid();
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
    environ_target: Option<String>,
    environ_readable: bool,
}

fn processes(root: &Path, own_uid: u32, read_environ: bool) -> io::Result<Vec<ProcEntry>> {
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
        let cmdline: Vec<String> = cmd
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
        let environ = if read_environ
            && uid == own_uid
            && cmdline.first().is_some_and(|s| basename(s) == "cargo")
        {
            fs::read(dir.join("environ")).ok()
        } else {
            None
        };
        let environ_target = environ.as_ref().and_then(|bytes| {
            bytes.split(|b| *b == 0).find_map(|item| {
                item.strip_prefix(b"CARGO_TARGET_DIR=")
                    .map(|v| String::from_utf8_lossy(v).into_owned())
            })
        });
        found.push(ProcEntry {
            uid,
            pid,
            name: name.trim().into(),
            cmdline,
            cwd,
            ppid,
            environ_target,
            environ_readable: environ.is_some(),
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
    snapshot_with_processes(cfg, false).map(|(snap, _)| snap)
}

fn snapshot_with_processes(
    cfg: &Config,
    read_environ: bool,
) -> io::Result<(Snapshot, Vec<ProcEntry>)> {
    fs::create_dir_all(&cfg.gate_dir)?;
    // Our own uid, not the gate directory's owner: the directory can be shared with a group and
    // must not decide whose compilers count. CLAUDECAT_PROC_ROOT is the fixtures' fake proc, so
    // read the real /proc/self; fall back to the directory owner only if it is unreadable.
    let uid = fs::metadata("/proc/self")
        .map(|m| m.uid())
        .or_else(|_| fs::metadata(&cfg.gate_dir).map(|m| m.uid()))?;
    let procs = processes(&cfg.proc_root, uid, read_environ)?;
    let (load_1m, swap_used_mb, mem_available_mb) = metrics(&cfg.proc_root)?;
    let snap = Snapshot {
        load_1m,
        swap_used_mb,
        mem_available_mb,
        slots: slots(cfg)?,
        compiler_lock: compiler_lock(cfg)?,
        compiling: compiling(&procs, uid),
        orphan_brokers: brokers(&procs),
    };
    Ok((snap, procs))
}

fn duplicate(wanted: &Identity, snap: &Snapshot, procs: &[ProcEntry], uid: u32) -> Option<String> {
    let mut canonical_dir = None;
    let holders = snap
        .slots
        .iter()
        .filter(|s| s.occupied)
        .filter_map(|s| s.holder.as_ref())
        .chain(
            snap.compiler_lock
                .holder
                .iter()
                .filter(|_| snap.compiler_lock.occupied),
        );
    let candidates = holders.filter_map(|holder| {
        let cwd = holder.cwd.as_ref()?;
        let id = identity(&holder.command, Path::new(cwd), holder.target.as_deref())?;
        Some((holder.pid, Some(holder.started_at), id))
    });
    let running = procs
        .iter()
        .filter(|p| p.uid == uid && p.cmdline.first().is_some_and(|s| basename(s) == "cargo"))
        .filter_map(|p| {
            if !p.environ_readable && option_value(&p.cmdline, "--target-dir").is_none() {
                return None;
            }
            let words = &p.cmdline;
            let id = identity(words, Path::new(&p.cwd), p.environ_target.as_deref())?;
            Some((p.pid, None, id))
        });
    for (pid, since, candidate) in candidates.chain(running) {
        if wanted.argv != candidate.argv {
            continue;
        }
        let same_dir = wanted.dir == candidate.dir || {
            let canonical = canonical_dir.get_or_insert_with(|| fs::canonicalize(&wanted.dir).ok());
            canonical.as_ref() == Some(&candidate.dir)
        };
        if !same_dir {
            continue;
        }
        let target = if wanted.dir != candidate.dir
            && wanted.target.as_ref() == Some(&wanted.dir.join("target"))
        {
            canonical_dir
                .as_ref()
                .and_then(|p| p.as_ref())
                .map(|p| p.join("target"))
        } else {
            wanted.target.clone()
        };
        if target != candidate.target {
            continue;
        }
        let since = since
            .map(|started| {
                format!(
                    " since={}s",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs().saturating_sub(started))
                        .unwrap_or(0)
                )
            })
            .unwrap_or_default();
        return Some(format!("gate denied (duplicate) the same command is already running: pid={pid}{since} dir={} cmd=cargo {}. Its output is not visible to you; wait for it to finish, then run this once (the build will be warm). Do not poll with sleep in the foreground.", candidate.dir.display(), wanted.argv.join(" ")));
    }
    None
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
    serde_json::to_writer(
        &mut *file,
        &serde_json::json!({
            "pid": holder.pid,
            "kind": holder.kind,
            "command": holder.command,
            "started_at": holder.started_at,
            "cwd": holder.cwd,
            "target": holder.target,
        }),
    )?;
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

pub fn hook() -> i32 {
    let mut payload = String::new();
    if io::stdin().read_to_string(&mut payload).is_err() {
        return 0;
    }
    let Some((command, cwd)) = serde_json::from_str::<serde_json::Value>(&payload)
        .ok()
        .and_then(|v| {
            let command = v.pointer("/tool_input/command")?.as_str()?.to_owned();
            let cwd = v
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .or_else(|| env::current_dir().ok())?;
            Some((command, cwd))
        })
    else {
        return 0;
    };
    let (classification, identity) = classify_with_cwd(&command, &cwd);
    let Classification::Heavy(kind) = classification else {
        return 0;
    };
    let Ok((cfg, snap, procs)) = config().and_then(|cfg| {
        snapshot_with_processes(&cfg, true).map(|(snap, procs)| (cfg, snap, procs))
    }) else {
        return 0;
    };
    let uid = fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    let decision = identity
        .as_ref()
        .and_then(|id| duplicate(id, &snap, &procs, uid))
        .or_else(|| deny_reason(&snap, &cfg, kind).map(|r| format!("{r} Denied, not queued: nothing will run this command later. Retry it after the holders above finish; do not poll with sleep in the foreground.")));
    if let Some(reason) = decision {
        // One line, always: holder commands and compiler cwds are arbitrary bytes and may hold newlines.
        let reason = reason.replace(['\n', '\r'], " ");
        let _ = writeln!(io::stdout(), "{reason}");
        75
    } else {
        0
    }
}

fn cort_path() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &PathBuf| {
        p.is_file()
            && fs::metadata(p)
                .ok()
                .is_some_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("cort");
            if executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share/cortexyoung/cort"))
        .filter(executable)
}

pub fn install(install: bool) -> i32 {
    let Some(cort) = cort_path() else {
        eprintln!("gate: cort not found on PATH or in ~/.local/share/cortexyoung/cort");
        return 1;
    };
    let mut cmd = Command::new(cort);
    cmd.arg("hook-gate");
    if install {
        let exe = match env::current_exe() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("gate install: {e}");
                return 1;
            }
        };
        cmd.args(["set", "--"]).arg(exe).args(["gate", "hook"]);
    } else {
        cmd.arg("clear");
    }
    match cmd.status() {
        Ok(status) => status
            .code()
            .or_else(|| status.signal().map(|s| 128 + s))
            .unwrap_or(1),
        Err(e) => {
            eprintln!("gate: cannot run cort: {e}");
            1
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
                    cwd: env::current_dir()
                        .ok()
                        .and_then(|p| fs::canonicalize(p).ok())
                        .map(|p| p.to_string_lossy().into_owned()),
                    target: env::var("CARGO_TARGET_DIR").ok(),
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

#[cfg(test)]
mod duplicate_tests {
    use super::*;

    #[test]
    fn foreign_uid_cargo_is_not_a_duplicate() {
        let id = Identity {
            argv: vec!["test".into()],
            dir: PathBuf::from("/tmp/g4-uid"),
            target: Some(PathBuf::from("/tmp/g4-uid/target")),
        };
        let snap = Snapshot {
            load_1m: 0.0,
            swap_used_mb: 0,
            mem_available_mb: 0,
            slots: Vec::new(),
            compiler_lock: CompilerLock {
                holder: None,
                occupied: false,
            },
            compiling: Vec::new(),
            orphan_brokers: Vec::new(),
        };
        let process = ProcEntry {
            uid: 5001,
            pid: 123,
            name: "cargo".into(),
            cmdline: vec!["cargo".into(), "test".into()],
            cwd: "/tmp/g4-uid".into(),
            ppid: None,
            environ_target: None,
            environ_readable: true,
        };
        assert!(duplicate(&id, &snap, &[process], 5002).is_none());
    }
}
