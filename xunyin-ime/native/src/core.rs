//! 设置中心后端：配置读写、词库导入导出、用户词库备份恢复、皮肤、日志、引擎部署。
//!
//! 与界面无关，纯 Rust。逻辑与旧的 Tauri 后端一一对应，只是去掉了 Tauri 层。

use xunyin_rime::config::{AppearanceConfig, XunyinConfig, load_config, save_config, user_data_dir};
use xunyin_rime::RimeEngine;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Core {
    engine: Arc<Mutex<RimeEngine>>,
    pub shared_data_dir: PathBuf,
    pub user_data_dir: PathBuf,
}

/// 词库统计（界面用）。
#[derive(Default, Clone)]
pub struct DictStats {
    pub custom_phrase_count: usize,
    pub table_size_mb: f64,
    pub user_dict_exists: bool,
    pub last_import: String,
}

/// 一位界面资源（皮肤中心用）。
#[derive(Clone)]
pub struct SkinAsset {
    pub name: String,
    pub label: String,
    /// 一句话说明（皮肤中心鼠标悬停时显示）
    pub desc: String,
    /// "皮肤" = 来自皮肤目录；"内置" = 来自 DLL 内嵌副本
    pub source: String,
    pub w: usize,
    pub h: usize,
    pub bytes: Vec<u8>,
}

/// 一个细胞词库：目录名、中文名、是否启用。
#[derive(Clone)]
pub struct CellDict {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// 皮肤资源文件名（与 xunyin-tsf 内置资源、tools/export-skin-assets.ps1 一致）。
pub const SKIN_ASSET_FILES: [&str; 11] = [
    "bar_bg.bin",
    "cell_hover.bin",
    "cell_active.bin",
    "icon_gear.bin",
    "glyph_quan.bin",
    "glyph_ban.bin",
    "glyph_punct_cn.bin",
    "glyph_punct_en.bin",
    "glyph_zhong.bin",
    "glyph_ying.bin",
    "glyph_symbol.bin",
];

/// 内置资源：直接复用 TSF 里的同一批文件（编译进本程序，导出/预览时用）。
fn embedded_asset(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "bar_bg.bin" => include_bytes!("../../../xunyin-tsf/src/assets/bar_bg.bin"),
        "cell_hover.bin" => include_bytes!("../../../xunyin-tsf/src/assets/cell_hover.bin"),
        "cell_active.bin" => include_bytes!("../../../xunyin-tsf/src/assets/cell_active.bin"),
        "icon_gear.bin" => include_bytes!("../../../xunyin-tsf/src/assets/icon_gear.bin"),
        "glyph_quan.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_quan.bin"),
        "glyph_ban.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_ban.bin"),
        "glyph_punct_cn.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_punct_cn.bin"),
        "glyph_punct_en.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_punct_en.bin"),
        "glyph_zhong.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_zhong.bin"),
        "glyph_ying.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_ying.bin"),
        "glyph_symbol.bin" => include_bytes!("../../../xunyin-tsf/src/assets/glyph_symbol.bin"),
        _ => return None,
    })
}

fn skin_asset_label(name: &str) -> &'static str {
    // 格子只有 ~137px，标签控制在 3 个汉字以内；完整说明见 skin_asset_desc
    match name {
        "bar_bg.bin" => "条底",
        "cell_hover.bin" => "悬停",
        "cell_active.bin" => "激活",
        "icon_gear.bin" => "齿轮",
        "glyph_quan.bin" => "全角",
        "glyph_ban.bin" => "半角",
        "glyph_punct_cn.bin" => "中标点",
        "glyph_punct_en.bin" => "英标点",
        "glyph_zhong.bin" => "中文",
        "glyph_ying.bin" => "英文",
        "glyph_symbol.bin" => "符号",
        _ => "资源",
    }
}

/// 每份资源的用途说明（皮肤中心悬停提示 + 导出一项后给用户的提示语）。
fn skin_asset_desc(name: &str) -> &'static str {
    match name {
        "bar_bg.bin" => "状态栏整条：白底圆角、边框、左侧拖动柄点阵",
        "cell_hover.bin" => "鼠标悬停某一格时的高亮胶囊",
        "cell_active.bin" => "激活格（中文模式、符号面板打开时）的蓝底胶囊",
        "icon_gear.bin" => "最右侧设置齿轮图标",
        "glyph_quan.bin" => "全角状态图标（满月）",
        "glyph_ban.bin" => "半角状态图标（半月）",
        "glyph_punct_cn.bin" => "中文标点状态图标（，。）",
        "glyph_punct_en.bin" => "英文标点状态图标（,.）",
        "glyph_zhong.bin" => "中文模式格图标",
        "glyph_ying.bin" => "英文模式格图标",
        "glyph_symbol.bin" => "符号面板格图标（※）",
        _ => "界面资源",
    }
}

/// 导出单份资源到皮肤目录（点皮肤中心的一项时调用）。返回文件路径。
pub fn export_skin_asset(name: &str) -> Result<PathBuf, String> {
    if !SKIN_ASSET_FILES.contains(&name) {
        return Err("未知的资源名".into());
    }
    let bytes = match std::fs::read(user_data_dir().join("skins").join(name)) {
        Ok(b) if asset_size_ok(&b, name) => b,
        _ => embedded_asset(name)
            .ok_or_else(|| "找不到这份资源".to_string())?
            .to_vec(),
    };
    let dir = user_data_dir().join("skins");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(name);
    std::fs::write(&dst, bytes).map_err(|e| e.to_string())?;
    Ok(dst)
}

/// 资源尺寸：8 字节头 + w*h*4。
fn asset_dims(b: &[u8]) -> (usize, usize) {
    if b.len() < 8 {
        return (0, 0);
    }
    (
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize,
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize,
    )
}

fn asset_size_ok(b: &[u8], name: &str) -> bool {
    let (w, h) = asset_dims(b);
    let want = if name == "bar_bg.bin" { (324, 60) } else { (60, 60) };
    (w, h) == want && b.len() == 8 + w * h * 4
}

/// 从一段文本里抠出版本号：`1.0.4**（更新…` → `1.0.4`。
fn version_in(s: &str) -> Option<String> {
    let s = s.trim_start_matches(['v', 'V']);
    let mut out = String::new();
    let mut dots = 0usize;
    for c in s.chars() {
        if c.is_ascii_digit() {
            out.push(c);
        } else if c == '.' && !out.is_empty() && !out.ends_with('.') {
            out.push(c);
            dots += 1;
        } else {
            break;
        }
    }
    while out.ends_with('.') {
        out.pop();
    }
    if dots >= 1 && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

impl Core {
    pub fn new() -> Self {
        let (shared, user) = resolve_data_dirs();
        Self {
            engine: Arc::new(Mutex::new(RimeEngine::uninitialized())),
            shared_data_dir: shared,
            user_data_dir: user,
        }
    }

    /// 初始化引擎；首次需要建索引时在后台线程部署（不阻塞界面）。
    pub fn init_engine(&self) {
        let shared = self.shared_data_dir.to_string_lossy().into_owned();
        let user = self.user_data_dir.to_string_lossy().into_owned();
        let Ok(mut engine) = self.engine.lock() else {
            return;
        };
        if engine.is_initialized() {
            return;
        }
        let needs_deploy = match RimeEngine::init(&shared, &user, "xunyin-ime") {
            Ok(fresh) => {
                let needs = fresh.is_maintenance_mode();
                *engine = fresh;
                needs
            }
            Err(_) => return,
        };
        drop(engine);
        if needs_deploy {
            let engine_arc = self.engine.clone();
            std::thread::spawn(move || {
                if let Ok(e) = engine_arc.lock() {
                    let _ = e.deploy();
                }
            });
        }
    }


    pub fn redeploy(&self) {
        let engine_arc = self.engine.clone();
        let shared = self.shared_data_dir.clone();
        let page_size = load_config().input.page_size;
        std::thread::spawn(move || {
            // 先收录新装的方案（拷进来的 *.schema.yaml 不进 schema_list 就不会生效）
            let user = user_data_dir();
            match xunyin_rime::schema_list::sync_default_patch(&shared, &user, page_size) {
                Ok(true) => println!("xunyin: 发现新方案，已加入方案清单"),
                Ok(false) => {}
                Err(e) => eprintln!("xunyin: 方案清单更新失败：{e}"),
            }
            if let Ok(e) = engine_arc.lock() {
                // 完整检查式部署：会扫描到新方案、新词库（与「重新部署」语义一致）
                if !e.deploy_full() {
                    // 引擎不可用时也要通知一次：宿主进程会自行重建
                    xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
                    return;
                }
                // 等索引编译完再通知：宿主进程重载后会立刻读 build 目录，
                // 半成品索引会让新方案/新词库看起来没生效。
                let done = e.wait_deploy(300_000);
                if !done {
                    eprintln!("xunyin: 重新部署超时（索引可能仍在编译）");
                }
            }
            // 写重载请求：所有正在运行的应用（记事本 / Edge / ChatGPT …）
            // 都会自己重建引擎，不用重启。
            let ok = xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
            println!("xunyin: 重新部署完成，已通知宿主进程重载 ({ok})");
        });
    }

    pub fn schemas(&self) -> Vec<(String, String)> {
        self.engine
            .lock()
            .map(|e| e.get_schema_list())
            .unwrap_or_default()
    }

    // ---------------- 配置 ----------------

    pub fn load_config(&self) -> XunyinConfig {
        load_config()
    }

    /// 保存配置；返回 true 表示需要重建索引（模糊音/候选数变化）。
    pub fn save_config(&self, cfg: &XunyinConfig, old: &XunyinConfig) -> Result<bool, String> {
        save_config(cfg).map_err(|e| e.to_string())?;
        write_page_size(cfg.input.page_size, &self.shared_data_dir)?;
        let patch_changed = write_schema_patch(cfg, &self.shared_data_dir)?;
        let needs_deploy = patch_changed || old.input.page_size != cfg.input.page_size;
        // 不需要重建索引的改动（候选数以外的外观 / 按键行为等）：
        // 直接通知宿主进程重载配置，正在打字的应用立刻用新设置。
        // 需要重建索引时由 redeploy() 在编译完成后统一通知。
        if !needs_deploy {
            xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
        }
        Ok(needs_deploy)
    }

    pub fn reset_config(&self) -> XunyinConfig {
        let cfg = XunyinConfig::default();
        let _ = save_config(&cfg);
        let _ = write_page_size(cfg.input.page_size, &self.shared_data_dir);
        let _ = write_schema_patch(&cfg, &self.shared_data_dir);
        self.redeploy();
        cfg
    }

    // ---------------- 词库 ----------------

    pub fn dict_stats(&self) -> DictStats {
        let dir = user_data_dir();
        let phrase_path = dir.join("custom_phrase.txt");
        let custom_phrase_count = std::fs::read_to_string(&phrase_path)
            .map(|s| {
                s.lines()
                    .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                    .count()
            })
            .unwrap_or(0);
        let table_size_mb = std::fs::read_dir(dir.join("build"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".table.bin"))
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum::<u64>() as f64
                    / (1024.0 * 1024.0)
            })
            .unwrap_or(0.0);
        DictStats {
            custom_phrase_count,
            table_size_mb,
            user_dict_exists: dir.join("rime_xunyin.userdb").exists(),
            last_import: String::new(),
        }
    }

    /// 导入词库文件，按条去重追加到自定义短语表。
    ///
    /// 支持两种格式：
    /// * **搜狗细胞词库（`.scel`）**：二进制，词和拼音都在文件里（见 [`crate::scel`]）。
    ///   用户的旧词库大多是这个格式，所以按扩展名自动识别。
    /// * **文本短语表**：每行「词<TAB>编码」（Rime custom_phrase 格式）。
    pub fn import_dict_file(&self, path: &Path) -> Result<DictStats, String> {
        let dir = user_data_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let phrase_path = dir.join("custom_phrase.txt");

        let mut skipped = 0usize;
        let mut new_lines: Vec<String> = Vec::new();
        let is_scel = path
            .extension()
            .map(|e| e.to_ascii_lowercase() == "scel")
            .unwrap_or(false);

        if is_scel {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("读取「{}」失败：{e}", path.display()))?;
            let entries = crate::scel::parse(&bytes)
                .map_err(|e| format!("解析搜狗词库「{}」失败：{e}", path.display()))?;
            for e in entries {
                new_lines.push(format!("{}\t{}", e.word, e.code));
            }
        } else {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("读取「{}」失败：{e}", path.display()))?;
            for raw in content.lines() {
                // 去掉 BOM：外部编辑器/表格导出的文件常带，留着会被当成"词条+编码"
                let line = raw.trim_start_matches('\u{FEFF}').trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let valid = line.split(['\t', ' ']).filter(|s| !s.is_empty()).count() >= 2;
                if !valid {
                    skipped += 1;
                    continue;
                }
                new_lines.push(line.to_string());
            }
            if new_lines.is_empty() && skipped > 0 {
                return Err(
                    "无法解析该文件：文本短语表需要每行「词<TAB>编码」；搜狗词库请选 .scel 文件"
                        .into(),
                );
            }
        }
        if new_lines.is_empty() {
            return Err("这个文件里没有可导入的词条".into());
        }

        let existing: std::collections::HashSet<String> = std::fs::read_to_string(&phrase_path)
            .map(|s| {
                s.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect()
            })
            .unwrap_or_default();
        let mut out = std::fs::read_to_string(&phrase_path).unwrap_or_default();
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        let mut added = 0usize;
        for line in new_lines {
            if !existing.contains(&line) {
                out.push_str(&line);
                out.push('\n');
                added += 1;
            }
        }
        std::fs::write(&phrase_path, out).map_err(|e| e.to_string())?;
        let mut stats = self.dict_stats();
        let kind = if is_scel { "（搜狗词库）" } else { "" };
        stats.last_import = format!("已导入 {added} 条{kind}，跳过 {skipped} 条");
        // 自定义短语进入词库需要重新部署
        self.redeploy();
        Ok(stats)
    }

    pub fn export_dict(&self) -> Result<PathBuf, String> {
        let dir = user_data_dir();
        let src = dir.join("custom_phrase.txt");
        let export_dir = dir.join("export");
        std::fs::create_dir_all(&export_dir).map_err(|e| e.to_string())?;
        let dst = export_dir.join(format!("custom_phrase_{}.txt", stamp()));
        let content = std::fs::read_to_string(&src).unwrap_or_default();
        std::fs::write(&dst, content).map_err(|e| e.to_string())?;
        Ok(dst)
    }

    // ---------------- 用户词库备份 / 恢复 ----------------

    pub fn backup_user_dict(&self) -> Result<PathBuf, String> {
        let dir = user_data_dir();
        let dst = backup_root().join(format!("userdict_{}", stamp()));
        std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
        let mut dicts = 0usize;
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".userdb") && entry.path().is_dir() {
                copy_dir_all(&entry.path(), &dst.join(&name)).map_err(|e| e.to_string())?;
                dicts += 1;
            }
        }
        for f in user_dict_files(&dir) {
            if let Some(name) = f.file_name() {
                std::fs::copy(&f, dst.join(name)).map_err(|e| e.to_string())?;
            }
        }
        if dicts == 0 {
            return Err("没有找到用户词库（还没打字学习过词？）".into());
        }
        Ok(dst)
    }

    pub fn list_backups(&self) -> Vec<PathBuf> {
        let root = backup_root();
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    v.push(e.path());
                }
            }
        }
        v.sort();
        v.reverse();
        v
    }

    pub fn restore_user_dict(&self, src: &Path) -> Result<String, String> {
        if !src.is_dir() {
            return Err("备份目录不存在".into());
        }
        let user = user_data_dir();
        let safe = backup_root().join(format!("before_restore_{}", stamp()));
        std::fs::create_dir_all(&safe).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(&user).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".userdb") && entry.path().is_dir() {
                copy_dir_all(&entry.path(), &safe.join(&name)).map_err(|e| e.to_string())?;
            }
        }
        let mut restored = 0usize;
        for entry in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let target = user.join(&name);
            if entry.path().is_dir() && name.ends_with(".userdb") {
                if target.exists() {
                    std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
                }
                copy_dir_all(&entry.path(), &target).map_err(|e| e.to_string())?;
                restored += 1;
            } else if entry.path().is_file() {
                std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
            }
        }
        self.redeploy();
        Ok(format!("已恢复 {restored} 个词库（恢复前的已备份）"))
    }

    // ---------------- 皮肤 ----------------

    pub fn export_skin(&self) -> Result<PathBuf, String> {
        let cfg = load_config();
        let dir = user_data_dir().join("skins");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("skin_{}.json", stamp()));
        let doc = serde_json::json!({
            "name": format!("迅音皮肤 {}", stamp()),
            "version": 1,
            "appearance": cfg.appearance,
        });
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn import_skin(&self, path: &Path) -> Result<XunyinConfig, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取皮肤文件失败：{e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("皮肤文件不是合法 JSON：{e}"))?;
        let appearance: AppearanceConfig = serde_json::from_value(
            v.get("appearance")
                .cloned()
                .ok_or_else(|| "皮肤文件缺少 appearance 字段".to_string())?,
        )
        .map_err(|e| format!("皮肤文件 appearance 字段不完整：{e}"))?;
        let mut cfg = load_config();
        cfg.appearance = appearance;
        save_config(&cfg).map_err(|e| e.to_string())?;
        // 换皮肤后通知宿主进程重载，正在输入的应用立刻换配色
        xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
        Ok(cfg)
    }

    // ---------------- 目录 / 日志 ----------------

    pub fn open_data_dir(&self) -> Result<(), String> {
        let dir = user_data_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        open_path(&dir)
    }

    pub fn open_log_dir(&self) -> Result<(), String> {
        let dir = user_data_dir().join("logs");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        open_path(&dir)
    }

    /// 打开皮肤目录：里面的 `*.bin` 覆盖内置界面资源（状态栏图标等），
    /// 改完点「重载配置」即可生效，不用重启应用。
    pub fn open_skin_dir(&self) -> Result<(), String> {
        let dir = user_data_dir().join("skins");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        open_path(&dir)
    }

    /// 删除皮肤覆盖文件，回到 DLL 内置资源。
    pub fn restore_builtin_skin(&self) -> Result<String, String> {
        let dir = user_data_dir().join("skins");
        let mut n = 0usize;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().extension().map(|x| x == "bin").unwrap_or(false) {
                    std::fs::remove_file(e.path()).map_err(|e| e.to_string())?;
                    n += 1;
                }
            }
        }
        // 让输入法重新读资源
        xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
        Ok(format!("已删除 {n} 个皮肤文件，恢复内置图标（正在输入的应用会自动重载）"))
    }

    /// 清空用户词库（学习到的词频与新词、联想表一起清掉）。
    pub fn clear_user_dict(&self) -> Result<String, String> {
        let dir = user_data_dir();
        let mut removed = 0usize;
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".userdb") {
                if e.path().is_dir() {
                    std::fs::remove_dir_all(e.path()).map_err(|e| e.to_string())?;
                } else {
                    std::fs::remove_file(e.path()).map_err(|e| e.to_string())?;
                }
                removed += 1;
            }
        }
        // 学习到的联想表与删词黑名单也一起清掉
        for extra in ["assoc.txt", "blocked.txt"] {
            let p = dir.join(extra);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
        // 索引里还带着旧词频，重建一次让词库回到初始状态
        self.redeploy();
        Ok(format!("已清空 {removed} 个用户词库（含联想记录），正在重建索引"))
    }

    /// 导出用户词库（学习记录）到 `export/` 目录。
    pub fn export_user_dict(&self) -> Result<PathBuf, String> {
        let dir = user_data_dir();
        let dst = dir.join("export").join(format!("userdict_{}", stamp()));
        std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
        let mut n = 0usize;
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".userdb") {
                if e.path().is_dir() {
                    copy_dir_all(&e.path(), &dst.join(&name)).map_err(|e| e.to_string())?;
                } else {
                    std::fs::copy(e.path(), dst.join(&name)).map_err(|e| e.to_string())?;
                }
                n += 1;
            }
        }
        // 顺带把文本形式的学习记录一起导出，方便查看
        for extra in ["rime_xunyin.txt", "assoc.txt", "custom_phrase.txt"] {
            let p = dir.join(extra);
            if p.is_file() {
                std::fs::copy(&p, dst.join(extra)).map_err(|e| e.to_string())?;
            }
        }
        Ok(dst)
    }

    // ---------------- 细胞词库（专业词库）开关 ----------------

    /// 列出细胞词库：id、中文名、是否启用（读当前生效的词库导入表）。
    pub fn cell_dicts(&self) -> Vec<CellDict> {
        let imported = self.imported_cell_dicts();
        let mut ids: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.shared_data_dir.join("cn_dicts_cell")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if let Some(id) = n.strip_suffix(".dict.yaml") {
                    ids.push(id.to_string());
                }
            }
        }
        ids.sort();
        ids.into_iter()
            .map(|id| CellDict {
                enabled: imported.iter().any(|x| *x == id),
                label: cell_dict_label(&id).to_string(),
                id,
            })
            .collect()
    }

    /// 当前生效的细胞词库 id：优先用户级词库（有就是用户在设置里选过的），否则随包词库。
    fn imported_cell_dicts(&self) -> Vec<String> {
        let user = self.user_data_dir.join("rime_xunyin.dict.yaml");
        let path = if user.is_file() {
            user
        } else {
            self.shared_data_dir.join("rime_xunyin.dict.yaml")
        };
        std::fs::read_to_string(path)
            .map(|t| parse_cell_imports(&t))
            .unwrap_or_default()
    }

    /// 应用细胞词库勾选：生成**用户级** `rime_xunyin.dict.yaml`（随包的那份一个字不动），
    /// 然后重建索引。Rime 读词库时用户目录优先，所以这份覆盖立即生效。
    pub fn apply_cell_dicts(&self, enabled: &[String]) -> Result<String, String> {
        let base = std::fs::read_to_string(self.shared_data_dir.join("rime_xunyin.dict.yaml"))
            .map_err(|e| format!("读随包词库清单失败：{e}"))?;
        let out = regenerate_dict_yaml(&base, enabled);
        std::fs::create_dir_all(&self.user_data_dir).map_err(|e| e.to_string())?;
        let dst = self.user_data_dir.join("rime_xunyin.dict.yaml");
        std::fs::write(&dst, out).map_err(|e| e.to_string())?;
        self.redeploy();
        Ok(format!(
            "已应用 {} 个细胞词库，正在重建索引（约 1~3 分钟，完成后自动生效）",
            enabled.len()
        ))
    }

    /// 导出诊断包：版本、配置、日志、词库统计打成一个 zip（用系统自带 tar）。
    pub fn export_diag(&self) -> Result<PathBuf, String> {
        let root = user_data_dir().join("diag");
        let stage = root.join(format!("diag_{}", stamp()));
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(root.join("logs")).ok();
        let info = format!(
            "迅音输入法 诊断包\n生成时间: {}\n版本: {}\n共享数据目录: {}\n用户数据目录: {}\n\
             词库数据: 约 {:.1} MB\n自定义短语: {} 条\n用户词典: {}\n",
            stamp(),
            env!("CARGO_PKG_VERSION"),
            self.shared_data_dir.display(),
            self.user_data_dir.display(),
            self.dict_stats().table_size_mb,
            self.dict_stats().custom_phrase_count,
            if self.dict_stats().user_dict_exists { "已启用" } else { "未创建" },
        );
        std::fs::write(stage.join("版本与环境.txt"), info).map_err(|e| e.to_string())?;
        let cfg_path = user_data_dir().join("config.json");
        if cfg_path.is_file() {
            std::fs::copy(&cfg_path, stage.join("config.json")).map_err(|e| e.to_string())?;
        }
        for name in ["default.custom.yaml", "xunyin_rich.custom.yaml", "rime_xunyin.schema.yaml"] {
            let p = self.user_data_dir.join(name);
            if p.is_file() {
                std::fs::copy(&p, stage.join(name)).map_err(|e| e.to_string())?;
            }
        }
        // 日志只带尾部 2000 行，避免诊断包过大
        let log = self.user_data_dir.join("logs").join("tsf.log");
        if let Ok(text) = std::fs::read_to_string(&log) {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(2000);
            std::fs::write(stage.join("tsf.log"), lines[start..].join("\n"))
                .map_err(|e| e.to_string())?;
        }
        let zip = root.join(format!("xunyin-diag-{}.zip", stamp()));
        match std::process::Command::new("tar.exe")
            .arg("-a")
            .arg("-c")
            .arg("-f")
            .arg(&zip)
            .arg("-C")
            .arg(&root)
            .arg(stage.file_name().unwrap())
            .creation_flags(0x0800_0000)
            .status()
        {
            Ok(s) if s.success() => {
                let _ = std::fs::remove_dir_all(&stage);
                let _ = open_path(root.parent().unwrap_or(&root));
                Ok(zip)
            }
            _ => {
                // 没有 tar（很老的系统）就直接留目录
                let _ = open_path(&stage);
                Ok(stage)
            }
        }
    }

    /// 导出全部设置（config.json + 方案补丁）为单个 JSON。
    pub fn export_config(&self) -> Result<PathBuf, String> {
        let cfg = load_config();
        let dir = user_data_dir().join("export");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("config_{}.json", stamp()));
        let mut patches = serde_json::Map::new();
        for name in ["default.custom.yaml", "xunyin_rich.custom.yaml"] {
            let p = user_data_dir().join(name);
            if let Ok(text) = std::fs::read_to_string(&p) {
                patches.insert(name.to_string(), serde_json::Value::String(text));
            }
        }
        let bundle = serde_json::json!({
            "app": "xunyin-ime",
            "version": cfg.version,
            "config": cfg,
            "patches": patches,
        });
        let text = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// 导入全部设置（本机配置 + 方案补丁），随后重建索引并通知输入法重载。
    pub fn import_config(&self, path: &Path) -> Result<XunyinConfig, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("不是合法的设置文件：{e}"))?;
        let cfg: XunyinConfig = serde_json::from_value(
            v.get("config")
                .cloned()
                .ok_or_else(|| "缺少 config 字段".to_string())?,
        )
        .map_err(|e| format!("config 字段不完整：{e}"))?;
        save_config(&cfg).map_err(|e| e.to_string())?;
        if let Some(map) = v.get("patches").and_then(|p| p.as_object()) {
            for (name, val) in map {
                if !name.ends_with(".yaml") {
                    continue;
                }
                if let Some(text) = val.as_str() {
                    let _ = std::fs::write(user_data_dir().join(name), text);
                }
            }
        }
        self.redeploy();
        Ok(cfg)
    }

    /// 检查更新：读 GitHub Release 的最新 tag，与当前版本比较。
    pub fn check_update(&self) -> Result<String, String> {
        let url = "https://api.github.com/repos/itsypa/xunyin-ime/releases/latest";
        let body = http_get(url)?;
        let Some(tag) = json_string_field(&body, "tag_name") else {
            // 仓库还没有 Release 时 GitHub 返回的是带 message 的错误对象
            if let Some(msg) = json_string_field(&body, "message") {
                return Err(format!("仓库暂时没有可用版本（{msg}）"));
            }
            return Err("无法解析服务器返回内容".into());
        };
        let cur = env!("CARGO_PKG_VERSION");
        let latest = tag.trim_start_matches('v');
        if latest == cur {
            Ok(format!("已是最新版本（v{cur}）"))
        } else {
            Ok(format!("发现新版本 v{latest}（当前 v{cur}）——点「打开下载页」更新"))
        }
    }

    /// 最新版本的安装包下载地址（Release 资产里带 Setup 与 x64 的那个）。
    pub fn latest_installer_url(&self) -> Result<String, String> {
        let body = http_get("https://api.github.com/repos/itsypa/xunyin-ime/releases/latest")?;
        let mut hit: Option<String> = None;
        let mut rest = body.as_str();
        while let Some(i) = rest.find("\"browser_download_url\"") {
            rest = &rest[i + 22..];
            let Some(start) = rest.find('"') else { break };
            let tail = &rest[start + 1..];
            let Some(end) = tail.find('"') else { break };
            let url = &tail[..end];
            if url.contains("Setup") && url.contains("x64") && url.ends_with(".exe") {
                hit = Some(url.to_string());
                break;
            }
        }
        hit.ok_or_else(|| "这个版本还没有 x64 安装包（可能是便携包）".to_string())
    }

    /// 下载并运行最新安装包（安装器会自己结束本程序/覆盖安装）。
    pub fn download_and_install(&self) -> Result<String, String> {
        let url = self.latest_installer_url()?;
        let name = url.rsplit('/').next().unwrap_or("XunyinIME-Setup.exe");
        let dst = std::env::temp_dir().join(name);
        let bytes = http_get_bytes(&url)?;
        if bytes.len() < 100_000 {
            return Err(format!("下载内容异常（{} 字节）", bytes.len()));
        }
        std::fs::write(&dst, &bytes).map_err(|e| e.to_string())?;
        std::process::Command::new(&dst)
            .spawn()
            .map_err(|e| format!("启动安装包失败：{e}"))?;
        Ok(format!("已下载并启动安装包：{}", dst.display()))
    }

    /// 检查上游词库（rime-frost）是否有新版本：比对内置记录里的版本号。
    pub fn check_dict_update(&self) -> Result<String, String> {
        // UPSTREAM.md 里记录着随包的上游版本：`上游版本：rime-frost 1.0.4`
        let bundled = std::fs::read_to_string(self.shared_data_dir.join("UPSTREAM.md"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find(|l| l.contains("上游版本"))
                    .and_then(|l| l.split_whitespace().find_map(version_in))
            });
        let body = http_get("https://api.github.com/repos/gaboolic/rime-frost/releases/latest")?;
        let tag = json_string_field(&body, "tag_name")
            .ok_or_else(|| "无法解析服务器返回内容".to_string())?;
        let latest = tag.trim_start_matches('v');
        let cur = bundled.as_deref().unwrap_or("");
        if cur == latest {
            Ok(format!("词库已是最新（上游 {tag}）"))
        } else if cur.is_empty() {
            Ok(format!("上游词库最新版本：{tag}（本机未记录版本号）"))
        } else {
            Ok(format!(
                "上游词库有新版本：{tag}（本机 {cur}）。用 tools/update-upstream-dicts.ps1 同步后重新部署即可。"
            ))
        }
    }

    // ---------------- 上游词库更新（暂存 → 校验 → 部署） ----------------

    pub fn dict_update_state(&self) -> DictUpdateState {
        dict_update_state(&self.shared_data_dir, &self.user_data_dir)
    }

    /// 第一步：把上游词库包下载到暂存区并校验（不动数据目录）。
    pub fn download_dict_update(&self) -> Result<String, String> {
        download_dict_update(&self.shared_data_dir, &self.user_data_dir)
    }

    /// 第二步：把暂存区里校验过的词库部署到数据目录（先备份旧文件）。
    pub fn apply_dict_update(&self) -> Result<String, String> {
        let msg = apply_dict_update(&self.shared_data_dir, &self.user_data_dir)?;
        self.redeploy(); // 词库变了，重建索引
        Ok(msg)
    }

    /// 回滚上次部署：用备份覆盖回去，并重建索引。
    pub fn rollback_dict_update(&self) -> Result<String, String> {
        let msg = rollback_dict_update(&self.user_data_dir)?;
        self.redeploy();
        Ok(msg)
    }

    /// 打开暂存/备份目录。
    pub fn open_dict_update_dir(&self) -> Result<(), String> {
        let d = dict_update_root(&self.user_data_dir);
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        open_path(&d)
    }

    /// 一键更新词库：下载 → 校验（不过就不动数据目录）→ 部署 → 重建索引。
    /// 界面只需要这一颗按钮：两步流程的安全性是内部保证的，不给用户添决策负担。
    pub fn update_dict_now(&self) -> Result<String, String> {
        let msg = update_dict_now(&self.shared_data_dir, &self.user_data_dir)?;
        self.redeploy();
        Ok(msg)
    }

    // ---------------- 皮肤中心 ----------------

    /// 一位界面资源：名字、当前来源（皮肤目录 / 内置）、尺寸与像素。
    pub fn skin_assets(&self) -> Vec<SkinAsset> {
        let mut out = Vec::new();
        for name in SKIN_ASSET_FILES {
            let file = user_data_dir().join("skins").join(name);
            let (bytes, source) = match std::fs::read(&file) {
                Ok(b) if asset_size_ok(&b, name) => (b, "皮肤".to_string()),
                _ => match embedded_asset(name) {
                    Some(b) => (b.to_vec(), "内置".to_string()),
                    None => continue,
                },
            };
            let (w, h) = asset_dims(&bytes);
            out.push(SkinAsset {
                name: (*name).to_string(),
                label: skin_asset_label(name).to_string(),
                desc: skin_asset_desc(name).to_string(),
                source,
                w,
                h,
                bytes,
            });
        }
        out
    }

    /// 导出皮肤包：把**当前生效**的 11 个资源放到一个 zip 里（可直接分享/备份）。
    pub fn export_skin_pack(&self) -> Result<PathBuf, String> {
        let root = user_data_dir().join("export");
        let stage = root.join(format!("skin_{}", stamp()));
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        for a in self.skin_assets() {
            std::fs::write(stage.join(&a.name), &a.bytes).map_err(|e| e.to_string())?;
        }
        // 附一份说明，方便别人看懂这些 bin 是什么
        let readme = "迅音输入法皮肤包\n\n每个 *.bin = 8 字节头（u32 宽 + u32 高，小端）+ BGRA 像素。\n\
            图标类文件只用 alpha 通道（颜色由主题决定），做单色蒙版即可。\n\
            导入方式：设置中心 → 外观主题 → 皮肤中心 → 导入皮肤包。\n";
        std::fs::write(stage.join("说明.txt"), readme).map_err(|e| e.to_string())?;
        let zip = root.join(format!("xunyin-skin-{}.zip", stamp()));
        match std::process::Command::new("tar.exe")
            .arg("-a")
            .arg("-c")
            .arg("-f")
            .arg(&zip)
            .arg("-C")
            .arg(&root)
            .arg(stage.file_name().unwrap())
            .creation_flags(0x0800_0000)
            .status()
        {
            Ok(s) if s.success() => {
                let _ = std::fs::remove_dir_all(&stage);
                let _ = open_path(&root);
                Ok(zip)
            }
            _ => {
                let _ = open_path(&stage);
                Ok(stage)
            }
        }
    }

    /// 导入皮肤包（zip 或目录）：把里面的 *.bin 放进皮肤目录并让输入法重载。
    pub fn import_skin_pack(&self, src: &Path) -> Result<String, String> {
        let skins = user_data_dir().join("skins");
        std::fs::create_dir_all(&skins).map_err(|e| e.to_string())?;
        let mut copied = 0usize;
        let mut temp: Option<PathBuf> = None;
        let dir = if src.is_dir() {
            src.to_path_buf()
        } else {
            // zip：用系统自带 tar 解到临时目录
            let t = std::env::temp_dir().join(format!("xunyin-skin-{}", stamp()));
            std::fs::create_dir_all(&t).map_err(|e| e.to_string())?;
            let status = std::process::Command::new("tar.exe")
                .arg("-x")
                .arg("-f")
                .arg(src)
                .arg("-C")
                .arg(&t)
                .creation_flags(0x0800_0000)
                .status()
                .map_err(|e| format!("解压失败：{e}"))?;
            if !status.success() {
                return Err("解压失败：请确认是 zip 皮肤包".into());
            }
            temp = Some(t.clone());
            t
        };
        // 递归找 *.bin（皮肤包可能多套一层目录）
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if SKIN_ASSET_FILES.contains(
                    &p.file_name().unwrap_or_default().to_string_lossy().to_string().as_str(),
                ) {
                    let name = p.file_name().unwrap().to_string_lossy().to_string();
                    if asset_size_ok(&std::fs::read(&p).unwrap_or_default(), &name) {
                        std::fs::copy(&p, skins.join(&name)).map_err(|e| e.to_string())?;
                        copied += 1;
                    }
                }
            }
        }
        if let Some(t) = temp {
            let _ = std::fs::remove_dir_all(t);
        }
        if copied == 0 {
            return Err("皮肤包里没有有效资源（只认 11 个已知的 .bin 文件名）".into());
        }
        xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
        Ok(format!("已导入 {copied} 个皮肤资源（正在输入的应用会自动重载）"))
    }

    pub fn clear_logs(&self) -> u64 {
        let mut freed = 0u64;
        let mut targets: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(user_data_dir().join("logs")) {
            for e in rd.flatten() {
                if e.path().is_file() {
                    targets.push(e.path());
                }
            }
        }
        if let Ok(tmp) = std::env::var("TEMP") {
            targets.push(PathBuf::from(tmp).join("xunyin-tsf.log"));
        }
        for p in targets {
            if let Ok(md) = std::fs::metadata(&p) {
                freed += md.len();
            }
            let _ = std::fs::remove_file(&p);
        }
        freed
    }
}

// ---------------- 内部工具 ----------------

/// 定位 rime 数据目录：先看环境变量，再从 exe 所在目录逐级向上找 `rime/data`。
///
/// 覆盖安装布局（`<安装目录>\rime\data`）与开发布局（`xunyin-ime\rime\data`，
/// exe 在 `xunyin-ime\native\target\release` 时需上溯 3 级）。
pub fn resolve_data_dirs() -> (PathBuf, PathBuf) {
    let user = xunyin_rime::user_data_dir();
    if let Ok(p) = std::env::var("XUNYIN_SHARED_DATA") {
        return (PathBuf::from(p), user);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            let mut cur = Some(dir);
            for _ in 0..5 {
                let Some(d) = cur else { break };
                let cand = d.join("rime").join("data");
                if cand.join("default.yaml").exists() {
                    return (cand, user);
                }
                cur = d.parent().map(|p| p.to_path_buf());
            }
        }
    }
    (PathBuf::from("rime").join("data"), user)
}

/// 重写用户目录 `default.custom.yaml`：每页候选数 + 自动收录所有已安装方案
/// （「把新方案文件拷进数据目录 → 重新部署」就能用，不用手改 yaml）。
fn write_page_size(page_size: u32, shared: &Path) -> Result<(), String> {
    let user = user_data_dir();
    std::fs::create_dir_all(&user).map_err(|e| e.to_string())?;
    let changed = xunyin_rime::schema_list::sync_default_patch(shared, &user, page_size)
        .map_err(|e| e.to_string())?;
    if changed {
        println!("xunyin: default.custom.yaml 已更新（每页 {page_size} 个候选 + 收录已安装方案）");
    }
    Ok(())
}

/// 把模糊音 / 自学习开关写进 `<schema>.custom.yaml`；返回文件是否变化。
fn write_schema_patch(cfg: &XunyinConfig, shared: &Path) -> Result<bool, String> {
    let dir = user_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let schema_id = cfg.input.schema.trim();
    if schema_id.is_empty() {
        return Ok(false);
    }
    let file = dir.join(format!("{schema_id}.custom.yaml"));
    let base =
        xunyin_rime::config::read_schema_algebra(&shared.join(format!("{schema_id}.schema.yaml")));
    match cfg.input.schema_patch_yaml(&base) {
        Some(content) => {
            let unchanged = std::fs::read_to_string(&file)
                .map(|cur| cur == content)
                .unwrap_or(false);
            if unchanged {
                return Ok(false);
            }
            std::fs::write(&file, content).map_err(|e| e.to_string())?;
            Ok(true)
        }
        None => {
            if file.exists() {
                std::fs::remove_file(&file).map_err(|e| e.to_string())?;
                return Ok(true);
            }
            Ok(false)
        }
    }
}

fn open_path(p: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(p)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// 细胞词库中文名（上游 rime-frost 的细胞词库目录名 → 中文名）。
fn cell_dict_label(id: &str) -> &'static str {
    match id {
        "medication" => "医药",
        "industry_product" => "工业产品",
        "exthot" => "网络热词",
        "chess" => "象棋",
        "chess2" => "围棋",
        "animal" => "动物",
        "game" => "游戏",
        "idiom" => "成语",
        "sport" => "体育",
        "media" => "影视媒体",
        "shulihua" => "数理化",
        "food" => "饮食",
        "inputmethod" => "输入法用语",
        "history" => "历史",
        "place" => "地名",
        "geography" => "地理",
        "name2" => "人名（扩展）",
        "literature" => "文学",
        "music" => "音乐",
        "computer" => "计算机",
        "composite" => "综合",
        "name" => "人名",
        "luna" => "月相（备用）",
        _ => "细胞词库",
    }
}

/// 从词库 yaml 的 import_tables 里取出被导入的细胞词库 id（注释行不算）。
fn parse_cell_imports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("import_tables:") {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            let id = item.split('#').next().unwrap_or("").trim();
            if let Some(cell) = id.strip_prefix("cn_dicts_cell/") {
                if !cell.is_empty() {
                    out.push(cell.to_string());
                }
            }
        } else if t.is_empty() || t.starts_with('#') {
            continue;
        } else {
            break; // 出了 import_tables 块
        }
    }
    out
}

/// 重排词库清单：核心导入原样保留，细胞词库按勾选重建。
fn regenerate_dict_yaml(base: &str, enabled: &[String]) -> String {
    let lines: Vec<&str> = base.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("import_tables:") {
            out.push(line.to_string());
            i += 1;
            // 收集原来那一块：核心项原样保留，细胞项记下原始行以便按勾选回填
            let mut core: Vec<String> = Vec::new();
            let mut cell: Vec<(String, String)> = Vec::new();
            while i < lines.len() {
                let raw = lines[i];
                let t = raw.trim();
                if let Some(item) = t.strip_prefix("- ") {
                    let id = item.split('#').next().unwrap_or("").trim();
                    match id.strip_prefix("cn_dicts_cell/") {
                        Some(c) if !c.is_empty() => cell.push((c.to_string(), raw.to_string())),
                        _ => core.push(raw.to_string()),
                    }
                    i += 1;
                } else if t.starts_with('#') {
                    core.push(raw.to_string()); // 块内注释保留（含被注释掉的导入）
                    i += 1;
                } else if t.is_empty() {
                    i += 1;
                } else {
                    break;
                }
            }
            out.extend(core);
            out.push(String::new());
            out.push("  # 细胞词库（由设置中心 → 词库管理 → 细胞词库 勾选生成）".to_string());
            for id in enabled {
                match cell.iter().find(|(cid, _)| cid == id) {
                    Some((_, raw)) => out.push(raw.clone()),
                    None => out.push(format!("  - cn_dicts_cell/{id}")),
                }
            }
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// 极简 HTTPS GET（WinHTTP，不引第三方依赖）：检查更新用。
fn http_get(url: &str) -> Result<String, String> {
    String::from_utf8(http_get_bytes(url)?).map_err(|e| e.to_string())
}

/// 同上，但返回原始字节（下载安装包用）。
fn http_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;
    type Handle = *mut c_void;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(
            agent: *const u16,
            access: u32,
            proxy: *const u16,
            bypass: *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpConnect(session: Handle, host: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(
            connect: Handle,
            verb: *const u16,
            path: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept: *const *const u16,
            flags: u32,
        ) -> Handle;
        fn WinHttpSendRequest(
            request: Handle,
            headers: *const u16,
            headers_len: u32,
            optional: *const c_void,
            optional_len: u32,
            total_len: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> i32;
        fn WinHttpReadData(request: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32)
            -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
    }

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "只支持 https".to_string())?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let agent = wide("xunyin-ime");
    let host_w = wide(host);
    let path_w = wide(path);
    let verb = wide("GET");
    unsafe {
        let session = WinHttpOpen(agent.as_ptr(), 0, std::ptr::null(), std::ptr::null(), 0);
        if session.is_null() {
            return Err("WinHttpOpen 失败".into());
        }
        let conn = WinHttpConnect(session, host_w.as_ptr(), 443, 0);
        if conn.is_null() {
            WinHttpCloseHandle(session);
            return Err("连接失败".into());
        }
        let req = WinHttpOpenRequest(
            conn,
            verb.as_ptr(),
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if req.is_null() {
            WinHttpCloseHandle(conn);
            WinHttpCloseHandle(session);
            return Err("创建请求失败".into());
        }
        let ua = wide("User-Agent: xunyin-ime\r\nAccept: application/vnd.github+json\r\n");
        let sent = WinHttpSendRequest(req, ua.as_ptr(), ua.len() as u32 - 1, std::ptr::null(), 0, 0, 0);
        let ok = sent != 0 && WinHttpReceiveResponse(req, std::ptr::null_mut()) != 0;
        let mut body = Vec::new();
        if ok {
            let mut buf = [0u8; 4096];
            loop {
                let mut read: u32 = 0;
                if WinHttpReadData(req, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                    == 0
                    || read == 0
                {
                    break;
                }
                body.extend_from_slice(&buf[..read as usize]);
            }
        }
        WinHttpCloseHandle(req);
        WinHttpCloseHandle(conn);
        WinHttpCloseHandle(session);
        if !ok {
            return Err("网络请求失败（检查网络或代理）".into());
        }
        Ok(body)
    }
}

// ---------------- 上游词库更新：暂存 → 校验 → 部署（含回滚） ----------------
//
// 刻意分两步：第一步只往 `%APPDATA%\com.xunyin.ime\update\` 下载并校验，
// 第二步才动数据目录（先备份旧文件，可一键回滚）。这样下载失败或包不对
// 永远不会污染正在用的词库。

/// 词库更新状态（界面展示）。
pub struct DictUpdateState {
    /// 随包记录的上游版本（UPSTREAM.md）
    pub bundled: String,
    /// 暂存区里待部署的版本（空 = 没有）
    pub staged: String,
    /// 暂存区里通过校验的词库文件数
    pub staged_files: usize,
    /// 上次部署留下的备份目录名（空 = 没有）
    pub backup: String,
}

/// 设置中心自己的日志（GUI 程序没有控制台，排障靠它）。
fn core_log(msg: &str) {
    let dir = user_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let line = format!("[{}] {msg}\n", stamp());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("settings.log"))
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

fn dict_update_root(user: &Path) -> PathBuf {
    user.join("update")
}

/// 极简 key=value 状态文件（不引 JSON 依赖）。
fn read_kv(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    Some((k.trim().to_string(), v.trim().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn kv_get(kv: &[(String, String)], key: &str) -> String {
    kv.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

fn write_kv(path: &Path, pairs: &[(&str, String)]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text: String = pairs.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// 随包记录的上游词库版本。
fn bundled_frost_version(shared: &Path) -> String {
    std::fs::read_to_string(shared.join("UPSTREAM.md"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.contains("上游版本"))
                .and_then(|l| l.split_whitespace().find_map(version_in))
        })
        .unwrap_or_default()
}

/// 上游最新词库包：(tag, 下载地址)。优先 `rime-frost-schemas.zip`。
fn frost_dict_asset_url() -> Result<(String, String), String> {
    let body = http_get("https://api.github.com/repos/gaboolic/rime-frost/releases/latest")?;
    let tag = json_string_field(&body, "tag_name")
        .ok_or_else(|| "无法解析上游 release 信息".to_string())?;
    let mut fallback: Option<String> = None;
    let mut rest = body.as_str();
    while let Some(i) = rest.find("\"browser_download_url\"") {
        rest = &rest[i + 22..];
        let Some(start) = rest.find('"') else { break };
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('"') else { break };
        let url = &tail[..end];
        if url.ends_with(".zip") {
            if url.contains("schemas") {
                return Ok((tag, url.to_string()));
            }
            fallback = Some(url.to_string());
        }
    }
    fallback
        .map(|u| (tag, u))
        .ok_or_else(|| "上游这个版本没有可用的词库压缩包".to_string())
}

/// 在解压出来的目录里找 `cn_dicts` 与 `cn_dicts_cell`（上游包可能多套一层目录）。
fn find_dict_roots(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut stack = vec![dir.to_path_buf()];
    let mut core_root: Option<PathBuf> = None;
    let mut cell_root: Option<PathBuf> = None;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            match p.file_name().unwrap_or_default().to_string_lossy().as_ref() {
                "cn_dicts" => core_root = Some(p),
                "cn_dicts_cell" => cell_root = Some(p),
                _ => stack.push(p),
            }
        }
    }
    Some((core_root?, cell_root?))
}

/// 一份 Rime 词库文件的粗校验：头部要有 `name:`，且不能是空文件。
fn dict_file_ok(p: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(p) else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    // 上游词库的注释头可能上千行（base.dict.yaml 前 1890 行都是注释），所以不能只看开头：
    // 头部必须有 `name:`，并以 `...` 结束（Rime 词库的约定）。
    let mut has_name = false;
    let mut has_end = false;
    for l in text.lines().take(4000) {
        let t = l.trim_start();
        if !has_name && t.starts_with("name:") && t.len() > 6 {
            has_name = true;
        }
        if t == "..." {
            has_end = true;
            break;
        }
    }
    has_name && has_end
}

/// 收集目录里的 `*.dict.yaml`（相对路径 + 绝对路径）。
fn collect_dicts(root: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if p.is_dir() {
            collect_dicts(&p, &format!("{rel}/{name}"), out);
        } else if name.ends_with(".dict.yaml") {
            out.push((format!("{rel}/{name}"), p));
        }
    }
}

/// 目录是否可写（建删临时文件探测）：安装到 Program Files 时数据目录可能只读。
fn dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".xunyin-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"1") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn dict_update_state(shared: &Path, user: &Path) -> DictUpdateState {
    let root = dict_update_root(user);
    let pending = read_kv(&root.join("pending.kv"));
    let applied = read_kv(&root.join("applied.kv"));
    DictUpdateState {
        bundled: bundled_frost_version(shared),
        staged: kv_get(&pending, "tag"),
        staged_files: kv_get(&pending, "files").parse().unwrap_or(0),
        backup: PathBuf::from(kv_get(&applied, "backup"))
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

/// 一键更新词库（界面按钮用）：已是新版就跳过，否则 下载 → 校验 → 部署。
pub fn update_dict_now(shared: &Path, user: &Path) -> Result<String, String> {
    let (tag, _) = frost_dict_asset_url()?;
    let cur = bundled_frost_version(shared);
    if !cur.is_empty() && cur == tag.trim_start_matches('v') {
        return Ok(format!("词库已是最新（{tag}），无需更新"));
    }
    download_dict_update(shared, user)?;
    apply_dict_update(shared, user)
}

/// 第一步：下载上游词库包 → 解压到暂存区 → 校验 → 写 `pending.kv`。
/// **不动数据目录**，失败也不会影响正在用的词库。
pub fn download_dict_update(shared: &Path, user: &Path) -> Result<String, String> {
    let _ = shared;
    let (tag, url) = frost_dict_asset_url()?;
    let root = dict_update_root(user);
    let staged = root.join("staged");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;

    core_log(&format!("词库更新：开始下载 {url}"));
    let bytes = http_get_bytes(&url)?;
    if bytes.len() < 1_000_000 || bytes.len() > 400_000_000 {
        return Err(format!("下载内容大小异常（{} MB）", bytes.len() / 1_048_576));
    }
    let zip = root.join(format!("rime-frost-{tag}.zip"));
    std::fs::write(&zip, &bytes).map_err(|e| e.to_string())?;
    core_log(&format!(
        "词库更新：下载完成 {} MB，开始解压",
        bytes.len() / 1_048_576
    ));
    let status = std::process::Command::new("tar.exe")
        .arg("-x")
        .arg("-f")
        .arg(&zip)
        .arg("-C")
        .arg(&staged)
        .creation_flags(0x0800_0000)
        .status()
        .map_err(|e| format!("解压失败：{e}"))?;
    if !status.success() {
        return Err("解压失败（压缩包可能不完整）".into());
    }

    // 校验：必须找到两个词库目录，且里面的词库文件都像样
    let (core_root, cell_root) = find_dict_roots(&staged)
        .ok_or_else(|| "压缩包里找不到 cn_dicts / cn_dicts_cell".to_string())?;
    let mut files = Vec::new();
    collect_dicts(&core_root, "cn_dicts", &mut files);
    let core_count = files.len();
    collect_dicts(&cell_root, "cn_dicts_cell", &mut files);
    let cell_count = files.len() - core_count;
    for (rel, p) in files.iter() {
        if !dict_file_ok(p) {
            return Err(format!("词库文件校验失败：{rel}（不是有效的 Rime 词库）"));
        }
    }
    if core_count < 3 || cell_count < 5 {
        return Err(format!(
            "词库数量异常（基础 {core_count} 个 / 细胞 {cell_count} 个），已放弃这次更新"
        ));
    }
    write_kv(
        &root.join("pending.kv"),
        &[
            ("tag", tag.clone()),
            ("files", files.len().to_string()),
            ("dir", staged.display().to_string()),
            ("time", stamp()),
        ],
    )?;
    let _ = std::fs::remove_file(&zip); // 暂存目录里已有解压结果，压缩包不留
    core_log(&format!(
        "词库更新：{tag} 已通过校验（{} 个词库文件）",
        files.len()
    ));
    Ok(format!(
        "已下载并校验上游词库 {tag}（{} 个词库文件），点「部署到本机」生效",
        files.len()
    ))
}

/// 第二步：把暂存区部署到数据目录（先备份旧文件），并更新 UPSTREAM.md 的版本行。
pub fn apply_dict_update(shared: &Path, user: &Path) -> Result<String, String> {
    let root = dict_update_root(user);
    let pending = read_kv(&root.join("pending.kv"));
    let tag = kv_get(&pending, "tag");
    let staged = PathBuf::from(kv_get(&pending, "dir"));
    if tag.is_empty() || !staged.is_dir() {
        return Err("暂存区里没有待部署的词库，请先「下载到暂存」".into());
    }
    let (core_root, cell_root) =
        find_dict_roots(&staged).ok_or_else(|| "暂存区目录不完整".to_string())?;
    let mut files = Vec::new();
    collect_dicts(&core_root, "cn_dicts", &mut files);
    collect_dicts(&cell_root, "cn_dicts_cell", &mut files);

    // 安装到 Program Files 时数据目录可能只读：写不进去就落到用户目录（Rime 用户目录优先）
    let target = if dir_writable(shared) {
        shared.to_path_buf()
    } else {
        core_log("词库更新：数据目录不可写，改装到用户目录");
        user.to_path_buf()
    };

    // 备份将被覆盖的文件
    let backup = root.join(format!("backup-{}", stamp()));
    let mut replaced = 0usize;
    for (rel, _) in &files {
        let dst = target.join(rel);
        if dst.is_file() {
            let b = backup.join(rel);
            if let Some(d) = b.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&dst, &b).map_err(|e| e.to_string())?;
            replaced += 1;
        }
    }
    // 覆盖 / 新增
    let mut written = 0usize;
    for (rel, src) in &files {
        let dst = target.join(rel);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::copy(src, &dst).map_err(|e| format!("写入 {rel} 失败：{e}"))?;
        written += 1;
    }
    // 版本记录：随包那份能写就更新它，这样「检查词库更新」下次不会又报有新版本
    if target == shared {
        let upstream = shared.join("UPSTREAM.md");
        if let Ok(text) = std::fs::read_to_string(&upstream) {
            let new = text
                .lines()
                .map(|l| {
                    if l.contains("上游版本") {
                        format!(
                            "**上游版本：rime-frost {tag}**（更新上游词库时同步改这一行；设置中心的「检查词库更新」按它比对）"
                        )
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let _ = std::fs::write(&upstream, format!("{new}\n"));
        }
    }
    write_kv(
        &root.join("applied.kv"),
        &[
            ("tag", tag.clone()),
            ("files", written.to_string()),
            ("target", target.display().to_string()),
            ("backup", backup.display().to_string()),
            ("time", stamp()),
        ],
    )?;
    let _ = std::fs::remove_file(root.join("pending.kv"));
    let _ = std::fs::remove_dir_all(&staged);
    core_log(&format!(
        "词库更新：{tag} 已部署到 {}（写入 {written}，覆盖 {replaced}）",
        target.display()
    ));
    Ok(format!(
        "已部署上游词库 {tag}：写入 {written} 个文件（覆盖 {replaced} 个，旧文件已备份），正在重建索引"
    ))
}

/// 回滚：把上次部署备份的文件覆盖回去。
pub fn rollback_dict_update(user: &Path) -> Result<String, String> {
    let root = dict_update_root(user);
    let applied = read_kv(&root.join("applied.kv"));
    let backup = PathBuf::from(kv_get(&applied, "backup"));
    let target = PathBuf::from(kv_get(&applied, "target"));
    if backup.as_os_str().is_empty() || !backup.is_dir() || target.as_os_str().is_empty() {
        return Err("没有可回滚的备份（只有部署过才需要回滚）".into());
    }
    let mut restored = 0usize;
    let mut stack = vec![backup.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p
                .strip_prefix(&backup)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let dst = target.join(&rel);
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&p, &dst).map_err(|e| format!("回滚 {} 失败：{e}", rel.display()))?;
            restored += 1;
        }
    }
    let _ = std::fs::remove_file(root.join("applied.kv"));
    core_log(&format!("词库更新：已回滚 {restored} 个文件"));
    Ok(format!("已回滚 {restored} 个词库文件，正在重建索引"))
}

/// 从 JSON 文本里取一个字符串字段（不引 JSON 依赖，够用就行）。
fn json_string_field(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let start = body.find(&pat)? + pat.len();
    let rest = &body[start..];
    let colon = rest.find(':')? + 1;
    let rest = rest[colon..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "name: rime_xunyin\nversion: \"1\"\nimport_tables:\n  - cn_dicts/base     # 基础词库\n  - cn_dicts/others\n\n  # 细胞词库\n  - cn_dicts_cell/food\n  - cn_dicts_cell/animal\n...\n\n词\tci\n";

    #[test]
    fn parse_cell_imports_reads_only_cell_entries() {
        let v = parse_cell_imports(SAMPLE);
        assert_eq!(v, vec!["food".to_string(), "animal".to_string()]);
    }

    #[test]
    fn regenerate_keeps_core_and_applies_selection() {
        let out = regenerate_dict_yaml(SAMPLE, &["animal".to_string(), "music".to_string()]);
        // 核心导入原样保留（含行尾注释）
        assert!(out.contains("  - cn_dicts/base     # 基础词库"));
        assert!(out.contains("  - cn_dicts/others"));
        // 勾选里的 food 被去掉，animal 保留，music 新增
        assert!(!out.contains("cn_dicts_cell/food"));
        assert!(out.contains("cn_dicts_cell/animal"));
        assert!(out.contains("cn_dicts_cell/music"));
        // 表格数据与结尾的 ... 不能丢
        assert!(out.contains("\n..."));
        assert!(out.ends_with("词\tci\n"));
        // 重新解析应当只剩勾选的两个
        assert_eq!(
            parse_cell_imports(&out),
            vec!["animal".to_string(), "music".to_string()]
        );
    }

    #[test]
    fn asset_validation() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&60u32.to_le_bytes());
        buf.extend_from_slice(&60u32.to_le_bytes());
        buf.extend(std::iter::repeat(0u8).take(60 * 60 * 4));
        assert!(asset_size_ok(&buf, "glyph_ban.bin"));
        assert!(!asset_size_ok(&buf, "bar_bg.bin"));
        assert!(!asset_size_ok(&buf[..100], "glyph_ban.bin"));
    }

    /// 皮肤中心「点一项就导出」的底层动作：写文件、尺寸正确、来源变成"皮肤"。
    #[test]
    fn export_skin_asset_writes_usable_file() {
        let name = "glyph_symbol.bin";
        let p = export_skin_asset(name).expect("导出应成功");
        assert!(p.is_file(), "导出后文件应存在");
        let b = std::fs::read(&p).unwrap();
        assert!(asset_size_ok(&b, name), "导出的资源尺寸要合法");
        let core = Core::new();
        if let Some(a) = core.skin_assets().into_iter().find(|a| a.name == name) {
            assert_eq!(a.source, "皮肤", "皮肤目录里有文件后来源应变成皮肤");
            assert!(!a.desc.is_empty(), "每项都要有说明（悬停提示用）");
        }
        let _ = std::fs::remove_file(&p);
    }

    /// 内嵌资源必须齐全且尺寸合法（皮肤中心、导出资源包都依赖它）。
    #[test]
    fn embedded_skin_assets_are_complete() {
        for f in SKIN_ASSET_FILES {
            let b = embedded_asset(f).unwrap_or_else(|| panic!("缺内嵌资源 {f}"));
            assert!(asset_size_ok(b, f), "内嵌资源尺寸不对：{f}");
        }
    }

    /// 联网检查（CI 不跑）：cargo test -- --ignored --nocapture
    #[test]
    #[ignore = "需要网络"]
    fn live_update_checks() {
        let core = Core::new();
        println!("程序更新: {:?}", core.check_update());
        println!("安装包地址: {:?}", core.latest_installer_url());
        println!("词库更新: {:?}", core.check_dict_update());
    }

    /// 词库文件校验要能吃下"前面几十行注释头"的上游文件（41448.dict.yaml 就是这样）。
    #[test]
    fn dict_file_check_skips_comment_header() {
        let p = std::env::temp_dir().join("xunyin-dict-header-test.yaml");
        let mut text = String::from("# Rime dictionary\n# encoding: utf-8\n");
        for i in 0..30 {
            text.push_str(&format!("# 注释行 {i}\n"));
        }
        text.push_str("---\nname: cn_dicts/test\nversion: \"1\"\n...\n词\tci\n");
        std::fs::write(&p, text).unwrap();
        assert!(dict_file_ok(&p), "注释头很长时也应识别为有效词库");
        std::fs::write(&p, "# 只有注释，没有 name\n").unwrap();
        assert!(!dict_file_ok(&p), "没有 name 的不算词库");
        let _ = std::fs::remove_file(&p);
    }

    /// 暂存 → 部署 → 回滚的完整文件流程（全程用临时目录，不碰真实词库）。
    #[test]
    #[ignore = "会写临时目录"]
    fn dict_apply_and_rollback_roundtrip() {
        let base = std::env::temp_dir().join("xunyin-dict-update-test");
        let _ = std::fs::remove_dir_all(&base);
        let shared = base.join("shared");
        let user = base.join("user");

        // 目标目录里现有的词库（验证「覆盖前先备份」）
        for n in ["cn_dicts/base.dict.yaml", "cn_dicts_cell/food.dict.yaml"] {
            let p = shared.join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "---\nname: old\nversion: \"1\"\n...\n旧内容\n").unwrap();
        }
        // 暂存区：新版本词库
        let staged = user.join("update").join("staged");
        let names = [
            "cn_dicts/base.dict.yaml",
            "cn_dicts/ext.dict.yaml",
            "cn_dicts/others.dict.yaml",
            "cn_dicts_cell/food.dict.yaml",
            "cn_dicts_cell/animal.dict.yaml",
            "cn_dicts_cell/music.dict.yaml",
            "cn_dicts_cell/game.dict.yaml",
            "cn_dicts_cell/sport.dict.yaml",
        ];
        for n in names {
            let p = staged.join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "---\nname: new\nversion: \"9.9.9\"\n...\n新内容\n").unwrap();
        }
        write_kv(
            &user.join("update").join("pending.kv"),
            &[
                ("tag", "9.9.9".to_string()),
                ("files", names.len().to_string()),
                ("dir", staged.display().to_string()),
                ("time", "test".to_string()),
            ],
        )
        .unwrap();

        let msg = apply_dict_update(&shared, &user).unwrap();
        println!("部署: {msg}");
        assert!(shared.join("cn_dicts/ext.dict.yaml").is_file(), "新文件应被写入");
        let base_after = std::fs::read_to_string(shared.join("cn_dicts/base.dict.yaml")).unwrap();
        assert!(base_after.contains("新内容"), "同名文件应被覆盖");
        let backup = PathBuf::from(kv_get(&read_kv(&user.join("update").join("applied.kv")), "backup"));
        assert!(backup.join("cn_dicts/base.dict.yaml").is_file(), "覆盖过的文件要有备份");

        let msg = rollback_dict_update(&user).unwrap();
        println!("回滚: {msg}");
        let base_back = std::fs::read_to_string(shared.join("cn_dicts/base.dict.yaml")).unwrap();
        assert!(base_back.contains("旧内容"), "回滚应恢复原文件");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 真下载上游词库到暂存区（约 40MB，验证 URL/解压/校验；**不部署**）。
    #[test]
    #[ignore = "需要网络：下载 40MB"]
    fn live_download_dict_update_to_staging() {
        let shared = crate::core::resolve_data_dirs().0;
        let user = std::env::temp_dir().join("xunyin-dict-dl-user");
        let _ = std::fs::remove_dir_all(&user);
        let msg = download_dict_update(&shared, &user).unwrap();
        println!("下载: {msg}");
        let st = dict_update_state(&shared, &user);
        println!(
            "状态：随包 {} / 暂存 {} / {} 个文件",
            st.bundled, st.staged, st.staged_files
        );
        assert_eq!(st.staged, st.bundled.max(st.staged.clone()));
        assert!(st.staged_files > 20, "上游词库文件数应当 > 20");
        let _ = std::fs::remove_dir_all(&user);
    }
}

fn backup_root() -> PathBuf {
    user_data_dir().join("backup")
}

fn user_dict_files(dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let phrase = dir.join("custom_phrase.txt");
    if phrase.exists() {
        v.push(phrase);
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".custom.yaml") {
                v.push(e.path());
            }
        }
    }
    v
}

/// 形如 20260924-131500 的时间戳（本地时间）。
fn stamp() -> String {
    unsafe {
        let t = windows::Win32::System::SystemInformation::GetLocalTime();
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        )
    }
}
