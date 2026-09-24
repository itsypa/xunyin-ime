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
    /// 简体输出：true=简体，false=繁体。
    pub simplification: bool,
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
            simplification: true,
            page_size: 5,
            beep: false,
            punct_map: Vec::new(),
        }
    }
}

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
}
