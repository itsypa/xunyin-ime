//! 安装 [`Step`] 的具体实现 —— 每个能力一个类型，由 `plan::plan_install` 按清单装配。
//!
//! 新增一种安装能力 = 在此加一个 `impl Step<InstallCtx>` + 在 planner 里加一行门控，
//! 不再需要同时修改 GUI 与静默两条编排链。
//!
//! **有系统副作用的步骤必须把做成的产物写进 `ctx.receipt`**，否则卸载时撤销不掉。

use crate::manifest::{AutoStartInfo, DataDirInfo, ShortcutInfo, StartupInfo};
use crate::meta;

use super::receipt::ReceiptEntry;
use super::step::{InstallCtx, Reporter, Step};
use super::{
    acl, font, ime, is_uninstaller_entry, legacy, process, registry, residue, shortcut, userdata,
};
use super::{InstallMode, UNINSTALLER_NAME};

// ── 安装环境标志 ────────────────────────────────────────────────────────────

/// 写入 InstallerRunning 标志，防止宿主进程在安装期间被其他组件重新拉起。
pub struct SetInstallerRunning;

impl Step<InstallCtx<'_>> for SetInstallerRunning {
    fn name(&self) -> String {
        "正在准备安装环境...".into()
    }
    fn run(&self, _ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        registry::set_installer_running()
    }
}

/// 把回执写入注册表。放在计划末尾——此时所有产物都已记录。
///
/// 回执自身无需记录：它随应用注册表键一并被卸载删除。
///
/// **致命步骤**：回执是卸载的唯一依据，写不进去就意味着装出了一个卸不干净的系统
/// （COM/输入法/字体/自启动/ARP 全部就位却无从撤销）。与其报告「安装完成」再让用户
/// 卸载时静默漏掉一切，不如让安装明确失败。
pub struct PersistReceipt;

impl Step<InstallCtx<'_>> for PersistReceipt {
    fn name(&self) -> String {
        "正在写入安装回执...".into()
    }
    fn fatal(&self) -> bool {
        true
    }
    fn run(&self, ctx: &mut InstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        r.log(&format!(
            "回执含 {} 条可撤销产物",
            ctx.receipt.entries.len()
        ));
        ctx.receipt.save()
    }
}

/// 清除 InstallerRunning 标志。
pub struct ClearInstallerRunning;

impl Step<InstallCtx<'_>> for ClearInstallerRunning {
    fn name(&self) -> String {
        "正在完成安装...".into()
    }
    fn run(&self, _ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        registry::clear_installer_running()
    }
}

// ── 前置清理 ────────────────────────────────────────────────────────────────

/// 终止 app.process_names 声明的进程。
pub struct TerminateProcesses;

impl Step<InstallCtx<'_>> for TerminateProcesses {
    fn name(&self) -> String {
        "正在停止旧进程...".into()
    }
    fn run(&self, _ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let survivors = process::terminate_app_processes();
        if survivors.is_empty() {
            Ok(())
        } else {
            // 非致命：文件锁随后由 create_or_backup 改名兜底。但把杀不掉的进程写进日志，
            // 否则"某进程没杀掉"无从诊断。
            Err(format!(
                "以下进程未能终止（占用的文件将靠改名释放）: {}",
                survivors.join(", ")
            ))
        }
    }
}

/// 反注册上一版本的 COM 组件（升级场景）。
pub struct UnregisterOldCom;

impl Step<InstallCtx<'_>> for UnregisterOldCom {
    fn name(&self) -> String {
        "正在反注册旧 COM...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        ime::unregister_old_com(&ctx.config.install_dir)
    }
}

/// 清扫上一版/旧产品遗留、指向已消失 DLL 的悬空 TSF 注册（COM CLSID、CTF TIP、
/// 旧 NSIS 的 RunOnce 重注册触发器）。变体隔离：只清当前清单 clsid/profile 那一套键。
///
/// 非致命：清扫失败绝不能阻断安装。放在注册之前——清完由 `RegisterCom`/
/// `RegisterInputMethod` 重建为指向新 DLL 的正确注册。
pub struct SweepImeResidue;

impl Step<InstallCtx<'_>> for SweepImeResidue {
    fn name(&self) -> String {
        "正在清理输入法残留...".into()
    }
    fn run(&self, _ctx: &mut InstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        let Some(ime) = meta::manifest().ime.as_ref() else {
            return Ok(());
        };
        let report = residue::sweep_dangling_ime(ime, meta::app_id());
        for item in &report.removed {
            r.log(&format!("已清除残留：{}", item));
        }
        Ok(())
    }
}

/// 删除新版已移除的旧版遗留文件/目录。
pub struct CleanupLegacy;

impl Step<InstallCtx<'_>> for CleanupLegacy {
    fn name(&self) -> String {
        "正在清理旧版遗留文件...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        legacy::cleanup_legacy(&ctx.config.install_dir, r);
        Ok(())
    }
}

// ── 文件释放 ────────────────────────────────────────────────────────────────

/// 将整个压缩块解压到内存。与释放分开成步，使 UI 能单独显示解压阶段。
pub struct PrepareArchive;

impl Step<InstallCtx<'_>> for PrepareArchive {
    fn name(&self) -> String {
        "正在解压数据...".into()
    }
    fn fatal(&self) -> bool {
        true
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        std::fs::create_dir_all(&ctx.config.install_dir)
            .map_err(|e| format!("无法创建安装目录: {}", e))?;
        ctx.archive.prepare()
    }
}

/// 逐文件写盘。便携模式跳过卸载器——外部程序以该文件的存在判定为安装版。
pub struct ExtractFiles;

impl Step<InstallCtx<'_>> for ExtractFiles {
    fn name(&self) -> String {
        "正在释放文件...".into()
    }
    fn fatal(&self) -> bool {
        true
    }
    fn run(&self, ctx: &mut InstallCtx, r: &mut dyn Reporter) -> Result<(), String> {
        let entries: Vec<_> = ctx
            .archive
            .entries()
            .iter()
            .filter(|e| ctx.mode == InstallMode::Standard || !is_uninstaller_entry(&e.path))
            .cloned()
            .collect();

        let total = entries.len();
        for (i, entry) in entries.iter().enumerate() {
            let dest = ctx.config.install_dir.join(&entry.path);
            r.step_progress(&entry.path, (i + 1) as f32 / total.max(1) as f32);
            ctx.archive
                .extract_entry(entry, &dest)
                .map_err(|e| format!("释放 {} 失败: {}", entry.path, e))?;
        }
        Ok(())
    }
}

/// 给解压出的卸载器追加清单 overlay，使其自包含——安装目录不留散落文件。
///
/// **正常情况下这一步什么也不做。** overlay 的内容（清单 + logo）全部来自 `app.toml`，
/// 没有一个字节依赖安装期，因此现在由打包器在构建机上就追加好、随后签名，装机端拿到的
/// 卸载器已经是终态。本步只为旧包保底：用老版打包器（无 `prep-uninstaller`）产出的
/// 安装包里，uninstall.exe 仍是裸 stub，装完读不到清单会直接启动失败。
///
/// ⚠️ 判据不能省。已带 overlay 的卸载器是**签过名的**，再追加一次就把证书表顶到文件
/// 中间，Authenticode 要求证书表必须是最后一段，多一个字节即"No signature found"。
pub struct AppendUninstallerOverlay;

impl Step<InstallCtx<'_>> for AppendUninstallerOverlay {
    fn name(&self) -> String {
        "正在写入卸载器清单...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let uninstaller = ctx.config.install_dir.join(UNINSTALLER_NAME);
        if !uninstaller.exists() {
            return Ok(());
        }
        if crate::archive::has_manifest_overlay(&uninstaller) {
            return Ok(());
        }
        crate::archive::append_manifest_overlay(
            &uninstaller,
            ctx.archive.manifest_bytes(),
            ctx.archive.logo_bytes(),
        )
    }
}

/// 写入便携模式标记文件。
pub struct WritePortableMarker;

impl Step<InstallCtx<'_>> for WritePortableMarker {
    fn name(&self) -> String {
        "正在写入便携模式标记...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        std::fs::write(
            ctx.config.install_dir.join(meta::portable_marker()),
            "portable=1\n",
        )
        .map_err(|e| format!("无法写入便携标记: {}", e))
    }
}

// ── 系统集成 ────────────────────────────────────────────────────────────────

/// 给 app.acl_dlls 授予 ALL APPLICATION PACKAGES 读取权限。
///
/// 无回执：ACL 随文件一起被删除，无需单独撤销。
pub struct SetDllAcl;

impl Step<InstallCtx<'_>> for SetDllAcl {
    fn name(&self) -> String {
        "正在设置文件权限...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        acl::set_dll_permissions(&ctx.config.install_dir)
    }
}

/// 安装 [[font]] 声明的字体到系统。
pub struct InstallFonts;

impl Step<InstallCtx<'_>> for InstallFonts {
    fn name(&self) -> String {
        "正在安装字体...".into()
    }

    /// 逐个字体装、逐个记回执：某个字体失败不该让已装好的那些漏记，
    /// 否则卸载时它们会永久留在系统字体目录里。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let mut errors = Vec::new();

        for f in &meta::manifest().font {
            match font::install_one(&ctx.config.install_dir, f) {
                Ok(()) => ctx.receipt.push(ReceiptEntry::FontInstalled {
                    file: f.file.clone(),
                    display_name: f.display_name.clone(),
                }),
                Err(e) => errors.push(format!("{}: {}", f.file, e)),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// 注册 TSF COM 组件。
pub struct RegisterCom;

impl Step<InstallCtx<'_>> for RegisterCom {
    fn name(&self) -> String {
        "正在注册 COM 组件...".into()
    }
    fn needs_reboot_on_failure(&self) -> bool {
        true
    }

    /// 先记账再报错：x64 成功、x86 失败时若整体早退而丢掉 x64 那条，
    /// 卸载后 x64 的 CLSID 会永久滞留且指向已删除的 DLL。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let result = ime::register_com(&ctx.config.install_dir);

        for (dll, wow64) in result.registered {
            ctx.receipt.push(ReceiptEntry::ComRegistered {
                dll: dll.to_string_lossy().to_string(),
                wow64,
            });
        }

        if result.errors.is_empty() {
            Ok(())
        } else {
            Err(result.errors.join("; "))
        }
    }
}

/// 注册为系统输入法（TSF profile）。
pub struct RegisterInputMethod;

impl Step<InstallCtx<'_>> for RegisterInputMethod {
    fn name(&self) -> String {
        "正在注册系统输入法...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let outcome = ime::register_input_method();
        // ⚠️ 回执先记，再把失败抛上去。
        //
        // `InstallLayoutOrTip` 报 false 不等于「一点都没注册成」——它是个没有文档的
        // API，返回值含义不确定。漏记回执的代价是卸载时根本不去反注册，语言栏里
        // 永远留着这个输入法，而那时用户已经没有卸载程序可用了。多记一条的代价只是
        // 卸载时多做一次无害的反注册。两边不对称，所以宁可多记。
        if let Some(profile) = ime::profile_id() {
            ctx.receipt
                .push(ReceiptEntry::InputMethodRegistered { profile });
        }
        outcome
    }
}

/// 注册开机自启动。
pub struct SetAutoStart {
    pub info: AutoStartInfo,
}

impl Step<InstallCtx<'_>> for SetAutoStart {
    fn name(&self) -> String {
        "正在配置开机自启动...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        registry::set_auto_start(&ctx.config.install_dir, &self.info)?;
        ctx.receipt.push(ReceiptEntry::AutoStartSet {
            value_name: meta::app_id().to_string(),
        });
        Ok(())
    }
}

/// 注册自定义 URL 协议。
pub struct RegisterUrlProtocol;

impl Step<InstallCtx<'_>> for RegisterUrlProtocol {
    fn name(&self) -> String {
        "正在注册协议...".into()
    }
    /// 按「键是否真的存在」记账，而非按返回值：`register_url_protocol` 先建协议键、
    /// 再建 shell\open\command 子键，中途失败会留下孤儿键——不记就撤销不掉。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let protocol = meta::url_protocol();
        let result = registry::register_url_protocol(&ctx.config.install_dir);

        if registry::url_protocol_exists(protocol) {
            ctx.receipt.push(ReceiptEntry::UrlProtocolRegistered {
                protocol: protocol.to_string(),
            });
        }

        result
    }
}

/// 创建 [[shortcut]] 声明的快捷方式。
pub struct CreateShortcuts {
    pub items: Vec<ShortcutInfo>,
}

impl Step<InstallCtx<'_>> for CreateShortcuts {
    fn name(&self) -> String {
        "正在创建快捷方式...".into()
    }
    /// 逐条记账：某个快捷方式失败不该让已创建的那些漏记，否则卸载后它们会
    /// 永久留在桌面/开始菜单，且指向已被删除的 exe。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let created = shortcut::create_shortcuts(&ctx.config.install_dir, &self.items);

        for path in created.links {
            ctx.receipt.push(ReceiptEntry::ShortcutCreated {
                path: path.to_string_lossy().to_string(),
            });
        }
        if let Some(dir) = created.start_menu_dir {
            ctx.receipt.push(ReceiptEntry::StartMenuFolderCreated {
                path: dir.to_string_lossy().to_string(),
            });
        }

        if created.errors.is_empty() {
            Ok(())
        } else {
            Err(created.errors.join("; "))
        }
    }
}

/// 写入 Add/Remove Programs 卸载信息。
pub struct WriteUninstallInfo;

impl Step<InstallCtx<'_>> for WriteUninstallInfo {
    fn name(&self) -> String {
        "正在写入卸载信息...".into()
    }
    /// 按「键是否真的存在」记账：`write_uninstall_info` 先建键、再逐个写值，
    /// 中途失败会留下孤儿 ARP 键——不记就撤销不掉，应用永久显示在「应用和功能」里。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let result = registry::write_uninstall_info(ctx.config);

        if registry::uninstall_key_exists() {
            ctx.receipt.push(ReceiptEntry::UninstallInfoWritten {
                key: registry::uninstall_key_path(),
            });
        }

        result
    }
}

/// 首次安装时写入用户数据目录配置；升级时跳过以保留旧配置。
pub struct WriteDataDirConf {
    pub info: DataDirInfo,
}

impl Step<InstallCtx<'_>> for WriteDataDirConf {
    fn name(&self) -> String {
        "正在写入数据目录配置...".into()
    }
    /// 升级时不重写文件（保留用户既有配置），但**仍要记回执**：文件归本产品所有，
    /// 卸载时该删。只在首装时记的话，「装过又升过」的用户卸载必留残留，
    /// 而这个文件正是数据目录的解析来源，会污染下次全新安装。
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        let path = if ctx.is_fresh_install {
            userdata::write_datadir_conf(ctx.config.effective_data_dir(), &self.info.conf_file)?
        } else {
            userdata::datadir_conf_path(&self.info.conf_file)
        };

        ctx.receipt.push(ReceiptEntry::DataDirConfWritten {
            path: path.to_string_lossy().to_string(),
        });
        Ok(())
    }
}

/// 安装完成后启动主程序。
///
/// 无回执：启动的进程会在卸载前被 TerminateProcesses 停掉，无持久产物。
pub struct PrestartApp {
    pub info: StartupInfo,
}

impl Step<InstallCtx<'_>> for PrestartApp {
    fn name(&self) -> String {
        "正在启动服务...".into()
    }
    fn run(&self, ctx: &mut InstallCtx, _r: &mut dyn Reporter) -> Result<(), String> {
        process::prestart_app(&ctx.config.install_dir, self.info.exe_or(meta::main_exe()))
    }
}
