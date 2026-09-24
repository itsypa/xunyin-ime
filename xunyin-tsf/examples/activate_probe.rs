//! 诊断探针：验证 Windows TSF 是否能枚举/激活迅音输入法档案。
//! 用法: cargo run --offline --example activate_probe

use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;
use windows_core::{GUID, PCWSTR};

const CLSID_XUNYIN_TSF: GUID = GUID::from_u128(0xf3a2d6e1_4c5b_4e8a_9c1d_2a6b7c8d9e0f);
const GUID_XUNYIN_PROFILE: GUID =
    GUID::from_u128(0x5b8e9a2f_6d7c_4f1a_8e3b_4c5d6e7f8a9b);

fn guid_str(g: &GUID) -> String {
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

fn main() -> windows_core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    unsafe {
        println!("== 获取 ITfInputProcessorProfileMgr ==");
        let mgr: ITfInputProcessorProfileMgr = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        )?;
        println!("  获取成功");

        println!("== EnumProfiles(0x0804) 系统可见的输入法档案 ==");
        if let Ok(en) = mgr.EnumProfiles(0x0804) {
            loop {
                let mut arr = [core::mem::zeroed::<TF_INPUTPROCESSORPROFILE>(); 1];
                let mut fetched = 0u32;
                if en.Next(&mut arr, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                let p = &arr[0];
                println!(
                    "  profileType={} langid={:04X} clsid={} guidProfile={} catid={} caps=0x{:08X} flags=0x{:08X}",
                    p.dwProfileType,
                    p.langid,
                    guid_str(&p.clsid),
                    guid_str(&p.guidProfile),
                    guid_str(&p.catid),
                    p.dwCaps,
                    p.dwFlags
                );
            }
        } else {
            println!("  EnumProfiles 失败");
        }

        println!("== GetActiveProfile(键盘类别) ==");
        let mut active = core::mem::zeroed::<TF_INPUTPROCESSORPROFILE>();
        match mgr.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active) {
            Ok(()) => println!(
                "  当前激活: clsid={} guidProfile={} flags=0x{:08X}",
                guid_str(&active.clsid),
                guid_str(&active.guidProfile),
                active.dwFlags
            ),
            Err(e) => println!("  GetActiveProfile ERR: {e:?}"),
        }

        println!("== ActivateProfile(迅音) ==");
        match mgr.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            0x0804,
            &CLSID_XUNYIN_TSF,
            &GUID_XUNYIN_PROFILE,
            HKL(std::ptr::null_mut()),
            0,
        ) {
            Ok(()) => println!("  ActivateProfile 返回 OK"),
            Err(e) => println!(
                "  ActivateProfile ERR: {e:?} (0x{:08X})",
                e.code().0 as u32
            ),
        }

        println!("== 等待 2.5 秒，检查 DLL 是否被加载 ==");
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let name: Vec<u16> = "xunyin_tsf_v15.dll".encode_utf16().chain(std::iter::once(0)).collect();
        match GetModuleHandleW(PCWSTR(name.as_ptr())) {
            Ok(h) => println!("  xunyin_tsf_v15.dll 已加载到本进程: {h:?}"),
            Err(_) => println!("  xunyin_tsf_v15.dll 未加载到本进程"),
        }

        println!("== 再次查询当前激活档案 ==");
        match mgr.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active) {
            Ok(()) => println!(
                "  当前激活: clsid={} guidProfile={} flags=0x{:08X}",
                guid_str(&active.clsid),
                guid_str(&active.guidProfile),
                active.dwFlags
            ),
            Err(e) => println!("  GetActiveProfile ERR: {e:?}"),
        }

        println!("== 模拟应用创建 TSF 线程管理器 ==");
        match CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)
        {
            Ok(tim) => match tim.Activate() {
                Ok(_tid) => {
                    println!("  ITfThreadMgr::Activate OK");
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    match GetModuleHandleW(PCWSTR(name.as_ptr())) {
                        Ok(h) => println!("  xunyin_tsf_v15.dll 已加载到本进程: {h:?}"),
                        Err(_) => println!("  xunyin_tsf_v15.dll 仍未加载到本进程"),
                    }
                }
                Err(e) => println!("  ITfThreadMgr::Activate ERR: {e:?}"),
            },
            Err(e) => println!("  创建 ITfThreadMgr ERR: {e:?}"),
        }
    }
    Ok(())
}
