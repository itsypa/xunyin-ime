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
    std::env::var("PROCESSOR_ARCHITECTURE")
        .map(|arch| arch == "AMD64" || arch == "IA64")
        .unwrap_or(false)
}
