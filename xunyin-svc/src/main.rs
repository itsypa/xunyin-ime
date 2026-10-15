//! 迅音输入法常驻服务（第一阶段：骨架 + 协议 + 版本握手）。
//!
//! 为什么要独立进程：TSF 是 in-process 模型，DLL 被加载进每个正在输入的应用后
//! Windows 就锁住映像，代码改不了；而**这个服务可以随便重启**——宿主应用无感。
//! 于是"改一次代码就要重开 Edge/ChatGPT"的问题，靠把逻辑搬到服务里解决。
//!
//! 本阶段服务只做协议握手与版本报告，不改任何输入行为（零风险）；下一阶段把
//! librime 引擎与输入逻辑搬进来，再下一阶段把候选窗/状态栏搬进来。
//!
//! 协议：命名管道 `\\.\pipe\xunyin-ime-svc-v1`，消息 = 4 字节小端长度 + JSON 文本。

mod crashlog;
mod logic;

use logic::Logic;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE, ERROR_PIPE_CONNECTED};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

/// 服务构建标记：**改这里 + 重编服务 exe，运行中的 DLL 就会自动换用新服务**
/// （DLL 靠 exe 指纹变化触发重启）。协议兼容时"服务比 DLL 新"也能直接用，
/// 所以改服务代码**不需要重编 DLL、更不需要重开宿主应用**。
/// 记法：与 xunyin-rime::svc_proto::SVC_TAG 保持同一个值。
pub const SVC_TAG: &str = "20261014-4";
/// 协议版本：消息格式不兼容时才改。
pub const PROTO: u32 = 1;

/// 默认管道名；设了环境变量 `XUNYIN_SVC_PIPE` 就用它（测试里跑隔离实例用）。
/// 与 xunyin-rime::svc_proto 里的 PIPE_NAME 保持同一个值。
const PIPE_NAME: &str = r"\\.\pipe\xunyin-ime-svc-v1";

fn pipe_name() -> String {
    std::env::var("XUNYIN_SVC_PIPE").unwrap_or_else(|_| PIPE_NAME.to_string())
}

const MAX_MSG: usize = 1 << 20; // 单条消息上限 1MB
/// 没有客户端连接多久就自己退出（避免留僵尸进程；DLL 需要时会重新拉起）。
const IDLE_EXIT_MS: u64 = 60_000;

static CLIENTS: AtomicUsize = AtomicUsize::new(0);
/// 输入状态与引擎**进程内单例**：librime 的引擎是进程级单例，
/// 每个连接各 new 一个 Logic 会导致重复 init → 直接崩（实测踩过）。
/// 所有连接的按键都串行进这里，行为也因此与宿主进程内的旧实现一致。
static LOGIC: std::sync::Mutex<Option<Logic>> = std::sync::Mutex::new(None);

fn with_logic<R>(f: impl FnOnce(&mut Logic) -> R) -> R {
    let mut g = LOGIC.lock().unwrap_or_else(|e| e.into_inner());
    let l = g.get_or_insert_with(Logic::new);
    f(l)
}
/// 最近一次有客户端活动的时间（毫秒时间戳）
static LAST_ACTIVITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn touch() {
    LAST_ACTIVITY.store(now_ms(), Ordering::Relaxed);
}

/// 按键级诊断日志开关：用户数据目录里放一个 `log-input` 标记文件就开
/// （与宿主 DLL 里的 `log_input()` 同一个开关、同一个缓存时长）。
/// 默认关闭 —— 正常使用时不该为每颗键写一行日志。
fn log_keys() -> bool {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if at.elapsed() < std::time::Duration::from_secs(5) {
            return v;
        }
    }
    let on = std::env::var_os("XUNYIN_LOG_INPUT").is_some()
        || xunyin_rime::config::user_data_dir().join("log-input").exists();
    *g = Some((std::time::Instant::now(), on));
    on
}

/// 盯住「重载 / 重新部署」标记：装好新词库、改完配置后，服务里的引擎也要跟着重建，
/// 否则宿主的按键会继续落在旧索引上（机制与宿主 DLL 里的 poll_reload 一致）。
fn watch_reload() {
    std::thread::spawn(|| {
        let mut seen: Option<String> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let raw = xunyin_rime::reload::read().unwrap_or_default();
            match seen.as_ref() {
                // 首次只对基线：刚启动时本来就该读最新数据
                None => seen = Some(raw),
                Some(prev) if *prev == raw => {}
                Some(_) => {
                    seen = Some(raw.clone());
                    if !raw.trim().is_empty() {
                        log(format!("收到重载请求 -> {raw}"));
                        with_logic(|l| l.reload());
                    }
                }
            }
        }
    });
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 从 panic payload 里取出可读原因（写进服务日志）。
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic".to_string()
    }
}

/// 输入逻辑开关：默认开（服务就是"输入法本体"）。
/// 设 `XUNYIN_SVC_INPUT=0` 可让服务只做握手/心跳、不碰输入逻辑——
/// 出问题时用它是"退回 DLL 内逻辑"的应急开关。
fn input_enabled() -> bool {
    std::env::var("XUNYIN_SVC_INPUT").map(|v| v != "0").unwrap_or(true)
}

/// 跑一段输入逻辑：panic 不许把服务/连接线程带走，转成一条"服务侧出错"的回复，
/// 宿主 DLL 看到 error 就退回它自己的逻辑，用户照样能打字。
fn guard<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Ok(r),
        Err(p) => {
            let why = panic_text(&*p);
            log(format!("❌ 逻辑 panic：{why}"));
            Err(why)
        }
    }
}

/// 服务自己的日志（宿主进程的日志里看不到服务的事，分开写）。
fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    use std::io::Write as _;
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("com.xunyin.ime")
        .join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("svc.log"))
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts} pid={}] {msg}", std::process::id());
    }
}

fn read_msg(h: HANDLE) -> Option<Value> {
    let mut len_buf = [0u8; 4];
    if !read_exact(h, &mut len_buf) {
        log("read_msg: 读长度失败（对端关闭或出错）");
        return None;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_MSG {
        log(format!("read_msg: 长度异常 {len}"));
        return None;
    }
    let mut buf = vec![0u8; len];
    if !read_exact(h, &mut buf) {
        log("read_msg: 读正文失败");
        return None;
    }
    match serde_json::from_slice(&buf) {
        Ok(v) => Some(v),
        Err(e) => {
            log(format!("read_msg: JSON 解析失败 {e}"));
            None
        }
    }
}

fn read_exact(h: HANDLE, buf: &mut [u8]) -> bool {
    let mut off = 0usize;
    while off < buf.len() {
        let mut read = 0u32;
        match unsafe { ReadFile(h, Some(&mut buf[off..]), Some(&mut read), None) } {
            Ok(()) if read > 0 => off += read as usize,
            _ => return false,
        }
    }
    true
}

fn write_msg(h: HANDLE, v: &Value) -> bool {
    let text = v.to_string();
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    let mut off = 0usize;
    while off < out.len() {
        let mut written = 0u32;
        match unsafe { WriteFile(h, Some(&out[off..]), Some(&mut written), None) } {
            Ok(()) if written > 0 => off += written as usize,
            _ => return false,
        }
    }
    true
}

/// 处理一条客户端连接（每个连接一个线程，多个应用可以同时连）。
fn serve(h: HANDLE) {
    CLIENTS.fetch_add(1, Ordering::SeqCst);
    log(&format!("客户端接入，当前连接数 {}", CLIENTS.load(Ordering::SeqCst)));
    // 这条连接属于哪个宿主进程（hello 里带 pid）：按应用记忆中英状态要用它
    let mut client: u32 = 0;
    while let Some(msg) = read_msg(h) {
        let t = msg.get("t").and_then(|v| v.as_str()).unwrap_or("");
        let reply = match t {
            "hello" => {
                let pid = msg.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
                client = pid as u32;
                log(&format!(
                    "hello: 客户端 pid={pid} proto={:?} dll_tag={:?}",
                    msg.get("proto"),
                    msg.get("dll_tag")
                ));
                json!({
                    "t": "hello_ack",
                    "proto": PROTO,
                    "tag": SVC_TAG,
                    "pid": std::process::id(),
                    "exe": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
                })
            }
            "ping" => json!({ "t": "pong", "tag": SVC_TAG, "pid": std::process::id() }),
            // 预热：宿主 DLL 连上后第一件事就是发它，把"引擎首次初始化 + 首次编译索引"
            // 这几秒到几十秒提前做掉（否则用户敲的第一颗键就是一次漫长等待）。
            "warmup" => {
                if input_enabled() {
                    match guard(|| with_logic(|l| l.warmup())) {
                        Ok(note) => log(format!("预热完成：{note}")),
                        Err(why) => log(format!("预热失败：{why}")),
                    }
                }
                json!({ "t": "reply", "reply": { "consume": true } })
            }
            // ── 输入逻辑（第 2 阶段：从 DLL 搬过来的部分）──────────────
            // 服务持有唯一的 librime 引擎：宿主 DLL 只转发按键（见 xunyin-tsf/src/svc.rs）。
            // 这样"改输入逻辑"只要换掉这个 exe，宿主应用完全不用重启。
            "key" if input_enabled() => {
                let vk = msg.get("vk").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let mask = msg.get("mask").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let shift = msg.get("shift").and_then(|v| v.as_bool()).unwrap_or(false);
                match guard(|| with_logic(|l| l.key(client, vk, mask, shift))) {
                    Ok(r) => {
                        // 按键级诊断：只在用户数据目录里放了 `log-input` 标记时开
                        // （与宿主 DLL 的 log_input() 同一个开关）。排查"打不出字"时，
                        // 这一行能直接说明"按键到底有没有到服务、服务回了什么"。
                        if log_keys() {
                            log(format!(
                                "按键 pid={client} vk={vk} mask={mask} -> consume={} preedit={:?} commit={:?} 候选={} engine_ok={}",
                                r.consume,
                                r.preedit,
                                r.commit,
                                r.candidates.len(),
                                r.engine_ok
                            ));
                        }
                        json!({ "t": "reply", "reply": r })
                    }
                    Err(why) => json!({ "t": "reply", "reply": { "consume": false, "error": why } }),
                }
            }
            "select" if input_enabled() => {
                let index = msg.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                match guard(|| with_logic(|l| l.select(client, index))) {
                    Ok(r) => json!({ "t": "reply", "reply": r }),
                    Err(why) => json!({ "t": "reply", "reply": { "consume": false, "error": why } }),
                }
            }
            "status" => {
                match guard(|| with_logic(|l| l.status(client))) {
                    Ok(r) => json!({ "t": "reply", "reply": r }),
                    Err(why) => json!({ "t": "reply", "reply": { "consume": false, "error": why } }),
                }
            }
            "command" if input_enabled() => {
                let name = msg.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name == "redeploy" {
                    // 重建索引是分钟级的：把引擎收掉 + 打标记（这期间按键直接让宿主
                    // 退回自己的逻辑，不在这里等），真正的部署放后台线程跑，不持锁。
                    match with_logic(|l| l.begin_redeploy()) {
                        Some((shared, user)) => {
                            log("开始重建索引（后台）");
                            std::thread::spawn(move || {
                                let ok = Logic::deploy_redeploy(&shared, &user);
                                with_logic(|l| l.finish_redeploy());
                                log(format!("重建索引结束（{ok}）"));
                            });
                            json!({ "t": "reply", "reply": { "consume": true } })
                        }
                        None => {
                            log("重建索引：已有一次在进行，忽略");
                            json!({ "t": "reply", "reply": { "consume": true } })
                        }
                    }
                } else {
                    match guard(|| with_logic(|l| l.command(client, name))) {
                        Ok(r) => json!({ "t": "reply", "reply": r }),
                        Err(why) => {
                            json!({ "t": "reply", "reply": { "consume": false, "error": why } })
                        }
                    }
                }
            }
            "shutdown" => {
                log("收到 shutdown，退出");
                let _ = write_msg(h, &json!({ "t": "bye", "tag": SVC_TAG }));
                break;
            }
            other => {
                log(&format!("未知消息 {other:?}，原样返回"));
                json!({ "t": "echo", "from": other })
            }
        };
        if !write_msg(h, &reply) {
            break;
        }
    }
    CLIENTS.fetch_sub(1, Ordering::SeqCst);
    unsafe {
        let _ = DisconnectNamedPipe(h);
    }
    log("客户端断开");
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "--version" || arg == "-v" {
        println!("xunyin-svc {SVC_TAG} proto={PROTO}");
        return;
    }
    log(&format!(
        "服务启动 tag={SVC_TAG} proto={PROTO} 输入路径={} 管道={} exe={:?}",
        input_enabled(),
        pipe_name(),
        std::env::current_exe().map(|p| p.display().to_string())
    ));
    // panic 钩子：服务是隐藏窗口启动的，stderr 看不到；而且实测 after_key 里 panic 后
    // 展开（跑析构）会连带越界崩，catch_unwind 接不到——必须在展开前把原因落盘。
    std::panic::set_hook(Box::new(|info| {
        log(format!("❌ panic: {info}"));
    }));
    // 访问违例等致命异常也落盘（含指令地址、读写目标、调用链）
    crashlog::install();
    let pipe = pipe_name();
    let name = wide(&pipe);
    let mode = NAMED_PIPE_MODE(PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0);
    let open = FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0);
    touch();
    // 空闲退出：没有客户端且长时间没活动就退出，避免常年挂着一个没用的进程
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        if CLIENTS.load(Ordering::SeqCst) == 0
            && now_ms().saturating_sub(LAST_ACTIVITY.load(Ordering::Relaxed)) > IDLE_EXIT_MS
        {
            log("空闲超时，退出（宿主需要时会重新拉起）");
            std::process::exit(0);
        }
    });
    watch_reload();
    loop {
        let h = unsafe {
            CreateNamedPipeW(
                windows_core::PCWSTR(name.as_ptr()),
                open,
                mode,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                None,
            )
        };
        if h.is_invalid() {
            log("CreateNamedPipeW 失败，1 秒后重试");
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        let connected = match unsafe { ConnectNamedPipe(h, None) } {
            Ok(()) => true,
            Err(e) => e.code() == windows_core::HRESULT::from_win32(ERROR_PIPE_CONNECTED.0),
        };
        if connected {
            touch();
            // HANDLE 是裸指针，跨线程需要自己保证安全（这里句柄只归这个线程用）
            let h_addr = h.0 as usize;
            // 显式给足栈：输入逻辑这条链路很深（key → after_key → 各种 format!/Vec），
            // 默认 2MB 在 debug/release 混合调用下会踩到栈保护页，表现为
            // "固定偏移的 0xC0000005"（之前排查了很久，就是这个）。
            let spawned = std::thread::Builder::new()
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    let h = HANDLE(h_addr as *mut core::ffi::c_void);
                    serve(h);
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                });
            if spawned.is_err() {
                log("创建处理线程失败");
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
        } else {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}
