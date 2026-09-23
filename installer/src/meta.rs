//! 运行期安装配置访问层。
//!
//! 历史上这里是来自 Cargo.toml 的**编译期常量**，导致换一个应用就要重新编译。
//! 现在配置来自打包时嵌入归档头部的 [`AppManifest`]（见 `manifest.rs`），由
//! [`init`] 在启动时一次性载入全局，本模块提供只读访问器。**同一个预编译 stub
//! 通过更换 app.toml 即可为不同应用生成安装包。**
//!
//! 载入时机（两者都从自身追加的归档头部读取）：
//! - 安装器：归档 = 压缩块 + 文件清单 + 运行期清单；
//! - 卸载器：安装时被追加了「仅清单 overlay」（见 `archive::append_manifest_overlay`），
//!   故同样用 `ArchiveReader::open_current_exe` 自读，安装目录无散落文件。

use std::sync::OnceLock;

use crate::manifest::{AppManifest, LocalDataInfo};

static MANIFEST: OnceLock<AppManifest> = OnceLock::new();
static LOGO: OnceLock<Vec<u8>> = OnceLock::new();

/// 编译期默认 logo——清单未提供 logo 时的兜底，确保 UI 始终有图。
const DEFAULT_LOGO: &[u8] = include_bytes!("../assets/logo.png");

/// 设置运行期 logo 字节（bootstrap 时调用）。
pub fn set_logo(bytes: Vec<u8>) {
    let _ = LOGO.set(bytes);
}

/// 运行期 UI logo 字节；未载入或为空时回退到编译期默认 logo。
pub fn logo() -> &'static [u8] {
    match LOGO.get() {
        Some(v) if !v.is_empty() => v,
        _ => DEFAULT_LOGO,
    }
}

/// 载入清单（仅首次生效）。必须在任何访问器调用前完成。
pub fn init(manifest: AppManifest) {
    let _ = MANIFEST.set(manifest);
}

/// 从自身追加的归档头部载入清单与 logo。安装器与卸载器通用。
///
/// 自删除临时副本不含清单 overlay，但其流程不访问清单，故不应调用本函数。
pub fn bootstrap() -> Result<(), String> {
    let reader = crate::archive::ArchiveReader::open_current_exe()
        .map_err(|e| format!("无法打开自身归档: {}", e))?;
    let bytes = reader.manifest_bytes();
    if bytes.is_empty() {
        return Err("自身归档头部不含运行期清单".into());
    }
    init(AppManifest::from_toml_bytes(bytes)?);
    set_logo(reader.logo_bytes().to_vec());
    Ok(())
}

/// 获取全局清单。未初始化即 panic——属于编程错误（启动时必须先 init）。
pub fn manifest() -> &'static AppManifest {
    MANIFEST
        .get()
        .expect("manifest 未初始化：启动时必须先调用 meta::init")
}

// ── 字符串访问器（'static，因清单存于全局 OnceLock）─────────────────────────

pub fn app_id() -> &'static str {
    &manifest().app.id
}
pub fn app_display_name() -> &'static str {
    &manifest().app.display_name
}

/// 清单没初始化时返回 `None`，不 panic。
///
/// 给**出错收场的那些路径**用：它们可能跑在 bootstrap 之前，而收场代码自己再崩一次，
/// 换来的是一次无提示 abort（release profile 是 `panic = "abort"` + GUI 子系统，
/// 连崩溃信息都没处显示），比它本来要报告的那个问题更难查。
pub fn try_app_display_name() -> Option<&'static str> {
    MANIFEST.get().map(|m| m.app.display_name.as_str())
}
pub fn app_version() -> &'static str {
    &manifest().app.version
}
pub fn app_publisher() -> &'static str {
    &manifest().app.publisher
}
pub fn main_exe() -> &'static str {
    &manifest().app.main_exe
}
pub fn setting_exe() -> &'static str {
    &manifest().app.setting_exe
}
pub fn url_protocol() -> &'static str {
    &manifest().app.url_protocol
}
pub fn agreement_url() -> &'static str {
    &manifest().app.agreement_url
}
pub fn portable_marker() -> &'static str {
    &manifest().app.portable_marker
}
pub fn start_menu_folder() -> &'static str {
    manifest().start_menu_folder()
}

/// 向导窗口标题：清单留空则回退到 "<display_name> 安装向导"。
pub fn window_title() -> String {
    let t = manifest().app.window_title.trim();
    if t.is_empty() {
        format!("{} 安装向导", app_display_name())
    } else {
        t.to_string()
    }
}

// ── 列表访问器 ──────────────────────────────────────────────────────────────

pub fn process_names() -> &'static [String] {
    &manifest().app.process_names
}
pub fn acl_dlls() -> &'static [String] {
    &manifest().app.acl_dlls
}
pub fn legacy_files() -> &'static [String] {
    &manifest().app.legacy_files
}
pub fn legacy_dirs() -> &'static [String] {
    &manifest().app.legacy_dirs
}

/// 本机数据目录（`%LOCALAPPDATA%\{app.id}`）的卸载清理声明。
///
/// `None` = 清单没声明 `[localdata]` 段 = 卸载完全不碰那个目录。调用方据此决定
/// 是否连「清除本地缓存」这个勾选都不显示——勾了什么也不做的复选框比没有更糟。
pub fn localdata() -> Option<&'static LocalDataInfo> {
    manifest().localdata.as_ref()
}

// ── 默认路径模板（`{id}` 替换为 app.id；`%VAR%` 由向导展开）──────────────────

fn expand_id(template: &str) -> String {
    template.replace("{id}", app_id())
}

pub fn default_install_path() -> String {
    expand_id(&manifest().paths.install)
}
pub fn default_portable_path() -> String {
    expand_id(&manifest().paths.portable)
}
pub fn default_data_path() -> String {
    expand_id(&manifest().paths.data)
}

// ── 可覆盖文案（留空即回退到中性默认）────────────────────────────────────────

/// 清单值优先，留空则用 `default`。
fn string_or(value: &'static str, default: &'static str) -> &'static str {
    let t = value.trim();
    if t.is_empty() {
        default
    } else {
        t
    }
}

pub fn s_data_dir_hint() -> &'static str {
    string_or(&manifest().strings.data_dir_hint, "数据文件路径")
}
pub fn s_mode_hint() -> &'static str {
    string_or(
        &manifest().strings.mode_hint,
        "标准安装将程序注册到系统；便捷模式仅解压文件，不修改系统",
    )
}
pub fn s_agreement_text() -> &'static str {
    string_or(&manifest().strings.agreement_text, "《用户服务协议》")
}
pub fn s_user_data_label() -> &'static str {
    string_or(&manifest().strings.user_data_label, "删除用户配置数据")
}
pub fn s_cache_label() -> &'static str {
    string_or(&manifest().strings.cache_label, "清除本地缓存")
}

/// 删除用户数据的二次确认正文；`{path}` 替换为实际路径。
pub fn s_delete_data_confirm(path: &str) -> String {
    string_or(
        &manifest().strings.delete_data_confirm,
        "将永久删除 {path} 下的所有数据，卸载后无法恢复。\n\n确定要勾选删除吗？",
    )
    .replace("{path}", path)
}

// ── 窗口尺寸 ────────────────────────────────────────────────────────────────

pub fn install_win() -> (i32, i32) {
    let s = manifest().ui.install_win;
    (s.w, s.h)
}
pub fn uninstall_win() -> (i32, i32) {
    let s = manifest().ui.uninstall_win;
    (s.w, s.h)
}
