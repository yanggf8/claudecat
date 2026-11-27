//! Reading fleet rows: per-host summary, the human table, and `pick` ordering.
use crate::fleet_store::Row;
use serde::Serialize;

/// A row older than this (seconds) is reported IDLE: machines only report while running gates, so
/// silence means no work. Its resources are the last reported values.
pub const IDLE_SECS: u64 = 180;

/// The warm-up window a row is read with when it reports none. A row of a build that reports
/// `capacity.warmup_secs` is read with that -- the machine's own `CLAUDECAT_RESERVE_WARMUP_SECS`,
/// which is how long its reservations last; one that reports none gets the gate's default.
const DEFAULT_WARMUP_SECS: u64 = crate::gate::DEFAULT_RESERVE_WARMUP_SECS;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Idle,
    Unknown,
}

/// One machine, as its last row tells. Every figure is None where the row cannot tell it: a
/// machine whose resources could not be read, or a build older than the figure.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Summary {
    pub host: String,
    pub os: String,
    pub state: State,
    pub updated_at: String,
    pub age_secs: u64,
    pub cpus: Option<u64>,
    pub mem_total_mb: Option<u64>,
    pub load_1m: Option<f64>,
    pub max_load: Option<f64>,
    /// Load, in cores, that can still be added before the limit; never negative. Of a fresh row as
    /// it reported it, reservations subtracted; otherwise what the measured load leaves.
    pub load_headroom: Option<f64>,
    pub mem_available_mb: Option<u64>,
    /// Memory, in MB, that a new run can still get: as the fresh row reported it, reservations
    /// subtracted; otherwise all that is available.
    pub mem_headroom_mb: Option<u64>,
    /// Why a new build or test run would be denied on the machine now. Empty when it would start.
    pub blocked: Vec<String>,
    /// Runs still warming up on the machine, 0 in a row that has outlived its reservations; None
    /// where the build reports no capacity.
    pub warming: Option<usize>,
    /// Jobs under `gate run`: the machine's registrations, one per job whatever the number of
    /// compilers it has running. None where the row cannot tell -- the build reports no holders,
    /// or the machine is idle and has stopped reporting -- rather than 0, which would say that
    /// nothing runs.
    pub holders: Option<usize>,
    /// Compiler processes on the machine, as `status` lists them; None as for `holders`.
    pub compiling: Option<usize>,
    pub sessions_busy: usize,
    /// Claude sessions running on the machine.
    pub sessions_total: usize,
    pub sessions_unsupported: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Pool {
    Gradle,
    Other,
}

/// Memory a new Gradle run is assumed to need, in MB: the gate's default estimate for one test
/// fork (`DEFAULT_RESERVE_GRADLE_MB`).
pub const GRADLE_NEED_MB: u64 = 3072;
/// Load a new Gradle run is assumed to need, in cores: the gate's default for one test fork
/// (`DEFAULT_RESERVE_GRADLE_LOAD`).
pub const GRADLE_NEED_LOAD: f64 = 1.0;
/// The same for a run of the other pool that names no toolchain (`DEFAULT_RESERVE_OTHER_MB`); it
/// asks for no load.
pub const OTHER_NEED_MB: u64 = 1024;
pub const OTHER_NEED_LOAD: f64 = 0.0;

/// What a job asks of its host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Need {
    /// Memory, in MB.
    pub mb: u64,
    /// Load, in cores.
    pub load: f64,
}

impl Need {
    /// Nothing asked: every host with any room at all.
    pub const NOTHING: Need = Need { mb: 0, load: 0.0 };
}

impl Pool {
    /// What a new run of the pool is assumed to need: the needs `fleet pick --kind` sets.
    pub fn need(self) -> Need {
        match self {
            Pool::Gradle => Need {
                mb: GRADLE_NEED_MB,
                load: GRADLE_NEED_LOAD,
            },
            Pool::Other => Need {
                mb: OTHER_NEED_MB,
                load: OTHER_NEED_LOAD,
            },
        }
    }
}

/// A host fit for a job, with what it has left for it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Pick {
    pub host: String,
    /// Cores that can still be added before the limit; never negative.
    pub load_headroom: f64,
    /// MB a new run can still get.
    pub mem_headroom_mb: u64,
    pub cpus: Option<u64>,
    pub mem_total_mb: Option<u64>,
    pub load_1m: f64,
    pub max_load: f64,
    /// Always empty: a host with a reason in it is not listed.
    pub blocked: Vec<String>,
    /// The figures are the last reported ones of a machine that has gone quiet.
    pub idle: bool,
}

/// Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ`; None for anything else.
pub fn parse_rfc3339_utc(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days from civil (Howard Hinnant), the inverse of dates::civil_from_days
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// `12s`, `5m`, `3h`, `2d`.
pub fn format_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// Why a new build or test run would be denied on the machine now. A machine that reports
/// `capacity` says so itself; one that reports none -- every build older than it, counting slots
/// or not, which decides nothing here -- is judged on its measured load, all that the top-level
/// figures tell about it.
fn blocked(status: &serde_json::Value, load_1m: Option<f64>, max_load: Option<f64>) -> Vec<String> {
    if let Some(reasons) = status
        .get("capacity")
        .filter(|capacity| capacity.is_object())
        .and_then(|capacity| capacity.get("blocked"))
        .and_then(serde_json::Value::as_array)
    {
        return reasons
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    load_1m
        .zip(max_load)
        .filter(|(load, max_load)| load > max_load)
        .map(|_| vec!["load".to_owned()])
        .unwrap_or_default()
}

pub fn summarize(row: &Row, now: u64) -> Summary {
    let snap = &row.snapshot;
    let age_secs = parse_rfc3339_utc(&row.updated_at).map_or(u64::MAX, |t| now.saturating_sub(t));
    let status = snap.status.as_ref();
    let state = if status.is_none() {
        State::Unknown
    } else if age_secs > IDLE_SECS {
        State::Idle
    } else {
        State::Ok
    };
    let null = serde_json::Value::Null;
    let st = status.unwrap_or(&null);
    let idle = state == State::Idle;
    let capacity = st.get("capacity").filter(|c| c.is_object());
    let (load_1m, max_load) = (st["load_1m"].as_f64(), st["limits"]["max_load"].as_f64());
    let mem_available_mb = st["mem_available_mb"].as_u64();
    // Row age. A machine pushes at most once a minute, so by the time its row is read the runs it
    // held memory and load for may be past the warm-up window: what they reserved has by then shown
    // up in the measured load and the available memory, or never will. So a row older than the
    // window is read without its reservations -- the headroom is what the measured load leaves of
    // the limit and all that is available -- and a `load` block in it that only they caused, the
    // measured load being within the limit, is dropped with them. A fresh row keeps what it
    // reported.
    //
    // The window is the reporting machine's own, `capacity.warmup_secs`: the reservations in the
    // row decay over that long, whatever it is, and every run in it is past the window once the row
    // is. A row that reports none gets the default.
    let warmup_secs = capacity
        .and_then(|c| c["warmup_secs"].as_u64())
        .unwrap_or(DEFAULT_WARMUP_SECS);
    let stale = age_secs > warmup_secs;
    // A figure of the capacity, in a row young enough for its reservations to count.
    let fresh = |figure: &str| capacity.filter(|_| !stale).and_then(|c| c.get(figure));
    let measured = load_1m
        .zip(max_load)
        .map(|(load, max)| crate::gate::at_least_zero(max - load));
    let mut blocked = blocked(st, load_1m, max_load);
    if stale && load_1m.zip(max_load).is_some_and(|(load, max)| load <= max) {
        blocked.retain(|why| why != "load");
    }
    // Two figures, not their sum: a `gate run` is one job, however many compilers it has running.
    // An idle machine has stopped reporting, so what runs on it now is not to be known: None, not
    // 0. So is None for a build that reports no such list.
    let counted = |figure: &str| st[figure].as_array().map(Vec::len).filter(|_| !idle);
    Summary {
        host: row.host.clone(),
        os: snap.os.clone(),
        state,
        updated_at: row.updated_at.clone(),
        age_secs,
        cpus: st["limits"]["cpus"].as_u64(),
        mem_total_mb: st["limits"]["mem_total_mb"].as_u64(),
        load_1m,
        max_load,
        // Never negative, whatever a build before the floor reported.
        load_headroom: fresh("load_headroom")
            .and_then(serde_json::Value::as_f64)
            .map(crate::gate::at_least_zero)
            .or(measured),
        mem_available_mb,
        mem_headroom_mb: fresh("mem_headroom_mb")
            .and_then(serde_json::Value::as_u64)
            .or(mem_available_mb),
        blocked,
        // Nothing in a stale row is warming up any more.
        warming: capacity
            .and_then(|c| c["warming"].as_u64())
            .and_then(|w| usize::try_from(w).ok())
            .map(|warming| if stale { 0 } else { warming }),
        holders: counted("holders"),
        compiling: counted("compiling"),
        sessions_busy: if idle {
            0
        } else {
            snap.sessions.iter().filter(|s| s.cpu_percent > 5.0).count()
        },
        sessions_total: snap.sessions.len(),
        sessions_unsupported: snap.sessions_unsupported,
    }
}

/// The table of `claudecat fleet`: one line per host. Memory is in GB with one decimal, load and
/// its headroom in cores. RUN counts the `gate run` jobs and COMP the compiler processes, each on
/// its own; a dash says the row cannot tell, which is the case for every idle row. Idle rows show
/// the last reported load and memory, marked as old values.
pub fn render_table(rows: &[Summary]) -> String {
    const COLUMNS: usize = 13;
    let mut cells: Vec<[String; COLUMNS]> = vec![[
        "HOST", "STATE", "AGE", "CPUS", "MEM_GB", "LOAD", "HEADROOM", "FREE_GB", "BLOCKED", "WARM",
        "RUN", "COMP", "SESS",
    ]
    .map(str::to_owned)];
    let dash = || "-".to_owned();
    let gb = |mb: u64| format!("{:.1}", mb as f64 / 1024.0);
    for r in rows {
        // Idle rows show the last reported load and memory: mark them as old values.
        let old = if r.state == State::Idle { "*" } else { "" };
        cells.push([
            r.host.clone(),
            match r.state {
                State::Ok => "ok",
                State::Idle => "IDLE",
                State::Unknown => "UNKNOWN",
            }
            .to_owned(),
            format_age(r.age_secs),
            r.cpus.map_or_else(dash, |cpus| cpus.to_string()),
            r.mem_total_mb.map_or_else(dash, gb),
            match (r.load_1m, r.max_load) {
                (Some(l), Some(m)) => format!("{l:.2}/{m:.2}{old}"),
                _ => dash(),
            },
            r.load_headroom
                .map_or_else(dash, |headroom| format!("{headroom:.2}{old}")),
            r.mem_headroom_mb
                .map_or_else(dash, |mb| format!("{}{old}", gb(mb))),
            if r.blocked.is_empty() {
                dash()
            } else {
                r.blocked.join(",")
            },
            r.warming.map_or_else(dash, |warming| warming.to_string()),
            r.holders.map_or_else(dash, |holders| holders.to_string()),
            r.compiling
                .map_or_else(dash, |compiling| compiling.to_string()),
            if r.sessions_unsupported {
                dash()
            } else {
                r.sessions_total.to_string()
            },
        ]);
    }
    let widths: Vec<usize> = (0..COLUMNS)
        .map(|i| cells.iter().map(|c| c[i].len()).max().unwrap_or(0))
        .collect();
    let footnote = rows.iter().any(|r| r.state == State::Idle);
    let mut out = cells
        .iter()
        .map(|c| {
            let line: Vec<String> = c
                .iter()
                .zip(&widths)
                .map(|(v, w)| format!("{v:<w$}"))
                .collect();
            line.join("  ").trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if footnote {
        out.push_str("* last reported values (see AGE); IDLE = no recent report, no work\n");
    }
    out
}

/// Hosts with room for a job that needs `need`: readable (silent ones count as idle), not blocked,
/// with at least that much memory headroom and that much load headroom. One whose headroom is not
/// to be known is left out, there being no telling whether it fits. Best first: most load headroom,
/// then most memory headroom, then by name. Nothing else decides -- slots least of all.
pub fn pick(rows: &[Summary], need: Need) -> Vec<Pick> {
    let mut fit: Vec<Pick> = rows
        .iter()
        .filter(|r| matches!(r.state, State::Ok | State::Idle))
        .filter(|r| r.blocked.is_empty())
        .filter_map(|r| {
            Some(Pick {
                host: r.host.clone(),
                load_headroom: r.load_headroom?,
                mem_headroom_mb: r.mem_headroom_mb?,
                cpus: r.cpus,
                mem_total_mb: r.mem_total_mb,
                load_1m: r.load_1m?,
                max_load: r.max_load?,
                blocked: r.blocked.clone(),
                idle: r.state == State::Idle,
            })
        })
        .filter(|p| p.mem_headroom_mb >= need.mb && p.load_headroom >= need.load)
        .collect();
    fit.sort_by(|a, b| {
        b.load_headroom
            .total_cmp(&a.load_headroom)
            .then(b.mem_headroom_mb.cmp(&a.mem_headroom_mb))
            .then(a.host.cmp(&b.host))
    });
    fit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{assemble, Session};
    use serde_json::json;

    const NOW: u64 = 1_795_169_472;

    fn slot(i: usize, occupied: bool) -> serde_json::Value {
        json!({"index": i, "holder": null, "occupied": occupied})
    }

    /// The status document of a build older than `capacity` and `holders`: its slot pools, its
    /// compilers, and the limits such a build reported (6 CPUs, 8000 MB, a limit of 6).
    fn old_status(mem: u64, load: f64, used: usize, total: usize) -> serde_json::Value {
        let slots: Vec<_> = (0..total).map(|i| slot(i, i < used)).collect();
        json!({
            "load_1m": load, "mem_available_mb": mem, "swap_used_mb": 0,
            "slots": slots,
            "other_slots": [slot(0, false), slot(1, true)],
            "limits": {"slots": total, "other_slots": 2, "max_load": 6.0, "cpus": 6, "mem_total_mb": 8000},
            "compiling": [{"pid": 1, "name": "rustc", "cwd": "/x"}]
        })
    }

    /// The status document of a build that reports no slots (6 CPUs, 8000 MB, a limit of 6), with
    /// `capacity` when it has one.
    fn status(capacity: Option<serde_json::Value>) -> serde_json::Value {
        let mut status = json!({
            "load_1m": 0.5, "mem_available_mb": 9000, "swap_used_mb": 0,
            "holders": [], "compiling": [],
            "limits": {"cpus": 6, "mem_total_mb": 8000, "max_load": 6.0},
        });
        if let Some(capacity) = capacity {
            status["capacity"] = capacity;
        }
        status
    }

    /// A capacity as a build reports it while runs are warming up: `load` measured on a limit of 6
    /// with `reserved_load` cores more held for them, `mem` MB available with `reserved_mb` of them
    /// held, and `blocked`. `load_headroom` is as a build before the floor reported it: it may be
    /// negative.
    fn capacity(
        load: f64,
        mem: u64,
        reserved_load: f64,
        reserved_mb: u64,
        blocked: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "cpus": 6, "mem_total_mb": 8000, "max_load": 6.0, "load_1m": load,
            "reserved_load": reserved_load, "load_headroom": 6.0 - load - reserved_load,
            "mem_available_mb": mem, "reserved_mb": reserved_mb,
            "mem_headroom_mb": mem.saturating_sub(reserved_mb), "mem_pressure": 0.1,
            "blocked": blocked, "warming": 1,
        })
    }

    fn sess(cpu: f64) -> Session {
        Session {
            pid: 1,
            cwd: "/w".into(),
            cpu_percent: cpu,
            elapsed_secs: 9,
            tmux: None,
        }
    }

    fn row(host: &str, age: u64, status: Result<serde_json::Value, String>) -> Row {
        let snap = assemble(
            host,
            "linux",
            NOW - age,
            status,
            Some(vec![sess(50.0), sess(1.0), sess(5.0)]),
        );
        Row {
            host: host.into(),
            updated_at: snap.collected_at.clone(),
            snapshot: snap,
        }
    }

    /// A machine whose report is `age` seconds old and says that; its top-level figures are the
    /// same as its capacity's, as on a machine.
    fn reported(host: &str, age: u64, capacity: serde_json::Value) -> Summary {
        let mut st = status(Some(capacity.clone()));
        st["load_1m"] = capacity["load_1m"].clone();
        st["mem_available_mb"] = capacity["mem_available_mb"].clone();
        summarize(&row(host, age, Ok(st)), NOW)
    }

    /// A machine whose report is `age` seconds old and reports no capacity: nothing is reserved on
    /// it, so all `mem` MB are a run's to take.
    fn unreported(host: &str, age: u64, load: f64, mem: u64) -> Summary {
        let mut st = status(None);
        st["load_1m"] = load.into();
        st["mem_available_mb"] = mem.into();
        summarize(&row(host, age, Ok(st)), NOW)
    }

    fn json_of(s: &Summary) -> serde_json::Value {
        serde_json::to_value(s).unwrap()
    }

    fn hosts(picked: &[Pick]) -> Vec<&str> {
        picked.iter().map(|p| p.host.as_str()).collect()
    }

    #[test]
    fn rfc3339_round_trips_and_rejects_noise() {
        assert_eq!(
            parse_rfc3339_utc("2026-11-20T10:11:12Z"),
            Some(1_795_169_472)
        );
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_utc("2024-02-29T23:59:59Z"),
            Some(1_709_251_199)
        );
        assert_eq!(parse_rfc3339_utc("yesterday"), None);
    }

    #[test]
    fn age_is_formatted_in_the_largest_whole_unit() {
        assert_eq!(format_age(12), "12s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(300), "5m");
        assert_eq!(format_age(7300), "2h");
        assert_eq!(format_age(200_000), "2d");
    }

    /// A row of a build that counted slots is read on its top-level figures. It reports neither
    /// capacity nor holders: what is left of its load is figured here, all its available memory is
    /// a run's to take, and who is warming up or running under `gate run` is not to be known. Its
    /// compilers it does report, so COMP is.
    #[test]
    fn summary_reads_the_top_level_figures_of_a_row_that_counts_slots() {
        let s = summarize(&row("nuc", 12, Ok(old_status(5120, 0.5, 1, 3))), NOW);
        assert_eq!(s.host, "nuc");
        assert_eq!(s.state, State::Ok);
        assert_eq!(s.age_secs, 12);
        assert_eq!((s.cpus, s.mem_total_mb), (Some(6), Some(8000)));
        assert_eq!(s.mem_available_mb, Some(5120));
        assert_eq!((s.load_1m, s.max_load), (Some(0.5), Some(6.0)));
        assert_eq!(
            (s.load_headroom, s.mem_headroom_mb),
            (Some(5.5), Some(5120))
        );
        assert_eq!((s.warming, s.holders, s.compiling), (None, None, Some(1)));
        assert_eq!(s.blocked, Vec::<String>::new());
        // busy means cpu strictly above 5%
        assert_eq!((s.sessions_busy, s.sessions_total), (1, 3));
    }

    #[test]
    fn idle_after_180_seconds_not_at_180() {
        assert_eq!(
            summarize(&row("a", 180, Ok(old_status(1, 0.0, 0, 1))), NOW).state,
            State::Ok
        );
        assert_eq!(
            summarize(&row("a", 181, Ok(old_status(1, 0.0, 0, 1))), NOW).state,
            State::Idle
        );
    }

    /// Idle means the machine has stopped reporting: whether anything runs there now is not to be
    /// known -- two holders and a compiler were running when it last said -- so neither figure
    /// claims 0. The figures are the last reported ones.
    #[test]
    fn idle_row_has_no_work_but_keeps_the_machine_and_its_last_resources() {
        let mut st = status(Some(capacity(2.0, 9000, 0.0, 0, json!([]))));
        st["load_1m"] = 2.0.into();
        st["holders"] = json!([
            {"pid": 1, "kind": "build", "command": ["cargo", "test"], "started_at": 1},
            {"pid": 2, "kind": "other", "command": ["sleep", "1"], "started_at": 1},
        ]);
        st["compiling"] = json!([{"pid": 3, "name": "rustc", "cwd": "/x"}]);
        let s = summarize(&row("gpu", 500, Ok(st)), NOW);
        assert_eq!(s.state, State::Idle);
        let j = json_of(&s);
        assert_eq!(
            (j["holders"].clone(), j["compiling"].clone()),
            (serde_json::Value::Null, serde_json::Value::Null),
            "unknown, in the JSON: {j}"
        );
        assert!(j.get("running").is_none(), "{j}");
        // And a dash, not 0, in the table.
        let out = render_table(std::slice::from_ref(&s));
        let cells: Vec<&str> = out.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(cells[10..12], ["-", "-"], "{out}");
        assert_eq!(s.warming, Some(0));
        assert_eq!(s.sessions_busy, 0);
        assert_eq!(s.sessions_total, 3);
        assert_eq!((s.cpus, s.mem_total_mb), (Some(6), Some(8000)));
        assert_eq!(s.mem_available_mb, Some(9000));
        assert_eq!((s.load_1m, s.max_load), (Some(2.0), Some(6.0)));
        assert_eq!(s.age_secs, 500);
    }

    #[test]
    fn unreadable_resources_are_unknown_even_when_fresh() {
        let s = summarize(&row("mac", 1, Err("no /proc".into())), NOW);
        assert_eq!(s.state, State::Unknown);
        assert_eq!(s.mem_available_mb, None);
        assert_eq!(s.load_1m, None);
        assert_eq!((s.load_headroom, s.mem_headroom_mb), (None, None));
        assert_eq!((s.warming, s.holders, s.compiling), (None, None, None));
        assert_eq!(s.blocked, Vec::<String>::new());
    }

    #[test]
    fn unknown_wins_over_idle() {
        assert_eq!(
            summarize(&row("mac", 999, Err("x".into())), NOW).state,
            State::Unknown
        );
    }

    /// One header, and one line per host with what the machine has and what is left of it, for a
    /// row of every kind of build.
    #[test]
    fn table_has_one_header_and_one_line_per_kind_of_row() {
        let mut fresh = status(Some(capacity(0.5, 5120, 2.0, 1536, json!(["memory"]))));
        fresh["mem_available_mb"] = 5120.into();
        fresh["capacity"]["warming"] = 3.into();
        fresh["holders"] = json!([
            {"pid": 1, "kind": "build", "command": ["cargo", "test"], "started_at": 1},
            {"pid": 2, "kind": "other", "command": ["sleep", "1"], "started_at": 1},
        ]);
        fresh["compiling"] = json!([{"pid": 3, "name": "rustc", "cwd": "/x"}]);
        let mut no_capacity = status(None);
        no_capacity["load_1m"] = 6.5.into();
        no_capacity["mem_available_mb"] = 2000.into();
        let rows = vec![
            summarize(&row("nuc", 12, Ok(fresh)), NOW),
            summarize(&row("mgpu", 12, Ok(old_status(9000, 2.0, 3, 3))), NOW),
            summarize(&row("gpu", 400, Ok(old_status(9000, 2.0, 3, 3))), NOW),
            summarize(&row("a1", 12, Ok(no_capacity)), NOW),
            summarize(&row("mac", 5, Err("x".into())), NOW),
        ];
        let out = render_table(&rows);
        let lines: Vec<Vec<&str>> = out
            .lines()
            .map(|l| l.split_whitespace().collect())
            .collect();
        assert_eq!(lines.len(), 7, "{out}");
        assert_eq!(
            lines[0],
            [
                "HOST", "STATE", "AGE", "CPUS", "MEM_GB", "LOAD", "HEADROOM", "FREE_GB", "BLOCKED",
                "WARM", "RUN", "COMP", "SESS",
            ]
        );
        // A build with capacity: 6 - 0.5 - 2.0 cores left, 5120 - 1536 MB free, three runs
        // warming up, two `gate run` holders, one compiler (each its own figure: a run is not
        // counted once more per compiler it has running), and what denies a build there now.
        assert_eq!(
            lines[1],
            [
                "nuc",
                "ok",
                "12s",
                "6",
                "7.8",
                "0.50/6.00",
                "3.50",
                "3.5",
                "memory",
                "3",
                "2",
                "1",
                "3"
            ]
        );
        // A build that counted slots, with all of them used: read on its top-level figures; a dash
        // for what it never reported (capacity, holders), and its one compiler, which it does.
        assert_eq!(
            lines[2],
            [
                "mgpu",
                "ok",
                "12s",
                "6",
                "7.8",
                "2.00/6.00",
                "4.00",
                "8.8",
                "-",
                "-",
                "-",
                "1",
                "3"
            ]
        );
        // The same row once old: the last reported figures, marked as such, and who runs there
        // now not to be known.
        assert_eq!(
            lines[3],
            [
                "gpu",
                "IDLE",
                "6m",
                "6",
                "7.8",
                "2.00/6.00*",
                "4.00*",
                "8.8*",
                "-",
                "-",
                "-",
                "-",
                "3",
            ]
        );
        // A build with neither capacity nor slots: the same figures, and holders, so who runs.
        assert_eq!(
            lines[4],
            [
                "a1",
                "ok",
                "12s",
                "6",
                "7.8",
                "6.50/6.00",
                "0.00",
                "2.0",
                "load",
                "-",
                "0",
                "0",
                "3"
            ]
        );
        assert_eq!(
            lines[5],
            ["mac", "UNKNOWN", "5s", "-", "-", "-", "-", "-", "-", "-", "-", "-", "3",]
        );
        assert!(lines[6][0].starts_with('*'), "{out}");
        assert!(out.contains("AGE"), "{out}");
    }

    #[test]
    fn table_has_no_footnote_without_idle_rows() {
        let rows = vec![summarize(
            &row("nuc", 12, Ok(old_status(5120, 0.5, 1, 3))),
            NOW,
        )];
        let out = render_table(&rows);
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(!out.contains('*'), "{out}");
    }

    /// 4.4: headroom is how much load can still be added, so it is never negative. A build before
    /// the floor reports it below 0; a row older than its reservations, and a row of a build
    /// without capacity, have it figured here from the measured load, and are floored the same.
    #[test]
    fn load_headroom_is_never_negative() {
        let over = capacity(7.0, 4000, 0.0, 0, json!(["load"]));
        assert_eq!(
            over["load_headroom"], -1.0,
            "the fixture reports it as a build before the floor did"
        );
        assert_eq!(
            json_of(&reported("over", 5, over.clone()))["load_headroom"],
            0.0
        );
        // 0.5 measured and 6.0 reserved on a limit of 6.
        let booked = capacity(0.5, 4000, 6.0, 0, json!(["load"]));
        assert_eq!(booked["load_headroom"], -0.5);
        assert_eq!(
            json_of(&reported("booked", 5, booked))["load_headroom"],
            0.0
        );
        // Stale, and without capacity: 6.5 on 6.0.
        assert_eq!(
            json_of(&reported("stale", 100, over.clone()))["load_headroom"],
            0.0
        );
        assert_eq!(
            json_of(&unreported("old", 5, 6.5, 4000))["load_headroom"],
            0.0
        );
        // The table shows a floor of zero, never a minus.
        let out = render_table(&[
            reported("over", 5, over),
            reported("stale", 100, capacity(6.5, 4000, 0.0, 0, json!(["load"]))),
            unreported("old", 5, 6.5, 4000),
        ]);
        for line in out.lines().skip(1) {
            let cells: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(cells[6], "0.00", "{out}");
        }
        // A reported negative zero is 0 as well: `0.0` in JSON, `0.00` in the table.
        let mut hair = capacity(5.9, 4000, 0.0, 0, json!([]));
        hair["load_headroom"] = json!(-0.0);
        let hair = reported("hair", 5, hair);
        assert_eq!(
            serde_json::to_string(&json_of(&hair)["load_headroom"]).unwrap(),
            "0.0"
        );
        let out = render_table(std::slice::from_ref(&hair));
        let cells: Vec<&str> = out.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(cells[6], "0.00", "{out}");
    }

    /// 4.5: a report older than the warm-up window has outlived its reservations. None is
    /// subtracted from the headroom it is shown with, and nothing in it counts as warming up any
    /// more. The fixture reports no `warmup_secs`, so the gate's default (60 s) stands for it.
    #[test]
    fn a_stale_row_keeps_the_reservations_out_of_its_headroom() {
        // 1.0 measured with 2.0 more reserved; 9000 MB available, 4096 of them reserved.
        let cap = capacity(1.0, 9000, 2.0, 4096, json!([]));
        let stale = reported("stale", 100, cap.clone());
        assert_eq!(
            (stale.load_headroom, stale.mem_headroom_mb),
            (Some(5.0), Some(9000))
        );
        assert_eq!(stale.warming, Some(0));
        // At the end of the window the reservations still count; a second past it they do not.
        let at_end = reported("at-end", 60, cap.clone());
        assert_eq!(
            (at_end.load_headroom, at_end.mem_headroom_mb, at_end.warming),
            (Some(3.0), Some(4904), Some(1))
        );
        let past = reported("past", 61, cap);
        assert_eq!(
            (past.load_headroom, past.mem_headroom_mb, past.warming),
            (Some(5.0), Some(9000), Some(0))
        );
        // The table shows the same.
        let out = render_table(&[stale]);
        let cells: Vec<&str> = out.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(cells[6..10], ["5.00", "8.8", "-", "0"], "{out}");
    }

    /// A report inside the warm-up window still has its reservations: they are subtracted from the
    /// headroom it is shown with and picked on.
    #[test]
    fn a_fresh_row_subtracts_the_reservations() {
        // 1.0 measured with 2.0 more reserved; 9000 MB available, 4096 of them reserved.
        let fresh = reported("fresh", 5, capacity(1.0, 9000, 2.0, 4096, json!([])));
        assert_eq!(
            (fresh.load_headroom, fresh.mem_headroom_mb, fresh.warming),
            (Some(3.0), Some(4904), Some(1))
        );
        let alone = |need: Need| pick(std::slice::from_ref(&fresh), need).len();
        assert_eq!(
            alone(Need {
                mb: 4904,
                load: 3.0
            }),
            1,
            "exactly what is left"
        );
        assert_eq!(
            alone(Need {
                mb: 4905,
                load: 0.0
            }),
            0,
            "one MB short"
        );
        assert_eq!(alone(Need { mb: 0, load: 3.5 }), 0, "and one core short");
        // The twin reported 100 s ago is asked for the same and has all of it.
        let stale = reported("stale", 100, capacity(1.0, 9000, 2.0, 4096, json!([])));
        let alone = |need: Need| pick(std::slice::from_ref(&stale), need).len();
        assert_eq!(
            alone(Need {
                mb: 9000,
                load: 5.0
            }),
            1
        );
        assert_eq!(
            alone(Need {
                mb: 9001,
                load: 0.0
            }),
            0
        );
    }

    /// The window is the reporting machine's own, `capacity.warmup_secs`: one set to 600 s still
    /// has its reservations at 61 s, where the default of 60 s would have dropped them. A row that
    /// reports no window -- a build before the field -- is read with the default.
    #[test]
    fn a_row_keeps_its_reservations_for_its_own_warm_up_window() {
        // 1.0 measured with 2.0 more reserved; 9000 MB available, 4096 of them reserved; and the
        // same with 5.5 measured and 1.0 more reserved, which only the reservation puts over 6.
        let reported_capacity = |warmup: Option<u64>, pressed: bool| {
            let mut cap = if pressed {
                capacity(5.5, 9000, 1.0, 0, json!(["load"]))
            } else {
                capacity(1.0, 9000, 2.0, 4096, json!([]))
            };
            if let Some(warmup) = warmup {
                cap["warmup_secs"] = warmup.into();
            }
            cap
        };
        let subtracted = (Some(3.0), Some(4904), Some(1));
        let dropped = (Some(5.0), Some(9000), Some(0));
        let figures = |s: &Summary| (s.load_headroom, s.mem_headroom_mb, s.warming);
        // 61 s: past the default window, inside one of 600 s.
        let long = reported("long", 61, reported_capacity(Some(600), false));
        assert_eq!(figures(&long), subtracted, "{long:?}");
        assert_eq!(
            figures(&reported("short", 61, reported_capacity(Some(60), false))),
            dropped
        );
        assert_eq!(
            figures(&reported("default", 61, reported_capacity(None, false))),
            dropped
        );
        // The window's own end: at 600 s the reservations still count; a second past it they do not.
        assert_eq!(
            figures(&reported(
                "at-end",
                600,
                reported_capacity(Some(600), false)
            )),
            subtracted
        );
        assert_eq!(
            figures(&reported("past", 601, reported_capacity(Some(600), false))),
            dropped
        );
        // Picked on what is left: 4904 MB there, not the 9000 of a row whose window has passed.
        let fits = |s: &Summary, need: Need| pick(std::slice::from_ref(s), need).len();
        assert_eq!(
            fits(
                &long,
                Need {
                    mb: 9000,
                    load: 0.0
                }
            ),
            0
        );
        assert_eq!(
            fits(
                &long,
                Need {
                    mb: 4904,
                    load: 3.0
                }
            ),
            1
        );
        let short = reported("short", 61, reported_capacity(Some(60), false));
        assert_eq!(
            fits(
                &short,
                Need {
                    mb: 9000,
                    load: 5.0
                }
            ),
            1
        );
        // A `load` block that only the reservations cause stays with them.
        let blocked = |age: u64, warmup: Option<u64>| {
            reported("pressed", age, reported_capacity(warmup, true)).blocked
        };
        assert_eq!(blocked(61, Some(600)), ["load"]);
        assert_eq!(blocked(600, Some(600)), ["load"]);
        assert_eq!(blocked(601, Some(600)), Vec::<String>::new());
        assert_eq!(blocked(61, Some(60)), Vec::<String>::new());
        assert_eq!(blocked(61, None), Vec::<String>::new());
        // The table shows the same.
        let cells = |s: &Summary| -> Vec<String> {
            let out = render_table(std::slice::from_ref(s));
            out.lines()
                .nth(1)
                .unwrap()
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(cells(&long)[6..10], ["3.00", "4.8", "-", "1"]);
        assert_eq!(cells(&short)[6..10], ["5.00", "8.8", "-", "0"]);
    }

    /// A `load` in a stale row's `blocked` that only the reservations caused, the measured load
    /// being within the limit, has gone with them. One the measured load causes stays, and so does
    /// every other reason.
    #[test]
    fn a_stale_row_drops_a_load_block_that_only_the_reservations_caused() {
        // 5.5 + 1.0 reserved is over 6; 5.5 alone is not.
        let reserved_only = capacity(5.5, 9000, 1.0, 0, json!(["load"]));
        assert_eq!(
            reported("fresh", 5, reserved_only.clone()).blocked,
            ["load"]
        );
        let stale = reported("stale", 100, reserved_only);
        assert_eq!(stale.blocked, Vec::<String>::new());
        assert_eq!(stale.load_headroom, Some(0.5));
        // Both, each from its own cause: the memory one stays.
        let both = capacity(5.5, 9000, 1.0, 0, json!(["load", "memory"]));
        assert_eq!(reported("stale", 100, both).blocked, ["memory"]);
        // 6.5 measured is over the limit with or without the reservations.
        let measured = capacity(6.5, 9000, 1.0, 0, json!(["load"]));
        assert_eq!(reported("stale", 100, measured).blocked, ["load"]);
        // So the stale row is picked, and a fresh one saying the same is not.
        assert_eq!(
            hosts(&pick(std::slice::from_ref(&stale), Need::NOTHING)),
            ["stale"]
        );
        let fresh = reported("fresh", 5, capacity(5.5, 9000, 1.0, 0, json!(["load"])));
        assert!(pick(std::slice::from_ref(&fresh), Need::NOTHING).is_empty());
    }

    /// `fleet pick --need-load X`: a host must have X cores of load headroom, exactly that much
    /// being enough.
    #[test]
    fn pick_needs_the_load_headroom_asked_for() {
        let rows = [
            reported("room", 5, capacity(1.0, 9000, 0.0, 0, json!([]))), // 5.0 cores
            reported("exact", 5, capacity(4.0, 20000, 0.0, 0, json!([]))), // 2.0 cores
            reported("short", 5, capacity(4.5, 30000, 0.0, 0, json!([]))), // 1.5 cores
        ];
        let listed = |load: f64| -> Vec<String> {
            pick(&rows, Need { mb: 0, load })
                .into_iter()
                .map(|p| p.host)
                .collect()
        };
        assert_eq!(listed(0.0), ["room", "exact", "short"]);
        assert_eq!(listed(1.5), ["room", "exact", "short"]);
        assert_eq!(listed(2.0), ["room", "exact"]);
        assert_eq!(listed(2.5), ["room"]);
        assert!(listed(5.5).is_empty());
    }

    /// Most load headroom first; with the same load headroom, most memory headroom; with both the
    /// same, by name. Rows of every kind are on the one scale.
    #[test]
    fn pick_orders_by_load_headroom_then_memory_then_host() {
        let rows = [
            reported("b-much-memory", 5, capacity(3.0, 20000, 0.0, 0, json!([]))), // 3.0 cores, 20000 MB
            reported("c-most-memory", 5, capacity(3.0, 30000, 0.0, 0, json!([]))), // 3.0 cores, 30000 MB
            reported(
                "a-more-load-room",
                5,
                capacity(1.0, 1000, 0.0, 0, json!([])),
            ), // 5.0 cores, 1000 MB
            reported("d-less-memory", 5, capacity(3.0, 5000, 0.0, 0, json!([]))), // 3.0 cores, 5000 MB
            reported("e-same-as-d", 5, capacity(3.0, 5000, 0.0, 0, json!([]))), // 3.0 cores, 5000 MB
            // Reservations that have expired by now: 5.0 cores and all 5000 MB.
            reported("f-stale", 100, capacity(1.0, 5000, 2.0, 3000, json!([]))),
            // No capacity to report: 6.0 cores, 100 MB.
            unreported("g-old", 5, 0.0, 100),
            // Idle: 5.5 cores and 50000 MB, as last reported.
            unreported("h-idle", 500, 0.5, 50000),
        ];
        let picked = pick(&rows, Need::NOTHING);
        assert_eq!(
            hosts(&picked),
            [
                "g-old",
                "h-idle",
                "f-stale",
                "a-more-load-room",
                "c-most-memory",
                "b-much-memory",
                "d-less-memory",
                "e-same-as-d",
            ]
        );
        let p = |host: &str| picked.iter().find(|p| p.host == host).unwrap();
        assert!(!p("f-stale").idle);
        assert!(p("h-idle").idle);
        assert_eq!(p("g-old").cpus, Some(6));
    }

    /// A host that would be denied a build now is not picked, however much room it has, and
    /// neither is one whose resources could not be read.
    #[test]
    fn pick_never_lists_a_blocked_or_unreadable_host() {
        let rows = [
            reported(
                "pressed",
                5,
                capacity(0.5, 50000, 0.0, 0, json!(["memory"])),
            ),
            reported("loaded", 5, capacity(0.5, 50000, 6.0, 0, json!(["load"]))),
            unreported("over", 5, 6.5, 50000),
            unreported("old-over", 500, 6.5, 50000),
            summarize(&row("unk", 5, Err("x".into())), NOW),
            summarize(&row("unk-old", 500, Err("x".into())), NOW),
            reported("fine", 5, capacity(0.5, 9000, 0.0, 0, json!([]))),
            // Exactly at the limit is allowed.
            unreported("at-limit", 5, 6.0, 9000),
        ];
        assert_eq!(hosts(&pick(&rows, Need::NOTHING)), ["fine", "at-limit"]);
    }

    /// With no capacity either, the top-level `load_1m`, `limits.max_load` and `mem_available_mb`
    /// are what there is to judge by.
    #[test]
    fn rows_without_capacity_are_judged_on_the_top_level_figures() {
        let mut blind = status(None);
        blind["load_1m"] = serde_json::Value::Null;
        let rows = [
            unreported("over", 5, 6.5, 9000),
            unreported("fits", 5, 0.5, 9000),
            unreported("equal", 5, 6.0, 9000),
            unreported("empty", 5, 0.5, 0),
            summarize(&row("blind", 5, Ok(blind)), NOW),
            unreported("old-over", 500, 6.5, 9000),
        ];
        assert_eq!(
            hosts(&pick(&rows, Need { mb: 1, load: 0.0 })),
            ["fits", "equal"]
        );
        assert_eq!(rows[0].blocked, ["load"]);
        assert_eq!(rows[1].blocked, Vec::<String>::new());
        assert_eq!(rows[5].blocked, ["load"]);
    }
}
