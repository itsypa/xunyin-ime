use wind_installer::manifest::ProjectConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVersionInfo {
    pub company_name: String,
    pub file_description: String,
    pub file_version: String, // 格式化后的 4 位字符串，例如 "1.0.0.0"
    pub product_name: String,
    pub product_version: String, // 格式化后的 4 位字符串
    pub copyright: String,
    pub original_filename: String,
    pub internal_name: String,
}

/// 解析版本号字符串为 (major_u32, minor_u32)，分别存储 MS 和 LS 的打包值。
pub fn parse_version_string(v: &str) -> (u32, u32) {
    let mut parts = [0u16; 4];
    for (i, p) in v.split('.').enumerate() {
        if i >= 4 {
            break;
        }
        parts[i] = p.trim().parse::<u16>().unwrap_or(0);
    }
    let major = ((parts[0] as u32) << 16) | (parts[1] as u32);
    let minor = ((parts[2] as u32) << 16) | (parts[3] as u32);
    (major, minor)
}

/// 将版本字符串自动规范化为 A.B.C.D 格式。
pub fn format_version_4_parts(v: &str) -> String {
    let mut parts = [0u16; 4];
    for (i, p) in v.split('.').enumerate() {
        if i >= 4 {
            break;
        }
        parts[i] = p.trim().parse::<u16>().unwrap_or(0);
    }
    format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3])
}

/// 根据配置和推导规则解析版本信息属性
pub fn derive_version_info(cfg: &ProjectConfig, is_uninstaller: bool) -> ResolvedVersionInfo {
    let v_cfg = cfg.package.version_info.as_ref();
    let app = &cfg.manifest.app;

    let company_name = v_cfg
        .and_then(|vi| vi.company_name.as_ref())
        .cloned()
        .unwrap_or_else(|| app.publisher.clone());

    let file_description = v_cfg
        .and_then(|vi| vi.file_description.as_ref())
        .cloned()
        .unwrap_or_else(|| {
            if is_uninstaller {
                format!("{} 卸载程序", app.display_name)
            } else {
                format!("{} 安装程序", app.display_name)
            }
        });

    let raw_file_ver = v_cfg
        .and_then(|vi| vi.file_version.as_ref())
        .cloned()
        .unwrap_or_else(|| app.version.clone());
    let file_version = format_version_4_parts(&raw_file_ver);

    let product_name = v_cfg
        .and_then(|vi| vi.product_name.as_ref())
        .cloned()
        .unwrap_or_else(|| app.display_name.clone());

    let raw_prod_ver = v_cfg
        .and_then(|vi| vi.product_version.as_ref())
        .cloned()
        .unwrap_or_else(|| app.version.clone());
    let product_version = format_version_4_parts(&raw_prod_ver);

    let copyright = v_cfg
        .and_then(|vi| vi.copyright.as_ref())
        .cloned()
        .unwrap_or_else(|| format!("Copyright (C) 2026 {}", app.publisher));

    let original_filename = if is_uninstaller {
        "uninstall.exe".to_string()
    } else {
        v_cfg
            .and_then(|vi| vi.original_filename.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                let base_name = if cfg.package.output_name.is_empty() {
                    app.id.clone()
                } else {
                    cfg.package.output_name.clone()
                };
                format!("{}-{}.exe", base_name, app.version)
            })
    };

    let internal_name = if is_uninstaller {
        "uninstall".to_string()
    } else {
        app.id.clone()
    };

    ResolvedVersionInfo {
        company_name,
        file_description,
        file_version,
        product_name,
        product_version,
        copyright,
        original_filename,
        internal_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wind_installer::manifest::ProjectConfig;

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version_string("1.2.3"), (0x00010002, 0x00030000));
        assert_eq!(parse_version_string("0.8.2.15"), (0x00000008, 0x0002000f));
        assert_eq!(parse_version_string("invalid"), (0, 0));
    }

    #[test]
    fn test_format_version() {
        assert_eq!(format_version_4_parts("1.2.3"), "1.2.3.0");
        assert_eq!(format_version_4_parts("2.5"), "2.5.0.0");
        assert_eq!(format_version_4_parts("1.2.3.4"), "1.2.3.4");
    }

    /// 从 TOML 解析而非手工构造结构体：版本推导只关心 app 的几个字段，
    /// 手写全部字段会让每次给清单加字段都无谓地断掉本测试（serde 默认值本就覆盖了其余项）。
    const MINIMAL_CONFIG: &str = r#"
[app]
id           = "test-app"
display_name = "测试应用"
version      = "1.2.3"
publisher    = "测试发行商"
main_exe     = "test.exe"

[package]
compression = "zstd"
source_dir  = "src"
"#;

    #[test]
    fn test_derive_version_info_default() {
        let cfg = ProjectConfig::from_toml_str(MINIMAL_CONFIG).expect("解析配置失败");

        // Test installer derivation
        let info = derive_version_info(&cfg, false);
        assert_eq!(info.company_name, "测试发行商");
        assert_eq!(info.file_description, "测试应用 安装程序");
        assert_eq!(info.file_version, "1.2.3.0");
        assert_eq!(info.product_name, "测试应用");
        assert_eq!(info.product_version, "1.2.3.0");
        assert_eq!(info.copyright, "Copyright (C) 2026 测试发行商");
        assert_eq!(info.original_filename, "test-app-1.2.3.exe");
        assert_eq!(info.internal_name, "test-app");

        // Test uninstaller derivation
        let uninst_info = derive_version_info(&cfg, true);
        assert_eq!(uninst_info.company_name, "测试发行商");
        assert_eq!(uninst_info.file_description, "测试应用 卸载程序");
        assert_eq!(uninst_info.file_version, "1.2.3.0");
        assert_eq!(uninst_info.product_name, "测试应用");
        assert_eq!(uninst_info.product_version, "1.2.3.0");
        assert_eq!(uninst_info.copyright, "Copyright (C) 2026 测试发行商");
        assert_eq!(uninst_info.original_filename, "uninstall.exe");
        assert_eq!(uninst_info.internal_name, "uninstall");
    }
}
