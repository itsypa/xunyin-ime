//! 极简文件日志：用于诊断 TSF 激活/按键/编辑会话链路。
//!
//! 写入用户数据目录 `%APPDATA%\com.xunyin.ime\logs\tsf.log`（首选）与
//! `%TEMP%\xunyin-tsf.log`（兜底），多进程（ctfmon/各应用）共用、行级追加，
//! 带时间戳与进程 id。**超过 2 MB 自动轮转为 `tsf.log.1`**，避免无限增长。
//!
//! 不再写 DLL 同目录那份：安装布局下 DLL 位于 Program Files（无写权限），
//! 该路径注定失败还平白多一次系统调用。

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// 单个日志文件上限；超过即轮转为 `<名字>.1`（旧的 .1 被覆盖）。
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// 每写这么多行检查一次大小（避免每行都 stat）。
const ROTATE_CHECK_EVERY: u64 = 256;
static WRITE_COUNT: AtomicU64 = AtomicU64::new(0);

/// 候选日志路径（全部写入，任一成功即可读到）。
fn log_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    // 统一走共享 crate 的数据目录（安装时可能被指到别处，见 datadir.conf）
    let logs = xunyin_rime::user_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&logs);
    v.push(logs.join("tsf.log"));
    v.push(std::env::temp_dir().join("xunyin-tsf.log"));
    v
}

/// 超过上限则把日志轮转为 `<名字>.1`（覆盖旧的 .1），实现有界占用。
fn rotate_if_needed(path: &PathBuf) {
    let too_big = fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false);
    if !too_big {
        return;
    }
    let old = path.with_extension("log.1");
    let _ = fs::remove_file(&old);
    let _ = fs::rename(path, &old);
}

/// 写一行日志；失败静默忽略（日志不能影响输入功能）。
pub fn log(msg: &str) {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    let nth = WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
    let check_rotate = nth % ROTATE_CHECK_EVERY == 0;
    for path in log_paths() {
        if check_rotate {
            rotate_if_needed(&path);
        }
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
