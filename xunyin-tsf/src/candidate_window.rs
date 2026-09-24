#![allow(unused_must_use)]

//! 自绘候选窗（搜狗风格）。
//!
//! TSF 的 `ITfUIElementMgr::BeginUIElement` 只告知 TIP「由谁显示 UI」，
//! Windows 不会为第三方 TIP 渲染默认候选窗（搜狗/QQ 均为自绘）。
//! 本模块提供原生 HWND 候选窗：锚定组合光标、GDI 双缓冲绘制、点击选词、
//! DPI 适配。窗口创建于 TSF 线程，由该线程的消息循环驱动。

use crate::edit_session::{EditAction, EditSession};
use crate::engine;
use crate::text_service::SharedState;
use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfEditSession,
    ITfEditSession_Impl, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{implement, w, Ref, Result, PCWSTR};

const WND_CLASS: &str = "Xunyin.CandidateWindow";
const PAD_X: i32 = 10;
const PAD_TOP: i32 = 3;
const PAD_BOTTOM: i32 = 3;
const MIN_WIDTH: i32 = 110;
/// 组合期间光标追踪定时器（毫秒）。
const CARET_TIMER_ID: usize = 1;
const CARET_TIMER_MS: u32 = 80;

/// 候选窗共享配置缓存（TSF 高频路径，最多 1 秒重读一次）。
static CFG: OnceLock<xunyin_rime::config::CachedConfig> = OnceLock::new();

fn cfg() -> xunyin_rime::config::XunyinConfig {
    CFG.get_or_init(xunyin_rime::config::CachedConfig::new).get()
}

/// "#RRGGBB" → Windows COLORREF（0x00BBGGRR）。
fn parse_color(hex: &str, default: u32) -> u32 {
    let s = hex.trim_start_matches('#');
    if s.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(s, 16) {
            return ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF);
        }
    }
    default
}

/// 每进程一个候选窗（单 TSF 输入线程驱动）。
static WIN: OnceLock<Mutex<Option<CandidateWindow>>> = OnceLock::new();

fn win() -> &'static Mutex<Option<CandidateWindow>> {
    WIN.get_or_init(|| Mutex::new(None))
}

/// 每进程注册一次窗口类。
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

/// 组合终止通知接收器（点击选字提交时由编辑会话使用）。
#[implement(ITfCompositionSink)]
struct CompSink {
    shared: Arc<Mutex<SharedState>>,
}

impl ITfCompositionSink_Impl for CompSink_Impl {
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Ref<'_, ITfComposition>,
    ) -> Result<()> {
        if let Ok(mut st) = self.shared.lock() {
            st.composition = None;
        }
        Ok(())
    }
}

/// 只读编辑会话：组合期间定时重算光标锚点（候选窗跟随鼠标/方向键移动）。
#[implement(ITfEditSession)]
struct CaretProbe {
    shared: Arc<Mutex<SharedState>>,
    context: ITfContext,
}

impl ITfEditSession_Impl for CaretProbe_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        let mut st = match self.shared.lock() {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        if let Some(comp) = st.composition.clone() {
            if let Ok(range) = unsafe { comp.GetRange() } {
                if let Some((o, r)) = crate::edit_session::caret_origin_of(
                    &self.context,
                    ec,
                    &range,
                    Some((st.caret, st.preedit_len)),
                ) {
                    st.caret_origin = Some(o);
                    st.comp_rect = Some(r);
                    st.origin_caret = st.caret;
                }
            }
        }
        Ok(())
    }
}

/// 尽力用一次同步只读编辑会话刷新光标锚点（Chromium 可能授予读锁）。
/// 返回刷新后的 (锚点, 是否新鲜)；拿不到锁时返回原值。
pub fn probe_anchor_now(
    shared: &Arc<Mutex<SharedState>>,
) -> (Option<(i32, i32)>, bool) {
    let (ctx, tid) = {
        let st = match shared.lock() {
            Ok(s) => s,
            Err(_) => return (None, false),
        };
        (st.last_context.clone(), st.tid)
    };
    let Some(ctx) = ctx else {
        return (None, false);
    };
    if tid == 0 {
        return (None, false);
    }
    let probe = windows_core::ComObject::new(CaretProbe {
        shared: shared.clone(),
        context: ctx.clone(),
    });
    let sess: ITfEditSession = match probe.cast() {
        Ok(s) => s,
        Err(_) => return (None, false),
    };
    let _ = unsafe { ctx.RequestEditSession(tid, &sess, TF_ES_SYNC | TF_ES_READ) };
    match shared.lock() {
        Ok(st) => {
            let fresh = st.caret_origin.is_some() && st.origin_caret == st.pending_caret;
            (if fresh { st.caret_origin } else { None }, fresh)
        }
        Err(_) => (None, false),
    }
}

/// 提交/清除组合（点击选字后调用，运行于 TSF 线程）。
fn schedule_commit(shared: &Arc<Mutex<SharedState>>, commit: String) {
    let (ctx, tid) = {
        let st = match shared.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        (st.last_context.clone(), st.tid)
    };
    let Some(ctx) = ctx else { return };
    if tid == 0 {
        return;
    }
    let sink: ITfCompositionSink = match windows_core::ComObject::new(CompSink {
        shared: shared.clone(),
    })
    .cast()
    {
        Ok(s) => s,
        Err(_) => return,
    };
    let action = if commit.is_empty() {
        EditAction::Clear
    } else {
        EditAction::Commit(commit)
    };
    let session = windows_core::ComObject::new(EditSession {
        shared: shared.clone(),
        context: ctx.clone(),
        sink,
        action,
    });
    let sess: ITfEditSession = match session.cast() {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = unsafe { ctx.RequestEditSession(tid, &sess, TF_ES_SYNC | TF_ES_READWRITE) };
    hide_candidates();
}

/// 显示/刷新候选窗（TSF 线程调用）。
pub fn show_candidates(
    shared: Arc<Mutex<SharedState>>,
    items: Vec<String>,
    comments: Vec<String>,
    selected: usize,
    page: usize,
    page_size: usize,
    origin: Option<(i32, i32)>,
) {
    let mut g = match win().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if g.is_none() {
        let w = CandidateWindow::new(shared.clone());
        *g = Some(w);
    }
    let w = match g.as_mut() {
        Some(w) => w,
        None => return,
    };
    if !w.hwnd.0.is_null() {
        unsafe {
            set_window_data(w.hwnd, &*w as *const CandidateWindow as usize);
        }
    }
    *w.shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(shared);
    w.update(items, comments, selected, page, page_size, origin);
}

/// 隐藏候选窗。
pub fn hide_candidates() {
    if let Ok(g) = win().lock() {
        if let Some(w) = g.as_ref() {
            w.hide();
        }
    }
}

/// 自绘候选窗。
pub struct CandidateWindow {
    shared: Mutex<Option<Arc<Mutex<SharedState>>>>,
    hwnd: HWND,
    font: HFONT,
    row_h: i32,
    items: RefCell<Vec<String>>,
    comments: RefCell<Vec<String>>,
    selected: Cell<usize>,
    page: Cell<usize>,
    page_size: Cell<usize>,
    origin: Cell<(i32, i32)>,
    shown: Cell<bool>,
    /// 是否已定位过（首次定位允许 origin=(0,0)，后续忽略无效锚点防跳屏）。
    positioned: Cell<bool>,
    /// 当前在光标上方还是下方（滞回，避免候选数量变化时上下反复翻转）。
    above: Cell<bool>,
    font_pt: Cell<u32>,
    /// 当前字体所用的显示器 DPI（跟随候选窗所在屏，多屏不同缩放各自适配）。
    dpi: Cell<i32>,
}

// 候选窗只由 TSF 线程创建与访问（静态互斥锁串行化），
// 因此可以安全地在进程内共享指针。
unsafe impl Send for CandidateWindow {}
unsafe impl Sync for CandidateWindow {}

impl CandidateWindow {
    fn new(shared: Arc<Mutex<SharedState>>) -> Self {
        let hwnd = if ensure_window_class() {
            create_hwnd()
        } else {
            HWND(std::ptr::null_mut())
        };
        let font_pt = cfg().appearance.font_size.max(8);
        // 初始 DPI 取鼠标所在屏（候选窗随后跟随光标，update/on_timer 会按落点重算）
        let dpi = unsafe {
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_ok() {
                dpi_for_point(pt.x, pt.y)
            } else {
                96
            }
        };
        let (font, row_h) = create_font(font_pt, dpi);
        Self {
            shared: Mutex::new(Some(shared)),
            hwnd,
            font,
            row_h,
            items: RefCell::new(Vec::new()),
            comments: RefCell::new(Vec::new()),
            selected: Cell::new(0),
            page: Cell::new(0),
            page_size: Cell::new(5),
            origin: Cell::new((0, 0)),
            shown: Cell::new(false),
            positioned: Cell::new(false),
            above: Cell::new(false),
            font_pt: Cell::new(font_pt),
            dpi: Cell::new(dpi),
        }
    }

    /// 按候选窗落点所在显示器重建字体/行高（字号或显示器 DPI 变化时）。
    fn ensure_font_for(&mut self, origin: (i32, i32)) {
        let font_pt = cfg().appearance.font_size.max(8);
        let dpi = dpi_for_point(origin.0, origin.1);
        if font_pt == self.font_pt.get() && dpi == self.dpi.get() {
            return;
        }
        if !self.font.0.is_null() {
            unsafe {
                DeleteObject(HGDIOBJ(self.font.0));
            }
        }
        let (font, row_h) = create_font(font_pt, dpi);
        self.font = font;
        self.row_h = row_h;
        self.font_pt.set(font_pt);
        self.dpi.set(dpi);
    }

    fn update(
        &mut self,
        items: Vec<String>,
        comments: Vec<String>,
        selected: usize,
        page: usize,
        page_size: usize,
        origin: Option<(i32, i32)>,
    ) {
        if self.hwnd.0.is_null() {
            return;
        }
        *self.items.borrow_mut() = items;
        *self.comments.borrow_mut() = comments;
        self.selected.set(selected);
        self.page.set(page);
        if page_size > 0 {
            self.page_size.set(page_size);
        }
        if let Some(o) = origin {
            self.origin.set(o);
        }
        // 字号或所在显示器 DPI 变化时重建字体与行高
        self.ensure_font_for(self.origin.get());
        self.position_and_show();
        unsafe {
            InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    fn hide(&self) {
        if self.hwnd.0.is_null() {
            return;
        }
        if self.shown.replace(false) {
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
                KillTimer(Some(self.hwnd), CARET_TIMER_ID);
            }
        }
    }

    /// 组合期间定时重算光标位置并跟随（鼠标点击/方向键/滚动）。
    fn on_timer(&mut self) {
        if !self.shown.get() {
            unsafe {
                KillTimer(Some(self.hwnd), CARET_TIMER_ID);
            }
            return;
        }
        let shared = match self.shared.lock() {
            Ok(g) => match g.clone() {
                Some(s) => s,
                None => return,
            },
            Err(_) => return,
        };
        let (ctx, tid) = {
            let st = match shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            (st.last_context.clone(), st.tid)
        };
        let Some(ctx) = ctx else { return };
        if tid == 0 {
            return;
        }
        let probe = windows_core::ComObject::new(CaretProbe {
            shared: shared.clone(),
            context: ctx.clone(),
        });
        let sess: ITfEditSession = match probe.cast() {
            Ok(s) => s,
            Err(_) => return,
        };
        let _ = unsafe { ctx.RequestEditSession(tid, &sess, TF_ES_SYNC | TF_ES_READ) };
        let (origin, composing, fresh) = {
            let st = match shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            (
                st.caret_origin,
                st.composition.is_some(),
                st.origin_caret == st.pending_caret,
            )
        };
        if !composing {
            // 组合已被应用结束（如鼠标点击提交/取消）：隐藏候选窗
            self.hide();
            return;
        }
        // 锚点必须与 Rime 当前光标一致才移动，避免用异步会话的旧锚点来回跳
        if fresh {
            if let Some(o) = origin {
            if o != self.origin.get() {
                self.origin.set(o);
                self.positioned.set(true);
                // 跟随到别的显示器时按该屏 DPI 重排
                self.ensure_font_for(o);
                self.position_and_show();
            }
            }
        }
    }

    /// 计算窗口尺寸并定位显示（锚定组合光标下方，越界自动翻转/收拢）。
    fn position_and_show(&self) {
        let (w, h) = self.measure();
        let (ox, oy) = self.origin.get();
        // 锚点未知（0,0）且已定位过 → 保持原位，避免候选窗跳到屏幕左上角
        if ox == 0 && oy == 0 && self.positioned.get() {
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOMOVE,
                );
            }
            self.shown.set(true);
            return;
        }
        let mut x = ox;
        let mut y = 0;
        unsafe {
            let mon = MonitorFromPoint(
                POINT { x: ox, y: oy },
                MONITOR_DEFAULTTONEAREST,
            );
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                rcMonitor: RECT::default(),
                rcWork: RECT::default(),
                dwFlags: 0,
            };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                if x + w > mi.rcWork.right {
                    x = mi.rcWork.right - w;
                }
                if x < mi.rcWork.left {
                    x = mi.rcWork.left;
                }
                // 上下方位选择带滞回：优先保持当前一侧，只有当前侧放不下
                // 且另一侧放得下时才翻转，避免候选数量变化导致来回跳。
                let fits_below = oy + 2 + h <= mi.rcWork.bottom;
                let fits_above = oy - h - 2 >= mi.rcWork.top;
                let want_above = if self.above.get() {
                    if fits_above {
                        true
                    } else {
                        !fits_below
                    }
                } else if fits_below {
                    false
                } else {
                    true
                };
                y = if want_above { oy - h - 2 } else { oy + 2 };
                self.above.set(want_above);
                if y < mi.rcWork.top {
                    y = mi.rcWork.top;
                }
            }
            let alpha = (cfg().appearance.opacity.clamp(0, 100) as u32) * 255 / 100;
            // 每次都调用（含 SWP_SHOWWINDOW），保证窗口一定显示、透明度一定生效；
            // 不做“去重”优化，避免隐藏后重现或首次显示时漏调用导致候选框不可见。
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            // 圆角：用窗口区域裁剪（layered 窗口角落真正透明）
            let radius = cfg().appearance.corner_radius.clamp(0, 24) as i32;
            if radius > 0 {
                let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, radius * 2, radius * 2);
                if !rgn.0.is_null() {
                    SetWindowRgn(self.hwnd, Some(rgn), true);
                }
            } else {
                SetWindowRgn(self.hwnd, None, true);
            }
            crate::tsf_log!(
                "candidate_place: origin=({ox},{oy}) -> pos=({x},{y}) size=({w},{h}) above={}",
                self.above.get()
            );
            SetLayeredWindowAttributes(self.hwnd, COLORREF(0), alpha as u8, LWA_ALPHA);
            SetTimer(Some(self.hwnd), CARET_TIMER_ID, CARET_TIMER_MS, None);
        }
        self.positioned.set(true);
        self.shown.set(true);
    }

    fn measure(&self) -> (i32, i32) {
        let items = self.items.borrow();
        let comments = self.comments.borrow();
        let mut max_w = MIN_WIDTH;
        let count = items.len().max(1);
        unsafe {
            let dc = GetDC(Some(self.hwnd));
            if !dc.0.is_null() {
                SelectObject(dc, HGDIOBJ(self.font.0));
                for (i, item) in items.iter().enumerate() {
                    let label = format!("{}.{}", i + 1, item);
                    let comment = comments.get(i).cloned().unwrap_or_default();
                    let mut sz = SIZE::default();
                    let label16: Vec<u16> =
                        label.encode_utf16().chain(std::iter::once(0)).collect();
                    GetTextExtentPoint32W(dc, &label16, &mut sz);
                    let mut total = PAD_X * 2 + sz.cx + 24;
                    if !comment.is_empty() {
                        let comment16: Vec<u16> =
                            comment.encode_utf16().chain(std::iter::once(0)).collect();
                        GetTextExtentPoint32W(dc, &comment16, &mut sz);
                        total += sz.cx;
                    }
                    if total > max_w {
                        max_w = total;
                    }
                }
                ReleaseDC(Some(self.hwnd), dc);
            }
        }
        let h = PAD_TOP + (count as i32) * self.row_h + PAD_BOTTOM;
        (max_w, h)
    }

    fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(self.hwnd, &mut ps) };
        if hdc.0.is_null() {
            return;
        }
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc);
            let mem = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, rc.right - rc.left, rc.bottom - rc.top);
            SelectObject(mem, HGDIOBJ(bmp.0));
            SelectObject(mem, HGDIOBJ(self.font.0));

            let c = cfg().appearance;
            let radius = c.corner_radius.clamp(0, 24) as i32;
            if radius > 0 {
                let rgn = CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, radius * 2, radius * 2);
                if !rgn.0.is_null() {
                    let bg = CreateSolidBrush(COLORREF(parse_color(&c.bg, 0x00FFFFFF)));
                    FillRgn(mem, rgn, bg);
                    DeleteObject(HGDIOBJ(bg.0));
                    let border = CreateSolidBrush(COLORREF(parse_color(&c.border, 0x00C8C8C8)));
                    FrameRgn(mem, rgn, border, 1, 1);
                    DeleteObject(HGDIOBJ(border.0));
                    DeleteObject(HGDIOBJ(rgn.0));
                }
            } else {
                let bg = CreateSolidBrush(COLORREF(parse_color(&c.bg, 0x00FFFFFF)));
                FillRect(mem, &rc, bg);
                DeleteObject(HGDIOBJ(bg.0));
                let border = CreateSolidBrush(COLORREF(parse_color(&c.border, 0x00C8C8C8)));
                FrameRect(mem, &rc, border);
                DeleteObject(HGDIOBJ(border.0));
            }

            let items = self.items.borrow();
            let comments = self.comments.borrow();
            let count = items.len();
            let selected = self.selected.get();
            for i in 0..count {
                let y = PAD_TOP + (i as i32) * self.row_h;
                let is_sel = i == selected;
                if is_sel {
                    let sel_rect = RECT {
                        left: 1,
                        top: y,
                        right: rc.right - 1,
                        bottom: y + self.row_h,
                    };
                    let hl = CreateSolidBrush(COLORREF(parse_color(&c.selected_bg, 0x002A6FDB)));
                    FillRect(mem, &sel_rect, hl);
                    DeleteObject(HGDIOBJ(hl.0));
                }
                SetBkMode(mem, TRANSPARENT);
                let label = if c.show_index {
                    format!("{}.", i + 1)
                } else {
                    String::new()
                };
                let label16: Vec<u16> =
                    label.encode_utf16().chain(std::iter::once(0)).collect();
                let item = &items[i];
                let item16: Vec<u16> =
                    item.encode_utf16().chain(std::iter::once(0)).collect();
                let comment = comments.get(i).cloned().unwrap_or_default();
                let comment16: Vec<u16> =
                    comment.encode_utf16().chain(std::iter::once(0)).collect();
                let text_y = y + (self.row_h - self.font_height(mem)) / 2;
                SetTextColor(
                    mem,
                    COLORREF(if is_sel {
                        parse_color(&c.selected_text, 0x00FFFFFF)
                    } else {
                        parse_color(&c.comment, 0x00888888)
                    }),
                );
                TextOutW(mem, PAD_X, text_y, &label16);
                let label_w = text_width(mem, &label16);
                SetTextColor(
                    mem,
                    COLORREF(if is_sel {
                        parse_color(&c.selected_text, 0x00FFFFFF)
                    } else {
                        parse_color(&c.text, 0x00222222)
                    }),
                );
                TextOutW(mem, PAD_X + label_w + 6, text_y, &item16);
                if !comment.is_empty() {
                    let item_w = text_width(mem, &item16);
                    SetTextColor(mem, COLORREF(parse_color(&c.comment, 0x00999999)));
                    TextOutW(
                        mem,
                        PAD_X + label_w + 6 + item_w + 10,
                        text_y,
                        &comment16,
                    );
                }
            }

            let page = self.page.get();
            let page_size = self.page_size.get().max(1);
            let total_pages = (count + page_size - 1) / page_size;
            if total_pages > 1 {
                let text = format!("{}/{}", page + 1, total_pages);
                let text16: Vec<u16> =
                    text.encode_utf16().chain(std::iter::once(0)).collect();
                let mut sz = SIZE::default();
                GetTextExtentPoint32W(mem, &text16, &mut sz);
                SetTextColor(mem, COLORREF(parse_color(&c.comment, 0x00999999)));
                TextOutW(mem, rc.right - PAD_X - sz.cx, rc.bottom - PAD_BOTTOM - sz.cy, &text16);
            }
            BitBlt(
                hdc,
                0,
                0,
                rc.right - rc.left,
                rc.bottom - rc.top,
                Some(mem),
                0,
                0,
                SRCCOPY,
            );
            DeleteObject(HGDIOBJ(bmp.0));
            DeleteDC(mem);
            EndPaint(self.hwnd, &ps);
        }
    }

    fn font_height(&self, hdc: HDC) -> i32 {
        unsafe {
            let mut tm = TEXTMETRICW::default();
            GetTextMetricsW(hdc, &mut tm);
            tm.tmHeight
        }
    }

    fn on_click(&self, index: usize) {
        let shared = match self.shared.lock() {
            Ok(s) => s.clone(),
            Err(_) => return,
        };
        let Some(shared) = shared else { return };
        let mut commit = String::new();
        if let Some(eng) = engine::ensure_engine() {
            if let Ok(mut e) = eng.lock() {
                if e.select_candidate(index) {
                    commit = e.get_commit_text();
                }
            }
        }
        schedule_commit(&shared, commit);
    }
}

impl Drop for CandidateWindow {
    fn drop(&mut self) {
        if !self.hwnd.0.is_null() {
            unsafe {
                DestroyWindow(self.hwnd);
            }
        }
        if !self.font.0.is_null() {
            unsafe {
                DeleteObject(HGDIOBJ(self.font.0));
            }
        }
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
            w!("XunyinCandidates"),
            WINDOW_STYLE(WS_POPUP.0),
            0,
            0,
            1,
            1,
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

/// 取某点所在显示器的 DPI（多显示器不同缩放下各自适配）。
fn dpi_for_point(x: i32, y: i32) -> i32 {
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(mon, &mut mi.monitorInfo).as_bool() {
            return 96;
        }
        let dc = CreateDCW(PCWSTR(mi.szDevice.as_ptr()), PCWSTR(mi.szDevice.as_ptr()), None, None);
        if dc.0.is_null() {
            return 96;
        }
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSY).max(96);
        DeleteDC(dc);
        dpi
    }
}

/// 创建界面字体并按给定显示器 DPI 缩放行高。
fn create_font(font_pt: u32, dpi: i32) -> (HFONT, i32) {
    unsafe {
        let dpi = dpi.max(96);
        let height = -(dpi * font_pt as i32 / 72);
        let name: Vec<u16> = "Microsoft YaHei UI"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let font = CreateFontW(
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
        );
        let row_h = dpi * (font_pt as i32 + 10) / 72;
        (font, row_h)
    }
}

fn text_width(hdc: HDC, text: &[u16]) -> i32 {
    unsafe {
        let mut sz = SIZE::default();
        GetTextExtentPoint32W(hdc, text, &mut sz);
        sz.cx
    }
}

/// 把候选窗实例指针存入 GWLP_USERDATA（x86 上 LONG_PTR 为 32 位，需适配）。
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

/// 读取候选窗实例指针。
unsafe fn get_window_data(hwnd: HWND) -> usize {
    #[cfg(target_pointer_width = "64")]
    {
        GetWindowLongPtrW(hwnd, GWLP_USERDATA) as usize
    }
    #[cfg(target_pointer_width = "32")]
    {
        GetWindowLongW(hwnd, GWLP_USERDATA) as usize
    }
}

extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let userdata = get_window_data(hwnd);
        if userdata == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // 候选窗只由 TSF 线程创建与访问（静态互斥锁串行化），此处可变借用安全。
        let this = &mut *(userdata as *mut CandidateWindow);
        match msg {
            WM_PAINT => {
                this.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_TIMER => {
                if wparam.0 as usize == CARET_TIMER_ID {
                    this.on_timer();
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let y = ((lparam.0 >> 16) & 0xFFFF) as i32;
                if y >= PAD_TOP {
                    let row = (y - PAD_TOP) / this.row_h;
                    this.on_click(row as usize);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                set_window_data(hwnd, 0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
