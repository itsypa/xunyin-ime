//! 灵犀输入法 —— 设置中心（Tauri 后端）。
//!
//! 输入本身由系统级 TSF 文本服务（lingxi-tsf）承担；
//! 本应用负责：共享配置读写、词库导入/导出、引擎部署（建索引）、
//! 打开数据目录等设置能力。

use lingxi_rime::config::{AppearanceConfig, LingxiConfig, load_config, save_config, user_data_dir};
use lingxi_rime::RimeEngine;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;

const APP_NAME: &str = "lingxi-ime";

/// 全局状态：引擎（线程安全共享）+ 数据目录。
#[derive(Clone)]
struct AppState {
    engine: Arc<Mutex<RimeEngine>>,
    maintenance: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    shared_data_dir: PathBuf,
    user_data_dir: PathBuf,
}

/// 定位 rime 数据目录：shared 数据随应用资源打包，user 数据在用户目录。
fn resolve_data_dirs(app: &tauri::AppHandle) -> (PathBuf, PathBuf) {
    let user = lingxi_rime::user_data_dir();
    // 1. 环境变量显式指定
    if let Ok(p) = std::env::var("LINGXI_SHARED_DATA") {
        return (PathBuf::from(p), user);
    }
    // 2. 安装布局：资源目录 rime/data
    if let Ok(resource_dir) = app.path().resource_dir() {
        let shared = resource_dir.join("rime").join("data");
        if shared.join("default.yaml").exists() {
            return (shared, user);
        }
    }
    // 3. 开发布局：可执行文件位于 src-tauri/target/debug，数据在 src-tauri/rime/data
    if let Ok(exe_dir) = app.path().executable_dir() {
        // \\?\ 前缀路径下的 .. 不被可靠解析，先规范化
        let exe_dir = std::fs::canonicalize(&exe_dir).unwrap_or(exe_dir);
        for cand in [
            exe_dir.join("..").join("..").join("rime").join("data"),
            exe_dir.join("..").join("rime").join("data"),
            exe_dir.join("rime").join("data"),
        ] {
            if cand.join("default.yaml").exists() {
                return (cand, user);
            }
        }
    }
    (PathBuf::from(".").join("rime").join("data"), user)
}

/// 初始化引擎。由前端在启动时调用；需要部署时在后台线程执行。
#[tauri::command]
fn init_ime(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    eprintln!(
        "[lingxi-settings] init_ime shared={} user={}",
        state.shared_data_dir.display(),
        state.user_data_dir.display()
    );
    let mut engine = state
        .engine
        .lock()
        .map_err(|_| "engine lock failed".to_string())?;
    if engine.is_initialized() {
        return Ok(());
    }
    let shared = state.shared_data_dir.to_string_lossy().into_owned();
    let user = state.user_data_dir.to_string_lossy().into_owned();
    let needs_deploy = match RimeEngine::init(&shared, &user, APP_NAME) {
        Ok(fresh) => {
            let needs = fresh.is_maintenance_mode();
            *engine = fresh;
            needs
        }
        Err(e) => return Err(e),
    };
    if needs_deploy {
        let engine_arc = state.engine.clone();
        let handle = std::thread::spawn(move || {
            if let Ok(e) = engine_arc.lock() {
                let _ = e.deploy();
            }
        });
        *state
            .maintenance
            .lock()
            .map_err(|_| "maintenance lock".to_string())? = Some(handle);
    }
    let _ = app;
    Ok(())
}

/// 引擎是否在部署中。
#[tauri::command]
fn is_maintenance(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let engine = state.engine.lock().map_err(|_| "lock".to_string())?;
    Ok(engine.is_maintenance_mode())
}

/// 手动触发重新部署（词库导入/每页候选数变更后调用）。
#[tauri::command]
fn redeploy(state: tauri::State<'_, AppState>) -> Result<(), String> {
    start_redeploy(&state)
}

fn start_redeploy(state: &AppState) -> Result<(), String> {
    let engine_arc = state.engine.clone();
    let handle = std::thread::spawn(move || {
        if let Ok(e) = engine_arc.lock() {
            let _ = e.deploy();
        }
    });
    *state
        .maintenance
        .lock()
        .map_err(|_| "maintenance lock".to_string())? = Some(handle);
    Ok(())
}

/// 读取配置（不存在时以默认值落盘）。
#[tauri::command]
fn get_config() -> Result<LingxiConfig, String> {
    eprintln!("[lingxi-settings] get_config called");
    let cfg = load_config();
    let _ = save_config(&cfg);
    Ok(cfg)
}

/// 恢复默认设置（配置 + 每页候选数），并触发重建索引。
#[tauri::command]
fn reset_config(state: tauri::State<'_, AppState>) -> Result<LingxiConfig, String> {
    let cfg = LingxiConfig::default();
    save_config(&cfg).map_err(|e| e.to_string())?;
    write_page_size(cfg.input.page_size)?;
    start_redeploy(&state)?;
    eprintln!("[lingxi-settings] reset_config -> defaults applied");
    Ok(cfg)
}

/// 写入配置；同时把每页候选数写入 default.custom.yaml（部署时生效）。
#[tauri::command]
fn set_config(cfg: LingxiConfig, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let old = load_config();
    save_config(&cfg).map_err(|e| e.to_string())?;
    write_page_size(cfg.input.page_size)?;
    if old.input.page_size != cfg.input.page_size {
        start_redeploy(&state)?;
    }
    Ok(())
}

/// 把每页候选数写入用户目录 default.custom.yaml（Rime 合并生效）。
fn write_page_size(page_size: u32) -> Result<(), String> {
    let dir = user_data_dir();
    let content = format!(
        "patch:\n  menu/page_size: {}\n",
        page_size.clamp(1, 20)
    );
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("default.custom.yaml"), content).map_err(|e| e.to_string())?;
    Ok(())
}

/// 可用的输入方案。
#[derive(Serialize)]
struct SchemaInfo {
    id: String,
    name: String,
}

#[tauri::command]
fn list_schemas(state: tauri::State<'_, AppState>) -> Result<Vec<SchemaInfo>, String> {
    let engine = state.engine.lock().map_err(|_| "lock".to_string())?;
    Ok(engine
        .get_schema_list()
        .into_iter()
        .map(|(id, name)| SchemaInfo { id, name })
        .collect())
}

/// 词库统计。
#[derive(Serialize)]
struct DictStats {
    custom_phrase_count: usize,
    custom_phrase_file: String,
    table_size_mb: f64,
    user_dict_exists: bool,
    last_import: String,
}

fn dict_stats() -> DictStats {
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
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with(".table.bin")
                })
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum::<u64>() as f64
                / (1024.0 * 1024.0)
        })
        .unwrap_or(0.0);
    DictStats {
        custom_phrase_count,
        custom_phrase_file: phrase_path
            .to_string_lossy()
            .into_owned(),
        table_size_mb,
        user_dict_exists: dir.join("rime_lingxi.userdb").exists(),
        last_import: String::new(),
    }
}

#[tauri::command]
fn get_dict_stats() -> Result<DictStats, String> {
    Ok(dict_stats())
}

/// 导入词库内容（前端通过文件选择读取文本后传入）。
/// 支持格式：Rime 自定义短语（每行 `词<TAB>编码` 或 `词 编码`）。
/// 追加到 custom_phrase.txt（按行去重）并触发重新部署。
#[tauri::command]
fn import_dict_content(
    filename: String,
    content: String,
) -> Result<DictStats, String> {
    let dir = user_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let phrase_path = dir.join("custom_phrase.txt");

    // 解析并校验每一行
    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut new_lines: Vec<String> = Vec::new();
    let mut total_valid = 0usize;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let valid = line
            .split(['\t', ' '])
            .filter(|s| !s.is_empty())
            .count()
            >= 2;
        if !valid {
            skipped += 1;
            continue;
        }
        new_lines.push(line.to_string());
        total_valid += 1;
    }
    if new_lines.is_empty() && skipped > 0 {
        return Err(format!(
            "无法解析文件「{filename}」：需要每行「词<TAB>编码」格式"
        ));
    }

    // 去重追加
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
    for line in new_lines {
        if !existing.contains(&line) {
            out.push_str(&line);
            out.push('\n');
            added += 1;
        }
    }
    std::fs::write(&phrase_path, out).map_err(|e| e.to_string())?;
    let mut stats = dict_stats();
    stats.last_import = format!("已导入 {added} 条，跳过 {skipped} 条（共识别 {total_valid} 条）");
    Ok(stats)
}

/// 导出词库：把自定义短语写入导出目录并返回路径。
#[tauri::command]
fn export_dict() -> Result<String, String> {
    let dir = user_data_dir();
    let src = dir.join("custom_phrase.txt");
    let export_dir = dir.join("export");
    std::fs::create_dir_all(&export_dir).map_err(|e| e.to_string())?;
    let stamp = chrono_like_stamp();
    let dst = export_dir.join(format!("custom_phrase_{stamp}.txt"));
    let content = std::fs::read_to_string(&src).unwrap_or_default();
    std::fs::write(&dst, content).map_err(|e| e.to_string())?;
    Ok(dst.to_string_lossy().into_owned())
}

fn chrono_like_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", secs)
}

/// 打开用户数据目录（资源管理器）。
#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    let dir = user_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer.exe")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---------------- 皮肤（外观）导入 / 导出 ----------------
//
// 皮肤文件是一段 JSON：{ "name": ..., "version": 1, "appearance": { ... } }。
// 只承载外观段（主题/配色/字号/圆角/透明度/序号），换机器或分享给他人时导入即可套用。

/// 导出当前外观为皮肤文件，返回文件路径。
#[tauri::command]
fn export_skin() -> Result<String, String> {
    let cfg = load_config();
    let dir = user_data_dir().join("skins");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("skin_{}.json", chrono_like_stamp()));
    let doc = serde_json::json!({
        "name": format!("灵犀皮肤 {}", chrono_like_stamp()),
        "version": 1,
        "appearance": cfg.appearance,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// 从皮肤文件内容导入外观设置，返回套用后的完整配置。
#[tauri::command]
fn import_skin(content: String) -> Result<LingxiConfig, String> {
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

// ---------------- 用户词库备份 / 恢复 ----------------
//
// 用户打字学到的词存在 Rime 的 LevelDB 目录 `*.userdb/` 里，是二进制目录，
// 整目录复制即完整备份（Rime 自己的 --backup/--restore 也是这么做的）。
// 这里不引入 zip 依赖：备份落成带时间戳的文件夹，路径直接告诉用户。

/// 递归复制目录（源为目录）。
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
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

/// 用户词库备份根目录：<用户数据目录>\backup\
fn backup_root() -> std::path::PathBuf {
    user_data_dir().join("backup")
}

/// 需要纳入备份的用户侧文件（非 userdb 目录）。
fn user_dict_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    let phrase = dir.join("custom_phrase.txt");
    if phrase.exists() {
        v.push(phrase);
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".custom.yaml") {
                v.push(e.path());
            }
        }
    }
    v
}

/// 备份用户词库（userdb 目录 + 自造词/自定义配置），返回备份目录路径。
#[tauri::command]
fn backup_user_dict() -> Result<String, String> {
    let dir = user_data_dir();
    let dst = backup_root().join(format!("userdict_{}", chrono_like_stamp()));
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
        return Err("没有找到用户词库（还没有打字学习过词？）".into());
    }
    Ok(dst.to_string_lossy().into_owned())
}

/// 列出已有备份目录（按名称倒序，新的在前）。
#[tauri::command]
fn list_user_dict_backups() -> Result<Vec<String>, String> {
    let root = backup_root();
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                v.push(e.path().to_string_lossy().into_owned());
            }
        }
    }
    v.sort();
    v.reverse();
    Ok(v)
}

/// 从备份目录恢复用户词库；恢复前先把当前词库另存一份，避免误操作丢词。
#[tauri::command]
fn restore_user_dict(dir: String) -> Result<String, String> {
    let src = std::path::PathBuf::from(&dir);
    if !src.is_dir() {
        return Err(format!("备份目录不存在：{dir}"));
    }
    let user = user_data_dir();
    // 先保护性备份当前状态
    let safe = backup_root().join(format!("before_restore_{}", chrono_like_stamp()));
    std::fs::create_dir_all(&safe).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&user).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".userdb") && entry.path().is_dir() {
            copy_dir_all(&entry.path(), &safe.join(&name)).map_err(|e| e.to_string())?;
        }
    }
    // 覆盖恢复：先删旧的同名 userdb，再整目录拷回
    let mut restored = 0usize;
    for entry in std::fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
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
    Ok(format!(
        "已恢复 {restored} 个词库；恢复前的词库已备份到 {}",
        safe.to_string_lossy()
    ))
}

pub fn run() {
    tauri::Builder::default()
        // 单实例：状态栏「设置」按钮每次都会启动一个进程，已有实例时必须把
        // 那个窗口拉到前台并聚焦，否则用户看到的是"点了没反应"
        // （第二个实例会因共享 WebView2 数据目录起不来窗口）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            // 首次启动（状态栏「设置」按钮拉起的那个进程）把主窗口带到前台并聚焦，
            // 否则窗口可能出现在浏览器/编辑器后面，用户会以为"点了没反应"。
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                    if let Some(w) = handle.get_webview_window("main") {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                });
            }
            let (shared, user) = resolve_data_dirs(&app.handle());
            let state = AppState {
                engine: Arc::new(Mutex::new(RimeEngine::uninitialized())),
                maintenance: Arc::new(Mutex::new(None)),
                shared_data_dir: shared,
                user_data_dir: user,
            };
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_ime,
            is_maintenance,
            redeploy,
            get_config,
            reset_config,
            set_config,
            list_schemas,
            get_dict_stats,
            import_dict_content,
            export_dict,
            open_data_dir,
            export_skin,
            import_skin,
            backup_user_dict,
            list_user_dict_backups,
            restore_user_dict,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
