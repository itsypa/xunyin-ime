//! 诊断：用运行时数据目录（%APPDATA%\com.lingxi.ime）初始化引擎，
//! 输入 nihao 打印 preedit 与候选列表，对照测试环境差异。

use lingxi_rime::RimeEngine;

fn main() {
    let shared = match std::env::var("PROBE_SHARED_DIR") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("..")
            .join("lingxi-ime")
            .join("rime")
            .join("data"),
    };
    let user = match std::env::var("PROBE_USER_DIR") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("com.lingxi.ime"),
    };
    println!("shared={} user={}", shared.display(), user.display());

    // 可选：写入用户目录 default.custom.yaml 的每页候选数，验证部署后新会话生效
    if let Ok(ps) = std::env::var("PROBE_PAGE_SIZE") {
        if let Ok(n) = ps.trim().parse::<u32>() {
            std::fs::create_dir_all(&user).ok();
            std::fs::write(
                user.join("default.custom.yaml"),
                format!("patch:\n  menu/page_size: {}\n", n.clamp(1, 20)),
            )
            .ok();
            println!("PROBE_PAGE_SIZE={n} -> wrote default.custom.yaml");
        }
    }

    let mut engine =
        RimeEngine::init(&shared.to_string_lossy(), &user.to_string_lossy(), "runtime-probe")
            .expect("engine init failed");
    // 可选：只重建指定方案（用于预编译附加词典，如 emoji）
    if let Ok(schema) = std::env::var("PROBE_DEPLOY_SCHEMA") {
        let ok = engine.deploy_schema(&schema);
        println!("deploy_schema({schema}) -> {ok}");
        engine.finalize();
        return;
    }
    // 强制部署（首次构建 lingxi_rich 词库索引，可能耗时数分钟）
    eprintln!("deploying...");
    let _ = engine.deploy();
    let build_done = || {
        !engine.is_maintenance_mode()
            && user.join("build").join("default.yaml").exists()
    };
    for i in 0..2400 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if build_done() {
            break;
        }
        if i % 30 == 0 {
            eprintln!("deploy waiting... {}s", i + 1);
        }
    }
    println!("maintenance_mode={}", engine.is_maintenance_mode());
    println!(
        "api_version={:?} api_data_size={} (expected RimeApi struct size {})",
        engine.api_version(),
        engine.api_data_size(),
        std::mem::size_of::<lingxi_rime::rime_ffi::RimeApi>()
    );
    let schemas = engine.get_schema_list();
    println!("schemas={:?}", schemas.iter().map(|(a, b)| format!("{a}/{b}")).collect::<Vec<_>>());

    for input in ["nihao", "ni", "you"] {
        let id = engine.ensure_session();
        if id == 0 {
            println!("{input}: no session");
            continue;
        }
        for ch in input.chars() {
            engine.process_key(ch as i32, 0);
        }
        if let Some(ctx) = engine.get_context() {
            println!(
                "{input}: preedit={:?} cursor_pos={} page_size={} page_no={} candidates={:?}",
                ctx.preedit,
                ctx.cursor_pos,
                ctx.page_size,
                ctx.page_no,
                ctx.candidates
                    .iter()
                    .map(|c| c.text.clone())
                    .collect::<Vec<_>>()
            );
        } else {
            println!("{input}: no context");
        }
        engine.clear_composition();
    }

    // 方向键移动光标验证：输入 nihao 后按两次 KP_Left（逐字符），
    // 应看到 cursor_pos 每次只前移 1 个字符。
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        if let Some(ctx) = engine.get_context() {
            println!(
                "arrow test: preedit={:?} cursor_pos={} (before)",
                ctx.preedit, ctx.cursor_pos
            );
        }
        engine.process_key(0xFF96, 0); // KP_Left（逐字符）
        engine.process_key(0xFF96, 0); // KP_Left（逐字符）
        if let Some(ctx) = engine.get_context() {
            println!(
                "arrow test: preedit={:?} cursor_pos={} (after 2xKP_Left)",
                ctx.preedit, ctx.cursor_pos
            );
        }
        engine.process_key(0xFF08, 0); // Backspace 删除光标前字符
        if let Some(ctx) = engine.get_context() {
            println!(
                "arrow test: preedit={:?} cursor_pos={} (after Backspace)",
                ctx.preedit, ctx.cursor_pos
            );
        }
        engine.clear_composition();
    }

    // 方向键边界验证：按 10 次 KP_Left 应停在 0（不循环到末尾）
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        for _ in 0..10 {
            engine.process_key(0xFF96, 0); // KP_Left
        }
        let left_pos = engine
            .get_context()
            .map(|c| c.cursor_pos)
            .unwrap_or(999);
        for _ in 0..10 {
            engine.process_key(0xFF98, 0); // KP_Right
        }
        let right_pos = engine
            .get_context()
            .map(|c| c.cursor_pos)
            .unwrap_or(999);
        println!(
            "boundary test: after 10xLeft pos={left_pos} (expect 0), after 10xRight pos={right_pos} (expect 6)"
        );
        engine.clear_composition();
    }

    // set_caret_pos 映射验证：原始坐标 → 显示光标
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        for raw in [0usize, 1, 2, 3, 4, 5] {
            engine.set_caret_pos(raw);
            let pos = engine
                .get_context()
                .map(|c| c.cursor_pos)
                .unwrap_or(999);
            println!("caretmap: raw={raw} -> display_pos={pos}");
        }
        engine.clear_composition();
    }

    // Shift 切换中英验证：组合中按 Shift（commit_text 样式）松开后
    // ascii_mode 应打开，且当前组合被提交。
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        let before = engine
            .get_status()
            .map(|s| s.is_ascii_mode)
            .unwrap_or(false);
        engine.process_key(0xFFE1, 1); // Shift_L down（含 Shift 修饰）
        engine.process_key(0xFFE1, 1 | (1 << 30)); // Shift_L up（释放位）
        let shift_commit = engine.get_commit_text();
        let after = engine
            .get_status()
            .map(|s| s.is_ascii_mode)
            .unwrap_or(false);
        println!(
            "shift test: ascii_mode {before} -> {after} shift_commit={shift_commit:?}"
        );
        if shift_commit.is_empty() {
            engine.clear_composition();
        }
        if after {
            let mut out = String::new();
            for ch in "abc".chars() {
                let consumed = engine.process_key(ch as i32, 0);
                out.push_str(&engine.get_commit_text());
                let ctx = engine.get_context();
                println!(
                    "shift test: key={ch} consumed={consumed} commit_now={out:?} ctx_preedit={:?} ctx_cands={}",
                    ctx.as_ref().map(|c| c.preedit.clone()),
                    ctx.map(|c| c.candidates.len()).unwrap_or(0)
                );
            }
            println!("shift test: typing abc -> commits={out:?}");
        }
        // 再按一次 Shift（未组合）应切回中文
        engine.process_key(0xFFE1, 1);
        engine.process_key(0xFFE1, 1 | (1 << 30));
        let after2 = engine
            .get_status()
            .map(|s| s.is_ascii_mode)
            .unwrap_or(false);
        println!("shift test: ascii_mode {after} -> {after2} (toggle back)");
    }

    let _id = engine.ensure_session();
    println!("current_schema={:?}", engine.current_schema());

    // 英文模式验证：ascii_mode=true 时输入 hello 每个字母应透传（不产生组合/空格）
    let id = engine.ensure_session();
    if id != 0 {
        engine.set_option("ascii_mode", true);
        engine.clear_composition();
        let mut commits = Vec::new();
        for ch in "hello".chars() {
            let consumed = engine.process_key(ch as i32, 0);
            commits.push(engine.get_commit_text());
            let ctx = engine.get_context();
            println!(
                "ascii test: key={ch} consumed={consumed} preedit={:?} cands={} commit_now={:?}",
                ctx.as_ref().map(|c| c.preedit.clone()),
                ctx.map(|c| c.candidates.len()).unwrap_or(0),
                commits.last()
            );
        }
        println!("ascii test: commits={commits:?}");
        engine.set_option("ascii_mode", false);
        engine.clear_composition();
        // 中文模式下输入 hello：看组合串/候选
        for ch in "hello".chars() {
            engine.process_key(ch as i32, 0);
        }
        let ctx = engine.get_context();
        println!(
            "cn-mode hello: preedit={:?} candidates={:?}",
            ctx.as_ref().map(|c| c.preedit.clone()),
            ctx.map(|c| c.candidates.iter().map(|x| x.text.clone()).collect::<Vec<_>>())
        );
        engine.clear_composition();
    }

    // Emoji 词库验证：haha → 😄、zan → 👍、ku → 😭
    let id = engine.ensure_session();
    if id != 0 {
        for code in ["haha", "zan", "ku"] {
            engine.clear_composition();
            for ch in code.chars() {
                engine.process_key(ch as i32, 0);
            }
            let ctx = engine.get_context();
            println!(
                "emoji test: {code} -> {:?}",
                ctx.map(|c| c
                    .candidates
                    .iter()
                    .map(|x| x.text.clone())
                    .collect::<Vec<_>>())
            );
        }
        engine.clear_composition();
    }

    // 英文词库验证：hello → hello 等英文候选
    let id = engine.ensure_session();
    if id != 0 {
        engine.clear_composition();
        for ch in "hello".chars() {
            engine.process_key(ch as i32, 0);
        }
        let ctx = engine.get_context();
        println!(
            "eng test: hello -> {:?}",
            ctx.map(|c| c
                .candidates
                .iter()
                .map(|x| x.text.clone())
                .collect::<Vec<_>>())
        );
        engine.clear_composition();
    }

    // 空格选词验证：输入 nihao 后按空格应上屏候选词「你好」
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        let consumed = engine.process_key(0x20, 0); // Space
        let commit = engine.get_commit_text();
        println!("space test: consumed={consumed} commit={commit:?} (expect 你好)");
        engine.clear_composition();
    }

    // 回车行为验证：输入 nihao 后按 Enter / commit_composition / select_candidate
    let id = engine.ensure_session();
    if id != 0 {
        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        let ctx = engine.get_context();
        println!(
            "enter test: preedit={:?} highlighted={:?}",
            ctx.as_ref().map(|c| c.preedit.clone()),
            ctx.as_ref().map(|c| c.highlighted)
        );
        let consumed = engine.process_key(0xFF0D, 0); // Enter
        let commit = engine.get_commit_text();
        println!("enter test: via Enter consumed={consumed} commit={commit:?}");
        engine.clear_composition();

        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        let ok = engine.commit_composition();
        let commit = engine.get_commit_text();
        println!("enter test: via commit_composition ok={ok} commit={commit:?}");
        engine.clear_composition();

        for ch in "nihao".chars() {
            engine.process_key(ch as i32, 0);
        }
        let ok = engine.select_candidate(0);
        let commit = engine.get_commit_text();
        println!("enter test: via select_candidate(0) ok={ok} commit={commit:?}");
        engine.clear_composition();
    }

    // 标点行为验证：半角/全角模式下输入 , 和 / 是否直接上屏
    let id = engine.ensure_session();
    if id != 0 {
        for (label, full) in [("half_shape", false), ("full_shape", true)] {
            engine.set_option("full_shape", full);
            for ch in [',', '/', '.'] {
                engine.clear_composition();
                let consumed = engine.process_key(ch as i32, 0);
                let commit = engine.get_commit_text();
                let ctx = engine.get_context();
                println!(
                    "punct test: {label} key={ch} consumed={consumed} commit={commit:?} preedit={:?} candidates={}",
                    ctx.as_ref().map(|c| c.preedit.clone()),
                    ctx.map(|c| c.candidates.len()).unwrap_or(0)
                );
            }
        }
        engine.set_option("full_shape", false);
        engine.clear_composition();
    }

    // 对比 luna_pinyin 方案的标点行为
    engine.select_schema("luna_pinyin");
    let id = engine.ensure_session();
    if id != 0 {
        engine.clear_composition();
        let consumed = engine.process_key(',' as i32, 0);
        let commit = engine.get_commit_text();
        let ctx = engine.get_context();
        println!(
            "punct luna_pinyin: key=, consumed={consumed} commit={commit:?} preedit={:?} candidates={}",
            ctx.as_ref().map(|c| c.preedit.clone()),
            ctx.map(|c| c.candidates.len()).unwrap_or(0)
        );
        engine.clear_composition();
    }
    engine.select_schema("lingxi_rich");
    engine.ensure_session();
    engine.finalize();
}
