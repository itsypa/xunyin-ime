#![windows_subsystem = "windows"]

use std::path::PathBuf;

use clap::Parser;

mod archive;
mod installer;
mod manifest;
mod meta;
mod ui;
mod uninstaller;
mod util;

use util::exitcode;

/// Wind Installer - 轻量级 Windows 安装管理器
#[derive(Parser, Debug)]
#[command(name = "wind-installer")]
#[command(about = "Lightweight manifest-driven Windows installer")]
// 容忍未知参数：未来版本的安装器可能向已安装的旧版（卸载器）传入新 flag，
// 旧版不应因不认识的参数而报错退出，只解析自己认识的、忽略其余。
#[command(ignore_errors = true)]
struct Args {
    /// 运行模式
    #[command(subcommand)]
    mode: Option<Mode>,

    /// 完全静默安装（无任何界面）
    #[arg(long)]
    silent: bool,

    /// 带界面的静默安装：跳过配置页直接安装，显示进度，完成后自动退出。
    /// 应用内自动升级用这个 —— 用户需要看到进度，但不该被要求再确认一次安装路径。
    #[arg(long)]
    quiet: bool,

    /// 安装目录
    #[arg(long)]
    dir: Option<PathBuf>,

    /// 数据目录
    #[arg(long)]
    datadir: Option<PathBuf>,

    /// 保留用户数据（卸载时）
    #[arg(long)]
    keep_user_data: bool,

    /// 强制软渲染（禁用 Direct2D 硬件加速）；等效于设置环境变量 WIND_SOFT_RENDER=1
    #[arg(long)]
    soft_render: bool,
}

#[derive(Parser, Debug)]
enum Mode {
    /// 安装模式
    Install,
    /// 卸载模式
    Uninstall,
}

fn main() {
    let args = Args::parse();

    // 检测运行模式
    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_default();

    match args.mode {
        Some(Mode::Install) | None => {
            if exe_name.contains("uninstall") {
                run_uninstall(args);
            } else {
                run_install(args);
            }
        }
        Some(Mode::Uninstall) => {
            run_uninstall(args);
        }
    }
}

/// 启动诊断：把解析到的参数与提权状态追加到 %TEMP%\wind_installer_args.log。
///
/// 安装器是 `windows_subsystem = "windows"` 的 GUI 进程，没有控制台，`eprintln!` 的
/// 输出无处可见。排查「为何没有静默安装」时，这个文件是唯一能看到真相的地方 ——
/// 尤其能暴露 `request_elevation` 重启自身后参数丢失：日志里会出现两条记录，
/// 第一条 `silent=true admin=false`，第二条 `silent=false admin=true`。
fn log_startup(tag: &str, args: &Args) {
    util::log::append_startup_line(&format!(
        "{tag}: silent={} dir={:?} datadir={:?} admin={} argv={:?}\n",
        args.silent,
        args.dir,
        args.datadir,
        util::admin::is_admin(),
        std::env::args_os().collect::<Vec<_>>(),
    ));
}

/// 记录「以 3010 退出」的原因。
///
/// 静默模式退出码非 0 时，调用方唯一能查的就是这个文件；不落盘的话，
/// 用户只会看到「安装器返回了 3010」而无从知道是哪些文件卡住了。
fn log_reboot_required(tag: &str) {
    util::log::append_startup_line(&format!(
        "{tag}: exit={} reboot_required {}\n",
        exitcode::REBOOT_REQUIRED,
        util::reboot::pending_summary()
    ));
}

/// 运行安装
fn run_install(args: Args) {
    log_startup("install", &args);
    // 载入清单（来自自身追加的归档）
    if let Err(e) = meta::bootstrap() {
        eprintln!("无法载入安装清单: {}", e);
        std::process::exit(1);
    }

    // 检查管理员权限
    if !util::admin::is_admin() {
        util::admin::request_elevation().ok();
        std::process::exit(0);
    }

    // 检查 64 位系统
    if !util::path::is_64bit_system() {
        // TODO: 显示错误对话框
        std::process::exit(1);
    }

    // 检查单实例。
    //
    // ⚠️ 必须排在 bootstrap 与提权**之后**：被挡住时要报出应用名，清单没载入就会
    // panic（release 是 panic=abort + GUI 子系统，那是一次无提示的崩溃）；而提权前
    // 的这个进程只做「重启自身」一件事，让它持锁只会让提权后的自己被自己挡住。
    if let Some(pid) = util::single::another_instance_pid() {
        util::single::report_busy_and_exit(pid, args.silent);
    }

    if args.silent {
        let mut config = installer::config::InstallConfig::default();
        if let Some(dir) = args.dir {
            config.install_dir = dir;
        }
        if let Some(datadir) = args.datadir {
            config.custom_data_dir = Some(datadir);
            config.use_custom_data_dir = true;
        }

        let result = installer::perform_install(&config, installer::InstallMode::Standard);
        if !result.success {
            // 隔壁 need_reboot 那条一直配着 release_lock，这条从前没有 —— 持锁退出
            // 会在 %TEMP% 留下一个锁文件。它不会挡住下一次（主人已死，`lock_blocks`
            // 判 alive=false 就放行），但那是兜底在替它擦屁股，不是本该如此。
            util::single::release_lock();
            std::process::exit(exitcode::FAILURE);
        }
        if result.need_reboot {
            // 无界面模式没有「让用户看到提示再关闭」的余地，只能靠退出码传信。
            log_reboot_required("install");
            util::single::release_lock();
            std::process::exit(exitcode::REBOOT_REQUIRED);
        }
    } else {
        ui::install_wizard::run_install_wizard(ui::install_wizard::WizardOptions {
            quiet: args.quiet,
            install_dir: args.dir,
        });
    }

    util::single::release_lock();
}

/// 运行卸载
fn run_uninstall(args: Args) {
    log_startup("uninstall", &args);
    // 与 `uninstall.exe` 共用同一份收场：静默卸载未提权时报 5，不弹 UAC。
    // 这两条是同一个功能的两个实现，提权判定分成两套就已经分叉过一次
    // ——文档写着「`--silent` 未提权返回 5」，而这边照旧弹 UAC + exit(0)。
    if !util::admin::is_admin() {
        uninstaller::require_admin_or_exit(args.silent);
    }

    if uninstaller::selfdelete::is_self_delete_mode() {
        if let Some(dir) = uninstaller::selfdelete::self_delete_target() {
            uninstaller::selfdelete::execute_self_delete(&dir);
        }
        return;
    }

    // 载入清单（来自安装目录的 .manifest）
    if let Err(e) = meta::bootstrap() {
        eprintln!("无法载入卸载清单: {}", e);
        std::process::exit(1);
    }

    // 检查单实例——排在 bootstrap 之后，理由见 run_install 里同一处的注释。
    if let Some(pid) = util::single::another_instance_pid() {
        util::single::report_busy_and_exit(pid, args.silent);
    }

    if args.silent {
        // 退出码的三条出口（残留不算失败 / 3010 必须可达 / 其余为 0）都在
        // `uninstaller::run_silent` 里，与 `uninstall.exe --silent` 共用同一份——
        // 那边才是 ARP `QuietUninstallString` 真正调起的二进制。
        let code = uninstaller::run_silent(args.keep_user_data);
        util::single::release_lock();
        std::process::exit(code);
    }

    ui::uninstall_wizard::run_uninstall_wizard();
    util::single::release_lock();
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod arg_tests {
    use super::*;

    /// 应用内自动升级传入的正是这组参数；解析失败会静默退化成交互式向导
    /// （`ignore_errors = true` 让 clap 返回全默认值，silent 变回 false）。
    #[test]
    fn silent_install_with_quoted_dir() {
        let args = Args::parse_from([
            "wind-installer",
            "--silent",
            "--dir",
            r"C:\Program Files\Demo App",
        ]);
        assert!(args.silent, "--silent 未被识别");
        assert_eq!(args.dir, Some(PathBuf::from(r"C:\Program Files\Demo App")));
    }

    #[test]
    fn silent_alone() {
        let args = Args::parse_from(["wind-installer", "--silent"]);
        assert!(args.silent, "--silent 单独传入也未被识别");
    }

    /// 未知参数应被忽略而不影响已知参数 —— 这是 ignore_errors 的本意。
    #[test]
    fn unknown_flag_does_not_swallow_known_ones() {
        let args = Args::parse_from(["wind-installer", "--silent", "--future-flag"]);
        assert!(args.silent, "未知参数把 --silent 一起吞掉了");
    }
}
