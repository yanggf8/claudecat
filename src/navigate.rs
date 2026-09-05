//! navigate：從「意圖」到「目的地」的低成本路線指引
//! 輸入一句話（例如 "auth"、"find user creation"），輸出：
//! - 命中的符號（檔案:行號 + kind）
//! - 命中的檔案
//! - 建議的 cort 命令路線（cortexyoung 的精準查詢）
use crate::model::ProjectMap;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct NavigateHit {
    pub kind: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub exact: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigateResult {
    pub query: String,
    pub symbols: Vec<NavigateHit>,
    pub files: Vec<String>,
    pub route: Vec<String>,
}

/// 把查詢拆成 token（小寫、去符號）
fn tokens(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn symbol_matches(name: &str, kind: &str, toks: &[String]) -> bool {
    let n = name.to_lowercase();
    let k = kind.to_lowercase();
    toks.iter().any(|t| n.contains(t.as_str())) || toks.iter().any(|t| k.contains(t.as_str()))
}

pub fn navigate(map: &ProjectMap, query: &str) -> NavigateResult {
    let toks = tokens(query);
    let mut symbols: Vec<NavigateHit> = Vec::new();
    let mut files_hit: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for f in &map.key_files {
        let path_low = f.path.to_lowercase();
        let file_match = toks.iter().any(|t| path_low.contains(t.as_str()));
        if file_match {
            files_hit.insert(f.path.clone());
        }
        for sym in &f.symbols {
            if symbol_matches(&sym.name, &sym.kind, &toks) {
                let exact = toks.iter().any(|t| sym.name.to_lowercase() == *t);
                symbols.push(NavigateHit {
                    kind: sym.kind.clone(),
                    name: sym.name.clone(),
                    file: f.path.clone(),
                    line: sym.line,
                    exact,
                });
                files_hit.insert(f.path.clone());
            }
        }
    }

    // 排序：精確命中優先，其次種類，再行號
    symbols.sort_by(|a, b| {
        b.exact
            .cmp(&a.exact)
            .then_with(|| b.kind.cmp(&a.kind))
            .then_with(|| a.line.cmp(&b.line))
    });

    let mut route: Vec<String> = Vec::new();
    if symbols.is_empty() && files_hit.is_empty() {
        route.push(format!(
            "在地圖（top-{} 大檔）沒找到「{}」——試 cort 精確查詢：`cort context \"{query}\"` 或 `cort struct -p '{}'`",
            map.key_files.len(),
            query,
            query
        ));
    } else {
        if let Some(first) = symbols.first() {
            route.push(format!(
                "先讀 {}:{}（{} {}）",
                first.file, first.line, first.kind, first.name
            ));
        } else if let Some(first_file) = files_hit.iter().next() {
            route.push(format!("先讀 {}（檔案命中）", first_file));
        }
        if !symbols.is_empty() {
            let top = &symbols[0];
            route.push(format!(
                "接著用 cort 深挖符號：`cort context {} --content full -f lean`",
                top.name
            ));
            route.push(format!(
                "改動前檢查影響：`cort impact --symbol {} --depth 1 -f lean`",
                top.name
            ));
        } else {
            let f = files_hit.iter().next().unwrap();
            route.push(format!("用 cort 讀檔：`cort read {} -f lean`", f));
        }
        route.push(format!("若仍不中，擴大：`cort struct -p '{}' --lang <lang>`", query));
    }

    NavigateResult {
        query: query.to_string(),
        symbols: symbols.into_iter().take(20).collect(),
        files: files_hit.into_iter().take(10).collect(),
        route,
    }
}

pub fn render(r: &NavigateResult) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# claudecat navigate \"{}\" — {} 命中\n\n",
        r.query,
        r.symbols.len()
    ));

    if r.symbols.is_empty() && r.files.is_empty() {
        s.push_str("未命中。\n\n");
        for step in &r.route {
            s.push_str(&format!("- {step}\n"));
        }
        return s;
    }

    if !r.symbols.is_empty() {
        s.push_str("## 符號\n");
        s.push_str("| 位置 | 種類 | 符號 |\n|---|---|---|\n");
        for h in &r.symbols {
            s.push_str(&format!("| `{}:{}` | {} | {} |\n", h.file, h.line, h.kind, h.name));
        }
    }
    if !r.files.is_empty() {
        s.push_str("\n## 檔案\n");
        for f in &r.files {
            s.push_str(&format!("- `{f}`\n"));
        }
    }
    s.push_str("\n## 路線（低成本到目的地）\n");
    for (i, step) in r.route.iter().enumerate() {
        s.push_str(&format!("{}. {step}\n", i + 1));
    }
    s
}
