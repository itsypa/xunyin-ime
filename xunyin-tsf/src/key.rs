//! 按键工具：Windows 虚拟键码 → 字符 / librime 键码换算、修饰键状态。
//!
//! 与旧全局钩子方案共用同一套映射逻辑（TSF 不再需要 SendInput 注入，
//! 未消费的按键由系统直接交给应用）。
//!
//! 纯换算部分已挪到 xunyin_rime::keycode，与常驻服务共用一份实现
//! （服务接管按键后必须算出同样的键码），这里只做转发 + 补上需要 Win32 的部分。

use xunyin_rime::{kRimeModifierAlt, kRimeModifierCaps, kRimeModifierControl, kRimeModifierShift};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

pub use xunyin_rime::keycode::{
    char_keycode, is_function_key, is_shift_key, vk_to_char, vk_to_punct,
};

/// 计算完整修饰键掩码（Shift/Ctrl/Alt/Caps）。
pub fn modifier_mask() -> i32 {
    let mut mask = 0;
    if key_state_down(0x10) {
        mask |= kRimeModifierShift;
    }
    if key_state_down(0x11) {
        mask |= kRimeModifierControl;
    }
    if key_state_down(0x12) {
        mask |= kRimeModifierAlt;
    }
    if caps_lock_on() {
        mask |= kRimeModifierCaps;
    }
    mask
}

pub fn key_state_down(vk: u32) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

pub fn caps_lock_on() -> bool {
    unsafe { GetKeyState(0x14) & 1 != 0 }
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
        // 小键盘数字 → ASCII '0'-'9'
        assert_eq!(rime_keycode(0x60), 0x30);
        assert_eq!(rime_keycode(0x69), 0x39);
        // 控制键 → X11 keysym
        assert_eq!(rime_keycode(0x08), 0xFF08); // Backspace
        assert_eq!(rime_keycode(0x09), 0xFF09); // Tab
        assert_eq!(rime_keycode(0x0D), 0xFF0D); // Enter
        assert_eq!(rime_keycode(0x1B), 0xFF1B); // Esc
        assert_eq!(rime_keycode(0x2D), 0xFF63); // Insert
        assert_eq!(rime_keycode(0x2E), 0xFFFF); // Delete
        // 功能键 → X11 keysym（librime 使用 X11 风格键码）
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
        // 字母：按 shift 状态给大小写 ASCII（librime 拼音模式只认小写）
        assert_eq!(char_keycode(0x41, false), 0x61); // A → 'a'
        assert_eq!(char_keycode(0x41, true), 0x41); // Shift+A → 'A'
        assert_eq!(char_keycode(0x5A, false), 0x7A); // Z → 'z'
        // 数字：无 shift 直接透传；Shift+数字 → 上档符号
        assert_eq!(char_keycode(0x31, false), 0x31); // 1 → '1'
        assert_eq!(char_keycode(0x31, true), 0x21); // Shift+1 → '!'
        // 标点 → 对应 ASCII 字符（供 punctuator 匹配中文标点）
        assert_eq!(char_keycode(0xBC, false), b',' as i32);
        assert_eq!(char_keycode(0xBC, true), b'<' as i32);
        assert_eq!(char_keycode(0xBE, false), b'.' as i32);
        assert_eq!(char_keycode(0xBF, true), b'?' as i32);
        // 非字符键回退到 rime_keycode（X11 keysym）
        assert_eq!(char_keycode(0x0D, false), 0xFF0D);
        assert_eq!(char_keycode(0x08, false), 0xFF08);
        assert_eq!(char_keycode(0x20, false), 0x20); // Space
        assert_eq!(char_keycode(0x25, false), 0xFF96); // Left → KP_Left
    }

    #[test]
    fn function_key_classification() {
        assert!(is_function_key(0x08)); // Backspace
        assert!(is_function_key(0x0D)); // Enter
        assert!(is_function_key(0x1B)); // Esc
        assert!(is_function_key(0x20)); // Space
        assert!(is_function_key(0x27)); // Right
        assert!(is_function_key(0x22)); // PageDown
        assert!(!is_function_key(0x41)); // A
        assert!(!is_function_key(0x31)); // 1
    }
}
