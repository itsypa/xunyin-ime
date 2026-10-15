//! 网址 / 邮箱补全：`www.` 起头的域名、`xxxx@` 起头的邮箱。
//!
//! 和 `v` 模式同一套路：这些字符（`.` `@` `-` `/`）Rime 的拼音 speller 不收，所以
//! 由**我们自己维护一个缓冲串**，直接把缓冲串当组合串、把补全结果当叠加候选。
//! 空格 / 数字选候选，回车原样上屏，Esc 取消 —— 复用宿主与服务的既有叠加候选逻辑。

/// 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `www.baidu` → `www.baidu.com` 之类
    Url,
    /// `zhangsan` + `@` → `zhangsan@qq.com` 之类
    Email,
}

/// 缓冲区。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buf {
    pub kind: Kind,
    pub text: String,
}

/// 常见邮箱域名（按国内使用频率排）。
const MAIL_HOSTS: [&str; 6] = [
    "qq.com", "163.com", "gmail.com", "outlook.com", "126.com", "foxmail.com",
];

/// 常见顶级域名。
const TLDS: [&str; 5] = ["com", "cn", "com.cn", "net", "org"];

impl Buf {
    /// 邮箱：`local` 是刚才那串字母（还没上屏）。
    pub fn email(local: &str) -> Self {
        Buf {
            kind: Kind::Email,
            text: format!("{local}@"),
        }
    }

    /// 网址：`www.` 开头。
    pub fn url() -> Self {
        Buf {
            kind: Kind::Url,
            text: "www.".into(),
        }
    }

    /// 这个按键字符能进缓冲串吗（字母、数字、以及网址/邮箱里合法的符号）。
    pub fn push(&mut self, ch: char) -> bool {
        let ok = ch.is_ascii_alphanumeric()
            || match self.kind {
                Kind::Url => matches!(ch, '.' | '-' | '/' | ':' | '_'),
                Kind::Email => matches!(ch, '.' | '-' | '_' | '+'),
            };
        if ok {
            self.text.push(ch.to_ascii_lowercase());
        }
        ok
    }

    /// 退格。退到模式前缀就返回 false（调用方据此退出模式）。
    pub fn backspace(&mut self) -> bool {
        let keep = match self.kind {
            Kind::Url => 4, // "www."
            Kind::Email => self.text.find('@').map(|i| i + 1).unwrap_or(1),
        };
        if self.text.chars().count() <= keep {
            return false;
        }
        self.text.pop();
        true
    }

    /// 补全候选。(文本, 注释)
    pub fn candidates(&self) -> Vec<(String, String)> {
        match self.kind {
            Kind::Email => {
                let local = self.text.trim_end_matches('@');
                if local.is_empty() {
                    return Vec::new();
                }
                MAIL_HOSTS
                    .iter()
                    .map(|h| (format!("{local}@{h}"), "邮箱".to_string()))
                    .collect()
            }
            Kind::Url => {
                let rest = self.text.strip_prefix("www.").unwrap_or(&self.text);
                if rest.is_empty() {
                    return Vec::new();
                }
                // 已经写了完整域名（结尾是常见顶级域）就别再拼后缀，原样给它
                if TLDS.iter().any(|t| rest.ends_with(&format!(".{t}"))) {
                    return vec![(self.text.clone(), "网址".into())];
                }
                let host = rest.trim_end_matches('.');
                TLDS
                    .iter()
                    .map(|t| (format!("www.{host}.{t}"), "网址".to_string()))
                    .collect()
            }
        }
    }
}

/// 从按键判断"能进缓冲串的字符"（含 Shift 上的符号）。
pub fn char_for_key(vk: u32, shift: bool) -> Option<char> {
    match vk {
        0x32 => Some(if shift { '@' } else { '2' }),
        0xBE => Some(if shift { '>' } else { '.' }),
        0xBD => Some(if shift { '_' } else { '-' }),
        0xBF => Some(if shift { '?' } else { '/' }),
        0x30..=0x39 => char::from_u32('0' as u32 + (vk - 0x30)),
        0x41..=0x5A => {
            let c = char::from_u32('a' as u32 + (vk - 0x41))?;
            Some(if shift { c.to_ascii_uppercase() } else { c })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_candidates() {
        let mut b = Buf::email("zhangsan");
        assert_eq!(b.text, "zhangsan@");
        let c = b.candidates();
        assert_eq!(c[0].0, "zhangsan@qq.com");
        assert!(c.iter().any(|(t, _)| t == "zhangsan@gmail.com"));
        assert!(b.push('X'));
        // `@` 之后继续敲就是**域名**（"zhangsan@qq" 这种），直接往后接
        assert_eq!(b.text, "zhangsan@x");
    }

    #[test]
    fn url_candidates() {
        let mut b = Buf::url();
        assert_eq!(b.text, "www.");
        assert!(b.candidates().is_empty(), "光有 www. 先不出候选");
        for ch in "baidu".chars() {
            assert!(b.push(ch));
        }
        let c = b.candidates();
        assert_eq!(c[0].0, "www.baidu.com");
        assert!(c.iter().any(|(t, _)| t == "www.baidu.cn"));
        for ch in ".com".chars() {
            assert!(b.push(ch));
        }
        let c = b.candidates();
        assert_eq!(c.len(), 1, "写全了就不再拼 .com.com");
        assert_eq!(c[0].0, "www.baidu.com");
    }

    #[test]
    fn backspace_stops_at_prefix() {
        let mut b = Buf::url();
        assert!(b.push('a'));
        assert!(b.backspace());
        assert!(!b.backspace(), "退到 www. 就该退出模式");
        assert_eq!(b.text, "www.");
        let mut e = Buf::email("ab");
        assert!(e.push('q'));
        assert!(e.backspace(), "先退掉域名部分");
        assert!(!e.backspace(), "退到 ab@ 就该退出模式（本地部分要改得先退出）");
        assert_eq!(e.text, "ab@");
    }

    #[test]
    fn keys_map_to_chars() {
        assert_eq!(char_for_key(0x32, true), Some('@'));
        assert_eq!(char_for_key(0x32, false), Some('2'));
        assert_eq!(char_for_key(0xBE, false), Some('.'));
        assert_eq!(char_for_key(0x41, false), Some('a'));
        assert_eq!(char_for_key(0x41, true), Some('A'));
        assert_eq!(char_for_key(0x20, false), None);
    }
}
