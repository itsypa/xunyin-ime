//! 输入逻辑：**这就是"输入法本体"，从 TSF DLL 搬过来**（第 2 阶段）。
//!
//! 搬过来的东西：按键判定（顿号直上屏、Ctrl+数字删词、Ctrl+. 中英标点、Shift 中英、
//! v 模式、快捷输入 rq/sj/xq、自定义标点、回车行为）、候选（叠加候选 + 黑名单过滤）、
//! 联想学习、引擎热重载。
//!
//! 留在 DLL 的只有 TSF 适配（把这里返回的组合串/上屏文本写进宿主应用）——所以改这里的
//! 逻辑**不用重开 Edge/ChatGPT**：DLL 只转发按键，逻辑在服务里随时可重启。

use xunyin_rime::RimeEngine;
use xunyin_rime::{assoc, blocklist, quick_input};

/// 一次按键的处理结果（DLL 照着做就行）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Reply {
    /// 是否消费了这次按键（false = 交给应用自己处理）
    pub consume: bool,
    /// 组合串（显示用，已按下发配置插入音节分隔符）；空 = 结束组合
    pub preedit: String,
    /// 组合内光标（preedit 的 UTF-16 位置）
    pub caret: usize,
    /// 需要上屏的文本
    pub commit: String,
    pub candidates: Vec<String>,
    pub comments: Vec<String>,
    pub selected: usize,
    pub page: usize,
    pub page_size: usize,
    /// 叠加候选（快捷输入/联想/v 模式）个数：点击下标 < 它表示点的不是 Rime 候选
    pub overlay_len: usize,
    /// 状态栏用：中英 / 全半角 / 中英标点
    pub ascii: bool,
    pub full: bool,
    pub punct: bool,
    /// 引擎是否可用
    pub engine_ok: bool,
}

/// 服务内的输入状态（对应 DLL 里那份 SharedState 的"逻辑部分"）。
pub struct Logic {
    engine: Option<RimeEngine>,
    /// 叠加候选文本（快捷输入 / 联想 / v 模式）
    overlay: Vec<String>,
    /// 叠加候选的注释
    overlay_comments: Vec<String>,
    /// 显示下标 → Rime 候选下标（黑名单过滤后要用）
    cand_map: Vec<usize>,
    /// v 模式缓冲（含开头的 v）
    v_mode: Option<String>,
    /// 上次上屏的文本与时间（联想学习）
    last_commit: Option<(String, std::time::Instant)>,
    /// 联想候选
    assoc_followers: Vec<String>,
}

impl Logic {
    pub fn new() -> Self {
        Logic {
            engine: None,
            overlay: Vec::new(),
            overlay_comments: Vec::new(),
            cand_map: Vec::new(),
            v_mode: None,
            last_commit: None,
            assoc_followers: Vec::new(),
        }
    }

    /// 引擎懒初始化（首次按键时）。
    fn engine(&mut self) -> Option<&mut RimeEngine> {
        if self.engine.is_none() {
            let (shared, user) = data_dirs();
            if std::fs::create_dir_all(&user).is_err() {
                return None;
            }
            match RimeEngine::init(
                &shared.to_string_lossy(),
                &user.to_string_lossy(),
                "xunyin-svc",
            ) {
                Ok(mut e) => {
                    if e.is_maintenance_mode() {
                        let _ = e.deploy_full();
                        let _ = e.wait_deploy(20_000);
                    }
                    apply_config(&mut e);
                    crate::log(format!("引擎就绪（shared={}）", shared.display()));
                    // 诊断：FFI 结构体尺寸与 rime.dll 是否一致。
                    // 不一致会导致 librime 越界写我们的结构体 → 堆损坏 → 下次分配时
                    // 0xC0000005（本轮崩溃的表现正是这样）。
                    crate::log(format!(
                        "ABI 自检：api_version={:?} api_data_size={:?} sizeof(RimeApi)={} sizeof(RimeContext)={} sizeof(RimeStatus)={}",
                        e.api_version(),
                        e.api_data_size(),
                        std::mem::size_of::<xunyin_rime::rime_ffi::RimeApi>(),
                        std::mem::size_of::<xunyin_rime::rime_ffi::RimeContext>(),
                        std::mem::size_of::<xunyin_rime::rime_ffi::RimeStatus>(),
                    ));
                    self.engine = Some(e);
                }
                Err(e) => {
                    crate::log(format!("引擎初始化失败：{e}"));
                    return None;
                }
            }
        }
        self.engine.as_mut()
    }

    /// 处理一次按键。`mask` 是 kRimeModifier* 位掩码，`shift` 是 Shift 是否按下。
    pub fn key(&mut self, vk: u32, mask: i32, shift: bool) -> Reply {
        crate::log(format!("step1: 收到按键 vk=0x{vk:02X}"));
        let cfg = xunyin_rime::config::load_config();
        crate::log("step2: 配置读取完成");
        let ctrl = mask & xunyin_rime::kRimeModifierControl != 0;
        let alt = mask & xunyin_rime::kRimeModifierAlt != 0;
        crate::log("step3: 交给引擎（可能触发初始化）");
        let mut out = Reply {
            engine_ok: self.engine().is_some(),
            page_size: cfg.input.page_size as usize,
            ascii: cfg.input.ascii_mode,
            full: cfg.input.full_shape,
            punct: cfg.input.ascii_punct,
            ..Default::default()
        };
        crate::log("step4: 引擎可用，开始判定");
        // 状态在**开头**读（此时还没调用过任何会改动会话的 API，实测稳定）；
        // 放在 process_key 之后读会让服务崩（见 after_key 的说明）。
        self.read_options(&mut out);
        // ── Ctrl 组合 ────────────────────────────────────────────────
        if ctrl && !alt {
            // Ctrl+.：中英标点
            if vk == 0xBE {
                let mut c = xunyin_rime::config::load_config();
                c.input.ascii_punct = !c.input.ascii_punct;
                let _ = xunyin_rime::config::save_config(&c);
                if let Some(e) = self.engine() {
                    e.refresh_options(vec![("ascii_punct".into(), c.input.ascii_punct)]);
                }
                out.consume = true;
                out.punct = c.input.ascii_punct;
                xunyin_rime::reload::mark(xunyin_rime::reload::REQUEST_RELOAD);
                return out;
            }
            // Ctrl+空格：中英切换（可选设置）
            if vk == 0x20 && cfg.input.toggle_key == "ctrl_space" {
                if let Some(e) = self.engine() {
                    let mode = e.get_option("ascii_mode");
                    e.set_option("ascii_mode", !mode);
                    out.ascii = !mode;
                }
                out.consume = true;
                return out;
            }
            // Ctrl+数字：删词（当前页第 N 个）
            if (0x31..=0x39).contains(&vk) && cfg.input.delete_key == "ctrl_digit" {
                let idx = (vk - 0x31) as usize;
                out.consume = true;
                self.delete_at(idx, &mut out);
                return out;
            }
            // 其它 Ctrl 组合交给应用
            return out;
        }
        if alt {
            return out;
        }
        // ── v 模式 ─────────────────────────────────────────────────
        if cfg.input.enable_quick_input {
            if let Some(done) = self.v_mode_key(vk, shift, &mut out) {
                out.consume = done;
                return out;
            }
        }
        // ── 叠加候选选择键（空格 / 1-9）─────────────────────────────
        if let Some(done) = self.overlay_select(vk, &mut out) {
            out.consume = done;
            return out;
        }
        // ── 顿号键（中文标点、不带 Shift）→ 直接上屏「、」─────────────
        let composing = self.engine().map(|e| e.is_composing()).unwrap_or(false);
        if is_dunhao_key(vk, shift) && !composing {
            // 不用 get_status（见文件内说明），改读开关
            let ascii_now = self
                .engine()
                .map(|e| e.get_option("ascii_mode"))
                .unwrap_or(false);
            if !ascii_now && !cfg.input.ascii_punct {
                out.consume = true;
                out.commit = "、".into();
                return out;
            }
        }
        // ── 走 Rime ────────────────────────────────────────────────
        crate::log("step5: 走 Rime process_key");
        let keycode = rime_keycode(vk, shift);
        let enter_behavior = cfg.input.enter_behavior.clone();
        let mut custom_commit: Option<String> = None;
        let consumed = if vk == 0x0D && enter_behavior != "commit" {
            if let Some(e) = self.engine() {
                if e.is_composing() {
                    e.clear_composition();
                }
            }
            self.cancel_overlay();
            match enter_behavior.as_str() {
                "clear" => true,
                _ => false, // newline：交给宿主换行
            }
        } else if let Some(ch) = punct_char(vk, shift) {
            // 自定义标点优先（中文标点模式下）
            let ascii_now = self
                .engine()
                .map(|e| e.get_option("ascii_mode"))
                .unwrap_or(false);
            let hit = if cfg.input.ascii_punct || ascii_now {
                None
            } else {
                cfg.input.punct_lookup(ch)
            };
            match hit {
                Some(text) => {
                    let mut t = String::new();
                    if let Some(e) = self.engine() {
                        if e.is_composing() {
                            let _ = e.commit_composition();
                            t.push_str(&e.get_commit_text());
                        }
                    }
                    t.push_str(&text);
                    custom_commit = Some(t);
                    true
                }
                None => self
                    .engine()
                    .map(|e| e.process_key(keycode, mask))
                    .unwrap_or(false),
            }
        } else {
            self.engine()
                .map(|e| e.process_key(keycode, mask))
                .unwrap_or(false)
        };
        crate::log(format!("step6: process_key 返回 consumed={consumed}"));
        out.consume = consumed;
        if !consumed {
            self.cancel_overlay();
            return out;
        }
        // ── 取出结果 ──────────────────────────────────────────────
        let (commit, snapshot) = {
            let e = match self.engine() {
                Some(e) => e,
                None => return out,
            };
            let commit = custom_commit
                .unwrap_or_else(|| e.get_commit_text())
                .clone();
            let snap = e.get_context();
            (commit, snap)
        };
        crate::log("step7: 快照取完，进入 after_key");
        self.after_key(commit, snapshot, &cfg, &mut out);
        crate::log("step8: 处理完成");
        out
    }

    /// 候选窗点击：`display_index` 是**显示列表**里的下标（服务负责换算）。
    pub fn select(&mut self, display_index: usize) -> Reply {
        let cfg = xunyin_rime::config::load_config();
        let mut out = Reply {
            consume: true,
            page_size: cfg.input.page_size as usize,
            ..Default::default()
        };
        if display_index < self.overlay.len() {
            let text = self.overlay[display_index].clone();
            if let Some(e) = self.engine() {
                e.clear_composition();
            }
            self.cancel_overlay();
            self.learn_and_set_assoc(&text, &cfg);
            out.commit = text;
            out.candidates = self.overlay.clone();
            out.comments = self.overlay_comments.clone();
            out.overlay_len = self.overlay.len();
            out.selected = 0;
            return out;
        }
        let disp = display_index - self.overlay.len();
        let rime_idx = self.cand_map.get(disp).copied().unwrap_or(disp);
        let mut commit = String::new();
        if let Some(e) = self.engine() {
            if e.select_candidate(rime_idx) {
                commit = e.get_commit_text();
            }
        }
        if !commit.is_empty() {
            self.cancel_overlay();
            self.learn_and_set_assoc(&commit, &cfg);
            out.commit = commit;
            out.candidates = self.overlay.clone();
            out.comments = self.overlay_comments.clone();
            out.overlay_len = self.overlay.len();
            out.selected = 0;
        } else {
            out.consume = false;
        }
        out
    }

    /// 上屏后：学习「上一词 → 本词」，并把联想候选摆进叠加区（DLL 直接显示）。
    fn learn_and_set_assoc(&mut self, commit: &str, cfg: &xunyin_rime::XunyinConfig) {
        if commit.is_empty() {
            return;
        }
        if cfg.input.enable_assoc {
            if let Some((p, t)) = self.last_commit.take() {
                if t.elapsed() < std::time::Duration::from_secs(5) {
                    assoc::learn_and_maybe_save(&p, commit);
                }
            }
            self.assoc_followers = assoc::followers(commit);
        } else {
            self.assoc_followers.clear();
        }
        self.last_commit = Some((commit.to_string(), std::time::Instant::now()));
        if cfg.input.enable_assoc {
            self.overlay = self.assoc_followers.clone();
            self.overlay_comments = vec!["联想".to_string(); self.overlay.len()];
        } else {
            self.cancel_overlay();
        }
        self.cand_map.clear();
    }

    /// 状态查询（状态栏 / 新会话初始状态用）。
    pub fn status(&mut self) -> Reply {
        let cfg = xunyin_rime::config::load_config();
        let mut out = Reply {
            page_size: cfg.input.page_size as usize,
            ascii: cfg.input.ascii_mode,
            full: cfg.input.full_shape,
            punct: cfg.input.ascii_punct,
            engine_ok: self.engine().is_some(),
            ..Default::default()
        };
        self.read_options(&mut out);
        out
    }

    /// 命令（状态栏菜单等）：`toggle_mode` / `toggle_width` / `toggle_punct` /
    /// `reload` / `redeploy` / `select_schema:<id>`
    pub fn command(&mut self, name: &str) -> Reply {
        let mut cfg = xunyin_rime::config::load_config();
        match name {
            "toggle_mode" => {
                if let Some(e) = self.engine() {
                    let m = e.get_option("ascii_mode");
                    e.set_option("ascii_mode", !m);
                }
            }
            "toggle_width" => {
                cfg.input.full_shape = !cfg.input.full_shape;
                let _ = xunyin_rime::config::save_config(&cfg);
                if let Some(e) = self.engine() {
                    e.refresh_options(vec![("full_shape".into(), cfg.input.full_shape)]);
                }
            }
            "toggle_punct" => {
                cfg.input.ascii_punct = !cfg.input.ascii_punct;
                let _ = xunyin_rime::config::save_config(&cfg);
                if let Some(e) = self.engine() {
                    e.refresh_options(vec![("ascii_punct".into(), cfg.input.ascii_punct)]);
                }
            }
            "reload" => self.reload(false),
            "redeploy" => self.reload(true),
            other => {
                if let Some(id) = other.strip_prefix("select_schema:") {
                    if let Some(e) = self.engine() {
                        let _ = e.select_schema(id);
                    }
                    cfg.input.schema = id.to_string();
                    let _ = xunyin_rime::config::save_config(&cfg);
                } else {
                    crate::log(format!("未知命令 {other}"));
                }
            }
        }
        self.status()
    }

    /// 引擎热重载（配置/词库/方案变了时由 reload 标记触发，或状态栏菜单点）。
    pub fn reload(&mut self, with_deploy: bool) {
        let agent = |m: &str| crate::log(m.to_string());
        let _ = agent;
        let (shared, user) = data_dirs();
        let page_size = xunyin_rime::config::load_config().input.page_size;
        match xunyin_rime::schema_list::sync_default_patch(&shared, &user, page_size) {
            Ok(true) => crate::log("已把新装的方案补进 scheme 清单"),
            _ => {}
        }
        if let Some(e) = self.engine.as_mut() {
            if with_deploy {
                let _ = e.deploy_full();
                let _ = e.wait_deploy(180_000);
            }
            e.finalize();
            self.engine = None;
        }
        let _ = self.engine(); // 重建
        crate::log(format!("引擎已重载（with_deploy={with_deploy}）"));
    }

    // ---------------- 内部 ----------------

    fn cancel_overlay(&mut self) {
        self.overlay.clear();
        self.overlay_comments.clear();
    }

    /// 删词：先找到显示位置对应的词，再调 librime + 记黑名单。
    fn delete_at(&mut self, index: usize, out: &mut Reply) {
        let victim = if index < self.overlay.len() {
            Some(self.overlay[index].clone())
        } else {
            let disp = index - self.overlay.len();
            let rime_idx = self.cand_map.get(disp).copied().unwrap_or(disp);
            self.engine()
                .and_then(|e| e.get_context())
                .and_then(|c| c.candidates.get(rime_idx).map(|x| x.text.clone()))
        };
        if let Some(w) = victim {
            let disp = index.saturating_sub(self.overlay.len());
            if let Some(e) = self.engine() {
                let _ = e.delete_candidate_on_current_page(disp);
            }
            blocklist::block(&w);
            crate::log(format!("删词：{w}"));
        }
        // 刷新候选
        let cfg = xunyin_rime::config::load_config();
        let snap = self.engine().and_then(|e| e.get_context());
        self.after_key(String::new(), snap, &cfg, out);
    }

    /// 上屏之后：联想学习 + 准备联想候选，并汇总组合串/候选返回给 DLL。
    fn after_key(
        &mut self,
        commit: String,
        snapshot: Option<xunyin_rime::RimeContextSnapshot>,
        cfg: &xunyin_rime::XunyinConfig,
        out: &mut Reply,
    ) {
        // 联想：学习「上一词 → 本词」，并取出本词之后的联想候选
        if !commit.is_empty() {
            if cfg.input.enable_assoc {
                if let Some((p, t)) = self.last_commit.take() {
                    if t.elapsed() < std::time::Duration::from_secs(5) {
                        assoc::learn_and_maybe_save(&p, &commit);
                    }
                }
                self.assoc_followers = assoc::followers(&commit);
            } else {
                self.assoc_followers.clear();
            }
            self.last_commit = Some((commit.clone(), std::time::Instant::now()));
        }
        // 组合串（按设置插音节分隔符）
        let sep = cfg.input.separator();
        crate::log("step7a: separator 取到");
        let (preedit, caret) = snapshot
            .as_ref()
            .map(|s| format_preedit(&s.preedit, s.cursor_pos.min(s.preedit.len()), sep))
            .unwrap_or_default();
        crate::log(format!("step7b: 组合串={preedit:?} caret={caret}"));
        // 叠加候选：快捷输入（rq/sj/xq）→ 联想 → Rime 候选
        let mut overlay: Vec<String> = Vec::new();
        let mut overlay_comments: Vec<String> = Vec::new();
        if cfg.input.enable_quick_input {
            if let Some(t) = snapshot
                .as_ref()
                .and_then(|s| quick_input::trigger(&s.preedit))
            {
                for (text, comment) in quick_input::date_candidates(t, &quick_input::local_now()) {
                    overlay.push(text);
                    overlay_comments.push(comment);
                }
            }
        }
        if overlay.is_empty() && cfg.input.enable_assoc && preedit.is_empty() {
            for w in self.assoc_followers.clone() {
                overlay_comments.push("联想".into());
                overlay.push(w);
            }
        }
        crate::log(format!("step7c: 叠加候选 {} 个", overlay.len()));
        // Rime 候选 + 黑名单过滤 + 显示下标 → Rime 下标映射
        let (cands, comments, selected, page, page_size) = match snapshot.as_ref() {
            Some(s) => (
                s.candidates.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
                s.candidates.iter().map(|c| c.comment.clone()).collect::<Vec<_>>(),
                s.highlighted.max(0) as usize,
                s.page_no.max(0) as usize,
                s.page_size.max(0) as usize,
            ),
            None => (Vec::new(), Vec::new(), 0, 0, 0),
        };
        let mut cand_map: Vec<usize> = Vec::new();
        let mut f_c: Vec<String> = Vec::new();
        let mut f_m: Vec<String> = Vec::new();
        for (i, (t, m)) in cands.into_iter().zip(comments.into_iter()).enumerate() {
            if blocklist::is_blocked(&t) {
                continue;
            }
            cand_map.push(i);
            f_c.push(t);
            f_m.push(m);
        }
        crate::log(format!("step7d: 候选过滤后 {} 个", f_c.len()));
        out.preedit = preedit;
        out.caret = caret;
        out.commit = commit;
        out.overlay_len = overlay.len();
        out.comments = overlay_comments.into_iter().chain(f_m).collect();
        out.candidates = overlay.iter().cloned().chain(f_c).collect();
        out.selected = if overlay.is_empty() { selected } else { 0 };
        out.page = page;
        if page_size > 0 {
            out.page_size = page_size;
        }
        self.overlay = overlay;
        self.cand_map = cand_map;
        // 注意：这里不读引擎状态——实测"紧跟 process_key / 首次 process_key 之后"
        // 调用 get_status/get_option 会让服务进程 0xC0000005（DLL 里同样顺序没事）。
        // 状态改在**每次按键的最开头**读（那时还没碰过引擎），等价且稳定。
        crate::log("step7e: 状态取完");
        out.engine_ok = self.engine.is_some();
    }

    /// 从引擎读三个开关（中英 / 全半角 / 中英标点）。
    /// **只在按键开头调用**（见 after_key 里的说明：紧跟 process_key 之后调用会崩）。
    fn read_options(&mut self, out: &mut Reply) {
        if let Some(e) = self.engine() {
            out.ascii = e.get_option("ascii_mode");
            out.full = e.get_option("full_shape");
            out.punct = e.get_option("ascii_punct");
        }
    }

    /// v 模式（`v123` / `v1+2*3`）：Rime 的 speller 不收数字，这里自己维护组合串。
    /// 返回 Some(是否消费) —— None 表示没进 v 模式，继续走正常流程。
    fn v_mode_key(&mut self, vk: u32, shift: bool, out: &mut Reply) -> Option<bool> {
        if let Some(buf) = self.v_mode.clone() {
            if vk == 0x1B {
                self.v_mode = None;
                self.cancel_overlay();
                if let Some(e) = self.engine() {
                    e.clear_composition();
                }
                out.preedit.clear();
                return Some(true);
            }
            if vk == 0x08 {
                let mut chars: Vec<char> = buf.chars().collect();
                if chars.len() <= 1 {
                    self.v_mode = None;
                    self.cancel_overlay();
                    if let Some(e) = self.engine() {
                        e.clear_composition();
                    }
                    out.preedit.clear();
                } else {
                    chars.pop();
                    self.v_mode_update(&chars.into_iter().collect::<String>(), out);
                }
                return Some(true);
            }
            if vk == 0x20 || vk == 0x0D {
                let cands = quick_input::v_mode_candidates(&buf);
                let text = cands
                    .first()
                    .map(|(t, _)| t.clone())
                    .unwrap_or_else(|| buf.clone());
                self.v_mode = None;
                self.cancel_overlay();
                if let Some(e) = self.engine() {
                    e.clear_composition();
                }
                out.commit = text;
                out.preedit.clear();
                return Some(true);
            }
            if let Some(ch) = v_mode_char(vk, shift) {
                self.v_mode_update(&format!("{buf}{ch}"), out);
                return Some(true);
            }
            // 其它键：退出 v 模式，按键继续走正常流程
            self.v_mode = None;
            self.cancel_overlay();
            if let Some(e) = self.engine() {
                e.clear_composition();
            }
            out.preedit.clear();
            return None;
        }
        // 进入：Rime 组合里恰好是 "v"，紧接着按数字/运算符
        let ch = v_mode_char(vk, shift)?;
        let only_v = self
            .engine()
            .and_then(|e| e.get_context())
            .map(|c| c.preedit.eq_ignore_ascii_case("v"))
            .unwrap_or(false);
        if !only_v {
            return None;
        }
        self.v_mode_update(&format!("v{ch}"), out);
        Some(true)
    }

    fn v_mode_update(&mut self, buf: &str, out: &mut Reply) {
        self.v_mode = Some(buf.to_string());
        let cands = quick_input::v_mode_candidates(buf);
        let (items, comments): (Vec<String>, Vec<String>) = cands.into_iter().unzip();
        self.overlay = items.clone();
        self.overlay_comments = comments.clone();
        self.cand_map.clear();
        // v 模式的组合串就是缓冲区本身（Rime 里没有对应输入）
        if let Some(e) = self.engine() {
            e.clear_composition();
        }
        out.preedit = buf.to_string();
        out.caret = buf.encode_utf16().count();
        out.overlay_len = items.len();
        out.candidates = items;
        out.comments = comments;
        out.selected = 0;
    }

    /// 叠加候选的选择键（空格 = 第一项，1-9 = 第 N 项）。返回 Some 表示已处理。
    fn overlay_select(&mut self, vk: u32, out: &mut Reply) -> Option<bool> {
        if vk != 0x20 && !(0x31..=0x39).contains(&vk) {
            return None;
        }
        if self.overlay.is_empty() {
            return None;
        }
        let idx = if vk == 0x20 {
            0
        } else {
            (vk - 0x31) as usize
        };
        let r = self.select(idx);
        if !r.consume || r.commit.is_empty() {
            return None;
        }
        out.commit = r.commit;
        out.candidates.clear();
        out.overlay_len = 0;
        out.preedit.clear();
        Some(true)
    }
}

/// 顿号键：`/`(VK_OEM_2) 与 `\`(VK_OEM_5)，**不带 Shift** 才算（Shift+/ 是问号）。
fn is_dunhao_key(vk: u32, shift: bool) -> bool {
    !shift && matches!(vk, 0xBF | 0xDC)
}

/// v 模式可接受的字符（数字与四则运算符，含小键盘）。
fn v_mode_char(vk: u32, shift: bool) -> Option<char> {
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
        0x60..=0x69 => Some((b'0' + (vk - 0x60) as u8) as char),
        0x6A => Some('*'),
        0x6B => Some('+'),
        0x6D => Some('-'),
        0x6E => Some('.'),
        0x6F => Some('/'),
        _ => None,
    }
}

/// 标点键 → 字符（用于查自定义标点表）。
fn punct_char(vk: u32, shift: bool) -> Option<char> {
    let c = match vk {
        0xBA => if shift { ':' } else { ';' },
        0xBB => if shift { '+' } else { '=' },
        0xBC => if shift { '<' } else { ',' },
        0xBD => if shift { '_' } else { '-' },
        0xBE => if shift { '>' } else { '.' },
        0xBF => if shift { '?' } else { '/' },
        0xC0 => if shift { '~' } else { '`' },
        0xDB => if shift { '{' } else { '[' },
        0xDC => if shift { '|' } else { '\\' },
        0xDD => if shift { '}' } else { ']' },
        0xDE => if shift { '"' } else { '\'' },
        0x32 if shift => '@',
        0x33 if shift => '#',
        0x34 if shift => '$',
        0x35 if shift => '%',
        0x36 if shift => '^',
        0x37 if shift => '&',
        0x38 if shift => '*',
        0x39 if shift => '(',
        0x30 if shift => ')',
        0x31 if shift => '!',
        _ => return None,
    };
    Some(c)
}

/// Rime 用的 keycode（字母小写/大写、数字、功能键原样）。
fn rime_keycode(vk: u32, shift: bool) -> i32 {
    match vk {
        0x41..=0x5A => {
            if shift {
                vk as i32
            } else {
                (vk + 32) as i32
            }
        }
        0x30..=0x39 => vk as i32,
        _ => match vk {
            0x20 => 0x20,
            0x0D => 0x0D,
            0x08 => 0xFF08,
            0x1B => 0xFF1B,
            0x25 => 0xFF51,
            0x26 => 0xFF52,
            0x27 => 0xFF53,
            0x28 => 0xFF54,
            0x21 => 0xFF55,
            0x22 => 0xFF56,
            0x24 => 0xFF50,
            0x23 => 0xFF57,
            0x09 => 0xFF09,
            _ => vk as i32,
        },
    }
}

/// 组合串：把 Rime 的音节分隔空格换成配置的分隔符，并给出光标位置（UTF-16）。
fn format_preedit(text: &str, cursor_byte: usize, sep: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len() + 8);
    let mut caret = 0usize;
    for (i, ch) in text.char_indices() {
        if i >= cursor_byte {
            caret = out.encode_utf16().count();
        }
        if ch == ' ' {
            out.push_str(sep);
        } else {
            out.push(ch);
        }
    }
    if cursor_byte >= text.len() {
        caret = out.encode_utf16().count();
    }
    (out, caret)
}

/// 引擎数据目录（与 DLL、设置中心一致）。
fn data_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    let user = xunyin_rime::user_data_dir();
    if let Ok(p) = std::env::var("XUNYIN_SHARED_DATA") {
        return (std::path::PathBuf::from(p), user);
    }
    // 服务 exe 与 DLL 同目录（安装布局 <安装目录>\tsf\），逐级向上找 rime\data
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let mut cur = Some(dir.to_path_buf());
            for _ in 0..5 {
                let Some(d) = cur else { break };
                let cand = d.join("rime").join("data");
                if cand.join("default.yaml").exists() {
                    return (cand, user);
                }
                let dev = d.join("xunyin-ime").join("rime").join("data");
                if dev.join("default.yaml").exists() {
                    return (dev, user);
                }
                cur = d.parent().map(|p| p.to_path_buf());
            }
        }
    }
    (std::path::PathBuf::from("rime").join("data"), user)
}

/// 应用共享配置到引擎（默认方案 + 输入选项）。
fn apply_config(eng: &mut RimeEngine) {
    let cfg = xunyin_rime::config::load_config();
    let schema = std::env::var("XUNYIN_DEFAULT_SCHEMA").unwrap_or_else(|_| cfg.input.schema.clone());
    eng.set_default_schema(&schema);
    eng.set_default_options(vec![
        ("full_shape".into(), cfg.input.full_shape),
        ("ascii_punct".into(), cfg.input.ascii_punct),
        ("ascii_mode".into(), cfg.input.ascii_mode),
        ("simplification".into(), true),
    ]);
}
