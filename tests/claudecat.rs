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
    assert!(deps
        .iter()
        .any(|g| g.ecosystem == "crates.io" && g.deps.contains(&"serde".to_string())));
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
    let src =
        "function outer() {\n  const tmp = 1;\n  function inner() {}\n}\nclass A { method() {} }\n";
    let syms = claudecat::symbols::extract_symbols("javascript", src);
    // nested const / inner fn must not be reported; class A + method() should
    assert!(!syms.iter().any(|s| s.name == "tmp"));
    assert!(!syms.iter().any(|s| s.name == "inner"));
    assert!(syms.iter().any(|s| s.name == "A"));
    assert!(syms.iter().any(|s| s.name == "method"));
}

#[test]
fn navigate_returns_document_ranges_without_turning_headings_into_symbols() {
    let map = claudecat::model::ProjectMap {
        document_headings: vec![claudecat::model::DocumentHeading {
            path: "README.md".into(),
            heading_path: vec!["Documented limitations".into()],
            start_line: 10,
            end_line: 22,
            preview: "coverage is a recall screen".into(),
        }],
        ..Default::default()
    };
    let result = claudecat::navigate::navigate(&map, "coverage");
    assert!(result.symbols.is_empty());
    assert_eq!(result.documents[0].start_line, 10);
    assert!(result
        .route
        .iter()
        .any(|line| line.contains("cort read README.md")));
}

#[test]
fn cort_navigation_keeps_document_route_when_no_symbol_matches() {
    let map = claudecat::model::ProjectMap {
        root: ".".into(),
        document_headings: vec![claudecat::model::DocumentHeading {
            path: "README.md".into(),
            heading_path: vec!["Completeness".into()],
            start_line: 3,
            end_line: 9,
            preview: "coverage is a recall screen".into(),
        }],
        ..Default::default()
    };
    let result = claudecat::navigate::navigate_with_cort(&map, "completeness", vec![], false);
    assert!(result.symbols.is_empty());
    assert!(result
        .route
        .iter()
        .any(|line| line.contains("cort read README.md")));
}

#[test]
fn navigation_usage_is_local_and_does_not_store_raw_query() {
    let _lock = cort_env_lock();
    let data = temp_project();
    let _env = EnvVarGuard(
        "CLAUDECAT_DATA_DIR".into(),
        std::env::var("CLAUDECAT_DATA_DIR").ok(),
    );
    std::env::set_var("CLAUDECAT_DATA_DIR", &data);
    let map = claudecat::model::ProjectMap {
        document_headings: vec![claudecat::model::DocumentHeading {
            path: "README.md".into(),
            heading_path: vec!["Coverage".into()],
            start_line: 2,
            end_line: 8,
            preview: "a recall screen".into(),
        }],
        ..Default::default()
    };
    let result = claudecat::navigate::navigate(&map, "private navigation phrase");
    claudecat::usage::record_navigation(
        std::path::Path::new("/tmp/demo-project"),
        "private navigation phrase",
        true,
        1,
        &result,
    );
    let report = claudecat::usage::report(30).unwrap();
    assert_eq!(report.events, 1);
    assert_eq!(report.cort_events, 1);
    let raw = std::fs::read(data.join("usage.db")).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("private navigation phrase"));
}

#[test]
fn claude_md_update_is_idempotent_and_atomic() {
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# My Project\n\nsome content\n").unwrap();
    let (changed, content) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(changed);
    let (changed2, _) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(!changed2, "second update must be a no-op");
    assert!(
        !content.contains("<!-- claudecat:auto:begin -->"),
        "v2.1 起地圖不進 CLAUDE.md：auto 區塊不得再出現"
    );
    assert!(content.contains("# My Project"));
    assert!(content.contains("claudecat:guardrails:begin"));
    assert!(content.contains("claudecat:map-pointer:begin"));
    // no temp leftovers
    assert!(fs::read_dir(dir)
        .unwrap()
        .all(|e| e.unwrap().file_name() != ".claudecat.tmp"));
}

#[test]
fn strip_auto_block_removes_legacy_block_whole_lines() {
    let input = "# Rules\n\nbody text\n\n`<!-- claudecat:auto:begin -->\n## Project Map\n- **Root**: `/Users/guofang.mis/a/x`\n<!-- claudecat:auto:end -->\n\n<!-- claudecat:guardrails:begin -->\n<!-- claudecat:guardrails:end -->\n";
    let (out, removed) = claudecat::claude_md::strip_auto_block(input);
    assert!(removed);
    assert!(!out.contains("claudecat:auto"));
    assert!(!out.contains('`'), "行首殘留字元（反引號）應隨整行消失");
    assert!(!out.contains("Root"), "區塊內的機器路徑應整段消失");
    assert!(out.contains("# Rules"));
    assert!(out.contains("body text"));
    assert!(out.contains("claudecat:guardrails:begin"));
    assert!(!out.contains("\n\n\n"), "剝除後不得留下連續空行");
    let (out2, removed2) = claudecat::claude_md::strip_auto_block(&out);
    assert!(!removed2);
    assert_eq!(out, out2, "剝除必須冪等");
}

#[test]
fn strip_auto_block_truncates_when_end_marker_missing() {
    let input = "# Head\n\nbody\n\n<!-- claudecat:auto:begin -->\n## Map\n- torn";
    let (out, removed) = claudecat::claude_md::strip_auto_block(input);
    assert!(removed);
    assert_eq!(out, "# Head\n\nbody\n");
    assert!(!out.contains("torn"));
}

#[test]
fn strip_auto_block_noop_without_block() {
    let input = "# Rules\n\n- only rules\n";
    let (out, removed) = claudecat::claude_md::strip_auto_block(input);
    assert!(!removed);
    assert_eq!(out, input);
    let (out2, removed2) = claudecat::claude_md::strip_auto_block("");
    assert!(!removed2);
    assert_eq!(out2, "");
}

/// 暫時覆寫環境變數，drop 時還原（避免污染其他並行測試）
/// 序列化所有會改 process-global env（CORT_CACHE_DIR）的測試，
/// 避免 cargo 平行測試互相踩環境變數。
fn cort_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct EnvVarGuard(String, Option<String>);
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(v) => std::env::set_var(&self.0, v),
            None => std::env::remove_var(&self.0),
        }
    }
}

fn temp_project() -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("claudecat-test-{}", std::process::id()));
    let dir = base.join(format!("{}", rand_suffix()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // 測試平行起跑時 nanos 會撞位（macOS 時鐘粒度 > 1ns），兩個 fixture 就會共用
    // 同一個目錄、互相覆寫對方的 main.rs（症狀：auto_profile 兩個測試隨機互換著掛）。
    // 時鐘再疊一個進程內單調序號，迴避次序高低位元，保證進程內唯一。
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    nanos ^ seq.rotate_left(20)
}

#[test]
fn guardrails_preserved_and_seeded() {
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# P\n").unwrap();
    let (_, content) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(content.contains("claudecat:guardrails:begin"));
    // user adds a decision inside the guardrail block
    let edited = content.replace(
        "<!-- 技術決策 / Guardrails：每行一條",
        "<!-- 技術決策 / Guardrails：每行一條\n- 2D tilemap + Macroquad（禁 Python/3D）",
    );
    fs::write(&path, &edited).unwrap();
    // second update: marker already exists -> must NOT be re-seeded/overwritten
    let (_, content2) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(content2.contains("2D tilemap + Macroquad"));
    assert_eq!(content2.matches("claudecat:guardrails:begin").count(), 1);
}

#[test]
fn map_pointer_seeded_once_and_never_rewritten() {
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# P\n").unwrap();
    let (_, content) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(content.contains("claudecat:map-pointer:begin"));
    // user hand-edits inside the pointer block
    let edited = content.replace(
        "播種一次，之後永不改寫",
        "播種一次，之後永不改寫（手改註記）",
    );
    fs::write(&path, &edited).unwrap();
    // second update: pointer already exists -> must NOT be re-seeded/overwritten
    let (_, content2) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(
        content2.contains("（手改註記）"),
        "指標區內的使用者文字不得被覆寫"
    );
    assert_eq!(content2.matches("claudecat:map-pointer:begin").count(), 1);
}

#[test]
fn map_pointer_removed_by_user_is_not_reseeded() {
    use claudecat::claude_md::{PTR_BEGIN, PTR_END};
    use claudecat::guardrails::GR_BEGIN;
    let dir = temp_project();
    let path = dir.join("CLAUDE.md");
    fs::write(&path, "# P\n").unwrap();
    let (_, content) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(content.contains(GR_BEGIN));
    assert!(content.contains(PTR_BEGIN));
    // user deletes the whole map-pointer block (begin line through end line), keeps guardrails
    let begin = content.find(PTR_BEGIN).unwrap();
    let line_start = content[..begin].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = content.find(PTR_END).unwrap() + PTR_END.len();
    let line_end = content[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(content.len());
    let removed = format!("{}{}", &content[..line_start], &content[line_end..]);
    assert!(!removed.contains(PTR_BEGIN) && !removed.contains(PTR_END));
    fs::write(&path, &removed).unwrap();
    // second update: guardrails present + pointer absent -> user removed it, leave it absent
    let (changed, content2) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(!changed, "使用者刪掉的指標不得被補種");
    assert!(!content2.contains(PTR_BEGIN), "指標不得再出現");
    assert_eq!(content2.matches(GR_BEGIN).count(), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), removed);
}

#[test]
fn data_dir_honors_env_override_and_xdg_default() {
    let _lock = cort_env_lock();
    let home = temp_project();
    let xdg = home.join("xdg-data");
    let over = home.join("override");
    let _g1 = EnvVarGuard(
        "CLAUDECAT_DATA_DIR".to_string(),
        std::env::var("CLAUDECAT_DATA_DIR").ok(),
    );
    let _g2 = EnvVarGuard(
        "XDG_DATA_HOME".to_string(),
        std::env::var("XDG_DATA_HOME").ok(),
    );
    let _g3 = EnvVarGuard("HOME".to_string(), std::env::var("HOME").ok());
    std::env::set_var("CLAUDECAT_DATA_DIR", &over);
    assert_eq!(claudecat::data_dir::data_dir(), over, "覆寫優先");
    std::env::remove_var("CLAUDECAT_DATA_DIR");
    std::env::set_var("XDG_DATA_HOME", &xdg);
    assert_eq!(
        claudecat::data_dir::data_dir(),
        xdg.join("claudecat"),
        "XDG_DATA_HOME 次之"
    );
    std::env::remove_var("XDG_DATA_HOME");
    std::env::set_var("HOME", &home);
    assert_eq!(
        claudecat::data_dir::data_dir(),
        home.join(".local/share/claudecat"),
        "預設落在 HOME 下"
    );
}

#[test]
fn map_path_uses_cort_project_id() {
    let p = "/some/real/path";
    assert_eq!(
        claudecat::data_dir::map_path_for(p),
        claudecat::data_dir::data_dir()
            .join("projects")
            .join(claudecat::cort::project_id(p))
            .join("map.md")
    );
}

#[test]
fn map_file_body_wraps_section() {
    let section = "## Project Map (auto-maintained by claudecat)\n- **Root**: `/x`\n";
    let body = claudecat::data_dir::map_file_body(section);
    assert!(body.starts_with("# claudecat Project Map\n"));
    assert!(body.contains(section), "section 應原樣置入");
    assert!(!body.contains("claudecat:auto"));
    assert!(body.ends_with('\n'));
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
    assert!(
        profile.is_mini(),
        "small project should resolve to Mini, got {:?}",
        profile
    );
    let md = claudecat::outline::render_with_profile(&map, profile);
    assert!(md.contains("Mini"));
    assert!(
        !md.contains("Key files & symbols"),
        "mini must skip symbols"
    );
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
    assert_eq!(
        content.matches("| 日期").count(),
        1,
        "single header expected"
    );
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
    assert!(
        r.symbols.iter().any(|h| h.name == "authenticate"),
        "should hit authenticate"
    );
    // mod auth 精確命中優先；路線應含 cort context <命中符號>
    assert!(
        r.route.iter().any(|s| s.contains("cort context")),
        "route should suggest cort context"
    );
    let report_auth = claudecat::navigate::render(&r);
    assert!(report_auth.contains("auth") || report_auth.contains("authenticate"));
    let report = claudecat::navigate::render(&r);
    assert!(report.contains("路線"));
}

#[test]
fn cort_project_id_matches_sha256() {
    let id = claudecat::cort::project_id("/home/yanggf/a/claudecat");
    // 以 python hashlib 驗證過：77bf9a9b6e40...
    assert_eq!(&id[..12], "77bf9a9b6e40");
    // 確定性
    assert_eq!(id, claudecat::cort::project_id("/home/yanggf/a/claudecat"));
}

#[test]
fn cort_index_info_none_when_db_missing() {
    let dir = temp_project(); // 隨機目錄 → 無 cort DB
    let info = claudecat::cort::index_info(&dir);
    assert!(info.is_none(), "db 不存在應回 None，而非錯誤");
}

#[test]
fn cort_search_symbols_none_when_db_missing() {
    let dir = temp_project();
    let hits = claudecat::cort::search_symbols(&dir, "auth");
    assert!(hits.is_none());
}

/// cort 索引被寫入端持 EXCLUSIVE lock（模擬 cort hook 正在索引/寫入）時，
/// 一般唯讀 query 會 BUSY；claudecat 必須自動退回 immutable=1 仍能唯讀讀到索引。
#[cfg(unix)]
#[test]
fn cort_readonly_fallback_when_writer_holds_exclusive_lock() {
    use std::os::unix::fs::PermissionsExt;

    // 1) project + cache dir（cache 內放 cort 風格的 WAL DB）
    let proj = temp_project();
    let cache = std::env::temp_dir().join(format!(
        "claudecat-cort-cache-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    fs::create_dir_all(&cache).unwrap();
    let real_str = fs::canonicalize(&proj)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let pid = claudecat::cort::project_id(&real_str);
    let db_path = cache.join(format!("{pid}.db"));

    // 2) WAL mode + schema v4 相容表 + 資料（checkpoint 後資料在主檔，唯讀可直接讀）
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE projects (
               project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
               git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
             );
             CREATE TABLE chunks (
               chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL,
               symbol_name TEXT, chunk_type TEXT, start_line INTEGER NOT NULL,
               end_line INTEGER NOT NULL, content TEXT NOT NULL, language TEXT
             );
             CREATE TABLE relationships (
               source_chunk_id TEXT NOT NULL, target_chunk_id TEXT NOT NULL,
               rel_type TEXT NOT NULL, call_site_line INTEGER, confidence_score REAL NOT NULL
             );
             INSERT INTO projects VALUES ('{pid}', 'demo', '{real_str}', NULL, {now_ms}, 'test-extractor');
             INSERT INTO chunks VALUES ('c1', '{pid}', 'src/lib.rs', 'alpha', 'function', 1, 3, 'pub fn alpha() {{}}', 'Rust');
             INSERT INTO chunks VALUES ('c2', '{pid}', 'src/lib.rs', 'beta',  'function', 5, 9, 'pub fn beta() {{}}',  'Rust');
             INSERT INTO relationships VALUES ('c2', 'c1', 'calls', 6, 1.0);"
        ))
        .unwrap();
        // journal_mode 會回傳 row，須用 query_row 而非 execute_batch
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let _: (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
    }
    let _ = fs::remove_file(format!("{}{}", db_path.display(), ".db-shm"));
    let _ = fs::remove_file(format!("{}{}", db_path.display(), ".db-wal"));

    // 3) 寫入端持 EXCLUSIVE lock，直到測試結束才釋放
    let holder = rusqlite::Connection::open(&db_path).unwrap();
    holder
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE;")
        .unwrap();
    holder
        .execute_batch(
            "BEGIN; INSERT INTO chunks VALUES ('c3', 'x', 'x', 'x', 'x', 1, 1, 'x', 'x'); COMMIT;",
        )
        .unwrap();

    // 4) 情境成立：一般唯讀可以開，但第一次 query 就 BUSY
    let normal =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open 本身應成功");
    let err = normal.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0));
    assert!(
        err.is_err(),
        "EXCLUSIVE lock 下一般唯讀 query 必須 BUSY，才能證明 fallback 有必要"
    );

    // 5) claudecat 的 fallback 仍可唯讀讀到索引（不寫入）
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let info = claudecat::cort::index_info(&proj).expect("immutable fallback 應能開啟");
    assert_eq!(info.chunk_count, 2);
    assert_eq!(info.relationships_count, 1);
    let hits = claudecat::cort::search_symbols(&proj, "alpha").expect("fallback 應能搜尋");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].symbol.as_deref(), Some("alpha"));

    // 6) 清理：釋放 lock、還原 env
    drop(holder);
    drop(guard);
    let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755));
}

/// FTS 全文 fallback：symbol_name 未命中但 content 命中（`cort recall` 對應）。
/// 用 external-content FTS5 建合成 DB 實測唯讀查詢。
#[test]
fn cort_fts_finds_content_only_match() {
    let proj = temp_project();
    let cache = std::env::temp_dir().join(format!(
        "claudecat-cort-cache-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    fs::create_dir_all(&cache).unwrap();
    let real_str = fs::canonicalize(&proj)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let pid = claudecat::cort::project_id(&real_str);
    let db_path = cache.join(format!("{pid}.db"));

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE projects (
           project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
           git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
         );
         CREATE TABLE chunks (
           chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL,
           symbol_name TEXT, chunk_type TEXT, start_line INTEGER NOT NULL,
           end_line INTEGER NOT NULL, content TEXT NOT NULL, language TEXT
         );
         CREATE VIRTUAL TABLE chunks_fts USING fts5(
           content, symbol_name, file_path,
           content=chunks, content_rowid=rowid, tokenize='unicode61'
         );
         INSERT INTO projects VALUES ('{pid}', 'demo', '{real_str}', NULL, 0, 'test');
         INSERT INTO chunks VALUES ('c1', '{pid}', 'src/cort.rs', 'with_readonly', 'function', 71, 90, 'fn with_readonly uses the immutable fallback to open the db', 'Rust');
         INSERT INTO chunks VALUES ('c2', '{pid}', 'src/main.rs', 'parse_cli', 'function', 10, 30, 'fn parse_cli parses the argv arguments', 'Rust');
         INSERT INTO chunks_fts(rowid, content, symbol_name, file_path)
           SELECT rowid, content, symbol_name, file_path FROM chunks;"
    ))
    .unwrap();
    drop(conn);

    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    // symbol_name LIKE 找不到（immutable 只在 content）→ FTS 全文找得到
    assert!(
        claudecat::cort::search_symbols(&proj, "immutable").is_none(),
        "symbol LIKE 不應命中 content"
    );
    let hits = claudecat::cort::search_fts(&proj, "immutable").expect("FTS 應命中 content");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].symbol.as_deref(), Some("with_readonly"));
    assert!(hits[0]
        .content
        .as_deref()
        .unwrap_or("")
        .contains("immutable"));

    // 多詞 AND（`"immutable" AND "fallback"`）
    let two = claudecat::cort::search_fts(&proj, "immutable fallback").expect("AND 應命中");
    assert_eq!(two.len(), 1);
    // 無關詞 → None（誠實回退）
    assert!(claudecat::cort::search_fts(&proj, "zzz_nothing").is_none());

    drop(guard);
}

/// navigate_with_cort：content 摘要進路線（省一次 read）+ FTS 來源標示
#[test]
fn navigate_with_cort_includes_content_summary() {
    use claudecat::model::{FileInfo, ProjectMap, Symbol};

    let map = ProjectMap {
        root: "/tmp/claudecat-nav-test".to_string(),
        key_files: vec![FileInfo {
            path: "src/cort.rs".to_string(),
            language: Some("rust".to_string()),
            loc: 100,
            symbols: vec![Symbol {
                kind: "fn".to_string(),
                name: "with_readonly".to_string(),
                line: 71,
            }],
        }],
        ..Default::default()
    };
    let hit = claudecat::cort::CortHit {
        symbol: Some("with_readonly".to_string()),
        chunk_type: "function".to_string(),
        file: "src/cort.rs".to_string(),
        start_line: 71,
        end_line: 90,
        language: Some("Rust".to_string()),
        content: Some(
            "fn with_readonly uses the immutable fallback to open the database file read-only"
                .to_string(),
        ),
    };

    // FTS 來源：標示 FTS 全文命中 + content 摘要 + 仍給 cort context 深挖路徑
    let r = claudecat::navigate::navigate_with_cort(&map, "immutable", vec![hit.clone()], true);
    assert!(
        r.route.iter().any(|s| s.contains("FTS 全文命中")),
        "FTS 來源應標示，實際：{:?}",
        r.route
    );
    assert!(
        r.route
            .iter()
            .any(|s| s.contains("內文摘要") && s.contains("immutable fallback")),
        "路線應含 content 摘要"
    );
    assert!(
        r.route
            .iter()
            .any(|s| s.contains("cort context with_readonly")),
        "仍應含 cort context 深挖路徑"
    );

    // 一般命中（symbol LIKE）：標示全量索引命中、摘要同樣帶上
    let r2 = claudecat::navigate::navigate_with_cort(&map, "with_readonly", vec![hit], false);
    assert!(r2.route.iter().any(|s| s.contains("全量索引命中")));
    assert!(r2.route.iter().any(|s| s.contains("內文摘要")));
}

/// P1 回歸：last_indexed_at 是「毫秒」（與 cort 寫入一致）——40 天前的索引必須 STALE。
#[test]
fn cort_freshness_ms_stale_after_7_days() {
    let proj = temp_project();
    let cache = std::env::temp_dir().join(format!(
        "claudecat-cort-cache-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    fs::create_dir_all(&cache).unwrap();
    let real_str = fs::canonicalize(&proj)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let pid = claudecat::cort::project_id(&real_str);
    let db_path = cache.join(format!("{pid}.db"));
    let old_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 40 * 24 * 3600 * 1000;
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE projects (
               project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
               git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
             );
             CREATE TABLE chunks (
               chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL
             );
             CREATE TABLE relationships (
               source_chunk_id TEXT NOT NULL, target_chunk_id TEXT NOT NULL,
               rel_type TEXT NOT NULL, call_site_line INTEGER, confidence_score REAL NOT NULL
             );
             INSERT INTO projects VALUES ('{pid}', 'old', '{real_str}', NULL, {old_ms}, 'test');"
        ))
        .unwrap();
    }
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let info = claudecat::cort::index_info(&proj).expect("index_info 應有結果");
    assert!(!info.fresh, "40 天前的索引（毫秒），cort-status 應報 STALE");
    drop(guard);
}

/// P1 回歸：同一檔案裡長期指標表之後還有其他 section（使用者筆記）時，
/// 更新表格不得刪除它、也不得把它的資料列吞進表格。
#[test]
fn track_table_preserves_sibling_sections_in_one_file() {
    let dir = temp_project();
    fs::write(dir.join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    let m = claudecat::explore::compute(&claudecat_lib_scan(&dir));
    let f = dir.join("SESSION-EVIDENCE.md");
    // 1) 先建立長期指標表
    claudecat::explore::track_update(&f, &[&m]).unwrap();
    // 2) 之後接一段使用者筆記（含自己的表格）
    let mut content = fs::read_to_string(&f).unwrap();
    content.push_str("\n## 我的筆記\n\n| 日期 | 專案 |\n|---|---|\n| 2026-09-01 | `/old` |\n");
    fs::write(&f, &content).unwrap();

    // 3) 同日再更新（表格本身等價重建；重點在下方內容斷言）
    let (changed, _) = claudecat::explore::track_update(&f, &[&m]).unwrap();
    assert!(
        changed,
        "重寫會壓掉表格與下一個 section 間的空行 → 內容有變"
    );
    let after = fs::read_to_string(&f).unwrap();
    assert!(after.contains("## 我的筆記"), "筆記 section 標題不應被刪除");
    assert!(
        after.contains("| 2026-09-01 | `/old` |"),
        "筆記的資料列不應被吞進表格或刪除"
    );
    assert_eq!(
        after.matches(claudecat::explore::TRACK_SECTION).count(),
        1,
        "長期指標 section 標題應恰有一個"
    );
    assert_eq!(
        after.matches(&format!("`{}`", dir.display())).count(),
        1,
        "資料列應恰有一列（同日更新不重複）"
    );
}

/// P1 回歸：CLAUDE.md 是 symlink 時（cortexyoung 慣例：CLAUDE.md -> AGENTS.md，
/// 讓 Claude/Codex 兩個 harness 永不漂移），update 必須寫進 symlink 目標、
/// 不得把 symlink 取代成普通檔。v2.1 起「寫進目標」的內容是剝除舊版 auto 區塊
/// ＋播種——帶 legacy 區塊的本體經一次 update 應變成 rules-only。
#[test]
fn claude_md_update_writes_through_symlink() {
    let dir = temp_project();
    let legacy = "# AGENTS\n\nbody\n\n`<!-- claudecat:auto:begin -->\n## Project Map (auto-maintained by claudecat)\n- **Root**: `/home/yanggf/a/claudecat`\n<!-- claudecat:auto:end -->\n";
    fs::write(dir.join("AGENTS.md"), legacy).unwrap();
    std::os::unix::fs::symlink("AGENTS.md", dir.join("CLAUDE.md")).unwrap();

    let path = dir.join("CLAUDE.md");
    let (changed, _) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(changed);

    let meta = path.symlink_metadata().unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "symlink 不得被原子寫入取代成普通檔"
    );
    let agents = fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(agents.contains("# AGENTS"), "原內容保留");
    assert!(
        !agents.contains("claudecat:auto"),
        "舊版 auto 區塊應經 symlink 從本體剝除"
    );
    assert!(!agents.contains("/home/yanggf"), "機器路徑隨區塊消失");
    assert!(
        agents.contains("claudecat:map-pointer:begin"),
        "指標種子應播下"
    );

    let (changed2, _) = claudecat::claude_md::update_section(&path, false).unwrap();
    assert!(!changed2, "同內容重跑應 no-op");
}

/// P3 回歸：immutable URI 的路徑必須 percent-encode `%` `?` `#` 空白，
/// 否則 URI 語法會把路徑截斷在 query/fragment 邊界。
#[test]
fn cort_sqlite_uri_escapes_special_chars() {
    let p = std::path::Path::new("/home/u#1/my cache/db?x.db");
    let uri = claudecat::cort::sqlite_uri(p);
    assert_eq!(
        uri, "file:/home/u%231/my%20cache/db%3Fx.db?immutable=1",
        "URI 邊界字元必須編碼"
    );
    assert!(!uri.contains(' '));
}

/// host 探測在這台機器上要拿得到名字——macOS 沒有 /etc/hostname，
/// 靠 `hostname` 指令退路；拿不到就會被記成 unknown。
#[test]
fn host_resolves_on_this_machine() {
    let host = claudecat::host::resolve_host()
        .expect("host probe returned None: neither /etc/hostname nor `hostname` answered");
    assert!(!host.is_empty());
    assert!(!host.contains('\n'), "trim 過的名字不該帶換行");
}

/// `_cortex_meta` 三態 fixture：建一個「時戳全新、HEAD 不追究」的 cort 風格 DB，
/// `meta` 決定 `_cortex_meta` 的內容：None＝整張表不存在（舊版 cort DB）。
fn cort_meta_fixture(meta: Option<&[(&str, &str)]>) -> (std::path::PathBuf, std::path::PathBuf) {
    let proj = temp_project();
    let cache = std::env::temp_dir().join(format!(
        "claudecat-cort-meta-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    fs::create_dir_all(&cache).unwrap();
    let real_str = fs::canonicalize(&proj)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let pid = claudecat::cort::project_id(&real_str);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let conn = rusqlite::Connection::open(cache.join(format!("{pid}.db"))).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE projects (
           project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
           git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
         );
         CREATE TABLE chunks (
           chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL,
           symbol_name TEXT, chunk_type TEXT, start_line INTEGER NOT NULL,
           end_line INTEGER NOT NULL, content TEXT NOT NULL, language TEXT,
           chunk_source TEXT DEFAULT 'ast'
         );
         CREATE TABLE file_state (
           project_id TEXT NOT NULL, file_path TEXT NOT NULL, file_content_hash TEXT NOT NULL
         );
         CREATE TABLE relationships (
           source_chunk_id TEXT NOT NULL, target_chunk_id TEXT NOT NULL,
           rel_type TEXT NOT NULL, call_site_line INTEGER, confidence_score REAL NOT NULL
         );
         INSERT INTO projects VALUES ('{pid}', 'demo', '{real_str}', NULL, {now_ms}, 'test');
         INSERT INTO chunks VALUES ('c1', '{pid}', 'src/lib.rs', 'alpha', 'function', 1, 3, 'pub fn alpha()', 'Rust', 'ast');
         INSERT INTO file_state VALUES ('{pid}', 'src/lib.rs', 'h1');"
    ))
    .unwrap();
    if let Some(rows) = meta {
        conn.execute_batch(
            "CREATE TABLE _cortex_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        for (k, v) in rows {
            conn.execute("INSERT INTO _cortex_meta VALUES (?1, ?2)", [k, v])
                .unwrap();
        }
    }
    drop(conn);
    (proj, cache)
}

/// P1 回歸：cort 的 schema 遷移會設 `graph_pending=1` 而**不動** `last_indexed_at`/`git_head`
/// （cort `db.rs:322` vs `incremental.rs` 的同一 transaction 清 0），
/// 所以「時戳很新但圖是舊的」是真實狀態——claudecat 不得報 fresh。
#[test]
fn cort_graph_pending_is_not_fresh() {
    let (proj, cache) = cort_meta_fixture(Some(&[("SCHEMA_VERSION", "5"), ("graph_pending", "1")]));
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let info = claudecat::cort::index_info(&proj).expect("index_info 應有結果");
    assert_eq!(info.graph_pending, Some(true));
    assert_eq!(info.schema_version.as_deref(), Some("5"));
    assert!(!info.fresh, "圖已知落後時不得報 fresh（時戳再新也一樣）");
    drop(guard);
}

/// `graph_pending=0`（每次增量在同一 transaction 清 0）→ 行為與過去完全一致。
#[test]
fn cort_graph_pending_zero_keeps_fresh() {
    let (proj, cache) = cort_meta_fixture(Some(&[("SCHEMA_VERSION", "5"), ("graph_pending", "0")]));
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let info = claudecat::cort::index_info(&proj).expect("index_info 應有結果");
    assert_eq!(info.graph_pending, Some(false));
    assert!(info.fresh, "圖已重建 + 時戳新 → 仍是 fresh");
    drop(guard);
}

/// 舊版 cort DB 沒有 `_cortex_meta`：讀不到 ≠ 壞掉，也 ≠ 健康。
/// 兩欄 None、`fresh` 仍只由 HEAD+age 決定（絕不因為讀不到就打成 STALE——
/// 那正是 cort 自己修掉的「沒讀到卻報 drifted」）。
#[test]
fn cort_missing_meta_table_is_unknown_not_unhealthy() {
    let (proj, cache) = cort_meta_fixture(None);
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let info = claudecat::cort::index_info(&proj).expect("index_info 應有結果");
    assert_eq!(
        info.graph_pending, None,
        "讀不到就是無法判讀，不得 Some(false)"
    );
    assert_eq!(info.schema_version, None);
    assert!(info.fresh, "無法判讀不翻布林：HEAD+age 說 fresh 就是 fresh");
    drop(guard);
}

/// cort 端已有 Java 圖，claudecat 端沒有 grammar：地圖不得讓「空符號」看起來像「沒結構」。
/// 純 Rust 專案不得出現這句（不製造噪音）。
#[test]
fn outline_says_which_key_files_have_no_local_ast() {
    let dir = temp_project();
    fs::create_dir_all(dir.join("src")).unwrap();
    let mut java = String::from("package demo;\n\npublic class OrderService {\n");
    for i in 0..60 {
        java.push_str(&format!("  void run{i}() {{ }}\n"));
    }
    java.push_str("}\n");
    fs::write(dir.join("src/OrderService.java"), &java).unwrap();
    let map = claudecat_lib_scan(&dir);
    let md = claudecat::outline::render_markdown(&map);
    assert!(
        md.contains("`src/OrderService.java`"),
        "java 檔仍要進 key files（改成非 code 會讓 Java 專案整個消失）"
    );
    assert!(
        md.contains("java 檔無本地 AST") && md.contains("navigate --cort"),
        "空符號要明說原因與替代路徑，實際輸出：\n{md}"
    );

    let rust_only = temp_project();
    fs::create_dir_all(rust_only.join("src")).unwrap();
    fs::write(
        rust_only.join("src/main.rs"),
        "fn main() {}\npub fn alpha() {}\n",
    )
    .unwrap();
    let md2 = claudecat::outline::render_markdown(&claudecat_lib_scan(&rust_only));
    assert!(!md2.contains("無本地 AST"), "有 grammar 的專案不得出現這句");
}
