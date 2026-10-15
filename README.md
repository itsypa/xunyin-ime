# 迅音输入法 (Xunyin IME)

一款基于 Windows TSF（Text Services Framework）与 [Rime/librime](https://github.com/rime/librime)
引擎的现代中文输入法：系统级集成、本地运行、无广告、无遥测，界面与交互对标搜狗输入法。

## 功能

- **系统级 TSF 输入**：在任意应用（记事本、Edge、ChatGPT 等）中直接输入，无需全局钩子。
- **Rime 拼音引擎**：内置大规模词库（含细胞词库、英文词库、Emoji），本地运行。
- **仿 WindInput 状态栏**：拖动柄 + 中/英 + 标点 + 全/半 + 符号 + 设置齿轮，
  中文模式与符号面板打开时为蓝底；右键弹开关菜单；按显示器 DPI 自适应缩放。
- **候选框**：光标跟随、候选数量可调、点击/数字键/空格选词。
- **快捷输入**：`rq` 日期、`sj` 时间、`xq` 星期；`v` 模式（`v123` → 中文数字/大写/十六进制/二进制，
  `v1+2*3` → 直接算结果），候选插在最前面，空格或点击上屏。
- **删词**（搜狗式）：组合中 `Ctrl+数字` 删掉那个错词——librime 侧调 `delete_candidate`
  更新用户词库，同时记进本地黑名单（`blocked.txt`），保证"删了就不再出现"。
- **词语联想**：本地学习「上屏 A 后又上屏 B」，下次 A 上屏后把 B 作为联想候选给出（初始为空，用着用着才有效）。
- **敏感输入**：应用禁用输入法时（密码框等，`GUID_COMPARTMENT_KEYBOARD_DISABLED`）不接管按键、不弹候选窗。
- **候选窗**：竖排/横排、跟随光标/固定右下角、字体可选、注释可关。
- **符号面板**：状态栏「符号」格或右键菜单打开，常用标点/数学/单位/序号/箭头/特殊
  六类符号点选直接上屏；面板不抢焦点、可拖动。
- **设置中心**（原生 Win32 自绘，无 WebView2，启动快、占用小）：字号、候选数、全/半角、
  中英标点、主题与自定义颜色、标点自定义（把某键映射为指定文本）、**皮肤导入/导出**、
  自定义短语导入导出、**用户词库备份/恢复**；设置自动保存，约 1 秒后对新会话生效。
- **热重载**（对标小狼毫）：装好新方案 / 换完词库后点「重新部署」，正在运行的应用
  （记事本、浏览器、ChatGPT…）会自动重建引擎并用上新方案，**不用重启任何应用**。
- **32 位程序也能用**：安装包内含 x86 的 TSF DLL + 引擎 + 服务，64 位系统里的 32 位程序
  （老工具、部分游戏、国产小软件）照样能打中文。注意**只支持 64 位 Windows** ——
  32 位系统装不了（设置中心与安装器都是 x64 程序）。

## 目录结构

```
xunyin-ime/native/  设置中心（原生 Win32 + GDI+，无 WebView2）
xunyin-ime/rime/    Rime 运行时数据（共享词库、预编译索引、librime 的 rime.dll / rime.lib）
xunyin-rime/        librime 引擎共享封装（FFI + 会话管理 + 重载/方案清单），供 TSF 与设置中心复用
xunyin-tsf/         TSF 文本服务 DLL（按键路由、组合串写入、候选窗、状态栏）
xunyin-svc/         常驻服务进程：输入法本体（librime 引擎 + 全部输入逻辑），可随时重启
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

### 服务进程（输入逻辑已全部搬进去）

`xunyin-svc.exe` 是常驻服务进程，放在 TSF DLL 旁边。**输入法本体（librime 引擎 +
全部输入逻辑）跑在它里面**，宿主进程里的 TSF DLL 只负责"送按键、把结果写回应用"：

- DLL 连不上服务时会自己把 `xunyin-svc.exe` 拉起来（`CREATE_NO_WINDOW`，不留黑框）；
- DLL 每 2 秒比对一次服务 exe 的**指纹（mtime + 大小）**，发现磁盘上的服务被重编替换，
  就杀掉旧服务进程、按新 exe 重新连接 —— **宿主应用完全无感**；
- 协议只做"最低版本"校验：服务比 DLL 新也能直接用（所以**改服务代码不需要重编 DLL**）；
- 服务没有客户端连接 60 秒后自己退出，不常驻占资源；日志写在
  `%APPDATA%\com.xunyin.ime\logs\svc.log`（DLL 侧状态在 `tsf.log` 里搜 `svc:`）。

按键往返：DLL 把 `(vk, mask, shift)` 通过命名管道发给服务，服务算完回一个
`Reply`（组合串 / 光标 / 上屏文本 / 候选 / 状态位），DLL 照着写进 TSF 组合串与候选窗。
往返正常在 1ms 以内；请求连接带 80ms 读写超时，服务卡住/崩掉/报错时
**自动落回 DLL 里的同一套逻辑**（先做了状态同步：把正在显示的组合串喂给本进程引擎，
接着打不丢字），所以服务出问题不会让人打不了字。

应急开关：`set XUNYIN_SVC_INPUT=0` 后重启服务（或杀掉进程），服务就只做握手/心跳，
输入走 DLL 内逻辑。

服务里也有崩溃记录：遇到访问违例等致命异常时，会往 `svc.log` 写"哪条指令、
读还是写、访问了哪个地址、调用链"（`xunyin-svc/src/crashlog.rs`），
不用装调试器就能定位。

### 开发期：不依赖宿主应用直接驱动服务

```powershell
# 按键序列（N/I/H/A/O/SPACE/ENTER/BACK/LEFT/RIGHT/SEMI…），-Times 重复轮数
.\tools\svc-probe.ps1 -Keys "N,I,H,A,O,SPACE" -Times 3
# 候选点击（显示下标）与命令
.\tools\svc-probe.ps1 -Keys "N,I,SELECT:1,CMD:toggle_mode"
```

它会自己起服务、连管道、把每条按键的回复打出来，最后打印本次新增的 `svc.log`。
排查崩溃时 `tools/disasm_rva.py <exe> 0x<RVA>` 能把日志里那个偏移反汇编出来。

- DLL 连不上服务时会自己把 `xunyin-svc.exe` 拉起来（`CREATE_NO_WINDOW`，不留黑框）；
- DLL 每 2 秒比对一次服务 exe 的**指纹（mtime + 大小）**，发现磁盘上的服务被重编替换，
  就杀掉旧服务进程、按新 exe 重新连接 —— **宿主应用完全无感**；
- 协议只做"最低版本"校验：服务比 DLL 新也能直接用（所以**改服务代码不需要重编 DLL**）；
- 服务没有客户端连接 60 秒后自己退出，不常驻占资源；日志写在
  `%APPDATA%\com.xunyin.ime\logs\svc.log`（DLL 侧状态在 `tsf.log` 里搜 `svc:`）。

实测：连上服务 → 替换磁盘上的 exe → 日志出现"磁盘上的服务已更新 → 重启服务进程"，
随后自动连上新服务，**宿主进程（代表正在打字的 Edge / ChatGPT）没有重启**。

> 规划：第 3 阶段把候选窗/状态栏/符号面板搬进服务（此后改界面也不用重开）；
> 第 4 阶段补服务随登录自启与安装包里的注册/卸载清理。

`%APPDATA%\com.xunyin.ime\reload.request` 是「重载请求」标记文件：设置中心、
`deploy-tsf.ps1`、状态栏菜单都会往里写一条请求，**服务与每个宿主进程里的 TSF DLL**
都在盯它（状态栏定时器 400ms、按键路径每 3 秒各查一次），看到新请求就地重建 librime
引擎（finalize → initialize → 应用配置），于是：

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
| 输入选项（中英标点、全/半角默认值、按键行为、自定义标点、删词键、联想/快捷输入开关） | 同上 |
| 候选来源（Emoji / 英文单词 / 自定义短语 开关） | 保存即写方案补丁 → 重新部署 → 立刻生效 |
| 候选窗外观（排列、位置、字体、注释、字号、配色、圆角、不透明度） | 设置中心保存即写重载请求 → 立刻生效 |
| 状态栏图标、条底、格子高亮等**界面资源** | 改皮肤文件 → 「重载配置」→ 立刻生效（见下） |
| **输入逻辑**（按键行为、标点、删词、快捷输入、联想、组合串格式…） | 改完重建 `xunyin-svc.exe` 即可 → 服务自动换新 → **立刻生效** |
| 候选窗 / 状态栏 / 符号面板的**绘制代码**、TSF 交互代码 | 仍在 DLL 里，**必须完全退出重开应用**（或注销重登） |

### 皮肤（界面资源热更新）

状态栏的位图资源是**磁盘优先、内嵌兜底**：`%APPDATA%\com.xunyin.ime\skins\<名字>.bin`
存在且尺寸正确时就用它，否则用 DLL 里内嵌的那份。所以换图标只要改文件 + 点一次
「重载配置」（状态栏齿轮菜单，或设置中心「维护与诊断」），**所有正在运行的应用立即换上新图标**。

```powershell
.\tools\export-skin-assets.ps1          # 把内置资源导出到 %APPDATA%\com.xunyin.ime\skins
.\tools\export-skin-assets.ps1 -Remove  # 删掉皮肤文件，回到内置资源
```

格式：8 字节头（u32 宽 + u32 高，小端）+ 宽×高×4 字节 BGRA（自顶向下、非预乘 alpha）。
尺寸必须与内置一致（`bar_bg.bin` = 324×60，其余 = 60×60），不符会被忽略并回退内嵌——
blit 按常量步长取像素，长度不足会读越界，所以宁可忽略也不能照单全收。
图标类文件只用 alpha 通道（颜色由主题 tint 决定），做单色蒙版即可。

> 皮肤文件**优先级高于内置资源**，升级后内置图标若更新，旧副本会把它遮住；不想用皮肤
> 就跑一次 `-Remove`。资源目录也可以显式指定：环境变量 `XUNYIN_SKIN_DIR` 优先级最高。

设置中心 → 外观主题 → **皮肤中心**：11 个状态栏资源的缩略图与来源（皮肤/内置）、
导出资源包、导入资源包、打开皮肤目录、一键恢复内置。资源包就是这 11 个 `.bin` 打成
的 zip，可以备份或分享。

每一项都是**可点的**：点一下就把这份资源导出成文件、并用资源管理器定位到它，
改完回到设置中心点「重载配置」，正在输入的应用立刻换上新图标。鼠标停在某一项上会
显示它的用途说明（例如 `glyph_ban.bin` = 半角状态图标·半月）。

### 细胞词库（专业词库）开关

设置中心 → 词库管理 → **细胞词库**：勾掉用不到的专业词库（医药、象棋、游戏、数理化…）
再点「应用」。实现方式是把随包的 `rime_xunyin.dict.yaml` 复制一份到用户目录、
只重排 `import_tables`（核心导入一个字不改），Rime 读词库时用户目录优先——
**随包数据不被改动**，将来升级也不会冲突。实测（引擎探针）：用户级词表生效，
额外词库的词能直接打出来。

> 改完要重建一次词库索引（后台进行，完成后自动生效）。索引越小重建越快，
> 这也是关掉不用词库的实际收益。

原因：TSF 是 in-process 模型，`xunyin_tsf_x64.dll` 会被加载进**每一个正在输入的应用**，
进程一旦映射了 DLL，Windows 就锁住映像——代码和 `include_bytes!` 内嵌的资源
（状态栏图标就在里面，见 `xunyin-tsf/src/assets/`）都无法在进程内替换。部署脚本
复制新 DLL 时要把旧文件**改名让路**（日志里的「旧文件已改名让路」）正是这个锁导致的；
被改名的那份映像仍是老进程正在跑的内容，所以它们看起来"没更新"。

所以有一条分界线：**输入法本体（引擎 + 输入逻辑）已经在服务进程里**，改完重编
`xunyin-svc.exe` 就直接生效；还留在 DLL 里的是"跟宿主应用打交道的那层"
（TSF 按键路由、组合串写入、候选窗与状态栏绘制），这部分改代码仍要重开应用。
下一步（第 3 阶段）就是把候选窗/状态栏也搬进服务，那时改界面也不用重开了。

## GitHub Actions 自动打包

推送 `main` 分支（或手动触发 `workflow_dispatch`）后，
[.github/workflows/build.yml](.github/workflows/build.yml) 会在 `windows-latest`
上自动：

1. 编译 `xunyin-tsf`（x64 + x86 release）；
2. 编译设置中心（Tauri release + 前端 dist）；
3. 用 [wind-installer](https://github.com/huanfeng/wind-installer)（清单驱动）生成
   **安装器 + 卸载器**：`XunyinIME-Setup-<版本>-x64.exe`（只发 x64：本项目不支持
   32 位 Windows；32 位**程序**的支持在安装包内的 `tsf-x86`，与安装包架构无关）；
4. 上传产物并在 Release 中发布。

构建成功后还会自动发布 **GitHub Release**：每次构建注入唯一版本号
`0.1.<构建序号>`（合法 semver），Release 名称与安装包文件名均为英文
（`XunyinIME v0.1.<序号>` / `XunyinIME-Setup-0.1.<序号>-x64.exe`），避免中文
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
| `XunyinIME-Setup-<版本>-x64.exe` | 64 位 Windows | 安装器：注册 x64 与 x86 两套 TSF、建快捷方式、写卸载项，装完自动打开设置中心 |
| `XunyinIME-<版本>-portable.zip` | 64 位 Windows | 解压后以管理员运行其中的 `register-ime.ps1` 注册输入法 |

卸载：安装版走「设置 → 应用 → 迅音输入法」，也可直接运行安装目录下的 `uninstall.exe`
（支持 `--silent`、`--keep-user-data`）；便携版用 `register-ime.ps1 -Unregister`。

### 代码签名（可选）

未签名时 Windows 会弹 SmartScreen 提示。要启用签名，在仓库设置里加两个 secrets：
`SIGN_PFX_BASE64`（PFX 证书的 Base64）与 `SIGN_PFX_PASSWORD`，CI 会自动按
「加工卸载器 → 签名 → 打包 → 签名安装包」的顺序产出已签名安装包；
本地则用 `tools\pack-installer.ps1 -Sign -PfxPath <证书> -PfxPassword <口令>`。

## 测试

- `tools/tsf_test.html`：候选跟随/选词测试页。
- `tools/svc-probe.ps1`：不依赖宿主应用直接驱动服务（按键 / 选词 / 命令），
  输入逻辑与热更新回归用它最快。
- `xunyin-rime/examples/runtime_probe.rs`：引擎级功能验证（候选、方向键、Shift 切换等）。
- `xunyin-tsf` / `xunyin-rime`：`cargo test`（TSF 接口、按键映射、组合串换算、
  配置、快捷输入、联想、黑名单）。注意 `xunyin-rime` 里 `rime_engine::tests` 那几条
  端到端用例要完整部署一次词库（数分钟），平时按模块过滤跑即可：
  `cargo test --lib -- preedit:: keycode:: config:: quick_input:: assoc:: blocklist::`

## 维护

- **日志与隐私**：默认日志**不记用户输入**（原始按键、预编辑、候选词、上屏文本一律只记长度/数量），
  因为"导出诊断包"会把日志打包，可能被贴到公开 issue 里。排查输入问题时打开输入日志：

  ```powershell
  New-Item "$env:APPDATA\com.xunyin.ime\log-input" -Force   # 开（每 5 秒生效一次）
  Remove-Item "$env:APPDATA\com.xunyin.ime\log-input"        # 关
  ```

  也可以设环境变量 `XUNYIN_LOG_INPUT=1`（但 Edge/ChatGPT 这类现成进程不方便单独设）。
  日志位置：`%APPDATA%\com.xunyin.ime\logs\`（`tsf.log` = 宿主 DLL，`svc.log` = 常驻服务）。

- **按键失败策略（对齐水杉输入法的结论）**：通道失败时**丢这颗键 + 取消组合**，
  绝不"把组合串喂回本进程引擎重放"——回放会带来闪烁、双上屏和重试循环。
  失败后请求通道冷却 5 秒（这期间直接走本进程逻辑），避免每颗键都白等一次超时。

- **诊断包**：设置中心 → 关于 → 导出诊断包（版本/配置/日志/词库统计打包成 zip，里面只有本机信息）。
- **配置漫游**：关于 → 导出/导入全部设置（config.json + 方案补丁合成一个 JSON）。
- **检查更新**：关于 → 检查更新（读 GitHub Release 最新 tag 比对，不下载不安装；需要网络）。
- **下载并安装**：关于 → 下载并安装（下载官方安装包到临时目录并启动，需要网络；
  仓库还没发 Release 时会提示"暂时没有可用版本"）。
- **检查词库更新**：关于 → 检查词库更新（比对 UPSTREAM.md 记录的上游版本与
  rime-frost 最新 tag）。
- **上游词库更新（暂存 → 部署 → 回滚）**：词库管理 → 上游词库更新。刻意分两步：
  「下载到暂存」只写 `%APPDATA%\com.xunyin.ime\update\`（下载 rime-frost 的
  `rime-frost-schemas.zip` → 解压 → 校验词库文件与数量），**不碰数据目录**；
  「部署到本机」才把暂存的 `cn_dicts/`、`cn_dicts_cell/` 写进去，覆盖前先备份到
  `update/backup-<时间>`，并可「回滚上次」。安装目录只读时自动改装到用户目录
  （Rime 用户目录优先），并同步更新 UPSTREAM.md 的版本行。
- **用户词库**：词库管理 → 导出用户词库（学习记录 + 联想表 + 黑名单）、清空用户词库。

设置中心「维护与诊断」里的按钮：打开数据/日志/皮肤目录、清空日志、恢复默认设置、
重载配置、重新部署。

## 参考项目带来的待办

架构与 [水杉输入法（msime-windows）](https://github.com/metasequoiaime/msime-windows) 相同
（TSF DLL + 常驻 Server，隔版本化命名管道；协议只有一份定义；候选状态的权威在 Server），
已经照着它改掉的：失败不回放、请求通道冷却、重建索引不挡按键、日志不记用户输入。
还没做、但值得做的（都在后续阶段里）：

- **候选窗与状态栏搬进服务进程**（第 3 阶段）。这不只是为了热更新：水杉的 Server 带
  `uiAccess="true"` 清单才能把浮层画在**高完整性窗口**（管理员程序、部分游戏）之上；
  我们的候选窗现在是宿主进程内的普通窗口，在提权应用里根本不显示。搬过去顺带解决这一条。
- **按显示器 DPI 感知**（`PER_MONITOR_AWARE_V2`）：候选窗/状态栏跨屏移动、125%/150%/200%
  缩放时都要重新布局，不能缓存换算结果。
- **游戏兼容**（安装到 `System32\IME\<名字>\` + 代码签名）：CS2 这类开了 Trusted Mode 的
  游戏只放行"位于系统目录且有签名"的第三方 in-proc DLL，装在 Program Files 里切不出来。

## 第三方组件

- [Rime/librime](https://github.com/rime/librime)（LGPL）与内置 Rime 词库数据，见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
