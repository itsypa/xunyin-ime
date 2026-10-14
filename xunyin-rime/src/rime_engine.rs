//! Rime 引擎高级封装：管理全局引擎实例与会话，向 UI 提供安全接口。

use crate::rime_ffi::*;
use serde::Serialize;
use std::ffi::{c_char, c_int, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RimeEngine {
    api: *const RimeApi,
    session: Option<RimeSessionId>,
    traits: Box<RimeTraits>,
    initialized: bool,
    /// 会话默认方案：创建会话时自动选中（覆盖 Rime 记忆的上次方案）。
    default_schema: Option<String>,
    /// 会话默认选项：创建会话时自动应用（full_shape/ascii_punct 等）。
    default_options: Vec<(String, bool)>,
}

// librime 是全局单例引擎；同一时刻只允许一个引擎实例存在。
static ENGINE_LOCK: Mutex<()> = Mutex::new(());

/// librime 结构体尺寸自检结果：正常情况下是 None。
/// 如果将来换了更新的 librime，它的结构体比 we 声明的余量还大，这里会留下提示
/// （宿主把 `abi_note()` 写进日志），提示我们去 rime_ffi.rs 补 `_reserved`。
static ABI_NOTE: Mutex<Option<String>> = Mutex::new(None);

/// 取一次 ABI 自检提示（没有就返回 None）。
pub fn abi_note() -> Option<String> {
    ABI_NOTE.lock().ok().and_then(|g| g.clone())
}

fn check_abi<'a>(what: &str, reported_data_size: c_int, declared: usize) -> bool {
    let reported = reported_data_size.max(0) as usize + 4;
    if reported > declared {
        if let Ok(mut g) = ABI_NOTE.lock() {
            *g = Some(format!(
                "librime 的 {what} 实际是 {reported} 字节，超过我们声明的 {declared} 字节，\
                 请增大 rime_ffi.rs 里对应结构体的 _reserved 余量"
            ));
        }
        return false;
    }
    true
}

// RimeEngine 内部含裸指针（FFI），但所有访问都经过调用方的 Mutex 串行化，
// 且 librime 自身是线程安全的全局单例，因此声明 Send + Sync。
unsafe impl Send for RimeEngine {}
unsafe impl Sync for RimeEngine {}

impl RimeEngine {
    /// 未初始化的占位实例（由设置中心启动时创建，随后真正初始化）。
    pub fn uninitialized() -> Self {
        Self {
            api: std::ptr::null(),
            session: None,
            traits: Box::new(RimeTraits::default()),
            initialized: false,
            default_schema: None,
            default_options: Vec::new(),
        }
    }

    /// 设置会话默认方案（例如 xunyin_rich）。
    pub fn set_default_schema(&mut self, schema_id: &str) {
        self.default_schema = Some(schema_id.to_string());
    }

    /// 设置会话默认选项（例如 ("full_shape", true)），创建会话时自动应用。
    pub fn set_default_options(&mut self, options: Vec<(String, bool)>) {
        self.default_options = options;
    }

    /// 把选项即时应用到当前会话（若存在），并更新会话默认值。
    pub fn refresh_options(&mut self, options: Vec<(String, bool)>) {
        self.default_options = options.clone();
        let Some(id) = self.session else {
            return;
        };
        let api = unsafe { &*self.api };
        if let Some(f) = api.set_option {
            for (name, value) in options.iter() {
                let n = to_cstring(name);
                unsafe { f(id, n.as_ptr(), *value as Bool) };
            }
        }
    }

    /// librime 版本号字符串（如 "1.13.0"），用于诊断。
    pub fn api_version(&self) -> Option<String> {
        if !self.initialized || self.api.is_null() {
            return None;
        }
        let api = unsafe { &*self.api };
        api.get_version
            .map(|f| unsafe { crate::rime_ffi::cstr_to_string(f()) })
    }

    /// RimeApi 结构体大小（诊断用）。
    pub fn api_data_size(&self) -> c_int {
        if self.api.is_null() {
            return 0;
        }
        unsafe { (*self.api).data_size }
    }

    /// 诊断：量出 librime **实际**往 RimeContext 里写了多少字节。
    /// 做法：拿一块 0xA5 填满的缓冲区当 RimeContext 传进去，调用后看它写到第几字节，
    /// 并读回它自己写进 data_size 的值（+4 就是 librime 的 sizeof(RimeContext)）。
    /// 若写入字节数 > 我方 sizeof，说明我方结构体比 librime 小 —— librime 会越界写调用方
    /// 的栈，正是"指针低 32 位被清零"这类栈损坏的来源。
    pub fn probe_context_size(&self) -> String {
        let Some(id) = self.session else {
            return "无会话".into();
        };
        let api = unsafe { &*self.api };
        let Some(get) = api.get_context else {
            return "无 get_context".into();
        };
        let mut buf = vec![0xA5u8; 512];
        unsafe {
            let ctx = buf.as_mut_ptr() as *mut RimeContext;
            (*ctx).data_size = context_data_size();
            let ok = get(id, ctx);
            let wrote = buf.iter().rposition(|b| *b != 0xA5).map(|i| i + 1).unwrap_or(0);
            let data_size = (*ctx).data_size;
            let menu = &(*ctx).menu;
            // 头文件里 commit_text_preview 在 72、select_labels 在 80；librime 多出的 4 字节
            // 到底加在"结构体尾部"还是"菜单内部"，用这两个位置上的指针值判断：
            // 位置对了才能读成合法字符串，位置错了会读到字节拼接出来的假指针。
            let bytes = std::slice::from_raw_parts(buf.as_ptr(), 96);
            let q = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());
            let s_at = |o: usize| -> String {
                let p = q(o) as *const c_char;
                if p.is_null() {
                    return "null".into();
                }
                let t = crate::rime_ffi::cstr_to_string(p);
                if t.is_empty() {
                    "空串".into()
                } else {
                    format!("{t:?}")
                }
            };
            let layout = format!(
                "偏移@72={:#x}({:?}) @76={:#x} @80={:#x}({:?}) @84={:#x} @88={:#x}",
                q(72),
                s_at(72),
                q(76),
                q(80),
                s_at(80),
                q(84),
                q(88),
            );
            // 候选数组（librime 分配）按我们的 RimeCandidate 步长读，验证元素布局
            let mut cands = String::new();
            if !menu.candidates.is_null() {
                for i in 0..menu.num_candidates.min(3).max(0) as usize {
                    let c = &*menu.candidates.add(i);
                    cands.push_str(&format!(
                        "[{}={:?}/{:?}]",
                        i,
                        crate::rime_ffi::cstr_to_string_owned(c.text),
                        crate::rime_ffi::cstr_to_string_owned(c.comment)
                    ));
                }
            }
            let s = format!(
                "ok={ok} 写入={wrote}B librime 回写 data_size={data_size}(=> 它的 sizeof={}) 我方 sizeof={} \
                 menu{{page_size={} page_no={} last={} hl={} num={} cands={:?} keys={:?}}} {layout} 前几个候选={cands}",
                data_size + 4,
                std::mem::size_of::<RimeContext>(),
                menu.page_size,
                menu.page_no,
                menu.is_last_page,
                menu.highlighted_candidate_index,
                menu.num_candidates,
                menu.candidates,
                menu.select_keys,
            );
            if let Some(free) = api.free_context {
                free(ctx);
            }
            s
        }
    }

    /// 诊断：同 `probe_context_size`，针对 RimeStatus。
    pub fn probe_status_size(&self) -> String {
        let Some(id) = self.session else {
            return "无会话".into();
        };
        let api = unsafe { &*self.api };
        let Some(get) = api.get_status else {
            return "无 get_status".into();
        };
        let mut buf = vec![0xA5u8; 512];
        unsafe {
            let st = buf.as_mut_ptr() as *mut RimeStatus;
            (*st).data_size = status_data_size();
            let ok = get(id, st);
            let wrote = buf.iter().rposition(|b| *b != 0xA5).map(|i| i + 1).unwrap_or(0);
            let data_size = (*st).data_size;
            let s = format!(
                "ok={ok} 写入={wrote}B librime 回写 data_size={data_size}(=> 它的 sizeof={}) 我方 sizeof={}",
                data_size + 4,
                std::mem::size_of::<RimeStatus>(),
            );
            if let Some(free) = api.free_status {
                free(st);
            }
            s
        }
    }

    /// 当前会话已应用的方案 id（无会话返回 None）。
    pub fn current_schema(&mut self) -> Option<String> {
        let id = self.session?;
        let api = unsafe { &*self.api };
        if let Some(get) = api.get_current_schema {
            let mut buf = [0i8; 256];
            if unsafe { get(id, buf.as_mut_ptr(), buf.len()) } != 0 {
                return Some(unsafe { crate::rime_ffi::cstr_to_string(buf.as_ptr()) });
            }
        }
        None
    }

    /// 是否已完成初始化。
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// 引擎是否处于 composition 中(有拼音串或候选)。
    pub fn is_composing(&mut self) -> bool {
        self.get_status()
            .map(|s| s.is_composing)
            .unwrap_or(false)
    }

    /// 初始化 rime.dll：加载动态库并获取 API 函数表。
    pub fn init(
        shared_data_dir: &str,
        user_data_dir: &str,
        app_name: &str,
    ) -> Result<Self, String> {
        let _guard = ENGINE_LOCK.lock().map_err(|_| "engine lock poisoned")?;

        // 直接链接 rime.lib 时用 rime_get_api 符号；这里是显式声明，避免依赖 link 配置。
        let api = unsafe { rime_get_api() };
        if api.is_null() {
            return Err("rime_get_api() returned null".into());
        }
        let api_ref = unsafe { &*api };
        let setup = api_ref
            .setup
            .ok_or("api.setup missing")?;
        let initialize = api_ref
            .initialize
            .ok_or("api.initialize missing")?;

        // 构建 RimeTraits：data_size 用 librime 的口径（到最后一个真实字段为止），
        // 不能直接用 sizeof —— 尾部 _reserved 余量不算字段。
        let mut traits = Box::new(RimeTraits::default());
        traits.data_size = traits_data_size();
        traits.shared_data_dir = to_cstring(shared_data_dir).into_raw();
        traits.user_data_dir = to_cstring(user_data_dir).into_raw();
        traits.distribution_name = to_cstring("迅音输入法").into_raw();
        traits.distribution_code_name = to_cstring("xunyin-ime").into_raw();
        traits.distribution_version = to_cstring("0.1.0").into_raw();
        traits.app_name = to_cstring(app_name).into_raw();
        traits.min_log_level = 1; // warning
        traits.log_dir = to_cstring("").into_raw(); // 默认日志目录
        // prebuilt 数据目录：随应用打包的已编译词库索引（${shared}/build）。
        // staging 目录：部署时的临时构建目录（${user}/build）。
        let prebuilt = format!("{}/build", shared_data_dir.trim_end_matches('/'));
        let staging = format!("{}/build", user_data_dir.trim_end_matches('/'));
        traits.prebuilt_data_dir = to_cstring(&prebuilt).into_raw();
        traits.staging_dir = to_cstring(&staging).into_raw();

        unsafe {
            setup(traits.as_mut());
            initialize(traits.as_mut());
            // 部署任务（installation_update 等）由 levers 模块注册，
            // 仅 RimeInitialize 不会加载它；补调 deployer_initialize
            // 后 deploy() 才能正常工作（否则报 unknown deployment task）。
            if let Some(deployer_initialize) = api_ref.deployer_initialize {
                deployer_initialize(traits.as_mut());
            }
        }

        Ok(Self {
            api,
            session: None,
            traits,
            initialized: true,
            default_schema: None,
            default_options: Vec::new(),
        })
    }

    /// 引擎是否处于维护（部署）模式。
    pub fn is_maintenance_mode(&self) -> bool {
        if !self.initialized {
            return false;
        }
        let api = unsafe { &*self.api };
        api.is_maintenance_mode
            .map(|f| unsafe { f() } != 0)
            .unwrap_or(false)
    }

    /// 启动部署流程（建索引）。调用方需保证在后台线程执行。
    pub fn deploy(&self) -> bool {
        if !self.initialized {
            return false;
        }
        let api = unsafe { &*self.api };
        match api.deploy {
            Some(f) => {
                let r = unsafe { f() };
                r != 0
            }
            None => false,
        }
    }

    /// 完整检查式部署（`RimeStartMaintenance(full_check=true)`）：
    /// 会重新扫描共享/用户目录里的方案与配置文件，包括**新增**的方案文件。
    /// 小狼毫的「重新部署」就是这个语义。
    pub fn deploy_full(&self) -> bool {
        if !self.initialized {
            return false;
        }
        let api = unsafe { &*self.api };
        match api.start_maintenance {
            Some(f) => {
                let r = unsafe { f(1) };
                r != 0
            }
            None => self.deploy(),
        }
    }

    /// 等待正在进行的部署结束（最多 `timeout_ms` 毫秒）。
    /// 返回 true 表示已结束（或本来就不在维护模式）。
    pub fn wait_deploy(&self, timeout_ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if !self.is_maintenance_mode() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// 只重建单个方案（含该方案主翻译器词典的编译）。
    pub fn deploy_schema(&self, schema_id: &str) -> bool {
        if !self.initialized {
            return false;
        }
        let api = unsafe { &*self.api };
        match api.deploy_schema {
            Some(f) => {
                let s = to_cstring(schema_id);
                let r = unsafe { f(s.as_ptr()) };
                r != 0
            }
            None => false,
        }
    }

    /// 同步用户词典到 sync 目录（导出为 .txt 文本格式）。
    /// librime 会把用户词库写入 ${sync_dir}/用户ID/ 下。
    /// 注：librime 1.17 的 RimeApi 该字段为 null，实际同步由 lib.rs 手动实现。
    #[allow(dead_code)]
    pub fn sync_user_data(&self) -> bool {
        if !self.initialized {
            return false;
        }
        let api = unsafe { &*self.api };
        match api.sync_user_data {
            Some(f) => {
                let r = unsafe { f() };
                r != 0
            }
            None => false,
        }
    }

    /// 确保存在会话，返回会话 id。
    pub fn ensure_session(&mut self) -> RimeSessionId {
        if !self.initialized {
            return 0;
        }
        if let Some(id) = self.session {
            return id;
        }
        let api = unsafe { &*self.api };
        let Some(create) = api.create_session else {
            return 0;
        };
        let id = unsafe { create() };
        if id != 0 {
            self.session = Some(id);
            let api = unsafe { &*self.api };
            if let Some(schema) = self.default_schema.clone() {
                if let Some(f) = api.select_schema {
                    let s = to_cstring(&schema);
                    unsafe { f(id, s.as_ptr()) };
                }
            }
            if let Some(f) = api.set_option {
                for (name, value) in self.default_options.iter() {
                    let n = to_cstring(name);
                    unsafe { f(id, n.as_ptr(), *value as Bool) };
                }
            }
        }
        id
    }

    /// 销毁当前会话（切换输入框时调用）。
    pub fn destroy_session(&mut self) {
        if !self.initialized {
            return;
        }
        if let Some(id) = self.session {
            let api = unsafe { &*self.api };
            if let Some(f) = api.destroy_session {
                unsafe { f(id) };
            }
            self.session = None;
        }
    }

    /// 处理一次按键。keycode 为虚拟键码，mask 为修饰键位掩码。
    /// 返回 true 表示该按键被 Rime 消费（不需要再传给系统）。
    pub fn process_key(&mut self, keycode: i32, mask: i32) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        let f = api.process_key.expect("api.process_key missing");
        let r = unsafe { f(id, keycode, mask) };
        r != 0
    }

    /// 处理一段文本输入（如粘贴、语音输入），返回是否被消费。
    /// 优先使用 set_input（支持任意 Unicode）；老版本 librime 无此接口时
    /// 退化为逐字符按键序列。
    pub fn process_input(&mut self, input: &str) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.set_input {
            let s = to_cstring(input);
            return unsafe { f(id, s.as_ptr()) } != 0;
        }
        if let Some(f) = api.process_key {
            let mut consumed = false;
            for ch in input.chars() {
                let keycode = ch as i32;
                consumed = unsafe { f(id, keycode, 0) } != 0 || consumed;
            }
            return consumed;
        }
        false
    }

    /// 取回已上屏的文本（如果有未读的 commit）。
    pub fn get_commit_text(&mut self) -> String {
        let id = self.ensure_session();
        if id == 0 {
            return String::new();
        }
        let api = unsafe { &*self.api };
        if let (Some(get), Some(free)) = (api.get_commit, api.free_commit) {
            let mut commit = RimeCommit {
                data_size: commit_data_size(),
                text: std::ptr::null_mut(),
                _reserved: [0; 16],
            };
            let ok = unsafe { get(id, &mut commit) } != 0;
            let text = if ok {
                unsafe { cstr_to_string_owned(commit.text) }
            } else {
                String::new()
            };
            unsafe { free(&mut commit) };
            return text;
        }
        String::new()
    }

    /// 当前会话的原始输入串（如拼音 "nihao"），用于“Shift 直接上屏拼音”。
    /// 返回的指针在输入变化前有效，这里立即拷贝。
    pub fn get_input(&mut self) -> String {
        let Some(id) = self.session else {
            return String::new();
        };
        let api = unsafe { &*self.api };
        if let Some(f) = api.get_input {
            return unsafe { crate::rime_ffi::cstr_to_string(f(id)) };
        }
        String::new()
    }

    /// 设置原始输入串中的光标位置（方向键逐字移动用；librime 按原始输入字节计）。
    pub fn set_caret_pos(&mut self, pos: usize) {
        let Some(id) = self.session else {
            return;
        };
        let api = unsafe { &*self.api };
        if let Some(f) = api.set_caret_pos {
            unsafe { f(id, pos) };
        }
    }

    /// 获取当前输入上下文（拼音串 + 候选菜单）。
    pub fn get_context(&self) -> Option<RimeContextSnapshot> {
        let id = self.session?;
        if id == 0 {
            return None;
        }
        let api = unsafe { &*self.api };
        let (get, free) = (api.get_context?, api.free_context?);
        let mut ctx = RimeContext {
            data_size: context_data_size(),
            composition: RimeComposition {
                length: 0,
                cursor_pos: 0,
                sel_start: 0,
                sel_end: 0,
                preedit: std::ptr::null(),
            },
            menu: RimeMenu {
                page_size: 0,
                page_no: 0,
                is_last_page: 0,
                highlighted_candidate_index: 0,
                num_candidates: 0,
                candidates: std::ptr::null_mut(),
                select_keys: std::ptr::null_mut(),
            },
            commit_text_preview: std::ptr::null_mut(),
            select_labels: std::ptr::null_mut(),
            _reserved: [0; 40],
        };
        let ok = unsafe { get(id, &mut ctx) } != 0;
        if !ok {
            unsafe { free(&mut ctx) };
            return None;
        }
        check_abi("RimeContext", ctx.data_size, std::mem::size_of::<RimeContext>());
        let preedit = unsafe { cstr_to_string(ctx.composition.preedit) };
        let commit_preview = unsafe { cstr_to_string_owned(ctx.commit_text_preview) };
        let mut candidates = Vec::new();
        let num = ctx.menu.num_candidates.max(0) as usize;
        if !ctx.menu.candidates.is_null() {
            for i in 0..num {
                let cand = unsafe { &*ctx.menu.candidates.add(i) };
                candidates.push(Candidate {
                    text: unsafe { cstr_to_string_owned(cand.text) },
                    comment: unsafe { cstr_to_string_owned(cand.comment) },
                });
            }
        }
        let snapshot = RimeContextSnapshot {
            preedit,
            commit_preview,
            candidates,
            highlighted: ctx.menu.highlighted_candidate_index,
            page_no: ctx.menu.page_no,
            page_size: ctx.menu.page_size,
            // 组合串显示文本内的光标位置（preedit 的 UTF-8 字节偏移，
            // 来自 librime 的 preedit.caret_pos；方向键移动光标后随之变化）。
            cursor_pos: ctx.composition.cursor_pos.max(0) as usize,
        };
        unsafe { free(&mut ctx) };
        Some(snapshot)
    }

    /// 获取状态（schema 名、ascii 模式、全角等）。
    pub fn get_status(&self) -> Option<RimeStatusSnapshot> {
        let id = self.session?;
        if id == 0 {
            return None;
        }
        let api = unsafe { &*self.api };
        let (get, free) = (api.get_status?, api.free_status?);
        let mut st = RimeStatus {
            data_size: status_data_size(),
            schema_id: std::ptr::null_mut(),
            schema_name: std::ptr::null_mut(),
            is_disabled: 0,
            is_composing: 0,
            is_ascii_mode: 0,
            is_full_shape: 0,
            is_simplified: 0,
            is_traditional: 0,
            is_ascii_punct: 0,
            _reserved: [0; 16],
        };
        let ok = unsafe { get(id, &mut st) } != 0;
        if !ok {
            unsafe { free(&mut st) };
            return None;
        }
        check_abi("RimeStatus", st.data_size, std::mem::size_of::<RimeStatus>());
        let snapshot = RimeStatusSnapshot {
            schema_id: unsafe { cstr_to_string_owned(st.schema_id) },
            schema_name: unsafe { cstr_to_string_owned(st.schema_name) },
            is_composing: st.is_composing != 0,
            is_ascii_mode: st.is_ascii_mode != 0,
            is_full_shape: st.is_full_shape != 0,
            is_simplified: st.is_simplified != 0,
            is_ascii_punct: st.is_ascii_punct != 0,
        };
        unsafe { free(&mut st) };
        Some(snapshot)
    }

    /// 选择候选项（index 从 0 开始）。
    pub fn select_candidate(&mut self, index: usize) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.select_candidate {
            return unsafe { f(id, index) } != 0;
        }
        false
    }

    /// 删除当前页第 `index` 个候选（Ctrl+数字）：从用户词库/词频里抹掉这个错词，
    /// 之后不再优先出现。librime 1.7+ 提供该接口。
    pub fn delete_candidate_on_current_page(&mut self, index: usize) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.delete_candidate_on_current_page {
            return unsafe { f(id, index) } != 0;
        }
        false
    }

    /// 删除整页里第 `global_index` 个候选（跨页序号）。
    pub fn delete_candidate(&mut self, global_index: usize) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.delete_candidate {
            return unsafe { f(id, global_index) } != 0;
        }
        false
    }

    /// 候选翻页。
    pub fn page_up(&mut self) -> bool {
        self.process_key(0xff55 /* PageUp */, 0)
    }

    pub fn page_down(&mut self) -> bool {
        self.process_key(0xff56 /* PageDown */, 0)
    }

    /// 提交当前高亮候选（Enter 行为）。
    pub fn commit_composition(&mut self) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        api.commit_composition
            .map(|f| unsafe { f(id) } != 0)
            .unwrap_or(false)
    }

    /// 清空当前输入（Esc 行为）。
    pub fn clear_composition(&mut self) {
        let id = self.ensure_session();
        if id == 0 {
            return;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.clear_composition {
            unsafe { f(id) };
        }
    }

    /// 切换选项（ascii_mode / full_shape 等）。
    pub fn set_option(&mut self, name: &str, value: bool) {
        let id = self.ensure_session();
        if id == 0 {
            return;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.set_option {
            let s = to_cstring(name);
            unsafe { f(id, s.as_ptr(), value as Bool) };
        }
    }

    pub fn get_option(&mut self, name: &str) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.get_option {
            let s = to_cstring(name);
            return unsafe { f(id, s.as_ptr()) } != 0;
        }
        false
    }

    /// 切换输入方案。
    pub fn select_schema(&mut self, schema_id: &str) -> bool {
        let id = self.ensure_session();
        if id == 0 {
            return false;
        }
        let api = unsafe { &*self.api };
        if let Some(f) = api.select_schema {
            let s = to_cstring(schema_id);
            return unsafe { f(id, s.as_ptr()) } != 0;
        }
        false
    }

    /// 获取可用的输入方案列表。
    pub fn get_schema_list(&self) -> Vec<(String, String)> {
        if !self.initialized || self.api.is_null() {
            return Vec::new();
        }
        let api = unsafe { &*self.api };
        let Some(get) = api.get_schema_list else {
            return Vec::new();
        };
        let Some(free) = api.free_schema_list else {
            return Vec::new();
        };
        let mut list = RimeSchemaList {
            size: 0,
            list: std::ptr::null_mut(),
        };
        let ok = unsafe { get(&mut list) } != 0;
        if !ok {
            return Vec::new();
        }
        let mut out = Vec::new();
        if !list.list.is_null() {
            for i in 0..list.size {
                let item = unsafe { &*list.list.add(i) };
                out.push((
                    unsafe { cstr_to_string_owned(item.schema_id) },
                    unsafe { cstr_to_string_owned(item.name) },
                ));
            }
        }
        unsafe { free(&mut list) };
        out
    }

    /// 引擎释放（进程退出时调用）。
    pub fn finalize(&mut self) {
        if !self.initialized {
            return;
        }
        let api = unsafe { &*self.api };
        // 先销毁会话，再 finalize 引擎（finalize 之后不可再调用任何 API）
        if let Some(id) = self.session.take() {
            if let Some(f) = api.destroy_session {
                unsafe { f(id) };
            }
        }
        if let Some(f) = api.finalize {
            unsafe { f() };
        }
        // 释放 traits 中持有的 C 字符串
        let t = &mut *self.traits;
        for ptr in [
            t.shared_data_dir,
            t.user_data_dir,
            t.distribution_name,
            t.distribution_code_name,
            t.distribution_version,
            t.app_name,
            t.log_dir,
            t.prebuilt_data_dir,
            t.staging_dir,
        ] {
            if !ptr.is_null() {
                unsafe { drop(CString::from_raw(ptr as *mut c_char)) };
            }
        }
        // 置空指针，保证 finalize 可重复调用（避免双重释放）
        t.shared_data_dir = std::ptr::null();
        t.user_data_dir = std::ptr::null();
        t.distribution_name = std::ptr::null();
        t.distribution_code_name = std::ptr::null();
        t.distribution_version = std::ptr::null();
        t.app_name = std::ptr::null();
        t.log_dir = std::ptr::null();
        t.prebuilt_data_dir = std::ptr::null();
        t.staging_dir = std::ptr::null();
        self.initialized = false;
        self.api = std::ptr::null();
    }
}

impl Drop for RimeEngine {
    fn drop(&mut self) {
        // finalize 由显式调用负责（设置中心退出时），这里不做以免双重释放
    }
}

// 从 rime.dll 导出符号获取 API（直接链接 rime.lib 时存在）。
unsafe extern "C" {
    fn rime_get_api() -> *const RimeApi;
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RimeContextSnapshot {
    pub preedit: String,
    pub commit_preview: String,
    pub candidates: Vec<Candidate>,
    pub highlighted: i32,
    pub page_no: i32,
    pub page_size: i32,
    pub cursor_pos: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RimeStatusSnapshot {
    pub schema_id: String,
    pub schema_name: String,
    pub is_composing: bool,
    pub is_ascii_mode: bool,
    pub is_full_shape: bool,
    pub is_simplified: bool,
    pub is_ascii_punct: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端验证：初始化引擎 -> 输入拼音 -> 获取候选 -> 上屏
    #[test]
    fn test_engine_pinyin_input() {
        // 使用打包的 rime 数据目录（dev 模式资源目录）
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let shared = manifest
            .join("..")
            .join("xunyin-ime")
            .join("rime")
            .join("data");
        let user = std::env::temp_dir().join("xunyin-ime-test-user");
        std::fs::create_dir_all(&user).ok();

        let mut engine = RimeEngine::init(
            &shared.to_string_lossy(),
            &user.to_string_lossy(),
            "xunyin-ime-test",
        )
        .expect("engine init failed");

        // 强制部署并等待完成（build/default.yaml 生成即视为完成）
        let _ = engine.deploy();
        for _ in 0..600 {
            if !engine.is_maintenance_mode() && user.join("build").join("default.yaml").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        assert!(!engine.is_maintenance_mode(), "deploy did not finish");

        // 确保有会话
        let id = engine.ensure_session();
        assert!(id != 0, "session creation failed");

        // 输入 "nihao"
        for ch in "nihao".chars() {
            let keycode = ch as i32;
            assert!(engine.process_key(keycode, 0), "key {ch} not consumed");
        }

        // 获取上下文：应有 preedit "ni hao" 和候选
        let ctx = engine.get_context().expect("no context");
        // 显示可能为 "ni hao"（音节切分）或 "nihao"（整词码），均属正常
        assert!(
            ctx.preedit == "ni hao" || ctx.preedit == "nihao",
            "preedit mismatch: {}",
            ctx.preedit
        );
        assert!(!ctx.candidates.is_empty(), "no candidates for nihao");
        assert!(
            ctx.candidates.iter().any(|c| c.text == "你好"),
            "「你好」不在候选里: {:?}",
            ctx.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
        );
        eprintln!("preedit={} candidates={:?}", ctx.preedit, ctx.candidates);

        // 用数字键 '1' 选第一个候选（真实按键流，验证 recognizer 不吞数字键）
        let consumed = engine.process_key('1' as i32, 0);
        assert!(consumed, "digit '1' not consumed for candidate selection");
        let commit = engine.get_commit_text();
        assert_eq!(commit, "你好", "digit selection commit mismatch: {commit}");
        eprintln!("committed: {commit}");

        engine.finalize();
    }

    /// 验证 schema 列表可用
    #[test]
    fn test_schema_list() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let shared = manifest
            .join("..")
            .join("xunyin-ime")
            .join("rime")
            .join("data");
        let user = std::env::temp_dir().join("xunyin-ime-test-user2");
        std::fs::create_dir_all(&user).ok();
        let mut engine = RimeEngine::init(
            &shared.to_string_lossy(),
            &user.to_string_lossy(),
            "xunyin-ime-test",
        )
        .expect("engine init failed");
        let _ = engine.deploy();
        for _ in 0..600 {
            if !engine.is_maintenance_mode() && user.join("build").join("default.yaml").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let schemas = engine.get_schema_list();
        assert!(!schemas.is_empty(), "no schemas found");
        eprintln!("schemas: {:?}", schemas);

        // 迅音拼音方案：输入 "kaifa" 应出现「开发」（办公词库验证）
        if schemas.iter().any(|(id, _)| id == "xunyin_pinyin") {
            engine.select_schema("xunyin_pinyin");
            let id = engine.ensure_session();
            assert!(id != 0);
            for ch in "kaifa".chars() {
                engine.process_key(ch as i32, 0);
            }
            let ctx = engine.get_context().expect("no context for xunyin_pinyin");
            eprintln!("xunyin_pinyin preedit={} candidates={:?}", ctx.preedit, ctx.candidates);
            assert!(
                ctx.candidates.iter().any(|c| c.text == "开发"),
                "「开发」不在候选里: {:?}",
                ctx.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
            );
        }

        // 验证用户词库目录存在（sync 的源数据）
        let userdb_exists = std::fs::read_dir(&user)
            .map(|mut d| d.any(|e| e.map(|e| e.file_name().to_string_lossy().ends_with(".userdb")).unwrap_or(false)))
            .unwrap_or(false);
        eprintln!("userdb exists: {userdb_exists}");

        engine.finalize();
    }

    /// 验证随包数据只包含迅音拼音方案，且 OpenCC 简化数据齐备。
    ///
    /// 本输入法只做拼音：五笔/仓颉/双拼/笔画等方案已从随包数据中移除，
    /// 方案列表必须只剩「迅音拼音（增强词库）」与「迅音拼音」。
    #[test]
    fn test_opencc_simplifier() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let shared = manifest
            .join("..")
            .join("xunyin-ime")
            .join("rime")
            .join("data");
        let user = std::env::temp_dir().join("xunyin-ime-test-user3");
        std::fs::create_dir_all(&user).ok();

        // 数据完整性：opencc 配置与词典必须随包发布
        assert!(
            shared.join("opencc").join("t2s.json").exists(),
            "opencc/t2s.json 缺失"
        );
        assert!(
            shared.join("opencc").join("STCharacters.ocd2").exists(),
            "opencc/STCharacters.ocd2 缺失"
        );
        for gone in [
            "cangjie5.schema.yaml",
            "cangjie5.dict.yaml",
            "double_pinyin.schema.yaml",
            "double_pinyin_flypy.schema.yaml",
            "double_pinyin_mspy.schema.yaml",
            "stroke.schema.yaml",
            "stroke.dict.yaml",
            "luna_pinyin.schema.yaml",
        ] {
            assert!(
                !shared.join(gone).exists(),
                "与拼音无关的方案数据仍在随包文件里：{gone}"
            );
        }

        let mut engine = RimeEngine::init(
            &shared.to_string_lossy(),
            &user.to_string_lossy(),
            "xunyin-ime-test",
        )
        .expect("engine init failed");
        let _ = engine.deploy();
        for _ in 0..600 {
            if !engine.is_maintenance_mode() && user.join("build").join("default.yaml").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        let schemas = engine.get_schema_list();
        let ids: Vec<String> = schemas.iter().map(|(id, _)| id.clone()).collect();
        eprintln!("随包方案列表: {ids:?}");
        assert_eq!(
            ids,
            vec!["xunyin_rich".to_string(), "xunyin_pinyin".to_string()],
            "方案列表应只剩迅音拼音的两个方案"
        );

        engine.finalize();
    }
}
