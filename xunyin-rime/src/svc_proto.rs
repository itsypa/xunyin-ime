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
pub const SVC_TAG: &str = "20261014-2";

/// 服务对一次操作的回答。**DLL 与 xunyin-svc 共用这一个定义**，避免两边字段写歪。
///
/// 语义：DLL 只负责"把宿主应用里的按键送进来、把结果写回宿主"，
/// 判定与状态全在服务里（这样改逻辑只要换 exe，宿主应用不用重启）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Reply {
    /// 是否消费了这次按键（false = 交给宿主应用自己处理）
    pub consume: bool,
    /// 组合串（显示用，已按设置插入音节分隔符）；空 = 结束组合
    pub preedit: String,
    /// 组合内光标（preedit 的 UTF-8 字节位置）
    pub caret: usize,
    /// 需要上屏的文本
    pub commit: String,
    /// 候选（前 overlay_len 个是叠加候选：快捷输入/联想/v 模式）
    pub candidates: Vec<String>,
    pub comments: Vec<String>,
    pub selected: usize,
    pub page: usize,
    pub page_size: usize,
    /// 叠加候选个数（点击换算用）
    pub overlay_len: usize,
    /// 状态栏：中文/英文、半角/全角、中文标点/英文标点
    pub ascii: bool,
    pub full: bool,
    pub punct: bool,
    /// 引擎是否可用
    pub engine_ok: bool,
    /// 出错时的说明（服务侧逻辑异常时带上，DLL 会退回本进程逻辑）
    pub error: Option<String>,
}
