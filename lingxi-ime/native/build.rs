//! 嵌入应用清单：启用 comctl32 v6 现代控件外观 + Per-Monitor V2 DPI 感知。
//! 没有清单的话，公共控件会退回 Windows 95 风格外观。

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    let _ = embed_resource::compile("app.rc", embed_resource::NONE);
}
