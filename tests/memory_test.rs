mod common;

use std::fs;
use std::process::Command;

#[test]
fn memory_commands_delegate_to_cort_without_echoing_a_private_query_on_launch_failure() {
    let t = common::Tmp(common::temp_dir());
    let result = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .env("CORT_BIN", t.0.join("missing-cort"))
        .args([
            "memory",
            "recall",
            "private deployment",
            "--harness",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("cannot run cort memory"), "{stderr}");
    assert!(!stderr.contains("private deployment"));
}

/// ClaudeCat no longer parses these verbs at all; they reach cort only because `main()` forwards
/// them before clap runs. A verb that stopped forwarding would fail in clap instead, with a
/// different message, so every ledger verb is checked rather than recall alone.
#[test]
fn every_ledger_verb_forwards_to_cort() {
    let t = common::Tmp(common::temp_dir());
    for verb in [
        "configure",
        "sync",
        "status",
        "import-legacy",
        "record",
        "resolve",
        "recall",
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_claudecat"))
            .env("CORT_BIN", t.0.join("missing-cort"))
            .args(["memory", verb])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{verb}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("cannot run cort memory"),
            "{verb}: {stderr}"
        );
    }
}

/// `memory --help` used to forward too, so it printed cort's help and hid the two verbs only
/// ClaudeCat answers. It is answered here and points at cort for the rest; a verb's own `--help`
/// still forwards, because that verb's options live in cort.
#[test]
fn memory_help_is_answered_here_and_names_cort_for_the_ledger_verbs() {
    let t = common::Tmp(common::temp_dir());
    for spelling in ["--help", "-h", "help"] {
        let result = Command::new(env!("CARGO_BIN_EXE_claudecat"))
            .env("CORT_BIN", t.0.join("missing-cort"))
            .args(["memory", spelling])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success(),
            "{spelling}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(stdout.contains("usage"), "{spelling}: {stdout}");
        assert!(stdout.contains("eval-report"), "{spelling}: {stdout}");
        assert!(
            stdout.contains("cort memory --help"),
            "{spelling}: {stdout}"
        );
    }
    let verb_help = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .env("CORT_BIN", t.0.join("missing-cort"))
        .args(["memory", "recall", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&verb_help.stderr).contains("cannot run cort memory"));
}

#[test]
fn old_recall_rows_keep_an_unknown_machine_instead_of_being_relabelled() {
    let t = common::Tmp(common::temp_dir());
    let data = t.0.join("local-data");
    fs::create_dir_all(&data).unwrap();
    let conn = rusqlite::Connection::open(data.join("usage.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE memory_recall_log (
            ts INTEGER NOT NULL, project_id TEXT NOT NULL, harness_declared TEXT NOT NULL,
            outcome TEXT NOT NULL, ledger_status TEXT NOT NULL, hit_count INTEGER NOT NULL,
            unchanged_hits INTEGER NOT NULL, changed_hits INTEGER NOT NULL,
            missing_hits INTEGER NOT NULL, conflict_hits INTEGER NOT NULL,
            elapsed_ms INTEGER NOT NULL);
         INSERT INTO memory_recall_log VALUES
            (9999999999999, 'project', 'codex', 'ok', 'present', 1, 1, 0, 0, 0, 1);",
    )
    .unwrap();
    drop(conn);
    let result = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .env("CLAUDECAT_DATA_DIR", &data)
        .args(["memory", "usage", "--json"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["recall_calls"], 1);
    assert_eq!(report["by_machine"]["legacy_no_stamp:legacy_unknown"], 1);
}
