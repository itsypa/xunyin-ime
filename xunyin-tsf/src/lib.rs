//! 迅音输入法 —— 系统级 TSF 文本服务。
//!
//! 这是一个 COM in-proc 服务器（cdylib），通过 `DllRegisterServer` 注册为
//! Windows TSF 输入处理器（Text Input Processor），在任意应用中提供
//! 拼音输入，无需全局键盘钩子与 SendInput 注入。
//!
//! 架构：
//!   xunyin_tsf.dll（本 crate，TSF 文本服务，内嵌 librime）
//!     ├─ TextService       激活/按键/组合/候选（实现 TSF 接口）
//!     ├─ CandidateListUI   候选列表（系统默认候选窗渲染）
//!     ├─ DisplayAttribute  组合串下划线显示属性
//!     └─ ClassFactory      COM 类工厂

mod candidate;
mod candidate_window;
mod class_factory;
mod display_attr;
mod edit_session;
mod engine;
mod key;
mod log;
mod register;
mod symbol_panel;
mod status_bar;
mod text_service;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_POINTER};
use windows_core::{GUID, HRESULT, Interface};

/// 诊断用版本标记：每次构建更新，日志第一行可确认加载的 DLL 版本。
pub const DLL_TAG: &str = "20260925-2000";

/// 文本服务 CLSID（注册到 HKCU\Software\Classes\CLSID）。
pub const CLSID_XUNYIN_TSF: GUID = GUID::from_u128(0x7c4e9a38_2b61_4d5f_a8c3_9e7b5f1d2a64);
/// 中文（简体，中国）语言档案 GUID。
pub const GUID_XUNYIN_PROFILE: GUID =
    GUID::from_u128(0xd5b8f2c7_4a93_4e18_b6d2_3c9f7a5e8b41);
/// 组合串显示属性 GUID。
pub const GUID_XUNYIN_DISPLAY_ATTR: GUID =
    GUID::from_u128(0x8a3d6e19_5c24_4b7f_9d31_2e6a8c4f1b73);
/// 候选 UI 元素 GUID。
pub const GUID_XUNYIN_UI_ELEMENT: GUID =
    GUID::from_u128(0x4f7b2d9c_1a63_4e85_b2d7_6c9a3e5f8d21);

/// 中文（简体，中国）。
pub const LANGID_ZH_CN: u16 = 0x0804;

/// COM 服务器入口：查询类工厂。
#[no_mangle]
pub extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> HRESULT {
    crate::tsf_log!("DllGetClassObject called (tag={})", DLL_TAG);
    unsafe {
        if ppv.is_null() {
            return E_POINTER;
        }
        *ppv = core::ptr::null_mut();
        if rclsid.is_null() || *rclsid != CLSID_XUNYIN_TSF {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory = windows_core::ComObject::new(class_factory::ClassFactory);
        let cf: windows::Win32::System::Com::IClassFactory = factory.into_interface();
        cf.query(riid, ppv)
    }
}

/// COM 服务器入口：是否可以卸载。本服务无长驻全局状态，始终可卸载。
#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    HRESULT(0) // S_OK
}

/// 注册文本服务（写入 HKCU，无需管理员）。
#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    match register::register_server() {
        Ok(()) => HRESULT(0),
        Err(e) => {
            eprintln!("[xunyin-tsf] DllRegisterServer failed: {e}");
            e.code()
        }
    }
}

/// 注销文本服务。
#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    match register::unregister_server() {
        Ok(()) => HRESULT(0),
        Err(e) => {
            eprintln!("[xunyin-tsf] DllUnregisterServer failed: {e}");
            e.code()
        }
    }
}

// rename-touch
