//! 服务进程客户端：连接 / 握手 / 版本校验 / 热重启 / 按键转发。
//!
//! 设计要点（关系到能不能"改代码不重开应用"）：
//! * 所有管道操作都在**同一个后台线程**里做（连接、握手、心跳、按键收发都是它），
//!   UI/TSF 线程只读一个状态缓存 + 往队列里投一条消息，绝不阻塞输入；
//! * 每次握手比对服务返回的 tag 与本 DLL 期望的 tag：不一致就杀掉旧服务、
//!   按路径拉起新的 `xunyin-svc.exe` 再握手——这就是热重启；
//! * 服务不在（没启动/被杀/崩溃/卡住）时自动拉起并重连；**连不上也绝不吞键**，
//!   按键照旧走本进程逻辑，输入永远可用。
//!
//! 心跳和按键为什么必须同一个线程：命名管道是**单条消息流**，两个线程各读各的
//! 会把对方的回复读走，回复整体错位（历史坑）。这里由 worker 串行收发，从根上避免。

use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use xunyin_rime::svc_proto::{pipe_name, Reply, PROTO, SVC_TAG};

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

// ─────────────── 按键路径：把按键交给服务，拿回结果 ───────────────
//
// 输入逻辑已经全部搬进服务：宿主 DLL 只做"送按键、写结果"。
// 这样**改输入逻辑只要换掉 xunyin-svc.exe**，正在用的 Edge / ChatGPT 不用重启。
//
// 收发都由**后台 worker 线程**做（它那条连接就是阻塞 IO），调用方只在
// REPLY_TIMEOUT_MS 内等结果。**不要**给管道句柄设 SetCommTimeouts——实测在命名管道
// 句柄上它直接失败（ERROR_INVALID_FUNCTION，错误码 1）；当年就是这一步失败让
// "请求通道"永远建不起来，表现为"切到输入法后一个字都打不出来"（真实踩过这个坑）。
// 现在改用 PeekNamedPipe 轮询等字节（见 wait_bytes），不再依赖那个 API。
//
// 约束：
//   1) 不能阻塞输入：调用方等不到回复就走本进程逻辑，绝不长时间卡住按键；
//   2) 一旦超时，先**暂停**转发（PAUSE_MS），到点自动重试——服务恢复了就自己接回来，
//      免得服务有问题时每颗键都白等一个超时，也免得"一次超时就永久不再用服务"；
//   3) 失败永远只是"服务这条路暂时不可用"，输入本身必须照常可用（本进程逻辑接手）。

/// 一次请求：由后台 worker 线程实际收发。
struct Job {
    msg: serde_json::Value,
    reply: std::sync::mpsc::Sender<serde_json::Value>,
}

/// worker 线程的请求投递口（worker 启动时建立）。
static JOBS: OnceLock<std::sync::mpsc::Sender<Job>> = OnceLock::new();
/// 服务没回话时的"暂停转发"截止时刻（毫秒时间戳，0 = 正常）。到点自动恢复重试。
static PAUSE_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 调用方等待回复的上限（毫秒）。服务正常时一次往返 <1ms。
const REPLY_TIMEOUT_MS: u64 = 200;
/// 请求超时后暂停转发多久（毫秒）。
const PAUSE_MS: u64 = 3000;
/// 单次按键允许服务用多久（毫秒）。超了就当这条连接有问题，断开重连。
/// 引擎预热（见 worker 第 3 步）已经把"首次初始化/首次编译索引"那几十秒挡在前面了。
const KEY_BUDGET_MS: u64 = 10_000;
/// 握手后预热引擎的等待上限（毫秒）。首次部署要把词库编译成索引，可能几十秒。
const WARMUP_BUDGET_MS: u64 = 120_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 服务可用吗（读后台线程维护的状态缓存，不阻塞、不发起 IO）。
pub fn ready() -> bool {
    STATUS
        .lock()
        .map(|g| g.as_ref().map(|s| s.connected).unwrap_or(false))
        .unwrap_or(false)
}

/// 同步请求：返回服务回复里的 `reply` 字段（失败/超时返回 None，调用方退回本进程逻辑）。
fn request(msg: serde_json::Value) -> Option<serde_json::Value> {
    use std::sync::atomic::Ordering;
    if !ready() {
        return None;
    }
    let until = PAUSE_UNTIL.load(Ordering::Relaxed);
    if until != 0 {
        if now_ms() < until {
            return None;
        }
        PAUSE_UNTIL.store(0, Ordering::Relaxed); // 暂停期结束，再试一次
    }
    let tx = JOBS.get()?;
    let (rtx, rrx) = std::sync::mpsc::channel();
    tx.send(Job { msg, reply: rtx }).ok()?;
    match rrx.recv_timeout(Duration::from_millis(REPLY_TIMEOUT_MS)) {
        // worker 明确回了 null = 这条请求没能发出去（连接坏了），别当成服务回复
        Ok(v) if !v.is_null() => Some(v),
        Ok(_) => None,
        Err(_) => {
            PAUSE_UNTIL.store(now_ms() + PAUSE_MS, Ordering::Relaxed);
            crate::tsf_log!(
                "svc: 请求超时（{REPLY_TIMEOUT_MS}ms）→ 这颗键交给本进程逻辑，暂停转发 {PAUSE_MS}ms 后自动重试"
            );
            None
        }
    }
}

/// 把服务的一条消息拆出里面的 [Reply]。
///
/// **服务把结果包了一层**：`{"t":"reply","reply":{...真正的内容...}}`。
/// 这里必须先取 `reply` 再反序列化 —— 直接把整条消息喂给 `Reply`（它带
/// `#[serde(default)]`）会**静默**得到一份全默认的 Reply：consume=false、组合串为空。
/// 后果不是"报错"，而是每颗键都被当成"服务说不用管"而放行给应用（打不出中文），
/// 每次状态栏命令都石沉大海（中英/全半角切不动）。真实踩过这个坑。
///
/// 拿不到一条像样的 Reply 时返回 None，让调用方退回本进程逻辑。
fn parse_reply(v: serde_json::Value) -> Option<Reply> {
    // 兼容"没包一层"的旧格式：有 reply 就取出来用，没有就把整条当回复。
    let inner = v.get("reply").cloned().unwrap_or(v);
    // 必须确认它真的像一条 Reply：不能靠 serde 的默认值把"什么都没有"当成
    // "服务已经处理完了"。consume 是每条 Reply 都有的字段，拿它当哨兵。
    if inner.get("consume").is_none() {
        crate::tsf_log!("svc: 回复里没有 Reply（{inner}）→ 退回本进程逻辑");
        return None;
    }
    match serde_json::from_value::<Reply>(inner) {
        Ok(r) => {
            if let Some(e) = r.error.as_deref() {
                crate::tsf_log!("svc: 服务侧逻辑出错（{e}）→ 退回本进程逻辑");
                None
            } else {
                Some(r)
            }
        }
        Err(e) => {
            crate::tsf_log!("svc: 回复解析失败（{e}）→ 退回本进程逻辑");
            None
        }
    }
}

/// 把一次按键交给服务处理。
pub fn key(vk: u32, mask: i32, shift: bool) -> Option<Reply> {
    request(serde_json::json!({
        "t": "key", "vk": vk, "mask": mask, "shift": shift
    }))
    .and_then(parse_reply)
}

/// 候选窗点击（下标是**显示列表**里的下标，服务负责换算成 Rime 下标）。
pub fn select(index: usize) -> Option<Reply> {
    request(serde_json::json!({ "t": "select", "index": index })).and_then(parse_reply)
}

/// 状态栏动作 / 引擎命令：toggle_mode / toggle_width / toggle_punct / reload / redeploy /
/// select_schema:<id>。服务收到后会重读配置并（必要时）重建引擎。
pub fn command(name: &str) -> Option<Reply> {
    request(serde_json::json!({ "t": "command", "name": name })).and_then(parse_reply)
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
    // 管道名默认是协议里的常量；设了 XUNYIN_SVC_PIPE 就用它（测试里跑隔离实例用）
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name())
}

fn send(f: &mut std::fs::File, v: &serde_json::Value) -> std::io::Result<()> {
    let text = v.to_string();
    let bytes = text.as_bytes();
    f.write_all(&(bytes.len() as u32).to_le_bytes())?;
    f.write_all(bytes)?;
    f.flush()
}

/// 等管道上至少攒够 `need` 字节（最多等 `budget_ms`）。
///
/// 为什么不用 SetCommTimeouts：命名管道句柄上它直接失败（ERROR_INVALID_FUNCTION），
/// 而"读多久算超时"必须可控——否则服务一旦卡住，worker 就永久挂在这一次读上，
/// 连"换成新服务 exe"的热更新都做不了。所以用 PeekNamedPipe 轮询看字节数：
/// 消息都很小（<几十 KB）、轮询间隔 5ms，开销可以忽略。
fn wait_bytes(
    f: &std::fs::File,
    need: usize,
    budget_ms: u64,
    what: &str,
) -> std::io::Result<()> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;
    let h = HANDLE(f.as_raw_handle() as *mut core::ffi::c_void);
    let deadline = std::time::Instant::now() + Duration::from_millis(budget_ms);
    loop {
        let mut avail: u32 = 0;
        if let Err(e) = unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut avail), None) } {
            return Err(std::io::Error::other(format!("{what}：管道已断开（{e}）")));
        }
        if avail as usize >= need {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::other(format!(
                "{what}：等 {budget_ms}ms 只到了 {avail}/{need} 字节"
            )));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn recv(f: &mut std::fs::File, budget_ms: u64) -> std::io::Result<serde_json::Value> {
    wait_bytes(f, 4, budget_ms, "读消息头")?;
    let mut lb = [0u8; 4];
    f.read_exact(&mut lb)?;
    let len = u32::from_le_bytes(lb) as usize;
    if len == 0 || len > (1 << 20) {
        return Err(std::io::Error::other("消息长度异常"));
    }
    wait_bytes(f, len, budget_ms, "读消息体")?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::other(e.to_string()))
}

/// 拉起服务进程（不弹窗口）。
fn spawn_service() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = service_exe().ok_or_else(|| "找不到 xunyin-svc.exe".to_string())?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：服务是后台进程，不要黑框
    // 管道名被改过（测试隔离）时，拉起来的服务也得用同一个名字，否则两边对不上
    if let Ok(name) = std::env::var("XUNYIN_SVC_PIPE") {
        cmd.env("XUNYIN_SVC_PIPE", name);
    }
    cmd.spawn()
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
    let ack = recv(f, 5_000)?;
    let tag = ack.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let pid = ack.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let proto = ack.get("proto").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    Ok((tag, pid, proto))
}

/// 在给定连接上发一条消息并等回复（超时/断开返回说明文本）。
fn request_on(
    f: &mut std::fs::File,
    msg: &serde_json::Value,
    budget_ms: u64,
) -> Result<serde_json::Value, String> {
    send(f, msg).map_err(|e| format!("写失败 {e}"))?;
    recv(f, budget_ms).map_err(|e| format!("读失败 {e}"))
}

/// 服务 exe 的最后写入时刻（UNIX 毫秒）。
fn exe_mtime_ms(exe: &std::path::Path) -> Option<u64> {
    let md = std::fs::metadata(exe).ok()?;
    Some(
        md.modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64,
    )
}

/// 某个进程的启动时刻（UNIX 毫秒）。读不到（已退出/权限不够）返回 None。
fn process_start_ms(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut c = FILETIME::default();
        let mut e = FILETIME::default();
        let mut k = FILETIME::default();
        let mut u = FILETIME::default();
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        // FILETIME = 1601-01-01 起的 100ns 数 → 换成 UNIX 毫秒
        let ft = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
        Some(ft / 10_000 - 11_644_473_600_000)
    }
}

/// 文件时刻与进程启动时刻来自同一个时钟，这点容差只是防边界抖动。
const RESTART_SLACK_MS: u64 = 1_000;

/// 磁盘上的服务 exe 比"正在跑的那个服务进程"还新 → 踢掉旧进程、按新 exe 重来。
///
/// 这就是"改服务代码不用重开 Edge/ChatGPT"的核心判断（抽成纯函数是为了能单测）。
///
/// 为什么比"时刻"而不是比"两次读到的文件指纹"：后者在*部署之后才被加载进新宿主
/// 进程*的 DLL 身上会漏判——那时前后两次读到的指纹都是新 exe 的，看着"没变过"，
/// 于是一直用着旧服务。比"exe 写入时刻 vs 进程启动时刻"就没有这个洞。
fn needs_restart(exe_mtime: Option<u64>, svc_start: Option<u64>) -> bool {
    match (svc_start, exe_mtime) {
        // 还没连上任何服务：不需要"重启"，交给连接逻辑拉起
        (None, _) => false,
        // 服务在跑，但读不到 exe（被删/改名）→ 换新
        (Some(_), None) => true,
        // exe 是在服务进程启动**之后**才写的 → 跑的是旧代码
        (Some(start), Some(mtime)) => mtime > start + RESTART_SLACK_MS,
    }
}

/// 服务的协议版本能不能用（服务比 DLL 新也可以，只要不低于 DLL 需要的最低版本）。
fn proto_compatible(svc_proto: u32) -> bool {
    svc_proto >= PROTO
}

/// 心跳周期（也用来决定多久查一次"服务 exe 是否被重编替换"）。
const PING_INTERVAL: Duration = Duration::from_secs(2);

/// 后台线程主循环：连接 → 握手 → 预热 → 收发按键（含心跳），断了从头再来。
///
/// 它是这条管道连接**唯一**的读写者（见文件开头"心跳和按键必须同一个线程"的说明）。
fn worker() {
    // 请求投递口只建一次：连接断了也不影响投递（投不出去会超时，调用方退回本进程逻辑）
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let _ = JOBS.set(tx);
    let mut cur_pid: u32 = 0;
    loop {
        // ── 1) 连接 ──
        let mut conn = match connect() {
            Ok(f) => f,
            Err(_) => {
                // 服务没在跑：拉起来再连一次
                let spawn_err = spawn_service().err();
                match connect() {
                    Ok(f) => f,
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
        };

        // ── 2) 握手 + 版本校验 ──
        let (tag, pid, proto) = match handshake(&mut conn) {
            Ok(v) => v,
            Err(e) => {
                set_status(Status {
                    connected: false,
                    tag: String::new(),
                    pid: cur_pid,
                    last_error: format!("握手失败 {e}"),
                });
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if tag.is_empty() {
            // 不是我们的服务（管道名可能被别的程序占了）
            set_status(Status {
                connected: false,
                tag,
                pid,
                last_error: "握手响应异常".into(),
            });
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        if !proto_compatible(proto) {
            crate::tsf_log!("svc: 服务协议过旧（服务 proto={proto} / 需要 {PROTO}）→ 重启服务");
            if pid != 0 {
                kill_process(pid);
            }
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
        let detail = if tag == SVC_TAG {
            String::new()
        } else {
            format!("服务 {tag} / DLL 期望 {SVC_TAG}（协议兼容）")
        };

        // ── 3) 预热引擎 ──
        // 服务里的 librime 是"首次按键才初始化"的，首次还要把词库编译成索引，可能几十秒。
        // 不先把这一步做掉，用户敲的第一颗键就是几十秒的等待 → 必然超时。
        // 预热期间 connected 仍是 false：按键照旧走本进程逻辑，用户不会觉得"打不出字"。
        set_status(Status {
            connected: false,
            tag: tag.clone(),
            pid,
            last_error: "服务预热中（首次可能要几十秒）".into(),
        });
        let t0 = std::time::Instant::now();
        if let Err(e) = request_on(&mut conn, &serde_json::json!({ "t": "warmup" }), WARMUP_BUDGET_MS) {
            // 连接本身没毛病但服务没回话：这条连接**不能**继续用 —— 那条晚到的回复会被
            // 当成下一颗键的回复（消息错位）。丢掉它，重连一次。
            crate::tsf_log!(
                "svc: 引擎预热失败（{e}，{} ms）→ 重新连接",
                t0.elapsed().as_millis()
            );
            set_status(Status {
                connected: false,
                tag: String::new(),
                pid,
                last_error: format!("预热失败 {e}"),
            });
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        set_status(Status {
            connected: true,
            tag,
            pid,
            last_error: detail,
        });
        crate::tsf_log!(
            "svc: 引擎预热完成（{} ms）→ 按键交给服务",
            t0.elapsed().as_millis()
        );

        // ── 4) 收发循环：按键请求 + 心跳 + 热更新检查，全在这条连接上串行 ──
        let mut last_ping = std::time::Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(job) => match request_on(&mut conn, &job.msg, KEY_BUDGET_MS) {
                    Ok(v) => {
                        let _ = job.reply.send(v);
                    }
                    Err(e) => {
                        crate::tsf_log!(
                            "svc: 请求收发失败（{e}）→ 断开重连；这颗键由本进程逻辑处理"
                        );
                        let _ = job.reply.send(serde_json::Value::Null);
                        break;
                    }
                },
                // 没人按键：正常情况，靠下面的心跳维持连接
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                // 投递口断了（不会发生：JOBS 常驻持着发送端）；保守地停在这里，别空转
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            if last_ping.elapsed() >= PING_INTERVAL {
                last_ping = std::time::Instant::now();
                // 热更新：磁盘上的服务 exe 变了 → 踢掉旧进程（外层会按新 exe 重新拉起）
                let mtime = service_exe().as_deref().and_then(exe_mtime_ms);
                if needs_restart(mtime, process_start_ms(cur_pid)) {
                    crate::tsf_log!("svc: 磁盘上的服务已更新 → 重启服务进程（宿主应用无感）");
                    if cur_pid != 0 {
                        kill_process(cur_pid);
                    }
                    break;
                }
                match request_on(&mut conn, &serde_json::json!({ "t": "ping" }), 5_000) {
                    Ok(v) if v.get("t").and_then(|x| x.as_str()) == Some("pong") => {}
                    Ok(_) => {
                        crate::tsf_log!("svc: 心跳响应异常，重新连接");
                        break;
                    }
                    Err(e) => {
                        crate::tsf_log!("svc: 心跳失败（{e}），重新连接");
                        break;
                    }
                }
            }
        }

        // ── 5) 断开收尾：状态清掉、暂停转发，回到外层重连 ──
        drop(conn);
        set_status(Status {
            connected: false,
            tag: String::new(),
            pid: cur_pid,
            last_error: "连接已断开，重连中".into(),
        });
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 核心判断：磁盘上的服务 exe 比跑着的服务进程新 → 重启服务（热更新的触发点）
    #[test]
    fn restart_when_exe_newer_than_process() {
        let start = 1_700_000_000_000u64;
        assert!(
            !needs_restart(Some(start + 5_000), None),
            "还没连上服务时不算重启"
        );
        assert!(
            !needs_restart(Some(start - 5_000), Some(start)),
            "exe 是进程启动前写的 = 跑的就是它，不重启"
        );
        assert!(
            !needs_restart(Some(start + 500), Some(start)),
            "1 秒容差内不重启"
        );
        assert!(
            needs_restart(Some(start + 60_000), Some(start)),
            "exe 是进程启动后才写的 → 跑的是旧代码，要重启"
        );
        assert!(
            needs_restart(None, Some(start)),
            "读不到 exe（被删/改名）也要重启"
        );
    }

    /// 协议兼容性：服务比 DLL 新可以用，比 DLL 旧要重启
    #[test]
    fn proto_compat() {
        assert!(proto_compatible(PROTO));
        assert!(proto_compatible(PROTO + 1));
        assert!(!proto_compatible(PROTO - 1));
        assert!(!proto_compatible(0));
    }

    /// 服务把结果包在 `{"t":"reply","reply":{...}}` 里 —— 解析必须取出内层。
    ///
    /// 这条测试守的是"切到输入法后打不出中文"那类故障：把整条消息直接喂给带
    /// `#[serde(default)]` 的 `Reply` 不会报错，只会得到一份"consume=false、组合串为空"
    /// 的默认值，于是每颗键都被放行给应用、状态栏命令全部无效。
    #[test]
    fn parse_reply_unwraps_the_reply_field() {
        let msg = serde_json::json!({
            "t": "reply",
            "reply": {
                "consume": true, "preedit": "ni", "caret": 2,
                "candidates": ["你", "尼"], "selected": 0, "page_size": 9,
                "engine_ok": true
            }
        });
        let r = parse_reply(msg).expect("应当解析出服务回复");
        assert!(r.consume, "consume 必须被解析出来（否则键会被放行给应用）");
        assert_eq!(r.preedit, "ni");
        assert_eq!(r.caret, 2);
        assert_eq!(r.candidates, vec!["你".to_string(), "尼".to_string()]);
        assert!(r.engine_ok);

        // 没包一层的旧格式也要认
        let bare = serde_json::json!({ "consume": true, "preedit": "n" });
        assert!(parse_reply(bare).expect("旧格式也要能用").consume);

        // 服务侧报错 → 退回本进程逻辑
        let err = serde_json::json!({ "t": "reply", "reply": { "consume": false, "error": "boom" } });
        assert!(parse_reply(err).is_none(), "有 error 就退回本进程");

        // 完全不认识的消息（例如服务开了 XUNYIN_SVC_INPUT=0 时的回声）
        // **不能**被当成"服务已处理" —— 那又会退回"吞键/放行"的老毛病
        assert!(
            parse_reply(serde_json::json!({ "t": "echo", "from": "key" })).is_none(),
            "不认识的消息必须返回 None，让调用方退回本进程逻辑"
        );
    }

    /// 端到端：真的连一把服务 —— 握手 → 预热 → 按键 → **内容正确**。
    ///
    /// 这条测试守两件事（都是真实故障）：
    ///   1) 请求通道必须建得起来、按键必须能往返（曾经 worker 根本没建请求队列）；
    ///   2) 回包内容必须真的被解析出来 —— 只断言 `is_some()` 是不够的：
    ///      回复解成全默认值时同样是 `Some`，而结果是"打不出中文"。
    ///
    /// 用隔离的管道名（XUNYIN_SVC_PIPE），不打扰用户正在用的那个服务实例。
    /// 需要先构建服务：`cargo build --release --manifest-path ../xunyin-svc/Cargo.toml`。
    ///
    /// 手动跑：`cargo test -- --ignored --nocapture`
    #[test]
    #[ignore = "会拉起一个真实服务进程；手动跑 cargo test -- --ignored"]
    fn end_to_end_key_round_trip() {
        assert!(
            service_exe().is_some(),
            "找不到 xunyin-svc.exe：先 cargo build --release --manifest-path ../xunyin-svc/Cargo.toml"
        );
        std::env::set_var(
            "XUNYIN_SVC_PIPE",
            format!(r"\\.\pipe\xunyin-ime-svc-test-{}", std::process::id()),
        );
        // 这次要的是**真实的输入逻辑**（服务里的 librime），不是回声
        std::env::remove_var("XUNYIN_SVC_INPUT");
        poll();
        // 连上 + 预热成功之前 ready() 一直是 false
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        while !ready() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(ready(), "90s 没能连上服务并完成预热：{}", status_line());

        // n → 组合串 + 候选都必须解析出来
        let r = key(0x4E, 0, false).expect("按键没有回复 → 上游会丢键");
        assert!(r.consume, "N 应当被服务消费");
        assert!(r.engine_ok, "引擎应当就绪");
        assert_eq!(r.preedit, "n", "组合串没解析出来（回包被当成默认值了）");
        assert!(!r.candidates.is_empty(), "N 应当有候选");
        assert!(r.caret > 0, "组合内光标位置要能用");

        // 再加一颗键，确认不是只能通一次
        let r2 = key(0x49, 0, false).expect("第二颗键没有回复");
        assert!(r2.preedit.starts_with("ni"), "组合串应当累加：{:?}", r2.preedit);
        assert!(!r2.candidates.is_empty());

        // 收尾：把组合取消掉，别把状态留给别的客户端
        let _ = key(0x1B, 0, false);
    }
}
