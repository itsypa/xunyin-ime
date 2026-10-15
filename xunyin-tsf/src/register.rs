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
    // 语言栏/任务栏图标：把 DLL 同目录的 xunyin.ico 登记到 TIP 语言档案
    let ico = std::path::Path::new(&dll_path).parent().map(|d| d.join("xunyin.ico"));
    if let Some(ico) = ico.filter(|p| p.exists()) {
        let key = format!(
            r"Software\Microsoft\CTF\TIP\{}\LanguageProfile\0x00000804\{}",
            clsid,
            guid_string(&GUID_XUNYIN_PROFILE)
        );
        let _ = set_reg_string(HKEY_CURRENT_USER, &key, "IconFile", &ico.to_string_lossy());
        let _ = set_reg_dword(HKEY_CURRENT_USER, &key, "IconIndex", 0);
        // 语言列表里的显示名。AddLanguageProfile 写进去的那份没有以 NUL 结尾，
        // 注册表按 REG_SZ 读出来时后面会拖一串乱码（真实现象：
        // 「迅音输入法͜튠͜狑...」），所以这里用注册表 API 覆盖成干净的 REG_SZ。
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let _ = set_reg_string(root, &key, "Description", "迅音输入法");
        }
    }

    unsafe { profiles.Register(&CLSID_XUNYIN_TSF)? };
    // 末尾的 0 是 NUL 结尾：这个 API 写出来的档案名会被当 REG_SZ 读，
    // 不带结尾 0 的话读出来后面挂着一段乱码（见下面覆盖 Description 的说明）。
    let desc: Vec<u16> = "迅音输入法".encode_utf16().chain(std::iter::once(0)).collect();
    // 语言栏/任务栏图标：把 DLL 同目录的 xunyin.ico 作为**档案图标**传给 API。
    // 只写注册表 IconFile 时，语言栏仍显示语言默认的「简体」——图标必须经
    // AddLanguageProfile 的 bstrIconFile 参数注册才会被采用。
    let icon_u16: Vec<u16> = std::path::Path::new(&dll_path)
        .parent()
        .map(|d| d.join("xunyin.ico"))
        .filter(|p| p.exists())
        .map(|p| {
            p.to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        })
        .unwrap_or_default();
    // 已经是登记过的档案时这个调用会返回失败 —— **不能**因此中断后面的注册
    // （否则重复部署一次就把类别注册和用户输入法列表登记全跳过了）。
    if let Err(e) = unsafe {
        profiles.AddLanguageProfile(
            &CLSID_XUNYIN_TSF,
            LANGID_ZH_CN,
            &GUID_XUNYIN_PROFILE,
            &desc,
            &icon_u16,
            0,
        )
    } {
        eprintln!("[xunyin-tsf] AddLanguageProfile: {e}（多半是已登记过，继续）");
    }

    // 登记进"用户已安装输入法列表"（语言栏里能选到的那个列表）。
    //
    // 只调 AddLanguageProfile 是不够的：那只是把档案登记到**系统**，用户在语言栏 / Win+Space
    // 里能选到的列表（HKCU\Control Panel\International\User Profile）由 input.dll 的
    // InstallLayoutOrTip 维护。少这一步的后果不是"少一个选项"这么轻——机器上如果残留一条
    // 指向旧 CLSID 的条目，用户切过去时那个 CLSID 已经不存在，**一个字都打不出来**
    // （真实踩过：改了 CLSID 却没更新这个列表）。
    let lang16 = format!("{LANGID_ZH_CN:04X}");
    let profile_str = format!("{lang16}:{clsid}{}", guid_string(&GUID_XUNYIN_PROFILE));
    if !install_layout_or_tip(&profile_str, false) {
        eprintln!("[xunyin-tsf] InstallLayoutOrTip 失败，语言栏里可能看不到本输入法：{profile_str}");
    }

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

    // 先从"用户输入法列表"里摘掉（否则卸载后语言栏里还留着一个点了没反应的条目）
    let clsid = guid_string(&CLSID_XUNYIN_TSF);
    let profile_str = format!("{LANGID_ZH_CN:04X}:{clsid}{}", guid_string(&GUID_XUNYIN_PROFILE));
    let _ = install_layout_or_tip(&profile_str, true);

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

/// 调用 input.dll 的 `InstallLayoutOrTip`（系统输入法的入口）。
///
/// `profile` 形如 `0804:{CLSID}{PROFILE_GUID}`；`uninstall=true` 表示从用户的输入法
/// 列表里摘掉。这个导出没有正式文档，但正是语言栏 / Win+Space 背后用的那一个
/// （安装器也走它）。
fn install_layout_or_tip(profile: &str, uninstall: bool) -> bool {
    use windows::core::{s, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type Fn = unsafe extern "system" fn(*const u16, u32) -> bool;
    unsafe {
        let lib: Vec<u16> = "input.dll\0".encode_utf16().collect();
        let Ok(hmod) = LoadLibraryW(PCWSTR(lib.as_ptr())) else {
            return false;
        };
        let Some(proc) = GetProcAddress(hmod, s!("InstallLayoutOrTip")) else {
            return false;
        };
        let f: Fn = std::mem::transmute(proc);
        let wide: Vec<u16> = profile.encode_utf16().chain(std::iter::once(0)).collect();
        f(wide.as_ptr(), if uninstall { 1 } else { 0 })
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

/// 写一个 DWORD 注册表值（语言栏图标索引用）。
fn set_reg_dword(root: HKEY, subkey: &str, value_name: &str, data: u32) -> windows_core::Result<()> {
    unsafe {
        let subkey16: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let name16: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut key: HKEY = HKEY(core::ptr::null_mut());
        let err = RegCreateKeyW(root, PCWSTR(subkey16.as_ptr()), &mut key);
        if err.0 != 0 {
            return Err(windows_core::Error::from_hresult(windows_core::HRESULT(err.0 as i32)));
        }
        let bytes = data.to_le_bytes();
        let err2 = RegSetValueExW(key, PCWSTR(name16.as_ptr()), None, REG_DWORD, Some(&bytes));
        let _ = RegCloseKey(key);
        if err2.0 != 0 {
            return Err(windows_core::Error::from_hresult(windows_core::HRESULT(err2.0 as i32)));
        }
        Ok(())
    }
}
