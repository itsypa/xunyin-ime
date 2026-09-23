//! 卸载器（`uninstall.exe`）的命令行参数。
//!
//! ── 这里出过什么事 ────────────────────────────────────────────────────
//! `uninstall.exe` 此前**整体不解析参数**：main 里读完 `--self-delete` 就直接跑 GUI
//! 向导。而安装时写进注册表的 `QuietUninstallString` 是
//! `"…\uninstall.exe" --uninstall --silent` —— winget / SCCM / 批量卸载脚本按 ARP
//! 的约定调它，期待无人值守地卸完，实际弹出一个模态向导等人点「下一步」，
//! 部署任务就挂在那里直到超时。**静默卸载从来没有存在过。**
//!
//! 那条命令行里还有第二个坑：`--uninstall` 这个 flag **全仓没有任何定义**
//! （安装器那边的卸载模式是裸词子命令 `uninstall`，不是 flag）。安装器用 clap 且开着
//! `ignore_errors = true`，那个 flag 的语义是**从出错处截断**——遇到不认识的参数，
//! 它以及它之后的全部丢弃。于是即便哪天把 `--silent` 接上，也会被前面这个
//! `--uninstall` 一起吃掉。
//!
//! ── 为什么手写而不用 clap ──────────────────────────────────────────────
//! 正因为上面那条截断语义。卸载器统共两个 flag，手写解析既没有这个坑，也不必为它
//! 引一整套子命令体系；而「未知参数怎么办」这件事在这里必须是**显式**决定，
//! 不能交给一个 flag 的隐含语义。
//!
//! ── 未知参数一律跳过（不是「容忍」，是必须）─────────────────────────────
//! 存量机器上的 ARP 条目是**老版本安装器写的**，里面就有 `--uninstall`。新版卸载器
//! 必须能被那些条目正常调起，所以不认识的参数只能静静跳过。注意这与 clap 的
//! `ignore_errors` 不同：这里跳过的只是**那一个** token，它后面的照常解析。

// 两个二进制共用同一棵模块树，本模块只有 `uninstall.exe` 那个用得上；
// 编译 `wind-installer.exe` 时整块都是「未使用」。同 `util::exitcode`。
#![allow(dead_code)]

use std::ffi::OsStr;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct UninstallArgs {
    /// 完全无界面：不弹向导、不弹 UAC，结果只通过退出码与日志交付。
    pub silent: bool,
    /// 静默卸载时保留用户配置数据（`%APPDATA%\<app.id>`）。
    ///
    /// ARP 不会传它（控制面板没有这个开关），是给脚本用的；与
    /// `wind-installer.exe uninstall --keep-user-data` 同名同义。
    ///
    /// ⚠️ **只在 `--silent` 下起作用**。交互式向导用它自己的勾选框决定删不删用户数据
    /// （`uninstall_wizard.rs` 里 `keep_user_data` 恒为 false），所以单独传这个 flag
    /// 而不带 `--silent` 是「被接受、被记进启动日志、然后什么也不做」。
    /// 界面上的勾选优先于命令行是对的 —— 用户刚亲手勾过的东西不该被一个 flag 推翻。
    pub keep_user_data: bool,
}

/// 解析一串参数（**不含** argv[0]）。
///
/// 用 `OsStr` 而不是 `String`：Windows 的命令行可以带非法 UTF-16（孤立代理），
/// `std::env::args()` 撞上它会 panic —— 在 `panic = "abort"` + GUI 子系统下那是一次
/// 无提示崩溃，而这个二进制正是「控制面板点卸载」走的那条路。
pub fn parse<S: AsRef<OsStr>>(argv: &[S]) -> UninstallArgs {
    let mut out = UninstallArgs::default();
    for a in argv {
        // to_string_lossy 把非法码位换成 U+FFFD 而不是 panic；换出来的东西
        // 一定不等于下面任何一个 flag，于是自然落进「未知参数」那一支。
        match a.as_ref().to_string_lossy().as_ref() {
            "--silent" => out.silent = true,
            "--keep-user-data" => out.keep_user_data = true,
            _ => {}
        }
    }
    out
}

/// 从本进程的真实命令行解析。
pub fn from_env() -> UninstallArgs {
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    parse(&argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn no_args_is_interactive() {
        let empty: [&str; 0] = [];
        assert_eq!(parse(&empty), UninstallArgs::default());
    }

    #[test]
    fn silent_alone() {
        assert!(parse(&["--silent"]).silent);
    }

    /// **本次修复要钉住的那一条**：存量机器 ARP 里写的就是这个形状。
    /// `--uninstall` 无定义，但它绝不能把后面的 `--silent` 带走 —— 从前正是它带走的。
    #[test]
    fn legacy_arp_shape_still_goes_silent() {
        let args = parse(&["--uninstall", "--silent"]);
        assert!(
            args.silent,
            "存量 QuietUninstallString 的 --uninstall 又把 --silent 吞掉了"
        );
    }

    /// 未知参数只跳过它自己，不影响它**之前**解析到的东西
    /// （与 clap `ignore_errors` 的「从出错处截断」区别开）。
    #[test]
    fn unknown_flag_does_not_truncate_the_rest() {
        let args = parse(&["--silent", "--from-the-future", "--keep-user-data"]);
        assert!(args.silent, "未知参数把它前面的 --silent 抹掉了");
        assert!(
            args.keep_user_data,
            "未知参数把它后面的 --keep-user-data 截断了"
        );
    }

    #[test]
    fn keep_user_data_alone() {
        let args = parse(&["--keep-user-data"]);
        assert!(args.keep_user_data);
        assert!(!args.silent, "--keep-user-data 不该顺带打开静默");
    }

    /// 认的是完整 token，不是前缀/子串 —— 免得哪天 `--silently-something` 之类
    /// 的东西误触静默卸载这条不可逆的路。
    #[test]
    fn flags_match_whole_token_only() {
        assert!(!parse(&["--silent=true"]).silent);
        assert!(!parse(&["silent"]).silent);
        assert!(!parse(&["--no-silent"]).silent);
    }

    /// 非法 Unicode 参数不能让进程崩掉。
    ///
    /// 这条只在 Windows 上有意义（孤立代理是 UTF-16 的概念），也只有在那里才编译得了
    /// `OsStringExt::from_wide`；但崩的也正是 Windows 上那个二进制。
    #[cfg(windows)]
    #[test]
    fn lone_surrogate_does_not_panic() {
        use std::os::windows::ffi::OsStringExt;
        // 0xD800 是高位代理，单独出现即非法 UTF-16。
        let bad = OsString::from_wide(&[0x2D, 0x2D, 0xD800]);
        let args = parse(&[bad, OsString::from("--silent")]);
        assert!(args.silent, "非法参数把后面的 --silent 带走了");
    }

    /// 非 Windows 上占位，保证 `OsString` 这个 import 在两边都有用处。
    #[cfg(not(windows))]
    #[test]
    fn os_string_args_work() {
        assert!(parse(&[OsString::from("--silent")]).silent);
    }
}
