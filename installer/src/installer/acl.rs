use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::meta;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// ALL APPLICATION PACKAGES SID
const APP_PACKAGES_SID: &str = "*S-1-15-2-1";

/// BUILTIN\Users：数据目录要让普通用户能写（见 [`grant_users_modify_tree`]）。
const USERS_SID: &str = "*S-1-5-32-545";

/// 让普通用户能改这个目录树（本地修补）。
///
/// 为什么需要：数据目录默认落在 `%ProgramFiles%\Data` 这类位置，默认可写权限只给管理员，
/// 而输入法是**以普通用户身份**运行的（TSF DLL 跑在宿主进程里），写配置、写用户词库、
/// 写日志都会 Access Denied —— 表现为"设置存不上、学过的词不生效"。
/// 安装时（提权）把这一条补上：`Users:(OI)(CI)M` = 用户组对该目录树有修改权限。
pub fn grant_users_modify_tree(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let path_str = dir.to_string_lossy();
    let grant_arg = format!("{}:(OI)(CI)M", USERS_SID);
    let output = Command::new("icacls")
        .args([path_str.as_ref(), "/grant", grant_arg.as_str(), "/T", "/C", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run icacls: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("icacls failed for {}: {}", path_str, stderr));
    }
    Ok(())
}

/// 设置 DLL 权限（ALL APPLICATION PACKAGES 读取执行）
pub fn set_dll_permissions(install_dir: &Path) -> Result<(), String> {
    let dlls: Vec<_> = meta::acl_dlls()
        .iter()
        .map(|n| install_dir.join(n))
        .collect();

    for dll_path in &dlls {
        if dll_path.exists() {
            set_file_acl(dll_path, APP_PACKAGES_SID, "RX")?;
        }
    }

    Ok(())
}

/// 授予单个文件 ALL APPLICATION PACKAGES 读取执行权限。
///
/// 供部署到系统目录的 COM DLL 副本使用：AppContainer 宿主（UWP/沙箱应用）加载 COM
/// 服务器的前提，与上面的 `acl_dlls` 是同一授权，只是作用在系统副本上。
pub fn grant_app_packages_rx(path: &Path) -> Result<(), String> {
    set_file_acl(path, APP_PACKAGES_SID, "RX")
}

/// 设置文件 ACL（无窗口）
fn set_file_acl(path: &Path, sid: &str, permission: &str) -> Result<(), String> {
    let path_str = path.to_string_lossy();
    let grant_arg = format!("{}:({})", sid, permission);

    let output = Command::new("icacls")
        .args([&*path_str, "/grant", &grant_arg, "/c"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to run icacls: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("icacls failed for {}: {}", path_str, stderr));
    }

    Ok(())
}
