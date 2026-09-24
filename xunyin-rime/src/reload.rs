//! 重载 / 重新部署请求：让小狼毫式的「重新部署即可生效」成立。
//!
//! 设置中心、部署脚本、状态栏菜单都只往标记文件里写一条请求；
//! 每个宿主进程（记事本 / Edge / ChatGPT …）里的 TSF DLL 轮询这个文件，
//! 发现新请求就重建自己的 librime 引擎（finalize → initialize → 应用共享配置），
//! 所以装好新方案或改完配置后**不需要重启任何应用**。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 请求内容里的动作名：重载引擎与配置（索引由发起方负责重建，见 README「热重载」）。
pub const REQUEST_RELOAD: &str = "reload";

/// 标记文件路径（与配置同目录，随用户数据目录迁移）。
pub fn marker_path() -> PathBuf {
    crate::config::user_data_dir().join("reload.request")
}

/// 写一条请求。内容带毫秒时间戳，保证每次内容都不同（客户端按内容变化触发）。
pub fn mark(action: &str) -> bool {
    let dir = crate::config::user_data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp = dir.join("reload.request.tmp");
    if std::fs::write(&tmp, format!("{action} {ms}\n")).is_err() {
        return false;
    }
    std::fs::rename(&tmp, marker_path()).is_ok()
}

/// 读取当前请求原文（不存在或读失败返回 None）。
pub fn read() -> Option<String> {
    std::fs::read_to_string(marker_path()).ok()
}

/// 请求里的动作名（`reload` / `deploy`），无请求返回 None。
pub fn action_of(raw: &str) -> Option<&str> {
    let first = raw.split_whitespace().next()?;
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}
