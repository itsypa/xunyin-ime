//! 设置中心后端：配置读写、词库导入导出、用户词库备份恢复、皮肤、日志、引擎部署。
//!
//! 与界面无关，纯 Rust。逻辑与旧的 Tauri 后端一一对应，只是去掉了 Tauri 层。

use xunyin_rime::config::{AppearanceConfig, XunyinConfig, load_config, save_config, user_data_dir};
use xunyin_rime::RimeEngine;
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
        std::thread::spawn(move || {
            if let Ok(e) = engine_arc.lock() {
                let _ = e.deploy();
            }
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
        write_page_size(cfg.input.page_size)?;
        let patch_changed = write_schema_patch(cfg, &self.shared_data_dir)?;
        Ok(patch_changed || old.input.page_size != cfg.input.page_size)
    }

    pub fn reset_config(&self) -> XunyinConfig {
        let cfg = XunyinConfig::default();
        let _ = save_config(&cfg);
        let _ = write_page_size(cfg.input.page_size);
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

    /// 导入自定义短语文件（Rime 格式：每行「词<TAB>编码」），按行去重追加。
    pub fn import_dict_file(&self, path: &Path) -> Result<DictStats, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取「{}」失败：{e}", path.display()))?;
        let dir = user_data_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let phrase_path = dir.join("custom_phrase.txt");

        let mut skipped = 0usize;
        let mut new_lines: Vec<String> = Vec::new();
        for raw in content.lines() {
            let line = raw.trim();
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
            return Err("无法解析该文件：需要每行「词<TAB>编码」格式".into());
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
        stats.last_import = format!("已导入 {added} 条，跳过 {skipped} 条");
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

/// 把每页候选数写进用户目录 `default.custom.yaml`（Rime 部署时合并）。
fn write_page_size(page_size: u32) -> Result<(), String> {
    let dir = user_data_dir();
    let content = format!("patch:\n  menu/page_size: {}\n", page_size.clamp(1, 20));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("default.custom.yaml"), content).map_err(|e| e.to_string())
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
