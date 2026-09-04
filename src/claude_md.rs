//! Atomic update of the claudecat auto section inside CLAUDE.md
use std::fs;
use std::path::{Path, PathBuf};

pub const BEGIN_MARKER: &str = "<!-- claudecat:auto:begin -->";
pub const END_MARKER: &str = "<!-- claudecat:auto:end -->";

pub fn find_claude_md(root: &Path) -> PathBuf {
    let direct = root.join("CLAUDE.md");
    if direct.is_file() {
        return direct;
    }
    // walk up looking for CLAUDE.md (max 4 levels)
    let mut cur = root.to_path_buf();
    for _ in 0..4 {
        cur = cur.parent().map(|p| p.to_path_buf()).unwrap_or(cur.clone());
        let candidate = cur.join("CLAUDE.md");
        if candidate.is_file() {
            return candidate;
        }
    }
    direct
}

/// Update (or insert) the claudecat section in `path`.
/// Returns (changed: bool, new_content: String).
pub fn update_section(path: &Path, section: &str, dry_run: bool) -> std::io::Result<(bool, String)> {
    let existing = if path.is_file() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let block = format!("{BEGIN_MARKER}\n{section}{END_MARKER}\n");

    let mut new_content = if let Some(begin) = existing.find(BEGIN_MARKER) {
        let before = &existing[..begin];
        if let Some(end_rel) = existing[begin..].find(END_MARKER) {
            let end = begin + end_rel + END_MARKER.len();
            let after = existing[end..].trim_start_matches(['\n', '\r']);
            format!("{before}{block}{after}")
        } else {
            format!("{before}{block}")
        }
    } else {
        // Append at end
        let mut out = existing.clone();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&block);
        out
    };

    let mut changed = new_content != existing.as_str();

    // First-time setup: seed a guardrails block (never overwrite after creation)
    if !crate::guardrails::has_marker(&new_content) {
        let mut with_seed = new_content.clone();
        if !with_seed.ends_with('\n') {
            with_seed.push('\n');
        }
        with_seed.push_str(&crate::guardrails::seed_block());
        changed |= with_seed != existing.as_str();
        if changed && !dry_run {
            new_content = with_seed;
        }
    }

    if changed && !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // atomic write: temp file + rename
        let tmp = path.with_extension("claudecat.tmp");
        fs::write(&tmp, &new_content)?;
        fs::rename(&tmp, path)?;
    }
    Ok((changed, new_content))
}
