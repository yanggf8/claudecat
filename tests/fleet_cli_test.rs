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

fn status(mem: u64, load: f64, used: usize) -> Value {
    let slots: Vec<Value> = (0..3)
        .map(|i| json!({"index": i, "holder": null, "occupied": i < used}))
        .collect();
    json!({"load_1m": load, "mem_available_mb": mem, "swap_used_mb": 0, "slots": slots,
           "other_slots": [], "limits": {"slots": 3, "max_load": 6.0}, "compiling": []})
}

/// A row as the table stores it: (host, age in seconds, status document).
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

#[test]
fn fleet_json_lists_every_host_with_its_state() {
    let s = stub(
        vec![
            db_row("nuc", 10, Ok(status(5000, 0.5, 1))),
            db_row("old", 600, Ok(status(9000, 0.1, 0))),
            db_row("mac", 10, Err("cannot read".into())),
        ],
        200,
    );
    let o = run(&s, &["fleet", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let states: Vec<(&str, &str)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["host"].as_str().unwrap(), r["state"].as_str().unwrap()))
        .collect();
    assert_eq!(states, [("nuc", "ok"), ("old", "idle"), ("mac", "unknown")]);
    assert_eq!(v[0]["mem_available_mb"], 5000);
    assert_eq!(v[0]["slots_used"], 1);
    assert_eq!(v[0]["slots_total"], 3);
}

#[test]
fn fleet_table_marks_idle_and_unknown() {
    let s = stub(
        vec![
            db_row("old", 600, Ok(status(9000, 0.1, 0))),
            db_row("mac", 10, Err("x".into())),
        ],
        200,
    );
    let o = run(&s, &["fleet"]);
    let out = text(&o.stdout);
    assert!(out.lines().next().unwrap().starts_with("HOST"), "{out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("old") && l.contains("IDLE") && l.contains("9000*")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("mac") && l.contains("UNKNOWN")),
        "{out}"
    );
}

#[test]
fn pick_json_orders_fit_hosts_includes_idle_and_skips_full_unknown() {
    let s = stub(
        vec![
            db_row("small", 10, Ok(status(2000, 0.1, 0))),
            db_row("big", 10, Ok(status(9000, 0.1, 2))),
            db_row("full", 10, Ok(status(30000, 0.0, 3))),
            db_row("old", 600, Ok(status(40000, 0.0, 0))),
            db_row("mac", 600, Err("x".into())),
        ],
        200,
    );
    let o = run(&s, &["fleet", "pick", "--kind", "gradle", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let hosts: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["host"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, ["old", "big", "small"]);
    assert_eq!(v[0]["idle"], true);
    assert_eq!(v[1]["idle"], false);
}

#[test]
fn pick_with_no_fit_host_exits_1_with_a_reason() {
    let s = stub(vec![db_row("full", 10, Ok(status(30000, 0.0, 3)))], 200);
    let o = run(&s, &["fleet", "pick"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        text(&o.stderr).trim(),
        "fleet pick: no eligible host (every host is unknown or has no free slot)"
    );
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
