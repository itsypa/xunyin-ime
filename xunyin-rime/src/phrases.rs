//! 自定义短语：用户自己维护的「编码 → 词条」表。
//!
//! 文件就是 Rime 的 `custom_phrase.txt`（用户数据目录下），格式沿用它的约定：
//! 每行 `词条<TAB>编码[<TAB>权重]`，编码可以写多个（空格分隔），`#` 开头是注释。
//!
//! 为什么这里还要自己读一遍：Rime 把这张表**编译进 schema**，改一次要重新部署
//! （分钟级）。常驻服务拿着同一份文件，命中就把短语作为候选插到最前——加完立刻能用；
//! 等下次部署完成，Rime 自己也认这些短语（服务侧的叠加候选会按文本去重，不会出现两个）。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// 一条短语。编码可含多个（空格分隔），任一命中即算命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 上屏文本
    pub text: String,
    /// 编码（可能多个，空格分隔）
    pub codes: String,
    /// 权重（可空；Rime 里越大的越靠前）
    pub weight: Option<i32>,
}

/// 文件里的一行：注释原样保留（用户手写的注释不因为一次编辑就丢掉）。
#[derive(Debug, Clone)]
pub enum Row {
    Comment(String),
    Entry(Entry),
}

/// 短语文件路径（与设置中心、Rime 共用同一份）。
pub fn path() -> PathBuf {
    crate::config::user_data_dir().join("custom_phrase.txt")
}

/// 解析一行；不是有效条目返回 None。
fn parse_line(line: &str) -> Option<Entry> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    // 有 TAB 就按 TAB 切：Rime 的格式是「词条<TAB>编码<TAB>权重」，编码字段内部
    // 还可以有多个编码（空格分隔）。没 TAB（用户手写的空格分隔）时，末尾那个能解析成
    // 整数的当成权重，其余都算编码。
    let (text, codes, weight) = if t.contains('\t') {
        let f: Vec<&str> = t
            .split('\t')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if f.len() < 2 {
            return None;
        }
        (
            f[0].to_string(),
            f[1].to_string(),
            f.get(2).and_then(|w| w.parse::<i32>().ok()),
        )
    } else {
        let f: Vec<&str> = t.split_whitespace().collect();
        if f.len() < 2 {
            return None;
        }
        let last_is_num = f.len() >= 3 && f[f.len() - 1].parse::<i32>().is_ok();
        let (codes_end, weight) = if last_is_num {
            (
                f.len() - 1,
                f[f.len() - 1].parse::<i32>().ok(),
            )
        } else {
            (f.len(), None)
        };
        (f[0].to_string(), f[1..codes_end].join(" "), weight)
    };
    Some(Entry {
        text,
        codes,
        weight,
    })
}

/// 一行编码是否命中（大小写不敏感；编码字段里可能有多个）。
fn entry_matches(e: &Entry, code: &str) -> bool {
    e.codes
        .split_whitespace()
        .any(|c| c.eq_ignore_ascii_case(code))
}

/// 解析整份内容（保留注释行）。
pub fn parse_rows(content: &str) -> Vec<Row> {
    let mut out = Vec::new();
    for raw_line in content.lines() {
        // 编辑器/Excel 导出的文件常在开头带 UTF-8 BOM；不去掉的话第一条注释会被
        // 当成"词条 + 编码"混进短语表
        let line = raw_line.trim_start_matches('\u{FEFF}');
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('#') {
            out.push(Row::Comment(line.to_string()));
            continue;
        }
        match parse_line(line) {
            Some(e) => out.push(Row::Entry(e)),
            // 解析不了的行当注释留着，别静默丢用户数据
            None => out.push(Row::Comment(line.to_string())),
        }
    }
    out
}

/// 条目序列化回文件内容。
pub fn format_rows(rows: &[Row]) -> String {
    let mut s = String::from("# 迅音输入法 自定义短语（与 Rime 的 custom_phrase.txt 同一份）\n");
    s.push_str("# 格式：词条<TAB>编码[<TAB>权重]；编码可写多个（空格分隔）\n");
    for r in rows {
        match r {
            Row::Comment(c) => {
                s.push_str(c);
                s.push('\n');
            }
            Row::Entry(e) => {
                s.push_str(&e.text);
                s.push('\t');
                s.push_str(&e.codes);
                if let Some(w) = e.weight {
                    s.push('\t');
                    s.push_str(&w.to_string());
                }
                s.push('\n');
            }
        }
    }
    s
}

/// 读整份文件（不存在返回空）。
pub fn read_rows() -> Vec<Row> {
    std::fs::read_to_string(path())
        .map(|s| parse_rows(&s))
        .unwrap_or_default()
}

/// 写回整份文件（先写临时文件再改名，避免写一半被服务读到）。
pub fn write_rows(rows: &[Row]) -> Result<(), String> {
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("txt.tmp");
    std::fs::write(&tmp, format_rows(rows)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    invalidate_cache();
    Ok(())
}

// ---------------- 服务侧热路径：带 mtime 缓存的查询 ----------------

struct Cache {
    at: Instant,
    mtime: Option<SystemTime>,
    entries: Vec<Entry>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// 缓存失效（设置中心写完文件后、或服务收到重载请求时调用）。
pub fn invalidate_cache() {
    if let Ok(mut g) = CACHE.lock() {
        *g = None;
    }
}

/// 取当前条目（每 2 秒最多 stat 一次文件，文件变了就重读）。
fn entries_cached() -> Vec<Entry> {
    let mtime = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
    if let Ok(g) = CACHE.lock() {
        if let Some(c) = g.as_ref() {
            if c.at.elapsed() < Duration::from_secs(2) && c.mtime == mtime {
                return c.entries.clone();
            }
        }
    }
    let entries: Vec<Entry> = read_rows()
        .into_iter()
        .filter_map(|r| match r {
            Row::Entry(e) => Some(e),
            Row::Comment(_) => None,
        })
        .collect();
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(Cache {
            at: Instant::now(),
            mtime,
            entries: entries.clone(),
        });
    }
    entries
}

/// 编码精确命中哪些短语（按权重从大到小，权重相同保持文件顺序）。
///
/// `input` 是组合串去掉音节分隔符后的原始输入（例如 "nh"、"nidemingzi"）。
pub fn matching(input: &str) -> Vec<String> {
    let code = input.trim();
    if code.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(usize, i32, String)> = entries_cached()
        .into_iter()
        .enumerate()
        .filter(|(_, e)| entry_matches(e, code))
        .map(|(i, e)| (i, e.weight.unwrap_or(0), e.text))
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut seen = std::collections::HashSet::new();
    hits.into_iter()
        .map(|(_, _, t)| t)
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// 追加一条（导入用：已有同「词条+编码」的不重复加）。
pub fn append(entry: &Entry) -> Result<bool, String> {
    let mut rows = read_rows();
    let dup = rows.iter().any(|r| match r {
        Row::Entry(e) => e.text == entry.text && e.codes == entry.codes,
        Row::Comment(_) => false,
    });
    if dup {
        return Ok(false);
    }
    rows.push(Row::Entry(entry.clone()));
    write_rows(&rows)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rime_format() {
        let content = "# 注释\n你好\tnh nihao\n世界\tsj\t100\n坏行\n";
        let rows = parse_rows(content);
        assert_eq!(rows.len(), 4);
        match &rows[1] {
            Row::Entry(e) => {
                assert_eq!(e.text, "你好");
                // 编码字段里的多个编码要完整保留（"nh nihao" 是两个编码）
                assert_eq!(e.codes, "nh nihao");
                assert_eq!(e.weight, None);
                assert!(entry_matches(e, "nh") && entry_matches(e, "nihao"));
            }
            _ => panic!("第 2 行应是条目"),
        }
        match &rows[2] {
            Row::Entry(e) => {
                assert_eq!(e.text, "世界");
                assert_eq!(e.weight, Some(100));
            }
            _ => panic!("第 3 行应是条目"),
        }
        // 解析不了的行按注释保留，不丢用户数据
        assert!(matches!(&rows[3], Row::Comment(c) if c == "坏行"));
    }

    #[test]
    fn strips_utf8_bom() {
        // BOM 开头的文件：第一条注释不能被当成短语
        let content = "\u{FEFF}# 我的短语\n你好\tnh\n";
        let rows = parse_rows(content);
        assert_eq!(rows.len(), 2);
        assert!(matches!(&rows[0], Row::Comment(c) if c.contains("我的短语")));
        assert!(matches!(&rows[1], Row::Entry(e) if e.text == "你好"));
    }

    #[test]
    fn formats_roundtrip() {
        let rows = vec![
            Row::Comment("# 我的短语".into()),
            Row::Entry(Entry {
                text: "你好".into(),
                codes: "nh nihao".into(),
                weight: Some(7),
            }),
        ];
        let text = format_rows(&rows);
        let back = parse_rows(&text);
        let entries: Vec<&Entry> = back
            .iter()
            .filter_map(|r| match r {
                Row::Entry(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "你好");
        assert_eq!(entries[0].codes, "nh nihao");
        assert_eq!(entries[0].weight, Some(7));
        assert!(back.iter().any(|r| matches!(r, Row::Comment(c) if c.contains("我的短语"))));
    }

    #[test]
    fn entry_matches_any_code_case_insensitively() {
        let e = Entry {
            text: "你好".into(),
            codes: "nh NH nihao".into(),
            weight: None,
        };
        assert!(entry_matches(&e, "nh"));
        assert!(entry_matches(&e, "Nh"));
        assert!(entry_matches(&e, "nihao"));
        assert!(!entry_matches(&e, "n"));
        assert!(!entry_matches(&e, "nihaoo"));
    }
}
