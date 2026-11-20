//! Turso storage for fleet snapshots over the HTTP pipeline API (`POST <db>/v2/pipeline`).
//! One row per host in `claudecat_fleet`; credentials come from the environment or from the
//! file `cort memory configure` writes. The token never appears in output or error text.
use crate::fleet::FleetSnapshot;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

pub const NO_CONFIG: &str = "fleet: no Turso config; run `cort memory configure --url URL --token-stdin` or set CLAUDECAT_FLEET_URL and CLAUDECAT_FLEET_TOKEN";
pub const CREATE_SQL: &str = "CREATE TABLE IF NOT EXISTS claudecat_fleet (host TEXT PRIMARY KEY, updated_at TEXT NOT NULL, payload TEXT NOT NULL)";
pub const UPSERT_SQL: &str = "INSERT INTO claudecat_fleet (host, updated_at, payload) VALUES (?, ?, ?) ON CONFLICT(host) DO UPDATE SET updated_at = excluded.updated_at, payload = excluded.payload";
pub const SELECT_SQL: &str = "SELECT host, updated_at, payload FROM claudecat_fleet ORDER BY host";

/// Database URL and auth token. `Debug` hides the token.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Config {{ url: {:?}, token: <redacted> }}", self.url)
    }
}

/// The HTTP layer, behind a trait so tests never touch the network.
pub trait Http {
    /// POST `body` as JSON with a bearer token; returns the response body.
    fn post(&self, url: &str, token: &str, body: &str, timeout: Duration)
        -> Result<String, String>;
}

pub struct UreqHttp;

impl Http for UreqHttp {
    fn post(
        &self,
        url: &str,
        token: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        match agent
            .post(url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_string(body)
        {
            Ok(resp) => resp.into_string().map_err(|e| format!("reading response: {e}")),
            Err(ureq::Error::Status(401 | 403, _)) => {
                Err("Turso rejected the token (HTTP 401/403); re-run `cort memory configure --url URL --token-stdin` with a valid token".into())
            }
            Err(ureq::Error::Status(code, _)) => Err(format!("Turso answered HTTP {code}")),
            // The transport error text names the host, never a header.
            Err(ureq::Error::Transport(t)) => Err(format!("cannot reach Turso: {t}")),
        }
    }
}

/// `turso://h`, `libsql://h` or `https://h[/]` -> `https://h/v2/pipeline`.
pub fn pipeline_url(db_url: &str) -> String {
    let rest = db_url
        .strip_prefix("turso://")
        .or_else(|| db_url.strip_prefix("libsql://"))
        .or_else(|| db_url.strip_prefix("https://"))
        .unwrap_or(db_url);
    format!("https://{}/v2/pipeline", rest.trim_end_matches('/'))
}

/// Where `cort memory configure` keeps its credentials.
pub fn cort_config_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    let dir = if let Some(d) = set("CORT_MEMORY_DIR") {
        PathBuf::from(d)
    } else if let Some(x) = set("XDG_DATA_HOME") {
        PathBuf::from(x).join("cortexyoung/memory")
    } else {
        PathBuf::from(set("HOME")?).join(".local/share/cortexyoung/memory")
    };
    Some(dir.join("turso-memory.json"))
}

/// Environment overrides first, then cort's file.
pub fn resolve_config(env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    match (set("CLAUDECAT_FLEET_URL"), set("CLAUDECAT_FLEET_TOKEN")) {
        (Some(url), Some(token)) => return Ok(Config { url, token }),
        (Some(_), None) => return Err("fleet: CLAUDECAT_FLEET_URL is set but CLAUDECAT_FLEET_TOKEN is not; set both or unset the URL".into()),
        (None, Some(_)) => return Err("fleet: CLAUDECAT_FLEET_TOKEN is set but CLAUDECAT_FLEET_URL is not; set both or unset the token".into()),
        (None, None) => {}
    }
    let path = cort_config_path(env).ok_or_else(|| NO_CONFIG.to_owned())?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(NO_CONFIG.into()),
        Err(e) => return Err(format!("fleet: cannot read {}: {e}", path.display())),
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        format!(
            "fleet: {} is not valid JSON; re-run `cort memory configure --url URL --token-stdin`",
            path.display()
        )
    })?;
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match (field("url"), field("token")) {
        (Some(url), Some(token)) => Ok(Config { url, token }),
        _ => Err(format!(
            "fleet: {} lacks url or token; re-run `cort memory configure --url URL --token-stdin`",
            path.display()
        )),
    }
}

fn text_arg(v: &str) -> Value {
    json!({"type": "text", "value": v})
}

pub fn upsert_request(host: &str, updated_at: &str, payload: &str) -> Value {
    json!({"type": "execute", "stmt": {
        "sql": UPSERT_SQL,
        "args": [text_arg(host), text_arg(updated_at), text_arg(payload)],
    }})
}

fn execute(sql: &str) -> Value {
    json!({"type": "execute", "stmt": {"sql": sql}})
}

pub fn select_request() -> Value {
    execute(SELECT_SQL)
}

pub struct Row {
    pub host: String,
    pub updated_at: String,
    pub snapshot: FleetSnapshot,
}

pub struct Store<'a, H: Http> {
    pub config: Config,
    pub http: &'a H,
    pub timeout: Duration,
}

impl<H: Http> Store<'_, H> {
    /// Send `statements` (plus a close) in one pipeline call; returns one result per statement.
    fn pipeline(&self, statements: Vec<Value>) -> Result<Vec<Value>, String> {
        let n = statements.len();
        let mut requests = statements;
        requests.push(json!({"type": "close"}));
        let body = json!({"requests": requests}).to_string();
        let reply = self.http.post(
            &pipeline_url(&self.config.url),
            &self.config.token,
            &body,
            self.timeout,
        )?;
        let reply: Value = serde_json::from_str(&reply)
            .map_err(|_| "Turso sent a reply that is not JSON".to_owned())?;
        let results = reply["results"].as_array().cloned().unwrap_or_default();
        let mut out = Vec::with_capacity(n);
        for result in results.into_iter().take(n) {
            if result["type"] == "error" {
                let msg = result["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error");
                return Err(format!("Turso statement failed: {msg}"));
            }
            out.push(result);
        }
        if out.len() < n {
            return Err("Turso reply is missing statement results".into());
        }
        Ok(out)
    }

    pub fn push(&self, snap: &FleetSnapshot) -> Result<(), String> {
        let payload = serde_json::to_string(snap).map_err(|e| e.to_string())?;
        self.pipeline(vec![
            execute(CREATE_SQL),
            upsert_request(&snap.host, &snap.collected_at, &payload),
        ])
        .map(|_| ())
    }

    pub fn list(&self) -> Result<Vec<Row>, String> {
        let results = self.pipeline(vec![execute(CREATE_SQL), select_request()])?;
        let rows = results[1]["response"]["result"]["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let cell = |row: &Value, i: usize| row[i]["value"].as_str().map(str::to_owned);
        let mut out = Vec::new();
        for row in &rows {
            let (Some(host), Some(updated_at), Some(payload)) =
                (cell(row, 0), cell(row, 1), cell(row, 2))
            else {
                continue;
            };
            // A row this version cannot read (older or newer schema) is skipped, not fatal.
            if let Ok(snapshot) = serde_json::from_str::<FleetSnapshot>(&payload) {
                out.push(Row {
                    host,
                    updated_at,
                    snapshot,
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    struct Mock {
        calls: RefCell<Vec<(String, String, Value)>>,
        reply: String,
    }

    impl Http for Mock {
        fn post(&self, url: &str, token: &str, body: &str, _t: Duration) -> Result<String, String> {
            self.calls.borrow_mut().push((
                url.into(),
                token.into(),
                serde_json::from_str(body).unwrap(),
            ));
            Ok(self.reply.clone())
        }
    }

    fn mock(reply: Value) -> Mock {
        Mock {
            calls: RefCell::new(Vec::new()),
            reply: reply.to_string(),
        }
    }

    fn store(m: &Mock) -> Store<'_, Mock> {
        Store {
            config: Config {
                url: "turso://db.example.io".into(),
                token: "SECRET-TOKEN".into(),
            },
            http: m,
            timeout: Duration::from_secs(2),
        }
    }

    fn snap(host: &str) -> FleetSnapshot {
        crate::fleet::assemble(host, "linux", 0, Ok(json!({"load_1m": 0.5})), Some(vec![]))
    }

    fn ok_results(n: usize) -> Value {
        let mut r: Vec<Value> = (0..n)
            .map(|_| json!({"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":[]}}}))
            .collect();
        r.push(json!({"type":"ok","response":{"type":"close"}}));
        json!({"results": r})
    }

    #[test]
    fn pipeline_url_maps_schemes() {
        assert_eq!(pipeline_url("turso://h.io"), "https://h.io/v2/pipeline");
        assert_eq!(pipeline_url("libsql://h.io"), "https://h.io/v2/pipeline");
        assert_eq!(pipeline_url("https://h.io/"), "https://h.io/v2/pipeline");
        assert_eq!(pipeline_url("https://h.io"), "https://h.io/v2/pipeline");
        // plain http is kept so a test can point at a local stub
        assert_eq!(pipeline_url("http://127.0.0.1:9"), "http://127.0.0.1:9/v2/pipeline");
    }

    #[test]
    fn upsert_binds_values_as_parameters() {
        let evil = "h'); DROP TABLE claudecat_fleet; --";
        let req = upsert_request(evil, "2026-11-20T00:00:00Z", "{\"a\":1}");
        let stmt = &req["stmt"];
        assert_eq!(req["type"], "execute");
        assert_eq!(stmt["sql"], UPSERT_SQL);
        assert!(!stmt["sql"].as_str().unwrap().contains("DROP"));
        assert_eq!(
            stmt["args"],
            json!([
                {"type":"text","value":evil},
                {"type":"text","value":"2026-11-20T00:00:00Z"},
                {"type":"text","value":"{\"a\":1}"}
            ])
        );
        assert!(UPSERT_SQL.contains("ON CONFLICT(host) DO UPDATE"));
    }

    #[test]
    fn push_posts_create_then_upsert_then_close_with_bearer() {
        let m = mock(ok_results(2));
        store(&m).push(&snap("nuc")).unwrap();
        let calls = m.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (url, token, body) = &calls[0];
        assert_eq!(url, "https://db.example.io/v2/pipeline");
        assert_eq!(token, "SECRET-TOKEN");
        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0]["stmt"]["sql"], CREATE_SQL);
        assert_eq!(reqs[1]["stmt"]["sql"], UPSERT_SQL);
        assert_eq!(reqs[1]["stmt"]["args"][0]["value"], "nuc");
        assert_eq!(reqs[2], json!({"type":"close"}));
        let payload: Value =
            serde_json::from_str(reqs[1]["stmt"]["args"][2]["value"].as_str().unwrap()).unwrap();
        assert_eq!(payload["host"], "nuc");
    }

    #[test]
    fn push_reports_a_statement_error_without_the_token() {
        let m = mock(json!({"results":[
            {"type":"error","error":{"message":"no such table"}}
        ]}));
        let err = store(&m).push(&snap("nuc")).unwrap_err();
        assert!(err.contains("no such table"), "{err}");
        assert!(!err.contains("SECRET-TOKEN"), "{err}");
    }

    #[test]
    fn list_parses_rows_into_snapshots() {
        let payload = serde_json::to_string(&snap("mgpu")).unwrap();
        let m = mock(json!({"results":[
            {"type":"ok","response":{"type":"execute","result":{"cols":[],"rows":[]}}},
            {"type":"ok","response":{"type":"execute","result":{
                "cols":[{"name":"host"},{"name":"updated_at"},{"name":"payload"}],
                "rows":[[
                    {"type":"text","value":"mgpu"},
                    {"type":"text","value":"2026-11-20T00:00:00Z"},
                    {"type":"text","value":payload}
                ]]}}},
            {"type":"ok","response":{"type":"close"}}
        ]}));
        let rows = store(&m).list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, "mgpu");
        assert_eq!(rows[0].updated_at, "2026-11-20T00:00:00Z");
        assert_eq!(rows[0].snapshot.os, "linux");
        let calls = m.calls.borrow();
        assert_eq!(calls[0].2["requests"][0]["stmt"]["sql"], CREATE_SQL);
        assert_eq!(calls[0].2["requests"][1]["stmt"]["sql"], SELECT_SQL);
    }

    #[test]
    fn config_env_overrides_win() {
        let env = |k: &str| match k {
            "CLAUDECAT_FLEET_URL" => Some("turso://x.io".to_owned()),
            "CLAUDECAT_FLEET_TOKEN" => Some("T".to_owned()),
            _ => None,
        };
        let c = resolve_config(&env).unwrap();
        assert_eq!((c.url.as_str(), c.token.as_str()), ("turso://x.io", "T"));
    }

    #[test]
    fn config_half_set_env_is_an_error_naming_the_missing_one() {
        let env = |k: &str| (k == "CLAUDECAT_FLEET_URL").then(|| "turso://x.io".to_owned());
        let err = resolve_config(&env).unwrap_err();
        assert!(err.contains("CLAUDECAT_FLEET_TOKEN"), "{err}");
    }

    #[test]
    fn config_reads_corts_file_and_debug_hides_the_token() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("turso-memory.json"),
            r#"{"url":"turso://file.io","token":"FILE-TOKEN"}"#,
        )
        .unwrap();
        let dir = t.path().to_string_lossy().into_owned();
        let env = move |k: &str| (k == "CORT_MEMORY_DIR").then(|| dir.clone());
        let c = resolve_config(&env).unwrap();
        assert_eq!(c.url, "turso://file.io");
        assert_eq!(c.token, "FILE-TOKEN");
        assert!(!format!("{c:?}").contains("FILE-TOKEN"));
    }

    #[test]
    fn config_path_follows_cort_precedence() {
        let a = |k: &str| match k {
            "CORT_MEMORY_DIR" => Some("/m".to_owned()),
            "XDG_DATA_HOME" => Some("/x".to_owned()),
            "HOME" => Some("/h".to_owned()),
            _ => None,
        };
        assert_eq!(
            cort_config_path(&a),
            Some(PathBuf::from("/m/turso-memory.json"))
        );
        let b = |k: &str| match k {
            "XDG_DATA_HOME" => Some("/x".to_owned()),
            "HOME" => Some("/h".to_owned()),
            _ => None,
        };
        assert_eq!(
            cort_config_path(&b),
            Some(PathBuf::from("/x/cortexyoung/memory/turso-memory.json"))
        );
        let c = |k: &str| (k == "HOME").then(|| "/h".to_owned());
        assert_eq!(
            cort_config_path(&c),
            Some(PathBuf::from(
                "/h/.local/share/cortexyoung/memory/turso-memory.json"
            ))
        );
    }

    #[test]
    fn config_missing_gives_the_one_step_message() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().to_string_lossy().into_owned();
        let env = move |k: &str| (k == "CORT_MEMORY_DIR").then(|| dir.clone());
        assert_eq!(resolve_config(&env).unwrap_err(), NO_CONFIG);
    }
}
