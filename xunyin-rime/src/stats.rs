//! 输入统计：上屏字数 / 次数 / 打字时长 / 速度（今日 + 累计）。
//!
//! 计数点在**常驻服务**里（所有上屏的文本都从那里回给宿主），所以统计天然准确到
//! "真正上屏了多少字"，而不是按键数。落盘一个 `stats.json`，跨天自动翻页。
//!
//! 速度怎么算：记的是"相邻两次上屏的间隔"，单次最多计 10 秒（中间去倒水不会把速度
//! 拉成个位数），再用「今日字数 ÷ 今日打字分钟」得出字/分钟。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 相邻两次上屏超过这个间隔就不计入打字时长（毫秒）。
const MAX_GAP_MS: u64 = 10_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    /// 今日日期（本地时区的 `YYYY-MM-DD`）。
    pub date: String,
    pub today_chars: u64,
    pub today_commits: u64,
    /// 今日打字时长（毫秒，按上屏间隔累计，单次封顶见 [`MAX_GAP_MS`]）。
    pub today_ms: u64,
    pub total_chars: u64,
    pub total_commits: u64,
    /// 累计使用天数（有上屏的日子数）。
    pub days: u32,
    /// 上次上屏时刻（毫秒时间戳，本地时钟）。不落盘，只用于算间隔。
    #[serde(skip)]
    pub last_ms: u64,
}

impl Stats {
    /// 今日速度（字/分钟）。打字时长不足 1 秒时返回 None（免得除出天文数字）。
    pub fn today_speed_cpm(&self) -> Option<u32> {
        if self.today_ms < 1000 {
            return None;
        }
        let minutes = self.today_ms as f64 / 60_000.0;
        Some((self.today_chars as f64 / minutes).round() as u32)
    }

    /// 今日打字时长（分钟，一位小数）。
    pub fn today_minutes(&self) -> f64 {
        (self.today_ms as f64 / 60_000.0 * 10.0).round() / 10.0
    }

    /// 记一次上屏。纯计算，方便测：`date` 是今天的日期串，`now_ms` 是本地毫秒时间戳。
    pub fn apply(&mut self, chars: u64, date: &str, now_ms: u64) {
        if chars == 0 {
            return;
        }
        if self.date != date {
            // 跨天：翻页。第一次记录时 days 从 0 变 1。
            let first_ever = self.date.is_empty();
            self.date = date.to_string();
            self.today_chars = 0;
            self.today_commits = 0;
            self.today_ms = 0;
            self.last_ms = 0;
            if !first_ever {
                self.days = self.days.saturating_add(1);
            }
        }
        if self.today_commits == 0 {
            // 今天第一次上屏也算一天
            self.days = self.days.max(1);
        }
        if self.last_ms != 0 && now_ms > self.last_ms {
            self.today_ms = self
                .today_ms
                .saturating_add((now_ms - self.last_ms).min(MAX_GAP_MS));
        }
        self.last_ms = now_ms;
        self.today_chars = self.today_chars.saturating_add(chars);
        self.today_commits = self.today_commits.saturating_add(1);
        self.total_chars = self.total_chars.saturating_add(chars);
        self.total_commits = self.total_commits.saturating_add(1);
    }
}

/// 统计文件（和配置、词库同目录）。
pub fn path() -> PathBuf {
    crate::config::user_data_dir().join("stats.json")
}

/// 今天的日期串（本地时区）。
pub fn today() -> String {
    let (y, m, d) = local_ymd();
    format!("{y:04}-{m:02}-{d:02}")
}

fn local_ymd() -> (u16, u16, u16) {
    // xunyin-rime 用的是 windows-sys（不引 windows crate）
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut st) };
    (st.wYear, st.wMonth, st.wDay)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn load() -> Stats {
    let mut s: Stats = std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    // 跨天后先把今天的数字清零再显示，免得界面显示昨天的成绩
    let today = today();
    if !s.date.is_empty() && s.date != today {
        s.date = today;
        s.today_chars = 0;
        s.today_commits = 0;
        s.today_ms = 0;
        s.last_ms = 0;
    }
    s
}

pub fn save(s: &Stats) {
    let dir = crate::config::user_data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(s) else {
        return;
    };
    let tmp = dir.join("stats.json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path());
    }
}

/// 记一次上屏（`chars` = 上屏字数）。返回更新后的统计。
pub fn record(chars: u64) -> Stats {
    let mut s = load();
    s.apply(chars, &today(), now_ms());
    save(&s);
    s
}

/// 清零（界面上的"清空统计"）。
pub fn reset() -> Stats {
    let s = Stats::default();
    save(&s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_and_rolls_over() {
        let mut s = Stats::default();
        s.apply(2, "2026-10-15", 1_000);
        s.apply(3, "2026-10-15", 4_000); // 间隔 3 秒
        assert_eq!(s.today_chars, 5);
        assert_eq!(s.today_commits, 2);
        assert_eq!(s.today_ms, 3_000);
        assert_eq!(s.total_chars, 5);
        assert_eq!(s.days, 1);

        // 跨天：今日清零，累计保留，天数 +1
        s.apply(4, "2026-10-16", 10_000);
        assert_eq!(s.date, "2026-10-16");
        assert_eq!(s.today_chars, 4);
        assert_eq!(s.today_commits, 1);
        assert_eq!(s.total_chars, 9);
        assert_eq!(s.days, 2);
    }

    #[test]
    fn long_gap_is_capped() {
        let mut s = Stats::default();
        s.apply(1, "2026-10-15", 1_000);
        s.apply(1, "2026-10-15", 1_000 + 10 * 60_000); // 中间歇了 10 分钟
        assert_eq!(s.today_ms, MAX_GAP_MS, "长间隔只按 10 秒计");
    }

    #[test]
    fn speed_and_zero_guard() {
        let mut s = Stats::default();
        s.apply(60, "2026-10-15", 1_000);
        assert_eq!(s.today_speed_cpm(), None, "时长不足 1 秒不给速度");
        // 再来一次，间隔按 10 秒上限计 → 10 秒里 120 字，即"爆发速度" 720 字/分
        s.apply(60, "2026-10-15", 61_000);
        assert_eq!(s.today_ms, MAX_GAP_MS);
        assert_eq!(s.today_speed_cpm(), Some(720));
        assert!(s.today_minutes() > 0.0);
    }
}
