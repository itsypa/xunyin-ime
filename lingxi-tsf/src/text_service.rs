//! TSF 文本服务主体：激活/失活、按键路由、组合管理、候选 UI 调度。

use crate::display_attr;
use crate::candidate::CandidateListUI;
use crate::edit_session::{EditAction, EditSession};
use crate::{engine, key};
use lingxi_rime::{kRimeModifierAlt, kRimeModifierControl};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Interface, IUnknownImpl, Ref, Result, BOOL, GUID};

/// 把拼音音节间的分隔空格换成微软拼音风格的分隔符（"ni hao" → "ni'hao"）。
/// 空格与 "'"(U+0027) 均为单字节，替换后字节长度不变，光标偏移无需调整。
fn format_preedit(text: &str, caret_byte: usize) -> (String, usize) {
    (text.replace(' ', "'"), caret_byte.min(text.len()))
}

/// 进程内缓存的配置（每秒至多读一次文件），供按键热路径查询自定义标点等。
fn cached_cfg() -> lingxi_rime::config::LingxiConfig {
    static CFG: std::sync::OnceLock<lingxi_rime::config::CachedConfig> = std::sync::OnceLock::new();
    CFG.get_or_init(lingxi_rime::config::CachedConfig::new).get()
}

/// 文本服务共享状态。
///
/// 所有字段只允许在 TSF 线程（激活文本服务的线程）访问；
/// 用 Arc<Mutex> 仅是为了让编辑会话对象可以安全地随异步回调存活。
#[derive(Default)]
pub struct SharedState {
    pub tid: u32,
    pub active: bool,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub focused_doc: Option<ITfDocumentMgr>,
    /// 最近一次按键对应的输入上下文（点击候选窗提交时复用）。
    pub last_context: Option<ITfContext>,
    /// 组合串的屏幕锚点（候选窗定位）。
    pub caret_origin: Option<(i32, i32)>,
    /// 最近一次组合串的屏幕矩形（估算光标前进距离用）。
    pub comp_rect: Option<(i32, i32, i32, i32)>,
    /// 计算 caret_origin 时的光标位置（与 caret 不一致表示锚点过期，
    /// 候选窗不应按过期锚点移动，等待定时器用新锚点纠正）。
    pub origin_caret: usize,
    /// Rime 侧当前光标（每次按键从快照更新）。锚点只有在其 origin_caret
    /// 与它一致时才“新鲜”，否则是上一拍异步会话的旧锚点，候选窗不能据此移动。
    pub pending_caret: usize,
    /// 组合内光标位置（组合串 UTF-16 偏移，候选窗定位/下划线光标用）。
    pub caret: usize,
    /// 组合串 UTF-16 长度。
    pub preedit_len: usize,
    /// 上次写入组合串的文本（文本未变化时跳过 SetText，减少应用侧闪烁）。
    pub last_preedit: String,
    /// 当前组合是否已设置过下划线显示属性（组合期间只设置一次，
    /// 避免 Chromium 每次按键重复 OnUpdateComposition 导致组合重绘闪烁）。
    pub attr_applied: bool,
    /// Shift 键按下状态与是否夹带了其它按键（用于判定“裸 Shift”）。
    pub shift_down: bool,
    pub shift_chord: bool,
    /// 当前输入模式（候选窗状态标记用）。
    pub ascii_mode: bool,
    pub full_shape: bool,
    pub ascii_punct: bool,
    /// 简入繁出（状态栏「简/繁」格）。
    pub simplification: bool,
    pub composition: Option<ITfComposition>,
    pub ui_element_mgr: Option<ITfUIElementMgr>,
    pub ui_element: Option<windows_core::ComObject<CandidateListUI>>,
    pub ui_element_id: Option<u32>,
}

// SAFETY: 如模块注释所述，所有访问都发生在 TSF 线程；跨线程仅传递所有权。
unsafe impl Send for SharedState {}

/// TSF 文本服务对象。
#[implement(
    ITfTextInputProcessor,
    ITfTextInputProcessorEx,
    ITfThreadMgrEventSink,
    ITfKeyEventSink,
    ITfDisplayAttributeProvider,
    ITfCompositionSink,
)]
pub struct TextService {
    shared: Arc<Mutex<SharedState>>,
}

impl TextService {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(SharedState::default())),
        }
    }
}

impl TextService_Impl {
    /// 调度一次编辑会话（组合更新/提交/清除），优先同步执行。
    fn schedule_edit(
        &self,
        context: &ITfContext,
        preedit: &str,
        commit: &str,
        caret: usize,
    ) -> Result<()> {
        let action = if !commit.is_empty() {
            EditAction::Commit(commit.to_string())
        } else if !preedit.is_empty() {
            EditAction::Update(preedit.to_string(), caret)
        } else {
            EditAction::Clear
        };
        crate::tsf_log!("schedule_edit: action={action:?}");
        let st = self.shared.lock().map_err(lock_error)?;
        let tid = st.tid;
        drop(st);
        let sink: ITfCompositionSink = self.to_interface();
        let session = windows_core::ComObject::new(EditSession {
            shared: self.shared.clone(),
            context: context.clone(),
            sink,
            action,
        });
        let session_if: ITfEditSession = session.cast()?;
        // 注意：TF_ES_SYNC 单独使用只是「同步只读」会话，组合串写入
        // 必须叠加 TF_ES_READWRITE（位 1|2|4 = 7）才能 SetText/SetSelection。
        let hr = unsafe {
            context.RequestEditSession(
                tid,
                &session_if,
                TF_ES_SYNC | TF_ES_READWRITE,
            )
        };
        match &hr {
            Ok(h) => crate::tsf_log!("schedule_edit: sync granted hr=0x{:08X}", h.0 as u32),
            Err(e) => crate::tsf_log!(
                "schedule_edit: sync REQUEST ERR {e:?} (0x{:08X})",
                e.code().0 as u32
            ),
        }
        let hr = hr?;
        if !hr.is_ok() {
            // 应用正处于自己的编辑会话中：降级为异步编辑会话
            let hr2 =
                unsafe { context.RequestEditSession(tid, &session_if, TF_ES_ASYNCDONTCARE | TF_ES_READWRITE) };
            match &hr2 {
                Ok(h) => crate::tsf_log!("schedule_edit: async granted hr=0x{:08X}", h.0 as u32),
                Err(e) => crate::tsf_log!(
                    "schedule_edit: async REQUEST ERR {e:?} (0x{:08X})",
                    e.code().0 as u32
                ),
            }
        }
        Ok(())
    }

    /// 刷新自绘候选窗（有候选则显示/定位，无则隐藏）。
    fn update_candidates(
        &self,
        candidates: Vec<String>,
        comments: Vec<String>,
        selected: usize,
        page: usize,
        page_size: usize,
    ) {
        let (mut origin, shared, mut origin_fresh) = {
            let st = match self.shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            crate::tsf_log!(
                "update_candidates: origin={}",
                st.caret_origin
                    .map(|o| format!("({},{})", o.0, o.1))
                    .unwrap_or_else(|| "None".into())
            );
            // 锚点对应的光标位置必须与当前光标一致，否则跳过本次移动
            // （Chromium 等应用中异步编辑会话会让锚点滞后一拍，
            //  直接按滞后锚点移动会出现“先往前跳一下再跟回来”）。
            let fresh = st.caret_origin.is_some() && st.origin_caret == st.pending_caret;
            (
                if fresh { st.caret_origin } else { None },
                self.shared.clone(),
                fresh,
            )
        };
        // 锚点过期（异步会话滞后）时，先尽力同步刷新一次，
        // 能拿到读锁就直接用新锚点定位，避免“停一下再跳”的顿挫感。
        if !origin_fresh {
            let (o, f) = crate::candidate_window::probe_anchor_now(&shared);
            origin = o;
            origin_fresh = f;
        }
        // 仍拿不到新锚点（Chromium 异步会话）且光标是向前走的：用组合串
        // 平均字宽估算光标前进量，让候选窗先贴近新位置，落地后再微调。
        // 仅限已有组合（st.caret>0），避免新组合开始时用旧位置估算导致闪现错位。
        if !origin_fresh {
            if let Ok(st) = shared.lock() {
                if let (Some(prev), Some(rect)) = (st.caret_origin, st.comp_rect) {
                    let len = st.preedit_len.max(1) as f64;
                    let w = (rect.2 - rect.0) as f64 / len;
                    let delta = st.pending_caret.saturating_sub(st.caret);
                    if st.caret > 0 && delta > 0 && delta <= 2 {
                        let est_x = prev.0 + (w * delta as f64) as i32;
                        origin = Some((est_x, prev.1));
                        crate::tsf_log!(
                            "update_candidates: estimated origin ({},{})",
                            est_x,
                            prev.1
                        );
                    }
                }
            }
        }
        if candidates.is_empty() {
            crate::candidate_window::hide_candidates();
            return;
        }
        crate::candidate_window::show_candidates(
            shared,
            candidates,
            comments,
            selected,
            page,
            page_size,
            origin,
        );
        crate::tsf_log!("update_candidates: origin_fresh={origin_fresh}");
    }

    /// 按键真正处理：交给 librime，消费后更新组合/提交/候选。
    fn handle_key_down(&self, context: &ITfContext, vk: u32, lparam: usize) -> bool {
        // 只处理按下（bit31=0 表示 down）
        if (lparam & 0x8000_0000) != 0 {
            return false;
        }
        let mask = key::modifier_mask();
        // Ctrl+. ：切换中英标点（搜狗标准快捷键），其余 Ctrl/Alt 交给应用
        if mask & kRimeModifierControl != 0 && mask & kRimeModifierAlt == 0 {
            if vk == 0xBE {
                let mut cfg = lingxi_rime::config::load_config();
                cfg.input.ascii_punct = !cfg.input.ascii_punct;
                let _ = lingxi_rime::config::save_config(&cfg);
                if let Some(engine_arc) = engine::ensure_engine() {
                    if let Ok(mut e) = engine_arc.lock() {
                        e.refresh_options(vec![
                            ("ascii_punct".into(), cfg.input.ascii_punct),
                        ]);
                    }
                }
                if let Ok(mut st) = self.shared.lock() {
                    st.ascii_punct = cfg.input.ascii_punct;
                }
                crate::tsf_log!(
                    "keydown: Ctrl+. -> ascii_punct={}",
                    cfg.input.ascii_punct
                );
                return true;
            }
            return false;
        }
        if mask & (kRimeModifierControl | kRimeModifierAlt) != 0 {
            crate::tsf_log!("keydown: vk=0x{vk:02X} mask=0x{mask:X} -> passthrough (ctrl/alt)");
            return false;
        }
        let Some(engine_arc) = engine::ensure_engine() else {
            crate::tsf_log!("keydown: vk=0x{vk:02X} -> no engine");
            return false;
        };
        let mut eng = match engine_arc.lock() {
            Ok(e) => e,
            Err(_) => {
                crate::tsf_log!("keydown: vk=0x{vk:02X} -> engine lock failed");
                return false;
            }
        };
        // 组合中方向键：逐字符移动并跳过音节分隔符（"'" 不作为停靠位），
        // 到边界不循环；完全由输入法消费，避免应用也收到方向键。
        if (vk == 0x25 || vk == 0x27) && eng.is_composing() {
            return self.move_caret_skip_sep(&mut eng, context, vk == 0x25);
        }
        // 每 3 秒刷新一次共享配置，让全角/半角等设置即时生效
        {
            use std::sync::OnceLock;
            use std::time::Instant;
            static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
            let cfg_last = LAST.get_or_init(|| {
                Mutex::new(Instant::now() - std::time::Duration::from_secs(10))
            });
            let elapsed = cfg_last.lock().map(|t| t.elapsed().as_secs()).unwrap_or(0);
            if elapsed >= 3 {
                let cfg = lingxi_rime::config::load_config();
                // ascii_mode 不在此处应用：它由 Shift 键在 Rime 内切换，
                // 配置里的 ascii_mode 只是新会话的默认值（set_default_options）。
                eng.refresh_options(vec![
                    ("full_shape".into(), cfg.input.full_shape),
                    ("ascii_punct".into(), cfg.input.ascii_punct),
                    ("simplification".into(), cfg.input.simplification),
                ]);
                eng.set_default_options(vec![
                    ("full_shape".into(), cfg.input.full_shape),
                    ("ascii_punct".into(), cfg.input.ascii_punct),
                    ("ascii_mode".into(), cfg.input.ascii_mode),
                    ("simplification".into(), cfg.input.simplification),
                ]);
                // 默认方案/每页候选数变更：销毁会话，让下一次按键按新配置重建
                // （page_size 来自 schema 配置，部署完成后新建会话即生效）。
                static LAST_INPUT: OnceLock<Mutex<Option<(String, u32)>>> = OnceLock::new();
                let mut input_last = match LAST_INPUT
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                {
                    Ok(g) => g,
                    Err(poison) => poison.into_inner(),
                };
                let cur = (cfg.input.schema.clone(), cfg.input.page_size);
                let changed = input_last.as_ref() != Some(&cur);
                if changed {
                    *input_last = Some(cur);
                    eng.set_default_schema(&cfg.input.schema);
                    crate::tsf_log!(
                        "keydown: input config changed (schema={} page_size={}) -> reset session",
                        cfg.input.schema,
                        cfg.input.page_size
                    );
                    eng.destroy_session();
                }
                if let Ok(mut t) = cfg_last.lock() {
                    *t = Instant::now();
                }
            }
        }
        let shift = key::key_state_down(0x10);
        let keycode = key::char_keycode(vk, shift);
        // 回车走 Rime 原生逻辑：组合中把拼音原样上屏（与 Shift 一致）；
        // 选词（确认候选）只由空格承担。
        //
        // 自定义中文标点（设置 → 输入设置 → 标点自定义）优先于 Rime 内置标点表：
        // 命中时先把当前组合原样上屏，再上屏配置的文本；英文模式/英文标点下不生效。
        let mut custom_commit: Option<String> = None;
        let consumed = match key::vk_to_punct(vk, shift) {
            Some(ch) => {
                let cfg = cached_cfg();
                let ascii_now = eng
                    .get_status()
                    .map(|s| s.is_ascii_mode)
                    .unwrap_or(false);
                let hit = if cfg.input.ascii_punct || ascii_now {
                    None
                } else {
                    cfg.input.punct_lookup(ch)
                };
                match hit {
                    Some(out) => {
                        let mut text = String::new();
                        if eng.is_composing() {
                            let _ = eng.commit_composition();
                            text.push_str(&eng.get_commit_text());
                        }
                        text.push_str(&out);
                        crate::tsf_log!("keydown: punct_map {ch:?} -> {text:?}");
                        custom_commit = Some(text);
                        true
                    }
                    None => eng.process_key(keycode, mask),
                }
            }
            None => eng.process_key(keycode, mask),
        };
        crate::tsf_log!(
            "keydown: vk=0x{vk:02X} shift={shift} keycode=0x{keycode:X} mask=0x{mask:X} consumed={consumed}"
        );
        if !consumed {
            return false;
        }
        // 记录输入上下文与焦点文档（点击选字/候选窗定位依赖）
        {
            let mut st = match self.shared.lock() {
                Ok(s) => s,
                Err(_) => return false,
            };
            st.last_context = Some(context.clone());
            if let Some(tim) = st.thread_mgr.clone() {
                match unsafe { tim.GetFocus() } {
                    Ok(doc) => {
                        let changed = st
                            .focused_doc
                            .as_ref()
                            .map(|d| d.as_raw() != doc.as_raw())
                            .unwrap_or(true);
                        st.focused_doc = Some(doc);
                        if changed {
                            crate::tsf_log!("keydown: focused_doc refreshed");
                        }
                    }
                    Err(e) => crate::tsf_log!("keydown: GetFocus failed: {e}"),
                }
            }
        }
        let commit = custom_commit.unwrap_or_else(|| eng.get_commit_text());
        let snapshot = eng.get_context();
        // 记录当前模式（候选窗状态标记：中/英、全/半角）
        if let Ok(mut st) = self.shared.lock() {
            if let Some(status) = eng.get_status() {
                st.ascii_mode = status.is_ascii_mode;
                st.full_shape = status.is_full_shape;
            }
            st.simplification = eng.get_option("simplification");
        }
        crate::status_bar::refresh_status_bar(&self.shared);
        if vk == 0x0D {
            crate::tsf_log!(
                "keydown: Enter commit_text={commit:?} composing_snapshot={}",
                snapshot
                    .as_ref()
                    .map(|s| s.preedit.clone())
                    .unwrap_or_default()
            );
        }
        // 拼音音节间加分隔符号（"ni hao" → "ni'hao"，微软拼音风格）
        let (preedit, caret) = snapshot
            .as_ref()
            .map(|s| format_preedit(&s.preedit, s.cursor_pos.min(s.preedit.len())))
            .unwrap_or_default();
        let cands: Vec<(String, String)> = snapshot
            .as_ref()
            .map(|s| {
                s.candidates
                    .iter()
                    .map(|c| (c.text.clone(), c.comment.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let candidates: Vec<String> = cands.iter().map(|(t, _)| t.clone()).collect();
        let comments: Vec<String> = cands.iter().map(|(_, c)| c.clone()).collect();
        let selected = snapshot
            .as_ref()
            .map(|s| s.highlighted.max(0) as usize)
            .unwrap_or(0);
        let page = snapshot.as_ref().map(|s| s.page_no.max(0) as usize).unwrap_or(0);
        let page_size = snapshot.as_ref().map(|s| s.page_size.max(0) as usize).unwrap_or(5);
        let cand_count = candidates.len();
        drop(eng);

        // 记录 Rime 侧当前光标（候选窗锚点新鲜度判据）
        if let Ok(mut st) = self.shared.lock() {
            st.pending_caret = caret;
        }
        // 组合文本与光标都没变（例如翻页/高亮切换）→ 无需再跑编辑会话，
        // 减少浏览器里的重复组合更新（降低光标跳动）。
        let unchanged = if !commit.is_empty() {
            false
        } else {
            match self.shared.lock() {
                Ok(st) => {
                    st.composition.is_some()
                        && st.last_preedit == preedit
                        && st.caret == caret
                }
                Err(_) => false,
            }
        };
        if unchanged {
            crate::tsf_log!("keydown: preedit/caret unchanged, skip edit session");
        } else {
            let _ = self.schedule_edit(context, &preedit, &commit, caret);
        }
        self.update_candidates(candidates, comments, selected, page, page_size);
        crate::tsf_log!(
            "keydown: vk=0x{vk:02X} -> consumed, preedit={preedit:?} commit={commit:?} candidates={cand_count}"
        );
        true
    }

    /// 组合中方向键的显示空间移动：光标只停靠在字母之间，音节分隔符被跳过。
    /// 同步调用 Rime 的 set_caret_pos（原始输入坐标），保证退格/上屏一致。
    fn move_caret_skip_sep(
        &self,
        eng: &mut lingxi_rime::RimeEngine,
        context: &ITfContext,
        left: bool,
    ) -> bool {
        let Some(s) = eng.get_context() else {
            return true;
        };
        let (display, caret) =
            format_preedit(&s.preedit, s.cursor_pos.min(s.preedit.len()));
        let bytes = display.as_bytes();
        let len = bytes.len();
        let is_sep = |i: usize| bytes[i] == b'\'';
        // 位置 P 有效：光标左侧紧邻的不是分隔符（分隔符归到左侧音节，
        // "ni'|hao" 不算停靠位，每个字母仍保有各自的停靠点）。
        let valid = |p: usize| {
            if p == 0 || p >= len {
                return true;
            }
            !is_sep(p - 1)
        };
        let mut p = caret.min(len);
        if left {
            while p > 0 {
                p -= 1;
                if valid(p) {
                    break;
                }
            }
        } else {
            while p < len {
                p += 1;
                if valid(p) {
                    break;
                }
            }
        }
        // 原始光标 = 显示位置 - 前面分隔符数量（"'" 与空格同为 1 字节）
        let seps_before = bytes[..p].iter().filter(|b| **b == b'\'').count();
        let raw = p - seps_before;
        eng.set_caret_pos(raw);
        if let Ok(mut st) = self.shared.lock() {
            st.pending_caret = p;
        }
        let _ = self.schedule_edit(context, &display, "", p);
        crate::tsf_log!("arrow: display caret {caret} -> {p} (raw {raw})");
        true
    }
}

/// 预判按键是否可能被消费（无副作用，供 OnTestKeyDown）。
fn test_key(vk: u32, lparam: usize) -> bool {
    if (lparam & 0x8000_0000) != 0 {
        return false;
    }
    let mask = key::modifier_mask();
    // Ctrl+.：中英标点切换（本输入法消费）
    if mask & kRimeModifierControl != 0 && mask & kRimeModifierAlt == 0 && vk == 0xBE {
        return true;
    }
    if mask & (kRimeModifierControl | kRimeModifierAlt) != 0 {
        return false;
    }
    let shift = key::key_state_down(0x10);
    // 裸 Shift：交给 Rime 处理（中英切换），松开时按需切换
    if key::is_shift_key(vk) {
        return engine::engine().is_some();
    }
    // Shift+方向键/编辑键：也送达 Rime，让它知道“不是裸 Shift”，
    // 避免松开 Shift 时误切中英模式（文本选择等场景）。
    if shift && key::is_function_key(vk) {
        return engine::engine().is_some();
    }
    if key::vk_to_char(vk, shift).is_some() || key::vk_to_punct(vk, shift).is_some() {
        return true;
    }
    if key::is_function_key(vk) {
        return engine::engine()
            .map(|e| e.lock().map(|mut e| e.is_composing()).unwrap_or(false))
            .unwrap_or(false);
    }
    false
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> windows_core::Error {
    windows_core::Error::from(windows::Win32::Foundation::E_FAIL)
}

// ---------------- ITfTextInputProcessor ----------------

impl ITfTextInputProcessor_Impl for TextService_Impl {
    fn Activate(&self, ptim: Ref<'_, ITfThreadMgr>, tid: u32) -> Result<()> {
        crate::tsf_log!("TSF Activate: tid={tid}");
        // 引擎懒初始化（失败不阻断激活，按键将放行给应用）
        if engine::ensure_engine().is_none() {
            crate::tsf_log!("TSF Activate: engine init failed; keys will pass through");
        }
        let tim = ptim
            .cloned()
            .ok_or_else(|| windows_core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        {
            let mut st = self.shared.lock().map_err(lock_error)?;
            st.tid = tid;
            st.active = true;
            st.thread_mgr = Some(tim.clone());
            st.ui_element_mgr = tim.cast().ok();
            crate::tsf_log!(
                "TSF Activate: ui_element_mgr={}",
                if st.ui_element_mgr.is_some() {
                    "OK"
                } else {
                    "MISSING"
                }
            );
            // 初始焦点文档（候选窗定位依赖 GetDocumentMgr）
            match unsafe { tim.GetFocus() } {
                Ok(doc) => {
                    st.focused_doc = Some(doc);
                    crate::tsf_log!("TSF Activate: GetFocus ok");
                }
                Err(e) => crate::tsf_log!("TSF Activate: GetFocus failed: {e}"),
            }
        }
        // 线程管理器事件订阅（焦点/文档切换）
        let source: ITfSource = tim.cast()?;
        let tm_sink: ITfThreadMgrEventSink = self.to_interface();
        let cookie = unsafe { source.AdviseSink(&ITfThreadMgrEventSink::IID, &tm_sink) };
        crate::tsf_log!(
            "TSF Activate: thread mgr sink cookie={}",
            match &cookie {
                Ok(c) => format!("{c}"),
                Err(e) => format!("ERR {e}"),
            }
        );
        cookie?;
        // 按键事件订阅（前台键盘）
        let ksm: ITfKeystrokeMgr = tim.cast()?;
        let key_sink: ITfKeyEventSink = self.to_interface();
        let kr = unsafe { ksm.AdviseKeyEventSink(tid, &key_sink, true) };
        crate::tsf_log!(
            "TSF Activate: key sink result={}",
            kr.as_ref().map(|_| "OK".to_string()).unwrap_or_else(|e| format!("ERR {e}"))
        );
        kr?;
        // 显示状态栏（搜狗式）
        {
            let cfg = lingxi_rime::config::load_config();
            let (ascii, full, punct, s2t) = match engine::engine() {
                Some(eng) => match eng.lock() {
                    Ok(e) => e
                        .get_status()
                        .map(|s| {
                            (
                                s.is_ascii_mode,
                                s.is_full_shape,
                                cfg.input.ascii_punct,
                                cfg.input.simplification,
                            )
                        })
                        .unwrap_or((
                            cfg.input.ascii_mode,
                            cfg.input.full_shape,
                            cfg.input.ascii_punct,
                            cfg.input.simplification,
                        )),
                    Err(_) => (
                        cfg.input.ascii_mode,
                        cfg.input.full_shape,
                        cfg.input.ascii_punct,
                        cfg.input.simplification,
                    ),
                },
                None => (
                    cfg.input.ascii_mode,
                    cfg.input.full_shape,
                    cfg.input.ascii_punct,
                    cfg.input.simplification,
                ),
            };
            crate::status_bar::show_status_bar(self.shared.clone(), ascii, full, punct, s2t);
        }
        crate::tsf_log!("TSF Activate: done");
        Ok(())
    }

    fn Deactivate(&self) -> Result<()> {
        crate::tsf_log!("TSF Deactivate");
        let mut st = self.shared.lock().map_err(lock_error)?;
        if let (Some(mgr), Some(id)) = (st.ui_element_mgr.clone(), st.ui_element_id.take()) {
            unsafe {
                let _ = mgr.EndUIElement(id);
            }
        }
        st.ui_element = None;
        if let Some(tim) = st.thread_mgr.clone() {
            if let Ok(ksm) = tim.cast::<ITfKeystrokeMgr>() {
                unsafe {
                    let _ = ksm.UnadviseKeyEventSink(st.tid);
                }
            }
        }
        st.active = false;
        st.thread_mgr = None;
        st.ui_element_mgr = None;
        st.focused_doc = None;
        st.last_context = None;
        st.caret_origin = None;
        st.comp_rect = None;
        st.composition = None;
        st.shift_down = false;
        st.shift_chord = false;
        st.pending_caret = 0;
        crate::status_bar::hide_status_bar();
        crate::candidate_window::hide_candidates();
        Ok(())
    }
}

// ---------------- ITfTextInputProcessorEx ----------------

impl ITfTextInputProcessorEx_Impl for TextService_Impl {
    fn ActivateEx(
        &self,
        ptim: Ref<'_, ITfThreadMgr>,
        tid: u32,
        _dwflags: u32,
    ) -> Result<()> {
        ITfTextInputProcessor_Impl::Activate(self, ptim, tid)
    }
}

// ---------------- ITfThreadMgrEventSink ----------------

impl ITfThreadMgrEventSink_Impl for TextService_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, _pdim: Ref<'_, ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Ref<'_, ITfDocumentMgr>,
        _pdimprevfocus: Ref<'_, ITfDocumentMgr>,
    ) -> Result<()> {
        if let Ok(mut st) = self.shared.lock() {
            let new_raw = pdimfocus.cloned().map(|d| d.as_raw());
            let changed = st
                .focused_doc
                .as_ref()
                .map(|d| Some(d.as_raw()) != new_raw)
                .unwrap_or(true);
            st.focused_doc = pdimfocus.cloned();
            if changed {
                // 焦点切到另一个文档：旧的组合/锚点已失效，隐藏候选窗
                crate::tsf_log!("OnSetFocus: document changed -> reset caret anchor");
                st.caret_origin = None;
                st.comp_rect = None;
                st.composition = None;
                st.shift_down = false;
                st.shift_chord = false;
                st.pending_caret = 0;
                crate::candidate_window::hide_candidates();
            }
        }
        Ok(())
    }

    fn OnPushContext(&self, _pic: Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }

    fn OnPopContext(&self, _pic: Ref<'_, ITfContext>) -> Result<()> {
        Ok(())
    }
}

// ---------------- ITfKeyEventSink ----------------

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, fforeground: BOOL) -> Result<()> {
        // 输入焦点变化：本线程获得前台焦点且灵犀仍激活 → 显示状态栏；
        // 失去前台焦点 → 隐藏。避免后台进程残留的状态栏窗口盖住屏幕。
        crate::tsf_log!(
            "OnKeyFocus: foreground={}",
            fforeground.as_bool()
        );
        if fforeground.as_bool() {
            crate::status_bar::refresh_status_bar(&self.shared);
        } else {
            crate::status_bar::hide_status_bar();
        }
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let r = test_key(wparam.0 as u32, lparam.0 as usize);
        crate::tsf_log!(
            "OnTestKeyDown: vk=0x{:02X} lparam=0x{:X} -> {}",
            wparam.0 as u32,
            lparam.0 as usize,
            r
        );
        Ok(BOOL::from(r))
    }

    fn OnTestKeyUp(
        &self,
        _pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        // Shift 松开需要交给 Rime（ascii_composer 在松开时切换中英）
        let r = key::is_shift_key(wparam.0 as u32)
            && engine::engine().is_some();
        crate::tsf_log!(
            "OnTestKeyUp: vk=0x{:02X} -> {}",
            wparam.0 as u32,
            r
        );
        Ok(BOOL::from(r))
    }

    fn OnKeyDown(
        &self,
        pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = wparam.0 as u32;
        crate::tsf_log!(
            "OnKeyDown: vk=0x{:02X} lparam=0x{:X}",
            vk,
            lparam.0 as usize
        );
        // Shift 状态跟踪：用于判定“裸 Shift”（按下后没有夹带其它按键）
        if let Ok(mut st) = self.shared.lock() {
            if key::is_shift_key(vk) {
                st.shift_down = true;
                st.shift_chord = false;
            } else if st.shift_down {
                st.shift_chord = true;
            }
        }
        let handled = match pic.cloned() {
            Some(ctx) => self.handle_key_down(&ctx, vk, lparam.0 as usize),
            None => false,
        };
        crate::tsf_log!("OnKeyDown: vk=0x{vk:02X} -> handled={handled}");
        Ok(BOOL::from(handled))
    }

    fn OnKeyUp(
        &self,
        pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = wparam.0 as u32;
        if key::is_shift_key(vk) {
            let bare = {
                let st = self.shared.lock();
                match st {
                    Ok(mut s) => {
                        let bare = s.shift_down && !s.shift_chord;
                        s.shift_down = false;
                        s.shift_chord = false;
                        bare
                    }
                    Err(_) => false,
                }
            };
            if bare {
                crate::tsf_log!("OnKeyUp: bare Shift");
                if let Some(eng) = engine::engine() {
                    if let Ok(mut e) = eng.lock() {
                        if e.is_composing() {
                            // 组合中：直接把拼音上屏，不切换中英模式
                            let raw = e.get_input();
                            if !raw.is_empty() {
                                e.clear_composition();
                                if let Some(ctx) = pic.cloned() {
                                    let _ = self.schedule_edit(&ctx, "", &raw, 0);
                                }
                                crate::candidate_window::hide_candidates();
                                crate::tsf_log!("OnKeyUp: Shift -> commit pinyin {raw:?}");
                            }
                        } else {
                            // 未组合：切换中英（ascii_mode）
                            let mode = e.get_option("ascii_mode");
                            e.set_option("ascii_mode", !mode);
                            if let Ok(mut st) = self.shared.lock() {
                                st.ascii_mode = !mode;
                            }
                            crate::tsf_log!(
                                "OnKeyUp: Shift -> ascii_mode {} -> {}",
                                mode,
                                !mode
                            );
                            crate::status_bar::refresh_status_bar(&self.shared);
                        }
                    }
                }
            }
        }
        let _ = pic;
        let _ = lparam;
        Ok(BOOL(0))
    }

    fn OnPreservedKey(
        &self,
        _pic: Ref<'_, ITfContext>,
        _rguid: *const GUID,
    ) -> Result<BOOL> {
        Ok(BOOL(0))
    }
}

// ---------------- ITfDisplayAttributeProvider ----------------

impl ITfDisplayAttributeProvider_Impl for TextService_Impl {
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(IEnumTfDisplayAttributeInfo::from(
            display_attr::EnumDisplayAttributes::new(),
        ))
    }

    fn GetDisplayAttributeInfo(&self, guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
        if guid.is_null() || unsafe { *guid } != crate::GUID_LINGXI_DISPLAY_ATTR {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        }
        Ok(ITfDisplayAttributeInfo::from(
            display_attr::DisplayAttributeInfo,
        ))
    }
}

// ---------------- ITfCompositionSink ----------------

impl ITfCompositionSink_Impl for TextService_Impl {
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_core::ComObject;

    #[test]
    fn format_preedit_logic() {
        // 音节空格换成 "'"（字节长度不变，光标偏移保持不变）
        assert_eq!(format_preedit("ni hao", 6), ("ni'hao".into(), 6));
        assert_eq!(format_preedit("ni hao", 0), ("ni'hao".into(), 0));
        // 光标在 "ni h|ao"（字节 4）：替换后仍指向同一位置
        assert_eq!(format_preedit("ni hao", 4), ("ni'hao".into(), 4));
        // 无空格不受影响
        assert_eq!(format_preedit("nihao", 3), ("nihao".into(), 3));
        // 空串
        assert_eq!(format_preedit("", 0), (String::new(), 0));
    }

    /// 验证 TextService 的多接口 vtable：每个 TSF 接口都能从同一 COM 对象 QI 出来
    /// （这是 ctfmon 激活 TIP 的前提）。注：多接口 COM 对象 QI 不同接口时
    /// 返回的指针可能不同（偏移），只要 QI 成功即可。
    #[test]
    fn all_tsf_interfaces_qi() {
        let obj: ComObject<TextService> = ComObject::new(TextService::new());
        let unknown: windows_core::IUnknown = obj.cast().unwrap();

        // 从 IUnknown 再次 QI IUnknown，指针必须稳定（COM 规则）
        let unknown2: windows_core::IUnknown = unknown.cast().unwrap();
        assert_eq!(unknown.as_raw(), unknown2.as_raw());

        let tip: ITfTextInputProcessor = obj.cast().unwrap();
        let tip_ex: ITfTextInputProcessorEx = obj.cast().unwrap();
        let key: ITfKeyEventSink = obj.cast().unwrap();
        let tm: ITfThreadMgrEventSink = obj.cast().unwrap();
        let da: ITfDisplayAttributeProvider = obj.cast().unwrap();
        let cs: ITfCompositionSink = obj.cast().unwrap();

        // 各接口的 IUnknown QI 也应成功
        let _: windows_core::IUnknown = tip.cast().unwrap();
        let _: windows_core::IUnknown = key.cast().unwrap();
        let _: windows_core::IUnknown = cs.cast().unwrap();

        // 接口 GUID 唯一性抽查
        assert_ne!(ITfTextInputProcessor::IID, ITfKeyEventSink::IID);
        assert_ne!(ITfThreadMgrEventSink::IID, ITfCompositionSink::IID);
    }
}
