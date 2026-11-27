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
    parse_procargs(&buf)
}

/// KERN_PROCARGS2 的版面：argc（4 位元組）、exec 路徑、補齊用的 NUL、argv、environ。
/// 不信任長度：任何一段短少都回 None，不切片越界（核心回的長度不足時 gate 不能崩）。
fn parse_procargs(buf: &[u8]) -> Option<(Vec<String>, Vec<u8>)> {
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let mut rest = buf.get(4..)?;
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

/// 一個程序的一次 CPU／RSS 取樣。
pub struct TaskSample {
    /// 使用者＋系統 CPU 時間，單位 10ms tick。
    pub ticks: u64,
    /// 常駐記憶體，單位 kB。
    pub rss_kb: u64,
    /// 程序開始時間（秒＋微秒），同 `Proc::start`。
    pub start: u64,
}

/// mach absolute time 換成奈秒：一秒有 `hw.tbfrequency` 格。
/// 核心給的程序 CPU 時間是 mach absolute time，不是標頭檔寫的奈秒：Apple Silicon 的 timebase 是
/// 24MHz（一格 125/3 ns），Intel 才是 1GHz。直接當奈秒，Apple Silicon 上會少算 41.7 倍。
fn abs_time_ns(ticks: u64) -> Option<u64> {
    let hz = sysctl_raw::<u64>(c"hw.tbfrequency").filter(|hz| *hz > 0)?;
    u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(hz)).ok()
}

/// 程序的 CPU 時間與 RSS：CPU 取 `PROC_PIDTASKINFO` 的 user＋system，換成 10ms tick（＝Linux 100Hz，
/// 和 `/proc/<pid>/stat` 同單位）；RSS 取 `pti_resident_size` 換成 kB，同 `/proc/<pid>/status` 的
/// VmRSS。開始時間那一個 flavor 沒有，另取 `proc_info`。
/// 別人的程序或已結束的（還沒被收的殭屍也算）⇒ None，同 `proc_info`。
pub fn task_sample(pid: u32) -> Option<TaskSample> {
    let task: libc::proc_taskinfo = pidinfo(pid, libc::PROC_PIDTASKINFO)?;
    let start = proc_info(pid)?.start;
    let ns = abs_time_ns(task.pti_total_user.checked_add(task.pti_total_system)?)?;
    Some(TaskSample {
        ticks: ns / 10_000_000,
        rss_kb: task.pti_resident_size / 1024,
        start,
    })
}

/// (使用者＋系統 CPU 時間，單位 10ms＝Linux 100Hz tick；程序開始時間)，即 `task_sample` 的 CPU 部分。
/// 換成 tick 是為了和 Linux 的 `/proc/<pid>/stat` 同單位，`compiling` 只比前後是否增加。
pub fn cpu_sample(pid: u32) -> Option<(u64, u64)> {
    let sample = task_sample(pid)?;
    Some((sample.ticks, sample.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procargs_short_or_malformed_buffers_are_none_not_panics() {
        for buf in [
            &b""[..],
            &b"\x01"[..],
            &b"\x01\x00\x00"[..],
            &b"\x01\x00\x00\x00"[..],
            &b"\x01\x00\x00\x00/bin/x"[..],
        ] {
            assert!(parse_procargs(buf).is_none(), "{buf:?}");
        }
    }

    #[test]
    fn procargs_layout() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/node\0\0\0node\0app.mjs\0PATH=/bin\0HOME=/h\0\0junk");
        let (argv, env) = parse_procargs(&buf).expect("parsed");
        assert_eq!(argv, ["node", "app.mjs"]);
        assert_eq!(env, b"PATH=/bin\0HOME=/h\0");
    }

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

    // `task_sample` 與 `cpu_sample` 同源：自己的程序 RSS 大於 0，開始時間與 CPU tick 單位一致。
    // tick 在兩次取樣之間只增不減，所以夾在不等式裡比，不比相等。
    #[test]
    fn task_sample_of_own_process_agrees_with_cpu_sample() {
        let me = std::process::id();
        let (before, start) = cpu_sample(me).expect("own cpu_sample");
        let sample = task_sample(me).expect("own task_sample");
        let (after, _) = cpu_sample(me).expect("own cpu_sample again");
        assert!(sample.rss_kb > 0, "rss_kb {}", sample.rss_kb);
        assert_eq!(sample.start, start);
        assert!(
            before <= sample.ticks && sample.ticks <= after,
            "ticks {before} {} {after}",
            sample.ticks
        );
    }

    /// 這一條執行緒已經用掉的 CPU 時間（奈秒）。
    fn thread_cpu_ns() -> u128 {
        let mut ts = MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: 緩衝區大小就是 timespec；回傳 0 才成功，也才讀它。
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, ts.as_mut_ptr()) };
        let ts = (rc == 0)
            .then(|| unsafe { ts.assume_init() })
            .expect("thread cpu clock");
        (ts.tv_sec as i128 * 1_000_000_000 + ts.tv_nsec as i128) as u128
    }

    // tick 真的是 10ms：自己的程序用滿 0.2 秒 CPU 之後，tick 數要跟 getrusage(RUSAGE_SELF) 對得上。
    // 上面那條只比 `task_sample` 跟 `cpu_sample`，同源，單位錯了兩個一起錯；這條跟核心的 rusage 比。
    // 核心給的是 mach absolute time（Apple Silicon 一格 125/3 ns），當奈秒直接除只會剩 1/41.7。
    #[test]
    fn task_sample_counts_ten_millisecond_ticks() {
        // 轉到這條執行緒自己用滿 0.2 秒 CPU 為止，機器再忙也燒得到。
        std::thread::spawn(|| {
            let until = thread_cpu_ns() + 200_000_000;
            let mut x = 0u64;
            while thread_cpu_ns() < until {
                x = x.wrapping_add(1);
            }
            x
        })
        .join()
        .unwrap();
        let me = std::process::id();
        let before = task_sample(me).expect("own task_sample").ticks;
        let mut own = MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: rusage 是整數結構；回傳 0 才成功，也才讀它。
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, own.as_mut_ptr()) };
        let own = (rc == 0)
            .then(|| unsafe { own.assume_init() })
            .expect("getrusage");
        let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        let own_ticks = (secs(own.ru_utime) + secs(own.ru_stime)) * 100.0;
        let (after, _) = cpu_sample(me).expect("own cpu_sample");
        assert!(before >= 19, "0.2 s of CPU: {before} ticks");
        assert!(
            own_ticks >= before as f64 - 1.0 && own_ticks <= after as f64 + 1.0,
            "getrusage {own_ticks:.1} vs task_sample {before}..{after} ticks"
        );
    }
}
