//! File discovery (gitignore-aware) + LOC / language stats
use crate::model::{DirStat, FileInfo, ProjectMap};
use ignore::WalkBuilder;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CODE_EXT: &[(&str, &str)] = &[
    ("js", "javascript"), ("jsx", "javascript"), ("mjs", "javascript"), ("cjs", "javascript"),
    ("ts", "typescript"), ("tsx", "typescript"), ("mts", "typescript"), ("cts", "typescript"),
    ("py", "python"),
    ("rs", "rust"),
    ("go", "go"),
    ("c", "c"), ("h", "c"),
    ("cpp", "cpp"), ("cc", "cpp"), ("cxx", "cpp"), ("hpp", "cpp"), ("hh", "cpp"),
    ("java", "java"),
    ("rb", "ruby"),
    ("php", "php"),
    ("cs", "csharp"),
    ("swift", "swift"),
    ("kt", "kotlin"),
    ("sh", "shell"),
    ("toml", "config"), ("json", "config"), ("yaml", "config"), ("yml", "config"),
    ("ts", "typescript"), // dup kept for clarity
];

fn lang_for_ext(ext: &str) -> Option<&'static str> {
    CODE_EXT.iter().find(|(e, _)| *e == ext).map(|(_, l)| *l)
}

/// Always-excluded directory names, even without gitignore.
const ALWAYS_EXCLUDE: &[&str] = &[
    ".git", ".hg", ".svn", "node_modules", "target", "dist", "build", "out",
    ".next", ".nuxt", ".venv", "venv", "__pycache__", ".pytest_cache", ".mypy_cache",
    ".cache", "coverage", ".idea", ".vscode", "vendor", "Pods", "DerivedData",
    ".terraform", ".claudecat", "pids", "logs", "legacy", "archive", "archived", "old",
];

pub fn collect_files(root: &Path, top_n: usize) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .ignore(true)
        .follow_links(false);
    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            !ALWAYS_EXCLUDE.contains(&name.as_ref())
        } else {
            true
        }
    });
    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if lang_for_ext(ext).is_some() {
                    files.push(entry.into_path());
                }
            }
        }
    }
    files.sort();
    // Cap to the largest top_n files by LOC (fast pre-count)
    if files.len() > top_n.saturating_mul(4) {
        // Keep it bounded even for huge repos
        files.truncate(top_n * 4);
    }
    files
}

pub fn count_loc(path: &Path) -> Option<(usize, usize)> {
    // returns (total_lines, non_blank_lines)
    let data = std::fs::read(path).ok()?;
    if data.len() > 4 * 1024 * 1024 {
        return None; // skip huge files
    }
    let text = String::from_utf8_lossy(&data);
    let total = text.lines().count();
    let non_blank = text.lines().filter(|l| !l.trim().is_empty()).count();
    Some((total, non_blank))
}

pub fn analyze_project(root: &Path, top_n: usize, max_loc_files: usize) -> ProjectMap {
    let mut map = ProjectMap {
        root: root.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let files = collect_files(root, max_loc_files);

    let mut dir_stats: BTreeMap<String, DirStat> = BTreeMap::new();
    let mut languages: BTreeMap<String, usize> = BTreeMap::new();
    let mut total_files = 0usize;
    let mut total_loc = 0usize;
    let mut candidates: Vec<FileInfo> = Vec::new();

    for f in &files {
        let Some((_, non_blank)) = count_loc(f) else { continue };
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = lang_for_ext(ext);
        total_files += 1;
        total_loc += non_blank;
        if let Some(l) = lang {
            *languages.entry(l.to_string()).or_insert(0) += 1;
        }
        // directory aggregates
        let rel = f.strip_prefix(root).unwrap_or(f);
        let dir = rel.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let ds = dir_stats.entry(if dir.is_empty() { ".".to_string() } else { dir.clone() }).or_default();
        ds.files += 1;
        ds.loc += non_blank;
        // also count for every ancestor dir
        let mut cur = PathBuf::from(if dir.is_empty() { ".".to_string() } else { dir.clone() });
        while cur != Path::new(".") {
            let key = cur.to_string_lossy().into_owned();
            let ds = dir_stats.entry(key).or_default();
            ds.loc += non_blank;
            cur = cur.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            if cur.as_os_str().is_empty() {
                break;
            }
        }
        candidates.push(FileInfo {
            path: rel.to_string_lossy().into_owned(),
            language: lang.map(|s| s.to_string()),
            loc: non_blank,
            symbols: vec![],
        });
    }

    // keep top_n by loc (parsed for symbols later)
    candidates.sort_by(|a, b| b.loc.cmp(&a.loc));
    let key_files: Vec<FileInfo> = candidates.into_iter().take(top_n).collect();

    map.total_files = total_files;
    map.total_loc = total_loc;
    map.languages = languages;
    map.dir_stats = dir_stats;
    map.key_files = key_files;
    map
}

/// Compute a compact nested tree of directories (recursive).
pub fn tree_lines(dir_stats: &BTreeMap<String, DirStat>, max_depth: usize, budget: usize) -> Vec<String> {
    // Build nested map: path comps -> children
    let mut out = Vec::new();
    let mut children: BTreeMap<String, Vec<(String, &DirStat)>> = BTreeMap::new();
    // Only consider top-level dirs for line budget
    let mut roots: Vec<String> = Vec::new();
    for (dir, stat) in dir_stats {
        let comps: Vec<&str> = dir.split('/').filter(|c| !c.is_empty()).collect();
        if dir == "." {
            continue;
        }
        if comps.len() == 1 {
            roots.push(comps[0].to_string());
        }
        let key = comps[0].to_string();
        children.entry(key).or_default().push((dir.clone(), stat));
    }
    roots.sort();
    let mut remaining = budget;
    for root_name in roots {
        if remaining == 0 {
            break;
        }
        remaining -= 1;
        let stats = children.get(&root_name).cloned().unwrap_or_default();
        let loc: usize = stats.iter().map(|(_, s)| s.loc).sum();
        let files: usize = stats.iter().map(|(_, s)| s.files).sum();
        let mut line = format!("{root_name}/ ({files} files, {loc} LOC)");
        // sub-dirs one level deep
        let mut subs: Vec<(String, &DirStat)> = stats.iter().filter(|(d, _)| {
            let comps: Vec<&str> = d.split('/').filter(|c| !c.is_empty()).collect();
            comps.len() == 2
        }).map(|(d, s)| (d.clone(), *s)).collect();
        subs.sort_by(|a, b| b.1.loc.cmp(&a.1.loc));
        if max_depth >= 2 && !subs.is_empty() {
            line.push_str("  [");
            let take = subs.len().min(6);
            let parts: Vec<String> = subs[..take].iter()
                .map(|(d, s)| {
                    let name = d.rsplit('/').next().unwrap_or(d).to_string();
                    format!("{name}({})", s.files)
                }).collect();
            line.push_str(&parts.join(", "));
            line.push(']');
        }
        out.push(line);
    }
    out
}
