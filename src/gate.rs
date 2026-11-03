use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Fallbacks for a proc root that lacks cpuinfo or MemTotal; a real /proc always has both, and
// then the limits come from the machine (see `Limits`).
const DEFAULT_LOAD: f64 = 6.0;
// Memory is judged by current pressure, not swap used: on WSL2 idle pages of long-lived
// processes stay in swap forever, so swap used never drops even on an idle machine.
const DEFAULT_MEM_PRESSURE: f64 = 10.0;
const DEFAULT_AVAILABLE_MB: u64 = 1536;
const DEFAULT_SLOTS: usize = 3;
// Load average allowed per CPU: the fleet's long-standing 6 was tuned on the NUC, whose WSL
// exposes 6 CPUs, so 1.0 per CPU keeps it there.
const LOAD_PER_CPU: f64 = 1.0;
// Estimated peak memory of a fresh build, reserved only while its holder is still warming up.
const DEFAULT_RESERVE_GRADLE_MB: u64 = 3072;
const DEFAULT_RESERVE_CARGO_MB: u64 = 1536;
const DEFAULT_RESERVE_WARMUP_SECS: u64 = 60;

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

/// Index of the word a segment executes, past redirections, assignments and the
/// wrappers (`exec`, `command`, `nice`, `timeout`, `env`) together with their options.
fn command_index(words: &[String]) -> usize {
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
    i
}

fn segment_kind(words: &[String]) -> Option<Kind> {
    let mut i = command_index(words);
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
                Some("build" | "clippy" | "check" | "run" | "doc") => Some(Kind::Build),
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

/// The compiler family a command drives. Compilers of the same family exclude each other; two
/// families only compete for memory, which the reservation covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Toolchain {
    Cargo,
    Gradle,
}

const TOOLCHAINS: [Toolchain; 2] = [Toolchain::Cargo, Toolchain::Gradle];

impl Toolchain {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Gradle => "gradle",
        }
    }

    fn lock_name(self) -> String {
        format!("compiler-{}.lock", self.as_str())
    }

    fn estimate_mb(self, cfg: &Config) -> u64 {
        match self {
            Self::Cargo => cfg.reserve_cargo_mb,
            Self::Gradle => cfg.reserve_gradle_mb,
        }
    }
}

fn segment_toolchain(words: &[String]) -> Option<Toolchain> {
    segment_kind(words)?;
    Some(match basename(&words[command_index(words)]) {
        "cargo" => Toolchain::Cargo,
        _ => Toolchain::Gradle,
    })
}

/// Toolchains of every heavy segment of a shell command, sorted; empty when none is recognised.
fn command_toolchains(command: &str) -> Vec<Toolchain> {
    let Some(tokens) = lex(command) else {
        return Vec::new();
    };
    let mut segment = Vec::new();
    let mut found = BTreeSet::new();
    for token in tokens.iter().chain(std::iter::once(&ShellToken::Separator)) {
        match token {
            ShellToken::Word(word) => segment.push(word.clone()),
            ShellToken::Separator => {
                found.extend(segment_toolchain(&segment));
                segment.clear();
            }
        }
    }
    found.into_iter().collect()
}

/// Toolchains of an argv (a `gate run` command or a holder's), looking through `sh -c "..."`.
fn argv_toolchains(argv: &[String]) -> Vec<Toolchain> {
    if let Some(toolchain) = segment_toolchain(argv) {
        return vec![toolchain];
    }
    let i = command_index(argv);
    if argv
        .get(i)
        .is_some_and(|c| matches!(basename(c), "sh" | "bash" | "zsh"))
    {
        if let Some(script) = argv[i + 1..].iter().skip_while(|a| *a != "-c").nth(1) {
            return command_toolchains(script);
        }
    }
    Vec::new()
}

/// Test runners that are known not to drive cargo or Gradle. Anything else unrecognised (`make`,
/// `just`, a shell script) may, so it keeps holding every toolchain lock.
fn runs_no_compiler(argv: &[String]) -> bool {
    argv.get(command_index(argv)).is_some_and(|c| {
        matches!(
            basename(c),
            "npx"
                | "npm"
                | "pnpm"
                | "yarn"
                | "node"
                | "vitest"
                | "jest"
                | "playwright"
                | "pytest"
                | "python"
                | "python3"
        )
    })
}

fn process_toolchain(p: &Process) -> Option<Toolchain> {
    match p.name.as_str() {
        "rustc" => Some(Toolchain::Cargo),
        "java" => Some(Toolchain::Gradle),
        _ => None,
    }
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

/// The Cargo subcommand of an identity's argv, skipping global flags and their values
/// the way `segment_kind` does, so a flag value (a path) never stands in for it.
fn cargo_subcommand(argv: &[String]) -> String {
    let mut i = 0;
    while let Some(arg) = argv.get(i) {
        if arg.starts_with('-') {
            let takes_value = matches!(
                arg.as_str(),
                "-C" | "--config" | "--manifest-path" | "--target-dir" | "-Z" | "--color"
            );
            i += 1 + usize::from(takes_value);
        } else {
            return arg.clone();
        }
    }
    "cargo".into()
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
    let cargo = command_index(words);
    if basename(words.get(cargo)?) != "cargo" {
        return None;
    }
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

#[derive(Serialize)]
struct ToolchainLock {
    toolchain: Toolchain,
    holder: Option<Holder>,
    occupied: bool,
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
    /// PSI `some avg10` from `pressure/memory`; None when the kernel has no PSI.
    pub mem_pressure_some_avg10: Option<f64>,
    pub slots: Vec<Slot>,
    /// Pool for `--kind other` (LLM wrappers, scripts): separate from the build/test slots so a
    /// long, nearly idle wrapper never blocks a build.
    pub other_slots: Vec<Slot>,
    pub limits: Limits,
    /// Summary of `compiler_locks` for older readers: occupied if any toolchain lock is.
    pub compiler_lock: CompilerLock,
    compiler_locks: Vec<ToolchainLock>,
    pub compiling: Vec<Process>,
    pub orphan_brokers: Vec<Broker>,
    /// Recent hook admissions (see `Admission`); counted by `reserved` like warming-up holders.
    #[serde(skip)]
    admissions: Vec<Admission>,
}

/// The machine's resources and the limits derived from them, with where each limit came from.
#[derive(Serialize, Clone, Debug)]
pub struct Limits {
    pub cpus: Option<usize>,
    pub mem_total_mb: Option<u64>,
    pub slots: usize,
    pub slots_basis: String,
    pub other_slots: usize,
    pub other_slots_basis: String,
    pub max_load: f64,
    pub max_load_basis: String,
}

struct Config {
    proc_root: PathBuf,
    gate_dir: PathBuf,
    max_load: f64,
    max_mem_pressure: f64,
    min_available_mb: u64,
    slots: usize,
    other_slots: usize,
    limits: Limits,
    reserve_gradle_mb: u64,
    reserve_cargo_mb: u64,
    reserve_warmup_secs: u64,
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

fn positive_usize(key: &str) -> Option<usize> {
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
}

/// Logical CPUs listed in `cpuinfo`; None when the proc root has no such file.
fn cpus(root: &Path) -> Option<usize> {
    let n = fs::read_to_string(root.join("cpuinfo"))
        .ok()?
        .lines()
        .filter(|line| {
            line.split(':')
                .next()
                .is_some_and(|key| key.trim() == "processor")
        })
        .count();
    (n > 0).then_some(n)
}

fn mem_total_mb(root: &Path) -> Option<u64> {
    fs::read_to_string(root.join("meminfo"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|kb| kb / 1024)
}

/// Build/test slots: one per two CPUs, and no more than MemTotal holds of the largest toolchain
/// estimate. Other slots: one per CPU. Load: `LOAD_PER_CPU` per CPU. Environment variables win.
fn limits(root: &Path, per_slot_mb: u64) -> Limits {
    let cpus = cpus(root);
    let mem_total_mb = mem_total_mb(root);
    let (slots, slots_basis) = if let Some(n) = positive_usize("CLAUDECAT_SLOTS") {
        (n, "CLAUDECAT_SLOTS".to_owned())
    } else {
        match (cpus, mem_total_mb) {
            (Some(c), Some(m)) => {
                let by_cpu = (c / 2).max(1);
                let by_mem = usize::try_from(m / per_slot_mb.max(1))
                    .unwrap_or(usize::MAX)
                    .max(1);
                (
                    by_cpu.min(by_mem),
                    format!("min(cpus {c}/2={by_cpu}, MemTotal {m}MB/{per_slot_mb}MB={by_mem})"),
                )
            }
            (Some(c), None) => ((c / 2).max(1), format!("cpus {c}/2")),
            _ => (DEFAULT_SLOTS, "default (cpus unknown)".to_owned()),
        }
    };
    let (other_slots, other_slots_basis) = if let Some(n) = positive_usize("CLAUDECAT_OTHER_SLOTS")
    {
        (n, "CLAUDECAT_OTHER_SLOTS".to_owned())
    } else if let Some(c) = cpus {
        (c, format!("cpus {c}"))
    } else {
        (DEFAULT_SLOTS, "default (cpus unknown)".to_owned())
    };
    let env_load = env::var("CLAUDECAT_MAX_LOAD")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n > 0.0);
    #[allow(clippy::cast_precision_loss)]
    let (max_load, max_load_basis) = if let Some(n) = env_load {
        (n, "CLAUDECAT_MAX_LOAD".to_owned())
    } else if let Some(c) = cpus {
        (c as f64 * LOAD_PER_CPU, format!("cpus {c}x{LOAD_PER_CPU}"))
    } else {
        (DEFAULT_LOAD, "default (cpus unknown)".to_owned())
    };
    Limits {
        cpus,
        mem_total_mb,
        slots,
        slots_basis,
        other_slots,
        other_slots_basis,
        max_load,
        max_load_basis,
    }
}

fn config() -> io::Result<Config> {
    let proc_root = env::var_os("CLAUDECAT_PROC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc"));
    let reserve_gradle_mb = positive_u64("CLAUDECAT_RESERVE_GRADLE_MB", DEFAULT_RESERVE_GRADLE_MB);
    let reserve_cargo_mb = positive_u64("CLAUDECAT_RESERVE_CARGO_MB", DEFAULT_RESERVE_CARGO_MB);
    let limits = limits(&proc_root, reserve_gradle_mb.max(reserve_cargo_mb));
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
        proc_root,
        gate_dir,
        max_load: limits.max_load,
        max_mem_pressure: positive_f64("CLAUDECAT_MAX_MEM_PRESSURE", DEFAULT_MEM_PRESSURE),
        min_available_mb: positive_u64("CLAUDECAT_MIN_AVAILABLE_MB", DEFAULT_AVAILABLE_MB),
        slots: limits.slots,
        other_slots: limits.other_slots,
        limits,
        reserve_gradle_mb,
        reserve_cargo_mb,
        reserve_warmup_secs: positive_u64(
            "CLAUDECAT_RESERVE_WARMUP_SECS",
            DEFAULT_RESERVE_WARMUP_SECS,
        ),
    })
}

pub(crate) fn metrics(root: &Path) -> io::Result<(f64, u64, u64)> {
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

/// PSI is optional: a kernel without CONFIG_PSI has no file, and one booted with psi=0 fails the
/// read. Either way the gate falls back to MemAvailable alone instead of failing open.
pub(crate) fn mem_pressure(root: &Path) -> Option<f64> {
    fs::read_to_string(root.join("pressure/memory"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("some "))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}

/// Memory held back for holders still starting up. A holder older than the warm-up window is
/// already inside MemAvailable, so reserving it again would count it twice. A holder seen in both
/// its slot and a compiler lock counts once.
fn reserved(s: &Snapshot, cfg: &Config, now: u64) -> BTreeMap<Toolchain, u64> {
    let holders = s
        .slots
        .iter()
        .filter(|slot| slot.occupied)
        .filter_map(|slot| slot.holder.as_ref())
        .chain(
            s.compiler_locks
                .iter()
                .filter(|lock| lock.occupied)
                .filter_map(|lock| lock.holder.as_ref()),
        );
    let mut seen = BTreeSet::new();
    let mut table = BTreeMap::new();
    for holder in holders {
        if !seen.insert(holder.pid)
            || now.saturating_sub(holder.started_at) >= cfg.reserve_warmup_secs
        {
            continue;
        }
        for toolchain in argv_toolchains(&holder.command) {
            let slot = table.entry(toolchain).or_insert(0u64);
            *slot = slot.saturating_add(toolchain.estimate_mb(cfg));
        }
    }
    for admission in &s.admissions {
        if now.saturating_sub(admission.ts) >= cfg.reserve_warmup_secs {
            continue;
        }
        for toolchain in &admission.toolchains {
            let slot = table.entry(*toolchain).or_insert(0u64);
            *slot = slot.saturating_add(toolchain.estimate_mb(cfg));
        }
    }
    table
}

/// A command the PreToolUse hook allowed. Claude runs it directly, so it registers no holder;
/// until the warm-up window passes the row stands in for one. No command text is stored.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Admission {
    ts: u64,
    session: Option<String>,
    toolchains: Vec<Toolchain>,
}

const ADMISSION_FILE: &str = "admitted.jsonl";
const ADMISSION_LOCK: &str = "admission.lock";
const ADMISSION_MAX_ROWS: usize = 64;
const ADMISSION_LOCK_BUDGET: Duration = Duration::from_millis(50);
const ADMISSION_CLOCK_SLACK_SECS: u64 = 5;

/// Admissions younger than the warm-up window; unparsable lines are skipped.
fn read_admissions(dir: &Path, now: u64, warmup: u64) -> io::Result<Vec<Admission>> {
    let text = match fs::read_to_string(dir.join(ADMISSION_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Admission>(line).ok())
        // A row stamped in the future (clock step, hand edit) would otherwise stay "fresh"
        // until the clock catches up.
        .filter(|a| a.ts <= now.saturating_add(ADMISSION_CLOCK_SLACK_SECS))
        .filter(|a| now.saturating_sub(a.ts) < warmup)
        .map(|mut a| {
            a.toolchains.sort();
            a.toolchains.dedup();
            a
        })
        .collect())
}

/// Exclusive hold on `admission.lock`, serializing "check the table, then record" between hooks.
struct AdmissionGuard {
    dir: PathBuf,
    _lock: File,
}

impl AdmissionGuard {
    /// None on timeout or any I/O error: a broken cache must never decide an outcome, so callers
    /// fail open and just skip recording. (Fresh rows read without the lock are still real
    /// reservations and keep counting; only the atomic check-then-record is lost.)
    fn acquire(dir: &Path) -> Option<Self> {
        let lock = lock_file(&dir.join(ADMISSION_LOCK)).ok()?;
        let start = std::time::Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => {
                    return Some(Self {
                        dir: dir.to_owned(),
                        _lock: lock,
                    })
                }
                Err(std::fs::TryLockError::WouldBlock)
                    if start.elapsed() < ADMISSION_LOCK_BUDGET =>
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
    }

    /// Append `row` after pruning rows older than the warm-up window, keeping the newest 64.
    fn admit(&self, row: Admission, now: u64, warmup: u64) -> io::Result<()> {
        let mut rows = read_admissions(&self.dir, now, warmup)?;
        // Commands of one session normally run one after another, so a new admission replaces
        // that session's earlier one for the same toolchain: the earlier command has most likely
        // finished and its memory is already inside MemAvailable. Without this, a few quick
        // `cargo check` runs would add up to a reservation that denies on an idle machine.
        for old in rows.iter_mut().filter(|old| old.session == row.session) {
            old.toolchains.retain(|t| !row.toolchains.contains(t));
        }
        rows.retain(|old| !old.toolchains.is_empty());
        rows.push(row);
        let skip = rows.len().saturating_sub(ADMISSION_MAX_ROWS);
        let mut data = Vec::new();
        for row in &rows[skip..] {
            serde_json::to_writer(&mut data, row)?;
            data.push(b'\n');
        }
        let tmp = self.dir.join(format!("{ADMISSION_FILE}.tmp"));
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&data)?;
        fs::rename(&tmp, self.dir.join(ADMISSION_FILE))
    }
}

fn reserved_figure(table: &BTreeMap<Toolchain, u64>) -> String {
    if table.is_empty() {
        return "reserved=none".into();
    }
    let rows: Vec<_> = table
        .iter()
        .map(|(toolchain, mb)| format!("{}:{mb}MB", toolchain.as_str()))
        .collect();
    format!("reserved={}", rows.join(","))
}

/// Memory figures shared by every deny message, as key=value/limit.
fn memory_figures(s: &Snapshot, cfg: &Config) -> String {
    let psi = match s.mem_pressure_some_avg10 {
        Some(psi) => format!("{psi:.2}/{:.2}", cfg.max_mem_pressure),
        None => "n/a".into(),
    };
    format!(
        "psi_some_avg10={psi} avail={}MB/{}MB swap={}MB {}",
        s.mem_available_mb,
        cfg.min_available_mb,
        s.swap_used_mb,
        reserved_figure(&reserved(s, cfg, unix_now()))
    )
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

/// Lock-file prefix and size of the pool a kind takes its slot from.
fn pool(cfg: &Config, kind: Kind) -> (&'static str, usize) {
    if kind == Kind::Other {
        ("other", cfg.other_slots)
    } else {
        ("slot", cfg.slots)
    }
}

fn slots(cfg: &Config, prefix: &str, count: usize) -> io::Result<Vec<Slot>> {
    fs::create_dir_all(&cfg.gate_dir)?;
    (0..count)
        .map(|index| {
            let (occupied, holder) =
                probe_lock(&cfg.gate_dir.join(format!("{prefix}-{index}.lock")))?;
            Ok(Slot {
                index,
                holder,
                occupied,
            })
        })
        .collect()
}

fn compiler_locks(cfg: &Config) -> io::Result<Vec<ToolchainLock>> {
    TOOLCHAINS
        .iter()
        .map(|&toolchain| {
            let (occupied, holder) = probe_lock(&cfg.gate_dir.join(toolchain.lock_name()))?;
            Ok(ToolchainLock {
                toolchain,
                holder,
                occupied,
            })
        })
        .collect()
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
/// every build/test until it timed out. A gated Gradle build is covered by `compiler-gradle.lock` instead.
/// The JVM requirement and the dotted suffix keep `rg …GradleWorkerMain` and `tail -f` on a log --
/// mentions, not compilers -- from blocking the gate. Workers outlive the build too (issue #1: an
/// idle one denied every compile for three hours), so a worker is only a candidate; `compiling`
/// counts it only if its CPU ticks grew across a short sample.
fn is_gradle(p: &ProcEntry) -> bool {
    p.name == "java" && p.cmdline.iter().any(|a| a.ends_with(".GradleWorkerMain"))
}

fn cpu_sample(root: &Path, pid: u32) -> Option<(u64, u64)> {
    let stat = fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let (_, tail) = stat.rsplit_once(") ")?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let starttime = fields.get(19)?.parse().ok()?;
    Some((utime.checked_add(stime)?, starttime))
}

fn compiling(processes: &[ProcEntry], uid: u32, root: &Path) -> Vec<Process> {
    let workers: Vec<_> = processes
        .iter()
        .filter(|p| p.uid == uid && is_gradle(p))
        .map(|p| (p.pid, cpu_sample(root, p.pid)))
        .collect();
    if !workers.is_empty() {
        // The sample has to fit inside cort hook-gate's timeout (200 ms by default), which allows
        // the command when it expires: a longer sleep would turn every deny reason into an allow
        // whenever a worker exists. The cap is hook-gate's own maximum timeout. At 100 Hz ticks,
        // 100 ms is ~10 ticks for one busy thread.
        let millis = env::var("CLAUDECAT_GRADLE_SAMPLE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(100)
            .min(1000);
        std::thread::sleep(Duration::from_millis(millis));
    }
    processes
        .iter()
        .filter(|p| {
            p.uid == uid
                && (p.name == "rustc"
                    || workers.iter().any(|(pid, first)| {
                        *pid == p.pid
                            && first.zip(cpu_sample(root, *pid)).is_some_and(
                                |((ticks, start), (later_ticks, later_start))| {
                                    start == later_start && later_ticks > ticks
                                },
                            )
                    }))
        })
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

fn snapshot(cfg: &Config, check_compilers: bool) -> io::Result<Snapshot> {
    snapshot_with_processes(cfg, false, check_compilers).map(|(snap, _)| snap)
}

fn snapshot_with_processes(
    cfg: &Config,
    read_environ: bool,
    check_compilers: bool,
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
    let compiler_locks = compiler_locks(cfg)?;
    let first_held = compiler_locks.iter().find(|lock| lock.occupied);
    let snap = Snapshot {
        load_1m,
        swap_used_mb,
        mem_available_mb,
        mem_pressure_some_avg10: mem_pressure(&cfg.proc_root),
        slots: slots(cfg, "slot", cfg.slots)?,
        other_slots: slots(cfg, "other", cfg.other_slots)?,
        limits: cfg.limits.clone(),
        compiler_lock: CompilerLock {
            holder: first_held.and_then(|lock| lock.holder.clone()),
            occupied: first_held.is_some(),
        },
        compiler_locks,
        compiling: if check_compilers {
            compiling(&procs, uid, &cfg.proc_root)
        } else {
            Vec::new()
        },
        orphan_brokers: brokers(&procs),
        admissions: read_admissions(&cfg.gate_dir, unix_now(), cfg.reserve_warmup_secs)
            .unwrap_or_default(),
    };
    Ok((snap, procs))
}

fn duplicate(wanted: &Identity, snap: &Snapshot, procs: &[ProcEntry], uid: u32) -> Option<String> {
    let mut canonical_dir = None;
    let holders = snap
        .slots
        .iter()
        .chain(&snap.other_slots)
        .filter(|s| s.occupied)
        .filter_map(|s| s.holder.as_ref())
        .chain(
            snap.compiler_locks
                .iter()
                .filter(|lock| lock.occupied)
                .filter_map(|lock| lock.holder.as_ref()),
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
        // A target under the command's dir (the default, or a relative --target-dir or
        // CARGO_TARGET_DIR) follows the dir when only the canonical form matched.
        let target = match (
            &wanted.target,
            canonical_dir.as_ref().and_then(|p| p.as_ref()),
        ) {
            (Some(target), Some(canonical)) if wanted.dir != candidate.dir => Some(
                target
                    .strip_prefix(&wanted.dir)
                    .map(|rest| canonical.join(rest))
                    .unwrap_or_else(|_| target.clone()),
            ),
            _ => wanted.target.clone(),
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

impl Snapshot {
    fn pool(&self, kind: Kind) -> &[Slot] {
        if kind == Kind::Other {
            &self.other_slots
        } else {
            &self.slots
        }
    }
}

/// `wanted` is the toolchains the command drives; empty means unknown, and then every toolchain's
/// compilers and lock count against it and no reservation is checked. `pool_kind` picks the slot
/// pool (the declared kind); `kind` picks the checks (a JS test runner is checked as Other).
fn deny_reason(
    s: &Snapshot,
    cfg: &Config,
    kind: Kind,
    pool_kind: Kind,
    wanted: &[Toolchain],
) -> Option<(String, Vec<&'static str>)> {
    let (prefix, total) = pool(cfg, pool_kind);
    let pool_slots = s.pool(pool_kind);
    let counts = |t: Toolchain| wanted.is_empty() || wanted.contains(&t);
    let table = reserved(s, cfg, unix_now());
    let need = wanted
        .iter()
        .fold(0u64, |sum, t| sum.saturating_add(t.estimate_mb(cfg)));
    let reasons = [
        (s.load_1m > cfg.max_load, "load"),
        (
            s.mem_pressure_some_avg10
                .is_some_and(|psi| psi > cfg.max_mem_pressure)
                || s.mem_available_mb < cfg.min_available_mb,
            "memory",
        ),
        (pool_slots.iter().all(|slot| slot.occupied), "slots"),
        (
            kind.checks_compilers()
                && s.compiling
                    .iter()
                    .any(|p| process_toolchain(p).is_none_or(counts)),
            "compiler",
        ),
        (
            kind.checks_compilers()
                && s.compiler_locks
                    .iter()
                    .any(|lock| lock.occupied && counts(lock.toolchain)),
            "compiler-lock",
        ),
        (
            kind.checks_compilers()
                && need > 0
                && s.mem_available_mb
                    .saturating_sub(table.values().fold(0u64, |sum, mb| sum.saturating_add(*mb)))
                    < need,
            "reserved",
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
    let holders: Vec<_> = pool_slots
        .iter()
        .filter(|slot| slot.occupied)
        .map(|slot| match &slot.holder {
            Some(h) => format!(
                "{prefix}-{} pid={} kind={} cmd={} since={}",
                slot.index,
                h.pid,
                h.kind,
                h.command.join(" "),
                h.started_at
            ),
            None => format!("{prefix}-{} unknown", slot.index),
        })
        .collect();
    let compilers: Vec<_> = s
        .compiling
        .iter()
        .map(|p| format!("pid={} name={} cwd={}", p.pid, p.name, p.cwd))
        .collect();
    let compiler_locks: Vec<_> = s
        .compiler_locks
        .iter()
        .filter(|lock| lock.occupied)
        .map(|lock| match &lock.holder {
            Some(h) => format!(
                "{}: pid={} kind={} cmd={} since={}",
                lock.toolchain.as_str(),
                h.pid,
                h.kind,
                h.command.join(" "),
                h.started_at
            ),
            None => format!("{}: unknown", lock.toolchain.as_str()),
        })
        .collect();
    let compiler_lock = if compiler_locks.is_empty() {
        "none".into()
    } else {
        compiler_locks.join("; ")
    };
    let message = format!(
        "gate denied ({}) load={:.2}/{:.2} {} {}={}/{} holders=[{}] compiler_lock=[{}] compiling=[{}]",
        failed.join(","),
        s.load_1m,
        cfg.max_load,
        memory_figures(s, cfg),
        if prefix == "slot" { "slots" } else { "other_slots" },
        holders.len(),
        total,
        holders.join("; "),
        compiler_lock,
        compilers.join("; ")
    );
    Some((message, failed))
}

fn warning(error: &io::Error) {
    let _ = writeln!(io::stderr(), "gate warning: {error}; allowing work");
}

#[derive(Clone, Serialize, Deserialize)]
struct GateLogRow {
    ts: u64,
    session: Option<String>,
    id: Option<String>,
    sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed: Option<bool>,
    #[serde(flatten)]
    ctx: crate::gate_stats::DenyContext,
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short_hash(bytes: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn gate_log_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("CLAUDECAT_GATE_LOG") {
        return Ok(PathBuf::from(path));
    }
    if let Some(root) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(root).join("claudecat/gate-denies.jsonl"));
    }
    env::var_os("HOME")
        .map(|root| PathBuf::from(root).join(".local/state/claudecat/gate-denies.jsonl"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no gate log directory"))
}

fn read_log(path: &Path) -> io::Result<Vec<GateLogRow>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(bytes
        .split(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<GateLogRow>(line).ok())
        .collect())
}

pub(crate) fn compact_log(path: &Path, now: u64) -> io::Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let old_first = {
        use std::io::BufRead;
        let mut line = Vec::new();
        io::BufReader::new(File::open(path)?).read_until(b'\n', &mut line)?;
        serde_json::from_slice::<crate::gate_stats::LogTs>(&line)
            .ok()
            .is_some_and(|row| row.ts < now.saturating_sub(7 * 86400))
    };
    if metadata.len() <= 256 * 1024 && !old_first {
        return Ok(());
    }
    let bytes = fs::read(path)?;
    let mut kept = Vec::new();
    let mut size = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n').rev() {
        let Ok(row) = serde_json::from_slice::<crate::gate_stats::LogTs>(line) else {
            continue;
        };
        if row.ts < now.saturating_sub(7 * 86400) || row.ts > now {
            continue;
        }
        if size + line.len() > 128 * 1024 {
            break;
        }
        kept.push(line);
        size += line.len();
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no log parent"))?;
    for serial in 0..8 {
        let temp = dir.join(format!(
            ".gate-denies-{}-{now}-{serial}.tmp",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp);
        let Ok(mut file) = file else {
            if file
                .err()
                .is_some_and(|e| e.kind() == io::ErrorKind::AlreadyExists)
            {
                continue;
            }
            return Err(io::Error::other("cannot create gate log temp file"));
        };
        let result = (|| -> io::Result<()> {
            for line in kept.iter().rev() {
                file.write_all(line)?;
            }
            file.flush()?;
            fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "gate log temp files exist",
    ))
}

pub(crate) fn append_log<T: Serialize>(path: &Path, row: &T) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no log parent"))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir)?;
    let mut line = serde_json::to_vec(row)?;
    line.push(b'\n');
    if line.len() > 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "gate log row too long",
        ));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&line)
}

fn active_denies(rows: &[GateLogRow], now: u64, session: &str, id: &str) -> Vec<u64> {
    let mut denies = Vec::new();
    for row in rows {
        if row.session.as_deref() != Some(session) || row.id.as_deref() != Some(id) {
            continue;
        }
        if row.allowed == Some(true) {
            denies.clear();
        } else if row.reason.is_some() && row.ts <= now && row.ts >= now.saturating_sub(1800) {
            denies.push(row.ts);
        }
    }
    denies
}

fn record_deny(
    now: u64,
    session: Option<&str>,
    id: Option<&str>,
    sub: &str,
    reason: &str,
    ctx: crate::gate_stats::DenyContext,
) -> Option<(usize, u64)> {
    let path = gate_log_path().ok()?;
    compact_log(&path, now).ok()?;
    let rows = read_log(&path).ok()?;
    let prior = session
        .zip(id)
        .map(|(session, id)| active_denies(&rows, now, session, id));
    append_log(
        &path,
        &GateLogRow {
            ts: now,
            session: session.map(str::to_owned),
            id: id.map(str::to_owned),
            sub: sub.to_owned(),
            reason: Some(reason.to_owned()),
            allowed: None,
            ctx,
        },
    )
    .ok()?;
    prior.map(|denies| (denies.len() + 1, denies.first().copied().unwrap_or(now)))
}

fn record_allow(now: u64, session: Option<&str>, id: Option<&str>, sub: &str) -> io::Result<()> {
    let (Some(session), Some(id)) = (session, id) else {
        return Ok(());
    };
    let path = gate_log_path()?;
    compact_log(&path, now)?;
    let rows = read_log(&path)?;
    if active_denies(&rows, now, session, id).is_empty() {
        return Ok(());
    }
    append_log(
        &path,
        &GateLogRow {
            ts: now,
            session: Some(session.to_owned()),
            id: Some(id.to_owned()),
            sub: sub.to_owned(),
            reason: None,
            allowed: Some(true),
            ctx: Default::default(),
        },
    )
}

pub fn log(json: bool, days: u32) -> i32 {
    let result = (|| -> io::Result<serde_json::Value> {
        let path = gate_log_path()?;
        let now = unix_now();
        compact_log(&path, now)?;
        let rows = read_log(&path)?;
        let mut reasons = BTreeMap::<String, u64>::new();
        let mut subs = BTreeMap::<String, u64>::new();
        let mut runs = BTreeMap::<(String, String), (u64, u64)>::new();
        let mut distribution = [0_u64; 3];
        let mut durations = Vec::<u64>::new();
        for row in rows
            .into_iter()
            .filter(|row| row.ts >= now.saturating_sub(u64::from(days) * 86400) && row.ts <= now)
        {
            let key = row.session.zip(row.id);
            if row.allowed == Some(true) {
                if let Some((count, first)) = key.and_then(|key| runs.remove(&key)) {
                    distribution[(count.saturating_sub(1).min(2)) as usize] += 1;
                    durations.push(row.ts.saturating_sub(first));
                }
            } else if let Some(reason) = row.reason {
                *subs.entry(row.sub).or_default() += 1;
                for reason in reason.split(',').filter(|s| !s.is_empty()) {
                    *reasons.entry(reason.to_owned()).or_default() += 1;
                }
                if let Some(key) = key {
                    let run = runs.entry(key).or_insert((0, row.ts));
                    run.0 += 1;
                }
            }
        }
        for (count, _) in runs.values() {
            distribution[(count.saturating_sub(1).min(2)) as usize] += 1;
        }
        durations.sort_unstable();
        let percentile = |p: usize| -> Option<u64> {
            durations
                .get(((durations.len() * p).div_ceil(100)).saturating_sub(1))
                .copied()
        };
        Ok(
            serde_json::json!({"reasons": reasons, "subs": subs, "retries": {"1":distribution[0],"2":distribution[1],"3+":distribution[2]}, "seconds_to_allow": {"p50":percentile(50),"p95":percentile(95)}}),
        )
    })();
    match result {
        Ok(value) => {
            if json {
                println!("{value}");
            } else {
                println!("Deny reasons:");
                if let Some(map) = value["reasons"].as_object() {
                    for (key, count) in map {
                        println!("  {key}: {count}");
                    }
                }
                println!("Deny subcommands:");
                if let Some(map) = value["subs"].as_object() {
                    for (key, count) in map {
                        println!("  {key}: {count}");
                    }
                }
                println!(
                    "Retry runs: 1={} 2={} 3+={}",
                    value["retries"]["1"], value["retries"]["2"], value["retries"]["3+"]
                );
                println!(
                    "Seconds from first deny to allow: p50={} p95={}",
                    value["seconds_to_allow"]["p50"], value["seconds_to_allow"]["p95"]
                );
            }
            0
        }
        Err(e) => {
            eprintln!("gate log: {e}");
            1
        }
    }
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
    match config().and_then(|cfg| snapshot(&cfg, kind.checks_compilers()).map(|s| (cfg, s))) {
        Ok((cfg, s)) => match deny_reason(&s, &cfg, kind, kind, &[]) {
            Some((reason, _)) => {
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
    let Some((command, cwd, session_id)) = serde_json::from_str::<serde_json::Value>(&payload)
        .ok()
        .and_then(|v| {
            let command = v.pointer("/tool_input/command")?.as_str()?.to_owned();
            let cwd = v
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .or_else(|| env::current_dir().ok())?;
            let session_id = v
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            Some((command, cwd, session_id))
        })
    else {
        return 0;
    };
    let (classification, identity) = classify_with_cwd(&command, &cwd);
    let wanted = command_toolchains(&command);
    let Classification::Heavy(kind) = classification else {
        return 0;
    };
    let Ok((cfg, mut snap, procs)) = config().and_then(|cfg| {
        snapshot_with_processes(&cfg, true, kind.checks_compilers())
            .map(|(snap, procs)| (cfg, snap, procs))
    }) else {
        return 0;
    };
    let uid = fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX);
    let duplicate_reason = identity
        .as_ref()
        .and_then(|id| duplicate(id, &snap, &procs, uid));
    let session = session_id.as_deref().map(short_hash);
    // The snapshot above was taken without any lock (the Gradle CPU sample sleeps). Only the
    // check-then-record step is serialized, so a concurrent hook sees this one's admission.
    let record_admission =
        duplicate_reason.is_none() && kind.checks_compilers() && !wanted.is_empty();
    let guard = if record_admission {
        AdmissionGuard::acquire(&cfg.gate_dir).and_then(|guard| {
            snap.admissions =
                read_admissions(&cfg.gate_dir, unix_now(), cfg.reserve_warmup_secs).ok()?;
            Some(guard)
        })
    } else {
        None
    };
    let (decision, keys) = if let Some(reason) = duplicate_reason {
        (Some(reason), vec!["duplicate"])
    } else if let Some((reason, keys)) = deny_reason(&snap, &cfg, kind, kind, &wanted) {
        // Only slots, compilers and reservations have holders to wait for; load and memory deny
        // with holders=[].
        // "reserved" has a holder to wait for only while the table is non-empty; otherwise
        // MemAvailable alone is below the estimate.
        let has_holders = snap.slots.iter().any(|slot| slot.occupied)
            || snap.compiler_locks.iter().any(|lock| lock.occupied);
        let wait = if keys
            .iter()
            .any(|key| matches!(*key, "slots" | "compiler" | "compiler-lock"))
            || (keys.contains(&"reserved")
                && has_holders
                && !reserved(&snap, &cfg, unix_now()).is_empty())
        {
            "the holders above finish"
        } else if keys.contains(&"reserved") && !reserved(&snap, &cfg, unix_now()).is_empty() {
            "the reservation warm-up passes (reserved= above)"
        } else {
            "load and memory pressure drop (figures above)"
        };
        (Some(format!("{reason} Denied, not queued: nothing will run this command later. Retry it after {wait}; do not poll with sleep in the foreground.")), keys)
    } else {
        (None, Vec::new())
    };
    if let (None, Some(guard)) = (&decision, &guard) {
        let _ = guard.admit(
            Admission {
                ts: unix_now(),
                session: session.clone(),
                toolchains: wanted.clone(),
            },
            unix_now(),
            cfg.reserve_warmup_secs,
        );
    }
    drop(guard);
    let id = identity
        .as_ref()
        .and_then(|id| serde_json::to_vec(&(&id.argv, &id.dir, &id.target)).ok())
        .map(|bytes| short_hash(&bytes));
    let sub = identity
        .as_ref()
        .map(|id| cargo_subcommand(&id.argv))
        .unwrap_or_else(|| "gradle".into());
    let now = unix_now();
    if let Some(reason) = decision {
        let retry = record_deny(
            now,
            session.as_deref(),
            id.as_deref(),
            &sub,
            &keys.join(","),
            crate::gate_stats::DenyContext::new(
                snap.mem_available_mb,
                snap.mem_pressure_some_avg10,
                snap.load_1m,
            ),
        );
        let reason = if let Some((count, first)) = retry.filter(|(count, _)| *count >= 2) {
            let suffix = if count >= 3 {
                " Stop retrying it: do other work or tell the user what is blocking it."
            } else {
                ""
            };
            format!("retry {count} of this command in this session; first denied {}s ago, still blocked. {reason}{suffix}", now.saturating_sub(first))
        } else {
            reason
        };
        // One line, always: holder commands and compiler cwds are arbitrary bytes and may hold newlines.
        let reason = reason.replace(['\n', '\r'], " ");
        let _ = writeln!(io::stdout(), "{reason}");
        75
    } else {
        if identity.is_some() && session.is_some() {
            let _ = record_allow(now, session.as_deref(), id.as_deref(), &sub);
        }
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
    let result = config().and_then(|cfg| snapshot(&cfg, kind.checks_compilers()).map(|s| (cfg, s)));
    let (cfg, snap) = match result {
        Ok(pair) => pair,
        Err(e) => {
            warning(&e);
            return execute(command);
        }
    };
    let wanted = argv_toolchains(command);
    // A JS or Python test runner is no compiler: it needs a slot and the load/memory checks, but
    // neither toolchain lock nor the compiler checks, so it never queues behind (or blocks) a
    // cargo or Gradle build.
    let check_kind = if runs_no_compiler(command) {
        Kind::Other
    } else {
        kind
    };
    if let Some((reason, _)) = deny_reason(&snap, &cfg, check_kind, kind, &wanted) {
        let _ = writeln!(io::stderr(), "{reason}");
        return 75;
    }
    // Sorted, so two runs wanting both toolchains take the locks in the same order. An unknown
    // toolchain holds them all, as the single lock used to.
    let held: &[Toolchain] = if !wanted.is_empty() {
        &wanted
    } else if check_kind == Kind::Other {
        &[]
    } else {
        &TOOLCHAINS
    };
    let (prefix, total) = pool(&cfg, kind);
    for index in 0..total {
        let path = cfg.gate_dir.join(format!("{prefix}-{index}.lock"));
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
                let mut compilers = Vec::new();
                if kind.checks_compilers() {
                    for toolchain in held {
                        let mut lock = match lock_file(&cfg.gate_dir.join(toolchain.lock_name())) {
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
                                compilers.push(lock);
                            }
                            Err(std::fs::TryLockError::WouldBlock) => {
                                // Drop the slot and the locks taken so far before answering 75:
                                // the caller queued, so it must not keep a place it will not use.
                                // Only a race reaches this arm -- someone took the lock after the
                                // snapshot's pre-check -- so no test drives it; keep the drop
                                // when touching this.
                                drop(compilers);
                                drop(file);
                                let mut data = String::new();
                                let _ = lock.read_to_string(&mut data);
                                let _ = writeln!(
                                    io::stderr(),
                                    "gate denied (compiler-lock) toolchain={} holder=[{}] load={:.2} {}",
                                    toolchain.as_str(),
                                    data.trim(),
                                    snap.load_1m,
                                    memory_figures(&snap, &cfg)
                                );
                                return 75;
                            }
                            Err(e) => {
                                warning(&io::Error::other(e));
                                return execute(command);
                            }
                        }
                    }
                }
                let exit = crate::gate_stats::run_measured(kind.as_str(), command, &cfg.proc_root);
                drop(compilers);
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
        "gate denied (slots) load={:.2} {} {}={}/{} holders=[{}] compiling=[{}]",
        snap.load_1m,
        memory_figures(&snap, &cfg),
        if prefix == "slot" {
            "slots"
        } else {
            "other_slots"
        },
        total,
        total,
        snap.pool(kind)
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
    let snap = match config().and_then(|cfg| snapshot(&cfg, true)) {
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
            "load 1m: {:.2}; PSI memory some avg10: {}; MemAvailable: {} MB; swap used: {} MB",
            snap.load_1m,
            snap.mem_pressure_some_avg10
                .map_or_else(|| "n/a".into(), |psi| format!("{psi:.2}")),
            snap.mem_available_mb,
            snap.swap_used_mb
        );
        let l = &snap.limits;
        let _ = writeln!(
            io::stdout(),
            "limits: cpus={} MemTotal={}; slots={} [{}]; other_slots={} [{}]; max_load={:.2} [{}]",
            l.cpus.map_or_else(|| "?".into(), |c| c.to_string()),
            l.mem_total_mb
                .map_or_else(|| "?".into(), |m| format!("{m}MB")),
            l.slots,
            l.slots_basis,
            l.other_slots,
            l.other_slots_basis,
            l.max_load,
            l.max_load_basis
        );
        let pools = snap
            .slots
            .iter()
            .map(|slot| ("slot", slot))
            .chain(snap.other_slots.iter().map(|slot| ("other", slot)));
        for (prefix, slot) in pools {
            if prefix == "other" && !slot.occupied {
                continue;
            }
            match &slot.holder {
                Some(h) => {
                    let _ = writeln!(
                        io::stdout(),
                        "{prefix}-{}: pid={} kind={} cmd={} since={}",
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
                        "{prefix}-{}: occupied (unknown holder)",
                        slot.index
                    );
                }
                None => {
                    let _ = writeln!(io::stdout(), "slot-{}: empty", slot.index);
                }
            }
        }
        for lock in snap.compiler_locks.iter().filter(|lock| lock.occupied) {
            let name = lock.toolchain.as_str();
            match &lock.holder {
                Some(h) => {
                    let _ = writeln!(
                        io::stdout(),
                        "compiler lock ({name}): pid={} kind={} cmd={} since={}",
                        h.pid,
                        h.kind,
                        h.command.join(" "),
                        h.started_at
                    );
                }
                None => {
                    let _ = writeln!(
                        io::stdout(),
                        "compiler lock ({name}): occupied (unknown holder)"
                    );
                }
            }
        }
        if !snap.compiler_lock.occupied {
            let _ = writeln!(io::stdout(), "compiler lock: empty");
        }
        if let Ok(cfg) = config() {
            let _ = writeln!(
                io::stdout(),
                "{}",
                reserved_figure(&reserved(&snap, &cfg, unix_now()))
            );
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
            mem_pressure_some_avg10: None,
            slots: Vec::new(),
            other_slots: Vec::new(),
            limits: Limits {
                cpus: None,
                mem_total_mb: None,
                slots: 0,
                slots_basis: String::new(),
                other_slots: 0,
                other_slots_basis: String::new(),
                max_load: 0.0,
                max_load_basis: String::new(),
            },
            compiler_lock: CompilerLock {
                holder: None,
                occupied: false,
            },
            compiler_locks: Vec::new(),
            compiling: Vec::new(),
            orphan_brokers: Vec::new(),
            admissions: Vec::new(),
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
