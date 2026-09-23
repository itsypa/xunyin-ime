//! 卸载收尾：安装目录里最后剩下的是**正在运行的 uninstall.exe 自己**，它删不掉自己。
//!
//! 做法是把自身复制到 `%TEMP%`，让副本等原进程退出后删掉整个安装目录，再抹掉自己。
//!
//! ⚠️ 这条路是安装目录的**唯一**归宿：`reboot::schedule_dir_on_reboot` 的不变量 3
//! 刻意不碰正在运行的自身 exe（改名会让 `current_exe()` 失准、排队原路径又会在重装后
//! 删掉新的 uninstall.exe），明确把它让给这里。所以这里失败就没有第二道防线 ——
//! 安装目录永远留在盘上，而用户已经被告知卸载完成。本模块的重试与重启兜底就是为此。
//!
//! ⚠️ 但这条兜底对用户是**静默**的：副本跑起来时完成页早已关闭，它那本重启账本是
//! 进程局部的、没有任何人读，所以副本删不掉时用户什么提示都拿不到 —— 唯一的痕迹是
//! 启动日志与重启队列。别把「有兜底」读成「出事会告诉用户」。

use std::ffi::{OsStr, OsString};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::meta;
use crate::util::reboot;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 等原进程退出的上限。到点就往下走 —— 它多半已经退了，只是句柄拿不到。
///
/// 取 5 秒而不是更长：父进程 spawn 之后只剩 `std::process::exit(0)`（不 unwind、
/// 不跑析构、GUI 窗口不做拆除），量级是毫秒，5 秒已是几百倍余量，到点后还有 3.1 秒
/// 退避垫底。等太久有实打实的代价 —— PID 可能已被复用，这段时间里我们等的是个不相干
/// 的进程，而「卸载完立刻重装到同一目录」的窗口会被整段拉宽，窗口内副本
/// `remove_dir_all` 删掉的是用户**刚装好**的新目录。
const PARENT_WAIT_MS: u32 = 5_000;

/// 拿不到父进程 PID 时的老做法：盲等一段时间。
///
/// 只为兼容「副本由旧版本启动」这一种情形（命令行里没有第三个参数）。盲等正是本次
/// 要修掉的东西：等短了原进程还没退、目录删不掉，等长了纯粹让用户多等。
const BLIND_WAIT_MS: u64 = 800;

const FLAG: &str = "--self-delete";

/// 检查是否在自删除模式下运行
pub fn is_self_delete_mode() -> bool {
    std::env::args_os().any(|a| a == OsStr::new(FLAG))
}

/// 获取自删除目标目录（`--self-delete <dir> <pid>` 后面的参数）
pub fn self_delete_target() -> Option<PathBuf> {
    self_delete_args().0
}

/// 命令行的**写端**：`--self-delete <dir> <pid>`。
///
/// 与 [`parse_self_delete_args`] 成对出现，两端都从这里取形状 —— 各写各的必然漂移，
/// 而漂移的后果很难看：两个参数对调后副本会把 `"2628"` 当成目标目录，
/// 那个相对路径不存在，`remove_dir_with_retry` 于是提前返回「删掉了」，
/// 日志写下「安装目录已删除」，而安装目录完好无损地留在盘上。
fn self_delete_argv(dir: &Path, pid: u32) -> Vec<OsString> {
    vec![
        OsString::from(FLAG),
        dir.as_os_str().to_os_string(),
        OsString::from(pid.to_string()),
    ]
}

/// 命令行的**读端**。参数由调用方传入而不是就地读 `args()`，这样测试能直接喂给它 ——
/// 手抄一份「纯逻辑副本」来测的话，改真函数不会有任何测试变红。
fn parse_self_delete_args<S: AsRef<OsStr>>(args: &[S]) -> (Option<PathBuf>, Option<u32>) {
    let Some(pos) = args.iter().position(|a| a.as_ref() == OsStr::new(FLAG)) else {
        return (None, None);
    };
    let dir = args.get(pos + 1).map(|s| PathBuf::from(s.as_ref()));
    // PID 是后加的，旧版本启动的副本没有它，故为 `Option`；不是数字同样当没给 ——
    // 拿一个不相干的数字去等进程，最坏是白等到超时。
    let pid = args
        .get(pos + 2)
        .and_then(|s| s.as_ref().to_str())
        .and_then(|s| s.parse::<u32>().ok());
    (dir, pid)
}

/// `--self-delete` 后面的两个参数：目标目录与原进程 PID。
fn self_delete_args() -> (Option<PathBuf>, Option<u32>) {
    parse_self_delete_args(&std::env::args_os().collect::<Vec<_>>())
}

/// 卸载完成后调用：将自身复制到 %TEMP%，以 `--self-delete <install_dir> <pid>` 启动副本，
/// 然后立即退出当前进程（不返回）。副本负责删除安装目录并自我清除。
pub fn trigger_self_delete(install_dir: &Path) -> Result<(), String> {
    trigger_self_delete_with_code(install_dir, 0)
}

/// 同上，但指定本进程的退出码。
///
/// 静默路径需要它：那条路上「装成功了但请重启」要靠 **3010** 交给调用方，而
/// 原先这里写死 `exit(0)` —— 自删除一触发，退出码就被抹成 0，调用方再也看不到
/// 该重启这回事。GUI 那条没有这个问题（它有完成页可以显示提示），所以
/// `trigger_self_delete` 保持原样。
pub fn trigger_self_delete_with_code(install_dir: &Path, exit_code: i32) -> Result<(), String> {
    // ⚠️ 这一侧的每个失败都必须落盘。调用方是 `let _ = trigger_self_delete(&dir);`
    // 紧接 exit(0)，而本函数成功时根本不返回 —— 返回值只在失败时有意义，却没人看。
    // 最现实的失败是往 %TEMP% 拷一个 exe 并立刻执行：那是杀毒软件最常见的拦截规则之一。
    // 撞上时后果与本模块要修的缺陷一字不差：安装目录留在盘上、完成页已说卸载完成、
    // 没有提示。而父进程这一侧**排重启兜底也够不着** —— 不变量 3 会跳过正在运行的
    // uninstall.exe，随后只排下一条对非空目录无效的队列条目。所以日志是仅有的线索。
    let fail = |msg: String| -> Result<(), String> {
        log(&format!("self-delete: 触发失败 —— {msg}"));
        Err(msg)
    };

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return fail(format!("获取自身路径失败: {}", e)),
    };

    // 后缀取随机数而非 PID 的函数：同一个 PID 必得同一个文件名，上次的副本没删干净
    // （del 早了、或被杀毒软件锁着）而这次 PID 又撞上，fs::copy 就会失败。
    let temp_exe = std::env::temp_dir().join(format!(
        "{}_uninst_{:08x}.exe",
        meta::app_id().to_lowercase(),
        rand::random::<u32>()
    ));

    if let Err(e) = std::fs::copy(&current_exe, &temp_exe) {
        return fail(format!("复制到临时目录失败 {:?}: {}", temp_exe, e));
    }

    // 把自己的 PID 交给副本：让它等**这个进程真的退出**，而不是盲等一段时间。
    if let Err(e) = Command::new(&temp_exe)
        .args(self_delete_argv(install_dir, std::process::id()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        return fail(format!("启动清理进程失败 {:?}: {}", temp_exe, e));
    }

    log(&format!(
        "self-delete: 已启动清理副本 {:?}，目标 {}",
        temp_exe,
        install_dir.display()
    ));
    std::process::exit(exit_code);
}

/// 自删除模式执行体（在 %TEMP% 副本中运行）：
/// 等原进程退出 → 删除安装目录（重试 + 重启兜底）→ 用 cmd 延时删除自身。
pub fn execute_self_delete(install_dir: &Path) {
    let (_, parent) = self_delete_args();
    log(&format!(
        "self-delete: 开始 dir={} parent={:?}",
        install_dir.display(),
        parent
    ));

    wait_for_parent(parent);

    if remove_dir_with_retry(install_dir, std::thread::sleep) {
        log("self-delete: 安装目录已删除");
    } else {
        // 最后一道防线。没有它，安装目录就永远留在盘上 —— 而完成页已经告诉用户
        // 卸载完成了。schedule_dir_on_reboot 自底向上逐项处理，能当场删的当场删。
        log("self-delete: 安装目录删不掉，转排重启清理");
        reboot::schedule_dir_on_reboot(install_dir);
    }

    schedule_self_removal();
}

/// 等原 uninstall.exe 退出。
///
/// 从前这里是 `sleep(800ms)` 的盲等：赌赢了皆大欢喜，赌输了 `remove_dir_all` 撞上
/// 仍被占用的 uninstall.exe 而失败，且失败被 `let _ =` 吞掉 —— 安装目录留在盘上，
/// 没有重试、没有提示、没有日志。GUI 关窗慢、盘慢、杀毒软件扫一下都够输这一把。
fn wait_for_parent(pid: Option<u32>) {
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    let Some(pid) = pid else {
        std::thread::sleep(Duration::from_millis(BLIND_WAIT_MS));
        return;
    };

    // SAFETY: pid 只是个整数（OpenProcess 对任意值都安全，失败即 Err）；
    // handle 在同一作用域内开与关。
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            // 打不开多半是它已经退了 —— 那正是我们等的结果。
            return;
        };
        // 「父进程秒退」和「等满超时」在日志里长得一模一样的话，那条慢路径就没法诊断。
        if WaitForSingleObject(handle, PARENT_WAIT_MS) == WAIT_TIMEOUT {
            log(&format!(
                "self-delete: 等待原进程 {pid} 退出超时（{PARENT_WAIT_MS}ms），继续"
            ));
        }
        let _ = CloseHandle(handle);
    }
}

/// 删目录，删不掉就退避重试。全部失败返回 false（调用方负责兜底）。
///
/// `sleep` 由调用方注入，测试才不必真的等上几秒。
fn remove_dir_with_retry(dir: &Path, sleep: impl Fn(Duration)) -> bool {
    // 退避总时长约 3.1 秒。要防的是「原进程刚退、句柄还没被系统回收」和杀毒软件
    // 扫一遍这类**短暂**占用；长期占用重试多久都没用，交给重启兜底。
    const BACKOFF_MS: [u64; 5] = [100, 200, 400, 800, 1600];

    for delay in BACKOFF_MS {
        if is_gone(dir) {
            return true;
        }
        if std::fs::remove_dir_all(dir).is_ok() {
            return true;
        }
        sleep(Duration::from_millis(delay));
    }
    // 末尾这次尝试不是多余的：没有它，最后那 1600ms 退避就是白睡的
    // （睡完不再试就直接放弃）。整体是 6 次尝试 / 5 次退避 / 约 3.1 秒。
    is_gone(dir) || std::fs::remove_dir_all(dir).is_ok()
}

/// 目录是否**确实**不在了。
///
/// 不用 `Path::exists()`：它在权限错误、路径过长等情形下也返回 `false`，于是
/// 「读不到」会被当成「已删掉」—— 那正是本函数最不该出的谎报（对上游就是
/// 「安装目录已删除」的日志 + 不排重启兜底，而目录还在盘上）。
/// 只有明确的 NotFound 才算数，其余一律当作「还在，继续试」。
fn is_gone(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => false,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// 用 cmd 延时删除临时副本自身（无窗口）。
///
/// `ping` 当计时器是这条路的老写法，本次原样保留、未做验证。
/// （`timeout /t` 看着更直白，但它在 stdin 被重定向时会失败，而副本这里 stdin 是什么
/// 状态没有查证过 —— 没验过的理由不该写进注释当依据。）
///
/// 删不掉的后果只是 %TEMP% 里留个 exe，不影响卸载结果，故**不**往重启队列里加条目：
/// 那是个全局共享的值，每次卸载都加一条不划算。
fn schedule_self_removal() {
    let Ok(self_exe) = std::env::current_exe() else {
        return;
    };
    let cmd = format!(
        "ping 127.0.0.1 -n 3 >nul & del /f /q \"{}\"",
        self_exe.display()
    );
    let _ = Command::new("cmd")
        .args(["/c", &cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// 自删除副本是 GUI 子系统进程、没有控制台，出事时这行日志是唯一的线索。
///
/// ⚠️ 写的是启动日志（固定文件名），**不能**用 `RunLogger::uninstall()`：那个的文件名
/// 取自 `meta::app_id()`，而副本走的是 `--self-delete` 分支、根本不 bootstrap 清单，
/// 一调就 panic —— release profile 是 `panic = "abort"` + GUI 子系统，那是一次无提示
/// 崩溃，安装目录随之留在盘上。本文件里其余的 `meta::` 调用都只在 `trigger_self_delete`
/// 那一侧（它跑在已 bootstrap 的原进程里）。
fn log(line: &str) {
    crate::util::log::append_startup_line(&format!("{}\n", line));
}

// 以下为测试，须置于文件末尾：`#[cfg(test)] mod` 在非测试编译下整块消失，
// 把真实代码排在它后面会让人误以为文件到此为止。
#[cfg(test)]
mod self_delete_tests {
    use super::*;
    use std::cell::RefCell;

    // 变异检验已做：
    //   · remove_dir_with_retry 只试一次（去掉重试）→ retries_until_the_lock_goes_away 变红
    //   · 重试用完仍失败却返回 true → gives_up_and_reports_failure 变红
    //   · 去掉循环里的「已经不在」提前判断 → absent_dir_counts_as_removed 变红
    //     （返回值有末尾那次尝试兜着，变红的是「白退避三秒」这条；只断言返回值抓不到它）
    //   · 读端 parse_self_delete_args 的 pos+1/pos+2 写反 → what_we_send_is_what_we_parse 变红
    //   · 读端 PID 不校验数字 → parses_the_command_line_it_is_given 变红
    //   · 写端 self_delete_argv 把 dir 与 pid 对调 → what_we_send_is_what_we_parse 变红
    //
    // 本模块**没有**测试守着的四处，一并写在这里 —— 没有守卫这件事本身也要写下来，
    // 否则下一个人看到一片绿，又会把「判据全绿」读成「整条路验过了」：
    //   · `wait_for_parent` 全部：要真进程才测得动，只能靠代码审查。
    //   · `execute_self_delete` 的分支走向（哪条写日志、哪条转排重启）：可以靠注入
    //     `remove_dir_with_retry` 测，本次没做。
    //   · `is_gone` 的非 NotFound 分支：造不出「存在但 stat 失败」的目录。
    //   · `trigger_self_delete` 的失败落盘：靠结构而非测试 —— `fail` 是那个函数唯一的
    //     失败出口，三条早退全走它，要绕过去得主动另写一条 return。
    //
    // ⚠️ 做变异检验时，替换文本**必须在文件里唯一**，命中多处一律视为无效。
    // 这不是洁癖：本文件早先有一份「读端规则的纯逻辑副本」供测试调用，它与真函数里
    // 那句 `args.get(pos + 2).and_then(|s| s.parse::<u32>().ok())` 逐字相同，于是一次
    // 替换把两处一起改了 —— 测试如期变红，而变红的原因是副本被改了，关于产品代码
    // 什么也没证明。那种假绿比不做变异检验更糟。副本现已删除。

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wind_selfdel_{}_{}", std::process::id(), tag))
    }

    #[test]
    fn absent_dir_counts_as_removed() {
        // 目录已经不在 = 目的达成：不该报失败去白排一条重启清理，也不该在那儿退避
        // 等上三秒 —— 自删除副本跑在用户点完「完成」之后，白等就是白等。
        let dir = temp_dir("absent");
        let _ = std::fs::remove_dir_all(&dir);

        let slept = RefCell::new(0u32);
        assert!(remove_dir_with_retry(&dir, |_| *slept.borrow_mut() += 1));
        assert_eq!(*slept.borrow(), 0, "目录本就不在，一次退避都不该有");
    }

    #[test]
    fn removes_a_plain_dir_without_sleeping() {
        let dir = temp_dir("plain");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("f.bin"), b"x").unwrap();

        let slept = RefCell::new(0u32);
        assert!(remove_dir_with_retry(&dir, |_| *slept.borrow_mut() += 1));
        assert!(!dir.exists());
        assert_eq!(*slept.borrow(), 0, "一把就删掉的目录不该退避等待");
    }

    /// 占用是**短暂**的：第 2 次退避时释放。这正是要防的形态 —— 原进程刚退出、
    /// 句柄还没被系统回收，或杀毒软件正扫这个目录。
    #[test]
    fn retries_until_the_lock_goes_away() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_dir("transient");
        std::fs::create_dir_all(&dir).unwrap();
        let held = dir.join("held.bin");
        std::fs::write(&held, b"x").unwrap();
        let guard = RefCell::new(Some(
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0) // 不共享删除 —— 目录删不掉
                .open(&held)
                .expect("独占打开失败"),
        ));

        let rounds = RefCell::new(0u32);
        let ok = remove_dir_with_retry(&dir, |_| {
            let mut n = rounds.borrow_mut();
            *n += 1;
            if *n == 2 {
                *guard.borrow_mut() = None; // 第 2 轮退避时释放占用
            }
        });

        assert!(ok, "占用释放后应当删得掉");
        assert!(!dir.exists());
        assert!(*rounds.borrow() >= 2, "至少该退避重试到占用释放");
    }

    /// 占用**不会**释放：重试用完必须如实返回 false，让调用方去排重启兜底。
    /// 这里返回 true 的话，安装目录就此永远留在盘上而没有任何人知道。
    #[test]
    fn gives_up_and_reports_failure() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = temp_dir("stuck");
        std::fs::create_dir_all(&dir).unwrap();
        let held = dir.join("held.bin");
        std::fs::write(&held, b"x").unwrap();
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .expect("独占打开失败");

        assert!(
            !remove_dir_with_retry(&dir, |_| {}),
            "删不掉却报成功 = 安装目录永远留在盘上且无人知道"
        );

        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_the_command_line_it_is_given() {
        // 直接喂给真函数，不再手抄一份「纯逻辑副本」—— 副本只能证明它自己自洽，
        // 真函数的 pos+1 / pos+2 写反了、数字校验去掉了，副本都不会变红。
        assert_eq!(
            parse_self_delete_args(&["x.exe", FLAG, r"C:\App", "2628"]),
            (Some(PathBuf::from(r"C:\App")), Some(2628))
        );
        // 旧版本启动的副本没有第三个参数，回落盲等。
        assert_eq!(
            parse_self_delete_args(&["x.exe", FLAG, r"C:\App"]),
            (Some(PathBuf::from(r"C:\App")), None)
        );
        assert_eq!(parse_self_delete_args(&["x.exe"]), (None, None));
        // 不是数字就当没给，别拿它去等一个不相干的进程。
        assert_eq!(
            parse_self_delete_args(&["x.exe", FLAG, r"C:\App", "abc"]),
            (Some(PathBuf::from(r"C:\App")), None)
        );
    }

    /// 写端与读端必须对得上。
    ///
    /// 只测读端是不够的：把 `self_delete_argv` 里两个 `.arg()` 对调，读端的测试照样全绿，
    /// 而副本会把 `"2628"` 当成目标目录 —— 那个相对路径不存在，`remove_dir_with_retry`
    /// 于是走 `is_gone` 提前返回 true，日志写下「安装目录已删除」。
    /// 安装目录完好留在盘上，日志谎报成功，没有任何人会知道。
    ///
    /// ⚠️ 覆盖边界：这条只钉**写端与读端的形状**。两端都在 `OsStr` 域内、整条链路
    /// 一次编码边界都没跨，所以它对任何路径恒成立（ASCII、中文、非法 UTF-16 一样），
    /// 加几个花哨路径的用例不会提高它的上限。它**不覆盖** `CreateProcessW` ↔
    /// `GetCommandLineW` 那道真正的传参边界 —— 那里的风险不是字符集（全程 UTF-16），
    /// 而是引号转义（带空格要加引号、路径以反斜杠结尾时收尾反斜杠要翻倍），
    /// 由 std 的参数转义保证，本仓未验证。别把这条读成「参数传递已验证」。
    #[test]
    fn what_we_send_is_what_we_parse() {
        let dir = PathBuf::from(r"C:\Program Files\Demo App");
        let argv = self_delete_argv(&dir, 2628);

        // 真实调用里 argv 前面还有 exe 自身，这里照样摆一个，确保解析靠的是标志位
        // 而不是固定下标。
        let mut full = vec![OsString::from("uninst.exe")];
        full.extend(argv);

        assert_eq!(parse_self_delete_args(&full), (Some(dir), Some(2628)));
    }
}
