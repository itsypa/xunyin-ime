//! 词语联想（本地学习）：记住「上屏 A 之后紧接着上了屏 B」，下次 A 上屏后把 B 作为
//! 联想候选给出。纯本地、不联网，用一段时间后才逐渐有效（初始为空表）。
//!
//! 存储：`%APPDATA%\com.xunyin.ime\assoc.txt`，每行 `前词 \t 后词 \t 次数`。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// 每个前词最多保留的联想词数。
const PER_WORD: usize = 8;
/// 联想候选最多显示几个。
const SHOW_MAX: usize = 5;
/// 表规模上限（超出后按总次数裁剪）。
const MAX_KEYS: usize = 20_000;

type Table = HashMap<String, Vec<(String, u32)>>;

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

pub fn path() -> PathBuf {
    xunyin_rime::config::user_data_dir().join("assoc.txt")
}

/// 是否值得学习这条关系：太长的不学（句子级联想噪声大）。
fn learnable(prev: &str, next: &str) -> bool {
    !prev.is_empty()
        && !next.is_empty()
        && prev != next
        && prev.chars().count() <= 8
        && next.chars().count() <= 8
        && next.chars().all(|c| !c.is_whitespace())
}

fn parse(text: &str) -> Table {
    let mut t: Table = HashMap::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        let (Some(prev), Some(next), Some(cnt)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let Ok(cnt) = cnt.trim().parse::<u32>() else {
            continue;
        };
        if prev.is_empty() || next.is_empty() {
            continue;
        }
        t.entry(prev.to_string()).or_default().push((next.to_string(), cnt));
    }
    t
}

fn dump(t: &Table) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (prev, items) in t {
        for (next, cnt) in items {
            lines.push(format!("{prev}\t{next}\t{cnt}"));
        }
    }
    lines.sort();
    lines.join("\n")
}

fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    let mut guard = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let t = guard.get_or_insert_with(|| {
        let text = std::fs::read_to_string(path()).unwrap_or_default();
        parse(&text)
    });
    f(t)
}

/// 学一条「前词 → 后词」。返回 true 表示表有变化。
pub fn learn(prev: &str, next: &str) -> bool {
    if !learnable(prev, next) {
        return false;
    }
    with_table(|t| {
        let items = t.entry(prev.to_string()).or_default();
        match items.iter_mut().find(|(w, _)| w == next) {
            Some((_, c)) => *c += 1,
            None => items.push((next.to_string(), 1)),
        }
        items.sort_by(|a, b| b.1.cmp(&a.1));
        items.truncate(PER_WORD);
        true
    })
}

/// 取前词的联想候选（按次数降序）。
pub fn followers(prev: &str) -> Vec<String> {
    if prev.is_empty() {
        return Vec::new();
    }
    with_table(|t| {
        let mut v = t.get(prev).cloned().unwrap_or_default();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.into_iter().take(SHOW_MAX).map(|(w, _)| w).collect()
    })
}

/// 学习计数达到这个值就落盘（避免每次上屏都写文件）。
const SAVE_EVERY: u32 = 8;
static PENDING: Mutex<u32> = Mutex::new(0);

/// 记一笔学习并偶尔落盘（正常路径调用它，不要用 [`learn`] + [`save`] 手动组合）。
pub fn learn_and_maybe_save(prev: &str, next: &str) {
    if !learn(prev, next) {
        return;
    }
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    *p += 1;
    if *p >= SAVE_EVERY {
        *p = 0;
        drop(p);
        save();
    }
}

/// 落盘（表过大时先裁剪）。
pub fn save() -> bool {
    let text = with_table(|t| {
        if t.len() > MAX_KEYS {
            let mut keys: Vec<(String, u32)> = t
                .iter()
                .map(|(k, v)| (k.clone(), v.iter().map(|(_, c)| *c).sum::<u32>()))
                .collect();
            keys.sort_by(|a, b| b.1.cmp(&a.1));
            for (k, _) in keys.into_iter().skip(MAX_KEYS) {
                t.remove(&k);
            }
        }
        dump(t)
    });
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_err() {
        return false;
    }
    std::fs::rename(&tmp, &p).is_ok()
}

/// 清空（清空用户词库时一起调用）。
pub fn clear() {
    with_table(|t| t.clear());
    let _ = std::fs::remove_file(path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learn_and_recall() {
        // 用一个现实中不会出现的键，避免与用户真实词表冲突
        let prev = "测试键zzq";
        let next = "甲候选";
        let other = "乙候选";
        learn(prev, next);
        learn(prev, next);
        learn(prev, other);
        let f = followers(prev);
        assert_eq!(f.first().map(|s| s.as_str()), Some(next), "次数高的排前面");
        assert!(f.contains(&other.to_string()));
    }

    #[test]
    fn dump_parse_roundtrip() {
        let t = parse("A\tB\t3\nC\tD\t1\n");
        assert_eq!(t.get("A").unwrap()[0], ("B".to_string(), 3));
        let back = parse(&dump(&t));
        assert_eq!(back.get("C").unwrap()[0], ("D".to_string(), 1));
    }

    #[test]
    fn rejects_noise() {
        assert!(!learnable("", "x"));
        assert!(!learnable("abc", ""));
        assert!(!learnable("word", "word"));
        assert!(!learnable("x", "a b")); // 带空格不学
    }
}
