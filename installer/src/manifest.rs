//! 应用清单（AppManifest）—— 运行期安装配置的唯一来源。
//!
//! 设计目标：让 wind-installer 成为「通用安装器生成器」。应用身份、安装行为、
//! 输入法/字体等专属逻辑全部由打包时提供的 `app.toml` 描述，序列化进归档头部，
//! 由运行期读取。**同一个预编译 stub 通过更换 app.toml 即可为不同应用生成安装包，
//! 无需重新编译。**
//!
//! 三类结构：
//! - [`AppManifest`]：进归档、运行期读取的纯配置（无任何文件路径）。
//! - [`ProjectConfig`]：`app.toml` 的完整映射 = 清单 + `[package]` 打包参数。
//! - [`PackageConfig`]：仅打包期使用（压缩、源目录、logo/icon 路径等）。

// ProjectConfig/PackageConfig 及多个序列化/默认值辅助仅 wind-packer 使用，
// 在 installer/uninstaller 二进制中不构造，故模块级允许 dead_code。
#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 运行期清单：序列化为 TOML 文本存入归档头部，安装/卸载器启动时读取。
///
/// 不包含任何宿主机文件路径——logo 字节单独存于归档头部，icon 由 rcedit 写入 PE。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    pub app: AppInfo,
    #[serde(default)]
    pub ui: UiInfo,
    /// 向导主题色。缺省项回退到内置默认。
    #[serde(default)]
    pub theme: ThemeInfo,
    /// 默认路径模板。缺省项回退到内置默认。
    #[serde(default)]
    pub paths: PathsInfo,
    /// 可覆盖的领域相关文案。缺省项回退到内置的中性默认。
    #[serde(default)]
    pub strings: StringsInfo,
    /// 输入法（TSF）注册信息。整段缺省 = 跳过输入法注册。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ime: Option<ImeInfo>,
    /// 需要安装到系统的字体。空 = 跳过字体安装。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub font: Vec<FontInfo>,
    /// 开机自启动。整段缺省 = 不注册自启动。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<AutoStartInfo>,
    /// 快捷方式。空 = 不创建任何快捷方式。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcut: Vec<ShortcutInfo>,
    /// 安装完成后的启动行为。整段缺省 = 不启动任何程序。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<StartupInfo>,
    /// 用户数据目录配置落盘。整段缺省 = 不写——普通应用不需要这个约定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datadir: Option<DataDirInfo>,
    /// 本机数据目录（`%LOCALAPPDATA%\{app.id}`）的卸载清理声明。
    /// 整段缺省 = 卸载完全不碰这个目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localdata: Option<LocalDataInfo>,
}

/// 应用身份与安装行为。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    /// 应用标识：注册表键名、默认安装目录名等。
    pub id: String,
    /// 显示名称（向导标题、Add/Remove Programs 等）。
    pub display_name: String,
    /// 版本号。
    pub version: String,
    /// 发布者。
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    /// 主程序可执行文件名（相对安装目录）。
    pub main_exe: String,
    /// 设置程序可执行文件名（相对安装目录），可空。
    #[serde(default)]
    pub setting_exe: String,
    /// 开始菜单文件夹名，空则回退到 display_name。
    #[serde(default)]
    pub start_menu_folder: String,
    /// 向导窗口标题，空则回退到 "<display_name> 安装向导"。
    #[serde(default)]
    pub window_title: String,
    /// URL 协议名，空则跳过协议注册。
    #[serde(default)]
    pub url_protocol: String,
    /// 用户协议链接，空则不显示。
    #[serde(default)]
    pub agreement_url: String,
    /// 便携模式标记文件名。
    #[serde(default = "default_portable_marker")]
    pub portable_marker: String,
    /// 安装/卸载前需终止的进程名（不含 .exe）。
    #[serde(default)]
    pub process_names: Vec<String>,
    /// 需要 ALL APPLICATION PACKAGES 读取权限的 DLL（相对安装目录）。
    #[serde(default)]
    pub acl_dlls: Vec<String>,
    /// 升级时清理的旧版遗留文件（相对安装目录）。
    #[serde(default)]
    pub legacy_files: Vec<String>,
    /// 升级时清理的旧版遗留目录（相对安装目录）。
    #[serde(default)]
    pub legacy_dirs: Vec<String>,
}

/// UI 尺寸（纯运行期配置）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiInfo {
    #[serde(default = "default_install_win")]
    pub install_win: WinSize,
    #[serde(default = "default_uninstall_win")]
    pub uninstall_win: WinSize,
}

impl Default for UiInfo {
    fn default() -> Self {
        Self {
            install_win: default_install_win(),
            uninstall_win: default_uninstall_win(),
        }
    }
}

/// 窗口尺寸（像素）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WinSize {
    pub w: i32,
    pub h: i32,
}

/// 向导主题色，`"#RRGGBB"` 或 `"RRGGBB"`。留空的项回退到 `ui::theme` 的内置默认。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeInfo {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accent_hover: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accent_pressed: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bg_primary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bg_secondary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_primary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_secondary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text_muted: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub success: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// 警示色：用于「装完了但有事项待处理」（如需重启清理锁定文件）——
    /// 这类结果既不是成功也不是失败，用 error 色会让用户误以为装失败了。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub warning: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub border: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub divider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub track: String,
}

/// 默认路径模板。`{id}` 占位符在运行期替换为 `app.id`，`%VAR%` 由向导展开。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsInfo {
    /// 标准安装默认目录。
    #[serde(default = "default_install_path")]
    pub install: String,
    /// 便携模式默认目录。
    #[serde(default = "default_portable_path")]
    pub portable: String,
    /// 用户数据默认目录。
    #[serde(default = "default_data_path")]
    pub data: String,
}

impl Default for PathsInfo {
    fn default() -> Self {
        Self {
            install: default_install_path(),
            portable: default_portable_path(),
            data: default_data_path(),
        }
    }
}

/// 领域相关文案。留空则回退到中性默认——通用安装器不该在界面上说「词库」「输入法」。
///
/// 只收录会泄漏应用领域的文案；「安装路径」「更改」「立即安装」这类通用 chrome
/// 保持内置，不进清单，避免把清单撑成一张翻译表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StringsInfo {
    /// 数据目录输入框的占位提示。默认「数据文件路径」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data_dir_hint: String,
    /// 安装模式说明。默认「标准安装将程序注册到系统；便捷模式仅解压文件，不修改系统」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode_hint: String,
    /// 协议链接文字。默认「《用户服务协议》」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agreement_text: String,
    /// 卸载页「删除用户数据」勾选项文字。默认「删除用户配置数据」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_data_label: String,
    /// 卸载页「清除缓存」勾选项文字。默认「清除本地缓存」。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cache_label: String,
    /// 删除用户数据二次确认的正文。默认「将永久删除 {path} 下的所有数据，卸载后无法恢复。」
    /// 支持 `{path}` 占位符。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub delete_data_confirm: String,
}

/// 输入法（TSF）注册信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImeInfo {
    /// TSF CLSID，如 "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}"。
    pub clsid: String,
    /// TSF Profile GUID，格式同 `clsid`，取另一个 GUID。
    pub profile_guid: String,
    /// 语言 ID，如 "0804"（简体中文）。
    pub lang_id: String,
    /// 64 位 TSF DLL 文件名（相对安装目录）。
    pub dll_x64: String,
    /// 32 位 TSF DLL 文件名（相对安装目录），可空。
    #[serde(default)]
    pub dll_x86: String,
    /// 安装前是否清扫「指向已消失 DLL 的悬空注册」（本产品旧版/同 CLSID 前身卸载不净的
    /// COM CLSID、CTF TIP、旧 NSIS 的 RunOnce 重注册触发器）。
    ///
    /// 缺省 `false`——通用安装器不擅自改注册表。仅当打包器在 `app.toml` 显式置 `true`
    /// 时才启用。清扫严格按本段 clsid/profile 变体隔离，且只删悬空项，不碰健康注册。
    #[serde(default)]
    pub sweep_residue: bool,
    /// TSF DLL 的系统目录子路径（如 `"IME/MyIme"`），缺省空。
    ///
    /// 空 → 就地注册：对安装目录里的 DLL 跑 regsvr32，`InprocServer32` 指向安装目录。
    /// 非空 → 把 DLL 复制到 `%WINDIR%\System32\<此路径>\`（x86 到 `SysWOW64\<此路径>\`）
    /// 并对**系统副本**注册；`DllRegisterServer` 内的 `GetModuleFileName` 取到系统副本
    /// 路径，`InprocServer32` 自然指向它。
    ///
    /// 何时需要：开启 Trusted Mode 的游戏只放行系统目录下的 in-proc DLL，装在
    /// `Program Files` 的副本连加载都会被拒。该判据同时要求 DLL 已代码签名。
    ///
    /// ⚠️ 置了本项，安装器会一并写 `HKLM\Software\{app_id}\InstallDir`——DLL 搬离安装
    /// 目录后，产品进程只能靠它回指安装目录。
    #[serde(default)]
    pub system_subdir: String,
}

/// 字体安装信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontInfo {
    /// 字体文件名（如 "MyFont.ttf"），同时是写入 %WINDIR%\Fonts 的目标名。
    pub file: String,
    /// 注册表中的字体显示名（如 "My Font (TrueType)"）。
    pub display_name: String,
    /// 字体源路径，相对安装目录（如 "data/fonts/MyFont.ttf"）。
    pub source_rel: String,
}

/// 开机自启动（写 HKCU\...\Run）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoStartInfo {
    /// 段落存在即默认启用；置 false 可保留配置但不生效。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 自启动的可执行文件（相对安装目录），空则回退到 app.main_exe。
    #[serde(default)]
    pub exe: String,
    /// 附加命令行参数。
    #[serde(default)]
    pub args: String,
}

impl AutoStartInfo {
    /// 自启动目标文件名，空则回退到 main_exe。
    pub fn exe_or<'a>(&'a self, main_exe: &'a str) -> &'a str {
        non_empty(&self.exe).unwrap_or(main_exe)
    }
}

/// 快捷方式落地位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutLocation {
    /// 全局开始菜单的 app.start_menu_folder 子目录。
    #[default]
    StartMenu,
    /// 公共桌面。
    Desktop,
}

/// 快捷方式定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutInfo {
    /// 目标可执行文件（相对安装目录）。
    pub target: String,
    /// 快捷方式显示名（不含 .lnk），空则回退到 target 去扩展名后的文件名。
    #[serde(default)]
    pub name: String,
    /// 落地位置。
    #[serde(default)]
    pub location: ShortcutLocation,
    /// 快捷方式描述（悬停提示），空则回退到 name。
    #[serde(default)]
    pub description: String,
}

impl ShortcutInfo {
    /// 显示名，空则取 target 的文件名去扩展名。
    pub fn effective_name(&self) -> &str {
        non_empty(&self.name).unwrap_or_else(|| {
            let file = self
                .target
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&self.target);
            file.strip_suffix(".exe").unwrap_or(file)
        })
    }

    /// 描述，空则回退到显示名。
    pub fn effective_description(&self) -> &str {
        non_empty(&self.description).unwrap_or_else(|| self.effective_name())
    }
}

/// 展开清单占位符：`{main_exe}` `{setting_exe}` `{display_name}` `{app_id}`。
///
/// 让一份能力段配置对多个应用变体（如 dev/release，其 exe 名与显示名不同）通用：
/// 快捷方式写 `target = "{setting_exe}"`，安装器运行期按 `[app]` 字段替换，配置无需
/// 为每个变体各写一份。`{setting_exe}` 在未配置设置程序时展开为空串——调用方据此跳过。
pub fn expand_placeholders(
    s: &str,
    main_exe: &str,
    setting_exe: &str,
    display_name: &str,
    app_id: &str,
) -> String {
    s.replace("{main_exe}", main_exe)
        .replace("{setting_exe}", setting_exe)
        .replace("{display_name}", display_name)
        .replace("{app_id}", app_id)
}

/// 用户数据目录配置：首次安装时把用户选定的数据目录写到
/// `%LOCALAPPDATA%\{app.id}\{conf_file}`，供主程序启动时读取。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataDirInfo {
    /// 配置文件名。
    #[serde(default = "default_conf_file")]
    pub conf_file: String,
    /// 数据目录的「确属本产品」标志物（文件或子目录名，相对数据目录）。
    ///
    /// 卸载删除用户数据前，若目录非空则要求至少命中其一，否则跳过删除。缺省为空
    /// = 不做内容检查（通用应用不必声明）。存在的意义：`conf_file` 是用户可编辑的
    /// 明文路径，而下游动作是 `remove_dir_all` + 桌面备份，判错一次不可逆。
    #[serde(default)]
    pub markers: Vec<String>,
}

/// 本机数据目录（`%LOCALAPPDATA%\{app.id}`）里由**应用运行期**创建的东西。
///
/// 安装器往这个目录里只写过一样东西——`[datadir].conf_file`，它有回执、由卸载撤销。
/// 主程序在同一层写的缓存、日志、本机状态不是安装动作的产物，回执里没有它们，安装器
/// 也无从猜测它们叫什么——那是应用的内部布局，只能由产品清单声明。
///
/// **整段缺省 = 卸载完全不碰这个目录**（保守默认，见 AGENTS.md 规则 4）。
///
/// 分成两组而不是一个「都删」开关，是因为卸载向导有两个**语义不同**的勾选，而这个目录
/// 里两类东西都有：`cache_dirs` 是可重建产物，`state_files` 是删了就没的用户侧状态。
/// 合成一组的后果是用户只勾了「清除本地缓存」却把状态一并丢了——而他明确没有勾另一个。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalDataInfo {
    /// 可重建产物（缓存、日志……），相对 `%LOCALAPPDATA%\{app.id}`。
    /// 跟随卸载向导的「清除本地缓存」勾选。
    #[serde(default)]
    pub cache_dirs: Vec<String>,
    /// 本机状态与标记文件，相对同一目录。跟随「删除用户数据」勾选。
    #[serde(default)]
    pub state_files: Vec<String>,
    /// 上面两组清完后，若 `%LOCALAPPDATA%\{app.id}` 已空则连目录一并移除。
    ///
    /// 落地用**非递归**的 `remove_dir`：「空才删」由 OS 在删除的同一步里保证。先
    /// `read_dir` 判一遍再删的话，中间那个窗口足够应用写回一个文件，删掉的就成了
    /// 「刚才是空的」而不是「现在是空的」。
    #[serde(default)]
    pub remove_dir_when_empty: bool,
}

/// 一个 `[localdata]` 条目被拒的原因。分开报——它们要求作者做的修改完全不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalDataRejection {
    /// 归一化后指向作用域根本身（`""` / `"."` / `"..."` / `". ."`）。
    ResolvesToRoot,
    /// 含 `..` 分量：能走出 `%LOCALAPPDATA%\{app.id}`。
    EscapesLocalDir,
    /// 任一分量带盘符或路径前缀：`join` 会**整个替换**掉前缀。
    NotRelative,
    /// 分量尾部带点或空格：Win32 会剥掉它们，写的是 `logs .`、删的却是 `logs`。
    TrailingDotOrSpace,
}

/// 归一化 **并** 校验，一次做完：`Ok` 给出真正要拿去 `join` 的相对路径，`Err` 给出理由。
///
/// **校验必须作用在最终要用的那个值上。** 上一版把守卫打在原始串上、却拿逐分量拼出来的
/// `PathBuf` 去 `join`——两者不是同一个东西，于是整串「看着」是相对路径、拼出来却不是：
///
/// - `logs/C:x`、`./C:x`、` C:x`、`cache/C:evil` 全部放行。`PathBuf::push` 遇到**任何**带
///   前缀的分量都会截断整个缓冲区（不只第一个），于是拼出来是 `C:x`；`root.join("C:x")`
///   的结果仍是 `C:x`，解析为 C: 盘**当前目录**下的 `x`，删除落到作用域根之外。旧守卫查的
///   是 `raw.chars().nth(1)`，即**整串**的第 2 个字符，非首分量一个都拦不住。
/// - `...`、`. .`、`.. .` 全部放行。Win32 会剥掉分量尾部的点与空格，剥完为空即指回父目录
///   自己——实测 `remove_dir_all(root.join("..."))` 返回 `Ok(())` 并把整棵树删光。于是用户
///   只勾了「清除本地缓存」，另一组的用户状态跟着一起没了。
///
/// 两者是同一个成因：**被校验的串与被使用的串不是同一个**。所以这里不再有两条路径——
/// 分量切完就地逐个判，通过的那份分量直接拼成结果，中间没有第二次转换的余地。
fn resolve_local_data_entry(entry: &str) -> Result<PathBuf, LocalDataRejection> {
    let raw = entry.replace('\\', "/");
    if raw.starts_with('/') {
        return Err(LocalDataRejection::NotRelative);
    }
    let parts = local_data_parts(&raw);
    if parts.is_empty() {
        return Err(LocalDataRejection::ResolvesToRoot);
    }
    for p in &parts {
        if p == ".." {
            return Err(LocalDataRejection::EscapesLocalDir);
        }
        // 盘符、ADS（`a:b`）、任何带 `:` 的写法：`push` 会拿它当新的根。
        if p.contains(':') {
            return Err(LocalDataRejection::NotRelative);
        }
        // 剥掉 Win32 会忽略的尾部点与空格后什么都不剩 = 这个分量指的是父目录自己。
        let trimmed = p.trim_end_matches(['.', ' ']);
        if trimmed.is_empty() {
            return Err(LocalDataRejection::ResolvesToRoot);
        }
        if trimmed != p.as_str() {
            return Err(LocalDataRejection::TrailingDotOrSpace);
        }
        // 兜底：不是「单独一个普通名字」的一律不放行（前缀、根、`.`、`..`）。上面几条
        // 各覆盖一种已知形态，这条挡的是它们都没想到的那些。
        let mut comps = Path::new(p.as_str()).components();
        if !matches!(comps.next(), Some(Component::Normal(_))) || comps.next().is_some() {
            return Err(LocalDataRejection::NotRelative);
        }
    }
    Ok(parts.iter().collect())
}

/// 判定一个 `[localdata]` 条目该不该被拒。`None` 即安全。
pub fn classify_local_data_entry(entry: &str) -> Option<LocalDataRejection> {
    resolve_local_data_entry(entry).err()
}

/// 归一化条目为路径分量。大小写原样保留——Windows 路径不敏感，但拼出来是要给人看的。
fn local_data_parts(raw: &str) -> Vec<String> {
    raw.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_string)
        .collect()
}

/// 安全条目 → 归一化后的相对路径；不安全 → `None`。运行期清理只要这个是非判断，
/// 要给人看的理由走 [`classify_local_data_entry`]。**两者出自同一次解析**，不会漂移。
pub fn safe_local_data_rel(entry: &str) -> Option<PathBuf> {
    resolve_local_data_entry(entry).ok()
}

/// 安装完成后的启动行为。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupInfo {
    /// 安装完成后立即以 DETACHED_PROCESS 启动目标程序。
    #[serde(default)]
    pub prestart: bool,
    /// 启动的可执行文件（相对安装目录），空则回退到 app.main_exe。
    #[serde(default)]
    pub exe: String,
}

impl StartupInfo {
    /// 预启动目标文件名，空则回退到 main_exe。
    pub fn exe_or<'a>(&'a self, main_exe: &'a str) -> &'a str {
        non_empty(&self.exe).unwrap_or(main_exe)
    }
}

// ── app.toml 完整映射（打包期）─────────────────────────────────────────────

/// `app.toml` 的完整结构 = 运行期清单 + 打包参数。
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    #[serde(flatten)]
    pub manifest: AppManifest,
    pub package: PackageConfig,
}

/// 仅打包期使用的参数，不进归档。
#[derive(Debug, Clone, Deserialize)]
pub struct PackageConfig {
    /// 压缩算法 "zstd" / "lzma"。
    #[serde(default = "default_compression")]
    pub compression: String,
    /// 源目录（待打包的应用构建产物）。
    pub source_dir: String,
    /// 输出文件名（不含扩展名与版本号）。
    #[serde(default)]
    pub output_name: String,
    /// 输出目录。
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    /// UI 中显示的 logo 图片路径（PNG），字节会被嵌入归档头部。
    #[serde(default)]
    pub logo: String,
    /// 安装器 EXE 图标路径（.ico），由 rcedit 写入 PE 资源。
    #[serde(default)]
    pub icon: String,
    /// PE 资源版本信息。
    #[serde(default)]
    pub version_info: Option<VersionInfoConfig>,
}

/// 版本信息，用于写入 PE 文件的 Version Info 资源。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionInfoConfig {
    #[serde(default)]
    pub company_name: Option<String>,
    #[serde(default)]
    pub file_description: Option<String>,
    #[serde(default)]
    pub file_version: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub product_version: Option<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    #[serde(default)]
    pub original_filename: Option<String>,
}

// ── 序列化 / 反序列化 ──────────────────────────────────────────────────────

impl AppManifest {
    /// 序列化为 TOML 文本字节（存入归档头部）。
    pub fn to_toml_bytes(&self) -> Result<Vec<u8>, String> {
        toml::to_string(self)
            .map(String::into_bytes)
            .map_err(|e| format!("Failed to serialize manifest: {}", e))
    }

    /// 从 TOML 文本字节反序列化（从归档头部 / 安装目录 .manifest 读取）。
    ///
    /// **刻意不调用 [`AppManifest::validate`]**：这是运行期入口，清单已经嵌在
    /// 用户手里那个安装包里了。在这里报错等于让一个已发布的安装/卸载器彻底起不来，
    /// 而用户既改不了那份清单、也就无从自救。校验放在打包期（[`ProjectConfig::from_toml_str`]）
    /// 拦；对已发布的旧包，由 `installer::legacy` 在执行前逐条跳过受保护目录兜底。
    pub fn from_toml_bytes(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| format!("Manifest is not valid UTF-8: {}", e))?;
        toml::from_str(text).map_err(|e| format!("Failed to parse manifest: {}", e))
    }

    // ── 带回退的访问器（消除调用点的空值判断）─────────────────────────────

    /// 开始菜单文件夹名，空则回退到 display_name。
    pub fn start_menu_folder(&self) -> &str {
        non_empty(&self.app.start_menu_folder).unwrap_or(&self.app.display_name)
    }
}

impl ProjectConfig {
    /// 从 app.toml 文本解析，并做打包期校验。
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let cfg: Self =
            toml::from_str(text).map_err(|e| format!("Failed to parse app.toml: {}", e))?;
        cfg.manifest.validate()?;
        Ok(cfg)
    }
}

// ── 内容目录：`legacy_dirs` 的禁区 ───────────────────────────────────────────

/// **内容目录**的常用名——不得出现在 `legacy_dirs` 里。
///
/// `legacy_dirs` 的语义是「上一版装出来、新版**不再装**的目录」，`CleanupLegacy`
/// 在解包**之前**把它们整个 `remove_dir_all`。而内容目录不是遗留物：
///
/// - `data` —— 应用随包分发的资源目录，几乎每个带资源的应用都有一个。它由解包
///   **正向覆盖**维护，从来不需要先被删掉。
/// - `data_custom` —— 与 `data` 同级的定制内容层，惯例是**不在安装包里**：由部署方
///   放置、应用只读。正因为它不在包里，删掉之后**没有任何东西会把它装回来**——这是
///   它区别于 `data` 的特有理由，也是这条禁令的分量所在。
///
/// 为什么校验必须存在：`legacy_dirs` 是整个安装流程里**唯一**会在升级时删除安装目录
/// 下内容的入口（`PrepareArchive` 只 `create_dir_all`，`ExtractFiles` 只正向遍历包内
/// 条目，都不反查差集）。往这个列表里写一个内容目录名，就等于让升级流程删用户数据；
/// 而这种故障只在真正部署了那一层的机器上复现得出来，作者的开发机上永远是好的。
/// **`data` 与 `data_custom` 的深度规则不同，这不是疏漏，别顺手统一掉。**
///
/// 差别直接来自上面两条理由的差别：
///
/// - `data` 随包分发，解包会把内容正向覆盖回来。所以「删掉一个新版不再分发的子目录」
///   恰恰是 `legacy_dirs` 的**正当用法**——`data/old_themes` 该放行。一并拦下等于把这个
///   功能真正有用的场景削掉。故 [`DepthRule::ExactOnly`]：只拦 `data` 本身。
/// - `data_custom` 的危险性来自「不在包里、删了不会回来」，而这条对它的**子目录一字不差
///   地成立**——`data_custom/themes` 删掉同样没有任何东西会装回来。故
///   [`DepthRule::AnyDepth`]：首段命中即拦，任何深度。而且
///   `legacy_dirs = ["data_custom/themes"]` 比写 `data_custom` 现实得多——作者想清掉旧版
///   留下的某个子目录，很自然就这么写。
pub const RESERVED_LEGACY_DIRS: &[(&str, DepthRule)] = &[
    ("data", DepthRule::ExactOnly),
    ("data_custom", DepthRule::AnyDepth),
];

/// 受保护内容目录的匹配深度。见 [`RESERVED_LEGACY_DIRS`] 里为什么两者不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthRule {
    /// 只拦目录本身；它的子目录放行。
    ExactOnly,
    /// 首段命中即拦，任何深度。
    AnyDepth,
}

/// 一个 `legacy_dirs` 条目被拒的原因。
///
/// **两类原因必须分开报，不能合成一句。** 用户写 `data/..` 时，「不接受含 `..` 的路径」
/// 才是有用的信息；报成「`data` 是受保护目录」会让人以为换个名字就行，于是改写成
/// `plugins/..`——那同样是删光整个安装目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyDirRejection {
    /// 含 `..` 分量：能走出安装目录，与具体名字无关。
    EscapesInstallDir,
    /// 命中受保护的内容目录（携带命中的那个名字）。
    ContentDir(&'static str),
}

/// 判定一个 `legacy_dirs` 条目该不该被拒，以及为什么。
///
/// **归一化按路径分量做，不按前后缀剥。** 「剥前后缀」那种写法（`trim_matches('/')` →
/// `trim_start_matches("./")` → `trim()`）的毛病是剥的顺序固定、每种剥法只做一遍，于是
/// `.//data`（剥一次 `./` 得 `/data` 就停手）、`data/.`（首尾都不是 `/`、也不以 `./`
/// 开头）全都漏出去——而这两个写法 `install_dir.join(rel)` 解析得到，`remove_dir_all`
/// **真能把 `data` 整个删掉**（实测）。切成分量后，空段与 `.` 段一并丢弃，三类写法一次覆盖。
///
/// **含 `..` 分量的条目一律拒绝，且优先于内容目录判定。** 它能走出自己的子树：`data/..`
/// 就是 `install_dir` 本身，拿去 `remove_dir_all` 等于升级时把整个安装目录（含
/// `data_custom`）删光——实测确认。守卫无从对这种条目讲道理，只能拒绝；`legacy_dirs`
/// 本就该是「安装目录下的相对目录名」。
pub fn classify_legacy_dir(name: &str) -> Option<LegacyDirRejection> {
    let parts: Vec<String> = name
        .replace('\\', "/")
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_ascii_lowercase())
        .collect();

    if parts.iter().any(|p| p == "..") {
        return Some(LegacyDirRejection::EscapesInstallDir);
    }
    let head = parts.first()?;
    let has_deeper = parts.len() > 1;
    RESERVED_LEGACY_DIRS
        .iter()
        .find(|(reserved, rule)| reserved == head && (!has_deeper || *rule == DepthRule::AnyDepth))
        .map(|(reserved, _)| LegacyDirRejection::ContentDir(reserved))
}

/// `name` 是否不得作为 `legacy_dirs` 条目执行。运行期兜底（`installer::legacy`）只需
/// 这个是非判断；要给人看的理由走 [`classify_legacy_dir`]。
pub fn is_reserved_legacy_dir(name: &str) -> bool {
    classify_legacy_dir(name).is_some()
}

impl AppManifest {
    /// 打包期校验。失败即拒绝出包——比打出一个升级时会删用户数据的安装包好。
    pub fn validate(&self) -> Result<(), String> {
        if let Some((bad, why)) = self
            .app
            .legacy_dirs
            .iter()
            .find_map(|d| classify_legacy_dir(d).map(|why| (d, why)))
        {
            return Err(match why {
                LegacyDirRejection::EscapesInstallDir => format!(
                    "legacy_dirs 不接受含 `..` 的路径 {:?}：`install_dir.join(它)` 会走出\
                     安装目录（`data/..` 解析出来就是安装目录本身），随后的 remove_dir_all \
                     就删到了 legacy_dirs 管不着的地方——实测 `data/..` 会把整个安装目录\
                     删光。这与写的是哪个名字无关，`plugins/..` 一样。legacy_dirs 的条目\
                     必须是安装目录下的相对目录名",
                    bad
                ),
                LegacyDirRejection::ContentDir(_) => format!(
                    "legacy_dirs 不得包含 {:?}：它指向安装目录下的内容层，不是旧版遗留物。\
                     `data` 由解包正向覆盖、无需先删（但它的子目录允许清理）；\
                     `data_custom` 惯例上不在安装包里，删掉之后没有任何东西会把它装回来，\
                     故连子目录一并保护（规则见 manifest::RESERVED_LEGACY_DIRS）",
                    bad
                ),
            });
        }

        // [localdata] 的两组条目：作用域根是 %LOCALAPPDATA%\{app.id}，下游动作是
        // remove_dir_all。校验放在打包期，是因为这里错的代价不是「装不上」而是「卸载时
        // 删到别处」——那要等真有人卸载才看得见，且看见时已经删完了。
        if let Some(local) = &self.localdata {
            for (field, entries) in [
                ("cache_dirs", &local.cache_dirs),
                ("state_files", &local.state_files),
            ] {
                let Some((bad, why)) = entries
                    .iter()
                    .find_map(|e| classify_local_data_entry(e).map(|why| (e, why)))
                else {
                    continue;
                };
                return Err(match why {
                    LocalDataRejection::ResolvesToRoot => format!(
                        "[localdata].{} 的条目 {:?} 归一化后指向作用域根本身：\
                         `%LOCALAPPDATA%\\{{app.id}}.join(它)` 就是那个目录，随后的 \
                         remove_dir_all 会把整个本机数据目录删光——包括另一组条目\
                         对应的、用户这次并没有勾选删除的东西。请写出具体的子路径",
                        field, bad
                    ),
                    LocalDataRejection::EscapesLocalDir => format!(
                        "[localdata].{} 不接受含 `..` 的路径 {:?}：它能走出 \
                         `%LOCALAPPDATA%\\{{app.id}}`，删到的是这一段管不着的地方。\
                         这与写的是哪个名字无关，`logs/..` 与 `cache/..` 一样",
                        field, bad
                    ),
                    LocalDataRejection::NotRelative => format!(
                        "[localdata].{} 的条目 {:?} 里有分量带盘符或路径前缀：\
                         `PathBuf::push` 遇到这种分量会**截断整个缓冲区**——不只第一个，\
                         `logs/C:x` 拼出来也是 `C:x`，`join` 之后落到 C: 盘当前目录，\
                         而不是 `%LOCALAPPDATA%\\{{app.id}}` 之下。注意 `C:x` 的 \
                         `is_absolute()` 为 false，光看 Path 判不出来",
                        field, bad
                    ),
                    LocalDataRejection::TrailingDotOrSpace => format!(
                        "[localdata].{} 的条目 {:?} 有分量以点或空格结尾：Win32 解析路径时\
                         会把它们剥掉，于是写的是 `logs .`、删的却是 `logs`。这类条目删得掉\
                         东西、但删的不是你写的那个，故一律拒绝而不是默默归一化",
                        field, bad
                    ),
                });
            }
        }
        Ok(())
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn default_true() -> bool {
    true
}
fn default_conf_file() -> String {
    "datadir.conf".to_string()
}
fn default_install_path() -> String {
    r"%ProgramFiles%\{id}".to_string()
}
fn default_portable_path() -> String {
    r"%USERPROFILE%\{id}".to_string()
}
fn default_data_path() -> String {
    r"%APPDATA%\{id}".to_string()
}
fn default_portable_marker() -> String {
    "portable_mode".to_string()
}
fn default_install_win() -> WinSize {
    WinSize { w: 520, h: 490 }
}
fn default_uninstall_win() -> WinSize {
    WinSize { w: 480, h: 440 }
}
fn default_compression() -> String {
    "zstd".to_string()
}
fn default_output_dir() -> String {
    "./dist".to_string()
}
