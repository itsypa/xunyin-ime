use std::mem;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::meta;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 终止清单 app.process_names 声明的进程；返回仍存活（杀不掉）的进程映像名。
///
/// 以 Win32 `TerminateProcess` 为主手段、**每轮**直接强杀，不再仅依赖 PowerShell
/// `Stop-Process`（后者在部分机器上会静默失效，而旧实现只在最后一轮才兜底 Win32）；
/// PowerShell 调用保留为额外兜底。
///
/// 返回值非空即表示有进程终止不了——调用方应写入安装日志。历史上这里用 `eprintln`
/// 告警，但 GUI（windows subsystem）无控制台，告警全部丢失，导致"某进程没杀掉"无从诊断。
pub fn terminate_app_processes() -> Vec<String> {
    // 输入统一小写，与 find_pids 内部对实际进程名的小写比较对齐（防 process_names 含大写时漏杀）
    let images: Vec<String> = meta::process_names()
        .iter()
        .map(|n| format!("{}.exe", n.to_lowercase()))
        .collect();

    for _ in 0..4 {
        let alive: Vec<&String> = images
            .iter()
            .filter(|img| !find_pids(img).is_empty())
            .collect();
        if alive.is_empty() {
            return Vec::new();
        }

        // 主手段：Win32 TerminateProcess 逐 PID 强杀（直接、可靠、无外部进程依赖）
        for img in &alive {
            for pid in find_pids(img) {
                force_kill_pid(pid);
            }
        }
        // 兜底：单次 PowerShell Stop-Process 覆盖 Win32 偶发失败的边角
        let names: Vec<&str> = alive.iter().map(|s| s.trim_end_matches(".exe")).collect();
        stop_process_all(&names);

        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    images
        .into_iter()
        .filter(|img| !find_pids(img).is_empty())
        .collect()
}

/// 单次 PowerShell Stop-Process 杀掉多个进程名（只启动一次 PowerShell，开销小）
fn stop_process_all(names: &[&str]) {
    // 格式: 'foo','bar'
    let name_list = names
        .iter()
        .map(|n| format!("'{}'", n))
        .collect::<Vec<_>>()
        .join(",");

    let cmd = format!(
        "Get-Process -Name {} -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue",
        name_list
    );

    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &cmd,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output(); // 阻塞直到 PowerShell 退出
}

/// Win32 TerminateProcess 强制终止单个 PID（不等待退出）
fn force_kill_pid(pid: u32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
}

/// 用 CreateToolhelp32Snapshot 枚举指定名称（已 lowercase）的所有 PID
fn find_pids(exe_lower: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return pids,
        };
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
                if name == exe_lower {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

/// 安装完成后启动指定程序（DETACHED_PROCESS 独立于安装器进程组）
pub fn prestart_app(install_dir: &Path, exe: &str) -> Result<(), String> {
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let exe_path = install_dir.join(exe);
    if !exe_path.exists() {
        return Err(format!("{} not found", exe));
    }

    Command::new(&exe_path)
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map_err(|e| format!("Failed to start service: {}", e))?;

    Ok(())
}
