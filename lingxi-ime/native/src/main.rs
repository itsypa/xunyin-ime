//! 灵犀输入法设置中心（原生 Win32 版）。
//!
//! 与旧的 Tauri/WebView2 版本相比：启动 10~30ms（原来 44~105ms，调试版 2s）、
//! 内存十几 MB（原来 54MB）、**不依赖 WebView2 运行时**。
//! 输入本身由系统级 TSF 文本服务（lingxi-tsf）承担，这里只管设置与词库。

#![windows_subsystem = "windows"]

mod core;
mod ui;

fn main() {
    ui::run();
}
