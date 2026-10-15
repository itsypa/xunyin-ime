# 本地对 wind-installer 的修改

本目录是 [wind-installer](https://github.com/huanfeng/wind-installer)（MIT）的副本，
为**迅音输入法**的需求做了以下修改。上游升级时请对照本文合并。

## 1. `src/installer/ime.rs`

| 改动 | 原因 |
|---|---|
| 注册路径一律使用**反斜杠**（清单 `dll_x64`/`dll_x86`/`acl_dlls`） | 32 位 `regsvr32`（SysWOW64）无法处理混用正斜杠的路径：`tsf-x86/xunyin_tsf_x86.dll` 会被当成开关，注册静默失败（实测退出码 3），改反斜杠即成功 |
| 32 位进程注册 x64 视图时走 `%WINDIR%\Sysnative\regsvr32.exe` | 32 位进程访问 `System32` 会被 WOW64 重定向到 SysWOW64，永远注册不了 x64 视图 |
| ~~32 位 Windows 上跳过 x64 注册~~（已删除） | 安装器只发 x64 且在 32 位系统上直接退出（`main.rs` 要求 64 位系统），这条分支不可达；32 位**程序**的支持由 `tsf-x86` 承担，与系统位数无关 |

## 2. `tools/packer/main.rs`

| 改动 | 原因 |
|---|---|
| 新增 `is_pe32()`；对 32 位 stub/卸载器**跳过 PE 版本信息与图标注入** | 实测 editpe 在 PE32 下会把 `OptionalHeader.SizeOfImage` 写坏（归零后只加新节大小），产物被 Windows 判为「不是有效的 Win32 应用程序」。跳过注入后归档/清单追加照常，功能不受影响，仅少自定义图标与版本号 |

## 2.5 安装目录 / 数据目录（2026-10-15 一轮实机安装发现的问题）

| 改动 | 原因 |
|---|---|
| `src/installer/config.rs`：`InstallConfig::default()` 的默认路径改为展开清单 `[paths]`（install/portable/data） | 上游写死 `%ProgramFiles%\{app.id}` / `%APPDATA%\{app.id}`，而向导界面上显示的却是 `[paths]` 里的值 —— 用户看到"装到 `C:\Program Files\XunyinIME`"，实际落在 `...\com.xunyin.ime`（实机安装时确认过） |
| `src/ui/install_wizard.rs`：`finalize_picked_folder()` 不再自动追加 `app.id` 子目录 | 用户选了目录之后又被套一层 `com.xunyin.ime`（反向域名当文件夹名，不像给人看的）：选了 `D:\Apps` 结果装到 `D:\Apps\com.xunyin.ime`。现在"选的目录就是安装目录" |
| `src/ui/install_wizard.rs`：清单没有 `[datadir]` 段时**不显示**"数据目录"输入框，改为一行说明 | 没有 `[datadir]` 时安装器不写数据目录配置（`plan.rs` 有判断）、主程序也不读它，那个输入框改了不生效 —— 是个会误导人的假控件 |
| `src/util/path.rs`：新增 `expand_path()`，向导里的同名实现改为调用它 | 界面与安装配置各写一份 `%VAR%` 展开时，两边的默认路径会悄悄分叉（就是上一条问题的成因之一） |

### 2.6 目录语义：选父目录 → 程序 `<父>\XunyinIME`、数据 `<父>\Data`（2026-10-15 定稿）

| 改动 | 原因 |
|---|---|
| `src/meta.rs`：新增 `install_folder_name()`（取 `[paths].install` 的末段，即 `XunyinIME`） | 追加子目录时用的应该是**产品名**，不是 `app.id`（反向域名不像给人看的文件夹名） |
| `src/ui/install_wizard.rs`：`finalize_picked_folder()` = `<所选>\XunyinIME`；新增 `sibling_data_dir()` = 安装目录的**父目录**下的 `Data`；选完安装目录后**同步**更新数据目录栏 | 与产品定稿一致：默认 `%ProgramFiles%\XunyinIME` + `%ProgramFiles%\Data`；选 `D:\Program Files` 则 `D:\Program Files\XunyinIME` + `D:\Program Files\Data` |
| `src/ui/install_wizard.rs`：数据目录栏改为只读（去掉"更改"按钮） | 数据目录由安装位置推导，避免"改了不生效/两边不一致"；要单独指定用 `--datadir` |
| `src/main.rs`：`--dir` 保持**安装目录本身**（不追加），未给 `--datadir` 时数据目录取 `<安装目录的父目录>\Data` | `--dir` 还被应用内升级使用（必须原地覆盖，追加会装出第二份）；数据目录则与程序并排，和向导的结果一致 |
| `src/installer/acl.rs` + `steps.rs`：新增 `grant_users_modify_tree()`，写完数据目录配置后给数据目录授 `Users:(OI)(CI)M` | 数据目录默认落在 `%ProgramFiles%\Data`，只有管理员可写；而输入法以普通用户身份运行（写配置/词库/日志）——不授权就是"设置存不上、学过的词不生效" |

## 3. `vendor/windui`（依赖）

GUI 库 `windui` 0.16.1 在 i686 下编译不过，本地副本已修：
`SetWindowLongPtrW` 的 `isize`/`i32` 与 `NOTIFYICONDATAW` packed 字段引用两类错误。
详见 `vendor/windui/PATCHES.md`。

## 4. `vendor/editpe`（依赖）

上游 0.2.3 的 `VersionInfo::build()` 用 UTF-8 字节数计算版本资源头部长度，
含中文时结构错乱（Windows 读不到版本信息）。本地副本已改为 UTF-16 码元数。
