#![allow(unused_must_use)]

//! 状态栏（参照 WindInput 工具栏规格实现）。
//!
//! 形制：横向圆角小条，条厚 30dp，左侧 12dp 拖动柄（2×3 点阵），
//! 之后每格 30dp：中/英（文字，中文模式蓝底白字）、标点（矢量图标，全/半角换图）、
//! 全半角（满月=全角 / 弯月=半角）、符号（※ 图标，面板打开时蓝底）、
//! 设置（齿轮，颜色更淡）。
//!
//! 渲染：资源按 2 倍分辨率预渲染（白底 245 alpha、圆角=厚度×0.3、1dp 同色边框、
//! 格内高亮胶囊内缩 4dp）；图标与文字为**单色 alpha 蒙版**，运行时按状态 tint，
//! 再整帧交给 UpdateLayeredWindow（per-pixel alpha），并按显示器 DPI 缩放。
//!
//! 行为：默认右下角贴工作区边缘；可拖动（会话内记忆，重启回默认）；
//! 左键点格切换状态，右键任意位置打开设置中心；失焦/切换输入法自动隐藏。

use crate::engine;
use crate::text_service::SharedState;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, GetDC,
    GetDeviceCaps, GetMonitorInfoW, HBRUSH, MonitorFromPoint, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ, LOGPIXELSX, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{w, PCWSTR};

const WND_CLASS: &str = "Lingxi.StatusBar";

// ── 几何（逻辑 dp；资源为 2 倍）─────────────────────────────────────────────
const GRIP_W: i32 = 12; // 拖动柄宽度
const CELL_W: i32 = 30; // 每格宽度
const HEIGHT: i32 = 30; // 条厚
const WIDTH: i32 = GRIP_W + CELL_W * 5; // 162：柄 + 5 格
const ASSET_W: usize = (WIDTH as usize) * 2;
const ASSET_H: usize = (HEIGHT as usize) * 2;
const TILE: usize = (CELL_W as usize) * 2; // 单格 tile 尺寸（2 倍）

// 格序号
const CELL_MODE: i32 = 0;
const CELL_PUNCT: i32 = 1;
const CELL_WIDTH: i32 = 2;
const CELL_SYMBOL: i32 = 3;
const CELL_SETTINGS: i32 = 4;
const CELL_COUNT: i32 = 5;

// ── 配色（对齐 WindInput 默认主题）──────────────────────────────────────────
const FG: [u8; 3] = [72, 72, 78]; // 常态内容
const HL_FG: [u8; 3] = [255, 255, 255]; // 激活格内容
const SETTINGS_FG: [u8; 3] = [140, 140, 148]; // 设置齿轮更淡

// 自定义窗口消息（TSF 线程 → UI 线程）
const MSG_SHOW: u32 = WM_APP + 1; // WPARAM 位：1=ascii 2=full 4=punct
const MSG_HIDE: u32 = WM_APP + 2;
const MSG_REPAINT: u32 = WM_APP + 3;

// 右键菜单命令 id
const CMD_MODE: usize = 1001;
const CMD_WIDTH: usize = 1002;
const CMD_PUNCT: usize = 1003;
const CMD_SETTINGS: usize = 1005;
const CMD_SYM: usize = 1007;

fn cell_x(i: i32) -> i32 {
    GRIP_W + i * CELL_W
}

/// 命中测试（逻辑坐标）：返回格序号，拖动柄或条外返回 -1。
fn hit_cell(x: i32, y: i32) -> i32 {
    if y < 0 || y >= HEIGHT || x < GRIP_W || x >= WIDTH {
        return -1;
    }
    let i = (x - GRIP_W) / CELL_W;
    if i < 0 || i >= CELL_COUNT {
        -1
    } else {
        i
    }
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
    bar_bg: &'static [u8],
    cell_hover: &'static [u8],
    cell_active: &'static [u8],
    icon_width_full: &'static [u8],
    icon_width_half: &'static [u8],
    icon_punct_full: &'static [u8],
    icon_punct_half: &'static [u8],
    icon_gear: &'static [u8],
    glyph_zhong: &'static [u8],
    glyph_ying: &'static [u8],
    glyph_symbol: &'static [u8],
}

fn assets() -> &'static Assets {
    static A: OnceLock<Assets> = OnceLock::new();
    A.get_or_init(|| Assets {
        bar_bg: include_bytes!("assets/bar_bg.bin"),
        cell_hover: include_bytes!("assets/cell_hover.bin"),
        cell_active: include_bytes!("assets/cell_active.bin"),
        icon_width_full: include_bytes!("assets/icon_width_full.bin"),
        icon_width_half: include_bytes!("assets/icon_width_half.bin"),
        icon_punct_full: include_bytes!("assets/icon_punct_full.bin"),
        icon_punct_half: include_bytes!("assets/icon_punct_half.bin"),
        icon_gear: include_bytes!("assets/icon_gear.bin"),
        glyph_zhong: include_bytes!("assets/glyph_zhong.bin"),
        glyph_ying: include_bytes!("assets/glyph_ying.bin"),
        glyph_symbol: include_bytes!("assets/glyph_symbol.bin"),
    })
}

/// 解码 BGRA 资源（文件头：u32 w + u32 h + BGRA 像素，自顶向下）。
fn decode(data: &[u8]) -> Vec<u8> {
    data[8..].to_vec()
}

/// 把 tile（TILE×TILE BGRA）以 source-over 混合到整帧 dst（dst_w 宽）的 (ox, oy)。
fn blit_over(dst: &mut [u8], dst_w: usize, tile: &[u8], ox: usize, oy: usize) {
    for ty in 0..TILE {
        let dy = oy + ty;
        if dy >= ASSET_H {
            break;
        }
        for tx in 0..TILE {
            let dx = ox + tx;
            if dx >= dst_w {
                break;
            }
            let si = (ty * TILE + tx) * 4;
            let di = (dy * dst_w + dx) * 4;
            let sa = tile[si + 3] as u32;
            if sa == 0 {
                continue;
            }
            if sa == 255 {
                dst[di] = tile[si];
                dst[di + 1] = tile[si + 1];
                dst[di + 2] = tile[si + 2];
                dst[di + 3] = 255;
                continue;
            }
            let da = dst[di + 3] as u32;
            let oa = sa + da * (255 - sa) / 255;
            if oa > 0 {
                let ia = 255 - sa;
                dst[di] = ((tile[si] as u32 * sa + dst[di] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 1] =
                    ((tile[si + 1] as u32 * sa + dst[di + 1] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 2] =
                    ((tile[si + 2] as u32 * sa + dst[di + 2] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 3] = oa as u8;
            }
        }
    }
}

/// 把**单色蒙版** tile 按 tint 上色后 source-over 混合到整帧。
/// 蒙版只用其 alpha（预渲染为白色），颜色完全由 tint 决定——与 WindInput 的
/// "SVG 仅作 alpha 蒙版、由工具栏按主题 tint" 同一模型。
fn blit_tinted(dst: &mut [u8], dst_w: usize, mask: &[u8], ox: usize, oy: usize, tint: [u8; 3]) {
    for ty in 0..TILE {
        let dy = oy + ty;
        if dy >= ASSET_H {
            break;
        }
        for tx in 0..TILE {
            let dx = ox + tx;
            if dx >= dst_w {
                break;
            }
            let sa = mask[(ty * TILE + tx) * 4 + 3] as u32;
            if sa == 0 {
                continue;
            }
            let di = (dy * dst_w + dx) * 4;
            if sa == 255 {
                dst[di] = tint[0];
                dst[di + 1] = tint[1];
                dst[di + 2] = tint[2];
                continue; // 底色不透明，保持 255
            }
            let da = dst[di + 3] as u32;
            let oa = sa + da * (255 - sa) / 255;
            if oa > 0 {
                let ia = 255 - sa;
                dst[di] = ((tint[0] as u32 * sa + dst[di] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 1] =
                    ((tint[1] as u32 * sa + dst[di + 1] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 2] =
                    ((tint[2] as u32 * sa + dst[di + 2] as u32 * da * ia / 255) / oa) as u8;
                dst[di + 3] = oa as u8;
            }
        }
    }
}

/// 合成一帧：整条背景 + 各格高亮/悬停 + 各格内容（图标/文字按状态 tint）。
fn composite_frame(hover: i32, ascii: bool, full: bool, punct: bool, symbol_on: bool) -> Vec<u8> {
    let a = assets();
    let mut f = decode(a.bar_bg);
    for i in 0..CELL_COUNT {
        let ox = (cell_x(i) as usize) * 2;
        // 激活格：中文模式、符号面板打开，都是蓝底
        let active = (i == CELL_MODE && !ascii) || (i == CELL_SYMBOL && symbol_on);
        if active {
            blit_over(&mut f, ASSET_W, &decode(a.cell_active), ox, 0);
        } else if hover == i {
            blit_over(&mut f, ASSET_W, &decode(a.cell_hover), ox, 0);
        }
        let (mask, tint) = match i {
            CELL_MODE => (
                if ascii { a.glyph_ying } else { a.glyph_zhong },
                if active { HL_FG } else { FG },
            ),
            CELL_PUNCT => (
                if punct { a.icon_punct_half } else { a.icon_punct_full },
                FG,
            ),
            CELL_WIDTH => (
                if full { a.icon_width_full } else { a.icon_width_half },
                FG,
            ),
            CELL_SYMBOL => (a.glyph_symbol, if active { HL_FG } else { FG }),
            _ => (a.icon_gear, SETTINGS_FG),
        };
        blit_tinted(&mut f, ASSET_W, mask, ox, 0, tint);
    }
    f
}

// ---------------- 窗口 ----------------

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

// StatusBar 只在其专属 UI 线程上访问；静态 Mutex 仅用于持有。
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
            hover: std::cell::Cell::new(-1),
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
        // 默认：工作区右下角，紧贴任务栏上方与右侧边缘
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
        let frame2x = composite_frame(
            self.hover.get(),
            self.ascii.get(),
            self.full.get(),
            self.punct.get(),
            crate::symbol_panel::visible(),
        );
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

    /// 左键点格：按格序号切换状态 / 打开设置。
    fn on_click(&self, cell: i32) {
        if cell == CELL_SETTINGS {
            self.open_settings();
            return;
        }
        if cell == CELL_SYMBOL {
            // 符号面板：同一格再点一次收起
            crate::symbol_panel::toggle();
            return;
        }
        let Some(eng) = engine::engine() else { return };
        let Ok(mut e) = eng.lock() else { return };
        match cell {
            CELL_MODE => {
                let mode = e.get_option("ascii_mode");
                e.set_option("ascii_mode", !mode);
                if let Some(sh) = latest_shared() {
                    if let Ok(mut st) = sh.lock() {
                        st.ascii_mode = !mode;
                    }
                }
                self.update(!mode, self.full.get(), self.punct.get());
            }
            CELL_WIDTH => {
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
            CELL_PUNCT => {
                let mut cfg = lingxi_rime::config::load_config();
                cfg.input.ascii_punct = !cfg.input.ascii_punct;
                let _ = lingxi_rime::config::save_config(&cfg);
                e.refresh_options(vec![("ascii_punct".into(), cfg.input.ascii_punct)]);
                if let Some(sh) = latest_shared() {
                    if let Ok(mut st) = sh.lock() {
                        st.ascii_punct = cfg.input.ascii_punct;
                    }
                }
                self.update(self.ascii.get(), self.full.get(), cfg.input.ascii_punct);
            }
            _ => {}
        }
    }

    fn open_settings(&self) {
        if let Some(exe) = settings_exe_path() {
            let _ = std::process::Command::new(exe)
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：避免控制台黑框
                .spawn();
        }
    }

    /// 右键菜单：四个开关 + 打开设置（对齐 WindInput「右键弹主菜单」）。
    fn show_menu(&self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            AppendMenuW(
                menu,
                MF_STRING | check(!self.ascii.get()),
                CMD_MODE,
                w!("中文模式"),
            )
            .ok();
            AppendMenuW(menu, MF_STRING | check(self.full.get()), CMD_WIDTH, w!("全角")).ok();
            AppendMenuW(
                menu,
                MF_STRING | check(!self.punct.get()),
                CMD_PUNCT,
                w!("中文标点"),
            )
            .ok();
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            AppendMenuW(menu, MF_STRING, CMD_SYM, w!("符号面板")).ok();
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("打开设置中心…")).ok();

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            SetForegroundWindow(self.hwnd);
            let ret = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN,
                pt.x,
                pt.y,
                Some(0),
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            match ret.0 as usize {
                CMD_MODE => self.on_click(CELL_MODE),
                CMD_WIDTH => self.on_click(CELL_WIDTH),
                CMD_PUNCT => self.on_click(CELL_PUNCT),
                CMD_SYM => crate::symbol_panel::toggle(),
                CMD_SETTINGS => self.open_settings(),
                _ => {}
            }
        }
    }
}

/// 定位设置中心可执行文件，兼容两种布局：
/// - 安装布局：<安装目录>\lingxi-ime.exe（TSF DLL 在 tsf\ 或 tsf-x86\ 子目录）；
/// - 开发布局：lingxi-ime\src-tauri\target\debug\lingxi-ime.exe。
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
        let dll_dir = dll.parent()?.to_path_buf();
        // 安装布局：逐级向上找 lingxi-ime.exe
        let mut dir = dll_dir.clone();
        for _ in 0..6 {
            let exe = dir.join("lingxi-ime.exe");
            if exe.exists() {
                return Some(exe);
            }
            dir = dir.parent()?.to_path_buf();
        }
        // 开发布局：工作区根下 lingxi-ime\src-tauri\target\debug\
        let mut dir = dll_dir;
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

/// 双线性缩放 BGRA 帧（2 倍资源 → 目标窗口像素尺寸）。
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
                this.update(flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
                this.show();
                LRESULT(0)
            }
            MSG_HIDE => {
                this.hide();
                LRESULT(0)
            }
            MSG_REPAINT => {
                // 外部状态变化（如软键盘面板开关）→ 重画一格高亮
                this.push();
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
                    let sx = (lparam.0 & 0xFFFF) as i32;
                    let sy = ((lparam.0 >> 16) & 0xFFFF) as i32;
                    let (off_x, off_y) = this.drag_off.get();
                    if !this.dragged.get() && (sx - off_x).abs() + (sy - off_y).abs() > 3 {
                        this.dragged.set(true);
                        if this.hover.replace(-1) != -1 {
                            this.push();
                        }
                    }
                    if this.dragged.get() {
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
                    }
                } else {
                    let s = this.dpi_scale();
                    let lx = ((lparam.0 & 0xFFFF) as f32 / s) as i32;
                    let ly = (((lparam.0 >> 16) & 0xFFFF) as f32 / s) as i32;
                    let h = hit_cell(lx, ly);
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
                    if this.dragged.get() {
                        // 拖动结束：按新位置所在显示器的 DPI 重新渲染
                        this.push();
                    } else {
                        let s = this.dpi_scale();
                        let lx = ((lparam.0 & 0xFFFF) as f32 / s) as i32;
                        let ly = (((lparam.0 >> 16) & 0xFFFF) as f32 / s) as i32;
                        let cell = hit_cell(lx, ly);
                        if cell >= 0 {
                            this.on_click(cell);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // 右键任意位置 → 弹出主菜单（对齐 WindInput）
                this.show_menu();
                LRESULT(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                if this.hover.replace(-1) != -1 {
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
            PostMessageW(Some(hwnd), MSG_SHOW, WPARAM(flags), LPARAM(0));
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

/// 请求状态栏重画（软键盘等外部状态变化时调用，不创建窗口）。
pub fn notify_repaint() {
    let Some(g) = BAR_HWND.get() else {
        return;
    };
    let guard = match g.lock() {
        Ok(x) => x,
        Err(e) => e.into_inner(),
    };
    if let Some(h) = *guard {
        unsafe {
            PostMessageW(
                Some(HWND(h as *mut core::ffi::c_void)),
                MSG_REPAINT,
                WPARAM(0),
                LPARAM(0),
            );
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
