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
    pub slots_used: usize,
    pub slots_total: usize,
    pub other_slots_used: usize,
    pub other_slots_total: usize,
    pub mem_available_mb: Option<u64>,
    pub load_1m: Option<f64>,
    pub max_load: Option<f64>,
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

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Pick {
    pub host: String,
    pub free_slots: usize,
    pub mem_available_mb: u64,
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

fn slot_counts(status: &serde_json::Value, key: &str) -> (usize, usize) {
    let slots = status[key].as_array().map_or(&[][..], Vec::as_slice);
    (
        slots.iter().filter(|s| s["occupied"] == true).count(),
        slots.len(),
    )
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
    let (mut slots_used, slots_total) = slot_counts(st, "slots");
    let (mut other_slots_used, other_slots_total) = slot_counts(st, "other_slots");
    if idle {
        slots_used = 0;
        other_slots_used = 0;
    }
    Summary {
        host: row.host.clone(),
        os: snap.os.clone(),
        state,
        updated_at: row.updated_at.clone(),
        age_secs,
        slots_used,
        slots_total,
        other_slots_used,
        other_slots_total,
        mem_available_mb: st["mem_available_mb"].as_u64(),
        load_1m: st["load_1m"].as_f64(),
        max_load: st["limits"]["max_load"].as_f64(),
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
            if readable {
                format!("{}/{}", r.slots_used, r.slots_total)
            } else {
                dash()
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

/// Hosts fit for a new heavy job, best first: readable (silent ones count as idle), a free slot in the pool, most
/// MemAvailable, then lowest load ratio.
pub fn pick(rows: &[Summary], pool: Pool) -> Vec<Pick> {
    let mut fit: Vec<Pick> = rows
        .iter()
        .filter(|r| matches!(r.state, State::Ok | State::Idle))
        .filter_map(|r| {
            let (used, total) = match pool {
                Pool::Gradle => (r.slots_used, r.slots_total),
                Pool::Other => (r.other_slots_used, r.other_slots_total),
            };
            let (mem, load, max_load) = (r.mem_available_mb?, r.load_1m?, r.max_load?);
            (total > used).then(|| Pick {
                host: r.host.clone(),
                free_slots: total - used,
                mem_available_mb: mem,
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
    fit.sort_by(|a, b| {
        b.mem_available_mb
            .cmp(&a.mem_available_mb)
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
        assert_eq!((s.slots_used, s.slots_total), (1, 3));
        assert_eq!((s.other_slots_used, s.other_slots_total), (1, 2));
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
        assert_eq!((s.slots_used, s.slots_total), (0, 3));
        assert_eq!((s.other_slots_used, s.other_slots_total), (0, 2));
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
        assert_eq!(p.free_slots, 2);
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
