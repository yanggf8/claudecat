//! Atomic update of the claudecat auto section inside CLAUDE.md
use std::fs;
use std::path::{Path, PathBuf};

pub const BEGIN_MARKER: &str = "<!-- claudecat:auto:begin -->";
pub const END_MARKER: &str = "<!-- claudecat:auto:end -->";

pub fn find_claude_md(root: &Path) -> PathBuf {
    // V2: 只寫 --root/CLAUDE.md，不做向上搜尋（避免子目錄汙染父專案）。
    // 需要父專案時請自行指定，或之後提供 --discover-claude-md。
    root.join("CLAUDE.md")
}

/// Update (or insert) the claudecat section in `path`.
/// Returns (changed: bool, new_content: String).
pub fn update_section(
    path: &Path,
    section: &str,
    dry_run: bool,
) -> std::io::Result<(bool, String)> {
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
        // CLAUDE.md 常見是 symlink（如 cortexyoung：CLAUDE.md -> AGENTS.md，讓兩個
        // harness 永不漂移）。rename 直接蓋 path 會把 symlink 換成普通檔——
        // 寫入目標必須是解析後的本體；tmp 也放目標目錄，rename 才是同檔案系統原子操作。
        let target = match fs::read_link(path) {
            Ok(link) if link.is_absolute() => link,
            Ok(link) => path.parent().unwrap_or_else(|| Path::new(".")).join(link),
            Err(_) => path.to_path_buf(),
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // atomic write: temp file + rename（唯一 tmp 名 + fsync）
        let tmp = target.with_extension(format!(
            "claudecat.{}.{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp)?;
            f.write_all(new_content.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &target)?;
    }
    Ok((changed, new_content))
}
