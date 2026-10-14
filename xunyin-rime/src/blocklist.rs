//! 删词黑名单：`Ctrl+数字` 删掉的候选不再出现在候选表里。
//!
//! librime 自带的 `delete_candidate` 只动**用户词库**（学习词），词库原生词删不掉——
//! 实测删完还照样出现。所以这里再记一份自己的黑名单，在候选展示层过滤掉，
//! 做到「删了就真的不再出现」（搜狗那种效果）。
//!
//! 存储：`%APPDATA%\com.xunyin.ime\blocked.txt`，每行一个词。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_ENTRIES: usize = 20_000;

static BLOCKED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn path() -> PathBuf {
    crate::config::user_data_dir().join("blocked.txt")
}

fn with_set<R>(f: impl FnOnce(&mut HashSet<String>) -> R) -> R {
    let mut guard = BLOCKED.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(|| {
        std::fs::read_to_string(path())
            .map(|t| {
                t.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    });
    f(set)
}

/// 这个词是否已被删除（不再展示）。
pub fn is_blocked(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    with_set(|s| s.contains(word))
}

/// 把词加进黑名单并落盘。
pub fn block(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let added = with_set(|s| s.insert(word.to_string()));
    if added {
        save();
    }
    added
}

/// 落盘（超过上限时只保留最近的若干条）。
pub fn save() -> bool {
    let text = with_set(|s| {
        let mut v: Vec<String> = s.iter().cloned().collect();
        if v.len() > MAX_ENTRIES {
            v.sort();
            v.truncate(MAX_ENTRIES);
            *s = v.iter().cloned().collect();
        }
        v.join("\n")
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
    with_set(|s| s.clear());
    let _ = std::fs::remove_file(path());
}

/// 已删除的词数（设置页展示用）。
pub fn len() -> usize {
    with_set(|s| s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_lookup() {
        // 用独一无二的键：这个测试会真的写 blocked.txt（用户文件），
        // 所以既不能依赖"文件里没有它"，跑完也要把测试条目清掉。
        let w = format!(
            "测试词{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        assert!(!is_blocked(&w));
        assert!(block(&w));
        assert!(is_blocked(&w));
        assert!(!block(&w), "重复加入不算新增");
        assert!(!is_blocked(""));
        with_set(|s| {
            s.remove(&w);
        });
        save();
    }
}
