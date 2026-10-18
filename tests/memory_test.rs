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

    common::write(&t.0, "docs/decision.md", "# Decision\nUse option B.\n");
    let found = memory::recall(&t.0, "deployment option").unwrap().hits;
    assert_eq!(found[0].evidence_status, "source_changed");

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
fn source_must_be_in_repo_and_cli_exposes_the_evidence_state() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "A fact.\n");
    assert!(memory::record(&t.0, "fact", "A", Path::new("../facts.md"), 1, None).is_err());
    assert!(memory::record(&t.0, "fact", "A", Path::new("facts.md"), 2, None).is_err());

    let bin = env!("CARGO_BIN_EXE_claudecat");
    let recorded = Command::new(bin)
        .args([
            "memory",
            "record",
            "--root",
            t.0.to_str().unwrap(),
            "--key",
            "fact",
            "--claim",
            "A",
            "--source",
            "facts.md",
            "--line",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let recalled = Command::new(bin)
        .args([
            "memory",
            "recall",
            "fact",
            "--root",
            t.0.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(recalled.status.success());
    let hits: serde_json::Value = serde_json::from_slice(&recalled.stdout).unwrap();
    assert_eq!(hits["ledger_status"], "present");
    assert_eq!(hits["hits"][0]["evidence_status"], "source_unchanged");
    assert_eq!(hits["hits"][0]["source"], "facts.md");
}

#[test]
fn absent_ledger_is_disclosed_instead_of_treated_as_an_empty_memory() {
    let t = common::Tmp(common::temp_dir());
    let result = memory::recall(&t.0, "anything").unwrap();
    assert_eq!(result.ledger_status, "absent");
    assert!(result.hits.is_empty());
}

#[test]
fn simultaneous_agent_records_keep_both_entries() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "facts.md", "First fact.\nSecond fact.\n");
    let bin = env!("CARGO_BIN_EXE_claudecat");
    let mut first = Command::new(bin)
        .args([
            "memory",
            "record",
            "--root",
            t.0.to_str().unwrap(),
            "--key",
            "first",
            "--claim",
            "First fact",
            "--source",
            "facts.md",
            "--line",
            "1",
        ])
        .spawn()
        .unwrap();
    let mut second = Command::new(bin)
        .args([
            "memory",
            "record",
            "--root",
            t.0.to_str().unwrap(),
            "--key",
            "second",
            "--claim",
            "Second fact",
            "--source",
            "facts.md",
            "--line",
            "2",
        ])
        .spawn()
        .unwrap();
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());
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
