use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProjectMeta {
    pub name: String,
    pub project_type: String,
    pub language: String,
    pub framework: String,
    pub package_manager: String,
    pub entry_points: Vec<String>,
    pub run_command: Option<String>,
    pub build_command: Option<String>,
    pub scripts: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub language: Option<String>,
    pub loc: usize,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DepGroup {
    pub ecosystem: String,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DirStat {
    pub files: usize,
    pub loc: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProjectMap {
    pub root: String,
    pub meta: ProjectMeta,
    /// relative path -> aggregate stats
    pub dir_stats: BTreeMap<String, DirStat>,
    pub total_files: usize,
    pub total_loc: usize,
    pub languages: BTreeMap<String, usize>, // lang -> file count
    pub key_files: Vec<FileInfo>,
    pub deps: Vec<DepGroup>,
    pub guardrails: Vec<String>,
    pub excluded_paths: Vec<String>,
    pub generated_at: String,
    pub errors: Vec<String>,
}
