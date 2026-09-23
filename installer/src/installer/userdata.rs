use std::path::{Path, PathBuf};

use crate::meta;

/// 数据目录配置文件的绝对路径 `%LOCALAPPDATA%\{app.id}\{conf_file}`（纯计算，不落盘）。
///
/// 回执要记这个路径——无论本次是否真的写了它。升级时不重写（保留用户既有配置），
/// 但文件仍属本产品，卸载时该删；不记就会永久残留，并污染下次全新安装的数据目录解析。
pub fn datadir_conf_path(conf_file: &str) -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
        let up = std::env::var("USERPROFILE").unwrap_or_default();
        format!(r"{}\AppData\Local", up)
    });
    PathBuf::from(local_app_data)
        .join(meta::app_id())
        .join(conf_file)
}

/// 读取已落盘的数据目录配置，返回其中记录的路径。
///
/// 「谁存在谁生效」——与主程序读端（`wind-config` 的 `custom_userdata_dir`）同一语义：
/// 清单未声明 `[datadir]` 段、文件不存在、内容为空，均返回 `None`，调用方回退默认位置。
///
/// 安装向导与卸载器都必须走这里：向导据此显示「本机实际生效的数据目录」，卸载器据此
/// 决定删哪个目录。两处各写一份读取实现一旦漂移，就是「界面显示 A、卸载删了 B」。
pub fn read_datadir_conf() -> Option<PathBuf> {
    let conf_file = &meta::manifest().datadir.as_ref()?.conf_file;
    let content = std::fs::read_to_string(datadir_conf_path(conf_file)).ok()?;
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    Some(PathBuf::from(content))
}

/// 将用户数据目录写入配置文件，返回写入的绝对路径。
/// 仅在首次安装时调用；升级时跳过以保留旧配置。
pub fn write_datadir_conf(data_dir: &Path, conf_file: &str) -> Result<PathBuf, String> {
    let path = datadir_conf_path(conf_file);
    let conf_dir = path.parent().ok_or("配置路径无父目录")?;

    std::fs::create_dir_all(conf_dir).map_err(|e| format!("Failed to create conf dir: {}", e))?;
    std::fs::write(&path, data_dir.to_string_lossy().as_bytes())
        .map_err(|e| format!("Failed to write {}: {}", conf_file, e))?;

    Ok(path)
}
