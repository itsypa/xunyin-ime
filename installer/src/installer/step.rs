//! 步骤抽象 —— 编排的唯一真相，安装与卸载共用。
//!
//! 历史上 GUI 路径与静默路径各自手写了一遍步骤链（安装、卸载各一对），已漂移出缺陷：
//! 静默安装漏写 datadir.conf，GUI 安装不设 InstallerRunning 标志。现在四条路径共用
//! [`run_plan`]，差异收敛到两处：
//! - **装什么** → `plan::plan_install` / `uninstaller::plan::plan_uninstall`
//! - **怎么汇报** → [`Reporter`]（GUI 传 channel 实现，CLI 传 stderr 实现）
//!
//! [`Step`] 对上下文泛型：安装用 [`InstallCtx`]，卸载用 `uninstaller::steps::UninstallCtx`。
//! 两者的执行语义（失败是否致命、进度如何汇报）完全一致，故 [`run_plan`] 只需一份。

use crate::archive::ArchiveReader;
use crate::util::reboot;

use super::config::InstallConfig;
use super::receipt::Receipt;
use super::InstallMode;

/// 安装步骤的上下文。
pub struct InstallCtx<'a> {
    pub config: &'a InstallConfig,
    pub mode: InstallMode,
    /// 是否首次安装（非升级）——决定是否写入用户数据目录配置。
    pub is_fresh_install: bool,
    /// 自身归档，供解压与卸载器 overlay 追加使用。
    pub archive: &'a mut ArchiveReader,
    /// 步骤把做成的产物记在这里，卸载时反向回放。
    pub receipt: &'a mut Receipt,
}

/// 一个安装/卸载动作。
///
/// 失败语义由 [`Step::fatal`] 决定：致命步骤（如解压）失败即中止，
/// 非致命步骤（注册类）失败仅记警告并继续。
pub trait Step<C> {
    /// 进度文案，如「正在注册 COM 组件...」。
    fn name(&self) -> String;

    fn run(&self, ctx: &mut C, reporter: &mut dyn Reporter) -> Result<(), String>;

    /// 失败是否应中止整个流程。默认否（仅警告）。
    fn fatal(&self) -> bool {
        false
    }

    /// 本步骤失败是否意味着需要重启才能完全生效。
    fn needs_reboot_on_failure(&self) -> bool {
        false
    }
}

/// 进度与日志出口。GUI / CLI 各自实现。
pub trait Reporter {
    /// 第 `index` 个步骤（共 `total` 个）开始执行。
    fn step_begin(&mut self, index: usize, total: usize, name: &str);
    /// 步骤内细粒度进度：`detail` 为当前子项，`fraction` ∈ [0,1] 为步骤内完成比例。
    fn step_progress(&mut self, detail: &str, fraction: f32);
    /// 普通日志，不影响进度。
    fn log(&mut self, msg: &str);
    /// 警告：非致命步骤失败。
    fn warn(&mut self, msg: &str);
}

/// 把经过的每一条警告记下来，其余原样转发给内层 Reporter。
///
/// 为什么需要它：卸载计划**没有致命步骤**（一路尽力而为，一个删不掉的注册表值不该
/// 让剩下的产物全部残留），于是 [`run_plan`] 对卸载永远返回 `Ok`——「有哪一步没干成」
/// 这件事**只存在于 warn 里**。而 `CliReporter` 与 GUI 的 Reporter 都把 warn 写进
/// `eprintln!`，两个二进制又都是 `windows_subsystem = "windows"`、没有控制台，
/// 于是那些话进了一个不存在的句柄，调用方只好一律汇报「卸载成功」。
///
/// 它同时收两个来源，这是用装饰器而不是在 `run_plan` 里加计数的原因：
/// - `run_plan` 因非致命步骤失败发出的 warn；
/// - 步骤**内部**自己发的 warn（`UndoReceipt` 逐条撤销的失败就是这么报的，
///   那一步本身永远返回 `Ok`）。
pub struct WarningCollector<'r> {
    inner: &'r mut dyn Reporter,
    warnings: Vec<String>,
}

impl<'r> WarningCollector<'r> {
    pub fn new(inner: &'r mut dyn Reporter) -> Self {
        Self {
            inner,
            warnings: Vec::new(),
        }
    }

    pub fn into_warnings(self) -> Vec<String> {
        self.warnings
    }
}

impl Reporter for WarningCollector<'_> {
    fn step_begin(&mut self, index: usize, total: usize, name: &str) {
        self.inner.step_begin(index, total, name);
    }
    fn step_progress(&mut self, detail: &str, fraction: f32) {
        self.inner.step_progress(detail, fraction);
    }
    fn log(&mut self, msg: &str) {
        self.inner.log(msg);
    }
    fn warn(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
        self.inner.warn(msg);
    }
}

/// 计划执行结果。
pub struct RunOutcome {
    pub need_reboot: bool,
}

/// 按序执行步骤计划。致命步骤失败立即返回 Err，非致命失败记警告后继续。
///
/// `need_reboot` 由两个来源合并而成：
/// 1. **步骤失败**且 [`Step::needs_reboot_on_failure`]（如 COM 注册失败）；
/// 2. **步骤成功但留下了锁定文件**——升级时旧 DLL 被占用，解压走「改名让路 +
///    排重启删除」后步骤是成功返回的，这条信息只存在于 [`reboot`] 账本里。
///
/// 第 2 条是主路径：带锁升级几乎总是走它，而它此前完全没有回传通道。
pub fn run_plan<C>(
    plan: &[Box<dyn Step<C>>],
    ctx: &mut C,
    reporter: &mut dyn Reporter,
) -> Result<RunOutcome, String> {
    let total = plan.len();
    let mut need_reboot = false;

    for (i, step) in plan.iter().enumerate() {
        let name = step.name();
        reporter.step_begin(i, total, &name);

        if let Err(e) = step.run(ctx, reporter) {
            if step.fatal() {
                return Err(e);
            }
            if step.needs_reboot_on_failure() {
                need_reboot = true;
            }
            reporter.warn(&format!("{}: {}", name, e));
        }
    }

    // 收尾处一次性读账本：各步骤无需（也无从）自行汇报锁定文件。
    if reboot::is_reboot_pending() {
        need_reboot = true;
        reporter.log(&format!("待重启清理：{}", reboot::pending_summary()));
        for item in reboot::pending_items() {
            reporter.log(&format!(
                "  {} {:?}",
                if item.scheduled {
                    "已排队"
                } else {
                    "未排队"
                },
                item.path
            ));
        }
    }

    Ok(RunOutcome { need_reboot })
}

/// 静默/CLI 路径的 Reporter：步骤名与警告写 stderr，细粒度进度丢弃。
pub struct CliReporter;

impl Reporter for CliReporter {
    fn step_begin(&mut self, index: usize, total: usize, name: &str) {
        eprintln!("[{}/{}] {}", index + 1, total, name);
    }
    fn step_progress(&mut self, _detail: &str, _fraction: f32) {}
    fn log(&mut self, msg: &str) {
        eprintln!("  {}", msg);
    }
    fn warn(&mut self, msg: &str) {
        eprintln!("Warning: {}", msg);
    }
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod warning_collector_tests {
    use super::*;

    // 变异检验已做：
    //   · WarningCollector::warn 不再记录（只转发）→ 两条 collects_* 全红
    //   · run_plan 把非致命失败改成静默跳过（不调 warn）→ collects_non_fatal_step_failures 红
    //   · WarningCollector 不转发给内层 → forwards_everything_inward 红

    struct Spy {
        seen: Vec<String>,
    }

    impl Reporter for Spy {
        fn step_begin(&mut self, index: usize, total: usize, name: &str) {
            self.seen.push(format!("begin {index}/{total} {name}"));
        }
        fn step_progress(&mut self, detail: &str, _fraction: f32) {
            self.seen.push(format!("progress {detail}"));
        }
        fn log(&mut self, msg: &str) {
            self.seen.push(format!("log {msg}"));
        }
        fn warn(&mut self, msg: &str) {
            self.seen.push(format!("warn {msg}"));
        }
    }

    /// 一步「自己失败」的非致命步骤 —— run_plan 会替它发 warn。
    struct FailingStep;
    impl Step<()> for FailingStep {
        fn name(&self) -> String {
            "会失败的一步".into()
        }
        fn run(&self, _ctx: &mut (), _r: &mut dyn Reporter) -> Result<(), String> {
            Err("删不掉".into())
        }
    }

    /// 一步「自己返回 Ok 但内部报警告」的步骤 —— UndoReceipt 逐条撤销就是这个形状。
    struct WarnsInsideStep;
    impl Step<()> for WarnsInsideStep {
        fn name(&self) -> String {
            "内部报警的一步".into()
        }
        fn run(&self, _ctx: &mut (), r: &mut dyn Reporter) -> Result<(), String> {
            r.warn("撤销失败 [反注册 COM]");
            Ok(())
        }
    }

    struct CleanStep;
    impl Step<()> for CleanStep {
        fn name(&self) -> String {
            "干净的一步".into()
        }
        fn run(&self, _ctx: &mut (), _r: &mut dyn Reporter) -> Result<(), String> {
            Ok(())
        }
    }

    fn collect(plan: Vec<Box<dyn Step<()>>>) -> (Vec<String>, bool) {
        let mut spy = Spy { seen: Vec::new() };
        let mut collector = WarningCollector::new(&mut spy);
        let outcome = run_plan(&plan, &mut (), &mut collector);
        (collector.into_warnings(), outcome.is_ok())
    }

    #[test]
    fn clean_run_collects_nothing() {
        let (warnings, ok) = collect(vec![Box::new(CleanStep)]);
        assert!(ok);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn collects_non_fatal_step_failures() {
        // 非致命步骤失败时 run_plan 返回 Ok —— 卸载正是全靠这个「一路尽力而为」。
        // 所以 Ok 不等于干净，得看警告。
        let (warnings, ok) = collect(vec![Box::new(FailingStep)]);
        assert!(ok, "非致命失败不该中止计划");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("删不掉"), "{warnings:?}");
    }

    #[test]
    fn collects_warnings_raised_inside_a_step() {
        // UndoReceipt 那种形状：步骤自己返回 Ok，失败只从内部 warn 出来。
        // 只在 run_plan 里计数是抓不到它的 —— 这正是用装饰器的理由。
        let (warnings, ok) = collect(vec![Box::new(WarnsInsideStep)]);
        assert!(ok);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("反注册 COM"), "{warnings:?}");
    }

    #[test]
    fn collects_both_sources_in_order() {
        let (warnings, _) = collect(vec![
            Box::new(WarnsInsideStep),
            Box::new(CleanStep),
            Box::new(FailingStep),
        ]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("反注册 COM"), "{warnings:?}");
        assert!(warnings[1].contains("删不掉"), "{warnings:?}");
    }

    #[test]
    fn forwards_everything_inward() {
        // 收集不能把事件吞掉：进度与警告照样要到达真正的 Reporter（界面/日志）。
        let mut spy = Spy { seen: Vec::new() };
        {
            let mut collector = WarningCollector::new(&mut spy);
            let plan: Vec<Box<dyn Step<()>>> =
                vec![Box::new(WarnsInsideStep), Box::new(FailingStep)];
            let _ = run_plan(&plan, &mut (), &mut collector);
        }
        assert!(
            spy.seen.iter().any(|e| e.starts_with("begin 0/2")),
            "{:?}",
            spy.seen
        );
        assert_eq!(
            spy.seen.iter().filter(|e| e.starts_with("warn ")).count(),
            2,
            "两条警告都该转发给内层: {:?}",
            spy.seen
        );
    }
}
