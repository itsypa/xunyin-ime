#![allow(unused_must_use)]

//! 软键盘 / 符号面板。
//!
//! 运行在**独立 UI 线程**（自带消息循环），窗口 `WS_EX_NOACTIVATE`：面板不会抢走
//! 目标应用的焦点，按键用 `SendInput` 注入——
//!   * 字母/数字/标点/退格/回车/空格/Esc → 注入真实虚拟键，走输入法组合与候选；
//!   * 符号面板里的符号 → 注入 `VK_PACKET` Unicode 直接上屏，不参与组合。
//!
//! 两种模式共用同一窗口，由标题栏页签切换：
//!   * 键盘：数字行 + 三行 QWERTY + 功能行；
//!   * 符号：常用标点 / 数学 / 单位 / 序号 / 箭头 / 特殊 六类。
//!
//! 渲染：GDI 按当前显示器 DPI 直接绘制（文字 ClearType 清晰），窗口形状用
//! `SetWindowRgn` 裁圆角。面板可拖动，位置仅会话内记忆。

use std::mem::size_of;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SendInput, SetCapture, TrackMouseEvent, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{w, PCWSTR};

const WND_CLASS: &str = "Lingxi.SoftKeyboard";

// 尺寸（逻辑 dp，绘制时按显示器 DPI 放大）
const PAD: i32 = 8;
const HEADER_H: i32 = 26;
const KEY_W: i32 = 40;
const KEY_H: i32 = 36;
const GAP: i32 = 4;
const COLS: i32 = 10;
const ROWS_KB: i32 = 5;
const ROWS_SYM: i32 = 4;

const PANEL_W: i32 = PAD * 2 + COLS * KEY_W + (COLS - 1) * GAP; // 452
const PANEL_H_KB: i32 = HEADER_H + PAD + ROWS_KB * KEY_H + (ROWS_KB - 1) * GAP + PAD;
const PANEL_H_SYM: i32 = HEADER_H + PAD + ROWS_SYM * KEY_H + (ROWS_SYM - 1) * GAP + PAD;

// 配色（COLORREF = 0x00BBGGRR，与状态栏同一套色）
const C_PANEL: u32 = 0x00F7F7F7; // 面板底 #F7F7F7
const C_BORDER: u32 = 0x00E2DEDE; // 边框 #DEDEE2
const C_KEY: u32 = 0x00FFFFFF; // 键面白
const C_KEY_HOVER: u32 = 0x00FCF6F0; // 悬停浅蓝 #F0F6FC
const C_KEY_DOWN: u32 = 0x00F7E9D9; // 按下 #D9E9F7
const C_TEXT: u32 = 0x004E4848; // 键字 #48484E
const C_TITLE: u32 = 0x00948C8C; // 标题/未选中页签 #8C8C94
const C_ACTIVE: u32 = 0x00F48542; // 选中页签主题蓝 #4285F4
const C_GRIP: u32 = 0x00C2BABA; // 拖动柄点 #BABAC2

// 面板消息
const SC_SHOW: u32 = WM_APP + 1; // wparam: 1=键盘 2=符号
const SC_HIDE: u32 = WM_APP + 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Keyboard,
    Symbol,
}

impl Mode {
    fn from_wparam(v: usize) -> Mode {
        if v == 2 {
            Mode::Symbol
        } else {
            Mode::Keyboard
        }
    }
    fn height(self) -> i32 {
        match self {
            Mode::Keyboard => PANEL_H_KB,
            Mode::Symbol => PANEL_H_SYM,
        }
    }
}

/// 一个键。
#[derive(Clone, Copy)]
enum Key {
    /// 字符键：注入对应虚拟键（字母/数字/标点），走输入法组合
    Char(char),
    /// 符号键：注入该字符串（Unicode 直上屏）
    Symbol(&'static str),
    /// 功能键：标签 + 占宽格数 + 虚拟键码
    Vk(&'static str, i32, u16),
}

impl Key {
    fn label(&self) -> String {
        match self {
            Key::Char(c) => c.to_string(),
            Key::Symbol(s) => (*s).to_string(),
            Key::Vk(t, _, _) => (*t).to_string(),
        }
    }
    fn span(&self) -> i32 {
        match self {
            Key::Vk(_, w, _) => *w,
            _ => 1,
        }
    }
    fn is_fn(&self) -> bool {
        matches!(self, Key::Vk(..))
    }
}

fn keyboard_rows() -> Vec<Vec<Key>> {
    vec![
        "1234567890".chars().map(Key::Char).collect(),
        "qwertyuiop".chars().map(Key::Char).collect(),
        "asdfghjkl;".chars().map(Key::Char).collect(),
        "zxcvbnm,./".chars().map(Key::Char).collect(),
        vec![
            Key::Vk("Esc", 1, 0x1B),
            Key::Vk("退格", 2, 0x08),
            Key::Vk("空格", 4, 0x20),
            Key::Vk("回车", 2, 0x0D),
            Key::Vk("关闭", 1, 0x1B),
        ],
    ]
}

/// 符号分类：页签名 + 符号表（每行 10 个，最多 4 行）。
const SYM_TABS: [&str; 6] = ["常用", "数学", "单位", "序号", "箭头", "特殊"];
const SYM_SETS: [&[&str]; 6] = [
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
];

// ---------------- 输入注入 ----------------

fn send_key(vk: u16) {
    unsafe {
        let mut inputs = [INPUT::default(); 2];
        inputs[0].r#type = INPUT_KEYBOARD;
        inputs[0].Anonymous = INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: Default::default(),
                time: 0,
                dwExtraInfo: 0,
            },
        };
        inputs[1] = inputs[0];
        inputs[1].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

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

/// 字符键 → 虚拟键码（US 布局；字母数字与常用标点足矣）。
fn vk_for(c: char) -> Option<u16> {
    match c {
        '0'..='9' => Some(c as u16),
        'a'..='z' => Some(c.to_ascii_uppercase() as u16),
        ';' => Some(0xBA),
        '\'' => Some(0xDE),
        ',' => Some(0xBC),
        '.' => Some(0xBE),
        '/' => Some(0xBF),
        _ => None,
    }
}

fn press(key: &Key) {
    match key {
        Key::Char(c) => {
            if let Some(vk) = vk_for(*c) {
                send_key(vk);
            } else {
                send_unicode(*c);
            }
        }
        Key::Symbol(s) => {
            for ch in s.chars() {
                send_unicode(ch);
            }
        }
        Key::Vk(_, _, vk) => send_key(*vk),
    }
}

// ---------------- 窗口 ----------------

struct Panel {
    hwnd: HWND,
    mode: Mode,
    sym_tab: usize,
    scale: f32,
    hover: i32,
    pressed: i32,
    dragging: bool,
    drag_off: (i32, i32),
    dragged: bool,
    font: HFONT,
    font_fn: HFONT,
}

unsafe impl Send for Panel {}
unsafe impl Sync for Panel {}

/// 键位矩形（逻辑 dp）。
fn key_rect(row: i32, col: i32, span: i32) -> (i32, i32, i32, i32) {
    let x = PAD + col * (KEY_W + GAP);
    let y = HEADER_H + PAD + row * (KEY_H + GAP);
    let w = span * KEY_W + (span - 1) * GAP;
    (x, y, x + w, y + KEY_H)
}

fn sym_tab_rect(i: usize) -> (i32, i32, i32, i32) {
    let w = 52;
    let x = 16 + i as i32 * (w + 4);
    (x, 34, x + w, 34 + 22)
}

fn header_rects() -> ((i32, i32, i32, i32), (i32, i32, i32, i32), (i32, i32, i32, i32)) {
    let w = 52;
    let h = HEADER_H - 8;
    let kb = (16, 4, 16 + w, 4 + h);
    let sym = (16 + w + 4, 4, 16 + w * 2 + 4, 4 + h);
    let close = (PANEL_W - 16 - h, 4, PANEL_W - 16, 4 + h);
    (kb, sym, close)
}

fn in_rect(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && x <= r.2 && y >= r.1 && y <= r.3
}

fn hit_key(p: &Panel, x: i32, y: i32) -> i32 {
    let lx = (x as f32 / p.scale) as i32;
    let ly = (y as f32 / p.scale) as i32;
    if ly < HEADER_H {
        return -1;
    }
    let mut idx = 0i32;
    match p.mode {
        Mode::Keyboard => {
            for (r, row) in keyboard_rows().iter().enumerate() {
                let mut col = 0i32;
                for k in row {
                    if in_rect(key_rect(r as i32, col, k.span()), lx, ly) {
                        return idx;
                    }
                    col += k.span();
                    idx += 1;
                }
            }
        }
        Mode::Symbol => {
            for (i, _) in SYM_SETS[p.sym_tab].iter().enumerate() {
                let r = (i / COLS as usize) as i32;
                if r >= ROWS_SYM {
                    break;
                }
                let c = (i % COLS as usize) as i32;
                if in_rect(key_rect(r, c, 1), lx, ly) {
                    return idx;
                }
                idx += 1;
            }
        }
    }
    -1
}

fn key_at(p: &Panel, idx: i32) -> Option<Key> {
    if idx < 0 {
        return None;
    }
    let mut i = 0i32;
    match p.mode {
        Mode::Keyboard => {
            for row in keyboard_rows() {
                for k in row {
                    if i == idx {
                        return Some(k);
                    }
                    i += 1;
                }
            }
        }
        Mode::Symbol => {
            for s in SYM_SETS[p.sym_tab].iter() {
                if i == idx {
                    return Some(Key::Symbol(s));
                }
                i += 1;
            }
        }
    }
    None
}

fn mem_pos_get() -> Option<(i32, i32)> {
    static POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    *POS.get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn mem_pos_set(v: (i32, i32)) {
    static POS: OnceLock<Mutex<Option<(i32, i32)>>> = OnceLock::new();
    *POS.get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(v);
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

fn create_hwnd(mode: Mode) -> HWND {
    unsafe {
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = WND_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        match CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
            PCWSTR(class_name.as_ptr()),
            w!("LingxiSoftKeyboard"),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            PANEL_W,
            mode.height(),
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
        let base = if small { 12.0 } else { 15.0 };
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
    fn new(mode: Mode) -> Self {
        let hwnd = if ensure_window_class() {
            create_hwnd(mode)
        } else {
            HWND(std::ptr::null_mut())
        };
        let scale = if hwnd.0.is_null() { 1.0 } else { dpi_scale(hwnd) };
        Self {
            hwnd,
            mode,
            sym_tab: 0,
            scale,
            hover: -1,
            pressed: -1,
            dragging: false,
            drag_off: (0, 0),
            dragged: false,
            font: make_font(scale, false),
            font_fn: make_font(scale, true),
        }
    }

    fn px(&self, v: i32) -> i32 {
        ((v as f32) * self.scale).round() as i32
    }

    fn win_size(&self) -> (i32, i32) {
        (self.px(PANEL_W), self.px(self.mode.height()))
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

    fn show(&mut self, mode: Mode, toggle_same: bool) {
        unsafe {
            let visible = IsWindowVisible(self.hwnd).as_bool();
            if toggle_same && visible && self.mode == mode {
                self.hide();
                return;
            }
        }
        self.refresh_dpi();
        self.mode = mode;
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

            // 拖动柄点阵（2×3）
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

            // 标题栏页签 + 关闭
            let (kb_r, sym_r, close_r) = header_rects();
            for (r, text, active) in [
                (kb_r, "键盘", self.mode == Mode::Keyboard),
                (sym_r, "符号", self.mode == Mode::Symbol),
            ] {
                let rr = sr(r);
                if active {
                    fill_round(mem, rr, px(6), C_ACTIVE);
                    draw_label(mem, rr, text, self.font_fn, 0x00FFFFFF);
                } else {
                    draw_label(mem, rr, text, self.font_fn, C_TITLE);
                }
            }
            draw_label(mem, sr(close_r), "✕", self.font_fn, C_TITLE);

            // 符号分类页签
            if self.mode == Mode::Symbol {
                for (i, name) in SYM_TABS.iter().enumerate() {
                    let rr = sr(sym_tab_rect(i));
                    if i == self.sym_tab {
                        fill_round(mem, rr, px(5), C_ACTIVE);
                        draw_label(mem, rr, name, self.font_fn, 0x00FFFFFF);
                    } else {
                        draw_label(mem, rr, name, self.font_fn, C_TITLE);
                    }
                }
            }

            // 键位
            let mut idx = 0i32;
            let draw_key = |r: (i32, i32, i32, i32), label: &str, small: bool, state: i32| {
                let rr = sr(r);
                let fill = match state {
                    2 => C_KEY_DOWN,
                    1 => C_KEY_HOVER,
                    _ => C_KEY,
                };
                fill_round(mem, rr, px(6), fill);
                stroke_round(mem, rr, px(6), C_BORDER);
                draw_label(
                    mem,
                    rr,
                    label,
                    if small { self.font_fn } else { self.font },
                    C_TEXT,
                );
            };
            match self.mode {
                Mode::Keyboard => {
                    for (ri, row) in keyboard_rows().iter().enumerate() {
                        let mut col = 0i32;
                        for k in row {
                            let state = if idx == self.pressed {
                                2
                            } else if idx == self.hover {
                                1
                            } else {
                                0
                            };
                            draw_key(key_rect(ri as i32, col, k.span()), &k.label(), k.is_fn(), state);
                            col += k.span();
                            idx += 1;
                        }
                    }
                }
                Mode::Symbol => {
                    for (i, sym) in SYM_SETS[self.sym_tab].iter().enumerate() {
                        let r = (i / COLS as usize) as i32;
                        if r >= ROWS_SYM {
                            break;
                        }
                        let c = (i % COLS as usize) as i32;
                        let state = if idx == self.pressed {
                            2
                        } else if idx == self.hover {
                            1
                        } else {
                            0
                        };
                        draw_key(key_rect(r, c, 1), sym, false, state);
                        idx += 1;
                    }
                }
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
            SC_SHOW => {
                p.show(Mode::from_wparam(wparam.0), true);
                LRESULT(0)
            }
            SC_HIDE => {
                p.hide();
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
                p.pressed = hit_key(p, mx, my);
                SetCapture(hwnd);
                InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let mx = (lparam.0 & 0xFFFF) as i32;
                let my = ((lparam.0 >> 16) & 0xFFFF) as i32;
                let s = p.scale;
                if p.dragging {
                    let (dx, dy) = p.drag_off;
                    let (cx, cy) = ((mx as f32 / s) as i32, (my as f32 / s) as i32);
                    if !p.dragged && (cx - dx).abs() + (cy - dy).abs() > 3 {
                        p.dragged = true;
                    }
                    if p.dragged {
                        let (mut x, mut y) = mem_pos_get().unwrap_or((0, 0));
                        x += cx - dx;
                        y += cy - dy;
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
                    let idx = hit_key(p, mx, my);
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
                    } else if ly < HEADER_H {
                        let (kb_r, sym_r, close_r) = header_rects();
                        if in_rect(close_r, lx, ly) {
                            p.hide();
                        } else if in_rect(kb_r, lx, ly) {
                            p.mode = Mode::Keyboard;
                            p.show(Mode::Keyboard, false);
                        } else if in_rect(sym_r, lx, ly) {
                            p.mode = Mode::Symbol;
                            p.show(Mode::Symbol, false);
                        }
                    } else if p.mode == Mode::Symbol
                        && (0..SYM_TABS.len()).any(|i| in_rect(sym_tab_rect(i), lx, ly))
                    {
                        for i in 0..SYM_TABS.len() {
                            if in_rect(sym_tab_rect(i), lx, ly) {
                                p.sym_tab = i;
                                break;
                            }
                        }
                    } else {
                        let idx = hit_key(p, mx, my);
                        if let Some(k) = key_at(p, idx) {
                            press(&k);
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
            .name("lingxi-softkeyboard".into())
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
        if !ensure_window_class() {
            return;
        }
        let hwnd = create_hwnd(Mode::Keyboard);
        if hwnd.0.is_null() {
            return;
        }
        let owner = OWNER.get_or_init(|| Mutex::new(None));
        *owner.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Box::new(Panel::new(Mode::Keyboard)));
        let ptr = owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|p| &**p as *const Panel as usize)
            .unwrap_or(0);
        set_window_data(hwnd, ptr);
        *PANEL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hwnd.0 as usize);
        // 建立窗口后按 DPI 定一次尺寸与圆角
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

/// 切换面板（同模式再点一次即隐藏）。mode: 1=软键盘 2=符号面板。
pub fn toggle(mode: u32) {
    if let Some(hwnd) = panel_hwnd() {
        unsafe {
            PostMessageW(Some(hwnd), SC_SHOW, WPARAM(mode as usize), LPARAM(0));
        }
    }
}

#[allow(dead_code)]
/// 隐藏面板（供将来从设置中心或快捷键收起时调用）。
pub fn hide() {
    if let Some(hwnd) = panel_hwnd() {
        unsafe {
            PostMessageW(Some(hwnd), SC_HIDE, WPARAM(0), LPARAM(0));
        }
    }
}

/// 面板当前是否可见。**不会**触发线程/窗口创建（供状态栏画高亮用）。
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
