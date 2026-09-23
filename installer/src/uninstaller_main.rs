#![windows_subsystem = "windows"]
#![allow(dead_code, unused_imports)]

mod archive;
mod installer;
mod manifest;
mod meta;
mod ui;
mod uninstaller;
mod util;

use util::exitcode;

fn main() {
    // 自删除模式：由临时副本调用，不需要 UI 也不访问清单，直接清理后退出
    if uninstaller::selfdelete::is_self_delete_mode() {
        if let Some(dir) = uninstaller::selfdelete::self_delete_target() {
            uninstaller::selfdelete::execute_self_delete(&dir);
        }
        return;
    }

    // 参数解析必须排在最前（自删除那条除外——它有自己的一套 flag，且跑在临时副本里）：
    // 下面每一步的收场方式都取决于「这次是不是静默调用」。
    let args = uninstaller::args::from_env();
    log_startup(&args);

    if !util::admin::is_admin() {
        uninstaller::require_admin_or_exit(args.silent);
    }

    // 载入清单（来自安装目录的 .manifest，位于卸载器自身旁）
    if let Err(e) = meta::bootstrap() {
        eprintln!("无法载入卸载清单: {}", e);
        std::process::exit(exitcode::FAILURE);
    }

    // 检查单实例。排在 bootstrap 之后是为了**提示框里能报出应用名**。
    //
    // ⚠️ 这里曾经是一条硬约束（「清单没载入就 panic」），现在不是了：`busy_title()`
    // 被抽出来改用不 panic 的 `try_app_display_name()` 之后，即便排在前面也只会退化成
    // 通用标题「安装程序」。注释保留这段历史，是因为**约束确实松过一档**——
    // 别再照着「会崩」去推断别处的顺序有多不可动。
    if let Some(pid) = util::single::another_instance_pid() {
        util::single::report_busy_and_exit(pid, args.silent);
    }

    if args.silent {
        let code = uninstaller::run_silent(args.keep_user_data);
        util::single::release_lock();
        std::process::exit(code);
    }

    ui::uninstall_wizard::run_uninstall_wizard();
    // ⚠️ **这一行在 GUI 路径上永远执行不到**，看着会执行而已：向导内部有三个
    // `process::exit`（完成页按钮、关窗，以及 `trigger_self_delete` 成功时那个），
    // 没有一条会 return 回来。后果是 `%TEMP%\wind_installer.lock` 留在盘上。
    //
    // 不修是权衡后的决定：锁是新格式 `"<pid>|<创建时间>"`，下次读到时主人早已不在，
    // `lock_blocks` 判 `alive == false` 直接放行——它是**不再挡人的垃圾文件**，
    // 不是僵尸锁。而修它要动向导那三个 exit 点，其中自删除那个紧跟在「副本已启动」
    // 之后，往中间插任何东西都是给自删除时序平添风险。
    util::single::release_lock();
}

/// 启动决策日志，追加到 `%TEMP%\wind_installer_args.log`。
///
/// 与安装器 `main.rs` 的 `log_startup` 是同一件事、同一个文件。卸载器此前**一行都不写**：
/// 它是 `windows_subsystem = "windows"` 的无控制台进程，`eprintln!` 等于丢弃；而
/// 「为什么点了静默卸载还是弹出向导」这种问题发生在向导起来之前，那时 `RunLogger`
/// 都还没建，出了事查无可查 —— 那个缺陷正是这么在 ARP 里躺了很久没人发现的。
///
/// 记 `argv` 而不只记解析结果：调用方传了什么、解析成了什么，两边都在，才判得出
/// 到底是调用方写错了参数，还是这边没认出来。
fn log_startup(args: &uninstaller::args::UninstallArgs) {
    util::log::append_startup_line(&format!(
        "uninstall-start: silent={} keep_user_data={} admin={} argv={:?}\n",
        args.silent,
        args.keep_user_data,
        util::admin::is_admin(),
        std::env::args_os().collect::<Vec<_>>(),
    ));
}
