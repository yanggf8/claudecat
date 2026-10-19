mod common;

use claudecat::memory::{self, MemoryLedger};
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn source_change_retracts_freshness_and_a_correction_replaces_the_active_claim() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "docs/decision.md", "# Decision\nUse option A.\n");
    let first = memory::record(
        &t.0,
        "deployment option",
        "Option A is current",
        Path::new("docs/decision.md"),
        2,
        None,
    )
    .unwrap();
    let recalled = memory::recall(&t.0, "deployment option").unwrap();
    assert_eq!(recalled.ledger_status, "present");
    let found = recalled.hits;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].evidence_status, "source_unchanged");
    assert_eq!(found[0].source_line_status.as_deref(), Some("same_line"));

    common::write(&t.0, "docs/decision.md", "# Decision\nUse option B.\n");
    let found = memory::recall(&t.0, "deployment option").unwrap().hits;
    assert_eq!(found[0].evidence_status, "source_changed");
    assert_eq!(
        found[0].source_line_status.as_deref(),
        Some("changed_or_moved")
    );

    let second = memory::record(
        &t.0,
        "deployment option",
        "Option B is current",
        Path::new("docs/decision.md"),
        2,
        Some(&first.id),
    )
    .unwrap();
    let found = memory::recall(&t.0, "deployment option").unwrap().hits;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].entry.id, second.id);
    assert_eq!(found[0].evidence_status, "source_unchanged");
    assert_eq!(memory::load(&t.0).unwrap().entries.len(), 2);
}

#[test]
fn unrelated_file_edit_stays_unresolved_even_when_the_cited_line_matches() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "Decision A.\nDetails.\n");
    memory::record(&t.0, "decision", "A", Path::new("facts.md"), 1, None).unwrap();
    common::write(&t.0, "facts.md", "Decision A.\nUpdated details.\n");
    let hit = &memory::recall(&t.0, "decision").unwrap().hits[0];
    assert_eq!(hit.evidence_status, "source_changed");
    assert_eq!(hit.source_line_status.as_deref(), Some("same_line"));
}

#[test]
fn one_resolution_can_close_two_concurrent_active_claims() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "Decision C.\n");
    let first = memory::record(&t.0, "decision", "A", Path::new("facts.md"), 1, None).unwrap();
    let mut ledger = memory::load(&t.0).unwrap();
    let mut concurrent = first.clone();
    concurrent.id = "second".into();
    concurrent.claim = "B".into();
    ledger.entries.push(concurrent.clone());
    let mut resolution = first.clone();
    resolution.id = "resolution".into();
    resolution.claim = "C".into();
    resolution.resolves = vec![first.id.clone(), concurrent.id.clone()];
    ledger.entries.push(resolution.clone());
    fs::write(
        memory::ledger_path(&t.0),
        serde_json::to_string(&ledger).unwrap(),
    )
    .unwrap();
    let hits = memory::recall(&t.0, "decision").unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.id, resolution.id);
}

#[test]
fn ambiguous_or_unreadable_memory_never_becomes_a_quiet_answer() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "source fact\n");
    let first = memory::record(&t.0, "owner", "Alice", Path::new("facts.md"), 1, None).unwrap();
    assert!(memory::record(&t.0, "owner", "Bob", Path::new("facts.md"), 1, None).is_err());

    let path = memory::ledger_path(&t.0);
    let mut ledger = memory::load(&t.0).unwrap();
    let mut concurrent = first.clone();
    concurrent.id = "other-branch".into();
    concurrent.claim = "Bob".into();
    ledger.entries.push(concurrent);
    fs::write(&path, serde_json::to_string_pretty(&ledger).unwrap()).unwrap();
    let found = memory::recall(&t.0, "owner").unwrap().hits;
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|h| h.evidence_status == "conflict"));

    fs::write(&path, "not JSON").unwrap();
    assert!(memory::recall(&t.0, "owner").is_err());
}

#[test]
fn source_must_be_in_repo() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "A fact.\n");
    assert!(memory::record(&t.0, "fact", "A", Path::new("../facts.md"), 1, None).is_err());
    assert!(memory::record(&t.0, "fact", "A", Path::new("facts.md"), 2, None).is_err());
}

#[test]
fn absent_ledger_is_disclosed_instead_of_treated_as_an_empty_memory() {
    let t = common::Tmp(common::temp_dir());
    let result = memory::recall(&t.0, "anything").unwrap();
    assert_eq!(result.ledger_status, "absent");
    assert!(result.hits.is_empty());
}

#[test]
fn simultaneous_legacy_agent_records_keep_both_entries() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "First fact.\nSecond fact.\n");
    let first_root = t.0.clone();
    let second_root = t.0.clone();
    let first = std::thread::spawn(move || {
        memory::record(
            &first_root,
            "first",
            "First fact",
            Path::new("facts.md"),
            1,
            None,
        )
    });
    let second = std::thread::spawn(move || {
        memory::record(
            &second_root,
            "second",
            "Second fact",
            Path::new("facts.md"),
            2,
            None,
        )
    });
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    let ledger = memory::load(&t.0).unwrap();
    assert_eq!(ledger.entries.len(), 2);
}

#[test]
fn a_hand_edited_future_supersession_is_rejected() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "A fact.\n");
    let first = memory::record(&t.0, "fact", "A", Path::new("facts.md"), 1, None).unwrap();
    let second =
        memory::record(&t.0, "fact", "B", Path::new("facts.md"), 1, Some(&first.id)).unwrap();
    let mut ledger: MemoryLedger = memory::load(&t.0).unwrap();
    ledger.entries[0].supersedes = Some(second.id);
    fs::write(
        memory::ledger_path(&t.0),
        serde_json::to_string(&ledger).unwrap(),
    )
    .unwrap();
    assert!(memory::recall(&t.0, "fact").is_err());
}

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
