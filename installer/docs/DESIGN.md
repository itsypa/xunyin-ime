# Wind Installer 设计文档

## 1. 项目概述

基于 `wind-ui-rust` GUI 框架的轻量级 Windows 安装器**生成工具**。

**设计理念**：类微信输入法风格，1-2 步完成安装。应用身份、安装行为、输入法/字体等
专属逻辑全部由打包时提供的 `app.toml` 描述，序列化进归档头部，由安装/卸载器运行期读取。
**同一个预编译 stub（wind-installer.exe）配不同 app.toml 即可为不同应用生成安装包，
无需重新编译。** 根目录 `app.toml` 是一份演示用的虚构应用清单，把各能力段的写法都列了出来；
本文档的界面示意图则以真实产品「清风输入法」为例，那是本安装器的第一个使用者。

配置分两层：
- **运行期清单**（`[app]`/`[ui]`/`[ime]`/`[[font]]`）→ 嵌入归档头部，安装/卸载器读取；
- **打包参数**（`[package]`：源目录、压缩、logo/icon 路径）→ 仅 `wind-packer` 使用，不进归档。

输入法 TSF 注册与字体安装为可选段：`[ime]`/`[[font]]` 存在即启用，缺省即跳过——
普通应用删除这两段即可。EXE 图标由 `wind-packer` 用纯 Rust 的 `editpe` 按 `app.toml`
写入（无外部依赖，CI 离线可用）。

### 对标 NSIS 的优势

| 维度 | NSIS | Wind Installer |
|------|------|----------------|
| 二进制体积 | ~100KB stub | ~0.5MB（含 GUI） |
| 运行时内存 | ~5MB | ~3.6MB |
| UI 定制性 | 脚本驱动，有限 | Rust 原生，完全可控 |
| 压缩算法 | Zlib/LZMA | Zstd + LZMA 双支持 |
| 安装步骤 | 3-5 步向导 | 1-2 步极简流程 |

## 2. 物理架构

安装包由两部分拼接：

```
┌─────────────────────────┐  ← 文件起始
│   Stub (installer.exe)  │  ← 自解压程序（GUI + 解压逻辑）；图标已由 editpe 写入
│   ~0.5 MB               │
├─────────────────────────┤  ← Stub 结束位置
│   Solid Compressed Block│  ← 全部文件拼成一条流整体压缩（跨文件复用字典）
├─────────────────────────┤
│   Header                │
│   - Magic: "WINDPKG\0"  │
│   - version / 压缩类型    │
│   - manifest（运行期清单）│  ← v3：app.toml 的 [app]/[ui]/[ime]/[[font]] 的 TOML 文本
│   - logo（UI 图片字节）   │  ← v3：UI 显示的 logo，未压缩，廉价读取
│   - 文件路径/偏移/大小表  │
├─────────────────────────┤
│   Footer (16 bytes)     │
│   - Header offset (u64) │
│   - Magic: "WINDEND\0"  │
└─────────────────────────┘  ← 文件结束
```

> **打包管线**：`wind-packer build` 先用 `editpe` 给**干净 stub**写入图标，再压缩源目录、
> 嵌入 manifest/logo 到头部，最后拼接 `stub + 压缩块 + Header + Footer`。
>
> ⚠️ 「写图标」必须在「有尾部 overlay」**之前**。`editpe` 并不会丢弃 overlay（它原地改
> 字节数组、`write_file` 直接写出），但资源节放不下时会**新增一个节**，插在所有节之后、
> 尾部数据之前 —— overlay 于是被整体后移，而 Footer 里的 `header_offset` 是**绝对偏移**，
> 一挪就对不上，归档从此打不开。顺序反了不会有任何报错，只有「装到一半读不出清单」。
>
> 卸载器走同一套结构、但只有头部：`wind-packer prep-uninstaller` 在**进归档之前**给它写
> 版本信息与图标、并追加一个「0 条目、只含 manifest + logo」的 overlay，于是它自包含，
> 安装目录不需要任何散落的配置文件。这一步单独暴露成子命令是为了给代码签名腾位置——
> 加工完即为终态，签完就不能再改（见 §5.2.1）。`build` 在没人提前 prep 时会自己补做。

## 3. 归档格式详细规范

### 3.1 Footer（文件末尾 16 字节）

```
Offset  Size  Field
0       8     header_offset: u64 (LE)  — Payload Header 起始偏移
8       8     magic: "WINDEND\0"
```

### 3.2 Header（v3）

```
Offset  Size       Field
0       8          magic: "WINDPKG\0"
8       4          version: u32 (LE) — 当前版本 = 3
12      1          compression: u8  — 0=Zstd, 1=LZMA
13      4          manifest_len: u32 (LE)            ← v3 新增
17      manifest_len  manifest: TOML 文本（运行期清单 AppManifest）
+0      4          logo_len: u32 (LE)                ← v3 新增
+4      logo_len   logo: 图片字节（UI 显示，可为 0）
+0      4          entry_count: u32 (LE)
+4      8          solid_compressed_size: u64 (LE)   — 固实压缩块字节数
+12     ...        entries: [Entry; entry_count]
```

manifest 与 logo 位于头部、未压缩，运行期无需解压固实块即可廉价读取（启动时显示 UI）。

### 3.3 Entry

```
Offset  Size      Field
0       2         path_len: u16 (LE)
2       path_len  path: UTF-8 bytes（相对路径，如 "wind_tsf.dll"）
+0      8         offset: u64 (LE) — 在**解压后数据流**中的字节起点（固实压缩，与文件偏移无关）
+8      8         compressed_size: u64 (LE) — 固实模式下恒为 0
+16     8         original_size: u64 (LE)
+24     4         crc32: u32 (LE) — 原始数据的 CRC32
```

> **固实压缩（v2 起）**：所有文件原始数据拼成一条流整体压缩一次，压缩器跨文件复用字典，
> 对含大量相似文件（词典 YAML 等）的归档压缩率显著优于逐文件压缩。`entry.offset` 因此是
> 解压后流中的偏移，与 stub 大小无关——`bundle` 拼接时头部字节可原样复制，仅需更新 Footer
> 的 `header_offset`（加上 stub 大小）。

## 4. 压缩算法

### Zstd（默认）
- 解压速度极快（~500 MB/s）
- 内存可控
- 压缩级别：19（高压缩率）

### LZMA（可选）
- 压缩率最高
- 解压较慢但压缩包更小
- 适合对体积极度敏感的场景

## 5. 安装流程

### 5.1 编排架构

安装流程不是一条写死的语句序列，而是由清单**声明式装配**出的步骤计划：

```
app.toml ──► AppManifest ──► plan::plan_install(manifest, mode) ──► Vec<Box<dyn Step>>
                                                                          │
                                                    step::run_plan(&mut dyn Reporter)
                                                                          │
                                                    ┌─────────────────────┴──────────────┐
                                              CliReporter                          GuiReporter
                                            (--silent, stderr)              (向导，channel → 进度条)
```

- **`plan_install` 是「装什么、按什么顺序装」的唯一真相**。GUI 与静默路径共用同一份计划，
  差异仅在 `Reporter` 实现——历史上两条链各自手写，已漂移出缺陷。
- **能力段缺省 = 该步骤不入计划**：`[ime]`/`[[font]]`/`[autostart]`/`[[shortcut]]`/
  `[startup]`/`[datadir]` 任一缺省，对应步骤不会出现在计划里。新增能力 =
  加一个 `impl Step` + 在 planner 里加一行门控。
- **失败语义**由 `Step::fatal()` 决定：解压类步骤失败即中止；注册类步骤失败仅记警告并继续。
- 进度按「已完成步骤数 + 步内比例」折算，而非按解压文件数。

### 5.2 标准安装（步骤计划）

按 `plan_install` 装配顺序，括号内为门控条件（无标注则无条件）：

1. **写 InstallerRunning 标志**：防止宿主进程在安装期间被其他组件重新拉起
2. **停止进程**（`app.process_names` 非空）
3. **反注册旧 COM**（含 `[ime]`）
4. **清理旧版遗留文件**（`legacy_files`/`legacy_dirs` 非空）
5. **解压数据 + 释放文件**：致命步骤，失败即中止
6. **追加卸载器清单 overlay**（uninstall.exe 尚未自带 overlay 时）：旧包保底，
   见 §5.2.1
7. **设置权限**（`acl_dlls` 非空）：ALL APPLICATION PACKAGES 读取执行
8. **安装字体**（含 `[[font]]`）
9. **注册 COM + 注册输入法**（含 `[ime]`）
10. **配置自启动**（含 `[autostart]` 且 `enabled`）：HKCU Run 键
11. **注册 URL 协议**（`url_protocol` 非空）
12. **创建快捷方式**（含 `[[shortcut]]`）：开始菜单 / 桌面
13. **写入卸载信息**：Add/Remove Programs 注册表项
14. **写数据目录配置**（含 `[datadir]` 且首次安装）
15. **预启动**（含 `[startup]` 且 `prestart`）
16. **清除 InstallerRunning 标志**

「是否首次安装」按**机器**判定（注册表 `DisplayVersion`），而非按安装目录——
`datadir.conf` 写在 `%LOCALAPPDATA%` 是机器全局的，两者维度必须对齐。

#### 5.2.1 第 6 步为什么带门控：卸载器要能签名

overlay 的内容（清单 + logo）全部来自 `app.toml`，没有一个字节依赖安装期，因此它现在
由**打包器在构建机上**追加（`wind-packer prep-uninstaller`），第 6 步正常情况下什么
都不做。

前移的理由是代码签名：Authenticode 要求证书表是 PE 的最后一段（`offset + size == 文件
长度`），装机端只要往尾部追加一个字节，签名就变成 "No signature found"。只要 overlay
还在安装期追加，`uninstall.exe` 就**永远签不了**。前移之后顺序变成
`prep-uninstaller → 签名 → 进归档 → 装机端只解压`，签名一路完好。

第 6 步保留下来只为**旧包**：老版打包器产出的安装包里，`uninstall.exe` 仍是裸 stub，
不补一次就读不到清单、启动即失败。门控判据是 `archive::has_manifest_overlay()`——
它必须挡在追加之前，否则会把打包期签好的名毁掉。

### 5.3 便携安装

计划恒为三步，**不触碰系统任何位置**（即便清单声明了输入法/字体/自启动）：

1. 解压数据
2. 释放文件（**跳过 uninstall.exe**——便携版检测到该文件会误判为安装版）
3. 写入 `app.portable_marker` 标记文件

## 6. 卸载流程

### 6.0 回执驱动

卸载**不读清单**，只按安装时写下的「回执」反向回放。

安装期每个有系统副作用的步骤，把**做成的具体产物**记进回执（绝对路径、TSF profile
字符串、注册表键全名），计划末尾由 `PersistReceipt` 落盘到
`HKLM\Software\{app_id}` 的 `Receipt` 值（TOML 文本，不落安装目录以维持
「安装目录不留散落文件」）。卸载时 `UndoReceipt` 逆序撤销每一条。

相对「手写镜像卸载脚本」的三个好处：

| 场景 | 手写镜像 | 回执驱动 |
|---|---|---|
| 安装中途失败 | 按清单猜，撤销从未做成的事 | 没做成就没进回执，不会撤销 |
| 升级改了清单（如 `display_name`） | 按新清单找旧键，找不到 → 残留 | 回执记的是旧键全名，照样清掉 |
| 新增一种能力 | 要记得在卸载侧加对应反操作 | 安装侧记回执即可，卸载侧自动支持 |

因此 `plan_uninstall` 对任何应用都是同一份计划——卸载器不需要知道 IME、字体是什么。

**顺序上的硬约束**（`tests/step_plan.rs` 断言守护）：

- 回执落盘必须晚于所有有副作用的步骤，否则最后几步的产物漏记；
- 撤销必须早于删安装文件（反注册 COM 需要 DLL 还在盘上）；
- 删应用注册表键必须晚于撤销（回执就存在那个键里）；
- **需要「解析后才知道、撤销后就问不到」的信息，必须在撤销前固化**——
  用户数据目录由 `UninstallCtx::new` 提前解析，因为 `UndoReceipt` 会删掉
  数据目录配置文件，之后再算就只能拿到默认位置。

### 6.0.1 记账纪律

回执一旦丢账，产物就**永久**卸不掉——没有第二道防线。故写入侧必须和撤销侧一样
「尽力而为」，以下四条是踩过的坑：

1. **先记账，再报错**。步骤内做了 N 件事、第 k 件失败时，前 k-1 件必须已入账。
   `?` 早退会把已做成的部分连同错误一起丢掉（`InstallFonts`/`CreateShortcuts`/
   `RegisterCom` 均逐项记账；底层函数返回「已做成的产物 + 错误」而非 `Result<全部>`）。
2. **按实际存在记账，而非按返回值**。先建键、再写值的操作（ARP 条目、URL 协议）
   中途失败会留下孤儿键——事后查存在性才记得准。
3. **续写旧回执，而非从空起**。安装用 `Receipt::load_or_default()` 读回旧回执再
   续写（`push` 按等值去重）。否则新版删掉某能力段时，旧版装的产物就此失去唯一
   撤销依据。同理，升级时不重写的文件（`datadir.conf`）**仍要记账**——文件归本
   产品所有，卸载时该删。
4. **回执写失败 = 安装失败**。`PersistReceipt` 是致命步骤：与其报告「安装完成」
   却留下一个卸不干净的系统，不如让安装明确失败。

**退化路径**：无回执（便携装/没装过）时跳过撤销、仍删文件；回执损坏时额外发出警告
（有产物却撤销不了，属异常）。两种情况下 `RemoveOwnUninstallInfo` 都会无条件移除
ARP 条目——这是「卸载不读清单」的唯一例外，因为 ARP 键路径与正在运行的卸载器同源，
不存在猜错风险；缺了它，回执失效时应用会永久留在「应用和功能」且卸载按钮点了没反应。

### 6.1 自删除策略

```
uninstall.exe (在安装目录)
    ↓ 复制自身到 %TEMP%\wind_uninstall_<random>.exe
    ↓ 启动新进程，退出当前进程
%TEMP%\wind_uninstall_<random>.exe
    ↓ 执行清理
    ↓ 删除原安装目录的 uninstall.exe
    ↓ 删除自身
```

### 6.2 清理步骤

1. 停止进程（3 阶段：taskkill → PowerShell → REBOOTOK）
2. 移除系统输入法（InstallLayoutOrTip + ILOT_UNINSTALL）
3. 注销 COM（regsvr32 /u）
4. 卸载系统字体
5. 删除安装文件（BackupIfLocked 模式）
6. 清理快捷方式
7. 清理注册表
8. 处理用户数据（可选）

### 6.3 用户数据清理选项

| 数据类型 | 路径 | 默认行为 |
|---------|------|---------|
| 用户配置 | %APPDATA%\WindInput | 可选删除，默认保留 |
| 本地缓存 | %LOCALAPPDATA%\WindInput\cache | 默认删除 |
| WebView2 缓存 | %TEMP%\wind_setting | 始终删除 |
| 数据目录配置 | %LOCALAPPDATA%\WindInput\datadir.conf | 跟随用户配置 |

## 7. UI 设计

### 7.0 清单驱动的部分

界面骨架（页数、控件布局）是内置的，以下三类由清单驱动，换应用无需改代码：

| 清单段 | 作用 | 缺省行为 |
|---|---|---|
| `[theme]` | 主题色，`"#RRGGBB"` 或 `"RRGGBB"` | 逐项回退到内置中性蓝色系；**非法值静默回退**，不让安装器起不来 |
| `[paths]` | 默认路径模板，`{id}` 替换为 `app.id` | `%ProgramFiles%\{id}` / `%USERPROFILE%\{id}` / `%APPDATA%\{id}` |
| `[strings]` | 会泄漏应用领域的文案 | 回退到中性默认（不含「输入法」「词库」字样） |

`[strings]` 只收录领域相关文案；「安装路径」「更改」「立即安装」这类通用 chrome
保持内置，避免把清单撑成一张翻译表。默认文案的领域中性由 `tests/ui_defaults.rs`
断言守护——普通应用装出来的界面不该出现输入法词汇。

下文示意图以清风输入法为例——它的清单里 `[strings]` 显式声明了输入法专用措辞，
正好演示「领域文案从哪来」。仓库自带的 `app.toml` 是另一份虚构示例，措辞不同。

### 7.1 安装界面（3 页）

**Page 1 - 安装类型选择**
```
┌─────────────────────────────────────────────┐
│  [图标]  清风输入法 安装程序                  │
│                                             │
│  请选择安装方式：                             │
│                                             │
│  ○ 标准安装（推荐）                           │
│    注册输入法到系统，开机自动启动              │
│                                             │
│  ○ 便携模式                                  │
│    仅解压文件，不修改系统                      │
│                                             │
│  安装路径: [C:\Program Files\WindInput]      │
│                                             │
│           [ 下一步 ]  [ 取消 ]               │
└─────────────────────────────────────────────┘
```

**Page 2 - 数据目录 + 安装进度**
```
┌─────────────────────────────────────────────┐
│  [图标]  清风输入法 安装程序                  │
│                                             │
│  数据存储位置：                               │
│  ● 默认位置 (%APPDATA%\WindInput)           │
│  ○ 自定义位置: [________________] [浏览]     │
│                                             │
│  正在安装...                                 │
│  ████████████████░░░░░░░░  68%              │
│                                             │
│  正在释放文件: wind_input.exe                │
│                                             │
│           [ 取消 ]                           │
└─────────────────────────────────────────────┘
```

**Page 3 - 完成**
```
┌─────────────────────────────────────────────┐
│  [图标]  清风输入法 安装完成                  │
│                                             │
│  ✓ 清风输入法 已成功安装到您的电脑           │
│                                             │
│  [x] 立即启动 清风输入法 设置                │
│                                             │
│           [ 完成 ]                           │
└─────────────────────────────────────────────┘
```

### 7.2 卸载界面（2 页）

**Page 1 - 确认 + 用户数据选项**
```
┌─────────────────────────────────────────────┐
│  [图标]  清风输入法 卸载程序                  │
│                                             │
│  确定要卸载 清风输入法 吗？                   │
│                                             │
│  用户数据处理：                               │
│  [ ] 清除用户配置数据（输入状态、自定义短语）  │
│      %APPDATA%\WindInput                    │
│  [x] 备份配置数据到桌面（推荐）               │
│  [x] 清除本地缓存数据（词库缓存）             │
│      %LOCALAPPDATA%\WindInput\cache         │
│                                             │
│  [x] 我已阅读并确认卸载                      │
│                                             │
│           [ 卸载 ]  [ 取消 ]                 │
└─────────────────────────────────────────────┘
```

**Page 2 - 完成**
```
┌─────────────────────────────────────────────┐
│  [图标]  清风输入法 卸载完成                  │
│                                             │
│  ✓ 清风输入法 已从您的电脑中移除              │
│                                             │
│  ⚠ 部分文件正被占用，需重启电脑才能彻底清除    │  ← 仅 need_reboot 时
│    已排入系统清理队列，重启后将自动删除        │
│                                             │
│           [ 完成 ]                           │
└─────────────────────────────────────────────┘
```

重启提示**只提示不代劳**：不提供「立即重启」按钮——安装/卸载器无从判断用户手头
有没有没保存的工作。删不掉的文件已通过 `MoveFileEx(DELAY_UNTIL_REBOOT)` 排入
系统队列，用户何时重启都能清掉。安装完成页同理（见 `ui::install_wizard`），
且 `--quiet` 模式在这种情况下**放弃自动退出**，把窗口留给用户亲手关闭——
自动升级本就发生在用户没盯着屏幕的时候，2 秒后自弹自灭等于把提示扔了。

## 8. 项目结构

```
wind-installer/
├── Cargo.toml
├── build.rs                   # 嵌入 UAC Manifest + 图标
├── assets/
│   ├── app.manifest           # requireAdministrator
│   └── installer.ico
├── src/
│   ├── main.rs                # 入口：检测模式（安装/卸载/打包）
│   ├── lib.rs
│   ├── archive/               # 归档格式与压缩
│   │   ├── mod.rs
│   │   ├── format.rs          # 结构定义
│   │   ├── reader.rs          # 流式读取解压
│   │   └── writer.rs          # 打包写入
│   ├── installer/             # 安装逻辑
│   │   ├── mod.rs
│   │   ├── config.rs          # 安装配置
│   │   ├── extract.rs         # 文件释放
│   │   ├── registry.rs        # 注册表操作
│   │   ├── shortcut.rs        # 快捷方式
│   │   ├── font.rs            # 字体安装
│   │   ├── acl.rs             # 权限设置
│   │   ├── process.rs         # 进程管理
│   │   └── ime.rs             # TSF/COM 注册
│   ├── uninstaller/           # 卸载逻辑
│   │   ├── mod.rs
│   │   ├── cleanup.rs         # 清理逻辑
│   │   └── selfdelete.rs      # 自删除
│   ├── ui/                    # 安装界面
│   │   ├── mod.rs
│   │   ├── theme.rs           # 主题
│   │   ├── install_wizard.rs  # 安装向导
│   │   └── uninstall_wizard.rs# 卸载向导
│   └── util/                  # 工具函数
│       ├── mod.rs
│       ├── admin.rs           # UAC 权限
│       ├── single.rs          # 单实例
│       └── path.rs            # 路径工具
├── tools/
│   └── packer/                # 打包工具
│       ├── Cargo.toml
│       └── main.rs
└── docs/
    └── DESIGN.md
```

## 9. 依赖选型

```toml
[dependencies]
windui = { path = "../wind-ui-rust" }
zstd = "0.13"                  # Zstd 解压
xz2 = "0.1"                    # LZMA 解压
windows = { version = "0.58", features = [...] }
winreg = "0.52"                # 注册表封装
mslnk = "0.1"                  # 快捷方式
crc32fast = "1.4"              # CRC32 校验
serde = { version = "1", features = ["derive"] }
toml = "0.8"                   # 配置文件
clap = { version = "4", features = ["derive"] }

[build-dependencies]
embed-resource = "2.4"         # 嵌入 Manifest
```

## 10. Anti-Virus 对策

| 策略 | 实现 |
|------|------|
| 代码签名 | signtool Authenticode 签名 |
| Payload 混淆 | 简单 XOR 混淆避免特征码 |
| 行为透明 | UI 反馈所有操作 |
| 标准 API | 使用官方 windows crate |

## 11. 开发路线

| 阶段 | 内容 | 工作量 |
|------|------|--------|
| Phase 1 | 归档格式 + 流式解压 + 打包工具 | 2-3 天 |
| Phase 2 | 安装逻辑（文件 + 注册表 + TSF） | 3-4 天 |
| Phase 3 | GUI 界面（基于 windui） | 2-3 天 |
| Phase 4 | 卸载器 + 自删除 | 2 天 |
| Phase 5 | 测试 + 签名集成 | 2-3 天 |
