//! 迅音输入法设置中心 —— 自绘 UI（无 WebView、也不使用系统控件外观）。
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

use std::os::windows::process::CommandExt;
use crate::core::{CellDict, Core, DictStats, SkinAsset};
use xunyin_rime::config::XunyinConfig;
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


// ---------------- 设计令牌（看这里就能改整套观感）----------------

// 圆角、字号、间距只留这几档 —— 参照水杉输入法设置页的做法：**不再到处写
// 12px / 10px / 13.5px 之类的中间值**，否则各页看起来就不是一套东西。
/// 卡片圆角
const R_CARD: i32 = 8;
/// 控件圆角（下拉、按钮、输入框、分段）
const R_CTRL: i32 = 8;
/// 小控件/列表行圆角
const R_ROW: i32 = 6;
/// 勾选框圆角
const R_BOX: i32 = 5;
/// 字号层级：页面标题 / 分组标题 / 正文 / 说明
const FS_PAGE: i32 = 18;
const FS_SECTION: i32 = 15;
const FS_BODY: i32 = 14;
const FS_CAPTION: i32 = 12;

// 纵向节奏也收成几档：卡片标题 42、每个设置行 46、卡片底部留白 10。
// 所有卡片高度都由 `card_h(行数)` 算出来，不再一页手写一串 92/226/330 ——
// 那样改一项就要跟着调数字，很容易出现"卡片比内容矮 4px、控件压到描边上"。
/// 卡片标题区（含上下留白）
const CARD_TITLE_H: i32 = 42;
/// 一个设置行（标签 + 右侧控件）
const ROW_H: i32 = 46;
/// 卡片底部留白
const CARD_PAD_B: i32 = 10;
/// 右侧控件统一宽度（下拉、分段、开关列）
const CTRL_W: i32 = 168;
/// 控件高度
const CTRL_H: i32 = 32;
/// 侧栏宽
const SIDE_W: i32 = 212;
/// 页面标题 + 一句说明的总高
const PAGE_HEAD_H: i32 = 64;
/// 两栏间距
const COL_GAP: i32 = 16;

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
    /// 控件描边（比卡片描边更明显一点，控件才"立"得起来）
    field_border: u32,
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
    /// 轨道/未打开开关的底色（中性灰，不带蓝味）
    track: u32,
    /// 危险操作（删除）
    danger: u32,
    /// 深色模式
    dark: bool,
}

/// 两栏流式卡片布局：每张卡片放进**当前较矮**的那一栏。
///
/// 以前页面是手工左栏 4 张、右栏 4 张，结果两栏差出几百像素：一栏早早到底、
/// 另一栏还要滚很久（实测最差 735px），既难看又要多滚。
struct Cols {
    x: [i32; 2],
    y: [i32; 2],
    w: i32,
    gap: i32,
    /// 宽窗两栏、窄窗单栏。窗口缩到最小（内容区约 670 逻辑像素）时，
    /// 两栏每张卡只剩 330 宽 —— 标签和右侧控件会挤在一起，所以直接单栏。
    single: bool,
}

impl Cols {
    fn new(x: i32, y0: i32, w: i32, gap: i32, single: bool) -> Self {
        let col_w = if single { w } else { (w - gap) / 2 };
        Cols {
            x: [x, x + col_w + gap],
            y: [y0, y0],
            w: col_w,
            gap,
            single,
        }
    }
    /// 指定放到哪一栏（需要固定阅读顺序时用，自动配平会打乱卡片的先后）。
    /// 单栏模式下忽略栏号、按调用顺序往下排。
    fn put(&mut self, col: usize, h: i32) -> (i32, i32, i32) {
        let col = if self.single { 0 } else { col };
        let p = (self.x[col], self.y[col], self.w);
        self.y[col] += h + self.gap;
        p
    }
    fn bottom(&self) -> i32 {
        if self.single {
            self.y[0] - self.gap
        } else {
            self.y[0].max(self.y[1]) - self.gap
        }
    }
}

/// 两栏卡片分配（保留给未来需要"自动配平"的页面；当前各页都用 Cols::put
/// 手工分栏，阅读顺序更可控）。
#[allow(dead_code)]
fn balance(hs: &[i32], x0: i32, y0: i32, w: i32, gap: i32) -> Vec<(i32, i32, i32)> {
    // 窗口窄了就退回**单栏**：两栏各自只剩三百来像素时，标签和右侧控件会挤在一起
    // （水杉输入法干脆固定单栏 + 最大宽度，就是这个道理）。这里保留宽屏两栏，
    // 但保证窄窗不会烂掉。
    if w < 880 {
        let mut y = y0;
        let mut out = Vec::with_capacity(hs.len());
        for h in hs {
            out.push((x0, y, w));
            y += h + gap;
        }
        return out;
    }
    let col_w = (w - gap) / 2;
    let xs = [x0, x0 + col_w + gap];
    let mut order: Vec<usize> = (0..hs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(hs[i]));
    let mut y = [y0, y0];
    let mut col_of = vec![0usize; hs.len()];
    for &i in &order {
        let c = if y[0] <= y[1] { 0 } else { 1 };
        col_of[i] = c;
        y[c] += hs[i] + gap;
    }
    let mut cursor = [y0, y0];
    let mut out = vec![(0, 0, col_w); hs.len()];
    for (i, h) in hs.iter().enumerate() {
        let c = col_of[i];
        out[i] = (xs[c], cursor[c], col_w);
        cursor[c] += h + gap;
    }
    out
}

/// 在资源管理器里定位到某个文件（explorer /select,<path>）。
fn reveal_in_explorer(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .creation_flags(0x0800_0000)
        .spawn();
}

/// 内容区页头：大标题 + 一句话说明（说明从 PAGES 里取，和侧栏保持同一套文案）。
/// 返回卡片起始 y。
fn page_header(app: &mut App, dc: HDC, x: i32, y: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let def = &PAGES[app.page];
    let title = def.title;
    let subtitle = def.subtitle;
    unsafe {
        text(
            dc,
            RECT { left: x, top: y, right: x + w, bottom: y + app.s(28) },
            title,
            app.font_title,
            t.text,
            DT_LEFT,
            true,
        );
        if !subtitle.is_empty() {
            text_ellipsis(
                dc,
                RECT { left: x, top: y + app.s(30), right: x + w, bottom: y + app.s(50) },
                subtitle,
                app.font_small,
                t.text3,
                DT_LEFT,
            );
        }
    }
    y + app.s(PAGE_HEAD_H)
}

fn theme_from(cfg: &XunyinConfig, dark: bool) -> Theme {
    let accent = hex_argb(&cfg.appearance.accent, 0xFF2A6FDB);
    // 深色底上同一个强调色会显暗：往白里提亮一点再用（水杉输入法也是这么做的：
    // 亮色主题 #6f6bc7、深色主题用更亮的 #8E8CD8）。用户自己配的主色只改色相不改这套关系。
    let accent = if dark {
        mix_argb(accent, 0xFFFFFFFF, 0.18)
    } else {
        accent
    };
    if dark {
        // 深色：**中性灰**（对齐 Win11 / 水杉输入法），不用带蓝味的深灰——
        // 蓝底 + 蓝字容易显脏、显旧。
        Theme {
            bg: 0xFF202020,
            sidebar: 0xFF181818,
            card: 0xFF262626,
            border: 0xFF3A3A3A,
            field_border: 0xFF4A4A4A,
            divider: 0xFF303030,
            text: 0xFFF0F0F0,
            text2: 0xFFC0C0C0,
            text3: 0xFF8C8C8C,
            hover: 0xFF2C2C2C,
            pressed: 0xFF383838,
            field: 0xFF2D2D2D,
            accent,
            // 选中项底色：**把强调色按低比例混进卡片底**，而不是拿强调色当底——
            // 后者会让深色文字压在中等饱和的蓝上，对比度只有 2:1 出头，读不清。
            accent_soft: mix_argb(0xFF262626, accent, 0.22),
            track: 0xFF4A4A4A,
            danger: 0xFFE8A0A0,
            dark: true,
        }
    } else {
        // 亮色：背景 #F3F3F3 + 白色卡片 + 8% 灰描边（Fluent / Notion 那一路）
        Theme {
            bg: 0xFFF3F3F3,
            sidebar: 0xFFFAFAFA,
            card: 0xFFFFFFFF,
            border: 0xFFE8E8E8,
            field_border: 0xFFDCDCDC,
            divider: 0xFFECECEC,
            text: 0xFF242424,
            text2: 0xFF5A5A5A,
            text3: 0xFF8A8A8A,
            hover: 0xFFF2F2F2,
            pressed: 0xFFE8E8E8,
            field: 0xFFFFFFFF,
            accent,
            accent_soft: mix_argb(0xFFFFFFFF, accent, 0.12),
            track: 0xFFDCDCDC,
            danger: 0xFFB42318,
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
    CornerRadius,
    PadX,
    PadY,
    MinWidth,
    CandGap,
    BarMargin,
    Fuzzy(usize),
    /// 模糊音快速预设：0 南方口音 1 前后鼻音 2 全部 3 清空
    FuzzyPreset(usize),
    PunctKey,
    PunctOut,
    PunctAdd,
    PunctDel,
    PunctRow(usize),
    DictImport,
    DictImportDir,
    DictExport,
    ImportDictExport,
    PhraseImport,
    StatsReset,
    PhraseQuery,
    PhraseCode,
    PhraseText,
    PhraseWeight,
    PhraseAdd,
    PhraseDel,
    PhraseRow(usize),
    BackupNow,
    Restore,
    BackupRow(usize),
    OpenData,
    OpenLogs,
    OpenSkins,
    SkinRestore,
    ClearLogs,
    ReloadConfig,
    Redeploy,
    ResetAll,
    Update,
    UpdateCheck,
    DiagExport,
    ConfigExport,
    ConfigImport,
    ClearUserDict,
    ExportUserDict,
    SegLayout,
    SegFollow,
    FontCombo,
    SkinPackExport,
    SkinPackImport,
    SkinItem(usize),
    CellDict(usize),
    CellApply,
    DictRollback,
    DictUpdateOne,
    UpdateInstall,
    DeleteKeyCombo,
    SwitchComment,
    SwitchAssoc,
    SwitchQuick,
    SwitchPerApp,
    SwitchEmoji,
    SwitchEnglish,
    SwitchPhrase,
    ComboItem(usize),
    /// 右侧滚动条的拖动命中区（内容超出窗口时才登记）
    ScrollBar,
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
    PhraseQuery,
    PhraseCode,
    PhraseText,
    PhraseWeight,
}

// ---------------- 应用状态 ----------------

/// 一页设置：图标、标题、一句说明、所属分组。
///
/// 以前只有 4 页，每页塞十来张卡片（"输入设置"一页里同时有方案、按键、模糊音、
/// 标点），找一项要来回扫。现在按"输入 / 外观 / 词库 / 其他"四组拆成 9 页，
/// 每页只放同一类设置；分组名直接画在侧栏里，用户在侧栏就能看出分类。
struct PageDef {
    icon: &'static str,
    title: &'static str,
    subtitle: &'static str,
    group: &'static str,
}

/// 侧栏图标取自 `Segoe Fluent Icons` / `Segoe MDL2 Assets` 的私有区码位，
/// 每个都在 Win10 的 MDL2 里用 tools/render-glyph.ps1 打过点阵确认。
/// E765 键盘、E8D4 键帽、E8CB 钥匙、E790 调色板、E7F4 显示器、
/// E8C4 状态条、E8F1 书架、E8A5 文档、E946 圆圈 i。
const PAGES: [PageDef; 9] = [
    PageDef { icon: "\u{E765}", title: "常规输入", subtitle: "输入方案、中英模式、候选来源与学习", group: "输入" },
    PageDef { icon: "\u{E8D4}", title: "按键与标点", subtitle: "回车、切换键、拼音分隔符与自定义标点", group: "输入" },
    PageDef { icon: "\u{E8CB}", title: "模糊音", subtitle: "按口音放宽声母、韵母的匹配", group: "输入" },
    PageDef { icon: "\u{E790}", title: "主题与配色", subtitle: "候选窗预览、主题预设与逐项配色", group: "外观" },
    PageDef { icon: "\u{E7F4}", title: "候选窗", subtitle: "排列、字体、字号、圆角与内边距", group: "外观" },
    PageDef { icon: "\u{E8C4}", title: "状态栏与皮肤", subtitle: "右下角状态栏与界面资源文件", group: "外观" },
    PageDef { icon: "\u{E8F1}", title: "词库管理", subtitle: "词库统计、细胞词库、导入与备份", group: "词库" },
    PageDef { icon: "\u{E8A5}", title: "自定义短语", subtitle: "按编码直达常用词句，添加后立即生效", group: "词库" },
    PageDef { icon: "\u{E946}", title: "关于与维护", subtitle: "版本、快捷键、维护与输入统计", group: "其他" },
];
const PAGES_EMOJI: [&str; 9] = ["⌨", "⌨", "🔑", "🎨", "▣", "▤", "📚", "📝", "ⓘ"];

struct App {
    hwnd: HWND,
    core: Core,
    cfg: XunyinConfig,
    saved: XunyinConfig,
    schemas: Vec<(String, String)>,
    stats: DictStats,
    /// 输入统计（上屏字数/速度；数据由常驻服务写，这里只读）
    input_stats: xunyin_rime::stats::Stats,
    backups: Vec<std::path::PathBuf>,
    /// 皮肤中心的资源列表（皮肤目录优先，否则内置）
    skins: Vec<SkinAsset>,
    /// 细胞词库勾选状态（未点「应用」前只是本地草稿）
    cells: Vec<CellDict>,
    cell_dirty: bool,
    /// 上游词库更新状态（版本/暂存/备份）
    dict_state: crate::core::DictUpdateState,
    /// 后台任务结果槽（下载/部署/回滚都在后台线程做，避免界面卡住）
    task: std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>,
    task_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 1=下载词库 2=部署词库 3=回滚词库（决定完成后做什么）
    task_kind: u8,
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
    /// 自定义短语表（原样保留注释行，编辑后整份写回）
    phrase_rows: Vec<xunyin_rime::phrases::Row>,
    /// 查询框内容：按编码或词条过滤
    phrase_query: String,
    /// 新增用的三个输入框
    phrase_code: String,
    phrase_text: String,
    phrase_weight: String,
    /// 当前选中的短语（在 phrase_rows 里的下标）
    phrase_sel: Option<usize>,
    combo_open: Option<Id>,
    dpi: u32,
    theme: Theme,
    font: HFONT,
    font_bold: HFONT,
    font_small: HFONT,
    font_title: HFONT,
    /// 分组标题（卡片标题）：比正文大一档、半粗
    font_section: HFONT,
    /// 图标字体（Segoe Fluent Icons / Segoe MDL2 Assets）；取不到时用 emoji 兜底
    font_icon: HFONT,
    icon_font_ok: bool,
    toast: String,
    toast_until: std::time::Instant,
    engine_ready: bool,
    dragging_slider: Option<Id>,
    /// 正在拖动滚动条时记下 (轨道顶, 轨道高, 滑块高)，鼠标移动据此换算滚动位置
    scroll_geom: Option<(i32, i32, i32)>,
    dragging_scroll: bool,
    gdiplus: usize,
    scroll_max: i32,
    /// 本帧画过的卡片矩形（XUNYIN_UI_DUMP 时导出，用于自动检查越界/重叠）
    cards: Vec<RECT>,
    /// 最近一次鼠标位置（悬停提示用）
    mouse: POINT,
}

fn main_of(hwnd: HWND) -> Option<&'static mut App> {
    unsafe {
        let p = app_ptr(hwnd);
        if p.is_null() {
            None
        } else {
            Some(&mut *p)
        }
    }
}

/// 窗口用户数据里存/取 App 指针。
///
/// 必须按指针宽度分情况：`SetWindowLongPtrW` 在 64 位下第三参是 `isize`，
/// 而在 **32 位下它就是 `SetWindowLongW`，第三参是 `i32`**。直接写
/// `ptr as isize` 的结果是 x86 目标**编译不过** —— 安装包里那份 32 位设置中心
/// 就是这么一直缺着的（它压根没被成功构建出来过）。
unsafe fn set_app_ptr(hwnd: HWND, app: *mut App) {
    #[cfg(target_pointer_width = "64")]
    let value = app as isize;
    #[cfg(target_pointer_width = "32")]
    let value = app as i32;
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, value);
}

unsafe fn app_ptr(hwnd: HWND) -> *mut App {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    // 32 位下拿到的是 i32：先当无符号再转指针，避免负值符号扩展
    #[cfg(target_pointer_width = "64")]
    {
        raw as *mut App
    }
    #[cfg(target_pointer_width = "32")]
    {
        (raw as u32 as usize) as *mut App
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

// 直接用自己声明的 `DrawTextW`，不走 windows crate 那个包装：那个包装把
// **含结尾 NUL 的缓冲区长度**当作 `cchTextLength` 传下去，于是结尾的 NUL 被当成
// 一个字符，画成 .notdef 方框——界面上就是每段文字后面跟一个小方块
// （图标旁边那一格看起来最像一个多余的 `[`）。
#[link(name = "user32")]
extern "system" {
    #[link_name = "DrawTextW"]
    fn draw_text_w(hdc: HDC, text: *const u16, cch: i32, rect: *mut RECT, format: u32) -> i32;
}

/// 按"不含结尾 NUL 的字符数"调用 GDI 的 DrawTextW。
unsafe fn draw_text_len(dc: HDC, rect: &mut RECT, buf: &[u16], flags: DRAW_TEXT_FORMAT) {
    let cch = buf.len().saturating_sub(1) as i32; // 去掉 wide() 补的那个 0
    let _ = draw_text_w(dc, buf.as_ptr(), cch, rect, flags.0);
}

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
    // 主题色是 0xAARRGGBB，GDI 的 COLORREF 是 0x00BBGGRR —— 必须先换字节序，
    // 否则强调色会变成橙色、危险红会变成蓝色（灰度色看不出，所以一直没暴露）。
    let _ = SetTextColor(dc, COLORREF(argb_to_colorref(color)));
    let buf = wide(s);
    let mut rect = r;
    let mut flags = DRAW_TEXT_FORMAT(align.0 | DT_NOPREFIX.0);
    if vcenter {
        flags = DRAW_TEXT_FORMAT(flags.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
    } else {
        flags = DRAW_TEXT_FORMAT(flags.0 | DT_WORDBREAK.0);
    }
    draw_text_len(dc, &mut rect, &buf, flags);
    SelectObject(dc, old);
}

/// 单行文字，超宽时用省略号收尾（GDI 的 DT_END_ELLIPSIS）。
/// 说明文字放在卡片左半栏，写长一点就会被硬切；用省略号至少看得出"还有下文"。
unsafe fn text_ellipsis(
    dc: HDC,
    r: RECT,
    s: &str,
    font: HFONT,
    color: u32,
    align: DRAW_TEXT_FORMAT,
) {
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let _ = SetBkMode(dc, TRANSPARENT);
    let _ = SetTextColor(dc, COLORREF(argb_to_colorref(color)));
    let buf = wide(s);
    let mut rect = r;
    let flags =
        DRAW_TEXT_FORMAT(align.0 | DT_NOPREFIX.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);
    draw_text_len(dc, &mut rect, &buf, flags);
    SelectObject(dc, old);
}

/// ARGB → COLORREF（GDI 文字/画笔用）。
///
/// ARGB 的字节序是 A R G B，COLORREF 是 0x00BBGGRR —— 要做的是把 **R 与 B 对调**。
/// 以前这里取的是低字节当 R（其实是 B），于是强调色文字画成了橙色、
/// 危险红画成了蓝色；因为大多数文字是灰/白（R=G=B），一直没被看出来。
fn argb_to_colorref(c: u32) -> u32 {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    r | (g << 8) | (b << 16)
}

impl App {
    /// 构造：读取配置、初始化 GDI+ 与字体。
    fn create(hwnd: HWND) -> Box<App> {
        let core = Core::new();
        let cfg = core.load_config();
        let skins = core.skin_assets();
        let cells = core.cell_dicts();
        let dict_state = core.dict_update_state();
        let dark = system_prefers_dark();
        let theme = theme_from(&cfg, dark);
        let mut app = Box::new(App {
            hwnd,
            core,
            saved: cfg.clone(),
            cfg,
            schemas: Vec::new(),
            stats: DictStats::default(),
            input_stats: xunyin_rime::stats::Stats::default(),
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
            phrase_rows: xunyin_rime::phrases::read_rows(),
            phrase_query: String::new(),
            phrase_code: String::new(),
            phrase_text: String::new(),
            phrase_weight: String::new(),
            phrase_sel: None,
            skins,
            cells,
            cell_dirty: false,
            dict_state,
            task: std::sync::Arc::new(std::sync::Mutex::new(None)),
            task_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_kind: 0,
            combo_open: None,
            dpi: 96,
            theme,
            font: HFONT::default(),
            font_bold: HFONT::default(),
            font_small: HFONT::default(),
            font_title: HFONT::default(),
            font_section: HFONT::default(),
            font_icon: HFONT::default(),
            icon_font_ok: false,
            toast: String::new(),
            toast_until: std::time::Instant::now(),
            engine_ready: false,
            dragging_slider: None,
            scroll_geom: None,
            dragging_scroll: false,
            gdiplus: 0,
            scroll_max: 0,
            cards: Vec::new(),
            mouse: POINT { x: 0, y: 0 },
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
        // 调试用：XUNYIN_UI_PAGE=0..8 直接打开指定页（配合 XUNYIN_UI_DUMP 检查布局）
        if let Ok(p) = std::env::var("XUNYIN_UI_PAGE") {
            if let Ok(n) = p.trim().parse::<usize>() {
                app.page = n.min(PAGES.len() - 1);
            }
        }
        app
    }

    // ---------------- 绘制：控件 ----------------

    fn push_hit(&mut self, id: Id, r: RECT) {
        self.hits.push(Hit { id, rect: r });
    }

    /// 短语表当前要显示的行（按查询词过滤；空查询显示全部）。
    /// 返回的是 `phrase_rows` 里的下标，点击/删除都靠它定位。
    fn phrase_view(&self) -> Vec<usize> {
        let q = self.phrase_query.trim().to_lowercase();
        self.phrase_rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                xunyin_rime::phrases::Row::Entry(e) => {
                    if q.is_empty()
                        || e.codes.to_lowercase().contains(&q)
                        || e.text.to_lowercase().contains(&q)
                    {
                        Some(i)
                    } else {
                        None
                    }
                }
                xunyin_rime::phrases::Row::Comment(_) => None,
            })
            .collect()
    }

    /// 短语条数（写进概况统计）。
    fn phrase_count(&self) -> usize {
        self.phrase_rows
            .iter()
            .filter(|r| matches!(r, xunyin_rime::phrases::Row::Entry(_)))
            .count()
    }

    /// 整表写回文件（服务侧 2 秒内自动读到，不用重新部署）。
    fn save_phrases(&mut self) {
        match xunyin_rime::phrases::write_rows(&self.phrase_rows) {
            Ok(()) => {
                self.stats.custom_phrase_count = self.phrase_count();
                let n = self.phrase_count();
                // 服务侧的叠加候选是**立刻**生效的；这里再写一条重载请求，让
                // librime 自己的 custom_phrase 表也跟上（引擎重建约 1 秒，不是重建索引）
                xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
                self.toast(&format!("短语已保存（共 {n} 条），输入时立刻生效"));
            }
            Err(e) => self.toast(&format!("短语保存失败：{e}")),
        }
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
    fn card(&mut self, dc: HDC, x: i32, y: i32, w: i32, h: i32, title: &str, desc: &str) -> i32 {
        let t = self.theme.clone();
        let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        self.cards.push(r);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                // 轻投影：1px 偏移的一层柔边（原来 2px/35% 太重，卡片像浮在半空）
                g.fill_round(
                    RECT { left: r.left, top: r.top + 1, right: r.right, bottom: r.bottom + 1 },
                    R_CARD,
                    argb(0x00000000, if t.dark { 0.28 } else { 0.05 }),
                );
                g.fill_round(r, R_CARD, t.card);
                g.stroke_round(r, R_CARD, t.border, 1.0);
            }
            let ty = y + self.s(14);
            let tr = RECT { left: x + self.s(18), top: ty, right: x + w - self.s(18), bottom: ty + self.s(22) };
            text(dc, tr, title, self.font_section, t.text, DT_LEFT, true);
            // 标题右侧一行小字：这张卡是干什么的。
            // 传空串就不画 —— 大多数卡片名本身就够清楚，只在容易混淆的地方写。
            if !desc.is_empty() {
                text(dc, tr, desc, self.font_small, t.text3, DT_RIGHT, true);
            }
        }
        // 与 card_h() 的 CARD_TITLE_H 对齐；这里以前写死 14/24，150% 缩放下
        // 会比卡片高度少 17px，第一行贴着标题。
        y + self.s(CARD_TITLE_H)
    }

    /// 行标签（左） + 右侧控件；返回行高。
    fn row(&mut self, dc: HDC, x: i32, y: i32, w: i32, label: &str, desc: &str) -> RECT {
        let t = self.theme.clone();
        unsafe {
            if desc.is_empty() {
                // 只有标签时垂直居中，整行看起来不会偏上
                let lr = RECT { left: x, top: y + self.s(2), right: x + w / 2, bottom: y + self.s(42) };
                text(dc, lr, label, self.font, t.text, DT_LEFT, true);
            } else {
                // 标签在上、说明在下：两行各占 20px，中间留 2px，
                // 以前标签 4..24、说明 22..40，两行会压在一起。
                let lr = RECT { left: x, top: y + self.s(1), right: x + w / 2, bottom: y + self.s(22) };
                text(dc, lr, label, self.font, t.text, DT_LEFT, true);
                let dr = RECT { left: x, top: y + self.s(23), right: x + w / 2, bottom: y + self.s(42) };
                text_ellipsis(dc, dr, desc, self.font_small, t.text3, DT_LEFT);
            }
        }
        RECT { left: x + w / 2, top: y + self.s(4), right: x + w, bottom: y + self.s(40) }
    }

    /// 一行标准设置：标签在左、开关在右。所有页面都走这几个封装，
    /// 控件宽度、行高、垂直居中就不会各页一个样。
    fn row_toggle(&mut self, dc: HDC, x: i32, y: i32, w: i32, label: &str, desc: &str, id: Id, on: bool) {
        let r = self.row(dc, x, y, w, label, desc);
        self.toggle_switch(dc, id, r, on);
    }

    fn row_combo(&mut self, dc: HDC, x: i32, y: i32, w: i32, label: &str, desc: &str, id: Id, value: &str) {
        let r = self.row(dc, x, y, w, label, desc);
        let cb = self.row_ctl(r, CTRL_W);
        self.combo(dc, id, cb, value);
    }

    fn row_seg(&mut self, dc: HDC, x: i32, y: i32, w: i32, label: &str, desc: &str, id: Id, options: &[&str], sel: usize) {
        let r = self.row(dc, x, y, w, label, desc);
        let seg = self.ctl_seg(r);
        self.segmented(dc, id, seg, options, sel);
    }

    fn row_slider(
        &mut self,
        dc: HDC,
        x: i32,
        y: i32,
        w: i32,
        label: &str,
        desc: &str,
        id: Id,
        value: i32,
        min: i32,
        max: i32,
        suffix: &str,
    ) {
        let r = self.row(dc, x, y, w, label, desc);
        let sl = self.ctl_slider(r);
        self.slider(dc, id, sl, value, min, max, suffix);
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
                // 轨道用"卡片底 + 一点文字色"，别用纯白：纯白轨道放在白卡片上
                // 等于没画，用户看不出这是一组分段按钮。
                let track_bg = if t.dark {
                    mix_argb(t.card, 0xFFFFFFFF, 0.07)
                } else {
                    mix_argb(t.card, t.text, 0.045)
                };
                g.fill_round(r, R_CTRL, track_bg);
                for (i, _) in options.iter().enumerate() {
                    if i == sel {
                        let sr = RECT {
                            left: r.left + (i as i32) * seg + 2,
                            top: r.top + 2,
                            right: r.left + ((i as i32) + 1) * seg - 2,
                            bottom: r.bottom - 2,
                        };
                        let sel_bg = if t.dark {
                            mix_argb(t.card, 0xFFFFFFFF, 0.12)
                        } else {
                            t.card
                        };
                        g.fill_round(sr, R_CTRL - 2, sel_bg);
                        g.stroke_round(
                            sr,
                            R_CTRL - 2,
                            if t.dark { t.field_border } else { t.border },
                            1.0,
                        );
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
                Id::SegLayout => Id::ComboItem(400 + i),
                Id::SegFollow => Id::ComboItem(500 + i),
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
                g.fill_round(tr, track_h / 2, t.track);
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
                    if self.dragging_slider == Some(id) { t.accent } else { t.field_border },
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
                g.fill_round(r, R_CTRL, if hot { t.hover } else { t.field });
                g.stroke_round(r, R_CTRL, if hot { t.accent } else { t.field_border }, 1.0);
            }
            let tr = RECT { left: r.left + self.s(12), top: r.top, right: r.right - self.s(34), bottom: r.bottom };
            text_ellipsis(dc, tr, value, self.font, t.text, DT_LEFT);
            // 展开箭头
            let (cx, cy) = (r.right - self.s(16), (r.top + r.bottom) / 2);
            gdi_polyline(
                dc,
                &[
                    (cx - self.s(5), cy - self.s(2)),
                    (cx, cy + self.s(3)),
                    (cx + self.s(5), cy - self.s(2)),
                ],
                t.text3,
                self.s(2).max(1),
            );
        }
        self.push_hit(id, r);
    }

    fn button(&mut self, dc: HDC, id: Id, r: RECT, label: &str, primary: bool) {
        self.button_ex(dc, id, r, label, primary, false)
    }

    /// 危险操作（删除 / 清空）：中性底 + 红色文字，别用大红色实心块抢视线。
    fn button_danger(&mut self, dc: HDC, id: Id, r: RECT, label: &str) {
        self.button_ex(dc, id, r, label, false, true)
    }

    /// 轻量强调按钮：浅色强调底 + 强调色文字。用于"快捷预设"这类次要入口，
    /// 一页里既有主按钮又有它时，不会像三颗实心蓝按钮那样抢视线。
    fn button_soft(&mut self, dc: HDC, id: Id, r: RECT, label: &str) {
        let t = self.theme.clone();
        let hot = self.hover == Some(id);
        let down = self.pressed == Some(id);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                let bg = if down {
                    mix_argb(t.accent_soft, t.accent, 0.18)
                } else if hot {
                    mix_argb(t.accent_soft, t.accent, 0.10)
                } else {
                    t.accent_soft
                };
                g.fill_round(r, R_CTRL, bg);
            }
            text(dc, r, label, self.font, t.accent, DT_CENTER, true);
        }
        self.push_hit(id, r);
    }

    fn button_ex(&mut self, dc: HDC, id: Id, r: RECT, label: &str, primary: bool, danger: bool) {
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
                } else if danger {
                    (if down { t.pressed } else if hot { t.hover } else { t.card }, t.danger)
                } else {
                    (
                        if down { t.pressed } else if hot { t.hover } else { t.card },
                        t.text,
                    )
                };
                g.fill_round(r, R_CTRL, bg);
                if !primary {
                    g.stroke_round(r, R_CTRL, t.field_border, 1.0);
                }
                let _ = fg;
            }
            text(
                dc,
                r,
                label,
                self.font,
                if primary {
                    0xFFFFFFFF
                } else if danger {
                    t.danger
                } else {
                    t.text
                },
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
                    g.fill_round(br, R_BOX, t.accent);
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
                    g.fill_round(br, R_BOX, t.card);
                    g.stroke_round(br, R_BOX, if self.hover == Some(id) { t.accent } else { t.text3 }, 1.0);
                }
            }
            let lr = RECT { left: br.right + self.s(9), top: r.top, right: r.right, bottom: r.bottom };
            text(dc, lr, label, self.font_small, t.text, DT_LEFT, true);
        }
        self.push_hit(id, r);
    }

    fn chip(&mut self, dc: HDC, id: Id, r: RECT, color: u32) {
        let t = self.theme.clone();
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, R_ROW, color);
                g.stroke_round(r, R_ROW, t.field_border, 1.0);
            }
        }
        self.push_hit(id, r);
    }

    fn text_field(&mut self, dc: HDC, id: Id, r: RECT, value: &str, placeholder: &str) {
        let t = self.theme.clone();
        let focused = matches!(
            (id, self.focus),
            (Id::PunctKey, Focus::PunctKey)
                | (Id::PunctOut, Focus::PunctOut)
                | (Id::PhraseQuery, Focus::PhraseQuery)
                | (Id::PhraseCode, Focus::PhraseCode)
                | (Id::PhraseText, Focus::PhraseText)
                | (Id::PhraseWeight, Focus::PhraseWeight)
        );
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, R_CTRL, t.field);
                g.stroke_round(r, R_CTRL, if focused { t.accent } else { t.field_border }, 1.0);
            }
            let tr = RECT { left: r.left + self.s(10), top: r.top, right: r.right - self.s(8), bottom: r.bottom };
            if value.is_empty() && !focused {
                text(dc, tr, placeholder, self.font_small, t.text3, DT_LEFT, true);
            } else {
                text(dc, tr, value, self.font, t.text, DT_LEFT, true);
                if focused {
                    // 光标
                    let w = App::text_width_of(dc, value, self.font);
                    let cx = tr.left + w + 1;
                    if let Some(g) = Gfx::new(dc) {
                        let cr = RECT { left: cx, top: r.top + self.s(5), right: cx + self.s(2), bottom: r.bottom - self.s(5) };
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
                g.fill_round(r, R_CTRL, t.field);
                g.stroke_round(r, R_CTRL, t.field_border, 1.0);
            }
        }
        if items.is_empty() {
            let er = RECT { left: r.left + self.s(12), top: r.top, right: r.right - self.s(12), bottom: r.bottom };
            unsafe {
                text(dc, er, empty, self.font_small, t.text3, DT_LEFT, true);
            }
            return;
        }
        let row_h = self.s(26);
        for (i, it) in items.iter().enumerate() {
            let top = r.top + self.s(4) + (i as i32) * row_h;
            if top + row_h > r.bottom - self.s(4) {
                break;
            }
            let rr = RECT { left: r.left + self.s(4), top, right: r.right - self.s(4), bottom: top + row_h };
            let rid = id_of_row(i);
            let is_sel = selected == Some(i);
            let hot = self.hover == Some(rid);
            unsafe {
                if let Some(g) = Gfx::new(dc) {
                    if is_sel {
                        g.fill_round(rr, R_ROW, t.accent_soft);
                    } else if hot {
                        g.fill_round(rr, R_ROW, t.hover);
                    }
                }
                let tr = RECT { left: rr.left + self.s(8), top: rr.top, right: rr.right - self.s(8), bottom: rr.bottom };
                text(dc, tr, it, self.font_small, if is_sel { t.accent } else { t.text }, DT_LEFT, true);
            }
            self.push_hit(rid, rr);
        }
    }

unsafe fn text_width_of(dc: HDC, s: &str, font: HFONT) -> i32 {
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let buf = wide(s);
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &buf[..buf.len() - 1], &mut sz);
    SelectObject(dc, old);
    sz.cx
}

/// 字体族是否真的存在。
/// `CreateFontW` 对不存在的族名**不报错**，只会在选进 DC 时静默替换成别的字体，
/// 于是图标码位会渲染成方框；所以这里选进一个临时 DC 再回读实际字体名。
unsafe fn font_face_exists(font: &HFONT) -> bool {
    let face = Self::font_face_name(font);
    face.contains("Fluent") || face.contains("MDL2")
}

/// 字体实际选中的字面名（CreateFontW 对不存在的族名会静默替换，所以要看这个）。
unsafe fn font_face_name(font: &HFONT) -> String {
    if font.0.is_null() {
        return String::new();
    }
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return String::new();
    }
    let old = SelectObject(dc, HGDIOBJ(font.0));
    let mut buf = [0u16; 64];
    let n = GetTextFaceW(dc, Some(&mut buf));
    SelectObject(dc, old);
    let _ = DeleteDC(dc);
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n as usize - 1).min(buf.len())])
}
    fn s(&self, v: i32) -> i32 {
        ((v as f32) * (self.dpi as f32 / 96.0)).round() as i32
    }

    /// `n` 个标准设置行的卡片高度（标题 + 行 + 底部留白）。
    fn card_h(&self, rows: i32) -> i32 {
        self.s(CARD_TITLE_H + rows * ROW_H + CARD_PAD_B)
    }

    /// 按当前内容区宽度决定单栏还是两栏。阈值取 780 逻辑像素：
    /// 默认 1160 宽窗口（150% 下内容区约 890）是两栏，缩到最小（约 690）自动单栏。
    fn cols(&self, x: i32, y: i32, w: i32) -> Cols {
        Cols::new(x, y, w, self.s(COL_GAP), w < self.s(780))
    }

    /// 内容不是标准行时：卡片高度 = 标题 + 自定义内容高 + 底部留白。
    fn card_h_inner(&self, inner: i32) -> i32 {
        self.s(CARD_TITLE_H) + inner + self.s(CARD_PAD_B)
    }

    /// 设置行右侧控件区（下拉 / 分段 / 开关列统一右对齐、同高）。
    /// 以前每张卡片各写一个宽度（160 / 230 / 全宽），右边缘对不齐，看着就散。
    fn row_ctl(&self, r: RECT, w: i32) -> RECT {
        RECT {
            left: r.right - self.s(w),
            top: r.top + self.s(4),
            right: r.right,
            bottom: r.top + self.s(4) + self.s(CTRL_H),
        }
    }

    /// 分段控件比下拉略矮，底边与下拉对齐。
    fn ctl_seg(&self, r: RECT) -> RECT {
        RECT {
            left: r.right - self.s(CTRL_W),
            top: r.top + self.s(6),
            right: r.right,
            bottom: r.top + self.s(6) + self.s(CTRL_H),
        }
    }

    /// 滑块：从行中段开始，右侧留出数值文字的宽度。
    fn ctl_slider(&self, r: RECT) -> RECT {
        RECT {
            left: r.left,
            top: r.top + self.s(6),
            right: r.right - self.s(82),
            bottom: r.top + self.s(6) + self.s(22),
        }
    }

    /// 一行内等宽按钮的位置（从左到右）。
    fn btn_row(&self, x: i32, y: i32, w: i32, n: i32, h: i32) -> Vec<RECT> {
        let gap = self.s(8);
        let bw = (w - gap * (n - 1)) / n;
        (0..n)
            .map(|i| RECT {
                left: x + i * (bw + gap),
                top: y,
                right: x + i * (bw + gap) + bw,
                bottom: y + self.s(h),
            })
            .collect()
    }

    fn rebuild_fonts(&mut self) {
        unsafe {
            for f in [
                self.font,
                self.font_bold,
                self.font_small,
                self.font_title,
                self.font_section,
                self.font_icon,
            ] {
                if !f.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(f.0));
                }
            }
            let mk_family = |family: &str, px: i32, weight: i32| {
                let fam = wide(family);
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
                    PCWSTR(fam.as_ptr()),
                )
            };
            let mk = |px: i32, weight: i32| mk_family("Microsoft YaHei UI", px, weight);
            self.font = mk(FS_BODY, FW_NORMAL.0 as i32);
            self.font_bold = mk(FS_BODY, FW_SEMIBOLD.0 as i32);
            self.font_small = mk(FS_CAPTION, FW_NORMAL.0 as i32);
            self.font_title = mk(FS_PAGE, FW_SEMIBOLD.0 as i32);
            self.font_section = mk(FS_SECTION, FW_SEMIBOLD.0 as i32);
            // 侧栏图标：Win11 是 Segoe Fluent Icons，Win10 是 Segoe MDL2 Assets，
            // 两者这几个码位是同一套（都实测过点阵）。都没有（精简系统）就退回 emoji。
            let mut icon_ok = false;
            for family in ["Segoe Fluent Icons", "Segoe MDL2 Assets"] {
                let f = mk_family(family, 20, FW_NORMAL.0 as i32);
                if Self::font_face_exists(&f) {
                    self.font_icon = f;
                    icon_ok = true;
                    break;
                }
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
            self.icon_font_ok = icon_ok;
            // 诊断：XUNYIN_UI_DUMP 时把侧栏图标实际用的字体记下来（排查图标显示问题用）
            if let Ok(dir) = std::env::var("XUNYIN_UI_DUMP") {
                let face = if icon_ok {
                    Self::font_face_name(&self.font_icon)
                } else {
                    "（无图标字体，用 emoji 兜底）".to_string()
                };
                let _ = std::fs::write(std::path::Path::new(&dir).join("icon-font.txt"), face);
            }
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

/// 模糊音组（id 必须与 xunyin-rime::config::FUZZY_GROUPS 一致）。
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

/// 删词键（误上屏的词：Ctrl+数字 删掉当前页第 N 个候选）。
const DELETE_ITEMS: [(&str, &str); 2] = [
    ("ctrl_digit", "Ctrl + 数字"),
    ("off", "关闭"),
];

/// 候选窗字体（名称必须是系统里真实存在的字体族）。
const FONT_ITEMS: [(&str, &str); 5] = [
    ("Microsoft YaHei UI", "微软雅黑 UI"),
    ("Microsoft YaHei", "微软雅黑"),
    ("SimSun", "宋体"),
    ("SimHei", "黑体"),
    ("DengXian", "等线"),
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
    // 任何一项手动改色都算自定义配色：否则「浅色」预设还亮着，但实际颜色已经不同
    a.theme = "custom".into();
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

/// 当前配色是否就是某个预设（用来把选中预设画成实心按钮）。
fn theme_preset_active(app: &App, i: usize) -> bool {
    let a = &app.cfg.appearance;
    match i {
        0 => a.theme == "light",
        1 => a.theme == "dark",
        _ => {
            a.theme == "custom"
                && a.bg.eq_ignore_ascii_case("#F4F8F1")
                && a.accent.eq_ignore_ascii_case("#3E8E41")
        }
    }
}
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

/// 深色模式下把标题栏也切成深色：否则内容是全黑、标题栏还是白的，很割裂。
/// Win10 1809 用的属性号是 19，之后的系统是 20，先试 20 再退回 19。
fn set_dark_title_bar(hwnd: HWND, dark: bool) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWINDOWATTRIBUTE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    let value = windows::core::BOOL::from(dark);
    let ptr = &value as *const _ as *const core::ffi::c_void;
    let cb = size_of::<windows::core::BOOL>() as u32;
    unsafe {
        if DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, ptr, cb).is_err() {
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(19), ptr, cb);
        }
    }
}
// ---------------- 预览 / 下拉浮层 / 交互 ----------------

impl App {
    fn punct_sel(&self) -> Option<usize> {
        self.punct_sel
    }

    /// 候选窗预览：桌面底 + 圆角候选窗（首条高亮）。
    ///
    /// 这里尽量用**真实配置**画：字号、字体族、圆角、左右/上下内边距、序号、
    /// 注释、横排/竖排都跟着设置走 —— 用户拖一下滑块就能看到差别，而不是看一张
    /// 永远不变的示意图。
    fn draw_candidate_preview(&self, dc: HDC, r: RECT) {
        let a = &self.cfg.appearance;
        let t = &self.theme;
        let s = |v: i32| self.s(v);
        let dpi_scale = self.dpi as f32 / 96.0;
        let text_px = ((a.font_size as f32) * 4.0 / 3.0 * dpi_scale).round() as i32;
        let row_h = (text_px + s(14)).max(s(24));
        let radius = a.corner_radius.clamp(0, 24) as i32;
        let pad_x = self.s(a.cand_pad_x.min(40) as i32);
        let pad_y = self.s(a.cand_pad_y.min(24) as i32);
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                g.fill_round(r, 10, if t.dark { 0xFF1B1D22 } else { 0xFFF0F2F6 });
            }
            let avail_w = (r.right - r.left - s(36)).max(s(160));
            let w = avail_w
                .min(s(420))
                .max(self.s(a.cand_min_width.min(320) as i32));
            let inner_w = (w - pad_x * 2).max(s(40));
            let chars = ["你", "您", "牛", "鸟", "捏"];

            // 字体族用真实配置（以前写死微软雅黑，换字体预览不变）
            let family = if a.font_family.trim().is_empty() {
                "Microsoft YaHei UI"
            } else {
                a.font_family.as_str()
            };
            let fam = wide(family);
            let mk = |px: i32, bold: bool| {
                CreateFontW(
                    -px.max(9),
                    0, 0, 0,
                    if bold { FW_SEMIBOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                    0, 0, 0,
                    DEFAULT_CHARSET,
                    OUT_TT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    PCWSTR(fam.as_ptr()),
                )
            };
            let font = mk(text_px, false);
            let small = mk((text_px * 4 / 5).max(9), false);
            let idx_font = mk((text_px * 4 / 5).max(9), false);

            let rr;
            if a.layout == "horizontal" {
                // 横排：单行，项间距取配置的候选间距
                let gap = s(a.cand_gap.min(48) as i32).clamp(s(6), s(30));
                let mut n = 5i32;
                while n > 1 && (inner_w - gap * (n - 1)) / n < s(46) {
                    n -= 1;
                }
                let cell_w = ((inner_w - gap * (n - 1)) / n).max(s(36));
                let h = row_h + pad_y * 2;
                let l = r.left + (r.right - r.left - w) / 2;
                let top0 = r.top + (r.bottom - r.top - h) / 2;
                rr = RECT { left: l, top: top0, right: l + w, bottom: top0 + h };
                if let Some(g) = Gfx::new(dc) {
                    g.fill_round(rr, radius, hex_argb(&a.bg, 0xFFFFFFFF));
                    g.stroke_round(rr, radius, hex_argb(&a.border, 0xFFD8DCE3), 1.0);
                }
                let rgn = CreateRoundRectRgn(rr.left + 1, rr.top + 1, rr.right, rr.bottom, (radius * 2).max(1), (radius * 2).max(1));
                let _ = SelectClipRgn(dc, Some(rgn));
                for i in 0..n {
                    let left = rr.left + pad_x + i * (cell_w + gap);
                    let cell = RECT { left, top: rr.top + pad_y, right: left + cell_w, bottom: rr.top + pad_y + row_h };
                    if i == 0 {
                        if let Some(g) = Gfx::new(dc) {
                            g.fill_round(cell, 4, hex_argb(&a.selected_bg, 0xFF2A6FDB));
                        }
                    }
                    let color = if i == 0 {
                        hex_argb(&a.selected_text, 0xFFFFFFFF)
                    } else {
                        hex_argb(&a.text, 0xFF222222)
                    };
                    let label = if a.show_index {
                        format!("{}.{}", i + 1, chars[i as usize])
                    } else {
                        chars[i as usize].to_string()
                    };
                    text(dc, cell, &label, font, color, DT_CENTER, true);
                }
                let _ = SelectClipRgn(dc, None);
                let _ = DeleteObject(HGDIOBJ(rgn.0));
            } else {
                // 竖排：高度按字号与内边距算，最多 5 行
                let box_h = (r.bottom - r.top - s(24)).max(s(80));
                let max_rows = (((box_h - pad_y * 2) / row_h).max(1)).min(5);
                let h = row_h * max_rows + pad_y * 2;
                let l = r.left + (r.right - r.left - w) / 2;
                let top0 = r.top + (r.bottom - r.top - h) / 2;
                rr = RECT { left: l, top: top0, right: l + w, bottom: top0 + h };
                if let Some(g) = Gfx::new(dc) {
                    g.fill_round(rr, radius, hex_argb(&a.bg, 0xFFFFFFFF));
                    g.stroke_round(rr, radius, hex_argb(&a.border, 0xFFD8DCE3), 1.0);
                }
                let rgn = CreateRoundRectRgn(rr.left + 1, rr.top + 1, rr.right, rr.bottom, (radius * 2).max(1), (radius * 2).max(1));
                let _ = SelectClipRgn(dc, Some(rgn));
                for (i, label) in chars.iter().take(max_rows as usize).enumerate() {
                    let top = rr.top + pad_y + (i as i32) * row_h;
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
                    let comment_color = if i == 0 {
                        mix_argb(
                            hex_argb(&a.selected_text, 0xFFFFFFFF),
                            hex_argb(&a.selected_bg, 0xFF2A6FDB),
                            0.28,
                        )
                    } else {
                        hex_argb(&a.comment, 0xFF999999)
                    };
                    let mut xt = rr.left + pad_x;
                    if a.show_index {
                        let ir = RECT { left: xt, top, right: xt + s(24), bottom: top + row_h };
                        let idx_color = if i == 0 {
                            mix_argb(
                                hex_argb(&a.selected_text, 0xFFFFFFFF),
                                hex_argb(&a.selected_bg, 0xFF2A6FDB),
                                0.35,
                            )
                        } else {
                            hex_argb(&a.comment, 0xFF999999)
                        };
                        text(dc, ir, &format!("{}.", i + 1), idx_font, idx_color, DT_LEFT, true);
                        xt += s(22);
                    }
                    let tr = RECT { left: xt, top, right: rr.right - pad_x, bottom: top + row_h };
                    text(dc, tr, label, font, color, DT_LEFT, true);
                    if a.show_comment {
                        let tw = Self::text_width_of(dc, label, font);
                        let cr = RECT {
                            left: xt + tw + s(10),
                            top,
                            right: rr.right - pad_x,
                            bottom: top + row_h,
                        };
                        text_ellipsis(dc, cr, "nǐ", small, comment_color, DT_LEFT);
                    }
                }
                let _ = SelectClipRgn(dc, None);
                let _ = DeleteObject(HGDIOBJ(rgn.0));
            }
            let _ = DeleteObject(HGDIOBJ(font.0));
            let _ = DeleteObject(HGDIOBJ(small.0));
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
        // 靠近窗口底部时向上展开：否则下拉会被窗口裁掉一半（以前只能凭感觉滚动页面）
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut client);
        }
        let below = anchor.bottom + self.s(4);
        let top = if below + h > client.bottom - self.s(8) {
            (anchor.top - self.s(4) - h).max(self.s(8))
        } else {
            below
        };
        let r = RECT {
            left: anchor.left,
            top,
            right: anchor.right,
            bottom: top + h,
        };
        unsafe {
            if let Some(g) = Gfx::new(dc) {
                // 下拉是浮层：投影比卡片重一点才分得出层次
                g.fill_round(
                    RECT { left: r.left - 2, top: r.top + 2, right: r.right + 2, bottom: r.bottom + 4 },
                    R_CARD,
                    argb(0x00000000, if t.dark { 0.45 } else { 0.10 }),
                );
                g.fill_round(r, R_CARD, t.card);
                g.stroke_round(r, R_CARD, t.field_border, 1.0);
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

    /// 上游词库更新：下载/部署/回滚都放后台线程做（40MB 下载不能让界面卡住）。
    /// kind: 1=下载到暂存 2=部署 3=回滚
    fn spawn_dict_task(&mut self, kind: u8) {
        use std::sync::atomic::Ordering;
        if self.task_running.load(Ordering::SeqCst) {
            self.toast("上一个任务还在跑，稍等一下");
            return;
        }
        let shared = self.core.shared_data_dir.clone();
        let user = self.core.user_data_dir.clone();
        let slot = self.task.clone();
        let flag = self.task_running.clone();
        if let Ok(mut g) = self.task.lock() {
            *g = None;
        }
        flag.store(true, Ordering::SeqCst);
        self.task_kind = kind;
        std::thread::spawn(move || {
            let r = match kind {
                4 => crate::core::update_dict_now(&shared, &user),
                _ => crate::core::rollback_dict_update(&user),
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            flag.store(false, Ordering::SeqCst);
        });
        unsafe {
            let _ = SetTimer(Some(self.hwnd), TIMER_TASK, 500, None);
        }
        self.toast(match kind {
            4 => "正在检查并更新词库（需要时会下载约 40MB）…",
            _ => "正在回滚词库…",
        });
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
        // 点到任何非文本框的控件，都先把输入焦点收掉，否则会继续往看不见的框里打字
        if !matches!(
            id,
            Id::PunctKey
                | Id::PunctOut
                | Id::PhraseQuery
                | Id::PhraseCode
                | Id::PhraseText
                | Id::PhraseWeight
        ) {
            self.focus = Focus::None;
        }
        match id {
            Id::Nav(i) => {
                self.page = i;
                self.scroll = 0;
                self.combo_open = None;
                self.focus = Focus::None;
            }
            // 所有下拉都在这里登记：以前漏了「删词键」和「字体」，
            // 点上去毫无反应（下拉项都在，就是打不开）。
            Id::SchemaCombo
            | Id::EnterCombo
            | Id::ToggleCombo
            | Id::SepCombo
            | Id::DeleteKeyCombo
            | Id::FontCombo => {
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
                    Some(Id::DeleteKeyCombo) => {
                        if let Some((k, _)) = DELETE_ITEMS.get(i) {
                            self.cfg.input.delete_key = (*k).into();
                            self.mark_dirty();
                        }
                    }
                    Some(Id::FontCombo) => {
                        if let Some((k, _)) = FONT_ITEMS.get(i) {
                            self.cfg.appearance.font_family = (*k).into();
                            self.mark_dirty();
                        }
                    }
                    Some(Id::SegLayout) => {
                        self.cfg.appearance.layout = if i == 1 { "horizontal".into() } else { "vertical".into() };
                        self.mark_dirty();
                    }
                    Some(Id::SegFollow) => {
                        self.cfg.appearance.follow_mode = if i == 1 { "fixed".into() } else { "caret".into() };
                        self.mark_dirty();
                    }
                    _ => {}
                }
            }
            Id::SegFull => {}
            Id::SegPunct => {}
            Id::SegAscii => {}
            Id::SegLayout => {}
            Id::SegFollow => {}
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
            Id::SwitchComment => {
                self.cfg.appearance.show_comment = !self.cfg.appearance.show_comment;
                self.mark_dirty();
            }
            Id::SwitchAssoc => {
                self.cfg.input.enable_assoc = !self.cfg.input.enable_assoc;
                self.mark_dirty();
            }
            Id::SwitchQuick => {
                self.cfg.input.enable_quick_input = !self.cfg.input.enable_quick_input;
                self.mark_dirty();
            }
            Id::SwitchPerApp => {
                self.cfg.input.per_app_mode = !self.cfg.input.per_app_mode;
                let on = self.cfg.input.per_app_mode;
                self.mark_dirty();
                self.toast(if on {
                    "中英状态按应用分别记忆（每个应用各记各的）"
                } else {
                    "中英状态所有应用统一"
                });
            }
            Id::SwitchEmoji => {
                self.cfg.input.enable_emoji = !self.cfg.input.enable_emoji;
                self.mark_dirty();
            }
            Id::SwitchEnglish => {
                self.cfg.input.enable_english = !self.cfg.input.enable_english;
                self.mark_dirty();
            }
            Id::SwitchPhrase => {
                self.cfg.input.enable_custom_phrase = !self.cfg.input.enable_custom_phrase;
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
            Id::FuzzyPreset(kind) => {
                let preset: &[&str] = match kind {
                    0 => &["zh_z", "ch_c", "sh_s", "n_l"],
                    1 => &["an_ang", "en_eng", "in_ing"],
                    2 => &[
                        "zh_z", "ch_c", "sh_s", "n_l", "f_h", "r_l", "an_ang", "en_eng", "in_ing",
                        "ian_iang", "uan_uang",
                    ],
                    _ => &[],
                };
                if kind == 3 {
                    self.cfg.input.fuzzy.clear();
                    self.toast("已清空模糊音");
                } else {
                    // 预设是"并集"：不会把用户已经勾的组挤掉，只会补上没勾的
                    for g in preset {
                        if !self.cfg.input.fuzzy.iter().any(|f| f == g) {
                            self.cfg.input.fuzzy.push((*g).to_string());
                        }
                    }
                    self.toast(&format!("已启用模糊音（共 {} 组）", self.cfg.input.fuzzy.len()));
                }
                self.mark_dirty();
            }
            Id::PunctKey => self.focus = Focus::PunctKey,
            Id::PunctOut => self.focus = Focus::PunctOut,
            Id::PunctRow(i) => self.punct_sel = Some(i),
            Id::PhraseQuery => self.focus = Focus::PhraseQuery,
            Id::PhraseCode => self.focus = Focus::PhraseCode,
            Id::PhraseText => self.focus = Focus::PhraseText,
            Id::PhraseWeight => self.focus = Focus::PhraseWeight,
            Id::PhraseRow(vi) => {
                // vi 是"当前显示表"里的下标，换算回 phrase_rows 的下标
                if let Some(&ri) = self.phrase_view().get(vi) {
                    self.phrase_sel = Some(ri);
                    // 方便"改一条"：点中就把内容回填到下面三个框里
                    if let Some(xunyin_rime::phrases::Row::Entry(e)) = self.phrase_rows.get(ri) {
                        self.phrase_code = e.codes.clone();
                        self.phrase_text = e.text.clone();
                        self.phrase_weight = e.weight.map(|w| w.to_string()).unwrap_or_default();
                    }
                }
            }
            Id::PhraseAdd => {
                let code = self.phrase_code.trim().to_string();
                let text_v = self.phrase_text.trim().to_string();
                let weight = self.phrase_weight.trim().parse::<i32>().ok();
                if code.is_empty() || text_v.is_empty() {
                    self.toast("请先填写「编码」与「词条」");
                } else {
                    let entry = xunyin_rime::phrases::Entry {
                        text: text_v,
                        codes: code,
                        weight,
                    };
                    // 同编码同词条就更新，否则追加
                    let existing = self.phrase_rows.iter_mut().find(|r| match r {
                        xunyin_rime::phrases::Row::Entry(e) => {
                            e.text == entry.text && e.codes == entry.codes
                        }
                        _ => false,
                    });
                    match existing {
                        Some(xunyin_rime::phrases::Row::Entry(e)) => {
                            e.weight = entry.weight;
                        }
                        _ => self
                            .phrase_rows
                            .push(xunyin_rime::phrases::Row::Entry(entry)),
                    }
                    self.phrase_code.clear();
                    self.phrase_text.clear();
                    self.phrase_weight.clear();
                    self.save_phrases();
                }
            }
            Id::PhraseDel => match self.phrase_sel {
                Some(i) if i < self.phrase_rows.len() => {
                    self.phrase_rows.remove(i);
                    self.phrase_sel = None;
                    self.save_phrases();
                }
                _ => self.toast("先在列表里选中一条短语"),
            },
            Id::PunctAdd => {
                let k = self.punct_key.trim().to_string();
                let o = self.punct_out.trim().to_string();
                if k.is_empty() || o.is_empty() {
                    self.toast("请先填写「按键」与「上屏文本」");
                } else {
                    if let Some(p) = self.cfg.input.punct_map.iter_mut().find(|p| p.key == k) {
                        p.out = o;
                    } else {
                        self.cfg.input.punct_map.push(xunyin_rime::config::PunctPair { key: k, out: o });
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
                            self.saved = self.cfg.clone();
                            self.theme = theme_from(&self.cfg, self.theme.dark);
                            self.mark_dirty();
                            self.toast("皮肤已套用");
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::DictImport => {
                // 批量词库通道（低优先级）：支持搜狗细胞词库 .scel 与文本词表
                if let Some(path) = pick_file(
                    self.hwnd,
                    &[
                        ("词库文件（txt / scel）", "*.scel;*.txt"),
                        ("搜狗细胞词库", "*.scel"),
                        ("文本词表", "*.txt"),
                        ("所有文件", "*.*"),
                    ],
                    "scel",
                ) {
                    match self.core.import_dict_file(&path) {
                        Ok(stats) => {
                            self.stats = stats;
                            self.toast(&self.stats.last_import.clone());
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::DictImportDir => {
                if let Some(dir) = pick_folder(self.hwnd) {
                    match self.core.import_dict_dir(&dir) {
                        Ok(stats) => {
                            self.stats = stats;
                            self.toast(&self.stats.last_import.clone());
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::PhraseImport => {
                if let Some(path) = pick_file(
                    self.hwnd,
                    &[("短语文件", "*.txt"), ("所有文件", "*.*")],
                    "txt",
                ) {
                    match self.core.import_phrase_file(&path) {
                        Ok(stats) => {
                            self.stats = stats;
                            self.toast(&self.stats.last_import.clone());
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::StatsReset => {
                let _ = self.core.reset_input_stats();
                self.toast("输入统计已清零");
            }
            Id::DictExport => match self.core.export_dict() {
                Ok(p) => self.toast(&format!("已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::ImportDictExport => match self.core.export_imported_dict() {
                Ok(p) => self.toast(&format!("已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::ExportUserDict => match self.core.export_user_dict() {
                Ok(p) => self.toast(&format!("用户词库已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::ClearUserDict => match self.core.clear_user_dict() {
                Ok(msg) => {
                    self.stats = self.core.dict_stats();
                    self.toast(&msg);
                }
                Err(e) => self.toast(&format!("清空失败：{e}")),
            },
            Id::DiagExport => match self.core.export_diag() {
                Ok(p) => self.toast(&format!("诊断包已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::ConfigExport => match self.core.export_config() {
                Ok(p) => self.toast(&format!("配置已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::ConfigImport => {
                if let Some(path) = pick_file(self.hwnd, &[("配置文件", "*.json"), ("所有文件", "*.*")], "json") {
                    match self.core.import_config(&path) {
                        Ok(cfg) => {
                            self.cfg = cfg;
                            self.saved = self.cfg.clone();
                            self.theme = theme_from(&self.cfg, self.theme.dark);
                            self.stats = self.core.dict_stats();
                            self.toast("配置已导入（输入法会自动重载）");
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::UpdateCheck => match self.core.check_update() {
                Ok(msg) => self.toast(&msg),
                Err(e) => self.toast(&format!("检查更新失败：{e}")),
            },
            Id::SkinRestore => match self.core.restore_builtin_skin() {
                Ok(msg) => {
                    // 皮肤中心的缩略图/来源标记要跟着回到「内置」
                    self.skins = self.core.skin_assets();
                    self.toast(&msg);
                }
                Err(e) => self.toast(&format!("恢复失败：{e}")),
            },
            Id::SkinPackExport => match self.core.export_skin_pack() {
                Ok(p) => self.toast(&format!("皮肤包已导出：{}", p.display())),
                Err(e) => self.toast(&format!("导出失败：{e}")),
            },
            Id::SkinPackImport => {
                if let Some(path) = pick_file(self.hwnd, &[("皮肤包", "*.zip"), ("所有文件", "*.*")], "zip") {
                    match self.core.import_skin_pack(&path) {
                        Ok(msg) => {
                            self.skins = self.core.skin_assets();
                            self.toast(&msg);
                        }
                        Err(e) => self.toast(&format!("导入失败：{e}")),
                    }
                }
            }
            Id::SkinItem(i) => {
                // 点某一项 = 把这份资源导到皮肤目录并定位到文件，方便直接改
                let name = self.skins.get(i).map(|a| a.name.clone());
                if let Some(name) = name {
                    match crate::core::export_skin_asset(&name) {
                        Ok(p) => {
                            self.skins = self.core.skin_assets();
                            reveal_in_explorer(&p);
                            self.toast(&format!(
                                "已导出 {name} 到皮肤目录并用资源管理器打开；改完点「重载配置」即时生效"
                            ));
                        }
                        Err(e) => self.toast(&format!("导出失败：{e}")),
                    }
                }
            }
            Id::CellDict(i) => {
                if let Some(c) = self.cells.get_mut(i) {
                    c.enabled = !c.enabled;
                    self.cell_dirty = true;
                }
            }
            Id::CellApply => {
                let enabled: Vec<String> = self
                    .cells
                    .iter()
                    .filter(|c| c.enabled)
                    .map(|c| c.id.clone())
                    .collect();
                match self.core.apply_cell_dicts(&enabled) {
                    Ok(msg) => {
                        self.cell_dirty = false;
                        self.toast(&msg);
                    }
                    Err(e) => self.toast(&format!("应用失败：{e}")),
                }
            }
            Id::UpdateInstall => match self.core.download_and_install() {
                Ok(msg) => self.toast(&msg),
                Err(e) => self.toast(&format!("更新失败：{e}")),
            },
            Id::DictRollback => self.spawn_dict_task(3),
            Id::DictUpdateOne => self.spawn_dict_task(4),
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
            Id::OpenSkins => {
                if let Err(e) = self.core.open_skin_dir() {
                    self.toast(&format!("打开失败：{e}"));
                }
            }
            Id::ClearLogs => {
                let freed = self.core.clear_logs();
                self.toast(&format!("已清空日志（{} KB）", freed / 1024));
            }
            Id::ReloadConfig => {
                // 通知所有正在运行的应用重载配置（不用重启，像小狼毫的重新部署）
                let ok = xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
                // 皮肤文件可能在外部改过，顺手刷新缩略图/来源标记
                self.skins = self.core.skin_assets();
                self.toast(if ok {
                    "已通知输入法重载配置（正在输入的应用会立刻生效）"
                } else {
                    "通知失败：无法写入重载标记"
                });
            }
            Id::Redeploy => {
                self.core.redeploy();
                self.toast("已开始重新部署（编译完成后自动生效，无需重启应用）");
            }
            Id::ResetAll => {
                self.cfg = self.core.reset_config();
                self.saved = self.cfg.clone();
                self.theme = theme_from(&self.cfg, self.theme.dark);
                self.toast("已恢复默认设置（正在重建索引）");
            }
            Id::Update => unsafe {
                let url = wide("https://github.com/itsypa/xunyin-ime/releases/latest");
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
/// 皮肤资源缩略图：把 BGRA 资源按方框平均缩放并**预先与底色混合**（GDI 的
/// StretchDIBits 不做 per-pixel alpha），图标类按主题色 tint。
unsafe fn draw_skin_thumb(dc: HDC, r: RECT, a: &SkinAsset, tint: u32, bg: u32) {
    let (sw, sh) = (a.w, a.h);
    let (dw, dh) = ((r.right - r.left).max(1) as usize, (r.bottom - r.top).max(1) as usize);
    if sw == 0 || sh == 0 || a.bytes.len() < 8 + sw * sh * 4 {
        return;
    }
    // 底色（含主题 tint 的合成底）：图标是白色蒙版，按 tint 上色后再混到底色上
    let (bg_b, bg_g, bg_r) = ((bg & 0xFF) as f32, ((bg >> 8) & 0xFF) as f32, ((bg >> 16) & 0xFF) as f32);
    let is_bar = a.name == "bar_bg.bin";
    let (t_b, t_g, t_r) = ((tint & 0xFF) as f32, ((tint >> 8) & 0xFF) as f32, ((tint >> 16) & 0xFF) as f32);
    let src = &a.bytes[8..];
    let mut buf = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            // 源区间（方框平均）
            let x0 = x * sw / dw;
            let x1 = (((x + 1) * sw).div_ceil(dw)).min(sw).max(x0 + 1);
            let y0 = y * sh / dh;
            let y1 = (((y + 1) * sh).div_ceil(dh)).min(sh).max(y0 + 1);
            let mut sa = 0f32;
            let mut sb = 0f32;
            let mut sg = 0f32;
            let mut sr = 0f32;
            let mut n = 0f32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let o = (sy * sw + sx) * 4;
                    let alpha = src[o + 3] as f32 / 255.0;
                    if is_bar {
                        sb += src[o] as f32 * alpha;
                        sg += src[o + 1] as f32 * alpha;
                        sr += src[o + 2] as f32 * alpha;
                    } else {
                        // 图标只用 alpha（预渲染为白色），颜色取 tint
                        sb += t_b * alpha;
                        sg += t_g * alpha;
                        sr += t_r * alpha;
                    }
                    sa += alpha;
                    n += 1.0;
                }
            }
            if n == 0.0 {
                continue;
            }
            let a_avg = (sa / n).clamp(0.0, 1.0);
            let (cr, cg, cb) = (sr / n, sg / n, sb / n);
            let o = (y * dw + x) * 4;
            let mix = |c: f32, bgc: f32| (c * a_avg + bgc * (1.0 - a_avg)).clamp(0.0, 255.0) as u8;
            buf[o] = mix(cb, bg_b);
            buf[o + 1] = mix(cg, bg_g);
            buf[o + 2] = mix(cr, bg_r);
            buf[o + 3] = 255;
        }
    }
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = dw as i32;
    bi.bmiHeader.biHeight = -(dh as i32); // 自顶向下
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;
    StretchDIBits(
        dc,
        r.left,
        r.top,
        dw as i32,
        dh as i32,
        0,
        0,
        dw as i32,
        dh as i32,
        Some(buf.as_ptr() as *const std::ffi::c_void),
        &bi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
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
        Id::DeleteKeyCombo => DELETE_ITEMS.iter().map(|(k, l)| ((*k).to_string(), (*l).to_string())).collect(),
        Id::FontCombo => FONT_ITEMS.iter().map(|(k, l)| ((*k).to_string(), (*l).to_string())).collect(),
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

/// 选一个文件夹（批量导入词库用）。
///
/// 用系统自带的 `SHBrowseForFolderW`：一条依赖都不用加，也不需要 COM。
fn pick_folder(hwnd: HWND) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Shell::{SHBrowseForFolderW, SHGetPathFromIDListW, BROWSEINFOW};
    unsafe {
        // 结构体整体清零 = 无回调、无图标、无根目录，正是我们要的默认
        let mut bi: BROWSEINFOW = std::mem::zeroed();
        let title = wide("选择词库所在文件夹");
        bi.hwndOwner = hwnd;
        bi.lpszTitle = PCWSTR(title.as_ptr());
        let idl = SHBrowseForFolderW(&mut bi);
        if idl.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let ok = SHGetPathFromIDListW(idl, &mut buf).as_bool();
        // IDList 由 shell 分配，用完必须释放（否则每次选文件夹都漏一块）
        windows::Win32::System::Com::CoTaskMemFree(Some(idl as *const core::ffi::c_void));
        if !ok {
            return None;
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
        Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

// ---------------- 窗口过程与入口 ----------------

const MSG_DEFERRED: u32 = WM_APP + 1;
const TIMER_SAVE: usize = 1;
const TIMER_TOAST: usize = 2;
const TIMER_ENGINE: usize = 3;
/// 后台任务轮询（词库下载/部署/回滚）
const TIMER_TASK: usize = 4;

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    }
}

/// 拖动滚动条：按鼠标 y 把滑块中心移到指针处，再换算成滚动位置。
fn drag_scroll(app: &mut App, y: i32) {
    let Some((top, track_h, thumb_h)) = app.scroll_geom else {
        return;
    };
    let span = (track_h - thumb_h).max(1);
    let frac = ((y - top - thumb_h / 2) as f32 / span as f32).clamp(0.0, 1.0);
    app.scroll = (frac * app.scroll_max.max(0) as f32).round() as i32;
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
        Id::CornerRadius => {
            let v = (frac * 24.0).round() as i32;
            app.cfg.appearance.corner_radius = v.clamp(0, 24) as u32;
            app.mark_dirty();
        }
        Id::PadX => {
            let v = (frac * 30.0).round() as i32;
            app.cfg.appearance.cand_pad_x = v.clamp(0, 30) as u32;
            app.mark_dirty();
        }
        Id::PadY => {
            let v = (frac * 16.0).round() as i32;
            app.cfg.appearance.cand_pad_y = v.clamp(0, 16) as u32;
            app.mark_dirty();
        }
        Id::MinWidth => {
            let v = 60 + (frac * 240.0 / 10.0).round() as i32 * 10;
            app.cfg.appearance.cand_min_width = v.clamp(60, 300) as u32;
            app.mark_dirty();
        }
        Id::CandGap => {
            let v = (frac * 48.0).round() as i32;
            app.cfg.appearance.cand_gap = v.clamp(0, 48) as u32;
            app.mark_dirty();
        }
        Id::BarMargin => {
            let v = (frac * 40.0).round() as i32;
            app.cfg.appearance.bar_margin = v.clamp(0, 40) as u32;
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
                set_app_ptr(hwnd, ptr);
                let _ = PostMessageW(Some(hwnd), MSG_DEFERRED, WPARAM(0), LPARAM(0));
                let _ = SetTimer(Some(hwnd), TIMER_ENGINE, 1200, None);
                LRESULT(0)
            }
            MSG_DEFERRED => {
                if let Some(app) = main_of(hwnd) {
                    app.core.init_engine();
                    app.schemas = app.core.schemas();
                    app.stats = app.core.dict_stats();
                    app.input_stats = app.core.input_stats();
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
                    app.mouse = pt;
                    if app.dragging_scroll {
                        drag_scroll(app, pt.y);
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        return LRESULT(0);
                    }
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
                        Some(Id::ScrollBar) => {
                            app.dragging_scroll = true;
                            drag_scroll(app, pt.y);
                        }
                        Some(
                            id @ (Id::PageSize
                            | Id::FontSize
                            | Id::Opacity
                            | Id::CornerRadius
                            | Id::PadX
                            | Id::PadY
                            | Id::MinWidth
                            | Id::CandGap
                            | Id::BarMargin),
                        ) => {
                            app.dragging_slider = Some(id);
                            drag_slider(app, id, pt.x);
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
                    if app.dragging_scroll {
                        app.dragging_scroll = false;
                    } else if app.dragging_slider.is_some() {
                        app.dragging_slider = None;
                    } else if let Some(id) = pressed {
                        if Some(id) == hit {
                            app.activate(id);
                        }
                    } else {
                        // 点空白处：收起下拉、取消文本焦点
                        app.combo_open = None;
                        app.focus = Focus::None;
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Some(app) = main_of(hwnd) {
                    let delta = (((wparam.0 >> 16) & 0xFFFF) as i16 as i32) / 120;
                    app.scroll = (app.scroll - delta * app.s(64)).clamp(0, app.scroll_max.max(0));
                    // 下拉是跟着控件矩形画的，滚动后位置就错了，直接收起
                    app.combo_open = None;
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
                        (0x08, Focus::PhraseQuery) => {
                            app.phrase_query.pop();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        (0x08, Focus::PhraseCode) => {
                            app.phrase_code.pop();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        (0x08, Focus::PhraseText) => {
                            app.phrase_text.pop();
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        (0x08, Focus::PhraseWeight) => {
                            app.phrase_weight.pop();
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
                                Focus::PhraseQuery => app.phrase_query.push(ch),
                                Focus::PhraseCode => app.phrase_code.push(ch),
                                Focus::PhraseText => app.phrase_text.push(ch),
                                Focus::PhraseWeight => app.phrase_weight.push(ch),
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
                } else if wparam.0 == TIMER_TASK {
                    // 后台任务（词库下载/部署/回滚）完成检查
                    // 顺带刷新输入统计：只在"关于与维护"页开着时读一次小文件，
                    // 这样窗口开着打字，数字也会跟着涨
                    if let Some(app) = main_of(hwnd) {
                        if app.page == 8 {
                            let fresh = app.core.input_stats();
                            if fresh != app.input_stats {
                                app.input_stats = fresh;
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        }
                    }
                    let mut done: Option<Result<String, String>> = None;
                    let mut kind = 0u8;
                    if let Some(app) = main_of(hwnd) {
                        kind = app.task_kind;
                        if let Ok(mut g) = app.task.lock() {
                            if g.is_some() {
                                done = g.take();
                            }
                        }
                    }
                    if let Some(r) = done {
                        let _ = KillTimer(Some(hwnd), TIMER_TASK);
                        if let Some(app) = main_of(hwnd) {
                            app.task_kind = 0;
                            match r {
                                Ok(msg) => {
                                    app.toast(&msg);
                                    // 部署/回滚之后要重建索引并刷新统计
                                    if kind == 2 || kind == 3 {
                                        app.core.redeploy();
                                        app.stats = app.core.dict_stats();
                                    }
                                }
                                Err(e) => app.toast(&format!("失败：{e}")),
                            }
                            app.dict_state = app.core.dict_update_state();
                            let _ = InvalidateRect(Some(hwnd), None, false);
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
        let cls = wide("Xunyin.Settings.Instance");
        let _ = CreateMutexW(None, false, PCWSTR(cls.as_ptr()));
        let main_cls = wide("Xunyin.Settings.SelfDrawn");
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
        let title = wide("迅音输入法设置");
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
        // 深色模式下让标题栏跟着深色（内容区是自绘的，DWM 不知道）
        if system_prefers_dark() {
            set_dark_title_bar(hwnd, true);
        }
        // 初始窗口按 DPI 放大，并留出任务栏/屏幕边缘：以前写死 1020×700 物理像素，
        // 150% 缩放下只有 680×467 逻辑像素，比最小尺寸还小，一打开就挤成一团。
        {
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let sw = GetSystemMetrics(SM_CXSCREEN).max(640);
            let sh = GetSystemMetrics(SM_CYSCREEN).max(480);
            let ww = (1160 * dpi / 96).min(sw - 24).max(760 * dpi / 96);
            let wh = (780 * dpi / 96).min(sh - 80).max(560 * dpi / 96);
            let wx = ((sw - ww) / 2).max(0);
            let wy = ((sh - wh) / 2).max(0);
            let _ = SetWindowPos(hwnd, None, wx, wy, ww, wh, SWP_NOZORDER);
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
        // 卡片矩形每帧重算（以前只在启动时清空，悬停重绘会一直往 Vec 里追加 ——
        // 既是内存泄漏，也让 XUNYIN_UI_DUMP 的布局检查混进上一帧的旧卡片）
        app.cards.clear();
        let t = app.theme.clone();

        if let Some(g) = Gfx::new(mem) {
            g.fill_round(RECT { left: 0, top: 0, right: w, bottom: h }, 0, t.bg);
        }
        // 侧栏
        let side_w = app.s(SIDE_W);
        if let Some(g) = Gfx::new(mem) {
            g.fill_round(RECT { left: 0, top: 0, right: side_w, bottom: h }, 0, t.sidebar);
            g.fill_round(
                RECT { left: side_w - app.s(1), top: 0, right: side_w, bottom: h },
                0,
                t.border,
            );
        }
        {
            let lr = RECT { left: app.s(20), top: app.s(16), right: side_w - app.s(12), bottom: app.s(44) };
            text(mem, lr, "迅音输入法", app.font_title, t.text, DT_LEFT, true);
            let sr = RECT { left: app.s(21), top: app.s(43), right: side_w - app.s(12), bottom: app.s(61) };
            text(mem, sr, "设置中心", app.font_small, t.text3, DT_LEFT, true);
        }
        // 分组导航：组名是小号浅色字，条目 34px 高；9 页 + 4 个组名在最小窗口里也放得下
        let mut ny = app.s(78);
        let mut last_group = "";
        let nav_h = app.s(34);
        for (i, page) in PAGES.iter().enumerate() {
            if page.group != last_group {
                if !last_group.is_empty() {
                    ny += app.s(6);
                }
                let gr = RECT {
                    left: app.s(22),
                    top: ny,
                    right: side_w - app.s(12),
                    bottom: ny + app.s(20),
                };
                text(mem, gr, page.group, app.font_small, t.text3, DT_LEFT, true);
                ny += app.s(22);
                last_group = page.group;
            }
            let r = RECT { left: app.s(12), top: ny, right: side_w - app.s(12), bottom: ny + nav_h };
            let id = Id::Nav(i);
            let sel = app.page == i;
            let hot = app.hover == Some(id);
            if let Some(g) = Gfx::new(mem) {
                if sel {
                    g.fill_round(r, R_CTRL, t.accent_soft);
                } else if hot {
                    g.fill_round(r, R_CTRL, t.hover);
                }
            }
            // 选中态：左侧一条圆角指示条（Fluent 的做法），比整块高亮底更干净
            if sel {
                if let Some(g) = Gfx::new(mem) {
                    let cy = (r.top + r.bottom) / 2;
                    g.fill_round(
                        RECT {
                            left: app.s(6),
                            top: cy - app.s(9),
                            right: app.s(10),
                            bottom: cy + app.s(9),
                        },
                        app.s(2),
                        t.accent,
                    );
                }
            }
            let (glyph, ifont) = if app.icon_font_ok {
                (page.icon, app.font_icon)
            } else {
                (PAGES_EMOJI[i], app.font)
            };
            let ir = RECT { left: r.left + app.s(12), top: r.top, right: r.left + app.s(34), bottom: r.bottom };
            text(mem, ir, glyph, ifont, if sel { t.accent } else { t.text2 }, DT_LEFT, true);
            let nr = RECT { left: r.left + app.s(38), top: r.top, right: r.right - app.s(6), bottom: r.bottom };
            text(
                mem,
                nr,
                page.title,
                if sel { app.font_bold } else { app.font },
                t.text,
                DT_LEFT,
                true,
            );
            app.hits.push(Hit { id, rect: r });
            ny += nav_h;
        }
        {
            // 页脚（版本 + 自动保存提示）上方加一条细分隔线，把导航和说明分开
            if let Some(g) = Gfx::new(mem) {
                g.fill_round(
                    RECT {
                        left: app.s(20),
                        top: h - app.s(62),
                        right: side_w - app.s(20),
                        bottom: h - app.s(61),
                    },
                    0,
                    t.divider,
                );
            }
            let br = RECT { left: app.s(20), top: h - app.s(54), right: side_w - app.s(12), bottom: h - app.s(32) };
            text(mem, br, &format!("版本 {}", env!("CARGO_PKG_VERSION")), app.font_small, t.text3, DT_LEFT, true);
            let hr = RECT { left: app.s(20), top: h - app.s(32), right: side_w - app.s(12), bottom: h - app.s(12) };
            text(mem, hr, "所有更改自动保存", app.font_small, t.text3, DT_LEFT, true);
        }

        // 内容区（滚动 + 裁剪）。右侧留 12px 给滚动条，卡片右边缘不会和它打架。
        let cx = side_w + app.s(20);
        let cw = (w - cx - app.s(24)).max(app.s(520));
        let cy0 = app.s(20);
        let clip = CreateRectRgn(cx, cy0, w, h);
        let _ = SelectClipRgn(mem, Some(clip));
        let used = match app.page {
            0 => draw_input(app, mem, cx, cy0 - app.scroll, cw),
            1 => draw_keys(app, mem, cx, cy0 - app.scroll, cw),
            2 => draw_fuzzy(app, mem, cx, cy0 - app.scroll, cw),
            3 => draw_appearance(app, mem, cx, cy0 - app.scroll, cw),
            4 => draw_candidate(app, mem, cx, cy0 - app.scroll, cw),
            5 => draw_status(app, mem, cx, cy0 - app.scroll, cw),
            6 => draw_dict(app, mem, cx, cy0 - app.scroll, cw),
            7 => draw_phrases(app, mem, cx, cy0 - app.scroll, cw),
            _ => draw_about(app, mem, cx, cy0 - app.scroll, cw),
        };
        // 注意：绘制时已经减去了 scroll，所以这里要先加回来才是真正的内容高度。
        // 之前直接拿 used 算，scroll_max 会随当前滚动位置变化 → 滚到底再上滚时
        // 每帧的边界都不一样，表现为抖动。
        let content_h = used - (cy0 - app.scroll);
        app.scroll_max = (content_h + cy0 - (h - app.s(12))).max(0);
        // 窗口变大 / 页面内容变短后，旧的滚动位置可能已经越界，钳一下
        if app.scroll > app.scroll_max {
            app.scroll = app.scroll_max;
        }
        // 调试导出（仅当设置 XUNYIN_UI_DUMP 时）：供自动化检查布局越界与滚动稳定性
        if let Ok(dir) = std::env::var("XUNYIN_UI_DUMP") {
            use std::io::Write;
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::path::Path::new(&dir).join("paint.log"))
            {
                let _ = writeln!(
                    f,
                    "page={} scroll={} scroll_max={} content_h={} client={}x{}",
                    app.page, app.scroll, app.scroll_max, content_h, w, h
                );
            }
            let mut lines = String::new();
            for hh in &app.hits {
                let r = hh.rect;
                lines.push_str(&format!(
                    "{:?} {} {} {} {}\n",
                    hh.id, r.left, r.top, r.right, r.bottom
                ));
            }
            let _ = std::fs::write(std::path::Path::new(&dir).join("hits.txt"), lines);
            // 卡片矩形：用于自动检查两栏是否参差、卡片是否越界/重叠
            let mut cards = String::new();
            for r in &app.cards {
                cards.push_str(&format!(
                    "{} {} {} {}\n",
                    r.left, r.top, r.right, r.bottom
                ));
            }
            let _ = std::fs::write(std::path::Path::new(&dir).join("cards.txt"), cards);
        }
        let _ = SelectClipRgn(mem, None);
        let _ = DeleteObject(HGDIOBJ(clip.0));

        // 滚动条：内容超出窗口时显示。没有它时用户只能靠滚轮猜"下面还有没有"。
        // 页面切换/内容变短后 scroll_max 归零，这里自然把滚动条收掉。
        app.scroll_geom = None;
        if app.scroll_max > 0 {
            let track_top = cy0 + app.s(2);
            let track_bot = h - app.s(14);
            let track_h = (track_bot - track_top).max(1);
            let visible = (h - cy0 - app.s(10)).max(1) as f32;
            let ratio = (visible / content_h.max(1) as f32).clamp(0.0, 1.0);
            let thumb_h = ((track_h as f32 * ratio).round() as i32).clamp(app.s(36), track_h);
            let frac = (app.scroll as f32 / app.scroll_max.max(1) as f32).clamp(0.0, 1.0);
            let thumb_top = track_top + ((track_h - thumb_h) as f32 * frac).round() as i32;
            let tx = w - app.s(12);
            let hot = app.dragging_scroll || app.hover == Some(Id::ScrollBar);
            if let Some(g) = Gfx::new(mem) {
                g.fill_round(
                    RECT { left: tx, top: track_top, right: tx + app.s(6), bottom: track_bot },
                    app.s(3),
                    argb(t.text, if t.dark { 0.10 } else { 0.05 }),
                );
                g.fill_round(
                    RECT {
                        left: tx,
                        top: thumb_top,
                        right: tx + app.s(6),
                        bottom: thumb_top + thumb_h,
                    },
                    app.s(3),
                    argb(t.text2, if hot { 0.55 } else { 0.30 }),
                );
            }
            // 命中区比滑块宽：6px 的细条不好抓
            app.hits.push(Hit {
                id: Id::ScrollBar,
                rect: RECT { left: w - app.s(20), top: track_top, right: w - app.s(2), bottom: track_bot },
            });
            app.scroll_geom = Some((track_top, track_h, thumb_h));
        }

        // 下拉浮层
        app.draw_combo_popup(mem);

        // 调试：浮层画完后再导出一份完整命中区（含下拉项/滚动条），
        // 自动化点击测试用它找 ComboItem；布局检查仍用上面那份（不含浮层）。
        if let Ok(dir) = std::env::var("XUNYIN_UI_DUMP") {
            let mut lines = String::new();
            for hh in &app.hits {
                let r = hh.rect;
                lines.push_str(&format!(
                    "{:?} {} {} {} {}\n",
                    hh.id, r.left, r.top, r.right, r.bottom
                ));
            }
            let _ = std::fs::write(std::path::Path::new(&dir).join("hits-all.txt"), lines);
        }

        // 悬停提示（皮肤中心的每一项、按钮等给一句话说明）
        // 悬停提示：(标题, 说明, 操作提示) 三行
        let tip: Option<(String, String, String)> = match app.hover {
            Some(Id::SkinItem(i)) => app.skins.get(i).map(|a| {
                (
                    format!("{} · {}", a.label, a.name),
                    a.desc.to_string(),
                    "点一下导出并定位文件；改完重载配置生效".to_string(),
                )
            }),
            Some(Id::DictRollback) => Some((
                "回滚上次部署".into(),
                "用备份覆盖回去并重建索引".into(),
                String::new(),
            )),
            Some(Id::SkinRestore) => Some((
                "恢复内置皮肤".into(),
                "删掉皮肤目录里的覆盖文件，回到内置图标".into(),
                String::new(),
            )),
            _ => None,
        };
        if let Some((msg, desc, hint)) = tip {
            {
                let lines: Vec<&str> = if hint.is_empty() {
                    vec![msg.as_str(), desc.as_str()]
                } else {
                    vec![msg.as_str(), desc.as_str(), hint.as_str()]
                };
                let tw = lines
                    .iter()
                    .map(|l| App::text_width_of(mem, l, app.font_small))
                    .max()
                    .unwrap_or(app.s(200))
                    + app.s(24);
                let th = app.s(12 + 24 * lines.len() as i32);
                let mut tx = app.mouse.x + app.s(14);
                let mut ty = app.mouse.y + app.s(18);
                if tx + tw > w - app.s(8) {
                    tx = (w - app.s(8) - tw).max(app.s(8));
                }
                if ty + th > h - app.s(8) {
                    ty = (app.mouse.y - th - app.s(8)).max(app.s(8));
                }
                let tr = RECT { left: tx, top: ty, right: tx + tw, bottom: ty + th };
                if let Some(g) = Gfx::new(mem) {
                    // 说明浮标：跟主题走的中性卡片，不再写死蓝底
                    g.fill_round(tr, R_CARD, t.card);
                    g.stroke_round(tr, R_CARD, t.field_border, 1.0);
                }
                for (i, l) in lines.iter().enumerate() {
                    let top = tr.top + app.s(6) + (i as i32) * app.s(24);
                    let color = if i == 0 { t.text } else { t.text3 };
                    text(
                        mem,
                        RECT { left: tr.left + app.s(12), top, right: tr.right - app.s(12), bottom: top + app.s(22) },
                        l,
                        app.font_small,
                        color,
                        DT_LEFT,
                        true,
                    );
                }
            }
        }

        // 提示条
        if !app.toast.is_empty() && std::time::Instant::now() < app.toast_until {
            let msg = app.toast.clone();
            let tw = App::text_width_of(mem, &msg, app.font) + app.s(30);
            let r = RECT { left: cx, top: h - app.s(48), right: cx + tw, bottom: h - app.s(16) };
            if let Some(g) = Gfx::new(mem) {
                // 提示条：深色主题用略亮的灰、浅色主题用深灰 + 白字（白底 + 深字的"系统提示"
                // 在这套界面里太轻，容易看不见）
                g.fill_round(r, R_CTRL, if t.dark { 0xFF323232 } else { 0xFF2B2B2B });
                g.stroke_round(r, R_CTRL, if t.dark { 0xFF4A4A4A } else { 0x00000000 }, 1.0);
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

fn draw_input(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 左栏：输入方案 + 输入模式；右栏：候选来源 + 学习与快捷。
    // 两栏各 2 张卡片、总高相同（144+236 / 190+190），底部自然对齐。
    let (x, cy0, col_w) = cols.put(0, app.card_h(2));
    let cy = app.card(dc, x, cy0, col_w, app.card_h(2), "输入方案与候选", "");
    let schema_name = app
        .schemas
        .iter()
        .find(|(id, _)| *id == app.cfg.input.schema)
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| app.cfg.input.schema.clone());
    let inner = col_w - app.s(36);
    app.row_combo(dc, x + app.s(18), cy, inner, "默认输入方案", "决定键位与词库", Id::SchemaCombo, &schema_name);
    app.row_slider(
        dc,
        x + app.s(18),
        cy + app.s(ROW_H),
        inner,
        "每页候选数",
        "竖排时一屏显示几个",
        Id::PageSize,
        app.cfg.input.page_size as i32,
        3,
        10,
        " 个",
    );

    let mode = app.card_h(4);
    let (x, cy0, col_w) = cols.put(0, mode);
    let cy = app.card(dc, x, cy0, col_w, mode, "输入模式", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_seg(
        dc, lx, cy, inner,
        "全角 / 半角", "标点与空格全宽",
        Id::SegFull, &["半角", "全角"], if app.cfg.input.full_shape { 1 } else { 0 },
    );
    app.row_seg(
        dc, lx, cy + app.s(ROW_H), inner,
        "中英标点", "Ctrl + . 可临时切换",
        Id::SegPunct, &["中文", "英文"], if app.cfg.input.ascii_punct { 1 } else { 0 },
    );
    app.row_seg(
        dc, lx, cy + app.s(ROW_H) * 2, inner,
        "默认中英模式", "新应用启动时的状态",
        Id::SegAscii, &["中文", "英文"], if app.cfg.input.ascii_mode { 1 } else { 0 },
    );
    app.row_toggle(
        dc, lx, cy + app.s(ROW_H) * 3, inner,
        "中英状态按应用记忆", "关闭后所有应用统一",
        Id::SwitchPerApp, app.cfg.input.per_app_mode,
    );

    let src = app.card_h(3);
    let (x, cy0, col_w) = cols.put(1, src);
    let cy = app.card(dc, x, cy0, col_w, src, "候选来源", "关闭后不再出现该类候选");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    for (i, (label, desc, id, on)) in [
        ("Emoji 候选", "附带表情候选", Id::SwitchEmoji, app.cfg.input.enable_emoji),
        ("英文单词", "中英混输，如 hello", Id::SwitchEnglish, app.cfg.input.enable_english),
        ("自定义短语", "短语表里的词优先", Id::SwitchPhrase, app.cfg.input.enable_custom_phrase),
    ]
    .into_iter()
    .enumerate()
    {
        app.row_toggle(dc, lx, cy + app.s(ROW_H) * i as i32, inner, label, desc, id, on);
    }

    let assist = app.card_h(3);
    let (x, cy0, col_w) = cols.put(1, assist);
    let cy = app.card(dc, x, cy0, col_w, assist, "学习与快捷", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    for (i, (label, desc, id, on)) in [
        ("用户词典自学习", "记住新词与词频", Id::SwitchLearn, app.cfg.input.enable_user_dict),
        ("词语联想", "上屏后提示下一个词", Id::SwitchAssoc, app.cfg.input.enable_assoc),
        ("快捷输入", "rq / sj / v 算式", Id::SwitchQuick, app.cfg.input.enable_quick_input),
    ]
    .into_iter()
    .enumerate()
    {
        app.row_toggle(dc, lx, cy + app.s(ROW_H) * i as i32, inner, label, desc, id, on);
    }

    cols.bottom() + app.s(10)
}

fn draw_keys(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    let keys_h = app.card_h(4);
    let (x, cy0, col_w) = cols.put(0, keys_h);
    let cy = app.card(dc, x, cy0, col_w, keys_h, "按键行为", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let rows = [
        ("回车键", "组合中按回车的处理", Id::EnterCombo, enter_text(app)),
        ("中英切换键", "裸按 Shift 或 Ctrl+空格", Id::ToggleCombo, toggle_text(app)),
        ("拼音分隔符", "音节之间显示什么", Id::SepCombo, sep_text(app)),
        ("删词键", "删掉当前页第 N 个候选", Id::DeleteKeyCombo, find_label(&DELETE_ITEMS, &app.cfg.input.delete_key)),
    ];
    for (i, (label, desc, id, shown)) in rows.into_iter().enumerate() {
        app.row_combo(dc, lx, cy + app.s(ROW_H) * i as i32, inner, label, desc, id, &shown);
    }

    // 自定义标点：列表 + 两个输入框 + 添加/删除。两条输入行比硬塞一行好读。
    let punct_inner = app.s(160 + 12 + 32 + 10 + 34);
    let punct_h = app.card_h_inner(punct_inner);
    let (x, cy0, col_w) = cols.put(1, punct_h);
    let cy = app.card(dc, x, cy0, col_w, punct_h, "自定义标点", "留空则沿用内置标点表");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let list_rect = RECT { left: lx, top: cy, right: lx + inner, bottom: cy + app.s(160) };
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
    let key_w = app.s(96);
    let kr = RECT { left: lx, top: fy, right: lx + key_w, bottom: fy + app.s(32) };
    let ov = RECT { left: kr.right + app.s(8), top: fy, right: lx + inner, bottom: fy + app.s(32) };
    app.text_field(dc, Id::PunctKey, kr, &app.punct_key.clone(), "按键，如 ，");
    app.text_field(dc, Id::PunctOut, ov, &app.punct_out.clone(), "上屏文本，如 ……");
    let fy = fy + app.s(42);
    let btns = app.btn_row(lx, fy, inner, 2, 34);
    app.button(dc, Id::PunctAdd, btns[0], "添加", true);
    app.button_danger(dc, Id::PunctDel, btns[1], "删除所选");

    cols.bottom() + app.s(10)
}

fn draw_fuzzy(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let y0 = page_header(app, dc, x0, y0, w);

    // 模糊音组：整行 4 列勾选，下面一排快速预设。
    let card_h = app.card_h_inner(app.s(3 * 30 + 12 + 34));
    let cy = app.card(dc, x0, y0, w, card_h, "模糊音组", "勾选后自动重建索引，几秒后生效");
    let inner = w - app.s(36);
    let lx = x0 + app.s(18);
    let grid_gap = app.s(8);
    let cell_w = (inner - grid_gap * 3) / 4;
    for i in 0..FUZZY_LABELS.len() {
        let (gid, label) = FUZZY_LABELS[i];
        let col = (i % 4) as i32;
        let row = (i / 4) as i32;
        let r = RECT {
            left: lx + col * (cell_w + grid_gap),
            top: cy + row * app.s(30),
            right: lx + col * (cell_w + grid_gap) + cell_w - app.s(6),
            bottom: cy + row * app.s(30) + app.s(26),
        };
        let on = app.cfg.input.fuzzy.iter().any(|f| f == gid);
        app.checkbox(dc, Id::Fuzzy(i), r, label, on);
    }
    let py = cy + app.s(3 * 30) + app.s(12);
    let presets = app.btn_row(lx, py, inner, 4, 34);
    for (i, label) in ["南方口音常用", "前后鼻音", "全部开启", "清空"].iter().enumerate() {
        if i == 3 {
            app.button_danger(dc, Id::FuzzyPreset(i), presets[i], label);
        } else {
            app.button_soft(dc, Id::FuzzyPreset(i), presets[i], label);
        }
    }

    // 说明卡：三列小字，比一大段段落好扫。
    let iy = y0 + card_h + app.s(COL_GAP);
    let ih = app.card_h_inner(app.s(64));
    let cy = app.card(dc, x0, iy, w, ih, "模糊音怎么工作", "");
    let inner = w - app.s(36);
    let lx = x0 + app.s(18);
    let col_w = (inner - app.s(24) * 2) / 3;
    let notes = [
        ("双向派生", "开启「zh ↔ z」后，zhu 与 zu 都能出「主」"),
        ("保留标准音", "模糊音是在标准读音之外补充，不替换正确拼音"),
        ("按需启用", "只勾你分不清的组；勾太多会让候选变杂"),
    ];
    for (i, (title, desc)) in notes.iter().enumerate() {
        let bx = lx + i as i32 * (col_w + app.s(24));
        unsafe {
            text(
                dc,
                RECT { left: bx, top: cy, right: bx + col_w, bottom: cy + app.s(22) },
                title,
                app.font_bold,
                t.text,
                DT_LEFT,
                true,
            );
            text(
                dc,
                RECT { left: bx, top: cy + app.s(24), right: bx + col_w, bottom: cy + app.s(64) },
                desc,
                app.font_small,
                t.text3,
                DT_LEFT,
                false,
            );
        }
    }

    iy + ih + app.s(10)
}

fn draw_appearance(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 左栏：预览 + 主题预设；右栏：逐项配色。
    let preview_h = app.card_h_inner(app.s(186));
    let (x, cy0, col_w) = cols.put(0, preview_h);
    let cy = app.card(dc, x, cy0, col_w, preview_h, "候选窗预览", "跟随下面的设置实时变化");
    let pr = RECT { left: x + app.s(18), top: cy, right: x + col_w - app.s(18), bottom: cy + app.s(186) };
    app.draw_candidate_preview(dc, pr);

    let preset_h = app.card_h_inner(app.s(34 + 8 + 34));
    let (x, cy0, col_w) = cols.put(0, preset_h);
    let cy = app.card(dc, x, cy0, col_w, preset_h, "主题与配色", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let presets = app.btn_row(lx, cy, inner, 3, 34);
    for (i, name) in ["浅色", "深色", "护眼"].iter().enumerate() {
        app.button(dc, Id::ThemePreset(i), presets[i], name, theme_preset_active(app, i));
    }
    let files = app.btn_row(lx, cy + app.s(42), inner, 2, 34);
    app.button(dc, Id::SkinExport, files[0], "导出配色文件…", false);
    app.button(dc, Id::SkinImport, files[1], "导入配色文件…", false);

    let color_h = app.card_h_inner(app.s(40 * 7));
    let (x, cy0, col_w) = cols.put(1, color_h);
    let cy = app.card(dc, x, cy0, col_w, color_h, "自定义颜色", "点色块或「选择…」调色");
    let mut fy = cy;
    for (i, label) in COLOR_LABELS.iter().enumerate() {
        let color = color_of(app, i);
        let lr = RECT { left: x + app.s(18), top: fy + app.s(6), right: x + app.s(112), bottom: fy + app.s(28) };
        unsafe {
            text(dc, lr, label, app.font, app.theme.text, DT_LEFT, true);
        }
        let cr = RECT { left: x + app.s(116), top: fy + app.s(6), right: x + app.s(164), bottom: fy + app.s(28) };
        app.chip(dc, Id::ColorPick(i), cr, color);
        let hex = argb_hex(color);
        let hr = RECT { left: cr.right + app.s(10), top: fy, right: cr.right + app.s(106), bottom: fy + app.s(34) };
        unsafe {
            text(dc, hr, &hex, app.font_small, app.theme.text2, DT_LEFT, true);
        }
        let br = RECT { left: x + col_w - app.s(96), top: fy + app.s(2), right: x + col_w - app.s(18), bottom: fy + app.s(32) };
        app.button(dc, Id::ColorPick(i), br, "选择…", false);
        fy += app.s(40);
    }

    cols.bottom() + app.s(10)
}

fn draw_candidate(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 左：布局与字体、文字与外观；右：显示内容、内边距与间距。
    let layout_h = app.card_h(3);
    let (x, cy0, col_w) = cols.put(0, layout_h);
    let cy = app.card(dc, x, cy0, col_w, layout_h, "布局与字体", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_seg(
        dc, lx, cy, inner,
        "排列", "竖排适合逐字挑选",
        Id::SegLayout, &["竖排", "横排"], if app.cfg.appearance.layout == "horizontal" { 1 } else { 0 },
    );
    app.row_seg(
        dc, lx, cy + app.s(ROW_H), inner,
        "位置", "固定时贴在状态栏上方",
        Id::SegFollow, &["跟随光标", "固定右下"], if app.cfg.appearance.follow_mode == "fixed" { 1 } else { 0 },
    );
    let shown = find_label(&FONT_ITEMS, &app.cfg.appearance.font_family);
    app.row_combo(
        dc, lx, cy + app.s(ROW_H) * 2, inner,
        "字体", "候选文字使用的字体",
        Id::FontCombo, &shown,
    );

    let text_h = app.card_h(3);
    let (x, cy0, col_w) = cols.put(0, text_h);
    let cy = app.card(dc, x, cy0, col_w, text_h, "文字与外观", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_slider(dc, lx, cy, inner, "字号", "单位 pt，随 DPI 缩放", Id::FontSize, app.cfg.appearance.font_size as i32, 9, 26, " pt");
    app.row_slider(dc, lx, cy + app.s(ROW_H), inner, "不透明度", "100% 为完全不透明", Id::Opacity, app.cfg.appearance.opacity as i32, 30, 100, " %");
    app.row_slider(dc, lx, cy + app.s(ROW_H) * 2, inner, "圆角", "0 为直角", Id::CornerRadius, app.cfg.appearance.corner_radius as i32, 0, 24, " px");

    let show_h = app.card_h(2);
    let (x, cy0, col_w) = cols.put(1, show_h);
    let cy = app.card(dc, x, cy0, col_w, show_h, "显示内容", "");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_toggle(dc, lx, cy, inner, "显示候选序号", "1. 2. 3. …", Id::SwitchIndex, app.cfg.appearance.show_index);
    app.row_toggle(dc, lx, cy + app.s(ROW_H), inner, "显示候选注释", "拼音或来源提示", Id::SwitchComment, app.cfg.appearance.show_comment);

    let space_h = app.card_h(4);
    let (x, cy0, col_w) = cols.put(1, space_h);
    let cy = app.card(dc, x, cy0, col_w, space_h, "内边距与间距", "数值按显示器 DPI 缩放");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_slider(dc, lx, cy, inner, "左右内边距", "", Id::PadX, app.cfg.appearance.cand_pad_x as i32, 0, 30, " px");
    app.row_slider(dc, lx, cy + app.s(ROW_H), inner, "上下内边距", "", Id::PadY, app.cfg.appearance.cand_pad_y as i32, 0, 16, " px");
    app.row_slider(dc, lx, cy + app.s(ROW_H) * 2, inner, "最小宽度", "", Id::MinWidth, app.cfg.appearance.cand_min_width as i32, 60, 300, " px");
    app.row_slider(dc, lx, cy + app.s(ROW_H) * 3, inner, "横排间距", "仅横排排列时生效", Id::CandGap, app.cfg.appearance.cand_gap as i32, 0, 48, " px");

    cols.bottom() + app.s(10)
}

fn draw_status(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let gap = app.s(COL_GAP);
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 状态栏开关 + 边距
    let status_h = app.card_h(2);
    let (x, cy0, col_w) = cols.put(0, status_h);
    let cy = app.card(dc, x, cy0, col_w, status_h, "状态栏", "贴在屏幕右下角的那一条");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    app.row_toggle(dc, lx, cy, inner, "显示状态栏", "关闭后只能靠快捷键切换", Id::SwitchStatus, app.cfg.appearance.show_status_bar);
    app.row_slider(dc, lx, cy + app.s(ROW_H), inner, "距右下角边距", "", Id::BarMargin, app.cfg.appearance.bar_margin as i32, 0, 40, " px");

    // 皮肤资源说明（点击资源卡片会导出到皮肤目录）
    let help_h = app.card_h_inner(app.s(84));
    let (x, cy0, col_w) = cols.put(1, help_h);
    let cy = app.card(dc, x, cy0, col_w, help_h, "皮肤资源", "点任意一项导出并打开文件");
    let lx = x + app.s(18);
    let lines = [
        "资源是 *.bin：8 字节头 + BGRA 像素。",
        "图标只用 alpha 通道，按主题色着色。",
        "改完点「关于与维护 → 重载配置」即时生效。",
    ];
    for (i, line) in lines.iter().enumerate() {
        unsafe {
            text(
                dc,
                RECT { left: lx, top: cy + i as i32 * app.s(26), right: x + col_w - app.s(18), bottom: cy + i as i32 * app.s(26) + app.s(24) },
                line,
                app.font_small,
                t.text3,
                DT_LEFT,
                true,
            );
        }
    }

    // 皮肤中心：整行 6 列缩略图（11 份资源 = 2 行），下面一排操作。
    let skin_inner = app.s(2 * 96 + 8 + 12 + 34);
    let skin_h = app.card_h_inner(skin_inner);
    let sy = cols.bottom() + gap;
    let cy = app.card(dc, x0, sy, w, skin_h, "皮肤中心", "点击资源导出到皮肤目录");
    let inner = w - app.s(36);
    let lx = x0 + app.s(18);
    let cols_n = 6;
    let cell_gap = app.s(8);
    let cell_w = (inner - cell_gap * (cols_n - 1)) / cols_n;
    let cell_h = app.s(96);
    for i in 0..app.skins.len() {
        let col = (i % cols_n as usize) as i32;
        let row = (i / cols_n as usize) as i32;
        let left = lx + col * (cell_w + cell_gap);
        let top = cy + row * (cell_h + cell_gap);
        let cell = RECT { left, top, right: left + cell_w, bottom: top + cell_h };
        let hot = app.hover == Some(Id::SkinItem(i));
        if let Some(g) = unsafe { Gfx::new(dc) } {
            if hot {
                unsafe { g.fill_round(cell, R_ROW, t.hover) };
            }
        }
        let a = &app.skins[i];
        let (tw, th) = if a.name == "bar_bg.bin" {
            (app.s(92), app.s(30))
        } else {
            (app.s(48), app.s(48))
        };
        let thumb = RECT {
            left: cell.left + (cell_w - tw) / 2,
            top: cell.top + app.s(6) + (app.s(48) - th) / 2,
            right: cell.left + (cell_w - tw) / 2 + tw,
            bottom: cell.top + app.s(6) + (app.s(48) - th) / 2 + th,
        };
        let tint = match a.name.as_str() {
            "cell_active.bin" => t.accent,
            // 悬停胶囊是白色蒙版，直接按文字色着色会变成一块黑方块
            "cell_hover.bin" => mix_argb(t.text, t.card, 0.45),
            _ => t.text,
        };
        unsafe { draw_skin_thumb(dc, thumb, a, tint, t.card) };
        unsafe {
            text(
                dc,
                RECT { left: cell.left + app.s(4), top: cell.top + app.s(58), right: cell.right - app.s(4), bottom: cell.top + app.s(78) },
                &a.label,
                app.font_small,
                t.text,
                DT_CENTER,
                true,
            );
            text(
                dc,
                RECT { left: cell.left + app.s(4), top: cell.top + app.s(76), right: cell.right - app.s(4), bottom: cell.top + app.s(94) },
                &a.source,
                app.font_small,
                t.text3,
                DT_CENTER,
                true,
            );
        }
        app.hits.push(Hit { id: Id::SkinItem(i), rect: cell });
    }
    let by = cy + app.s(2 * 96 + 8 + 12);
    let actions = app.btn_row(lx, by, inner, 4, 34);
    app.button(dc, Id::SkinPackExport, actions[0], "导出皮肤包…", false);
    app.button(dc, Id::SkinPackImport, actions[1], "导入皮肤包…", false);
    app.button(dc, Id::SkinRestore, actions[2], "恢复内置皮肤", false);
    app.button(dc, Id::OpenSkins, actions[3], "打开皮肤目录", false);

    sy + skin_h + app.s(10)
}
fn draw_dict(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let gap = app.s(COL_GAP);
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 左栏：统计 → 上游更新 → 细胞词库
    let stats_h = app.card_h_inner(app.s(3 * 36));
    let (x, cy0, col_w) = cols.put(0, stats_h);
    let cy = app.card(dc, x, cy0, col_w, stats_h, "词库统计", "");
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

    // 上游词库更新：一颗按钮走完（下载→校验→部署），校验不过不动本机词库；另留回滚
    let up_h = app.card_h_inner(app.s(24 + 10 + 34));
    let (x, cy0, col_w) = cols.put(0, up_h);
    let cy = app.card(dc, x, cy0, col_w, up_h, "上游词库更新", "");
    let st = &app.dict_state;
    let line = format!(
        "本机词库 {} · 上次部署备份 {}",
        if st.bundled.is_empty() { "未记录" } else { &st.bundled },
        if st.backup.is_empty() { "无" } else { "有" }
    );
    unsafe {
        text(dc, RECT { left: x + app.s(18), top: cy, right: x + col_w - app.s(18), bottom: cy + app.s(24) }, &line, app.font_small, t.text2, DT_LEFT, true);
    }
    let inner = col_w - app.s(36);
    let btns = app.btn_row(x + app.s(18), cy + app.s(34), inner, 2, 34);
    app.button(dc, Id::DictUpdateOne, btns[0], "更新词库", true);
    app.button(dc, Id::DictRollback, btns[1], "回滚上次", false);

    // 用户词库管理：导出学习记录 / 清空（清空会重建索引，单独提示）
    let user_h = app.card_h(1);
    let (x, cy0, col_w) = cols.put(0, user_h);
    let cy = app.card(dc, x, cy0, col_w, user_h, "用户词库", "导出学习记录，或清空重新学习");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let btns = app.btn_row(lx, cy, inner, 2, 34);
    app.button(dc, Id::ExportUserDict, btns[0], "导出用户词库", false);
    app.button_danger(dc, Id::ClearUserDict, btns[1], "清空用户词库");

    // 右栏：导入词库（批量通道，低优先级）
    let import_h = app.card_h_inner(app.s(34 + 8 + 34 + 10));
    let (x, cy0, col_w) = cols.put(1, import_h);
    let cy = app.card(dc, x, cy0, col_w, import_h, "导入词库", "支持 txt / 搜狗 .scel，低优先级");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let btns = app.btn_row(lx, cy, inner, 2, 34);
    app.button(dc, Id::DictImport, btns[0], "导入词库（txt/scel）…", true);
    app.button(dc, Id::DictImportDir, btns[1], "导入整个文件夹…", false);
    let by = cy + app.s(42);
    app.button(dc, Id::ImportDictExport, RECT { left: lx, top: by, right: lx + (inner - app.s(8)) / 2, bottom: by + app.s(34) }, "导出已导入词库", false);
    unsafe {
        let info = format!("已导入 {} 条（低优先级）", app.stats.imported_count);
        text(
            dc,
            RECT { left: lx + (inner - app.s(8)) / 2 + app.s(8), top: by, right: lx + inner, bottom: by + app.s(34) },
            &info,
            app.font_small,
            t.text3,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
            true,
        );
    }

    // 用户词库备份 / 恢复：备份列表单独一张卡，别和导入混在一起
    let backup_inner = app.s(34 + 8 + 160);
    let backup_h = app.card_h_inner(backup_inner);
    let (x, cy0, col_w) = cols.put(1, backup_h);
    let cy = app.card(dc, x, cy0, col_w, backup_h, "用户词库备份 / 恢复", "备份保存在数据目录的 backups 里");
    let inner = col_w - app.s(36);
    let lx = x + app.s(18);
    let btns = app.btn_row(lx, cy, inner, 2, 34);
    app.button(dc, Id::BackupNow, btns[0], "立即备份", true);
    app.button(dc, Id::Restore, btns[1], "恢复所选备份", false);
    let lr = RECT { left: lx, top: cy + app.s(42), right: lx + inner, bottom: cy + app.s(202) };
    let items: Vec<String> = app
        .backups
        .iter()
        .map(|b| b.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
        .collect();
    let sel = Some(app.backup_sel);
    app.list(dc, Id::BackupRow, lr, &items, sel, "还没有备份");

    // 细胞词库：整行 6 列（26 项 = 5 行），比挤在半栏里少滚一屏
    let cell_cols = 6i32;
    let rows = ((app.cells.len() as i32 + cell_cols - 1) / cell_cols).max(1);
    let cell_h = app.card_h_inner(app.s(rows * 28 + 8 + 34));
    let cell_top = cols.bottom() + gap;
    let cell_desc = if app.cell_dirty {
        "有改动：点「应用」重建索引（约 1~3 分钟）"
    } else {
        "专业词库，按需勾选后点「应用」"
    };
    let cy = app.card(dc, x0, cell_top, w, cell_h, "细胞词库", cell_desc);
    let inner = w - app.s(36);
    let lx = x0 + app.s(18);
    let cell_gap = app.s(8);
    let cw = (inner - cell_gap * (cell_cols - 1)) / cell_cols;
    let items: Vec<(String, bool)> = app.cells.iter().map(|c| (c.label.clone(), c.enabled)).collect();
    for (i, (label, on)) in items.iter().enumerate() {
        let bx = lx + (i as i32 % cell_cols) * (cw + cell_gap);
        let by = cy + (i as i32 / cell_cols) * app.s(28);
        app.checkbox(
            dc,
            Id::CellDict(i),
            RECT { left: bx, top: by, right: bx + cw - app.s(6), bottom: by + app.s(24) },
            label,
            *on,
        );
    }
    let by = cy + app.s(rows * 28 + 8);
    app.button(dc, Id::CellApply, RECT { left: lx, top: by, right: lx + app.s(120), bottom: by + app.s(34) }, "应用", true);

    cell_top + cell_h + app.s(10)
}

fn draw_phrases(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let y0 = page_header(app, dc, x0, y0, w);
    let inner = w - app.s(36);
    let lx = x0 + app.s(18);
    let card_h = app.card_h_inner(app.s(32 + 10 + 178 + 10 + 32 + 10 + 34 + 10 + 22));
    let cy = app.card(dc, x0, y0, w, card_h, "短语表", "添加后立即生效，不用重新部署");

    // 查询
    let qr = RECT { left: lx, top: cy, right: lx + inner - app.s(140), bottom: cy + app.s(32) };
    app.text_field(dc, Id::PhraseQuery, qr, &app.phrase_query.clone(), "按编码或词条筛选，如 nh");
    let view = app.phrase_view();
    let total = app.phrase_count();
    let shown_n = view.len().min(5);
    unsafe {
        text(
            dc,
            RECT { left: qr.right + app.s(10), top: cy, right: lx + inner, bottom: cy + app.s(32) },
            &format!("显示 {shown_n} / {total} 条"),
            app.font_small,
            t.text3,
            DT_RIGHT,
            true,
        );
    }

    // 表格：表头 + 5 行。整页只有这一张卡，宽度够，编码/词条/权重可以分列对齐。
    let table = RECT { left: lx, top: cy + app.s(42), right: lx + inner, bottom: cy + app.s(42 + 178) };
    if let Some(g) = unsafe { Gfx::new(dc) } {
        unsafe {
            g.fill_round(table, R_CTRL, t.field);
            g.stroke_round(table, R_CTRL, t.field_border, 1.0);
        }
    }
    let x_code = table.left + app.s(12);
    let x_text = table.left + (inner as f32 * 0.30) as i32;
    let x_weight = table.right - app.s(110);
    let header_h = app.s(26);
    unsafe {
        text(dc, RECT { left: x_code, top: table.top, right: x_text - app.s(8), bottom: table.top + header_h }, "编码", app.font_small, t.text3, DT_LEFT, true);
        text(dc, RECT { left: x_text, top: table.top, right: x_weight - app.s(8), bottom: table.top + header_h }, "词条", app.font_small, t.text3, DT_LEFT, true);
        text(dc, RECT { left: x_weight, top: table.top, right: table.right - app.s(12), bottom: table.top + header_h }, "权重", app.font_small, t.text3, DT_RIGHT, true);
    }
    if let Some(g) = unsafe { Gfx::new(dc) } {
        unsafe {
            g.fill_round(
                RECT { left: table.left + app.s(8), top: table.top + header_h, right: table.right - app.s(8), bottom: table.top + header_h + 1 },
                0,
                t.divider,
            );
        }
    }
    let row_h = app.s(30);
    let shown = view.iter().take(5).enumerate();
    let mut shown_any = false;
    for (vi, &ri) in shown {
        shown_any = true;
        let top = table.top + header_h + app.s(2) + vi as i32 * row_h;
        let rr = RECT { left: table.left + app.s(4), top, right: table.right - app.s(4), bottom: top + row_h };
        let is_sel = app.phrase_sel == Some(ri);
        let hot = app.hover == Some(Id::PhraseRow(vi));
        if let Some(g) = unsafe { Gfx::new(dc) } {
            unsafe {
                if is_sel {
                    g.fill_round(rr, 6, t.accent_soft);
                } else if hot {
                    g.fill_round(rr, 6, t.hover);
                }
            }
        }
        let (code, body, weight) = match &app.phrase_rows[ri] {
            xunyin_rime::phrases::Row::Entry(e) => (
                e.codes.clone(),
                e.text.clone(),
                e.weight.map(|w| w.to_string()).unwrap_or_else(|| "—".into()),
            ),
            xunyin_rime::phrases::Row::Comment(c) => (String::new(), c.clone(), String::new()),
        };
        let color = if is_sel { t.accent } else { t.text };
        unsafe {
            text(dc, RECT { left: x_code, top: rr.top, right: x_text - app.s(8), bottom: rr.bottom }, &code, app.font_bold, color, DT_LEFT, true);
            text_ellipsis(dc, RECT { left: x_text, top: rr.top, right: x_weight - app.s(8), bottom: rr.bottom }, &body, app.font, color, DT_LEFT);
            text(dc, RECT { left: x_weight, top: rr.top, right: table.right - app.s(12), bottom: rr.bottom }, &weight, app.font_small, t.text3, DT_RIGHT, true);
        }
        app.hits.push(Hit { id: Id::PhraseRow(vi), rect: rr });
    }
    if !shown_any {
        unsafe {
            text(
                dc,
                RECT { left: table.left + app.s(12), top: table.top + header_h, right: table.right - app.s(12), bottom: table.bottom },
                if app.phrase_query.trim().is_empty() { "还没有短语" } else { "没有匹配的短语" },
                app.font_small,
                t.text3,
                DT_LEFT,
                true,
            );
        }
    }
    // 新增 / 修改
    let fy = table.bottom + app.s(10);
    let code_w = app.s(170);
    let weight_w = app.s(110);
    let cr = RECT { left: lx, top: fy, right: lx + code_w, bottom: fy + app.s(32) };
    let tr = RECT { left: cr.right + app.s(8), top: fy, right: lx + inner - weight_w - app.s(8), bottom: fy + app.s(32) };
    let wr = RECT { left: lx + inner - weight_w, top: fy, right: lx + inner, bottom: fy + app.s(32) };
    app.text_field(dc, Id::PhraseCode, cr, &app.phrase_code.clone(), "编码，如 nh");
    app.text_field(dc, Id::PhraseText, tr, &app.phrase_text.clone(), "词条，如 你好");
    app.text_field(dc, Id::PhraseWeight, wr, &app.phrase_weight.clone(), "权重（可空）");

    let by = fy + app.s(42);
    let btns = app.btn_row(lx, by, inner, 4, 34);
    app.button(dc, Id::PhraseAdd, btns[0], "添加 / 更新", true);
    app.button_danger(dc, Id::PhraseDel, btns[1], "删除所选");
    app.button(dc, Id::PhraseImport, btns[2], "导入短语（txt）…", false);
    app.button(dc, Id::DictExport, btns[3], "导出短语文件", false);

    let hint = if app.stats.last_import.is_empty() {
        "格式：每行「词条 + 制表符 + 编码 +（可选）权重」，点中一行可回填修改".to_string()
    } else {
        app.stats.last_import.clone()
    };
    unsafe {
        text(
            dc,
            RECT { left: lx, top: by + app.s(42), right: lx + inner, bottom: by + app.s(64) },
            &hint,
            app.font_small,
            t.text3,
            DT_LEFT,
            true,
        );
    }

    y0 + card_h + app.s(10)
}

fn draw_about(app: &mut App, dc: HDC, x0: i32, y0: i32, w: i32) -> i32 {
    let t = app.theme.clone();
    let gap = app.s(COL_GAP);
    let y0 = page_header(app, dc, x0, y0, w);
    let mut cols = app.cols(x0, y0, w);

    // 左栏：维护 → 备份与诊断 → 更新；右栏：快捷键。两栏高度接近。
    let mcols = if cols.w > app.s(400) { 3 } else { 2 };
    let mrows = (7 + mcols - 1) / mcols;
    let mh = app.card_h_inner(app.s(mrows * 42 - 8));
    let (x, cy0, col_w) = cols.put(0, mh);
    let engine = if app.engine_ready { "引擎就绪" } else { "引擎加载中" };
    let cy = app.card(dc, x, cy0, col_w, mh, "维护", engine);
    let inner = col_w - app.s(36);
    let bw = (inner - app.s(8) * (mcols - 1)) / mcols;
    for (i, (id, label)) in [
        (Id::OpenData, "打开数据目录"),
        (Id::OpenLogs, "打开日志目录"),
        (Id::OpenSkins, "打开皮肤目录"),
        (Id::ClearLogs, "清空日志"),
        (Id::ResetAll, "恢复默认设置"),
        (Id::ReloadConfig, "重载配置"),
        (Id::Redeploy, "重新部署"),
    ]
    .iter()
    .enumerate()
    {
        let bx = x + app.s(18) + ((i as i32) % mcols) * (bw + app.s(8));
        let by = cy + ((i as i32) / mcols) * app.s(42);
        // 最后一颗「重新部署」跨满整行：7 个按钮按 3 列排会剩一个孤儿，拉通更整齐
        let right = if i == 6 { x + col_w - app.s(18) } else { bx + bw };
        let r = RECT { left: bx, top: by, right, bottom: by + app.s(34) };
        // 清空日志 / 恢复默认设置是"会丢东西"的操作，用危险样式（中性底 + 红字）
        if matches!(id, Id::ClearLogs | Id::ResetAll) {
            app.button_danger(dc, *id, r, label);
        } else {
            app.button(dc, *id, r, label, false);
        }
    }

    let dh = app.card_h_inner(app.s(2 * 42 - 8));
    let (x, cy0, col_w) = cols.put(0, dh);
    let cy = app.card(dc, x, cy0, col_w, dh, "备份与诊断", "导出诊断包与设置 · 恢复皮肤");
    let inner = col_w - app.s(36);
    let btns = app.btn_row(x + app.s(18), cy, inner, 2, 34);
    for (i, (id, label)) in [
        (Id::DiagExport, "导出诊断包"),
        (Id::ConfigExport, "导出全部设置"),
        (Id::ConfigImport, "导入全部设置"),
        (Id::SkinRestore, "恢复内置皮肤"),
    ]
    .iter()
    .enumerate()
    {
        let r = btns[i % 2];
        let r = RECT { top: r.top + ((i as i32) / 2) * app.s(42), bottom: r.bottom + ((i as i32) / 2) * app.s(42), ..r };
        app.button(dc, *id, r, label, false);
    }

    let uh = app.card_h_inner(app.s(2 * 42 - 8));
    let (x, cy0, col_w) = cols.put(0, uh);
    let cy = app.card(dc, x, cy0, col_w, uh, "更新", "检查新版本 · 下载安装");
    let inner = col_w - app.s(36);
    let btns = app.btn_row(x + app.s(18), cy, inner, 2, 34);
    app.button(dc, Id::UpdateCheck, btns[0], "检查更新", true);
    app.button(dc, Id::Update, btns[1], "打开下载页", false);
    app.button(dc, Id::UpdateInstall, RECT { left: btns[0].left, top: cy + app.s(42), right: btns[0].right, bottom: cy + app.s(76) }, "下载并安装", false);

    // 快捷键：宽卡分两列（7 + 6 行），窄卡单列 13 行
    let wide_keys = cols.w > app.s(600);
    let key_rows = if wide_keys { 7 } else { 13 };
    let kh = app.card_h_inner(app.s(key_rows * 34));
    let (rx, ry, col_w) = cols.put(1, kh);
    let cy = app.card(dc, rx, ry, col_w, kh, "快捷键", "");
    let lines = [
        ("Shift", "组合中出拼音"),
        ("回车", "默认原样上屏（行为可改）"),
        ("空格", "确认高亮候选词"),
        ("1-9", "选择对应候选"),
        ("Ctrl+.", "切换中英标点"),
        ("Ctrl+数字", "删词后不再出现"),
        ("Tab / [ ] / - =", "候选翻页"),
        ("rq / sj / xq", "日期 / 时间 / 星期"),
        ("v 数字或算式", "大写/进制/算式"),
        ("← / →", "拼音内逐字移动"),
        ("状态栏「符号」", "打开/收起符号面板"),
        ("右键状态栏", "开关菜单"),
        ("状态栏齿轮", "打开本设置中心"),
    ];
    let key_inner = col_w - app.s(36);
    let half = (key_inner - app.s(24)) / 2;
    for (i, (k, v)) in lines.iter().enumerate() {
        let col = if wide_keys { (i / 7) as i32 } else { 0 };
        let row = if wide_keys { (i % 7) as i32 } else { i as i32 };
        let bx = rx + app.s(18) + col * (half + app.s(24));
        let fy = cy + row * app.s(34);
        let kw = if wide_keys { app.s(108) } else { app.s(132) };
        let kr = RECT { left: bx, top: fy, right: bx + kw, bottom: fy + app.s(28) };
        let vr = RECT {
            left: bx + kw,
            top: fy,
            right: if wide_keys { bx + half } else { rx + col_w - app.s(18) },
            bottom: fy + app.s(28),
        };
        unsafe {
            text(dc, kr, k, app.font_bold, t.text, DT_LEFT, true);
            text(dc, vr, v, app.font_small, t.text2, DT_LEFT, true);
        }
    }

    // 输入统计：整行 6 项 + 右侧清零按钮（放两列卡片下面太挤，铺满一行反而清爽）
    let sy = cols.bottom() + gap;
    let sh = app.card_h_inner(app.s(46));
    let cy = app.card(dc, x0, sy, w, sh, "输入统计", "只统计真正上屏的字");
    let st = app.input_stats.clone();
    let speed = match st.today_speed_cpm() {
        Some(v) => format!("{v} 字/分"),
        None => "—".to_string(),
    };
    let rows = [
        ("今日上屏", format!("{} 字", st.today_chars)),
        ("今日次数", format!("{} 次", st.today_commits)),
        ("今日速度", speed),
        ("打字时长", format!("{} 分钟", st.today_minutes())),
        ("累计字数", format!("{} 字", st.total_chars)),
        ("使用天数", format!("{} 天", st.days)),
    ];
    let reset_w = app.s(110);
    let metrics_w = w - app.s(36) - reset_w - app.s(12);
    let cw = metrics_w / 6;
    for (i, (k, v)) in rows.iter().enumerate() {
        let bx = x0 + app.s(18) + i as i32 * cw;
        let kr = RECT { left: bx, top: cy, right: bx + cw - app.s(8), bottom: cy + app.s(20) };
        let vr = RECT { left: bx, top: cy + app.s(18), right: bx + cw - app.s(8), bottom: cy + app.s(44) };
        unsafe {
            text(dc, kr, k, app.font_small, t.text3, DT_LEFT, true);
            text(dc, vr, v, app.font_bold, t.text, DT_LEFT, true);
        }
    }
    app.button(
        dc,
        Id::StatsReset,
        RECT { left: x0 + w - app.s(18) - reset_w, top: cy + app.s(4), right: x0 + w - app.s(18), bottom: cy + app.s(38) },
        "清零统计",
        false,
    );

    sy + sh + app.s(10)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 主题色是 0xAARRGGBB，GDI 的 COLORREF 是 0x00BBGGRR：R/B 必须对调。
    /// 这个方向搞反时，强调色文字会变橙、危险红会变蓝（灰度文字看不出，很容易漏）。
    #[test]
    fn colorref_swaps_red_and_blue() {
        assert_eq!(argb_to_colorref(0xFF2A6FDB), 0x00DB_6F2A);
        assert_eq!(argb_to_colorref(0xFFB4_2318), 0x0018_23B4);
        assert_eq!(argb_to_colorref(0xFFFF_FFFF), 0x00FF_FFFF);
    }

    /// 窄窗单栏时，put(1, ..) 也要落到唯一一栏，并使用整行宽度。
    #[test]
    fn single_column_uses_full_width() {
        let mut c = Cols::new(100, 50, 1200, 16, true);
        let (x, y, w) = c.put(1, 100);
        assert_eq!((x, y, w), (100, 50, 1200));
        let (_, y2, _) = c.put(0, 40);
        assert_eq!(y2, 50 + 100 + 16);
    }

    /// 两栏时第二栏的起点 = 第一栏宽度 + 间距。
    #[test]
    fn two_columns_place_second_column_right() {
        let c = Cols::new(0, 0, 1000, 20, false);
        assert_eq!(c.w, 490);
        assert_eq!(c.x, [0, 510]);
    }
}