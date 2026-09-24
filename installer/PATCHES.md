# 本地对 wind-installer 的修改

本目录是 [wind-installer](https://github.com/huanfeng/wind-installer)（MIT）的副本，
为**迅音输入法**的需求做了以下修改。上游升级时请对照本文合并。

## 1. `src/installer/ime.rs`

| 改动 | 原因 |
|---|---|
| 注册路径一律使用**反斜杠**（清单 `dll_x64`/`dll_x86`/`acl_dlls`） | 32 位 `regsvr32`（SysWOW64）无法处理混用正斜杠的路径：`tsf-x86/xunyin_tsf_x86.dll` 会被当成开关，注册静默失败（实测退出码 3），改反斜杠即成功 |
| 32 位进程注册 x64 视图时走 `%WINDIR%\Sysnative\regsvr32.exe` | 32 位进程访问 `System32` 会被 WOW64 重定向到 SysWOW64，永远注册不了 x64 视图 |
| 32 位 Windows 上跳过 x64 注册（`is_64bit_windows()` 判定） | 32 位系统里 x64 DLL 无法加载，注册必然失败，徒留告警 |

## 2. `tools/packer/main.rs`

| 改动 | 原因 |
|---|---|
| 新增 `is_pe32()`；对 32 位 stub/卸载器**跳过 PE 版本信息与图标注入** | 实测 editpe 在 PE32 下会把 `OptionalHeader.SizeOfImage` 写坏（归零后只加新节大小），产物被 Windows 判为「不是有效的 Win32 应用程序」。跳过注入后归档/清单追加照常，功能不受影响，仅少自定义图标与版本号 |

## 3. `vendor/windui`（依赖）

GUI 库 `windui` 0.16.1 在 i686 下编译不过，本地副本已修：
`SetWindowLongPtrW` 的 `isize`/`i32` 与 `NOTIFYICONDATAW` packed 字段引用两类错误。
详见 `vendor/windui/PATCHES.md`。

## 4. `vendor/editpe`（依赖）

上游 0.2.3 的 `VersionInfo::build()` 用 UTF-8 字节数计算版本资源头部长度，
含中文时结构错乱（Windows 读不到版本信息）。本地副本已改为 UTF-16 码元数。
