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
pub const SVC_TAG: &str = "20261014-1";
/// 协议版本：消息格式不兼容时才改。
pub const PROTO: u32 = 1;

const PIPE_NAME: &str = r"\\.\pipe\xunyin-ime-svc-v1";
const MAX_MSG: usize = 1 << 20; // 单条消息上限 1MB
/// 没有客户端连接多久就自己退出（避免留僵尸进程；DLL 需要时会重新拉起）。
const IDLE_EXIT_MS: u64 = 60_000;

static CLIENTS: AtomicUsize = AtomicUsize::new(0);
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 服务自己的日志（宿主进程的日志里看不到服务的事，分开写）。
fn log(msg: &str) {
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
        return None;
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_MSG {
        return None;
    }
    let mut buf = vec![0u8; len];
    if !read_exact(h, &mut buf) {
        return None;
    }
    serde_json::from_slice(&buf).ok()
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
    while let Some(msg) = read_msg(h) {
        let t = msg.get("t").and_then(|v| v.as_str()).unwrap_or("");
        let reply = match t {
            "hello" => {
                let pid = msg.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
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
        "服务启动 tag={SVC_TAG} proto={PROTO} exe={:?}",
        std::env::current_exe().map(|p| p.display().to_string())
    ));
    let name = wide(PIPE_NAME);
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
            std::thread::spawn(move || {
                let h = HANDLE(h_addr as *mut core::ffi::c_void);
                serve(h);
                unsafe {
                    let _ = CloseHandle(h);
                }
            });
        } else {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}
