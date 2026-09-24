fn main() {
    // 链接 rime.lib(静态导入库,导出 rime_get_api 等符号)。
    // rime 预编译产物位于 lingxi-ime/rime(monorepo 布局)。
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // x86 目标使用 rime-x86（32 位 librime），x64 使用 rime（64 位 librime）。
    let is_x86 = std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .map(|w| w == "32")
        .unwrap_or(false);
    let rime_dir = manifest_dir
        .join("..")
        .join("lingxi-ime")
        .join(if is_x86 { "rime-x86" } else { "rime" });
    println!("cargo:rustc-link-search=native={}", rime_dir.display());
    println!("cargo:rustc-link-lib=dylib=rime");

    // 把 rime.dll 复制到本 crate 的输出目录（保证运行时能按 DLL 名加载）。
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
        std::fs::create_dir_all(dll_dst.parent().unwrap()).ok();
        let _ = std::fs::copy(&dll_src, &dll_dst);
        println!("cargo:rerun-if-changed={}", dll_src.display());
    }
    println!("cargo:rerun-if-changed={}", dll_dst.display());
}

// 强制重新链接：librime 升级后需要重新拷贝 rime.dll
// relink 20260925
