//! 迅音输入法 —— librime 引擎共享封装。
//!
//! 供两类消费者复用：
//!   * xunyin-ime(原生设置中心)
//!   * xunyin-tsf(系统级 TSF 文本服务 DLL)
//!
//! 包含 rime.dll 的 FFI 绑定、RimeEngine 高级封装(会话/上下文/状态快照)、
//! 以及数据目录解析工具。

pub mod assoc;
pub mod blocklist;
pub mod config;
pub mod keycode;
pub mod lunar;
mod lunar_table;
pub mod phrases;
pub mod quick_input;
pub mod preedit;
pub mod reload;
pub mod rime_engine;
pub mod rime_ffi;
pub mod schema_list;
pub mod svc_proto;

pub use config::{XunyinConfig, load_config, save_config, user_data_dir};
pub use rime_engine::{Candidate, RimeContextSnapshot, RimeEngine, RimeStatusSnapshot};
pub use rime_ffi::{
    kRimeModifierAlt, kRimeModifierCaps, kRimeModifierControl, kRimeModifierRelease,
    kRimeModifierShift,
};

use std::path::PathBuf;

/// 解析 Rime 数据目录：
///   * shared：随包发布的共享数据(词典/schema/opencc),位于资源目录下 `rime/data`
///   * user：用户数据(用户词库/日志/sync),位于 `%APPDATA%\<app_identifier>`
///
/// `resource_dir` 传设置中心资源目录或 DLL 所在目录。
pub fn resolve_data_dirs(
    resource_dir: &std::path::Path,
    app_identifier: &str,
) -> (PathBuf, PathBuf) {
    let shared = resource_dir.join("rime").join("data");
    let user = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| resource_dir.join("rime").join("user"))
        .join(app_identifier);
    (shared, user)
}

// rename-touch
