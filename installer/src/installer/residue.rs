//! 输入法（TSF）残留清扫 —— 清除「上一版/旧产品」卸载不干净、指向已消失 DLL 的
//! **悬空注册表项**，避免它们污染系统输入法子系统（症状：某些应用无法开启输入法，
//! 重启也不恢复）。
//!
//! ## 变体隔离（务必理解）
//! 所有要清的键都从**当前清单的 [ime] 段**（`clsid` / `profile_guid`）推导。正式版打包
//! 用 EE30/EE31、Dev 版用 DEB0/DEB1，二者的 clsid 不同 —— 因此正式版安装器物理上只可能
//! 触及 EE30 那套键，Dev 版只可能触及 DEB0 那套。**开发版与正式版按设计要能共存，本模块
//! 绝不会互删**，隔离性来自「配置即变体身份」，无需任何变体判断分支。
//!
//! ## 只清「悬空」，不碰健康注册
//! 判据：`HKLM\Software\Classes\CLSID\{clsid}\InprocServer32` 指向的 DLL 在磁盘上**已不存在**。
//! 判定只看注册里记录的那个路径本身——注册指向系统目录副本（现行部署形态）时，
//! 副本在即健康，安装目录里文件的去留不影响判定，不会因系统副本而误判。
//! - 原地升级（注册指向系统目录旧副本、副本仍在）→ 判为健康 → 不动，留给随后的
//!   `RegisterCom` 覆盖部署。
//! - 注册指向的 DLL 被删或被改名成 `.old_*`（`UnregisterOldCom` 因副本不存在而跳过）
//!   → 判为悬空 → 清除 → `ExtractFiles` 落地新 DLL、`RegisterCom` 部署系统副本后
//!   重建为正确注册。
//!
//! ## 刻意不做
//! 不遍历删除 `HKCU\...\CTF\Assemblies` / `SortOrder`：那是 Windows 维护的输入法排序缓存，
//! 误删会打乱用户**整个**输入法列表；且重新启用本 profile 时系统会自行重建这些条目。

use std::path::Path;

use winreg::enums::*;
use winreg::RegKey;

use crate::manifest::ImeInfo;

/// 类别注册核心根：TSF 把每个 text service 的 profile/category 挂在此 CLSID 子键下。
const CTF_TIP: &str = r"SOFTWARE\Microsoft\CTF\TIP";

/// 一次清扫的结果，供步骤写入日志。非致命——清扫失败绝不能阻断安装。
#[derive(Default)]
pub struct SweepReport {
    /// 已清除项的人类可读描述。
    pub removed: Vec<String>,
}

impl SweepReport {
    fn note(&mut self, msg: impl Into<String>) {
        self.removed.push(msg.into());
    }
}

/// 清扫当前变体（由 `ime` 的 clsid/profile 决定）遗留的悬空 TSF 注册。
///
/// 判定悬空只看注册表里 `InprocServer32` 记录的 DLL 路径是否存在，与本次安装目录
/// 无关（旧注册可能指向任意历史路径）。
pub fn sweep_dangling_ime(ime: &ImeInfo, app_id: &str) -> SweepReport {
    let mut report = SweepReport::default();

    // 1) x64 COM 插槽：HKLM\Software\Classes\CLSID\{clsid}
    let clsid_x64 = format!(r"Software\Classes\CLSID\{}", ime.clsid);
    let x64_dangling = clsid_dangling(&clsid_x64);
    if x64_dangling == Some(true) && delete_hklm_tree(&clsid_x64) {
        report.note(format!("悬空 COM 注册 (x64) CLSID\\{}", ime.clsid));
    }

    // 2) x86 COM 插槽：HKLM\Software\Classes\WOW6432Node\CLSID\{clsid}（独立判定）
    let clsid_x86 = format!(r"Software\Classes\WOW6432Node\CLSID\{}", ime.clsid);
    if clsid_dangling(&clsid_x86) == Some(true) && delete_hklm_tree(&clsid_x86) {
        report.note(format!(
            "悬空 COM 注册 (x86) WOW6432Node\\CLSID\\{}",
            ime.clsid
        ));
    }

    // 3) & 4) CTF TIP 登记：仅当主服务（x64）判为悬空时清理。健康或原地升级不动，
    //    重建交给 RegisterInputMethod（InstallLayoutOrTip）。
    if x64_dangling == Some(true) {
        let tip = format!(r"{}\{}", CTF_TIP, ime.clsid);
        if delete_hklm_tree(&tip) {
            report.note(format!("悬空 TSF TIP (HKLM) {}", ime.clsid));
        }
        if delete_hkcu_tree(&tip) {
            report.note(format!("悬空 TSF TIP (每用户) {}", ime.clsid));
        }
    }

    // 5) 旧 NSIS 安装器在 COM 注册失败时写入的「重启重注册」触发器。若在重启前卸载，
    //    这两个 RunOnce 值不会被清，重启后仍 regsvr32 一个可能已失效的 DLL 路径。
    //    值名沿旧安装器约定 `{app_id}_Register[X86]OnReboot`；app_id 属清单，故变体隔离。
    for suffix in ["_RegisterOnReboot", "_RegisterX86OnReboot"] {
        let value = format!("{}{}", app_id, suffix);
        if delete_runonce_value(&value) {
            report.note(format!("旧重启重注册触发器 RunOnce\\{}", value));
        }
    }

    report
}

/// 判定某 CLSID 是否为「悬空」注册。
/// - `None`：该 CLSID 无注册（无 InprocServer32 子键）——无需处理。
/// - `Some(false)`：InprocServer32 指向的 DLL 存在——健康，不动。
/// - `Some(true)`：键存在但 DLL 缺失（被删/改名），或 InprocServer32 无有效路径——悬空。
fn clsid_dangling(clsid_subkey: &str) -> Option<bool> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let inproc = hklm
        .open_subkey_with_flags(format!(r"{}\InprocServer32", clsid_subkey), KEY_READ)
        .ok()?;

    // 默认值 = DllRegisterServer 写入的 GetModuleFileNameW 原始路径（无引号）。
    let dll_path: String = inproc.get_value("").unwrap_or_default();
    let trimmed = dll_path.trim().trim_matches('"');
    if trimmed.is_empty() {
        // 键在、路径空：属破损注册，视为悬空。
        return Some(true);
    }
    Some(!Path::new(trimmed).exists())
}

/// 递归删除 HKLM 下子键；返回是否确有删除动作（键原先存在）。忽略「不存在」之外的错误。
fn delete_hklm_tree(subkey: &str) -> bool {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if hklm.open_subkey_with_flags(subkey, KEY_READ).is_err() {
        return false; // 本就不存在
    }
    match hklm.delete_subkey_all(subkey) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Warning: 清扫 HKLM\\{} 失败: {}", subkey, e);
            false
        }
    }
}

/// 递归删除 HKCU 下子键；语义同 [`delete_hklm_tree`]。
fn delete_hkcu_tree(subkey: &str) -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if hkcu.open_subkey_with_flags(subkey, KEY_READ).is_err() {
        return false;
    }
    match hkcu.delete_subkey_all(subkey) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Warning: 清扫 HKCU\\{} 失败: {}", subkey, e);
            false
        }
    }
}

/// 删除 HKLM RunOnce 下的单个值；返回是否确有删除（值原先存在）。
fn delete_runonce_value(value_name: &str) -> bool {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let Ok(runonce) = hklm.open_subkey_with_flags(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
        KEY_READ | KEY_WRITE,
    ) else {
        return false;
    };
    // 先探在不在，避免把「值不存在」当成错误刷日志。
    if runonce.get_raw_value(value_name).is_err() {
        return false;
    }
    match runonce.delete_value(value_name) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Warning: 清扫 RunOnce\\{} 失败: {}", value_name, e);
            false
        }
    }
}
