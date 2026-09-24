//! DllRegisterServer / DllUnregisterServer：TSF 输入处理器注册。
//!
//! 采用 HKCU 级注册（无需管理员）：
//!   * HKCU\Software\Classes\CLSID\{...} COM 类注册
//!   * ITfInputProcessorProfiles：TIP 语言档案
//!   * ITfCategoryMgr：TIP_KEYBOARD / DISPLAYATTRIBUTEPROVIDER / TSF3 等类别

use crate::{CLSID_XUNYIN_TSF, GUID_XUNYIN_PROFILE, LANGID_ZH_CN};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::*;
use windows_core::{GUID, PCWSTR};

pub fn register_server() -> windows_core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    let dll_path = module_path()?;
    let clsid = guid_string(&CLSID_XUNYIN_TSF);
    let base = format!(r"Software\Classes\CLSID\{clsid}");
    set_reg_string(HKEY_CURRENT_USER, &base, "", "迅音输入法 TSF 文本服务")?;
    let inproc = format!(r"{base}\InprocServer32");
    set_reg_string(HKEY_CURRENT_USER, &inproc, "", &dll_path)?;
    set_reg_string(HKEY_CURRENT_USER, &inproc, "ThreadingModel", "Apartment")?;

    // 注册 TIP 语言档案（中文简体）
    let profiles: ITfInputProcessorProfiles =
        unsafe { CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        )? };
    unsafe { profiles.Register(&CLSID_XUNYIN_TSF)? };
    let desc: Vec<u16> = "迅音输入法".encode_utf16().collect();
    unsafe { profiles.AddLanguageProfile(
        &CLSID_XUNYIN_TSF,
        LANGID_ZH_CN,
        &GUID_XUNYIN_PROFILE,
        &desc,
        &[0u16],
        0,
    )? };

    // 注册类别（与系统 CHS IME 一致的标准键盘 TIP 类别集；
    // 注意：不注册 TSF3 —— Win10 上 TSF3 类别会让 ctfmon 走 TSF3 加载路径，
    // 而我们只实现经典 TSF 契约，系统会因此跳过加载本 DLL）
    let catmgr: ITfCategoryMgr = unsafe {
        CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?
    };
    let categories = [
        GUID_TFCAT_TIP_KEYBOARD,
        GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
        GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
        GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
        GUID_TFCAT_TIPCAP_COMLESS,
        GUID_TFCAT_TIPCAP_DUALMODE,
        GUID_TFCAT_TIPCAP_SECUREMODE,
        GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    ];
    unsafe {
        for cat in categories {
            catmgr.RegisterCategory(&CLSID_XUNYIN_TSF, &cat, &CLSID_XUNYIN_TSF)?;
        }
    }

    // Category\Item\{clsid} 的显示名（与系统 IME 结构一致；HKLM 需要管理员，
    // regsvr32 提权运行时有效，失败不影响其余注册）
    let item_desc = format!(r"SOFTWARE\Microsoft\CTF\TIP\{clsid}\Category\Item\{clsid}");
    let _ = set_reg_string(HKEY_LOCAL_MACHINE, &item_desc, "", "迅音输入法");

    eprintln!(
        "[xunyin-tsf] registered: CLSID={clsid} dll={dll_path}"
    );
    Ok(())
}

pub fn unregister_server() -> windows_core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    if let Ok(profiles) = unsafe {
        CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        )
    } {
        let _ = unsafe { profiles.RemoveLanguageProfile(
            &CLSID_XUNYIN_TSF,
            LANGID_ZH_CN,
            &GUID_XUNYIN_PROFILE,
        ) };
        let _ = unsafe { profiles.Unregister(&CLSID_XUNYIN_TSF) };
    }
    if let Ok(catmgr) = unsafe {
        CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
    } {
        let categories = [
            GUID_TFCAT_TIP_KEYBOARD,
            GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
            GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
            GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
            GUID_TFCAT_TIPCAP_COMLESS,
            GUID_TFCAT_TIPCAP_DUALMODE,
            GUID_TFCAT_TIPCAP_SECUREMODE,
            GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
        ];
        for cat in categories {
            let _ = unsafe {
                catmgr.UnregisterCategory(&CLSID_XUNYIN_TSF, &cat, &CLSID_XUNYIN_TSF)
            };
        }
    }

    let clsid = guid_string(&CLSID_XUNYIN_TSF);
    let path = format!(r"Software\Classes\CLSID\{clsid}");
    unsafe {
        let path16: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path16.as_ptr()));
    }
    Ok(())
}

fn module_path() -> windows_core::Result<String> {
    unsafe {
        // GetModuleFileNameW(None) 返回的是宿主进程（regsvr32/被输入应用）路径，
        // 必须用本 DLL 内函数地址定位 DLL 自身模块。
        let mut hmod: windows::Win32::Foundation::HMODULE = core::mem::zeroed();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let addr = (module_path as fn() -> windows_core::Result<String>) as usize;
        GetModuleHandleExW(flags, PCWSTR(addr as *const u16), &mut hmod)?;
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmod), &mut buf);
        if len == 0 {
            return Err(windows_core::Error::from_win32());
        }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn set_reg_string(
    root: HKEY,
    subkey: &str,
    value_name: &str,
    data: &str,
) -> windows_core::Result<()> {
    unsafe {
        let subkey16: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let value_name16: Vec<u16> =
            value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut key: HKEY = HKEY(core::ptr::null_mut());
        let err = RegCreateKeyW(root, PCWSTR(subkey16.as_ptr()), &mut key);
        if err.0 != 0 {
            return Err(windows_core::Error::from_hresult(windows_core::HRESULT(
                err.0 as i32,
            )));
        }
        let wide: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let err2 = RegSetValueExW(
            key,
            PCWSTR(value_name16.as_ptr()),
            None,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(key);
        if err2.0 != 0 {
            return Err(windows_core::Error::from_hresult(windows_core::HRESULT(
                err2.0 as i32,
            )));
        }
        Ok(())
    }
}

fn guid_string(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    )
}
