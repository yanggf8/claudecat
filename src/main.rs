use claudecat::claude_md;
use claudecat::cort;
use claudecat::cort_audit;
use claudecat::data_dir;
use claudecat::doctor;
use claudecat::explore;
use claudecat::guardrails;
use claudecat::manifest;
use claudecat::markdown;
use claudecat::memory_eval;
use claudecat::model::{self, MapProfile};
use claudecat::navigate;
use claudecat::outline;
use claudecat::symbols;
use claudecat::turso_memory;
use claudecat::usage;
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
    /// 體檢本機的追蹤循環；--install 一鍵部署全部 crontab 條目（幂等）
    Doctor {
        /// 專案根目錄
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// 安裝每日追蹤 + 分析的 crontab 條目
        #[arg(long)]
        install: bool,
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
    /// 把每日分析的發現寫進文件的發現區塊（原子；區塊外原樣保留）
    Findings {
        /// 發現內容（markdown）；用 `-` 從 stdin 讀
        text: String,
        /// 寫進哪個文件
        #[arg(long, default_value = "CORT-AUDIT.md")]
        file: PathBuf,
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
    /// 顯示本機導航統計；不包含原始查詢內容
    Usage {
        /// 統計窗口（天）
        #[arg(long, default_value_t = 30)]
        days: u32,
        /// JSON 輸出
        #[arg(long)]
        json: bool,
    },
    /// 查詢或記錄有來源的專案記憶；使用 Turso Database 同步
    #[command(
        after_help = "其餘子指令（configure、record、recall、status、sync 等）原樣轉交給 cort；用法見 `cort memory --help`。"
    )]
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// 將本機結構化收集事件批次同步到共用 TursoDB
    Telemetry {
        #[command(subcommand)]
        action: TelemetryAction,
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

/// 只列出 ClaudeCat 自己處理的兩個子指令。其餘 memory 子指令（configure、sync、status、
/// import-legacy、record、resolve、recall）在 `main()` 解析參數前就整段轉給 `cort memory`。
#[derive(Subcommand)]
enum MemoryAction {
    /// 本機查詢事件統計，不含查詢內容，也不代表答案正確
    Usage {
        #[arg(long, default_value_t = 30)]
        days: u32,
        #[arg(long)]
        json: bool,
    },
    /// 統計人工評分的成對試驗，檢查每筆原始輸出的 SHA-256
    EvalReport {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TelemetryAction {
    /// 寫入 telemetry 同步用的 Turso 連線設定（token 由 stdin 讀入）
    Configure {
        #[arg(long)]
        url: String,
        #[arg(long)]
        token_stdin: bool,
    },
    /// 同步 cort 用量、ClaudeCat 導航與記憶查詢、每日成本報告
    Sync {
        #[arg(long)]
        cort_usage: Option<PathBuf>,
        #[arg(long)]
        claudecat_usage: Option<PathBuf>,
        #[arg(long)]
        cost_reports: Option<PathBuf>,
        #[arg(long)]
        json: bool,
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
    map.document_headings = walk::collect_markdown_files(&root)
        .iter()
        .flat_map(|path| markdown::index_file(&root, path))
        .collect();

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
    // Cortexyoung owns the shared memory database. ClaudeCat remains a client, forwarding the
    // memory verbs and their stdin unchanged so it cannot acquire a second active ledger.
    // `memory --help` stays here: forwarded, it hid the two verbs only ClaudeCat answers.
    let raw: Vec<_> = std::env::args_os().skip(1).collect();
    if raw.first().is_some_and(|arg| arg == "memory")
        && !matches!(
            raw.get(1).and_then(|arg| arg.to_str()),
            Some("usage" | "eval-report" | "--help" | "-h" | "help")
        )
    {
        let bin = std::env::var_os("CORT_BIN").unwrap_or_else(|| "cort".into());
        match std::process::Command::new(bin).args(&raw).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("cannot run cort memory: {e}");
                std::process::exit(1);
            }
        }
    }
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
                // schema / 圖狀態與 HEAD+age 分開講：三種「不健康」混成一句就沒人知道要做什麼
                println!(
                    "  schema: {} graph_pending: {}",
                    info.schema_version.as_deref().unwrap_or("?"),
                    match info.graph_pending {
                        Some(true) => "1",
                        Some(false) => "0",
                        None => "?",
                    }
                );
                match info.graph_pending {
                    Some(true) => eprintln!(
                        "  hint: 圖是升級/中斷前的舊邊（graph_pending=1）——反向依賴不可信，執行 `cort index` 全量重建"
                    ),
                    None => eprintln!(
                        "  hint: `_cortex_meta` 無法判讀——不保證圖已重建（不視為健康，也不據此判 STALE）"
                    ),
                    Some(false) => {}
                }
                if !info.fresh && info.graph_pending != Some(true) {
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
        Commands::Doctor { root, install } => {
            if install {
                let manifest_dir = env!("CARGO_MANIFEST_DIR");
                let existing = doctor::read_crontab();
                let mut updated = existing.clone();
                if doctor::has_track_entry(&updated) {
                    println!("Track cron: 已存在，不重複安裝");
                } else {
                    updated = doctor::merge_entry(
                        &updated,
                        &doctor::track_cron_line(manifest_dir, &root),
                        doctor::has_track_entry,
                    );
                    println!("Track cron -> 已安裝");
                }
                match doctor::analysis_bin() {
                    Ok(bin) => {
                        let line =
                            doctor::analysis_cron_line(&bin, manifest_dir, &doctor::runtime_dirs());
                        let (next, changed) = doctor::merge_analysis_entry(&updated, &line);
                        updated = next;
                        let had = doctor::has_analysis_entry(&existing);
                        let runner = std::path::Path::new(&bin)
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| bin.clone());
                        if changed && had {
                            println!("Analysis cron -> 已切換至 {runner}");
                        } else if changed {
                            println!("Analysis cron -> 已安裝（{runner}）");
                        } else {
                            println!("Analysis cron: 已是最新（{runner}）");
                        }
                    }
                    Err(e) => eprintln!("Analysis cron: {e}——既有條目維持原樣"),
                }
                if updated != existing {
                    if let Err(e) = doctor::write_crontab(&updated) {
                        eprintln!("Failed to write crontab: {e}");
                        std::process::exit(1);
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
        Commands::Findings { text, file } => {
            let body = if text == "-" {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("讀 stdin 失敗：{e}");
                    std::process::exit(1);
                }
                buf
            } else {
                text
            };
            if body.trim().is_empty() {
                eprintln!("發現內容是空的——不寫入（空區塊比沒有更糟：看起來像跑過且沒發現）");
                std::process::exit(1);
            }
            match cort_audit::findings_update(&file, &body) {
                Ok((changed, _)) => println!(
                    "Findings -> {}{}",
                    file.display(),
                    if changed {
                        ""
                    } else {
                        "（內容相同，未變更）"
                    }
                ),
                Err(e) => {
                    eprintln!("寫入失敗：{e}");
                    std::process::exit(1);
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
            let mut cort_hits = 0;
            if use_cort {
                if let Some(hits) = cort::search_symbols(&root, &query) {
                    cort_hits = hits.len();
                    r = navigate::navigate_with_cort(&map, &query, hits, false);
                } else if let Some(hits) = cort::search_fts(&root, &query) {
                    cort_hits = hits.len();
                    r = navigate::navigate_with_cort(&map, &query, hits, true);
                } else {
                    eprintln!("cort index 無命中（或未索引）。已回退到 tree-sitter 地圖結果。");
                }
            }
            usage::record_navigation(&root, &query, use_cort, cort_hits, &r);
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
        Commands::Usage { days, json } => match usage::report(days) {
            Some(report) if json => match serde_json::to_string_pretty(&report) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("JSON serialization failed: {e}");
                    std::process::exit(1);
                }
            },
            Some(report) => {
                println!("navigation usage ({}d)", report.window_days);
                println!(
                    "events={} cort={} symbol_hits={} document_hits={} no_hits={}",
                    report.events,
                    report.cort_events,
                    report.symbol_hit_events,
                    report.document_hit_events,
                    report.no_hit_events
                );
                for (path, count) in report.top_documents {
                    println!("document={} hits={}", path, count);
                }
            }
            None => {
                eprintln!("無法讀取本機導航統計");
                std::process::exit(1);
            }
        },
        Commands::Memory { action } => match action {
            MemoryAction::Usage { days, json } => match usage::memory_report(days) {
                Ok(report) if json => match serde_json::to_string_pretty(&report) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("memory usage JSON: {e}");
                        std::process::exit(1);
                    }
                },
                Ok(report) => {
                    println!("memory recall usage ({}d)", report.window_days);
                    println!(
                        "calls={} errors={} synced={} offline_or_pending={} absent={} no_match={} unchanged_hits={} changed_hits={} missing_hits={} conflict_hits={}",
                        report.recall_calls, report.errors, report.synced_calls,
                        report.offline_or_pending_calls, report.absent_ledgers,
                        report.no_match_calls, report.unchanged_hits, report.changed_hits,
                        report.missing_hits, report.conflict_hits
                    );
                    for (harness, count) in report.by_harness_declared {
                        println!("harness_declared={harness} calls={count}");
                    }
                    for (machine, count) in report.by_machine {
                        println!("machine={machine} calls={count}");
                    }
                    for (project, count) in report.by_project_id {
                        println!("project_id={project} calls={count}");
                    }
                    println!(
                        "recall_elapsed_ms total={} max={}",
                        report.total_recall_elapsed_ms, report.max_recall_elapsed_ms
                    );
                }
                Err(e) => {
                    eprintln!("memory usage: {e}");
                    std::process::exit(1);
                }
            },
            MemoryAction::EvalReport { file, json } => match memory_eval::report(&file) {
                Ok(report) if json => match serde_json::to_string_pretty(&report) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("memory eval-report JSON: {e}");
                        std::process::exit(1);
                    }
                },
                Ok(report) => {
                    println!(
                        "paired memory eval: trials={} complete_pairs={} incomplete_pairs={}",
                        report.total_trials,
                        report.complete_pairs,
                        report.incomplete_pairs.len()
                    );
                    println!(
                        "correct: on={} off={} improved={} regressed={} both_correct={} both_wrong={}",
                        report.overall.memory_on_correct,
                        report.overall.memory_off_correct,
                        report.overall.improved,
                        report.overall.regressed,
                        report.overall.both_correct,
                        report.overall.both_wrong
                    );
                    println!("{}", report.reading);
                }
                Err(e) => {
                    eprintln!("memory eval-report: {e}");
                    std::process::exit(1);
                }
            },
        },
        Commands::Telemetry { action } => match action {
            TelemetryAction::Configure { url, token_stdin } => {
                if !token_stdin {
                    eprintln!(
                        "telemetry configure: pass --token-stdin to read the auth token privately"
                    );
                    std::process::exit(1);
                }
                use std::io::Read;
                let mut token = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut token) {
                    eprintln!("telemetry configure: {e}");
                    std::process::exit(1);
                }
                match turso_memory::configure(&url, &token) {
                    Ok(path) => println!("Turso telemetry configured at {}", path.display()),
                    Err(e) => {
                        eprintln!("telemetry configure: {e}");
                        std::process::exit(1);
                    }
                }
            }
            TelemetryAction::Sync {
                cort_usage,
                claudecat_usage,
                cost_reports,
                json,
            } => match turso_memory::sync_telemetry(
                cort_usage.as_deref(),
                claudecat_usage.as_deref(),
                cost_reports.as_deref(),
            ) {
                Ok(report) if json => match serde_json::to_string_pretty(&report) {
                    Ok(body) => println!("{body}"),
                    Err(e) => {
                        eprintln!("telemetry JSON: {e}");
                        std::process::exit(1);
                    }
                },
                Ok(report) => println!(
                    "telemetry {}: scanned={} inserted={} already_present={} aliases={} unresolved_projects={} unmatched_project_rows={} missing={}",
                    report.sync_status,
                    report.scanned,
                    report.inserted,
                    report.already_present,
                    report.project_aliases,
                    report.unresolved_project_ids.len(),
                    report.unmatched_project_rows,
                    report.missing_sources.join(",")
                ),
                Err(e) => {
                    eprintln!("telemetry sync: {e}");
                    std::process::exit(1);
                }
            },
        },
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
            let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            let map_path = data_dir::map_path_for(&real.to_string_lossy());
            if !dry_run {
                if let Err(e) =
                    data_dir::write_atomic(&map_path, &data_dir::map_file_body(&section))
                {
                    eprintln!("Failed to write map {}: {e}", map_path.display());
                    std::process::exit(1);
                }
            }
            if dry_run {
                println!("map (dry-run) -> {}", map_path.display());
            } else {
                println!("map -> {}", map_path.display());
            }
            let path = claude_md::find_claude_md(&root);
            match claude_md::update_section(&path, dry_run) {
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
