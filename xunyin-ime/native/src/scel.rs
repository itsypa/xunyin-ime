//! 搜狗细胞词库（`.scel`）解析：让用户能把手里的搜狗词库直接导进迅音。
//!
//! # 文件结构
//!
//! 下面这套结构是用**搜狗输入法 16.9 安装包自带的那份词库**（`scd\60393.scel`）
//! 实测出来的，不是照抄文档：
//!
//! ```text
//! 0x00   u32            头部长度（实测 0x1540）
//! …                     头部里含词库名等信息（UTF-16）
//! 头部之后              拼音表：u32 条数，其后每条 = u16 字节长度 + UTF-16 音节
//! 拼音表之后            词区：u16 拼音索引[k] + u16 词字节长度(=2k)
//!                                 + k × UTF-16 汉字 + u16 词频
//! ```
//!
//! 词区里**每条记录之间还夹着长度不固定的附加数据**（含义不明，实测有词频、标志位
//! 和一批零/随机值）。所以这里不是"顺着读"，而是按记录特征扫描：长度字段必须等于
//! 2k、紧跟 k 个汉字、且其前正好有 k 个合法拼音索引（都小于拼音表条数）。三条同时
//! 成立才收下，错认的概率极低 —— 在样例上取出的记录与我们的字表注音逐字核对，一致
//! （不一致的几处都是多音字，属正常）。

/// 一个词条：词 + 输入码（拼音音节直接相连，例如 `nihao`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub word: String,
    pub code: String,
}

/// 单个词最多多少字（超过就当成误匹配）。
const MAX_WORD_CHARS: usize = 32;
/// 拼音表条数上限（样例 439，留足余量）。
const MAX_SYLLABLES: u32 = 4096;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(o)?, *b.get(o + 1)?]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

/// UTF-16LE 取 `units` 个码元。
fn utf16_at(b: &[u8], o: usize, units: usize) -> Option<String> {
    let end = o.checked_add(units * 2)?;
    let raw = b.get(o..end)?;
    let mut s = String::with_capacity(units);
    for ch in raw.chunks_exact(2) {
        s.push(char::from_u32(u16::from_le_bytes([ch[0], ch[1]]) as u32)?);
    }
    Some(s)
}

/// 汉字（含扩展 A 区）。词库里出现生僻字很正常。
fn is_hanzi(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// 解析一份 `.scel`，返回词条列表（按文件里的顺序，已按词去重）。
pub fn parse(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    if bytes.len() < 16 {
        return Err("文件太小，不像搜狗细胞词库".into());
    }
    let hdr = u32_at(bytes, 0).ok_or("读头部失败")? as usize;
    if hdr < 8 || hdr >= bytes.len() {
        return Err(format!("头部长度 {hdr} 不合理，不像搜狗细胞词库"));
    }

    // ── 拼音表 ──
    let count = u32_at(bytes, hdr).ok_or("读拼音表失败")?;
    if count == 0 || count > MAX_SYLLABLES {
        return Err(format!("拼音表条数 {count} 不合理"));
    }
    let mut syl: Vec<String> = Vec::with_capacity(count as usize);
    let mut p = hdr + 4;
    for i in 0..count {
        // 每条 = u16 序号 + u16 字节长度 + UTF-16 音节（序号跟着走，正好当校验用）
        let idx = u16_at(bytes, p).ok_or("拼音表被截断")?;
        if idx as u32 != i {
            return Err(format!("拼音表第 {i} 条序号是 {idx}，结构对不上"));
        }
        let n_bytes = u16_at(bytes, p + 2).ok_or("拼音表被截断")? as usize;
        if n_bytes == 0 || n_bytes > 64 || n_bytes % 2 != 0 {
            return Err(format!("拼音表第 {i} 条长度 {n_bytes} 不合理"));
        }
        let s = utf16_at(bytes, p + 4, n_bytes / 2).ok_or("拼音表被截断")?;
        // 音节只可能是小写字母（含 ' 的方言音节也给过）
        if !s.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') {
            return Err(format!("拼音表第 {i} 条不是音节：{s:?}"));
        }
        syl.push(s.to_lowercase());
        p += 4 + n_bytes;
    }
    let body = p;

    // ── 词区：按记录特征扫描 ──
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut q = body;
    while q + 6 <= bytes.len() {
        let Some(len_raw) = u16_at(bytes, q) else { break };
        let len_bytes = len_raw as usize;
        let k = len_bytes / 2;
        let ok_len = len_raw > 0
            && len_bytes % 2 == 0
            && k <= MAX_WORD_CHARS
            && q + 2 + len_bytes + 2 <= bytes.len()
            && q >= body + 2 * k;
        if !ok_len {
            q += 2;
            continue;
        }
        // 必须紧跟 k 个汉字
        let Some(word) = utf16_at(bytes, q + 2, k) else {
            q += 2;
            continue;
        };
        if !word.chars().all(is_hanzi) {
            q += 2;
            continue;
        }
        // 词前面那 k 个 u16 必须都是合法拼音索引
        let mut code = String::with_capacity(k * 4);
        let mut idx_ok = true;
        for i in 0..k {
            match u16_at(bytes, q - 2 * k + 2 * i) {
                Some(ix) if (ix as u32) < count => code.push_str(&syl[ix as usize]),
                _ => {
                    idx_ok = false;
                    break;
                }
            }
        }
        if !idx_ok {
            q += 2;
            continue;
        }
        if seen.insert(word.clone()) {
            out.push(Entry { word, code });
        }
        // 跳过本条的 长度字段 + 词 + 词频
        q += 2 + len_bytes + 2;
    }

    if out.is_empty() {
        return Err("没解析出任何词条：可能不是搜狗细胞词库，或版本格式不同".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按实测结构造一份最小的 `.scel`，用来做往返测试。
    fn build(words: &[(&str, &[u16])]) -> Vec<u8> {
        let syllables: Vec<&str> = vec!["ni", "hao", "shi", "jie"];
        let mut out: Vec<u8> = Vec::new();
        let hdr = 0x1540usize; // 头部长度用样例的同一量级，中间补零
        out.extend_from_slice(&(hdr as u32).to_le_bytes());
        out.resize(hdr, 0);
        // 拼音表
        out.extend_from_slice(&(syllables.len() as u32).to_le_bytes());
        for (i, s) in syllables.iter().enumerate() {
            out.extend_from_slice(&(i as u16).to_le_bytes());
            let u: Vec<u16> = s.encode_utf16().collect();
            out.extend_from_slice(&((u.len() * 2) as u16).to_le_bytes());
            for c in u {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        // 词区
        for (word, idx) in words {
            for ix in *idx {
                out.extend_from_slice(&ix.to_le_bytes());
            }
            let u: Vec<u16> = word.encode_utf16().collect();
            out.extend_from_slice(&((u.len() * 2) as u16).to_le_bytes());
            for c in u {
                out.extend_from_slice(&c.to_le_bytes());
            }
            out.extend_from_slice(&37u16.to_le_bytes()); // 词频
            out.extend_from_slice(&[0u8; 4]); // 记录之间的附加数据
        }
        out
    }

    #[test]
    fn round_trip() {
        let f = build(&[("你好", &[0, 1]), ("世界", &[2, 3])]);
        let got = parse(&f).expect("应当解析成功");
        assert_eq!(
            got,
            vec![
                Entry { word: "你好".into(), code: "nihao".into() },
                Entry { word: "世界".into(), code: "shijie".into() },
            ]
        );
    }

    #[test]
    fn dedupes_same_word() {
        let f = build(&[("你好", &[0, 1]), ("你好", &[0, 1]), ("世界", &[2, 3])]);
        let got = parse(&f).expect("应当解析成功");
        assert_eq!(got.len(), 2, "同一个词只该留一条");
    }

    #[test]
    fn rejects_text_files() {
        let e = parse(b"hello\tworld\n").unwrap_err();
        assert!(e.contains("不像") || e.contains("拼音表"), "错误说明要能看懂：{e}");
    }

    /// 拿真实的搜狗词库验一把（本机分析时留下的样例；没有就跳过，不当失败）。
    ///
    /// 这条测试守的是"结构理解有没有跑偏"：真实文件的词区里夹着附加数据，
    /// 只有按记录特征扫描才取得出来。
    #[test]
    fn parses_real_sogou_sample_if_present() {
        for p in [
            r"D:\ref-sogou\payload\16.9.0.5535\scd\60393.scel",
            r"D:\pinyin_guanwang_16.9b.exe",
        ] {
            let path = std::path::Path::new(p);
            if !path.is_file() || p.ends_with(".exe") {
                continue;
            }
            let bytes = std::fs::read(path).unwrap();
            let got = parse(&bytes).expect("真实的搜狗词库应当能解析");
            assert!(got.len() > 80, "样例里应当有上百个词条，实际 {}", got.len());
            // 抽查：词条拼音必须是音节拼起来的，且长度对得上
            for e in got.iter().take(20) {
                assert!(!e.code.is_empty());
                assert!(e.code.chars().all(|c| c.is_ascii_alphabetic() || c == '\''));
            }
            assert!(got.iter().any(|e| e.word == "艾巴优教育"), "样例里应当有「艾巴优教育」");
            return;
        }
    }
}
