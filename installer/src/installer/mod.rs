pub mod acl;
pub mod config;
pub mod font;
pub mod ime;
pub mod legacy;
pub mod plan;
pub mod process;
pub mod receipt;
pub mod registry;
pub mod residue;
pub mod shortcut;
pub mod step;
pub mod steps;
pub mod userdata;

use crate::archive::ArchiveReader;
use crate::meta;
use config::InstallConfig;
use receipt::Receipt;
use step::{run_plan, CliReporter, InstallCtx};

/// 卸载器文件名（安装目录内）
pub const UNINSTALLER_NAME: &str = "uninstall.exe";

/// 判断归档条目是否为卸载器。便携模式不释放它——外部程序以该文件的存在判定为安装版。
pub fn is_uninstaller_entry(path: &str) -> bool {
    path.eq_ignore_ascii_case(UNINSTALLER_NAME)
}

/// 安装模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// 标准安装 - 注册到系统
    Standard,
    /// 便携模式 - 仅解压文件
    Portable,
}

/// 安装结果
#[derive(Debug)]
#[allow(dead_code)]
pub struct InstallResult {
    pub success: bool,
    pub message: String,
    pub need_reboot: bool,
}

/// 执行完整安装流程（静默/CLI 路径）。
///
/// GUI 路径见 `ui::install_wizard`——两者共用 `plan::plan_install` 生成的同一份
/// 步骤计划，仅 Reporter 实现不同。
pub fn perform_install(config: &InstallConfig, mode: InstallMode) -> InstallResult {
    let mut archive = match ArchiveReader::open_current_exe() {
        Ok(a) => a,
        Err(e) => {
            return InstallResult {
                success: false,
                message: format!("Failed to open archive: {}", e),
                need_reboot: false,
            }
        }
    };

    // 与 GUI 路径同一判定：datadir.conf 写在 %LOCALAPPDATA% 是机器全局的，
    // 故「是否首装」也必须按机器判定，不能按 install_dir 是否存在卸载器判定——
    // 否则装到新目录会覆盖老用户已有的数据目录配置。
    let is_fresh_install = registry::detect_installed_version().is_none();
    // 续写旧回执而非从空起：新清单里已删掉的能力（如上一版装了字体、这一版删了
    // [[font]] 段）其产物仍在系统上，回执被覆盖就永远撤销不掉。push 会按等值去重。
    //
    // 声明顺序也有讲究：receipt 必须先于 plan——plan 的类型带 InstallCtx 的生命周期，
    // 后声明会让它在 receipt 之后析构而借用检查不过。
    let mut receipt = Receipt::load_or_default();
    let plan = plan::plan_install(meta::manifest(), mode);
    let mut reporter = CliReporter;

    let mut ctx = InstallCtx {
        config,
        mode,
        is_fresh_install,
        archive: &mut archive,
        receipt: &mut receipt,
    };

    match run_plan(&plan, &mut ctx, &mut reporter) {
        Ok(outcome) => InstallResult {
            success: true,
            message: "Installation completed successfully".into(),
            need_reboot: outcome.need_reboot,
        },
        Err(e) => {
            // 致命失败：确保不残留 InstallerRunning 标志，否则宿主进程将永久停摆
            let _ = registry::clear_installer_running();
            InstallResult {
                success: false,
                message: e,
                need_reboot: false,
            }
        }
    }
}
