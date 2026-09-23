//! 卸载 [`Step`] 的实现。
//!
//! 核心是 [`UndoReceipt`]：按安装回执反向回放，**不查清单**。卸载器因此不需要知道
//! IME、字体是什么——它只认回执里的产物类型。

use std::path::{Path, PathBuf};

use crate::installer::receipt::{Receipt, ReceiptEntry};
use crate::installer::step::{Reporter, Step};
use crate::installer::{font, ime, process, registry};

use super::cleanup::{self, CleanupOptions};

/// 卸载步骤的上下文。
pub struct UninstallCtx<'a> {
    pub options: &'a CleanupOptions,
    /// 用户数据目录，**在任何撤销动作之前解析好**。
    ///
    /// 不能等到 CleanupUserData 再现算：UndoReceipt 会删掉数据目录配置文件，
    /// 届时只能拿到默认位置，用户自定义的数据目录会被漏删。
    pub user_data_dir: PathBuf,
    /// 安装目录删除失败时置位，最终转化为「需要重启」。
    pub need_reboot: bool,
}

impl<'a> UninstallCtx<'a> {
    /// 构造上下文，并立即把「解析后才知道、撤销后就问不到」的信息固化下来。
    pub fn new(options: &'a CleanupOptions) -> Self {
        Self {
            user_data_dir: options.user_data_dir(),
            options,
            need_reboot: false,
        }
    }
}

/// 写入 InstallerRunning 标志，防止宿主进程在卸载期间被重新拉起。
pub struct SetInstallerRunning;

impl Step<UninstallCtx<'_>> for SetInstallerRunning {
    fn name(&self) -> String {
        "正在准备卸载环境...".into()
    }
    fn run(&self, _ctx: &mut UninstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        registry::set_installer_running()
    }
}

/// 终止 app.process_names 声明的进程。
pub struct TerminateProcesses;

impl Step<UninstallCtx<'_>> for TerminateProcesses {
    fn name(&self) -> String {
        "正在停止进程...".into()
    }
    fn run(&self, _ctx: &mut UninstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let survivors = process::terminate_app_processes();
        if survivors.is_empty() {
            Ok(())
        } else {
            Err(format!("以下进程未能终止: {}", survivors.join(", ")))
        }
    }
}

/// 按回执反向回放，撤销安装期做成的每一件事。
///
/// 单条撤销失败不中断其余条目——卸载要尽力而为，一个删不掉的注册表值不该
/// 让剩下的产物全部残留。
pub struct UndoReceipt;

impl Step<UninstallCtx<'_>> for UndoReceipt {
    fn name(&self) -> String {
        "正在撤销系统更改...".into()
    }

    fn run(&self, _ctx: &mut UninstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        // 在此就地读取（而非由调用方传入），是为了把「回执损坏」这个异常汇报出去：
        // 键不存在是正常的（没装过/便携装），内容损坏则意味着有产物却撤销不了。
        let receipt = match Receipt::try_load() {
            Ok(Some(r)) => r,
            Ok(None) => {
                r.log("无安装回执，跳过系统更改撤销");
                return Ok(());
            }
            Err(e) => {
                r.warn(&format!(
                    "安装回执损坏，无法撤销系统更改（可能有残留）: {}",
                    e
                ));
                return Ok(());
            }
        };

        let total = receipt.entries.len();
        for (i, entry) in receipt.undo_order().enumerate() {
            r.step_progress(&describe(entry), (i + 1) as f32 / total.max(1) as f32);
            if let Err(e) = undo(entry) {
                r.warn(&format!("撤销失败 [{}]: {}", describe(entry), e));
            }
        }
        Ok(())
    }
}

/// 撤销单条回执产物。
fn undo(entry: &ReceiptEntry) -> Result<(), String> {
    match entry {
        ReceiptEntry::ComRegistered { dll, wow64 } => {
            ime::unregister_com_path(Path::new(dll), *wow64)
        }
        ReceiptEntry::InputMethodRegistered { profile } => ime::unregister_profile(profile),
        ReceiptEntry::FontInstalled { file, display_name } => {
            font::uninstall_font_file(file, display_name)
        }
        ReceiptEntry::AutoStartSet { value_name } => registry::remove_auto_start_value(value_name),
        ReceiptEntry::UrlProtocolRegistered { protocol } => {
            registry::unregister_url_protocol_named(protocol)
        }
        ReceiptEntry::ShortcutCreated { path } => remove_file_if_exists(Path::new(path)),
        ReceiptEntry::StartMenuFolderCreated { path } => {
            let dir = PathBuf::from(path);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("删除 {:?} 失败: {}", dir, e))?;
            }
            Ok(())
        }
        ReceiptEntry::UninstallInfoWritten { key } => registry::remove_uninstall_key(key),
        ReceiptEntry::DataDirConfWritten { path } => remove_file_if_exists(Path::new(path)),
    }
}

/// 回执条目的人类可读描述，用于日志与进度。
fn describe(entry: &ReceiptEntry) -> String {
    match entry {
        ReceiptEntry::ComRegistered { dll, .. } => format!("反注册 COM {}", dll),
        ReceiptEntry::InputMethodRegistered { .. } => "反注册系统输入法".into(),
        ReceiptEntry::FontInstalled { file, .. } => format!("卸载字体 {}", file),
        ReceiptEntry::AutoStartSet { .. } => "移除开机自启动".into(),
        ReceiptEntry::UrlProtocolRegistered { protocol } => format!("移除协议 {}://", protocol),
        ReceiptEntry::ShortcutCreated { path } => format!("删除快捷方式 {}", path),
        ReceiptEntry::StartMenuFolderCreated { .. } => "删除开始菜单文件夹".into(),
        ReceiptEntry::UninstallInfoWritten { .. } => "移除卸载信息".into(),
        ReceiptEntry::DataDirConfWritten { .. } => "删除数据目录配置".into(),
    }
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("删除 {:?} 失败: {}", path, e))?;
    }
    Ok(())
}

/// 删除安装目录下的文件。
pub struct DeleteInstallFiles;

impl Step<UninstallCtx<'_>> for DeleteInstallFiles {
    fn name(&self) -> String {
        "正在删除程序文件...".into()
    }
    fn run(&self, ctx: &mut UninstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        // 这里原本还挂着 `.inspect_err(|_| ctx.need_reboot = true)`，但 delete_install_files
        // 恒返回 Ok，那句永远触发不了。删不掉的东西由它自己 `schedule_dir_on_reboot`
        // 排进账本，run_plan 收尾时统一读账本 —— 重启提示走的一直是那条路。
        cleanup::delete_install_files(&ctx.options.install_dir, r)
    }
}

/// 按用户勾选清理用户数据 / 缓存。
pub struct CleanupUserData;

impl Step<UninstallCtx<'_>> for CleanupUserData {
    fn name(&self) -> String {
        "正在清理用户数据...".into()
    }
    fn run(&self, ctx: &mut UninstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        cleanup::cleanup_user_data(ctx.options, &ctx.user_data_dir, r)
    }
}

/// 无条件删除本应用的 ARP（应用和功能）条目，作为回执缺失时的兜底。
///
/// 这是「卸载不读清单」的**唯一合理例外**：ARP 键路径由当前清单的 display_name 决定，
/// 而正在运行的这个卸载器就是该 ARP 条目 UninstallString 指向的程序——二者同源，
/// 不存在猜错的风险。
///
/// 兜底的必要性：回执写失败/损坏时若无此步，ARP 条目将永久残留且 UninstallString
/// 指向已被删除的 uninstall.exe——用户在设置里点卸载毫无反应，也无法从 UI 移除。
/// 与回执里的 UninstallInfoWritten 重复执行无害（第二次删不存在的键仅 warn）。
pub struct RemoveOwnUninstallInfo;

impl Step<UninstallCtx<'_>> for RemoveOwnUninstallInfo {
    fn name(&self) -> String {
        "正在移除应用列表条目...".into()
    }
    fn run(&self, _ctx: &mut UninstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let key = registry::uninstall_key_path();
        if registry::uninstall_key_exists() {
            registry::remove_uninstall_key(&key)?;
        }
        Ok(())
    }
}

/// 删除应用注册表键（含回执自身）。必须放在 UndoReceipt 之后。
pub struct RemoveAppKey;

impl Step<UninstallCtx<'_>> for RemoveAppKey {
    fn name(&self) -> String {
        "正在清理注册表...".into()
    }
    fn run(&self, _ctx: &mut UninstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        registry::remove_app_key()
    }
}
