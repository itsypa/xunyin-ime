//! 农历转换（农历 1901–2100 年，公历到 2101-01-28）：`nl` 快捷输入的候选，
//! 也是 `rq` 里附带的农历。
//!
//! 数据表在 `lunar_table.rs`（脚本从 Windows 自带的 .NET 农历日历导出，见该文件头）。
//! 这里只做换算：公历 → 农历 + 干支 + 生肖 + 传统月名/日名。

/// 表里第一年是 1901。
const FIRST_YEAR: i32 = 1901;
/// 表的起点：1901-02-19 是**农历 1901 年正月初一**。
const BASE: (i32, u32, u32) = (1901, 2, 19);

const MONTH_NAMES: [&str; 12] = [
    "正月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "冬月", "腊月",
];
const GAN: [&str; 10] = ["甲", "乙", "丙", "丁", "戊", "己", "庚", "辛", "壬", "癸"];
const ZHI: [&str; 12] = [
    "子", "丑", "寅", "卯", "辰", "巳", "午", "未", "申", "酉", "戌", "亥",
];
const ZODIAC: [&str; 12] = [
    "鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪",
];

/// 公历 → 儒略日序号（Howard Hinnant 的 days_from_civil，便于直接做差）。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = ((m as i64) + 9) % 12; // 3 月 = 0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 某农历年的信息：月份数、闰月（0=无，否则为平月序号）、逐月天数。
struct YearInfo {
    months: u32,
    leap: u32,
    bit_len: u32,
}

fn year_info(year: i32) -> Option<YearInfo> {
    let idx = (year - FIRST_YEAR) as usize;
    let v = *crate::lunar_table::LUNAR_TABLE.get(idx)?;
    Some(YearInfo {
        months: (v >> 24) & 0xF,
        leap: (v >> 16) & 0xF,
        bit_len: v & 0x1FFF,
    })
}

fn month_days(info: &YearInfo, index0: u32) -> u32 {
    if info.bit_len & (1 << index0) != 0 {
        30
    } else {
        29
    }
}

/// 农历日期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lunar {
    pub year: i32,
    /// 传统叫法，例如 `正月` / `闰六月` / `冬月`。
    pub month_name: String,
    /// 例如 `初六` / `十五` / `三十`。
    pub day_name: String,
    /// 干支纪年，例如 `丙午`。
    pub ganzhi: String,
    /// 生肖，例如 `马`。
    pub zodiac: String,
    /// 是不是闰月。
    pub is_leap: bool,
}

impl Lunar {
    /// `2026年九月初六`
    pub fn full(&self) -> String {
        format!("{}年{}{}", self.year, self.month_name, self.day_name)
    }

    /// `九月初六`
    pub fn month_day(&self) -> String {
        format!("{}{}", self.month_name, self.day_name)
    }

    /// `丙午年九月初六`
    pub fn ganzhi_year(&self) -> String {
        format!("{}年{}{}", self.ganzhi, self.month_name, self.day_name)
    }
}

/// 日名：初一…初十 / 十一…十九 / 二十 / 廿一…廿九 / 三十。
fn day_name(d: u32) -> String {
    const TENS: [&str; 3] = ["初", "十", "廿"];
    const UNITS: [&str; 11] = [
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
    ];
    match d {
        1..=10 => format!("初{}", UNITS[d as usize]),
        11..=19 => format!("十{}", UNITS[(d - 10) as usize]),
        20 => "二十".into(),
        21..=29 => format!("廿{}", UNITS[(d - 20) as usize]),
        30 => "三十".into(),
        _ => {
            let _ = TENS;
            String::new()
        }
    }
}

/// 公历 → 农历；超出表范围（1901-02-19 之前 / 农历 2100 年过完）返回 None。
pub fn to_lunar(y: i32, m: u32, d: u32) -> Option<Lunar> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut offset = days_from_civil(y, m, d) - days_from_civil(BASE.0, BASE.1, BASE.2);
    if offset < 0 {
        return None;
    }
    // 逐年减，直到落在某一年里
    let mut year = FIRST_YEAR;
    loop {
        let info = year_info(year)?;
        let mut year_days = 0;
        for i in 0..info.months {
            year_days += month_days(&info, i) as i64;
        }
        if offset < year_days {
            break;
        }
        offset -= year_days;
        year += 1;
    }
    // 逐月减
    let info = year_info(year)?;
    let mut month_idx0 = 0u32;
    for i in 0..info.months {
        let len = month_days(&info, i) as i64;
        if offset < len {
            month_idx0 = i;
            break;
        }
        offset -= len;
    }
    let day = offset as u32 + 1;

    // 月份编号（.NET 口径：闰月占一个序号，紧跟在它前面那个平月之后）
    let ordinal = month_idx0 + 1;
    let is_leap = info.leap > 0 && ordinal == info.leap + 1;
    let plain = if info.leap > 0 && ordinal > info.leap {
        ordinal - 1
    } else {
        ordinal
    };
    let mut month_name = MONTH_NAMES
        .get((plain.saturating_sub(1)) as usize)
        .copied()
        .unwrap_or("正月")
        .to_string();
    if is_leap {
        month_name.insert_str(0, "闰");
    }

    let gz = (((year - 4) % 60) + 60) % 60;
    Some(Lunar {
        year,
        month_name,
        day_name: day_name(day),
        ganzhi: format!("{}{}", GAN[(gz % 10) as usize], ZHI[(gz % 12) as usize]),
        zodiac: ZODIAC[(gz % 12) as usize].to_string(),
        is_leap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 春节（正月初一）必须对得上 —— 这几天的公历日期是公开事实，用来钉住数据表。
    #[test]
    fn spring_festivals() {
        for (y, m, d) in [
            (2023, 1, 22),
            (2024, 2, 10),
            (2025, 1, 29),
            (2026, 2, 17),
        ] {
            let l = to_lunar(y, m, d).expect("表范围内");
            assert_eq!(l.month_name, "正月", "{y}-{m}-{d} 应当是正月初一");
            assert_eq!(l.day_name, "初一", "{y}-{m}-{d} 应当是正月初一");
            assert_eq!(l.year, y as i32);
        }
    }

    /// 与 .NET 日历逐条核对过的抽样点（数据表的来源与它同一份）。
    #[test]
    fn matches_reference_samples() {
        let cases: [((i32, u32, u32), (i32, &str, &str)); 5] = [
            ((1901, 2, 19), (1901, "正月", "初一")),
            ((2025, 7, 25), (2025, "闰六月", "初一")),
            ((2026, 10, 15), (2026, "九月", "初六")),
            ((2033, 12, 22), (2033, "闰冬月", "初一")),
            // 注意：这里写的是**传统月名**。2099 年有闰二月，所以 .NET 口径的第 12 个月
            // 落到传统叫法的「冬月」（闰月插在二月后，后面的月份序号都往后挪了一位）。
            ((2100, 1, 1), (2099, "冬月", "廿一")),
        ];
        for ((y, m, d), (ly, mn, dn)) in cases {
            let l = to_lunar(y, m, d).expect("表范围内");
            assert_eq!((l.year, l.month_name.as_str(), l.day_name.as_str()), (ly, mn, dn), "{y}-{m}-{d}");
        }
    }

    #[test]
    fn ganzhi_and_zodiac() {
        // 2026 年是丙午（马）年
        let l = to_lunar(2026, 10, 15).unwrap();
        assert_eq!(l.ganzhi, "丙午");
        assert_eq!(l.zodiac, "马");
        // 1901 年是辛丑（牛）年 —— 表起点那天
        let l = to_lunar(1901, 2, 19).unwrap();
        assert_eq!(l.ganzhi, "辛丑");
        assert_eq!(l.zodiac, "牛");
    }

    #[test]
    fn out_of_range_is_none() {
        assert!(to_lunar(1900, 1, 1).is_none());
        // 表里最后一年是农历 2100 年，它一直排到公历 2101-01-28
        assert!(to_lunar(2101, 6, 1).is_none(), "超出表范围应当返回 None");
    }

    #[test]
    fn day_names_are_traditional() {
        assert_eq!(day_name(1), "初一");
        assert_eq!(day_name(10), "初十");
        assert_eq!(day_name(15), "十五");
        assert_eq!(day_name(20), "二十");
        assert_eq!(day_name(21), "廿一");
        assert_eq!(day_name(30), "三十");
    }
}
