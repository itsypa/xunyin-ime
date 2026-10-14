//! 服务进程客户端：连接 / 握手 / 版本校验 / 热重启。
//!
//! 设计要点（关系到能不能"改代码不重开应用"）：
//! * 所有管道操作都在**后台线程**做，UI/TSF 线程只读一个状态缓存，绝不阻塞输入；
//! * 每次握手比对服务返回的 tag 与本 DLL 期望的 tag：不一致就杀掉旧服务、
//!   按路径拉起新的 `xunyin-svc.exe` 再握手——这就是热重启；
//! * 服务不在（没启动/被杀/崩溃）时自动拉起；连不上也不影响打字（本阶段服务
//!   还没接管输入逻辑，只做握手与心跳）。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use xunyin_rime::svc_proto::{PIPE_NAME, PROTO, SVC_TAG};

/// 服务连接状态（UI 线程只读这个）。
#[derive(Default, Clone)]
struct Status {
    connected: bool,
    tag: String,
    pid: u32,
    /// 最近一次错误（诊断用）
    last_error: String,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static STARTED: OnceLock<()> = OnceLock::new();

fn set_status(s: Status) {
    let mut g = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    let changed = match g.as_ref() {
        Some(old) => old.connected != s.connected || old.tag != s.tag || old.pid != s.pid,
        None => true,
    };
    if changed {
        crate::tsf_log!(
            "svc: 状态变化 connected={} tag={} pid={} err={}",
            s.connected,
            s.tag,
            s.pid,
            s.last_error
        );
    }
    *g = Some(s);
}

/// 当前服务状态快照（诊断/日志用）。
pub fn status_line() -> String {
    let g = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some(s) if s.connected => format!("服务已连接 tag={} pid={}", s.tag, s.pid),
        Some(s) => format!("服务未连接（{}）", s.last_error),
        None => "服务未启动".into(),
    }
}

/// 由状态栏定时器 / 按键路径调用：首次调用拉起后台线程，之后只读状态。
pub fn poll() {
    STARTED.get_or_init(|| {
        crate::tsf_log!("svc: 启动服务客户端线程（期望 tag={SVC_TAG}）");
        std::thread::spawn(worker);
    });
}

/// 服务 exe 的位置：优先 DLL 同目录（安装布局），其次开发布局的 target/{release,debug}。
fn service_exe() -> Option<PathBuf> {
    let mut dir = crate::status_bar::module_dir_for_svc()?;
    loop {
        let same = dir.join("xunyin-svc.exe");
        if same.is_file() {
            return Some(same);
        }
        for profile in ["release", "debug"] {
            let p = dir
                .join("xunyin-svc")
                .join("target")
                .join(profile)
                .join("xunyin-svc.exe");
            if p.is_file() {
                return Some(p);
            }
        }
        dir = dir.parent()?.to_path_buf();
    }
}

fn connect() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
}

fn send(f: &mut std::fs::File, v: &serde_json::Value) -> std::io::Result<()> {
    let text = v.to_string();
    let bytes = text.as_bytes();
    f.write_all(&(bytes.len() as u32).to_le_bytes())?;
    f.write_all(bytes)?;
    f.flush()
}

fn recv(f: &mut std::fs::File) -> std::io::Result<serde_json::Value> {
    let mut lb = [0u8; 4];
    f.read_exact(&mut lb)?;
    let len = u32::from_le_bytes(lb) as usize;
    if len == 0 || len > (1 << 20) {
        return Err(std::io::Error::other("消息长度异常"));
    }
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::other(e.to_string()))
}

/// 拉起服务进程（不弹窗口）。
fn spawn_service() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = service_exe().ok_or_else(|| "找不到 xunyin-svc.exe".to_string())?;
    std::process::Command::new(&exe)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：服务是后台进程，不要黑框
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("拉起服务失败：{e}"))
}

/// 结束一个进程（版本不一致时用来踢掉旧服务）。
fn kill_process(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 0);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
}

/// 一次完整握手：返回值是服务的 tag。
fn handshake(f: &mut std::fs::File) -> std::io::Result<(String, u32, u32)> {
    send(
        f,
        &serde_json::json!({ "t": "hello", "proto": PROTO, "pid": std::process::id(), "dll_tag": SVC_TAG }),
    )?;
    let ack = recv(f)?;
    let tag = ack.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let pid = ack.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let proto = ack.get("proto").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    Ok((tag, pid, proto))
}

/// 服务 exe 的指纹（修改时间 + 大小）。
///
/// 热更新就靠它：**磁盘上的服务被重编替换后，指纹变化，DLL 就重启服务进程**——
/// 于是"改服务代码"不需要重开任何宿主应用。DLL 自己的代码没变，也就不需要重编。
fn fingerprint(exe: &std::path::Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(exe).ok()?;
    let mtime = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    Some((mtime, md.len()))
}

/// 磁盘上的服务 exe 变化了（重编替换过）→ 需要重启服务进程。
/// 抽成纯函数是为了能单测：这条判断就是"改服务代码不用重开宿主应用"的核心。
fn needs_restart(fp_on_disk: Option<(u64, u64)>, fp_connected: Option<(u64, u64)>) -> bool {
    match (fp_connected, fp_on_disk) {
        // 还没连上任何服务：不需要"重启"，交给连接逻辑拉起
        (None, _) => false,
        // 服务在跑，但磁盘上的 exe 变了（或读不到指纹）→ 换新
        (Some(_), None) => true,
        (Some(a), Some(b)) => a != b,
    }
}

/// 服务的协议版本能不能用（服务比 DLL 新也可以，只要不低于 DLL 需要的最低版本）。
fn proto_compatible(svc_proto: u32) -> bool {
    svc_proto >= PROTO
}

/// 后台线程主循环：保证"连上、版本对、活着"。
fn worker() {
    let mut conn: Option<std::fs::File> = None;
    let mut cur_pid: u32 = 0;
    let mut conn_fp: Option<(u64, u64)> = None;
    loop {
        // 0) 服务 exe 有没有被重编替换？换了就踢掉旧进程（下一轮按新 exe 重启）
        let exe = service_exe();
        let fp = exe.as_deref().and_then(fingerprint);
        if conn.is_some() && needs_restart(fp, conn_fp) {
            crate::tsf_log!("svc: 磁盘上的服务已更新 → 重启服务进程（宿主应用无感）");
            if cur_pid != 0 {
                kill_process(cur_pid);
            }
            conn = None;
            std::thread::sleep(Duration::from_millis(300));
        }
        // 1) 连接
        if conn.is_none() {
            match connect() {
                Ok(f) => {
                    conn = Some(f);
                }
                Err(e) => {
                    // 服务没在跑：拉起来再连一次
                    let spawn_err = spawn_service().err();
                    match connect() {
                        Ok(f) => {
                            conn = Some(f);
                        }
                        Err(e2) => {
                            set_status(Status {
                                connected: false,
                                tag: String::new(),
                                pid: 0,
                                last_error: format!(
                                    "连接失败 {e2}{}",
                                    spawn_err.map(|s| format!("（{s}）")).unwrap_or_default()
                                ),
                            });
                            std::thread::sleep(Duration::from_secs(2));
                            continue;
                        }
                    }
                }
            }
        }
        // 2) 握手 + 版本校验
        if let Some(f) = conn.as_mut() {
            match handshake(f) {
                Ok((tag, pid, proto)) => {
                    if tag.is_empty() {
                        // 不是我们的服务（可能是别的程序占了这个管道名）
                        set_status(Status {
                            connected: false,
                            tag,
                            pid,
                            last_error: "握手响应异常".into(),
                        });
                        conn = None;
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                    if !proto_compatible(proto) {
                        // 3) 服务协议太旧（装的是老版本）→ 踢掉，等新 exe 装好
                        crate::tsf_log!("svc: 服务协议过旧（服务 proto={proto} / 需要 {PROTO}）→ 重启服务");
                        if pid != 0 {
                            kill_process(pid);
                        }
                        conn = None;
                        cur_pid = 0;
                        set_status(Status {
                            connected: false,
                            tag: tag.clone(),
                            pid,
                            last_error: "服务协议过旧，已请求重启".into(),
                        });
                        std::thread::sleep(Duration::from_millis(300));
                        continue;
                    }
                    // tag 不同只是"服务比 DLL 新/旧"，协议兼容就直接用（并记下当前 exe 指纹）
                    cur_pid = pid;
                    conn_fp = fp;
                    let same_tag = tag == SVC_TAG;
                    let detail = if same_tag {
                        String::new()
                    } else {
                        format!("服务 {tag} / DLL 期望 {SVC_TAG}（协议兼容）")
                    };
                    set_status(Status {
                        connected: true,
                        tag,
                        pid,
                        last_error: detail,
                    });
                }
                Err(e) => {
                    conn = None;
                    set_status(Status {
                        connected: false,
                        tag: String::new(),
                        pid: cur_pid,
                        last_error: format!("握手失败 {e}"),
                    });
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            }
        }
        // 4) 心跳（每 2 秒一次；失败就断开重连）
        std::thread::sleep(Duration::from_secs(2));
        if let Some(f) = conn.as_mut() {
            if send(f, &serde_json::json!({ "t": "ping" })).is_err()
                || recv(f)
                    .map(|v| v.get("t").and_then(|x| x.as_str()) != Some("pong"))
                    .unwrap_or(true)
            {
                crate::tsf_log!("svc: 心跳失败，重新连接");
                conn = None;
                set_status(Status {
                    connected: false,
                    tag: String::new(),
                    pid: cur_pid,
                    last_error: "心跳失败".into(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 核心判断：磁盘上的服务 exe 变了 → 重启服务（热更新的触发点）
    #[test]
    fn restart_when_exe_changes() {
        let a = Some((111u64, 222u64));
        let b = Some((333u64, 222u64)); // 重编后 mtime 变了
        assert!(!needs_restart(a, None), "还没连上服务时不算重启");
        assert!(!needs_restart(a, a), "指纹没变不该重启");
        assert!(needs_restart(b, a), "exe 变了就该重启服务");
        assert!(needs_restart(Some((1, 2)), Some((1, 3))), "大小变了也算");
        assert!(needs_restart(None, a), "读不到 exe（被删/改名）也要重启");
    }

    /// 协议兼容性：服务比 DLL 新可以用，比 DLL 旧要重启
    #[test]
    fn proto_compat() {
        assert!(proto_compatible(PROTO));
        assert!(proto_compatible(PROTO + 1));
        assert!(!proto_compatible(PROTO - 1));
        assert!(!proto_compatible(0));
    }
}
