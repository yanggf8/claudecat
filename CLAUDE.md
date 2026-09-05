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
- **Scale**: 18 files, 2348 LOC [config(1), rust(17)]

### Directory structure
```
src/ (11 files, 1875 LOC)
tests/ (6 files, 473 LOC)  [common(1)]
Total: 17 files, 2348 LOC (code)
```

### Key files & symbols
- `src/manifest.rs` (428 LOC, rust)  — 6:const PRIMARY_RANK; 15:fn detect_project_meta; 121:fn rank_of; 128:fn refine_package_manager; 140:fn parse_package_json; 201:fn parse_cargo_toml; 252:fn parse_pyproject; 292:fn parse_requirements; 314:fn parse_go_mod; 352:fn parse_gemfile; 379:fn parse_composer; 400:fn infer_framework
- `src/main.rs` (313 LOC, rust)  — 18:struct Cli; 24:enum Commands; 104:enum MapArg; 110:impl impl MapArg; 111:fn into_profile; 121:enum Format; 127:fn now_iso; 144:fn civil_from_days; 157:fn analyze; 196:fn main
- `tests/claudecat.rs` (264 LOC, rust)  — 5:fn manifest_detects_rust_and_deps; 18:fn symbols_extract_rust_items; 28:fn symbols_skip_nested_fn_noise; 39:fn claude_md_update_is_idempotent_and_atomic; 54:fn temp_project; 61:fn rand_suffix; 67:fn guardrails_preserved_and_seeded; 87:fn guardrails_load_from_file; 96:fn explore_report_has_savings_section; 115:fn claudecat_lib_scan; 144:fn track_appends_and_updates_same_day_row; 165:fn auto_profile_picks_mini_for_small_project; 179:fn auto_profile_picks_full_for_large_project; 194:fn track_update_handles_multiple_repos; 216:fn grok_regression_dir_loc_no_double_count; 231:fn grok_regression_track_keeps_sibling_repo; 253:fn grok_regression_update_root_stays_local; 266:fn navigate_finds_symbol_and_route
- `src/walk.rs` (231 LOC, rust)  — 9:const CODE_EXT; 28:fn lang_for_ext; 34:const ALWAYS_EXCLUDE; 44:fn collect_files; 93:fn count_loc; 105:fn direct_stat; 110:fn rollup; 130:fn analyze_project; 188:fn tree_lines; 240:fn total_loc_files
- `src/explore.rs` (216 LOC, rust)  — 9:const TRACK_SECTION; 12:struct CoverageRow; 19:struct ExploreMetrics; 33:fn est_tokens_from_loc; 37:fn compute; 41:fn compute_with_profile; 86:fn render; 124:fn row_md; 142:fn track_update; 232:fn track_append
- `src/symbols.rs` (183 LOC, rust)  — 5:fn lang_for; 19:fn interesting_kinds; 37:fn name_of; 108:fn is_plain_name; 112:fn kind_label; 135:const FUNCTION_LIKE; 141:fn is_nested; 157:fn extract_symbols
- `src/outline.rs` (156 LOC, rust)  — 5:fn render_markdown; 9:fn render_with_profile; 16:fn render_full; 123:fn render_mini
- `src/navigate.rs` (140 LOC, rust)  — 10:struct NavigateHit; 19:struct NavigateResult; 27:fn tokens; 36:fn symbol_matches; 42:fn navigate; 118:fn render
- `src/model.rs` (81 LOC, rust)  — 5:struct ProjectMeta; 18:struct Symbol; 25:struct FileInfo; 33:struct DepGroup; 39:struct DirStat; 45:struct ProjectMap; 63:enum MapProfile; 71:impl impl MapProfile; 72:fn is_mini; 78:fn resolve_profile
- `src/claude_md.rs` (74 LOC, rust)  — 5:const BEGIN_MARKER; 6:const END_MARKER; 8:fn find_claude_md; 16:fn update_section
- `tests/walk_test.rs` (56 LOC, rust)  — 1:mod common; 5:fn analyze_project_counts_files_loc_and_langs; 40:fn top_n_limits_key_files; 51:fn tree_lines_compacts_structure
- `tests/cli_test.rs` (51 LOC, rust)  — 1:mod common; 4:fn bin; 9:fn scan_json_is_valid_and_complete; 25:fn update_dry_run_does_not_write; 39:fn update_writes_section_and_is_idempotent
- `src/guardrails.rs` (44 LOC, rust)  — 5:const GR_BEGIN; 6:const GR_END; 9:fn extract; 23:fn load; 41:fn seed_block; 47:fn has_marker
- `tests/manifest_test.rs` (40 LOC, rust)  — 1:mod common; 5:fn package_json_full_parse; 21:fn pyproject_and_requirements; 35:fn go_mod_detection
- `tests/symbols_test.rs` (40 LOC, rust)  — 1:mod common; 5:fn typescript_interfaces_types_enums_classes; 17:fn python_functions_classes_decorators; 28:fn go_functions_methods; 38:fn c_functions_structs

### Dependencies (declared)
- `crates.io`: clap, ignore, serde, serde_json, tempfile, toml, tree-sitter, tree-sitter-c, tree-sitter-cpp, tree-sitter-go, tree-sitter-javascript, tree-sitter-python, tree-sitter-rust, tree-sitter-typescript

*Generated at 2026-09-05T02:55:02Z by claudecat* — facts from manifests + AST; framework/entry may be inferred from deps/paths where manifest lacks them.
<!-- claudecat:auto:end -->
<!-- claudecat:guardrails:begin -->
<!-- 技術決策 / Guardrails：每行一條，例如 `2D tilemap + Macroquad（禁 Python/3D）`、`插件一律裝在 Claude Code 內`。claudecat 只在此區不存在時建立，之後永不覆寫。 -->
<!-- claudecat:guardrails:end -->
