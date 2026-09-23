//! 安装计划装配 —— 由清单声明的能力段展开为有序 [`Step`] 列表。
//!
//! 这是「装什么、按什么顺序装」的唯一真相：GUI 与静默路径都调用 [`plan_install`]，
//! 差异仅在 Reporter 实现。

use crate::manifest::AppManifest;

use super::step::{InstallCtx, Step};
use super::steps::*;
use super::InstallMode;

/// 依据清单与安装模式装配安装步骤。
///
/// 便携模式只做「解压 + 写标记」，不触碰系统任何位置——这是 Standard/Portable
/// 的全部区别，无需在各步骤内部再判断模式。
pub fn plan_install<'a>(m: &AppManifest, mode: InstallMode) -> Vec<Box<dyn Step<InstallCtx<'a>>>> {
    let mut plan: Vec<Box<dyn Step<InstallCtx<'a>>>> = Vec::new();

    if mode == InstallMode::Portable {
        plan.push(Box::new(PrepareArchive));
        plan.push(Box::new(ExtractFiles));
        plan.push(Box::new(WritePortableMarker));
        return plan;
    }

    plan.push(Box::new(SetInstallerRunning));

    if !m.app.process_names.is_empty() {
        plan.push(Box::new(TerminateProcesses));
    }
    if m.ime.is_some() {
        plan.push(Box::new(UnregisterOldCom));
    }
    // 清扫是打包器显式声明的能力（[ime].sweep_residue = true），缺省不入计划。
    // 反注册处理「旧 DLL 仍在」的升级；清扫补上「旧 DLL 已消失」的悬空残留——
    // 正是 UnregisterOldCom 因 dll.exists()==false 跳过的那个缺口。
    if m.ime.as_ref().is_some_and(|i| i.sweep_residue) {
        plan.push(Box::new(SweepImeResidue));
    }
    if !m.app.legacy_files.is_empty() || !m.app.legacy_dirs.is_empty() {
        plan.push(Box::new(CleanupLegacy));
    }

    plan.push(Box::new(PrepareArchive));
    plan.push(Box::new(ExtractFiles));
    plan.push(Box::new(AppendUninstallerOverlay));

    if !m.app.acl_dlls.is_empty() {
        plan.push(Box::new(SetDllAcl));
    }
    if !m.font.is_empty() {
        plan.push(Box::new(InstallFonts));
    }
    if m.ime.is_some() {
        plan.push(Box::new(RegisterCom));
        plan.push(Box::new(RegisterInputMethod));
    }
    if let Some(auto) = m.autostart.as_ref().filter(|a| a.enabled) {
        plan.push(Box::new(SetAutoStart { info: auto.clone() }));
    }
    if !m.app.url_protocol.trim().is_empty() {
        plan.push(Box::new(RegisterUrlProtocol));
    }
    if !m.shortcut.is_empty() {
        plan.push(Box::new(CreateShortcuts {
            items: m.shortcut.clone(),
        }));
    }

    plan.push(Box::new(WriteUninstallInfo));

    if let Some(datadir) = m.datadir.as_ref() {
        plan.push(Box::new(WriteDataDirConf {
            info: datadir.clone(),
        }));
    }

    if let Some(startup) = m.startup.as_ref().filter(|s| s.prestart) {
        plan.push(Box::new(PrestartApp {
            info: startup.clone(),
        }));
    }

    // 回执必须在所有有副作用的步骤之后落盘
    plan.push(Box::new(PersistReceipt));
    plan.push(Box::new(ClearInstallerRunning));
    plan
}
