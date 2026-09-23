pub mod install_wizard;
pub mod theme;
pub mod uninstall_wizard;

use windui::platform::Renderer;

/// 向导窗口使用的渲染后端。
/// 默认 [`Renderer::Auto`]（D2D 硬件加速优先，设备建不起来自动回退软光栅）；
/// 以下任一条件强制软光栅：
///   - 环境变量 `WIND_SOFT_RENDER` 非空且不为 `"0"`
///   - 命令行包含 `--soft-render`
///
/// **不用 [`Renderer::Gpu`]**：那一档在拿不到 GPU 时报错终止，而安装器往往是用户在
/// 这台机器上运行的第一个程序——显卡驱动没装好就起不来，用户无从自救。这里要的
/// 恰恰是静默回退。
pub fn renderer() -> Renderer {
    let env_soft = std::env::var("WIND_SOFT_RENDER")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    let arg_soft = std::env::args().any(|a| a == "--soft-render");
    if env_soft || arg_soft {
        Renderer::Software
    } else {
        Renderer::Auto
    }
}
