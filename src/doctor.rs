//! doctor：體檢本機的追蹤循環（cron / 數據源 / host / cort 索引），並可一鍵部署。
//! 數據（usage.db）天生是每台機器各自的，所以「部署」= 每台機器跑一次
//! `claudecat doctor --install`；各機的列靠 host 欄位在 CORT-AUDIT.md 共存。
use std::path::Path;

/// 每日追蹤的 cron 條目（09:17，避開整點；cargo 用絕對路徑——cron 的 PATH 很瘦）
pub fn track_cron_line(manifest_dir: &str, root: &Path) -> String {
    format!(
        "17 9 * * * cd {manifest_dir} && $HOME/.cargo/bin/cargo run -q --manifest-path \
         {manifest_dir}/Cargo.toml -- cort-audit --root {} --track CORT-AUDIT.md >> \
         {manifest_dir}/cort-audit.log 2>&1",
        root.display()
    )
}

/// 每日分析的 cron 條目（09:29；headless agent，prompt 檔在 repo 裡可審查、可版本化）。
/// 執行檔絕對路徑在安裝時解析（cron 的 PATH 很瘦）。
pub fn analysis_cron_line(bin: &str, manifest_dir: &str) -> String {
    format!(
        "29 9 * * * {bin} -p --dangerously-skip-permissions \
         \"$(cat {manifest_dir}/cort-audit-analysis-prompt.md)\" >> \
         {manifest_dir}/cort-audit-analysis.log 2>&1"
    )
}

/// 判斷既有 crontab 是否已含追蹤條目（寬鬆比對：cort-audit + --track）
pub fn has_track_entry(crontab: &str) -> bool {
    crontab
        .lines()
        .any(|l| l.contains("cort-audit") && l.contains("--track"))
}

/// 判斷既有 crontab 是否已含分析條目
pub fn has_analysis_entry(crontab: &str) -> bool {
    crontab
        .lines()
        .any(|l| l.contains("cort-audit-analysis-prompt.md"))
}

/// 分析條目用哪顆執行檔。`CLAUDECAT_ANALYSIS_BIN` 指名（musecode/claude/絕對路徑）；
/// 未指定 = auto：musecode 優先（額度與 claude 訂閱獨立，claude 限額週不會拖垮循環），
/// 退 claude。額度狀況改變時調頭＝換 env 值重跑一次 `--install`，條目會被重寫。
pub fn analysis_bin() -> Result<String, String> {
    let prefer = std::env::var("CLAUDECAT_ANALYSIS_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    let mut order: Vec<String> = Vec::new();
    match &prefer {
        Some(name) => order.push(name.clone()),
        None => {
            order.push("musecode".to_string());
            order.push("claude".to_string());
        }
    }
    for name in &order {
        if let Some(p) = resolve_bin(name) {
            return Ok(p);
        }
    }
    Err(match prefer {
        Some(n) => format!("找不到指定的分析執行檔：{n}"),
        None => "找不到 musecode 也找不到 claude".to_string(),
    })
}

fn resolve_bin(name: &str) -> Option<String> {
    if let Ok(o) = std::process::Command::new("which").arg(name).output() {
        if o.status.success() {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(p);
            }
        }
    }
    let p = format!("{}/.local/bin/{name}", std::env::var("HOME").ok()?);
    Path::new(&p).is_file().then_some(p)
}

/// 併入分析條目：既有條目用的若不是目前偏好的執行檔（`bin`）→ 移除重寫；
/// 已是偏好執行檔 → 不動。回傳 (新內容, 是否有變更)。
pub fn merge_analysis_entry(existing: &str, line: &str, bin: &str) -> (String, bool) {
    let is_analysis = |l: &str| l.contains("cort-audit-analysis-prompt.md");
    let stale = |l: &str| is_analysis(l) && !l.contains(bin);
    let kept: Vec<&str> = existing.lines().filter(|l| !stale(l)).collect();
    let up_to_date = kept.iter().any(|l| is_analysis(l));
    if up_to_date {
        let out = format!("{}\n", kept.join("\n"));
        let changed = existing != out;
        return (out, changed);
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    (out, true)
}

/// 把一條 cron 條目併進既有 crontab（幂等：`present` 判定已存在 → 原樣返回）
pub fn merge_entry(existing: &str, line: &str, present: fn(&str) -> bool) -> String {
    if present(existing) {
        return existing.to_string();
    }
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
    out.push('\n');
    out
}

/// 讀目前使用者的 crontab（無 crontab 或 crontab 不存在 → 空字串）
pub fn read_crontab() -> String {
    std::process::Command::new("crontab")
        .arg("-l")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// 寫回 crontab（完整內容走 stdin 的 `crontab -`）
pub fn write_crontab(content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_ref()
        .expect("stdin must be piped")
        .write_all(content.as_bytes())?;
    if child.wait()?.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("crontab - exited non-zero"))
    }
}

/// 體檢報告：循環賴以運作的每一環，逐項 ✓/✗，✗ 帶下一步
pub fn report(root: &Path) -> String {
    let mut s = String::from("# claudecat doctor\n\n");
    let host = std::fs::read_to_string("/etc/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty());
    s.push_str(&format!(
        "- [{}] host 可讀（{}）——多機的列靠它區分\n",
        tick(host.is_some()),
        host.as_deref().unwrap_or("unknown")
    ));

    let cache = crate::cort::cache_dir();
    s.push_str(&format!(
        "- [{}] cort cache dir：{}\n",
        tick(cache.is_dir()),
        cache.display()
    ));
    let usage = cache.join("usage.db");
    s.push_str(&format!(
        "- [{}] usage.db（用量數據源）{}\n",
        tick(usage.is_file()),
        if usage.is_file() {
            String::new()
        } else {
            "— 裝了 cort 並用過之後就會有".to_string()
        }
    ));

    let pid_db = std::fs::canonicalize(root)
        .ok()
        .and_then(|r| r.to_str().map(crate::cort::db_path_for));
    let indexed = pid_db.as_ref().is_some_and(|p| p.is_file());
    s.push_str(&format!(
        "- [{}] 本專案 cort 索引（{}）{}\n",
        tick(indexed),
        root.display(),
        if indexed {
            match crate::cort::index_info(root) {
                Some(info) if info.fresh => "— fresh".to_string(),
                Some(_) => "— STALE（執行 `cort index`）".to_string(),
                None => "— 存在但無法讀取".to_string(),
            }
        } else {
            "— 執行 `cort index` 後可用 navigate --cort".to_string()
        }
    ));

    let track_file = root.join("CORT-AUDIT.md");
    s.push_str(&format!(
        "- [{}] 長期指標檔：{}\n",
        tick(track_file.is_file()),
        track_file.display()
    ));

    let crontab = read_crontab();
    let installed = has_track_entry(&crontab);
    s.push_str(&format!(
        "- [{}] 每日追蹤 crontab {}\n",
        tick(installed),
        if installed {
            String::new()
        } else {
            "— 跑 `claudecat doctor --install` 一鍵安裝".to_string()
        }
    ));
    match analysis_bin() {
        Ok(bin) => {
            let up_to_date = has_analysis_entry(&crontab)
                && crontab
                    .lines()
                    .any(|l| l.contains("cort-audit-analysis-prompt.md") && l.contains(&bin));
            let runner = Path::new(&bin)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| bin.clone());
            s.push_str(&format!(
                "- [{}] 每日分析 crontab（headless {} -p）{}\n",
                tick(up_to_date),
                runner,
                if up_to_date {
                    String::new()
                } else if has_analysis_entry(&crontab) {
                    "— 條目用的不是目前的執行檔，跑 `claudecat doctor --install` 切換".to_string()
                } else {
                    "— 跑 `claudecat doctor --install` 一鍵安裝".to_string()
                }
            ));
        }
        Err(e) => s.push_str(&format!(
            "- [✗] 每日分析 crontab（{e}）——可用 CLAUDECAT_ANALYSIS_BIN 指定執行檔\n"
        )),
    }
    s
}

fn tick(ok: bool) -> char {
    if ok {
        '✓'
    } else {
        '✗'
    }
}
