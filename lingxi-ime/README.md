# 灵犀输入法 (Lingxi IME)

基于 [librime](https://github.com/rime/librime) 引擎的现代化 Windows 输入法。
输入核心为系统级 **TSF（Text Services Framework）文本服务 DLL**，可在任意
应用（含以管理员运行、DirectInput/原始输入读取的游戏）中提供原生组合串与
自绘候选窗（搜狗风格，跟随光标）；**原生 Win32 设置中心**（无 WebView 依赖，
启动 ~30ms、内存 ~15MB）负责输入方案、全角/半角、模糊音、按键行为、
主题配色、词库导入导出与日志诊断。

仓库布局：

```
D:\灵犀输入法\
├── lingxi-tsf\     系统级 TSF 文本服务 DLL（输入核心，cdylib，x64/x86 双架构）
├── lingxi-rime\    共享 Rime 引擎封装 crate（TSF 与伴侣应用共用）
├── lingxi-ime\     设置中心（native\ = 原生 Win32 应用；rime\ = 共享词库数据）
└── installer\      Windows 安装器 / 卸载器（wind-installer 清单驱动）
```

## 功能特性

- 拼音输入（灵犀拼音：内置办公与开发词库、emoji 与符号速记）
- 只做拼音：随包仅灵犀拼音两套方案（增强词库 / 精简），不夹带五笔、双拼、笔画
- 系统级 TSF 文本服务：任意应用原生组合串 + 自绘候选窗，无注入
- 候选窗跟随光标（插入点锚定 + 100ms 定时重探，鼠标/方向键移动实时跟随）
- 自绘候选窗：浅色/深色/护眼主题、字体大小、不透明度、自定义颜色、显示序号
- 全角/半角切换（设置中心或 Shift+Space）、中英标点、简繁输出、每页候选数
- 词库导入/导出（Rime 自定义短语格式）、重建索引、打开数据目录
- x64 / x86(32 位) 双架构支持：同一 CLSID 按 WOW64 视图注册，32/64 位应用各自加载对应 DLL

## 架构（TSF 方案）

```
┌──────────────────────────────────────────────────────┐
│ lingxi_tsf.dll（TSF 文本服务，COM in-proc 服务器）    │
│   ├─ TextService        激活/按键/组合/候选调度       │
│   ├─ CandidateWindow    自绘候选窗（跟随光标/点击选字）│
│   ├─ DisplayAttribute   组合串下划线显示属性          │
│   └─ ClassFactory       COM 类工厂                    │
│   └─ lingxi-rime         共享 Rime 引擎封装           │
└──────────────────────────────────────────────────────┘
        │ 激活 / 按键 / 编辑会话（TSF 协议）
        ▼
任意应用（记事本、浏览器、游戏、终端……）
```

输入流程：系统把按键交给 TSF 文本服务 → librime 引擎处理 → 通过
`RequestEditSession` 在应用上下文写入组合串 / 上屏文本 → 组合串由系统
按显示属性（下划线）渲染，候选窗由本服务自绘（原生 HWND，DPI 适配）。

## 开发

依赖：Node.js 18+、Rust 1.70+、MSVC Build Tools（VS2022）、librime 预编译产物
（x64：lingxi-ime/rime/；x86：lingxi-ime/rime-x86/）。

### 一键部署（推荐）

```powershell
# 编译 + 签名 + 注册 + 重启输入服务（x64 与 x86 分别执行）
powershell -File tools/deploy-tsf.ps1 -Arch x64 -DllName lingxi_tsf_v13.dll
powershell -File tools/deploy-tsf.ps1 -Arch x86 -DllName lingxi_tsf_v13.dll

# 设置中心（务必启用 custom-protocol 特性，否则会去连 Vite 开发服务器）
powershell -File tools/deploy-settings.ps1 -Arch x64
```

注册表按 WOW64 视图写入同一 CLSID 的 `InprocServer32`：64 位应用加载 x64 DLL，
32 位应用（或原生 32 位 Windows）加载 x86 DLL，互不干扰。

编译 TSF DLL（输出 `lingxi-tsf/target/debug/lingxi_tsf.dll`，rime.dll 自动复制到旁）：

```bash
cd lingxi-tsf
cargo build --offline
```

手动注册输入法（写 HKCU，无需管理员；`regsvr32 /s` 静默执行）：

```bash
regsvr32 /s lingxi_tsf.dll
```

注销：`regsvr32 /u /s lingxi_tsf.dll`。普通用户运行写 HKCU（仅当前用户）；
若以管理员运行，TSF 会把语言档案写入 HKLM（所有用户可用）。
注册后在系统语言栏添加「中文（简体）- 灵犀输入法」，或按 Win+Space 切换。

> 注意：更换 DLL 文件前需先退出所有加载了该 DLL 的应用（如 Edge/记事本），
> 否则文件被占用无法覆盖。

引擎测试（共享 crate）：

```bash
cd lingxi-rime
cargo test --lib --offline -- --test-threads=1
```

设置中心（原生 Win32，无需 Node/WebView）：

```bash
cd lingxi-ime\native
cargo build              # 调试版（启动 ~130ms）
cargo build --release    # 发布版（启动 ~30ms）
# 或走部署脚本：编译 + 签名（+ 可选拉起）
powershell -File ..\tools\deploy-settings.ps1 -Arch x64
```

## 构建安装包

```bash
powershell -File tools\pack-installer.ps1 -Arch both
```

产物位于 `build\installer-dist\`（安装包）与 `build\`（便携 zip）。

## 快捷键

| 快捷键 | 功能 |
| --- | --- |
| Shift | 组合中按 Shift 直接把拼音上屏（不切英文）；未组合时按 Shift 切换中英 |
| Tab / Shift+Tab | 候选翻页 |
| - / = | 候选翻页（有菜单时） |
| 1-0 | 选择候选 |
| Enter | 上屏当前高亮候选 |
| Esc | 清除组合 |
| Backspace | 删除拼音 / 回退 |

## Rime 数据

- 共享数据（词典、schema、opencc）：`lingxi-ime/rime/data/`，随包发布
- 用户数据（用户词库、日志、sync 备份）：`%APPDATA%\com.lingxi.ime\`
- 首次激活自动部署建索引，期间按键放行给应用

## 已知限制

- 标点映射按标准美式键盘布局（QWERTY）
- x86 版内置 librime 1.11.2（取自小狼毫 0.16.3 安装包），功能与 x64 版一致
