use std::fs;


#[test]
fn manifest_detects_rust_and_deps() {
    let dir = temp_project();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n[dependencies]\nserde = \"1\"\nclap = \"4\"\n",
    )
    .unwrap();
    let (meta, deps) = claudecat::manifest::detect_project_meta(&dir);
    assert_eq!(meta.language, "Rust");
    assert!(deps.iter().any(|g| g.ecosystem == "crates.io" && g.deps.contains(&"serde".to_string())));
}

#[test]
fn symbols_extract_rust_items() {
    let src = "pub struct User { name: String }\nimpl User { fn greet(&self) {} }\npub fn main() {}\nmod foo;\n";
    let syms = claudecat::symbols::extract_symbols("rust", src);
    let names: Vec<String> = syms.iter().map(|s| s.name.clone()).collect();
    assert!(names.iter().any(|n| n == "User"));
    assert!(names.iter().any(|n| n == "main"));
    assert!(syms.iter().any(|s| s.name.starts_with("impl")));
}

#[test]
fn symbols_skip_nested_fn_noise() {
    let src = "function outer() {\n  const tmp = 1;\n  function inner() {}\n}\nclass A { method() {} }\n";
    let syms = claudecat::symbols::extract_symbols("javascript", src);
    // nested const / inner fn must not be reported; class A + method() should
    assert!(!syms.iter().any(|s| s.name == "tmp"));
    assert!(!syms.iter().any(|s| s.name == "inner"));
    assert!(syms.iter().any(|s| s.name == "A"));
    assert!(syms.iter().any(|s| s.name == "method"));
}

#[test]
fn claude_md_update_is_idempotent_and_atomic() {
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# My Project\n\nsome content\n").unwrap();
    let section = "## Map\n- x: 1\n";
    let (changed, content) = claudecat::claude_md::update_section(&path, section, false).unwrap();
    assert!(changed);
    let (changed2, _) = claudecat::claude_md::update_section(&path, section, false).unwrap();
    assert!(!changed2, "second update must be a no-op");
    assert!(content.contains("<!-- claudecat:auto:begin -->"));
    assert!(content.contains("# My Project"));
    // no temp leftovers
    assert!(fs::read_dir(dir).unwrap().all(|e| e.unwrap().file_name() != ".claudecat.tmp"));
}

fn temp_project() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("claudecat-test-{}", std::process::id()));
    let dir = base.join(format!("{}", rand_suffix()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

#[test]
fn guardrails_preserved_and_seeded() {
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# P\n").unwrap();
    let section = "## Map\n";
    let (_, content) = claudecat::claude_md::update_section(&path, section, false).unwrap();
    assert!(content.contains("claudecat:guardrails:begin"));
    // user adds a decision inside the guardrail block
    let edited = content.replace(
        "<!-- 技術決策 / Guardrails：每行一條",
        "<!-- 技術決策 / Guardrails：每行一條\n- 2D tilemap + Macroquad（禁 Python/3D）",
    );
    fs::write(&path, &edited).unwrap();
    // second update: marker already exists -> must NOT be re-seeded/overwritten
    let (_, content2) = claudecat::claude_md::update_section(&path, section, false).unwrap();
    assert!(content2.contains("2D tilemap + Macroquad"));
    assert_eq!(content2.matches("claudecat:guardrails:begin").count(), 1);
}

#[test]
fn guardrails_load_from_file() {
    let dir = temp_project();
    let path = dir.join("claudecat-guardrails.md");
    fs::write(&path, "# decisions\n- 插件一律裝在 Claude Code 內\n").unwrap();
    let items = claudecat::guardrails::load(&dir, "");
    assert!(items.iter().any(|i| i.contains("插件一律裝")));
}

#[test]
fn explore_report_has_savings_section() {
    let dir = temp_project();
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\nfn helper() {}\n").unwrap();
    let map = claudecat_lib_scan(&dir);
    let report = claudecat::explore::render_explore(&map);
    assert!(report.contains("Estimated token savings"));
    assert!(report.contains("覆蓋率"));
}

fn claudecat_lib_scan(root: &std::path::Path) -> claudecat::model::ProjectMap {
    let mut map = claudecat::walk::analyze_project(root, 10, 80);
    let (meta, deps) = claudecat::manifest::detect_project_meta(root);
    map.meta = meta;
    map.deps = deps;
    map.generated_at = "2026-09-05T00:00:00Z".into();
    map
}
