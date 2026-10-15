#![allow(dead_code)]

use std::path::PathBuf;

/// 获取 Program Files 路径
pub fn get_program_files() -> PathBuf {
    std::env::var("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Program Files"))
}

/// 获取 APPDATA 路径
pub fn get_app_data() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
            PathBuf::from(user_profile).join("AppData\\Roaming")
        })
}

/// 获取 LOCALAPPDATA 路径
pub fn get_local_app_data() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
            PathBuf::from(user_profile).join("AppData\\Local")
        })
}

/// 获取临时目录路径
pub fn get_temp_dir() -> PathBuf {
    std::env::temp_dir()
}

/// 获取桌面路径
pub fn get_desktop() -> PathBuf {
    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
    PathBuf::from(user_profile).join("Desktop")
}

/// 获取开始菜单程序路径
pub fn get_start_menu_programs() -> PathBuf {
    let program_data =
        std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    PathBuf::from(program_data)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
}

/// 获取 Windows 字体目录
pub fn get_fonts_dir() -> PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(windir).join("Fonts")
}

/// 检查是否为 64 位系统
pub fn is_64bit_system() -> bool {
    // 本地修补：32 位进程跑在 64 位 Windows 上时，`PROCESSOR_ARCHITECTURE` 是 `x86`，
    // 真实架构放在 `PROCESSOR_ARCHITEW6432`。只看前者会让 32 位安装器把自己误判成
    // 「32 位系统」而拒绝安装（上游只发 x64 stub，没暴露这个问题）。
    let arch = std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default();
    let wow = std::env::var("PROCESSOR_ARCHITEW6432").unwrap_or_default();
    matches!(arch.as_str(), "AMD64" | "IA64") || matches!(wow.as_str(), "AMD64" | "IA64")
}

/// 展开路径模板里的 `%VAR%` 占位符（清单 `[paths]` 用的就是这种写法）。
///
/// 本地修补：从向导模块搬到这里，供界面与安装配置共用一份实现——两处各写一份时，
/// 界面显示的默认路径与实际安装路径会悄悄分叉。
pub fn expand_path(template: &str) -> PathBuf {
    let mut result = template.to_string();
    for (var, fallback) in [
        ("ProgramFiles", r"C:\Program Files"),
        ("APPDATA", r"C:\Users\Default\AppData\Roaming"),
        ("LOCALAPPDATA", r"C:\Users\Default\AppData\Local"),
        ("USERPROFILE", r"C:\Users\Default"),
    ] {
        let token = format!("%{var}%");
        if result.contains(&token) {
            let val = std::env::var(var).unwrap_or_else(|_| fallback.to_string());
            result = result.replace(&token, &val);
        }
    }
    PathBuf::from(result)
}
