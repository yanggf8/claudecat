# CLAUDE.md

本檔案提供 Claude Code (claude.ai/code) 在本 repo 工作時的指引。
`<!-- claudecat:auto:begin -->
## Project Map (auto-maintained by claudecat)
- **Root**: `/home/yanggf/a/claudecat`
- **Type**: Rust application/library
- **Language**: Rust
- **Package manager**: cargo
- **Entry points**: claudecat (src/main.rs), src/main.rs
- **Run**: `cargo run`
- **Build**: `cargo build`
- **Scale**: 17 files, 1553 LOC [config(1), rust(16)]

### Directory structure
```
src/ (10 files, 2432 LOC)
tests/ (6 files, 638 LOC)  [common(1)]
```

### Key files & symbols
- `src/manifest.rs` (340 LOC, rust)  — 5:fn detect_project_meta; 104:fn merge_meta; 129:fn parse_package_json; 184:fn parse_cargo_toml; 224:fn parse_pyproject; 265:fn parse_requirements; 287:fn parse_go_mod; 309:fn infer_framework
- `src/walk.rs` (191 LOC, rust)  — 7:const CODE_EXT; 26:fn lang_for_ext; 31:const ALWAYS_EXCLUDE; 38:fn collect_files; 82:fn count_loc; 94:fn analyze_project; 154:fn tree_lines
- `src/symbols.rs` (183 LOC, rust)  — 5:fn lang_for; 19:fn interesting_kinds; 37:fn name_of; 108:fn is_plain_name; 112:fn kind_label; 135:const FUNCTION_LIKE; 141:fn is_nested; 157:fn extract_symbols
- `src/main.rs` (173 LOC, rust)  — 17:struct Cli; 23:enum Commands; 60:enum Format; 66:fn now_iso; 83:fn civil_from_days; 96:fn analyze; 135:fn main
- `tests/claudecat.rs` (103 LOC, rust)  — 5:fn manifest_detects_rust_and_deps; 18:fn symbols_extract_rust_items; 28:fn symbols_skip_nested_fn_noise; 39:fn claude_md_update_is_idempotent_and_atomic; 54:fn temp_project; 61:fn rand_suffix; 67:fn guardrails_preserved_and_seeded; 87:fn guardrails_load_from_file; 96:fn explore_report_has_savings_section; 107:fn claudecat_lib_scan
- `src/outline.rs` (98 LOC, rust)  — 5:fn render_markdown
- `src/claude_md.rs` (73 LOC, rust)  — 5:const BEGIN_MARKER; 6:const END_MARKER; 8:fn find_claude_md; 27:fn update_section
- `src/explore.rs` (53 LOC, rust)  — 5:fn est_tokens_from_loc; 9:fn render_explore
- `src/model.rs` (53 LOC, rust)  — 5:struct ProjectMeta; 18:struct Symbol; 25:struct FileInfo; 33:struct DepGroup; 39:struct DirStat; 45:struct ProjectMap
- `tests/walk_test.rs` (52 LOC, rust)  — 1:mod common; 5:fn analyze_project_counts_files_loc_and_langs; 36:fn top_n_limits_key_files; 47:fn tree_lines_compacts_structure
- `tests/cli_test.rs` (51 LOC, rust)  — 1:mod common; 4:fn bin; 9:fn scan_json_is_valid_and_complete; 25:fn update_dry_run_does_not_write; 39:fn update_writes_section_and_is_idempotent
- `src/guardrails.rs` (44 LOC, rust)  — 5:const GR_BEGIN; 6:const GR_END; 9:fn extract; 23:fn load; 41:fn seed_block; 47:fn has_marker
- `tests/manifest_test.rs` (40 LOC, rust)  — 1:mod common; 5:fn package_json_full_parse; 21:fn pyproject_and_requirements; 35:fn go_mod_detection
- `tests/symbols_test.rs` (40 LOC, rust)  — 1:mod common; 5:fn typescript_interfaces_types_enums_classes; 17:fn python_functions_classes_decorators; 28:fn go_functions_methods; 38:fn c_functions_structs
- `Cargo.toml` (29 LOC, config)

### Dependencies (declared)
- `crates.io`: clap, ignore, serde, serde_json, tempfile, toml, tree-sitter, tree-sitter-c, tree-sitter-cpp, tree-sitter-go, tree-sitter-javascript, tree-sitter-python, tree-sitter-rust, tree-sitter-typescript

*Generated at 2026-09-04T16:50:51Z by claudecat* — facts from manifests + AST, no inference.
<!-- claudecat:auto:end -->
<!-- claudecat:guardrails:begin -->
<!-- 技術決策 / Guardrails：每行一條，例如 `2D tilemap + Macroquad（禁 Python/3D）`、`插件一律裝在 Claude Code 內`。claudecat 只在此區不存在時建立，之後永不覆寫。 -->
<!-- claudecat:guardrails:end -->
