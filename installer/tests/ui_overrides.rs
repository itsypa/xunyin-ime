//! 清单声明 [theme]/[paths]/[strings] 时的覆盖行为。
//!
//! 见 ui_defaults.rs 顶部关于「为何拆成两个文件」与「文件名为何避开 install」的说明。

#![cfg(windows)]

use wind_installer::manifest::AppManifest;
use wind_installer::{meta, ui::theme};

/// 逐项覆盖，并混入一个非法颜色（accent_hover）验证静默回退。
///
/// 用 `r##"…"##` 而非 `r#"…"#`：TOML 里的 `"#112233"` 含 `"#` 序列，会提前终止
/// 单井号原始字符串。
const OVERRIDDEN: &str = r##"
[app]
id           = "Demo"
display_name = "Demo App"
version      = "1.0.0"
publisher    = "Demo Inc"
main_exe     = "demo.exe"

[theme]
accent       = "#112233"
text_primary = "AABBCC"
accent_hover = "不是颜色"
warning      = "#C0FFEE"

[paths]
install  = 'D:\Apps\{id}'
portable = 'E:\Portable\{id}\bin'

[strings]
data_dir_hint       = "素材库、配置路径"
user_data_label     = "删除用户素材和配置数据"
delete_data_confirm = "将永久删除 {path} 下的所有素材，无法恢复。"
"##;

fn init() {
    meta::init(AppManifest::from_toml_bytes(OVERRIDDEN.as_bytes()).expect("解析清单失败"));
}

#[test]
fn declared_strings_win_over_defaults() {
    init();
    assert_eq!(meta::s_data_dir_hint(), "素材库、配置路径");
    assert_eq!(meta::s_user_data_label(), "删除用户素材和配置数据");
}

#[test]
fn undeclared_strings_still_fall_back() {
    init();
    // [strings] 段存在但未声明 cache_label / mode_hint → 仍用中性默认
    assert_eq!(meta::s_cache_label(), "清除本地缓存");
    assert!(!meta::s_mode_hint().contains("输入法"));
}

#[test]
fn declared_confirm_text_substitutes_path() {
    init();
    let text = meta::s_delete_data_confirm(r"D:\Data");
    assert_eq!(text, r"将永久删除 D:\Data 下的所有素材，无法恢复。");
}

#[test]
fn path_templates_are_overridable_and_expand_id() {
    init();
    assert_eq!(meta::default_install_path(), r"D:\Apps\Demo");
    assert_eq!(meta::default_portable_path(), r"E:\Portable\Demo\bin");
    // [paths] 段存在但未声明 data → 回退内置模板
    assert_eq!(meta::default_data_path(), r"%APPDATA%\Demo");
}

#[test]
fn theme_accepts_both_hash_and_bare_hex() {
    init();
    assert_eq!(theme::accent(), 0x112233);
    assert_eq!(theme::text_primary(), 0xAABBCC);
    assert_eq!(theme::warning(), 0xC0FFEE);
}

#[test]
fn malformed_color_falls_back_instead_of_panicking() {
    init();
    // 一个拼错的颜色不该让整个安装器起不来
    assert_eq!(theme::accent_hover(), 0x6BA3FF);
}
