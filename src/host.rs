//! 主機名探測：`/etc/hostname`（Linux 慣例）優先；macOS 沒有這個檔，退回 `hostname` 指令。

/// host 名：`/etc/hostname`（Linux 慣例）優先；macOS 沒有這個檔，退回 `hostname` 指令。
/// 兩條路都拿不到才回 None——一條永遠 ✗ 的檢查在 Mac 上等於把這台機器永遠記成 unknown。
pub fn resolve_host() -> Option<String> {
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return Some(h.to_string());
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}
