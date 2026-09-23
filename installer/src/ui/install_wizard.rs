use std::path::PathBuf;

use windui::app::App;
use windui::core::EventCtx;
use windui::geometry::Color;
use windui::platform::PickDialog;
use windui::signal::signal;
use windui::spec::Align;
use windui::ui::Element;
// 仅无边框标题栏用得到；非 frameless 构建下整段标题栏不存在，导入也不该存在。
#[cfg(feature = "frameless")]
use windui::ui::WindowButtonKind;

use windui::prelude::Sender;

use super::theme;
use crate::installer::step::Reporter;
use crate::installer::InstallMode;
use crate::meta;

const PAGE_CONFIG: usize = 0;
const PAGE_PROGRESS: usize = 1;
const PAGE_FINISH: usize = 2;

/// quiet 模式在置灰的配置页停留多久后自动开始安装。
/// 太短则闪一下看不清装的是什么，太长则拖慢升级。
const QUIET_PREVIEW_MS: u64 = 1000;
/// quiet 模式安装成功后，完成页停留多久自动退出。
const QUIET_FINISH_MS: u64 = 2000;

enum ProgressMsg {
    Status(String),
    /// 总体进度 ∈ [0,1]，由 [`GuiReporter`] 按「已完成步骤 + 步内比例」折算。
    Progress(f32),
    Finished {
        ok: bool,
        /// 成功时为日志路径，失败时为错误信息。
        detail: String,
        /// 有文件被占用、清不掉，需重启系统才能彻底清理。
        /// 这不是失败——新版已就位可正常使用，只是旧文件还赖在盘上。
        need_reboot: bool,
    },
    /// quiet 模式停留片刻后开始安装 —— 由后台线程发出，UI 线程据此切到进度页。
    /// 切页必须回到 UI 线程做（Signal 非 Send），故不能在延迟线程里直接写。
    BeginInstall,
}

/// 把 [`Reporter`] 事件转成 UI 通道消息，并同步写安装日志。
///
/// 进度按步骤折算而非按文件数——旧实现只统计解压文件数，导致注册类步骤全部
/// 挤在 99% 处不动。
struct GuiReporter {
    tx: Sender<ProgressMsg>,
    logger: crate::util::log::RunLogger,
    index: usize,
    total: usize,
}

impl GuiReporter {
    fn emit(&self, fraction_within_step: f32) {
        let overall = (self.index as f32 + fraction_within_step) / self.total.max(1) as f32;
        let _ = self
            .tx
            .send(ProgressMsg::Progress(overall.clamp(0.0, 0.99)));
    }
}

impl Reporter for GuiReporter {
    fn step_begin(&mut self, index: usize, total: usize, name: &str) {
        self.index = index;
        self.total = total;
        self.logger.log(name);
        let _ = self.tx.send(ProgressMsg::Status(name.to_string()));
        self.emit(0.0);
    }

    fn step_progress(&mut self, detail: &str, fraction: f32) {
        self.logger.log(&format!("  {}", detail));
        self.emit(fraction);
    }

    fn log(&mut self, msg: &str) {
        self.logger.log(msg);
    }

    fn warn(&mut self, msg: &str) {
        self.logger.log(&format!("  警告: {}", msg));
    }
}

/// 向导启动选项。
#[derive(Debug, Clone, Default)]
pub struct WizardOptions {
    /// 带界面的静默安装：跳过配置页直接安装，完成后自动退出。
    ///
    /// 用于应用内自动升级 —— 用户已经在设置里点过「立即安装」，不该再被要求
    /// 确认一遍安装路径（升级必须原地进行，路径可改反而会造成双份安装）。
    /// 与完全静默（`--silent`，无任何界面）的区别是仍显示进度，让用户知道在装什么。
    pub quiet: bool,
    /// 预设安装目录；`quiet` 模式下由调用方传入当前安装位置。
    pub install_dir: Option<PathBuf>,
}

/// 后台执行安装计划。向导的「立即安装」按钮与 quiet 模式共用此入口，
/// 确保两条路径跑的是同一套逻辑（此前该逻辑内联在按钮闭包里，无法复用）。
fn spawn_install(
    tx: Sender<ProgressMsg>,
    install_dir: PathBuf,
    data_dir: PathBuf,
    mode: InstallMode,
    is_fresh_install: bool,
) {
    std::thread::spawn(move || run_install_plan(tx, install_dir, data_dir, mode, is_fresh_install));
}

/// 同步执行安装计划（调用方负责放到后台线程）。
///
/// 与 [`spawn_install`] 分开，是为了让 quiet 模式能在**同一个**延迟线程里先 sleep、
/// 再发切页消息、然后接着安装 —— 不必为了延迟而多起一个线程。
fn run_install_plan(
    tx: Sender<ProgressMsg>,
    install_dir: PathBuf,
    data_dir: PathBuf,
    mode: InstallMode,
    is_fresh_install: bool,
) {
    {
        let mut logger = crate::util::log::RunLogger::install();
        logger.log(&format!("安装目录: {:?}", install_dir));
        logger.log(&format!("安装模式: {:?}", mode));
        let log_path = logger.path.to_string_lossy().to_string();

        let config = crate::installer::config::InstallConfig {
            install_dir,
            // 数据目录始终显式传入，config 是步骤读取数据目录的唯一入口
            custom_data_dir: Some(data_dir),
            use_custom_data_dir: true,
            ..Default::default()
        };

        let mut archive = match crate::archive::ArchiveReader::open_current_exe() {
            Ok(a) => a,
            Err(e) => {
                let msg = format!("无法打开安装数据: {}", e);
                logger.log_error(&msg);
                tx.send(ProgressMsg::Finished {
                    ok: false,
                    detail: msg,
                    need_reboot: false,
                })
                .ok();
                return;
            }
        };

        // 续写旧回执而非从空起（新清单删掉的能力其产物仍需可撤销）；
        // 且须先于 plan 声明（plan 类型带 InstallCtx 生命周期）
        let mut receipt = crate::installer::receipt::Receipt::load_or_default();

        // 与静默路径共用同一份计划，仅 Reporter 不同
        let plan = crate::installer::plan::plan_install(crate::meta::manifest(), mode);
        let mut reporter = GuiReporter {
            tx: tx.clone(),
            logger,
            index: 0,
            total: plan.len(),
        };

        let mut ctx = crate::installer::step::InstallCtx {
            config: &config,
            mode,
            is_fresh_install,
            archive: &mut archive,
            receipt: &mut receipt,
        };

        match crate::installer::step::run_plan(&plan, &mut ctx, &mut reporter) {
            Ok(outcome) => {
                if outcome.need_reboot {
                    reporter.log("=== 安装完成（有文件待重启后清理）===");
                } else {
                    reporter.log("=== 安装完成 ===");
                }
                tx.send(ProgressMsg::Finished {
                    ok: true,
                    detail: log_path,
                    need_reboot: outcome.need_reboot,
                })
                .ok();
            }
            Err(e) => {
                // 致命失败：清除标志，否则宿主进程将永久停摆
                let _ = crate::installer::registry::clear_installer_running();
                reporter.log(&format!("=== 安装失败: {} ===", e));
                tx.send(ProgressMsg::Finished {
                    ok: false,
                    detail: e,
                    need_reboot: false,
                })
                .ok();
            }
        }
    }
}

pub fn run_install_wizard(opts: WizardOptions) {
    // ---- 运行期窗口尺寸 / 标题（来自清单）----
    let (win_w, win_h) = meta::install_win();
    let title = meta::window_title();

    // ---- 状态（Signal<T> 是 Copy 句柄，move 闭包自动复制，无需 clone 样板）----
    let is_fresh_install = crate::installer::registry::detect_installed_version().is_none();
    // quiet 模式同样从配置页开始（只是整页置灰不可改），停留片刻再自动进入安装。
    // 直接跳到进度页会让用户来不及看清"在装什么、装到哪"，观感上像是窗口闪了一下。
    let current_page = signal(PAGE_CONFIG);
    let install_mode = signal(0usize);
    let install_dir = signal(
        opts.install_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(default_install_dir),
    );
    let install_dir_portable = signal(default_portable_dir());
    let data_dir = signal(default_data_dir());
    let progress_text = signal(String::from("正在准备安装..."));
    let progress_value = signal(0.0f32);
    let finish_success = signal(false);
    let finish_error = signal(String::new());
    // 装完了但有文件被占用清不掉 —— 完成页据此显示重启提示，
    // 且 quiet 模式据此放弃自动退出（见下方 channel 处理）。
    let finish_reboot = signal(false);
    let config_error = signal(String::new());
    let agreed = signal(false);

    // bool 是 Copy，可被下面的 channel 闭包直接捕获
    let quiet = opts.quiet;

    // ---- 跨线程进度通道（on_message 在 UI 线程调用，可直接写 Signal）----
    let mut app = App::new(title.as_str(), win_w, win_h);
    // windui 0.12 起 on_message 收 `&mut EventCtx`（宿主能力通道：toast / 对话框）。
    // 0.14 起通道回调里的 close / close_forced / window_op 被**丢弃**：通道挂在 App 级、
    // 借哪棵树排空是实现细节，「关掉哪个窗口」本就不确定。要关窗请走窗口自身的交互。
    // 这里只写 Signal 切页，用不上 ctx。
    let tx = app.channel::<ProgressMsg>(move |_ctx, msg| match msg {
        ProgressMsg::Status(s) => progress_text.set(s),
        ProgressMsg::Progress(f) => progress_value.set(f),
        // 延迟结束，切到进度页；安装已在发出此消息的那个线程里继续进行
        ProgressMsg::BeginInstall => current_page.set(PAGE_PROGRESS),
        ProgressMsg::Finished {
            ok,
            detail,
            need_reboot,
        } => {
            progress_value.set(1.0);
            finish_success.set(ok);
            finish_reboot.set(need_reboot);
            if !ok {
                finish_error.set(detail);
            }
            current_page.set(PAGE_FINISH);
            // quiet 模式装完自动退出：先切到完成页停留片刻再关，用户能看到"装完了"
            // 而不是窗口凭空消失。
            //
            // 两种情况**不**自动关，都要求用户亲手关闭：
            // - 失败：否则错误信息一闪而过无从排查；
            // - 需重启：这是唯一告知用户「还有一步要做」的时机。自动升级本就发生在
            //   用户没盯着屏幕的时候，2 秒后自弹自灭等于把提示扔了。
            if quiet && ok && !need_reboot {
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(QUIET_FINISH_MS));
                    std::process::exit(0);
                });
            }
        }
    });

    // 「立即安装」按钮的 move 闭包会拿走 tx 的所有权，quiet 模式的那份须提前复制
    let quiet_tx = tx.clone();

    // ============================================================
    //  PAGE 0：配置页
    // ============================================================
    let page_config = Element::col()
        .fill()
        .weight(1.0)
        .padding_xy(40, 0)
        .spacing(8)
        .visible_when(move || current_page.get() == PAGE_CONFIG)
        // quiet 模式整页置灰不可交互：升级必须原地进行，路径与模式都不允许改动。
        // enabled_when 作用于整个子树，故不必逐个控件禁用（且新增控件自动受控）。
        .enabled_when(move || !quiet)
        // ── 品牌区（Logo + 应用名 + 版本）─────────────────────────
        .child(Element::leaf().weight(1.0))
        .child(
            Element::col()
                .width_match()
                .spacing(6)
                .cross(Align::Center)
                .child(Element::image_bytes(meta::logo()).size(72, 72).corner(18.0))
                .child(
                    Element::label(meta::app_display_name())
                        .width_match()
                        .font_size(22.0)
                        .fg(Color::hex(theme::text_primary()))
                        .text_align(Align::Center),
                )
                .child(
                    Element::label(meta::app_version())
                        .width_match()
                        .font_size(12.0)
                        .fg(Color::hex(theme::text_muted()))
                        .text_align(Align::Center),
                ),
        )
        .child(Element::leaf().height(10))
        // 安装目录（行高固定 34，内容随模式切换，避免布局抖动）
        .child(
            Element::row()
                .width_match()
                .height(34)
                .spacing(8)
                .cross(Align::Center)
                .child(
                    Element::label("安装目录")
                        .width(64)
                        .font_size(12.0)
                        .fg(Color::hex(theme::text_secondary())),
                )
                .child(
                    Element::text_input(install_dir, "安装路径")
                        .weight(1.0)
                        .height(34)
                        .visible_when(move || install_mode.get() == 0),
                )
                .child(
                    Element::button("更改")
                        .height(34)
                        .visible_when(move || install_mode.get() == 0)
                        .on_click(move |ctx: &mut EventCtx| {
                            ctx.request_pick_folder(
                                folder_pick_dialog("选择安装目录", &install_dir.get()),
                                move |path| {
                                    if let Some(path) = path {
                                        install_dir.set(
                                            finalize_picked_folder(path)
                                                .to_string_lossy()
                                                .to_string(),
                                        );
                                    }
                                },
                            );
                        }),
                )
                .child(
                    Element::text_input(install_dir_portable, "安装路径")
                        .weight(1.0)
                        .height(34)
                        .visible_when(move || install_mode.get() == 1),
                )
                .child(
                    Element::button("更改")
                        .height(34)
                        .visible_when(move || install_mode.get() == 1)
                        .on_click(move |ctx: &mut EventCtx| {
                            ctx.request_pick_folder(
                                folder_pick_dialog("选择安装目录", &install_dir_portable.get()),
                                move |path| {
                                    if let Some(path) = path {
                                        install_dir_portable.set(
                                            finalize_picked_folder(path)
                                                .to_string_lossy()
                                                .to_string(),
                                        );
                                    }
                                },
                            );
                        }),
                ),
        )
        // 数据目录（行高固定 34；便捷模式显示说明文字）
        .child(
            Element::row()
                .width_match()
                .height(34)
                .spacing(8)
                .cross(Align::Center)
                .child(
                    Element::label("数据目录")
                        .width(64)
                        .font_size(12.0)
                        .fg(Color::hex(theme::text_secondary())),
                )
                .child(
                    Element::text_input(data_dir, meta::s_data_dir_hint())
                        .weight(1.0)
                        .height(34)
                        // 0.12 的 `enabled` 直接收 bool，不必再为一个常量分配信号槽
                        // （信号槽当前不回收，旧写法每次构建都占一个永不释放的位置）。
                        .enabled(is_fresh_install)
                        .visible_when(move || install_mode.get() == 0),
                )
                .child(
                    Element::button("更改")
                        .height(34)
                        .enabled(is_fresh_install)
                        .visible_when(move || install_mode.get() == 0)
                        .on_click(move |ctx: &mut EventCtx| {
                            ctx.request_pick_folder(
                                folder_pick_dialog("选择数据目录", &data_dir.get()),
                                move |path| {
                                    if let Some(path) = path {
                                        data_dir.set(
                                            finalize_picked_folder(path)
                                                .to_string_lossy()
                                                .to_string(),
                                        );
                                    }
                                },
                            );
                        }),
                )
                .child(
                    Element::label("便捷模式不配置数据目录")
                        .weight(1.0)
                        .font_size(12.0)
                        .fg(Color::hex(theme::text_muted()))
                        .visible_when(move || install_mode.get() == 1),
                ),
        )
        // 安装模式
        .child(
            Element::col()
                .width_match()
                .spacing(5)
                .child(
                    Element::row()
                        .width_match()
                        .spacing(16)
                        .child(Element::radio("标准安装（推荐）", install_mode, 0))
                        .child(Element::radio("便捷模式", install_mode, 1)),
                )
                .child(
                    Element::label(meta::s_mode_hint())
                        .font_size(11.0)
                        .fg(Color::hex(theme::text_muted()))
                        .width_match(),
                ),
        )
        // 用户协议
        .child(
            Element::row()
                .cross(Align::Center)
                .spacing(4)
                .child(Element::checkbox("我已阅读并同意", agreed))
                .child(if meta::agreement_url().is_empty() {
                    Element::label(meta::s_agreement_text())
                        .font_size(13.0)
                        .fg(Color::hex(theme::text_secondary()))
                } else {
                    Element::link(meta::s_agreement_text()).url(meta::agreement_url())
                }),
        )
        // 校验错误提示（仅未勾协议时可见）
        .child(
            Element::label_signal(config_error)
                .font_size(11.0)
                .fg(Color::hex(theme::error()))
                .align(Align::Center),
        )
        // 安装按钮（align=Center 使其在父 col 中水平居中）
        .child(
            Element::button("立即安装")
                .width(300)
                .height(48)
                .corner(8.0)
                .bg(Color::hex(theme::accent()))
                .fg(Color::hex(0xFFFFFF))
                .align(Align::Center)
                .enabled_signal(agreed)
                .on_click(move |_ctx: &mut EventCtx| {
                    config_error.set(String::new());
                    progress_value.set(0.0);
                    current_page.set(PAGE_PROGRESS);

                    let install_dir_val = if install_mode.get() == 0 {
                        expand_env_path(&install_dir.get())
                    } else {
                        expand_env_path(&install_dir_portable.get())
                    };
                    let data_dir_val = expand_env_path(&data_dir.get());
                    let mode = if install_mode.get() == 0 {
                        InstallMode::Standard
                    } else {
                        InstallMode::Portable
                    };

                    spawn_install(
                        tx.clone(),
                        install_dir_val,
                        data_dir_val,
                        mode,
                        is_fresh_install,
                    );
                }),
        )
        .child(Element::leaf().weight(1.0));

    // ============================================================
    //  PAGE 1：进度页
    // ============================================================
    let page_progress = Element::col()
        .fill()
        .weight(1.0)
        .padding_xy(56, 0)
        .visible_when(move || current_page.get() == PAGE_PROGRESS)
        .child(Element::leaf().weight(1.0))
        .child(
            Element::col()
                .width_match()
                .spacing(10)
                .cross(Align::Center)
                .child(
                    Element::label("正在安装中，请稍候...")
                        .font_size(14.0)
                        .fg(Color::hex(theme::text_primary())),
                )
                .child(
                    Element::progress(progress_value)
                        .width_match()
                        .height(6)
                        .corner(3.0),
                )
                .child(
                    Element::label_signal(progress_text)
                        .font_size(12.0)
                        .fg(Color::hex(theme::text_secondary()))
                        .width_match()
                        .text_align(Align::Center),
                ),
        )
        .child(Element::leaf().weight(1.0));

    // ============================================================
    //  PAGE 2：完成页
    // ============================================================
    let page_finish = Element::col()
        .fill()
        .weight(1.0)
        .padding_xy(48, 0)
        .visible_when(move || current_page.get() == PAGE_FINISH)
        .child(Element::leaf().weight(1.0))
        // 安装成功
        .child(
            Element::col()
                .width_match()
                .spacing(8)
                .cross(Align::Center)
                .visible_when(move || finish_success.get())
                .child(
                    Element::label("✓")
                        .font_size(44.0)
                        .fg(Color::hex(theme::success())),
                )
                .child(
                    Element::label("安装完成")
                        .font_size(18.0)
                        .fg(Color::hex(theme::text_primary())),
                )
                .child(
                    Element::label(format!(
                        "{} 已准备就绪，可以开始使用",
                        meta::app_display_name()
                    ))
                    .font_size(13.0)
                    .fg(Color::hex(theme::text_secondary()))
                    .width_match()
                    .text_align(Align::Center),
                ),
        )
        // 装成功了，但有旧文件被占用清不掉。
        //
        // 用 warning 而非 error 色：新版文件已全部就位、注册也完成了，程序现在就能用，
        // 只是旧版残留（多半是仍被 ctfmon 加载的 DLL）要等重启才能从盘上抹掉。
        // 用红色会让用户以为装失败了而去重装——重装解决不了任何问题。
        //
        // 只提示、不代劳重启：安装器无从判断用户手头有没有没保存的工作。
        .child(
            Element::col()
                .width_match()
                .spacing(4)
                .cross(Align::Center)
                .visible_when(move || finish_success.get() && finish_reboot.get())
                .child(Element::leaf().height(10))
                .child(
                    Element::label("部分旧版文件仍被占用，建议重启电脑完成清理")
                        .font_size(12.0)
                        .fg(Color::hex(theme::warning()))
                        .width_match()
                        .text_align(Align::Center),
                )
                .child(
                    Element::label("不影响现在使用；重启后系统会自动清除这些残留")
                        .font_size(11.0)
                        .fg(Color::hex(theme::text_muted()))
                        .width_match()
                        .text_align(Align::Center),
                ),
        )
        // 安装失败
        .child(
            Element::col()
                .width_match()
                .spacing(8)
                .cross(Align::Center)
                .visible_when(move || !finish_success.get())
                .child(
                    Element::label("✗")
                        .font_size(44.0)
                        .fg(Color::hex(theme::error())),
                )
                .child(
                    Element::label("安装失败")
                        .font_size(18.0)
                        .fg(Color::hex(theme::text_primary())),
                )
                .child(
                    Element::label_signal(finish_error)
                        .font_size(12.0)
                        .fg(Color::hex(theme::error()))
                        .width_match()
                        .text_align(Align::Center),
                ),
        )
        .child(Element::leaf().weight(1.0))
        // 完成按钮
        .child(
            Element::button("完 成")
                .width(200)
                .height(48)
                .corner(8.0)
                .bg(Color::hex(theme::accent()))
                .fg(Color::hex(0xFFFFFF))
                .align(Align::Center)
                .on_click(move |_ctx: &mut EventCtx| {
                    std::process::exit(0);
                }),
        )
        .child(Element::leaf().height(20));

    // ============================================================
    //  无边框自定义标题栏：浅色，与窗口背景同色，左侧标题右侧按钮
    // ============================================================
    #[cfg(feature = "frameless")]
    let title_bar = Element::row()
        .width_match()
        .height(36)
        .cross(Align::Center)
        .bg(Color::hex(theme::bg_primary()))
        .window_drag()
        .child(Element::leaf().width(14))
        .child(
            Element::label(title.clone())
                .font_size(12.0)
                .fg(Color::hex(theme::text_secondary())),
        )
        .child(Element::leaf().weight(1.0))
        .child(
            Element::window_button(WindowButtonKind::Minimize)
                .fg(Color::hex(theme::text_secondary())),
        )
        .child(
            Element::window_button(WindowButtonKind::Close).fg(Color::hex(theme::text_secondary())),
        );

    // ============================================================
    //  组装根节点
    // ============================================================
    let root = Element::col()
        .size(win_w, win_h)
        .bg(Color::hex(theme::bg_primary()));

    #[cfg(feature = "frameless")]
    let root = root.child(title_bar);

    let root = root
        .child(page_config)
        .child(page_progress)
        .child(page_finish);

    let app = app
        .centered()
        .resizable(false)
        .renderer(super::renderer())
        .bg(Color::hex(theme::bg_primary()))
        .content(root);

    #[cfg(feature = "frameless")]
    let app = app.frameless();

    // quiet 模式无人点「立即安装」，这里代为发起：先让置灰的配置页停留片刻，
    // 用户得以看清应用名、版本与安装目录，再自动转入进度页开始安装。
    //
    // 放在 run() 之前：channel 的 pump 已注册，线程此刻发来的消息不会丢失。
    // 模式固定 Standard —— 便携安装由调用方（wind-setting）拦下走完整向导，
    // 不会走到这里（无人值守升级会把便携改造成标准安装）。
    if quiet {
        let dir = expand_env_path(&install_dir.get());
        let data = expand_env_path(&data_dir.get());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(QUIET_PREVIEW_MS));
            // 先切页再装：消息按序处理，进度页必定先于第一条进度就位
            quiet_tx.send(ProgressMsg::BeginInstall).ok();
            run_install_plan(quiet_tx, dir, data, InstallMode::Standard, is_fresh_install);
        });
    }

    app.run();
}

fn default_install_dir() -> String {
    use winreg::enums::*;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key_path = format!(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{}",
        crate::meta::app_display_name()
    );
    if let Ok(key) = hklm.open_subkey_with_flags(&key_path, KEY_READ) {
        if let Ok(dir) = key.get_value::<String, _>("InstallLocation") {
            if !dir.is_empty() {
                return dir;
            }
        }
    }
    crate::meta::default_install_path()
}

fn default_portable_dir() -> String {
    crate::meta::default_portable_path()
}

/// 数据目录初值：优先显示 `datadir.conf` 里**本机实际生效**的路径。
///
/// 这是「二次安装不允许改数据目录」的另一半——控件禁用只保证改不了，值仍来自清单
/// 模板；不读回真实路径，界面就会把一个装在 D 盘的数据目录显示成 `%APPDATA%\{id}`。
///
/// conf 缺失时回退到清单默认：此时本次安装若非首装也不会写 conf（见 `WriteDataDirConf`），
/// 主程序读端同样回退到这个默认位置，二者一致。
///
/// `pub` 是为了让集成测试直接钉住「界面显示什么」——控件禁用与初值取值是两件事，
/// 只测底层读函数漏得掉「读了但没接到界面上」这类回归。
pub fn default_data_dir() -> String {
    if let Some(dir) = crate::installer::userdata::read_datadir_conf() {
        return dir.to_string_lossy().to_string();
    }
    crate::meta::default_data_path()
}

/// 展开路径中的 %VAR% 环境变量占位符。
fn expand_env_path(s: &str) -> std::path::PathBuf {
    let mut result = s.to_string();
    for (var, fallback) in &[
        ("ProgramFiles", r"C:\Program Files"),
        ("APPDATA", r"C:\Users\Default\AppData\Roaming"),
        ("LOCALAPPDATA", r"C:\Users\Default\AppData\Local"),
        ("USERPROFILE", r"C:\Users\Default"),
    ] {
        let token = format!("%{}%", var);
        if result.contains(&token) {
            let val = std::env::var(var).unwrap_or_else(|_| fallback.to_string());
            result = result.replace(&token, &val);
        }
    }
    std::path::PathBuf::from(result)
}

/// 构建选目录用的 `PickDialog`：起始目录取当前配置路径（`%VAR%` 展开后）最近的
/// 已存在祖先目录，而不是让 Shell 用它自己记住的上次访问位置——那个位置完全不
/// 可控（可能是网络共享/已拔出的移动盘/云盘同步目录），既是"偶发打开卡顿"的
/// 诱因之一，也会让"更改"按钮的默认落点跟用户已经填好的路径对不上。
fn folder_pick_dialog(title: &str, current_path: &str) -> PickDialog {
    let mut dialog = PickDialog::new().title(title);
    let expanded = expand_env_path(current_path);
    let mut probe = Some(expanded.as_path());
    while let Some(p) = probe {
        if p.exists() {
            dialog = dialog.directory(p);
            break;
        }
        probe = p.parent();
    }
    dialog
}

/// 用户选完目录后的收尾：若选的目录名不是 APP_ID，自动追加子目录。
fn finalize_picked_folder(path: PathBuf) -> PathBuf {
    if path
        .file_name()
        .map(|n| n != crate::meta::app_id())
        .unwrap_or(true)
    {
        path.join(crate::meta::app_id())
    } else {
        path
    }
}
