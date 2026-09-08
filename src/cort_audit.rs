//! cort-audit：把「與 cortexyoung 整合是否達成」變成可收集、可追蹤的數據
//! （索引健康 / 覆蓋缺口 / FTS 同步 / 用量），全部唯讀；
//! `claudecat cort-audit --track <file>` 寫入長期指標表（原子、同日更新）。
use crate::cort::{CortAudit, CortAuditIndex, UsageWindow};
use crate::dates::today_iso;
use std::path::Path;

pub const TRACK_SECTION: &str = "## 長期指標 (claudecat cort-audit)";

fn core_verb_count(u: &UsageWindow) -> i64 {
    ["context", "impact", "recall", "struct", "read"]
        .iter()
        .map(|v| u.by_command.get(*v).copied().unwrap_or(0))
        .sum()
}

fn deep_verb_count(u: &UsageWindow) -> i64 {
    u.by_command.get("context").copied().unwrap_or(0)
        + u.by_command.get("recall").copied().unwrap_or(0)
}

/// Option<i64> 呈現：None = 無法判讀，顯示 `?`（絕不顯示 0 假裝健康）
fn opt_i64(v: &Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
}

fn render_index(i: &CortAuditIndex, s: &mut String) {
    s.push_str("## 索引健康\n");
    s.push_str(&format!(
        "- fresh={}（git head 相符={}，索引距今 {} 天）\n",
        if i.fresh { "fresh" } else { "STALE" },
        i.git_head_matches,
        i.index_age_days
            .map(|d| d.to_string())
            .unwrap_or_else(|| "?".into())
    ));
    s.push_str(&format!(
        "- chunks={} relationships={} name={} path={}\n",
        i.chunk_count, i.relationships_count, i.name, i.path
    ));
    s.push_str(&format!(
        "- schema={} graph_pending={}\n",
        i.schema_version.as_deref().unwrap_or("?"),
        match i.graph_pending {
            Some(true) => "1（圖落後 chunks）",
            Some(false) => "0",
            None => "?（無法判讀）",
        }
    ));
    s.push_str(&format!(
        "- file_state={} files，chunked={} files，FTS docs={}（{}）\n",
        opt_i64(&i.file_state_files),
        opt_i64(&i.chunked_files),
        opt_i64(&i.fts_docs),
        match i.fts_drift {
            Some(0) => "synced".to_string(),
            Some(n) => format!("drift={n}"),
            None => "unknown".to_string(),
        }
    ));
}

fn render_coverage(i: &CortAuditIndex, s: &mut String) {
    s.push_str("\n## 覆蓋缺口\n");
    match i.not_chunked_total {
        None => {
            s.push_str("- 覆蓋狀態無法判讀（file_state/chunks 查詢失敗）——不假裝「無缺口」\n");
        }
        Some(0) => {
            if i.chunk_count == 0 && i.file_state_files == Some(0) {
                s.push_str(
                    "- 空索引（chunks=0、file_state=0）——索引可能壞了或尚未掃描，不算「無缺口」\n",
                );
            } else {
                s.push_str(&format!(
                    "- 無缺口（file_state 全部都有 chunk；含 unparsed chunk 的檔案 {} 檔）\n",
                    i.files_with_unparsed_chunks
                ));
            }
        }
        Some(total) => {
            s.push_str(&format!(
                "- {total} 檔在 file_state 但從未被 chunk（completeness 缺口；列出前 {} 檔）：\n",
                i.not_chunked_files.len()
            ));
            for f in &i.not_chunked_files {
                s.push_str(&format!("  - `{f}`\n"));
            }
        }
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
    if !u.declines.is_empty() {
        s.push_str("\ntop declines（no_shape 歸因；cortexyoung c290c383 起有數據）：\n");
        // not_a_search_tool 是「本來就不是搜尋」的正確沉默（baseline），不是 tuning 目標——
        // 混進排序會把行動靶心擠掉（2026-09-07 實測 165 筆 baseline 壓過一切），另行呈報。
        let baseline = u
            .declines
            .get("no_shape/not_a_search_tool")
            .copied()
            .unwrap_or(0);
        let actionable: Vec<_> = u
            .declines
            .iter()
            .filter(|(k, _)| !k.ends_with("/not_a_search_tool"))
            .collect();
        let no_shape = u.suggest_outcomes.get("no_shape").copied().unwrap_or(0);
        let mut ranked: Vec<_> = actionable.to_vec();
        ranked.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        for (k, c) in ranked.iter().take(5) {
            s.push_str(&format!(
                "- {k}: {c}（佔 no_shape {:.0}%）\n",
                **c as f64 / no_shape.max(1) as f64 * 100.0
            ));
        }
        if baseline > 0 {
            s.push_str(&format!(
                "- （baseline not_a_search_tool: {baseline}，不列入排序）\n"
            ));
        }
    }
    if !u.by_harness.is_empty() {
        // `harness` 只在 hook payload 上（動詞命令沒有），所以這張表只能講
        // 「router 對誰開了口」，不能講「誰真的用了 cort」——標題就說清楚。
        s.push_str("\nharness 切面（僅 hook payload：router 面對誰；v3 payload 起）：\n");
        s.push_str(
            "\n| harness | hook-suggest | 命中 | 命中率 | no_shape | top decline（排除 baseline） | refresh |\n|---|---:|---:|---:|---:|---|---:|\n",
        );
        let mut rows: Vec<_> = u.by_harness.iter().collect();
        rows.sort_by_key(|(_, st)| std::cmp::Reverse(st.suggests));
        for (h, st) in rows {
            s.push_str(&format!(
                "| `{h}` | {} | {} | {} | {} | {} | {} |\n",
                st.suggests,
                st.hits,
                if st.suggests > 0 {
                    format!("{:.2}%", st.hits as f64 / st.suggests as f64 * 100.0)
                } else {
                    "-".to_string()
                },
                st.no_shape,
                st.top_decline
                    .as_ref()
                    .map(|(t, c)| format!("{t}={c}"))
                    .unwrap_or_else(|| "-".into()),
                st.refreshes,
            ));
        }
        if u.harness_unknown > 0 {
            s.push_str(&format!(
                "- 另有 {} 筆 hook 列沒有 `harness` 欄（v3 前的歷史列），不計入上表\n",
                u.harness_unknown
            ));
        }
        for (h, st) in &u.by_harness {
            if st.declared_mismatch > 0 {
                s.push_str(&format!(
                    "- `{h}` 有 {} 筆 `harness_declared` 與實際不符——信任 declared 值的歸因會被汙染\n",
                    st.declared_mismatch
                ));
            }
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
            if a.db_exists {
                s.push_str(
                    "## 索引健康\n- cort DB 存在但無法讀取（schema 不相容或檔案損毀？）——不假裝「尚未索引」\n",
                );
            } else {
                s.push_str("## 索引健康\n- 尚未對本專案建立 cort 索引。\n");
            }
        }
    }
    match &a.usage {
        Some(u) => render_usage(u, &mut s),
        None => {
            s.push_str("\n## 用量\n- usage.db 不存在（cort 尚未被使用過）\n");
        }
    }

    // 7 天早期訊號（與 --window 的長期趨勢互補）
    if let Some(u7) = &a.usage_7d {
        s.push_str(&format!(
            "\n## 7 天早期訊號\n- 命令 {}、deep（context+recall）{}\n",
            u7.total_commands,
            deep_verb_count(u7)
        ));
    }

    s.push_str("\n## 解讀 & 行動（規則式）\n");
    let mut hints: Vec<String> = Vec::new();
    if let Some(i) = &a.index {
        // 圖落後與 HEAD/age 落後是兩件事，行動也不同——不混成同一句
        match i.graph_pending {
            Some(true) => hints.push(
                "graph_pending=1：relationships 是升級或中斷前的舊邊 → 反向依賴（`cort impact` / `navigate --cort` 路線）先別信，執行 `cort index` 全量重建"
                    .to_string(),
            ),
            None => hints.push(
                "`_cortex_meta` 無法判讀（表不存在或查詢失敗）→ 不視為圖已重建；舊版 cort DB 才會沒有這張表"
                    .to_string(),
            ),
            Some(false) => {}
        }
        if !i.fresh && i.graph_pending != Some(true) {
            hints
                .push("索引 STALE → 執行 `cort index`（或檢查 hook-refresh 是否在跑）".to_string());
        }
        if i.chunk_count == 0 && i.file_state_files == Some(0) {
            hints.push(
                "空索引（0 chunks、0 file_state）→ 執行 `cort index` 或檢查 extractor/路徑"
                    .to_string(),
            );
        }
        match i.not_chunked_total {
            Some(n) if n > 0 => hints.push(format!(
                "coverage 缺口：{n} 檔在 file_state 但從未被 chunk → 先確認是否本來就沒有可 chunk 的宣告（如只 import 後呼叫的 driver 檔，實測 5/5 是這種；見 cortexyoung#2），再查 extractor 規則"
            )),
            None => hints.push(
                "覆蓋查詢失敗 → 不視為「無缺口」；檢查 cort schema 版本差異".to_string(),
            ),
            _ => {}
        }
        match i.fts_drift {
            Some(0) => {}
            Some(n) => hints.push(format!(
                "FTS 索引與 chunks 不同步（drift={n}）→ cort 端重建 FTS"
            )),
            None => {
                hints.push("FTS 同步無法判讀（chunks_fts 查詢失敗）→ 不假裝 synced".to_string())
            }
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
            // `unspecified`＝旗標與 transcript 都認不出來源（cort main.rs 的 fallback）。
            // 本機三個接線的 harness 全都帶 `--harness`，所以 agent 流量落不進這格：
            // 實測那些列是開發/安裝時手打的探針（2026-09-09 查證，見 CORT-INTEGRATION.md）。
            // 分母不動（動了跨日不可比），但污染要講出來——30d 實測 40 命中裡有 9 筆是自測。
            if let Some(st) = u.by_harness.get("unspecified") {
                if st.hits > 0 {
                    hints.push(format!(
                        "命中數含 {} 筆 `unspecified`（非 agent 流量的手動探針）→ agent 實際命中 {}／{}；比率請用這組",
                        st.hits,
                        suggests - st.hits,
                        suggest_total - st.suggests
                    ));
                }
            }
            // harness 切面：有量卻 0 命中的 harness 是可指名的靶心（總命中率看不出來）
            for (h, st) in &u.by_harness {
                if st.suggests >= 100 && st.hits == 0 {
                    hints.push(format!(
                        "`{h}` {} 筆 hook-suggest、0 命中 → router 對這個 harness 從沒開過口（top decline: {}）",
                        st.suggests,
                        st.top_decline
                            .as_ref()
                            .map(|(t, c)| format!("{t}={c}"))
                            .unwrap_or_else(|| "無".into())
                    ));
                }
                if st.declared_mismatch > 0 {
                    hints.push(format!(
                        "`{h}` 有 {} 筆 `harness_declared` 不符 → 任何按宣告值分群的歸因都要改用實測值",
                        st.declared_mismatch
                    ));
                }
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

/// 每日分析的發現區塊標題（與長期指標表同檔共存，各自 section）
pub const FINDINGS_SECTION: &str = "## 每日分析發現 (claudecat cort-audit)";

/// 把每日分析的發現寫進文件的發現 section（原子；section 外的內容原樣保留）。
///
/// 為什麼要有這個：排程分析原本只把結論印到 `cort-audit-analysis.log`，等於沒回報
/// （2026-09-09 使用者指出）。發現屬於文件、不屬於 CLAUDE.md——CLAUDE.md 只留規則，
/// 最多放一行指引。**整段取代、只留最新一天**：歷史在 git，文件裡疊成流水帳只會沒人看。
pub fn findings_update(path: &Path, body: &str) -> std::io::Result<(bool, String)> {
    let existing = if path.is_file() {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    let block = format!(
        "{FINDINGS_SECTION}\n\n_{}_\n\n{}\n",
        today_iso(),
        body.trim()
    );
    // section 邊界與長期指標表同一套規則：標題到下一個 `#` 標題（或 EOF）
    let new_content = match existing.find(FINDINGS_SECTION) {
        Some(start) => {
            let after_title = start + FINDINGS_SECTION.len();
            let end = after_title
                + crate::explore::next_heading_offset(&existing[after_title..])
                    .unwrap_or(existing.len() - after_title);
            format!("{}{}{}", &existing[..start], block, &existing[end..])
        }
        None => {
            let mut out = existing.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(&block);
            out
        }
    };
    let changed = new_content != existing;
    if changed {
        crate::explore::atomic_write(path, &new_content)?;
    }
    Ok((changed, new_content))
}

/// cort-audit 長期指標列（單行）：長期趨勢（30d）+ 早期訊號（7d）+ deep 動詞
pub fn row_md(a: &CortAudit) -> String {
    let idx = a.index.as_ref();
    // 不加欄（09-06 那列少一格 host 已經證明加欄的代價），改讓既有 fresh 格說得更準：
    // 圖落後寫 STALE/graph（跟 HEAD/age 的 STALE 分得開），讀不到 meta 寫 fresh?
    let fresh = idx
        .map(|i| match (i.fresh, i.graph_pending) {
            (_, Some(true)) => "STALE/graph",
            (false, _) => "STALE",
            (true, None) => "fresh?",
            (true, Some(false)) => "fresh",
        })
        .unwrap_or("no-index");
    let chunks = idx
        .map(|i| i.chunk_count.to_string())
        .unwrap_or_else(|| "-".into());
    let rels = idx
        .map(|i| i.relationships_count.to_string())
        .unwrap_or_else(|| "-".into());
    let not_chunked = idx
        .map(|i| opt_i64(&i.not_chunked_total))
        .unwrap_or_else(|| "-".into());
    let fts = idx
        .map(|i| match i.fts_drift {
            Some(0) => "synced".to_string(),
            Some(n) => format!("drift={n}"),
            None => "unknown".to_string(),
        })
        .unwrap_or_else(|| "-".into());
    let usage = a.usage.as_ref();
    let cmds = usage
        .map(|u| u.total_commands.to_string())
        .unwrap_or_else(|| "-".into());
    let core = usage
        .map(|u| core_verb_count(u).to_string())
        .unwrap_or_else(|| "-".into());
    let deep = usage
        .map(|u| deep_verb_count(u).to_string())
        .unwrap_or_else(|| "-".into());
    let u7 = a.usage_7d.as_ref();
    let cmds7 = u7
        .map(|u| u.total_commands.to_string())
        .unwrap_or_else(|| "-".into());
    let deep7 = u7
        .map(|u| deep_verb_count(u).to_string())
        .unwrap_or_else(|| "-".into());
    let decline_top = usage
        .and_then(|u| {
            u.declines
                .iter()
                // baseline（本來就不是搜尋的命令）不是行動靶心——排序排除
                .filter(|(k, _)| !k.ends_with("/not_a_search_tool"))
                .max_by_key(|(_, c)| **c)
                .map(|(k, c)| (k.rsplit('/').next().unwrap_or(k).to_string(), *c))
        })
        .map(|(tag, c)| format!("{tag}={c}"))
        .unwrap_or_else(|| "-".into());
    format!(
        "| {} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        today_iso(),
        a.root,
        a.host,
        fresh,
        chunks,
        rels,
        not_chunked,
        fts,
        cmds,
        core,
        deep,
        cmds7,
        deep7,
        decline_top,
    )
}

/// 把多個 cort-audit 寫進文件的長期指標表（原子、同日更新；語意同 explore track）
pub fn track_update(path: &Path, audits: &[&CortAudit]) -> std::io::Result<(bool, String)> {
    let window = audits.first().map(|a| a.window_days).unwrap_or(30);
    let header = format!(
        "{}\n\n| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | FTS drift | 命令數/{window}d | core/{window}d | deep/{window}d | 命令數/7d | deep/7d | decline-top |\n|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
        TRACK_SECTION
    );
    let rows: Vec<String> = audits.iter().map(|a| row_md(a)).collect();
    crate::explore::track_table(path, TRACK_SECTION, &header, &rows, Some(3))
}
