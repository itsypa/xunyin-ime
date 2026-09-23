use std::path::{Path, PathBuf};

use crate::manifest::{expand_placeholders, ShortcutInfo, ShortcutLocation};
use crate::meta;

/// `create_shortcuts` 实际创建了什么 + 失败信息。
///
/// 不用 `Result<_, String>` 是因为回执必须记下「已经做成的部分」：一个条目失败时若
/// 整体返回 Err 而丢掉已创建的那些，卸载后它们会永久留在桌面/开始菜单，
/// 且指向已被删除的 exe。
#[derive(Debug, Default)]
pub struct CreatedShortcuts {
    /// 创建成功的 .lnk 绝对路径。
    pub links: Vec<PathBuf>,
    /// 若创建过开始菜单快捷方式，则为那个文件夹（卸载时整个删除）。
    pub start_menu_dir: Option<PathBuf>,
    pub errors: Vec<String>,
}

/// 创建清单 [[shortcut]] 段声明的快捷方式。目标不存在的条目跳过（不报错）。
/// 逐条尝试，返回实际创建成功的那些。
pub fn create_shortcuts(install_dir: &Path, items: &[ShortcutInfo]) -> CreatedShortcuts {
    let mut created = CreatedShortcuts::default();

    // 占位符替换值（使能力段配置对 dev/release 等变体通用）
    let (main_exe, setting_exe, display, app_id) = (
        meta::main_exe(),
        meta::setting_exe(),
        meta::app_display_name(),
        meta::app_id(),
    );
    let expand = |s: &str| expand_placeholders(s, main_exe, setting_exe, display, app_id);

    for item in items {
        // {setting_exe} 在未配置设置程序时展开为空 → 跳过该快捷方式
        let target_rel = expand(&item.target);
        if target_rel.trim().is_empty() {
            continue;
        }
        let target = install_dir.join(&target_rel);
        if !target.exists() {
            continue;
        }

        let dir = location_dir(item.location);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            created
                .errors
                .push(format!("无法创建快捷方式目录 {:?}: {}", dir, e));
            continue;
        }

        let link_path = dir.join(format!("{}.lnk", expand(item.effective_name())));
        match create_shortcut(
            &target,
            &link_path,
            &install_dir.to_string_lossy(),
            &expand(item.effective_description()),
        ) {
            Ok(()) => {
                if item.location == ShortcutLocation::StartMenu {
                    created.start_menu_dir = Some(dir);
                }
                created.links.push(link_path);
            }
            Err(e) => created.errors.push(e),
        }
    }

    created
}

// 删除快捷方式由 `uninstaller::steps::UndoReceipt` 按回执记录的绝对路径逐个撤销，
// 无需在此按清单反推——清单改过名字也不会留下孤儿 .lnk。

fn create_shortcut(
    target: &Path,
    link_path: &Path,
    working_dir: &str,
    description: &str,
) -> Result<(), String> {
    let mut link =
        mslnk::ShellLink::new(target).map_err(|e| format!("无法创建快捷方式对象: {}", e))?;

    link.set_working_dir(Some(working_dir.to_string()));
    link.set_name(Some(description.to_string()));

    link.create_lnk(link_path)
        .map_err(|e| format!("无法保存快捷方式 {:?}: {}", link_path, e))?;

    Ok(())
}

fn location_dir(loc: ShortcutLocation) -> PathBuf {
    match loc {
        ShortcutLocation::StartMenu => start_menu_dir(),
        ShortcutLocation::Desktop => desktop_dir(),
    }
}

/// 全局开始菜单下的应用文件夹。
fn start_menu_dir() -> PathBuf {
    let program_data =
        std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    PathBuf::from(program_data)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(meta::start_menu_folder())
}

/// 公共桌面（对所有用户可见）。
fn desktop_dir() -> PathBuf {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".to_string());
    PathBuf::from(public).join("Desktop")
}
