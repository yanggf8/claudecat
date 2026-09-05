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
    // 足夠大的 fixture（>420 行 code），full map 才能穩定低於全讀成本
    let mut body = String::from("use std::collections::HashMap;\n\nfn main() {}\n");
    for i in 0..420 {
        body.push_str(&format!("pub fn worker_{i}() -> usize {{ {i} }}\n"));
    }
    fs::write(dir.join("src/main.rs"), &body).unwrap();
    let map = claudecat_lib_scan(&dir);
    let m = claudecat::explore::compute(&map);
    let report = claudecat::explore::render(&m);
    assert!(report.contains("Map vs full-read") || report.contains("Map overhead"));
    assert!(report.contains("覆蓋率"));
    assert!(m.read_tokens > 0 && m.map_tokens > 0);
    assert!(m.savings_pct > 0.0);
}

fn claudecat_lib_scan(root: &std::path::Path) -> claudecat::model::ProjectMap {
    let mut map = claudecat::walk::analyze_project(root, 10, None);
    let (meta, deps) = claudecat::manifest::detect_project_meta(root);
    map.meta = meta;
    map.deps = deps;
    // 與 main.rs 的 analyze() 一致：對 key files 抽 symbols
    for f in &mut map.key_files {
        if let Some(lang) = &f.language {
            if let Ok(src) = std::fs::read_to_string(root.join(&f.path)) {
                let l = match lang.as_str() {
                    "typescript" => "typescript",
                    "javascript" => "javascript",
                    "python" => "python",
                    "rust" => "rust",
                    "go" => "go",
                    "c" => "c",
                    "cpp" => "cpp",
                    _ => continue,
                };
                f.symbols = claudecat::symbols::extract_symbols(l, &src);
            }
        }
    }
    map.generated_at = "2026-09-05T00:00:00Z".into();
    map
}


#[test]
fn track_appends_and_updates_same_day_row() {
    let dir = temp_project();
    let target = dir.join("SESSION-EVIDENCE.md");
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let map = claudecat_lib_scan(&dir);
    let m = claudecat::explore::compute(&map);
    let (changed, _) = claudecat::explore::track_append(&target, &m).unwrap();
    assert!(changed);
    let content1 = fs::read_to_string(&target).unwrap();
    assert!(content1.contains("## 長期指標 (claudecat explore)"));
    assert!(content1.contains(m.date.as_str()));
    // same-day second run: row replaced, not duplicated
    let (changed2, _) = claudecat::explore::track_append(&target, &m).unwrap();
    assert!(!changed2, "same date+project row should be idempotent");
    let content2 = fs::read_to_string(&target).unwrap();
    assert_eq!(content2.matches(m.date.as_str()).count(), 1);
}

#[test]
fn auto_profile_picks_mini_for_small_project() {
    let dir = temp_project();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"tiny\"\n").unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let map = claudecat_lib_scan(&dir);
    let profile = claudecat::model::resolve_profile(map.total_loc, map.total_files, None);
    assert!(profile.is_mini(), "small project should resolve to Mini, got {:?}", profile);
    let md = claudecat::outline::render_with_profile(&map, profile);
    assert!(md.contains("Mini"));
    assert!(!md.contains("Key files & symbols"), "mini must skip symbols");
}

#[test]
fn auto_profile_picks_full_for_large_project() {
    let dir = temp_project();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"big\"\n").unwrap();
    let mut body = String::from("fn main() {}\n");
    for i in 0..500 {
        body.push_str(&format!("pub fn f{i}() -> usize {{ {i} }}\n"));
    }
    fs::write(dir.join("src/main.rs"), &body).unwrap();
    let map = claudecat_lib_scan(&dir);
    let profile = claudecat::model::resolve_profile(map.total_loc, map.total_files, None);
    assert!(!profile.is_mini(), "large project should resolve to Full");
}

#[test]
fn track_update_handles_multiple_repos() {
    let dir = temp_project();
    let target = dir.join("METRICS.md");
    let r1 = dir.join("repo1");
    let r2 = dir.join("repo2");
    for r in [&r1, &r2] {
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join("Cargo.toml"), "[package]\nname=\"r\"\n").unwrap();
        fs::write(r.join("src/main.rs"), "fn main() {}\n").unwrap();
    }
    let m1 = claudecat::explore::compute(&claudecat_lib_scan(&r1));
    let m2 = claudecat::explore::compute(&claudecat_lib_scan(&r2));
    let refs = vec![&m1, &m2];
    let (changed, _) = claudecat::explore::track_update(&target, &refs).unwrap();
    assert!(changed);
    let content = fs::read_to_string(&target).unwrap();
    assert_eq!(content.matches("| 日期").count(), 1, "single header expected");
    assert_eq!(content.matches(&format!("`{}`", r1.display())).count(), 1);
    assert_eq!(content.matches(&format!("`{}`", r2.display())).count(), 1);
}

#[test]
fn grok_regression_dir_loc_no_double_count() {
    let dir = temp_project();
    fs::create_dir_all(dir.join("src/sub")).unwrap();
    fs::write(dir.join("src/a.rs"), "fn a(){}\n").unwrap();
    fs::write(dir.join("src/sub/b.rs"), "fn b(){}\n").unwrap();
    let map = claudecat_lib_scan(&dir);
    // 每個子目錄 loc 加總 == total_loc（雙計會讓它大於）
    let sum: usize = map.dir_stats.values().map(|d| d.loc).sum();
    assert_eq!(sum, map.total_loc, "目錄 LOC 不得雙計");
    let src = map.dir_stats.get("src").unwrap();
    let sub = map.dir_stats.get("src/sub").unwrap();
    assert_eq!(src.loc + sub.loc, map.total_loc);
}

#[test]
fn grok_regression_track_keeps_sibling_repo() {
    let dir = temp_project();
    let target = dir.join("M.md");
    let foo = dir.join("foo");
    let foobar = dir.join("foo-bar");
    for r in [&foo, &foobar] {
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join("src/main.rs"), "fn main(){}\n").unwrap();
    }
    let m1 = claudecat::explore::compute(&claudecat_lib_scan(&foo));
    let m2 = claudecat::explore::compute(&claudecat_lib_scan(&foobar));
    claudecat::explore::track_update(&target, &[&m1]).unwrap();
    claudecat::explore::track_update(&target, &[&m2]).unwrap();
    // 更新 foo 時不得刪掉 foo-bar
    let m1b = claudecat::explore::compute(&claudecat_lib_scan(&foo));
    claudecat::explore::track_update(&target, &[&m1b]).unwrap();
    let content = fs::read_to_string(&target).unwrap();
    assert!(content.contains("foo-bar"), "sibling repo row must survive");
    assert!(content.contains(&format!("`{}`", foo.display())));
}

#[test]
fn grok_regression_update_root_stays_local() {
    let dir = temp_project();
    let deep = dir.join("sub/deep");
    fs::create_dir_all(&deep).unwrap();
    fs::write(dir.join("CLAUDE.md"), "# parent\n").unwrap();
    let section = "x";
    let path = claudecat::claude_md::find_claude_md(&deep);
    // 直接 join 回傳 root/CLAUDE.md，而非向上找父專案
    assert_eq!(path, deep.join("CLAUDE.md"));
    let _ = section;
}

#[test]
fn navigate_finds_symbol_and_route() {
    let dir = temp_project();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "mod auth;\nfn main() {}\npub fn authenticate(name: &str) -> bool { true }\npub fn create_user() -> usize { 1 }\n",
    )
    .unwrap();
    let map = claudecat_lib_scan(&dir);
    let r = claudecat::navigate::navigate(&map, "auth");
    assert!(r.symbols.iter().any(|h| h.name == "authenticate"), "should hit authenticate");
    // mod auth 精確命中優先；路線應含 cort context <命中符號>
    assert!(r.route.iter().any(|s| s.contains("cort context")), "route should suggest cort context");
    let report_auth = claudecat::navigate::render(&r);
    assert!(report_auth.contains("auth") || report_auth.contains("authenticate"));
    let report = claudecat::navigate::render(&r);
    assert!(report.contains("路線"));
}
