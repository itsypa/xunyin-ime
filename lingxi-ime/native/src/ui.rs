//! 设置中心界面：主窗口 + 四个页签（输入 / 外观 / 词库 / 关于）。
//!
//! 全部用 Win32 公共控件（标签页、按钮、复选框、下拉、滑块、列表框、编辑框），
//! 没有 WebView。候选窗预览与颜色色块是自绘的小窗口类（GDI）。

#![allow(clippy::too_many_arguments)]

use crate::core::{Core, DictStats};
use lingxi_rime::config::{AppearanceConfig, LingxiConfig};
use std::mem::size_of;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, GetOpenFileNameW, CHOOSECOLORW, OPENFILENAMEW, CC_FULLOPEN, CC_RGBINIT,
    OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::w;

// ---------------- 控件样式（自带取值，避开 crate 里的类型差异）----------------
/// SendMessageW 的薄封装（统一成 usize/isize，省去 Option 包装）。
unsafe fn send(h: HWND, msg: u32, w: usize, l: isize) -> isize {
    SendMessageW(h, msg, Some(WPARAM(w)), Some(LPARAM(l))).0
}

const S_LEFT: u32 = 0x0000;
const S_CENTERIMAGE: u32 = 0x0200;
const S_GROUPBOX: u32 = 0x0007;
const S_AUTOCHECKBOX: u32 = 0x0003;
const S_PUSHBUTTON: u32 = 0x0000;
const S_DROPDOWNLIST: u32 = 0x0003;
const S_AUTOHSCROLL: u32 = 0x0080;
const S_NOTIFY: u32 = 0x0001;
const S_TBS_HORZ: u32 = 0x0000;
const S_TBS_NOTICKS: u32 = 0x0010;
/// CBN_SELCHANGE
const NOTIFY_SELCHANGE: u32 = 1;
/// TCN_SELCHANGE（TCN_FIRST - 1）
const NOTIFY_TABCHANGE: u32 = 0xFFFF_FDD9;

const TBM_GETPOS_: u32 = 0x0400;
const TBM_SETPOS_: u32 = 0x0405;
const TBM_SETRANGE_: u32 = 0x0406;
// ---------------- 控件 ID ----------------
const ID_SCHEMA: usize = 1001;
const ID_CHK_FULL: usize = 1010;
const ID_CHK_PUNCT: usize = 1011;
const ID_CHK_ASCII: usize = 1012;
const ID_SLIDER_PAGE: usize = 1020;
const ID_STATIC_PAGE: usize = 1021;
const ID_CMB_ENTER: usize = 1030;
const ID_CMB_TOGGLE: usize = 1031;
const ID_CMB_SEP: usize = 1032;
const ID_FUZZY_BASE: usize = 1040; // +0..10
const ID_CHK_LEARN: usize = 1060;
const ID_LIST_PUNCT: usize = 1070;
const ID_EDIT_PUNCT_KEY: usize = 1071;
const ID_EDIT_PUNCT_OUT: usize = 1072;
const ID_BTN_PUNCT_ADD: usize = 1073;
const ID_BTN_PUNCT_DEL: usize = 1074;

const ID_PREVIEW: usize = 2000;
const ID_SLIDER_FONT: usize = 2010;
const ID_STATIC_FONT: usize = 2011;
const ID_SLIDER_OPACITY: usize = 2012;
const ID_STATIC_OPACITY: usize = 2013;
const ID_CHK_INDEX: usize = 2014;
const ID_CHK_STATUSBAR: usize = 2015;
const ID_COLOR_BASE: usize = 2020; // +0..6 色块；+100 为对应按钮
const ID_COLOR_BTN_BASE: usize = 2120;
const ID_BTN_SKIN_EXPORT: usize = 2030;
const ID_BTN_SKIN_IMPORT: usize = 2031;
const ID_THEME_BASE: usize = 2040; // +0..2 预设

const ID_STAT_PHRASE: usize = 3001;
const ID_STAT_TABLE: usize = 3002;
const ID_STAT_USERDB: usize = 3003;
const ID_BTN_DICT_IMPORT: usize = 3010;
const ID_BTN_DICT_EXPORT: usize = 3011;
const ID_BTN_BACKUP: usize = 3020;
const ID_LIST_BACKUP: usize = 3021;
const ID_BTN_RESTORE: usize = 3022;

const ID_STATIC_VERSION: usize = 4001;
const ID_BTN_OPEN_DATA: usize = 4010;
const ID_BTN_OPEN_LOGS: usize = 4011;
const ID_BTN_CLEAR_LOGS: usize = 4012;
const ID_BTN_UPDATE: usize = 4013;
const ID_BTN_RESET: usize = 4014;
const ID_STATIC_SHORTCUTS: usize = 4020;
const ID_STATIC_TOAST: usize = 4030;

const TIMER_SAVE: usize = 1;
const TIMER_TOAST: usize = 2;

const WND_MAIN: &str = "Lingxi.Settings.Main";
const WND_PAGE: &str = "Lingxi.Settings.Page";
const WND_PREVIEW: &str = "Lingxi.Settings.Preview";
const WND_SWATCH: &str = "Lingxi.Settings.Swatch";

const TAB_COUNT: usize = 4;
const RELEASE_URL: &str = "https://github.com/itsypa/lingxi-ime/releases/latest";

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

/// 颜色项：配置字段、显示名、取/存闭包索引。
const COLOR_FIELDS: [(&str, &str); 7] = [
    ("accent", "主色调"),
    ("bg", "背景色"),
    ("text", "文字颜色"),
    ("selected_bg", "选中背景"),
    ("selected_text", "选中文字"),
    ("comment", "注释颜色"),
    ("border", "边框颜色"),
];

const ENTER_ITEMS: [(&str, &str); 3] = [
    ("commit", "上屏输入的拼音（默认）"),
    ("clear", "丢弃当前输入"),
    ("newline", "丢弃并换行"),
];
const TOGGLE_ITEMS: [(&str, &str); 3] = [
    ("shift", "Shift（默认）"),
    ("ctrl_space", "Ctrl + 空格"),
    ("none", "不切换"),
];
const SEP_ITEMS: [(&str, &str); 5] = [
    ("'", "撇号 '   ni'hao"),
    ("-", "连字符 -  ni-hao"),
    (",", "逗号 ,   ni,hao"),
    (" ", "空格     ni hao"),
    ("", "不显示   nihao"),
];

// ---------------- 应用状态 ----------------

struct App {
    hwnd: HWND,
    core: Core,
    cfg: LingxiConfig,
    saved: LingxiConfig,
    schemas: Vec<(String, String)>,
    stats: DictStats,
    tab: HWND,
    pages: [HWND; TAB_COUNT],
    preview: HWND,
    toast: HWND,
    font_ui: HFONT,
    font_title: HFONT,
    font_preview: HFONT,
    dpi: u32,
    toast_until: std::time::Instant,
    /// 每个控件的 DIP 矩形（DPI 变化时统一重排）
    placements: Vec<(HWND, (i32, i32, i32, i32))>,
    cur_tab: usize,
    engine_ready: bool,
}

fn app_of(hwnd: HWND) -> Option<&'static mut App> {
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

fn scale(app: &App, v: i32) -> i32 {
    ((v as f32) * (app.dpi as f32 / 96.0)).round() as i32
}

// ---------------- 入口 ----------------

pub fn run() {
    unsafe {
        // 单实例：状态栏齿轮会不断拉起本程序，已有窗口就把它提到前面
        let cls = wide("Lingxi.Settings.SingleInstance");
        let _ = CreateMutexW(None, false, PCWSTR(cls.as_ptr()));
        if let Ok(existing) = FindWindowW(PCWSTR(wide(WND_MAIN).as_ptr()), PCWSTR::null()) {
            if !existing.0.is_null() {
                let _ = ShowWindow(existing, SW_RESTORE);
                let _ = SetForegroundWindow(existing);
                return;
            }
        }
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        register_classes(HINSTANCE(hmod.0));

        let title = wide("灵犀输入法设置");
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide(WND_MAIN).as_ptr()),
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

fn register_classes(hinst: HINSTANCE) {
    unsafe {
        let classes: [(&str, WNDPROC); 4] = [
            (WND_MAIN, Some(main_proc)),
            (WND_PAGE, Some(page_proc)),
            (WND_PREVIEW, Some(preview_proc)),
            (WND_SWATCH, Some(swatch_proc)),
        ];
        for (name, proc) in classes {
            let cls = wide(name);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
                lpfnWndProc: proc,
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut core::ffi::c_void),
                lpszClassName: PCWSTR(cls.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
    }
}

// ---------------- 控件创建小工具 ----------------

impl App {
    fn new(hwnd: HWND) -> Self {
        let core = Core::new();
        let cfg = core.load_config();
        Self {
            hwnd,
            core,
            saved: cfg.clone(),
            cfg,
            schemas: Vec::new(),
            stats: DictStats::default(),
            tab: HWND::default(),
            pages: [HWND::default(); TAB_COUNT],
            preview: HWND::default(),
            toast: HWND::default(),
            font_ui: HFONT::default(),
            font_title: HFONT::default(),
            font_preview: HFONT::default(),
            dpi: 96,
            toast_until: std::time::Instant::now(),
            placements: Vec::new(),
            cur_tab: 0,
            engine_ready: false,
        }
    }

    /// 新建控件：DIP 坐标记录在 placements 里，DPI 变化时统一重排。
    fn mk(
        &mut self,
        parent: HWND,
        class: PCWSTR,
        text: &str,
        style: u32,
        id: usize,
        r: (i32, i32, i32, i32),
    ) -> HWND {
        unsafe {
            let t = wide(text);
            let c = if class == PCWSTR::null() {
                w!("STATIC")
            } else {
                class
            };
            let hmod = GetModuleHandleW(None).unwrap_or_default();
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                c,
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                r.0,
                r.1,
                r.2,
                r.3,
                Some(parent),
                Some(HMENU(id as *mut core::ffi::c_void)),
                Some(HINSTANCE(hmod.0)),
                None,
            )
            .unwrap_or_default();
            let _ = send(h, WM_SETFONT, self.font_ui.0 as usize, 1);
            self.placements.push((h, r));
            h
        }
    }

    fn set_fonts(&mut self) {
        let dpi = self.dpi as i32;
        unsafe {
            for f in [self.font_ui, self.font_title, self.font_preview] {
                if !f.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(f.0));
                }
            }
            let name = wide("Microsoft YaHei UI");
            let mk = |px: i32, weight: i32| {
                CreateFontW(
                    -((px * dpi / 96).max(11)),
                    0,
                    0,
                    0,
                    weight,
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
            };
            self.font_ui = mk(14, FW_NORMAL.0 as i32);
            self.font_title = mk(20, FW_SEMIBOLD.0 as i32);
            self.font_preview = mk(16, FW_NORMAL.0 as i32);
        }
    }
}

/// 十六进制颜色 ↔ COLORREF（0x00BBGGRR）。
fn hex_to_colorref(s: &str) -> COLORREF {
    let t = s.trim_start_matches('#');
    if t.len() != 6 {
        return COLORREF(0);
    }
    let r = u8::from_str_radix(&t[0..2], 16).unwrap_or(0) as u32;
    let g = u8::from_str_radix(&t[2..4], 16).unwrap_or(0) as u32;
    let b = u8::from_str_radix(&t[4..6], 16).unwrap_or(0) as u32;
    COLORREF(r | (g << 8) | (b << 16))
}

fn colorref_to_hex(c: COLORREF) -> String {
    let v = c.0;
    format!(
        "#{:02X}{:02X}{:02X}",
        v & 0xFF,
        (v >> 8) & 0xFF,
        (v >> 16) & 0xFF
    )
}

/// 颜色项按索引读写（与 COLOR_FIELDS 同序）。
fn color_get(a: &AppearanceConfig, i: usize) -> COLORREF {
    match i {
        0 => hex_to_colorref(&a.accent),
        1 => hex_to_colorref(&a.bg),
        2 => hex_to_colorref(&a.text),
        3 => hex_to_colorref(&a.selected_bg),
        4 => hex_to_colorref(&a.selected_text),
        5 => hex_to_colorref(&a.comment),
        _ => hex_to_colorref(&a.border),
    }
}

fn color_set(a: &mut AppearanceConfig, i: usize, c: COLORREF) {
    let hex = colorref_to_hex(c);
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
}

/// 主题预设（与网页版一致）。
fn apply_theme(a: &mut AppearanceConfig, which: usize) {
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
}

// ---------------- 建立控件 ----------------

const P_INPUT: usize = 0;
const P_APPEAR: usize = 1;
const P_DICT: usize = 2;
const P_ABOUT: usize = 3;

const PAGES: [&str; TAB_COUNT] = ["输入设置", "外观主题", "词库管理", "关于"];

fn bs_groupbox() -> u32 {
    S_GROUPBOX
}

fn create_controls(app: &mut App) {
    unsafe {
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        // 标签页
        let tab = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WC_TABCONTROLW,
            PCWSTR::null(),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPSIBLINGS.0),
            0,
            0,
            10,
            10,
            Some(app.hwnd),
            Some(HMENU(900 as *mut core::ffi::c_void)),
            Some(HINSTANCE(hmod.0)),
            None,
        )
        .unwrap_or_default();
        app.tab = tab;
        let _ = send(tab, WM_SETFONT, app.font_ui.0 as usize, 1);
        for (i, name) in PAGES.iter().enumerate() {
            let mut t = wide(name);
            let mut item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: windows::core::PWSTR(t.as_mut_ptr()),
                ..Default::default()
            };
            let _ = send(tab, TCM_INSERTITEMW, i, &mut item as *mut _ as isize);
        }
        // 四个页面窗口
        for i in 0..TAB_COUNT {
            let page = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(wide(WND_PAGE).as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_CHILD.0 | WS_CLIPCHILDREN.0),
                0,
                0,
                10,
                10,
                Some(app.hwnd),
                None,
                Some(HINSTANCE(hmod.0)),
                None,
            )
            .unwrap_or_default();
            app.pages[i] = page;
        }
        // 底部提示条
        app.toast = app.mk(
            app.hwnd,
            PCWSTR::null(),
            "设置改动即时生效并自动保存",
            S_LEFT | S_CENTERIMAGE,
            ID_STATIC_TOAST,
            (12, 660, 980, 22),
        );
        build_input_page(app);
        build_appearance_page(app);
        build_dict_page(app);
        build_about_page(app);
    }
}

fn build_input_page(app: &mut App) {
    let page = app.pages[P_INPUT];
        app.mk(page, w!("BUTTON"), "输入方案", bs_groupbox(), 0, (8, 8, 470, 62));
        app.mk(page, PCWSTR::null(), "默认方案", S_LEFT, 0, (24, 22, 86, 20));
        app.mk(
            page,
            w!("COMBOBOX"),
            "",
            S_DROPDOWNLIST | WS_VSCROLL.0 | WS_TABSTOP.0,
            ID_SCHEMA,
            (118, 20, 344, 200),
        );

        app.mk(page, w!("BUTTON"), "输入模式", bs_groupbox(), 0, (8, 78, 470, 116));
        app.mk(page, w!("BUTTON"), "全角 / 半角", S_AUTOCHECKBOX | WS_TABSTOP.0, ID_CHK_FULL, (24, 100, 200, 24));
        app.mk(page, w!("BUTTON"), "中英标点（中文标点 / 英文标点）", S_AUTOCHECKBOX | WS_TABSTOP.0, ID_CHK_PUNCT, (24, 130, 400, 24));
        app.mk(page, w!("BUTTON"), "默认中英模式为英文", S_AUTOCHECKBOX | WS_TABSTOP.0, ID_CHK_ASCII, (24, 160, 400, 24));

        app.mk(page, w!("BUTTON"), "候选窗", bs_groupbox(), 0, (8, 202, 470, 62));
        app.mk(page, PCWSTR::null(), "每页候选数", S_LEFT, 0, (24, 224, 86, 20));
        app.mk(page, PCWSTR::null(), "", S_LEFT, ID_STATIC_PAGE, (420, 224, 50, 20));
        let slider = app.mk(
            page,
            TRACKBAR_CLASSW,
            "",
            S_TBS_HORZ | S_TBS_NOTICKS | WS_TABSTOP.0,
            ID_SLIDER_PAGE,
            (118, 216, 292, 30),
        );
        unsafe {
            let _ = send(slider, TBM_SETRANGE_, 1, ((3 << 16) | 10) as isize);
        }

        app.mk(page, w!("BUTTON"), "按键行为", bs_groupbox(), 0, (8, 272, 470, 172));
        for (i, (label, id)) in [("回车键", ID_CMB_ENTER), ("中英切换键", ID_CMB_TOGGLE), ("拼音分隔符", ID_CMB_SEP)]
            .iter()
            .enumerate()
        {
            let y = 294 + (i as i32) * 44;
            app.mk(page, PCWSTR::null(), label, S_LEFT, 0, (24, y + 4, 96, 20));
            app.mk(
                page,
                w!("COMBOBOX"),
                "",
                S_DROPDOWNLIST | WS_VSCROLL.0 | WS_TABSTOP.0,
                *id,
                (122, y, 340, 200),
            );
        }

        app.mk(page, w!("BUTTON"), "模糊音", bs_groupbox(), 0, (8, 452, 470, 168));
        app.mk(
            page,
            PCWSTR::null(),
            "勾选后不标准读音也能出字（如 zh↔z：zongguo 也能出「中国」）",
            S_LEFT,
            0,
            (24, 470, 440, 20),
        );
        for (i, (_, label)) in FUZZY_LABELS.iter().enumerate() {
            let col = (i % 3) as i32;
            let row = (i / 3) as i32;
            app.mk(
                page,
                w!("BUTTON"),
                label,
                S_AUTOCHECKBOX | WS_TABSTOP.0,
                ID_FUZZY_BASE + i,
                (24 + col * 148, 494 + row * 26, 144, 24),
            );
        }

        app.mk(page, w!("BUTTON"), "词库学习", bs_groupbox(), 0, (494, 8, 470, 62));
        app.mk(
            page,
            w!("BUTTON"),
            "用户词典自学习（记新词 / 调频）",
            S_AUTOCHECKBOX | WS_TABSTOP.0,
            ID_CHK_LEARN,
            (510, 30, 400, 24),
        );

        app.mk(page, w!("BUTTON"), "标点自定义", bs_groupbox(), 0, (494, 78, 470, 400));
        app.mk(
            page,
            PCWSTR::null(),
            "仅对中文标点生效：把某个按键改成要上屏的文本（留空则用内置标点表）。",
            S_LEFT,
            0,
            (510, 96, 440, 34),
        );
        app.mk(
            page,
            w!("LISTBOX"),
            "",
            WS_BORDER.0 | WS_VSCROLL.0 | S_NOTIFY | WS_TABSTOP.0,
            ID_LIST_PUNCT,
            (510, 134, 438, 190),
        );
        app.mk(page, PCWSTR::null(), "按键", S_LEFT, 0, (510, 334, 40, 20));
        app.mk(page, w!("EDIT"), "", WS_BORDER.0 | S_AUTOHSCROLL | WS_TABSTOP.0, ID_EDIT_PUNCT_KEY, (510, 354, 70, 26));
        app.mk(page, PCWSTR::null(), "上屏文本", S_LEFT, 0, (594, 334, 80, 20));
        app.mk(page, w!("EDIT"), "", WS_BORDER.0 | S_AUTOHSCROLL | WS_TABSTOP.0, ID_EDIT_PUNCT_OUT, (594, 354, 200, 26));
        app.mk(page, w!("BUTTON"), "添加", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_PUNCT_ADD, (806, 354, 68, 26));
        app.mk(page, w!("BUTTON"), "删除所选", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_PUNCT_DEL, (880, 354, 68, 26));
        app.mk(
            page,
            PCWSTR::null(),
            "提示：模糊音或每页候选数改动后会重建词库索引，稍等片刻生效。",
            S_LEFT,
            0,
            (510, 396, 440, 40),
        );
}

fn build_appearance_page(app: &mut App) {
    let page = app.pages[P_APPEAR];
    unsafe {
        app.mk(page, w!("BUTTON"), "候选窗预览", bs_groupbox(), 0, (8, 8, 470, 250));
        let preview = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide(WND_PREVIEW).as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            24,
            34,
            438,
            210,
            Some(page),
            Some(HMENU(ID_PREVIEW as *mut core::ffi::c_void)),
            Some(HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0)),
            None,
        )
        .unwrap_or_default();
        app.preview = preview;
        app.placements.push((preview, (24, 34, 438, 210)));

        app.mk(page, w!("BUTTON"), "自定义颜色", bs_groupbox(), 0, (494, 8, 470, 250));
        for (i, (_, label)) in COLOR_FIELDS.iter().enumerate() {
            let y = 26 + (i as i32) * 31;
            app.mk(page, PCWSTR::null(), label, S_LEFT, 0, (510, y + 2, 90, 20));
            let sw = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(wide(WND_SWATCH).as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                606,
                y + 3,
                46,
                18,
                Some(page),
                Some(HMENU((ID_COLOR_BASE + i) as *mut core::ffi::c_void)),
                Some(HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0)),
                None,
            )
            .unwrap_or_default();
            app.placements.push((sw, (606, y + 3, 46, 18)));
            app.mk(
                page,
                w!("BUTTON"),
                "选择…",
                S_PUSHBUTTON | WS_TABSTOP.0,
                ID_COLOR_BTN_BASE + i,
                (662, y, 140, 26),
            );
        }

        app.mk(page, w!("BUTTON"), "主题预设", bs_groupbox(), 0, (8, 266, 470, 84));
        for (i, label) in ["浅色", "深色", "护眼"].iter().enumerate() {
            app.mk(
                page,
                w!("BUTTON"),
                label,
                S_PUSHBUTTON | WS_TABSTOP.0,
                ID_THEME_BASE + i,
                (24 + (i as i32) * 152, 296, 140, 30),
            );
        }

        app.mk(page, w!("BUTTON"), "显示", bs_groupbox(), 0, (494, 266, 470, 236));
        app.mk(page, PCWSTR::null(), "字体大小", S_LEFT, 0, (510, 292, 70, 20));
        app.mk(page, PCWSTR::null(), "", S_LEFT, ID_STATIC_FONT, (856, 292, 96, 20));
        let fs = app.mk(page, TRACKBAR_CLASSW, "", S_TBS_HORZ | S_TBS_NOTICKS | WS_TABSTOP.0, ID_SLIDER_FONT, (590, 284, 260, 30));
        let _ = send(fs, TBM_SETRANGE_, 1, ((9 << 16) | 26) as isize);
        app.mk(page, PCWSTR::null(), "窗口不透明度", S_LEFT, 0, (510, 330, 76, 20));
        app.mk(page, PCWSTR::null(), "", S_LEFT, ID_STATIC_OPACITY, (856, 330, 96, 20));
        let os = app.mk(page, TRACKBAR_CLASSW, "", S_TBS_HORZ | S_TBS_NOTICKS | WS_TABSTOP.0, ID_SLIDER_OPACITY, (590, 322, 260, 30));
        let _ = send(os, TBM_SETRANGE_, 1, ((30 << 16) | 100) as isize);
        app.mk(page, w!("BUTTON"), "显示候选序号", S_AUTOCHECKBOX | WS_TABSTOP.0, ID_CHK_INDEX, (510, 366, 300, 24));
        app.mk(page, w!("BUTTON"), "显示右下角状态栏", S_AUTOCHECKBOX | WS_TABSTOP.0, ID_CHK_STATUSBAR, (510, 396, 300, 24));
        app.mk(page, PCWSTR::null(), "字号把鼠标停在滑块上会有等大演示；预览固定尺寸不被撑大。", S_LEFT, 0, (510, 424, 440, 40));

        app.mk(page, w!("BUTTON"), "皮肤导入导出", bs_groupbox(), 0, (8, 358, 470, 96));
        app.mk(page, w!("BUTTON"), "导出皮肤", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_SKIN_EXPORT, (24, 386, 120, 30));
        app.mk(page, w!("BUTTON"), "导入皮肤", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_SKIN_IMPORT, (156, 386, 120, 30));
        app.mk(
            page,
            PCWSTR::null(),
            "皮肤保存当前外观（配色/字号/圆角/透明度），可备份或分享。",
            S_LEFT,
            0,
            (24, 420, 440, 20),
        );
    }
}

fn build_dict_page(app: &mut App) {
    let page = app.pages[P_DICT];
        app.mk(page, w!("BUTTON"), "词库统计", bs_groupbox(), 0, (8, 8, 470, 130));
        for (id, label) in [
            (ID_STAT_PHRASE, "自定义短语：—"),
            (ID_STAT_TABLE, "词库数据：—"),
            (ID_STAT_USERDB, "用户词典：—"),
        ] {
            app.mk(page, PCWSTR::null(), label, S_LEFT, id, (24, 34 + ((id - ID_STAT_PHRASE) as i32) * 30, 440, 22));
        }

        app.mk(page, w!("BUTTON"), "自定义短语", bs_groupbox(), 0, (8, 146, 470, 150));
        app.mk(
            page,
            PCWSTR::null(),
            "导入文件需每行「词<TAB>编码」；导入后自动重建索引。",
            S_LEFT,
            0,
            (24, 168, 440, 34),
        );
        app.mk(page, w!("BUTTON"), "导入短语文件…", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_DICT_IMPORT, (24, 210, 130, 30));
        app.mk(page, w!("BUTTON"), "导出当前短语", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_DICT_EXPORT, (166, 210, 130, 30));
        app.mk(page, PCWSTR::null(), "", S_LEFT, ID_STAT_PHRASE + 100, (24, 252, 440, 22));

        app.mk(page, w!("BUTTON"), "用户词库备份 / 恢复", bs_groupbox(), 0, (494, 8, 470, 340));
        app.mk(
            page,
            PCWSTR::null(),
            "用户词典是你打字学到的词（二进制目录），整目录备份即完整备份。",
            S_LEFT,
            0,
            (510, 30, 440, 34),
        );
        app.mk(page, w!("BUTTON"), "立即备份", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_BACKUP, (510, 70, 110, 30));
        app.mk(
            page,
            w!("LISTBOX"),
            "",
            WS_BORDER.0 | WS_VSCROLL.0 | S_NOTIFY | WS_TABSTOP.0,
            ID_LIST_BACKUP,
            (510, 110, 440, 150),
        );
        app.mk(page, w!("BUTTON"), "恢复所选备份", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_RESTORE, (510, 270, 130, 30));
        app.mk(
            page,
            PCWSTR::null(),
            "恢复前会自动把当前词库另存一份，误操作可再恢复。",
            S_LEFT,
            0,
            (652, 276, 300, 20),
        );

        app.mk(page, w!("BUTTON"), "提示", bs_groupbox(), 0, (494, 356, 470, 120));
        app.mk(
            page,
            PCWSTR::null(),
            "词库索引体积较大时，导入/恢复后的重建需要几秒到几十秒；\r\n期间输入法仍可正常打字（用旧索引），完成后自动切到新索引。",
            S_LEFT,
            0,
            (510, 380, 440, 80),
        );
}

fn build_about_page(app: &mut App) {
    let page = app.pages[P_ABOUT];
    let dpi = app.dpi as i32;
    unsafe {
        let title = app.mk(page, PCWSTR::null(), "灵犀输入法 · 设置中心", S_LEFT, 0, (8, 8, 470, 32));
        let _ = send(title, WM_SETFONT, app.font_title.0 as usize, 1);
        app.mk(
            page,
            PCWSTR::null(),
            &format!("版本 {}", env!("CARGO_PKG_VERSION")),
            S_LEFT,
            ID_STATIC_VERSION,
            (8, 46, 470, 22),
        );
        app.mk(
            page,
            PCWSTR::null(),
            "系统级 TSF 拼音输入法，基于 Rime 引擎与增强词库；无广告、无遥测、本地运行。",
            S_LEFT,
            0,
            (8, 70 + (dpi - dpi), 470, 24),
        );

        app.mk(page, w!("BUTTON"), "维护与诊断", bs_groupbox(), 0, (8, 100, 470, 150));
        app.mk(page, w!("BUTTON"), "打开数据目录", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_OPEN_DATA, (24, 128, 130, 30));
        app.mk(page, w!("BUTTON"), "打开日志目录", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_OPEN_LOGS, (166, 128, 130, 30));
        app.mk(page, w!("BUTTON"), "清空日志", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_CLEAR_LOGS, (308, 128, 130, 30));
        app.mk(page, w!("BUTTON"), "恢复默认设置", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_RESET, (24, 168, 130, 30));
        app.mk(
            page,
            PCWSTR::null(),
            "输入异常时先看日志：里面记录了按键、组合、候选与状态栏事件；\r\n把 tsf.log 发到 issue 即可定位问题。",
            S_LEFT,
            0,
            (24, 206, 440, 40),
        );

        app.mk(page, w!("BUTTON"), "更新", bs_groupbox(), 0, (494, 100, 470, 150));
        app.mk(page, w!("BUTTON"), "查看最新版本", S_PUSHBUTTON | WS_TABSTOP.0, ID_BTN_UPDATE, (510, 128, 130, 30));
        app.mk(
            page,
            PCWSTR::null(),
            "本输入法不联网自检：点上面的按钮用浏览器打开发布页，\r\n对照最新版本号决定是否升级。",
            S_LEFT,
            0,
            (510, 168, 440, 70),
        );

        app.mk(page, w!("BUTTON"), "快捷键", bs_groupbox(), 0, (8, 260, 956, 320));
        let keys = "Shift\t组合中上屏拼音；未组合切换中/英（切换键可在「输入设置」改）\r\n\
回车\t默认把拼音原样上屏（不选词），行为可改\r\n\
空格\t确认高亮候选词\r\n\
1-9\t选择对应候选\r\n\
Ctrl+.\t切换中英标点\r\n\
Tab / [ ] / - =\t候选翻页\r\n\
← / →\t拼音内逐字移动（跳过分隔符，到头不循环）\r\n\
状态栏「符号」格\t打开/收起符号面板（蓝底=已打开）\r\n\
符号面板\t六类符号点选直上屏；可拖动，✕ 关闭\r\n\
右键状态栏\t弹出开关菜单（中英/全角/标点/符号/设置）\r\n\
状态栏齿轮\t打开本设置中心";
        app.mk(page, PCWSTR::null(), keys, S_LEFT, ID_STATIC_SHORTCUTS, (24, 288, 920, 280));
    }
}

// ---------------- 控件读写助手 ----------------

fn ctl(parent: HWND, id: usize) -> HWND {
    unsafe { GetDlgItem(Some(parent), id as i32).unwrap_or_default() }
}

fn chk_set(parent: HWND, id: usize, on: bool) {
    unsafe {
        let _ = send(ctl(parent, id), BM_SETCHECK, if on { 1 } else { 0 }, 0);
    }
}

fn chk_get(parent: HWND, id: usize) -> bool {
    unsafe { send(ctl(parent, id), BM_GETCHECK, 0, 0) == 1 }
}

fn text_set(parent: HWND, id: usize, s: &str) {
    unsafe {
        let t = wide(s);
        let _ = SetWindowTextW(ctl(parent, id), PCWSTR(t.as_ptr()));
    }
}

fn text_get(parent: HWND, id: usize) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(ctl(parent, id), &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn combo_cur(parent: HWND, id: usize) -> usize {
    unsafe {
        let v = send(ctl(parent, id), CB_GETCURSEL, 0, 0);
        if v < 0 {
            0
        } else {
            v as usize
        }
    }
}

fn fill_combo(parent: HWND, id: usize, items: &[&str], sel: usize) {
    unsafe {
        let c = ctl(parent, id);
        let _ = send(c, CB_RESETCONTENT, 0, 0);
        for it in items {
            let t = wide(it);
            let _ = send(c, CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        let _ = send(c, CB_SETCURSEL, sel, 0);
    }
}

fn value_index(items: &[(&str, &str)], v: &str) -> usize {
    items.iter().position(|(k, _)| *k == v).unwrap_or(0)
}

// ---------------- 布局 ----------------

fn layout(app: &mut App) {
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(app.hwnd, &mut rc).is_err() {
            return;
        }
        let (cw, ch) = (rc.right, rc.bottom);
        let toast_h = scale(app, 24);
        if !app.toast.0.is_null() {
            let _ = SetWindowPos(
                app.toast,
                None,
                scale(app, 12),
                ch - toast_h - scale(app, 4),
                cw - scale(app, 24),
                toast_h,
                SWP_NOZORDER,
            );
        }
        let tab = app.tab;
        let tab_h = (ch - scale(app, 34)).max(scale(app, 240));
        let _ = SetWindowPos(
            tab,
            None,
            scale(app, 8),
            scale(app, 8),
            cw - scale(app, 16),
            tab_h,
            SWP_NOZORDER,
        );
        // 标签页显示区（去掉页签头）
        let mut disp = RECT {
            left: 0,
            top: 0,
            right: cw - scale(app, 16),
            bottom: tab_h,
        };
        let _ = send(tab, TCM_ADJUSTRECT, 0, &mut disp as *mut _ as isize);
        let pw = (disp.right - disp.left).max(scale(app, 780));
        let ph = (disp.bottom - disp.top).max(scale(app, 420));
        for (i, page) in app.pages.iter().enumerate() {
            let _ = SetWindowPos(*page, Some(HWND_TOP), disp.left, disp.top, pw, ph, SWP_NOACTIVATE);
            let _ = ShowWindow(*page, if i == app.cur_tab { SW_SHOW } else { SW_HIDE });
        }
        // 页内控件按 DIP 重排
        let places = app.placements.clone();
        for (h, r) in places {
            let _ = SetWindowPos(
                h,
                None,
                scale(app, r.0),
                scale(app, r.1),
                scale(app, r.2),
                scale(app, r.3),
                SWP_NOZORDER,
            );
        }
        let _ = InvalidateRect(Some(app.preview), None, true);
    }
}

// ---------------- 刷新控件（配置 → 界面） ----------------

fn refresh_punct_list(app: &mut App) {
    let p = app.pages[P_INPUT];
    unsafe {
        let lb = ctl(p, ID_LIST_PUNCT);
        let _ = send(lb, LB_RESETCONTENT, 0, 0);
        for pair in &app.cfg.input.punct_map {
            let t = wide(&format!("{}  →  {}", pair.key, pair.out));
            let _ = send(lb, LB_ADDSTRING, 0, t.as_ptr() as isize);
        }
    }
}

fn refresh_stats(app: &mut App) {
    let p = app.pages[P_DICT];
    let s = app.stats.clone();
    text_set(
        p,
        ID_STAT_PHRASE,
        &format!("自定义短语：{} 条", s.custom_phrase_count),
    );
    text_set(
        p,
        ID_STAT_TABLE,
        &format!("词库数据：约 {:.0} MB", s.table_size_mb),
    );
    text_set(
        p,
        ID_STAT_USERDB,
        &format!("用户词典：{}", if s.user_dict_exists { "已启用" } else { "未创建" }),
    );
    if !s.last_import.is_empty() {
        text_set(app.pages[P_DICT], ID_STAT_PHRASE + 100, &s.last_import);
    }
}

fn refresh_backups(app: &mut App) {
    let p = app.pages[P_DICT];
    let backups = app.core.list_backups();
    unsafe {
        let lb = ctl(p, ID_LIST_BACKUP);
        let _ = send(lb, LB_RESETCONTENT, 0, 0);
        for b in &backups {
            let name = b
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let t = wide(&name);
            let _ = send(lb, LB_ADDSTRING, 0, t.as_ptr() as isize);
        }
        if !backups.is_empty() {
            let _ = send(lb, LB_SETCURSEL, 0, 0);
        }
    }
}

fn refresh_controls(app: &mut App) {
    let p1 = app.pages[P_INPUT];
    let p2 = app.pages[P_APPEAR];
    unsafe {
        // 方案下拉
        let c = ctl(p1, ID_SCHEMA);
        let _ = send(c, CB_RESETCONTENT, 0, 0);
        let mut sel = 0usize;
        if app.schemas.is_empty() {
            let t = wide(&app.cfg.input.schema);
            let _ = send(c, CB_ADDSTRING, 0, t.as_ptr() as isize);
        } else {
            for (i, (id, name)) in app.schemas.iter().enumerate() {
                let t = wide(name);
                let _ = send(c, CB_ADDSTRING, 0, t.as_ptr() as isize);
                if *id == app.cfg.input.schema {
                    sel = i;
                }
            }
        }
        let _ = send(c, CB_SETCURSEL, sel, 0);

        chk_set(p1, ID_CHK_FULL, app.cfg.input.full_shape);
        chk_set(p1, ID_CHK_PUNCT, !app.cfg.input.ascii_punct);
        chk_set(p1, ID_CHK_ASCII, app.cfg.input.ascii_mode);
        chk_set(p1, ID_CHK_LEARN, app.cfg.input.enable_user_dict);

        let _ = send(
            ctl(p1, ID_SLIDER_PAGE),
            TBM_SETPOS_,
            1,
            app.cfg.input.page_size as isize,
        );
        text_set(p1, ID_STATIC_PAGE, &format!("{} 个", app.cfg.input.page_size));

        let enter: Vec<&str> = ENTER_ITEMS.iter().map(|(_, l)| *l).collect();
        fill_combo(
            p1,
            ID_CMB_ENTER,
            &enter,
            value_index(&ENTER_ITEMS, &app.cfg.input.enter_behavior),
        );
        let toggle: Vec<&str> = TOGGLE_ITEMS.iter().map(|(_, l)| *l).collect();
        fill_combo(
            p1,
            ID_CMB_TOGGLE,
            &toggle,
            value_index(&TOGGLE_ITEMS, &app.cfg.input.toggle_key),
        );
        let sep: Vec<&str> = SEP_ITEMS.iter().map(|(_, l)| *l).collect();
        fill_combo(
            p1,
            ID_CMB_SEP,
            &sep,
            value_index(&SEP_ITEMS, &app.cfg.input.pinyin_separator),
        );
        for (i, (id, _)) in FUZZY_LABELS.iter().enumerate() {
            chk_set(
                p1,
                ID_FUZZY_BASE + i,
                app.cfg.input.fuzzy.iter().any(|f| f == id),
            );
        }
        refresh_punct_list(app);

        // 外观
        let _ = send(
            ctl(p2, ID_SLIDER_FONT),
            TBM_SETPOS_,
            1,
            app.cfg.appearance.font_size as isize,
        );
        text_set(
            p2,
            ID_STATIC_FONT,
            &format!("{} pt", app.cfg.appearance.font_size),
        );
        let _ = send(
            ctl(p2, ID_SLIDER_OPACITY),
            TBM_SETPOS_,
            1,
            app.cfg.appearance.opacity as isize,
        );
        text_set(
            p2,
            ID_STATIC_OPACITY,
            &format!("{} %", app.cfg.appearance.opacity),
        );
        chk_set(p2, ID_CHK_INDEX, app.cfg.appearance.show_index);
        chk_set(p2, ID_CHK_STATUSBAR, app.cfg.appearance.show_status_bar);
        for (i, (_, label)) in COLOR_FIELDS.iter().enumerate() {
            let hex = colorref_to_hex(color_get(&app.cfg.appearance, i));
            text_set(p2, ID_COLOR_BTN_BASE + i, &format!("{label}  {hex}"));
        }
        let _ = InvalidateRect(Some(app.preview), None, true);
    }
    refresh_stats(app);
    refresh_backups(app);
}

// ---------------- 保存 / 提示 ----------------

const MSG_DEFERRED_INIT: u32 = WM_APP + 1;
const TIMER_ENGINE: usize = 3;

fn mark_dirty(app: &mut App) {
    unsafe {
        let _ = SetTimer(Some(app.hwnd), TIMER_SAVE, 400, None);
    }
}

fn toast(app: &mut App, msg: &str) {
    text_set(app.hwnd, ID_STATIC_TOAST, msg);
    app.toast_until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    unsafe {
        let _ = SetTimer(Some(app.hwnd), TIMER_TOAST, 5000, None);
    }
}

fn save_now(app: &mut App) {
    let cfg = app.cfg.clone();
    let old = app.saved.clone();
    match app.core.save_config(&cfg, &old) {
        Ok(needs_redeploy) => {
            app.saved = cfg;
            if needs_redeploy {
                app.core.redeploy();
                toast(app, "设置已保存，正在重建词库索引…");
            } else {
                toast(app, "设置已保存");
            }
        }
        Err(e) => toast(app, &format!("保存失败：{e}")),
    }
}

// ---------------- 颜色选择 / 文件选择 ----------------

fn pick_color(app: &mut App, idx: usize) {
    unsafe {
        let mut custom = [COLORREF(0x00FF_FFFF); 16];
        let mut cc = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: app.hwnd,
            rgbResult: color_get(&app.cfg.appearance, idx),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_FULLOPEN | CC_RGBINIT,
            ..Default::default()
        };
        if ChooseColorW(&mut cc).as_bool() {
            color_set(&mut app.cfg.appearance, idx, cc.rgbResult);
            mark_dirty(app);
            refresh_controls(app);
        }
    }
}

/// 打开文件选择框；filter 形如 [("短语文件","*.txt"), ("所有文件","*.*")]。
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

// ---------------- 命令处理 ----------------

fn on_command(app: &mut App, id: usize, code: u32) {
    let p1 = app.pages[P_INPUT];
    let p2 = app.pages[P_APPEAR];
    match id {
        ID_SCHEMA if code == NOTIFY_SELCHANGE => {
            let i = combo_cur(p1, ID_SCHEMA);
            if let Some((sid, _)) = app.schemas.get(i) {
                app.cfg.input.schema = sid.clone();
                mark_dirty(app);
            }
        }
        ID_CHK_FULL => {
            app.cfg.input.full_shape = chk_get(p1, ID_CHK_FULL);
            mark_dirty(app);
        }
        ID_CHK_PUNCT => {
            app.cfg.input.ascii_punct = !chk_get(p1, ID_CHK_PUNCT);
            mark_dirty(app);
        }
        ID_CHK_ASCII => {
            app.cfg.input.ascii_mode = chk_get(p1, ID_CHK_ASCII);
            mark_dirty(app);
        }
        ID_CHK_LEARN => {
            app.cfg.input.enable_user_dict = chk_get(p1, ID_CHK_LEARN);
            mark_dirty(app);
        }
        ID_CMB_ENTER if code == NOTIFY_SELCHANGE => {
            let i = combo_cur(p1, ID_CMB_ENTER);
            app.cfg.input.enter_behavior = ENTER_ITEMS[i.min(2)].0.into();
            mark_dirty(app);
        }
        ID_CMB_TOGGLE if code == NOTIFY_SELCHANGE => {
            let i = combo_cur(p1, ID_CMB_TOGGLE);
            app.cfg.input.toggle_key = TOGGLE_ITEMS[i.min(2)].0.into();
            mark_dirty(app);
        }
        ID_CMB_SEP if code == NOTIFY_SELCHANGE => {
            let i = combo_cur(p1, ID_CMB_SEP);
            app.cfg.input.pinyin_separator = SEP_ITEMS[i.min(4)].0.into();
            mark_dirty(app);
        }
        ID_BTN_PUNCT_ADD => {
            let key = text_get(p1, ID_EDIT_PUNCT_KEY).trim().to_string();
            let out = text_get(p1, ID_EDIT_PUNCT_OUT).trim().to_string();
            if key.is_empty() || out.is_empty() {
                toast(app, "请先填写「按键」与「上屏文本」");
                return;
            }
            let list = &mut app.cfg.input.punct_map;
            if let Some(existing) = list.iter_mut().find(|p| p.key == key) {
                existing.out = out;
            } else {
                list.push(lingxi_rime::config::PunctPair { key, out });
            }
            refresh_punct_list(app);
            mark_dirty(app);
            toast(app, "已添加自定义标点");
        }
        ID_BTN_PUNCT_DEL => {
            let sel = unsafe {
                send(ctl(p1, ID_LIST_PUNCT), LB_GETCURSEL, 0, 0)
            };
            if sel >= 0 {
                let i = sel as usize;
                if i < app.cfg.input.punct_map.len() {
                    app.cfg.input.punct_map.remove(i);
                    refresh_punct_list(app);
                    mark_dirty(app);
                    toast(app, "已删除该条自定义标点");
                }
            }
        }
        ID_CHK_INDEX => {
            app.cfg.appearance.show_index = chk_get(p2, ID_CHK_INDEX);
            mark_dirty(app);
            unsafe {
                let _ = InvalidateRect(Some(app.preview), None, true);
            }
        }
        ID_CHK_STATUSBAR => {
            app.cfg.appearance.show_status_bar = chk_get(p2, ID_CHK_STATUSBAR);
            mark_dirty(app);
        }
        ID_BTN_SKIN_EXPORT => match app.core.export_skin() {
            Ok(p) => toast(app, &format!("已导出皮肤：{}", p.display())),
            Err(e) => toast(app, &format!("导出失败：{e}")),
        },
        ID_BTN_SKIN_IMPORT => {
            if let Some(path) = pick_file(
                app.hwnd,
                &[("皮肤文件", "*.json"), ("所有文件", "*.*")],
                "json",
            ) {
                match app.core.import_skin(&path) {
                    Ok(cfg) => {
                        app.cfg = cfg;
                        refresh_controls(app);
                        mark_dirty(app);
                        toast(app, "皮肤已套用");
                    }
                    Err(e) => toast(app, &format!("导入失败：{e}")),
                }
            }
        }
        ID_BTN_DICT_IMPORT => {
            if let Some(path) = pick_file(
                app.hwnd,
                &[("短语文件", "*.txt"), ("所有文件", "*.*")],
                "txt",
            ) {
                match app.core.import_dict_file(&path) {
                    Ok(stats) => {
                        app.stats = stats;
                        refresh_stats(app);
                        toast(app, &app.stats.last_import.clone());
                    }
                    Err(e) => toast(app, &format!("导入失败：{e}")),
                }
            }
        }
        ID_BTN_DICT_EXPORT => match app.core.export_dict() {
            Ok(p) => toast(app, &format!("已导出：{}", p.display())),
            Err(e) => toast(app, &format!("导出失败：{e}")),
        },
        ID_BTN_BACKUP => match app.core.backup_user_dict() {
            Ok(p) => {
                refresh_backups(app);
                toast(app, &format!("已备份到：{}", p.display()));
            }
            Err(e) => toast(app, &format!("备份失败：{e}")),
        },
        ID_BTN_RESTORE => {
            let backups = app.core.list_backups();
            let sel = unsafe {
                send(ctl(app.pages[P_DICT], ID_LIST_BACKUP), LB_GETCURSEL, 0, 0) as i32
            };
            let sel = if sel < 0 { 0 } else { sel as usize };
            if let Some(src) = backups.get(sel) {
                match app.core.restore_user_dict(&src.clone()) {
                    Ok(msg) => toast(app, &msg),
                    Err(e) => toast(app, &format!("恢复失败：{e}")),
                }
            } else {
                toast(app, "请先在列表里选择一个备份");
            }
        }
        ID_BTN_OPEN_DATA => {
            if let Err(e) = app.core.open_data_dir() {
                toast(app, &format!("打开失败：{e}"));
            }
        }
        ID_BTN_OPEN_LOGS => {
            if let Err(e) = app.core.open_log_dir() {
                toast(app, &format!("打开失败：{e}"));
            }
        }
        ID_BTN_CLEAR_LOGS => {
            let freed = app.core.clear_logs();
            toast(app, &format!("已清空日志（释放 {} KB）", freed / 1024));
        }
        ID_BTN_RESET => {
            app.cfg = app.core.reset_config();
            refresh_controls(app);
            toast(app, "已恢复默认设置（正在重建索引）");
        }
        ID_BTN_UPDATE => unsafe {
            let url = wide(RELEASE_URL);
            let op = wide("open");
            ShellExecuteW(
                Some(app.hwnd),
                PCWSTR(op.as_ptr()),
                PCWSTR(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        },
        _ => {
            // 主题预设 / 颜色按钮
            if (ID_THEME_BASE..ID_THEME_BASE + 3).contains(&id) {
                let i = id - ID_THEME_BASE;
                apply_theme(&mut app.cfg.appearance, i);
                refresh_controls(app);
                mark_dirty(app);
                toast(app, "已套用主题预设");
            } else if (ID_COLOR_BTN_BASE..ID_COLOR_BTN_BASE + 7).contains(&id) {
                let i = id - ID_COLOR_BTN_BASE;
                pick_color(app, i);
            } else if (ID_FUZZY_BASE..ID_FUZZY_BASE + FUZZY_LABELS.len()).contains(&id) {
                let i = id - ID_FUZZY_BASE;
                let gid = FUZZY_LABELS[i].0;
                let on = chk_get(p1, id);
                let list = &mut app.cfg.input.fuzzy;
                if on {
                    if !list.iter().any(|f| f == gid) {
                        list.push(gid.to_string());
                    }
                } else {
                    list.retain(|f| f != gid);
                }
                mark_dirty(app);
            }
        }
    }
}

// ---------------- 窗口过程 ----------------

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let mut app = Box::new(App::new(hwnd));
                app.dpi = GetDpiForWindow(hwnd).max(96);
                app.set_fonts();
                let ptr = Box::into_raw(app);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
                if let Some(app) = app_of(hwnd) {
                    create_controls(app);
                    layout(app);
                    refresh_controls(app);
                    // 引擎初始化放到窗口显示之后：点开立即有界面
                    let _ = PostMessageW(Some(hwnd), MSG_DEFERRED_INIT, WPARAM(0), LPARAM(0));
                    let _ = SetTimer(Some(hwnd), TIMER_ENGINE, 1200, None);
                }
                LRESULT(0)
            }
            MSG_DEFERRED_INIT => {
                if let Some(app) = app_of(hwnd) {
                    app.core.init_engine();
                    app.schemas = app.core.schemas();
                    app.stats = app.core.dict_stats();
                    refresh_controls(app);
                    app.engine_ready = !app.schemas.is_empty();
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_SAVE {
                    let _ = KillTimer(Some(hwnd), TIMER_SAVE);
                    if let Some(app) = app_of(hwnd) {
                        save_now(app);
                    }
                } else if wparam.0 == TIMER_ENGINE {
                    if let Some(app) = app_of(hwnd) {
                        if !app.engine_ready {
                            let schemas = app.core.schemas();
                            if !schemas.is_empty() {
                                app.schemas = schemas;
                                app.stats = app.core.dict_stats();
                                refresh_controls(app);
                                app.engine_ready = true;
                            }
                        }
                        if app.engine_ready {
                            let _ = KillTimer(Some(hwnd), TIMER_ENGINE);
                        }
                    }
                } else if wparam.0 == TIMER_TOAST {
                    let _ = KillTimer(Some(hwnd), TIMER_TOAST);
                    text_set(hwnd, ID_STATIC_TOAST, "设置改动即时生效并自动保存");
                }
                LRESULT(0)
            }
            WM_HSCROLL => {
                if let Some(app) = app_of(hwnd) {
                    let bar = HWND(lparam.0 as *mut core::ffi::c_void);
                    let id = GetDlgCtrlID(bar) as usize;
                    let pos = send(bar, TBM_GETPOS_, 0, 0) as u32;
                    match id {
                        ID_SLIDER_PAGE => {
                            app.cfg.input.page_size = pos.clamp(3, 10);
                            text_set(
                                app.pages[P_INPUT],
                                ID_STATIC_PAGE,
                                &format!("{} 个", app.cfg.input.page_size),
                            );
                            mark_dirty(app);
                        }
                        ID_SLIDER_FONT => {
                            app.cfg.appearance.font_size = pos.clamp(9, 26);
                            text_set(
                                app.pages[P_APPEAR],
                                ID_STATIC_FONT,
                                &format!("{} pt", app.cfg.appearance.font_size),
                            );
                            mark_dirty(app);
                            let _ = InvalidateRect(Some(app.preview), None, true);
                        }
                        ID_SLIDER_OPACITY => {
                            app.cfg.appearance.opacity = pos.clamp(30, 100) as u8;
                            text_set(
                                app.pages[P_APPEAR],
                                ID_STATIC_OPACITY,
                                &format!("{} %", app.cfg.appearance.opacity),
                            );
                            mark_dirty(app);
                            let _ = InvalidateRect(Some(app.preview), None, true);
                        }
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_NOTIFY => {
                let hdr = &*(lparam.0 as *const NMHDR);
                if hdr.code == NOTIFY_TABCHANGE {
                    if let Some(app) = app_of(hwnd) {
                        let idx = send(app.tab, TCM_GETCURSEL, 0, 0);
                        app.cur_tab = (idx.max(0) as usize).min(TAB_COUNT - 1);
                        layout(app);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as usize;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if let Some(app) = app_of(hwnd) {
                    on_command(app, id, code);
                }
                LRESULT(0)
            }
            WM_SIZE => {
                if let Some(app) = app_of(hwnd) {
                    layout(app);
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                if let Some(app) = app_of(hwnd) {
                    app.dpi = GetDpiForWindow(hwnd).max(96);
                    app.set_fonts();
                    let places = app.placements.clone();
                    for (h, _) in places {
                        let _ = send(h, WM_SETFONT, app.font_ui.0 as usize, 1);
                    }
                    layout(app);
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96) as i32;
                mmi.ptMinTrackSize.x = 940 * dpi / 96;
                mmi.ptMinTrackSize.y = 640 * dpi / 96;
                LRESULT(0)
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

/// 页面窗口：把控件通知转发给主窗口处理。
unsafe extern "system" fn page_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND | WM_HSCROLL | WM_NOTIFY => {
                let parent = GetAncestor(hwnd, GA_PARENT);
                if !parent.0.is_null() {
                    return LRESULT(send(parent, msg, wparam.0, lparam.0));
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 颜色色块：画当前颜色；点击等同于点对应的「选择…」按钮。
unsafe extern "system" fn swatch_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let idx = (GetDlgCtrlID(hwnd) as usize).saturating_sub(ID_COLOR_BASE);
                let color = app_of(GetAncestor(hwnd, GA_ROOT))
                    .map(|a| color_get(&a.cfg.appearance, idx))
                    .unwrap_or(COLORREF(0));
                let brush = CreateSolidBrush(color);
                let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00B0B0B0));
                let ob = SelectObject(hdc, HGDIOBJ(brush.0));
                let op = SelectObject(hdc, HGDIOBJ(pen.0));
                let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, 6, 6);
                SelectObject(hdc, ob);
                SelectObject(hdc, op);
                let _ = DeleteObject(HGDIOBJ(brush.0));
                let _ = DeleteObject(HGDIOBJ(pen.0));
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_LBUTTONUP => {
                let idx = (GetDlgCtrlID(hwnd) as usize).saturating_sub(ID_COLOR_BASE);
                let page = GetAncestor(hwnd, GA_PARENT);
                let btn = ID_COLOR_BTN_BASE + idx;
                let _ = send(
                    page,
                    WM_COMMAND,
                    btn | ((BN_CLICKED as usize) << 16),
                    hwnd.0 as isize,
                );
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 候选窗预览（自绘，固定尺寸）。
unsafe extern "system" fn preview_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                if let Some(app) = app_of(GetAncestor(hwnd, GA_ROOT)) {
                    draw_preview(app, hdc, rc);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 画预览：浅灰底 + 候选窗（5 条候选，首条高亮），字号按设置换算（1pt = 4/3 px）。
unsafe fn draw_preview(app: &App, hdc: HDC, rc: RECT) {
    let a = &app.cfg.appearance;
    let s = |v: i32| ((v as f32) * (app.dpi as f32 / 96.0)).round() as i32;
    let dpi_scale = app.dpi as f32 / 96.0;
    let text_px = ((a.font_size as f32) * 4.0 / 3.0 * dpi_scale).round() as i32;
    let row_h = (text_px + s(14)).max(s(24));

    // 背景（模拟桌面）
    let bg = CreateSolidBrush(COLORREF(0x00F2F2F2));
    FillRect(hdc, &rc, bg);
    let _ = DeleteObject(HGDIOBJ(bg.0));

    let w = (rc.right - rc.left - s(40)).clamp(s(160), s(430));
    let h = row_h * 5 + s(12);
    let l = rc.left + (rc.right - rc.left - w) / 2;
    let t = rc.top + (rc.bottom - rc.top - h) / 2;
    let (r, b) = (l + w, t + h);

    let brush = CreateSolidBrush(hex_to_colorref(&a.bg));
    let pen = CreatePen(PS_SOLID, 1, hex_to_colorref(&a.border));
    let ob = SelectObject(hdc, HGDIOBJ(brush.0));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let radius = s(a.corner_radius as i32).max(2) * 2;
    let _ = RoundRect(hdc, l, t, r, b, radius, radius);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    let _ = DeleteObject(HGDIOBJ(brush.0));
    let _ = DeleteObject(HGDIOBJ(pen.0));

    // 圆角裁剪，避免高亮条溢出圆角
    let rgn = CreateRoundRectRgn(l + 1, t + 1, r, b, radius, radius);
    let _ = SelectClipRgn(hdc, Some(rgn));

    let name = wide("Microsoft YaHei UI");
    let mk_font = |px: i32| {
        CreateFontW(
            -px.max(9),
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
    };
    let font = mk_font(text_px);
    let idx_font = mk_font((text_px * 4 / 5).max(9));
    let _ = SetBkMode(hdc, TRANSPARENT);

    for (i, text) in ["你", "您", "牛", "鸟", "捏"].iter().enumerate() {
        let top = t + s(6) + (i as i32) * row_h;
        if i == 0 {
            let sel = CreateSolidBrush(hex_to_colorref(&a.selected_bg));
            let rr = RECT {
                left: l + 1,
                top,
                right: r - 1,
                bottom: top + row_h,
            };
            FillRect(hdc, &rr, sel);
            let _ = DeleteObject(HGDIOBJ(sel.0));
        }
        let color = if i == 0 {
            hex_to_colorref(&a.selected_text)
        } else {
            hex_to_colorref(&a.text)
        };
        let mut x_text = l + s(12);
        if a.show_index {
            let mut label = wide(&format!("{}.", i + 1));
            let mut ir = RECT {
                left: x_text,
                top,
                right: x_text + s(28),
                bottom: top + row_h,
            };
            SelectObject(hdc, HGDIOBJ(idx_font.0));
            let _ = SetTextColor(hdc, hex_to_colorref(&a.comment));
            DrawTextW(
                hdc,
                &mut label,
                &mut ir,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
            );
            x_text += s(26);
            SelectObject(hdc, HGDIOBJ(font.0));
        }
        let _ = SetTextColor(hdc, color);
        let mut t16 = wide(text);
        let mut tr = RECT {
            left: x_text,
            top,
            right: r - s(8),
            bottom: top + row_h,
        };
        DrawTextW(
            hdc,
            &mut t16,
            &mut tr,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
        );
    }
    let _ = SelectClipRgn(hdc, None);
    let _ = DeleteObject(HGDIOBJ(rgn.0));
    let _ = DeleteObject(HGDIOBJ(font.0));
    let _ = DeleteObject(HGDIOBJ(idx_font.0));
}
