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

