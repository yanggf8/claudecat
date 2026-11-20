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
        _url: &str,
        _token: &str,
        _body: &str,
        _timeout: Duration,
    ) -> Result<String, String> {
        Err("todo".into())
    }
}

/// `turso://h`, `libsql://h` or `https://h[/]` -> `https://h/v2/pipeline`.
pub fn pipeline_url(_db_url: &str) -> String {
    String::new()
}

/// Where `cort memory configure` keeps its credentials.
pub fn cort_config_path(_env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    None
}

/// Environment overrides first, then cort's file.
pub fn resolve_config(_env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    Err(String::new())
}

pub fn upsert_request(_host: &str, _updated_at: &str, _payload: &str) -> Value {
    Value::Null
}

pub fn select_request() -> Value {
    Value::Null
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
    pub fn push(&self, _snap: &FleetSnapshot) -> Result<(), String> {
        Err("todo".into())
    }

    pub fn list(&self) -> Result<Vec<Row>, String> {
        Err("todo".into())
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
