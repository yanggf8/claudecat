//! Offline, reviewer-scored paired evaluation for the project-memory trial.
//! The program checks provenance and denominators; it cannot judge semantic correctness.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    MemoryOn,
    MemoryOff,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trial {
    pub pair_id: String,
    pub task_id: String,
    pub scenario: String,
    pub harness: String,
    pub model: String,
    pub machine_id: String,
    pub machine_source: String,
    pub session_id: String,
    pub prompt_sha256: String,
    /// Hash or commit naming the source snapshot excluding the memory ledger.
    pub source_snapshot: String,
    pub tool_policy_id: String,
    pub condition: Condition,
    /// Path relative to the dataset; the artifact must be retained for a reviewer to inspect.
    pub artifact: PathBuf,
    pub artifact_sha256: String,
    pub lookup_observed: bool,
    pub source_read_observed: bool,
    pub answer_correct: bool,
    pub citation_correct: bool,
    pub uncertainty_correct: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub task_id: String,
    pub scenario: String,
    /// Frozen answer key and source evidence, prepared before running either arm.
    pub answer_key_artifact: PathBuf,
    pub answer_key_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub schema: u32,
    pub rubric_id: String,
    pub cases: Vec<EvalCase>,
    pub trials: Vec<Trial>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PairSummary {
    pub pairs: usize,
    pub memory_on_correct: usize,
    pub memory_off_correct: usize,
    pub improved: usize,
    pub regressed: usize,
    pub both_correct: usize,
    pub both_wrong: usize,
    pub memory_on_lookup_observed: usize,
    pub memory_off_lookup_observed: usize,
    pub memory_on_source_read_observed: usize,
    pub memory_off_source_read_observed: usize,
    pub memory_on_citation_correct: usize,
    pub memory_off_citation_correct: usize,
    pub memory_on_uncertainty_correct: usize,
    pub memory_off_uncertainty_correct: usize,
    pub token_pairs: usize,
    pub memory_on_tokens: u64,
    pub memory_off_tokens: u64,
    pub elapsed_pairs: usize,
    pub memory_on_elapsed_ms: u64,
    pub memory_off_elapsed_ms: u64,
}

impl PairSummary {
    fn add(&mut self, on: &Trial, off: &Trial) {
        self.pairs += 1;
        self.memory_on_correct += usize::from(on.answer_correct);
        self.memory_off_correct += usize::from(off.answer_correct);
        match (on.answer_correct, off.answer_correct) {
            (true, false) => self.improved += 1,
            (false, true) => self.regressed += 1,
            (true, true) => self.both_correct += 1,
            (false, false) => self.both_wrong += 1,
        }
        self.memory_on_lookup_observed += usize::from(on.lookup_observed);
        self.memory_off_lookup_observed += usize::from(off.lookup_observed);
        self.memory_on_source_read_observed += usize::from(on.source_read_observed);
        self.memory_off_source_read_observed += usize::from(off.source_read_observed);
        self.memory_on_citation_correct += usize::from(on.citation_correct);
        self.memory_off_citation_correct += usize::from(off.citation_correct);
        self.memory_on_uncertainty_correct += usize::from(on.uncertainty_correct);
        self.memory_off_uncertainty_correct += usize::from(off.uncertainty_correct);
        if let (Some(oi), Some(oo), Some(fi), Some(fo)) = (
            on.input_tokens,
            on.output_tokens,
            off.input_tokens,
            off.output_tokens,
        ) {
            self.token_pairs += 1;
            self.memory_on_tokens = self.memory_on_tokens.saturating_add(oi.saturating_add(oo));
            self.memory_off_tokens = self.memory_off_tokens.saturating_add(fi.saturating_add(fo));
        }
        if let (Some(ot), Some(ft)) = (on.elapsed_ms, off.elapsed_ms) {
            self.elapsed_pairs += 1;
            self.memory_on_elapsed_ms = self.memory_on_elapsed_ms.saturating_add(ot);
            self.memory_off_elapsed_ms = self.memory_off_elapsed_ms.saturating_add(ft);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub rubric_id: String,
    pub total_trials: usize,
    pub complete_pairs: usize,
    pub incomplete_pairs: Vec<String>,
    pub overall: PairSummary,
    /// A machine, harness and model must agree within each pair.
    pub by_machine_harness_model: BTreeMap<String, PairSummary>,
    pub by_scenario: BTreeMap<String, PairSummary>,
    pub reading: String,
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn validate_artifact(base: &Path, path: &Path, expected_hash: &str) -> Result<(), String> {
    if !safe_relative(path) {
        return Err(format!("invalid artifact path: {}", path.display()));
    }
    let path = base.join(path);
    let real = fs::canonicalize(&path)
        .map_err(|e| format!("cannot read artifact {}: {e}", path.display()))?;
    if !real.starts_with(base) || !real.is_file() {
        return Err(format!(
            "artifact leaves dataset directory: {}",
            path.display()
        ));
    }
    let bytes = fs::read(&real).map_err(|e| format!("cannot read artifact: {e}"))?;
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != expected_hash {
        return Err(format!("artifact hash mismatch: {}", path.display()));
    }
    Ok(())
}

pub fn report(file: &Path) -> Result<EvalReport, String> {
    let body = fs::read_to_string(file).map_err(|e| format!("cannot read eval file: {e}"))?;
    let dataset: Dataset =
        serde_json::from_str(&body).map_err(|e| format!("invalid eval JSON: {e}"))?;
    if dataset.schema != 1 || dataset.rubric_id.trim().is_empty() {
        return Err("eval schema must be 1 and rubric_id nonempty".into());
    }
    let real_file = fs::canonicalize(file).map_err(|e| format!("cannot open eval file: {e}"))?;
    let base = real_file
        .parent()
        .ok_or_else(|| "eval file has no parent directory".to_string())?;
    let mut cases = HashMap::new();
    for case in &dataset.cases {
        if case.task_id.trim().is_empty() || case.scenario.trim().is_empty() {
            return Err("eval case identifiers must be nonempty".into());
        }
        validate_artifact(base, &case.answer_key_artifact, &case.answer_key_sha256)?;
        if cases.insert(case.task_id.as_str(), case).is_some() {
            return Err(format!("duplicate eval case {}", case.task_id));
        }
    }
    let mut groups: HashMap<&str, (Option<&Trial>, Option<&Trial>)> = HashMap::new();
    let mut session_ids = std::collections::HashSet::new();
    for trial in &dataset.trials {
        for value in [
            &trial.pair_id,
            &trial.task_id,
            &trial.scenario,
            &trial.harness,
            &trial.model,
            &trial.machine_id,
            &trial.machine_source,
            &trial.session_id,
            &trial.prompt_sha256,
            &trial.source_snapshot,
            &trial.tool_policy_id,
        ] {
            if value.trim().is_empty() {
                return Err("eval trial identifiers must be nonempty".into());
            }
        }
        if trial.prompt_sha256.len() != 64
            || !trial.prompt_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("invalid prompt SHA-256 in pair {}", trial.pair_id));
        }
        if !session_ids.insert(trial.session_id.as_str()) {
            return Err(format!("duplicate eval session {}", trial.session_id));
        }
        let Some(case) = cases.get(trial.task_id.as_str()) else {
            return Err(format!("trial refers to missing case {}", trial.task_id));
        };
        if case.scenario != trial.scenario {
            return Err(format!(
                "trial scenario differs from case {}",
                trial.task_id
            ));
        }
        validate_artifact(base, &trial.artifact, &trial.artifact_sha256)?;
        let pair = groups.entry(&trial.pair_id).or_default();
        let arm = match trial.condition {
            Condition::MemoryOn => &mut pair.0,
            Condition::MemoryOff => &mut pair.1,
        };
        if arm.replace(trial).is_some() {
            return Err(format!("duplicate condition in pair {}", trial.pair_id));
        }
    }
    let mut out = EvalReport {
        rubric_id: dataset.rubric_id,
        total_trials: dataset.trials.len(),
        complete_pairs: 0,
        incomplete_pairs: Vec::new(),
        overall: PairSummary::default(),
        by_machine_harness_model: BTreeMap::new(),
        by_scenario: BTreeMap::new(),
        reading: "Reviewer-scored paired outcomes; artifact hashes guard provenance, not grade accuracy. Only complete pairs enter outcome totals. No causal claim follows from usage counts alone.".into(),
    };
    for (pair_id, (on, off)) in groups {
        let (Some(on), Some(off)) = (on, off) else {
            out.incomplete_pairs.push(pair_id.to_string());
            continue;
        };
        if on.task_id != off.task_id
            || on.scenario != off.scenario
            || on.harness != off.harness
            || on.model != off.model
            || on.machine_id != off.machine_id
            || on.machine_source != off.machine_source
            || on.prompt_sha256 != off.prompt_sha256
            || on.source_snapshot != off.source_snapshot
            || on.tool_policy_id != off.tool_policy_id
        {
            return Err(format!(
                "pair {pair_id} compares different task or environment"
            ));
        }
        out.complete_pairs += 1;
        out.overall.add(on, off);
        let group = format!(
            "{}:{}|{}|{}",
            on.machine_source, on.machine_id, on.harness, on.model
        );
        out.by_machine_harness_model
            .entry(group)
            .or_default()
            .add(on, off);
        out.by_scenario
            .entry(on.scenario.clone())
            .or_default()
            .add(on, off);
    }
    out.incomplete_pairs.sort();
    Ok(out)
}
