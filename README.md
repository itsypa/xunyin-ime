# 迅音输入法 (Xunyin IME)

一款基于 Windows TSF（Text Services Framework）与 [Rime/librime](https://github.com/rime/librime)
引擎的现代中文输入法：系统级集成、本地运行、无广告、无遥测，界面与交互对标搜狗输入法。

## 功能

- **系统级 TSF 输入**：在任意应用（记事本、Edge、ChatGPT 等）中直接输入，无需全局钩子。
- **Rime 拼音引擎**：内置大规模词库（含细胞词库、英文词库、Emoji），本地运行。
- **仿 WindInput 状态栏**：拖动柄 + 中/英 + 标点 + 全/半 + 符号 + 设置齿轮，
  中文模式与符号面板打开时为蓝底；右键弹开关菜单；按显示器 DPI 自适应缩放。
- **候选框**：光标跟随、候选数量可调、点击/数字键/空格选词。
- **符号面板**：状态栏「符号」格或右键菜单打开，常用标点/数学/单位/序号/箭头/特殊
  六类符号点选直接上屏；面板不抢焦点、可拖动。
- **设置中心**（原生 Win32 自绘，无 WebView2，启动快、占用小）：字号、候选数、全/半角、
  中英标点、主题与自定义颜色、标点自定义（把某键映射为指定文本）、**皮肤导入/导出**、
  自定义短语导入导出、**用户词库备份/恢复**；设置自动保存，约 1 秒后对新会话生效。
- **热重载**（对标小狼毫）：装好新方案 / 换完词库后点「重新部署」，正在运行的应用
  （记事本、浏览器、ChatGPT…）会自动重建引擎并用上新方案，**不用重启任何应用**。
- **x86 / x64 双架构**：32 位与 64 位应用均可用。

## 目录结构

```
xunyin-ime/native/  设置中心（原生 Win32 + GDI+，无 WebView2）
xunyin-ime/rime/    Rime 运行时数据（共享词库、预编译索引、librime 的 rime.dll / rime.lib）
xunyin-rime/        librime 引擎共享封装（FFI + 会话管理 + 重载/方案清单），供 TSF 与设置中心复用
xunyin-tsf/         TSF 文本服务 DLL（拼音输入、候选窗、状态栏）
tools/              部署、签名、资源生成脚本与测试页
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

   脚本会把产物复制到**固定路径**的暂存布局 `build\dev-deploy\`（与安装包布局一致：
   `tsf\xunyin_tsf_x64.dll`、`tsf-x86\xunyin_tsf_x86.dll`），签名后注册 CLSID/TIP 档案。

   > 注册表只指向这个固定路径，**不会**指向 `target\debug`：TSF 会把输入法 DLL 加载进
   > **每一个正在输入的进程**，而 cargo 每次构建都会重写 `target\debug`，路径一变就会出现
   > "已打开的进程持着旧 DLL、注册表已是新 DLL"的错位，症状是**所有输入法都打不出中文**。
   > 脚本也**不再强杀 ctfmon / TextInputHost**（强杀会把已打开应用的输入法状态打断）。

2. 构建设置中心（原生应用，无需 Node）：

   ```powershell
   .\tools\deploy-settings.ps1        # 出 release 并安装到用户目录（-Debug 才用调试版）
   ```

3. 切换输入法：系统语言栏选择「中文（简体）- 迅音输入法」，或 Win+Space。

> 改 DLL 本身仍需**彻底退出并重开**已打开的应用（或注销重登）才会加载新版本；
> 改词库 / 方案 / 设置则不用——写一次重载请求即可（见下方「热重载」）。

## 热重载（重新部署 / 重载配置）

`%APPDATA%\com.xunyin.ime\reload.request` 是「重载请求」标记文件：设置中心、
`deploy-tsf.ps1`、状态栏菜单都会往里写一条请求，每个宿主进程里的 TSF DLL
（状态栏定时器 400ms、按键路径每 3 秒各查一次）看到新请求就地重建 librime 引擎
（finalize → initialize → 应用配置），于是：

- **装新输入方案**：把 `<id>.schema.yaml` 与词库丢进 `xunyin-ime\rime\data\`，
  点状态栏齿轮菜单的「重新部署（重建索引）」——方案会自动补进 `schema_list`、
  词库被编译，正在输入的应用立刻能选用，无需重启；
- **改设置 / 换皮肤**：设置中心保存时自动写重载请求，所有应用即时生效；
- **只想重读配置**：状态栏齿轮菜单 →「重载配置」。

> 多个进程不会同时部署：只有点「重新部署」的那个进程做编译，编译完成后才通知
> 其他进程重载（避免并发写同一份索引）。

### 什么能热重载、什么必须重启应用

热重载重建的是 **librime 引擎 + 从磁盘读回来的配置**，所以分界线是「编译进 DLL 的东西」：

| 改什么 | 怎么生效 |
|---|---|
| 词库、输入方案、模糊音、每页候选数 | 重新部署 → 立刻生效（索引重编译后各进程重读） |
| 外观设置（字号、候选数、主题配色、不透明度、显示索引） | 设置中心保存即写重载请求 → 立刻生效 |
| 输入选项（中英标点、全/半角默认值、按键行为、自定义标点） | 同上 |
| 状态栏图标、候选窗绘制方式、按键处理逻辑等**代码/内嵌资源** | **必须完全退出重开应用**（或注销重登） |

原因：TSF 是 in-process 模型，`xunyin_tsf_x64.dll` 会被加载进**每一个正在输入的应用**，
进程一旦映射了 DLL，Windows 就锁住映像——代码和 `include_bytes!` 内嵌的资源
（状态栏图标就在里面，见 `xunyin-tsf/src/assets/`）都无法在进程内替换。部署脚本
复制新 DLL 时要把旧文件**改名让路**（日志里的「旧文件已改名让路」）正是这个锁导致的；
被改名的那份映像仍是老进程正在跑的内容，所以它们看起来"没更新"。

## GitHub Actions 自动打包

推送 `main` 分支（或手动触发 `workflow_dispatch`）后，
[.github/workflows/build.yml](.github/workflows/build.yml) 会在 `windows-latest`
上自动：

1. 编译 `xunyin-tsf`（x64 + x86 release）；
2. 编译设置中心（Tauri release + 前端 dist）；
3. 用 [wind-installer](https://github.com/huanfeng/wind-installer)（清单驱动）生成
   **安装器 + 卸载器**：`XunyinIME-Setup-<版本>-x64.exe`（64 位 Windows）与
   `XunyinIME-Setup-<版本>-x86.exe`（32 位 Windows）；
4. 上传产物并在 Release 中发布。

构建成功后还会自动发布 **GitHub Release**：每次构建注入唯一版本号
`0.1.<构建序号>`（合法 semver），Release 名称与安装包文件名均为英文
（`XunyinIME v0.1.<序号>` / `XunyinIME-Setup-0.1.<序号>-x64.exe` / `-x86.exe`），避免中文
编码显示问题；应用本身安装后仍显示中文名「迅音输入法」。以后想手动控制版本时，
可以改为按 `v*` 标签触发。

打包布局（安装目录内）：

```
xunyin-ime.exe                   设置中心
rime.dll                         x64 rime 引擎（设置中心旁）
rime\data\...                    共享词库
tsf\xunyin_tsf_x64.dll           x64 TSF + 同目录 rime.dll（注册 64 位视图）
tsf-x86\xunyin_tsf_x86.dll       x86 TSF + 32 位 rime.dll（注册 32 位视图）
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

状态栏位于工作区右下角，自左至右：**拖动柄**、中/英、标点（中文画「，。」/ 英文画「,.」）、
全/半角（**满月=全角 / 半月=半角**）、符号（※，蓝底=面板已打开）、设置齿轮。

- 左键点格切换对应状态；中文模式格与符号格为蓝底；
- **右键任意位置**弹出开关菜单（中文模式 / 全角 / 中文标点 / 符号面板 / 打开设置）；
- 可拖动（会话内记忆位置，重启回默认）；失焦或切换输入法自动隐藏；
- 界面资源由 `tools/gen-statusbar-assets.ps1` 按 2 倍分辨率预渲染，运行时按显示器 DPI 缩放。
- 语言栏 / 任务栏图标是 `xunyin-ime\icons\xunyin.ico`（蓝底白字「迅」，16~256 多尺寸），
  由 `tools/gen-app-icon.ps1` 生成；DllRegisterServer 会把它作为**语言档案图标**注册
  （只写注册表 IconFile 不生效，语言栏会退回显示「简体」二字）。

## 安装方式

| 方式 | 适用 | 说明 |
|---|---|---|
| `XunyinIME-Setup-<版本>-x64.exe` | 64 位 Windows | 安装器：注册双架构 TSF、建快捷方式、写卸载项，装完自动打开设置中心 |
| `XunyinIME-<版本>-portable.zip` | 任意 Windows（含 32 位） | 解压后以管理员运行其中的 `register-ime.ps1` 注册输入法 |

卸载：安装版走「设置 → 应用 → 迅音输入法」，也可直接运行安装目录下的 `uninstall.exe`
（支持 `--silent`、`--keep-user-data`）；便携版用 `register-ime.ps1 -Unregister`。

### 代码签名（可选）

未签名时 Windows 会弹 SmartScreen 提示。要启用签名，在仓库设置里加两个 secrets：
`SIGN_PFX_BASE64`（PFX 证书的 Base64）与 `SIGN_PFX_PASSWORD`，CI 会自动按
「加工卸载器 → 签名 → 打包 → 签名安装包」的顺序产出已签名安装包；
本地则用 `tools\pack-installer.ps1 -Sign -PfxPath <证书> -PfxPassword <口令>`。

## 测试

- `tools/tsf_test.html`：候选跟随/选词测试页。
- `xunyin-rime/examples/runtime_probe.rs`：引擎级功能验证（候选、方向键、Shift 切换等）。
- `xunyin-tsf`：`cargo test`（TSF 接口、按键映射、组合逻辑）。

## 第三方组件

- [Rime/librime](https://github.com/rime/librime)（LGPL）与内置 Rime 词库数据，见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
