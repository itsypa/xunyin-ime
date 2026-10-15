//! TSF 文本服务主体：激活/失活、按键路由、组合管理、候选 UI 调度。

use crate::display_attr;
use crate::candidate::CandidateListUI;
use crate::edit_session::{EditAction, EditSession};
use crate::{engine, key};
use xunyin_rime::{kRimeModifierAlt, kRimeModifierControl};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Variant::VT_I4;
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, Interface, IUnknownImpl, Ref, Result, BOOL, GUID};

/// 读某个 compartment 管理器上的「输入法已被禁用」标志（密码框等敏感输入）。
fn compartment_disabled<T: Interface>(mgr: &T) -> bool {
    let Ok(cm) = mgr.cast::<ITfCompartmentMgr>() else {
        return false;
    };
    let Ok(c) = (unsafe { cm.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED) }) else {
        return false;
    };
    let Ok(v) = (unsafe { c.GetValue() }) else {
        return false;
    };
    // VARIANT 的内部是 union，读取需要 unsafe
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        inner.vt == VT_I4 && inner.Anonymous.lVal != 0
    }
}

/// 顿号键判定：`/`(VK_OEM_2) 与 `\`(VK_OEM_5) 在**没有按 Shift**时直接上屏「、」。
/// 按了 Shift 就不是顿号键了（Shift+/ = `?` → 交给 Rime 出「？」，Shift+\ = `|`）。
fn is_dunhao_key(vk: u32, shift: bool) -> bool {
    !shift && matches!(vk, 0xBF | 0xDC)
}

/// v 模式可接受的输入字符（数字与四则运算符）。
fn v_mode_char(vk: u32, mask: i32) -> Option<char> {
    let shift = mask & xunyin_rime::kRimeModifierShift != 0;
    match vk {
        0x30..=0x39 if !shift => Some((b'0' + (vk - 0x30) as u8) as char),
        0x30 if shift => Some(')'),
        0x39 if shift => Some('('),
        0x38 if shift => Some('*'),
        0xBB if shift => Some('+'),
        0xBD => Some('-'),
        0xBF => Some('/'),
        0xBE => Some('.'),
        0x35 if shift => Some('%'),
        // 小键盘：数字与 + - * / .（v 模式算式的常用输入）
        0x60..=0x69 => Some((b'0' + (vk - 0x60) as u8) as char),
        0x6A => Some('*'),
        0x6B => Some('+'),
        0x6D => Some('-'),
        0x6E => Some('.'),
        0x6F => Some('/'),
        _ => None,
    }
}

// 组合串换算（原始串 ↔ 显示串）与常驻服务共用一份实现：xunyin_rime::preedit。
// 服务接管按键后，两边必须用**同一套映射**，否则显示光标和 Rime 的光标会错位。
use xunyin_rime::preedit::{build as build_preedit, format as format_preedit};

/// 进程内缓存的配置（每秒至多读一次文件），供按键热路径查询自定义标点等。
fn cached_cfg() -> xunyin_rime::config::XunyinConfig {
    static CFG: std::sync::OnceLock<xunyin_rime::config::CachedConfig> = std::sync::OnceLock::new();
    CFG.get_or_init(xunyin_rime::config::CachedConfig::new).get()
}

/// 是否允许把**用户输入**（原始按键、预编辑、候选词、上屏文本）写进日志。
///
/// 默认关：日志会被打进"导出诊断包"，可能被贴到公开 issue 里，所以生产日志只记
/// 长度、数量、状态与错误码（水杉输入法在 windows/AGENTS.md 里也是这条规矩：
/// 生产日志不得记录用户原始按键、预编辑、候选词或提交文本）。
///
/// 排查输入问题时开这个开关（两种方式，任选）：
///   * 在数据目录建一个空文件 `%APPDATA%\com.xunyin.ime\log-input`（**推荐**：
///     宿主应用是 Edge/ChatGPT 这类现成进程，没法给它单独设环境变量）；
///   * 或者设环境变量 `XUNYIN_LOG_INPUT=1`。
/// 文件方式每 5 秒复查一次，删掉文件即恢复"只记长度"。
fn log_input() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if at.elapsed() < Duration::from_secs(5) {
            return v;
        }
    }
    let on = std::env::var("XUNYIN_LOG_INPUT").is_ok()
        || xunyin_rime::config::user_data_dir()
            .join("log-input")
            .exists();
    *g = Some((Instant::now(), on));
    on
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
    pub composition: Option<ITfComposition>,
    pub ui_element_mgr: Option<ITfUIElementMgr>,
    pub ui_element: Option<windows_core::ComObject<CandidateListUI>>,
    pub ui_element_id: Option<u32>,
    /// 叠加候选（快捷输入 / 联想）：显示在 Rime 候选之前，选中后直接上屏。
    /// 点击候选窗时按这个列表映射下标。
    pub overlay: Vec<String>,
    /// 最近一次上屏后的联想词（上屏那一刻就算好，随即显示）。
    pub assoc: Vec<String>,
    /// 上一次上屏的文本与时间（联想学习用；隔太久的不配对，避免学出无意义的组合）。
    pub last_commit: Option<(String, std::time::Instant)>,
    /// v 模式缓冲（含开头的 `v`，如 `v123`）；None = 未激活。
    pub v_mode: Option<String>,
    /// 候选下标映射：显示下标（去掉叠加候选后的偏移）→ Rime 候选下标。
    /// 黑名单过滤掉候选后，靠它把选词下标对上（空表示恒等映射）。
    pub cand_map: Vec<usize>,
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
    /// 把服务返回的结果写回宿主：组合串 / 上屏文本 / 候选窗。
    /// 叠加候选与黑名单过滤服务已经算好，这里只负责显示与提交。
    fn apply_service_reply(
        &self,
        context: &ITfContext,
        r: &xunyin_rime::svc_proto::Reply,
    ) -> bool {
        // 模式状态（状态栏的中英/全半角/标点图标靠它）
        if let Ok(mut st) = self.shared.lock() {
            st.ascii_mode = r.ascii;
            st.full_shape = r.full;
            st.ascii_punct = r.punct;
        }
        crate::status_bar::refresh_status_bar(&self.shared);
        // 服务不消费这次按键：把本进程残留的组合串收干净，再把按键让给应用
        // （例如回车设为"交给宿主换行"时，服务那边已经丢掉组合串了）
        if !r.consume && r.preedit.is_empty() && r.commit.is_empty() {
            let showing = self
                .shared
                .lock()
                .map(|st| st.composition.is_some())
                .unwrap_or(false);
            if showing {
                let _ = self.schedule_edit(context, "", "", 0);
            }
            crate::candidate_window::hide_candidates();
            return false;
        }
        // 记录输入上下文与焦点文档（点击选字 / 候选窗定位依赖）
        {
            let mut st = match self.shared.lock() {
                Ok(s) => s,
                Err(_) => return r.consume,
            };
            st.last_context = Some(context.clone());
            if let Some(tim) = st.thread_mgr.clone() {
                if let Ok(doc) = unsafe { tim.GetFocus() } {
                    st.focused_doc = Some(doc);
                }
            }
        }
        let preedit = r.preedit.clone();
        let commit = r.commit.clone();
        let caret = r.caret;
        if let Ok(mut st) = self.shared.lock() {
            st.pending_caret = caret;
        }
        // 组合串与光标都没变（翻页/高亮切换等）→ 不重跑编辑会话，减少浏览器里的跳动
        let unchanged = if !commit.is_empty() {
            false
        } else {
            match self.shared.lock() {
                Ok(st) => st.composition.is_some() && st.last_preedit == preedit && st.caret == caret,
                Err(_) => false,
            }
        };
        if !unchanged {
            let _ = self.schedule_edit(context, &preedit, &commit, caret);
        }
        // 候选：前 overlay_len 个是叠加候选（快捷输入/联想/v 模式），其余是 Rime 候选
        let n_overlay = r.overlay_len.min(r.candidates.len());
        if let Ok(mut st) = self.shared.lock() {
            st.overlay = r.candidates[..n_overlay].to_vec();
            st.cand_map = (n_overlay..r.candidates.len()).collect();
        }
        self.update_candidates(
            r.candidates.clone(),
            r.comments.clone(),
            r.selected,
            r.page,
            r.page_size.max(1),
        );
        r.consume
    }

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

    /// 当前上下文是否被应用禁用了输入法（密码框、受保护输入等）：
    /// 这种情况下不能接管按键，也不能弹候选窗。
    fn keyboard_disabled(&self) -> bool {
        let (tm, dm) = match self.shared.lock() {
            Ok(st) => (st.thread_mgr.clone(), st.focused_doc.clone()),
            Err(_) => return false,
        };
        if let Some(tm) = tm {
            if compartment_disabled(&tm) {
                return true;
            }
        }
        if let Some(dm) = dm {
            if compartment_disabled(&dm) {
                return true;
            }
        }
        false
    }

    /// 清掉叠加候选（快捷输入 / 联想）。
    fn clear_overlay(&self) {
        if let Ok(mut st) = self.shared.lock() {
            st.overlay.clear();
        }
    }

    /// v 模式的按键处理（`v123` / `v1+2*3`）：Rime 的 speller 不收数字，
    /// 所以进入 v 模式后组合串由我们自己维护。返回 Some 表示按键已处理完。
    fn v_mode_key(&self, context: &ITfContext, vk: u32, mask: i32) -> Option<bool> {
        if !cached_cfg().input.enable_quick_input {
            return None;
        }
        let plain = mask & (kRimeModifierControl | kRimeModifierAlt) == 0;
        let current = match self.shared.lock() {
            Ok(st) => st.v_mode.clone(),
            Err(_) => None,
        };
        let Some(buf) = current else {
            // 进入条件：Rime 组合里恰好是 "v"，紧接着按数字或运算符
            if !plain {
                return None;
            }
            let ch = v_mode_char(vk, mask)?;
            let only_v = engine::engine()
                .and_then(|a| a.lock().ok().and_then(|e| e.get_context()))
                .map(|c| c.preedit.eq_ignore_ascii_case("v"))
                .unwrap_or(false);
            if !only_v {
                return None;
            }
            self.v_mode_update(context, &format!("v{ch}"));
            return Some(true);
        };
        if vk == 0x1B {
            // Esc：退出 v 模式并清掉组合
            self.v_mode_leave();
            let _ = self.schedule_edit(context, "", "", 0);
            crate::candidate_window::hide_candidates();
            return Some(true);
        }
        if vk == 0x08 {
            // Backspace：退到只剩 "v" 就退出
            let mut chars: Vec<char> = buf.chars().collect();
            if chars.len() <= 1 {
                self.v_mode_leave();
                let _ = self.schedule_edit(context, "", "", 0);
                crate::candidate_window::hide_candidates();
            } else {
                chars.pop();
                self.v_mode_update(context, &chars.into_iter().collect::<String>());
            }
            return Some(true);
        }
        // 注意：v 模式里数字是**输入**（v123 的 1/2/3），不能当候选选择键用，
        // 否则第二个数字就会变成"选第 2 项"（实测踩过）。上屏统一用空格/回车。
        if plain && (vk == 0x20 || vk == 0x0D) {
            let cands = xunyin_rime::quick_input::v_mode_candidates(&buf);
            let text = cands
                .first()
                .map(|(t, _)| t.clone())
                .unwrap_or_else(|| buf.clone());
            if log_input() {
                crate::tsf_log!("vmode: 上屏 {text:?}（buf={buf:?}）");
            } else {
                crate::tsf_log!("vmode: 上屏 1 项（{} 字）", text.chars().count());
            }
            self.v_mode_leave();
            let _ = self.schedule_edit(context, "", &text, 0);
            crate::candidate_window::hide_candidates();
            return Some(true);
        }
        if plain {
            if let Some(ch) = v_mode_char(vk, mask) {
                self.v_mode_update(context, &format!("{buf}{ch}"));
                return Some(true);
            }
        }
        // 其它键：退出 v 模式，让按键继续走正常流程
        self.v_mode_leave();
        let _ = self.schedule_edit(context, "", "", 0);
        None
    }

    /// 刷新 v 模式的组合串与候选。
    fn v_mode_update(&self, context: &ITfContext, buf: &str) {
        if let Ok(mut st) = self.shared.lock() {
            st.v_mode = Some(buf.to_string());
        }
        let caret = buf.encode_utf16().count();
        let _ = self.schedule_edit(context, buf, "", caret);
        let cands = xunyin_rime::quick_input::v_mode_candidates(buf);
        let (items, comments): (Vec<String>, Vec<String>) = cands.into_iter().unzip();
        if log_input() {
            crate::tsf_log!("vmode: buf={buf:?} 候选={items:?}");
        } else {
            crate::tsf_log!(
                "vmode: 算式 {} 个字符，候选 {} 项",
                buf.chars().count(),
                items.len()
            );
        }
        if let Ok(mut st) = self.shared.lock() {
            st.overlay = items.clone();
        }
        self.update_candidates(items, comments, 0, 0, cached_cfg().input.page_size as usize);
    }

    fn v_mode_leave(&self) {
        if let Ok(mut st) = self.shared.lock() {
            st.v_mode = None;
            st.overlay.clear();
        }
    }

    /// 叠加候选（快捷输入 / 联想）的选择键：空格（第一项）或 1-9。
    /// 命中就上屏，不再交给 Rime。返回 Some 表示按键已处理完。
    fn overlay_select_key(&self, context: &ITfContext, vk: u32, mask: i32) -> Option<bool> {
        if mask & (kRimeModifierControl | kRimeModifierAlt) != 0 {
            return None;
        }
        if vk != 0x20 && !(0x31..=0x39).contains(&vk) {
            return None;
        }
        let overlay = match self.shared.lock() {
            Ok(st) => st.overlay.clone(),
            Err(_) => Vec::new(),
        };
        if overlay.is_empty() {
            return None;
        }
        let idx = if vk == 0x20 {
            0
        } else {
            (vk - 0x31) as usize
        };
        if idx < overlay.len() {
            let text = overlay[idx].clone();
            if log_input() {
                crate::tsf_log!("overlay: 上屏 {text:?}（快捷输入/联想）");
            } else {
                crate::tsf_log!("overlay: 上屏 1 项（{} 字）", text.chars().count());
            }
            if let Some(arc) = engine::ensure_engine() {
                if let Ok(mut e) = arc.lock() {
                    e.clear_composition(); // 丢掉 rq 之类的拼音
                }
            }
            self.clear_overlay();
            let _ = self.schedule_edit(context, "", &text, 0);
            crate::candidate_window::hide_candidates();
            return Some(true);
        }
        // 下标落在 Rime 候选段：换算成 Rime 下标（黑名单过滤过要查映射表），选它并上屏
        let disp = idx - overlay.len();
        let rime_idx = match self.shared.lock() {
            Ok(st) => st.cand_map.get(disp).copied().unwrap_or(disp),
            Err(_) => disp,
        };
        let mut commit = String::new();
        if let Some(arc) = engine::ensure_engine() {
            if let Ok(mut e) = arc.lock() {
                if e.select_candidate(rime_idx) {
                    commit = e.get_commit_text();
                }
            }
        }
        if commit.is_empty() {
            return None; // 越界：交给正常流程
        }
        self.clear_overlay();
        let _ = self.schedule_edit(context, "", &commit, 0);
        crate::candidate_window::hide_candidates();
        Some(true)
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
        // 应用禁用输入法时（密码框等）不接管按键，也不弹候选窗
        if self.keyboard_disabled() {
            crate::tsf_log!("keydown: vk=0x{vk:02X} -> 敏感输入（应用已禁用输入法），不接管");
            crate::candidate_window::hide_candidates();
            return false;
        }
        // ── 服务优先：输入逻辑在 xunyin-svc.exe 里 ─────────────────────────
        // 改输入逻辑只要换掉服务 exe，正在用的 Edge / ChatGPT 不用重启。
        // 服务没连上 / 超时 / 报错 → 落到下面的本进程逻辑，输入不中断。
        crate::svc::poll();
        if crate::svc::ready() {
            let shift_now = key::key_state_down(0x10);
            if let Some(reply) = crate::svc::key(vk, mask, shift_now) {
                return self.apply_service_reply(context, &reply);
            }
            // 服务这条路暂时不通（超时/断开/服务侧报错）：**绝不能吞键**。
            // 吞了就是"切到输入法之后一个字都打不出来"（真实踩过这个坑：请求通道
            // 建不起来时每颗键都被这里丢掉，连数字都上不去）。
            // 现在把这一颗键交给下面的本进程逻辑处理，服务恢复后自动接回来。
            crate::tsf_log!("svc: 服务暂不可用 -> 这颗键交给本进程逻辑（不丢键）");
        }
        // v 模式（v123 / v1+2*3）由我们自己维护组合串；命中即消费按键
        if let Some(consumed) = self.v_mode_key(context, vk, mask) {
            return consumed;
        }
        // 叠加候选（快捷输入 / 联想）的选择键优先
        if let Some(consumed) = self.overlay_select_key(context, vk, mask) {
            return consumed;
        }
        // Ctrl+数字：删词（把误上屏的词从用户词库/词频里去掉）
        let mut delete_index: Option<usize> = None;
        if mask & kRimeModifierControl != 0
            && mask & kRimeModifierAlt == 0
            && (0x31..=0x39).contains(&vk)
            && cached_cfg().input.delete_key == "ctrl_digit"
        {
            delete_index = Some((vk - 0x31) as usize);
        }
        // Ctrl+. ：切换中英标点（搜狗标准快捷键），其余 Ctrl/Alt 交给应用
        if mask & kRimeModifierControl != 0 && mask & kRimeModifierAlt == 0 {
            if vk == 0xBE {
                let mut cfg = xunyin_rime::config::load_config();
                cfg.input.ascii_punct = !cfg.input.ascii_punct;
                let _ = xunyin_rime::config::save_config(&cfg);
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
            // Ctrl+空格 切换中英（可选设置；默认关，用裸 Shift 切换）
            if vk == 0x20 && cached_cfg().input.toggle_key == "ctrl_space" {
                if let Some(engine_arc) = engine::ensure_engine() {
                    if let Ok(mut e) = engine_arc.lock() {
                        let mode = e.get_option("ascii_mode");
                        e.set_option("ascii_mode", !mode);
                        if let Ok(mut st) = self.shared.lock() {
                            st.ascii_mode = !mode;
                        }
                        crate::tsf_log!("keydown: Ctrl+Space -> ascii_mode {} -> {}", mode, !mode);
                        crate::status_bar::refresh_status_bar(&self.shared);
                    }
                }
                return true;
            }
            // Ctrl+数字（删词）继续往下走，其余 Ctrl/Alt 组合交给应用
            if delete_index.is_none() {
                return false;
            }
        }
        if mask & (kRimeModifierControl | kRimeModifierAlt) != 0 && delete_index.is_none() {
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
                // 顺带看一眼「重载 / 重新部署」请求：装好新方案或改完配置后，
                // 不重启应用也能生效（对标小狼毫的重新部署）。本进程内是
                // 立刻重建引擎，因此本按键会落到新引擎上。
                crate::engine::poll_reload();
                crate::svc::poll();
                let cfg = xunyin_rime::config::load_config();
                // ascii_mode 不在此处应用：它由 Shift 键在 Rime 内切换，
                // 配置里的 ascii_mode 只是新会话的默认值（set_default_options）。
                eng.refresh_options(vec![
                    ("full_shape".into(), cfg.input.full_shape),
                    ("ascii_punct".into(), cfg.input.ascii_punct),
                    ("simplification".into(), true),
                ]);
                eng.set_default_options(vec![
                    ("full_shape".into(), cfg.input.full_shape),
                    ("ascii_punct".into(), cfg.input.ascii_punct),
                    ("ascii_mode".into(), cfg.input.ascii_mode),
                    ("simplification".into(), true),
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
        // 顿号键由 TSF 层直接接管（中文状态）：/ 与 \ 都上屏「、」，不进组合、不查标点表。
        //
        // 背景：方案里改 punctuator、以及改 symbols.yaml 的 full_shape '/' 都试过，
        // 实测仍会出全角斜杠 ／（Rime 的 full_shape 全角转换在标点表之上生效）。
        // 这一层是我们自己能完全掌控的位置，实测 consumed=true 且直接上屏「、」。
        //
        // 注意：**必须放掉 Shift 组合**——Shift+/ 是问号（要出「？」），
        // Shift+\ 是竖线；早期版本不看 shift，把问号也吃成了「、」（用户实测反馈）。
        // 英文模式（ascii_mode / 英文标点）同样不拦，按键照原样透传。
        if is_dunhao_key(vk, shift) && !eng.is_composing() {
            let ascii_now = eng.get_status().map(|s| s.is_ascii_mode).unwrap_or(false);
            if !ascii_now && !cached_cfg().input.ascii_punct {
                crate::tsf_log!("keydown: 顿号键 vk=0x{vk:02X} -> 直接上屏「、」");
                let _ = self.schedule_edit(context, "", "、", 0);
                crate::candidate_window::hide_candidates();
                return true;
            }
        }
        // 回车键行为（设置 → 输入设置 → 回车键）：
        //   commit = 把拼音原样上屏（Rime 原生，与裸 Shift 一致，默认）
        //   clear  = 丢弃当前输入
        //   newline= 丢弃当前输入并把回车交给宿主（换行）
        let mut enter_override: Option<bool> = None;
        if vk == 0x0D {
            match cached_cfg().input.enter_behavior.as_str() {
                "clear" => {
                    if eng.is_composing() {
                        eng.clear_composition();
                    }
                    crate::tsf_log!("keydown: Enter -> clear");
                    enter_override = Some(true);
                }
                "newline" => {
                    if eng.is_composing() {
                        eng.clear_composition();
                        crate::tsf_log!("keydown: Enter -> clear + newline");
                    }
                    enter_override = Some(false);
                }
                _ => {}
            }
        }
        let consumed = match enter_override {
            Some(c) => c,
            None => match delete_index {
                // Ctrl+数字：删掉当前页第 N 个候选（误上屏的词不再出现）
                Some(i) => {
                    // 显示下标 → Rime 下标 → 被删的词
                    let (overlay, map) = match self.shared.lock() {
                        Ok(st) => (st.overlay.clone(), st.cand_map.clone()),
                        Err(_) => (Vec::new(), Vec::new()),
                    };
                    let victim = if i < overlay.len() {
                        Some(overlay[i].clone())
                    } else {
                        let disp = i - overlay.len();
                        let rime_idx = map.get(disp).copied().unwrap_or(disp);
                        eng.get_context()
                            .and_then(|c| c.candidates.get(rime_idx).map(|x| x.text.clone()))
                    };
                    // librime 的删词只对用户词库的学习词有效，词库原生词删不掉，
                    // 所以再记一层自己的黑名单，保证"删了就不再出现"。
                    let api_ok = eng.delete_candidate_on_current_page(i.saturating_sub(overlay.len()));
                    let blocked = victim.as_deref().map(xunyin_rime::blocklist::block).unwrap_or(false);
                    match &victim {
                        Some(w) => {
                            if log_input() {
                                crate::tsf_log!(
                                    "keydown: Ctrl+{} 删词 {w:?}（librime={api_ok} 黑名单={blocked}）",
                                    i + 1
                                );
                            } else {
                                crate::tsf_log!(
                                    "keydown: Ctrl+{} 删词（{} 字，librime={api_ok} 黑名单={blocked}）",
                                    i + 1,
                                    w.chars().count()
                                );
                            }
                        }
                        None => crate::tsf_log!("keydown: Ctrl+{} 删词：该位置没有候选", i + 1),
                    }
                    true
                }
                None => match key::vk_to_punct(vk, shift) {
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
                        if log_input() {
                            crate::tsf_log!("keydown: punct_map {ch:?} -> {text:?}");
                        } else {
                            crate::tsf_log!("keydown: punct_map 命中（{} 字）", text.chars().count());
                        }
                        custom_commit = Some(text);
                        true
                    }
                    None => eng.process_key(keycode, mask),
                }
            }
            None => eng.process_key(keycode, mask),
                },
            },
        };
        // 逐键轨迹默认不写（含原始按键，量大）：排查时开 XUNYIN_LOG_INPUT=1
        if log_input() {
            crate::tsf_log!(
                "keydown: vk=0x{vk:02X} shift={shift} keycode=0x{keycode:X} mask=0x{mask:X} consumed={consumed}"
            );
        }
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
        // 联想：学习「上一词 → 本词」，并取出本词之后的联想候选
        if !commit.is_empty() {
            let cfg = cached_cfg();
            let prev = match self.shared.lock() {
                Ok(mut st) => st.last_commit.take(),
                Err(_) => None,
            };
            if cfg.input.enable_assoc {
                if let Some((p, t)) = prev {
                    // 隔太久的不算「接着打」，避免学出无意义的搭配
                    if t.elapsed() < std::time::Duration::from_secs(5) {
                        xunyin_rime::assoc::learn_and_maybe_save(&p, &commit);
                    }
                }
                let followers = xunyin_rime::assoc::followers(&commit);
                if let Ok(mut st) = self.shared.lock() {
                    st.assoc = followers;
                }
            } else if let Ok(mut st) = self.shared.lock() {
                st.assoc.clear();
            }
            if let Ok(mut st) = self.shared.lock() {
                st.last_commit = Some((commit.clone(), std::time::Instant::now()));
            }
        }
        // 记录当前模式（候选窗状态标记：中/英、全/半角）
        if let Ok(mut st) = self.shared.lock() {
            if let Some(status) = eng.get_status() {
                st.ascii_mode = status.is_ascii_mode;
                st.full_shape = status.is_full_shape;
            }
        }
        crate::status_bar::refresh_status_bar(&self.shared);
        if vk == 0x0D {
            if log_input() {
                crate::tsf_log!(
                    "keydown: Enter commit_text={commit:?} composing_snapshot={}",
                    snapshot
                        .as_ref()
                        .map(|s| s.preedit.clone())
                        .unwrap_or_default()
                );
            } else {
                crate::tsf_log!(
                    "keydown: Enter 上屏 {} 字，组合剩 {} 字",
                    commit.chars().count(),
                    snapshot.as_ref().map(|s| s.preedit.chars().count()).unwrap_or(0)
                );
            }
        }
        // 拼音音节间加分隔符号（"ni hao" → "ni'hao"，微软拼音风格）
        let sep = cached_cfg().input.separator();
        let (preedit, caret) = snapshot
            .as_ref()
            .map(|s| format_preedit(&s.preedit, s.cursor_pos.min(s.preedit.len()), sep))
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
        // 叠加候选：快捷输入（rq/sj/xq）→ 联想 → Rime 候选
        let cfg = cached_cfg();
        let mut overlay: Vec<String> = Vec::new();
        let mut overlay_comments: Vec<String> = Vec::new();
        // 自定义短语优先（与服务侧同一套规则：编码与原始输入完全一致就插到最前）
        if cfg.input.enable_custom_phrase {
            if let Some(snap) = snapshot.as_ref() {
                let code: String = snap.preedit.chars().filter(|c| !c.is_whitespace()).collect();
                for text in xunyin_rime::phrases::matching(&code) {
                    overlay.push(text);
                    overlay_comments.push("短语".into());
                }
            }
        }
        if cfg.input.enable_quick_input {
            if let Some(t) = snapshot
                .as_ref()
                .and_then(|s| xunyin_rime::quick_input::trigger(&s.preedit))
            {
                for (text, comment) in
                    xunyin_rime::quick_input::date_candidates(t, &xunyin_rime::quick_input::local_now())
                {
                    overlay.push(text);
                    overlay_comments.push(comment);
                }
                if log_input() {
                    crate::tsf_log!("keydown: 快捷输入 {t} -> {} 个候选", overlay.len());
                } else {
                    crate::tsf_log!("keydown: 快捷输入命中 -> {} 个候选", overlay.len());
                }
            }
        }
        if overlay.is_empty() && cfg.input.enable_assoc && preedit.is_empty() {
            let assoc = match self.shared.lock() {
                Ok(st) => st.assoc.clone(),
                Err(_) => Vec::new(),
            };
            for w in assoc {
                overlay_comments.push("联想".into());
                overlay.push(w);
            }
        }
        // 黑名单：删掉的错词不再展示；顺带记下显示下标 → Rime 下标的映射
        let mut cand_map: Vec<usize> = Vec::with_capacity(candidates.len());
        let (disp_candidates, disp_comments) = {
            let mut c = overlay.clone();
            let mut m = overlay_comments;
            for (i, (t, cm)) in candidates.into_iter().zip(comments.into_iter()).enumerate() {
                // 短语已经是叠加候选了，Rime 自己也认出它时别重复显示
                if overlay.iter().any(|o| o == &t) {
                    continue;
                }
                if xunyin_rime::blocklist::is_blocked(&t) {
                    crate::tsf_log!("keydown: 1 个候选已被删词过滤");
                    continue;
                }
                cand_map.push(i);
                c.push(t);
                m.push(cm);
            }
            (c, m)
        };
        let disp_selected = if overlay.is_empty() { selected } else { 0 };
        if let Ok(mut st) = self.shared.lock() {
            st.overlay = overlay;
            st.cand_map = cand_map;
        }
        self.update_candidates(disp_candidates, disp_comments, disp_selected, page, page_size);
        if log_input() {
            crate::tsf_log!(
                "keydown: vk=0x{vk:02X} -> consumed, preedit={preedit:?} commit={commit:?} candidates={cand_count}"
            );
        } else {
            crate::tsf_log!(
                "keydown: 已消费，组合 {} 字，上屏 {} 字，候选 {cand_count} 个",
                preedit.chars().count(),
                commit.chars().count()
            );
        }
        true
    }

    /// 组合中方向键的显示空间移动：光标只停靠在字母之间，音节分隔符被跳过。
    /// 同步调用 Rime 的 set_caret_pos（原始输入坐标），保证退格/上屏一致。
    fn move_caret_skip_sep(
        &self,
        eng: &mut xunyin_rime::RimeEngine,
        context: &ITfContext,
        left: bool,
    ) -> bool {
        let Some(s) = eng.get_context() else {
            return true;
        };
        let sep = cached_cfg().input.separator();
        let p = build_preedit(&s.preedit, sep);
        let display = p.display;
        let map = p.to_raw;
        let caret = p.to_display[s.cursor_pos.min(s.preedit.len())];
        let bytes = display.as_bytes();
        let len = bytes.len();
        // 显示位置 i 是不是分隔符的起始字节（分隔符可空，长度可不为 1）
        let is_sep = |i: usize| !sep.is_empty() && bytes[i..].starts_with(sep.as_bytes());
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
        // 显示位置 → 原始光标：直接查映射表（分隔符长度与空格不同，不能再靠减数量）
        let raw = map.get(p).copied().unwrap_or(s.preedit.len());
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
///
/// "是否组合中"这类判断在服务接管输入时要看**宿主里的组合串**（服务每次按键都会
/// 把组合串同步回来），否则本进程里可能根本没有引擎。
fn test_key(shared: &Arc<Mutex<SharedState>>, vk: u32, lparam: usize) -> bool {
    if (lparam & 0x8000_0000) != 0 {
        return false;
    }
    let mask = key::modifier_mask();
    // 提前把服务客户端线程拉起来：这样"刚打开的应用里的第一次按键"也能尽快走服务
    crate::svc::poll();
    let service = crate::svc::ready();
    let composing = || {
        if service {
            shared
                .lock()
                .map(|st| st.composition.is_some())
                .unwrap_or(false)
        } else {
            engine::engine()
                .map(|e| e.lock().map(|mut e| e.is_composing()).unwrap_or(false))
                .unwrap_or(false)
        }
    };
    let backend = || service || engine::engine().is_some();
    // Ctrl+.：中英标点切换（本输入法消费）
    if mask & kRimeModifierControl != 0 && mask & kRimeModifierAlt == 0 && vk == 0xBE {
        return true;
    }
    // Ctrl+数字：删词。**只在组合中**接管——否则会把浏览器的 Ctrl+1..9
    // （切换标签页）抢走。
    if mask & kRimeModifierControl != 0
        && mask & kRimeModifierAlt == 0
        && (0x31..=0x39).contains(&vk)
        && cached_cfg().input.delete_key == "ctrl_digit"
    {
        return composing();
    }
    if mask & (kRimeModifierControl | kRimeModifierAlt) != 0 {
        return false;
    }
    let shift = key::key_state_down(0x10);
    // 裸 Shift：交给 Rime 处理（中英切换），松开时按需切换
    // 这里一律吞下：真正的处理在 OnKeyUp（服务或本进程），若两边都没准备好，
    // handle_key_down 会返回 false，按键照样透传给应用。
    if key::is_shift_key(vk) {
        return true;
    }
    // Shift+方向键/编辑键：也送达 Rime，让它知道“不是裸 Shift”，
    // 避免松开 Shift 时误切中英模式（文本选择等场景）。
    if shift && key::is_function_key(vk) {
        return backend();
    }
    if key::vk_to_char(vk, shift).is_some() || key::vk_to_punct(vk, shift).is_some() {
        return true;
    }
    if key::is_function_key(vk) {
        return composing();
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
            let cfg = xunyin_rime::config::load_config();
            let (ascii, full, punct) = match engine::engine() {
                Some(eng) => match eng.lock() {
                    Ok(e) => e
                        .get_status()
                        .map(|s| (s.is_ascii_mode, s.is_full_shape, cfg.input.ascii_punct))
                        .unwrap_or((
                            cfg.input.ascii_mode,
                            cfg.input.full_shape,
                            cfg.input.ascii_punct,
                        )),
                    Err(_) => (cfg.input.ascii_mode, cfg.input.full_shape, cfg.input.ascii_punct),
                },
                None => (cfg.input.ascii_mode, cfg.input.full_shape, cfg.input.ascii_punct),
            };
            // 状态栏总开关（设置 → 外观主题 → 状态栏）
            if cfg.appearance.show_status_bar {
                crate::status_bar::show_status_bar(self.shared.clone(), ascii, full, punct);
            } else {
                crate::status_bar::hide_status_bar();
            }
        }
        // 激活时就拉起服务客户端：服务先起来，用户开始打字时它已经在跑了
        crate::svc::poll();
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
        // 输入焦点变化：本线程获得前台焦点且迅音仍激活 → 显示状态栏；
        // 失去前台焦点 → 隐藏。避免后台进程残留的状态栏窗口盖住屏幕。
        crate::tsf_log!(
            "OnKeyFocus: foreground={}",
            fforeground.as_bool()
        );
        if fforeground.as_bool() && cached_cfg().appearance.show_status_bar {
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
        let r = test_key(&self.shared, wparam.0 as u32, lparam.0 as usize);
        if log_input() {
            crate::tsf_log!(
                "OnTestKeyDown: vk=0x{:02X} lparam=0x{:X} -> {}",
                wparam.0 as u32,
                lparam.0 as usize,
                r
            );
        }
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
            && (crate::svc::ready() || engine::engine().is_some());
        if log_input() {
            crate::tsf_log!("OnTestKeyUp: vk=0x{:02X} -> {}", wparam.0 as u32, r);
        }
        Ok(BOOL::from(r))
    }

    fn OnKeyDown(
        &self,
        pic: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let vk = wparam.0 as u32;
        if log_input() {
            crate::tsf_log!(
                "OnKeyDown: vk=0x{:02X} lparam=0x{:X}",
                vk,
                lparam.0 as usize
            );
        }
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
        if log_input() {
            crate::tsf_log!("OnKeyDown: vk=0x{vk:02X} -> handled={handled}");
        }
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
            // 中英切换键设为 Ctrl+空格/关闭时，裸 Shift 不再承担切换职责
            if bare && cached_cfg().input.toggle_key == "shift" {
                crate::tsf_log!("OnKeyUp: bare Shift");
                // 服务接管输入时，组合状态与中英开关都在服务里，必须让它来处理
                if crate::svc::ready() {
                    if let Some(r) = crate::svc::command("shift_up") {
                        if !r.commit.is_empty() {
                            if let Some(ctx) = pic.cloned() {
                                let _ = self.schedule_edit(&ctx, "", &r.commit, 0);
                            }
                            crate::candidate_window::hide_candidates();
                            crate::tsf_log!("OnKeyUp: Shift -> 上屏拼音 {} 字", r.commit.chars().count());
                        } else {
                            // 注意作用域：**必须先放掉 shared 的锁再刷状态栏**。
                            // refresh_status_bar 内部还要读 shared，持着锁调它 = 同一个线程
                            // 二次加锁 = 死锁 —— 表现是"按 Shift 切中英，宿主直接卡死"
                            // （explorer 卡死时整个任务栏都不响应，看起来像所有应用都死了）。
                            if let Ok(mut st) = self.shared.lock() {
                                st.ascii_mode = r.ascii;
                                st.full_shape = r.full;
                            }
                            crate::status_bar::refresh_status_bar(&self.shared);
                        }
                    }
                } else if let Some(eng) = engine::engine() {
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
                                crate::tsf_log!("OnKeyUp: Shift -> 上屏拼音 {} 字", raw.chars().count());
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
        if guid.is_null() || unsafe { *guid } != crate::GUID_XUNYIN_DISPLAY_ATTR {
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

    /// 顿号键判定：不带 Shift 才算顿号键；Shift+/ 必须留给问号（用户实测反馈的 bug）。
    #[test]
    fn dunhao_key_requires_no_shift() {
        assert!(is_dunhao_key(0xBF, false));
        assert!(is_dunhao_key(0xDC, false));
        assert!(!is_dunhao_key(0xBF, true), "Shift+/ 是问号，不能当顿号吃掉");
        assert!(!is_dunhao_key(0xDC, true), "Shift+\\ 是竖线");
        assert!(!is_dunhao_key(0xBE, false), "句号键不是顿号键");
    }

    #[test]
    fn format_preedit_logic() {
        // 音节空格换成 "'"（字节长度不变，光标偏移保持不变）
        assert_eq!(format_preedit("ni hao", 6, "'"), ("ni'hao".into(), 6));
        assert_eq!(format_preedit("ni hao", 0, "'"), ("ni'hao".into(), 0));
        // 光标在 "ni h|ao"（字节 4）：替换后仍指向同一位置
        assert_eq!(format_preedit("ni hao", 4, "'"), ("ni'hao".into(), 4));
        // 无空格不受影响
        assert_eq!(format_preedit("nihao", 3, "'"), ("nihao".into(), 3));
        // 空串
        assert_eq!(format_preedit("", 0, "'"), (String::new(), 0));
    }

    /// 分隔符可选：空串（不留分隔）与其它单字节分隔符下，光标都要落在正确位置。
    #[test]
    fn format_preedit_with_configurable_separator() {
        // 空格样式：显示仍是 "ni hao"，光标在 "ni |hao" 位置 3
        assert_eq!(format_preedit("ni hao", 3, " "), ("ni hao".into(), 3));
        // 连字符
        assert_eq!(format_preedit("ni hao", 4, "-"), ("ni-hao".into(), 4));
        // 逗号
        assert_eq!(format_preedit("xuan ting", 9, ","), ("xuan,ting".into(), 9));
        // 不留分隔：显示变短，光标必须跟着回退（原始 "ni |hao"=3 → 显示 "ni|hao"=2）
        assert_eq!(format_preedit("ni hao", 3, ""), ("nihao".into(), 2));
        assert_eq!(format_preedit("ni hao", 4, ""), ("nihao".into(), 3));
        assert_eq!(format_preedit("ni hao", 6, ""), ("nihao".into(), 5));
        // 映射表本身：显示位置 → 原始位置
        let p = build_preedit("ni hao", "'");
        assert_eq!(p.display, "ni'hao");
        assert_eq!(p.to_raw.len(), p.display.len() + 1);
        assert_eq!(p.to_raw[0], 0);
        assert_eq!(p.to_raw[2], 2); // 分隔符本身
        assert_eq!(p.to_raw[3], 3); // 'h'
        assert_eq!(*p.to_raw.last().unwrap(), 6);
        // 不留分隔时：原始位置之间被"吃掉"的空格不占显示位
        let q = build_preedit("ni hao", "");
        assert_eq!(q.display, "nihao");
        assert_eq!(q.to_raw, vec![0, 1, 3, 4, 5, 6]);
        assert_eq!(q.to_display, vec![0, 1, 2, 2, 3, 4, 5]);
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
