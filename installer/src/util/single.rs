//! 安装器单实例锁（`%TEMP%\wind_installer.lock`）。
//!
//! ── 这个锁出过什么事 ──────────────────────────────────────────────────
//! 锁文件里原本只存一个 PID。Windows 会复用 PID，所以一个早已死去的安装器留下的锁，
//! 只要它那个 PID 正被别的进程占着，就会被读成「另一个实例在跑」—— 而且没有任何
//! 年龄兜底能把它放出来。后果是**后续所有安装与卸载都被永久挡死**，一次都跑不起来。
//!
//! 更糟的是从前挡住时的收场是 `exit(0)`：静默、无任何提示、退出码还在告诉调用方
//! 「成功了」。用户看到的是「双击安装程序没反应」，静默安装的脚本则会把「什么都
//! 没做」记成一次成功的安装。2026-09-14 在编译机上实测撞到过这一条。
//!
//! 这与 `InstallerRunning` 注册表标记是同一个形态的缺陷（见 registry.rs 与
//! wind_tsf/include/InstallerGuard.h），修法也同构：**记下主人的身份，并给一条出口。**
//!
//! ── 判据 ────────────────────────────────────────────────────────────
//! 锁文件写 `"<pid>|<进程创建时间 FILETIME>"`。PID 可以重复，`(PID, 创建时间)` 不会。
//! 旧版本写的锁只有 `"<pid>"`，认不出身份，那条路回落到「锁文件超过 24 小时即遗物」。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

// `ERROR_INSTALL_ALREADY_RUNNING`(1618)。定义在 `util::exitcode`：退出码是对外契约，
// 集中一处，免得两个二进制的 main 里各长出一套。
use crate::util::exitcode::ALREADY_RUNNING as EXIT_ALREADY_RUNNING;

/// 全局单实例标志
static INSTANCE_RUNNING: AtomicBool = AtomicBool::new(false);

/// 锁文件多久算遗物。
///
/// 只在**拿不到主人身份**时才用得上（旧版本写的锁）—— 有身份时 `(pid, 创建时间)`
/// 已经给得出确定答案，一次跨天的慢安装不该被年龄误伤。
///
/// 取 24 小时：任何真实的安装/卸载都远在其内，它要防的不是慢安装，而是把
/// 「永久挡死」变成「有界挡死」。与 InstallerGuard 的硬兜底同一个量级、同一个理由。
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// 另一个实例是否正持锁。
///
/// 返回 `Some(pid)` 表示确有实例在跑（pid 供提示与排查用），此时**本进程没有拿到锁**；
/// 返回 `None` 表示锁已归本进程所有，可以继续。
///
/// ⚠️ 两条本函数**做不到**的事，别把它当成比实际更强的保证：
///
/// 1. **这是 check-then-act，不是互斥。** 两个实例同时启动、都读到「无锁」、再都去写，
///    谁都挡不住谁。真正的互斥要靠具名互斥体（`CreateMutexW(L"Global\\…")` +
///    `ERROR_ALREADY_EXISTS`）；锁文件只负责**识别遗物**，那才是它这次被修的原因。
///    并跑还会级联：两个实例里先结束的那个 `release_lock` 会把后者的锁一并删掉，
///    于是第三个进来时看到的是空地。
///    现在真正挡住并发的其实是 **UAC 的串行化**（安全桌面同一时刻只显示一个提权
///    提示），所以这个窗口平时够不着；**`EnableLUA=0` 或以内置 Administrator 运行时
///    没有这道串行化**，两次快速双击才是真正碰得到它的场景。日后上互斥体时从这里入手。
/// 2. **作用域是 per-user，不是 machine-wide。** 锁在 `%TEMP%`，标准用户输入管理员
///    凭据提权时，两边的 `%TEMP%` 根本不是同一个文件。
///
/// 调用点须排在清单 bootstrap 与提权**之后**，理由见 `report_busy_and_exit` 与
/// main.rs 里的注释。
pub fn another_instance_pid() -> Option<u32> {
    acquire(&lock_file_path(), SystemTime::now())
}

/// 释放单实例锁
pub fn release_lock() {
    release(&lock_file_path());
}

/// [`another_instance_pid`] 的本体，路径与时钟由调用方给 —— 否则整条接线
/// （判定 → 抢锁 → 置位）没有任何测试够得着。
fn acquire(path: &Path, now: SystemTime) -> Option<u32> {
    if let Some(state) = read_lock(path, now) {
        if lock_blocks(state.has_identity, state.alive, state.stale_by_age) {
            return Some(state.pid);
        }
    }

    let _ = std::fs::write(path, current_stamp());
    INSTANCE_RUNNING.store(true, Ordering::SeqCst);

    None
}

/// [`release_lock`] 的本体。只删本进程真正拿到过的锁 —— 没拿到就动手，
/// 删的是别人正拿着的那把。
fn release(path: &Path) {
    if INSTANCE_RUNNING.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(path);
        INSTANCE_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 被另一个实例挡住时的收场：留下可查的记录，非静默时再弹个框，然后以 1618 退出。
///
/// 三件事一件都不能少 —— 从前这里是个光秃秃的 `exit(0)`：
/// 用户只看到「双击没反应」，脚本只看到「退出码 0 = 装好了」，
/// 而真相（是哪个 PID 占着、锁文件在哪）一点痕迹都不留。
pub fn report_busy_and_exit(pid: u32, silent: bool) -> ! {
    let lock = lock_file_path();
    let detail = format!(
        "另一个安装程序实例正在运行（进程 {}），本次未做任何改动。\r\n\r\n\
         若确认没有安装程序在运行，删除下面这个文件后重试：\r\n{}",
        pid,
        lock.display()
    );

    crate::util::log::append_startup_line(&busy_log_line(pid, &lock));

    if !silent {
        show_warning(&busy_title(), &detail);
    }

    std::process::exit(EXIT_ALREADY_RUNNING);
}

/// 提示框的标题。
///
/// ⚠️ 这里**不能**用会 panic 的 `meta::app_display_name()`：[`report_busy_and_exit`]
/// 可能跑在清单 bootstrap 之前，而 release profile 是 `panic = "abort"` + GUI 子系统
/// —— 收场代码自己崩掉，换来的是一次无提示 abort，退出码也不再是 1618，框根本弹不出来。
/// 而 `UninstallString` 指的正是卸载器那个二进制，那条路就是「控制面板点卸载」。
///
/// 单独抽出来是为了**测得住这件事**：`report_busy_and_exit` 返回 `!`，没法直接测，
/// 换回会 panic 的访问器不会有任何断言察觉。
fn busy_title() -> String {
    match crate::meta::try_app_display_name() {
        Some(name) => format!("{} 安装程序", name),
        None => "安装程序".to_string(),
    }
}

/// 被挡住时写进启动日志的那一行。
///
/// 抽出来是为了钉得住：静默模式下调用方只看得到一个退出码，这行日志是**唯一**
/// 能告诉人「是谁占着锁、锁在哪」的东西，格式写坏了不会有任何地方报错。
fn busy_log_line(pid: u32, lock: &Path) -> String {
    format!(
        "aborted: exit={} another_instance_pid={} lock={}\n",
        EXIT_ALREADY_RUNNING,
        pid,
        lock.display()
    )
}

fn lock_file_path() -> PathBuf {
    std::env::temp_dir().join("wind_installer.lock")
}

/// 锁文件读出来的实况。
struct LockState {
    pid: u32,
    /// 锁里是否记了创建时间（旧版本写的锁没有）
    has_identity: bool,
    /// 主人进程是否仍在（有身份时还要求创建时间吻合）
    alive: bool,
    stale_by_age: bool,
}

/// 读锁文件并当场判定实况。
///
/// `now` 由调用方传入而不是在这里取 —— 「过了 24 小时的锁要放行」是本次修复的核心
/// 承诺，测试得能把时间拨过去验它，否则这条兜底只能靠人眼看。
fn read_lock(path: &Path, now: SystemTime) -> Option<LockState> {
    let content = std::fs::read_to_string(path).ok()?;
    let (pid, create_time) = parse_stamp(content.trim())?;

    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let stale_by_age = stale_verdict(mtime, now);

    let alive = match create_time {
        Some(ct) => process_matches(pid, ct),
        None => is_process_alive(pid),
    };

    Some(LockState {
        pid,
        has_identity: create_time.is_some(),
        alive,
        stale_by_age,
    })
}

/// 锁是否有效地挡住本进程。
///
/// - `has_identity` 锁里记了创建时间
/// - `alive`        主人进程是否仍在（有身份时还要求创建时间吻合）
/// - `stale_by_age` 锁文件已超过 [`STALE_AFTER`]
fn lock_blocks(has_identity: bool, alive: bool, stale_by_age: bool) -> bool {
    // 主人没了 —— 不管哪种格式，这都是遗物。
    if !alive {
        return false;
    }
    // 身份吻合且还活着：真有一个实例在跑，挡住它是这个锁的正当用途。
    if has_identity {
        return true;
    }
    // 旧版本写的锁：那个 PID 活着，但认不出到底是不是当年立锁的那个进程
    // （PID 被复用就会长这样）。只能看年龄——这条是「永久挡死」唯一的出口。
    !stale_by_age
}

/// 解析锁文件内容。
///
/// - `"2628|133712345678901234"` → 新格式，带身份
/// - `"2628"`                    → 旧版本写的，只有 PID
/// - 其它一律 `None`（认不出来的锁不该有发言权，调用方会直接抢锁覆盖它）
fn parse_stamp(s: &str) -> Option<(u32, Option<u64>)> {
    let (pid_s, ft_s) = match s.split_once('|') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    // 不用 trim / 不接受正负号：这个格式是本程序自己写下的，任何偏离都该判为「不认识」。
    if pid_s.is_empty() || !pid_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = pid_s.parse().ok()?;
    // PID 0 是 System Idle Process，永远「活着」——认了它，锁就永久挡死。
    if pid == 0 {
        return None;
    }
    match ft_s {
        None => Some((pid, None)),
        Some(ft_s) => {
            if ft_s.is_empty() || !ft_s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((pid, Some(ft_s.parse::<u64>().ok()?)))
        }
    }
}

/// 年龄这一路的最终判定：拿不到修改时间时判为遗物。
///
/// 取不到修改时间 = 判不出它**不是**遗物 → 放行。这一支只在「没有身份可查」的那条
/// 路上起作用，而那条路正是本次缺陷的原产地：旧格式锁 + PID 被复用 + 年龄又判不出来，
/// 反手就是同一个没有出口的永久挡死。本模块的取向是「拿不准就当没有实例」——
/// 挡错是永久挡死，放错最多让两个安装器并跑一次。
/// （TSF 读端的取向与此相反，别把那边的话抄过来。）
///
/// 单独抽出来是为了测得到：`None` 那一支在正常 NTFS 卷上几乎造不出来
/// （文件都读成功了，`metadata().modified()` 还失败），塞在 `read_lock` 里就是一段
/// 没有任何断言守着的代码。
fn stale_verdict(mtime: Option<SystemTime>, now: SystemTime) -> bool {
    match mtime {
        Some(m) => is_stale(m, now),
        None => true,
    }
}

/// 锁文件距今是否已超过 [`STALE_AFTER`]。
///
/// 锁文件的时间比现在还晚时**按幅度分**，这是刻意的：
/// - 小幅（NTP 校正、夏令时、文件系统时间戳精度）→ 按新鲜处理。此时另一个安装器
///   很可能真的在跑，放第二个进来才是坏事。
/// - 大幅（超过 [`STALE_AFTER`]：虚拟机快照回滚、时钟曾被设到未来、锁文件是从别的
///   机器拷来的）→ 这个 mtime 是垃圾数据，判不出它不是遗物，按本模块的取向放行。
///   一刀切当「新鲜」的话，一个未来时间戳的锁会把后续安装永久挡死。
fn is_stale(modified: SystemTime, now: SystemTime) -> bool {
    match now.duration_since(modified) {
        Ok(elapsed) => elapsed > STALE_AFTER,
        Err(skew) => skew.duration() > STALE_AFTER,
    }
}

/// 本进程的锁戳：`"<pid>|<创建时间>"`，取不到创建时间时退回只写 PID。
///
/// 格式与注册表里的 `InstallerRunningOwner` 一致（见 registry.rs），但两者是各自
/// 独立的约定：那个是跨仓、跨语言给 TSF DLL 读的，这个只在本程序内部读写。
fn current_stamp() -> String {
    match current_create_time() {
        Some(ft) => format!("{}|{}", std::process::id(), ft),
        None => std::process::id().to_string(),
    }
}

fn filetime_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

fn current_create_time() -> Option<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: 四个出参都是本栈上的有效对象；GetCurrentProcess 返回伪句柄，无需关闭。
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    Some(filetime_to_u64(creation))
}

/// 通过 OpenProcess + GetExitCodeProcess 判断 PID 是否仍在运行，无子进程/窗口。
///
/// `STILL_ACTIVE` 是 259，所以一个**真以 259 退出**的进程会被判成还活着。
/// `WaitForSingleObject(handle, 0)` 没有这个歧义，但它要 `SYNCHRONIZE` 访问权，
/// 而 `PROCESS_QUERY_LIMITED_INFORMATION` 不含它 —— 为了消一个够不着的歧义去多要
/// 一项权限，换来的是在权限受限时连「它还在不在」都答不出来。本程序的退出码只有
/// 0 / 1 / 1618 / 3010，撞不上 259，故维持现状。
///
/// 也别打 `GetProcessTimes` 的 `lpExitTime` 的主意（看着像能不加权限就消掉歧义）：
/// MSDN 写明进程**未退出**时那个结构的内容是 undefined，那不是一条可用的路。
fn is_process_alive(pid: u32) -> bool {
    with_process(pid, |handle| unsafe {
        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(handle, &mut exit_code);
        exit_code == 259 // STILL_ACTIVE
    })
    .unwrap_or(false)
}

/// PID 仍在运行**且**创建时间吻合。
///
/// 后半句才是 PID 复用的解药：PID 还在，但已经是别的进程了，创建时间对不上。
fn process_matches(pid: u32, create_time: u64) -> bool {
    with_process(pid, |handle| unsafe {
        let mut exit_code: u32 = 0;
        let _ = GetExitCodeProcess(handle, &mut exit_code);
        if exit_code != 259 {
            return false;
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).is_err() {
            // 打得开却读不到时间，认不出是不是它 —— 判为「不是」。
            // 与 TSF 那端相反的取向是故意的：那边放行会让安装期失去保护，
            // 这边放行最多让两个安装器并跑一次，而挡错则是永久挡死。
            return false;
        }
        filetime_to_u64(creation) == create_time
    })
    .unwrap_or(false)
}

/// 打开进程、跑一段判定、收好句柄。打不开一律 `None`（当作「不在」）。
///
/// ⚠️ `f` 若 unwind 则句柄泄漏。release profile 是 `panic = "abort"`，那里不存在
/// 这条路；`cargo test` 用的 dev profile 会 unwind，但传进来的都是不会 panic 的判定。
/// 要往 `f` 里塞会 panic 的东西，先把这里改成 RAII 守卫。
fn with_process<T>(pid: u32, f: impl FnOnce(HANDLE) -> T) -> Option<T> {
    // SAFETY: pid 只是个整数，OpenProcess 对任意值都是安全的（失败即 Err）。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let result = f(handle);
    // SAFETY: handle 来自上面那次成功的 OpenProcess，此处是它唯一的一次关闭。
    let _ = unsafe { CloseHandle(handle) };
    Some(result)
}

fn show_warning(title: &str, text: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };

    let title = HSTRING::from(title);
    let text = HSTRING::from(text);
    // SAFETY: 两个字符串在调用期间都活着；无父窗口传 None。
    unsafe {
        MessageBoxW(
            None,
            &text,
            &title,
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

/// 程序退出时清理（Ctrl-C）
#[allow(dead_code)]
pub fn setup_cleanup_on_exit() {
    ctrlc::set_handler(move || {
        release_lock();
        std::process::exit(0);
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    // 变异检验已做，逐条对应「把兜底去掉会怎样」：
    //   · lock_blocks 改回「见锁即挡」→ stale_old_format_lock_releases / dead_owner_releases 变红
    //   · lock_blocks 让旧格式也无条件挡 → stale_old_format_lock_releases 变红
    //   · parse_stamp 不校验 pid != 0 → zero_pid_rejected 变红
    //   · parse_stamp 用 trim + 接受符号 → sloppy_input_rejected 变红
    //   · STALE_AFTER 改大改小 → stale_boundary_is_24h 变红
    //   · process_matches 去掉创建时间比对 → own_process_matches_its_own_identity 变红
    //   · is_stale 把大幅未来时间戳一刀切当「新鲜」→ far_future_mtime_is_stale 变红
    //   · stale_verdict 的 None 支退回「挡住」→ unreadable_mtime_releases 变红
    //   · acquire 抢锁时写回旧格式(只写 pid) → acquire_then_block_then_release 变红
    //   · release 不删文件 / 不清 INSTANCE_RUNNING → 同上变红
    //   · 退出码或日志格式被改动 → exit_code_and_log_line_are_pinned 变红
    //   · busy_title 换回会 panic 的 app_display_name()
    //     → busy_title_survives_a_missing_manifest 变红
    // 改测试时请保住这个性质——一条永远绿的测试不会告诉你任何事。
    //
    // 下半场（zombie_lock_* 起）走的是真文件 + 真 Win32：读锁文件、取修改时间、
    // 查进程存活。纯逻辑那几条测不到这些，而它们才是真正会出错的地方。

    #[test]
    fn parses_both_formats() {
        assert_eq!(parse_stamp("2628"), Some((2628, None)));
        assert_eq!(
            parse_stamp("2628|133712345678901234"),
            Some((2628, Some(133_712_345_678_901_234)))
        );
        assert_eq!(parse_stamp("1|0"), Some((1, Some(0))));
        assert_eq!(parse_stamp("4294967295|1"), Some((4294967295, Some(1))));
    }

    #[test]
    fn zero_pid_rejected() {
        // PID 0 是 System Idle Process，永远「活着」——认了它，锁就永久挡死。
        assert_eq!(parse_stamp("0"), None);
        assert_eq!(parse_stamp("0|123"), None);
    }

    #[test]
    fn sloppy_input_rejected() {
        for s in [
            "",
            " 2628",
            "+2628",
            "-2628",
            "26 28",
            "abc",
            "2628|",
            "|123",
            "2628|abc",
            "2628|12|34",
            "2628|123 ",
            "4294967296",
            "1|99999999999999999999999",
        ] {
            assert_eq!(parse_stamp(s), None, "本该判为不认识: {s:?}");
        }
    }

    #[test]
    fn dead_owner_releases() {
        // 主人没了就是遗物，两种格式都该放行 —— 这是修复前会永久挡死的那一支。
        assert!(!lock_blocks(true, false, false));
        assert!(!lock_blocks(true, false, true));
        assert!(!lock_blocks(false, false, false));
    }

    #[test]
    fn live_owner_with_identity_blocks() {
        // 真有实例在跑：挡住它是这个锁的正当用途，年龄不该误伤一次跨天的慢安装。
        assert!(lock_blocks(true, true, false));
        assert!(lock_blocks(true, true, true));
    }

    #[test]
    fn stale_old_format_lock_releases() {
        // 旧格式 + PID 看着活着（多半是被复用了）：新鲜时仍挡，过了 24h 必须放行。
        // 没有后面这一条，就是「后续所有安装/卸载永久挡死」的那个形态。
        assert!(lock_blocks(false, true, false));
        assert!(!lock_blocks(false, true, true));
    }

    #[test]
    fn stale_boundary_is_24h() {
        // ⚠️ 刻意写死 24 小时，而不是用 STALE_AFTER 算 —— 拿常量验常量，改了常量
        // 测试跟着改，这条断言就永远绿。阈值是对外承诺的行为，要钉死在测试里。
        assert_eq!(STALE_AFTER, Duration::from_secs(24 * 60 * 60));
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let day = Duration::from_secs(24 * 60 * 60);
        assert!(!is_stale(base, base));
        assert!(!is_stale(base, base + day));
        assert!(is_stale(base, base + day + Duration::from_secs(1)));
    }

    #[test]
    fn unreadable_mtime_releases() {
        // 判不出年龄就放行 —— 若这里退回「挡住」，旧格式锁 + PID 复用 + 时间读不到
        // 就又是一个没有出口的永久挡死。
        let now = SystemTime::now();
        assert!(stale_verdict(None, now));
        // 有时间时仍按正常判据走，别把上面那条写成无条件放行。
        assert!(!stale_verdict(Some(now), now));
        assert!(stale_verdict(
            Some(now - Duration::from_secs(25 * 60 * 60)),
            now
        ));
    }

    #[test]
    fn small_clock_skew_treated_as_fresh() {
        // 锁文件的时间比现在略晚（NTP 校正、时间戳精度）：另一个安装器很可能真的在跑，
        // 按新鲜处理，别放第二个进来。
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(!is_stale(base, base - Duration::from_secs(1)));
        assert!(!is_stale(base, base - Duration::from_secs(60 * 60)));
    }

    #[test]
    fn far_future_mtime_is_stale() {
        // 锁文件的时间比现在晚出一整个窗口以上：这个 mtime 是垃圾（快照回滚、时钟曾被
        // 设到未来、锁是从别的机器拷来的）。一刀切当「新鲜」的话，它会把后续安装永久挡死。
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let day = Duration::from_secs(24 * 60 * 60);
        assert!(!is_stale(base, base - day)); // 恰好一天：还在窗口内
        assert!(is_stale(base, base - day - Duration::from_secs(1)));
        assert!(is_stale(base, SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn busy_title_survives_a_missing_manifest() {
        // `report_busy_and_exit` 可能跑在清单 bootstrap 之前（控制面板点卸载那条路就是），
        // 那里再 panic 一次，换来的是无提示 abort，退出码也不再是 1618。
        //
        // 本测试二进制从不调 `meta::bootstrap`（`meta::init` 只出现在 tests/ 下的几个
        // 集成测试里，各自独立进程），所以这里必然走 None 那一支 —— 把 busy_title
        // 换回会 panic 的 `meta::app_display_name()` 的话，这条会直接炸。
        assert_eq!(busy_title(), "安装程序");
        assert!(crate::meta::try_app_display_name().is_none());
    }

    #[test]
    fn current_stamp_round_trips() {
        // 写端产物必须被读端规则接受，且 pid 是本进程。
        let stamp = current_stamp();
        let (pid, ft) = parse_stamp(&stamp).expect("本进程写下的锁戳读不回来");
        assert_eq!(pid, std::process::id());
        // 2020-01-01 UTC 的 FILETIME。GetProcessTimes 的出参顺序写错(拿到 kernel/user
        // 时间)会得到一个小得多的时长而非时间点，过不了这一关。
        const FT_2020: u64 = 132_223_104_000_000_000;
        assert!(
            ft.map(|v| v > FT_2020).unwrap_or(false),
            "创建时间不像时间点: {ft:?}"
        );
    }

    /// 在临时目录里摆一个锁文件，跑完自动删。
    ///
    /// 用真文件而不是把内容直接喂给判定函数：`read_lock` 里读文件、取修改时间、
    /// 查进程这三步才是真正会出错的地方，只测纯逻辑等于没测到它们。
    fn with_lock_file<T>(content: &str, f: impl FnOnce(&Path) -> T) -> T {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "wind_installer_test_{}_{}.lock",
            std::process::id(),
            n
        ));
        std::fs::write(&path, content).expect("写测试锁文件");
        let result = f(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    /// 走完整条链路：读文件 → 解析 → 查进程 → 判定。返回「是否挡住」。
    fn blocks_now(content: &str, now: SystemTime) -> bool {
        with_lock_file(content, |path| match read_lock(path, now) {
            Some(s) => lock_blocks(s.has_identity, s.alive, s.stale_by_age),
            // 认不出来的锁没有发言权，调用方会直接抢锁覆盖它。
            None => false,
        })
    }

    #[test]
    fn zombie_lock_from_an_old_version_eventually_releases() {
        // 这条就是被修的那个缺陷：旧版本写的锁只有 PID，PID 被复用后看着永远「活着」。
        // 拿本进程的 PID 冒充那个被复用的进程——它当然活着。
        let old_format = std::process::id().to_string();
        let now = SystemTime::now();

        // 新鲜时仍要挡住：不然旧版本安装器正在跑的时候就放第二个进来了。
        assert!(blocks_now(&old_format, now));

        // 过了 24 小时必须放行。没有这一条，后续所有安装/卸载都被永久挡死。
        assert!(!blocks_now(
            &old_format,
            now + Duration::from_secs(25 * 60 * 60)
        ));
    }

    #[test]
    fn live_owner_blocks_however_long_it_takes() {
        // 新格式 + 身份吻合(就是本进程)：真有实例在跑，多久都得挡。
        let stamp = current_stamp();
        assert!(stamp.contains('|'), "本进程应当写得出带身份的锁戳: {stamp}");
        let now = SystemTime::now();
        assert!(blocks_now(&stamp, now));
        assert!(blocks_now(
            &stamp,
            now + Duration::from_secs(30 * 24 * 60 * 60)
        ));
    }

    #[test]
    fn reused_pid_with_wrong_identity_releases() {
        // PID 活着但创建时间对不上 —— 正是 PID 复用的样子，必须立刻放行，不必等 24h。
        let ft = current_create_time().expect("取不到本进程创建时间");
        let reused = format!("{}|{}", std::process::id(), ft + 1);
        assert!(!blocks_now(&reused, SystemTime::now()));
    }

    #[test]
    fn dead_pid_and_garbage_release() {
        let now = SystemTime::now();
        // 不存在的 PID：遗物。
        //
        // 99998 不会 flaky，但靠的是个实现细节：Windows 的 PID 恒为 4 的倍数，
        // 99998 % 4 == 2，永远分配不出来。要改这个数，请挑另一个不是 4 的倍数的。
        assert!(!blocks_now("99998|1", now));
        assert!(!blocks_now("99998", now));
        // 认不出来的内容：不该有发言权。
        assert!(!blocks_now("", now));
        assert!(!blocks_now("not-a-pid", now));
        assert!(!blocks_now("0", now));
    }

    #[test]
    fn missing_lock_file_never_blocks() {
        let path = std::env::temp_dir().join(format!(
            "wind_installer_test_absent_{}.lock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        assert!(read_lock(&path, SystemTime::now()).is_none());
    }

    #[test]
    fn exit_code_and_log_line_are_pinned() {
        // 退出码是比 24h 更硬的对外契约（README 的退出码表、AGENTS.md 的调用方契约都
        // 写着它），同样不许拿常量验常量。
        assert_eq!(EXIT_ALREADY_RUNNING, 1618);

        // 静默模式下调用方只看得到一个退出码，这行日志是唯一能说清「谁占着锁、锁在哪」
        // 的东西；格式写坏了不会有任何地方报错，只能靠这条钉住。
        let line = busy_log_line(2628, Path::new("C:\\Temp\\wind_installer.lock"));
        assert_eq!(
            line,
            "aborted: exit=1618 another_instance_pid=2628 lock=C:\\Temp\\wind_installer.lock\n"
        );
    }

    /// 整条接线：判定 → 抢锁 → 置位 → 释放。
    ///
    /// 写成一个 `#[test]` 是刻意的：`INSTANCE_RUNNING` 是全局的，拆成多条会在并行
    /// 跑测试时互相踩。本模块只有这一条碰它。
    #[test]
    fn acquire_then_block_then_release() {
        let path = std::env::temp_dir().join(format!(
            "wind_installer_test_acquire_{}.lock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let now = SystemTime::now();

        // ① 空地：拿得到锁，并且落盘的是带身份的新格式。
        assert_eq!(acquire(&path, now), None);
        let written = std::fs::read_to_string(&path).expect("抢到锁却没写下锁文件");
        assert_eq!(
            parse_stamp(&written).map(|(pid, _)| pid),
            Some(std::process::id())
        );
        assert!(
            written.contains('|'),
            "落盘的应当是带身份的新格式: {written}"
        );

        // ② 锁里记的就是本进程、本进程当然活着 → 第二个实例会被挡住，并拿到那个 pid。
        assert_eq!(acquire(&path, now), Some(std::process::id()));

        // ③ 释放后文件消失。
        release(&path);
        assert!(!path.exists(), "release 之后锁文件仍在");

        // ④ 没拿到锁就不许动别人的锁。摆一把「别人的」，再 release 一次，它必须还在。
        //    INSTANCE_RUNNING 在 ③ 已清零，这一步守的就是那次清零：不清的话，
        //    这次 release 会把别人正拿着的锁删掉。
        std::fs::write(&path, "99998|1").expect("摆不下别人的锁");
        release(&path);
        assert!(path.exists(), "release 删掉了本进程并未持有的锁");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn own_process_matches_its_own_identity() {
        // 真调 Win32：本进程用自己的身份去比对必然吻合；把创建时间改一位就必须不吻合
        // （这就是 PID 复用那条路上救命的一步）。
        let ft = current_create_time().expect("本进程必然取得到自己的创建时间");
        let me = std::process::id();
        assert!(process_matches(me, ft));
        assert!(!process_matches(me, ft + 1));
    }
}
