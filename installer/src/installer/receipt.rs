//! 安装回执 —— 记录安装期**实际做成了什么**，卸载时反向回放。
//!
//! 卸载此前是把安装步骤反过来又手写一遍，靠读清单猜"当初大概装了这些"。这有三个毛病：
//! 部分失败的安装会被当成完整安装来撤销；两版之间清单改了（如 `display_name` 变更）
//! 就找不到旧注册表键；每加一个能力都要在卸载侧记得加对应的反操作。
//!
//! 回执改为记录**具体产物**（绝对路径、profile 字符串、注册表键全名），于是：
//! - 卸载器不需要知道 IME/字体是什么，只需按条目类型执行反操作；
//! - 没做成的事不会进回执，也就不会被撤销；
//! - 撤销不依赖当前清单，升级后仍能正确清理上一版留下的东西。
//!
//! 存储位置为注册表 `HKLM\Software\{app_id}` 的 `Receipt` 值（TOML 文本）——
//! 不落在安装目录，以维持「安装目录不留散落文件」这条约定。

use serde::{Deserialize, Serialize};
use winreg::enums::*;
use winreg::RegKey;

use crate::meta;

/// 回执值名。
const RECEIPT_VALUE: &str = "Receipt";

/// 一条可撤销的安装产物。
///
/// 每个变体记录的都是**撤销所需的全部信息**，不回头查清单。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ReceiptEntry {
    /// regsvr32 注册的 COM DLL（系统目录副本的绝对路径；撤销时反注册并删除该副本）。
    /// `wow64` 表示用 SysWOW64 的 regsvr32 注册（32 位 DLL）。
    ComRegistered { dll: String, wow64: bool },
    /// TSF 输入法 profile 字符串（`<lang>:<clsid><guid>`）。
    InputMethodRegistered { profile: String },
    /// 装入 `%WINDIR%\Fonts` 的字体。`display_name` 是 Fonts 注册表里的值名。
    FontInstalled { file: String, display_name: String },
    /// `HKCU\...\Run` 下的自启动值。
    AutoStartSet { value_name: String },
    /// `HKCU\Software\Classes` 下的协议键。
    UrlProtocolRegistered { protocol: String },
    /// 创建的快捷方式文件（绝对路径）。
    ShortcutCreated { path: String },
    /// 创建的开始菜单文件夹（绝对路径）。
    StartMenuFolderCreated { path: String },
    /// Add/Remove Programs 键（`HKLM` 下的完整子键路径）。
    UninstallInfoWritten { key: String },
    /// 写入的数据目录配置文件（绝对路径）。
    DataDirConfWritten { path: String },
}

/// 一次安装产生的全部可撤销产物，按执行先后排列。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Receipt {
    #[serde(default)]
    pub entries: Vec<ReceiptEntry>,
}

impl Receipt {
    /// 记一条产物。等值条目去重——升级会把旧回执读进来续写，重复安装同一版本
    /// 不该让回执无限膨胀。
    pub fn push(&mut self, entry: ReceiptEntry) {
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }

    /// 撤销顺序 = 安装顺序的逆序。
    pub fn undo_order(&self) -> impl Iterator<Item = &ReceiptEntry> {
        self.entries.iter().rev()
    }

    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string(self).map_err(|e| format!("序列化回执失败: {}", e))
    }

    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("解析回执失败: {}", e))
    }

    /// 写入 `HKLM\Software\{app_id}` 的 `Receipt` 值。
    pub fn save(&self) -> Result<(), String> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let (key, _) = hklm
            .create_subkey(app_key())
            .map_err(|e| format!("无法创建应用注册表键: {}", e))?;
        key.set_value(RECEIPT_VALUE, &self.to_toml()?)
            .map_err(|e| format!("无法写入回执: {}", e))
    }

    /// 读取回执，区分两种「读不到」：
    /// - `Ok(None)`：从未写入（首装、便携装、或本就没装过）——正常。
    /// - `Err`：键在但内容损坏——**异常**，意味着有产物却撤销不了，必须让调用方汇报，
    ///   不能压成「无可撤销项」悄悄跳过。
    pub fn try_load() -> Result<Option<Self>, String> {
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let Ok(key) = hklm.open_subkey_with_flags(app_key(), KEY_READ) else {
            return Ok(None);
        };
        let Ok(text) = key.get_value::<String, _>(RECEIPT_VALUE) else {
            return Ok(None);
        };
        Self::from_toml(&text).map(Some)
    }

    /// 尽力读取，读不到就当空。用于安装期续写旧回执——那里读不到旧回执并非异常
    /// （首装本就没有），且新回执马上会覆盖它。
    pub fn load_or_default() -> Self {
        Self::try_load().ok().flatten().unwrap_or_default()
    }
}

fn app_key() -> String {
    format!(r"Software\{}", meta::app_id())
}
