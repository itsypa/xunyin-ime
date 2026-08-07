//! 灵犀输入法 —— 设置中心（Tauri 后端）。
//!
//! 输入本身由系统级 TSF 文本服务（lingxi-tsf）承担；
//! 本应用负责：共享配置读写、词库导入/导出、引擎部署（建索引）、
//! 打开数据目录等设置能力。

use lingxi_rime::config::{LingxiConfig, load_config, save_config, user_data_dir};
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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
