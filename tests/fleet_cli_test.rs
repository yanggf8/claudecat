//! `claudecat fleet ...` end to end against a local stub of Turso's HTTP pipeline endpoint.
use claudecat::fleet::assemble;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>, // (authorization header, body)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The status document of a build older than `capacity` and `holders`: its slot pools, and the
/// limits such a build reported.
fn status(mem: u64, load: f64, used: usize) -> Value {
    let slots: Vec<Value> = (0..3)
        .map(|i| json!({"index": i, "holder": null, "occupied": i < used}))
        .collect();
    json!({"load_1m": load, "mem_available_mb": mem, "swap_used_mb": 0, "slots": slots,
           "other_slots": [], "compiling": [],
           "limits": {"slots": 3, "other_slots": 0, "cpus": 6, "mem_total_mb": 8000, "max_load": 6.0}})
}

/// The status document of a machine that reports `capacity` and no slots (every build with the
/// load-only gate): 6 CPUs, 8000 MB, `max_load`, nothing reserved, and `blocked` as given.
fn new_status(host_load: f64, mem: u64, max_load: f64, blocked: &[&str]) -> Value {
    json!({
        "load_1m": host_load, "mem_available_mb": mem, "swap_used_mb": 0, "mem_pressure_some_avg10": 0.1,
        "holders": [], "compiling": [],
        "limits": {"cpus": 6, "mem_total_mb": 8000, "max_load": max_load, "max_load_basis": "cpus 6x1"},
        "capacity": {
            "cpus": 6, "mem_total_mb": 8000, "max_load": max_load, "load_1m": host_load,
            "reserved_load": 0.0, "load_headroom": max_load - host_load, "mem_available_mb": mem,
            "reserved_mb": 0, "mem_headroom_mb": mem, "mem_pressure": 0.1,
            "blocked": blocked, "warming": 0,
        }
    })
}

/// The same with `reserved_mb` held for `warming` runs still warming up, as the machine reports it
/// while they are.
fn reserved(mut status: Value, reserved_mb: u64, warming: usize) -> Value {
    let mem = status["capacity"]["mem_available_mb"].as_u64().unwrap();
    status["capacity"]["reserved_mb"] = reserved_mb.into();
    status["capacity"]["mem_headroom_mb"] = mem.saturating_sub(reserved_mb).into();
    status["capacity"]["warming"] = warming.into();
    status
}

fn db_row(host: &str, age: u64, st: Result<Value, String>) -> Vec<Value> {
    let snap = assemble(host, "linux", now() - age, st, Some(vec![]));
    vec![
        json!({"type":"text","value":host}),
        json!({"type":"text","value":snap.collected_at}),
        json!({"type":"text","value":serde_json::to_string(&snap).unwrap()}),
    ]
}

/// Serve the pipeline endpoint on 127.0.0.1: SELECTs get `rows`, anything else succeeds,
/// unless `status_code` says to fail every call.
fn stub(rows: Vec<Vec<Value>>, status_code: u16) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head_end, len) = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (i + 4, len);
                }
                if n == 0 {
                    return;
                }
            };
            while buf.len() < head_end + len {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let auth = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("authorization: ")
                        .or_else(|| l.strip_prefix("Authorization: "))
                })
                .unwrap_or("")
                .to_owned();
            let body: Value =
                serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
            let is_select = body.to_string().contains("SELECT host");
            seen.lock().unwrap().push((auth, body));
            let reply = if is_select {
                json!({"results":[
                    {"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":[]}}},
                    {"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":rows}}},
                    {"type":"ok","response":{"type":"close"}}]})
            } else {
                json!({"results":[
                    {"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":[]}}},
                    {"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":[]}}},
                    {"type":"ok","response":{"type":"close"}}]})
            }
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status_code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    Stub { url, requests }
}

fn run(stub: &Stub, args: &[&str]) -> Output {
    let t = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .args(args)
        .env("CLAUDECAT_FLEET_URL", &stub.url)
        .env("CLAUDECAT_FLEET_TOKEN", "TOPSECRET-TOKEN")
        .env("CLAUDECAT_DATA_DIR", t.path())
        .env("CLAUDECAT_GATE_DIR", t.path().join("gate"))
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn no_config_prints_the_one_step_fix_and_exits_1() {
    let t = tempfile::tempdir().unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .args(["fleet"])
        .env_remove("CLAUDECAT_FLEET_URL")
        .env_remove("CLAUDECAT_FLEET_TOKEN")
        .env("CORT_MEMORY_DIR", t.path())
        .env("HOME", t.path())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        text(&o.stderr).trim(),
        "fleet: no Turso config; run `cort memory configure --url URL --token-stdin` or set CLAUDECAT_FLEET_URL and CLAUDECAT_FLEET_TOKEN"
    );
}

#[test]
fn push_upserts_this_host_with_a_bearer_token_and_prints_no_secret() {
    let s = stub(vec![], 200);
    let o = run(&s, &["fleet", "push"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let reqs = s.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].0, "Bearer TOPSECRET-TOKEN");
    let upsert = &reqs[0].1["requests"][1]["stmt"];
    assert!(upsert["sql"]
        .as_str()
        .unwrap()
        .starts_with("INSERT INTO claudecat_fleet"));
    let payload: Value =
        serde_json::from_str(upsert["args"][2]["value"].as_str().unwrap()).unwrap();
    assert_eq!(payload["host"], upsert["args"][0]["value"]);
    assert_eq!(payload["claudecat_version"], env!("CARGO_PKG_VERSION"));
    assert!(!text(&o.stdout).contains("TOPSECRET"));
    assert!(!text(&o.stderr).contains("TOPSECRET"));
}

#[test]
fn push_failure_is_one_error_line_and_nonzero() {
    let s = stub(vec![], 500);
    let o = run(&s, &["fleet", "push"]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert_eq!(err.trim().lines().count(), 1, "{err}");
    assert!(err.starts_with("fleet push: "), "{err}");
    assert!(!err.contains("TOPSECRET"));
}

/// Run `fleet` or `fleet pick` and parse its JSON; it must have exited 0 and said nothing.
fn json_of(s: &Stub, args: &[&str]) -> Value {
    let o = run(s, args);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(text(&o.stderr), "", "nothing to say without --kind");
    serde_json::from_slice(&o.stdout).unwrap()
}

fn hosts(v: &Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r["host"].as_str().unwrap())
        .collect()
}

fn sorted_keys(v: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

#[test]
fn fleet_json_lists_every_host_with_its_state() {
    let s = stub(
        vec![
            db_row("nuc", 10, Ok(status(5000, 0.5, 1))),
            db_row("old", 600, Ok(status(9000, 0.25, 0))),
            db_row("mac", 10, Err("cannot read".into())),
        ],
        200,
    );
    let v = json_of(&s, &["fleet", "--json"]);
    let states: Vec<(&str, &str)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["host"].as_str().unwrap(), r["state"].as_str().unwrap()))
        .collect();
    assert_eq!(states, [("nuc", "ok"), ("old", "idle"), ("mac", "unknown")]);
    assert_eq!(v[0]["mem_available_mb"], 5000);
    assert_eq!(v[0]["load_headroom"], 5.5);
}

/// `fleet --json` is what a coordinator reads by program: the capacity of each host in figures
/// every build reports the same way, and no slot count from any build. Who runs is two figures,
/// `holders` (the `gate run` registrations: one job each, whatever the number of compilers it has
/// running) and `compiling` (the compiler processes).
#[test]
fn fleet_json_reports_the_capacity_of_each_host_and_no_slot_count() {
    let mut gpu = new_status(0.5, 9000, 16.0, &["memory"]);
    gpu["holders"] = json!([
        {"pid": 1, "kind": "build", "command": ["cargo", "test"], "started_at": 1},
        {"pid": 2, "kind": "other", "command": ["sleep", "1"], "started_at": 1},
    ]);
    gpu["compiling"] = json!([
        {"pid": 3, "name": "rustc", "cwd": "/x"},
        {"pid": 4, "name": "rustc", "cwd": "/x"},
        {"pid": 5, "name": "rustc", "cwd": "/x"},
    ]);
    let s = stub(
        vec![
            db_row("gpu", 10, Ok(gpu)),
            db_row("nuc", 10, Ok(status(5000, 0.5, 1))),
        ],
        200,
    );
    let v = json_of(&s, &["fleet", "--json"]);
    let gpu = &v[0];
    assert_eq!(
        sorted_keys(gpu),
        [
            "age_secs",
            "blocked",
            "compiling",
            "cpus",
            "holders",
            "host",
            "load_1m",
            "load_headroom",
            "max_load",
            "mem_available_mb",
            "mem_headroom_mb",
            "mem_total_mb",
            "os",
            "sessions_busy",
            "sessions_total",
            "sessions_unsupported",
            "state",
            "updated_at",
            "warming",
        ],
        "{gpu}"
    );
    for (key, value) in [
        ("cpus", json!(6)),
        ("mem_total_mb", json!(8000)),
        ("load_1m", json!(0.5)),
        ("max_load", json!(16.0)),
        ("load_headroom", json!(15.5)),
        ("mem_available_mb", json!(9000)),
        ("mem_headroom_mb", json!(9000)),
        ("blocked", json!(["memory"])),
        ("warming", json!(0)),
        ("holders", json!(2)),
        ("compiling", json!(3)),
    ] {
        assert_eq!(gpu[key], value, "{key}");
    }
    assert!(gpu.get("running").is_none(), "one figure is gone: {gpu}");
    // An old row is judged on its top-level figures; it tells nothing of what it never reported
    // (capacity, holders), and what compilers it had running.
    let nuc = &v[1];
    for (key, value) in [
        ("cpus", json!(6)),
        ("mem_total_mb", json!(8000)),
        ("load_headroom", json!(5.5)),
        ("mem_headroom_mb", json!(5000)),
        ("blocked", json!([])),
        ("warming", Value::Null),
        ("holders", Value::Null),
        ("compiling", json!(0)),
    ] {
        assert_eq!(nuc[key], value, "{key}");
    }
    assert!(!v.to_string().contains("slot"), "{v}");
}

/// A machine past the IDLE threshold has stopped reporting: whether anything runs on it now is
/// not to be known, though two holders and a compiler were running when it last said. The JSON
/// says null and the table a dash; neither claims 0.
#[test]
fn an_idle_machine_does_not_report_that_nothing_runs() {
    let mut quiet = new_status(0.5, 9000, 6.0, &[]);
    quiet["holders"] = json!([
        {"pid": 1, "kind": "build", "command": ["cargo", "test"], "started_at": 1},
        {"pid": 2, "kind": "other", "command": ["sleep", "1"], "started_at": 1},
    ]);
    quiet["compiling"] = json!([{"pid": 3, "name": "rustc", "cwd": "/x"}]);
    let s = stub(vec![db_row("quiet", 600, Ok(quiet))], 200);
    let v = json_of(&s, &["fleet", "--json"]);
    assert_eq!(v[0]["state"], "idle");
    assert_eq!(
        (v[0]["holders"].clone(), v[0]["compiling"].clone()),
        (Value::Null, Value::Null),
        "{}",
        v[0]
    );
    assert!(v[0].get("running").is_none(), "{}", v[0]);
    let o = run(&s, &["fleet"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    let cells: Vec<&str> = out.lines().nth(1).unwrap().split_whitespace().collect();
    assert_eq!(cells[..2], ["quiet", "IDLE"], "{out}");
    assert_eq!(cells[10..12], ["-", "-"], "{out}");
}

/// The reservations of a row are kept until the reporting machine's own warm-up window has
/// passed -- `capacity.warmup_secs` -- and not the default: a machine set to 600 s is still read
/// with them at 100 s, where a row that names 60 s, or says nothing of the window, is not.
#[test]
fn a_row_keeps_its_reservations_for_the_reporting_machines_warm_up_window() {
    let booked = |warmup: Option<u64>| {
        let mut st = reserved(new_status(0.5, 4000, 6.0, &[]), 4096, 1);
        if let Some(warmup) = warmup {
            st["capacity"]["warmup_secs"] = warmup.into();
        }
        st
    };
    let s = stub(
        vec![
            db_row("long", 100, Ok(booked(Some(600)))),
            db_row("short", 100, Ok(booked(Some(60)))),
            db_row("unsaid", 100, Ok(booked(None))),
        ],
        200,
    );
    let v = json_of(&s, &["fleet", "--json"]);
    let row = |host: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|r| r["host"] == host)
            .unwrap()
    };
    // 4000 MB available, 4096 of them reserved: nothing is left, and a run is warming up.
    let long = row("long");
    assert_eq!(
        (long["mem_headroom_mb"].clone(), long["warming"].clone()),
        (json!(0), json!(1)),
        "{long}"
    );
    for host in ["short", "unsaid"] {
        let r = row(host);
        assert_eq!(
            (r["mem_headroom_mb"].clone(), r["warming"].clone()),
            (json!(4000), json!(0)),
            "{host}: {r}"
        );
    }
    // Picked on what is left: a job needing all 4000 MB fits the two whose window has passed.
    assert_eq!(
        hosts(&json_of(
            &s,
            &["fleet", "pick", "--need-mb", "4000", "--json"]
        )),
        ["short", "unsaid"]
    );
}

#[test]
fn fleet_table_marks_idle_and_unknown() {
    let s = stub(
        vec![
            db_row("old", 600, Ok(status(9000, 0.25, 0))),
            db_row("mac", 10, Err("x".into())),
        ],
        200,
    );
    let o = run(&s, &["fleet"]);
    let out = text(&o.stdout);
    assert!(out.lines().next().unwrap().starts_with("HOST"), "{out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("old") && l.contains("IDLE") && l.contains("8.8*")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("mac") && l.contains("UNKNOWN")),
        "{out}"
    );
}

/// One row per host with what the machine has and what is left of it. A row of an old build is
/// read on its top-level figures and shows a dash for what that build never reported.
#[test]
fn fleet_table_lists_the_capacity_of_every_kind_of_row() {
    let mut gpu = new_status(0.5, 9000, 16.0, &["memory"]);
    gpu["holders"] = json!([
        {"pid": 1, "kind": "build", "command": ["cargo", "test"], "started_at": 1},
        {"pid": 2, "kind": "other", "command": ["sleep", "1"], "started_at": 1},
    ]);
    gpu["compiling"] = json!([{"pid": 3, "name": "rustc", "cwd": "/x"}]);
    gpu["capacity"]["warming"] = 3.into();
    let s = stub(
        vec![
            db_row("gpu", 10, Ok(gpu)),
            db_row("nuc", 10, Ok(status(5000, 0.5, 1))),
            db_row("old", 600, Ok(status(9000, 0.25, 0))),
            db_row("mac", 10, Err("x".into())),
        ],
        200,
    );
    let o = run(&s, &["fleet"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    let lines: Vec<Vec<&str>> = out
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(
        lines[0],
        [
            "HOST", "STATE", "AGE", "CPUS", "MEM_GB", "LOAD", "HEADROOM", "FREE_GB", "BLOCKED",
            "WARM", "RUN", "COMP", "SESS",
        ],
        "{out}"
    );
    // The age is 10s or 11s, by when the clock last turned over: not what this is about.
    let row = |i: usize| {
        let mut cells = lines[i].clone();
        cells.remove(2);
        cells
    };
    // A build with capacity: headroom after the reservations (none here), who is warming up, the
    // two `gate run` holders and the one compiler, each to its own column.
    assert_eq!(
        row(1),
        [
            "gpu",
            "ok",
            "6",
            "7.8",
            "0.50/16.00",
            "15.50",
            "8.8",
            "memory",
            "3",
            "2",
            "1",
            "0"
        ],
        "{out}"
    );
    // A build that still counted slots: CPU, memory, load and what is left of them from its
    // top-level figures; a dash for what it never reported (capacity, holders), and its compilers,
    // none here.
    assert_eq!(
        row(2),
        [
            "nuc",
            "ok",
            "6",
            "7.8",
            "0.50/6.00",
            "5.50",
            "4.9",
            "-",
            "-",
            "-",
            "0",
            "0"
        ],
        "{out}"
    );
    // The same row when old: the last reported figures, marked as such.
    assert_eq!(
        row(3),
        [
            "old",
            "IDLE",
            "6",
            "7.8",
            "0.25/6.00*",
            "5.75*",
            "8.8*",
            "-",
            "-",
            "-",
            "-",
            "0",
        ],
        "{out}"
    );
    assert_eq!(
        row(4),
        ["mac", "UNKNOWN", "-", "-", "-", "-", "-", "-", "-", "-", "-", "0",],
        "{out}"
    );
    assert!(lines[5][0].starts_with('*'), "{out}");
}

/// Most load headroom first, then most memory. A row of any build counts, an IDLE one too with its
/// figures flagged old, and an unreadable one never.
#[test]
fn pick_json_orders_by_load_headroom_includes_idle_and_skips_unknown() {
    let s = stub(
        vec![
            db_row("small", 10, Ok(status(2000, 0.25, 0))),
            db_row("big", 10, Ok(status(9000, 0.25, 2))),
            db_row("full", 10, Ok(status(30000, 0.0, 3))),
            db_row("old", 600, Ok(status(40000, 0.0, 0))),
            db_row("mac", 600, Err("x".into())),
        ],
        200,
    );
    let v = json_of(&s, &["fleet", "pick", "--json"]);
    assert_eq!(hosts(&v), ["old", "full", "big", "small"]);
    assert_eq!(v[0]["idle"], true);
    assert_eq!(v[1]["idle"], false);
    // Per host: what is left of its load and memory, the totals they are a share of, and why it
    // would be blocked, which is never, or it would not be listed.
    assert_eq!(
        sorted_keys(&v[1]),
        [
            "blocked",
            "cpus",
            "host",
            "idle",
            "load_1m",
            "load_headroom",
            "max_load",
            "mem_headroom_mb",
            "mem_total_mb",
        ],
        "{}",
        v[1]
    );
    let full = &v[1];
    for (key, value) in [
        ("load_headroom", json!(6.0)),
        ("mem_headroom_mb", json!(30000)),
        ("cpus", json!(6)),
        ("mem_total_mb", json!(8000)),
        ("load_1m", json!(0.0)),
        ("max_load", json!(6.0)),
        ("blocked", json!([])),
    ] {
        assert_eq!(full[key], value, "{key}");
    }
    assert!(!v.to_string().contains("slot"), "{v}");
    // The same in text, one line per host, best first.
    let o = run(&s, &["fleet", "pick"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        [
            "old load_headroom=6.00 mem_headroom_mb=40000 load=0.00/6.00",
            "full load_headroom=6.00 mem_headroom_mb=30000 load=0.00/6.00",
            "big load_headroom=5.75 mem_headroom_mb=9000 load=0.25/6.00",
            "small load_headroom=5.75 mem_headroom_mb=2000 load=0.25/6.00",
        ]
    );
}

#[test]
fn pick_with_no_room_exits_1_with_a_reason() {
    let s = stub(
        vec![
            db_row("over", 10, Ok(status(30000, 6.5, 0))),
            db_row("blocked", 10, Ok(new_status(0.5, 30000, 6.0, &["memory"]))),
        ],
        200,
    );
    let o = run(&s, &["fleet", "pick"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        text(&o.stderr).trim(),
        "fleet pick: no eligible host (every host is unknown, blocked, or has less memory or load headroom than asked for)"
    );
    // The same when the only host has room, only not as much as asked.
    let s = stub(
        vec![db_row("small", 10, Ok(new_status(0.5, 2000, 6.0, &[])))],
        200,
    );
    let o = run(
        &s,
        &["fleet", "pick", "--need-mb", "4000", "--need-load", "8"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        text(&o.stderr).trim(),
        "fleet pick: no eligible host (every host is unknown, blocked, or has less memory or load headroom than asked for)"
    );
}

/// Slots are nobody's criterion: a row of a build that still counts them is picked like any other,
/// whatever it says about them.
#[test]
fn pick_lists_an_old_row_whose_slots_are_all_used() {
    let s = stub(vec![db_row("full", 10, Ok(status(30000, 0.0, 3)))], 200);
    let v = json_of(&s, &["fleet", "pick", "--json"]);
    assert_eq!(hosts(&v), ["full"]);
    assert_eq!(v[0]["load_headroom"], 6.0);
    assert_eq!(v[0]["mem_headroom_mb"], 30000);
}

/// `--need-mb` and `--need-load` keep only the hosts with that much memory headroom and that much
/// load headroom, each on its own and together.
#[test]
fn pick_need_mb_and_need_load_keep_only_hosts_with_room() {
    let s = stub(
        vec![
            // 5.0 cores, 9000 MB
            db_row("roomy", 10, Ok(new_status(1.0, 9000, 6.0, &[]))),
            // 1.5 cores, 20000 MB
            db_row("tight-load", 10, Ok(new_status(4.5, 20000, 6.0, &[]))),
            // 5.5 cores, 2000 MB
            db_row("tight-mem", 10, Ok(new_status(0.5, 2000, 6.0, &[]))),
            // 2.0 cores, 4000 MB
            db_row("exact", 10, Ok(new_status(4.0, 4000, 6.0, &[]))),
            db_row("blocked", 10, Ok(new_status(0.5, 30000, 6.0, &["memory"]))),
        ],
        200,
    );
    assert_eq!(
        hosts(&json_of(&s, &["fleet", "pick", "--json"])),
        ["tight-mem", "roomy", "exact", "tight-load"]
    );
    assert_eq!(
        hosts(&json_of(
            &s,
            &["fleet", "pick", "--need-mb", "4000", "--json"]
        )),
        ["roomy", "exact", "tight-load"]
    );
    assert_eq!(
        hosts(&json_of(
            &s,
            &["fleet", "pick", "--need-load", "2", "--json"]
        )),
        ["tight-mem", "roomy", "exact"]
    );
    assert_eq!(
        hosts(&json_of(
            &s,
            &[
                "fleet",
                "pick",
                "--need-mb",
                "4000",
                "--need-load",
                "2",
                "--json"
            ]
        )),
        ["roomy", "exact"]
    );
}

/// `--kind` stays for one version. It sets the needs of its runs (gradle: 3072 MB and 1 core;
/// other: 1024 MB and no load), `--need-mb` and `--need-load` override them one by one, and it
/// says on stderr, in one line, which flags replace it -- naming the needs in effect (see the
/// next test for what that means where a flag overrides).
#[test]
fn pick_kind_sets_the_needs_says_it_is_deprecated_and_yields_to_the_new_flags() {
    let s = stub(
        vec![
            // 1.0 cores, 4000 MB
            db_row("gradle-sized", 10, Ok(new_status(5.0, 4000, 6.0, &[]))),
            // 0.5 cores, 2000 MB
            db_row("other-sized", 10, Ok(new_status(5.5, 2000, 6.0, &[]))),
            // 0.5 cores, 4000 MB
            db_row("wide-but-loaded", 10, Ok(new_status(5.5, 4000, 6.0, &[]))),
            // 0.5 cores, 500 MB
            db_row("tiny", 10, Ok(new_status(5.5, 500, 6.0, &[]))),
        ],
        200,
    );
    let pick = |args: &[&str]| {
        let o = run(&s, args);
        let stderr: Vec<String> = text(&o.stderr).lines().map(str::to_owned).collect();
        // With no eligible host there is no stdout: the list is empty then.
        let v: Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| json!([]));
        (o.status.code(), stderr, hosts(&v).join(","))
    };
    let notice = |mb: &str, load: &str| {
        format!("fleet pick: --kind is deprecated and will be removed in the next version; use --need-mb {mb} --need-load {load} instead")
    };
    assert_eq!(
        pick(&["fleet", "pick", "--kind", "gradle", "--json"]),
        (
            Some(0),
            vec![notice("3072", "1")],
            "gradle-sized".to_owned()
        )
    );
    assert_eq!(
        pick(&["fleet", "pick", "--kind", "other", "--json"]),
        (
            Some(0),
            vec![notice("1024", "0")],
            "gradle-sized,wide-but-loaded,other-sized".to_owned()
        )
    );
    // An explicit need wins over the kind's; the other need stays the kind's. The notice names
    // the needs in effect, so following it changes nothing.
    assert_eq!(
        pick(&[
            "fleet",
            "pick",
            "--kind",
            "gradle",
            "--need-load",
            "0.5",
            "--json"
        ]),
        (
            Some(0),
            vec![notice("3072", "0.5")],
            "gradle-sized,wide-but-loaded".to_owned()
        )
    );
    assert_eq!(
        pick(&[
            "fleet",
            "pick",
            "--kind",
            "other",
            "--need-mb",
            "2500",
            "--json"
        ]),
        (
            Some(0),
            vec![notice("2500", "0")],
            "gradle-sized,wide-but-loaded".to_owned()
        )
    );
    // With no host left, the deprecation is still the only other line.
    assert_eq!(
        pick(&["fleet", "pick", "--kind", "other", "--need-mb", "2500", "--need-load", "2", "--json"]),
        (
            Some(1),
            vec![
                notice("2500", "2"),
                "fleet pick: no eligible host (every host is unknown, blocked, or has less memory or load headroom than asked for)".to_owned(),
            ],
            String::new()
        )
    );
    // The same needs spelled out say nothing.
    assert_eq!(
        pick(&[
            "fleet",
            "pick",
            "--need-mb",
            "3072",
            "--need-load",
            "1",
            "--json"
        ]),
        (Some(0), vec![], "gradle-sized".to_owned())
    );
}

/// The `--kind` notice names the needs in effect, not the kind's defaults: where `--need-mb` or
/// `--need-load` is given, its value is the one in the notice. One that named the defaults would
/// send the reader to a command that asks for something else.
#[test]
fn pick_kind_notice_names_the_needs_in_effect_not_the_kinds_defaults() {
    // 5.5 cores, 16384 MB: every need below fits.
    let s = stub(
        vec![db_row("huge", 10, Ok(new_status(0.5, 16384, 6.0, &[])))],
        200,
    );
    let noticed = |args: &[&str]| {
        let o = run(&s, args);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
        assert_eq!(
            hosts(&serde_json::from_slice::<Value>(&o.stdout).unwrap()),
            ["huge"]
        );
        text(&o.stderr)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        noticed(&[
            "fleet",
            "pick",
            "--kind",
            "gradle",
            "--need-mb",
            "8192",
            "--json"
        ]),
        ["fleet pick: --kind is deprecated and will be removed in the next version; use --need-mb 8192 --need-load 1 instead"]
    );
    assert_eq!(
        noticed(&[
            "fleet",
            "pick",
            "--kind",
            "gradle",
            "--need-mb",
            "8192",
            "--need-load",
            "2",
            "--json"
        ]),
        ["fleet pick: --kind is deprecated and will be removed in the next version; use --need-mb 8192 --need-load 2 instead"]
    );
    assert_eq!(
        noticed(&[
            "fleet",
            "pick",
            "--kind",
            "other",
            "--need-load",
            "3",
            "--json"
        ]),
        ["fleet pick: --kind is deprecated and will be removed in the next version; use --need-mb 1024 --need-load 3 instead"]
    );
    // The notice asks for exactly what the command did: a host one MB short of it is not listed.
    let t = stub(
        vec![db_row("short", 10, Ok(new_status(0.5, 8191, 6.0, &[])))],
        200,
    );
    let o = run(
        &t,
        &[
            "fleet",
            "pick",
            "--kind",
            "gradle",
            "--need-mb",
            "8192",
            "--json",
        ],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stdout).is_empty(), "{}", text(&o.stdout));
}

/// A blocked machine is not picked, whatever its memory. One whose memory the reservations have
/// used up is not either while its report is fresh -- `blocked` does not say so, the machine
/// figures it for a run that needs nothing -- and is once the report is older than the warm-up
/// window: the reservations are over by then.
#[test]
fn pick_skips_blocked_machines_and_reserved_memory_only_while_the_report_is_fresh() {
    let s = stub(
        vec![
            db_row("small", 10, Ok(new_status(0.5, 9000, 6.0, &[]))),
            db_row(
                "blocked-load",
                10,
                Ok(new_status(0.5, 50000, 6.0, &["load"])),
            ),
            db_row("big", 10, Ok(new_status(0.5, 20000, 6.0, &[]))),
            db_row(
                "blocked-memory",
                10,
                Ok(new_status(0.5, 30000, 6.0, &["memory"])),
            ),
            db_row(
                "used-up",
                10,
                Ok(reserved(new_status(0.5, 4000, 6.0, &[]), 4096, 1)),
            ),
            db_row(
                "room",
                10,
                Ok(reserved(new_status(0.5, 4000, 6.0, &[]), 0, 0)),
            ),
        ],
        200,
    );
    for kind in ["gradle", "other"] {
        let o = run(&s, &["fleet", "pick", "--kind", kind, "--json"]);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(hosts(&v), ["big", "small", "room"], "{kind}");
        assert_eq!(v[2]["mem_headroom_mb"], 4000);
    }
    let v = json_of(&s, &["fleet", "--json"]);
    let used_up = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["host"] == "used-up")
        .unwrap();
    assert_eq!(used_up["blocked"], json!([]));
    assert_eq!(used_up["mem_available_mb"], 4000);
    assert_eq!(used_up["mem_headroom_mb"], 0);

    // The same machine, reported 100 s ago: nothing is reserved any more, so all 4000 MB are
    // the run's, and nothing is warming up.
    let s = stub(
        vec![db_row(
            "used-up",
            100,
            Ok(reserved(new_status(0.5, 4000, 6.0, &[]), 4096, 1)),
        )],
        200,
    );
    let o = run(&s, &["fleet", "pick", "--kind", "gradle", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(hosts(&v), ["used-up"]);
    assert_eq!(v[0]["mem_headroom_mb"], 4000);
    let v = json_of(&s, &["fleet", "--json"]);
    assert_eq!(v[0]["mem_headroom_mb"], 4000);
    assert_eq!(v[0]["load_headroom"], 5.5);
    assert_eq!(v[0]["warming"], 0);
}

/// A machine that reports neither capacity nor slots is judged on its top-level figures: all the
/// memory it has available is the run's, and 2000 MB is less than a Gradle run needs and more
/// than one of the other kind.
#[test]
fn pick_needs_a_pools_worth_of_memory_from_a_machine_that_reports_no_capacity() {
    let mut old = new_status(0.5, 2000, 6.0, &[]);
    old.as_object_mut().unwrap().remove("capacity");
    let s = stub(vec![db_row("old", 10, Ok(old))], 200);
    let o = run(&s, &["fleet", "pick", "--kind", "gradle", "--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o.stdout));
    assert_eq!(
        text(&o.stderr).lines().last(),
        Some("fleet pick: no eligible host (every host is unknown, blocked, or has less memory or load headroom than asked for)")
    );
    let o = run(&s, &["fleet", "pick", "--kind", "other", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(hosts(&v), ["old"]);
    assert_eq!(v[0]["mem_headroom_mb"], 2000);
    assert_eq!(v[0]["load_headroom"], 5.5);
}

fn wait_for(s: &Stub, n: usize) -> usize {
    for _ in 0..50 {
        if s.requests.lock().unwrap().len() >= n {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    s.requests.lock().unwrap().len()
}

/// Run `claudecat <args>` on the real /proc (no fixture marker), as an agent would.
fn run_live(s: &Stub, data: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .args(args)
        .env("CLAUDECAT_FLEET_URL", &s.url)
        .env("CLAUDECAT_FLEET_TOKEN", "TOPSECRET-TOKEN")
        .env("CLAUDECAT_DATA_DIR", data)
        .env("CLAUDECAT_GATE_DIR", data.join("gate"))
        .env("CLAUDECAT_MIN_AVAILABLE_MB", "1")
        .env("CLAUDECAT_MAX_LOAD", "100000")
        .env("CLAUDECAT_MAX_MEM_PRESSURE", "100")
        .env_remove("CLAUDECAT_PROC_ROOT")
        .output()
        .unwrap()
}

#[test]
fn status_pushes_in_the_background_at_most_once_a_minute() {
    let s = stub(vec![], 200);
    let data = tempfile::tempdir().unwrap();
    let first = run_live(&s, data.path(), &["status"]);
    assert_eq!(first.status.code(), Some(0), "{}", text(&first.stderr));
    assert_eq!(wait_for(&s, 1), 1, "first status must push");
    run_live(&s, data.path(), &["status"]);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(
        s.requests.lock().unwrap().len(),
        1,
        "second call inside 60 s must not push"
    );
    // age the stamp past the interval: the next call pushes again
    std::fs::write(data.path().join("fleet-last-push"), "1\n").unwrap();
    run_live(&s, data.path(), &["status"]);
    assert_eq!(wait_for(&s, 2), 2);
}

#[test]
fn gate_run_and_check_also_trigger_the_push() {
    let s = stub(vec![], 200);
    let data = tempfile::tempdir().unwrap();
    let o = run_live(
        &s,
        data.path(),
        &["gate", "run", "--kind", "other", "--", "true"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(wait_for(&s, 1), 1);
    std::fs::write(data.path().join("fleet-last-push"), "1\n").unwrap();
    let o = run_live(&s, data.path(), &["gate", "check", "--kind", "other"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(wait_for(&s, 2), 2);
}

#[test]
fn an_unreachable_fleet_never_fails_or_delays_the_gate() {
    let dead = Stub {
        url: "http://127.0.0.1:9".into(),
        requests: Default::default(),
    };
    let data = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let o = run_live(
        &dead,
        data.path(),
        &["gate", "run", "--kind", "other", "--", "true"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(!text(&o.stderr).contains("fleet"), "{}", text(&o.stderr));
}

#[test]
fn fixture_proc_root_never_pushes() {
    let s = stub(vec![], 200);
    let data = tempfile::tempdir().unwrap();
    let proc_dir = data.path().join("proc");
    std::fs::create_dir(&proc_dir).unwrap();
    std::fs::write(proc_dir.join("loadavg"), "0.1 0 0 1/1 1\n").unwrap();
    std::fs::write(
        proc_dir.join("meminfo"),
        "MemAvailable: 4096000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .args(["status"])
        .env("CLAUDECAT_FLEET_URL", &s.url)
        .env("CLAUDECAT_FLEET_TOKEN", "T")
        .env("CLAUDECAT_DATA_DIR", data.path())
        .env("CLAUDECAT_GATE_DIR", data.path().join("gate"))
        .env("CLAUDECAT_PROC_ROOT", &proc_dir)
        .output()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(s.requests.lock().unwrap().is_empty());
}
