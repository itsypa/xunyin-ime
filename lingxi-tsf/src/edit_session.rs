//! 编辑会话：在目标应用的文本上下文里执行组合/提交/清除。

use crate::text_service::SharedState;
use crate::GUID_LINGXI_DISPLAY_ATTR;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4,
};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, BOOL, Interface, Result};

/// 一次编辑会话要执行的动作。
#[derive(Debug, Clone)]
pub enum EditAction {
    /// 更新组合串（preedit）与组合内光标位置（preedit 的 UTF-8 字节偏移）。
    Update(String, usize),
    /// 提交文本（结束组合）。
    Commit(String),
    /// 清除组合（Esc）。
    Clear,
}

#[implement(ITfEditSession)]
pub struct EditSession {
    pub shared: Arc<Mutex<SharedState>>,
    pub context: ITfContext,
    pub sink: ITfCompositionSink,
    pub action: EditAction,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> Result<()> {
        crate::tsf_log!("DoEditSession: action={:?}", self.action);
        let result = (|| -> Result<()> {
            let mut st = self
                .shared
                .lock()
                .map_err(|_| windows_core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            match &self.action {
                EditAction::Update(text, caret_byte) => {
                    let text16: Vec<u16> = text.encode_utf16().collect();
                    let text_changed = st.last_preedit != *text;
                    if let Some(comp) = st.composition.clone() {
                        let range = unsafe { comp.GetRange()? };
                        // 组合文本未变化（例如只按方向键移动光标）时跳过 SetText，
                        // 避免应用反复重建组合串导致光标闪烁。
                        if text_changed {
                            unsafe { range.SetText(ec, 0, &text16)? };
                            // 显示属性每个组合只设置一次：属性挂在范围对象上，
                            // SetText 后依然有效。重复设置会触发 Chromium 的
                            // 重复组合更新（OnUpdateComposition ×2）造成重绘闪烁。
                            if !st.attr_applied {
                                set_display_attribute(&self.context, ec, &range)?;
                                st.attr_applied = true;
                            }
                        }
                        let caret_u16 = utf16_prefix_len(text, *caret_byte);
                        move_caret_to(&self.context, ec, range, caret_u16)?;
                        crate::tsf_log!("DoEditSession: update existing comp ok");
                    } else {
                        // 首次出现组合串：插入 preedit 并启动组合
                        let insert: ITfInsertAtSelection = self.context.cast()?;
                        let range = unsafe {
                            insert.InsertTextAtSelection(
                                ec,
                                INSERT_TEXT_AT_SELECTION_FLAGS(0),
                                &text16,
                            )?
                        };
                        let comp_mgr: ITfContextComposition = self.context.cast()?;
                        let comp =
                            unsafe { comp_mgr.StartComposition(ec, &range, &self.sink)? };
                        st.composition = Some(comp);
                        set_display_attribute(&self.context, ec, &range)?;
                        st.attr_applied = true;
                        let caret_u16 = utf16_prefix_len(text, *caret_byte);
                        move_caret_to(&self.context, ec, range, caret_u16)?;
                        crate::tsf_log!("DoEditSession: started composition ok");
                    }
                    // 记录组合内光标（候选窗定位与下次更新用）
                    st.caret = utf16_prefix_len(text, *caret_byte);
                    st.preedit_len = text16.len();
                    crate::tsf_log!(
                        "DoEditSession: composition caret={} len={} text_changed={} preedit={:?}",
                        st.caret,
                        st.preedit_len,
                        text_changed,
                        text
                    );
                    st.last_preedit = text.clone();
                    // 记录组合光标锚点（候选窗定位用）：组合串范围底部中心
                    if let Some(comp) = st.composition.clone() {
                        if let Ok(range) = unsafe { comp.GetRange() } {
                            if let Some((o, r)) = caret_origin_of(
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
                }
                EditAction::Commit(text) => {
                    st.caret_origin = None;
                    st.comp_rect = None;
                    st.origin_caret = 0;
                    st.pending_caret = 0;
                    st.caret = 0;
                    st.preedit_len = 0;
                    st.last_preedit = String::new();
                    st.attr_applied = false;
                    let text16: Vec<u16> = text.encode_utf16().collect();
                    if let Some(comp) = st.composition.clone() {
                        let range = unsafe { comp.GetRange()? };
                        unsafe { range.SetText(ec, 0, &text16)? };
                        clear_display_attribute(&self.context, ec, &range)?;
                        unsafe { comp.EndComposition(ec)? };
                        st.composition = None;
                        move_caret_to(&self.context, ec, range, text16.len())?;
                        crate::tsf_log!("DoEditSession: commit (ended comp) ok");
                    } else {
                        let insert: ITfInsertAtSelection = self.context.cast()?;
                        unsafe {
                            insert.InsertTextAtSelection(
                                ec,
                                INSERT_TEXT_AT_SELECTION_FLAGS(0),
                                &text16,
                            )?;
                        }
                        crate::tsf_log!("DoEditSession: commit (insert) ok");
                    }
                }
                EditAction::Clear => {
                    st.caret_origin = None;
                    st.comp_rect = None;
                    st.origin_caret = 0;
                    st.pending_caret = 0;
                    st.caret = 0;
                    st.preedit_len = 0;
                    st.last_preedit = String::new();
                    st.attr_applied = false;
                    if let Some(comp) = st.composition.clone() {
                        let range = unsafe { comp.GetRange()? };
                        clear_display_attribute(&self.context, ec, &range)?;
                        unsafe { range.SetText(ec, 0, &[])? };
                        unsafe { comp.EndComposition(ec)? };
                        st.composition = None;
                        crate::tsf_log!("DoEditSession: clear ok");
                    }
                }
            }
            Ok(())
        })();
        match &result {
            Ok(()) => crate::tsf_log!("DoEditSession: done"),
            Err(e) => crate::tsf_log!(
                "DoEditSession: FAILED {e:?} (0x{:08X})",
                e.code().0 as u32
            ),
        }
        result
    }
}

/// 组合串屏幕锚点。
///
/// 不同应用对 GetTextExt 的坐标约定不一致：多数返回屏幕坐标，
/// 部分返回视图（客户区）坐标。用 GetScreenExt 判别并归一化：
/// 若文本矩形落在视图屏幕矩形内 → 已为屏幕坐标；否则叠加视图原点。
pub(crate) fn caret_origin_of(
    context: &ITfContext,
    ec: u32,
    range: &ITfRange,
    caret_info: Option<(usize, usize)>,
) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    let view = match unsafe { context.GetActiveView() } {
        Ok(v) => v,
        Err(e) => {
            crate::tsf_log!("caret_origin: GetActiveView FAILED {e:?}");
            return None;
        }
    };
    // 优先取「插入点」范围（选区），它与光标位置一致，跟随最准；
    // 取不到（部分应用对折叠范围返回空矩形）时回退到组合串范围。
    let mut rect = RECT::default();
    let mut clipped = BOOL(0);
    let mut from_selection = false;
    let mut sels = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    let sel_ok = unsafe { context.GetSelection(0, 1, &mut sels, &mut fetched) };
    match (sel_ok, fetched) {
        (Ok(()), n) if n > 0 => {
            if let Some(sel_range) = sels[0].range.as_ref() {
                if unsafe { view.GetTextExt(ec, sel_range, &mut rect, &mut clipped) }.is_ok() {
                    // 光标矩形通常为零宽：left==right；若非零宽选区，取右端（LTR 插入点）
                    if rect.right > rect.left {
                        rect.left = rect.right;
                    }
                    from_selection = true;
                    crate::tsf_log!(
                        "caret_origin: selection ext=({},{})-({},{}) clipped={}",
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom,
                        clipped.0,
                    );
                }
            }
        }
        (Err(e), _) => crate::tsf_log!("caret_origin: GetSelection FAILED {e:?}"),
        _ => {}
    }
    // 释放 GetSelection 返回的 range（TF_SELECTION.range 为 ManuallyDrop）
    unsafe {
        ManuallyDrop::drop(&mut sels[0].range);
    }
    if !from_selection {
        if unsafe { view.GetTextExt(ec, range, &mut rect, &mut clipped) }.is_err() {
            crate::tsf_log!("caret_origin: GetTextExt FAILED (clipped={})", clipped.0);
            return None;
        }
        // 组合串锚点：优先取光标所在位置（LTR）。光标在末尾→矩形右端；
        // 光标在中间→按比例估算 x（选区取不到时的近似）。
        let anchor_x = match caret_info {
            Some((caret, len)) if len > 0 && caret < len => {
                let ratio = caret as f64 / len as f64;
                rect.left + ((rect.right - rect.left) as f64 * ratio) as i32
            }
            Some((_, _)) => rect.right,
            None => {
                if rect.right > rect.left {
                    rect.right
                } else {
                    rect.left
                }
            }
        };
        rect.left = anchor_x;
        crate::tsf_log!(
            "caret_origin: composition ext=({},{})-({},{}) clipped={}",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            clipped.0,
        );
    }
    let screen = match unsafe { view.GetScreenExt() } {
        Ok(s) => s,
        Err(_) => RECT::default(),
    };
    // 文本矩形是否落在视图屏幕范围内（判别坐标空间）
    let inside = rect.left >= screen.left
        && rect.left <= screen.right
        && rect.top >= screen.top
        && rect.top <= screen.bottom;
    let (ox, oy) = if inside {
        (rect.left, rect.bottom)
    } else {
        (rect.left + screen.left, rect.bottom + screen.top)
    };
    // 钳制到视图屏幕范围内：长文档/滚动场景下应用可能返回文档坐标
    // （y 巨大）或退化矩形（y=0），不钳制会导致候选窗贴屏幕顶部/跳屏。
    let (ox, oy) = if screen.right > screen.left && screen.bottom > screen.top {
        (
            ox.clamp(screen.left, screen.right - 1),
            oy.clamp(screen.top, screen.bottom - 2),
        )
    } else {
        (ox, oy)
    };
    crate::tsf_log!(
        "caret_origin: text=({},{})-({},{}) clipped={} screen=({},{})-({},{}) inside={inside} as_screen=({},{}) as_client=({},{}) -> clamped=({ox},{oy})",
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        clipped.0,
        screen.left,
        screen.top,
        screen.right,
        screen.bottom,
        rect.left,
        rect.bottom,
        rect.left + screen.left,
        rect.bottom + screen.top,
    );
    Some(((ox, oy), (rect.left, rect.top, rect.right, rect.bottom)))
}

/// 给组合串范围设置显示属性（下划线）。
fn set_display_attribute(context: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let prop: ITfProperty = unsafe { context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
    // 把显示属性 GUID 注册成 TfGuidAtom，再以 VT_I4 VARIANT 作为属性值
    let catmgr: ITfCategoryMgr = unsafe {
        CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?
    };
    let atom = unsafe { catmgr.RegisterGUID(&GUID_LINGXI_DISPLAY_ATTR)? };
    let value = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: atom as i32 },
            }),
        },
    };
    unsafe { prop.SetValue(ec, range, &value) }
}

/// 清除组合串范围的显示属性。
fn clear_display_attribute(context: &ITfContext, ec: u32, range: &ITfRange) -> Result<()> {
    let prop: ITfProperty = unsafe { context.GetProperty(&GUID_PROP_ATTRIBUTE)? };
    unsafe { prop.Clear(ec, range) }
}

/// 把插入点移到 range 内的第 `pos` 个 UTF-16 码元处（组合串更新后光标跟随，
/// 方向键移动组合内光标时由 Rime 的 cursor_pos 换算而来）。
fn move_caret_to(context: &ITfContext, ec: u32, range: ITfRange, pos: usize) -> Result<()> {
    unsafe {
        // 用 range 的副本定位：折叠到起点 → 平移 end 到 pos → 再折叠成零宽光标
        let caret = range.Clone()?;
        let positioned = (|| -> Result<()> {
            caret.Collapse(ec, TF_ANCHOR_START)?;
            let mut acp = 0i32;
            caret.ShiftEnd(ec, pos as i32, &mut acp, std::ptr::null())?;
            caret.Collapse(ec, TF_ANCHOR_END)?;
            Ok(())
        })();
        // 个别应用不支持按偏移定位（如 ShiftEnd 失败）：回退到组合末尾，
        // 保证组合串更新仍能完成，只是光标停在末尾。
        if positioned.is_err() {
            let _ = caret.Collapse(ec, TF_ANCHOR_END);
        }
        let mut sel = TF_SELECTION {
            range: ManuallyDrop::new(Some(caret)),
            style: TF_SELECTIONSTYLE {
                ase: TF_AE_NONE,
                fInterimChar: windows_core::BOOL(0),
            },
        };
        context.SetSelection(ec, std::slice::from_ref(&sel))?;
        // TF_SELECTION.range 是 ManuallyDrop，SetSelection 已复制所需引用，
        // 这里手动释放我们持有的引用。
        ManuallyDrop::drop(&mut sel.range);
    }
    Ok(())
}

/// preedit（UTF-8）的字节偏移 → UTF-16 码元数（TSF 位置单位）。
fn utf16_prefix_len(text: &str, byte_pos: usize) -> usize {
    let b = byte_pos.min(text.len());
    match text.get(..b) {
        Some(prefix) => prefix.encode_utf16().count(),
        None => text.encode_utf16().count(),
    }
}
