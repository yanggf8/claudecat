//! explore：量化 Claude 若無地圖時的探索成本 vs 地圖成本
//! 作為長期指標：`claudecat track <file>` 把指標寫進文件的長期指標表
use crate::model::{MapProfile, ProjectMap};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::Path;

pub const TRACK_SECTION: &str = "## 長期指標 (claudecat explore)";

#[derive(Debug, Clone, Serialize)]
pub struct CoverageRow {
    pub k: usize,
    pub loc: usize,
    pub pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExploreMetrics {
    pub root: String,
    pub date: String,
    pub total_files: usize,
    pub total_loc: usize,
    pub map_chars: usize,
    pub map_tokens: usize,
    pub read_tokens: usize,
    pub savings_pct: f64,
    pub coverage: Vec<CoverageRow>,
    pub top10_pct: f64,
    pub map_overhead: bool,
}

fn est_tokens_from_loc(loc: usize) -> usize {
    loc * 6 // 粗略：每行約 6 tokens
}

pub fn compute(map: &ProjectMap) -> ExploreMetrics {
    compute_with_profile(map, MapProfile::Full)
}

pub fn compute_with_profile(map: &ProjectMap, profile: MapProfile) -> ExploreMetrics {
    let map_md = crate::outline::render_with_profile(map, profile);
    let map_chars = map_md.chars().count();
    let map_tokens = map_chars / 4;
    let read_tokens = est_tokens_from_loc(map.total_loc);
    let savings_pct = if read_tokens > 0 {
        (1.0 - map_tokens as f64 / read_tokens as f64) * 100.0
    } else {
        0.0
    };
    // 標記地圖成本是否反而超過全讀（小專案）
    let map_overhead = map_tokens > read_tokens && read_tokens > 0;
    let mut coverage = Vec::new();
    let mut acc = 0usize;
    for (i, f) in map.key_files.iter().enumerate() {
        acc += f.loc;
        let k = i + 1;
        if [5, 10, 20, 50].contains(&k) {
            coverage.push(CoverageRow {
                k,
                loc: acc,
                pct: acc as f64 / map.total_loc.max(1) as f64 * 100.0,
            });
        }
    }
    let top10_pct = coverage
        .iter()
        .find(|r| r.k == 10)
        .map(|r| r.pct)
        .unwrap_or(0.0);
    ExploreMetrics {
        root: map.root.clone(),
        date: map.generated_at[..10].to_string(),
        total_files: map.total_files,
        total_loc: map.total_loc,
        map_chars,
        map_tokens,
        read_tokens,
        savings_pct,
        coverage,
        top10_pct,
        map_overhead,
    }
}

pub fn render(m: &ExploreMetrics) -> String {
    let mut s = String::new();
    s.push_str("# Exploration Cost Report (claudecat explore)\n\n");
    s.push_str(&format!("- **Root**: `{}`\n", m.root));
    s.push_str(&format!(
        "- **Code scale**: {} files, {} LOC\n",
        m.total_files, m.total_loc
    ));
    s.push_str(&format!(
        "- **Map token cost**: ~{} tokens ({} chars)\n",
        m.map_tokens, m.map_chars
    ));
    s.push_str(&format!(
        "- **Read-everything cost**: ~{} tokens (LOC×6)\n",
        m.read_tokens
    ));
    if m.map_overhead {
        s.push_str(&format!("- **Map overhead**: +{:.0}% (地圖成本高於全讀，小專案建議 mini)\n", m.savings_pct.abs()));
    } else {
        s.push_str(&format!("- **Map vs full-read**: 地圖省 ~{:.1}% token\n", m.savings_pct));
    }

    s.push_str("\n## 只看 top-K 檔案的覆蓋率\n\n");
    s.push_str("| K | 累計 LOC | 佔總 LOC % |\n|---|--------:|----------:|\n");
    for r in &m.coverage {
        s.push_str(&format!("| {} | {} | {:.1}% |\n", r.k, r.loc, r.pct));
    }
    if m.total_loc > 0 {
        s.push_str(&format!("| 全部 | {} | 100% |\n", m.total_loc));
    }

    s.push_str("\n## 解讀\n\n");
    s.push_str("- **Map token cost** = `claudecat update` 寫進 CLAUDE.md 的地圖成本（不到 1k tokens）。\n");
    s.push_str("- **Read-everything cost** = 若 Claude 沒有地圖、只能把全部程式碼讀完的粗估（LOC×6）。\n");
    s.push_str("- 真實 session 資料顯示 64–96% 工具呼叫花在探索；地圖把「找結構」變成「看地圖」。\n");
    s
}

pub fn row_md(m: &ExploreMetrics) -> String {
    format!(
        "| {} | `{}` | {} | {} | ~{} | ~{} | {:.1}% | {:.1}% |",
        m.date,
        m.root,
        m.total_files,
        m.total_loc,
        m.map_tokens,
        m.read_tokens,
        m.savings_pct,
        m.top10_pct,
    )
}

/// 把多個指標更新進文件的「長期指標」表格：
/// - 當天已存在且 root 在本次更新集合內的行 -> 以新值取代
/// - 其餘歷史行保留；新 root 追加
/// 原子寫入；回傳 (changed, file_path)。
pub fn track_update(path: &Path, metrics: &[&ExploreMetrics]) -> std::io::Result<(bool, String)> {
    let existing = if path.is_file() {
        fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };

    let header = format!(
        "{}\n\n| 日期 | 專案 | 檔案 | LOC | map tokens | 全讀 tokens | 節省%(map-vs-read) | top-10 覆蓋% |\n|---|---|---:|---:|---:|---:|---:|---:|\n",
        TRACK_SECTION
    );

    // 解析既有表格資料列（`|` 開頭）
    let mut old_rows: Vec<String> = Vec::new();
    if let Some(b) = existing.find(TRACK_SECTION) {
        for line in existing[b..].lines() {
            let t = line.trim();
            if t.starts_with('|') && !t.starts_with("|---") && !t.starts_with("| 日期") {
                old_rows.push(t.to_string());
            }
        }
    }

    fn row_root(row: &str) -> Option<String> {
        let cols: Vec<&str> = row.split('|').collect();
        cols.get(2)
            .map(|c| c.trim().trim_matches('`').to_string())
    }

    // 欄數正規化：舊格式（例如多一個裝飾欄）對齊新 header
    fn normalize_row(row: &str, ncols: usize) -> String {
        let cols: Vec<&str> = row.split('|').collect();
        if cols.len() <= ncols + 2 {
            return row.to_string();
        }
        let kept: Vec<&str> = cols[1..=ncols].iter().map(|c| c.trim()).collect();
        format!("| {} |", kept.join(" | "))
    }
    let ncols = header.matches('|').count() - 1; // 欄數 = 管道數 - 1（前後各一）
    let keep: Vec<String> = old_rows
        .into_iter()
        .filter(|row| {
            // 只刪「同一天 + root 精確相等」的舊列（避免 foo 誤刪 foo-bar）
            let r = row_root(row);
            for m in metrics.iter() {
                if row.contains(&m.date) && r.as_deref() == Some(m.root.as_str()) {
                    return false;
                }
            }
            true
        })
        .map(|row| normalize_row(&row, ncols))
        .collect();

    let prefix = if let Some(b) = existing.find(TRACK_SECTION) {
        existing[..b].to_string()
    } else {
        let mut p = existing.clone();
        if !p.is_empty() && !p.ends_with('\n') {
            p.push('\n');
        }
        if !p.is_empty() {
            p.push('\n');
        }
        p
    };

    let mut table = String::new();
    table.push_str(&header);
    for r in &keep {
        table.push_str(r);
        table.push('\n');
    }
    for m in metrics {
        table.push_str(&row_md(m));
        table.push('\n');
    }
    let new_content = format!("{prefix}{table}");
    let changed = new_content != existing;
    if changed {
        let tmp = path.with_extension("track.tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(new_content.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
    }
    Ok((changed, path.to_string_lossy().into_owned()))
}

/// 相容舊 API（單 repo）
pub fn track_append(path: &Path, m: &ExploreMetrics) -> std::io::Result<(bool, String)> {
    track_update(path, &[m])
}
