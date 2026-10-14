//! rime.dll 的 FFI 绑定层。
//!
//! 直接对 librime 的 C API（rime_api.h）做声明，并提供安全的 Rust 封装。

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};

pub type RimeSessionId = usize;
pub type Bool = i32;

/// 说明：librime 往调用方给的结构体里写数据时，**按它自己的 sizeof 写**，
/// 不会按我们声明的结构体大小裁剪。这个 rime.dll（RimeGetVersion = 1.17.0）实测：
///   * sizeof(RimeContext) = 92 字节（本文件按 rime_api.h 定义出来只有 88）
///   * sizeof(RimeStatus)  = 60 字节（本文件是 56）
/// 也就是说，头文件定义比 DLL 少 4 字节 —— 每次 RimeGetContext 都会往调用方栈上多写
/// 4 字节，症状是"随机 0xC0000005、指针低 32 位被清零"（排查了很久的真凶）。
///
/// 因此下面这些"由 librime 填充"的结构体统一在尾部加 `_reserved` 余量：
///   * 字段偏移与本文件一致（前面的字段位置是对的，实测校验过）；
///   * 尾部余量保证 librime 多写几字节也只是写进余量里，不会写坏调用方的栈；
///   * 传给 librime 的 data_size 用 `offset_of!(T, _reserved)`，即"最后一个真实字段
///     的结束位置"，与 rime_api.h 里 RIME_STRUCT_INIT 的口径一致（sizeof - 4）。

/// librime 引擎是全局单例，其 API 函数指针表是线程安全的（内部有锁）。
/// 这里显式声明 Send/Sync，使 RimeApi 可放入共享状态。
unsafe impl Send for RimeApi {}
unsafe impl Sync for RimeApi {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RimeTraits {
    pub data_size: c_int,
    pub shared_data_dir: *const c_char,
    pub user_data_dir: *const c_char,
    pub distribution_name: *const c_char,
    pub distribution_code_name: *const c_char,
    pub distribution_version: *const c_char,
    pub app_name: *const c_char,
    pub modules: *const *const c_char,
    pub min_log_level: c_int,
    pub log_dir: *const c_char,
    pub prebuilt_data_dir: *const c_char,
    pub staging_dir: *const c_char,
    /// 尾部余量（见文件头说明）：librime 只读这个结构体，多留空间是为了
    /// 万一上游给 RimeTraits 加了字段，读到的也是我们这块内存而不是越界读。
    pub _reserved: [u8; 64],
}

impl Default for RimeTraits {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
pub struct RimeComposition {
    pub length: c_int,
    pub cursor_pos: c_int,
    pub sel_start: c_int,
    pub sel_end: c_int,
    pub preedit: *const c_char,
}

#[repr(C)]
pub struct RimeCandidate {
    pub text: *mut c_char,
    pub comment: *mut c_char,
    pub reserved: *mut c_void,
}

#[repr(C)]
pub struct RimeMenu {
    pub page_size: c_int,
    pub page_no: c_int,
    pub is_last_page: Bool,
    pub highlighted_candidate_index: c_int,
    pub num_candidates: c_int,
    pub candidates: *mut RimeCandidate,
    pub select_keys: *mut c_char,
}

#[repr(C)]
pub struct RimeContext {
    pub data_size: c_int,
    pub composition: RimeComposition,
    pub menu: RimeMenu,
    pub commit_text_preview: *mut c_char,
    pub select_labels: *mut *mut c_char,
    /// 尾部余量（见文件头说明）：librime 1.17 在这里还有 4 字节字段。
    pub _reserved: [u8; 40],
}

#[repr(C)]
pub struct RimeStatus {
    pub data_size: c_int,
    pub schema_id: *mut c_char,
    pub schema_name: *mut c_char,
    pub is_disabled: Bool,
    pub is_composing: Bool,
    pub is_ascii_mode: Bool,
    pub is_full_shape: Bool,
    pub is_simplified: Bool,
    pub is_traditional: Bool,
    pub is_ascii_punct: Bool,
    /// 尾部余量（见文件头说明）：librime 1.17 在这里还有 8 字节字段。
    pub _reserved: [u8; 16],
}

#[repr(C)]
pub struct RimeCommit {
    pub data_size: c_int,
    pub text: *mut c_char,
    /// 尾部余量（见文件头说明）。
    pub _reserved: [u8; 16],
}

/// 与 rime_api.h 中 `typedef struct rime_config_t { void* ptr; }` 保持一致。
#[repr(C)]
pub struct RimeConfig {
    pub ptr: *mut c_void,
}

/// 与 rime_api.h 中 rime_config_iterator_t 保持一致。
#[repr(C)]
pub struct RimeConfigIterator {
    pub list: *mut c_void,
    pub map: *mut c_void,
    pub index: c_int,
    pub key: *const c_char,
    pub path: *const c_char,
}

#[repr(C)]
pub struct RimeModule {
    pub data_size: c_int,
    pub module_name: *const c_char,
    pub initialize: Option<unsafe extern "C" fn()>,
    pub finalize: Option<unsafe extern "C" fn()>,
    pub get_api: Option<unsafe extern "C" fn() -> *mut c_void>,
}

/// 与 rime_api.h 中 rime_string_slice_t 保持一致。
#[repr(C)]
pub struct RimeStringSlice {
    pub str: *const c_char,
    pub length: usize,
}
pub type RimeProtoBuilder = c_void;

#[repr(C)]
pub struct RimeSchemaListItem {
    pub schema_id: *mut c_char,
    pub name: *mut c_char,
    pub reserved: *mut c_void,
}

#[repr(C)]
pub struct RimeSchemaList {
    pub size: usize,
    pub list: *mut RimeSchemaListItem,
}

#[repr(C)]
pub struct RimeApi {
    pub data_size: c_int,
    pub setup: Option<unsafe extern "C" fn(*mut RimeTraits)>,
    pub set_notification_handler: Option<
        unsafe extern "C" fn(
            Option<RimeNotificationHandler>,
            *mut c_void,
        ),
    >,
    pub initialize: Option<unsafe extern "C" fn(*mut RimeTraits)>,
    pub finalize: Option<unsafe extern "C" fn()>,
    pub start_maintenance: Option<unsafe extern "C" fn(Bool) -> Bool>,
    pub is_maintenance_mode: Option<unsafe extern "C" fn() -> Bool>,
    pub join_maintenance_thread: Option<unsafe extern "C" fn()>,
    pub deployer_initialize: Option<unsafe extern "C" fn(*mut RimeTraits)>,
    pub prebuild: Option<unsafe extern "C" fn() -> Bool>,
    pub deploy: Option<unsafe extern "C" fn() -> Bool>,
    pub deploy_schema: Option<unsafe extern "C" fn(*const c_char) -> Bool>,
    pub deploy_config_file: Option<
        unsafe extern "C" fn(*const c_char, *const c_char) -> Bool,
    >,
    pub sync_user_data: Option<unsafe extern "C" fn() -> Bool>,
    pub create_session: Option<unsafe extern "C" fn() -> RimeSessionId>,
    pub find_session: Option<unsafe extern "C" fn(RimeSessionId) -> Bool>,
    pub destroy_session: Option<unsafe extern "C" fn(RimeSessionId) -> Bool>,
    pub cleanup_stale_sessions: Option<unsafe extern "C" fn()>,
    pub cleanup_all_sessions: Option<unsafe extern "C" fn()>,
    pub process_key: Option<unsafe extern "C" fn(RimeSessionId, c_int, c_int) -> Bool>,
    pub commit_composition: Option<unsafe extern "C" fn(RimeSessionId) -> Bool>,
    pub clear_composition: Option<unsafe extern "C" fn(RimeSessionId)>,
    pub get_commit: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeCommit) -> Bool>,
    pub free_commit: Option<unsafe extern "C" fn(*mut RimeCommit)>,
    pub get_context: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeContext) -> Bool>,
    pub free_context: Option<unsafe extern "C" fn(*mut RimeContext)>,
    pub get_status: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeStatus) -> Bool>,
    pub free_status: Option<unsafe extern "C" fn(*mut RimeStatus)>,
    pub set_option: Option<unsafe extern "C" fn(RimeSessionId, *const c_char, Bool)>,
    pub get_option: Option<unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool>,
    pub set_property: Option<
        unsafe extern "C" fn(RimeSessionId, *const c_char, *const c_char),
    >,
    pub get_property: Option<
        unsafe extern "C" fn(RimeSessionId, *const c_char, *mut c_char, usize) -> Bool,
    >,
    pub get_schema_list: Option<unsafe extern "C" fn(*mut RimeSchemaList) -> Bool>,
    pub free_schema_list: Option<unsafe extern "C" fn(*mut RimeSchemaList)>,
    pub get_current_schema: Option<unsafe extern "C" fn(RimeSessionId, *mut c_char, usize) -> Bool>,
    pub select_schema: Option<unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool>,
    // configuration
    pub schema_open: Option<unsafe extern "C" fn(*const c_char, *mut RimeConfig) -> Bool>,
    pub config_open: Option<unsafe extern "C" fn(*const c_char, *mut RimeConfig) -> Bool>,
    pub config_close: Option<unsafe extern "C" fn(*mut RimeConfig) -> Bool>,
    pub config_get_bool: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut Bool) -> Bool>,
    pub config_get_int: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut c_int) -> Bool>,
    pub config_get_double: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut f64) -> Bool>,
    pub config_get_string: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut c_char, usize) -> Bool>,
    pub config_get_cstring: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> *const c_char>,
    pub config_update_signature: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> Bool>,
    pub config_begin_map: Option<unsafe extern "C" fn(*mut RimeConfigIterator, *mut RimeConfig, *const c_char) -> Bool>,
    pub config_next: Option<unsafe extern "C" fn(*mut RimeConfigIterator) -> Bool>,
    pub config_end: Option<unsafe extern "C" fn(*mut RimeConfigIterator)>,
    // testing
    pub simulate_key_sequence: Option<unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool>,
    // module
    pub register_module: Option<unsafe extern "C" fn(*mut RimeModule) -> Bool>,
    pub find_module: Option<unsafe extern "C" fn(*const c_char) -> *mut RimeModule>,
    pub run_task: Option<unsafe extern "C" fn(*const c_char) -> Bool>,
    // deprecated getters
    pub get_shared_data_dir: Option<unsafe extern "C" fn() -> *const c_char>,
    pub get_user_data_dir: Option<unsafe extern "C" fn() -> *const c_char>,
    pub get_sync_dir: Option<unsafe extern "C" fn() -> *const c_char>,
    pub get_user_id: Option<unsafe extern "C" fn() -> *const c_char>,
    pub get_user_data_sync_dir: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    // config init / string
    pub config_init: Option<unsafe extern "C" fn(*mut RimeConfig) -> Bool>,
    pub config_load_string: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> Bool>,
    // config setters
    pub config_set_bool: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, Bool) -> Bool>,
    pub config_set_int: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, c_int) -> Bool>,
    pub config_set_double: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, f64) -> Bool>,
    pub config_set_string: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *const c_char) -> Bool>,
    // config complex structures
    pub config_get_item: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut RimeConfig) -> Bool>,
    pub config_set_item: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char, *mut RimeConfig) -> Bool>,
    pub config_clear: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> Bool>,
    pub config_create_list: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> Bool>,
    pub config_create_map: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> Bool>,
    pub config_list_size: Option<unsafe extern "C" fn(*mut RimeConfig, *const c_char) -> usize>,
    pub config_begin_list: Option<unsafe extern "C" fn(*mut RimeConfigIterator, *mut RimeConfig, *const c_char) -> Bool>,
    // input
    pub get_input: Option<unsafe extern "C" fn(RimeSessionId) -> *const c_char>,
    pub get_caret_pos: Option<unsafe extern "C" fn(RimeSessionId) -> usize>,
    pub select_candidate: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub get_version: Option<unsafe extern "C" fn() -> *const c_char>,
    pub set_caret_pos: Option<unsafe extern "C" fn(RimeSessionId, usize)>,
    pub select_candidate_on_current_page: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub candidate_list_begin: Option<unsafe extern "C" fn(RimeSessionId, *mut c_void) -> Bool>,
    pub candidate_list_next: Option<unsafe extern "C" fn(*mut c_void) -> Bool>,
    pub candidate_list_end: Option<unsafe extern "C" fn(*mut c_void)>,
    pub user_config_open: Option<unsafe extern "C" fn(*const c_char, *mut RimeConfig) -> Bool>,
    pub candidate_list_from_index: Option<unsafe extern "C" fn(RimeSessionId, *mut c_void, c_int) -> Bool>,
    pub get_prebuilt_data_dir: Option<unsafe extern "C" fn() -> *const c_char>,
    pub get_staging_dir: Option<unsafe extern "C" fn() -> *const c_char>,
    pub commit_proto: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeProtoBuilder)>,
    pub context_proto: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeProtoBuilder)>,
    pub status_proto: Option<unsafe extern "C" fn(RimeSessionId, *mut RimeProtoBuilder)>,
    pub get_state_label: Option<unsafe extern "C" fn(RimeSessionId, *const c_char, Bool) -> *const c_char>,
    pub delete_candidate: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub delete_candidate_on_current_page: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub get_state_label_abbreviated: Option<unsafe extern "C" fn(RimeSessionId, *const c_char, Bool, Bool) -> RimeStringSlice>,
    pub set_input: Option<unsafe extern "C" fn(RimeSessionId, *const c_char) -> Bool>,
    pub get_shared_data_dir_s: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    pub get_user_data_dir_s: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    pub get_prebuilt_data_dir_s: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    pub get_staging_dir_s: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    pub get_sync_dir_s: Option<unsafe extern "C" fn(*mut c_char, usize)>,
    pub highlight_candidate: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub highlight_candidate_on_current_page: Option<unsafe extern "C" fn(RimeSessionId, usize) -> Bool>,
    pub change_page: Option<unsafe extern "C" fn(RimeSessionId, Bool) -> Bool>,
}

/// librime 的通知回调：`(context, session_id, message_type, message_value)`。
pub type RimeNotificationHandler =
    unsafe extern "C" fn(*mut c_void, RimeSessionId, *const c_char, *const c_char);

/// librime 口径的 data_size：**到最后一个真实字段结束为止的长度**
/// （等价于 rime_api.h 里的 `RIME_STRUCT_INIT`，即 sizeof - sizeof(data_size)）。
/// 直接用 sizeof 会把尾部 `_reserved` 余量也当成"有字段"，所以这里取 _reserved 的偏移。
pub const fn traits_data_size() -> c_int {
    std::mem::offset_of!(RimeTraits, _reserved) as c_int
}
pub const fn context_data_size() -> c_int {
    std::mem::offset_of!(RimeContext, _reserved) as c_int
}
pub const fn status_data_size() -> c_int {
    std::mem::offset_of!(RimeStatus, _reserved) as c_int
}
pub const fn commit_data_size() -> c_int {
    std::mem::offset_of!(RimeCommit, _reserved) as c_int
}

/// 从 C 字符串指针转为 Rust String（空指针返回空串）
pub unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

/// 从 C 字符串指针转为 Rust String（可空、可能已释放的场景用拷贝）
pub unsafe fn cstr_to_string_owned(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

/// Rust String -> CString
pub fn to_cstring(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

// 常用键码常量（供参考；实际按键码由 input_router 换算）
#[allow(dead_code)]
pub const RIME_KEYCODE_ENTER: c_int = 13;
#[allow(dead_code)]
pub const RIME_KEYCODE_BACKSPACE: c_int = 8;
#[allow(dead_code)]
pub const RIME_KEYCODE_ESCAPE: c_int = 27;
#[allow(dead_code)]
pub const RIME_KEYCODE_SPACE: c_int = 32;
#[allow(dead_code)]
pub const RIME_KEYCODE_TAB: c_int = 9;
#[allow(dead_code)]
pub const RIME_KEYCODE_SHIFT: c_int = 16;
#[allow(dead_code)]
pub const RIME_KEYCODE_CTRL: c_int = 17;
#[allow(dead_code)]
pub const RIME_KEYCODE_ALT: c_int = 18;

/// 通用按键修饰键掩码（与 librime 的 RimeModifier 一致）
#[allow(dead_code)]
pub const kRimeModifierNone: c_int = 0;
pub const kRimeModifierShift: c_int = 1 << 0;
pub const kRimeModifierControl: c_int = 1 << 1;
pub const kRimeModifierAlt: c_int = 1 << 2;
pub const kRimeModifierCaps: c_int = 1 << 3;
/// 按键释放位（librime 用 kReleaseMask 区分 down/up）。
pub const kRimeModifierRelease: c_int = 1 << 30;
