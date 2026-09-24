//! 灵犀输入法共享配置。
//!
//! 配置存于 `%APPDATA%\com.lingxi.ime\config.json`，
//! 由 TSF 输入服务（读取）与 Tauri 设置中心（读写）共用。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

/// 输入行为配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// 默认输入方案（schema_id）。
    pub schema: String,
    /// 全角/半角：true=全角，false=半角。
    pub full_shape: bool,
    /// 中英标点：true=英文标点。
    pub ascii_punct: bool,
    /// 中英模式：true=英文（直接上屏）。
    pub ascii_mode: bool,
    /// 每页候选数。
    pub page_size: u32,
    /// 候选提示音（预留，默认关闭）。
    pub beep: bool,
    /// 自定义中文标点：按键字符 → 上屏文本。
    ///
    /// 仅在**中文标点**模式（`ascii_punct = false`）下生效；命中的按键不再交给
    /// Rime 的 punctuator，而是先上屏当前组合、再上屏这里配置的文本。
    /// 留空（默认）即完全沿用 Rime 内置标点表。
    pub punct_map: Vec<PunctPair>,
    /// 回车键行为：`commit`=把拼音原样上屏（默认，与 Shift 一致）｜
    /// `clear`=丢弃当前输入｜`newline`=丢弃输入并换行（把回车交给宿主）。
    pub enter_behavior: String,
    /// 中英切换键：`shift`=裸按 Shift（默认）｜`ctrl_space`=Ctrl+空格｜`none`=关闭。
    pub toggle_key: String,
    /// 拼音音节分隔符：`'`（默认）｜`-`｜`,`｜空格｜空字符串（不显示分隔）。
    pub pinyin_separator: String,
    /// 模糊音：启用的等价组 id 列表（见 [`InputConfig::fuzzy_rules`]）。
    pub fuzzy: Vec<String>,
    /// 用户词典自学习（记新词 / 调频）总开关。
    pub enable_user_dict: bool,
}

/// 一条自定义标点映射。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunctPair {
    /// 按键：单个 ASCII 字符（如 "," "." "?" ";" "[" "]" "~" 等）。
    pub key: String,
    /// 上屏文本（可多字，如 "——" 或 "……"）。
    pub out: String,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            schema: "lingxi_rich".into(),
            // 中文输入默认全角（标点/空格全宽），英文（ascii_mode）天然半角。
            full_shape: true,
            ascii_punct: false,
            ascii_mode: false,
            page_size: 5,
            beep: false,
            punct_map: Vec::new(),
            enter_behavior: "commit".into(),
            toggle_key: "shift".into(),
            pinyin_separator: "'".into(),
            fuzzy: Vec::new(),
            enable_user_dict: true,
        }
    }
}

/// 模糊音可选组：id、设置页显示名、对应的 Rime algebra 规则。
///
/// 规则用 `derive`（派生读音）而不是 `xform`（改写读音）：模糊音应当**同时**
/// 保留标准读音，即"zhu/zhu 都能出主/猪"，而不是把 zhu 变成 zu。
pub const FUZZY_GROUPS: &[(&str, &str, &[&str])] = &[
    ("zh_z", "zh ↔ z（知/资）", &["derive/^zh/z/", "derive/^z/zh/"]),
    ("ch_c", "ch ↔ c（吃/次）", &["derive/^ch/c/", "derive/^c/ch/"]),
    ("sh_s", "sh ↔ s（诗/思）", &["derive/^sh/s/", "derive/^s/sh/"]),
    ("n_l", "n ↔ l（南/兰）", &["derive/^n/l/", "derive/^l/n/"]),
    ("f_h", "f ↔ h（福/湖）", &["derive/^f/h/", "derive/^h/f/"]),
    ("r_l", "r ↔ l（热/乐）", &["derive/^r/l/", "derive/^l/r/"]),
    (
        "an_ang",
        "an ↔ ang（三/桑）",
        &["derive/(.*)an$/$1ang/", "derive/(.*)ang$/$1an/"],
    ),
    (
        "en_eng",
        "en ↔ eng（森/僧）",
        &["derive/(.*)en$/$1eng/", "derive/(.*)eng$/$1en/"],
    ),
    (
        "in_ing",
        "in ↔ ing（金/京）",
        &["derive/(.*)in$/$1ing/", "derive/(.*)ing$/$1in/"],
    ),
    (
        "ian_iang",
        "ian ↔ iang（先/香）",
        &["derive/(.*)ian$/$1iang/", "derive/(.*)iang$/$1ian/"],
    ),
    (
        "uan_uang",
        "uan ↔ uang（专/装）",
        &["derive/(.*)uan$/$1uang/", "derive/(.*)uang$/$1uan/"],
    ),
];

impl InputConfig {
    /// 查自定义标点：命中返回 Some(输出文本)；未命中/空配置返回 None（走 Rime 内置）。
    pub fn punct_lookup(&self, key: char) -> Option<String> {
        if self.punct_map.is_empty() {
            return None;
        }
        let mut k = String::new();
        k.push(key);
        self.punct_map
            .iter()
            .find(|p| p.key == k)
            .map(|p| p.out.clone())
    }

    /// 有效分隔符（非法值一律回退默认 `'`）。
    pub fn separator(&self) -> &'static str {
        match self.pinyin_separator.as_str() {
            "-" => "-",
            "," => ",",
            " " => " ",
            "" => "",
            _ => "'",
        }
    }

    /// 启用的模糊音规则（按 FUZZY_GROUPS 顺序展开）。
    pub fn fuzzy_rules(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (id, _, rules) in FUZZY_GROUPS {
            if self.fuzzy.iter().any(|f| f == id) {
                out.extend(rules.iter().map(|r| (*r).to_string()));
            }
        }
        out
    }

    /// 生成 `<schema>.custom.yaml` 的内容（模糊音 + 用户词典开关 + 基础 algebra）。
    ///
    /// `base_algebra` 为随包 schema 里 `speller/algebra` 的原始规则：Rime 的 `patch`
    /// 对列表是**整体替换**，所以必须把基础规则一并写回，否则模糊音会把缩略/纠错等
    /// 内置规则挤掉。
    pub fn schema_patch_yaml(&self, base_algebra: &[String]) -> Option<String> {
        let fuzzy = self.fuzzy_rules();
        if fuzzy.is_empty() && self.enable_user_dict {
            return None; // 无需补丁
        }
        let mut s = String::from("# 由灵犀输入法设置中心生成；手工修改会在下次保存设置时被覆盖。\npatch:\n");
        if !fuzzy.is_empty() {
            s.push_str("  speller/algebra:\n");
            for r in fuzzy.iter().chain(base_algebra.iter()) {
                s.push_str("    - ");
                s.push_str(r);
                s.push('\n');
            }
        }
        if !self.enable_user_dict {
            s.push_str("  translator/enable_user_dict: false\n");
        }
        Some(s)
    }
}

/// 候选窗外观配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// 候选文字字号（pt）。
    pub font_size: u32,
    /// 主题："light" | "dark" | "custom"。
    pub theme: String,
    /// 主色调（选中高亮）。
    pub accent: String,
    /// 背景色。
    pub bg: String,
    /// 正文颜色。
    pub text: String,
    /// 选中项背景色。
    pub selected_bg: String,
    /// 选中项文字颜色。
    pub selected_text: String,
    /// 注释（拼音）颜色。
    pub comment: String,
    /// 边框颜色。
    pub border: String,
    /// 圆角半径（px，仅自绘候选窗使用）。
    pub corner_radius: u32,
    /// 窗口不透明度（0-100）。
    pub opacity: u8,
    /// 是否在候选窗显示序号。
    pub show_index: bool,
    /// 是否显示右下角状态栏。
    pub show_status_bar: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            font_size: 13,
            theme: "light".into(),
            accent: "#2A6FDB".into(),
            bg: "#FFFFFF".into(),
            text: "#222222".into(),
            selected_bg: "#2A6FDB".into(),
            selected_text: "#FFFFFF".into(),
            comment: "#999999".into(),
            border: "#C8C8C8".into(),
            corner_radius: 6,
            opacity: 100,
            show_index: true,
            show_status_bar: true,
        }
    }
}

/// 词库配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictConfig {
    /// 自定义短语文件名（位于用户数据目录）。
    pub custom_phrase_file: String,
}

impl Default for DictConfig {
    fn default() -> Self {
        Self {
            custom_phrase_file: "custom_phrase.txt".into(),
        }
    }
}

/// 顶层配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LingxiConfig {
    pub input: InputConfig,
    pub appearance: AppearanceConfig,
    pub dict: DictConfig,
    /// 配置版本。
    pub version: u32,
}

impl Default for LingxiConfig {
    fn default() -> Self {
        Self {
            input: InputConfig::default(),
            appearance: AppearanceConfig::default(),
            dict: DictConfig::default(),
            version: 1,
        }
    }
}

/// 用户数据目录：`%APPDATA%\com.lingxi.ime`（与 TSF 引擎/设置中心共用）。
pub fn user_data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.lingxi.ime")
}

/// 配置文件路径。
pub fn config_path() -> PathBuf {
    user_data_dir().join("config.json")
}

/// 从随包 schema（`<schema_id>.schema.yaml`）里取出 `speller/algebra` 的规则列表。
///
/// 只做缩进解析（不引入 YAML 依赖）：定位 `speller:` 段下的 `algebra:`，收集其后
/// 缩进更深的 `- xxx` 行，直到遇到同级或更浅的键。
pub fn read_schema_algebra(schema_path: &std::path::Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(schema_path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_speller = false;
    let mut in_algebra = false;
    let mut algebra_indent = 0usize;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if !in_speller {
            if trimmed == "speller:" && indent == 0 {
                in_speller = true;
            }
            continue;
        }
        if !in_algebra {
            if indent == 0 {
                break; // 出了 speller 段
            }
            if trimmed == "algebra:" {
                in_algebra = true;
                algebra_indent = indent;
            }
            continue;
        }
        if indent <= algebra_indent {
            break; // algebra 列表结束
        }
        if let Some(rule) = trimmed.strip_prefix("- ") {
            // 去掉可能的行内注释与引号
            let rule = rule.split(" #").next().unwrap_or(rule).trim();
            let rule = rule.trim_matches('"').trim_matches('\'');
            if !rule.is_empty() {
                out.push(rule.to_string());
            }
        }
    }
    out
}

/// 从磁盘读取配置；文件不存在或损坏时返回默认值。
pub fn load_config() -> LingxiConfig {
    match fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => LingxiConfig::default(),
    }
}

/// 写入配置（原子替换）。
pub fn save_config(cfg: &LingxiConfig) -> std::io::Result<()> {
    let dir = user_data_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let tmp = dir.join("config.json.tmp");
    let data = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// 带缓存的配置读取（用于 TSF 高频路径）：
/// 仅在文件修改后重新解析，最多每 1 秒检查一次。
pub struct CachedConfig {
    last_check: Mutex<(std::time::SystemTime, LingxiConfig)>,
}

impl CachedConfig {
    pub fn new() -> Self {
        Self {
            last_check: Mutex::new((std::time::UNIX_EPOCH, LingxiConfig::default())),
        }
    }

    pub fn get(&self) -> LingxiConfig {
        let mut guard = match self.last_check.lock() {
            Ok(g) => g,
            Err(_) => return LingxiConfig::default(),
        };
        let now = std::time::SystemTime::now();
        if now
            .duration_since(guard.0)
            .map(|d| d.as_secs() < 1)
            .unwrap_or(true)
        {
            return guard.1.clone();
        }
        let cfg = load_config();
        guard.0 = now;
        guard.1 = cfg.clone();
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧版配置文件（没有 punct_map 字段）必须能反序列化 —— 否则升级后设置会全丢。
    #[test]
    fn old_config_without_punct_map_still_parses() {
        let json = r#"{
            "input": {
                "schema": "lingxi_rich",
                "full_shape": true,
                "ascii_punct": false,
                "ascii_mode": false,
                "simplification": true,
                "page_size": 6,
                "beep": false
            },
            "appearance": {},
            "dict": {},
            "version": 1
        }"#;
        let cfg: LingxiConfig = serde_json::from_str(json).expect("旧配置应能解析");
        assert_eq!(cfg.input.page_size, 6);
        assert!(cfg.input.punct_map.is_empty(), "缺失字段应回落到空表");
        assert!(cfg.input.punct_lookup(',').is_none());
    }

    /// 自定义标点：序列化往返后仍可查到；未命中返回 None（走 Rime 内置标点表）。
    #[test]
    fn punct_map_roundtrip_and_lookup() {
        let mut cfg = LingxiConfig::default();
        cfg.input.punct_map = vec![
            PunctPair {
                key: ",".into(),
                out: "，".into(),
            },
            PunctPair {
                key: "-".into(),
                out: "——".into(),
            },
        ];
        let json = serde_json::to_string(&cfg).expect("序列化");
        let back: LingxiConfig = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(back.input.punct_lookup(',').as_deref(), Some("，"));
        assert_eq!(back.input.punct_lookup('-').as_deref(), Some("——"));
        assert!(back.input.punct_lookup('.').is_none());
    }

    /// 新增字段同样必须向后兼容（旧 config.json 里没有它们）。
    #[test]
    fn new_input_fields_default_on_old_config() {
        let json = r#"{ "input": { "schema": "lingxi_rich" }, "version": 1 }"#;
        let cfg: LingxiConfig = serde_json::from_str(json).expect("旧配置应能解析");
        assert_eq!(cfg.input.enter_behavior, "commit");
        assert_eq!(cfg.input.toggle_key, "shift");
        assert_eq!(cfg.input.separator(), "'");
        assert!(cfg.input.enable_user_dict);
        assert!(cfg.appearance.show_status_bar);
        assert!(cfg.input.fuzzy.is_empty());
    }

    /// 分隔符：非法值回退默认，空串表示不显示分隔。
    #[test]
    fn separator_normalization() {
        let mut cfg = InputConfig::default();
        cfg.pinyin_separator = "-".into();
        assert_eq!(cfg.separator(), "-");
        cfg.pinyin_separator = "".into();
        assert_eq!(cfg.separator(), "");
        cfg.pinyin_separator = "？？".into();
        assert_eq!(cfg.separator(), "'");
    }

    /// 模糊音规则：按组顺序展开，未启用的组不出规则。
    #[test]
    fn fuzzy_rules_expand_in_order() {
        let mut cfg = InputConfig::default();
        cfg.fuzzy = vec!["sh_s".into(), "zh_z".into()];
        let rules = cfg.fuzzy_rules();
        assert_eq!(
            rules,
            vec![
                "derive/^zh/z/".to_string(),
                "derive/^z/zh/".to_string(),
                "derive/^sh/s/".to_string(),
                "derive/^s/sh/".to_string(),
            ]
        );
    }

    /// 无模糊音 + 自学习开启 = 不需要补丁；否则补丁必须带上基础 algebra（Rime 的
    /// patch 对列表是整体替换，漏了会把缩略等内置规则挤掉）。
    #[test]
    fn schema_patch_yaml_content() {
        let base = vec!["abbrev/^([a-z]).+$/$1/".to_string()];
        let mut cfg = InputConfig::default();
        assert!(cfg.schema_patch_yaml(&base).is_none(), "无需补丁时不应写文件");

        cfg.fuzzy = vec!["zh_z".into()];
        let yaml = cfg.schema_patch_yaml(&base).expect("应生成补丁");
        let z = yaml.find("derive/^zh/z/").expect("含模糊规则");
        let b = yaml.find("abbrev/^([a-z]).+$/$1/").expect("含基础规则");
        assert!(z < b, "模糊规则应排在内置规则之前");
        assert!(!yaml.contains("enable_user_dict"), "自学习默认开，不该写进补丁");

        cfg.fuzzy.clear();
        cfg.enable_user_dict = false;
        let yaml = cfg.schema_patch_yaml(&base).expect("应生成补丁");
        assert!(yaml.contains("translator/enable_user_dict: false"));
        assert!(!yaml.contains("speller/algebra"), "无模糊音时不写 algebra");
    }

    /// schema algebra 解析：只取 speller 段下 algebra 的列表项。
    #[test]
    fn read_schema_algebra_parses_list() {
        let mut p = std::env::temp_dir();
        p.push(format!("lingxi-algebra-test-{}.yaml", std::process::id()));
        std::fs::write(
            &p,
            "schema:\n  schema_id: t\nspeller:\n  alphabet: abc\n  algebra:\n    - erase/^hm$/\n    - abbrev/^([a-z]).+$/$1/   # 注释\npunctuator:\n  x: y\n",
        )
        .expect("写临时 schema");
        let got = read_schema_algebra(&p);
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            got,
            vec![
                "erase/^hm$/".to_string(),
                "abbrev/^([a-z]).+$/$1/".to_string()
            ]
        );
    }
}
