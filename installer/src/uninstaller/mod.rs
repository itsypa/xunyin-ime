pub mod args;
pub mod cleanup;
pub mod plan;
pub mod selfdelete;
pub mod steps;

use std::path::Path;

use cleanup::CleanupOptions;

use crate::installer::step::{run_plan, CliReporter, WarningCollector};
use crate::util::log::RunLogger;
use steps::UninstallCtx;

/// 卸载结果
#[derive(Debug)]
#[allow(dead_code)]
pub struct UninstallResult {
    /// 是否**干干净净**地卸完了。
    ///
    /// 注意这不是「卸载过程有没有跑完」——卸载计划无致命步骤，它总能跑到最后。
    /// 这里为假表示**有产物没清掉**（反注册失败、文件删不动、回执损坏……），
    /// 详情在 [`warnings`](Self::warnings) 与卸载日志里。
    pub success: bool,
    pub message: String,
    pub need_reboot: bool,
    /// 没干成的那些事，逐条原文。为空才等于 `success`。
    pub warnings: Vec<String>,
    /// 卸载日志路径，出问题时唯一能看的东西。
    pub log_path: String,
}

/// 执行完整卸载流程（静默/CLI 路径）。
///
/// GUI 路径见 `ui::uninstall_wizard`——两者共用 `plan::plan_uninstall` 生成的
/// 同一份步骤计划，仅 Reporter 实现不同。
pub fn perform_uninstall(options: &CleanupOptions) -> UninstallResult {
    let mut logger = RunLogger::uninstall();
    logger.log(&format!("安装目录: {:?}", options.install_dir));
    let log_path = logger.path.to_string_lossy().to_string();

    let plan = plan::plan_uninstall();
    let mut reporter = CliReporter;
    let mut collector = WarningCollector::new(&mut reporter);

    let mut ctx = UninstallCtx::new(options);

    let outcome = run_plan(&plan, &mut ctx, &mut collector);
    // 卸载计划无致命步骤：一路尽力而为，删不掉的东西转化为「需要重启」
    let need_reboot = ctx.need_reboot || outcome.as_ref().map(|o| o.need_reboot).unwrap_or(false);

    let mut warnings = collector.into_warnings();
    // 真有致命步骤时（目前一个都没有）也别把它吞掉。
    if let Err(e) = outcome {
        warnings.push(e);
    }

    let _ = crate::installer::registry::clear_installer_running();

    for w in &warnings {
        logger.log_error(w);
    }
    if need_reboot {
        logger.log("部分文件被占用，已排入重启清理队列");
    }
    logger.log(if warnings.is_empty() {
        "=== 卸载完成 ==="
    } else {
        "=== 卸载完成，但有项目未能清除 ==="
    });

    UninstallResult {
        success: warnings.is_empty(),
        message: if warnings.is_empty() {
            "Uninstallation completed".into()
        } else {
            format!("Uninstallation finished with {} problem(s)", warnings.len())
        },
        need_reboot,
        warnings,
        log_path,
    }
}

/// 静默卸载的完整收场：跑一遍卸载，再把结果翻译成**退出码**。
///
/// 两个二进制共用这一份：`wind-installer.exe uninstall --silent` 与
/// `uninstall.exe --silent`（后者才是 ARP `QuietUninstallString` 指向的那个）。
/// 退出码是写在 README 与 AGENTS.md 里的对外契约，分两处各写一遍迟早会分叉——
/// 事实上卸载器那边此前根本没有这段代码，`--silent` 一路跌进 GUI 向导。
///
/// 三条出口，顺序不能动：
/// - **有残留 ≠ 失败**。卸载已经跑完、ARP 条目多半也已移除，调用方重试没有意义；
///   报成非 0 只会让批量部署把它当待重试项反复跑。详情写进卸载日志，这里留一行指路。
/// - **`need_reboot` 的判断不能排在残留之后被挡掉**：3010 是对外承诺，必须可达。
/// - 其余即 0。
pub fn run_silent(keep_user_data: bool) -> i32 {
    use crate::util::exitcode;

    let options = CleanupOptions {
        keep_user_data,
        ..Default::default()
    };
    let result = perform_uninstall(&options);

    if !result.success {
        // ⚠️ 这行**不写 `exit=`**：它先于 need_reboot 判断，此刻还不知道最终退出码。
        // 从前这里硬写 `exit=0`，而真实退出码可能是 3010，日志里两行自相矛盾。
        crate::util::log::append_startup_line(&format!(
            "uninstall: residue={} log={}\n",
            result.warnings.len(),
            result.log_path
        ));
    }

    let code = if result.need_reboot {
        // 无界面模式没有「让用户看到提示再关窗」的余地，退出码是唯一的通道；
        // 而光给一个 3010、不说是哪些文件卡住了，调用方同样无从查起。
        crate::util::log::append_startup_line(&format!(
            "uninstall: exit={} reboot_required {}\n",
            exitcode::REBOOT_REQUIRED,
            crate::util::reboot::pending_summary()
        ));
        exitcode::REBOOT_REQUIRED
    } else {
        exitcode::SUCCESS
    };

    // ⚠️ 自删除这一步不能省，而且**只有静默路径会走到这里**（GUI 是完成页按钮触发）。
    //
    // 不接上的话，`uninstall.exe --silent` 的收尾是两头落空：
    //   1. `delete_install_files` 试着删正在运行的自己 —— Windows 上必失败，`let _ =` 吞掉；
    //   2. `remove_dir_all` 失败 → `schedule_dir_on_reboot`；
    //   3. 遍历撞上自身 exe，走 `is_self` 分支：不删、不改名、**不排队**；
    //   4. 目录删不掉但 `deferred_to_self_delete` 为真 → **也不排队、不记账**。
    // 第 3/4 步是 `reboot.rs` 的「不变量 3」，它的前提是**后面一定有自删除流程**。
    // 静默路径没有，于是安装目录连同 uninstall.exe 永久留在盘上，而账本为空
    // → `need_reboot == false` → 退出 0。调用方拿到的是一次「干净的卸载」。
    //
    // 判据与 GUI 完成页逐字相同：**只有干干净净卸完才自删除**。有产物没清掉时
    // 删掉整个目录是最坏的选择 —— 比如 COM 还注册着而 DLL 已经没了，用户既看不到
    // 残留、也没法再跑一次 uninstall.exe 重试。
    if result.success && is_running_from(&options.install_dir) {
        // 成功时不返回（内部 exit(code)）。返回了就说明副本没起来，日志已记，
        // 此时目录留在盘上是**已知且有记录**的结局，比静默留下好。
        let _ = selfdelete::trigger_self_delete_with_code(&options.install_dir, code);
    }

    code
}

/// 本进程是不是就躺在待删的那个目录里。
///
/// 只有这种情形才需要自删除。`wind-installer.exe uninstall --silent` 通常在别处，
/// 那条路上安装目录已被 `remove_dir_all` 正常收走，再拷一个副本到 `%TEMP%` 纯属多余
/// —— 而「往 %TEMP% 拷 exe 并立刻执行」正是杀毒软件最常见的拦截规则之一，
/// 不需要时就别去踩。
fn is_running_from(install_dir: &Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(parent) = exe.parent() else {
        return false;
    };
    // canonicalize 消除 8.3 短名与大小写差异；任一侧解析不了就退回原样比较。
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    norm(parent) == norm(install_dir)
}

/// 非管理员时的分岔。**两条路都 `exit`，不返回。**
///
/// 静默调用**不弹 UAC**：静默的语义是「不产生任何 UI」，而 UAC 提示框就是 UI。
/// 从前这里无条件走「弹 UAC + `exit(0)`」，于是 `QuietUninstallString` 在未提权的
/// 调用方（计划任务、无人值守部署）手里表现为：安全桌面上冒出一个没人看得见的提权框，
/// 而调用方当场拿到退出码 0 —— 一次连开始都没开始的卸载被记成了成功。
///
/// 交互式那条维持原样：用户就在屏幕前，弹 UAC 正是他期待的。原进程 `exit(0)` 也仍然
/// 合理 —— 真正的卸载由提权后的新进程接手，而人看的是窗口不是退出码。
///
/// ⚠️ **卸载专用，安装那条别照搬。** `wind-installer.exe --silent` 安装未提权时仍是
/// 「弹 UAC + exit(0)」，那是有意的：wind-setting 的应用内自动升级依赖它，
/// `request_elevation` 专门转发原始 argv 就是为它服务的，改成 5 会当场打断升级。
/// 卸载没有这样的调用方 —— 它的静默入口只有 ARP 的 `QuietUninstallString`。
pub fn require_admin_or_exit(silent: bool) -> ! {
    use crate::util::exitcode;
    if silent {
        crate::util::log::append_startup_line(&format!(
            "uninstall: exit={} needs_admin argv={:?}\n",
            exitcode::ACCESS_DENIED,
            std::env::args_os().collect::<Vec<_>>()
        ));
        std::process::exit(exitcode::ACCESS_DENIED);
    }
    crate::util::admin::request_elevation().ok();
    std::process::exit(exitcode::SUCCESS);
}
