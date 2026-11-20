//! Reading fleet rows: per-host summary, the human table, and `pick` ordering.
use crate::fleet_store::Row;
use serde::Serialize;

/// A row older than this (seconds) is reported STALE and never picked.
pub const STALE_SECS: u64 = 180;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Stale,
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
}

/// Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ`; None for anything else.
pub fn parse_rfc3339_utc(_s: &str) -> Option<u64> {
    None
}

/// `12s`, `5m`, `3h`, `2d`.
pub fn format_age(_secs: u64) -> String {
    String::new()
}

pub fn summarize(_row: &Row, _now: u64) -> Summary {
    Summary {
        host: String::new(),
        os: String::new(),
        state: State::Unknown,
        updated_at: String::new(),
        age_secs: 0,
        slots_used: 0,
        slots_total: 0,
        other_slots_used: 0,
        other_slots_total: 0,
        mem_available_mb: None,
        load_1m: None,
        max_load: None,
        sessions_busy: 0,
        sessions_total: 0,
        sessions_unsupported: false,
        compilers: 0,
    }
}

pub fn render_table(_rows: &[Summary]) -> String {
    String::new()
}

/// Hosts fit for a new heavy job, best first: fresh, readable, a free slot in the pool, most
/// MemAvailable, then lowest load ratio.
pub fn pick(_rows: &[Summary], _pool: Pool) -> Vec<Pick> {
    Vec::new()
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
    fn stale_after_180_seconds_not_at_180() {
        assert_eq!(
            summarize(&row("a", 180, Ok(status(1, 0.0, 1.0, 0, 1))), NOW).state,
            State::Ok
        );
        assert_eq!(
            summarize(&row("a", 181, Ok(status(1, 0.0, 1.0, 0, 1))), NOW).state,
            State::Stale
        );
    }

    #[test]
    fn unreadable_resources_are_unknown_even_when_fresh() {
        let s = summarize(&row("mac", 1, Err("no /proc".into())), NOW);
        assert_eq!(s.state, State::Unknown);
        assert_eq!(s.mem_available_mb, None);
        assert_eq!(s.load_1m, None);
    }

    #[test]
    fn unknown_wins_over_stale() {
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
        assert_eq!(lines.len(), 4, "{out}");
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
            ["mgpu", "STALE", "6m", "3/3", "900", "2.00/8.00", "1/3", "1"]
        );
        assert_eq!(
            lines[3],
            ["mac", "UNKNOWN", "5s", "-", "-", "-", "1/3", "-"]
        );
    }

    fn pick_rows() -> Vec<Summary> {
        vec![
            summarize(&row("small", 5, Ok(status(2000, 0.1, 6.0, 0, 3))), NOW),
            summarize(&row("big", 5, Ok(status(9000, 3.0, 6.0, 1, 3))), NOW),
            summarize(&row("bigquiet", 5, Ok(status(9000, 0.5, 6.0, 1, 3))), NOW),
            summarize(&row("full", 5, Ok(status(20000, 0.0, 6.0, 3, 3))), NOW),
            summarize(&row("stale", 500, Ok(status(30000, 0.0, 6.0, 0, 3))), NOW),
            summarize(&row("unk", 5, Err("x".into())), NOW),
        ]
    }

    #[test]
    fn pick_orders_by_memory_then_load_and_drops_unfit_hosts() {
        let order: Vec<_> = pick(&pick_rows(), Pool::Gradle)
            .into_iter()
            .map(|p| p.host)
            .collect();
        assert_eq!(order, ["bigquiet", "big", "small"]);
    }

    #[test]
    fn pick_reports_free_slots_and_load_ratio() {
        let p = &pick(&pick_rows(), Pool::Gradle)[0];
        assert_eq!(p.free_slots, 2);
        assert_eq!(p.mem_available_mb, 9000);
        assert!((p.load_ratio - 0.5 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn pick_other_pool_uses_other_slots() {
        // Every fixture has other_slots 1/2 used, so each fresh readable host has one free.
        let hosts: Vec<_> = pick(&pick_rows(), Pool::Other)
            .into_iter()
            .map(|p| p.host)
            .collect();
        assert_eq!(hosts, ["full", "bigquiet", "big", "small"]);
    }
}
