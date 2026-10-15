# 迅音输入法 (Xunyin IME)

基于 Windows TSF 与 [Rime/librime](https://github.com/rime/librime) 的中文输入法：
系统级集成、本地运行、无广告无遥测，界面与交互对标搜狗输入法。

- 只支持 **64 位 Windows**；64 位系统里的**32 位程序**照样能用（安装包内含 x86 的 DLL + 引擎 + 服务）。
- 输入法本体（引擎 + 全部输入逻辑）跑在常驻服务 `xunyin-svc.exe` 里，宿主进程里的 TSF DLL 只做
  "送按键、把结果写回应用"，所以**改输入逻辑不用重开任何应用**。

## 功能

- **输入**：全拼 + 模糊音 + 纠错表；`rq`/`sj`/`xq`/`nl` → 日期/时间/星期/农历；`v` 模式
  （`v123` → 中文数字·大写·十六进制，`v1+2*3` → 直接算结果）；`zhangsan@`、`www.baidu` 补全邮箱/网址。
- **候选框**：跟随光标或固定位置、竖排/横排、字号/主题/自定义配色，点击·数字·空格选词。
- **状态栏**：中/英、中英标点、全/半角、符号面板、设置齿轮；可拖动、按显示器 DPI 缩放、右键出菜单。
- **符号面板**：常用/数学/单位/序号/箭头/特殊/表情/颜文字，点一下直接上屏。
- **词库**：内置大词库 + 细胞词库可开关；**导入搜狗细胞词库（`.scel`）与文本词表**（低优先级，
  不抢普通候选）；自定义短语（最高优先级）；用户词典自学习；上游词库更新与回滚；用户词库备份/恢复。
- **删词**：组合中 `Ctrl+数字` 删掉错词（同时进黑名单，之后不再出现）。
- **联想**：本地学习"上屏 A 后常跟着打 B"。
- **外观**：皮肤资源可改、可导出导入；状态栏与候选框按 DPI 缩放。
- **输入统计**：今日字数/次数/速度/时长，累计字数与使用天数。
- **隐私**：默认日志**不记用户输入**；密码框等敏感输入不接管按键、不弹候选窗。

## 安装

| 产物 | 说明 |
|---|---|
| `XunyinIME-Setup-<版本>-x64.exe` | 安装器：注册 x64 与 x86 两套 TSF、建快捷方式、写卸载项，装完打开设置中心 |
| `XunyinIME-<版本>-portable.zip` | 解压后以管理员运行 `register-ime.ps1` 注册输入法 |

切换输入法：语言栏选「中文（简体）→ 迅音输入法」，或 Win+Space。
卸载：安装版走「设置 → 应用」，或安装目录里的 `uninstall.exe`（支持 `--silent`、`--keep-user-data`）；
便携版用 `register-ime.ps1 -Unregister`。

## 目录结构

```
xunyin-svc/          常驻服务：librime 引擎 + 全部输入逻辑（可随时重启）
xunyin-tsf/          TSF 文本服务 DLL：按键路由、组合串写回、候选窗、状态栏
xunyin-rime/         librime 封装（FFI / 会话 / 重载 / 方案清单），服务与设置中心共用
xunyin-ime/native/   设置中心（原生 Win32 自绘，无 WebView）
xunyin-ime/rime/     Rime 运行时数据（词库、方案、rime.dll）
tools/               部署、打包、资源生成脚本与调试工具
installer/           安装器（清单在 installer/app.toml）
```

## 构建

要求：Rust（MSVC 工具链）+ `rustup target add i686-pc-windows-msvc` + VS Build Tools（C++ 桌面负载）。
**不需要 Node** —— 设置中心是原生 Win32 程序。

```powershell
.\tools\deploy-tsf.ps1 -Arch x64        # 编译 + 签名 + 注册 TSF（-Arch x86 是给 32 位程序用的那份）
.\tools\deploy-settings.ps1             # 设置中心（release）
.\tools\pack-installer.ps1              # 出安装包；版本号取自 xunyin-ime\native\Cargo.toml
.\tools\pack-installer.ps1 -SkipBuild -Sign -PfxPath <证书> -PfxPassword <口令>   # 签名版
```

部署脚本把产物放在固定的 `build\dev-deploy\`，注册表只指向那里（不指向 `target\`）：TSF 会把 DLL
加载进**每个正在输入的应用**，路径一变就会出现"注册表已是新 DLL、进程还持着旧的"的错位。
**改 DLL 代码仍需完全退出重开应用**；改词库/方案/配置/服务都不用。

## 什么能热更新，什么必须重开应用

| 改什么 | 怎么生效 |
|---|---|
| 词库、输入方案、模糊音、每页候选数 | 设置中心「重新部署」→ 后台重建索引后立刻生效 |
| 外观、按键行为、候选来源开关 | 保存即写重载请求 → 立刻生效 |
| 状态栏/候选框的皮肤资源 | 改文件 + 「重载配置」→ 立刻生效 |
| **输入逻辑**（重编 `xunyin-svc.exe`） | 服务自动换新 → **立刻生效，不用重开应用** |
| **DLL 里的代码**（TSF 交互、候选窗/状态栏绘制） | 必须完全退出重开应用（或注销重登） |

服务是怎么被换掉的：DLL 定时比对"服务 exe 的写入时刻 vs 运行中服务进程的启动时刻"，发现磁盘上的
exe 更新就杀掉旧服务、按新 exe 重启，宿主无感；协议只做最低版本校验（服务比 DLL 新可直接用）。
服务超时/断开时那颗键**交给本进程逻辑处理（绝不吞键）**，并暂停转发 3 秒后自动重试。
服务无客户端 60 秒自动退出；同一条管道已有服务时新进程直接退出（单实例，避免两份引擎抢用户词库）。

## 调试与测试

```powershell
.\tools\svc-probe.ps1 -Keys "N,I,H,A,O,SPACE" -Times 3       # 不依赖宿主应用直接驱动服务
.\tools\svc-probe.ps1 -Keys "N,I,SELECT:1,CMD:toggle_mode"  # 选词与命令
cargo test                                                  # 各 crate 的单元测试
cargo test --lib -- preedit:: keycode:: config:: quick_input:: stats::   # 跳过要完整部署的引擎用例
```

`xunyin-rime` 的 `rime_engine::tests` 首次运行要完整部署一次词库（数分钟，之后走缓存）。
`tools/tsf_test.html` 是浏览器里的候选跟随测试页；崩溃日志里的 RVA 用
`tools/disasm_rva.py <exe> 0x<RVA>` 反汇编。

## 数据与维护

数据目录 `%APPDATA%\com.xunyin.ime\`：`config.json`、`stats.json`、`custom_phrase.txt`（自定义短语）、
`import_dict.txt`（导入的词库）、`skins\`、`logs\`、`reload.request`（重载标记）。

- **日志**：默认不记用户输入（只记长度/数量）。排查输入问题：`New-Item "$env:APPDATA\com.xunyin.ime\log-input" -Force`
  （5 秒内生效），或设 `XUNYIN_LOG_INPUT=1`。日志在 `logs\tsf.log`（宿主 DLL）与 `logs\svc.log`（服务）。
- 设置中心「关于与维护」→ **备份与诊断**：导出诊断包、导出/导入全部设置、恢复内置皮肤；
  **维护**：打开数据/日志/皮肤目录、清空日志、恢复默认设置、重载配置、重新部署。
- 词库管理：细胞词库开关、导入词库（txt/scel，含整个文件夹）、导入/导出短语、
  检查词库更新、上游词库更新（下载到暂存 → 部署 → 回滚）、导出/清空用户词库。

## 自动构建

[.github/workflows/build.yml](.github/workflows/build.yml) **手动触发**（不挂 push）：编译 x64+x86 的
TSF/服务与设置中心 → 打包 x64 安装包与便携 zip → 发布 Release（标签 `v0.1.<构建序号>`，文件名一律英文）。
仓库 secrets 里配 `SIGN_PFX_BASE64` / `SIGN_PFX_PASSWORD` 即自动签名（不配则跳过，装时会有 SmartScreen 提示）。

## 待办

- **候选窗/状态栏/符号面板搬进服务进程**：改界面也不用重开应用；顺带解决"提权应用（管理员程序、
  部分游戏）里浮层画不出来"——那需要 `uiAccess` 清单。
- **游戏兼容**：装到 `System32\IME\<名字>\` + 代码签名（CS2 这类 Trusted Mode 只放行系统目录里的签名 DLL）。
- 多显示器 / 混合 DPI 场景的进一步打磨。

## 第三方

- [Rime/librime](https://github.com/rime/librime)（LGPL）与内置 Rime 词库数据，见
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
- 安装器派生自 [huanfeng/wind-installer](https://github.com/huanfeng/wind-installer)，本地改动见
  [installer/PATCHES.md](installer/PATCHES.md)。
