//! Reading fleet rows: per-host summary, the human table, and `pick` ordering.
use crate::fleet_store::Row;
use serde::Serialize;

/// A row older than this (seconds) is reported IDLE: machines only report while running gates, so
/// silence means no work. Its resources are the last reported values.
pub const IDLE_SECS: u64 = 180;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Idle,
    Unknown,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Summary {
    pub host: String,
    pub os: String,
    pub state: State,
    pub updated_at: String,
    pub age_secs: u64,
    /// Slots of the build pool in use and in all; None where the machine reports none, as every
    /// build does since the gate stopped counting them. Then `blocked` and the memory and load
    /// below are what there is to judge by.
    pub slots_used: Option<usize>,
    pub slots_total: Option<usize>,
    pub other_slots_used: Option<usize>,
    pub other_slots_total: Option<usize>,
    pub mem_available_mb: Option<u64>,
    /// What is left of that memory after what is reserved for runs still warming up, as the machine
    /// reports it (`capacity.mem_headroom_mb`); None where the machine reports no capacity.
    pub mem_headroom_mb: Option<u64>,
    pub load_1m: Option<f64>,
    pub max_load: Option<f64>,
    /// Why a new build or test run would be denied on the machine now: `capacity.blocked`, or for
    /// a machine that reports no capacity what its measured load alone says. Empty when it would
    /// start.
    pub blocked: Vec<String>,
    pub sessions_busy: usize,
    pub sessions_total: usize,
    pub sessions_unsupported: bool,
    pub compilers: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Pool {
    Gradle,
    Other,
}

/// Memory a new Gradle run is assumed to need, in MB: the gate's default estimate for one test
/// fork (`DEFAULT_RESERVE_GRADLE_MB`).
pub const GRADLE_NEED_MB: u64 = 3072;
/// The same for a run of the other pool that names no toolchain
/// (`DEFAULT_RESERVE_OTHER_MB`).
pub const OTHER_NEED_MB: u64 = 1024;

impl Pool {
    /// Memory a new run of the pool is assumed to need, in MB. Batch B replaces these defaults
    /// with the figure each job brings (`fleet pick --need-mb`).
    pub fn need_mb(self) -> u64 {
        match self {
            Pool::Gradle => GRADLE_NEED_MB,
            Pool::Other => OTHER_NEED_MB,
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Pick {
    pub host: String,
    /// Slots of the pool still free; None where the machine reports no slots.
    pub free_slots: Option<usize>,
    pub mem_available_mb: u64,
    /// The memory a new run can still get, as the machine reports it: `mem_available_mb` minus
    /// what is reserved for runs still warming up. None where the machine reports no capacity;
    /// then `mem_available_mb` is what the host was judged and ordered on.
    pub mem_headroom_mb: Option<u64>,
    pub load_1m: f64,
    pub max_load: f64,
    pub load_ratio: f64,
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

/// Slots of one pool in use and in all, as the machine reported them before the gate stopped
/// counting them; None for one that reports no such pool.
fn slot_counts(status: &serde_json::Value, key: &str) -> Option<(usize, usize)> {
    let slots = status.get(key)?.as_array()?;
    Some((
        slots.iter().filter(|s| s["occupied"] == true).count(),
        slots.len(),
    ))
}

/// Why a new build or test run would be denied on the machine now. A machine that reports
/// `capacity` says so itself; one that reports none is judged on its measured load, all that the
/// top-level figures tell about it.
fn blocked(status: &serde_json::Value, load_1m: Option<f64>, max_load: Option<f64>) -> Vec<String> {
    if let Some(reasons) = status
        .get("capacity")
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
    // Idle: no report means no work, so nothing is in use; totals stay as last reported.
    let (slots_used, slots_total) = slot_counts(st, "slots").unzip();
    let (other_slots_used, other_slots_total) = slot_counts(st, "other_slots").unzip();
    let (load_1m, max_load) = (st["load_1m"].as_f64(), st["limits"]["max_load"].as_f64());
    Summary {
        host: row.host.clone(),
        os: snap.os.clone(),
        state,
        updated_at: row.updated_at.clone(),
        age_secs,
        slots_used: slots_used.map(|used| if idle { 0 } else { used }),
        slots_total,
        other_slots_used: other_slots_used.map(|used| if idle { 0 } else { used }),
        other_slots_total,
        blocked: blocked(st, load_1m, max_load),
        mem_available_mb: st["mem_available_mb"].as_u64(),
        mem_headroom_mb: st["capacity"]["mem_headroom_mb"].as_u64(),
        load_1m,
        max_load,
        sessions_busy: if idle {
            0
        } else {
            snap.sessions.iter().filter(|s| s.cpu_percent > 5.0).count()
        },
        sessions_total: snap.sessions.len(),
        sessions_unsupported: snap.sessions_unsupported,
        compilers: if idle {
            0
        } else {
            st["compiling"].as_array().map_or(0, Vec::len)
        },
    }
}

pub fn render_table(rows: &[Summary]) -> String {
    let mut cells: Vec<[String; 8]> = vec![[
        "HOST",
        "STATE",
        "AGE",
        "SLOTS",
        "MEM_AVAIL_MB",
        "LOAD",
        "CLAUDE(busy/total)",
        "COMPILERS",
    ]
    .map(str::to_owned)];
    let dash = || "-".to_owned();
    for r in rows {
        let readable = r.state != State::Unknown;
        // Idle rows show the last reported memory/load: mark them as old values.
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
            match (readable, r.slots_used.zip(r.slots_total)) {
                (true, Some((used, total))) => format!("{used}/{total}"),
                _ => dash(),
            },
            r.mem_available_mb
                .map_or_else(dash, |m| format!("{m}{old}")),
            match (r.load_1m, r.max_load) {
                (Some(l), Some(m)) => format!("{l:.2}/{m:.2}{old}"),
                _ => dash(),
            },
            if r.sessions_unsupported {
                dash()
            } else {
                format!("{}/{}", r.sessions_busy, r.sessions_total)
            },
            if readable {
                r.compilers.to_string()
            } else {
                dash()
            },
        ]);
    }
    let widths: Vec<usize> = (0..8)
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

/// Whether a new run of `pool` fits on the host. A machine that still reports slots of that pool
/// is judged on them, as before. One that reports none -- every build since the gate stopped
/// counting them -- is judged on its capacity: `blocked` says whether a build would be denied
/// there now, and the memory left after the reservations must cover the run. `blocked` alone does
/// not do: the machine figures it for a run that needs nothing, so a host whose memory is all
/// reserved still reports none, and the next Gradle job sent there is denied `reserved`. With no
/// capacity either, the top-level figures are all there is.
fn fits(r: &Summary, pool: Pool) -> bool {
    let (used, total) = match pool {
        Pool::Gradle => (r.slots_used, r.slots_total),
        Pool::Other => (r.other_slots_used, r.other_slots_total),
    };
    match used.zip(total) {
        Some((used, total)) => total > used,
        None => match r.mem_headroom_mb {
            Some(headroom) => r.blocked.is_empty() && headroom >= pool.need_mb(),
            // An older binary reports no reservations: all that is available is the run's to take.
            None => {
                r.blocked.is_empty() && r.mem_available_mb.is_some_and(|mem| mem >= pool.need_mb())
            }
        },
    }
}

/// Hosts fit for a new heavy job, best first: readable (silent ones count as idle), fit by `fits`,
/// most memory headroom, then lowest load ratio.
pub fn pick(rows: &[Summary], pool: Pool) -> Vec<Pick> {
    let mut fit: Vec<Pick> = rows
        .iter()
        .filter(|r| matches!(r.state, State::Ok | State::Idle))
        .filter(|r| fits(r, pool))
        .filter_map(|r| {
            let (used, total) = match pool {
                Pool::Gradle => (r.slots_used, r.slots_total),
                Pool::Other => (r.other_slots_used, r.other_slots_total),
            };
            let (mem, load, max_load) = (r.mem_available_mb?, r.load_1m?, r.max_load?);
            Some(Pick {
                host: r.host.clone(),
                free_slots: used.zip(total).map(|(used, total)| total - used),
                mem_available_mb: mem,
                mem_headroom_mb: r.mem_headroom_mb,
                load_1m: load,
                max_load,
                load_ratio: if max_load > 0.0 {
                    load / max_load
                } else {
                    f64::MAX
                },
                idle: r.state == State::Idle,
            })
        })
        .collect();
    // Most memory headroom first: what is left after the reservations where the machine reports
    // them, else all that is available, which is all an older binary can tell.
    let headroom = |p: &Pick| p.mem_headroom_mb.unwrap_or(p.mem_available_mb);
    fit.sort_by(|a, b| {
        headroom(b)
            .cmp(&headroom(a))
            .then(a.load_ratio.total_cmp(&b.load_ratio))
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

    /// A status document shaped like `status --json`.
    fn status(mem: u64, load: f64, max_load: f64, used: usize, total: usize) -> serde_json::Value {
        let slots: Vec<_> = (0..total).map(|i| slot(i, i < used)).collect();
        json!({
            "load_1m": load, "mem_available_mb": mem, "swap_used_mb": 0,
            "slots": slots,
            "other_slots": [slot(0, false), slot(1, true)],
            "limits": {"slots": total, "max_load": max_load},
            "compiling": [{"pid": 1, "name": "rustc", "cwd": "/x"}]
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

    #[test]
    fn summary_reads_the_status_document() {
        let s = summarize(&row("nuc", 12, Ok(status(5120, 0.52, 6.0, 1, 3))), NOW);
        assert_eq!(s.host, "nuc");
        assert_eq!(s.state, State::Ok);
        assert_eq!(s.age_secs, 12);
        assert_eq!((s.slots_used, s.slots_total), (Some(1), Some(3)));
        assert_eq!(
            (s.other_slots_used, s.other_slots_total),
            (Some(1), Some(2))
        );
        assert_eq!(s.mem_available_mb, Some(5120));
        assert_eq!(s.load_1m, Some(0.52));
        assert_eq!(s.max_load, Some(6.0));
        // busy means cpu strictly above 5%
        assert_eq!((s.sessions_busy, s.sessions_total), (1, 3));
        assert_eq!(s.compilers, 1);
    }

    #[test]
    fn idle_after_180_seconds_not_at_180() {
        assert_eq!(
            summarize(&row("a", 180, Ok(status(1, 0.0, 1.0, 0, 1))), NOW).state,
            State::Ok
        );
        assert_eq!(
            summarize(&row("a", 181, Ok(status(1, 0.0, 1.0, 0, 1))), NOW).state,
            State::Idle
        );
    }

    #[test]
    fn idle_row_has_no_work_but_keeps_totals_and_last_resources() {
        let s = summarize(&row("mgpu", 500, Ok(status(900, 2.0, 8.0, 3, 3))), NOW);
        assert_eq!(s.state, State::Idle);
        assert_eq!((s.slots_used, s.slots_total), (Some(0), Some(3)));
        assert_eq!(
            (s.other_slots_used, s.other_slots_total),
            (Some(0), Some(2))
        );
        assert_eq!(s.compilers, 0);
        assert_eq!(s.sessions_busy, 0);
        assert_eq!(s.mem_available_mb, Some(900));
        assert_eq!(s.load_1m, Some(2.0));
        assert_eq!(s.max_load, Some(8.0));
        assert_eq!(s.age_secs, 500);
    }

    #[test]
    fn unreadable_resources_are_unknown_even_when_fresh() {
        let s = summarize(&row("mac", 1, Err("no /proc".into())), NOW);
        assert_eq!(s.state, State::Unknown);
        assert_eq!(s.mem_available_mb, None);
        assert_eq!(s.load_1m, None);
    }

    #[test]
    fn unknown_wins_over_idle() {
        assert_eq!(
            summarize(&row("mac", 999, Err("x".into())), NOW).state,
            State::Unknown
        );
    }

    #[test]
    fn table_has_one_header_and_one_line_per_host() {
        let rows = vec![
            summarize(&row("nuc", 12, Ok(status(5120, 0.52, 6.0, 1, 3))), NOW),
            summarize(&row("mgpu", 400, Ok(status(900, 2.0, 8.0, 3, 3))), NOW),
            summarize(&row("mac", 5, Err("x".into())), NOW),
        ];
        let out = render_table(&rows);
        let lines: Vec<Vec<&str>> = out
            .lines()
            .map(|l| l.split_whitespace().collect())
            .collect();
        assert_eq!(lines.len(), 5, "{out}");
        assert_eq!(
            lines[0],
            [
                "HOST",
                "STATE",
                "AGE",
                "SLOTS",
                "MEM_AVAIL_MB",
                "LOAD",
                "CLAUDE(busy/total)",
                "COMPILERS"
            ]
        );
        assert_eq!(
            lines[1],
            ["nuc", "ok", "12s", "1/3", "5120", "0.52/6.00", "1/3", "1"]
        );
        assert_eq!(
            lines[2],
            [
                "mgpu",
                "IDLE",
                "6m",
                "0/3",
                "900*",
                "2.00/8.00*",
                "0/3",
                "0"
            ]
        );
        assert_eq!(
            lines[3],
            ["mac", "UNKNOWN", "5s", "-", "-", "-", "1/3", "-"]
        );
        assert!(lines[4][0].starts_with('*'), "{out}");
        assert!(out.contains("AGE"), "{out}");
    }

    #[test]
    fn table_has_no_footnote_without_idle_rows() {
        let rows = vec![summarize(
            &row("nuc", 12, Ok(status(5120, 0.52, 6.0, 1, 3))),
            NOW,
        )];
        let out = render_table(&rows);
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(!out.contains('*'), "{out}");
    }

    fn pick_rows() -> Vec<Summary> {
        vec![
            summarize(&row("small", 5, Ok(status(2000, 0.1, 6.0, 0, 3))), NOW),
            summarize(&row("big", 5, Ok(status(9000, 3.0, 6.0, 1, 3))), NOW),
            summarize(&row("bigquiet", 5, Ok(status(9000, 0.5, 6.0, 1, 3))), NOW),
            summarize(&row("full", 5, Ok(status(20000, 0.0, 6.0, 3, 3))), NOW),
            summarize(&row("idle", 500, Ok(status(30000, 0.0, 6.0, 3, 3))), NOW),
            summarize(&row("unkold", 500, Err("x".into())), NOW),
            summarize(&row("unk", 5, Err("x".into())), NOW),
        ]
    }

    #[test]
    fn pick_orders_by_memory_then_load_and_drops_unfit_hosts() {
        let order: Vec<_> = pick(&pick_rows(), Pool::Gradle)
            .into_iter()
            .map(|p| p.host)
            .collect();
        assert_eq!(order, ["idle", "bigquiet", "big", "small"]);
    }

    #[test]
    fn pick_reports_free_slots_and_load_ratio() {
        let picks = pick(&pick_rows(), Pool::Gradle);
        let p = picks.iter().find(|p| p.host == "bigquiet").unwrap();
        assert_eq!(p.free_slots, Some(2));
        assert_eq!(p.mem_available_mb, 9000);
        assert!((p.load_ratio - 0.5 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn pick_flags_idle_hosts_so_callers_know_resources_are_old() {
        let picks = pick(&pick_rows(), Pool::Gradle);
        assert!(picks.iter().find(|p| p.host == "idle").unwrap().idle);
        assert!(!picks.iter().find(|p| p.host == "small").unwrap().idle);
        // never an unreadable host, fresh or old
        assert!(picks.iter().all(|p| p.host != "unk" && p.host != "unkold"));
    }

    /// The status document of a machine that reports no slots: `capacity` when it has one, and
    /// otherwise only the top-level figures.
    fn no_slots(capacity: Option<serde_json::Value>) -> serde_json::Value {
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

    fn capacity(blocked: serde_json::Value) -> serde_json::Value {
        json!({
            "cpus": 6, "mem_total_mb": 8000, "max_load": 6.0, "load_1m": 0.5,
            "reserved_load": 2.0, "load_headroom": 3.5, "mem_available_mb": 9000,
            "reserved_mb": 1536, "mem_headroom_mb": 7464, "mem_pressure": 0.1,
            "blocked": blocked, "warming": 1,
        })
    }

    fn blocked_of(summary: &Summary) -> serde_json::Value {
        serde_json::to_value(summary).unwrap()["blocked"].clone()
    }

    /// 4.5: a machine that reports no slots is judged on `capacity.blocked`, under either pool,
    /// never on a slot count of 0/0.
    #[test]
    fn rows_without_slots_are_judged_on_capacity() {
        let s = |host: &str, status: serde_json::Value| summarize(&row(host, 5, Ok(status)), NOW);
        let free = s("small", no_slots(Some(capacity(json!([])))));
        let big = s("big", no_slots(Some(capacity(json!([])))));
        let mut more = capacity(json!(["load"]));
        more["mem_available_mb"] = 50000.into();
        let pressed = s("pressed", no_slots(Some(capacity(json!(["memory"])))));
        let loaded = s("loaded", no_slots(Some(more)));
        let rows = vec![pressed, loaded, free, big];
        for pool in [Pool::Gradle, Pool::Other] {
            let picked: Vec<_> = pick(&rows, pool).into_iter().collect();
            let hosts: Vec<_> = picked.iter().map(|p| p.host.as_str()).collect();
            assert_eq!(hosts, ["big", "small"], "{pool:?}");
            assert!(
                picked
                    .iter()
                    .all(|p| serde_json::to_value(p).unwrap()["free_slots"].is_null()),
                "a machine that reports no slots has no free ones to report"
            );
        }
        let by_host = |host: &str| rows.iter().find(|r| r.host == host).unwrap();
        assert_eq!(blocked_of(by_host("small")), json!([]));
        assert_eq!(blocked_of(by_host("pressed")), json!(["memory"]));
        assert_eq!(blocked_of(by_host("loaded")), json!(["load"]));
        let json = serde_json::to_value(by_host("small")).unwrap();
        for key in [
            "slots_used",
            "slots_total",
            "other_slots_used",
            "other_slots_total",
        ] {
            assert_eq!(json[key], serde_json::Value::Null, "{key}");
        }
        assert_eq!(json["mem_available_mb"], 9000);
        assert_eq!(json["load_1m"], 0.5);
        assert_eq!(json["max_load"], 6.0);
    }

    /// 4.5: with neither slots nor capacity, the top-level `load_1m`, `limits.max_load` and
    /// `mem_available_mb` are what there is to judge by.
    #[test]
    fn rows_without_slots_or_capacity_are_judged_on_the_top_level_figures() {
        let s = |host: &str, status: serde_json::Value| summarize(&row(host, 5, Ok(status)), NOW);
        let mut over = no_slots(None);
        over["load_1m"] = 6.01.into();
        let mut equal = no_slots(None);
        equal["load_1m"] = 6.0.into();
        let mut empty = no_slots(None);
        empty["mem_available_mb"] = 0.into();
        let mut unknown_load = no_slots(None);
        unknown_load["load_1m"] = serde_json::Value::Null;
        let idle = summarize(&row("old", 500, Ok(over.clone())), NOW);
        let rows = vec![
            s("over", over),
            s("fits", no_slots(None)),
            s("equal", equal),
            s("empty", empty),
            s("unknown", unknown_load),
            idle,
        ];
        let hosts: Vec<_> = pick(&rows, Pool::Gradle)
            .into_iter()
            .map(|p| p.host)
            .collect();
        // Both fit: 9000MB each, so the one with the lower load ratio is first.
        assert_eq!(hosts, ["fits", "equal"]);
        assert_eq!(blocked_of(&rows[0]), json!(["load"]));
        assert_eq!(blocked_of(&rows[1]), json!([]));
    }

    /// A capacity of a machine with `mem` MB available, `reserved_mb` of them held for runs still
    /// warming up, and nothing else the matter.
    fn reserved_capacity(mem: u64, reserved_mb: u64) -> serde_json::Value {
        let mut capacity = capacity(json!([]));
        capacity["mem_available_mb"] = mem.into();
        capacity["reserved_mb"] = reserved_mb.into();
        capacity["mem_headroom_mb"] = mem.saturating_sub(reserved_mb).into();
        capacity
    }

    /// A machine that reports no slots, `mem` MB available and a load of `load`, with that
    /// capacity when it has one.
    fn machine(host: &str, mem: u64, load: f64, capacity: Option<serde_json::Value>) -> Summary {
        let mut status = no_slots(capacity);
        status["mem_available_mb"] = mem.into();
        status["load_1m"] = load.into();
        if status["capacity"].is_object() {
            status["capacity"]["load_1m"] = load.into();
        }
        summarize(&row(host, 5, Ok(status)), NOW)
    }

    /// A host whose memory headroom is used up by reservations does not fit, though `blocked` is
    /// empty: the machine figures that for a run that needs nothing. What is left after the
    /// reservations must cover a run of the pool.
    #[test]
    fn rows_without_slots_need_memory_headroom_for_a_run_of_the_pool() {
        let used_up = machine("used-up", 4000, 0.5, Some(reserved_capacity(4000, 4096)));
        assert_eq!(used_up.blocked, Vec::<String>::new());
        assert_eq!(used_up.mem_available_mb, Some(4000));
        assert_eq!(used_up.mem_headroom_mb, Some(0));
        let room = machine("room", 4000, 0.5, Some(reserved_capacity(4000, 0)));
        assert_eq!(room.mem_headroom_mb, Some(4000));
        for pool in [Pool::Gradle, Pool::Other] {
            let picks = pick(&[used_up.clone(), room.clone()], pool);
            let hosts: Vec<_> = picks.iter().map(|p| p.host.as_str()).collect();
            assert_eq!(hosts, ["room"], "{pool:?}");
            assert_eq!(picks[0].mem_headroom_mb, Some(4000));
        }
        // One MB short of what a Gradle run needs is not enough; exactly that much is.
        let short = machine("short", 9000, 0.5, Some(reserved_capacity(9000, 5929)));
        assert!(pick(std::slice::from_ref(&short), Pool::Gradle).is_empty());
        let exact = machine("exact", 9000, 0.5, Some(reserved_capacity(9000, 5928)));
        assert_eq!(pick(std::slice::from_ref(&exact), Pool::Gradle).len(), 1);
    }

    /// A binary older than `capacity` reports neither it nor slots: its top-level memory must
    /// cover a run of the pool, where any at all used to do.
    #[test]
    fn rows_without_capacity_need_a_pools_worth_of_memory_available() {
        let old = machine("old", 2000, 0.5, None);
        assert_eq!(old.mem_headroom_mb, None);
        assert!(
            pick(std::slice::from_ref(&old), Pool::Gradle).is_empty(),
            "2000 MB is less than a Gradle run needs"
        );
        assert_eq!(
            pick(std::slice::from_ref(&old), Pool::Other).len(),
            1,
            "and more than one of the other pool"
        );
        let gradle = machine("gradle", GRADLE_NEED_MB, 0.5, None);
        assert_eq!(pick(std::slice::from_ref(&gradle), Pool::Gradle).len(), 1);
    }

    /// Fit hosts have the most memory headroom first -- what is left after the reservations where
    /// the machine reports them, else all that is available -- then the lowest load ratio.
    #[test]
    fn pick_orders_by_memory_headroom_then_load() {
        // 9200 MB available, 6000 of them reserved: the most available, the least left.
        let rows = [
            machine("booked", 9200, 0.1, Some(reserved_capacity(9200, 6000))),
            machine("tie", 5000, 0.2, Some(reserved_capacity(5000, 1000))),
            machine("room", 4000, 0.5, Some(reserved_capacity(4000, 0))),
            machine("old", 3500, 0.0, None),
        ];
        let hosts: Vec<_> = pick(&rows, Pool::Gradle)
            .into_iter()
            .map(|p| p.host)
            .collect();
        assert_eq!(hosts, ["tie", "room", "old", "booked"]);
    }

    /// A machine that reports slots is judged on them as before, whatever its capacity says.
    #[test]
    fn rows_with_slots_keep_their_rule() {
        // Build slots 3/3, other slots 1/2: the build pool is full, the other one is not.
        let mut full_status = status(9000, 0.1, 6.0, 3, 3);
        full_status["capacity"] = capacity(json!([]));
        let full = summarize(&row("full", 5, Ok(full_status)), NOW);
        let alone = |s: &Summary, pool: Pool| pick(std::slice::from_ref(s), pool);
        assert!(alone(&full, Pool::Gradle).is_empty());
        assert_eq!(alone(&full, Pool::Other).len(), 1);
        // Free slots, and a capacity that says a build would be denied: the slots decide.
        let mut free_status = status(9000, 0.1, 6.0, 0, 3);
        free_status["capacity"] = capacity(json!(["load"]));
        let free_but_blocked = summarize(&row("fb", 5, Ok(free_status)), NOW);
        assert_eq!(alone(&free_but_blocked, Pool::Gradle).len(), 1);
        assert_eq!(alone(&free_but_blocked, Pool::Other).len(), 1);
    }

    #[test]
    fn table_shows_a_dash_where_the_machine_reports_no_slots() {
        let rows = vec![
            summarize(&row("nuc", 12, Ok(status(5120, 0.52, 6.0, 1, 3))), NOW),
            summarize(
                &row("gpu", 12, Ok(no_slots(Some(capacity(json!([])))))),
                NOW,
            ),
        ];
        let out = render_table(&rows);
        let lines: Vec<Vec<&str>> = out
            .lines()
            .map(|l| l.split_whitespace().collect())
            .collect();
        assert_eq!(
            lines[1],
            ["nuc", "ok", "12s", "1/3", "5120", "0.52/6.00", "1/3", "1"]
        );
        assert_eq!(
            lines[2],
            ["gpu", "ok", "12s", "-", "9000", "0.50/6.00", "1/3", "0"]
        );
    }

    #[test]
    fn pick_other_pool_uses_other_slots() {
        // Every fixture has other_slots 1/2 used, so each fresh readable host has one free.
        let hosts: Vec<_> = pick(&pick_rows(), Pool::Other)
            .into_iter()
            .map(|p| p.host)
            .collect();
        assert_eq!(hosts, ["idle", "full", "bigquiet", "big", "small"]);
    }
}
