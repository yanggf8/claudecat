use claudecat::claude_md;
use claudecat::cort;
use claudecat::cort_audit;
use claudecat::doctor;
use claudecat::explore;
use claudecat::guardrails;
use claudecat::manifest;
use claudecat::model::{self, MapProfile};
use claudecat::navigate;
use claudecat::outline;
use claudecat::symbols;
use claudecat::walk;

use clap::{Parser, Subcommand, ValueEnum};
use model::ProjectMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(
    name = "claudecat",
    version,
    about = "ClaudeCat V2 — 給 Claude Code 一張可信任的專案導航地圖"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 掃描專案並輸出導航地圖
    Scan {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 輸出格式
        #[arg(long, value_enum, default_value = "markdown")]
        format: Format,
        /// 要解析符號的最大檔案數
        #[arg(long, default_value_t = 15)]
        top_files: usize,
        /// 地圖樣式：auto(依規模) | mini(迷你) | full(導航)
        #[arg(long, value_enum, default_value = "auto")]
        map: MapArg,
    },
    /// 量化探索成本（無地圖 vs 地圖 token 粗估）
    Explore {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 要解析符號的最大檔案數
        #[arg(long, default_value_t = 50)]
        top_files: usize,
        /// 以 JSON 輸出指標（機器可讀）
        #[arg(long)]
        json: bool,
        /// 地圖樣式：auto(依規模) | mini(迷你) | full(導航)
        #[arg(long, value_enum, default_value = "auto")]
        map: MapArg,
    },
    /// 顯示 cortexyoung/cort 索引狀態（新鮮度、chunk、relationships）
    CortStatus {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// 體檢本機的追蹤循環；--install-track 一鍵部署每日 crontab（幂等）
    Doctor {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 安裝每日追蹤 crontab 條目
        #[arg(long)]
        install_track: bool,
    },
    /// cort 整合審計：索引健康 / 覆蓋缺口 / FTS 同步 / 用量（收集數據驗證 → 據此改善）
    CortAudit {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 用量統計窗口（天）
        #[arg(long, default_value_t = 30)]
        window: u32,
        /// JSON 輸出
        #[arg(long)]
        json: bool,
        /// 寫進文件的長期指標表（原子、同日更新），例如 SESSION-EVIDENCE.md
        #[arg(long)]
        track: Option<PathBuf>,
    },
    /// 導航：從一句話找到目的地符號/檔案，並給出 cort 低成本路線
    Navigate {
        /// 要找什麼（例如 auth / user creation）
        query: String,
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 要解析符號的最大檔案數
        #[arg(long, default_value_t = 30)]
        top_files: usize,
        /// JSON 輸出
        #[arg(long)]
        json: bool,
        /// 優先使用 cortexyoung/cort 全量索引（更完整；需先 cort index）
        #[arg(long)]
        cort: bool,
    },
    /// 把 explore 指標寫進文件的「長期指標」表（原子、同日同專案更新）
    Track {
        /// 目標 markdown 檔（例如 SESSION-EVIDENCE.md）
        file: PathBuf,
        /// 專案根目錄（可重複；省略＝cwd）
        #[arg(long, default_value = ".")]
        root: Vec<PathBuf>,
        /// 一行一個 repo 路徑的文字檔（與 --root 併用）
        #[arg(long)]
        roots_file: Option<PathBuf>,
        /// 要解析符號的最大檔案數
        #[arg(long, default_value_t = 50)]
        top_files: usize,
        /// 地圖樣式：auto(依規模) | mini(迷你) | full(導航)
        #[arg(long, value_enum, default_value = "auto")]
        map: MapArg,
    },
    /// 更新 CLAUDE.md 的 claudecat 自動區塊
    Update {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 只顯示差異，不寫入
        #[arg(long)]
        dry_run: bool,
        /// 要解析符號的最大檔案數
        #[arg(long, default_value_t = 15)]
        top_files: usize,
        /// 地圖樣式：auto(依規模) | mini(迷你) | full(導航)
        #[arg(long, value_enum, default_value = "auto")]
        map: MapArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum MapArg {
    Auto,
    Mini,
    Full,
}

impl MapArg {
    fn into_profile(self) -> Option<MapProfile> {
        match self {
            MapArg::Auto => None,
            MapArg::Mini => Some(MapProfile::Mini),
            MapArg::Full => Some(MapProfile::Full),
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Markdown,
    Json,
}

fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // UTC-ish timestamp without extra deps
    let days = secs / 86400;
    let (y, m, d) = claudecat::dates::civil_from_days(days as i64);
    let (hh, mm, ss) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn analyze(root: &PathBuf, top_files: usize, map_flag: Option<MapProfile>) -> ProjectMap {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
    let mut map = walk::analyze_project(&root, top_files, map_flag);

    // manifest metadata + deps
    let (meta, deps) = manifest::detect_project_meta(&root);
    map.meta = meta;
    map.deps = deps;

    // symbols for key files
    for f in &mut map.key_files {
        if let Some(lang) = &f.language {
            let lang_for_parse = match lang.as_str() {
                "typescript" => "typescript",
                "javascript" => "javascript",
                "python" => "python",
                "rust" => "rust",
                "go" => "go",
                "c" => "c",
                "cpp" => "cpp",
                _ => continue,
            };
            let full = root.join(&f.path);
            if let Ok(src) = std::fs::read_to_string(&full) {
                f.symbols = symbols::extract_symbols(lang_for_parse, &src);
            }
        }
    }

    // guardrails: 開發者維護的技術決策（claudecat-guardrails.md 或既有 CLAUDE.md 區塊）
    let claude_md_path = claude_md::find_claude_md(&root);
    let claude_existing = std::fs::read_to_string(&claude_md_path).unwrap_or_default();
    map.guardrails = guardrails::load(&root, &claude_existing);

    map.generated_at = now_iso();
    map.errors = vec![];
    map
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Scan {
            root,
            format,
            top_files,
            map: mf,
        } => {
            let map = analyze(&root, top_files, mf.into_profile());
            match format {
                Format::Markdown => {
                    println!("{}", outline::render_with_profile(&map, map.profile_used))
                }
                Format::Text => {
                    let md = outline::render_with_profile(&map, map.profile_used);
                    // text mode: strip markdown * emphasis only
                    println!("{}", md.replace('*', ""));
                }
                Format::Json => match serde_json::to_string_pretty(&map) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("JSON serialization failed: {e}");
                        std::process::exit(1);
                    }
                },
            }
        }
        Commands::Explore {
            root,
            top_files,
            json,
            map: mf,
        } => {
            let map = analyze(&root, top_files, mf.into_profile());
            let m = explore::compute_with_profile(&map, map.profile_used);
            if json {
                match serde_json::to_string_pretty(&m) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("JSON serialization failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!("{}", explore::render(&m));
            }
        }
        Commands::CortStatus { root } => match cort::index_info(&root) {
            Some(info) => {
                println!(
                    "cort index: {} ({}) — chunks={} relationships={} fresh={}",
                    info.name,
                    info.path,
                    info.chunk_count,
                    info.relationships_count,
                    if info.fresh { "fresh" } else { "STALE or old" }
                );
                if let Some(head) = &info.git_head {
                    println!("  git head: {}", &head[..head.len().min(12)]);
                }
                if let Some(t) = info.last_indexed_at {
                    println!("  last indexed: {}", t);
                }
                if !info.fresh {
                    eprintln!("  hint: 執行 `cort index` 更新索引");
                }
            }
            None => {
                if cort::db_exists(&root) {
                    println!("cort index: DB 存在但無法讀取（{}）——schema 不相容或檔案損毀？不假裝「尚未索引」", root.display());
                } else {
                    println!("cort index: 無（尚未對 {} 建立索引）", root.display());
                    eprintln!("  hint: 在該專案執行 `cort index` 後再用 navigate --cort");
                }
            }
        },
        Commands::Doctor {
            root,
            install_track,
        } => {
            if install_track {
                let line = doctor::track_cron_line(env!("CARGO_MANIFEST_DIR"), &root);
                let existing = doctor::read_crontab();
                if doctor::has_track_entry(&existing) {
                    println!("Track cron: 已存在，不重複安裝");
                } else {
                    match doctor::write_crontab(&doctor::merge_crontab(&existing, &line)) {
                        Ok(()) => println!("Track cron -> 已安裝：\n  {line}"),
                        Err(e) => {
                            eprintln!("Failed to install track crontab: {e}");
                            std::process::exit(1);
                        }
                    }
                }
            }
            println!("{}", doctor::report(&root));
        }
        Commands::CortAudit {
            root,
            window,
            json,
            track,
        } => {
            let a = cort::audit(&root, window);
            if json {
                match serde_json::to_string_pretty(&a) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("JSON serialization failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!("{}", cort_audit::render(&a));
            }
            if let Some(f) = track {
                match cort_audit::track_update(&f, &[&a]) {
                    Ok((changed, path)) => println!(
                        "{} {}",
                        if changed {
                            "Audit metrics ->"
                        } else {
                            "Up to date:"
                        },
                        path
                    ),
                    Err(e) => {
                        eprintln!("Failed to track cort audit into {}: {e}", f.display());
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Navigate {
            query,
            root,
            top_files,
            json,
            cort: use_cort,
        } => {
            let map = analyze(&root, top_files, Some(MapProfile::Full));
            let mut r = navigate::navigate(&map, &query);
            if use_cort {
                if let Some(hits) = cort::search_symbols(&root, &query) {
                    r = navigate::navigate_with_cort(&map, &query, hits, false);
                } else if let Some(hits) = cort::search_fts(&root, &query) {
                    r = navigate::navigate_with_cort(&map, &query, hits, true);
                } else {
                    eprintln!("cort index 無命中（或未索引）。已回退到 tree-sitter 地圖結果。");
                }
            }
            if json {
                match serde_json::to_string_pretty(&r) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("JSON serialization failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!("{}", navigate::render(&r));
            }
        }
        Commands::Track {
            file,
            root,
            roots_file,
            top_files,
            map: mf,
        } => {
            // roots：--root 可重複；有 --roots-file 時忽略預設 "."；最後去重
            let mut roots: Vec<PathBuf> = Vec::new();
            if let Some(rf) = roots_file {
                if let Ok(text) = std::fs::read_to_string(&rf) {
                    for line in text.lines() {
                        let t = line.trim();
                        if !t.is_empty() && !t.starts_with('#') {
                            roots.push(PathBuf::from(t));
                        }
                    }
                } else {
                    eprintln!("Cannot read roots file: {}", rf.display());
                    std::process::exit(1);
                }
            } else {
                roots = root.clone();
                if roots.is_empty() {
                    roots.push(PathBuf::from("."));
                }
            }
            // 去重（canonical path）
            let mut seen = std::collections::HashSet::new();
            let roots: Vec<PathBuf> = roots
                .into_iter()
                .filter(|r| {
                    let canon = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
                    seen.insert(canon)
                })
                .collect();
            let mut metrics: Vec<explore::ExploreMetrics> = Vec::new();
            for r in &roots {
                let map = analyze(r, top_files, mf.into_profile());
                metrics.push(explore::compute_with_profile(&map, map.profile_used));
            }
            let refs: Vec<&explore::ExploreMetrics> = metrics.iter().collect();
            match explore::track_update(&file, &refs) {
                Ok((changed, path)) => {
                    let n = metrics.len();
                    println!(
                        "{} {} ({} repo{})",
                        if changed {
                            "Recorded metrics ->"
                        } else {
                            "Up to date:"
                        },
                        path,
                        n,
                        if n == 1 { "" } else { "s" }
                    );
                }
                Err(e) => {
                    eprintln!("Failed to track metric into {}: {e}", file.display());
                    std::process::exit(1);
                }
            }
        }
        Commands::Update {
            root,
            dry_run,
            top_files,
            map: mf,
        } => {
            let map = analyze(&root, top_files, mf.into_profile());
            let section = outline::render_with_profile(&map, map.profile_used);
            let path = claude_md::find_claude_md(&root);
            match claude_md::update_section(&path, &section, dry_run) {
                Ok((changed, _content)) => {
                    if dry_run {
                        println!(
                            "{} (dry-run) {}",
                            if changed {
                                "WOULD UPDATE"
                            } else {
                                "UP TO DATE"
                            },
                            path.display()
                        );
                    } else if changed {
                        println!("Updated {}", path.display());
                    } else {
                        println!("Up to date: {}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("Failed to update {}: {e}", path.display());
                    std::process::exit(1);
                }
            }
        }
    }
}
