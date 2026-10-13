//! 进程级 librime 引擎单例：懒初始化 + 数据目录解析 + 热重载。
//!
//! 热重载（对标小狼毫的「重新部署」）：设置中心 / 部署脚本 / 状态栏菜单往
//! `%APPDATA%\com.xunyin.ime\reload.request` 写一条请求，本 DLL 的
//! [`poll_reload`] 发现新请求后就在原地重建引擎（finalize → initialize →
//! 应用共享配置），于是**装好新方案或改完配置后不用重启任何应用**。

use xunyin_rime::RimeEngine;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ENGINE: OnceLock<Arc<Mutex<RimeEngine>>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());
/// 已处理过的重载请求内容（用于判断「有新请求」）。
static RELOAD_SEEN: Mutex<Option<String>> = Mutex::new(None);
/// 同一个进程内同一时刻只跑一个重载线程。
static RELOAD_RUNNING: AtomicBool = AtomicBool::new(false);

/// 获取已初始化的引擎（未初始化返回 None，不会触发初始化）。
pub fn engine() -> Option<Arc<Mutex<RimeEngine>>> {
    ENGINE.get().cloned()
}

/// 确保引擎已初始化（首次调用会加载 rime.dll 并完成部署）。
/// 返回 None 表示初始化失败（例如 rime.dll 缺失）。
///
/// 引擎失效（例如重载失败）后会**就地重建**，保持 `Arc` 身份不变，
/// 这样长期持有引擎句柄的调用方（状态栏、文本服务）依然可用。
pub fn ensure_engine() -> Option<Arc<Mutex<RimeEngine>>> {
    if let Some(arc) = engine().filter(|a| is_alive(a)) {
        return Some(arc);
    }
    let _guard = INIT_LOCK.lock().ok()?;
    if let Some(arc) = engine() {
        if is_alive(&arc) {
            return Some(arc);
        }
        // 就地重建（复用同一把锁 / 同一个 Arc）
        {
            let mut guard = arc.lock().ok()?;
            let fresh = build_engine()?;
            *guard = fresh;
        }
        return Some(arc);
    }
    let fresh = build_engine()?;
    let arc = Arc::new(Mutex::new(fresh));
    let _ = ENGINE.set(arc.clone());
    Some(arc)
}

fn is_alive(arc: &Arc<Mutex<RimeEngine>>) -> bool {
    arc.lock().map(|e| e.is_initialized()).unwrap_or(false)
}

/// 组装一个新引擎：init → 首次部署 → 应用共享配置。
fn build_engine() -> Option<RimeEngine> {
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
        let ok = eng.deploy_full();
        // 等一小会儿：索引没建完就开会话会表现为「打不出中文」
        let done = eng.wait_deploy(20_000);
        crate::tsf_log!("engine init: deploy returned {ok} finished={done}");
    }
    // 应用共享配置：默认方案 + 输入选项（全角/半角、中英标点、中英模式）
    apply_config(&mut eng);
    crate::tsf_log!("engine ready");
    Some(eng)
}

/// 应用共享配置（默认方案 + 输入选项）。
fn apply_config(eng: &mut RimeEngine) {
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
        "apply_config: schema={default_schema} full_shape={} ascii_punct={} ascii_mode={} simplification=true",
        cfg.input.full_shape,
        cfg.input.ascii_punct,
        cfg.input.ascii_mode
    );
}

/// 请求一次热重载：写标记文件，通知**所有**宿主进程（不止当前这个）重载。
pub fn request_reload(action: &str) {
    let ok = xunyin_rime::reload::mark(action);
    crate::tsf_log!("engine: 写入重载请求 action={action} ok={ok}");
}

/// 轮询重载请求；发现新请求就后台重建引擎。返回 true 表示本轮发现了新请求。
///
/// 由状态栏定时器（前台时 400ms）与按键路径（每 3 秒）调用。
pub fn poll_reload() -> bool {
    let raw = xunyin_rime::reload::read().unwrap_or_default();
    let mut seen = match RELOAD_SEEN.lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    match seen.as_ref() {
        // 首次调用只对基线，不做重载：新进程启动时引擎本来就会读到最新数据。
        None => {
            *seen = Some(raw);
            return false;
        }
        Some(prev) if *prev == raw => return false,
        Some(_) => {}
    }
    *seen = Some(raw.clone());
    drop(seen);
    if raw.trim().is_empty() {
        return false;
    }
    let action = xunyin_rime::reload::action_of(&raw).unwrap_or("").to_string();
    crate::tsf_log!("engine: 收到重载请求 -> {action}");
    if RELOAD_RUNNING.swap(true, Ordering::SeqCst) {
        crate::tsf_log!("engine: 已有重载在跑，忽略本条");
        return true;
    }
    std::thread::spawn(move || {
        // 稍等片刻，让发起部署的进程把索引写完。
        std::thread::sleep(std::time::Duration::from_millis(300));
        // 需求方是 deploy 时，本进程不重复部署（避免多进程同时写同一份 build 目录），
        // 只重建引擎读取已经落盘的新索引。
        reload(false);
        RELOAD_RUNNING.store(false, Ordering::SeqCst);
    });
    true
}

/// 就地热重载：finalize 旧引擎 → 新建 → 应用配置。
///
/// `with_deploy` 为 true 时先重建索引（装新方案 / 换词库后用）。
/// 返回是否重载成功。
pub fn reload(with_deploy: bool) -> bool {
    let Some(arc) = ensure_engine() else {
        crate::tsf_log!("engine reload: 引擎不可用，跳过");
        return false;
    };
    let _guard = match INIT_LOCK.lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    let mut guard = match arc.lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    crate::tsf_log!("engine reload: deploy={with_deploy} 开始");
    if with_deploy {
        // 1) 收录新装方案：拷进数据目录的 *.schema.yaml 不写进 schema_list 不会生效
        let (shared, user) = data_dirs();
        let page_size = xunyin_rime::config::load_config().input.page_size;
        match xunyin_rime::schema_list::sync_default_patch(&shared, &user, page_size) {
            Ok(true) => crate::tsf_log!("engine reload: default.custom.yaml 已收录新方案"),
            Ok(false) => {}
            Err(e) => crate::tsf_log!("engine reload: 方案清单更新失败 {e}"),
        }
        // 2) 完整检查式部署（会编译新方案 / 新词库）
        if guard.is_initialized() {
            let started = guard.deploy_full();
            crate::tsf_log!("engine reload: deploy() -> {started}");
            // 部署在维护线程里跑，等它结束再重建，避免读到半成品索引。
            let done = guard.wait_deploy(180_000);
            crate::tsf_log!("engine reload: 维护线程结束={done}");
        }
    }
    // librime 同一进程内必须先 finalize 才能重新 setup/initialize
    guard.finalize();
    let Some(fresh) = build_engine() else {
        crate::tsf_log!("engine reload: 重建失败（下次按键会再试）");
        return false;
    };
    *guard = fresh;
    drop(guard);
    // 重载后组合态已丢失：收起可能还停留在屏幕上的候选窗。
    crate::candidate_window::hide_candidates();
    // 皮肤资源（状态栏图标等）从磁盘读，重载时清缓存 → 下一帧就用上新文件
    crate::status_bar::invalidate_assets();
    crate::status_bar::notify_repaint();
    crate::tsf_log!("engine reload: 完成");
    true
}

/// 只在当前进程重建索引（发起方用），完成后通知其他进程重载引擎。
pub fn deploy_here_then_notify_others() {
    if RELOAD_RUNNING.swap(true, Ordering::SeqCst) {
        crate::tsf_log!("engine: 已有重载/部署在跑，忽略");
        return;
    }
    std::thread::spawn(move || {
        // 重建索引 + 本进程重载：
        reload(true);
        // 其他宿主进程只需要重新读索引（不需要各自再部署一遍）
        request_reload("reload");
        RELOAD_RUNNING.store(false, Ordering::SeqCst);
    });
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
        // 1) DLL 旁或上一级（安装布局：<安装目录>\rime\data）
        for cand in [
            dll.join("rime").join("data"),
            dll.join("..").join("rime").join("data"),
        ] {
            if cand.join("default.yaml").exists() {
                return cand;
            }
        }
        // 2) 逐级向上找 xunyin-ime\rime\data（开发布局：DLL 可能在
        //    xunyin-tsf\target\debug 或 build\dev-deploy\tsf 下，深度不同，
        //    写死相对层级会找不到——找不到就没有方案可用，表现为"打不出中文"）
        let mut cur = Some(dll.clone());
        for _ in 0..7 {
            let Some(d) = cur else { break };
            let cand = d.join("xunyin-ime").join("rime").join("data");
            if cand.join("default.yaml").exists() {
                return cand;
            }
            cur = d.parent().map(|p| p.to_path_buf());
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
