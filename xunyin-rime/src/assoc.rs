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
    crate::config::user_data_dir().join("assoc.txt")
}

/// 联想只针对“词”：至少含一个字母、数字或汉字。
///
/// 纯标点/符号/空格不能当联想的前词或后词 —— 否则会学出
/// `。 -> ‘`、`一下 -> ，` 这类垃圾，打标点时服务会把联想候选一起返回，
/// 候选窗先闪一下（随后因为组合已结束又隐藏），看起来像“标点没有直接上屏”。
pub fn is_wordish(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric())
}

/// 是否值得学习这条关系：太长的不学（句子级联想噪声大）。
fn learnable(prev: &str, next: &str) -> bool {
    !prev.is_empty()
        && !next.is_empty()
        && prev != next
        && is_wordish(prev)
        && is_wordish(next)
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
        // 旧版可能把标点当词学进来；加载时直接丢掉，下次 save 会顺手清理文件
        if !is_wordish(prev) || !is_wordish(next) {
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
///
/// 旧版本可能已经把标点学进 `assoc.txt`（`。 -> ‘` 之类）；这里在展示时
/// 一并过滤掉，升级后不必手工清表也不会再冒出标点候选。
pub fn followers(prev: &str) -> Vec<String> {
    // 前词本身不是“词”（标点/符号）就不该有联想 —— 旧版 assoc.txt 里可能还留着
    // `， -> 包括` 这样的记录，直接在入口挡掉。
    if prev.is_empty() || !is_wordish(prev) {
        return Vec::new();
    }
    with_table(|t| {
        let mut v = t.get(prev).cloned().unwrap_or_default();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.into_iter()
            .map(|(w, _)| w)
            .filter(|w| is_wordish(w))
            .take(SHOW_MAX)
            .collect()
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

    /// 旧文件里的「标点 -> 词」记录在加载时就被丢弃（下次 save 会清理磁盘）。
    #[test]
    fn parse_drops_punctuation_rows() {
        let t = parse("，\t包括\t9\n。\t‘\t2\n你好\t世界\t3\n");
        assert!(!t.contains_key("，"));
        assert!(!t.contains_key("。"));
        assert_eq!(t.get("你好").unwrap()[0], ("世界".to_string(), 3));
    }

    #[test]
    fn rejects_noise() {
        assert!(!learnable("", "x"));
        assert!(!learnable("abc", ""));
        assert!(!learnable("word", "word"));
        assert!(!learnable("x", "a b")); // 带空格不学
    }

    /// 标点不是“词”：既不学习，也不作为联想候选返回。
    #[test]
    fn punctuation_is_not_associable() {
        assert!(!is_wordish("，"));
        assert!(!is_wordish("。"));
        assert!(!is_wordish("……"));
        assert!(!is_wordish("😀"));
        assert!(is_wordish("你好"));
        assert!(is_wordish("hello"));
        assert!(is_wordish("123"));

        assert!(!learnable("，", "。"));
        assert!(!learnable("你好", "，"));
        assert!(learnable("你好", "世界"));

        // 旧数据里以标点为前词的记录也不能再产生联想
        assert!(followers("，").is_empty());
        assert!(followers("。").is_empty());

        // 模拟旧版 assoc.txt 里的脏数据：展示时必须把标点过滤掉
        let prev = "测试键_标点过滤zzq";
        with_table(|t| {
            t.insert(
                prev.to_string(),
                vec![("，".to_string(), 9), ("世界".to_string(), 1)],
            );
        });
        assert_eq!(followers(prev), vec!["世界".to_string()]);
    }
}
