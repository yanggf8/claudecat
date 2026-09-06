//! cort-audit：把「與 cortexyoung 整合是否達成」變成可收集、可追蹤的數據
//! （索引健康 / 覆蓋缺口 / FTS 同步 / 用量），全部唯讀；
//! `claudecat cort-audit --track <file>` 寫入長期指標表（原子、同日更新）。
use crate::cort::{CortAudit, CortAuditIndex, UsageWindow};
use std::path::Path;

pub const TRACK_SECTION: &str = "## 長期指標 (claudecat cort-audit)";

fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    // Howard Hinnant's algorithm（與 main.rs 一致，避免外部依賴）
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn core_verb_count(u: &UsageWindow) -> i64 {
    ["context", "impact", "recall", "struct", "read"]
        .iter()
        .map(|v| u.by_command.get(*v).copied().unwrap_or(0))
        .sum()
}

fn render_index(i: &CortAuditIndex, s: &mut String) {
    s.push_str("## 索引健康\n");
    s.push_str(&format!(
        "- fresh={}（git head 相符={}，索引距今 {} 天）\n",
        if i.fresh { "fresh" } else { "STALE" },
        i.git_head_matches,
        i.index_age_days.map(|d| d.to_string()).unwrap_or_else(|| "?".into())
    ));
    s.push_str(&format!(
        "- chunks={} relationships={} name={} path={}\n",
        i.chunk_count, i.relationships_count, i.name, i.path
    ));
    s.push_str(&format!(
        "- file_state={} files，chunked={} files，FTS docs={}（synced={}）\n",
        i.file_state_files, i.chunked_files, i.fts_docs, i.fts_synced
    ));
}

fn render_coverage(i: &CortAuditIndex, s: &mut String) {
    s.push_str("\n## 覆蓋缺口\n");
    if i.not_chunked_total == 0 {
        s.push_str(&format!(
            "- 無缺口（file_state 全部都有 chunk；含 unparsed chunk 的檔案 {} 檔）\n",
            i.files_with_unparsed_chunks
        ));
        return;
    }
    s.push_str(&format!(
        "- {} 檔在 file_state 但從未被 chunk（completeness 缺口；列出前 {} 檔）：\n",
        i.not_chunked_total,
        i.not_chunked_files.len()
    ));
    for f in &i.not_chunked_files {
        s.push_str(&format!("  - `{f}`\n"));
    }
    s.push_str(&format!(
        "- 含 unparsed chunk 的檔案：{} 檔\n",
        i.files_with_unparsed_chunks
    ));
}

fn render_usage(u: &UsageWindow, s: &mut String) {
    s.push_str(&format!("\n## 用量（last {} 天）\n", u.window_days));
    s.push_str(&format!(
        "- 總命令數 {}；核心動詞（context/impact/recall/struct/read）{}；saved_bytes={}\n",
        u.total_commands,
        core_verb_count(u),
        u.saved_bytes
    ));
    s.push_str("\n| command | count |\n|---|---:|\n");
    for (k, v) in &u.by_command {
        s.push_str(&format!("| `{k}` | {v} |\n"));
    }
    if !u.suggest_outcomes.is_empty() {
        s.push_str("\nhook-suggest 結果：\n");
        for (k, v) in &u.suggest_outcomes {
            s.push_str(&format!("- {k}: {v}\n"));
        }
    }
    if !u.refresh_outcomes.is_empty() {
        s.push_str("\nhook-refresh 結果：\n");
        for (k, v) in &u.refresh_outcomes {
            s.push_str(&format!("- {k}: {v}\n"));
        }
    }
    s.push_str(&format!(
        "\nerrors={} index_stale_queries={}\n",
        u.errors, u.index_stale_queries
    ));
}

/// 把 CortAudit 渲染成可讀報告（含規則式「解讀 / 行動」）
pub fn render(a: &CortAudit) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# claudecat cort-audit — {}（用量窗口 {} 天）\n\n",
        a.root, a.window_days
    ));
    match &a.index {
        Some(i) => {
            render_index(i, &mut s);
            render_coverage(i, &mut s);
        }
        None => {
            s.push_str("## 索引健康\n- 尚未對本專案建立 cort 索引。\n");
        }
    }
    match &a.usage {
        Some(u) => render_usage(u, &mut s),
        None => {
            s.push_str("\n## 用量\n- usage.db 不存在（cort 尚未被使用過）\n");
        }
    }

    s.push_str("\n## 解讀 & 行動（規則式）\n");
    let mut hints: Vec<String> = Vec::new();
    if let Some(i) = &a.index {
        if !i.fresh {
            hints.push("索引 STALE → 執行 `cort index`（或檢查 hook-refresh 是否在跑）".to_string());
        }
        if i.not_chunked_total > 0 {
            hints.push(format!(
                "coverage 缺口：{} 檔在 file_state 但從未被 chunk → 先確認是否本來就沒有可 chunk 的宣告（如只 import 後呼叫的 driver 檔，實測 5/5 是這種；見 cortexyoung#2），再查 extractor 規則",
                i.not_chunked_total
            ));
        }
        if !i.fts_synced {
            hints.push(format!(
                "FTS 索引與 chunks 不同步（docs={} vs chunks={}）→ cort 端重建 FTS",
                i.fts_docs, i.chunk_count
            ));
        }
    }
    if let Some(u) = &a.usage {
        let suggests = u.suggest_outcomes.get("hit").copied().unwrap_or(0)
            + u.suggest_outcomes.get("hit_yielded").copied().unwrap_or(0)
            + u.suggest_outcomes.get("hit_stale").copied().unwrap_or(0);
        let suggest_total = u
            .by_command
            .get("hook-suggest")
            .copied()
            .unwrap_or(0)
            .max(u.suggest_outcomes.values().sum());
        if u.total_commands > 0 {
            let core = core_verb_count(u);
            if core * 100 < u.total_commands {
                hints.push(format!(
                    "核心動詞用量低（{core}/{total}）→ adoption 瓶頸：讓 claudecat navigate 當 front door 帶入 cort",
                    core = core,
                    total = u.total_commands
                ));
            }
            let deep = u.by_command.get("context").copied().unwrap_or(0)
                + u.by_command.get("recall").copied().unwrap_or(0);
            if deep < 5 {
                hints.push(format!(
                    "深挖動詞用量極低（context+recall={deep}）→ agent 幾乎不問「這個符號還有誰在用/上下文」"
                ));
            }
            if suggest_total >= 100 && suggests * 100 < suggest_total {
                hints.push(format!(
                    "hook-suggest 命中率 {suggests}/{suggest_total}（<1%）→ router 大多 no_shape，檢查 hook shape 規則"
                ));
            }
        }
    }
    if hints.is_empty() {
        s.push_str("- 目前沒有需要行動的異常\n");
    } else {
        for h in &hints {
            s.push_str(&format!("- {h}\n"));
        }
    }
    s
}

/// cort-audit 長期指標列（單行）
pub fn row_md(a: &CortAudit) -> String {
    let idx = a.index.as_ref();
    let fresh = idx
        .map(|i| if i.fresh { "fresh" } else { "STALE" })
        .unwrap_or("no-index");
    let chunks = idx
        .map(|i| i.chunk_count.to_string())
        .unwrap_or_else(|| "-".into());
    let rels = idx
        .map(|i| i.relationships_count.to_string())
        .unwrap_or_else(|| "-".into());
    let not_chunked = idx
        .map(|i| i.not_chunked_total.to_string())
        .unwrap_or_else(|| "-".into());
    let fts = idx
        .map(|i| if i.fts_synced { "synced" } else { "diff" })
        .unwrap_or("-");
    let usage = a.usage.as_ref();
    let cmds = usage
        .map(|u| u.total_commands.to_string())
        .unwrap_or_else(|| "-".into());
    let core = usage
        .map(|u| core_verb_count(u).to_string())
        .unwrap_or_else(|| "-".into());
    format!(
        "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} |",
        today_iso(),
        a.root,
        fresh,
        chunks,
        rels,
        not_chunked,
        fts,
        cmds,
        core,
    )
}

/// 把多個 cort-audit 寫進文件的長期指標表（原子、同日更新；語意同 explore track）
pub fn track_update(path: &Path, audits: &[&CortAudit]) -> std::io::Result<(bool, String)> {
    let window = audits.first().map(|a| a.window_days).unwrap_or(30);
    let header = format!(
        "{}\n\n| 日期 | 專案 | fresh | chunks | relationships | 未chunk檔 | FTS | 命令數/{window}d | core動詞 |\n|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
        TRACK_SECTION
    );
    let rows: Vec<String> = audits.iter().map(|a| row_md(a)).collect();
    crate::explore::track_table(path, TRACK_SECTION, &header, &rows)
}
