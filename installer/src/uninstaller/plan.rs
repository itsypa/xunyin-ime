//! 卸载计划装配 —— 与 `installer::plan` 对称，是「卸什么、按什么顺序卸」的唯一真相。
//!
//! 注意这里**不读清单**：撤销什么完全由安装回执决定。加一种新能力时，只要安装侧
//! 记了回执、`uninstaller::steps::undo` 认得那个条目类型，卸载侧就自动支持——
//! 不需要在这里加任何门控。这正是回执模型相对「手写镜像卸载脚本」的核心优势。

use crate::installer::step::Step;

use super::steps::*;

/// 装配卸载步骤。顺序上有三条硬约束：
/// - **撤销必须早于删文件**：反注册 COM 需要 DLL 还在盘上；
/// - **删应用键必须晚于撤销**：回执就存在那个键里；
/// - **用户数据目录须在撤销前解析**：撤销会删掉数据目录配置文件
///   （由 `UninstallCtx::new` 在构造时固化）。
pub fn plan_uninstall<'a>() -> Vec<Box<dyn Step<UninstallCtx<'a>>>> {
    vec![
        Box::new(SetInstallerRunning),
        Box::new(TerminateProcesses),
        Box::new(UndoReceipt),
        // 回执缺失/损坏时的兜底，确保应用不会永久留在「应用和功能」里
        Box::new(RemoveOwnUninstallInfo),
        Box::new(DeleteInstallFiles),
        Box::new(CleanupUserData),
        Box::new(RemoveAppKey),
    ]
}
