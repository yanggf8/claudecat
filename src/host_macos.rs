//! macOS 的主機讀數：macOS 沒有 `/proc`，gate 原本讀 `loadavg`、`meminfo`、`/proc/<pid>/…` 的地方
//! 在這裡改用系統呼叫（getloadavg、sysctl、libproc）。只在「真的沒有 proc 樹」時使用——
//! 測試用 `CLAUDECAT_PROC_ROOT` 指到的假 proc 目錄照舊走檔案，所以 Linux 與 fixture 行為不變。

use std::ffi::{c_void, CStr};
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::path::Path;

/// proc 根目錄根本不存在（macOS 的 `/proc`）才改走系統呼叫；存在的目錄（測試 fixture）照舊讀檔，
/// fixture 少了某個檔就照 Linux 的規則處理（讀不到 ⇒ gate 擋下），不偷偷換成讀這台機器。
pub fn native(root: &Path) -> bool {
    !root.exists()
}

pub fn uid() -> u32 {
    // SAFETY: getuid 不會失敗、沒有參數。
    unsafe { libc::getuid() }
}

fn sysctl_raw<T: Copy>(name: &CStr) -> Option<T> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut len = size_of::<T>();
    // SAFETY: 緩衝區大小就是 T；核心寫入的長度不等於 T 就丟掉結果。
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast::<c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

pub fn cpus() -> Option<usize> {
    sysctl_raw::<i32>(c"hw.logicalcpu")
        .or_else(|| sysctl_raw::<i32>(c"hw.ncpu"))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
}

pub fn mem_total_mb() -> Option<u64> {
    sysctl_raw::<u64>(c"hw.memsize").map(|b| b / 1024 / 1024)
}

/// (1 分鐘 load、已用 swap MB、可用記憶體 MB)。
/// 可用記憶體取 `kern.memorystatus_level`（核心估的「可用」百分比，`memory_pressure` 印的
/// 「System-wide memory free percentage」就是它）乘上實體記憶體——它把可回收的 inactive／purgeable
/// 頁算進去，語意最接近 Linux 的 MemAvailable；只算 free 頁會把 macOS 正常的檔案快取當成不足。
pub fn metrics() -> io::Result<(f64, u64, u64)> {
    let mut loads = [0f64; 3];
    // SAFETY: 陣列長 3，正好是要的筆數。
    if unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) } < 1 {
        return Err(io::Error::other("missing load average"));
    }
    let swap = sysctl_raw::<libc::xsw_usage>(c"vm.swapusage")
        .map(|s| s.xsu_used / 1024 / 1024)
        .ok_or_else(|| io::Error::other("missing vm.swapusage"))?;
    let total = mem_total_mb().ok_or_else(|| io::Error::other("missing hw.memsize"))?;
    let level = sysctl_raw::<i32>(c"kern.memorystatus_level")
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n <= 100)
        .ok_or_else(|| io::Error::other("missing kern.memorystatus_level"))?;
    Ok((loads[0], swap, total * level / 100))
}

/// 一個程序的基本資料；拿不到（已結束、權限不足）就是 None。
pub struct Proc {
    pub uid: u32,
    pub ppid: u32,
    pub name: String,
    /// 程序開始時間（秒＋微秒），用來辨認 pid 有沒有被重用。
    pub start: u64,
}

fn c_chars(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn pidinfo<T>(pid: u32, flavor: libc::c_int) -> Option<T> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut value = MaybeUninit::<T>::uninit();
    let size = libc::c_int::try_from(size_of::<T>()).ok()?;
    // SAFETY: 緩衝區大小就是 T；回傳的位元組數不等於 T 視為失敗。
    let n =
        unsafe { libc::proc_pidinfo(pid, flavor, 0, value.as_mut_ptr().cast::<c_void>(), size) };
    (n == size).then(|| unsafe { value.assume_init() })
}

pub fn proc_info(pid: u32) -> Option<Proc> {
    let bsd: libc::proc_bsdinfo = pidinfo(pid, libc::PROC_PIDTBSDINFO)?;
    // pbi_name 是 2×MAXCOMLEN，較長；空的才退回 pbi_comm（同 Linux comm 的 15 字截斷）。
    let name = Some(c_chars(&bsd.pbi_name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| c_chars(&bsd.pbi_comm));
    Some(Proc {
        uid: bsd.pbi_uid,
        ppid: bsd.pbi_ppid,
        name,
        start: bsd
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(bsd.pbi_start_tvusec),
    })
}

pub fn all_pids() -> Vec<u32> {
    // SAFETY: 先用空緩衝問數量，再配置多一點空間；回傳值是實際寫入的 pid 數。
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    let cap = usize::try_from(n).unwrap_or(0) + 64;
    let mut buf = vec![0 as libc::pid_t; cap];
    let bytes = libc::c_int::try_from(cap * size_of::<libc::pid_t>()).unwrap_or(libc::c_int::MAX);
    let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast::<c_void>(), bytes) };
    buf.truncate(usize::try_from(got).unwrap_or(0).min(cap));
    buf.into_iter()
        .filter_map(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0)
        .collect()
}

/// 工作目錄；別人的程序或已結束 ⇒ None。
pub fn cwd(pid: u32) -> Option<String> {
    let info: libc::proc_vnodepathinfo = pidinfo(pid, libc::PROC_PIDVNODEPATHINFO)?;
    let flat: Vec<libc::c_char> = info.pvi_cdir.vip_path.iter().flatten().copied().collect();
    Some(c_chars(&flat)).filter(|s| !s.is_empty())
}

/// argv 與環境變數（KERN_PROCARGS2：argc、exec 路徑、argv、environ，皆以 NUL 分隔）。
/// 只有自己的程序讀得到環境；讀不到 ⇒ None。
pub fn args_env(pid: u32) -> Option<(Vec<String>, Vec<u8>)> {
    let mut argmax: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: mib 兩項、輸出是一個 c_int。
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast::<c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; usize::try_from(argmax).ok()?];
    let mut size = buf.len();
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        libc::c_int::try_from(pid).ok()?,
    ];
    // SAFETY: 緩衝區長度 size，核心回寫實際長度。
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast::<c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size < size_of::<libc::c_int>() {
        return None;
    }
    buf.truncate(size);
    let argc = usize::try_from(i32::from_ne_bytes(buf[..4].try_into().ok()?)).ok()?;
    let mut rest = &buf[4..];
    // 跳過 exec 路徑與它後面補齊用的 NUL。
    let skip = rest.iter().position(|b| *b == 0)?;
    rest = &rest[skip..];
    let start = rest.iter().position(|b| *b != 0)?;
    rest = &rest[start..];
    let mut parts = rest.split(|b| *b == 0);
    let argv: Vec<String> = parts
        .by_ref()
        .take(argc)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    let env: Vec<u8> = parts
        .take_while(|s| !s.is_empty())
        .flat_map(|s| s.iter().copied().chain([0]))
        .collect();
    Some((argv, env))
}

/// (使用者＋系統 CPU 時間，單位 10ms＝Linux 100Hz tick；程序開始時間)。
/// 換成 tick 是為了和 Linux 的 `/proc/<pid>/stat` 同單位，`compiling` 只比前後是否增加。
pub fn cpu_sample(pid: u32) -> Option<(u64, u64)> {
    let task: libc::proc_taskinfo = pidinfo(pid, libc::PROC_PIDTASKINFO)?;
    let start = proc_info(pid)?.start;
    let ns = task.pti_total_user.checked_add(task.pti_total_system)?;
    Some((ns / 10_000_000, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_machine() {
        let (load, _swap, avail) = metrics().expect("metrics");
        assert!(load >= 0.0);
        let total = mem_total_mb().expect("memsize");
        assert!(avail <= total && total > 0);
        assert!(cpus().expect("cpus") > 0);
    }

    #[test]
    fn sees_own_process() {
        let me = std::process::id();
        assert!(all_pids().contains(&me));
        let p = proc_info(me).expect("own bsdinfo");
        assert_eq!(p.uid, uid());
        let (argv, env) = args_env(me).expect("own procargs");
        assert!(!argv.is_empty());
        assert!(
            env.windows(5).any(|w| w == b"PATH="),
            "own environment readable"
        );
        assert_eq!(
            cwd(me).as_deref(),
            std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned))
                .as_deref()
        );
        assert!(cpu_sample(me).is_some());
    }
}
