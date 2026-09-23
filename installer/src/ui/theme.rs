//! 向导主题色。
//!
//! 取值优先级：清单 `[theme]` 段 → 内置默认。内置默认是一套中性蓝色系，
//! 与 windui 默认 palette.accent 对齐；应用可在 app.toml 里逐项覆盖，
//! 无需重新编译 stub。

#![allow(dead_code)]

use crate::meta;

// ── 内置默认（清单未覆盖时生效）─────────────────────────────────────────────

const DEF_ACCENT: u32 = 0x4C8BF5;
const DEF_ACCENT_HOVER: u32 = 0x6BA3FF;
const DEF_ACCENT_PRESSED: u32 = 0x3A6FD0;
const DEF_BG_PRIMARY: u32 = 0xFFFFFF;
const DEF_BG_SECONDARY: u32 = 0xF7F8FA;
const DEF_TEXT_PRIMARY: u32 = 0x191919;
const DEF_TEXT_SECONDARY: u32 = 0x888888;
const DEF_TEXT_MUTED: u32 = 0xBDBDBD;
const DEF_SUCCESS: u32 = 0x07C160;
const DEF_ERROR: u32 = 0xFA5151;
const DEF_WARNING: u32 = 0xFA9D3B;
const DEF_BORDER: u32 = 0xE5E5E5;
const DEF_DIVIDER: u32 = 0xF0F0F0;
const DEF_TRACK: u32 = 0xEAEAEA;

/// 解析 `"#RRGGBB"` / `"RRGGBB"`；空串或格式非法时回退到 `fallback`。
///
/// 非法值静默回退而非 panic：一个拼错的颜色不该让整个安装器起不来。
fn parse_color(s: &str, fallback: u32) -> u32 {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 {
        return fallback;
    }
    u32::from_str_radix(t, 16).unwrap_or(fallback)
}

macro_rules! theme_color {
    ($name:ident, $field:ident, $default:ident) => {
        pub fn $name() -> u32 {
            parse_color(&meta::manifest().theme.$field, $default)
        }
    };
}

theme_color!(accent, accent, DEF_ACCENT);
theme_color!(accent_hover, accent_hover, DEF_ACCENT_HOVER);
theme_color!(accent_pressed, accent_pressed, DEF_ACCENT_PRESSED);
theme_color!(bg_primary, bg_primary, DEF_BG_PRIMARY);
theme_color!(bg_secondary, bg_secondary, DEF_BG_SECONDARY);
theme_color!(text_primary, text_primary, DEF_TEXT_PRIMARY);
theme_color!(text_secondary, text_secondary, DEF_TEXT_SECONDARY);
theme_color!(text_muted, text_muted, DEF_TEXT_MUTED);
theme_color!(success, success, DEF_SUCCESS);
theme_color!(error, error, DEF_ERROR);
theme_color!(warning, warning, DEF_WARNING);
theme_color!(border, border, DEF_BORDER);
theme_color!(divider, divider, DEF_DIVIDER);
theme_color!(track, track, DEF_TRACK);

// parse_color 的行为经 tests/ui_defaults.rs 与 tests/ui_overrides.rs 走公开 API 覆盖：
// 本 crate 的 --lib 测试二进制名为 wind_installer-<hash>.exe，含 "install" 会命中
// Windows UAC 安装器检测启发式而无法在普通权限下启动，故内联单测跑不起来。
