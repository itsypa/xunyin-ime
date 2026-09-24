# 灵犀输入法 (Lingxi IME)

一款基于 Windows TSF（Text Services Framework）与 [Rime/librime](https://github.com/rime/librime)
引擎的现代中文输入法：系统级集成、本地运行、无广告、无遥测，界面与交互对标搜狗输入法。

## 功能

- **系统级 TSF 输入**：在任意应用（记事本、Edge、ChatGPT 等）中直接输入，无需全局钩子。
- **Rime 拼音引擎**：内置大规模词库（含细胞词库、英文词库、Emoji），本地运行。
- **仿 WindInput 状态栏**：拖动柄 + 中/英 + 标点 + 全/半 + 简/繁 + 设置齿轮，
  中文模式与简入繁出为蓝底白字；右键弹开关菜单；按显示器 DPI 自适应缩放。
- **候选框**：光标跟随、候选数量可调、点击/数字键/空格选词。
- **软键盘 / 符号面板**：状态栏「键盘」格或右键菜单打开；键盘按键走输入法组合，
  符号（常用标点/数学/单位/序号/箭头/特殊 六类）直接上屏；面板不抢焦点、可拖动。
- **设置中心**（Tauri）：字号、候选数、全/半角、中英标点、简繁、主题与自定义颜色、
  自定义短语导入导出、**用户词库备份/恢复**；设置自动保存，约 1 秒后对新会话生效。
- **x86 / x64 双架构**：32 位与 64 位应用均可用。

## 目录结构

```
lingxi-ime/    设置中心（Tauri 2 + React）与 Rime 运行时数据（词库、rime.dll）
lingxi-rime/   librime 引擎共享封装（FFI + 会话管理），供 TSF 与设置中心复用
lingxi-tsf/    TSF 文本服务 DLL（拼音输入、候选窗、状态栏）
tools/         部署、签名、状态栏资源生成脚本与测试页
```

## 构建要求

- Rust（MSVC 工具链），含 `i686-pc-windows-msvc` target：
  `rustup target add i686-pc-windows-msvc`
- Node.js（构建设置中心前端）
- VS Build Tools（C++ 桌面负载，编译 librime 依赖）
- 可选：自签名代码签名证书（不签名也能注册使用）

## 构建与部署

1. 编译并注册 TSF 文本服务（x64 / x86 各一次）：

   ```powershell
   .\tools\deploy-tsf.ps1 -Arch x64
   .\tools\deploy-tsf.ps1 -Arch x86
   ```

   脚本会自动编译、签名、注册 CLSID/TIP 档案并重启 ctfmon。

2. 构建设置中心（需 `custom-protocol` 特性，否则前端走 localhost）：

   ```powershell
   cd lingxi-ime
   npm install
   npm run build
   cd src-tauri
   cargo build --features custom-protocol
   ```

3. 切换输入法：系统语言栏选择「中文（简体）- 灵犀输入法」，或 Win+Space。

> 修改 DLL 后请**彻底退出并重开**已打开的应用，让新进程加载新版本；或注销重登。

## GitHub Actions 自动打包

推送 `main` 分支（或手动触发 `workflow_dispatch`）后，
[.github/workflows/build.yml](.github/workflows/build.yml) 会在 `windows-latest`
上自动：

1. 编译 `lingxi-tsf`（x64 + x86 release）；
2. 编译设置中心（Tauri release + 前端 dist）；
3. 用 [wind-installer](https://github.com/huanfeng/wind-installer)（清单驱动）生成
   **安装器 + 卸载器**：`LingxiIME-Setup-<版本>-x64.exe`；
4. 上传产物并在 Release 中发布。

构建成功后还会自动发布 **GitHub Release**：每次构建注入唯一版本号
`0.1.<构建序号>`（合法 semver），Release 名称与安装包文件名均为英文
（`LingxiIME v0.1.<序号>` / `LingxiIME-Setup-0.1.<序号>-x64.exe` 等），避免中文
编码显示问题；应用本身安装后仍显示中文名「灵犀输入法」。以后想手动控制版本时，
可以改为按 `v*` 标签触发。

打包布局（安装目录内）：

```
lingxi-ime.exe                   设置中心
rime.dll                         x64 rime 引擎（设置中心旁）
rime\data\...                    共享词库
tsf\lingxi_tsf_x64.dll           x64 TSF + 同目录 rime.dll（注册 64 位视图）
tsf-x86\lingxi_tsf_x86.dll       x86 TSF + 32 位 rime.dll（注册 32 位视图）
uninstall.exe                    卸载器（控制面板/设置里可卸载）
```

## 本地打包安装器

```powershell
.\tools\pack-installer.ps1                        # 版本号取自 tauri.conf.json
.\tools\pack-installer.ps1 -Version 0.2.0 -Compression zstd
.\tools\pack-installer.ps1 -Arch x64              # 只出 x64 安装包
.\tools\pack-installer.ps1 -SkipBuild             # 复用已构建产物，只重新打包
```

安装器清单在 [installer/app.toml](installer/app.toml)（应用信息、TSF 注册的
CLSID/GUID、快捷方式、启动行为、文案等）。安装器支持 `--silent` / `--quiet` /
`--dir <目录>`，卸载器为安装目录下的 `uninstall.exe`（支持 `--silent`、
`--keep-user-data`，卸载时可选删除用户词库与配置）。

> 未配置代码签名证书时，安装包会触发 Windows SmartScreen 提示，属正常现象；
> 发布正式版时可在构建中配置签名证书（签名后重跑 `pack-installer.ps1`）。

## 状态栏

状态栏位于工作区右下角，自左至右：**拖动柄**、中/英、标点（全角画圆环 / 半角画句点）、
全/半角（满月=全角 / 弯月=半角）、简/繁（蓝底=简入繁出已启用）、设置齿轮。

- 左键点格切换对应状态；中文模式格与繁出格为蓝底白字；
- **右键任意位置**弹出开关菜单（中文模式 / 全角 / 中文标点 / 简入繁出 / 打开设置）；
- 可拖动（会话内记忆位置，重启回默认）；失焦或切换输入法自动隐藏；
- 界面资源由 `tools/gen-statusbar-assets.ps1` 按 2 倍分辨率预渲染，运行时按显示器 DPI 缩放。

## 安装方式

| 方式 | 适用 | 说明 |
|---|---|---|
| `LingxiIME-Setup-<版本>-x64.exe` | 64 位 Windows | 安装器：注册双架构 TSF、建快捷方式、写卸载项，装完自动打开设置中心 |
| `LingxiIME-<版本>-portable.zip` | 任意 Windows（含 32 位） | 解压后以管理员运行其中的 `register-ime.ps1` 注册输入法 |

卸载：安装版走「设置 → 应用 → 灵犀输入法」，也可直接运行安装目录下的 `uninstall.exe`
（支持 `--silent`、`--keep-user-data`）；便携版用 `register-ime.ps1 -Unregister`。

### 代码签名（可选）

未签名时 Windows 会弹 SmartScreen 提示。要启用签名，在仓库设置里加两个 secrets：
`SIGN_PFX_BASE64`（PFX 证书的 Base64）与 `SIGN_PFX_PASSWORD`，CI 会自动按
「加工卸载器 → 签名 → 打包 → 签名安装包」的顺序产出已签名安装包；
本地则用 `tools\pack-installer.ps1 -Sign -PfxPath <证书> -PfxPassword <口令>`。

## 测试

- `tools/tsf_test.html`：候选跟随/选词测试页。
- `lingxi-rime/examples/runtime_probe.rs`：引擎级功能验证（候选、方向键、Shift 切换等）。
- `lingxi-tsf`：`cargo test`（TSF 接口、按键映射、组合逻辑）。

## 第三方组件

- [Rime/librime](https://github.com/rime/librime)（LGPL）与内置 Rime 词库数据，见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
