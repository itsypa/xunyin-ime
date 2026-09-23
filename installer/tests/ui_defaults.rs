//! 清单未声明 [theme]/[paths]/[strings] 时的内置默认。
//!
//! 核心断言：**默认文案不含任何应用领域词汇**——一个普通应用装出来的界面，
//! 不该出现「输入法」「词库」。这是 UI 去专用化的验收标准。
//!
//! 与 ui_overrides.rs 分成两个文件是必须的：`meta::init` 基于 `OnceLock`，
//! 一个进程只能载入一份清单，而 Cargo 的每个集成测试文件是独立进程。
//!
//! 文件名不含 "install"/"setup"/"update"/"patch"：命中 Windows UAC 安装器检测
//! 启发式的测试二进制无法在普通权限下启动（os error 740）。

#![cfg(windows)]

use wind_installer::manifest::AppManifest;
use wind_installer::{meta, ui::theme};

/// 只声明身份，不声明任何 UI 配置。
const MINIMAL: &str = r#"
[app]
id           = "Demo"
display_name = "Demo App"
version      = "1.0.0"
publisher    = "Demo Inc"
main_exe     = "demo.exe"
"#;

fn init() {
    meta::init(AppManifest::from_toml_bytes(MINIMAL.as_bytes()).expect("解析清单失败"));
}

#[test]
fn defaults_are_domain_neutral() {
    init();

    for s in [
        meta::s_data_dir_hint(),
        meta::s_mode_hint(),
        meta::s_user_data_label(),
        meta::s_cache_label(),
    ] {
        assert!(!s.contains("输入法"), "默认文案泄漏了应用领域: {}", s);
        assert!(!s.contains("词库"), "默认文案泄漏了应用领域: {}", s);
    }

    assert_eq!(meta::s_data_dir_hint(), "数据文件路径");
    assert_eq!(meta::s_user_data_label(), "删除用户配置数据");
    assert_eq!(meta::s_cache_label(), "清除本地缓存");
}

#[test]
fn delete_confirm_substitutes_path_placeholder() {
    init();
    let text = meta::s_delete_data_confirm(r"%APPDATA%\Demo");
    assert!(
        text.contains(r"%APPDATA%\Demo"),
        "未替换 {{path}}: {}",
        text
    );
    assert!(!text.contains("{path}"), "占位符残留: {}", text);
    assert!(!text.contains("词库"));
}

#[test]
fn default_path_templates_expand_app_id() {
    init();
    assert_eq!(meta::default_install_path(), r"%ProgramFiles%\Demo");
    assert_eq!(meta::default_portable_path(), r"%USERPROFILE%\Demo");
    assert_eq!(meta::default_data_path(), r"%APPDATA%\Demo");
}

#[test]
fn theme_falls_back_to_builtin_palette() {
    init();
    assert_eq!(theme::accent(), 0x4C8BF5);
    assert_eq!(theme::text_primary(), 0x191919);
    assert_eq!(theme::error(), 0xFA5151);
    // 「装完了但需重启清理」用的警示色，必须与 error 区分开——
    // 同色会让用户把「有残留」误读成「装失败」而去重装。
    assert_eq!(theme::warning(), 0xFA9D3B);
    assert_ne!(theme::warning(), theme::error());
}
