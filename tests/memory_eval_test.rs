mod common;

use claudecat::memory_eval;
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn paired_report_counts_improvements_and_excludes_incomplete_pairs() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "answer-key.txt", "The reviewed answer is B.\n");
    let mut trials = Vec::new();
    for (name, pair, condition, correct, lookup) in [
        ("on.txt", "pair-1", "memory_on", true, true),
        ("off.txt", "pair-1", "memory_off", false, false),
        ("incomplete.txt", "pair-2", "memory_on", true, true),
    ] {
        let bytes = format!("frozen transcript for {name}\n");
        fs::write(t.0.join(name), &bytes).unwrap();
        trials.push(serde_json::json!({
            "pair_id": pair,
            "task_id": pair,
            "scenario": "corrected_decision",
            "harness": "codex",
            "model": "test-model",
            "machine_id": "test-machine",
            "machine_source": "fixture",
            "session_id": name,
            "prompt_sha256": digest(b"same prompt"),
            "source_snapshot": "same-source",
            "tool_policy_id": "same-policy",
            "condition": condition,
            "artifact": name,
            "artifact_sha256": digest(bytes.as_bytes()),
            "lookup_observed": lookup,
            "source_read_observed": true,
            "answer_correct": correct,
            "citation_correct": correct,
            "uncertainty_correct": true,
            "input_tokens": 10,
            "output_tokens": 5,
            "elapsed_ms": 100
        }));
    }
    let file = t.0.join("eval.json");
    fs::write(
        &file,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "rubric_id": "reviewed-v1",
            "cases": [
                {"task_id": "pair-1", "scenario": "corrected_decision", "answer_key_artifact": "answer-key.txt", "answer_key_sha256": digest(b"The reviewed answer is B.\n")},
                {"task_id": "pair-2", "scenario": "corrected_decision", "answer_key_artifact": "answer-key.txt", "answer_key_sha256": digest(b"The reviewed answer is B.\n")}
            ],
            "trials": trials
        }))
        .unwrap(),
    )
    .unwrap();
    let result = memory_eval::report(&file).unwrap();
    assert_eq!(result.total_trials, 3);
    assert_eq!(result.complete_pairs, 1);
    assert_eq!(result.incomplete_pairs, ["pair-2"]);
    assert_eq!(result.overall.improved, 1);
    assert_eq!(result.overall.memory_off_correct, 0);
    assert_eq!(result.overall.token_pairs, 1);

    let cli = Command::new(env!("CARGO_BIN_EXE_claudecat"))
        .current_dir(&t.0)
        .args(["memory", "eval-report", "eval.json", "--json"])
        .output()
        .unwrap();
    assert!(cli.status.success());
    let json: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(json["overall"]["improved"], 1);

    fs::write(t.0.join("on.txt"), "changed artifact").unwrap();
    assert!(memory_eval::report(&file)
        .unwrap_err()
        .contains("hash mismatch"));
}

#[test]
fn paired_report_rejects_mismatched_environments() {
    let t = common::Tmp(common::temp_dir());
    common::write(&t.0, "one.txt", "artifact\n");
    common::write(&t.0, "answer-key.txt", "The reviewed answer.\n");
    let hash = digest(b"artifact\n");
    let trial = |condition: &str, machine: &str| {
        serde_json::json!({
            "pair_id": "pair-1",
            "task_id": "task-1",
            "scenario": "stale_source",
            "harness": "codex",
            "model": "test-model",
            "machine_id": machine,
            "machine_source": "fixture",
            "session_id": condition,
            "prompt_sha256": digest(b"same prompt"),
            "source_snapshot": "same-source",
            "tool_policy_id": "same-policy",
            "condition": condition,
            "artifact": "one.txt",
            "artifact_sha256": hash,
            "lookup_observed": false,
            "source_read_observed": true,
            "answer_correct": true,
            "citation_correct": true,
            "uncertainty_correct": true,
            "input_tokens": null,
            "output_tokens": null,
            "elapsed_ms": null
        })
    };
    let file = t.0.join("eval.json");
    fs::write(
        &file,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "rubric_id": "reviewed-v1",
            "cases": [{"task_id": "task-1", "scenario": "stale_source", "answer_key_artifact": "answer-key.txt", "answer_key_sha256": digest(b"The reviewed answer.\n")}],
            "trials": [trial("memory_on", "machine-a"), trial("memory_off", "machine-b")]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(memory_eval::report(&file)
        .unwrap_err()
        .contains("different task or environment"));
}
