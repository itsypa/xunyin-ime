//! 组合串换算：**原始串 ↔ 显示串**（DLL 与常驻服务共用一份）。
//!
//! Rime 的 preedit 用空格分音节（"ni hao"），界面上按设置显示成 "ni'hao"。
//! 分隔符可以是空串、也可以是多字节，长度不再等于空格，所以必须老实建映射表：
//!   * `to_display`：原始字节位置 → 显示字节位置（组合串与光标显示用）
//!   * `to_raw`    ：显示字节位置 → 原始字节位置（方向键移动后回写 Rime 用）
//!
//! 光标位置一律是 **UTF-8 字节偏移**（TSF 组合串的显示位置）。

/// 原始串与显示串的对照表。
pub struct Preedit {
    /// 显示串（空格已替换成分隔符）
    pub display: String,
    /// 下标 = 显示字节位置，值 = 原始字节位置（长度 = display.len() + 1）
    pub to_raw: Vec<usize>,
    /// 下标 = 原始字节位置，值 = 显示字节位置（长度 = text.len() + 1）
    pub to_display: Vec<usize>,
}

/// 按分隔符把原始串渲染成显示串，并建好双向映射。
pub fn build(text: &str, sep: &str) -> Preedit {
    let mut display = String::with_capacity(text.len() + 8);
    let mut to_raw: Vec<usize> = Vec::with_capacity(text.len() + 1);
    let mut to_display: Vec<usize> = vec![0usize; text.len() + 1];
    for (i, ch) in text.char_indices() {
        to_display[i] = display.len();
        if ch == ' ' {
            // 音节分隔：显示成分隔符（可能为空串），映射指回空格本身
            for _ in 0..sep.len() {
                to_raw.push(i);
            }
            display.push_str(sep);
        } else {
            for b in 0..ch.len_utf8() {
                to_display[i + b] = display.len();
                to_raw.push(i + b);
            }
            display.push(ch);
        }
    }
    to_display[text.len()] = display.len();
    to_raw.push(text.len());
    Preedit {
        display,
        to_raw,
        to_display,
    }
}

/// 显示串（含分隔符）与显示光标位置。
pub fn format(text: &str, caret_byte: usize, sep: &str) -> (String, usize) {
    let p = build(text, sep);
    let caret = p
        .to_display
        .get(caret_byte.min(text.len()))
        .copied()
        .unwrap_or(p.display.len());
    (p.display, caret)
}

/// 组合中的方向键移动：**逐个字母**移动、跳过音节分隔符、到两端停住（不循环）。
///
/// 返回新的显示光标位置。判据：光标左侧紧邻分隔符的位置不算停靠点
/// （分隔符归到左侧音节，"ni'|hao" 不是停靠位，每个字母仍各有停靠点）。
pub fn move_caret(display: &str, sep: &str, caret: usize, left: bool) -> usize {
    let bytes = display.as_bytes();
    let len = bytes.len();
    let is_sep = |i: usize| !sep.is_empty() && bytes[i..].starts_with(sep.as_bytes());
    let valid = |p: usize| p == 0 || p >= len || !is_sep(p - 1);
    let mut p = caret.min(len);
    if left {
        while p > 0 {
            p -= 1;
            if valid(p) {
                break;
            }
        }
    } else {
        while p < len {
            p += 1;
            if valid(p) {
                break;
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_replaces_syllable_spaces() {
        assert_eq!(format("ni hao", 6, "'"), ("ni'hao".into(), 6));
        assert_eq!(format("ni hao", 0, "'"), ("ni'hao".into(), 0));
        assert_eq!(format("ni hao", 4, "'"), ("ni'hao".into(), 4));
        assert_eq!(format("nihao", 3, "'"), ("nihao".into(), 3));
        assert_eq!(format("", 0, "'"), (String::new(), 0));
    }

    #[test]
    fn format_with_configurable_separator() {
        assert_eq!(format("ni hao", 3, " "), ("ni hao".into(), 3));
        assert_eq!(format("ni hao", 4, "-"), ("ni-hao".into(), 4));
        assert_eq!(format("xuan ting", 9, ","), ("xuan,ting".into(), 9));
        // 不留分隔：显示变短，光标必须跟着回退
        assert_eq!(format("ni hao", 3, ""), ("nihao".into(), 2));
        assert_eq!(format("ni hao", 4, ""), ("nihao".into(), 3));
        assert_eq!(format("ni hao", 6, ""), ("nihao".into(), 5));
    }

    #[test]
    fn maps_are_consistent() {
        let p = build("ni hao", "'");
        assert_eq!(p.display, "ni'hao");
        assert_eq!(p.to_raw.len(), p.display.len() + 1);
        assert_eq!(p.to_display.len(), "ni hao".len() + 1);
        assert_eq!(p.to_raw[0], 0);
        assert_eq!(*p.to_raw.last().unwrap(), "ni hao".len());
    }

    #[test]
    fn caret_moves_letter_by_letter_skipping_sep() {
        // "ni'hao"：从末尾往左走，每次跨过一个字母
        assert_eq!(move_caret("ni'hao", "'", 6, true), 5);
        assert_eq!(move_caret("ni'hao", "'", 5, true), 4);
        // 4 → 2：位置 3 是分隔符之后，不属于停靠点
        assert_eq!(move_caret("ni'hao", "'", 4, true), 2);
        assert_eq!(move_caret("ni'hao", "'", 2, true), 1);
        assert_eq!(move_caret("ni'hao", "'", 1, true), 0);
        // 最左边再往左不动
        assert_eq!(move_caret("ni'hao", "'", 0, true), 0);
        // 往右
        assert_eq!(move_caret("ni'hao", "'", 0, false), 1);
        assert_eq!(move_caret("ni'hao", "'", 2, false), 4);
        assert_eq!(move_caret("ni'hao", "'", 4, false), 5);
        assert_eq!(move_caret("ni'hao", "'", 6, false), 6);
    }

    #[test]
    fn caret_moves_without_separator() {
        assert_eq!(move_caret("nihao", "", 3, true), 2);
        assert_eq!(move_caret("nihao", "", 3, false), 4);
        assert_eq!(move_caret("", "", 0, true), 0);
    }
}
