#![allow(unused_must_use)]

//! 符号面板：六类常用符号（常用标点 / 数学 / 单位 / 序号 / 箭头 / 特殊）点选直上屏。
//!
//! 运行在**独立 UI 线程**（自带消息循环），窗口 `WS_EX_NOACTIVATE`：面板不抢走目标应用
//! 的焦点，点击符号用 `SendInput` 注入 `VK_PACKET` Unicode 直接上屏（不参与拼音组合，
//! 也不经标点转换）。窗口按显示器 DPI 用 GDI 直接绘制，形状用 `SetWindowRgn` 裁圆角；
//! 可拖动，位置仅会话内记忆。

use std::mem::size_of;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SendInput, SetCapture, TrackMouseEvent, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{w, PCWSTR};

const WND_CLASS: &str = "Xunyin.SymbolPanel";

// 尺寸（逻辑 dp，绘制时按显示器 DPI 放大）
const PAD: i32 = 8;
const HEADER_H: i32 = 26;
const TABS_Y: i32 = 30;
const TABS_H: i32 = 22;
const GRID_Y: i32 = 58;
const KEY_W: i32 = 40;
const KEY_H: i32 = 36;
const GAP: i32 = 4;
const COLS: i32 = 10;
const ROWS: i32 = 3;

const PANEL_W: i32 = PAD * 2 + COLS * KEY_W + (COLS - 1) * GAP; // 452
const PANEL_H: i32 = GRID_Y + ROWS * KEY_H + (ROWS - 1) * GAP + PAD; // 182

// 配色（COLORREF = 0x00BBGGRR，与状态栏同一套色）
const C_PANEL: u32 = 0x00F7F7F7; // 面板底 #F7F7F7
const C_BORDER: u32 = 0x00E2DEDE; // 边框 #DEDEE2
const C_KEY: u32 = 0x00FFFFFF; // 键面白
// 悬停/按下用与状态栏同一套淡蓝（原来的 #F0F6FC 在白键面上几乎看不出）
const C_KEY_HOVER: u32 = 0x00FEF0E8; // 悬停 #E8F0FE
const C_KEY_DOWN: u32 = 0x00FDE4D6; // 按下 #D6E4FD
const C_TEXT: u32 = 0x004E4848; // 字色 #48484E
const C_TITLE: u32 = 0x00948C8C; // 标题/未选中页签 #8C8C94
const C_ACTIVE: u32 = 0x00F48542; // 选中页签主题蓝 #4285F4
const C_GRIP: u32 = 0x00C2BABA; // 拖动柄点 #BABAC2

// 面板消息
const SP_SHOW: u32 = WM_APP + 1;
const SP_HIDE: u32 = WM_APP + 2;

// 前台性轮询：本进程不再是前台（切到别的程序）→ 自动收起面板，避免残留在屏幕上
const POLL_TIMER_ID: usize = 2;
const POLL_MS: u32 = 400;

/// 分类页签名 + 符号表（每类 30 个 = 3 行 × 10 列）。
const SYM_TABS: [&str; 8] = ["常用", "数学", "单位", "序号", "箭头", "特殊", "表情", "颜文字"];
const SYM_SETS: [&[&str]; 8] = [
    &[
        "，", "。", "？", "！", "；", "：", "、", "“", "”", "‘", "’", "（", "）", "《", "》", "【",
        "】", "…", "—", "·", "～", "〈", "〉", "「", "」", "『", "』", "〔", "〕", "｜",
    ],
    &[
        "＋", "－", "×", "÷", "＝", "≠", "≈", "≤", "≥", "∞", "∑", "√", "∫", "±", "∓", "∝", "∠",
        "⊥", "∥", "∈", "∉", "∪", "∩", "∵", "∴", "∶", "∷", "∂", "∇", "≡",
    ],
    &[
        "℃", "℉", "°", "％", "‰", "㎜", "㎝", "㎞", "㎡", "㎏", "㏄", "µ", "Ω", "Å", "㏑", "㏒",
        "㎎", "㎥", "㍱", "㍲", "㍳", "㍴", "㍵", "㍶", "㎧", "㎨", "㎩", "㎪", "㎫", "㎬",
    ],
    &[
        "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑴", "⑵", "⑶", "⑷", "⑸", "⑹", "⑺",
        "⑻", "⑼", "⑽", "⒈", "⒉", "⒊", "⒋", "⒌", "⒍", "⒎", "⒏", "⒐", "⒑",
    ],
    &[
        "←", "↑", "→", "↓", "↔", "↕", "⇐", "⇑", "⇒", "⇓", "⇔", "⇕", "↖", "↗", "↘", "↙", "↺",
        "↻", "⟵", "⟶", "⟷", "⟹", "⟸", "⇄", "⇅", "⇆", "⇋", "⇌", "↩", "↪",
    ],
    &[
        "★", "☆", "♥", "♡", "✓", "✗", "※", "§", "¶", "©", "®", "™", "€", "£", "¥", "¢", "№", "☀",
        "☁", "☂", "☃", "♠", "♣", "♦", "♪", "♫", "✉", "☎", "✔", "✿",
    ],
    // 表情：单码位/简单组合的 emoji（不放入 ZWJ 全家福那种，注入会散架）
    &[
        "😀", "😄", "😁", "😆", "😅", "😂", "🙂", "😉", "😍", "😘", "🤔", "😐", "😴", "😪", "😭", "😢",
        "😡", "😱", "🥰", "😎", "🤝", "👍", "👌", "🙏", "💪", "🎉", "🔥", "⭐", "✅", "❌",
    ],
    // 颜文字：都比键面窄（≤4 个半角字符），放得下
    &[
        "^_^", "^o^", "^_~", ">_<", ">_>", "<_<", "T_T", "TAT", "QAQ", "Q_Q", "-_-", "=_=", "=w=", "o_o",
        "O_O", "0_0", "*_*", "+_+", "Orz", "orz", "OTL", ">_<", "(笑)", "(哭)", "(汗)", "(怒)", "(囧)",
        "(萌)", "(拜)", "(惊)",
    ],
];

/// 注入一个 Unicode 字符（`VK_PACKET` 直上屏，不经输入法组合）。
fn send_unicode(ch: char) {
    unsafe {
        let mut inputs = [INPUT::default(); 2];
        inputs[0].r#type = INPUT_KEYBOARD;
        inputs[0].Anonymous = INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: ch as u16,
                dwFlags: KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: 0,
            },
        };
        inputs[1] = inputs[0];
        inputs[1].Anonymous.ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

// ---------------- 窗口 ----------------

struct Panel {
    hwnd: HWND,
    scale: f32,
    tab: usize,
    hover: i32,
    pressed: i32,
    dragging: bool,
    drag_off: (i32, i32),
    drag_win: (i32, i32),
    drag_cur: (i32, i32),
    dragged: bool,
    font: HFONT,
    font_fn: HFONT,
}

unsafe impl Send for Panel {}
unsafe impl Sync for Panel {}

fn key_rect(row: i32, col: i32) -> (i32, i32, i32, i32) {
    let x = PAD + col * (KEY_W + GAP);
    let y = GRID_Y + row * (KEY_H + GAP);
    (x, y, x + KEY_W, y + KEY_H)
}

fn tab_rect(i: usize) -> (i32, i32, i32, i32) {
    // 页签等分面板宽度：页签数量增减都不会溢出面板（从前写死 60 宽，加到 8 个就出界了）
    let n = SYM_TABS.len() as i32;
    let gap = 4;
    let w = (PANEL_W - PAD * 2 - gap * (n - 1)) / n;
    let x = PAD + i as i32 * (w + gap);
    (x, TABS_Y, x + w, TABS_Y + TABS_H)
}

fn close_rect() -> (i32, i32, i32, i32) {
    let s = HEADER_H - 8;
    (PANEL_W - PAD - s, 4, PANEL_W - PAD, 4 + s)
}

fn in_rect(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && x <= r.2 && y >= r.1 && y <= r.3
}

/// 命中测试（物理像素坐标）→ 符号下标，未命中返回 -1。
fn hit_symbol(p: &Panel, x: i32, y: i32) -> i32 {
    let lx = (x as f32 / p.scale) as i32;
    let ly = (y as f32 / p.scale) as i32;
    if ly < GRID_Y {
        return -1;
    }
    for (i, _) in SYM_SETS[p.tab].iter().enumerate() {
        let r = (i / COLS as usize) as i32;
        if r >= ROWS {
            break;
        }
        let c = (i % COLS as usize) as i32;
        if in_rect(key_rect(r, c), lx, ly) {
            return i as i32;
        }
    }
    -1
}

/// 会话内记忆的面板位置（必须共用同一个 static，理由见状态栏 mem_pos 注释）。
static MEM_POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();

fn mem_pos() -> &'static Mutex<Option<(i32, i32)>> {
    MEM_POS.get_or_init(|| Mutex::new(None))
}

fn mem_pos_get() -> Option<(i32, i32)> {
    *mem_pos().lock().unwrap_or_else(|e| e.into_inner())
}

fn mem_pos_set(v: (i32, i32)) {
    *mem_pos().lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
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

fn create_hwnd() -> HWND {
    unsafe {
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = WND_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        match CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
            PCWSTR(class_name.as_ptr()),
            w!("XunyinSymbolPanel"),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            PANEL_W,
            PANEL_H,
            None,
            None,
            Some(HINSTANCE(hmod.0)),
            None,
        ) {
            Ok(h) => h,
            Err(_) => HWND(std::ptr::null_mut()),
        }
    }
}

fn dpi_scale(hwnd: HWND) -> f32 {
    unsafe {
        let dc = GetDC(Some(hwnd));
        if dc.0.is_null() {
            return 1.0;
        }
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSY).max(96);
        ReleaseDC(Some(hwnd), dc);
        dpi as f32 / 96.0
    }
}

fn make_font(scale: f32, small: bool) -> HFONT {
    unsafe {
        let base = if small { 12.0 } else { 16.0 };
        let height = -((base * scale).round() as i32).max(8);
        let name: Vec<u16> = "Microsoft YaHei UI"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

fn apply_region(hwnd: HWND, w: i32, h: i32, r: i32) {
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r);
        if !rgn.0.is_null() {
            SetWindowRgn(hwnd, Some(rgn), true);
        }
    }
}

impl Panel {
    fn new() -> Self {
        let hwnd = if ensure_window_class() {
            create_hwnd()
        } else {
            HWND(std::ptr::null_mut())
        };
        let scale = if hwnd.0.is_null() { 1.0 } else { dpi_scale(hwnd) };
        Self {
            hwnd,
            scale,
            tab: 0,
            hover: -1,
            pressed: -1,
            dragging: false,
            drag_off: (0, 0),
            drag_win: (0, 0),
            drag_cur: (0, 0),
            dragged: false,
            font: make_font(scale, false),
            font_fn: make_font(scale, true),
        }
    }

    fn px(&self, v: i32) -> i32 {
        ((v as f32) * self.scale).round() as i32
    }

    fn win_size(&self) -> (i32, i32) {
        (self.px(PANEL_W), self.px(PANEL_H))
    }

    fn refresh_dpi(&mut self) {
        let s = dpi_scale(self.hwnd);
        if (s - self.scale).abs() > 0.01 {
            self.scale = s;
            unsafe {
                DeleteObject(HGDIOBJ(self.font.0));
                DeleteObject(HGDIOBJ(self.font_fn.0));
            }
            self.font = make_font(s, false);
            self.font_fn = make_font(s, true);
        }
    }

    fn show(&mut self) {
        unsafe {
            if IsWindowVisible(self.hwnd).as_bool() {
                self.hide();
                return;
            }
        }
        self.refresh_dpi();
        self.hover = -1;
        self.pressed = -1;
        let (x, y) = if let Some(p) = mem_pos_get() {
            p
        } else {
            let wa = work_area();
            let (w, h) = self.win_size();
            let p = (wa.right - w - 12, wa.bottom - h - 56);
            mem_pos_set(p);
            p
        };
        let (w, h) = self.win_size();
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            apply_region(self.hwnd, w, h, self.px(12));
            InvalidateRect(Some(self.hwnd), None, true);
        }
        crate::status_bar::notify_repaint();
    }

    fn hide(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
        crate::status_bar::notify_repaint();
    }
}

// ---------------- 绘制 ----------------

fn fill_round(hdc: HDC, r: (i32, i32, i32, i32), radius: i32, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
        let old_b = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_p = SelectObject(hdc, HGDIOBJ(pen.0));
        RoundRect(hdc, r.0, r.1, r.2, r.3, radius * 2, radius * 2);
        SelectObject(hdc, old_b);
        SelectObject(hdc, old_p);
        DeleteObject(HGDIOBJ(brush.0));
        DeleteObject(HGDIOBJ(pen.0));
    }
}

fn stroke_round(hdc: HDC, r: (i32, i32, i32, i32), radius: i32, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(C_KEY));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
        let old_b = SelectObject(hdc, HGDIOBJ(brush.0));
        let old_p = SelectObject(hdc, HGDIOBJ(pen.0));
        RoundRect(hdc, r.0, r.1, r.2, r.3, radius * 2, radius * 2);
        SelectObject(hdc, old_b);
        SelectObject(hdc, old_p);
        DeleteObject(HGDIOBJ(brush.0));
        DeleteObject(HGDIOBJ(pen.0));
    }
}

fn draw_label(hdc: HDC, r: (i32, i32, i32, i32), text: &str, font: HFONT, color: u32) {
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(color));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: r.0,
            top: r.1,
            right: r.2,
            bottom: r.3,
        };
        DrawTextW(hdc, &mut wide, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(hdc, old);
    }
}

impl Panel {
    fn paint(&self) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if hdc.0.is_null() {
                return;
            }
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc);
            let (w, h) = (rc.right, rc.bottom);
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            SelectObject(mem, HGDIOBJ(bmp.0));
            let s = self.scale;
            let px = |v: i32| ((v as f32) * s).round() as i32;
            let sr = |r: (i32, i32, i32, i32)| (px(r.0), px(r.1), px(r.2), px(r.3));

            // 面板底 + 圆角边框
            fill_round(mem, (0, 0, w, h), px(12), C_PANEL);
            let pen = CreatePen(PS_SOLID, 1, COLORREF(C_BORDER));
            let old_p = SelectObject(mem, HGDIOBJ(pen.0));
            let old_b = SelectObject(mem, GetStockObject(NULL_BRUSH));
            RoundRect(mem, 0, 0, w, h, px(12) * 2, px(12) * 2);
            SelectObject(mem, old_p);
            SelectObject(mem, old_b);
            DeleteObject(HGDIOBJ(pen.0));

            // 拖动柄点阵（2×3）+ 标题 + 关闭
            let grip = CreateSolidBrush(COLORREF(C_GRIP));
            for row in 0..3 {
                for col in 0..2 {
                    let gx = px(6 + col * 4);
                    let gy = px(9 + row * 4);
                    let gr = RECT {
                        left: gx,
                        top: gy,
                        right: gx + px(2),
                        bottom: gy + px(2),
                    };
                    FillRect(mem, &gr, grip);
                }
            }
            DeleteObject(HGDIOBJ(grip.0));
            draw_label(
                mem,
                (px(26), 0, px(90), px(HEADER_H)),
                "符号",
                self.font,
                C_TEXT,
            );
            draw_label(mem, sr(close_rect()), "✕", self.font_fn, C_TITLE);

            // 分类页签
            for (i, name) in SYM_TABS.iter().enumerate() {
                let rr = sr(tab_rect(i));
                if i == self.tab {
                    fill_round(mem, rr, px(5), C_ACTIVE);
                    draw_label(mem, rr, name, self.font_fn, 0x00FFFFFF);
                } else {
                    draw_label(mem, rr, name, self.font_fn, C_TITLE);
                }
            }

            // 符号格
            for (i, sym) in SYM_SETS[self.tab].iter().enumerate() {
                let r = (i / COLS as usize) as i32;
                if r >= ROWS {
                    break;
                }
                let c = (i % COLS as usize) as i32;
                let rr = sr(key_rect(r, c));
                let state = if i as i32 == self.pressed {
                    2
                } else if i as i32 == self.hover {
                    1
                } else {
                    0
                };
                let fill = match state {
                    2 => C_KEY_DOWN,
                    1 => C_KEY_HOVER,
                    _ => C_KEY,
                };
                fill_round(mem, rr, px(6), fill);
                stroke_round(mem, rr, px(6), C_BORDER);
                draw_label(mem, rr, sym, self.font, C_TEXT);
            }

            BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            DeleteObject(HGDIOBJ(bmp.0));
            DeleteDC(mem);
            EndPaint(self.hwnd, &ps);
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let userdata = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if userdata == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let p = &mut *(userdata as *mut Panel);
        match msg {
            SP_SHOW => {
                p.show();
                LRESULT(0)
            }
            SP_HIDE => {
                p.hide();
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == POLL_TIMER_ID {
                    let visible = IsWindowVisible(p.hwnd).as_bool();
                    if visible {
                        let fg = GetForegroundWindow();
                        let mut pid = 0u32;
                        GetWindowThreadProcessId(fg, Some(&mut pid));
                        if pid != GetCurrentProcessId() {
                            crate::tsf_log!("symbol panel: 本进程已非前台(pid={pid}) -> 收起");
                            p.hide();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_PAINT => {
                p.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                if let Ok(c) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(c));
                }
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let mx = (lparam.0 & 0xFFFF) as i32;
                let my = ((lparam.0 >> 16) & 0xFFFF) as i32;
                let s = p.scale;
                p.dragging = true;
                p.dragged = false;
                p.drag_off = ((mx as f32 / s) as i32, (my as f32 / s) as i32);
                // 拖动锚点用屏幕坐标（客户区坐标会随窗口移动而变，做增量会抖）
                let mut pt = POINT::default();
                GetCursorPos(&mut pt);
                p.drag_win = mem_pos_get().unwrap_or((0, 0));
                p.drag_cur = (pt.x, pt.y);
                p.pressed = hit_symbol(p, mx, my);
                SetCapture(hwnd);
                InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let mx = (lparam.0 & 0xFFFF) as i32;
                let my = ((lparam.0 >> 16) & 0xFFFF) as i32;
                if p.dragging {
                    let mut pt = POINT::default();
                    GetCursorPos(&mut pt);
                    let (cur_x, cur_y) = p.drag_cur;
                    if !p.dragged && (pt.x - cur_x).abs() + (pt.y - cur_y).abs() > 3 {
                        p.dragged = true;
                    }
                    if p.dragged {
                        let (win_x, win_y) = p.drag_win;
                        let mut x = win_x + (pt.x - cur_x);
                        let mut y = win_y + (pt.y - cur_y);
                        let wa = work_area();
                        let (w, h) = p.win_size();
                        x = x.clamp(wa.left - w + 40, wa.right - 40);
                        y = y.clamp(wa.top, wa.bottom - h + 20);
                        mem_pos_set((x, y));
                        SetWindowPos(
                            hwnd,
                            Some(HWND_TOPMOST),
                            x,
                            y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_NOREDRAW,
                        );
                    }
                } else {
                    let idx = hit_symbol(p, mx, my);
                    if idx != p.hover {
                        p.hover = idx;
                        InvalidateRect(Some(hwnd), None, true);
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
                if p.dragging {
                    ReleaseCapture();
                    p.dragging = false;
                    let mx = (lparam.0 & 0xFFFF) as i32;
                    let my = ((lparam.0 >> 16) & 0xFFFF) as i32;
                    let s = p.scale;
                    let (lx, ly) = ((mx as f32 / s) as i32, (my as f32 / s) as i32);
                    if p.dragged {
                        p.dragged = false;
                    } else if in_rect(close_rect(), lx, ly) {
                        p.hide();
                    } else if ly >= TABS_Y && ly <= TABS_Y + TABS_H {
                        for i in 0..SYM_TABS.len() {
                            if in_rect(tab_rect(i), lx, ly) {
                                p.tab = i;
                                p.hover = -1;
                                break;
                            }
                        }
                    } else {
                        let idx = hit_symbol(p, mx, my);
                        if idx >= 0 {
                            if let Some(s) = SYM_SETS[p.tab].get(idx as usize) {
                                for ch in s.chars() {
                                    send_unicode(ch);
                                }
                            }
                        }
                    }
                    p.pressed = -1;
                    InvalidateRect(Some(hwnd), None, true);
                }
                LRESULT(0)
            }
            0x02A3 => {
                if p.hover != -1 {
                    p.hover = -1;
                    InvalidateRect(Some(hwnd), None, true);
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

// ---------------- 独立 UI 线程与对外接口 ----------------

static PANEL: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
static OWNER: OnceLock<Mutex<Option<Box<Panel>>>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

fn panel_hwnd() -> Option<HWND> {
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("xunyin-symbol-panel".into())
            .spawn(ui_thread);
    });
    let g = PANEL.get_or_init(|| Mutex::new(None));
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

fn ui_thread() {
    unsafe {
        // 消息窗口与绘制窗口必须是**同一个** hwnd：Panel::new() 自己创建窗口，
        // 这里直接沿用它的 hwnd（旧实现另建了一个窗口，导致可见窗口既收不到
        // WM_LBUTTON*（点了没反应）也收不到 WM_PAINT（面板空白））。
        let owner = OWNER.get_or_init(|| Mutex::new(None));
        let (hwnd, ptr) = {
            let mut g = owner.lock().unwrap_or_else(|e| e.into_inner());
            *g = Some(Box::new(Panel::new()));
            match g.as_ref() {
                // Box 在堆上，地址稳定；窗口数据里存这个地址
                Some(p) if !p.hwnd.0.is_null() => (p.hwnd, &**p as *const Panel as usize),
                _ => {
                    crate::tsf_log!("symbol panel: 窗口创建失败（窗口类或 DPI 异常）");
                    return;
                }
            }
        };
        set_window_data(hwnd, ptr);
        *PANEL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hwnd.0 as usize);
        crate::tsf_log!("symbol panel: ready hwnd={:?}", hwnd);
        {
            // 前台性轮询定时器（见 POLL_TIMER_ID 注释）
            let _ = SetTimer(Some(hwnd), POLL_TIMER_ID, POLL_MS, None);
        }
        {
            let mut g = owner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(p) = g.as_mut() {
                p.refresh_dpi();
                let (w, h) = p.win_size();
                SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_NOMOVE,
                );
                apply_region(hwnd, w, h, p.px(12));
            }
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 切换符号面板：已打开则收起。
pub fn toggle() {
    match panel_hwnd() {
        Some(hwnd) => {
            crate::tsf_log!("symbol panel: toggle -> hwnd={:?}", hwnd);
            unsafe {
                PostMessageW(Some(hwnd), SP_SHOW, WPARAM(0), LPARAM(0));
            }
        }
        None => crate::tsf_log!("symbol panel: toggle 失败——面板窗口未创建"),
    }
}

/// 面板当前是否可见（不触发线程/窗口创建，供状态栏画高亮用）。
pub fn visible() -> bool {
    let Some(g) = PANEL.get() else {
        return false;
    };
    let guard = match g.lock() {
        Ok(x) => x,
        Err(e) => e.into_inner(),
    };
    match *guard {
        Some(h) => unsafe { IsWindowVisible(HWND(h as *mut core::ffi::c_void)).as_bool() },
        None => false,
    }
}
