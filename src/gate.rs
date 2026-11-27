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
// Load average allowed per CPU: the fleet's long-standing 6 was tuned on the NUC, whose WSL
// exposes 6 CPUs, so 1.0 per CPU keeps it there.
const LOAD_PER_CPU: f64 = 1.0;
// Estimated peak memory of a fresh build, reserved only while its holder is still warming up.
// The Gradle figure is per test fork: a Gradle test run with N forks reserves N times this,
// because backend tests with 2 parallel forks peaked at ~5.4 GB on mgpu (2026-11-23), so one
// flat figure under-reserved them.
const DEFAULT_RESERVE_GRADLE_MB: u64 = 3072;
const DEFAULT_RESERVE_CARGO_MB: u64 = 1536;
const DEFAULT_RESERVE_WARMUP_SECS: u64 = 60;
// Estimated load (busy cores) reserved over the same window, and decaying with it. Cargo's scales
// with the machine: 2.0 on a 6-CPU NUC, 4.0 on 16 cores. A Gradle test run takes the Gradle figure
// per fork. Provisional: `gate usage` measured CPU seconds per run, not cores.
const DEFAULT_RESERVE_CARGO_LOAD: f64 = 2.0;
const CARGO_LOAD_PER_CPU: f64 = 0.25;
const DEFAULT_RESERVE_GRADLE_LOAD: f64 = 1.0;
// Memory reserved for a run of kind `other` that names no toolchain. Dispatch sends many at once
// and their RSS reaches several GB, so each holds this back while it warms up.
const DEFAULT_RESERVE_OTHER_MB: u64 = 1024;
// How long `gate run` waits for `admission.lock`; a hook waits ADMISSION_LOCK_BUDGET.
const RUN_LOCK_BUDGET: Duration = Duration::from_secs(2);

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

/// The compiler family a command drives: what is estimated and reserved for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Toolchain {
    Cargo,
    Gradle,
}

impl Toolchain {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Gradle => "gradle",
        }
    }

    /// Estimated peak memory of one run, in MB. A Gradle test run multiplies this by its forks.
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

/// The forks a Gradle invocation states: the last of its `-PtestForks=N` and `-P testForks=N`
/// arguments, when it parses as an integer of at least 1. Anything else is left to the default.
fn stated_test_forks(args: &[String]) -> Option<u64> {
    args.iter()
        .enumerate()
        .filter_map(|(i, arg)| {
            if arg == "-P" {
                return args
                    .get(i + 1)
                    .and_then(|next| next.strip_prefix("testForks="));
            }
            arg.strip_prefix("-PtestForks=")
        })
        .next_back()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|forks| *forks >= 1)
}

/// The build's own default when a Gradle test run states no fork count: 2 on a machine with at
/// least 8192 MB and 4 CPUs, else 1 (also when either figure is unknown).
fn default_test_forks(cfg: &Config) -> u64 {
    match (cfg.limits.mem_total_mb, cfg.limits.cpus) {
        (Some(mb), Some(cpus)) if mb >= 8192 && cpus >= 4 => 2,
        _ => 1,
    }
}

/// Forks of a Gradle segment's test run: what it states, else the build's default. A segment
/// that names no test task (`test`, `:backend:test`) is no test run and keeps 1.
fn gradle_forks(words: &[String], cfg: &Config) -> u64 {
    let args = &words[command_index(words) + 1..];
    if !names_test_task(args) {
        return 1;
    }
    stated_test_forks(args).unwrap_or_else(|| default_test_forks(cfg))
}

/// Gradle options whose value is the next word. That word is never a task: `-x test` excludes
/// the test task, and a `--tests test` filter or a `-p test` directory names none.
const GRADLE_VALUE_OPTIONS: [&str; 22] = [
    "-x",
    "--exclude-task",
    "--tests",
    "-p",
    "--project-dir",
    "-P",
    "--project-prop",
    "-D",
    "--system-prop",
    "-g",
    "--gradle-user-home",
    "-I",
    "--init-script",
    "-b",
    "--build-file",
    "-c",
    "--settings-file",
    "--console",
    "--warning-mode",
    "--max-workers",
    "--include-build",
    "--project-cache-dir",
];

/// Whether a Gradle argument list runs a test task (`test`, `:backend:test`), skipping option
/// values so `./gradlew build -x test` is not taken for a test run.
fn names_test_task(args: &[String]) -> bool {
    let mut skip_value = false;
    for arg in args {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if GRADLE_VALUE_OPTIONS.contains(&arg.as_str()) {
            skip_value = true;
            continue;
        }
        if arg == "test" || arg.ends_with(":test") {
            return true;
        }
    }
    false
}

/// What one heavy segment drives: its toolchain and, for Gradle, its test forks.
fn segment_estimate(words: &[String], cfg: &Config) -> Option<(Toolchain, u64)> {
    let toolchain = segment_toolchain(words)?;
    let forks = if toolchain == Toolchain::Gradle {
        gradle_forks(words, cfg)
    } else {
        1
    };
    Some((toolchain, forks))
}

/// What a command drives and the memory reserved for it. `toolchains` is sorted and without
/// repeats; `gradle_forks` is the forks of its Gradle test runs, each taking the full Gradle
/// estimate. Cargo is never scaled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Estimate {
    toolchains: Vec<Toolchain>,
    gradle_forks: u64,
}

impl Estimate {
    /// Adds a segment: its toolchain once, and its forks when they are the command's most. The
    /// segments of one command run one after another, so the largest run sets the peak.
    fn add(&mut self, toolchain: Toolchain, forks: u64) {
        if !self.toolchains.contains(&toolchain) {
            self.toolchains.push(toolchain);
        }
        self.gradle_forks = self.gradle_forks.max(forks);
    }

    /// Estimated peak memory, in MB, of the command's run of `toolchain`.
    fn mb(&self, toolchain: Toolchain, cfg: &Config) -> u64 {
        let forks = if toolchain == Toolchain::Gradle {
            self.gradle_forks
        } else {
            1
        };
        toolchain.estimate_mb(cfg).saturating_mul(forks)
    }

    /// Estimated peak memory, in MB, of the whole command.
    fn total_mb(&self, cfg: &Config) -> u64 {
        self.toolchains
            .iter()
            .fold(0u64, |sum, t| sum.saturating_add(self.mb(*t, cfg)))
    }

    /// Adds the estimate of one run to `table`, as `reserved` sums up holders and admissions.
    fn reserve_into(&self, table: &mut BTreeMap<Toolchain, u64>, cfg: &Config) {
        for toolchain in &self.toolchains {
            let slot = table.entry(*toolchain).or_insert(0u64);
            *slot = slot.saturating_add(self.mb(*toolchain, cfg));
        }
    }

    /// Estimated load (busy cores) of the command's run of `toolchain`.
    fn toolchain_load(&self, toolchain: Toolchain, cfg: &Config) -> f64 {
        match toolchain {
            Toolchain::Cargo => cfg.reserve_cargo_load,
            #[allow(clippy::cast_precision_loss)]
            Toolchain::Gradle => cfg.reserve_gradle_load * self.gradle_forks as f64,
        }
    }

    /// Estimated peak load of the whole command. Its segments run one after another, so the
    /// largest sets it.
    fn load(&self, cfg: &Config) -> f64 {
        self.toolchains
            .iter()
            .fold(0.0, |peak, t| peak.max(self.toolchain_load(*t, cfg)))
    }
}

/// Estimate of a shell command: every heavy segment, sorted; empty when none is recognised.
fn command_estimate(command: &str, cfg: &Config) -> Estimate {
    let mut estimate = Estimate::default();
    let Some(tokens) = lex(command) else {
        return estimate;
    };
    let mut segment = Vec::new();
    for token in tokens.iter().chain(std::iter::once(&ShellToken::Separator)) {
        match token {
            ShellToken::Word(word) => segment.push(word.clone()),
            ShellToken::Separator => {
                if let Some((toolchain, forks)) = segment_estimate(&segment, cfg) {
                    estimate.add(toolchain, forks);
                }
                segment.clear();
            }
        }
    }
    estimate.toolchains.sort();
    estimate
}

/// Estimate of an argv (a `gate run` command or a holder's), looking through `sh -c "..."`.
fn argv_estimate(argv: &[String], cfg: &Config) -> Estimate {
    let mut estimate = Estimate::default();
    if let Some((toolchain, forks)) = segment_estimate(argv, cfg) {
        estimate.add(toolchain, forks);
        return estimate;
    }
    let i = command_index(argv);
    if argv
        .get(i)
        .is_some_and(|c| matches!(basename(c), "sh" | "bash" | "zsh"))
    {
        if let Some(script) = argv[i + 1..].iter().skip_while(|a| *a != "-c").nth(1) {
            return command_estimate(script, cfg);
        }
    }
    estimate
}

/// Test runners that are known not to drive cargo or Gradle: they are checked on the `other`
/// row. Anything else unrecognised (`make`, `just`, a shell script) may drive one.
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

    /// Whether `compiling` is listed for this kind. Information only: a compiler decides nothing.
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
    /// The `gate run` runs holding a lock file in the gate directory, the new registrations and
    /// the slot and compiler locks of runs started before the upgrade alike.
    pub holders: Vec<Holder>,
    pub limits: Limits,
    /// Compilers of our own uid, for information: they decide nothing.
    pub compiling: Vec<Process>,
    pub orphan_brokers: Vec<Broker>,
    /// Recent hook admissions (see `Admission`); counted by `reserved` like warming-up holders.
    #[serde(skip)]
    admissions: Vec<Admission>,
    /// Whose lock files count, and where they are: for `refresh`.
    #[serde(skip)]
    uid: u32,
    #[serde(skip)]
    gate_dir: PathBuf,
}

/// The machine's resources and the limits derived from them, with where each limit came from.
#[derive(Serialize, Clone, Debug)]
pub struct Limits {
    pub cpus: Option<usize>,
    pub mem_total_mb: Option<u64>,
    pub max_load: f64,
    pub max_load_basis: String,
}

struct Config {
    proc_root: PathBuf,
    gate_dir: PathBuf,
    max_load: f64,
    max_mem_pressure: f64,
    min_available_mb: u64,
    limits: Limits,
    reserve_gradle_mb: u64,
    reserve_cargo_mb: u64,
    reserve_warmup_secs: u64,
    reserve_cargo_load: f64,
    reserve_gradle_load: f64,
    reserve_other_mb: u64,
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

/// Our own uid. Linux reads the real /proc/self (CLAUDECAT_PROC_ROOT may be a fixture); macOS has no
/// /proc, so it asks the kernel.
pub fn own_uid() -> io::Result<u32> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::host_macos::uid())
    }
    #[cfg(not(target_os = "macos"))]
    {
        fs::metadata("/proc/self").map(|m| m.uid())
    }
}

/// Prefix of the error `metrics` returns when load or memory cannot be read. The gate denies on it
/// instead of allowing: with no figures it cannot tell an idle machine from one that is swapping.
const UNREADABLE: &str = "machine resources unreadable";

pub(crate) fn own_uid_for_stats() -> Option<u32> {
    own_uid().ok()
}

fn unreadable(e: &io::Error) -> bool {
    e.to_string().starts_with(UNREADABLE)
}

fn unreadable_reason(e: &io::Error) -> String {
    format!("gate denied (resources) {e}; refusing heavy work until load and memory can be read")
}

/// Logical CPUs listed in `cpuinfo`; None when the proc root has no such file.
fn cpus(root: &Path) -> Option<usize> {
    #[cfg(target_os = "macos")]
    if crate::host_macos::native(root) {
        return crate::host_macos::cpus();
    }
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
    #[cfg(target_os = "macos")]
    if crate::host_macos::native(root) {
        return crate::host_macos::mem_total_mb();
    }
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

/// Load limit: `LOAD_PER_CPU` per CPU. Environment variables win. There is no slot limit: the
/// gate judges load (with what is reserved for warming-up runs) and memory.
fn limits(root: &Path) -> Limits {
    let cpus = cpus(root);
    let mem_total_mb = mem_total_mb(root);
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
        max_load,
        max_load_basis,
    }
}

/// Load reserved for a cargo run: `CLAUDECAT_RESERVE_CARGO_LOAD`, else at least 2.0 and a quarter
/// of the CPUs.
fn reserve_cargo_load(cpus: Option<usize>) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let default = cpus.map_or(DEFAULT_RESERVE_CARGO_LOAD, |c| {
        DEFAULT_RESERVE_CARGO_LOAD.max(c as f64 * CARGO_LOAD_PER_CPU)
    });
    positive_f64("CLAUDECAT_RESERVE_CARGO_LOAD", default)
}

fn config() -> io::Result<Config> {
    let proc_root = env::var_os("CLAUDECAT_PROC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc"));
    let reserve_gradle_mb = positive_u64("CLAUDECAT_RESERVE_GRADLE_MB", DEFAULT_RESERVE_GRADLE_MB);
    let reserve_cargo_mb = positive_u64("CLAUDECAT_RESERVE_CARGO_MB", DEFAULT_RESERVE_CARGO_MB);
    let limits = limits(&proc_root);
    let gate_dir = if let Some(dir) = env::var_os("CLAUDECAT_GATE_DIR") {
        PathBuf::from(dir)
    } else if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("claudecat/gate")
    } else {
        // Use the real proc mount, as snapshot() does: CLAUDECAT_PROC_ROOT may be a fixture.
        let uid = own_uid()?;
        PathBuf::from(format!("/tmp/claudecat-{uid}/gate"))
    };
    let reserve_cargo_load = reserve_cargo_load(limits.cpus);
    Ok(Config {
        proc_root,
        gate_dir,
        max_load: limits.max_load,
        max_mem_pressure: positive_f64("CLAUDECAT_MAX_MEM_PRESSURE", DEFAULT_MEM_PRESSURE),
        min_available_mb: positive_u64("CLAUDECAT_MIN_AVAILABLE_MB", DEFAULT_AVAILABLE_MB),
        limits,
        reserve_gradle_mb,
        reserve_cargo_mb,
        reserve_warmup_secs: positive_u64(
            "CLAUDECAT_RESERVE_WARMUP_SECS",
            DEFAULT_RESERVE_WARMUP_SECS,
        ),
        reserve_cargo_load,
        reserve_gradle_load: positive_f64(
            "CLAUDECAT_RESERVE_GRADLE_LOAD",
            DEFAULT_RESERVE_GRADLE_LOAD,
        ),
        reserve_other_mb: positive_u64("CLAUDECAT_RESERVE_OTHER_MB", DEFAULT_RESERVE_OTHER_MB),
    })
}

pub(crate) fn metrics(root: &Path) -> io::Result<(f64, u64, u64)> {
    read_metrics(root).map_err(|e| io::Error::other(format!("{UNREADABLE}: {e}")))
}

fn read_metrics(root: &Path) -> io::Result<(f64, u64, u64)> {
    #[cfg(target_os = "macos")]
    if crate::host_macos::native(root) {
        return crate::host_macos::metrics();
    }
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

/// What the runs still warming up hold back for the ones that follow.
#[derive(Debug, Default, Clone)]
struct Reservation {
    /// Memory per toolchain, in MB.
    memory: BTreeMap<Toolchain, u64>,
    /// Memory of `other` runs that name no toolchain, in MB.
    other_mb: u64,
    /// Load, decaying linearly over the warm-up window.
    load: f64,
    /// Seconds until the last of these runs is past the window.
    warmup_left: u64,
    /// Runs still inside the window, holders and admissions alike, whether they reserve anything
    /// or not.
    warming: usize,
}

impl Reservation {
    /// Memory held back in all.
    fn mb(&self) -> u64 {
        self.memory
            .values()
            .fold(self.other_mb, |sum, mb| sum.saturating_add(*mb))
    }
}

/// Adds what one run holds back `elapsed` seconds into the warm-up window: the estimate of its
/// command, and `reserve_other_mb` for a run of the `other` row that names no toolchain. The
/// memory is held in full for the whole window; the load decays linearly, roughly as load_1m picks
/// up the real load, so the two neither double count at the end of the window nor leave a gap
/// after it. Every run that reserves anything, memory or load, also lengthens `warmup_left`: a
/// denial it causes is over no sooner than that.
fn reserve_into(
    estimate: &Estimate,
    other_row: bool,
    elapsed: u64,
    cfg: &Config,
    out: &mut Reservation,
) {
    estimate.reserve_into(&mut out.memory, cfg);
    if other_row {
        out.other_mb = out.other_mb.saturating_add(cfg.reserve_other_mb);
    }
    if estimate.toolchains.is_empty() && !other_row {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let left = (cfg.reserve_warmup_secs - elapsed) as f64 / cfg.reserve_warmup_secs as f64;
    out.load += estimate.load(cfg) * left;
    out.warmup_left = out.warmup_left.max(cfg.reserve_warmup_secs - elapsed);
}

/// What the holders and admissions still inside the warm-up window reserve. A run older than the
/// window is already inside MemAvailable and load_1m, so reserving it again would count it twice.
/// One stamped in the future (clock step, hand edit) reserves nothing until the clock catches up.
fn reserved(s: &Snapshot, cfg: &Config, now: u64) -> Reservation {
    let warming = |started_at: u64| {
        started_at <= now.saturating_add(ADMISSION_CLOCK_SLACK_SECS)
            && now.saturating_sub(started_at) < cfg.reserve_warmup_secs
    };
    let mut out = Reservation::default();
    for holder in &s.holders {
        if warming(holder.started_at) {
            out.warming += 1;
            // The run was checked on the `other` row -- declared `other`, or a test runner that
            // drives no compiler whatever kind it declared (`gate run --kind test -- python3 …`) --
            // so it is memory it holds back, like any other run of that row.
            let other_row = holder.kind == "other" || runs_no_compiler(&holder.command);
            let estimate = argv_estimate(&holder.command, cfg);
            reserve_into(
                &estimate,
                other_row,
                now.saturating_sub(holder.started_at),
                cfg,
                &mut out,
            );
        }
    }
    for admission in &s.admissions {
        if warming(admission.ts) {
            out.warming += 1;
            // Only a command that names a toolchain is admitted, so a row never needs `other_mb`.
            reserve_into(
                &admission.estimate(),
                false,
                now.saturating_sub(admission.ts),
                cfg,
                &mut out,
            );
        }
    }
    out
}

fn one() -> u64 {
    1
}

fn is_one(n: &u64) -> bool {
    *n == 1
}

/// A denial of `gate check` or `gate run` goes into the deny log like a hook's, so that the log
/// shows every path that was refused. The row names neither a session nor a command, and nothing
/// a dispatch sends is recorded: only the path, its kind and the figures.
fn record_path_deny(path: &str, kind: Kind, reasons: &str, ctx: crate::gate_stats::DenyContext) {
    let _ = record_deny(
        unix_now(),
        None,
        None,
        &format!("{path}:{}", kind.as_str()),
        reasons,
        ctx,
    );
}

/// A command the PreToolUse hook allowed. Claude runs it directly, so it registers no holder;
/// until the warm-up window passes the row stands in for one. No command text is stored.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Admission {
    ts: u64,
    session: Option<String>,
    toolchains: Vec<Toolchain>,
    /// Forks of a Gradle test run. A row without the field (written before it existed) is one
    /// fork, and it is written only when it differs, so such rows stay in the old shape.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    gradle_forks: u64,
}

impl Admission {
    fn estimate(&self) -> Estimate {
        Estimate {
            toolchains: self.toolchains.clone(),
            gradle_forks: self.gradle_forks,
        }
    }
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
            // A hand-edited 0 would reserve nothing where one fork is the least a run takes.
            a.gradle_forks = a.gradle_forks.max(1);
            a
        })
        .collect())
}

/// Exclusive hold on `admission.lock`: the one critical section of the hook and of `gate run`,
/// where each re-reads the registry, decides and records.
struct AdmissionGuard {
    dir: PathBuf,
    _lock: File,
}

impl AdmissionGuard {
    /// None on timeout or any I/O error: a broken cache must never decide an outcome, so callers
    /// fail open and just skip recording. (Fresh rows read without the lock are still real
    /// reservations and keep counting; only the atomic check-then-record is lost.)
    fn acquire(dir: &Path, budget: Duration) -> Option<Self> {
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
                Err(std::fs::TryLockError::WouldBlock) if start.elapsed() < budget => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
    }

    /// What `critical` returns, with the lock held while it runs and released before this
    /// returns: nothing the caller does afterwards -- starting a child, printing -- is under it.
    fn under<T>(self, critical: impl FnOnce() -> T) -> T {
        critical()
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

fn reserved_figure(table: &Reservation) -> String {
    let mut rows: Vec<_> = table
        .memory
        .iter()
        .map(|(toolchain, mb)| format!("{}:{mb}MB", toolchain.as_str()))
        .collect();
    if table.other_mb > 0 {
        rows.push(format!("other:{}MB", table.other_mb));
    }
    if rows.is_empty() {
        return "reserved=none".into();
    }
    format!("reserved={}", rows.join(","))
}

/// Memory figures shared by every deny message, as key=value/limit.
fn memory_figures(s: &Snapshot, cfg: &Config, table: &Reservation) -> String {
    let psi = match s.mem_pressure_some_avg10 {
        Some(psi) => format!("{psi:.2}/{:.2}", cfg.max_mem_pressure),
        None => "n/a".into(),
    };
    format!(
        "psi_some_avg10={psi} avail={}MB/{}MB swap={}MB {}",
        s.mem_available_mb,
        cfg.min_available_mb,
        s.swap_used_mb,
        reserved_figure(table)
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

/// The registration of a `gate run`: its lock file, held until dropped.
struct Registration {
    /// The lock. Never read: it is kept for the lock it holds, released when dropped.
    _file: File,
    path: PathBuf,
}

impl Drop for Registration {
    /// Unlinks the file while its lock is still held (`_file` is dropped after this returns), so
    /// nobody finds a listed file unlocked and mistakes it for a stale one to clean up, or for a
    /// holder.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Sixteen hex digits for a registration's name: eight bytes of `/dev/urandom` or, without it, a
/// hash of the clock, our pid and thread. The pid alone could be reused by the next run.
fn random_hex() -> String {
    let mut bytes = [0u8; 8];
    if File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .is_err()
    {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let digest = Sha256::digest(format!(
            "{} {nanos} {:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        bytes.copy_from_slice(&digest[..8]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create the gate directory, private (0700) like every parent it has to create: its default path
/// under `/tmp` is predictable, and it holds lock files only our own uid may count.
fn ensure_gate_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(path)
}

/// Register a `gate run` for `holder` by the protocol of the load-only design: the file is created
/// under a name no enumerator lists and locked at once, the holder JSON is written and synced, and
/// only then does the file get its listed name. Whatever carries a listed name is therefore locked
/// and complete.
fn register(cfg: &Config, holder: &Holder) -> io::Result<Registration> {
    let random = random_hex();
    let temporary = cfg.gate_dir.join(format!(".reg-{random}"));
    let path = cfg
        .gate_dir
        .join(format!("run-{}-{random}.lock", holder.pid));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.lock()?;
    let written = write_holder(&mut file, holder).and_then(|()| file.sync_all());
    match written {
        Ok(()) => fs::rename(&temporary, &path).map(|()| Registration { _file: file, path }),
        Err(e) => {
            let _ = fs::remove_file(&temporary);
            Err(e)
        }
    }
}

/// Whether `path` still names the file `file` is open on: same device and inode.
fn same_file(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(opened), Ok(listed)) => opened.dev() == listed.dev() && opened.ino() == listed.ino(),
        _ => false,
    }
}

fn is_run_lock(name: &str) -> bool {
    name.starts_with("run-") && name.ends_with(".lock")
}

/// The `gate run` runs registered in `dir`: this build's `run-<pid>-<random>.lock`, and the
/// `slot-*.lock`, `other-*.lock` and `compiler-*.lock` of a run started before the upgrade (a pid
/// holding several of those counts once). Only files owned by `uid` count, and they are opened
/// read-only: enumerating must not create anything.
///
/// A listed file is locked for its whole life, so one that is not locked is stale: a `gate run`
/// killed by a signal runs no cleanup. Whoever finds it removes it -- under the lock just taken,
/// and only while the path still names that very file. A path naming another file was replaced
/// meanwhile and is not ours to remove.
///
/// What counts is the file that was opened, never the path: it is opened through no symlink, and
/// its owner and type are read from the open file itself.
fn holders(dir: &Path, uid: u32) -> io::Result<Vec<Holder>> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            is_run_lock(name)
                || (name.ends_with(".lock")
                    && ["slot-", "other-", "compiler-"]
                        .iter()
                        .any(|prefix| name.starts_with(prefix)))
        })
        .collect();
    names.sort_by_key(|name| (!is_run_lock(name), name.clone()));
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for name in names {
        let path = dir.join(&name);
        // A look, not a check: the file opened is checked below. It only keeps the gate from
        // opening -- and blocking on -- whatever is no regular file (the open of a FIFO waits for
        // its other end).
        let Ok(looked) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !looked.is_file() {
            continue;
        }
        // Through no symlink: in a directory that is not private, a planted link must not make
        // the gate lock some other file. And the owner is read from the file just opened, not
        // from the path before: the path could have named something else by then.
        let Ok(mut file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.uid() != uid {
            continue;
        }
        match file.try_lock() {
            Ok(()) => {
                if is_run_lock(&name) && same_file(&file, &path) {
                    let _ = fs::remove_file(&path);
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let mut data = String::new();
                let holder = file
                    .read_to_string(&mut data)
                    .ok()
                    .and_then(|_| serde_json::from_str(&data).ok())
                    .unwrap_or(Holder {
                        pid: 0,
                        kind: "unknown".into(),
                        command: Vec::new(),
                        started_at: 0,
                        cwd: None,
                        target: None,
                    });
                if seen.insert(holder.pid) {
                    found.push(holder);
                }
            }
            Err(_) => continue,
        }
    }
    Ok(found)
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
    #[cfg(target_os = "macos")]
    if crate::host_macos::native(root) {
        return Ok(native_processes(own_uid, read_environ));
    }
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

/// macOS: the same entries from libproc. Other users' argv, cwd and environment are not readable;
/// those fields stay empty, as an unreadable /proc entry leaves them on Linux.
#[cfg(target_os = "macos")]
fn native_processes(own_uid: u32, read_environ: bool) -> Vec<ProcEntry> {
    use crate::host_macos as mac;
    let mut found: Vec<ProcEntry> = mac::all_pids()
        .into_iter()
        .filter_map(|pid| {
            let info = mac::proc_info(pid)?;
            let (cmdline, env) = mac::args_env(pid).unwrap_or_default();
            let environ = (read_environ
                && info.uid == own_uid
                && !env.is_empty()
                && cmdline.first().is_some_and(|s| basename(s) == "cargo"))
            .then_some(env);
            let environ_target = environ.as_ref().and_then(|bytes| {
                bytes.split(|b| *b == 0).find_map(|item| {
                    item.strip_prefix(b"CARGO_TARGET_DIR=")
                        .map(|v| String::from_utf8_lossy(v).into_owned())
                })
            });
            Some(ProcEntry {
                uid: info.uid,
                pid,
                name: info.name,
                cmdline,
                cwd: mac::cwd(pid).unwrap_or_else(|| "?".into()),
                ppid: (info.ppid > 0).then_some(info.ppid),
                environ_target,
                environ_readable: environ.is_some(),
            })
        })
        .collect();
    found.sort_by_key(|p| p.pid);
    found
}

/// Gradle only counts while a test or compile worker JVM is running: its main class is an argument
/// ending in `.GradleWorkerMain`. The daemon is deliberately not counted -- it stays resident for
/// hours after a build (three by default) and is idle most of that time, so counting it refused
/// every build/test until it timed out. A gated Gradle build is covered by what its holder reserves instead.
/// The JVM requirement and the dotted suffix keep `rg …GradleWorkerMain` and `tail -f` on a log --
/// mentions, not compilers -- from blocking the gate. Workers outlive the build too (issue #1: an
/// idle one denied every compile for three hours), so a worker is only a candidate; `compiling`
/// counts it only if its CPU ticks grew across a short sample.
fn is_gradle(p: &ProcEntry) -> bool {
    p.name == "java" && p.cmdline.iter().any(|a| a.ends_with(".GradleWorkerMain"))
}

fn cpu_sample(root: &Path, pid: u32) -> Option<(u64, u64)> {
    #[cfg(target_os = "macos")]
    if crate::host_macos::native(root) {
        return crate::host_macos::cpu_sample(pid);
    }
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
    ensure_gate_dir(&cfg.gate_dir)?;
    // Our own uid, not the gate directory's owner: the directory can be shared with a group and
    // must not decide whose compilers count. CLAUDECAT_PROC_ROOT is the fixtures' fake proc, so
    // read the real /proc/self; fall back to the directory owner only if it is unreadable.
    let uid = own_uid().or_else(|_| fs::metadata(&cfg.gate_dir).map(|m| m.uid()))?;
    let procs = processes(&cfg.proc_root, uid, read_environ)?;
    let (load_1m, swap_used_mb, mem_available_mb) = metrics(&cfg.proc_root)?;
    let snap = Snapshot {
        load_1m,
        swap_used_mb,
        mem_available_mb,
        mem_pressure_some_avg10: mem_pressure(&cfg.proc_root),
        holders: holders(&cfg.gate_dir, uid)?,
        limits: cfg.limits.clone(),
        compiling: if check_compilers {
            compiling(&procs, uid, &cfg.proc_root)
        } else {
            Vec::new()
        },
        orphan_brokers: brokers(&procs),
        admissions: read_admissions(&cfg.gate_dir, unix_now(), cfg.reserve_warmup_secs)
            .unwrap_or_default(),
        uid,
        gate_dir: cfg.gate_dir.clone(),
    };
    Ok((snap, procs))
}

fn duplicate(wanted: &Identity, snap: &Snapshot, procs: &[ProcEntry], uid: u32) -> Option<String> {
    let mut canonical_dir = None;
    let holders = snap.holders.iter();
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
    /// Re-read the registry under the admission lock: the holders and admissions were read
    /// without it.
    fn refresh(&mut self, cfg: &Config) -> io::Result<()> {
        self.holders = holders(&self.gate_dir, self.uid)?;
        self.admissions = read_admissions(&self.gate_dir, unix_now(), cfg.reserve_warmup_secs)?;
        Ok(())
    }
}

/// Why a command may not start: its message, the reasons in it, and the figures it was decided on.
struct Denial {
    /// The message, one line, for whoever asked.
    message: String,
    /// The reasons in the message, in its order.
    reasons: Vec<&'static str>,
    /// What the runs still warming up held back, and for how much longer.
    table: Reservation,
    /// The load the command asks for on the row that decided: none on the `other` row.
    need_load: f64,
}

impl Denial {
    /// The system figures of this denial, for the deny log.
    fn context(&self, s: &Snapshot) -> crate::gate_stats::DenyContext {
        crate::gate_stats::DenyContext::new(
            s.mem_available_mb,
            s.mem_pressure_some_avg10,
            s.load_1m,
        )
        // The figures of the message: the load reserved on the machine, and what this command
        // asked for on its row.
        .with_loads(self.table.load, self.need_load)
    }
}

/// Why a command of `kind`, estimated as `wanted`, may not start, on the reservations of this
/// moment.
fn deny_reason(s: &Snapshot, cfg: &Config, kind: Kind, wanted: &Estimate) -> Option<Denial> {
    deny_on(s, cfg, kind, wanted, reserved(s, cfg, unix_now()))
}

/// The same on reservations already summed up, so that figures reported beside the reasons come
/// from the very table the reasons were read off. The build row counts the measured load together
/// with the load reserved for runs still warming up and the command's own. `other` keeps to the
/// measured load (external callers see no change there) and instead must find `reserve_other_mb`
/// beyond the reserved memory.
fn deny_on(
    s: &Snapshot,
    cfg: &Config,
    kind: Kind,
    wanted: &Estimate,
    table: Reservation,
) -> Option<Denial> {
    let other = kind == Kind::Other;
    let (need_mb, need_load, reserved_load) = if other {
        (cfg.reserve_other_mb, 0.0, 0.0)
    } else {
        (wanted.total_mb(cfg), wanted.load(cfg), table.load)
    };
    let reasons = [
        (s.load_1m + reserved_load + need_load > cfg.max_load, "load"),
        (
            s.mem_pressure_some_avg10
                .is_some_and(|psi| psi > cfg.max_mem_pressure)
                || s.mem_available_mb < cfg.min_available_mb,
            "memory",
        ),
        (
            need_mb > 0 && s.mem_available_mb.saturating_sub(table.mb()) < need_mb,
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
    let holders: Vec<_> = s
        .holders
        .iter()
        .map(|h| match h.pid {
            0 => "unknown".to_owned(),
            pid => format!(
                "pid={pid} kind={} cmd={} since={}",
                h.kind,
                h.command.join(" "),
                h.started_at
            ),
        })
        .collect();
    let compilers: Vec<_> = s
        .compiling
        .iter()
        .map(|p| format!("pid={} name={} cwd={}", p.pid, p.name, p.cwd))
        .collect();
    let message = format!(
        "gate denied ({}) load={:.2}/{:.2} reserved_load={:.2} need_load={:.2} {} holders=[{}] compiling=[{}]",
        failed.join(","),
        s.load_1m,
        cfg.max_load,
        table.load,
        need_load,
        memory_figures(s, cfg, &table),
        holders.join("; "),
        compilers.join("; ")
    );
    Some(Denial {
        message,
        reasons: failed,
        table,
        need_load,
    })
}

fn two_decimals(n: f64) -> f64 {
    (n * 100.0).round() / 100.0
}

/// What the machine has left, in figures every machine reports the same way so that a coordinator
/// can compare them. All of it is read off one snapshot and one sum of the reservations, so the
/// figures agree with each other and with what the gate would decide.
#[derive(Serialize)]
struct Capacity {
    cpus: Option<usize>,
    mem_total_mb: Option<u64>,
    max_load: f64,
    load_1m: f64,
    /// Estimated load of the runs still warming up, decayed to now.
    reserved_load: f64,
    /// `max_load - load_1m - reserved_load`: negative once a build would be denied on load.
    load_headroom: f64,
    mem_available_mb: u64,
    /// All the memory reserved, the `other` row's included.
    reserved_mb: u64,
    /// `mem_available_mb - reserved_mb`, never below 0.
    mem_headroom_mb: u64,
    /// PSI `some avg10`; None where the kernel has no PSI.
    mem_pressure: Option<f64>,
    /// Why a build or test run that needs nothing of its own would be denied right now. Empty when
    /// it would start: a coordinator that sees headroom here need not be told otherwise.
    blocked: Vec<&'static str>,
    /// Runs still warming up, holders and admissions alike.
    warming: usize,
}

fn capacity(s: &Snapshot, cfg: &Config, table: Reservation) -> Capacity {
    let (reserved_mb, reserved_load, warming) = (table.mb(), table.load, table.warming);
    // The gate's own reasons, for a run that asks for nothing: with nothing needed, memory already
    // reserved denies nobody, so only load and memory pressure can be in it.
    let blocked = deny_on(s, cfg, Kind::Build, &Estimate::default(), table)
        .map_or_else(Vec::new, |denial| denial.reasons);
    Capacity {
        cpus: cfg.limits.cpus,
        mem_total_mb: cfg.limits.mem_total_mb,
        max_load: cfg.max_load,
        load_1m: s.load_1m,
        reserved_load: two_decimals(reserved_load),
        load_headroom: two_decimals(cfg.max_load - s.load_1m - reserved_load),
        mem_available_mb: s.mem_available_mb,
        reserved_mb,
        mem_headroom_mb: s.mem_available_mb.saturating_sub(reserved_mb),
        mem_pressure: s.mem_pressure_some_avg10,
        blocked,
        warming,
    }
}

/// The `status --json` document, which `fleet push` uploads as well: the snapshot, and the
/// capacity read from it.
#[derive(Serialize)]
struct StatusDocument {
    #[serde(flatten)]
    snapshot: Snapshot,
    capacity: Capacity,
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

pub fn unix_now() -> u64 {
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
    crate::fleet_cmd::maybe_push_background();
    match config().and_then(|cfg| snapshot(&cfg, kind.checks_compilers()).map(|s| (cfg, s))) {
        Ok((cfg, s)) => match deny_reason(&s, &cfg, kind, &Estimate::default()) {
            Some(denial) => {
                let _ = writeln!(io::stderr(), "{}", denial.message);
                record_path_deny("check", kind, &denial.reasons.join(","), denial.context(&s));
                75
            }
            None => 0,
        },
        Err(e) if unreadable(&e) => {
            let _ = writeln!(io::stderr(), "{}", unreadable_reason(&e));
            record_path_deny("check", kind, "resources", Default::default());
            75
        }
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
    let Classification::Heavy(kind) = classification else {
        return 0;
    };
    let (cfg, mut snap, procs) = match config().and_then(|cfg| {
        snapshot_with_processes(&cfg, true, kind.checks_compilers())
            .map(|(snap, procs)| (cfg, snap, procs))
    }) {
        Ok(t) => t,
        Err(e) if unreadable(&e) => {
            let _ = writeln!(
                io::stdout(),
                "{}",
                unreadable_reason(&e).replace(['\n', '\r'], " ")
            );
            return 75;
        }
        Err(_) => return 0,
    };
    // After `config`: a Gradle test run that states no fork count takes the build's default,
    // which depends on this machine's memory and CPUs.
    let wanted = command_estimate(&command, &cfg);
    let uid = own_uid().unwrap_or(u32::MAX);
    let duplicate_reason = identity
        .as_ref()
        .and_then(|id| duplicate(id, &snap, &procs, uid));
    let session = session_id.as_deref().map(short_hash);
    // The snapshot above was taken without any lock (the Gradle CPU sample sleeps). Only the
    // check-then-record step is serialized, under the lock `gate run` shares, so whatever is
    // registered or admitted meanwhile is read here and this hook's admission is read next.
    let record_admission = duplicate_reason.is_none() && !wanted.toolchains.is_empty();
    let guard = if record_admission {
        AdmissionGuard::acquire(&cfg.gate_dir, ADMISSION_LOCK_BUDGET)
            .and_then(|guard| snap.refresh(&cfg).ok().map(|()| guard))
    } else {
        None
    };
    // The load figures of a denial by resources, for its deny log row. A duplicate is not one:
    // the resources are fine, the same command is already running.
    let mut loads = None;
    let (decision, keys) = if let Some(reason) = duplicate_reason {
        (Some(reason), vec!["duplicate"])
    } else if let Some(denial) = deny_reason(&snap, &cfg, kind, &wanted) {
        // A denial the reservations cause is over once they are, at most when the longest warm-up
        // left has passed. Otherwise -- load or memory pressure alone, or a need that MemAvailable
        // alone cannot meet -- the wait is for load and memory pressure.
        let by_reservation = (denial.reasons.contains(&"reserved") && denial.table.mb() > 0)
            || (denial.reasons.contains(&"load")
                && denial.table.load > 0.0
                && snap.load_1m + wanted.load(&cfg) <= cfg.max_load);
        let wait = if by_reservation {
            format!("the warm-up passes (at most {}s)", denial.table.warmup_left)
        } else {
            "load and memory pressure drop (figures above)".to_owned()
        };
        loads = Some((denial.table.load, denial.need_load));
        let message = format!("{} Denied, not queued: nothing will run this command later. Retry it after {wait}; do not poll with sleep in the foreground.", denial.message);
        (Some(message), denial.reasons)
    } else {
        (None, Vec::new())
    };
    if let (None, Some(guard)) = (&decision, &guard) {
        let _ = guard.admit(
            Admission {
                ts: unix_now(),
                session: session.clone(),
                toolchains: wanted.toolchains.clone(),
                gradle_forks: wanted.gradle_forks,
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
        let mut ctx = crate::gate_stats::DenyContext::new(
            snap.mem_available_mb,
            snap.mem_pressure_some_avg10,
            snap.load_1m,
        );
        if let Some((reserved_load, need_load)) = loads {
            ctx = ctx.with_loads(reserved_load, need_load);
        }
        let retry = record_deny(
            now,
            session.as_deref(),
            id.as_deref(),
            &sub,
            &keys.join(","),
            ctx,
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

/// How a `gate run` may start, as decided under `admission.lock`.
enum Start {
    /// Denied: the message for stderr, with its reasons and figures.
    Denied(Denial),
    /// Registered: the lock file is held until this is dropped.
    Registered(Registration),
    /// Nothing could be written to the registry: warn and run without a registration.
    Unregistered(io::Error),
}

pub fn run(kind: Kind, command: &[String]) -> i32 {
    crate::fleet_cmd::maybe_push_background();
    let (cfg, mut snap) =
        match config().and_then(|cfg| snapshot(&cfg, kind.checks_compilers()).map(|s| (cfg, s))) {
            Ok(pair) => pair,
            Err(e) if unreadable(&e) => {
                let _ = writeln!(io::stderr(), "{}", unreadable_reason(&e));
                record_path_deny("run", kind, "resources", Default::default());
                return 75;
            }
            Err(e) => {
                warning(&e);
                return execute(command);
            }
        };
    let wanted = argv_estimate(command, &cfg);
    // A JS or Python test runner is no compiler: it is checked on the `other` row.
    let check_kind = if runs_no_compiler(command) {
        Kind::Other
    } else {
        kind
    };
    // The snapshot above was taken without any lock (the Gradle CPU sample sleeps). Check and
    // register in the one critical section the hook shares, so two runs starting together cannot
    // both find nothing reserved. Without the lock -- it stays busy, or the registry is unreadable
    // -- warn and run, as for any other gate failure.
    let guarded = AdmissionGuard::acquire(&cfg.gate_dir, RUN_LOCK_BUDGET)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{ADMISSION_LOCK} stayed locked for over {}ms",
                    RUN_LOCK_BUDGET.as_millis()
                ),
            )
        })
        .and_then(|guard| snap.refresh(&cfg).map(|()| guard));
    let guard = match guarded {
        Ok(guard) => guard,
        Err(e) => {
            warning(&e);
            return execute(command);
        }
    };
    // Decide and register under the lock, and under it only: it is released before anything is
    // done with the outcome. A child started under it -- on the way out after a failed
    // registration, say -- would hold it for its whole runtime, and every other run and hook
    // would wait out its budget until then.
    let start = guard.under(|| {
        if let Some(denial) = deny_reason(&snap, &cfg, check_kind, &wanted) {
            return Start::Denied(denial);
        }
        let holder = Holder {
            pid: std::process::id(),
            kind: kind.as_str().into(),
            command: command.to_vec(),
            started_at: unix_now(),
            cwd: env::current_dir()
                .ok()
                .and_then(|p| fs::canonicalize(p).ok())
                .map(|p| p.to_string_lossy().into_owned()),
            target: env::var("CARGO_TARGET_DIR").ok(),
        };
        match register(&cfg, &holder) {
            Ok(registration) => Start::Registered(registration),
            Err(e) => Start::Unregistered(e),
        }
    });
    match start {
        Start::Denied(denial) => {
            let _ = writeln!(io::stderr(), "{}", denial.message);
            // Outside the lock, like everything else done with the decision.
            record_path_deny(
                "run",
                kind,
                &denial.reasons.join(","),
                denial.context(&snap),
            );
            75
        }
        Start::Unregistered(e) => {
            warning(&e);
            execute(command)
        }
        Start::Registered(registration) => {
            let exit = crate::gate_stats::run_measured(kind.as_str(), command, &cfg.proc_root);
            drop(registration);
            exit
        }
    }
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

/// The `status --json` document as a value, for the fleet snapshot.
pub fn status_value() -> io::Result<serde_json::Value> {
    document()
        .map(|(document, _)| document)
        .and_then(|document| serde_json::to_value(&document).map_err(io::Error::other))
}

/// The snapshot of this moment with the capacity read from it -- what `status` prints, as JSON or
/// as text, and what `fleet push` uploads -- and the sum of reservations both were taken from, so
/// that no figure of one report is a second older than another.
fn document() -> io::Result<(StatusDocument, Reservation)> {
    let cfg = config()?;
    let snap = snapshot(&cfg, true)?;
    let table = reserved(&snap, &cfg, unix_now());
    let capacity = capacity(&snap, &cfg, table.clone());
    Ok((
        StatusDocument {
            snapshot: snap,
            capacity,
        },
        table,
    ))
}

/// `CLAUDECAT_SLOTS` and `CLAUDECAT_OTHER_SLOTS` used to cap the runs. They do nothing now.
fn obsolete_slot_variables() -> Vec<&'static str> {
    ["CLAUDECAT_SLOTS", "CLAUDECAT_OTHER_SLOTS"]
        .into_iter()
        .filter(|key| env::var_os(key).is_some())
        .collect()
}

/// The gate directory is private to our uid. One that group or others can get into -- made before
/// it was 0700 -- is worth a notice, with its fix.
fn open_gate_dir(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| {
        format!(
            "gate directory {} is open to other users (mode {mode:o}); run chmod 700 on it",
            path.display()
        )
    })
}

pub fn status(json: bool) -> i32 {
    crate::fleet_cmd::maybe_push_background();
    let (snap, capacity, table) = match document() {
        Ok((
            StatusDocument {
                snapshot: snap,
                capacity,
            },
            table,
        )) => (snap, capacity, table),
        Err(e) => {
            let _ = writeln!(io::stderr(), "status warning: {e}");
            return 0;
        }
    };
    let open_dir = open_gate_dir(&snap.gate_dir);
    if json {
        match serde_json::to_string_pretty(&StatusDocument {
            snapshot: snap,
            capacity,
        }) {
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
            "limits: cpus={} MemTotal={}; max_load={:.2} [{}]",
            l.cpus.map_or_else(|| "?".into(), |c| c.to_string()),
            l.mem_total_mb
                .map_or_else(|| "?".into(), |m| format!("{m}MB")),
            l.max_load,
            l.max_load_basis
        );
        let total = |figure: Option<String>| figure.unwrap_or_else(|| "?".to_owned());
        let _ = writeln!(
            io::stdout(),
            "capacity: cpus={} mem_total={} max_load={:.2} load_1m={:.2} reserved_load={:.2} load_headroom={:.2}",
            total(capacity.cpus.map(|c| c.to_string())),
            total(capacity.mem_total_mb.map(|mb| format!("{mb}MB"))),
            capacity.max_load,
            capacity.load_1m,
            capacity.reserved_load,
            capacity.load_headroom
        );
        let _ = writeln!(
            io::stdout(),
            "capacity: mem_available={}MB reserved={}MB mem_headroom={}MB psi={} blocked={} warming={}",
            capacity.mem_available_mb,
            capacity.reserved_mb,
            capacity.mem_headroom_mb,
            capacity
                .mem_pressure
                .map_or_else(|| "n/a".into(), |psi| format!("{psi:.2}")),
            if capacity.blocked.is_empty() {
                "none".to_owned()
            } else {
                capacity.blocked.join(",")
            },
            capacity.warming
        );
        for h in &snap.holders {
            let _ = writeln!(
                io::stdout(),
                "holder: pid={} kind={} cmd={} since={}",
                h.pid,
                h.kind,
                h.command.join(" "),
                h.started_at
            );
        }
        let _ = writeln!(
            io::stdout(),
            "{} reserved_load={:.2}",
            reserved_figure(&table),
            table.load
        );
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
    // Notices go to stderr, so `--json` stays a document.
    if !json {
        let obsolete = obsolete_slot_variables();
        if !obsolete.is_empty() {
            let _ = writeln!(
                io::stderr(),
                "status warning: {} set, but the gate no longer counts slots: it judges load and memory",
                obsolete.join(" and ")
            );
        }
    }
    if let Some(notice) = open_dir {
        let _ = writeln!(io::stderr(), "status warning: {notice}");
    }
    0
}

#[cfg(test)]
mod registration_tests {
    use super::*;

    /// Only lock files owned by our own uid are holders. Seeing that takes a foreign owner, which
    /// needs root, so this calls the enumeration with another uid instead of going through the CLI.
    #[test]
    fn only_lock_files_of_our_own_uid_are_holders() {
        let dir = tempfile::tempdir().unwrap();
        let own = own_uid().unwrap();
        let path = dir.path().join("run-7-0000000000000001.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.try_lock().unwrap();
        serde_json::to_writer(
            &file,
            &serde_json::json!({"pid":7,"kind":"build","command":["cargo","test"],"started_at":1}),
        )
        .unwrap();
        let foreign = own
            .checked_add(1)
            .filter(|uid| *uid != 0)
            .unwrap_or(own - 1);
        assert_eq!(holders(dir.path(), own).unwrap().len(), 1);
        assert!(holders(dir.path(), foreign).unwrap().is_empty());
        assert!(path.exists(), "a file of another uid is not ours to remove");
    }
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
            holders: Vec::new(),
            limits: Limits {
                cpus: None,
                mem_total_mb: None,
                max_load: 0.0,
                max_load_basis: String::new(),
            },
            compiling: Vec::new(),
            orphan_brokers: Vec::new(),
            admissions: Vec::new(),
            uid: 5002,
            gate_dir: PathBuf::new(),
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

#[cfg(test)]
mod estimate_tests {
    use super::*;

    const GRADLE: Toolchain = Toolchain::Gradle;
    const CARGO: Toolchain = Toolchain::Cargo;

    /// The default estimates, on a machine with `cpus` CPUs and `mem_total_mb`.
    fn config(cpus: usize, mem_total_mb: u64) -> Config {
        let limits = Limits {
            cpus: Some(cpus),
            mem_total_mb: Some(mem_total_mb),
            max_load: 6.0,
            max_load_basis: String::new(),
        };
        Config {
            proc_root: PathBuf::new(),
            gate_dir: PathBuf::new(),
            max_load: 6.0,
            max_mem_pressure: 10.0,
            min_available_mb: 1536,
            limits,
            reserve_gradle_mb: 3072,
            reserve_cargo_mb: 1536,
            reserve_warmup_secs: 60,
            reserve_cargo_load: 2.0,
            reserve_gradle_load: 1.0,
            reserve_other_mb: 1024,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            load_1m: 0.0,
            swap_used_mb: 0,
            mem_available_mb: 0,
            mem_pressure_some_avg10: None,
            holders: Vec::new(),
            limits: config(16, 32_000).limits,
            compiling: Vec::new(),
            orphan_brokers: Vec::new(),
            admissions: Vec::new(),
            uid: 0,
            gate_dir: PathBuf::new(),
        }
    }

    fn holder(pid: u32, command: &[&str], started_at: u64) -> Holder {
        Holder {
            pid,
            kind: "build".into(),
            command: command.iter().map(|w| w.to_string()).collect(),
            started_at,
            cwd: None,
            target: None,
        }
    }

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|w| w.to_string()).collect()
    }

    fn row(session: Option<&str>, toolchains: &[Toolchain], gradle_forks: u64) -> Admission {
        Admission {
            ts: 1000,
            session: session.map(str::to_owned),
            toolchains: toolchains.to_vec(),
            gradle_forks,
        }
    }

    /// The reservation table of `rows` alone, after a round trip through a fresh admission file.
    fn reserved_rows(rows: &[Admission]) -> BTreeMap<Toolchain, u64> {
        let dir = tempfile::tempdir().unwrap();
        let mut text = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut text, row).unwrap();
            text.push(b'\n');
        }
        fs::write(dir.path().join(ADMISSION_FILE), text).unwrap();
        let mut snap = snapshot();
        snap.admissions = read_admissions(dir.path(), 1000, 60).unwrap();
        assert_eq!(snap.admissions.len(), rows.len(), "every row stays fresh");
        reserved(&snap, &config(16, 32_000), 1000).memory
    }

    #[test]
    fn gradle_test_forks_follow_the_command() {
        // 16 CPUs and 32 GB: the build's default is 2 forks.
        let cfg = config(16, 32_000);
        let forks = |command: &str| command_estimate(command, &cfg).gradle_forks;
        assert_eq!(forks("./gradlew test -PtestForks=3"), 3);
        assert_eq!(forks("gradlew :backend:test -P testForks=4"), 4);
        // Invalid values are no statement: the build's default applies.
        assert_eq!(forks("./gradlew test -PtestForks=0"), 2);
        assert_eq!(forks("./gradlew test -PtestForks=x"), 2);
        assert_eq!(forks("./gradlew test -P testForks="), 2);
        assert_eq!(forks("./gradlew test -P"), 2);
        assert_eq!(forks("./gradlew :backend:test"), 2);
        assert_eq!(forks("./gradlew test -PtestForks=3 -P testForks=x"), 2);
        // Flags change nothing, and wrappers are skipped on the way to the arguments.
        assert_eq!(
            forks("./gradlew test --rerun-tasks --no-daemon -PtestForks=3"),
            3
        );
        assert_eq!(
            forks("timeout -s TERM 30s env A=b ./gradlew test -PtestForks=3"),
            3
        );
        assert_eq!(
            forks("/opt/gradle/bin/gradle :backend:test -PtestForks=3"),
            3
        );
        // No test task is no test run, whatever the command says about forks.
        assert_eq!(forks("./gradlew build"), 1);
        assert_eq!(forks("./gradlew build -PtestForks=3"), 1);
        assert_eq!(
            forks("./gradlew testClasses :backend:assemble -PtestForks=3"),
            1
        );
        // Only the arguments after the command name are tasks.
        assert_eq!(forks("X=:test ./gradlew build"), 1);
        assert_eq!(forks("env test=./gradlew ./gradlew build"), 1);
    }

    #[test]
    fn default_gradle_test_forks_follow_the_machine() {
        let forks = |cpus, mem_total_mb| default_test_forks(&config(cpus, mem_total_mb));
        assert_eq!(forks(16, 32_000), 2);
        assert_eq!(forks(4, 8192), 2);
        assert_eq!(forks(2, 32_000), 1);
        assert_eq!(forks(3, 32_000), 1);
        assert_eq!(forks(16, 6000), 1);
        assert_eq!(forks(16, 8191), 1);
        let mut cfg = config(16, 32_000);
        cfg.limits.cpus = None;
        assert_eq!(default_test_forks(&cfg), 1);
        cfg = config(16, 32_000);
        cfg.limits.mem_total_mb = None;
        assert_eq!(default_test_forks(&cfg), 1);
    }

    #[test]
    fn only_gradle_test_runs_are_scaled() {
        // 16 CPUs and 32 GB: a test run without a statement takes 2 forks.
        let cfg = config(16, 32_000);
        let total = |command: &str| command_estimate(command, &cfg).total_mb(&cfg);
        assert_eq!(total("./gradlew test"), 2 * 3072);
        assert_eq!(total("./gradlew test -PtestForks=3"), 3 * 3072);
        assert_eq!(total("./gradlew build"), 3072);
        // An option's value is no task: `-x test` excludes the tests, `--tests test` filters them
        // in a run that names no test task, `-p test` is a directory.
        assert_eq!(total("./gradlew build -x test"), 3072);
        assert_eq!(total("./gradlew build --exclude-task test"), 3072);
        assert_eq!(total("./gradlew -p test build"), 3072);
        assert_eq!(total("./gradlew build -x test test"), 2 * 3072);
        assert_eq!(total("./gradlew -x :lint:check :backend:test"), 2 * 3072);
        // Cargo keeps its own estimate, whatever its arguments say.
        assert_eq!(command_estimate("cargo test", &cfg).toolchains, vec![CARGO]);
        assert_eq!(total("cargo test"), 1536);
        assert_eq!(total("cargo test -PtestForks=8"), 1536);
        assert_eq!(
            total("cargo build && ./gradlew test -PtestForks=2"),
            1536 + 2 * 3072
        );
        // The segments of one command run one after another: the largest sets the peak, and a
        // toolchain is counted once.
        assert_eq!(
            total("./gradlew test -PtestForks=5 && ./gradlew test -PtestForks=2"),
            5 * 3072
        );
        assert_eq!(total("./gradlew test && ./gradlew build"), 2 * 3072);
        let both = command_estimate("./gradlew test -PtestForks=2 && cargo build", &cfg);
        assert_eq!(both.toolchains, vec![CARGO, GRADLE], "sorted");
        assert_eq!(both.total_mb(&cfg), 2 * 3072 + 1536);
        // Nothing recognised: nothing to reserve.
        assert_eq!(command_estimate("echo hi", &cfg), Estimate::default());
        assert_eq!(
            command_estimate("echo \"unterminated", &cfg),
            Estimate::default()
        );
        // Saturation, not wrapping.
        let mut huge = config(16, 32_000);
        huge.reserve_gradle_mb = u64::MAX;
        assert_eq!(
            command_estimate("./gradlew test -PtestForks=2", &huge).total_mb(&huge),
            u64::MAX
        );
    }

    #[test]
    fn argv_estimates_look_through_sh_c() {
        let cfg = config(2, 6000);
        let total = |argv: &[&str]| argv_estimate(&words(argv), &cfg).total_mb(&cfg);
        assert_eq!(total(&["./gradlew", "test", "-PtestForks=3"]), 3 * 3072);
        assert_eq!(
            total(&["sh", "-c", "./gradlew test -PtestForks=3"]),
            3 * 3072
        );
        assert_eq!(
            total(&["bash", "-l", "-c", "cd x && ./gradlew test -PtestForks=3"]),
            3 * 3072
        );
        // 2 CPUs: the build's default is one fork.
        assert_eq!(total(&["sh", "-c", "./gradlew test"]), 3072);
        assert_eq!(total(&["cargo", "test"]), 1536);
        assert_eq!(
            argv_estimate(&words(&["make", "all"]), &cfg),
            Estimate::default()
        );
        assert_eq!(
            argv_estimate(&words(&["sh", "-c", "echo hi"]), &cfg),
            Estimate::default()
        );
    }

    #[test]
    fn warming_up_holders_reserve_gradle_per_fork() {
        let cfg = config(16, 32_000);
        let run = &["./gradlew", "test", "-PtestForks=2"];
        let mut snap = snapshot();
        snap.holders.push(holder(77, run, 1000));
        // A second holder adds its own forks, and looks through `sh -c`.
        snap.holders.push(holder(
            78,
            &["sh", "-c", "./gradlew test -PtestForks=3"],
            1000,
        ));
        let table = BTreeMap::from([(GRADLE, 5 * 3072)]);
        assert_eq!(reserved(&snap, &cfg, 1010).memory, table);
        // Past the warm-up window its memory is inside MemAvailable already.
        assert_eq!(reserved(&snap, &cfg, 1060).memory, BTreeMap::new());
        // The load is 1.0 per fork and decays linearly: ten seconds in 5/6 is left, halfway 1/2.
        assert!((reserved(&snap, &cfg, 1010).load - 5.0 * 50.0 / 60.0).abs() < 1e-9);
        assert!((reserved(&snap, &cfg, 1030).load - 2.5).abs() < 1e-9);
        assert_eq!(reserved(&snap, &cfg, 1060).load, 0.0);
        // A holder stamped in the future reserves nothing until the clock catches up (5s slack).
        snap.holders[0].started_at = 1012;
        assert_eq!(
            reserved(&snap, &cfg, 1010).memory,
            BTreeMap::from([(GRADLE, 5 * 3072)])
        );
        snap.holders[0].started_at = 1016;
        assert_eq!(
            reserved(&snap, &cfg, 1010).memory,
            BTreeMap::from([(GRADLE, 3 * 3072)])
        );
        snap.holders[0].started_at = 1015;
        assert_eq!(
            reserved(&snap, &cfg, 1010).memory,
            BTreeMap::from([(GRADLE, 5 * 3072)])
        );
    }

    #[test]
    fn admission_rows_reserve_gradle_per_fork() {
        // A row written before the field existed is one fork, on a machine whose default is 2.
        let old = r#"{"ts":1000,"session":null,"toolchains":["gradle"]}"#;
        let admission: Admission = serde_json::from_str(old).unwrap();
        assert_eq!(admission.gradle_forks, 1);
        let table = BTreeMap::from([(GRADLE, 3072)]);
        assert_eq!(reserved_rows(&[admission]), table);
        // One fork is not written, so such rows keep the old shape.
        assert_eq!(
            serde_json::to_string(&row(None, &[GRADLE], 1)).unwrap(),
            old
        );
        // A hand-edited zero is one fork too.
        let zero: Admission = serde_json::from_str(
            r#"{"ts":1000,"session":null,"toolchains":["gradle"],"gradle_forks":0}"#,
        )
        .unwrap();
        assert_eq!(reserved_rows(&[zero]), table);
        // The field scales Gradle only.
        let table = BTreeMap::from([(GRADLE, 2 * 3072), (CARGO, 1536)]);
        assert_eq!(reserved_rows(&[row(None, &[CARGO, GRADLE], 2)]), table);
    }

    #[test]
    fn a_sessions_new_run_replaces_the_old_one_forks_included() {
        let dir = tempfile::tempdir().unwrap();
        let guard = AdmissionGuard::acquire(dir.path(), ADMISSION_LOCK_BUDGET).unwrap();
        let table = |rows: Vec<Admission>| {
            let mut snap = snapshot();
            snap.admissions = rows;
            reserved(&snap, &config(16, 32_000), 1000).memory
        };
        // The same session runs its commands one after another: the later Gradle run replaces the
        // earlier one, with its own forks.
        guard.admit(row(Some("a"), &[GRADLE], 4), 1000, 60).unwrap();
        guard.admit(row(Some("a"), &[GRADLE], 2), 1000, 60).unwrap();
        let rows = read_admissions(dir.path(), 1000, 60).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].gradle_forks, 2);
        assert_eq!(table(rows), BTreeMap::from([(GRADLE, 2 * 3072)]));
        // Another session's run, or the same session's run of another toolchain, keeps its own.
        guard.admit(row(Some("b"), &[GRADLE], 3), 1000, 60).unwrap();
        guard.admit(row(Some("a"), &[CARGO], 1), 1000, 60).unwrap();
        let rows = read_admissions(dir.path(), 1000, 60).unwrap();
        assert_eq!(rows.len(), 3, "{rows:?}");
        let expected = BTreeMap::from([(GRADLE, 5 * 3072), (CARGO, 1536)]);
        assert_eq!(table(rows), expected);
    }
}
