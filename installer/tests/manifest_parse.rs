//! AppManifest / ProjectConfig 解析与 roundtrip 测试。

use wind_installer::manifest::{AppManifest, ProjectConfig};

const SAMPLE: &str = r#"
[app]
id              = "DemoApp"
display_name    = "示例应用"
version         = "0.1.0"
publisher       = "Demo Inc."
main_exe        = "demo_app.exe"
setting_exe     = "demo_settings.exe"
start_menu_folder = "示例应用"
window_title    = "示例应用 安装向导"
url_protocol    = "demoapp"
agreement_url   = "https://example.com/eula"
backup_dir      = "DemoApp_Backup"
process_names   = ["demo_app", "demo_helper"]
acl_dlls        = ["demo_tsf.dll", "demo_tsf_x86.dll"]
legacy_files    = ["demo_old.dll"]

[ui]
install_win   = { w = 520, h = 490 }
uninstall_win = { w = 480, h = 440 }

[ime]
clsid        = "{A1B2C3D4-E5F6-4789-A0B1-C2D3E4F5A6B7}"
profile_guid = "{A1B2C3D4-E5F6-4789-A0B1-C2D3E4F5A6B8}"
lang_id      = "0804"
dll_x64      = "demo_tsf.dll"
dll_x86      = "demo_tsf_x86.dll"

[[font]]
file         = "DemoFont.ttf"
display_name = "Demo Font (TrueType)"
source_rel   = "data/fonts/DemoFont.ttf"

[package]
compression = "lzma"
source_dir  = "../DemoApp/build"
output_name = "DemoApp-Setup"
output_dir  = "./dist"
logo        = "assets/logo.png"
icon        = "assets/installer.ico"
"#;

#[test]
fn project_config_parses_all_sections() {
    let cfg = ProjectConfig::from_toml_str(SAMPLE).expect("解析 app.toml 失败");

    assert_eq!(cfg.manifest.app.id, "DemoApp");
    assert_eq!(cfg.manifest.app.display_name, "示例应用");
    assert_eq!(cfg.manifest.app.process_names, ["demo_app", "demo_helper"]);
    assert_eq!(
        cfg.manifest.app.acl_dlls,
        ["demo_tsf.dll", "demo_tsf_x86.dll"]
    );
    assert_eq!(cfg.manifest.app.legacy_files, ["demo_old.dll"]);
    assert!(cfg.manifest.app.legacy_dirs.is_empty());

    assert_eq!(cfg.manifest.ui.install_win.w, 520);
    assert_eq!(cfg.manifest.ui.uninstall_win.h, 440);

    let ime = cfg.manifest.ime.as_ref().expect("应有 ime 段");
    assert_eq!(ime.lang_id, "0804");
    assert_eq!(ime.dll_x64, "demo_tsf.dll");

    assert_eq!(cfg.manifest.font.len(), 1);
    assert_eq!(cfg.manifest.font[0].file, "DemoFont.ttf");
    assert_eq!(cfg.manifest.font[0].display_name, "Demo Font (TrueType)");

    assert_eq!(cfg.package.compression, "lzma");
    assert_eq!(cfg.package.source_dir, "../DemoApp/build");
    assert_eq!(cfg.package.logo, "assets/logo.png");
    assert_eq!(cfg.package.icon, "assets/installer.ico");
}

/// 仓库自带的 app.toml 必须始终能被当前结构体解析——防止配置与代码漂移。
#[test]
fn repo_app_toml_parses_with_all_capability_sections() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/app.toml");
    let text = std::fs::read_to_string(path).expect("读取仓库 app.toml 失败");
    let cfg = ProjectConfig::from_toml_str(&text).expect("解析仓库 app.toml 失败");

    let autostart = cfg.manifest.autostart.as_ref().expect("应有 autostart 段");
    assert!(autostart.enabled);
    assert_eq!(autostart.exe_or("fallback.exe"), "demo_app.exe");

    assert_eq!(cfg.manifest.shortcut.len(), 2);
    assert_eq!(cfg.manifest.shortcut[0].effective_name(), "示例应用 设置");

    assert!(
        cfg.manifest
            .startup
            .as_ref()
            .expect("应有 startup 段")
            .prestart
    );
    assert_eq!(
        cfg.manifest
            .datadir
            .as_ref()
            .expect("应有 datadir 段")
            .conf_file,
        "datadir.conf"
    );

    // UI 段：仓库自带清单显式覆盖了领域相关文案，内置默认则保持中性
    assert_eq!(cfg.manifest.theme.accent, "#4C8BF5");
    assert_eq!(cfg.manifest.paths.install, r"%ProgramFiles%\{id}");
    assert_eq!(cfg.manifest.strings.data_dir_hint, "项目文件、配置路径");
}

/// 占位符使一份快捷方式配置对 dev/release 变体通用：同一段 config，按各变体的
/// [app] 字段展开出不同的 target/name，打包脚本无需为每个变体各维护一份配置。
#[test]
fn shortcut_placeholders_expand_per_variant() {
    use wind_installer::manifest::expand_placeholders;

    // release 变体
    assert_eq!(
        expand_placeholders(
            "{setting_exe}",
            "demo_app.exe",
            "demo_settings.exe",
            "示例应用",
            "DemoApp"
        ),
        "demo_settings.exe"
    );
    assert_eq!(
        expand_placeholders(
            "{display_name} 设置",
            "demo_app.exe",
            "demo_settings.exe",
            "示例应用",
            "DemoApp"
        ),
        "示例应用 设置"
    );

    // dev 变体：同一份 config 文本，展开出 dev 的 exe 名与显示名
    assert_eq!(
        expand_placeholders(
            "{setting_exe}",
            "demo_app_dev.exe",
            "demo_settings_dev.exe",
            "示例应用 (开发版)",
            "DemoAppDev"
        ),
        "demo_settings_dev.exe"
    );
    assert_eq!(
        expand_placeholders(
            "卸载 {display_name}",
            "demo_app_dev.exe",
            "demo_settings_dev.exe",
            "示例应用 (开发版)",
            "DemoAppDev"
        ),
        "卸载 示例应用 (开发版)"
    );
}

/// 未配置设置程序时 {setting_exe} 展开为空——create_shortcuts 据此跳过该快捷方式，
/// 无需在配置里为「有/无设置程序」各写一份。
#[test]
fn setting_exe_placeholder_empty_when_unset() {
    use wind_installer::manifest::expand_placeholders;
    let expanded = expand_placeholders("{setting_exe}", "app.exe", "", "App", "App");
    assert!(expanded.trim().is_empty());
}

/// 无占位符的字面 target 原样保留。
#[test]
fn literal_target_passes_through_unchanged() {
    use wind_installer::manifest::expand_placeholders;
    assert_eq!(
        expand_placeholders("uninstall.exe", "app.exe", "set.exe", "App", "App"),
        "uninstall.exe"
    );
}

/// 能力段的空值回退：name 取 target 文件名，exe 回退 main_exe。
#[test]
fn capability_fallbacks_resolve_from_target_and_main_exe() {
    let toml = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"

[autostart]

[[shortcut]]
target = "bin/tool.exe"

[startup]
prestart = true
"#;
    let m = AppManifest::from_toml_bytes(toml.as_bytes()).expect("解析失败");

    let autostart = m.autostart.as_ref().unwrap();
    assert!(autostart.enabled, "段落存在即默认启用");
    assert_eq!(autostart.exe_or(&m.app.main_exe), "app.exe");

    // name 留空 → 取 target 的文件名去扩展名；description 留空 → 回退 name
    assert_eq!(m.shortcut[0].effective_name(), "tool");
    assert_eq!(m.shortcut[0].effective_description(), "tool");

    assert_eq!(
        m.startup.as_ref().unwrap().exe_or(&m.app.main_exe),
        "app.exe"
    );
}

#[test]
fn manifest_toml_byte_roundtrip_preserves_all_fields() {
    let cfg = ProjectConfig::from_toml_str(SAMPLE).unwrap();
    let original = cfg.manifest;

    let bytes = original.to_toml_bytes().expect("序列化失败");
    let parsed = AppManifest::from_toml_bytes(&bytes).expect("反序列化失败");

    assert_eq!(parsed.app.id, original.app.id);
    assert_eq!(parsed.app.display_name, original.app.display_name);
    assert_eq!(parsed.app.process_names, original.app.process_names);
    assert_eq!(parsed.app.url_protocol, original.app.url_protocol);
    assert_eq!(parsed.ui.install_win.w, original.ui.install_win.w);
    assert_eq!(
        parsed.ime.as_ref().unwrap().clsid,
        original.ime.as_ref().unwrap().clsid
    );
    assert_eq!(parsed.font.len(), 1);
    assert_eq!(parsed.font[0].source_rel, original.font[0].source_rel);
}

#[test]
fn defaults_apply_with_minimal_manifest() {
    let minimal = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "myapp.exe"

[package]
source_dir = "./build"
"#;
    let cfg = ProjectConfig::from_toml_str(minimal).expect("最小配置应可解析");

    // 缺省段
    assert!(cfg.manifest.ime.is_none());
    assert!(cfg.manifest.font.is_empty());
    // 带默认值的字段
    assert_eq!(cfg.manifest.app.portable_marker, "portable_mode");
    assert_eq!(cfg.manifest.ui.install_win.w, 520);
    assert_eq!(cfg.package.compression, "zstd");
    assert_eq!(cfg.package.output_dir, "./dist");
    // 回退访问器
    assert_eq!(cfg.manifest.start_menu_folder(), "My App"); // 回退到 display_name
}

#[test]
fn project_config_parses_version_info() {
    let toml_str = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "myapp.exe"

[package]
source_dir = "./build"
[package.version_info]
company_name      = "My Company"
file_description  = "My File Description"
file_version      = "1.0.0.0"
product_name      = "My Product"
product_version   = "1.0.0.0"
copyright         = "Copyright (c) 2026"
original_filename = "myapp.exe"
"#;
    let cfg = ProjectConfig::from_toml_str(toml_str).expect("解析包含 version_info 的配置失败");
    let version_info = cfg
        .package
        .version_info
        .as_ref()
        .expect("应当解析出 version_info");
    assert_eq!(version_info.company_name.as_deref(), Some("My Company"));
    assert_eq!(
        version_info.file_description.as_deref(),
        Some("My File Description")
    );
    assert_eq!(version_info.file_version.as_deref(), Some("1.0.0.0"));
    assert_eq!(version_info.product_name.as_deref(), Some("My Product"));
    assert_eq!(version_info.product_version.as_deref(), Some("1.0.0.0"));
    assert_eq!(
        version_info.copyright.as_deref(),
        Some("Copyright (c) 2026")
    );
    assert_eq!(version_info.original_filename.as_deref(), Some("myapp.exe"));

    // 验证缺省情况
    let toml_str_empty_info = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "myapp.exe"

[package]
source_dir = "./build"
[package.version_info]
"#;
    let cfg_empty =
        ProjectConfig::from_toml_str(toml_str_empty_info).expect("解析空 version_info 失败");
    let info_empty = cfg_empty
        .package
        .version_info
        .as_ref()
        .expect("空 version_info 块也应解析出 Option");
    assert!(info_empty.company_name.is_none());
    assert!(info_empty.file_description.is_none());

    // 验证完全不提供 version_info 时
    let toml_str_no_info = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "myapp.exe"

[package]
source_dir = "./build"
"#;
    let cfg_no_info =
        ProjectConfig::from_toml_str(toml_str_no_info).expect("不包含 version_info 应解析成功");
    assert!(cfg_no_info.package.version_info.is_none());
}

// ── legacy_dirs 不得点名内容目录 ─────────────────────────────────────────────
//
// 为什么校验非落在解析层不可：清单是**产品仓自己维护**的文件（本仓的 app.toml 只是
// 示例），而 `legacy_dirs` 的语义与执行在本仓。清单那侧的测试证明不了「安装器拿这个
// 字段干什么」，本仓的示例清单也管不住别人的产品清单。两者唯一的交汇点是打包器：
// 任何清单都必须经由 `ProjectConfig::from_toml_str`，故校验落在那里，本测试钉住它。

/// 内容目录特殊在哪：它们是安装目录下的**内容层**，不是旧版遗留物。
///
/// - `data` 是应用随包分发的资源目录，由解包正向覆盖维护，从不需要先删；
/// - `data_custom` 惯例上**根本不在安装包里**（由部署方放置、应用只读），删掉之后
///   没有任何东西会把它装回来——这是它区别于 `data` 的特有理由。
///
/// 而 `legacy_dirs` 是整个安装流程里唯一会在升级时删除安装目录下内容的入口
/// （`CleanupLegacy` 在解包前 `remove_dir_all`）。往里写一个内容目录名 = 让升级流程
/// 删用户数据，且这种故障只在真正部署了那一层的机器上复现得出来。
#[test]
fn reserved_content_dirs_rejected_in_legacy_dirs() {
    for bad in ["data", "data_custom"] {
        let toml = format!(
            r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"
legacy_dirs  = ["{bad}"]

[package]
source_dir = "./build"
"#
        );
        let err = ProjectConfig::from_toml_str(&toml)
            .expect_err(&format!("legacy_dirs = [\"{bad}\"] 必须被拒绝"));
        assert!(err.contains(bad), "错误信息应点名是哪一项: {err}");
    }
}

/// 绕过检查靠的是换个写法，不是换个名字。
///
/// `.//data` 与 `data/.` 不是纸面漏洞：`install_dir.join(rel)` 对这两个写法都解析得到，
/// `remove_dir_all` **真能把 `data` 整个删掉**（实测过）。它们曾经漏出去，是因为归一化
/// 在剥前后缀而不是切分量——剥的顺序固定、每种剥法只做一遍。
#[test]
fn reserved_dir_check_is_spelling_insensitive() {
    use wind_installer::manifest::is_reserved_legacy_dir;

    for bad in [
        "data",
        "Data",
        "DATA_CUSTOM",
        "data_custom/",
        "./data_custom",
        r"data_custom\",
        " data ",
        "/data/",
        "data//",
        // 剥前后缀式归一化漏掉的两种写法
        ".//data",
        "data/.",
        // `data_custom` 是 AnyDepth：子目录同样删了装不回来
        "data_custom/sub",
        r"data_custom\themes\dark",
        // `..` 走得出自己的子树：`data/..` 就是 install_dir 本身
        "data/..",
        "..",
        "plugins/../data",
    ] {
        assert!(is_reserved_legacy_dir(bad), "应判定为受保护目录: {bad:?}");
    }
}

/// 但别把正常的遗留目录一起拦下——`legacy_dirs` 本身是有用功能。
///
/// 尤其 `data/sub`：`data` 随包分发、解包会正向覆盖回来，**删一个新版不再分发的子目录
/// 正是 `legacy_dirs` 的正当用法**。这与 `data_custom/sub` 被拦是有意的不对称（两者的
/// 禁令理由不同，见 `RESERVED_LEGACY_DIRS`），不要觉得不一致而「顺手统一」。
#[test]
fn ordinary_legacy_dirs_still_allowed() {
    use wind_installer::manifest::is_reserved_legacy_dir;

    for ok in [
        "plugins",
        "data2",
        "olddata",
        "user_data",
        "data/sub",
        "data/old_themes",
        r"data\legacy\v1",
        "./data/sub",
    ] {
        assert!(!is_reserved_legacy_dir(ok), "不该被拦下: {ok:?}");
    }

    let toml = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"
legacy_dirs  = ["plugins", "old_cache"]

[package]
source_dir = "./build"
"#;
    let cfg = ProjectConfig::from_toml_str(toml).expect("普通 legacy_dirs 应可解析");
    assert_eq!(cfg.manifest.app.legacy_dirs, ["plugins", "old_cache"]);
}

/// 运行期入口刻意不校验：已发布的安装包若带着坏清单，在这里报错等于让它彻底起不来，
/// 用户既改不了那份清单也无从自救。拦截点在打包期，执行前再由 `legacy` 逐条跳过。
#[test]
fn runtime_manifest_load_stays_permissive() {
    let toml = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"
legacy_dirs  = ["data_custom"]
"#;
    let m = AppManifest::from_toml_bytes(toml.as_bytes())
        .expect("运行期载入不该因清单里的坏条目而失败");
    assert_eq!(m.app.legacy_dirs, ["data_custom"]);
    assert!(m.validate().is_err(), "但显式校验仍应判它不合法");
}

/// 含 `..` 的条目要有**自己的**错误文案，不能复用「这是受保护目录」。
///
/// 用户写 `data/..` 时，「不接受含 `..` 的路径」才是有用的信息。报成「`data` 是受保护
/// 目录」会让人以为换个名字就行，于是改写成 `plugins/..`——那同样是把整个安装目录
/// `remove_dir_all` 掉（实测：`data/..` 之后连基准目录本身都没了）。
#[test]
fn escaping_paths_get_their_own_error_message() {
    use wind_installer::manifest::{classify_legacy_dir, LegacyDirRejection};

    // 用 TOML **字面量字符串**（单引号）包 rel：基本字符串里 `data_custom\..\..` 的反斜杠
    // 会被当成转义序列，解析就先失败了，`expect_err` 拿到的是 TOML 报错而非校验报错——
    // 断言会因此变成假绿。本仓 `[paths]` 写 Windows 路径用的也是单引号。
    fn err_for(rel: &str) -> String {
        let toml = format!(
            r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"
legacy_dirs  = ['{rel}']

[package]
source_dir = "./build"
"#
        );
        let err = ProjectConfig::from_toml_str(&toml).expect_err(&format!("{rel:?} 必须被拒绝"));
        assert!(
            !err.starts_with("Failed to parse app.toml"),
            "{rel:?} 死在 TOML 解析上，根本没走到校验: {err}"
        );
        err
    }

    // `..` 与具体名字无关：换个名字照样拒，且理由不变
    for esc in ["data/..", "..", "plugins/../data", r"data_custom\..\.."] {
        assert_eq!(
            classify_legacy_dir(esc),
            Some(LegacyDirRejection::EscapesInstallDir),
            "{esc:?} 应判为「走出安装目录」而不是「内容目录」"
        );
        // `..` 那条文案**一个「内容」字都不许出现**：`plugins/..` 也走这条，而 `plugins`
        // 根本不是内容目录——说它是就是说假话，还会把人引到「换个名字就行」的错路上。
        let e = err_for(esc);
        assert!(
            e.contains("不接受含 `..` 的路径"),
            "{esc:?} 的文案没说清原因: {e}"
        );
        assert!(
            !e.contains("内容"),
            "{esc:?} 的文案说它是内容目录——对 plugins/.. 这就是假话: {e}"
        );
    }

    // 内容目录仍走原来那套文案，两边不串
    for content in ["data", "data_custom/sub"] {
        assert!(matches!(
            classify_legacy_dir(content),
            Some(LegacyDirRejection::ContentDir(_))
        ));
        let e = err_for(content);
        assert!(e.contains("内容层"), "{content:?} 的文案不对: {e}");
        assert!(
            !e.contains("不接受含 `..` 的路径"),
            "{content:?} 串到了 `..` 文案: {e}"
        );
    }
}
// ── [localdata]：%LOCALAPPDATA%\{app.id} 下的卸载清理声明 ─────────────────────
//
// 这一段的下游动作是 remove_dir_all，作用域根是一个**真实存在且装着用户状态**的目录，
// 所以守卫的三条拒绝理由各测各的：它们要求清单作者做的修改完全不同，合成一句会让人
// 以为换个名字就能过（`legacy_dirs` 的同类文案里已经吃过这个亏）。

fn manifest_with_localdata(body: &str) -> Result<AppManifest, String> {
    let toml = format!(
        r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"

[localdata]
{body}
"#
    );
    let m: AppManifest = toml::from_str(&toml).expect("TOML 应能解析");
    m.validate().map(|()| m)
}

/// 整段缺省 = 卸载完全不碰 %LOCALAPPDATA%\{app.id}。这是能力段的统一约定
/// （AGENTS.md 规则 2），也是通用安装器不擅自动系统的保守默认。
#[test]
fn localdata_absent_means_uninstall_leaves_local_dir_alone() {
    let toml = r#"
[app]
id           = "MyApp"
display_name = "My App"
version      = "1.0.0"
publisher    = "Me"
main_exe     = "app.exe"
"#;
    let m: AppManifest = toml::from_str(toml).unwrap();
    assert!(
        m.localdata.is_none(),
        "未声明 [localdata] 时必须是 None——Some(默认值) 会让卸载去删一个没人声明过的目录"
    );
}

/// 两组条目必须各自独立解析。写成一组的清单会解析失败而不是静默把 state_files 当缓存。
#[test]
fn localdata_parses_both_groups_separately() {
    let m = manifest_with_localdata(
        r#"cache_dirs  = ["cache", "logs"]
state_files = ["state.toml", "user_config.seen"]
remove_dir_when_empty = true"#,
    )
    .expect("合法清单不该被拒");
    let local = m.localdata.expect("[localdata] 应被解析出来");
    assert_eq!(local.cache_dirs, ["cache", "logs"]);
    assert_eq!(local.state_files, ["state.toml", "user_config.seen"]);
    assert!(local.remove_dir_when_empty);
}

/// 归一化后为空 = 指向作用域根本身。这是本段特有的致命形态：`root.join("")` 就是
/// %LOCALAPPDATA%\{app.id}，随后 remove_dir_all 会把整个目录删光——包括另一组条目
/// 对应的、用户这次并没有勾选删除的东西。两个勾选的语义在这一步一起作废。
#[test]
fn localdata_entries_resolving_to_root_are_rejected() {
    use wind_installer::manifest::{classify_local_data_entry, LocalDataRejection};

    for bad in ["", ".", "./", " ", ".//./"] {
        assert_eq!(
            classify_local_data_entry(bad),
            Some(LocalDataRejection::ResolvesToRoot),
            "{bad:?} 应被判为指向作用域根"
        );
    }
    let e = manifest_with_localdata(r#"cache_dirs = ["."]"#).expect_err("应被打包期校验拒绝");
    assert!(e.contains("作用域根"), "文案没说清是哪种错: {e}");
    assert!(e.contains("cache_dirs"), "文案没指出是哪一组: {e}");
}

/// `..` 与「绝对路径/盘符」分开报：前者要作者删掉 `..`，后者要他改写成相对路径。
/// 注意 `C:x` 的 `is_absolute()` 为 false，只看 Path 判不出来——`guard_shape` 也专门堵过。
#[test]
fn localdata_escaping_and_absolute_entries_rejected() {
    use wind_installer::manifest::{classify_local_data_entry, LocalDataRejection};

    for bad in ["..", "cache/..", "logs/../..", r"cache\..\.."] {
        assert_eq!(
            classify_local_data_entry(bad),
            Some(LocalDataRejection::EscapesLocalDir),
            "{bad:?} 应被判为走出作用域"
        );
    }
    for bad in ["/etc", "C:x", r"C:\Windows", "/"] {
        assert_eq!(
            classify_local_data_entry(bad),
            Some(LocalDataRejection::NotRelative),
            "{bad:?} 应被判为非相对路径"
        );
    }

    let e = manifest_with_localdata(r#"state_files = ['cache\..']"#).expect_err("应被拒");
    assert!(e.contains("`..`"), "`..` 的文案不对: {e}");
    assert!(
        !e.contains("不是相对路径"),
        "`..` 串到了绝对路径的文案上: {e}"
    );
    assert!(e.contains("state_files"), "文案没指出是哪一组: {e}");
}

/// 合法条目归一化后仍指向作用域根之下的同一个位置——反斜杠、多余的 `./` 与尾分隔符
/// 都是清单里手写路径的常见形态，归一化漏掉任何一种，那一项就会静默删不掉。
#[test]
fn safe_local_data_rel_normalizes_hand_written_paths() {
    use std::path::PathBuf;
    use wind_installer::manifest::safe_local_data_rel;

    for (raw, want) in [
        ("logs", "logs"),
        (".//logs/", "logs"),
        ("./logs/./", "logs"),
        (r"cache\sub", "cache/sub"),
        ("cache/sub", "cache/sub"),
    ] {
        assert_eq!(
            safe_local_data_rel(raw),
            Some(want.split('/').collect::<PathBuf>()),
            "{raw:?} 归一化结果不对"
        );
    }
    // 不安全条目一律 None——运行期兜底只需要这个是非判断。
    for bad in ["", ".", "cache/..", "C:x"] {
        assert!(safe_local_data_rel(bad).is_none(), "{bad:?} 不该放行");
    }
}
/// 回归：**非首分量**带盘符。旧守卫只查整串的第 2 个字符（`raw.chars().nth(1)`），于是
/// `logs/C:x` 这类整串看着是相对路径的写法全部放行，而 `PathBuf::push` 遇到任何带前缀的
/// 分量都会截断整个缓冲区——拼出来是 `C:x`，`root.join("C:x")` 仍是 `C:x`，解析为 C: 盘
/// **当前目录**下的 `x`，`remove_dir_all` 就落到了作用域根之外。实测四种变体全部逃逸。
#[test]
fn drive_prefix_in_any_component_is_rejected() {
    use wind_installer::manifest::{
        classify_local_data_entry, safe_local_data_rel, LocalDataRejection,
    };

    for bad in [
        "logs/C:x",
        "./C:x",
        " C:x",
        "cache/C:evil",
        r"cache\C:x",
        "a:b",
        "C:x",
    ] {
        assert_eq!(
            classify_local_data_entry(bad),
            Some(LocalDataRejection::NotRelative),
            "{bad:?} 应被判为非相对路径"
        );
        // 双保险：真正拿去 join 的那个值必须根本拿不到。
        assert!(safe_local_data_rel(bad).is_none(), "{bad:?} 不该放行");
    }
}

/// 回归：Win32 会剥掉分量尾部的点与空格，剥完为空即指回父目录自己。旧守卫只过滤
/// `trim()` 后恰好等于 `"."` 或空串的分量，于是 `...` / `. .` / `.. .` 全部放行。
/// 实测 `remove_dir_all(root.join("..."))` 返回 `Ok(())` 并把整棵树删光——用户只勾了
/// 「清除本地缓存」，另一组的用户状态跟着一起没了，正是 ResolvesToRoot 文档描述的灾难。
///
/// `...` 是清单作者写得出的手滑（想写「等等」），不是攻击构造。
#[test]
fn dot_and_space_forms_that_resolve_to_root_are_rejected() {
    use wind_installer::manifest::{
        classify_local_data_entry, safe_local_data_rel, LocalDataRejection,
    };

    for bad in ["...", ". .", ".. .", "....", "logs/...", " . "] {
        assert!(
            matches!(
                classify_local_data_entry(bad),
                Some(LocalDataRejection::ResolvesToRoot | LocalDataRejection::EscapesLocalDir)
            ),
            "{bad:?} 应被拒（实测它会让 remove_dir_all 删掉整个作用域根），实得 {:?}",
            classify_local_data_entry(bad)
        );
        assert!(safe_local_data_rel(bad).is_none(), "{bad:?} 不该放行");
    }
}

/// 尾部带点/空格的**普通**分量：`logs .` 在 Win32 下就是 `logs`。删得掉东西，但删的不是
/// 作者写的那个。默默归一化会让清单与实际动作对不上，故单独报一类错而不是悄悄放行。
#[test]
fn trailing_dot_or_space_components_get_their_own_error() {
    use wind_installer::manifest::{classify_local_data_entry, LocalDataRejection};

    // 只有**尾部点号**危险：`str::trim` 已经把分量两端的空格去掉了（TOML 里的行尾空格
    // 几乎都是手滑，且 Win32 同样忽略它），而它不动点号——`logs.` 因此原样活到 join，
    // 再由 Win32 解析成 `logs`。`logs .` 是两者的组合：去掉尾点后还剩一个尾空格。
    for bad in ["logs.", "cache/logs .", "logs ."] {
        assert_eq!(
            classify_local_data_entry(bad),
            Some(LocalDataRejection::TrailingDotOrSpace),
            "{bad:?} 应被判为尾部点/空格"
        );
    }
    let e = manifest_with_localdata(r#"cache_dirs = ["logs ."]"#).expect_err("应被拒");
    assert!(e.contains("点或空格结尾"), "文案不对: {e}");

    // 但**分量之间**的空格与点是正常文件名的一部分，不能误伤。
    // 分量**内部**的空格与点是正常文件名的一部分；两端的空格被 trim 掉后同样合法。
    for ok in [
        "my logs",
        "state.toml",
        "a.b.c",
        "cache/my logs",
        "logs ",
        "state.toml ",
    ] {
        assert_eq!(classify_local_data_entry(ok), None, "{ok:?} 不该被拒");
    }
}
/// 钉住守卫真正要保证的那个**下游性质**，而不是它的返回值：**放行的条目 join 之后必须
/// 仍在作用域根之下、且不等于根本身**。
///
/// 这条测试存在的理由是 P0-1 的成因：守卫函数的返回值与 `root.join(rel)` 的实际落点是
/// 两件事，而那次缺陷正是这两件事分叉造成的——`classify` 说「相对路径」，`join` 出来的
/// 却是 `C:x`。只断言枚举的测试对这种分叉是瞎的，无论取样多密。
///
/// 不碰文件系统，纯路径运算，故 root 用一个不存在的虚构路径即可。
#[test]
fn every_allowed_entry_stays_under_the_root_after_join() {
    use std::path::Path;
    use wind_installer::manifest::safe_local_data_rel;

    let root = Path::new(r"C:\Users\U\AppData\Local\MyApp");
    let probes = [
        "logs",
        "cache/sub",
        "my logs",
        "state.toml",
        // 以下若被放行就是缺陷；放行了也必须仍在根之下（两道都得过）
        "./C:x",
        "logs/C:x",
        " C:x",
        "C:x",
        "...",
        ". .",
        "..",
        "cache/..",
        "/etc",
        "",
    ];
    for e in probes {
        let Some(rel) = safe_local_data_rel(e) else {
            continue; // 被拒即安全
        };
        let joined = root.join(&rel);
        assert!(
            joined.starts_with(root),
            "{e:?} 放行后 join 逃出了作用域根: rel={rel:?} joined={joined:?}"
        );
        assert_ne!(joined, root, "{e:?} 放行后 join 就是作用域根本身");
    }
}
