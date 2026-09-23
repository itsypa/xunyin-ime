//! 「需要重启」账本的行为。
//!
//! 账本决定了安装/卸载完成页是否显示重启提示、quiet 模式是否放弃自动退出，
//! 以及 `--silent` 是否以 3010 退出——错判会让用户要么被莫名其妙的窗口挡住，
//! 要么永远收不到「还有残留」的提示。
//!
//! 放在 `tests/` 而非源码内联单测：本 crate 的 `--lib` 单测二进制名含 "install"，
//! 会命中 Windows UAC 安装器检测启发式而无法在普通权限下启动（见 `ui::theme` 末注）。

#![cfg(windows)]

use std::path::Path;

use wind_installer::util::reboot;

/// 账本是进程级共享的，而 cargo 默认并行跑测试——全部塞进一个 `#[test]` 顺序执行，
/// 避免彼此看见对方的记账。
#[test]
fn ledger_drives_reboot_verdict() {
    reboot::reset_ledger();

    // 什么都没发生 → 不该提示重启
    assert!(!reboot::is_reboot_pending(), "空账本不该判定需要重启");
    assert_eq!(reboot::pending_summary(), "", "空账本的摘要应为空串");

    // 一个被占用的旧 DLL 排进了删除队列
    reboot::record_pending(Path::new(r"C:\app\ime.dll.old_0001"), true);
    assert!(reboot::is_reboot_pending());
    assert_eq!(reboot::pending_summary(), "1 个文件待重启后清理");

    // 连排队都失败的要单独点出来：重启也不会自动清，属更严重的残留
    reboot::record_pending(Path::new(r"C:\app\stuck.dll"), false);
    assert_eq!(
        reboot::pending_summary(),
        "2 个文件待重启后清理（其中 1 个未能排入删除队列）"
    );

    // 同一路径重复记账不该虚增计数（改名失败后重试是常见路径）
    reboot::record_pending(Path::new(r"C:\app\stuck.dll"), false);
    reboot::record_pending(Path::new(r"C:\app\stuck.dll"), false);
    assert_eq!(reboot::pending_items().len(), 2, "重复路径应折叠为一条");

    // 先失败后成功 → 视为已排队（「曾经成功过」即可）
    reboot::record_pending(Path::new(r"C:\app\stuck.dll"), true);
    assert_eq!(
        reboot::pending_summary(),
        "2 个文件待重启后清理",
        "重试成功后不该再报「未能排入删除队列」"
    );

    reboot::reset_ledger();
    assert!(!reboot::is_reboot_pending(), "reset 后账本应清空");
}
