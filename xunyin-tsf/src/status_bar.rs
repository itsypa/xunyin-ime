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
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{w, PCWSTR};

const WND_CLASS: &str = "Xunyin.StatusBar";

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

// 前台性轮询：TSF 对**后台线程**的失焦通知并不可靠（实测：某个进程拿到过一次
// foreground=true 之后再没收到 false/Deactivate，状态栏就永久留在屏幕上）。
// 因此状态栏自己按定时器核对「本进程是否仍是前台进程」，不是就隐藏。
const POLL_TIMER_ID: usize = 1;
const POLL_MS: u32 = 400;

// 右键菜单命令 id
const CMD_MODE: usize = 1001;
const CMD_WIDTH: usize = 1002;
const CMD_PUNCT: usize = 1003;
const CMD_SETTINGS: usize = 1005;
const CMD_SYM: usize = 1007;
// 齿轮主菜单新增项（借鉴 WindInput 的工具栏主菜单：状态开关 + 方案/主题/候选数/模糊音子菜单 + 维护）
const CMD_SCHEMA_BASE: usize = 1100; // +0..3
const CMD_THEME_BASE: usize = 1110; // +0..2
const CMD_PAGE_BASE: usize = 1120; // +0..7 对应 3..10
const CMD_FUZZY_BASE: usize = 1140; // +0..N
const CMD_LEARN: usize = 1170;
const CMD_STATUSBAR: usize = 1171;
const CMD_REDEPLOY: usize = 1172;
const CMD_OPEN_DATA: usize = 1173;
const CMD_OPEN_LOGS: usize = 1174;
const CMD_CLEAR_LOGS: usize = 1175;
const CMD_UPDATE: usize = 1176;
const CMD_RELOAD: usize = 1177;

const THEME_PRESETS: [(&str, [&str; 8]); 3] = [
    (
        "浅色",
        [
            "light", "#FFFFFF", "#222222", "#2A6FDB", "#2A6FDB", "#FFFFFF", "#999999", "#D8DCE3",
        ],
    ),
    (
        "深色",
        [
            "dark", "#202127", "#E9E9EC", "#5B93F2", "#3A5FA8", "#FFFFFF", "#9AA1B2", "#3A3D46",
        ],
    ),
    (
        "护眼",
        [
            "custom", "#F4F8F1", "#2C3B2A", "#3E8E41", "#3E8E41", "#FFFFFF", "#7C9A79", "#C6DCC2",
        ],
    ),
];

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

/// 会话内记忆的位置。
///
/// 注意：这两个函数必须共用**同一个** static —— 早前各自声明一个 `static POS`，
/// 结果 set 写进 A、get 永远读空的 B，表现为「拖不动」：按下后基准位置取成 (0,0)
/// 让状态栏瞬移到左上角，松手时又被 push() 按默认右下角复位。
static MEM_POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();

fn mem_pos() -> &'static Mutex<Option<(i32, i32)>> {
    MEM_POS.get_or_init(|| Mutex::new(None))
}

fn mem_pos_get() -> Option<(i32, i32)> {
    *mem_pos().lock().unwrap_or_else(|e| e.into_inner())
}

fn mem_pos_set(p: (i32, i32)) {
    *mem_pos().lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

// ---------------- 预渲染资源 ----------------

struct Assets {
    bar_bg: &'static [u8],
    cell_hover: &'static [u8],
    cell_active: &'static [u8],
    icon_gear: &'static [u8],
    glyph_quan: &'static [u8],
    glyph_ban: &'static [u8],
    glyph_punct_cn: &'static [u8],
    glyph_punct_en: &'static [u8],
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
        icon_gear: include_bytes!("assets/icon_gear.bin"),
        glyph_quan: include_bytes!("assets/glyph_quan.bin"),
        glyph_ban: include_bytes!("assets/glyph_ban.bin"),
        glyph_punct_cn: include_bytes!("assets/glyph_punct_cn.bin"),
        glyph_punct_en: include_bytes!("assets/glyph_punct_en.bin"),
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
                if punct { a.glyph_punct_en } else { a.glyph_punct_cn },
                FG,
            ),
            CELL_WIDTH => (
                if full { a.glyph_quan } else { a.glyph_ban },
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
    /// 按下点的**客户区**坐标（只用来说明"点在哪一格"）
    drag_off: std::cell::Cell<(i32, i32)>,
    /// 拖动锚点：按下时的窗口左上角（屏幕坐标）
    drag_win: std::cell::Cell<(i32, i32)>,
    /// 拖动锚点：按下时的光标位置（屏幕坐标）
    drag_cur: std::cell::Cell<(i32, i32)>,
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
            drag_win: std::cell::Cell::new((0, 0)),
            drag_cur: std::cell::Cell::new((0, 0)),
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
        crate::tsf_log!("status bar click: cell={cell}");
        if cell == CELL_SETTINGS {
            // 齿轮格 → 主菜单（末项才是「设置…」），对齐 WindInput 的工具栏菜单
            self.show_menu();
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
                let mut cfg = xunyin_rime::config::load_config();
                cfg.input.full_shape = !cfg.input.full_shape;
                let _ = xunyin_rime::config::save_config(&cfg);
                e.refresh_options(vec![("full_shape".into(), cfg.input.full_shape)]);
                if let Some(sh) = latest_shared() {
                    if let Ok(mut st) = sh.lock() {
                        st.full_shape = cfg.input.full_shape;
                    }
                }
                self.update(self.ascii.get(), cfg.input.full_shape, self.punct.get());
            }
            CELL_PUNCT => {
                let mut cfg = xunyin_rime::config::load_config();
                cfg.input.ascii_punct = !cfg.input.ascii_punct;
                let _ = xunyin_rime::config::save_config(&cfg);
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
        match settings_exe_path() {
            Some(exe) => {
                crate::tsf_log!("open settings: {}", exe.display());
                match std::process::Command::new(&exe)
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：避免控制台黑框
                    .spawn()
                {
                    Ok(child) => {
                        // 放开前台限制：否则设置窗口可能开在浏览器后面，看起来像"点了没反应"
                        unsafe {
                            let _ = AllowSetForegroundWindow(child.id());
                        }
                        crate::tsf_log!("open settings: pid={} 已放开前台限制", child.id());
                    }
                    Err(e) => crate::tsf_log!("open settings: 启动失败 {e}"),
                }
            }
            None => crate::tsf_log!(
                "open settings: 未找到 xunyin-ime.exe（DLL 目录 {:?}）",
                module_dir()
            ),
        }
    }

    /// 主菜单（点齿轮格或右键弹出）：状态开关 + 方案/主题/候选数/模糊音子菜单 + 维护项。
    ///
    /// 借鉴 WindInput 的工具栏主菜单：设置入口不再直接开窗口，而是先给一个能立刻
    /// 改常用状态的菜单，最后一项才是「设置…」。
    fn show_menu(&self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            crate::tsf_log!("status bar: 打开主菜单");
            let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
            let cfg = xunyin_rime::config::load_config();
            let add = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                AppendMenuW(m, flags, id, PCWSTR(t.as_ptr())).ok();
            };
            add(
                menu,
                MF_STRING | check(!self.ascii.get()),
                CMD_MODE,
                "中文模式",
            );
            // 输入方案（只列我们的两个拼音方案）
            if let Ok(sub) = CreatePopupMenu() {
                let mut any = false;
                if let Some(eng) = engine::engine() {
                    if let Ok(e) = eng.lock() {
                        for (i, (sid, name)) in e.get_schema_list().iter().take(4).enumerate() {
                            add(
                                sub,
                                MF_STRING | check(*sid == cfg.input.schema),
                                CMD_SCHEMA_BASE + i,
                                name,
                            );
                            any = true;
                        }
                    }
                }
                let _ = any;
                AppendMenuW(menu, MF_POPUP | MF_STRING, sub.0 as usize, w!("输入方案")).ok();
            }
            // 每页候选数
            if let Ok(sub) = CreatePopupMenu() {
                for n in 3..=10usize {
                    add(
                        sub,
                        MF_STRING | check(cfg.input.page_size as usize == n),
                        CMD_PAGE_BASE + (n - 3),
                        &format!("{n} 个"),
                    );
                }
                AppendMenuW(menu, MF_POPUP | MF_STRING, sub.0 as usize, w!("每页候选数")).ok();
            }
            // 模糊音
            if let Ok(sub) = CreatePopupMenu() {
                for (i, (gid, label, _)) in xunyin_rime::config::FUZZY_GROUPS.iter().enumerate() {
                    add(
                        sub,
                        MF_STRING | check(cfg.input.fuzzy.iter().any(|f| f == gid)),
                        CMD_FUZZY_BASE + i,
                        label,
                    );
                }
                AppendMenuW(menu, MF_POPUP | MF_STRING, sub.0 as usize, w!("模糊音")).ok();
            }
            // 主题
            if let Ok(sub) = CreatePopupMenu() {
                for (i, (name, preset)) in THEME_PRESETS.iter().enumerate() {
                    add(
                        sub,
                        MF_STRING | check(cfg.appearance.theme == preset[0]),
                        CMD_THEME_BASE + i,
                        name,
                    );
                }
                AppendMenuW(menu, MF_POPUP | MF_STRING, sub.0 as usize, w!("主题")).ok();
            }
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            add(
                menu,
                MF_STRING | check(!self.punct.get()),
                CMD_PUNCT,
                "中文标点",
            );
            add(menu, MF_STRING | check(self.full.get()), CMD_WIDTH, "全角");
            add(
                menu,
                MF_STRING | check(cfg.appearance.show_status_bar),
                CMD_STATUSBAR,
                "显示状态栏",
            );
            add(
                menu,
                MF_STRING | check(cfg.input.enable_user_dict),
                CMD_LEARN,
                "用户词典自学习",
            );
            add(
                menu,
                MF_STRING | check(crate::symbol_panel::visible()),
                CMD_SYM,
                "符号面板",
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            add(menu, MF_STRING, CMD_RELOAD, "重载配置");
            add(menu, MF_STRING, CMD_REDEPLOY, "重新部署（重建索引）");
            add(menu, MF_STRING, CMD_OPEN_DATA, "打开数据目录");
            add(menu, MF_STRING, CMD_OPEN_LOGS, "打开日志目录");
            add(menu, MF_STRING, CMD_CLEAR_LOGS, "清空日志");
            AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).ok();
            add(menu, MF_STRING, CMD_UPDATE, "查看最新版本");
            add(menu, MF_STRING, CMD_SETTINGS, "设置…");

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
                CMD_LEARN => self.menu_toggle_learn(),
                CMD_STATUSBAR => self.menu_toggle_status_bar(),
                CMD_REDEPLOY => {
                    // 重建索引 + 本进程重载 + 通知其他宿主进程重载（无需重启应用）
                    crate::tsf_log!("status bar menu: 重新部署（重建索引）");
                    engine::deploy_here_then_notify_others();
                }
                CMD_RELOAD => {
                    crate::tsf_log!("status bar menu: 重载配置");
                    engine::request_reload("reload");
                    // 本进程立刻响应，不等下一轮轮询
                    engine::poll_reload();
                }
                CMD_OPEN_DATA => open_dir(&xunyin_rime::config::user_data_dir()),
                CMD_OPEN_LOGS => open_dir(&xunyin_rime::config::user_data_dir().join("logs")),
                CMD_CLEAR_LOGS => clear_logs(),
                CMD_UPDATE => open_url("https://github.com/itsypa/xunyin-ime/releases/latest"),
                id if id >= CMD_SCHEMA_BASE && id < CMD_SCHEMA_BASE + 4 => {
                    self.menu_select_schema(id - CMD_SCHEMA_BASE)
                }
                id if id >= CMD_THEME_BASE && id < CMD_THEME_BASE + 3 => {
                    self.menu_apply_theme(id - CMD_THEME_BASE)
                }
                id if id >= CMD_PAGE_BASE && id < CMD_PAGE_BASE + 8 => {
                    self.menu_set_page_size(3 + (id - CMD_PAGE_BASE) as u32)
                }
                id if id >= CMD_FUZZY_BASE
                    && id < CMD_FUZZY_BASE + xunyin_rime::config::FUZZY_GROUPS.len() =>
                {
                    self.menu_toggle_fuzzy(id - CMD_FUZZY_BASE)
                }
                _ => {}
            }
        }
    }

    // ---- 菜单动作：写配置 + 立刻生效（模糊音/候选数/自学习需要重建索引）----

    fn menu_save(&self, cfg: &xunyin_rime::config::XunyinConfig) {
        let _ = xunyin_rime::config::save_config(cfg);
        self.push();
        crate::status_bar::notify_repaint();
    }

    fn menu_select_schema(&self, i: usize) {
        let Some(eng) = engine::engine() else { return };
        let Ok(mut e) = eng.lock() else { return };
        let Some((sid, _)) = e.get_schema_list().get(i).cloned() else {
            return;
        };
        let _ = e.select_schema(&sid);
        drop(e);
        let mut cfg = xunyin_rime::config::load_config();
        cfg.input.schema = sid.clone();
        self.menu_save(&cfg);
        crate::tsf_log!("status bar menu: 切换方案 -> {sid}");
    }

    fn menu_apply_theme(&self, i: usize) {
        let Some((_, p)) = THEME_PRESETS.get(i) else {
            return;
        };
        let mut cfg = xunyin_rime::config::load_config();
        cfg.appearance.theme = p[0].into();
        cfg.appearance.bg = p[1].into();
        cfg.appearance.text = p[2].into();
        cfg.appearance.accent = p[3].into();
        cfg.appearance.selected_bg = p[4].into();
        cfg.appearance.selected_text = p[5].into();
        cfg.appearance.comment = p[6].into();
        cfg.appearance.border = p[7].into();
        self.menu_save(&cfg);
        crate::tsf_log!("status bar menu: 主题 -> {}", p[0]);
    }

    fn menu_set_page_size(&self, n: u32) {
        let mut cfg = xunyin_rime::config::load_config();
        cfg.input.page_size = n;
        let _ = xunyin_rime::config::save_config(&cfg);
        write_page_size(n);
        self.menu_redeploy("每页候选数");
    }

    fn menu_toggle_fuzzy(&self, i: usize) {
        let Some((gid, label, _)) = xunyin_rime::config::FUZZY_GROUPS.get(i) else {
            return;
        };
        let mut cfg = xunyin_rime::config::load_config();
        if let Some(pos) = cfg.input.fuzzy.iter().position(|f| f == gid) {
            cfg.input.fuzzy.remove(pos);
        } else {
            cfg.input.fuzzy.push((*gid).to_string());
        }
        let _ = xunyin_rime::config::save_config(&cfg);
        write_schema_patch(&cfg);
        crate::tsf_log!("status bar menu: 模糊音 {label} -> 重建索引");
        self.menu_redeploy("模糊音");
    }

    fn menu_toggle_learn(&self) {
        let mut cfg = xunyin_rime::config::load_config();
        cfg.input.enable_user_dict = !cfg.input.enable_user_dict;
        let _ = xunyin_rime::config::save_config(&cfg);
        write_schema_patch(&cfg);
        crate::tsf_log!(
            "status bar menu: 自学习 -> {}",
            cfg.input.enable_user_dict
        );
        self.menu_redeploy("自学习");
    }

    fn menu_toggle_status_bar(&self) {
        let mut cfg = xunyin_rime::config::load_config();
        cfg.appearance.show_status_bar = !cfg.appearance.show_status_bar;
        let _ = xunyin_rime::config::save_config(&cfg);
        crate::tsf_log!(
            "status bar menu: 显示状态栏 -> {}",
            cfg.appearance.show_status_bar
        );
        if !cfg.appearance.show_status_bar {
            self.hide();
        }
    }

    fn menu_redeploy(&self, why: &str) {
        crate::tsf_log!("status bar menu: {why} 变化 -> 后台重建索引 + 重载");
        engine::deploy_here_then_notify_others();
    }
}

/// 打开目录（资源管理器）。
fn open_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
}

/// 清空日志（用户目录 logs\*.log 与 %TEMP% 兜底文件）。
fn clear_logs() {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(xunyin_rime::config::user_data_dir().join("logs")) {
        for e in rd.flatten() {
            if e.path().is_file() && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    if let Ok(tmp) = std::env::var("TEMP") {
        if std::fs::remove_file(std::path::Path::new(&tmp).join("xunyin-tsf.log")).is_ok() {
            n += 1;
        }
    }
    crate::tsf_log!("status bar menu: 清空日志 {n} 个文件");
}

/// 用浏览器打开 URL。
fn open_url(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .creation_flags(0x0800_0000)
        .spawn();
}

/// 把每页候选数写进用户目录 default.custom.yaml。
fn write_page_size(n: u32) {
    let dir = xunyin_rime::config::user_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join("default.custom.yaml"),
        format!("patch:\n  menu/page_size: {}\n", n.clamp(1, 20)),
    );
}

/// 模糊音 / 自学习写入 `<schema>.custom.yaml`（与设置中心同一套逻辑）。
fn write_schema_patch(cfg: &xunyin_rime::config::XunyinConfig) {
    let Some(dll_dir) = module_dir() else { return };
    let mut shared = None;
    let mut cur = Some(dll_dir);
    for _ in 0..6 {
        let Some(d) = cur else { break };
        let cand = d.join("rime").join("data");
        if cand.join("default.yaml").exists() {
            shared = Some(cand);
            break;
        }
        cur = d.parent().map(|p| p.to_path_buf());
    }
    let Some(shared) = shared else { return };
    let dir = xunyin_rime::config::user_data_dir();
    let sid = cfg.input.schema.trim();
    if sid.is_empty() {
        return;
    }
    let file = dir.join(format!("{sid}.custom.yaml"));
    let base = xunyin_rime::config::read_schema_algebra(&shared.join(format!("{sid}.schema.yaml")));
    match cfg.input.schema_patch_yaml(&base) {
        Some(text) => {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, text);
        }
        None => {
            if file.exists() {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
}

/// 定位设置中心可执行文件，兼容两种布局：
/// - 安装布局：<安装目录>\xunyin-ime.exe（TSF DLL 在 tsf\ 或 tsf-x86\ 子目录）；
/// - 开发布局：xunyin-ime\native\target\{debug,release}\xunyin-ime.exe。
fn module_dir() -> Option<std::path::PathBuf> {
    unsafe {
        let mut hmod: HMODULE = core::mem::zeroed();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        let addr = (module_dir as fn() -> Option<std::path::PathBuf>) as usize;
        GetModuleHandleExW(flags, PCWSTR(addr as *const u16), &mut hmod).ok()?;
        let mut buf = [0u16; 1024];
        let len = GetModuleFileNameW(Some(hmod), &mut buf);
        if len == 0 {
            return None;
        }
        let dll = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        Some(dll.parent()?.to_path_buf())
    }
}

fn settings_exe_path() -> Option<std::path::PathBuf> {
    let dll_dir = module_dir()?;
    // 1) 同目录或逐级向上：安装布局 <安装目录>\xunyin-ime.exe（DLL 在 tsf\ 子目录）。
    //    注意：这里不能因为走到盘符根就提前 return，否则后面两种布局永远搜不到。
    let mut dir = Some(dll_dir.clone());
    while let Some(d) = dir {
        let exe = d.join("xunyin-ime.exe");
        if exe.is_file() {
            return Some(exe);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    // 2) 开发布局：工作区根下 xunyin-ime\native\target\{release,debug}\xunyin-ime.exe
    //
    // 注意顺序：**release 优先**（release 更小更快；安装布局第 1 步命中的本来就是
    // release 产物，不受影响）。设置中心已是原生 Win32 程序：窗口出现
    // release ≈ 30ms、debug ≈ 130ms（旧 Tauri/WebView2 版分别是 60~100ms / 2.1s）。
    let mut dir = Some(dll_dir);
    while let Some(d) = dir {
        for profile in ["release", "debug"] {
            let exe = d
                .join("xunyin-ime")
                .join("native")
                .join("target")
                .join(profile)
                .join("xunyin-ime.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    // 3) 兜底：默认安装位置 %ProgramFiles%\XunyinIME
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(pf) = std::env::var(key) {
            let exe = std::path::Path::new(&pf)
                .join("XunyinIME")
                .join("xunyin-ime.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
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
            w!("XunyinStatus"),
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
            WM_TIMER => {
                // 每个 tick 顺带看一眼「重载 / 重新部署」请求：小狼毫式的
                // 重新部署即可生效（不用重启记事本 / 浏览器 / ChatGPT）。
                if wparam.0 == POLL_TIMER_ID {
                    engine::poll_reload();
                }
                // 前台性核对：本进程已不是前台进程 → 隐藏（见 POLL_TIMER_ID 注释）
                if wparam.0 == POLL_TIMER_ID && this.shown.get() {
                    let fg = GetForegroundWindow();
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(fg, Some(&mut pid));
                    if pid != GetCurrentProcessId() {
                        crate::tsf_log!("status bar: 本进程已非前台(前台pid={pid}) -> 隐藏");
                        this.hide();
                    }
                }
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
                let mut pt = POINT::default();
                GetCursorPos(&mut pt);
                // 拖动以「窗口左上角 + 光标屏幕位移」计算：客户端坐标会随窗口移动而变化，
                // 用它做增量会让状态栏边拖边跳。
                let win0 = mem_pos_get().unwrap_or_else(|| this.pos());
                this.dragging.set(true);
                this.dragged.set(false);
                this.drag_off.set((x, y));
                this.drag_win.set(win0);
                this.drag_cur.set((pt.x, pt.y));
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if this.dragging.get() {
                    let mut pt = POINT::default();
                    GetCursorPos(&mut pt);
                    let (cur_x, cur_y) = this.drag_cur.get();
                    // 拖动判定用系统拖拽阈值（SM_CXDRAG/SM_CYDRAG），避免轻微抖动把点击吞掉
                    let thr_x = GetSystemMetrics(SM_CXDRAG).max(2);
                    let thr_y = GetSystemMetrics(SM_CYDRAG).max(2);
                    if !this.dragged.get()
                        && ((pt.x - cur_x).abs() > thr_x || (pt.y - cur_y).abs() > thr_y)
                    {
                        this.dragged.set(true);
                        if this.hover.replace(-1) != -1 {
                            this.push();
                        }
                    }
                    if this.dragged.get() {
                        let (win_x, win_y) = this.drag_win.get();
                        let mut x = win_x + (pt.x - cur_x);
                        let mut y = win_y + (pt.y - cur_y);
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
                        // 悬停态变化留一行日志，便于排查"悬停没反应"类反馈
                        crate::tsf_log!("status bar: hover {} -> {} (lx={lx} ly={ly})", this.hover.get(), h);
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
                        crate::tsf_log!("status bar: 抬起判定为拖动 -> 不触发点击");
                        // 拖动结束：按新位置所在显示器的 DPI 重新渲染
                        this.push();
                    } else {
                        let s = this.dpi_scale();
                        // 点击格以「按下点」为准（Windows 惯例）：轻微抖动不会点错格
                        let (dx, dy) = this.drag_off.get();
                        let lx = (dx as f32 / s) as i32;
                        let ly = (dy as f32 / s) as i32;
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
            .name("xunyin-statusbar".into())
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
        // 前台性轮询定时器（见 POLL_TIMER_ID 注释）
        SetTimer(Some(hwnd), POLL_TIMER_ID, POLL_MS, None);
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
