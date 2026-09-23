//! 安装回执的序列化与撤销顺序。
//!
//! 回执是卸载的唯一依据，故它的 TOML 往返必须无损：一条读不回来的产物 = 一件
//! 永远卸不掉的系统更改。这里不碰注册表（`save`/`load` 需要管理员权限），
//! 只测纯逻辑部分。

#![cfg(windows)]

use wind_installer::installer::receipt::{Receipt, ReceiptEntry};

fn sample() -> Receipt {
    Receipt {
        entries: vec![
            ReceiptEntry::ComRegistered {
                dll: r"C:\Program Files\Demo\demo_tsf.dll".into(),
                wow64: false,
            },
            ReceiptEntry::ComRegistered {
                dll: r"C:\Program Files\Demo\demo_tsf_x86.dll".into(),
                wow64: true,
            },
            ReceiptEntry::InputMethodRegistered {
                profile: "0804:{CLSID}{GUID}".into(),
            },
            ReceiptEntry::FontInstalled {
                file: "Demo.ttf".into(),
                display_name: "Demo (TrueType)".into(),
            },
            ReceiptEntry::AutoStartSet {
                value_name: "Demo".into(),
            },
            ReceiptEntry::UrlProtocolRegistered {
                protocol: "demo".into(),
            },
            ReceiptEntry::ShortcutCreated {
                path: r"C:\ProgramData\...\Demo 设置.lnk".into(),
            },
            ReceiptEntry::StartMenuFolderCreated {
                path: r"C:\ProgramData\...\Demo".into(),
            },
            ReceiptEntry::UninstallInfoWritten {
                key: r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Demo App".into(),
            },
            ReceiptEntry::DataDirConfWritten {
                path: r"C:\Users\X\AppData\Local\Demo\datadir.conf".into(),
            },
        ],
    }
}

#[test]
fn every_entry_kind_survives_toml_roundtrip() {
    let original = sample();
    let text = original.to_toml().expect("序列化失败");
    let parsed = Receipt::from_toml(&text).expect("反序列化失败");

    assert_eq!(parsed.entries, original.entries);
}

#[test]
fn wow64_flag_distinguishes_com_dlls() {
    // 撤销时要靠这个标志选 regsvr32 还是 SysWOW64\regsvr32，丢了就反注册不掉 32 位 DLL
    let text = sample().to_toml().unwrap();
    let parsed = Receipt::from_toml(&text).unwrap();

    let flags: Vec<bool> = parsed
        .entries
        .iter()
        .filter_map(|e| match e {
            ReceiptEntry::ComRegistered { wow64, .. } => Some(*wow64),
            _ => None,
        })
        .collect();
    assert_eq!(flags, [false, true]);
}

#[test]
fn undo_order_is_reverse_of_install_order() {
    let r = sample();
    let undone: Vec<&ReceiptEntry> = r.undo_order().collect();

    assert_eq!(undone.len(), r.entries.len());
    assert_eq!(undone[0], r.entries.last().unwrap());
    assert_eq!(*undone.last().unwrap(), &r.entries[0]);
}

#[test]
fn empty_receipt_roundtrips() {
    // 卸载读到空回执时应退化为「无可撤销项」，而不是解析失败
    let text = Receipt::default().to_toml().unwrap();
    assert!(Receipt::from_toml(&text).unwrap().entries.is_empty());
}

#[test]
fn malformed_receipt_is_rejected_not_silently_accepted() {
    // load 侧据此把「键不存在」（正常）与「内容损坏」（有产物却撤销不了）区分开
    assert!(Receipt::from_toml("这不是 TOML {{{").is_err());
}

// ── 升级语义：安装续写旧回执，而非从空起 ────────────────────────────────────

/// 升级到「删掉了某能力」的新版本时，旧版产物必须仍可撤销。
///
/// 这是回执被覆盖式写入时最容易丢账的场景：v1 装了字体，v2 删掉 [[font]] 段 →
/// v2 的计划里根本没有 InstallFonts → 若从空回执起，FontInstalled 就此丢失 →
/// 卸载后字体永久留在系统里。
#[test]
fn upgrade_retains_entries_the_new_version_no_longer_produces() {
    let mut receipt = sample(); // 模拟 v1 装完后读回的旧回执
    let font_entry = ReceiptEntry::FontInstalled {
        file: "Demo.ttf".into(),
        display_name: "Demo (TrueType)".into(),
    };
    assert!(receipt.entries.contains(&font_entry));

    // v2 不再装字体，只记了自己产生的那些
    receipt.push(ReceiptEntry::ComRegistered {
        dll: r"C:\Program Files\Demo\demo_tsf_v2.dll".into(),
        wow64: false,
    });

    assert!(
        receipt.entries.contains(&font_entry),
        "v1 装的字体丢出回执后将永久卸不掉"
    );
}

/// 反复重装同一版本不该让回执无限膨胀。
#[test]
fn push_dedupes_equal_entries() {
    let mut receipt = Receipt::default();
    let entry = ReceiptEntry::AutoStartSet {
        value_name: "Demo".into(),
    };

    receipt.push(entry.clone());
    receipt.push(entry.clone());
    receipt.push(entry);

    assert_eq!(receipt.entries.len(), 1);
}

/// 去重只看等值，不同产物必须各记一条。
#[test]
fn push_keeps_distinct_entries_of_same_kind() {
    let mut receipt = Receipt::default();
    receipt.push(ReceiptEntry::ShortcutCreated {
        path: r"C:\A.lnk".into(),
    });
    receipt.push(ReceiptEntry::ShortcutCreated {
        path: r"C:\B.lnk".into(),
    });

    assert_eq!(receipt.entries.len(), 2);
}
