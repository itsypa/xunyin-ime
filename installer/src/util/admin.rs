use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// 检查当前进程是否以管理员（提升的令牌）运行。
/// 直接读取进程令牌的 TokenElevation，无任何子进程/窗口。
pub fn is_admin() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        let _ = CloseHandle(token);

        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// 用 ShellExecuteW "runas" 请求 UAC 提权并重新启动自身。
/// 不产生任何子进程窗口（标准 UAC 对话框由系统弹出，不是控制台）。
pub fn request_elevation() -> Result<(), String> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Failed to get current exe: {}", e))?;

    // 转发原始命令行参数。不转发的话，提权重启后的实例拿不到 `--silent --dir ...`，
    // 应用内自动升级会静默退化成交互式向导（用户看到的是"升级时路径居然可以改"）。
    // 含空格的参数补引号；路径以反斜杠结尾的极端情况未处理（会转义掉结尾引号），
    // 但安装目录不会以反斜杠结尾，实际不构成问题。
    //
    // ⚠️ 全程走 `OsString`，两个理由，缺一不可：
    // 1. `std::env::args()` 撞上非法 Unicode（Windows 命令行允许孤立代理）会 **panic**，
    //    而 release 是 `panic = "abort"` + GUI 子系统 —— 那是一次无提示崩溃，
    //    发生在「控制面板点卸载 → 请求提权」这条路上。`uninstaller::args` 里有一条
    //    `lone_surrogate_does_not_panic` 钉着解析那一侧，但它管不到这里，
    //    保证只覆盖半条路等于没有保证。
    // 2. 也**不能**用 `to_string_lossy` 绕开。`uninstaller::args::parse` 里用了它、而且
    //    是对的，但那是**读端**：解析只需要「U+FFFD 不等于任何 flag 字面量」，替换掉的
    //    东西本来就要被丢弃。这里是**转发端**，同一个替换的性质完全变了 —— 它会悄悄
    //    改写交给子进程的参数，提权后的实例拿到的路径与用户给的不是同一个，而且没有
    //    任何地方会报错。别照着 args.rs 的先例把 lossy 抄到这儿。
    //    `OsString` 一路带到 `encode_wide`，原样进 `ShellExecuteW`。
    let mut params = std::ffi::OsString::new();
    for a in std::env::args_os().skip(1) {
        if !params.is_empty() {
            params.push(" ");
        }
        // 判「要不要补引号」只看得懂的那部分：非法码位既不是空格也不是引号，
        // 看漏它不影响判断，而拼接用的仍是原始的 `a`。
        let lossy = a.to_string_lossy();
        if lossy.contains(' ') && !lossy.starts_with('"') {
            params.push("\"");
            params.push(&a);
            params.push("\"");
        } else {
            params.push(&a);
        }
    }

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let path: Vec<u16> = current_exe
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let params_w: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        params.encode_wide().chain(std::iter::once(0)).collect()
    };

    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(path.as_ptr()),
            if params.is_empty() {
                windows::core::PCWSTR::null()
            } else {
                windows::core::PCWSTR(params_w.as_ptr())
            },
            None,
            SW_SHOWNORMAL,
        )
    };

    // ShellExecuteW 返回值 > 32 表示成功
    if result.0 as isize <= 32 {
        return Err(format!("ShellExecuteW runas failed: {}", result.0 as isize));
    }

    Ok(())
}
