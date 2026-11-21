fn main() {
    // 链接 rime.lib（静态导入库，导出 rime_get_api 等符号）。
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // x86 目标使用 rime-x86（32 位 librime），x64 使用 rime（64 位 librime）。
    let is_x86 = std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .map(|w| w == "32")
        .unwrap_or(false);
    let rime_dir = manifest_dir
        .join("..")
        .join("xunyin-ime")
        .join(if is_x86 { "rime-x86" } else { "rime" });
    println!("cargo:rustc-link-search=native={}", rime_dir.display());
    println!("cargo:rustc-link-lib=dylib=rime");

    // 把 rime.dll 复制到本 DLL 旁边，保证运行时按 DLL 名加载。
    // OUT_DIR 形如 target[/{triple}]/{profile}/build/{pkg}/out，
    // 向上 3 级即目标 profile 目录（兼容原生与 --target 交叉构建）。
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let profile_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("OUT_DIR too shallow")
        .to_path_buf();
    let dll_src = rime_dir.join("rime.dll");
    let dll_dst = profile_dir.join("rime.dll");
    if dll_src.exists() {
        // 运行时 DLL 搜索只认 exe 所在目录 / PATH，不会去父目录找：
        //  - target/debug            -> xunyin_tsf.dll 及设置中心
        //  - target/debug/deps       -> 本地 cargo test 的测试 exe
        //  - OUT_DIR                 -> CI 上 cargo test 的测试 exe 所在目录
        // 三处都放一份，否则测试进程会以 STATUS_DLL_NOT_FOUND (0xc0000135) 退出。
        for dst_dir in [
            profile_dir.clone(),
            profile_dir.join("deps"),
            std::path::PathBuf::from(&out_dir),
        ] {
            std::fs::create_dir_all(&dst_dir).ok();
            let _ = std::fs::copy(&dll_src, dst_dir.join("rime.dll"));
        }
        println!("cargo:rerun-if-changed={}", dll_src.display());
    }
    println!("cargo:rerun-if-changed={}", dll_dst.display());
}

// 强制重新链接：librime 升级后需要重新拷贝 rime.dll
// relink 20260925
// relink after librime rollback
