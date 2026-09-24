//! 灵犀输入法设置中心 —— 自绘 UI（无 WebView、也不使用系统控件外观）。
//!
//! 思路与 WindInput 一致：窗口只有一个，所有内容自己画。
//! · 形状（圆角卡片/胶囊/圆形滑块/阴影）用 GDI+ 抗锯齿绘制；
//! · 文字用 GDI ClearType（中文清晰度比位图控件好）；
//! · 布局在每次重绘前算一遍（build_layout），绘制与命中测试共用同一份矩形，
//!   不会出现"看着在这、点着在那"。
//!
//! 之所以不用系统控件：原生按钮/复选框/下拉的外观由 Windows 决定（90 年代风格），
//! 想让界面好看只能自绘；自绘同时保持零额外依赖、内存十几 MB。

#![allow(clippy::too_many_arguments)]

use crate::core::{Core, DictStats};
use lingxi_rime::config::{AppearanceConfig, LingxiConfig};
use std::mem::size_of;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM, WIN32_ERROR,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT;
use windows::Win32::Graphics::GdiPlus::{
    FillModeWinding, GdipAddPathArcI, GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath,
    GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
    GdipDeletePen, GdipDrawPath, GdipFillEllipseI, GdipFillPath, GdipFillRectangleI,
    GdipSetSmoothingMode, GdiplusStartup, GdiplusStartupInput, GpGraphics, GpPath, GpPen,
    GpSolidFill, SmoothingModeAntiAlias, Status, UnitPixel,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, GetOpenFileNameW, CHOOSECOLORW, OPENFILENAMEW, CC_FULLOPEN, CC_RGBINIT,
    OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::w;

// ---------------- 设计令牌（看这里就能改整套观感）----------------

/// 主题令牌（亮色/深色由 theme_from 生成）。
#[derive(Clone)]
struct Theme {
    /// 窗口底（侧栏之外的背景）
    bg: u32,
    /// 侧栏底
    sidebar: u32,
    /// 卡片底
    card: u32,
    /// 卡片描边
    border: u32,
    /// 分隔线
    divider: u32,
    /// 主要文字
    text: u32,
    /// 次要文字
    text2: u32,
    /// 更淡的说明文字
    text3: u32,
    /// 悬停底色
    hover: u32,
    /// 按下底色
    pressed: u32,
    /// 控件底色（未激活的胶囊/输入框）
    field: u32,
    /// 强调色（取自用户配置的主色调）
    accent: u32,
    /// 强调色的浅底（选中项背景）
    accent_soft: u32,
    /// 深色模式
    dark: bool,
}

fn theme_from(cfg: &LingxiConfig, dark: bool) -> Theme {
    let accent = hex_argb(&cfg.appearance.accent, 0xFF2A6FDB);
    if dark {
        Theme {
            bg: 0xFF16181D,
            sidebar: 0xFF1C1F26,
            card: 0xFF23262E,
            border: 0xFF32363F,
            divider: 0xFF2C3039,
            text: 0xFFE9EAF0,
            text2: 0xFFA3A9B7,
            text3: 0xFF7B8291,
            hover: 0xFF2B2F38,
            pressed: 0xFF343946,
            field: 0xFF2A2E37,
            accent,
            accent_soft: mix_argb(accent, 0xFF23262E, 0.22),
            dark: true,
        }
    } else {
        Theme {
            bg: 0xFFF4F5F8,
            sidebar: 0xFFFFFFFF,
            card: 0xFFFFFFFF,
            border: 0xFFECEEF4,
            divider: 0xFFF1F3F7,
            text: 0xFF1F2430,
            text2: 0xFF6B7280,
            text3: 0xFF9AA1AF,
            hover: 0xFFF4F6FA,
            pressed: 0xFFE9EDF5,
            field: 0xFFF3F5F9,
            accent,
            accent_soft: mix_argb(accent, 0xFFFFFFFF, 0.12),
            dark: false,
        }
    }
}

/// 十六进制颜色（#RRGGBB）→ GDI+ 的 0xAARRGGBB。
fn hex_argb(s: &str, fallback: u32) -> u32 {
    let t = s.trim_start_matches('#');
    if t.len() != 6 {
        return fallback;
    }
    match u32::from_str_radix(t, 16) {
        Ok(v) => 0xFF00_0000 | v,
        Err(_) => fallback,
    }
}

fn argb_hex(v: u32) -> String {
    format!("#{:06X}", v & 0x00FF_FFFF)
}

/// 两个 ARGB 按比例混合（t=0 取 a，t=1 取 b）。
fn mix_argb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |shift: u32| {
        let x = ((a >> shift) & 0xFF) as f32;
        let y = ((b >> shift) & 0xFF) as f32;
        ((x + (y - x) * t).round() as u32) & 0xFF
    };
    0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn argb(a: u32, alpha: f32) -> u32 {
    let al = ((alpha.clamp(0.0, 1.0)) * 255.0).round() as u32;
    (al << 24) | (a & 0x00FF_FFFF)
}

// ---------------- 控件标识与命中区 ----------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Id {
    Nav(usize),
    ThemePreset(usize),
    ColorPick(usize),
    SkinExport,
    SkinImport,
    SchemaCombo,
    EnterCombo,
    ToggleCombo,
    SepCombo,
    SegFull,
    SegPunct,
    SegAscii,
    SwitchLearn,
    SwitchIndex,
    SwitchStatus,
    PageSize,
    FontSize,
    Opacity,
    Fuzzy(usize),
    PunctKey,
    PunctOut,
    PunctAdd,
    PunctDel,
    PunctRow(usize),
    DictImport,
    DictExport,
    BackupNow,
    Restore,
    BackupRow(usize),
    OpenData,
    OpenLogs,
    ClearLogs,
    ResetAll,
    Update,
    ComboItem(usize),
}

#[derive(Clone)]
struct Hit {
    id: Id,
    rect: RECT,
}

/// 下拉/输入框状态。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    None,
    PunctKey,
    PunctOut,
}

// ---------------- 应用状态 ----------------

const PAGES: [(&str, &str, &str); 4] = [
    ("⌨", "输入设置", "方案、按键行为、模糊音"),
    ("🎨", "外观主题", "配色、字号、候选窗预览"),
    ("📚", "词库管理", "短语导入导出、用户词库备份"),
    ("ⓘ", "关于", "版本、快捷键、诊断维护"),
];

struct App {
    hwnd: HWND,
    core: Core,
    cfg: LingxiConfig,
    saved: LingxiConfig,
    schemas: Vec<(String, String)>,
    stats: DictStats,
    backups: Vec<std::path::PathBuf>,
    page: usize,
    scroll: i32,
    hits: Vec<Hit>,
    hover: Option<Id>,
    pressed: Option<Id>,
    focus: Focus,
    punct_key: String,
    punct_out: String,
    punct_sel: Option<usize>,
    backup_sel: usize,
    combo_open: Option<Id>,
    dpi: u32,
    theme: Theme,
    font: HFONT,
    font_bold: HFONT,
    font_small: HFONT,
    font_title: HFONT,
    toast: String,
    toast_until: std::time::Instant,
    engine_ready: bool,
    dragging_slider: Option<Id>,
    gdiplus: usize,
    scroll_max: i32,
}

fn main_of(hwnd: HWND) -> Option<&'static mut App> {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if p == 0 {
            None
        } else {
            Some(&mut *(p as *mut App))
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------- GDI+ 绘制助手 ----------------

/// 一次绘制用的 GDI+ 画布（抗锯齿圆角、圆形、描边）。
struct Gfx {
    g: *mut GpGraphics,
}

impl Gfx {
    unsafe fn new(hdc: HDC) -> Option<Self> {
        let mut g: *mut GpGraphics = std::ptr::null_mut();
        if GdipCreateFromHDC(hdc, &mut g) != Status(0) || g.is_null() {
            return None;
        }
        let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        Some(Self { g })
    }

    /// 圆角矩形路径（四个角各一段 90° 弧）。
    unsafe fn round_path(&self, r: RECT, radius: i32) -> *mut GpPath {
        let mut path: *mut GpPath = std::ptr::null_mut();
        if GdipCreatePath(FillModeWinding, &mut path) != Status(0) {
            return std::ptr::null_mut();
        }
        let d = radius.max(0) * 2;
        let (x, y, w, h) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
        GdipAddPathArcI(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArcI(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArcI(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArcI(path, x, y + h - d, d, d, 90.0, 90.0);
        let _ = GdipClosePathFigure(path);
        path
    }

    unsafe fn fill_round(&self, r: RECT, radius: i32, color: u32) {
        if r.right <= r.left || r.bottom <= r.top {
            return;
        }
        if radius <= 0 {
            let mut brush: *mut GpSolidFill = std::ptr::null_mut();
            if GdipCreateSolidFill(color, &mut brush) == Status(0) && !brush.is_null() {
                let _ = GdipFillRectangleI(
                    self.g,
                    brush as *mut _,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                );
                let _ = GdipDeleteBrush(brush as *mut _);
            }
            return;
        }
        let path = self.round_path(r, radius);
        if path.is_null() {
            return;
        }
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        if GdipCreateSolidFill(color, &mut brush) == Status(0) && !brush.is_null() {
            let _ = GdipFillPath(self.g, brush as *mut _, path);
            let _ = GdipDeleteBrush(brush as *mut _);
        }
        let _ = GdipDeletePath(path);
    }

    unsafe fn stroke_round(&self, r: RECT, radius: i32, color: u32, width: f32) {
        if r.right <= r.left || r.bottom <= r.top {
            return;
        }
        let path = self.round_path(r, radius);
        if path.is_null() {
            return;
        }
        let mut pen: *mut GpPen = std::ptr::null_mut();
        if GdipCreatePen1(color, width, UnitPixel, &mut pen) == Status(0) && !pen.is_null() {
            let _ = GdipDrawPath(self.g, pen, path);
            let _ = GdipDeletePen(pen);
        }
        let _ = GdipDeletePath(path);
    }

    unsafe fn fill_circle(&self, cx: i32, cy: i32, radius: i32, color: u32) {
        let mut brush: *mut GpSolidFill = std::ptr::null_mut();
        if GdipCreateSolidFill(color, &mut brush) == Status(0) && !brush.is_null() {
            let _ = GdipFillEllipseI(
                self.g,
                brush as *mut _,
                cx - radius,
                cy - radius,
                radius * 2,
                radius * 2,
            );
            let _ = GdipDeleteBrush(brush as *mut _);
        }
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            if !self.g.is_null() {
                let _ = GdipDeleteGraphics(self.g);
            }
        }
    }
}

/// GDI 文字（ClearType，比 GDI+ 的中文渲染更清晰）。
unsafe fn text(
    dc: HDC,
    r: RECT,
    s: &str,
    font: HFONT,
    color: u32,
    align: DRAW_TEXT_FORMAT,
    vcenter: bool,
) {
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let _ = SetBkMode(dc, TRANSPARENT);
    let _ = SetTextColor(dc, COLORREF(color));
    let mut buf = wide(s);
    let mut rect = r;
    let mut flags = DRAW_TEXT_FORMAT(align.0 | DT_NOPREFIX.0);
    if vcenter {
        flags = DRAW_TEXT_FORMAT(flags.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
    } else {
        flags = DRAW_TEXT_FORMAT(flags.0 | DT_WORDBREAK.0);
    }
    DrawTextW(dc, &mut buf, &mut rect, flags);
    SelectObject(dc, old);
}

/// ARGB → COLORREF（GDI 文字用）。
fn argb_to_colorref(c: u32) -> u32 {
    let r = c & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = (c >> 16) & 0xFF;
    r | (g << 8) | (b << 16)
}

impl App {
    /// 构造：读取配置、初始化 GDI+ 与字体。
    fn create(hwnd: HWND) -> Box<App> {
        let core = Core::new();
        let cfg = core.load_config();
        let dark = system_prefers_dark();
        let theme = theme_from(&cfg, dark);
        let mut app = Box::new(App {
            hwnd,
            core,
            saved: cfg.clone(),
            cfg,
            schemas: Vec::new(),
            stats: DictStats::default(),
            backups: Vec::new(),
            page: 0,
            scroll: 0,
            hits: Vec::new(),
            hover: None,
            pressed: None,
            focus: Focus::None,
            punct_key: String::new(),
            punct_out: String::new(),
            punct_sel: None,
            backup_sel: 0,
            combo_open: None,
            dpi: 96,
            theme,
            font: HFONT::default(),
            font_bold: HFONT::default(),
            font_small: HFONT::default(),
            font_title: HFONT::default(),
            toast: String::new(),
            toast_until: std::time::Instant::now(),
            engine_ready: false,
            dragging_slider: None,
            gdiplus: 0,
            scroll_max: 0,
        });
        unsafe {
            app.dpi = GetDpiForWindow(hwnd).max(96);
            let mut token = 0usize;
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: windows::core::BOOL(0),
                SuppressExternalCodecs: windows::core::BOOL(0),
            };
            let _ = GdiplusStartup(&mut token, &input, std::ptr::null_mut());
            app.gdiplus = token;
        }
        app.rebuild_fonts();
        app
    }

    // ---------------- 绘制：控件 ----------------

    fn push_hit(&mut self, id: Id, r: RECT) {
        self.hits.push(Hit { id, rect: r });
    }

    fn hit_test(&self, x: i32, y: i32) -> Option<Id> {
        // 后画的在上层（下拉菜单在最后），故倒序查找
        self.hits
            .iter()
            .rev()
            .find(|h| x >= h.rect.left && x < h.rect.right && y >= h.rect.top && y < h.rect.bottom)
            .map(|h| h.id)
    }

    /// 卡片：返回内容区左上角。
    fn card(&mut self, dc: HDC, x: i32, y: i32, w: i32, h: i32, title: &str, _desc: &str) -> i32 {
        let t = self.theme.clone();
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                // 轻投影：两层半透明圆角
                g.fill_round(
                    RECT { left: r.left, top: r.top + 2, right: r.right, bottom: r.bottom + 2 },
                    12,
                    argb(0x00101828, if t.dark { 0.35 } else { 0.06 }),
                );
                g.fill_round(r, 12, t.card);
                g.stroke_round(r, 12, t.border, 1.0);
            }
            let mut ty = y + 14;
            let tr = RECT { left: x + 18, top: ty, right: x + w - 18, bottom: ty + 22 };
            text(dc, tr, title, self.font_bold, t.text, DT_LEFT, true);
            let _ = ty;
        }
        y + 14 + 24 + self.s(4)
    }

    /// 行标签（左） + 右侧控件；返回行高。
    fn row(&mut self, dc: HDC, x: i32, y: i32, w: i32, label: &str, desc: &str) -> RECT {
        let t = self.theme.clone();
        unsafe {
            let lr = RECT { left: x, top: y + 6, right: x + w / 2, bottom: y + 26 };
            text(dc, lr, label, self.font, t.text, DT_LEFT, true);
            let _ = desc;
        }
        RECT { left: x + w / 2, top: y + 4, right: x + w, bottom: y + 40 }
    }

    fn toggle_switch(&mut self, dc: HDC, id: Id, r: RECT, on: bool) {
        let t = self.theme.clone();
        let w = self.s(44);
        let h = self.s(24);
        let x = r.right - w;
        let y = r.top + ((r.bottom - r.top) - h) / 2;
        let rr = RECT { left: x, top: y, right: x + w, bottom: y + h };
        let hot = self.hover == Some(id) || self.pressed == Some(id);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                let base = if on { t.accent } else if t.dark { 0xFF3A3F4A } else { 0xFFD8DCE5 };
                g.fill_round(rr, h / 2, if hot { mix_argb(base, 0xFFFFFFFF, 0.06) } else { base });
                let kx = if on { x + w - h / 2 } else { x + h / 2 };
                g.fill_circle(kx, y + h / 2, h / 2 - self.s(3), 0xFFFFFFFF);
            }
        }
        self.push_hit(id, rr);
    }

    fn segmented(&mut self, dc: HDC, id: Id, r: RECT, options: &[&str], sel: usize) {
        let t = self.theme.clone();
        let w = r.right - r.left;
        let n = options.len().max(1) as i32;
        let seg = w / n;
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 8, t.field);
                for (i, _) in options.iter().enumerate() {
                    if i == sel {
                        let sr = RECT {
                            left: r.left + (i as i32) * seg + 2,
                            top: r.top + 2,
                            right: r.left + ((i as i32) + 1) * seg - 2,
                            bottom: r.bottom - 2,
                        };
                        g.fill_round(sr, 6, t.card);
                        if !t.dark {
                            g.stroke_round(sr, 6, t.border, 1.0);
                        }
                    }
                }
            }
            for (i, name) in options.iter().enumerate() {
                let lr = RECT {
                    left: r.left + (i as i32) * seg,
                    top: r.top,
                    right: r.left + ((i as i32) + 1) * seg,
                    bottom: r.bottom,
                };
                let color = if i == sel { t.accent } else { t.text2 };
                text(dc, lr, name, self.font_small, color, DT_CENTER, true);
            }
        }
        // 每段各注册一个命中区，用 ComboItem(id 变体) 区分
        for i in 0..options.len() {
            let lr = RECT {
                left: r.left + (i as i32) * seg,
                top: r.top,
                right: r.left + ((i as i32) + 1) * seg,
                bottom: r.bottom,
            };
            let sub = match id {
                Id::SegFull => Id::ComboItem(100 + i),
                Id::SegPunct => Id::ComboItem(200 + i),
                Id::SegAscii => Id::ComboItem(300 + i),
                _ => id,
            };
            self.push_hit(sub, lr);
        }
    }

    fn slider(&mut self, dc: HDC, id: Id, r: RECT, value: i32, min: i32, max: i32, suffix: &str) {
        let t = self.theme.clone();
        let track_h = self.s(4);
        let knob = self.s(13);
        let w = r.right - r.left;
        let frac = ((value - min) as f32 / (max - min).max(1) as f32).clamp(0.0, 1.0);
        let cx = r.left + (frac * (w - knob) as f32) as i32 + knob / 2;
        let cy = (r.top + r.bottom) / 2;
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                let tr = RECT {
                    left: r.left,
                    top: cy - track_h / 2,
                    right: r.right,
                    bottom: cy + track_h / 2,
                };
                g.fill_round(tr, track_h / 2, if t.dark { 0xFF3A3F4A } else { 0xFFE4E8F0 });
                let fr = RECT { left: tr.left, top: tr.top, right: cx, bottom: tr.bottom };
                g.fill_round(fr, track_h / 2, t.accent);
                g.fill_circle(cx, cy, knob / 2, 0xFFFFFFFF);
                g.stroke_round(
                    RECT {
                        left: cx - knob / 2,
                        top: cy - knob / 2,
                        right: cx + knob / 2,
                        bottom: cy + knob / 2,
                    },
                    knob / 2,
                    if self.dragging_slider == Some(id) { t.accent } else { argb(0x00202838, 0.22) },
                    if self.dragging_slider == Some(id) { 2.0 } else { 1.0 },
                );
            }
            let vr = RECT { left: r.right + self.s(10), top: r.top, right: r.right + self.s(70), bottom: r.bottom };
            text(
                dc,
                vr,
                &format!("{value}{suffix}"),
                self.font_small,
                t.text2,
                DT_LEFT,
                true,
            );
        }
        self.push_hit(id, RECT { left: r.left - knob / 2, top: r.top - self.s(4), right: r.right + knob / 2, bottom: r.bottom + self.s(4) });
    }

    fn combo(&mut self, dc: HDC, id: Id, r: RECT, value: &str) {
        let t = self.theme.clone();
        let hot = self.hover == Some(id) || self.combo_open == Some(id);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 8, if hot { t.hover } else { t.field });
                g.stroke_round(r, 8, if hot { t.accent } else { t.border }, 1.0);
            }
            let tr = RECT { left: r.left + 12, top: r.top, right: r.right - 26, bottom: r.bottom };
            text(dc, tr, value, self.font, t.text, DT_LEFT, true);
            // 展开箭头
            let (cx, cy) = (r.right - 16, (r.top + r.bottom) / 2);
            if let Some(g) = Gfx::new(dc) {
                let mut path: *mut GpPath = std::ptr::null_mut();
                gdi_polyline(dc, &[(cx - 5, cy - 2), (cx, cy + 3), (cx + 5, cy - 2)], t.text3, 2);
            }
        }
        self.push_hit(id, r);
    }

    fn button(&mut self, dc: HDC, id: Id, r: RECT, label: &str, primary: bool) {
        let t = self.theme.clone();
        let hot = self.hover == Some(id);
        let down = self.pressed == Some(id);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                let (bg, fg) = if primary {
                    (
                        if down { mix_argb(t.accent, 0x00000000, 0.18) } else if hot { mix_argb(t.accent, 0xFFFFFFFF, 0.12) } else { t.accent },
                        0xFFFFFFFF,
                    )
                } else {
                    (
                        if down { t.pressed } else if hot { t.hover } else { t.card },
                        t.text,
                    )
                };
                g.fill_round(r, 8, bg);
                if !primary {
                    g.stroke_round(r, 8, t.border, 1.0);
                }
                let _ = fg;
            }
            text(
                dc,
                r,
                label,
                self.font,
                if primary { 0xFFFFFFFF } else { t.text },
                DT_CENTER,
                true,
            );
        }
        self.push_hit(id, r);
    }

    fn checkbox(&mut self, dc: HDC, id: Id, r: RECT, label: &str, checked: bool) {
        let t = self.theme.clone();
        let box_sz = self.s(17);
        let br = RECT {
            left: r.left,
            top: r.top + ((r.bottom - r.top) - box_sz) / 2,
            right: r.left + box_sz,
            bottom: r.top + ((r.bottom - r.top) - box_sz) / 2 + box_sz,
        };
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                if checked {
                    g.fill_round(br, 5, t.accent);
                    // 对勾
                    let (cx, cy) = ((br.left + br.right) / 2, (br.top + br.bottom) / 2);
                    let _ = g;
                    gdi_polyline(
                        dc,
                        &[
                            (cx - self.s(4), cy),
                            (cx - 1, cy + self.s(3)),
                            (cx + self.s(4), cy - self.s(3)),
                        ],
                        0xFFFFFFFF,
                        2,
                    );
                } else {
                    g.fill_round(br, 5, if t.dark { 0xFF2E323C } else { 0xFFFFFFFF });
                    g.stroke_round(br, 5, if self.hover == Some(id) { t.accent } else { argb(0x00606878, 0.45) }, 1.0);
                }
            }
            let lr = RECT { left: br.right + self.s(9), top: r.top, right: r.right, bottom: r.bottom };
            text(dc, lr, label, self.font_small, t.text, DT_LEFT, true);
        }
        self.push_hit(id, r);
    }

    fn chip(&mut self, dc: HDC, id: Id, r: RECT, color: u32) {
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 6, color);
                g.stroke_round(r, 6, argb(0x00202838, 0.25), 1.0);
            }
        }
        self.push_hit(id, r);
    }

    fn text_field(&mut self, dc: HDC, id: Id, r: RECT, value: &str, placeholder: &str) {
        let t = self.theme.clone();
        let focused = (id == Id::PunctKey && self.focus == Focus::PunctKey)
            || (id == Id::PunctOut && self.focus == Focus::PunctOut);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 8, t.field);
                g.stroke_round(r, 8, if focused { t.accent } else { t.border }, 1.0);
            }
            let tr = RECT { left: r.left + 10, top: r.top, right: r.right - 8, bottom: r.bottom };
            if value.is_empty() && !focused {
                text(dc, tr, placeholder, self.font_small, t.text3, DT_LEFT, true);
            } else {
                text(dc, tr, value, self.font, t.text, DT_LEFT, true);
                if focused {
                    // 光标
                    let w = App::text_width_of(dc, value, self.font);
                    let cx = tr.left + w + 1;
                    if let Some(g) = Gfx::new(dc) {
                        let cr = RECT { left: cx, top: r.top + 5, right: cx + self.s(2), bottom: r.bottom - 5 };
                        g.fill_round(cr, 1, t.accent);
                    }
                }
            }
        }
        self.push_hit(id, r);
    }

    fn list(&mut self, dc: HDC, id_of_row: impl Fn(usize) -> Id, r: RECT, items: &[String], selected: Option<usize>, empty: &str) {
        let t = self.theme.clone();
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 8, t.field);
                g.stroke_round(r, 8, t.border, 1.0);
            }
        }
        if items.is_empty() {
            let er = RECT { left: r.left + 12, top: r.top, right: r.right - 12, bottom: r.bottom };
            unsafe {
                text(dc, er, empty, self.font_small, t.text3, DT_LEFT, true);
            }
            return;
        }
        let row_h = self.s(26);
        for (i, it) in items.iter().enumerate() {
            let top = r.top + 4 + (i as i32) * row_h;
            if top + row_h > r.bottom - 4 {
                break;
            }
            let rr = RECT { left: r.left + 4, top, right: r.right - 4, bottom: top + row_h };
            let rid = id_of_row(i);
            let is_sel = selected == Some(i);
            let hot = self.hover == Some(rid);
            unsafe {
                if let Some(g) = Gfx::new(dc) {
                    if is_sel {
                        g.fill_round(rr, 6, t.accent_soft);
                    } else if hot {
                        g.fill_round(rr, 6, t.hover);
                    }
                }
                let tr = RECT { left: rr.left + 8, top: rr.top, right: rr.right - 8, bottom: rr.bottom };
                text(dc, tr, it, self.font_small, if is_sel { t.accent } else { t.text }, DT_LEFT, true);
            }
            self.push_hit(rid, rr);
        }
    }

unsafe fn text_width_of(dc: HDC, s: &str, font: HFONT) -> i32 {
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let mut buf = wide(s);
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &buf[..buf.len() - 1], &mut sz);
    SelectObject(dc, old);
    sz.cx
}
    fn s(&self, v: i32) -> i32 {
        ((v as f32) * (self.dpi as f32 / 96.0)).round() as i32
    }

    fn rebuild_fonts(&mut self) {
        unsafe {
            for f in [self.font, self.font_bold, self.font_small, self.font_title] {
                if !f.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(f.0));
                }
            }
            let name = wide("Microsoft YaHei UI");
            let mk = |px: i32, weight: i32| {
                CreateFontW(
                    -((px * self.dpi as i32 / 96).max(11)),
                    0,
                    0,
                    0,
                    weight,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_TT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    PCWSTR(name.as_ptr()),
                )
            };
            self.font = mk(14, FW_NORMAL.0 as i32);
            self.font_bold = mk(14, FW_SEMIBOLD.0 as i32);
            self.font_small = mk(12, FW_NORMAL.0 as i32);
            self.font_title = mk(17, FW_SEMIBOLD.0 as i32);
        }
    }

    fn toast(&mut self, msg: &str) {
        self.toast = msg.to_string();
        self.toast_until = std::time::Instant::now() + std::time::Duration::from_secs(6);
        unsafe {
            let _ = SetTimer(Some(self.hwnd), 2, 6000, None);
        }
    }

    fn mark_dirty(&mut self) {
        unsafe {
            let _ = SetTimer(Some(self.hwnd), 1, 400, None);
        }
    }

    fn save_now(&mut self) {
        let cfg = self.cfg.clone();
        let old = self.saved.clone();
        match self.core.save_config(&cfg, &old) {
            Ok(needs_redeploy) => {
                self.saved = cfg;
                if needs_redeploy {
                    self.core.redeploy();
                    self.toast("已保存，正在重建词库索引…");
                } else {
                    self.toast("设置已保存");
                }
            }
            Err(e) => self.toast(&format!("保存失败：{e}")),
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

// ---------------- 选项表与小工具 ----------------

/// 模糊音组（id 必须与 lingxi-rime::config::FUZZY_GROUPS 一致）。
const FUZZY_LABELS: [(&str, &str); 11] = [
    ("zh_z", "zh ↔ z"),
    ("ch_c", "ch ↔ c"),
    ("sh_s", "sh ↔ s"),
    ("n_l", "n ↔ l"),
    ("f_h", "f ↔ h"),
    ("r_l", "r ↔ l"),
    ("an_ang", "an ↔ ang"),
    ("en_eng", "en ↔ eng"),
    ("in_ing", "in ↔ ing"),
    ("ian_iang", "ian ↔ iang"),
    ("uan_uang", "uan ↔ uang"),
];

const COLOR_LABELS: [&str; 7] = [
    "主色调", "背景色", "文字颜色", "选中背景", "选中文字", "注释颜色", "边框颜色",
];

const ENTER_ITEMS: [(&str, &str); 3] = [
    ("commit", "上屏原始拼音"),
    ("clear", "丢弃当前输入"),
    ("newline", "丢弃并换行"),
];
const TOGGLE_ITEMS: [(&str, &str); 3] = [
    ("shift", "Shift"),
    ("ctrl_space", "Ctrl + 空格"),
    ("none", "不切换"),
];
const SEP_ITEMS: [(&str, &str); 5] = [
    ("'", "撇号  ni'hao"),
    ("-", "连字符  ni-hao"),
    (",", "逗号  ni,hao"),
    (" ", "空格  ni hao"),
    ("", "不显示  nihao"),
];

fn find_label(items: &[(&str, &str)], key: &str) -> String {
    items
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_else(|| key.to_string())
}

fn enter_text(app: &App) -> String {
    find_label(&ENTER_ITEMS, &app.cfg.input.enter_behavior)
}
fn toggle_text(app: &App) -> String {
    find_label(&TOGGLE_ITEMS, &app.cfg.input.toggle_key)
}
fn sep_text(app: &App) -> String {
    let v = app.cfg.input.pinyin_separator.as_str();
    find_label(&SEP_ITEMS, v)
}

/// 颜色项（与 COLOR_LABELS 同序）。
fn color_of(app: &App, i: usize) -> u32 {
    let a = &app.cfg.appearance;
    match i {
        0 => hex_argb(&a.accent, 0xFF2A6FDB),
        1 => hex_argb(&a.bg, 0xFFFFFFFF),
        2 => hex_argb(&a.text, 0xFF222222),
        3 => hex_argb(&a.selected_bg, 0xFF2A6FDB),
        4 => hex_argb(&a.selected_text, 0xFFFFFFFF),
        5 => hex_argb(&a.comment, 0xFF999999),
        _ => hex_argb(&a.border, 0xFFD8DCE3),
    }
}

fn set_color(app: &mut App, i: usize, v: u32) {
    let hex = argb_hex(v);
    let a = &mut app.cfg.appearance;
    match i {
        0 => {
            a.accent = hex.clone();
            a.selected_bg = hex;
            a.theme = "custom".into();
        }
        1 => {
            a.bg = hex;
            a.theme = "custom".into();
        }
        2 => {
            a.text = hex;
            a.theme = "custom".into();
        }
        3 => a.selected_bg = hex,
        4 => a.selected_text = hex,
        5 => a.comment = hex,
        _ => a.border = hex,
    }
    app.theme = theme_from(&app.cfg, app.theme.dark);
}

fn apply_theme(app: &mut App, which: usize) {
    let a = &mut app.cfg.appearance;
    match which {
        0 => {
            a.theme = "light".into();
            a.bg = "#FFFFFF".into();
            a.text = "#222222".into();
            a.accent = "#2A6FDB".into();
            a.selected_bg = "#2A6FDB".into();
            a.selected_text = "#FFFFFF".into();
            a.comment = "#999999".into();
            a.border = "#D8DCE3".into();
        }
        1 => {
            a.theme = "dark".into();
            a.bg = "#202127".into();
            a.text = "#E9E9EC".into();
            a.accent = "#5B93F2".into();
            a.selected_bg = "#3A5FA8".into();
            a.selected_text = "#FFFFFF".into();
            a.comment = "#9AA1B2".into();
            a.border = "#3A3D46".into();
        }
        _ => {
            a.theme = "custom".into();
            a.bg = "#F4F8F1".into();
            a.text = "#2C3B2A".into();
            a.accent = "#3E8E41".into();
            a.selected_bg = "#3E8E41".into();
            a.selected_text = "#FFFFFF".into();
            a.comment = "#7C9A79".into();
            a.border = "#C6DCC2".into();
        }
    }
    app.theme = theme_from(&app.cfg, app.theme.dark);
}

/// 系统是否偏好深色（AppsUseLightTheme = 0 即深色）。
fn system_prefers_dark() -> bool {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ,
        REG_DWORD,
    };
    unsafe {
        let mut hkey = Default::default();
        let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_READ, &mut hkey).is_err()
        {
            return false;
        }
        let name = wide("AppsUseLightTheme");
        let mut kind = REG_DWORD;
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let ok = RegQueryValueExW(
            hkey,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        ) == WIN32_ERROR(0);
        let _ = RegCloseKey(hkey);
        ok && data == 0
    }
}
// ---------------- 预览 / 下拉浮层 / 交互 ----------------

impl App {
    fn punct_sel(&self) -> Option<usize> {
        self.punct_sel
    }

    /// 候选窗预览：桌面底 + 圆角候选窗（首条高亮）。尺寸固定，字号大时少显示几行。
    fn draw_candidate_preview(&self, dc: HDC, r: RECT) {
        let a = &self.cfg.appearance;
        let t = &self.theme;
        let s = |v: i32| self.s(v);
        let dpi_scale = self.dpi as f32 / 96.0;
        let text_px = ((a.font_size as f32) * 4.0 / 3.0 * dpi_scale).round() as i32;
        let row_h = (text_px + s(14)).max(s(24));
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 10, if t.dark { 0xFF1B1D22 } else { 0xFFF0F2F6 });
            }
            let w = (r.right - r.left - s(36)).clamp(s(160), s(360));
            let box_h = (r.bottom - r.top - s(24)).max(s(80));
            let max_rows = (((box_h - s(12)) / row_h).max(1)).min(5);
            let h = row_h * max_rows + s(12);
            let l = r.left + (r.right - r.left - w) / 2;
            let top0 = r.top + (r.bottom - r.top - h) / 2;
            let rr = RECT { left: l, top: top0, right: l + w, bottom: top0 + h };
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(rr, a.corner_radius.min(16) as i32, hex_argb(&a.bg, 0xFFFFFFFF));
                g.stroke_round(rr, a.corner_radius.min(16) as i32, hex_argb(&a.border, 0xFFD8DCE3), 1.0);
            }
            let name = wide("Microsoft YaHei UI");
            let mk = |px: i32| {
                CreateFontW(
                    -px.max(9),
                    0, 0, 0,
                    FW_NORMAL.0 as i32,
                    0, 0, 0,
                    DEFAULT_CHARSET,
                    OUT_TT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    PCWSTR(name.as_ptr()),
                )
            };
            let font = mk(text_px);
            let idx_font = mk((text_px * 4 / 5).max(9));
            let rgn = CreateRoundRectRgn(rr.left + 1, rr.top + 1, rr.right, rr.bottom, s(10), s(10));
            let _ = SelectClipRgn(dc, Some(rgn));
            for (i, label) in ["你", "您", "牛", "鸟", "捏"].iter().take(max_rows as usize).enumerate() {
                let top = rr.top + s(6) + (i as i32) * row_h;
                if i == 0 {
                    if let Some(g) = Gfx::new(dc) {
                        g.fill_round(
                            RECT { left: rr.left + 1, top, right: rr.right - 1, bottom: top + row_h },
                            0,
                            hex_argb(&a.selected_bg, 0xFF2A6FDB),
                        );
                    }
                }
                let color = if i == 0 {
                    hex_argb(&a.selected_text, 0xFFFFFFFF)
                } else {
                    hex_argb(&a.text, 0xFF222222)
                };
                let mut xt = rr.left + s(12);
                if a.show_index {
                    let ir = RECT { left: xt, top, right: xt + s(26), bottom: top + row_h };
                    text(dc, ir, &format!("{}.", i + 1), idx_font, argb_to_colorref(hex_argb(&a.comment, 0xFF999999)), DT_LEFT, true);
                    xt += s(24);
                }
                let tr = RECT { left: xt, top, right: rr.right - s(8), bottom: top + row_h };
                text(dc, tr, label, font, argb_to_colorref(color), DT_LEFT, true);
            }
            let _ = SelectClipRgn(dc, None);
            let _ = DeleteObject(HGDIOBJ(rgn.0));
            let _ = DeleteObject(HGDIOBJ(font.0));
            let _ = DeleteObject(HGDIOBJ(idx_font.0));
        }
    }

    /// 展开的下拉：在控件下方画列表（同一窗口内浮层，命中区最后登记 → 优先命中）。
    fn draw_combo_popup(&mut self, dc: HDC) {
        let Some(open) = self.combo_open else { return };
        let Some(items) = combo_items(self, open) else { return };
        let Some(anchor) = self.hits.iter().find(|h| h.id == open).map(|h| h.rect) else {
            return;
        };
        let t = self.theme.clone();
        let row_h = self.s(30);
        let h = row_h * items.len() as i32 + self.s(10);
        let r = RECT {
            left: anchor.left,
            top: anchor.bottom + self.s(4),
            right: anchor.right,
            bottom: anchor.bottom + self.s(4) + h,
        };
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(RECT { left: r.left, top: r.top + 2, right: r.right, bottom: r.bottom + 2 }, 10, argb(0x00101828, if t.dark { 0.5 } else { 0.10 }));
                g.fill_round(r, 10, t.card);
                g.stroke_round(r, 10, t.border, 1.0);
            }
            for (i, (_, label)) in items.iter().enumerate() {
                let rr = RECT {
                    left: r.left + self.s(5),
                    top: r.top + self.s(5) + (i as i32) * row_h,
                    right: r.right - self.s(5),
                    bottom: r.top + self.s(5) + ((i as i32) + 1) * row_h,
                };
                let id = Id::ComboItem(i);
                let hot = self.hover == Some(id);
                if hot {
                    if let Some(g) = Gfx::new(dc) {
                        g.fill_round(rr, 6, t.hover);
                    }
                }
                text(dc, RECT { left: rr.left + 10, top: rr.top, right: rr.right - 8, bottom: rr.bottom }, label, self.font, t.text, DT_LEFT, true);
                self.hits.push(Hit { id, rect: rr });
            }
        }
    }

    fn open_color(&mut self, i: usize) {
        unsafe {
            let mut custom = [COLORREF(0x00FF_FFFF); 16];
            let mut cc = CHOOSECOLORW {
                lStructSize: size_of::<CHOOSECOLORW>() as u32,
                hwndOwner: self.hwnd,
                rgbResult: COLORREF(argb_to_colorref(color_of(self, i))),
                lpCustColors: custom.as_mut_ptr(),
                Flags: CC_FULLOPEN | CC_RGBINIT,
                ..Default::default()
            };
            if ChooseColorW(&mut cc).as_bool() {
                let v = cc.rgbResult.0;
                let argb = 0xFF00_0000 | ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF);
                set_color(self, i, argb);
                self.mark_dirty();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    /// 选中项激活（按钮、复选、下拉项、导航……）。
    fn activate(&mut self, id: Id) {
        match id {
            Id::Nav(i) => {
                self.page = i;
                self.scroll = 0;
            }
            Id::SchemaCombo | Id::EnterCombo | Id::ToggleCombo | Id::SepCombo => {
                self.combo_open = if self.combo_open == Some(id) { None } else { Some(id) };
            }
            Id::ComboItem(i) => {
                let open = self.combo_open;
                self.combo_open = None;
                match open {
                    Some(Id::SchemaCombo) => {
                        if let Some((sid, _)) = self.schemas.get(i) {
                            self.cfg.input.schema = sid.clone();
                            self.mark_dirty();
                        }
                    }
                    Some(Id::EnterCombo) => {
                        if let Some((k, _)) = ENTER_ITEMS.get(i) {
                            self.cfg.input.enter_behavior = (*k).into();
                            self.mark_dirty();
                        }
                    }
                    Some(Id::ToggleCombo) => {
                        if let Some((k, _)) = TOGGLE_ITEMS.get(i) {
                            self.cfg.input.toggle_key = (*k).into();
                            self.mark_dirty();
                        }
                    }
                    Some(Id::SepCombo) => {
                        if let Some((k, _)) = SEP_ITEMS.get(i) {
                            self.cfg.input.pinyin_separator = (*k).into();
                            self.mark_dirty();
                        }
                    }
                    _ => {}
                }
            }
            Id::SegFull => {}
            Id::SegPunct => {}
            Id::SegAscii => {}
            Id::SwitchLearn => {
                self.cfg.input.enable_user_dict = !self.cfg.input.enable_user_dict;
                self.mark_dirty();
            }
            Id::SwitchIndex => {
                self.cfg.appearance.show_index = !self.cfg.appearance.show_index;
                self.mark_dirty();
            }
            Id::SwitchStatus => {
                self.cfg.appearance.show_status_bar = !self.cfg.appearance.show_status_bar;
                self.mark_dirty();
            }
            Id::Fuzzy(i) => {
                let gid = FUZZY_LABELS[i].0;
                let list = &mut self.cfg.input.fuzzy;
                if let Some(pos) = list.iter().position(|f| f == gid) {
                    list.remove(pos);
                } else {
                    list.push(gid.to_string());
                }
                self.mark_dirty();
            }
            Id::PunctKey => self.focus = Focus::PunctKey,
            Id::PunctOut => self.focus = Focus::PunctOut,
            Id::PunctRow(i) => self.punct_sel = Some(i),
            Id::PunctAdd => {
                let k = self.punct_key.trim().to_string();
                let o = self.punct_out.trim().to_string();
                if k.is_empty() || o.is_empty() {
                    self.toast("请先填写「按键」与「上屏文本」");
                } else {
                    if let Some(p) = self.cfg.input.punct_map.iter_mut().find(|p| p.key == k) {
                        p.out = o;
                    } else {
                        self.cfg.input.punct_map.push(lingxi_rime::config::PunctPair { key: k, out: o });
                    }
                    self.punct_key.clear();
                    self.punct_out.clear();
                    self.mark_dirty();
                    self.toast("已添加自定义标点");
                }
            }
            Id::PunctDel => {
                if let Some(i) = self.punct_sel {
                    if i < self.cfg.input.punct_map.len() {
                        self.cfg.input.punct_map.remove(i);
                        self.punct_sel = None;
                        self.mark_dirty();
                        self.toast("已删除该条");
                    }
                } else {
                    self.toast("先在列表里选中一条");
                }
            }
            Id::ThemePreset(i) => {
                apply_theme(self, i);
                self.mark_dirty();
                self.toast("已套用主题预设");
            }
            Id::ColorPick(i) => self.open_color(i),
            Id::SkinExport => match self.core.export_skin() {
                Ok(p) => self.toast(&format!("已导出皮肤：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::SkinImport => {
                if let Some(path) = pick_file(self.hwnd, &[("皮肤文件", "*.json"), ("所有文件", "*.*")], "json") {
                    match self.core.import_skin(&path) {
                        Ok(cfg) => {
                            self.cfg = cfg;
                            self.theme = theme_from(&self.cfg, self.theme.dark);
                            self.mark_dirty();
                            self.toast("皮肤已套用");
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::DictImport => {
                if let Some(path) = pick_file(self.hwnd, &[("短语文件", "*.txt"), ("所有文件", "*.*")], "txt") {
                    match self.core.import_dict_file(&path) {
                        Ok(stats) => {
                            self.stats = stats;
                            self.toast(&self.stats.last_import.clone());
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::DictExport => match self.core.export_dict() {
                Ok(p) => self.toast(&format!("已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::BackupNow => match self.core.backup_user_dict() {
                Ok(p) => {
                    self.backups = self.core.list_backups();
                    self.toast(&format!("已备份到：{}", p.display()));
                }
                Err(e) => self.toast(&format!("备份失败：{e}")),
            },
            Id::BackupRow(i) => self.backup_sel = i,
            Id::Restore => {
                let backups = self.core.list_backups();
                if let Some(src) = backups.get(self.backup_sel).cloned() {
                    match self.core.restore_user_dict(&src) {
                        Ok(m) => self.toast(&m),
                        Err(e) => self.toast(&format!("恢复失败：{e}")),
                    }
                } else {
                    self.toast("还没有可恢复的备份");
                }
            }
            Id::OpenData => {
                if let Err(e) = self.core.open_data_dir() {
                    self.toast(&format!("打开失败：{e}"));
                }
            }
            Id::OpenLogs => {
                if let Err(e) = self.core.open_log_dir() {
                    self.toast(&format!("打开失败：{e}"));
                }
            }
            Id::ClearLogs => {
                let freed = self.core.clear_logs();
                self.toast(&format!("已清空日志（{} KB）", freed / 1024));
            }
            Id::ResetAll => {
                self.cfg = self.core.reset_config();
                self.theme = theme_from(&self.cfg, self.theme.dark);
                self.toast("已恢复默认设置（正在重建索引）");
            }
            Id::Update => unsafe {
                let url = wide("https://github.com/itsypa/lingxi-ime/releases/latest");
                let op = wide("open");
                ShellExecuteW(Some(self.hwnd), PCWSTR(op.as_ptr()), PCWSTR(url.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            },
            _ => {}
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

/// 下拉项来源。
fn combo_items(app: &App, id: Id) -> Option<Vec<(String, String)>> {
    Some(match id {
        Id::SchemaCombo => app
            .schemas
            .iter()
            .map(|(i, n)| (i.clone(), n.clone()))
            .collect(),
        Id::EnterCombo => ENTER_ITEMS.iter().map(|(k, l)| ((*k).to_string(), (*l).to_string())).collect(),
        Id::ToggleCombo => TOGGLE_ITEMS.iter().map(|(k, l)| ((*k).to_string(), (*l).to_string())).collect(),
        Id::SepCombo => SEP_ITEMS.iter().map(|(k, l)| ((*k).to_string(), (*l).to_string())).collect(),
        _ => return None,
    })
}

/// 文件选择框。
fn pick_file(hwnd: HWND, filter: &[(&str, &str)], def_ext: &str) -> Option<std::path::PathBuf> {
    unsafe {
        let mut f: Vec<u16> = Vec::new();
        for (name, pat) in filter {
            f.extend(name.encode_utf16());
            f.push(0);
            f.extend(pat.encode_utf16());
            f.push(0);
        }
        f.push(0);
        let ext = wide(def_ext);
        let mut buf = [0u16; 1024];
        let mut ofn = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: hwnd,
            lpstrFilter: PCWSTR(f.as_ptr()),
            lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            lpstrDefExt: PCWSTR(ext.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
            Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
        } else {
            None
        }
    }
}

// ---------------- 窗口过程与入口 ----------------

const MSG_DEFERRED: u32 = WM_APP + 1;
const TIMER_SAVE: usize = 1;
const TIMER_TOAST: usize = 2;
const TIMER_ENGINE: usize = 3;

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    }
}

/// 拖动滑块：按 x 反算取值。
fn drag_slider(app: &mut App, id: Id, x: i32) {
    let Some(rect) = app.hits.iter().find(|h| h.id == id).map(|h| h.rect) else {
        return;
    };
    let knob = app.s(13);
    let span = (rect.right - rect.left - knob).max(1);
    let frac = ((x - rect.left - knob / 2) as f32 / span as f32).clamp(0.0, 1.0);
    match id {
        Id::PageSize => {
            let v = 3 + (frac * (10 - 3) as f32).round() as i32;
            app.cfg.input.page_size = v.clamp(3, 10) as u32;
            app.mark_dirty();
        }
        Id::FontSize => {
            let v = 9 + (frac * (26 - 9) as f32).round() as i32;
            app.cfg.appearance.font_size = v.clamp(9, 26) as u32;
            app.mark_dirty();
        }
        Id::Opacity => {
            let v = 30 + (frac * (100 - 30) as f32).round() as i32;
            app.cfg.appearance.opacity = v.clamp(30, 100) as u8;
            app.mark_dirty();
        }
        _ => {}
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let app = App::create(hwnd);
                let ptr = Box::into_raw(app);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
                let _ = PostMessageW(Some(hwnd), MSG_DEFERRED, WPARAM(0), LPARAM(0));
                let _ = SetTimer(Some(hwnd), TIMER_ENGINE, 1200, None);
                LRESULT(0)
            }
            MSG_DEFERRED => {
                if let Some(app) = main_of(hwnd) {
                    app.core.init_engine();
                    app.schemas = app.core.schemas();
                    app.stats = app.core.dict_stats();
                    app.backups = app.core.list_backups();
                    app.engine_ready = !app.schemas.is_empty();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                match main_of(hwnd) {
                    Some(app) => paint(app),
                    None => {
                        let mut ps = PAINTSTRUCT::default();
                        let _ = BeginPaint(hwnd, &mut ps);
                        let _ = EndPaint(hwnd, &ps);
                    }
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                if let Some(app) = main_of(hwnd) {
                    let pt = point(lparam);
                    if let Some(id) = app.dragging_slider {
                        drag_slider(app, id, pt.x);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return LRESULT(0);
                    }
                    let hit = app.hit_test(pt.x, pt.y);
                    if hit != app.hover {
                        app.hover = hit;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(app) = main_of(hwnd) {
                    let pt = point(lparam);
                    let hit = app.hit_test(pt.x, pt.y);
                    app.pressed = hit;
                    match hit {
                        Some(Id::PageSize) | Some(Id::FontSize) | Some(Id::Opacity) => {
                            app.dragging_slider = hit;
                            if let Some(id) = hit {
                                drag_slider(app, id, pt.x);
                            }
                        }
                        _ => {}
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some(app) = main_of(hwnd) {
                    let pt = point(lparam);
                    let hit = app.hit_test(pt.x, pt.y);
                    let pressed = app.pressed;
                    app.pressed = None;
                    if app.dragging_slider.is_some() {
                        app.dragging_slider = None;
                    } else if let Some(id) = pressed {
                        if Some(id) == hit {
                            app.activate(id);
                        }
                    } else if app.combo_open.is_some() {
                        // 点空白处收起下拉
                        app.combo_open = None;
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Some(app) = main_of(hwnd) {
                    let delta = (((wparam.0 >> 16) & 0xFFFF) as i16 as i32) / 120;
                    app.scroll = (app.scroll - delta * app.s(64)).clamp(0, app.scroll_max.max(0));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if let Some(app) = main_of(hwnd) {
                    match (wparam.0 as u32, app.focus) {
                        (0x08, Focus::PunctKey) => {
                            app.punct_key.pop();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        (0x08, Focus::PunctOut) => {
                            app.punct_out.pop();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        (0x1B, _) => {
                            app.combo_open = None;
                            app.focus = Focus::None;
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_CHAR => {
                if let Some(app) = main_of(hwnd) {
                    if let Some(ch) = char::from_u32(wparam.0 as u32) {
                        if !ch.is_control() {
                            match app.focus {
                                Focus::PunctKey => app.punct_key.push(ch),
                                Focus::PunctOut => app.punct_out.push(ch),
                                Focus::None => {}
                            }
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_SAVE {
                    let _ = KillTimer(Some(hwnd), TIMER_SAVE);
                    if let Some(app) = main_of(hwnd) {
                        app.save_now();
                    }
                } else if wparam.0 == TIMER_TOAST {
                    let _ = KillTimer(Some(hwnd), TIMER_TOAST);
                    if let Some(app) = main_of(hwnd) {
                        app.toast.clear();
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                } else if wparam.0 == TIMER_ENGINE {
                    if let Some(app) = main_of(hwnd) {
                        if !app.engine_ready {
                            let schemas = app.core.schemas();
                            if !schemas.is_empty() {
                                app.schemas = schemas;
                                app.stats = app.core.dict_stats();
                                app.backups = app.core.list_backups();
                                app.engine_ready = true;
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                        if app.engine_ready {
                            let _ = KillTimer(Some(hwnd), TIMER_ENGINE);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_SIZE => {
                if let Some(app) = main_of(hwnd) {
                    app.hits.clear();
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                if let Some(app) = main_of(hwnd) {
                    app.dpi = GetDpiForWindow(hwnd).max(96);
                    app.rebuild_fonts();
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                mmi.ptMinTrackSize.x = 940 * dpi / 96;
                mmi.ptMinTrackSize.y = 620 * dpi / 96;
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if let Some(c) = LoadCursorW(None, IDC_ARROW).ok() {
                    SetCursor(Some(c));
                }
                LRESULT(1)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

pub fn run() {
    unsafe {
        let cls = wide("Lingxi.Settings.Instance");
        let _ = CreateMutexW(None, false, PCWSTR(cls.as_ptr()));
        let main_cls = wide("Lingxi.Settings.SelfDrawn");
        if let Ok(existing) = FindWindowW(PCWSTR(main_cls.as_ptr()), PCWSTR::null()) {
            if !existing.0.is_null() {
                let _ = ShowWindow(existing, SW_RESTORE);
                let _ = SetForegroundWindow(existing);
                return;
            }
        }
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0 | CS_DBLCLKS.0),
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hmod.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszClassName: PCWSTR(main_cls.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return;
        }
        let title = wide("灵犀输入法设置");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(main_cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1020,
            700,
            None,
            None,
            Some(HINSTANCE(hmod.0)),
            None,
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            return;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 用 GDI 画折线（细线抗锯齿交给系统即可）。
unsafe fn gdi_polyline(dc: HDC, pts: &[(i32, i32)], argb: u32, width: i32) {
    if pts.len() < 2 {
        return;
    }
    let pen = CreatePen(PS_SOLID, width, COLORREF(argb_to_colorref(argb)));
    let old = SelectObject(dc, HGDIOBJ(pen.0));
    let _ = MoveToEx(dc, pts[0].0, pts[0].1, None);
    for p in &pts[1..] {
        let _ = LineTo(dc, p.0, p.1);
    }
    SelectObject(dc, old);
    let _ = DeleteObject(HGDIOBJ(pen.0));
}
// ---------------- 整帧绘制 ----------------

fn paint(app: &mut App) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(app.hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(app.hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, HGDIOBJ(bmp.0));
        app.hits.clear();
        let t = app.theme.clone();

        if let Some(g) = Gfx::new(mem) {
            g.fill_round(RECT { left: 0, top: 0, right: w, bottom: h }, 0, t.bg);
        }
        // 侧栏
        let side_w = app.s(212);
        if let Some(g) = Gfx::new(mem) {
            g.fill_round(RECT { left: 0, top: 0, right: side_w, bottom: h }, 0, t.sidebar);
            g.fill_round(
                RECT { left: side_w - app.s(1), top: 0, right: side_w, bottom: h },
                0,
                t.border,
            );
        }
        {
            let lr = RECT { left: app.s(20), top: app.s(20), right: side_w - app.s(12), bottom: app.s(48) };
            text(mem, lr, "灵犀输入法", app.font_title, t.text, DT_LEFT, true);
            let sr = RECT { left: app.s(21), top: app.s(46), right: side_w - app.s(12), bottom: app.s(66) };
            text(mem, sr, "设置中心", app.font_small, t.text3, DT_LEFT, true);
        }
        for (i, (icon, name, _)) in PAGES.iter().enumerate() {
            let top = app.s(84 + (i as i32) * 50);
            let r = RECT { left: app.s(12), top, right: side_w - app.s(12), bottom: top + app.s(42) };
            let id = Id::Nav(i);
            let sel = app.page == i;
            let hot = app.hover == Some(id);
            if let Some(g) = Gfx::new(mem) {
                if sel {
                    g.fill_round(r, 10, t.accent_soft);
                } else if hot {
                    g.fill_round(r, 10, t.hover);
                }
            }
            let ir = RECT { left: r.left + app.s(12), top: r.top, right: r.left + app.s(40), bottom: r.bottom };
            text(mem, ir, icon, app.font, if sel { t.accent } else { t.text2 }, DT_LEFT, true);
            let nr = RECT { left: r.left + app.s(42), top: r.top, right: r.right - app.s(8), bottom: r.bottom };
            text(
                mem,
                nr,
                name,
                if sel { app.font_bold } else { app.font },
                if sel { t.accent } else { t.text },
                DT_LEFT,
                true,
            );
            app.hits.push(Hit { id, rect: r });
        }
        {
            let br = RECT { left: app.s(20), top: h - app.s(56), right: side_w - app.s(12), bottom: h - app.s(34) };
            text(mem, br, &format!("版本 {}", env!("CARGO_PKG_VERSION")), app.font_small, t.text3, DT_LEFT, true);
            let hr = RECT { left: app.s(20), top: h - app.s(34), right: side_w - app.s(12), bottom: h - app.s(14) };
            text(mem, hr, "改动即时生效并自动保存", app.font_small, t.text3, DT_LEFT, true);
        }

        // 内容区（滚动 + 裁剪）
        let cx = side_w + app.s(20);
        let cw = (w - cx - app.s(20)).max(app.s(520));
        let cy0 = app.s(20);
        let clip = CreateRectRgn(cx, cy0, w, h);
        let _ = SelectClipRgn(mem, Some(clip));
        let used = match app.page {
            0 => draw_input(app, mem, cx, cy0 - app.scroll, cw),
            1 => draw_appearance(app, mem, cx, cy0 - app.scroll, cw),
            2 => draw_dict(app, mem, cx, cy0 - app.scroll, cw),
            _ => draw_about(app, mem, cx, cy0 - app.scroll, cw),
        };
        app.scroll_max = (used + cy0 - (h - app.s(12))).max(0);
        let _ = SelectClipRgn(mem, None);
        let _ = DeleteObject(HGDIOBJ(clip.0));

        // 下拉浮层
        app.draw_combo_popup(mem);

        // 提示条
        if !app.toast.is_empty() && std::time::Instant::now() < app.toast_until {
            let msg = app.toast.clone();
            let tw = App::text_width_of(mem, &msg, app.font) + app.s(30);
            let r = RECT { left: cx, top: h - app.s(48), right: cx + tw, bottom: h - app.s(16) };
            if let Some(g) = Gfx::new(mem) {
                g.fill_round(r, 8, if t.dark { 0xFF2E323C } else { 0xFF23304A });
            }
            text(mem, r, &msg, app.font, 0xFFFFFFFF, DT_CENTER, true);
        }

        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = EndPaint(app.hwnd, &mut ps);
    }
}

fn draw_input(app: &mut App, dc: HDC, x: i32, y0: i32, w: i32) -> i32 {
    let gap = app.s(16);
    let col_w = (w - gap) / 2;
    let t = app.theme.clone();
    let mut y = y0;

    // 左栏
    let cy = app.card(dc, x, y, col_w, app.s(92), "默认输入方案", "");
    let schema_name = app
        .schemas
        .iter()
        .find(|(id, _)| *id == app.cfg.input.schema)
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| app.cfg.input.schema.clone());
    let combo = RECT { left: x + app.s(18), top: cy, right: x + col_w - app.s(18), bottom: cy + app.s(34) };
    app.combo(dc, Id::SchemaCombo, combo, &schema_name);
    y += app.s(116) + gap;

    let mode_h = app.s(190);
    let cy = app.card(dc, x, y, col_w, mode_h, "输入模式", "");
    let mut ry = cy;
    for (label, desc, id, names, sel) in [
        ("全角 / 半角", "标点与空格的字宽", Id::SegFull, ["半角", "全角"], if app.cfg.input.full_shape { 1 } else { 0 }),
        ("中英标点", "中文标点（，。）或英文标点（, .）", Id::SegPunct, ["中文", "英文"], if app.cfg.input.ascii_punct { 1 } else { 0 }),
        ("默认中英模式", "新会话直接输入英文", Id::SegAscii, ["中文", "英文"], if app.cfg.input.ascii_mode { 1 } else { 0 }),
    ] {
        let r = app.row(dc, x + app.s(18), ry, col_w - app.s(36), label, desc);
        let seg = RECT { left: r.right - app.s(148), top: r.top + app.s(6), right: r.right, bottom: r.top + app.s(36) };
        app.segmented(dc, id, seg, &names, sel);
        ry += app.s(52);
    }
    y += mode_h + gap;

    let cy = app.card(dc, x, y, col_w, app.s(96), "候选窗", "");
    let r = app.row(dc, x + app.s(18), cy, col_w - app.s(36), "每页候选数", "改动后自动重建词库索引");
    let sl = RECT { left: r.left, top: r.top + app.s(4), right: r.right - app.s(78), bottom: r.top + app.s(32) };
    app.slider(dc, Id::PageSize, sl, app.cfg.input.page_size as i32, 3, 10, " 个");
    y += app.s(104) + gap;

    let keys_h = app.s(196);
    let cy = app.card(dc, x, y, col_w, keys_h, "按键行为", "");
    let mut ry = cy;
    for (label, id, shown) in [
        ("回车键", Id::EnterCombo, enter_text(app)),
        ("中英切换键", Id::ToggleCombo, toggle_text(app)),
        ("拼音分隔符", Id::SepCombo, sep_text(app)),
    ] {
        let r = app.row(dc, x + app.s(18), ry, col_w - app.s(36), label, "");
        let cb = RECT { left: r.right - app.s(230), top: r.top + app.s(4), right: r.right, bottom: r.top + app.s(36) };
        app.combo(dc, id, cb, &shown);
        ry += app.s(52);
    }
    y += keys_h + gap;

    // 右栏
    let rx = x + col_w + gap;
    let mut ry = y0;
    let fuzzy_h = app.s(212);
    let cy = app.card(dc, rx, ry, col_w, fuzzy_h, "模糊音", "");
    let cell_w = (col_w - app.s(36)) / 3;
    for row in 0..4i32 {
        for col in 0..3i32 {
            let i = (row * 3 + col) as usize;
            if i >= FUZZY_LABELS.len() {
                break;
            }
            let (gid, label) = FUZZY_LABELS[i];
            let r = RECT {
                left: rx + app.s(18) + col * cell_w,
                top: cy + row * app.s(28),
                right: rx + app.s(18) + (col + 1) * cell_w - app.s(6),
                bottom: cy + row * app.s(28) + app.s(26),
            };
            let on = app.cfg.input.fuzzy.iter().any(|f| f == gid);
            app.checkbox(dc, Id::Fuzzy(i), r, label, on);
        }
    }
    let mut fy = cy + 4 * app.s(28) + app.s(4);
    let r = app.row(dc, rx + app.s(18), fy, col_w - app.s(36), "用户词典自学习", "记住你常选的词并调整词频");
    app.toggle_switch(dc, Id::SwitchLearn, r, app.cfg.input.enable_user_dict);
    fy += app.s(48);
    ry += fuzzy_h + gap;

    let punct_h = app.s(268);
    let cy = app.card(dc, rx, ry, col_w, punct_h, "标点自定义", "");
    let list_rect = RECT { left: rx + app.s(18), top: cy, right: rx + col_w - app.s(18), bottom: cy + app.s(140) };
    let items: Vec<String> = app
        .cfg
        .input
        .punct_map
        .iter()
        .map(|p| format!("{}   →   {}", p.key, p.out))
        .collect();
    let selected = app.punct_sel();
    app.list(dc, Id::PunctRow, list_rect, &items, selected, "暂无自定义（使用内置标点表）");
    let fy = list_rect.bottom + app.s(12);
    let inner = col_w - app.s(36);
    let kw = (inner - app.s(16)) / 4;
    let kr = RECT { left: rx + app.s(18), top: fy, right: rx + app.s(18) + kw, bottom: fy + app.s(32) };
    let ov = RECT { left: kr.right + app.s(8), top: fy, right: kr.right + app.s(8) + kw * 2, bottom: fy + app.s(32) };
    app.text_field(dc, Id::PunctKey, kr, &app.punct_key.clone(), "按键");
    app.text_field(dc, Id::PunctOut, ov, &app.punct_out.clone(), "上屏文本");
    let add = RECT { left: ov.right + app.s(8), top: fy, right: ov.right + app.s(8) + kw / 2, bottom: fy + app.s(32) };
    let del = RECT { left: add.right + app.s(6), top: fy, right: rx + col_w - app.s(18), bottom: fy + app.s(32) };
    app.button(dc, Id::PunctAdd, add, "添加", true);
    app.button(dc, Id::PunctDel, del, "删除", false);
    ry += punct_h + gap;
    y.max(ry) + app.s(10)
}

fn draw_appearance(app: &mut App, dc: HDC, x: i32, y0: i32, w: i32) -> i32 {
    let gap = app.s(16);
    let col_w = (w - gap) / 2;
    let mut y = y0;

    let prev_h = app.s(232);
    let cy = app.card(dc, x, y, col_w, prev_h, "候选窗预览", "");
    let pr = RECT { left: x + app.s(18), top: cy, right: x + col_w - app.s(18), bottom: cy + app.s(186) };
    app.draw_candidate_preview(dc, pr);
    y += prev_h + gap;

    let th = app.s(92);
    let cy = app.card(dc, x, y, col_w, th, "主题预设", "");
    let bw = (col_w - app.s(36) - app.s(16)) / 3;
    for (i, name) in ["浅色", "深色", "护眼"].iter().enumerate() {
        let r = RECT {
            left: x + app.s(18) + (i as i32) * (bw + app.s(8)),
            top: cy,
            right: x + app.s(18) + (i as i32) * (bw + app.s(8)) + bw,
            bottom: cy + app.s(34),
        };
        app.button(dc, Id::ThemePreset(i), r, name, false);
    }
    y += th + gap;

    let sh = app.s(92);
    let cy = app.card(dc, x, y, col_w, sh, "皮肤导入导出", "");
    app.button(dc, Id::SkinExport, RECT { left: x + app.s(18), top: cy, right: x + app.s(18) + app.s(120), bottom: cy + app.s(34) }, "导出皮肤", false);
    app.button(dc, Id::SkinImport, RECT { left: x + app.s(146), top: cy, right: x + app.s(146) + app.s(120), bottom: cy + app.s(34) }, "导入皮肤", false);
    y += sh + gap;

    let rx = x + col_w + gap;
    let mut ry = y0;
    let ch = app.s(46 + 40 * 7);
    let cy = app.card(dc, rx, ry, col_w, ch, "自定义颜色", "");
    let mut fy = cy;
    for (i, label) in COLOR_LABELS.iter().enumerate() {
        let color = color_of(app, i);
        let lr = RECT { left: rx + app.s(18), top: fy + app.s(4), right: rx + app.s(110), bottom: fy + app.s(28) };
        unsafe {
            text(dc, lr, label, app.font, app.theme.text, DT_LEFT, true);
        }
        let cr = RECT { left: rx + app.s(116), top: fy + app.s(5), right: rx + app.s(164), bottom: fy + app.s(27) };
        app.chip(dc, Id::ColorPick(i), cr, color);
        let hex = argb_hex(color);
        let hr = RECT { left: cr.right + app.s(10), top: fy, right: cr.right + app.s(120), bottom: fy + app.s(32) };
        unsafe {
            text(dc, hr, &hex, app.font_small, app.theme.text2, DT_LEFT, true);
        }
        let br = RECT { left: rx + col_w - app.s(96), top: fy + app.s(2), right: rx + col_w - app.s(18), bottom: fy + app.s(32) };
        app.button(dc, Id::ColorPick(i), br, "选择…", false);
        fy += app.s(40);
    }
    ry += ch + gap;

    let dh = app.s(228);
    let cy = app.card(dc, rx, ry, col_w, dh, "显示", "");
    let mut fy = cy;
    let r = app.row(dc, rx + app.s(18), fy, col_w - app.s(36), "字体大小", "");
    app.slider(dc, Id::FontSize, RECT { left: r.left, top: r.top + app.s(4), right: r.right - app.s(90), bottom: r.top + app.s(32) }, app.cfg.appearance.font_size as i32, 9, 26, " pt");
    fy += app.s(52);
    let r = app.row(dc, rx + app.s(18), fy, col_w - app.s(36), "窗口不透明度", "");
    app.slider(dc, Id::Opacity, RECT { left: r.left, top: r.top + app.s(4), right: r.right - app.s(90), bottom: r.top + app.s(32) }, app.cfg.appearance.opacity as i32, 30, 100, " %");
    fy += app.s(56);
    let r = app.row(dc, rx + app.s(18), fy, col_w - app.s(36), "显示候选序号", "");
    app.toggle_switch(dc, Id::SwitchIndex, r, app.cfg.appearance.show_index);
    fy += app.s(46);
    let r = app.row(dc, rx + app.s(18), fy, col_w - app.s(36), "显示右下角状态栏", "");
    app.toggle_switch(dc, Id::SwitchStatus, r, app.cfg.appearance.show_status_bar);
    ry += dh + gap;
    y.max(ry) + app.s(10)
}
fn draw_dict(app: &mut App, dc: HDC, x: i32, y0: i32, w: i32) -> i32 {
    let gap = app.s(16);
    let col_w = (w - gap) / 2;
    let t = app.theme.clone();
    let mut y = y0;

    let sh = app.s(152);
    let cy = app.card(dc, x, y, col_w, sh, "词库统计", "");
    let rows = [
        ("自定义短语", format!("{} 条", app.stats.custom_phrase_count)),
        ("词库数据", format!("约 {:.0} MB", app.stats.table_size_mb)),
        ("用户词典", if app.stats.user_dict_exists { "已启用".to_string() } else { "未创建".to_string() }),
    ];
    let mut fy = cy;
    for (k, v) in rows {
        let kr = RECT { left: x + app.s(18), top: fy, right: x + col_w / 2, bottom: fy + app.s(30) };
        let vr = RECT { left: x + col_w / 2, top: fy, right: x + col_w - app.s(18), bottom: fy + app.s(30) };
        unsafe {
            text(dc, kr, k, app.font, t.text2, DT_LEFT, true);
            text(dc, vr, &v, app.font_bold, t.text, DT_RIGHT, true);
        }
        fy += app.s(36);
    }
    y += sh + gap;

    let ih = app.s(124);
    let cy = app.card(dc, x, y, col_w, ih, "自定义短语", "");
    app.button(dc, Id::DictImport, RECT { left: x + app.s(18), top: cy, right: x + app.s(18) + app.s(140), bottom: cy + app.s(34) }, "导入短语文件…", true);
    app.button(dc, Id::DictExport, RECT { left: x + app.s(166), top: cy, right: x + app.s(166) + app.s(140), bottom: cy + app.s(34) }, "导出当前短语", false);
    if !app.stats.last_import.is_empty() {
        let hr = RECT { left: x + app.s(18), top: cy + app.s(46), right: x + col_w - app.s(18), bottom: cy + app.s(74) };
        let msg = app.stats.last_import.clone();
        unsafe {
            text(dc, hr, &msg, app.font_small, t.accent, DT_LEFT, true);
        }
    }
    y += ih + gap;

    let rx = x + col_w + gap;
    let bh = app.s(346);
    let cy = app.card(dc, rx, y0, col_w, bh, "用户词库备份 / 恢复", "");
    app.button(dc, Id::BackupNow, RECT { left: rx + app.s(18), top: cy, right: rx + app.s(18) + app.s(120), bottom: cy + app.s(34) }, "立即备份", true);
    let lr = RECT { left: rx + app.s(18), top: cy + app.s(48), right: rx + col_w - app.s(18), bottom: cy + app.s(208) };
    let items: Vec<String> = app
        .backups
        .iter()
        .map(|b| b.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let sel = Some(app.backup_sel);
    app.list(dc, Id::BackupRow, lr, &items, sel, "还没有备份");
    app.button(dc, Id::Restore, RECT { left: rx + app.s(18), top: cy + app.s(222), right: rx + app.s(18) + app.s(140), bottom: cy + app.s(256) }, "恢复所选备份", false);
    let hr = RECT { left: rx + app.s(170), top: cy + app.s(226), right: rx + col_w - app.s(18), bottom: cy + app.s(258) };
    unsafe {
        text(dc, hr, "恢复前自动另存当前词库", app.font_small, t.text3, DT_LEFT, true);
    }
    y = y.max(y0 + bh) + gap;
    y + app.s(10)
}

fn draw_about(app: &mut App, dc: HDC, x: i32, y0: i32, w: i32) -> i32 {
    let gap = app.s(16);
    let col_w = (w - gap) / 2;
    let t = app.theme.clone();
    let mut y = y0;

    let mh = app.s(200);
    let cy = app.card(dc, x, y, col_w, mh, "维护与诊断", "");
    app.button(dc, Id::OpenData, RECT { left: x + app.s(18), top: cy, right: x + app.s(18) + app.s(126), bottom: cy + app.s(34) }, "打开数据目录", false);
    app.button(dc, Id::OpenLogs, RECT { left: x + app.s(154), top: cy, right: x + app.s(154) + app.s(126), bottom: cy + app.s(34) }, "打开日志目录", false);
    app.button(dc, Id::ClearLogs, RECT { left: x + app.s(290), top: cy, right: x + app.s(290) + app.s(120), bottom: cy + app.s(34) }, "清空日志", false);
    app.button(dc, Id::ResetAll, RECT { left: x + app.s(18), top: cy + app.s(48), right: x + app.s(18) + app.s(140), bottom: cy + app.s(82) }, "恢复默认设置", false);
    y += mh + gap;

    let uh = app.s(92);
    let cy = app.card(dc, x, y, col_w, uh, "更新", "");
    app.button(dc, Id::Update, RECT { left: x + app.s(18), top: cy, right: x + app.s(18) + app.s(140), bottom: cy + app.s(34) }, "查看最新版本", true);
    y += uh + gap;

    let rx = x + col_w + gap;
    let kh = app.s(396);
    let cy = app.card(dc, rx, y0, col_w, kh, "快捷键", "");
    let lines = [
        ("Shift", "组合中上屏拼音；未组合切换中/英"),
        ("回车", "默认原样上屏（行为可改）"),
        ("空格", "确认高亮候选词"),
        ("1-9", "选择对应候选"),
        ("Ctrl+.", "切换中英标点"),
        ("Tab / [ ] / - =", "候选翻页"),
        ("← / →", "拼音内逐字移动"),
        ("状态栏「符号」", "打开/收起符号面板"),
        ("右键状态栏", "开关菜单"),
        ("状态栏齿轮", "打开本设置中心"),
    ];
    let mut fy = cy;
    for (k, v) in lines {
        let kr = RECT { left: rx + app.s(18), top: fy, right: rx + app.s(150), bottom: fy + app.s(28) };
        let vr = RECT { left: rx + app.s(150), top: fy, right: rx + col_w - app.s(18), bottom: fy + app.s(28) };
        unsafe {
            text(dc, kr, k, app.font_bold, t.text, DT_LEFT, true);
            text(dc, vr, v, app.font_small, t.text2, DT_LEFT, true);
        }
        fy += app.s(34);
    }
    y = y.max(y0 + kh) + gap;
    y + app.s(10)
}