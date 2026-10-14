//! 服务进程协议常量（DLL 与 xunyin-svc 共用一份，避免两边版本号写歪）。
//!
//! 架构：TSF DLL 留在宿主进程里（in-process，代码换不掉），把"输入法本体"
//! （引擎、输入逻辑、界面）逐步搬到 `xunyin-svc.exe`。服务是普通进程，**可以随时
//! 重启**——DLL 发现服务版本与自己期望的不一致，就杀掉旧服务、拉起新 exe，
//! 宿主应用无感。这就是"改代码不用重开 Edge/ChatGPT"的机制。

/// 管道名（\\.\pipe\ 前缀由两端各自拼）。
pub const PIPE_NAME: &str = r"\\.\pipe\xunyin-ime-svc-v1";
/// 协议版本：消息格式不兼容时才改。
pub const PROTO: u32 = 1;
/// 期望的服务构建标记。**改服务代码后把它 + xunyin-svc 的 SVC_TAG 一起改**，
/// 运行中的 DLL 就会自动换用新服务（不用重开任何应用）。
pub const SVC_TAG: &str = "20261014-1";
