#![allow(unused_must_use)]

//! 搜狗式状态栏（抗锯齿预渲染 + 逐像素合成 + UpdateLayeredWindow）。
//!
//! 界面资源由 tools/gen-statusbar-assets.ps1 预渲染为 BGRA 二进制（圆角、
//! 渐变、阴影、悬停态全部抗锯齿）；运行时不画任何 GDI 图形，只把背景帧与
//! 动态字形（中/英、全/半、标点）做 alpha 合成，再整帧交给
//! UpdateLayeredWindow（per-pixel alpha）。悬停只是换一帧，不会变形。
//!
//! 状态栏运行在独立 UI 线程（自带 GetMessage 消息循环），与宿主进程的
//! TSF 输入线程完全隔离，鼠标悬停/拖动不会受按键处理挤压而"转圈"。
//! TSF 线程通过 PostMessage 下发显示/隐藏/刷新命令。
//!
//! 行为：默认右下角紧贴任务栏上方与右缘；可拖动（会话内记忆，重启回默认）；
//! 点击按钮切换模式；右键或点 Logo 打开设置中心；失焦/切换输入法自动隐藏。

use crate::engine;
use crate::text_service::SharedState;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, GetDC,
    GetDeviceCaps, GetMonitorInfoW, HBRUSH, MonitorFromPoint, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ, LOGPIXELSX, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{w, PCWSTR};

const WND_CLASS: &str = "Lingxi.StatusBar";
const WIDTH: i32 = 170;
const HEIGHT: i32 = 34;
// 资源按 2 倍分辨率预渲染（逻辑 194x34 @96dpi → 388x68）
const ASSET_W: usize = 340;
const ASSET_H: usize = 68;

// 交互区域：0=背景，1=模式，2=全半角，3=标点，4=菜单，5=Logo，6=中段图标区
const BTN_MODE: i32 = 1;
const BTN_WIDTH: i32 = 2;
const BTN_MENU: i32 = 3;
const BTN_LOGO: i32 = 4;

// 自定义窗口消息（TSF 线程 → UI 线程）
const MSG_SHOW: u32 = WM_APP + 1; // WPARAM 位：1=ascii 2=full 4=punct
const MSG_HIDE: u32 = WM_APP + 2;

fn btn_rect(btn: i32) -> (i32, i32, i32, i32) {
    match btn {
        BTN_MODE => (48, 3, 84, HEIGHT - 3),
        BTN_WIDTH => (88, 3, 122, HEIGHT - 3),
        BTN_MENU => (134, 3, 156, HEIGHT - 3),
        _ => (0, 0, 0, 0),
    }
}

fn hit_test(x: i32, y: i32, s: f32) -> i32 {
    let l = |v: i32| ((v as f32) * s).round() as i32;
    if x >= l(14) && x <= l(36) && y >= l(5) && y <= l(29) {
        return BTN_LOGO;
    }
    for b in [BTN_MODE, BTN_WIDTH, BTN_MENU] {
        let (l0, t0, r0, b0) = btn_rect(b);
        if x >= l(l0) && x <= l(r0) && y >= l(t0) && y <= l(b0) {
            return b;
        }
    }
    0
}

/// 双线性缩放 BGRA 帧（2x 资源 → 目标窗口像素尺寸）。
fn scale_frame(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if dw == 0 || dh == 0 {
        return out;
    }
    for y in 0..dh {
        let sy = (y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5;
        let y0f = sy.max(0.0).floor();
        let y0 = (y0f as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let fy = (sy - y0f).clamp(0.0, 1.0);
        for x in 0..dw {
            let sx = (x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5;
            let x0f = sx.max(0.0).floor();
            let x0 = (x0f as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let fx = (sx - x0f).clamp(0.0, 1.0);
            let o = (y * dw + x) * 4;
            for c in 0..4 {
                let p00 = src[(y0 * sw + x0) * 4 + c] as f32;
                let p10 = src[(y0 * sw + x1) * 4 + c] as f32;
                let p01 = src[(y1 * sw + x0) * 4 + c] as f32;
                let p11 = src[(y1 * sw + x1) * 4 + c] as f32;
                let v = p00 * (1.0 - fx) * (1.0 - fy)
                    + p10 * fx * (1.0 - fy)
                    + p01 * (1.0 - fx) * fy
                    + p11 * fx * fy;
                out[o + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn mem_pos_get() -> Option<(i32, i32)> {
    static POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    *POS.get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn mem_pos_set(p: (i32, i32)) {
    static POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    *POS.get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(p);
}

// ---------------- 预渲染资源 ----------------

struct Assets {
    bg_normal: &'static [u8],
    bg_hover_mode: &'static [u8],
    bg_hover_width: &'static [u8],
    bg_hover_menu: &'static [u8],
    bg_hover_logo: &'static [u8],
    glyph_zhong: &'static [u8],
    glyph_ying: &'static [u8],
    glyph_quan: &'static [u8],
    glyph_ban: &'static [u8],
}

fn assets() -> &'static Assets {
    static A: OnceLock<Assets> = OnceLock::new();
    A.get_or_init(|| Assets {
        bg_normal: include_bytes!("assets/bg_normal.bin"),
        bg_hover_mode: include_bytes!("assets/bg_hover_mode.bin"),
        bg_hover_width: include_bytes!("assets/bg_hover_width.bin"),
        bg_hover_menu: include_bytes!("assets/bg_hover_menu.bin"),
        bg_hover_logo: include_bytes!("assets/bg_hover_logo.bin"),
        glyph_zhong: include_bytes!("assets/glyph_zhong.bin"),
        glyph_ying: include_bytes!("assets/glyph_ying.bin"),
        glyph_quan: include_bytes!("assets/glyph_quan.bin"),
        glyph_ban: include_bytes!("assets/glyph_ban.bin"),
    })
}

/// 解码 BGRA 帧（文件头：u32 w + u32 h + BGRA 像素，自顶向下）。
fn decode(data: &[u8]) -> Vec<u8> {
    let w = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let _ = (w, h);
    data[8..].to_vec()
}

/// alpha 合成：把字形帧叠到背景帧上（全帧尺寸一致）。
fn blend(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let sa = s[3] as u32;
        if sa == 0 {
            continue;
        }
        if sa == 255 {
            d.copy_from_slice(s);
            continue;
        }
        let da = d[3] as u32;
        let oa = sa + da * (255 - sa) / 255;
        if oa > 0 {
            let ia = 255 - sa;
            d[0] = ((s[0] as u32 * sa + d[0] as u32 * da * ia / 255) / oa) as u8;
            d[1] = ((s[1] as u32 * sa + d[1] as u32 * da * ia / 255) / oa) as u8;
            d[2] = ((s[2] as u32 * sa + d[2] as u32 * da * ia / 255) / oa) as u8;
            d[3] = oa as u8;
        }
    }
}

/// 按当前状态合成一帧（背景 + 三个动态字形）。
fn composite_frame(hover: i32, ascii: bool, full: bool) -> Vec<u8> {
    let a = assets();
    let bg = match hover {
        BTN_MODE => a.bg_hover_mode,
        BTN_WIDTH => a.bg_hover_width,
        BTN_MENU => a.bg_hover_menu,
        BTN_LOGO => a.bg_hover_logo,
        _ => a.bg_normal,
    };
    let mut f = decode(bg);
    blend(&mut f, decode(if ascii { a.glyph_ying } else { a.glyph_zhong }).as_slice());
    blend(&mut f, decode(if full { a.glyph_quan } else { a.glyph_ban }).as_slice());
    f
}

fn ensure_window_class() -> bool {
    static ATOM: OnceLock<u16> = OnceLock::new();
    if ATOM.get().is_some() {
        return true;
    }
    unsafe {
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = WND_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: HINSTANCE(hmod.0),
            hIcon: HICON(std::ptr::null_mut()),
            hCursor: HCURSOR(std::ptr::null_mut()),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszMenuName: PCWSTR(std::ptr::null()),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hIconSm: HICON(std::ptr::null_mut()),
        };
        let atom = RegisterClassExW(&wc);
        let _ = ATOM.set(atom);
        atom != 0
    }
}

struct StatusBar {
    hwnd: HWND,
    shown: std::cell::Cell<bool>,
    ascii: std::cell::Cell<bool>,
    full: std::cell::Cell<bool>,
    punct: std::cell::Cell<bool>,
    hover: std::cell::Cell<i32>,
    dragging: std::cell::Cell<bool>,
    drag_off: std::cell::Cell<(i32, i32)>,
    dragged: std::cell::Cell<bool>,
}

unsafe impl Send for StatusBar {}
unsafe impl Sync for StatusBar {}

impl StatusBar {
    fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            shown: std::cell::Cell::new(false),
            ascii: std::cell::Cell::new(false),
            full: std::cell::Cell::new(false),
            punct: std::cell::Cell::new(false),
            hover: std::cell::Cell::new(0),
            dragging: std::cell::Cell::new(false),
            drag_off: std::cell::Cell::new((0, 0)),
            dragged: std::cell::Cell::new(false),
        }
    }

    fn update(&self, ascii: bool, full: bool, punct: bool) {
        if self.ascii.get() != ascii || self.full.get() != full || self.punct.get() != punct {
            self.ascii.set(ascii);
            self.full.set(full);
            self.punct.set(punct);
            if self.shown.get() {
                self.push();
            }
        }
    }

    fn show(&self) {
        if self.hwnd.0.is_null() || self.shown.get() {
            return;
        }
        let (x, y) = self.pos();
        self.push_at(x, y);
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        self.shown.set(true);
    }

    fn hide(&self) {
        if self.hwnd.0.is_null() {
            return;
        }
        if self.shown.replace(false) {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    fn pos(&self) -> (i32, i32) {
        if let Some(p) = mem_pos_get() {
            return p;
        }
        // 默认：右下角，紧贴任务栏上方与右侧屏幕边缘
        let r = work_area();
        let (ww, wh) = self.win_size();
        let p = (r.right - ww - 2, r.bottom - wh - 2);
        mem_pos_set(p);
        p
    }

    fn dpi_scale(&self) -> f32 {
        unsafe {
            // 取窗口所在显示器的 DPI（多显示器不同缩放下也能自适应）
            let dc = GetDC(Some(self.hwnd));
            if dc.0.is_null() {
                return 1.0;
            }
            let dpi = GetDeviceCaps(Some(dc), LOGPIXELSX);
            ReleaseDC(Some(self.hwnd), dc);
            if dpi < 96 {
                1.0
            } else {
                dpi as f32 / 96.0
            }
        }
    }

    fn win_size(&self) -> (i32, i32) {
        let s = self.dpi_scale();
        (
            ((WIDTH as f32) * s).round() as i32,
            ((HEIGHT as f32) * s).round() as i32,
        )
    }

    /// 在当前位置重推一帧（状态/悬停变化时）。
    fn push(&self) {
        let (x, y) = self.pos();
        self.push_at(x, y);
    }

    /// 合成当前帧并 UpdateLayeredWindow 到 (x, y)。
    fn push_at(&self, x: i32, y: i32) {
        let frame2x =
        composite_frame(self.hover.get(), self.ascii.get(), self.full.get());
        let (ww, wh) = self.win_size();
        let frame = if ww as usize == ASSET_W && wh as usize == ASSET_H {
            frame2x
        } else {
            scale_frame(&frame2x, ASSET_W, ASSET_H, ww as usize, wh as usize)
        };
        unsafe {
            let screen = GetDC(None);
            if screen.0.is_null() {
                return;
            }
            let mem = CreateCompatibleDC(Some(screen));
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = ww;
            bi.bmiHeader.biHeight = -wh; // 自顶向下
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = match CreateDIBSection(Some(mem), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(d) => d,
                Err(_) => {
                    DeleteDC(mem);
                    ReleaseDC(None, screen);
                    return;
                }
            };
            if dib.0.is_null() || bits.is_null() {
                DeleteDC(mem);
                ReleaseDC(None, screen);
                return;
            }
            std::ptr::copy_nonoverlapping(frame.as_ptr() as *const u8, bits as *mut u8, frame.len());
            let old = SelectObject(mem, HGDIOBJ(dib.0));
            let pt_dst = POINT { x, y };
            let pt_src = POINT { x: 0, y: 0 };
            let size = SIZE { cx: ww, cy: wh };
            let bf = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                Some(screen),
                Some(&pt_dst),
                Some(&size),
                Some(mem),
                Some(&pt_src),
                COLORREF(0),
                Some(&bf),
                ULW_ALPHA,
            );
            SelectObject(mem, old);
            DeleteObject(HGDIOBJ(dib.0));
            DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }

    fn on_click(&self, btn: i32) {
        let Some(eng) = engine::engine() else { return };
        let Ok(mut e) = eng.lock() else { return };
        match btn {
            BTN_MODE => {
                let mode = e.get_option("ascii_mode");
                e.set_option("ascii_mode", !mode);
                if let Some(sh) = latest_shared() {
                    if let Ok(mut st) = sh.lock() {
                    st.ascii_mode = !mode;
                    }
                }
                self.update(!mode, self.full.get(), self.punct.get());
            }
            BTN_WIDTH => {
                let mut cfg = lingxi_rime::config::load_config();
                cfg.input.full_shape = !cfg.input.full_shape;
                let _ = lingxi_rime::config::save_config(&cfg);
                e.refresh_options(vec![("full_shape".into(), cfg.input.full_shape)]);
                if let Some(sh) = latest_shared() {
                    if let Ok(mut st) = sh.lock() {
                        st.full_shape = cfg.input.full_shape;
                    }
                }
                self.update(self.ascii.get(), cfg.input.full_shape, self.punct.get());
            }
            _ => {
                // Logo/菜单 → 打开设置中心（按 DLL 所在位置定位，避免写死路径）
                if let Some(exe) = settings_exe_path() {
                    let _ = std::process::Command::new(exe)
                        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：避免控制台黑框
                        .spawn();
                }
            }
        }
    }
}

/// 按本 DLL 所在目录向上查找工作区里的设置中心可执行文件，
/// 使状态栏设置入口在不同机器/目录下都能工作。
fn settings_exe_path() -> Option<std::path::PathBuf> {
    unsafe {
        let mut hmod: HMODULE = core::mem::zeroed();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let addr = (settings_exe_path as fn() -> Option<std::path::PathBuf>) as usize;
        GetModuleHandleExW(flags, PCWSTR(addr as *const u16), &mut hmod).ok()?;
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmod), &mut buf);
        if len == 0 {
            return None;
        }
        let dll = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        let mut dir = dll.parent()?.to_path_buf();
        for _ in 0..6 {
            let ime = dir
                .join("lingxi-ime")
                .join("src-tauri")
                .join("target")
                .join("debug")
                .join("lingxi-ime.exe");
            if ime.exists() {
                return Some(ime);
            }
            dir = dir.parent()?.to_path_buf();
        }
        None
    }
}

fn create_hwnd() -> HWND {
    unsafe {
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = WND_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        match CreateWindowExW(
            WINDOW_EX_STYLE(
                WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0,
            ),
            PCWSTR(class_name.as_ptr()),
            w!("LingxiStatus"),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(HINSTANCE(hmod.0)),
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(_) => HWND(std::ptr::null_mut()),
        }
    }
}

fn work_area() -> RECT {
    unsafe {
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTONEAREST);
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let userdata = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if userdata == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let this = &*(userdata as *const StatusBar);
        match msg {
            MSG_SHOW => {
                let flags = wparam.0 as usize;
                this.update(
                    flags & 1 != 0,
                    flags & 2 != 0,
                    flags & 4 != 0,
                );
                this.show();
                LRESULT(0)
            }
            MSG_HIDE => {
                this.hide();
                LRESULT(0)
            }
            WM_PAINT => {
                // 内容由 UpdateLayeredWindow 提供，仅验证区域避免重绘循环
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                // 强制箭头光标，避免沿用宿主进程残留的等待光标（悬停转圈）
                if let Ok(c) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(c));
                }
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xFFFF) as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
                this.dragging.set(true);
                this.dragged.set(false);
                this.drag_off.set((x, y));
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if this.dragging.get() {
                    this.dragged.set(true);
                    let sx = (lparam.0 & 0xFFFF) as i32;
                    let sy = ((lparam.0 >> 16) & 0xFFFF) as i32;
                    let (off_x, off_y) = this.drag_off.get();
                    let (mut x, mut y) = mem_pos_get().unwrap_or((0, 0));
                    x += sx - off_x;
                    y += sy - off_y;
                    let wa = work_area();
                    let (ww, wh) = this.win_size();
                    x = x.clamp(wa.left - ww + 24, wa.right - 24);
                    y = y.clamp(wa.top - 10, wa.bottom - wh + 10);
                    mem_pos_set((x, y));
                    // 内容由 ULW 提供，纯移动不重绘，避免闪烁
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        ww,
                        wh,
                        SWP_NOACTIVATE | SWP_NOREDRAW,
                    );
                } else {
                    let s = this.dpi_scale();
                    let h = hit_test(
                        (lparam.0 & 0xFFFF) as i32,
                        ((lparam.0 >> 16) & 0xFFFF) as i32,
                        s,
                    );
                    if h != this.hover.get() {
                        this.hover.set(h);
                        this.push();
                    }
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if this.dragging.replace(false) {
                    ReleaseCapture();
                    if !this.dragged.get() {
                        let x = (lparam.0 & 0xFFFF) as i32;
                        let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
                        this.on_click(hit_test(x, y, this.dpi_scale()));
                    } else {
                        // 拖动结束：按新位置所在显示器的 DPI 重新渲染
                        this.push();
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                this.on_click(BTN_MENU);
                LRESULT(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                if this.hover.replace(0) != 0 {
                    this.push();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn set_window_data(hwnd: HWND, ptr: usize) {
    #[cfg(target_pointer_width = "64")]
    {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
    }
    #[cfg(target_pointer_width = "32")]
    {
        SetWindowLongW(hwnd, GWLP_USERDATA, ptr as i32);
    }
}

// ---------------- 独立 UI 线程 ----------------

static BAR_HWND: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
static BAR_OWNER: OnceLock<Mutex<Option<Box<StatusBar>>>> = OnceLock::new();
static THREAD_STARTED: OnceLock<()> = OnceLock::new();
static LATEST_SHARED: OnceLock<Mutex<Option<Arc<Mutex<SharedState>>>>> = OnceLock::new();

fn latest_shared() -> Option<Arc<Mutex<SharedState>>> {
    LATEST_SHARED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn bar_hwnd() -> Option<HWND> {
    THREAD_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("lingxi-statusbar".into())
            .spawn(ui_thread_entry);
    });
    let g = BAR_HWND.get_or_init(|| Mutex::new(None));
    let mut guard = match g.lock() {
        Ok(x) => x,
        Err(e) => e.into_inner(),
    };
    if guard.is_none() {
        for _ in 0..200 {
            drop(guard);
            std::thread::sleep(std::time::Duration::from_millis(5));
            guard = match g.lock() {
                Ok(x) => x,
                Err(e) => e.into_inner(),
            };
            if guard.is_some() {
                break;
            }
        }
    }
    (*guard).map(|h| HWND(h as *mut core::ffi::c_void))
}

fn ui_thread_entry() {
    unsafe {
        if !ensure_window_class() {
            return;
        }
        let hwnd = create_hwnd();
        if hwnd.0.is_null() {
            return;
        }
        // Box 在静态中持有，地址稳定；userdata 指向它供 wnd_proc 使用
        let owner = BAR_OWNER.get_or_init(|| Mutex::new(None));
        *owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(StatusBar::new(hwnd)));
        let bar_ptr = owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|b| &**b as *const StatusBar as usize)
            .unwrap_or(0);
        set_window_data(hwnd, bar_ptr);
        *BAR_HWND
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hwnd.0 as usize);
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // 消息循环结束：释放 StatusBar（窗口由 WM_DESTROY 后销毁）
        *owner.lock().unwrap_or_else(|e| e.into_inner()) = None;
        DestroyWindow(hwnd);
    }
}

/// 显示/更新状态栏（TSF 线程调用，投递给 UI 线程）。
pub fn show_status_bar(shared: Arc<Mutex<SharedState>>, ascii: bool, full: bool, punct: bool) {
    *LATEST_SHARED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(shared);
    if let Some(hwnd) = bar_hwnd() {
        let flags = (ascii as usize) | ((full as usize) << 1) | ((punct as usize) << 2);
        unsafe {
            PostMessageW(Some(hwnd), MSG_SHOW, WPARAM(flags as usize), LPARAM(0));
        }
    }
}

/// 隐藏状态栏。
pub fn hide_status_bar() {
    if let Some(hwnd) = bar_hwnd() {
        unsafe {
            PostMessageW(Some(hwnd), MSG_HIDE, WPARAM(0), LPARAM(0));
        }
    }
}

/// 刷新模式（模式变化时调用）。
pub fn refresh_status_bar(shared: &Arc<Mutex<SharedState>>) {
    let (ascii, full, punct) = match shared.lock() {
        Ok(st) => (st.ascii_mode, st.full_shape, st.ascii_punct),
        Err(_) => return,
    };
    show_status_bar(shared.clone(), ascii, full, punct);
}
