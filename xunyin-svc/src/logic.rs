//! 输入逻辑：**这就是"输入法本体"，从 TSF DLL 搬过来**（第 2 阶段）。
//!
//! 搬过来的东西：按键判定（顿号直上屏、Ctrl+数字删词、Ctrl+. 中英标点、Shift 中英、
//! v 模式、快捷输入 rq/sj/xq、自定义标点、回车行为）、候选（叠加候选 + 黑名单过滤）、
//! 联想学习、引擎热重载。
//!
//! 留在 DLL 的只有 TSF 适配（把这里返回的组合串/上屏文本写进宿主应用）——所以改这里的
//! 逻辑**不用重开 Edge/ChatGPT**：DLL 只转发按键，逻辑在服务里随时可重启。

use xunyin_rime::keycode::{char_keycode, is_dunhao_key, vk_to_punct};
use xunyin_rime::preedit::{build as build_preedit, format as format_preedit, move_caret};
use xunyin_rime::RimeEngine;
use xunyin_rime::{assoc, blocklist, quick_input};

/// 一次按键的处理结果。类型定义在共享 crate 里（xunyin-rime::svc_proto::Reply），
/// DLL 反序列化用的是同一份，不会写歪。
pub use xunyin_rime::svc_proto::Reply;

/// 服务内的输入状态（对应 DLL 里那份 SharedState 的"逻辑部分"）。
pub struct Logic {
    engine: Option<RimeEngine>,
    /// 正在后台重建索引（分钟级）。这期间按键直接让宿主退回自己的逻辑，
    /// 不能让宿主在管道上等超时。
    redeploying: bool,
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
            redeploying: false,
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
                    let maint = e.is_maintenance_mode();
                    crate::log(format!("引擎 init 完成：维护模式={maint}"));
                    if maint {
                        let _ = e.deploy_full();
                        let done = e.wait_deploy(20_000);
                        crate::log(format!("首次部署结束={done}"));
                    }
                    apply_config(&mut e);
                    crate::log(format!("引擎就绪（shared={}）", shared.display()));
                    let (rb, rs) = crate::crashlog::module_base("rime.dll");
                    crate::log(format!(
                        "模块基址：rime.dll base=0x{rb:X} size=0x{rs:X}（崩溃日志里 rva= 开头的是本进程自身）"
                    ));
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
        if self.redeploying {
            // 索引在后台重建：不接按键，让宿主立刻退回它自己的逻辑（打字不卡）
            return Reply {
                consume: false,
                error: Some("引擎重建中".into()),
                ..Default::default()
            };
        }
        let cfg = xunyin_rime::config::load_config();
        let ctrl = mask & xunyin_rime::kRimeModifierControl != 0;
        let alt = mask & xunyin_rime::kRimeModifierAlt != 0;
        let mut out = Reply {
            engine_ok: self.engine().is_some(),
            page_size: cfg.input.page_size as usize,
            ascii: cfg.input.ascii_mode,
            full: cfg.input.full_shape,
            punct: cfg.input.ascii_punct,
            ..Default::default()
        };
        // 状态在按键开头读一次就够（中英/全半角/中英标点三个开关）。
        self.read_options(&mut out);
        if let Some(note) = xunyin_rime::rime_engine::abi_note() {
            crate::log(format!("⚠️ ABI：{note}"));
        }
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
        // ── 组合中方向键：逐个字母移动光标（跳过音节分隔符），两端停住 ──
        // 光标状态在引擎里（set_caret_pos），所以这一步必须在服务侧做，
        // 否则宿主 DLL 移动的光标与服务引擎里的组合状态会错位。
        if (vk == 0x25 || vk == 0x27)
            && self
                .engine()
                .map(|e| e.is_composing())
                .unwrap_or(false)
        {
            let sep = cfg.input.separator();
            let moved = {
                let Some(e) = self.engine() else { return out };
                let Some(s) = e.get_context() else {
                    return out;
                };
                let p = build_preedit(&s.preedit, sep);
                let caret = p
                    .to_display
                    .get(s.cursor_pos.min(s.preedit.len()))
                    .copied()
                    .unwrap_or(p.display.len());
                let new_caret = move_caret(&p.display, sep, caret, vk == 0x25);
                let raw = p.to_raw.get(new_caret).copied().unwrap_or(s.preedit.len());
                e.set_caret_pos(raw);
                new_caret
            };
            // 移动后重新取一次上下文：组合串与候选都照旧，只有光标变了
            let commit = String::new();
            let snapshot = self.engine().and_then(|e| e.get_context());
            self.after_key(commit, snapshot, &cfg, &mut out);
            out.consume = true;
            out.caret = moved;
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
        let keycode = char_keycode(vk, shift);
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
        } else if let Some(ch) = vk_to_punct(vk, shift) {
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
            (commit, e.get_context())
        };
        self.after_key(commit, snapshot, &cfg, &mut out);
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
        let mut commit = String::new();
        match name {
            // 裸 Shift 松开（对应设置里"中英切换键 = Shift"）：
            //   组合中 → 把拼音原样上屏，**不**切中英（用户：输入中按 Shift 直接出拼音）
            //   未组合 → 切换中英
            "shift_up" => {
                if let Some(e) = self.engine() {
                    if e.is_composing() {
                        let raw = e.get_input();
                        if !raw.is_empty() {
                            e.clear_composition();
                            self.cancel_overlay();
                            commit = raw;
                        }
                    } else {
                        let mode = e.get_option("ascii_mode");
                        e.set_option("ascii_mode", !mode);
                    }
                }
            }
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
            "reload" => self.reload(),
            // "redeploy"（重建索引）由 main.rs 特判走后台线程，不会到这里
            "redeploy" => self.reload(),
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
        let mut out = self.status();
        out.commit = commit;
        out.consume = true;
        out
    }

    /// 引擎热重载：收掉引擎再重建（配置/词库/方案变了时由 reload 标记触发）。
    /// **不重建索引**——那是分钟级操作，走 `begin_redeploy` + 后台线程。
    pub fn reload(&mut self) {
        if self.engine.is_none() {
            // 还没建过引擎（服务只是握手/心跳）→ 什么都不用做：
            // 下次真正建引擎时会读到最新的词库与配置。
            return;
        }
        sync_scheme_list();
        if let Some(e) = self.engine.as_mut() {
            e.finalize();
            self.engine = None;
        }
        let _ = self.engine(); // 重建
        crate::log("引擎已重载");
    }

    /// 重建索引第一步：把现有引擎收掉并打上"重建中"标记，返回部署所需的数据目录。
    /// **调用方拿到 job 后要在后台线程里跑 `deploy_redeploy`**，别在持锁时跑。
    pub fn begin_redeploy(&mut self) -> Option<(String, String)> {
        if self.redeploying {
            return None;
        }
        sync_scheme_list();
        if let Some(e) = self.engine.as_mut() {
            e.finalize();
            self.engine = None;
        }
        self.redeploying = true;
        let (shared, user) = data_dirs();
        Some((
            shared.to_string_lossy().into_owned(),
            user.to_string_lossy().into_owned(),
        ))
    }

    /// 重建索引第三步：部署结束后重建引擎、清标记。
    pub fn finish_redeploy(&mut self) {
        self.redeploying = false;
        let _ = self.engine(); // 重建（会读到刚建好的新索引）
        crate::log("重新部署结束，引擎已用新索引重建");
    }

    /// 重建索引第二步：在**不持锁**的后台线程里调用。
    pub fn deploy_redeploy(shared: &str, user: &str) -> bool {
        xunyin_rime::RimeEngine::deploy_only(shared, user)
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
            // 不记用户词（日志可能被打进诊断包）：只记长度与结果
            crate::log(format!("删词：命中 1 条（{} 字）", w.chars().count()));
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
        let (preedit, caret) = snapshot
            .as_ref()
            .map(|s| format_preedit(&s.preedit, s.cursor_pos.min(s.preedit.len()), sep))
            .unwrap_or_default();
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
        // 记下这一屏候选：候选窗点击/删词要用它把"显示下标"换算回 Rime 下标。
        self.overlay = overlay;
        self.cand_map = cand_map;
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

/// 把（新装的）方案补进 scheme 清单；重载/重建索引前都先跑一遍。
fn sync_scheme_list() {
    let (shared, user) = data_dirs();
    let page_size = xunyin_rime::config::load_config().input.page_size;
    if let Ok(true) = xunyin_rime::schema_list::sync_default_patch(&shared, &user, page_size) {
        crate::log("已把新装的方案补进 scheme 清单");
    }
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
