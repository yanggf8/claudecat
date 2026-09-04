//! explore：量化 Claude 若無地圖時的探索成本 vs 地圖成本
use crate::model::ProjectMap;
use crate::outline::render_markdown;

fn est_tokens_from_loc(loc: usize) -> usize {
    loc * 6 // 粗略：每行約 6 tokens
}

pub fn render_explore(map: &ProjectMap) -> String {
    let mut s = String::new();
    let map_md = render_markdown(map);
    let map_chars = map_md.chars().count();
    let map_tokens = map_chars / 4;
    let full_tokens = est_tokens_from_loc(map.total_loc);

    s.push_str("# Exploration Cost Report (claudecat explore)\n\n");
    s.push_str(&format!("- **Root**: `{}`\n", map.root));
    s.push_str(&format!(
        "- **Code scale**: {} files, {} LOC\n",
        map.total_files, map.total_loc
    ));
    s.push_str(&format!(
        "- **Map token cost**: ~{} tokens ({} chars)\n",
        map_tokens, map_chars
    ));
    s.push_str(&format!(
        "- **Read-everything cost**: ~{} tokens (LOC×6)\n",
        full_tokens
    ));
    if full_tokens > 0 {
        let pct = 100.0 - (map_tokens as f64 / full_tokens as f64) * 100.0;
        s.push_str(&format!("- **Estimated token savings**: {:.1}%\n", pct));
    }

    s.push_str("\n## 只看 top-K 檔案的覆蓋率\n\n");
    s.push_str("| K | 累計 LOC | 佔總 LOC % |\n|---|--------:|----------:|\n");
    let mut acc = 0usize;
    for (i, f) in map.key_files.iter().enumerate() {
        acc += f.loc;
        let k = i + 1;
        if [5, 10, 20, 50].contains(&k) {
            let pct = acc as f64 / map.total_loc.max(1) as f64 * 100.0;
            s.push_str(&format!("| {k} | {acc} | {pct:.1}% |\n"));
        }
    }
    if map.total_loc > 0 {
        s.push_str(&format!(
            "| 全部 | {} | 100% |\n",
            map.total_loc
        ));
    }

    s.push_str("\n## 解讀\n\n");
    s.push_str("- **Map token cost** = `claudecat update` 寫進 CLAUDE.md 的地圖成本（不到 1k tokens）。\n");
    s.push_str("- **Read-everything cost** = 若 Claude 沒有地圖、只能把全部程式碼讀完的粗估。\n");
    s.push_str("- 真實 session 資料顯示 64–96% 工具呼叫花在探索；地圖把「找結構」變成「看地圖」。\n");
    s
}
