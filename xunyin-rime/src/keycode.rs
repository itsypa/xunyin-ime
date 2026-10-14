//! 按键换算：Windows 虚拟键码 → 字符 / librime 键码（DLL 与常驻服务**共用一份**）。
//!
//! 为什么必须共用：宿主 DLL 把按键转成 (vk, shift) 送进服务，由服务算键码。
//! 两边各写一份映射，迟早出现"某个键在服务里算错"的问题（历史上 Enter、
//! 左右方向键、标点都踩过），所以这里只留一份实现。

/// 把虚拟键码映射为字符（字母/数字/常见上档符号）。
pub fn vk_to_char(vk: u32, shift: bool) -> Option<char> {
    if (0x41..=0x5A).contains(&vk) {
        let c = (b'a' + (vk - 0x41) as u8) as char;
        return Some(if shift { c.to_ascii_uppercase() } else { c });
    }
    if (0x30..=0x39).contains(&vk) {
        let d = (vk - 0x30) as u8;
        return Some(if shift {
            match d {
                0 => ')',
                1 => '!',
                2 => '@',
                3 => '#',
                4 => '$',
                5 => '%',
                6 => '^',
                7 => '&',
                8 => '*',
                9 => '(',
                _ => '?',
            }
        } else {
            (b'0' + d) as char
        });
    }
    // 小键盘数字（NumPad）
    if (0x60..=0x69).contains(&vk) {
        return Some((b'0' + (vk - 0x60) as u8) as char);
    }
    None
}

/// 常见的标点虚拟键码到字符的映射（依据标准 US 布局）。
pub fn vk_to_punct(vk: u32, shift: bool) -> Option<char> {
    let c = match vk {
        0xBA => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        0xBB => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        0xBC => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        0xBD => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        0xBE => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        0xBF => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        0xC0 => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        0xDB => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        0xDC => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        0xDD => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        0xDE => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        _ => return None,
    };
    Some(c)
}

/// 非字符键到 librime 键码（X11 keysym 风格）的换算。
/// 字符键（字母/数字/标点）请用 [char_keycode]。
pub fn rime_keycode(vk: u32) -> i32 {
    match vk {
        0x60..=0x69 => (vk - 0x60 + 0x30) as i32, // Numpad 0-9 → '0'-'9'
        0x08 => 0xFF08,                            // Backspace
        0x09 => 0xFF09,                            // Tab
        0x0D => 0xFF0D,                            // Enter / Return
        0x1B => 0xFF1B,                            // Escape
        0x2D => 0xFF63,                            // Insert
        0x2E => 0xFFFF,                            // Delete
        0x24 => 0xFF50,                            // Home
        0x23 => 0xFF57,                            // End
        0x25 => 0xFF96,                            // Left → XK_KP_Left（librime 逐字符移动）
        0x26 => 0xFF52,                            // Up
        0x27 => 0xFF98,                            // Right → XK_KP_Right（librime 逐字符移动）
        0x28 => 0xFF54,                            // Down
        0x21 => 0xFF55,                            // PageUp
        0x22 => 0xFF56,                            // PageDown
        _ => vk as i32,
    }
}

/// 把按键换算成 librime 的键码：
/// 字符键（字母/数字/标点）→ 该键实际产生的 ASCII 字符码；
/// 其余按键 → [rime_keycode] 的 X11 keysym。
///
/// 注意：Windows VK 字母码是大写（A=0x41），而 librime 按 X11 keysym 语义
/// 用小写表示字母键（a=0x61），拼音模式的正则只匹配小写。直接传 VK 码会
/// 变成大写组合串（如 "NIHAO"）且无法产出候选，必须按 shift 状态换算成字符。
pub fn char_keycode(vk: u32, shift: bool) -> i32 {
    if let Some(c) = vk_to_char(vk, shift) {
        return c as i32;
    }
    if let Some(c) = vk_to_punct(vk, shift) {
        return c as i32;
    }
    rime_keycode(vk)
}

/// 是否为 Shift 键（左/右）。
pub fn is_shift_key(vk: u32) -> bool {
    vk == 0x10
}

/// 该键是否为「候选/翻页/编辑」类功能键。
pub fn is_function_key(vk: u32) -> bool {
    matches!(
        vk,
        0x08 | 0x0D | 0x1B | 0x20 | 0x24 | 0x23 | 0x25..=0x28 | 0x21 | 0x22
    )
}

/// 顿号键判定：`/`(VK_OEM_2) 与 `\`(VK_OEM_5) 在**没有按 Shift**时直接上屏「、」。
/// 按了 Shift 就不是顿号键了（Shift+/ = `?` → 交给 Rime 出「？」，Shift+\ = `|`）。
pub fn is_dunhao_key(vk: u32, shift: bool) -> bool {
    !shift && matches!(vk, 0xBF | 0xDC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(vk_to_char(0x41, false), Some('a'));
        assert_eq!(vk_to_char(0x41, true), Some('A'));
        assert_eq!(vk_to_char(0x5A, false), Some('z'));
        assert_eq!(vk_to_char(0x5A, true), Some('Z'));
    }

    #[test]
    fn digits_and_shift_symbols() {
        assert_eq!(vk_to_char(0x30, false), Some('0'));
        assert_eq!(vk_to_char(0x31, true), Some('!'));
        assert_eq!(vk_to_char(0x37, true), Some('&'));
        assert_eq!(vk_to_char(0x39, true), Some('('));
        assert_eq!(vk_to_char(0x60, false), Some('0'));
        assert_eq!(vk_to_char(0x69, false), Some('9'));
    }

    #[test]
    fn punctuation() {
        assert_eq!(vk_to_punct(0xBA, false), Some(';'));
        assert_eq!(vk_to_punct(0xBA, true), Some(':'));
        assert_eq!(vk_to_punct(0xBE, false), Some('.'));
        assert_eq!(vk_to_punct(0xBE, true), Some('>'));
        assert_eq!(vk_to_punct(0xBF, true), Some('?'));
        assert_eq!(vk_to_punct(0xDB, false), Some('['));
        assert_eq!(vk_to_punct(0xDB, true), Some('{'));
        assert_eq!(vk_to_punct(0xDC, true), Some('|'));
        assert_eq!(vk_to_punct(0xDE, true), Some('"'));
        assert_eq!(vk_to_punct(0x20, false), None);
    }

    #[test]
    fn rime_keycode_mapping() {
        assert_eq!(rime_keycode(0x60), 0x30);
        assert_eq!(rime_keycode(0x69), 0x39);
        assert_eq!(rime_keycode(0x08), 0xFF08); // Backspace
        assert_eq!(rime_keycode(0x09), 0xFF09); // Tab
        assert_eq!(rime_keycode(0x0D), 0xFF0D); // Enter
        assert_eq!(rime_keycode(0x1B), 0xFF1B); // Esc
        assert_eq!(rime_keycode(0x2D), 0xFF63); // Insert
        assert_eq!(rime_keycode(0x2E), 0xFFFF); // Delete
        assert_eq!(rime_keycode(0x24), 0xFF50); // Home
        assert_eq!(rime_keycode(0x23), 0xFF57); // End
        assert_eq!(rime_keycode(0x25), 0xFF96); // Left → KP_Left（逐字符）
        assert_eq!(rime_keycode(0x26), 0xFF52); // Up
        assert_eq!(rime_keycode(0x27), 0xFF98); // Right → KP_Right（逐字符）
        assert_eq!(rime_keycode(0x28), 0xFF54); // Down
        assert_eq!(rime_keycode(0x21), 0xFF55); // PageUp
        assert_eq!(rime_keycode(0x22), 0xFF56); // PageDown
    }

    #[test]
    fn char_keycode_mapping() {
        assert_eq!(char_keycode(0x41, false), 0x61); // A → 'a'
        assert_eq!(char_keycode(0x41, true), 0x41); // Shift+A → 'A'
        assert_eq!(char_keycode(0x5A, false), 0x7A); // Z → 'z'
        assert_eq!(char_keycode(0x31, false), 0x31); // 1 → '1'
        assert_eq!(char_keycode(0x31, true), 0x21); // Shift+1 → '!'
        assert_eq!(char_keycode(0xBC, false), b',' as i32);
        assert_eq!(char_keycode(0xBC, true), b'<' as i32);
        assert_eq!(char_keycode(0xBE, false), b'.' as i32);
        assert_eq!(char_keycode(0xBF, true), b'?' as i32);
        assert_eq!(char_keycode(0x0D, false), 0xFF0D);
        assert_eq!(char_keycode(0x08, false), 0xFF08);
        assert_eq!(char_keycode(0x20, false), 0x20); // Space
        assert_eq!(char_keycode(0x25, false), 0xFF96); // Left → KP_Left
    }

    #[test]
    fn function_key_classification() {
        assert!(is_function_key(0x08));
        assert!(is_function_key(0x0D));
        assert!(is_function_key(0x1B));
        assert!(is_function_key(0x20));
        assert!(is_function_key(0x27));
        assert!(is_function_key(0x22));
        assert!(!is_function_key(0x41));
        assert!(!is_function_key(0x31));
    }

    #[test]
    fn dunhao_key_requires_no_shift() {
        assert!(is_dunhao_key(0xBF, false));
        assert!(is_dunhao_key(0xDC, false));
        assert!(!is_dunhao_key(0xBF, true), "Shift+/ 是问号，不能当顿号吃掉");
        assert!(!is_dunhao_key(0xDC, true), "Shift+\\ 是竖线");
        assert!(!is_dunhao_key(0xBE, false), "句号键不是顿号键");
    }
}
