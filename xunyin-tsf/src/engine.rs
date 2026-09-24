//! 进程级 librime 引擎单例：懒初始化 + 数据目录解析。

use xunyin_rime::RimeEngine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

static ENGINE: OnceLock<Arc<Mutex<RimeEngine>>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// 获取已初始化的引擎（未初始化返回 None，不会触发初始化）。
pub fn engine() -> Option<Arc<Mutex<RimeEngine>>> {
    ENGINE.get().cloned()
}

/// 确保引擎已初始化（首次调用会加载 rime.dll 并完成部署）。
/// 返回 None 表示初始化失败（例如 rime.dll 缺失）。
pub fn ensure_engine() -> Option<Arc<Mutex<RimeEngine>>> {
    if let Some(e) = engine() {
        return Some(e);
    }
    let _guard = INIT_LOCK.lock().ok()?;
    if let Some(e) = engine() {
        return Some(e);
    }
    let (shared, user) = data_dirs();
    crate::tsf_log!(
        "engine init: shared={} user={}",
        shared.display(),
        user.display()
    );
    if std::fs::create_dir_all(&user).is_err() {
        crate::tsf_log!("engine init: create user dir failed");
        return None;
    }
    let mut eng = RimeEngine::init(
        &shared.to_string_lossy(),
        &user.to_string_lossy(),
        "xunyin-tsf",
    )
    .map_err(|e| {
        crate::tsf_log!("engine init FAILED: {e}");
        e
    })
    .ok()?;
    // 首次运行需要建索引。使用随包预编译数据时通常瞬间完成；
    // 若确实需要完整部署，这里同步等待（仅在首次激活时发生一次）。
    if eng.is_maintenance_mode() {
        crate::tsf_log!("engine init: first-run deployment...");
        let ok = eng.deploy();
        crate::tsf_log!("engine init: deploy returned {ok}");
    }
    // 应用共享配置：默认方案 + 输入选项（全角/半角、中英标点、中英模式）
    let cfg = xunyin_rime::config::load_config();
    let default_schema = std::env::var("XUNYIN_DEFAULT_SCHEMA")
        .unwrap_or_else(|_| cfg.input.schema.clone());
    eng.set_default_schema(&default_schema);
    eng.set_default_options(vec![
        ("full_shape".into(), cfg.input.full_shape),
        ("ascii_punct".into(), cfg.input.ascii_punct),
        ("ascii_mode".into(), cfg.input.ascii_mode),
        // 本输入法只出简体：繁体输出（简入繁出）已从产品中移除，这里固定为简体。
        ("simplification".into(), true),
    ]);
    crate::tsf_log!(
        "engine init: schema={default_schema} full_shape={} ascii_punct={} ascii_mode={} simplification=true",
        cfg.input.full_shape,
        cfg.input.ascii_punct,
        cfg.input.ascii_mode
    );
    crate::tsf_log!("engine ready");
    let arc = Arc::new(Mutex::new(eng));
    let _ = ENGINE.set(arc.clone());
    Some(arc)
}

/// 定位本 DLL 所在目录。
fn module_dir() -> Option<PathBuf> {
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    unsafe {
        // 与注册逻辑一致：用本函数地址定位 DLL 自身模块，避免拿到宿主进程路径。
        let mut hmod: windows::Win32::Foundation::HMODULE = core::mem::zeroed();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let addr = (module_dir as fn() -> Option<PathBuf>) as usize;
        if GetModuleHandleExW(flags, windows_core::PCWSTR(addr as *const u16), &mut hmod).is_err() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmod), &mut buf);
        if len == 0 {
            return None;
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
            .parent()
            .map(|d| d.to_path_buf())
    }
}

/// 解析共享数据目录：
///   1. 环境变量 XUNYIN_SHARED_DATA
///   2. DLL 旁的 rime/data（安装布局）
///   3. 开发布局：../../../xunyin-ime/rime/data
fn shared_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("XUNYIN_SHARED_DATA") {
        return PathBuf::from(p);
    }
    if let Some(dll) = module_dir() {
        for cand in [
            dll.join("rime").join("data"),
            dll.join("..")
                .join("rime")
                .join("data"),
            dll.join("..")
                .join("..")
                .join("..")
                .join("xunyin-ime")
                .join("rime")
                .join("data"),
        ] {
            if cand.join("default.yaml").exists() {
                return cand;
            }
        }
        return dll.join("rime").join("data");
    }
    PathBuf::from("rime").join("data")
}

/// 用户数据目录（与设置中心共用，词库/日志互通）。
fn user_data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.xunyin.ime")
}

fn data_dirs() -> (PathBuf, PathBuf) {
    (shared_data_dir(), user_data_dir())
}
