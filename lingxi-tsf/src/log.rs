//! 极简文件日志：用于诊断 TSF 激活/按键/编辑会话链路。
//!
//! 日志同时写入多个可靠路径（DLL 旁 / 用户数据目录 / %TEMP%），
//! 多进程（ctfmon/各应用）共用，行级追加，带时间戳与进程 id。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// 定位本 DLL 所在目录（GetModuleFileNameW(None) 会拿到宿主进程路径）。
fn module_dir() -> Option<PathBuf> {
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    unsafe {
        let mut hmod: windows::Win32::Foundation::HMODULE = core::mem::zeroed();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let addr = (module_dir as fn() -> Option<PathBuf>) as usize;
        if GetModuleHandleExW(flags, windows_core::PCWSTR(addr as *const u16), &mut hmod).is_err() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmod), &mut buf);
        if len == 0 {
            return None;
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
            .parent()
            .map(|d| d.to_path_buf())
    }
}

/// 候选日志路径（全部写入，任一成功即可读到）。
fn log_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = module_dir() {
        v.push(dir.join("tsf.log"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let logs = PathBuf::from(appdata).join("com.lingxi.ime").join("logs");
        let _ = std::fs::create_dir_all(&logs);
        v.push(logs.join("tsf.log"));
    }
    v.push(std::env::temp_dir().join("lingxi-tsf.log"));
    v
}

/// 写一行日志；失败静默忽略（日志不能影响输入功能）。
pub fn log(msg: &str) {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    for path in log_paths() {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "[{now} pid={pid}] {msg}");
        }
    }
}

#[macro_export]
macro_rules! tsf_log {
    ($($arg:tt)*) => {
        $crate::log::log(&format!($($arg)*))
    };
}
