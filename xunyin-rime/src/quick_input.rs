//! 快捷输入：日期 / 时间 / 星期，以及 `v` 模式（数字转换 + 四则运算）。
//!
//! 这里是**纯函数**（不碰引擎、不碰窗口），便于单元测试；接进按键链路的胶水在
//! `text_service.rs`。两条路径：
//!   * `rq` / `sj` / `xq`：Rime 组合里本来就有这两个字母，直接把候选插到最前面；
//!   * `v123` / `v1+2*3`：Rime 的 speller 不收数字，所以由我们在按键层接管组合串。

/// 当前本地时间的快照（拆出来是为了能测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 1=周一 … 7=周日
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// 取本地时间（`GetLocalTime`）。
pub fn local_now() -> Now {
    // xunyin-rime 用的是 windows-sys（不引 windows crate）
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut st) };
    Now {
        year: st.wYear as i32,
        month: st.wMonth as u32,
        day: st.wDay as u32,
        weekday: st.wDayOfWeek as u32 + 1, // SYSTEMTIME: 0=周日
        hour: st.wHour as u32,
        minute: st.wMinute as u32,
        second: st.wSecond as u32,
    }
}

pub const WEEKDAYS: [&str; 7] = [
    "星期一", "星期二", "星期三", "星期四", "星期五", "星期六", "星期日",
];

/// 触发词（小写比较）：`rq` 日期、`sj` 时间、`xq` 星期、`v` v 模式。
///
/// 注意：Rime 的原始输入串用**空格分音节**（"r q"），所以比较前要把空格与
/// 撇号去掉，否则 `rq` 永远匹配不上（实测踩过这个坑）。
pub fn trigger(raw: &str) -> Option<&'static str> {
    let norm: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\'' && *c != '’')
        .collect::<String>()
        .to_ascii_lowercase();
    match norm.as_str() {
        "rq" => Some("rq"),
        "sj" => Some("sj"),
        "xq" => Some("xq"),
        "v" => Some("v"),
        _ => None,
    }
}

/// `rq` / `sj` / `xq` 的候选（文本, 注释）。
pub fn date_candidates(trigger: &str, now: &Now) -> Vec<(String, String)> {
    match trigger {
        "rq" => vec![
            (
                format!("{}年{}月{}日", now.year, now.month, now.day),
                format!("{} ({})", WEEKDAYS[(now.weekday - 1) as usize % 7], "日期"),
            ),
            (
                format!(
                    "{:04}-{:02}-{:02}",
                    now.year, now.month, now.day
                ),
                "日期".into(),
            ),
            (
                format!("{}月{}日", now.month, now.day),
                "日期".into(),
            ),
        ],
        "sj" => vec![
            (
                format!("{:02}:{:02}", now.hour, now.minute),
                "时间".into(),
            ),
            (
                format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second),
                "时间".into(),
            ),
            (
                format!(
                    "{}年{}月{}日 {:02}:{:02}",
                    now.year, now.month, now.day, now.hour, now.minute
                ),
                "日期时间".into(),
            ),
        ],
        "xq" => vec![
            (WEEKDAYS[(now.weekday - 1) as usize % 7].to_string(), "星期".into()),
            (
                format!(
                    "{} {}月{}日",
                    WEEKDAYS[(now.weekday - 1) as usize % 7],
                    now.month,
                    now.day
                ),
                "星期".into(),
            ),
        ],
        _ => Vec::new(),
    }
}

/// `v` 模式候选：buf 是 `v` 之后的输入（含 `v` 本身，如 `v123`、`v1+2*3`）。
pub fn v_mode_candidates(buf: &str) -> Vec<(String, String)> {
    let body = buf.strip_prefix(['v', 'V']).unwrap_or(buf).trim();
    let mut out: Vec<(String, String)> = Vec::new();
    if body.is_empty() {
        return out;
    }
    // 纯数字 → 数字转换（中文小写 / 大写 / 进制）
    if body.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = body.parse::<i64>() {
            out.push((number_to_chinese(n), "中文数字".into()));
            out.push((number_to_upper(n), "中文大写".into()));
            out.push((format!("{n}"), "阿拉伯数字".into()));
            if n >= 0 {
                out.push((format!("{n:#X}"), "十六进制".into()));
                out.push((format!("{n:#b}"), "二进制".into()));
            }
            return out;
        }
    }
    // 算式 → 计算结果
    if let Some(v) = eval_expr(body) {
        out.push((format_number(v), "计算结果".into()));
        out.push((format!("= {}", format_number(v)), "计算".into()));
    }
    out
}

/// 算式求值（四则运算 + 括号 + 一元负号）。
pub fn eval_expr(src: &str) -> Option<f64> {
    let mut p = ExprParser {
        chars: src.chars().filter(|c| !c.is_whitespace()).collect(),
        pos: 0,
    };
    let v = p.expr()?;
    if p.pos != p.chars.len() {
        return None; // 有剩余字符（不是合法算式）
    }
    if !v.is_finite() {
        return None;
    }
    Some(v)
}

fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

struct ExprParser {
    chars: Vec<char>,
    pos: usize,
}

impl ExprParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                let d = self.unary()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.atom()
    }
    fn atom(&mut self) -> Option<f64> {
        if self.eat('(') {
            let v = self.expr()?;
            if !self.eat(')') {
                return None;
            }
            return Some(v);
        }
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.pos += 1;
        }
        if start == self.pos {
            return None;
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse::<f64>()
            .ok()
    }
}

const CN_DIGITS: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
const CN_UPPER: [&str; 10] = ["零", "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

/// 阿拉伯数字 → 中文数字（一百二十三）。
pub fn number_to_chinese(n: i64) -> String {
    number_to_cn(n, &CN_DIGITS, false)
}

/// 阿拉伯数字 → 中文大写（壹佰贰拾叁）。
pub fn number_to_upper(n: i64) -> String {
    number_to_cn(n, &CN_UPPER, true)
}

/// 中文数字转换。`upper` 时"十"读作"拾"，且 10 读作"壹拾"（财务写法）。
fn number_to_cn(n: i64, digits: &[&str; 10], upper: bool) -> String {
    if n == 0 {
        return digits[0].to_string();
    }
    let neg = n < 0;
    let mut n = n.unsigned_abs();
    let ten = if upper { "拾" } else { "十" };
    let hundred = if upper { "佰" } else { "百" };
    let thousand = if upper { "仟" } else { "千" };
    let units = [
        ("", 1u64),
        (if upper { "万" } else { "万" }, 10_000),
        (if upper { "亿" } else { "亿" }, 100_000_000),
        ("兆", 1_000_000_000_000),
    ];
    let mut groups: Vec<(u64, &str)> = Vec::new();
    for (name, base) in units.iter().rev() {
        if *base == 1 {
            continue;
        }
        let g = (n / base) % 10_000;
        if g > 0 {
            groups.push((g, name));
        }
    }
    let rest = n % 10_000;
    // 从大到小拼接
    let mut out = String::new();
    for (g, name) in &groups {
        out.push_str(&four_to_cn(*g, digits, ten, hundred, thousand));
        out.push_str(name);
        n %= 10_000u64.pow(4);
    }
    if rest > 0 || out.is_empty() {
        if !out.is_empty() && rest < 1000 {
            out.push_str(digits[0]); // 例如 一亿零五
        }
        out.push_str(&four_to_cn(rest, digits, ten, hundred, thousand));
    }
    if neg {
        format!("负{out}")
    } else {
        out
    }
}

/// 0..=9999 的中文读法。
fn four_to_cn(n: u64, digits: &[&str; 10], ten: &str, hundred: &str, thousand: &str) -> String {
    if n == 0 {
        return String::new();
    }
    let d = [
        (n / 1000) % 10,
        (n / 100) % 10,
        (n / 10) % 10,
        n % 10,
    ];
    let names = [thousand, hundred, ten, ""];
    let mut out = String::new();
    let mut zero_pending = false;
    for (i, v) in d.iter().enumerate() {
        if *v == 0 {
            if !out.is_empty() && i < 3 {
                zero_pending = true;
            }
            continue;
        }
        if zero_pending {
            out.push_str(digits[0]);
            zero_pending = false;
        }
        // 一十三 → 十三（仅小写且是最高位时，财务大写保持"壹拾叁"）
        if i == 2 && *v == 1 && out.is_empty() && !ten.starts_with('拾') {
            out.push_str(ten);
        } else {
            out.push_str(digits[*v as usize]);
            out.push_str(names[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Now {
        Now {
            year: 2026,
            month: 10,
            day: 13,
            weekday: 2, // 星期二
            hour: 9,
            minute: 5,
            second: 7,
        }
    }

    #[test]
    fn triggers() {
        assert_eq!(trigger("rq"), Some("rq"));
        assert_eq!(trigger("RQ"), Some("rq"));
        // Rime 的原始串按音节带空格/撇号，必须能归一化
        assert_eq!(trigger("r q"), Some("rq"));
        assert_eq!(trigger("x'q"), Some("xq"));
        assert_eq!(trigger("sj"), Some("sj"));
        assert_eq!(trigger("xq"), Some("xq"));
        assert_eq!(trigger("v"), Some("v"));
        assert_eq!(trigger("n"), None);
        assert_eq!(trigger("rqq"), None);
    }

    #[test]
    fn date_candidates_content() {
        let c = date_candidates("rq", &now());
        assert_eq!(c[0].0, "2026年10月13日");
        assert_eq!(c[1].0, "2026-10-13");
        let s = date_candidates("sj", &now());
        assert_eq!(s[0].0, "09:05");
        assert_eq!(s[1].0, "09:05:07");
        let x = date_candidates("xq", &now());
        assert_eq!(x[0].0, "星期二");
    }

    #[test]
    fn v_mode_numbers() {
        let c = v_mode_candidates("v123");
        let texts: Vec<&str> = c.iter().map(|(t, _)| t.as_str()).collect();
        assert!(texts.contains(&"一百二十三"), "{texts:?}");
        assert!(texts.contains(&"壹佰贰拾叁"), "{texts:?}");
        assert!(texts.contains(&"123"), "{texts:?}");
        assert!(texts.contains(&"0x7B"), "{texts:?}");
    }

    #[test]
    fn v_mode_calc() {
        let c = v_mode_candidates("v1+2*3");
        assert_eq!(c[0].0, "7");
        assert_eq!(v_mode_candidates("v(10-4)/3")[0].0, "2");
        assert_eq!(v_mode_candidates("v1/0").len(), 0, "除零不出候选");
        assert!(v_mode_candidates("v1+").is_empty(), "没写完的算式不出候选");
    }

    #[test]
    fn chinese_numbers() {
        assert_eq!(number_to_chinese(10), "十");
        assert_eq!(number_to_chinese(13), "十三");
        assert_eq!(number_to_chinese(100), "一百");
        assert_eq!(number_to_chinese(101), "一百零一");
        assert_eq!(number_to_chinese(1100), "一千一百");
        assert_eq!(number_to_chinese(10005), "一万零五");
        assert_eq!(number_to_upper(1234), "壹仟贰佰叁拾肆");
        assert_eq!(number_to_upper(10), "壹拾");
        assert_eq!(number_to_chinese(0), "零");
    }
}
