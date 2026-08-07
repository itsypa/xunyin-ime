# 灵犀输入法 (Lingxi IME)

一款基于 Windows TSF（Text Services Framework）与 [Rime/librime](https://github.com/rime/librime)
引擎的现代中文输入法：系统级集成、本地运行、无广告、无遥测，界面与交互对标搜狗输入法。

## 功能

- **系统级 TSF 输入**：在任意应用（记事本、Edge、ChatGPT 等）中直接输入，无需全局钩子。
- **Rime 拼音引擎**：内置大规模词库（含细胞词库、英文词库、Emoji），本地运行。
- **搜狗式状态栏**：橙圆 Logo + 中/英 + 全/半 + 设置入口，可拖动，按显示器 DPI 自适应缩放。
- **候选框**：光标跟随、候选数量可调、点击/数字键/空格选词。
- **设置中心**（Tauri）：字体大小、候选数量、全/半角、中英标点、词库导入导出等。
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
2. 编译设置中心并生成 NSIS 安装包（内置词库、rime.dll 与 TSF DLL）；
3. 安装包内置注册逻辑：安装时自动注册 x64/x86 两个 TSF 视图，卸载时注销；
4. 上传产物：`灵犀输入法_<版本>_x64-setup.exe`（Actions 页面 Artifacts 下载）。

构建成功后还会自动发布 **GitHub Release**：每次构建注入唯一版本号
`0.1.<构建序号>`（合法 semver），Release 名称与安装包文件名均为英文
（`LingxiIME v0.1.<序号>` / `LingxiIME_0.1.<序号>_x64-setup.exe`），避免中文
编码显示问题；应用本身安装后仍显示中文名「灵犀输入法」。以后想手动控制版本时，
可以改为按 `v*` 标签触发。

打包布局（安装目录内）：

```
lingxi-ime.exe                   设置中心
rime.dll                         x64 rime 引擎
rime\data\...                    共享词库
tsf\lingxi_tsf_x64.dll           x64 TSF（注册 64 位视图）
tsf-x86\lingxi_tsf_x86.dll       x86 TSF + rime.dll（注册 32 位视图）
```

> 未配置代码签名证书时，安装包会触发 Windows SmartScreen 提示，属正常现象；
> 发布正式版时可在构建中配置签名证书。

## 状态栏

状态栏（搜狗式）位于右下角，含：Logo（打开设置）、中/英、全/半、≡（设置入口）。
可拖动（会话内记忆位置，重启回默认）。界面资源由 `tools/gen-statusbar-assets.ps1`
按 2 倍分辨率预渲染，运行时按显示器 DPI 自适应缩放。

## 测试

- `tools/tsf_test.html`：候选跟随/选词测试页。
- `lingxi-rime/examples/runtime_probe.rs`：引擎级功能验证（候选、方向键、Shift 切换等）。
- `lingxi-tsf`：`cargo test`（TSF 接口、按键映射、组合逻辑）。

## 第三方组件

- [Rime/librime](https://github.com/rime/librime)（LGPL）与内置 Rime 词库数据，见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
