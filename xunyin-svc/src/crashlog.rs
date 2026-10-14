//! 崩溃现场记录：服务是被宿主应用隐藏启动的，崩了看不到任何信息。
//! 这里挂一个 Vectored Exception Handler，把"哪条指令、读写哪个地址、调用链是什么"
//! 落到服务日志里——定位越界/栈损坏只需要读一行日志，不用装调试器。
//!
//! 只在致命异常（访问违例、栈溢出、栈被改写、堆损坏、非法指令）时写日志，
//! 且全局最多写 8 次，避免异常里再异常导致日志刷屏。

#![allow(non_snake_case)]

#[cfg(target_arch = "x86_64")]
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
#[cfg(target_arch = "x86_64")]
use windows::Win32::System::Diagnostics::Debug::CONTEXT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{GetModuleInformation, MODULEINFO};
use windows::Win32::System::Threading::GetCurrentProcess;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

// 栈展开用的例程只在 x64 上用到（32 位没有 unwind 信息，直接跳过调用链）
#[cfg(target_arch = "x86_64")]
extern "system" {
    fn RtlLookupFunctionEntry(
        control_pc: u64,
        image_base: *mut u64,
        history_table: *mut c_void,
    ) -> *mut c_void;
    fn RtlVirtualUnwind(
        handler_type: u32,
        image_base: u64,
        control_pc: u64,
        function_entry: *mut c_void,
        context_record: *mut CONTEXT,
        handler_data: *mut *mut c_void,
        establisher_frame: *mut u64,
        context_pointers: *mut c_void,
    ) -> *mut c_void;
}

static CRASHES: AtomicU32 = AtomicU32::new(0);

/// 安装崩溃记录（进程启动时调用一次）。
pub fn install() {
    unsafe {
        AddVectoredExceptionHandler(1, Some(veh));
    }
}

fn is_fatal(code: u32) -> bool {
    matches!(
        code,
        0xC000_0005 // ACCESS_VIOLATION
            | 0xC000_00FD // STACK_OVERFLOW
            | 0xC000_0409 // STACK_BUFFER_OVERRUN
            | 0xC000_0374 // HEAP_CORRUPTION
            | 0xC000_001D // ILLEGAL_INSTRUCTION
            | 0xC000_001E // PRIVILEGED_INSTRUCTION
            | 0xC000_0094 // INTEGER_DIVIDE_BY_ZERO
            | 0xC000_008C // ARRAY_BOUNDS_EXCEEDED
    )
}

/// 我们自己的模块基址与大小（用于把绝对地址折算成 RVA，好和 map/pdb 对齐）。
fn self_module() -> (u64, u64) {
    unsafe {
        let Ok(h) = GetModuleHandleW(None) else {
            return (0, 0);
        };
        let mut info = MODULEINFO::default();
        if GetModuleInformation(GetCurrentProcess(), h, &mut info, std::mem::size_of::<MODULEINFO>() as u32)
            .is_err()
        {
            return (h.0 as u64, 0);
        }
        (info.lpBaseOfDll as u64, info.SizeOfImage as u64)
    }
}

pub fn module_base(name: &str) -> (u64, u64) {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = GetModuleHandleW(windows_core::PCWSTR(wide.as_ptr())).unwrap_or(HMODULE(std::ptr::null_mut()));
        if h.0.is_null() {
            return (0, 0);
        }
        let mut info = MODULEINFO::default();
        if GetModuleInformation(GetCurrentProcess(), h, &mut info, std::mem::size_of::<MODULEINFO>() as u32).is_err()
        {
            return (h.0 as u64, 0);
        }
        (info.lpBaseOfDll as u64, info.SizeOfImage as u64)
    }
}

/// 把地址渲染成 "0x<rva>(+<模块>)" 或裸地址。
fn fmt_addr(addr: u64, self_base: u64, self_size: u64) -> String {
    if addr >= self_base && (self_size == 0 || addr < self_base + self_size) {
        return format!("0x{:X}(rva=0x{:X})", addr, addr - self_base);
    }
    format!("0x{addr:X}")
}

unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rec = (*info).ExceptionRecord;
    if rec.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rec = &*rec;
    let code = rec.ExceptionCode.0 as u32;
    if !is_fatal(code) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if CRASHES.fetch_add(1, Ordering::SeqCst) >= 8 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let (base, size) = self_module();
    let addr = rec.ExceptionAddress as u64;
    let mut msg = String::new();
    msg.push_str(&format!(
        "💥 致命异常 code=0x{code:X} 指令={} ",
        fmt_addr(addr, base, size)
    ));
    if rec.NumberParameters >= 2 {
        let rw = if rec.ExceptionInformation[0] == 0 { "读" } else { "写" };
        msg.push_str(&format!(
            "访问={rw} 目标={}",
            fmt_addr(rec.ExceptionInformation[1] as u64, base, size)
        ));
    }
    // 调用链：x64 用 RtlVirtualUnwind 逐帧展开（依赖 unwind 信息，release 也有）
    let ctx_ptr = (*info).ContextRecord;
    if !ctx_ptr.is_null() {
        #[cfg(target_arch = "x86_64")]
        {
            let mut ctx: CONTEXT = *ctx_ptr;
            let mut history = [0u8; 512];
            let mut pc = ctx.Rip;
            for frame in 0..24 {
                if pc == 0 {
                    break;
                }
                msg.push_str(&format!("\n    #{frame:02} {}", fmt_addr(pc, base, size)));
                let mut image_base = 0u64;
                let entry = RtlLookupFunctionEntry(
                    pc,
                    &mut image_base,
                    history.as_mut_ptr() as *mut c_void,
                );
                if entry.is_null() {
                    break;
                }
                let mut handler_data: *mut c_void = std::ptr::null_mut();
                let mut establisher = 0u64;
                RtlVirtualUnwind(
                    0,
                    image_base,
                    pc,
                    entry,
                    &mut ctx,
                    &mut handler_data,
                    &mut establisher,
                    std::ptr::null_mut(),
                );
                pc = ctx.Rip;
            }
        }
    }
    crate::log(msg);
    EXCEPTION_CONTINUE_SEARCH
}
