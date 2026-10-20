use claudecat::telemetry;
use rusqlite::Connection;
use std::fs;
use std::process::Command;

/// `claudecat memory configure` forwards to cort and writes cort's config, not the file telemetry
/// reads, so an unconfigured sync must name that file rather than send the user to a command
/// that would leave it missing.
#[test]
fn unconfigured_sync_names_the_file_it_reads_instead_of_a_command_that_cannot_write_it() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir(&data).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .env("CLAUDECAT_DATA_DIR", &data)
        .args(["telemetry", "sync", "--cort-usage"])
        .arg(tmp.path().join("none-cort.db"))
        .arg("--claudecat-usage")
        .arg(tmp.path().join("none-cat.db"))
        .arg("--cost-reports")
        .arg(tmp.path().join("none-costs"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    let expected = data.join("turso-memory.json");
    assert!(stderr.contains(&expected.display().to_string()), "{stderr}");
    assert!(!stderr.contains("claudecat memory configure"), "{stderr}");
}

#[test]
fn collection_keeps_each_local_event_and_its_source_attribution() {
    let tmp = tempfile::tempdir().unwrap();
    let cort = tmp.path().join("cort.db");
    let cat = tmp.path().join("cat.db");
    let costs = tmp.path().join("costs");
    fs::create_dir(&costs).unwrap();
    let cort_conn = Connection::open(&cort).unwrap();
    cort_conn.execute_batch(
        "CREATE TABLE _usage_meta (key TEXT, value TEXT);
         INSERT INTO _usage_meta VALUES ('MACHINE_ID','machine-a');
         INSERT INTO _usage_meta VALUES ('MACHINE_ID_SOURCE','test');
         CREATE TABLE command_log (id INTEGER PRIMARY KEY, ts INTEGER, project_id TEXT, command TEXT, args_summary TEXT);",
    )
    .unwrap();
    let conn = Connection::open(&cat).unwrap();
    conn.execute_batch(
        "CREATE TABLE navigation_log (ts INTEGER, project_id TEXT, mode TEXT);
         INSERT INTO navigation_log VALUES (101,'local-path-hash','cort');
         CREATE TABLE memory_recall_log (ts INTEGER, project_id TEXT, machine_id TEXT, machine_source TEXT, harness_declared TEXT);
         INSERT INTO memory_recall_log VALUES (102,'local-path-hash','machine-b','test','codex');",
    )
    .unwrap();
    fs::write(
        costs.join("2026-10-17.json"),
        r#"{"machine":{"id":"machine-c","source":"test"},"provider_tokens":42}"#,
    )
    .unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/example/repo.git"
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let local_project = claudecat::cort::project_id(&repo.to_string_lossy());
    cort_conn
        .execute(
            "INSERT INTO command_log VALUES (1,100,?1,'hook-suggest','{\"outcome\":\"hit\"}')",
            [&local_project],
        )
        .unwrap();
    let index = Connection::open(tmp.path().join("index.db")).unwrap();
    index
        .execute_batch("CREATE TABLE projects (project_id TEXT, path TEXT);")
        .unwrap();
    index
        .execute(
            "INSERT INTO projects VALUES (?1, ?2)",
            (&local_project, &repo.to_str().unwrap()),
        )
        .unwrap();
    let first = telemetry::scan(Some(&cort), Some(&cat), Some(&costs)).unwrap();
    let second = telemetry::scan(Some(&cort), Some(&cat), Some(&costs)).unwrap();
    assert_eq!(first.events.len(), 4);
    assert_eq!(first.legacy_cort_rows, 1);
    assert!(first.missing_sources.is_empty());
    assert_eq!(
        first.events.iter().map(|item| &item.id).collect::<Vec<_>>(),
        second
            .events
            .iter()
            .map(|item| &item.id)
            .collect::<Vec<_>>()
    );
    let cort_event = first
        .events
        .iter()
        .find(|item| item.source == "cort_command")
        .unwrap();
    assert_eq!(cort_event.machine_id, "machine-a");
    assert!(cort_event.payload.contains("hook-suggest"));
    let navigation = first
        .events
        .iter()
        .find(|item| item.source == "claudecat_navigation")
        .unwrap();
    assert_eq!(navigation.machine_id, "legacy_unknown");
    let recall = first
        .events
        .iter()
        .find(|item| item.source == "claudecat_memory_recall")
        .unwrap();
    assert_eq!(recall.machine_id, "machine-b");
    assert_eq!(recall.time_ms, Some(102));
    let cost = first
        .events
        .iter()
        .find(|item| item.source == "cost_daily")
        .unwrap();
    assert_eq!(cost.machine_id, "machine-c");
    assert_eq!(first.project_aliases.len(), 1);
    assert_eq!(first.unmatched_project_rows, 2);
    assert!(first
        .project_aliases
        .iter()
        .any(|alias| alias.machine_id == "machine-a" && alias.local_project_id == local_project));
}

#[test]
fn cort_schema_upgrade_keeps_old_event_id_and_stamps_new_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let cort = tmp.path().join("usage.db");
    let conn = Connection::open(&cort).unwrap();
    conn.execute_batch(
        "CREATE TABLE _usage_meta (key TEXT, value TEXT);
         INSERT INTO _usage_meta VALUES ('MACHINE_ID','old-db-machine');
         INSERT INTO _usage_meta VALUES ('MACHINE_ID_SOURCE','test');
         CREATE TABLE command_log (id INTEGER PRIMARY KEY, ts INTEGER, project_id TEXT, command TEXT);
         INSERT INTO command_log VALUES (1,100,NULL,'hook-suggest');",
    )
    .unwrap();
    let missing_cat = tmp.path().join("missing.db");
    let missing_cost = tmp.path().join("missing-cost");
    let before = telemetry::scan(Some(&cort), Some(&missing_cat), Some(&missing_cost)).unwrap();
    let old_id = before.events[0].id.clone();
    conn.execute_batch(
        "ALTER TABLE command_log ADD COLUMN machine_id TEXT NOT NULL DEFAULT 'legacy_unknown';
         ALTER TABLE command_log ADD COLUMN machine_source TEXT NOT NULL DEFAULT 'legacy_no_stamp';
         INSERT INTO command_log VALUES (2,101,NULL,'impact','new-machine','test');",
    )
    .unwrap();
    let after = telemetry::scan(Some(&cort), Some(&missing_cat), Some(&missing_cost)).unwrap();
    assert_eq!(after.events.len(), 2);
    assert_eq!(after.legacy_cort_rows, 1);
    assert_eq!(after.events[0].id, old_id);
    assert_eq!(after.events[0].machine_id, "old-db-machine");
    assert_eq!(after.events[1].machine_id, "new-machine");
}
