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
    assert!(fs::read_dir(dir)
        .unwrap()
        .all(|e| e.unwrap().file_name() != ".claudecat.tmp"));
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
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
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

/// cort-audit：索引健康 + 覆蓋缺口（file_state 有、chunks 無）+ FTS 同步
#[test]
fn cort_audit_reports_health_and_coverage() {
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
             INSERT INTO chunks VALUES ('c2', '{pid}', 'src/lib.rs', 'beta',  'function', 5, 9, 'pub fn beta()',  'Rust', 'unparsed');
             INSERT INTO file_state VALUES ('{pid}', 'src/lib.rs', 'h1');
             INSERT INTO file_state VALUES ('{pid}', 'legacy/test-x.js', 'h2');
             INSERT INTO relationships VALUES ('c1', 'c2', 'calls', 2, 1.0);"
        ))
        .unwrap();
    }

    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let a = claudecat::cort::audit_index(&proj).expect("audit_index 應有結果");
    assert!(a.fresh, "索引 0 天前應 fresh");
    assert_eq!(a.chunk_count, 2);
    assert_eq!(a.relationships_count, 1);
    assert_eq!(a.file_state_files, Some(2));
    assert_eq!(a.chunked_files, Some(1));
    assert_eq!(a.not_chunked_total, Some(1));
    assert_eq!(a.not_chunked_files, vec!["legacy/test-x.js".to_string()]);
    assert_eq!(a.files_with_unparsed_chunks, 1);
    // 無 chunks_fts 表 → docs/drift 皆「無法判讀」（None），不得偽稱 synced 或 0
    assert_eq!(a.fts_docs, None);
    assert_eq!(a.fts_drift, None);

    drop(guard);
}

/// cort-audit：用量窗口（usage.db command_log；window 外不計 + hook JSON 解析）
#[test]
fn cort_audit_usage_counts_window() {
    let cache = std::env::temp_dir().join(format!(
        "claudecat-cort-cache-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    fs::create_dir_all(&cache).unwrap();
    let db_path = cache.join("usage.db");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE command_log (
               id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, project_id TEXT,
               command TEXT NOT NULL, args_summary TEXT NOT NULL,
               status TEXT NOT NULL, error_code TEXT,
               read_source TEXT, requested_content_mode TEXT, effective_content_mode TEXT,
               receipt_hit INTEGER, index_stale INTEGER,
               bytes_out INTEGER NOT NULL, saved_bytes INTEGER NOT NULL
             );
             CREATE TABLE _usage_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO command_log (ts, command, args_summary, status, index_stale, bytes_out, saved_bytes) \
             VALUES (?1, 'impact', '{}', 'ok', 0, 100, 90)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO command_log (ts, command, args_summary, status, index_stale, bytes_out, saved_bytes) \
             VALUES (?1, 'hook-suggest', '{\"hook\":\"hit\"}', 'ok', 0, 0, 0)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO command_log (ts, command, args_summary, status, index_stale, bytes_out, saved_bytes) \
             VALUES (?1, 'impact', '{}', 'error', 1, 100, 10)",
            [&now],
        )
        .unwrap();
        // 窗口外（40 天前）不計
        conn.execute(
            "INSERT INTO command_log (ts, command, args_summary, status, index_stale, bytes_out, saved_bytes) \
             VALUES (?1, 'context', '{}', 'ok', 0, 100, 0)",
            [&(now - 40 * 24 * 3600 * 1000)],
        )
        .unwrap();
    }
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let u = claudecat::cort::audit_usage(30).expect("usage 應有結果");
    assert_eq!(u.window_days, 30);
    assert_eq!(u.total_commands, 3);
    assert_eq!(u.by_command.get("impact"), Some(&2));
    assert_eq!(u.by_command.get("context"), None);
    assert_eq!(u.suggest_outcomes.get("hit"), Some(&1));
    assert_eq!(u.errors, 1);
    assert_eq!(u.index_stale_queries, 1);
    assert_eq!(u.saved_bytes, 100);

    drop(guard);
}

/// cort-audit：--track 寫進長期指標表（同日重複不新增列）
#[test]
fn cort_audit_track_updates_table() {
    let a = claudecat::cort::CortAudit {
        root: "/tmp/fake-root".to_string(),
        window_days: 7,
        index: None,
        db_exists: false,
        usage: None,
        usage_7d: None,
    };
    let dir = temp_project();
    let f = dir.join("EVIDENCE.md");
    let (changed1, _) = claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    assert!(changed1);
    let content1 = fs::read_to_string(&f).unwrap();
    assert!(content1.contains("## 長期指標 (claudecat cort-audit)"));
    assert!(content1.contains("| 日期 | 專案 | fresh |"));
    assert!(content1.contains("core/7d"), "追蹤列應含 core 動詞欄");
    assert!(content1.contains("deep/7d"), "追蹤列應含 deep 動詞欄");
    assert!(content1.contains("命令數/7d"), "追蹤列應含 7 天早期訊號欄");
    assert!(content1.contains("`/tmp/fake-root`"));

    let (changed2, _) = claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    assert!(!changed2, "同日同 root 重複 track 應是 no-op");
    let content2 = fs::read_to_string(&f).unwrap();
    assert_eq!(content1, content2);
}

/// P1 回歸：last_indexed_at 是「毫秒」（與 cort 寫入一致）——40 天前的索引必須 STALE，
/// 且 index_info（cort-status）與 audit_index（cort-audit）兩套判定口徑一致。
#[test]
fn cort_freshness_ms_stale_after_7_days_and_consistent() {
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
    let audit = claudecat::cort::audit_index(&proj).expect("audit_index 應有結果");
    assert!(!audit.fresh, "audit 的 fresh 判定口徑應與 index_info 一致");
    drop(guard);
}

/// P1 回歸：--track 表格之後的內容（使用者筆記）必須原樣保留，
/// 不得被「section 掃到 EOF」的重寫刪掉。
#[test]
fn cort_audit_track_preserves_content_after_section() {
    let a = claudecat::cort::CortAudit {
        root: "/tmp/fake-root".to_string(),
        window_days: 7,
        index: None,
        db_exists: false,
        usage: None,
        usage_7d: None,
    };
    let dir = temp_project();
    let f = dir.join("EVIDENCE.md");
    let (c1, _) = claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    assert!(c1);
    // 使用者緊接在表格後加自己的筆記（無空行，讓同日重跑可精確 no-op）
    let mut content = fs::read_to_string(&f).unwrap();
    content.push_str("## 我的筆記\n\n重要結論 KEEPME-123\n");
    fs::write(&f, &content).unwrap();

    let (changed, _) = claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    assert!(!changed, "同日同 root、內容不變 → 應是 no-op");
    let after = fs::read_to_string(&f).unwrap();
    assert!(after.contains("KEEPME-123"), "表格後的筆記不應被刪除");
    assert!(after.contains("## 我的筆記"), "筆記 section 標題應保留");
}

/// P1 回歸：同一檔案裡 audit section 之後還有 explore section 時，
/// 更新 audit 不得刪除 explore section、也不得把它的資料列吞進 audit 表。
#[test]
fn track_table_preserves_sibling_sections_in_one_file() {
    let a = claudecat::cort::CortAudit {
        root: "/tmp/fake-root".to_string(),
        window_days: 7,
        index: None,
        db_exists: false,
        usage: None,
        usage_7d: None,
    };
    let dir = temp_project();
    let f = dir.join("SESSION-EVIDENCE.md");
    // 1) 先建立 audit 表格
    claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    // 2) 之後接一段 explore section（兩種指標共用同一份證據檔）
    let mut content = fs::read_to_string(&f).unwrap();
    content.push_str(
        "\n## 長期指標 (claudecat explore)\n\n| 日期 | 專案 |\n|---|---|\n| 2026-09-01 | `/old` |\n",
    );
    fs::write(&f, &content).unwrap();

    // 3) 同日再更新 audit（audit 表格本身等價重建；重點在下方內容斷言）
    let (changed, _) = claudecat::cort_audit::track_update(&f, &[&a]).unwrap();
    assert!(
        changed,
        "重寫會壓掉表格與下一個 section 間的空行 → 內容有變"
    );
    let after = fs::read_to_string(&f).unwrap();
    assert!(
        after.contains("## 長期指標 (claudecat explore)"),
        "explore section 標題不應被刪除"
    );
    assert!(
        after.contains("| 2026-09-01 | `/old` |"),
        "explore 的資料列不應被吞進 audit 表或刪除"
    );
    assert_eq!(
        after.matches(claudecat::cort_audit::TRACK_SECTION).count(),
        1,
        "audit section 標題應恰有一個"
    );
    assert!(
        after.matches("`/tmp/fake-root`").count() == 1,
        "audit 資料列應恰有一列（同日更新不重複）"
    );
}

/// P1 回歸：CLAUDE.md 是 symlink 時（cortexyoung 慣例：CLAUDE.md -> AGENTS.md，
/// 讓 Claude/Codex 兩個 harness 永不漂移），update 必須寫進 symlink 目標、
/// 不得把 symlink 取代成普通檔。
#[test]
fn claude_md_update_writes_through_symlink() {
    let dir = temp_project();
    fs::write(dir.join("AGENTS.md"), "# AGENTS\n\nbody\n").unwrap();
    std::os::unix::fs::symlink("AGENTS.md", dir.join("CLAUDE.md")).unwrap();

    let section = "## Map\n- x\n".to_string();
    let path = dir.join("CLAUDE.md");
    let (changed, _) = claudecat::claude_md::update_section(&path, &section, false).unwrap();
    assert!(changed);

    let meta = path.symlink_metadata().unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "symlink 不得被原子寫入取代成普通檔"
    );
    let agents = fs::read_to_string(dir.join("AGENTS.md")).unwrap();
    assert!(agents.contains("# AGENTS"), "原內容保留");
    assert!(agents.contains("## Map"), "section 應寫進 symlink 目標");

    let (changed2, _) = claudecat::claude_md::update_section(&path, &section, false).unwrap();
    assert!(!changed2, "同內容重跑應 no-op");
}

/// P2 回歸：file_state 表不存在（cort schema 版本差異）時，覆蓋必須是「無法判讀」，
/// 不得被 unwrap_or(0) 吞成「無缺口」——與當初 ?1 bug 同類的靜默零值。
#[test]
fn cort_audit_missing_file_state_table_is_not_silent_zero() {
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        // 故意不建 file_state / chunks_fts 表
        conn.execute_batch(&format!(
            "CREATE TABLE projects (
               project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
               git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
             );
             CREATE TABLE chunks (
               chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL
             );
             INSERT INTO projects VALUES ('{pid}', 'demo', '{real_str}', NULL, {now_ms}, 'test');"
        ))
        .unwrap();
    }
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let a = claudecat::cort::audit_index(&proj).expect("audit_index 應有結果");
    assert_eq!(
        a.not_chunked_total, None,
        "查詢失敗必須是「無法判讀」，不是 0"
    );
    assert_eq!(a.file_state_files, None);

    let audit = claudecat::cort::CortAudit {
        root: real_str,
        window_days: 30,
        index: Some(a),
        db_exists: true,
        usage: None,
        usage_7d: None,
    };
    let report = claudecat::cort_audit::render(&audit);
    assert!(
        report.contains("無法判讀"),
        "報告應明說無法判讀，不假裝無缺口"
    );
    drop(guard);
}

/// P2 回歸：FTS 同步判定用 rowid 雙向差異（drift），不用數量相等——
/// 一多一少時「數量相等」會偽稱 synced。
#[test]
fn cort_audit_fts_drift_detects_missing_and_extra_fts_rows() {
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        // drift 查詢只用 rowid join——測試用普通表即可，不需 fts5 模組
        conn.execute_batch(&format!(
            "CREATE TABLE projects (
               project_id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL,
               git_head TEXT, last_indexed_at INTEGER, extractor_version TEXT NOT NULL
             );
             CREATE TABLE chunks (
               chunk_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, file_path TEXT NOT NULL
             );
             CREATE TABLE chunks_fts (content TEXT);
             INSERT INTO projects VALUES ('{pid}', 'demo', '{real_str}', NULL, {now_ms}, 'test');
             INSERT INTO chunks VALUES ('c1', '{pid}', 'a.rs');
             INSERT INTO chunks VALUES ('c2', '{pid}', 'b.rs');
             INSERT INTO chunks VALUES ('c3', '{pid}', 'c.rs');
             INSERT INTO chunks_fts (rowid, content) VALUES (1, 'x');
             INSERT INTO chunks_fts (rowid, content) VALUES (99, 'ghost');"
        ))
        .unwrap();
    }
    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    let a = claudecat::cort::audit_index(&proj).expect("audit_index 應有結果");
    // docs 數量相等（2 == 2）但內容錯位：c2/c3 沒進 FTS、rowid 99 是孤兒
    assert_eq!(a.fts_docs, Some(2));
    assert_eq!(a.chunk_count, 3);
    assert_eq!(a.fts_drift, Some(3), "缺 2（c2,c3）+ 孤兒 1（rowid 99）= 3");
    assert_ne!(a.fts_drift, Some(0), "數量相等不得判為 synced");
    drop(guard);
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

/// P3 回歸：DB 檔存在但讀取失敗（如非 SQLite 檔、schema 全不相容）時，
/// 必須回報「存在但無法讀取」，不是誤報「尚未建立索引」。
#[test]
fn cort_audit_distinguishes_unreadable_db_from_missing_index() {
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
    // 檔案在，但不是 SQLite 資料庫
    fs::write(
        cache.join(format!("{pid}.db")),
        b"definitely not a database",
    )
    .unwrap();

    let _env_serial = cort_env_lock();
    let guard = EnvVarGuard(
        "CORT_CACHE_DIR".to_string(),
        std::env::var("CORT_CACHE_DIR").ok(),
    );
    std::env::set_var("CORT_CACHE_DIR", cache.to_str().unwrap());

    assert!(claudecat::cort::db_exists(&proj), "DB 檔存在");
    assert!(claudecat::cort::audit_index(&proj).is_none(), "讀取失敗");
    let a = claudecat::cort::audit(&proj, 30);
    assert!(a.db_exists);
    assert!(a.index.is_none());
    let report = claudecat::cort_audit::render(&a);
    assert!(
        report.contains("存在但無法讀取"),
        "應說「DB 存在但無法讀取」，不是「尚未建立索引」"
    );
    drop(guard);
}
