use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// 安装 / 卸载过程日志，写入 `%TEMP%\{app.id}-{install,uninstall}.log`
/// （文件名跟随清单，不同应用互不覆盖）。
///
/// 安装与卸载分成两个文件：出事时要看的往往只是其中一次，混在一起还得先找分界。
pub struct RunLogger {
    file: Option<File>,
    pub path: PathBuf,
}

impl Default for RunLogger {
    fn default() -> Self {
        Self::install()
    }
}

impl RunLogger {
    pub fn install() -> Self {
        Self::open("install", "=== 安装开始 ===")
    }

    /// 卸载过程日志。
    ///
    /// 卸载此前**全程不落盘**：两个 Reporter 的 `warn` 都是 `eprintln!`，而两个二进制
    /// 都是 `windows_subsystem = "windows"`、没有控制台 —— 反注册 COM 失败、文件删不掉
    /// 之类的话全写进了一个不存在的句柄，用户只看到「卸载成功」。
    pub fn uninstall() -> Self {
        Self::open("uninstall", "=== 卸载开始 ===")
    }

    fn open(kind: &str, banner: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{}-{}.log", crate::meta::app_id(), kind));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let mut logger = Self { file, path };
        logger.log(banner);
        logger
    }

    /// 写入一行日志（立即 flush，确保崩溃时不丢日志）
    pub fn log(&mut self, msg: &str) {
        let timestamp = utc_hms();
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "[{}] {}", timestamp, msg);
            let _ = f.flush();
        }
    }

    pub fn log_error(&mut self, msg: &str) {
        self.log(&format!("ERROR: {}", msg));
    }
}

/// 启动决策日志 `%TEMP%\wind_installer_args.log`。
///
/// 与上面的安装过程日志是两回事：安装器是 GUI 子系统程序，没有 stdout，而「怎么被
/// 调起来的」「为什么一上来就退了」这类事发生在安装流程开始之前，那时 RunLogger
/// 还没建。出问题时这个文件往往是唯一的线索。
pub fn startup_log_path() -> PathBuf {
    std::env::temp_dir().join("wind_installer_args.log")
}

/// 往启动决策日志追加一行（失败即忽略：诊断日志不该反过来影响流程）。
pub fn append_startup_line(line: &str) {
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(startup_log_path())
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

/// 从 SystemTime 提取 UTC HH:MM:SS
fn utc_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}
