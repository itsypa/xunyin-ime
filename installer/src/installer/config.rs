use std::path::PathBuf;

use crate::meta;

/// 安装配置
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InstallConfig {
    /// 应用名称
    pub app_name: String,
    /// 应用版本
    pub app_version: String,
    /// 安装目录
    pub install_dir: PathBuf,
    /// 数据目录（用户配置等应用自有数据）
    pub data_dir: PathBuf,
    /// 是否使用自定义数据目录
    pub use_custom_data_dir: bool,
    /// 自定义数据目录路径
    pub custom_data_dir: Option<PathBuf>,
    /// 开始菜单文件夹名
    pub start_menu_folder: String,
    /// 发布者
    pub publisher: String,
}

impl Default for InstallConfig {
    fn default() -> Self {
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
        let app_data = std::env::var("APPDATA").unwrap_or_else(|_| {
            let mut p = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
            p.push("AppData\\Roaming");
            p.to_string_lossy().to_string()
        });
        let _local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            let mut p = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
            p.push("AppData\\Local");
            p.to_string_lossy().to_string()
        });

        Self {
            app_name: meta::app_display_name().to_string(),
            app_version: meta::app_version().to_string(),
            install_dir: PathBuf::from(program_files).join(meta::app_id()),
            data_dir: PathBuf::from(app_data).join(meta::app_id()),
            use_custom_data_dir: false,
            custom_data_dir: None,
            start_menu_folder: meta::start_menu_folder().to_string(),
            publisher: meta::app_publisher().to_string(),
        }
    }
}

#[allow(dead_code)]
impl InstallConfig {
    /// 获取实际数据目录
    pub fn effective_data_dir(&self) -> &PathBuf {
        if self.use_custom_data_dir {
            if let Some(ref custom) = self.custom_data_dir {
                return custom;
            }
        }
        &self.data_dir
    }

    /// 验证数据目录是否有效
    pub fn validate_data_dir(&self, dir: &PathBuf) -> Result<(), String> {
        // 不能为空
        if dir.as_os_str().is_empty() {
            return Err("数据目录路径不能为空".into());
        }

        // 必须是绝对路径
        let dir_str = dir.to_string_lossy();
        if !dir_str.contains(':') {
            return Err("必须是绝对路径（如 D:\\WindData）".into());
        }

        // 不能是安装目录
        if dir == &self.install_dir {
            return Err("不能使用应用安装目录作为数据目录".into());
        }

        // 不能是安装目录的 data 子目录
        if dir == &self.install_dir.join("data") {
            return Err("不能使用应用安装目录的 data 目录".into());
        }

        // 不能是 Windows 系统目录
        let windir = std::env::var("WINDIR").unwrap_or_default();
        if dir_str.starts_with(&windir) {
            return Err("不能使用 Windows 系统目录".into());
        }

        // 不能在 Program Files 下
        let program_files = std::env::var("ProgramFiles").unwrap_or_default();
        let program_files_x86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
        if dir_str.starts_with(&program_files) || dir_str.starts_with(&program_files_x86) {
            return Err("不能使用系统程序目录".into());
        }

        Ok(())
    }
}
