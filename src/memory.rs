//! A small, reviewable project-memory ledger. The file lives in the repository so both coding
//! agents and other machines can read the same records after Git synchronization. A remembered
//! claim is never treated as proof: recall reports whether its source file is still byte-identical.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: u32 = 1;
const LEDGER_PATH: &str = ".claudecat/memory.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub claim: String,
    pub source: String,
    pub source_line: usize,
    pub source_sha256: String,
    pub recorded_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryLedger {
    pub schema: u32,
    pub entries: Vec<MemoryEntry>,
}

impl Default for MemoryLedger {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryHit {
    #[serde(flatten)]
    pub entry: MemoryEntry,
    /// `source_unchanged` checks bytes only. It does not prove the claim follows from the source.
    pub evidence_status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryRecall {
    /// An absent ledger is different from a present ledger with no matching entries.
    pub ledger_status: String,
    pub hits: Vec<MemoryHit>,
}

pub fn ledger_path(root: &Path) -> PathBuf {
    root.join(LEDGER_PATH)
}

pub fn load(root: &Path) -> Result<MemoryLedger, String> {
    let path = ledger_path(root);
    reject_ledger_symlinks(root)?;
    let body = match fs::read_to_string(&path) {
        Ok(body) => body,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(MemoryLedger::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let ledger: MemoryLedger =
        serde_json::from_str(&body).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    validate(&ledger)?;
    Ok(ledger)
}

fn reject_ledger_symlinks(root: &Path) -> Result<(), String> {
    for path in [root.join(".claudecat"), ledger_path(root)] {
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "memory path must not be a symlink: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate(ledger: &MemoryLedger) -> Result<(), String> {
    if ledger.schema != SCHEMA {
        return Err(format!("unsupported memory schema {}", ledger.schema));
    }
    let mut ids = HashSet::new();
    let by_id: HashMap<_, _> = ledger.entries.iter().map(|e| (e.id.as_str(), e)).collect();
    for entry in &ledger.entries {
        if entry.id.is_empty() || !ids.insert(entry.id.as_str()) {
            return Err(format!("empty or duplicate memory id: {}", entry.id));
        }
        if entry.key.trim().is_empty() || entry.claim.trim().is_empty() {
            return Err(format!("empty key or claim in memory {}", entry.id));
        }
        if entry.source_line == 0 || !safe_relative(Path::new(&entry.source)) {
            return Err(format!("invalid source in memory {}", entry.id));
        }
        if let Some(old) = &entry.supersedes {
            let Some(prior) = by_id.get(old.as_str()) else {
                return Err(format!("memory {} supersedes missing id {old}", entry.id));
            };
            // Supersession is append-only: referring to a later record would allow cycles and
            // could silently leave a key with no active answer after a hand-edited Git merge.
            if prior.key != entry.key || old == &entry.id || !ids.contains(old.as_str()) {
                return Err(format!("memory {} has invalid supersession", entry.id));
            }
        }
    }
    Ok(())
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn source_bytes(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    if !safe_relative(relative) {
        return Err("source must be a relative path inside the repository".into());
    }
    let real_root = fs::canonicalize(root).map_err(|e| format!("cannot open project root: {e}"))?;
    let real_source = fs::canonicalize(real_root.join(relative))
        .map_err(|e| format!("cannot open source {}: {e}", relative.display()))?;
    if !real_source.starts_with(&real_root) || !real_source.is_file() {
        return Err("source resolves outside the repository or is not a file".into());
    }
    fs::read(&real_source).map_err(|e| format!("cannot read source {}: {e}", relative.display()))
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn active_entries(ledger: &MemoryLedger) -> Vec<&MemoryEntry> {
    let replaced: HashSet<_> = ledger
        .entries
        .iter()
        .filter_map(|entry| entry.supersedes.as_deref())
        .collect();
    ledger
        .entries
        .iter()
        .filter(|entry| !replaced.contains(entry.id.as_str()))
        .collect()
}

/// Record an explicit claim with a source line. The caller must review whether that line actually
/// supports the claim; the program can only check that the line exists and detect later file edits.
pub fn record(
    root: &Path,
    key: &str,
    claim: &str,
    source: &Path,
    source_line: usize,
    supersedes: Option<&str>,
) -> Result<MemoryEntry, String> {
    let key = key.trim();
    let claim = claim.trim();
    if key.is_empty() || claim.is_empty() {
        return Err("key and claim must be nonempty".into());
    }
    let bytes = source_bytes(root, source)?;
    let source_text = std::str::from_utf8(&bytes).map_err(|_| "source is not UTF-8")?;
    if source_line == 0
        || source_text
            .lines()
            .nth(source_line - 1)
            .is_none_or(|line| line.trim().is_empty())
    {
        return Err("source line does not exist".into());
    }
    // A separate lock file survives atomic replacement of memory.json. Locking the ledger inode
    // itself would let a second writer lock the new inode and lose an update from the first.
    let real_root = fs::canonicalize(root).map_err(|e| format!("cannot open project root: {e}"))?;
    let lock_path =
        crate::data_dir::project_data_dir(&real_root.to_string_lossy()).join("memory.lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create memory lock dir: {e}"))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| format!("cannot open memory lock: {e}"))?;
    lock.lock()
        .map_err(|e| format!("cannot lock memory: {e}"))?;
    let mut ledger = load(root)?;
    let active = active_entries(&ledger);
    if active.iter().filter(|e| e.key == key).count() > 1 {
        return Err(
            "conflicting active entries for this key; resolve the ledger before recording".into(),
        );
    }
    match supersedes {
        Some(id) => {
            if !active.iter().any(|e| e.id == id && e.key == key) {
                return Err("superseded id must be the active entry for the same key".into());
            }
        }
        None if active.iter().any(|e| e.key == key) => {
            return Err("an active entry has this key; pass --supersedes with its id".into());
        }
        None => {}
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("clock error: {e}"))?
        .as_nanos();
    let source_name = source.to_string_lossy().to_string();
    let source_sha256 = hash(&bytes);
    let id =
        hash(format!("{key}\0{claim}\0{source_name}\0{source_sha256}\0{timestamp}").as_bytes());
    let entry = MemoryEntry {
        id,
        key: key.to_string(),
        claim: claim.to_string(),
        source: source_name,
        source_line,
        source_sha256,
        recorded_at: timestamp.to_string(),
        supersedes: supersedes.map(str::to_string),
    };
    ledger.entries.push(entry.clone());
    validate(&ledger)?;
    let body = serde_json::to_string_pretty(&ledger).map_err(|e| e.to_string())? + "\n";
    crate::data_dir::write_atomic(&ledger_path(root), &body)
        .map_err(|e| format!("cannot write memory ledger: {e}"))?;
    Ok(entry)
}

pub fn recall(root: &Path, query: &str) -> Result<MemoryRecall, String> {
    let present = ledger_path(root)
        .try_exists()
        .map_err(|e| format!("cannot check memory ledger: {e}"))?;
    let ledger = load(root)?;
    let terms: Vec<_> = query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let active = active_entries(&ledger);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for entry in &active {
        *counts.entry(&entry.key).or_default() += 1;
    }
    let mut hits = Vec::new();
    for entry in active {
        let haystack = format!("{} {}", entry.key, entry.claim).to_lowercase();
        if !terms.iter().all(|term| haystack.contains(term)) {
            continue;
        }
        let evidence_status = if counts[entry.key.as_str()] > 1 {
            "conflict"
        } else {
            match source_bytes(root, Path::new(&entry.source)) {
                Ok(bytes) if hash(&bytes) == entry.source_sha256 => "source_unchanged",
                Ok(_) => "source_changed",
                Err(_) => "source_missing",
            }
        };
        hits.push(MemoryHit {
            entry: entry.clone(),
            evidence_status: evidence_status.to_string(),
        });
    }
    hits.sort_by(|a, b| {
        a.entry
            .key
            .cmp(&b.entry.key)
            .then(a.entry.id.cmp(&b.entry.id))
    });
    Ok(MemoryRecall {
        ledger_status: if present { "present" } else { "absent" }.into(),
        hits,
    })
}
