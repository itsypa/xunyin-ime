# Wind Installer

清单驱动的 Windows 安装器生成器。同一个预编译的 stub 二进制配上不同的 `app.toml`，即可为不同应用生成安装包，无需重新编译。

清单在打包时嵌入归档头部，安装器与卸载器在运行期读取它。应用名称、界面文案、主题色、要注册的组件、要安装的字体等均来自清单，代码中不含具体应用的信息。

## 快速开始

编辑 `app.toml` 描述你的应用，然后运行打包脚本：

```powershell
.\scripts\pack.ps1
```

脚本会编译 stub、卸载器和打包工具，再调用 `wind-packer build` 生成安装程序，输出到 `[package] output_dir` 指定的目录。

已有二进制时也可以直接调用打包工具：

```
wind-packer build --config app.toml --stub wind-installer.exe
```

仓库自带的 `app.toml` 是一份演示用的虚构应用清单，列出了各字段的写法。

## 清单配置

`app.toml` 分两部分：运行期清单会嵌入归档供安装器读取，`[package]` 段仅打包时使用。

能力段缺省即不执行，不需要的段删掉即可：

| 段 | 缺省 | 声明后 |
|---|---|---|
| `[ime]` | 跳过 | 注册 TSF 输入法（COM + InstallLayoutOrTip） |
| `[[font]]` | 跳过 | 安装字体到 `%WINDIR%\Fonts` 并注册 |
| `[autostart]` | 不注册 | 写 `HKCU\...\Run` |
| `[[shortcut]]` | 不创建 | 开始菜单或桌面快捷方式 |
| `[startup]` | 装完不启动 | 装完以 `DETACHED_PROCESS` 启动主程序 |
| `[datadir]` | 不写 | 将向导中选定的数据目录落盘供主程序读取 |
| `[strings]` | 中性文案 | 覆盖为应用自己的措辞 |

只声明 `[app]` 段的应用，安装过程只做解压和写入卸载信息，不改动系统其他位置。修改注册表的行为都需要在清单中显式开启。

图标与 logo 同样由清单指定，打包时写入 PE 资源。

## 命令行

安装器：

| 参数 | 说明 |
|---|---|
| `--silent` | 完全静默，无界面 |
| `--quiet` | 带界面静默，跳过配置页并显示进度，完成后自动退出 |
| `--dir <DIR>` | 安装目录 |
| `--datadir <DIR>` | 数据目录 |
| `--keep-user-data` | 卸载时保留用户数据 |
| `--soft-render` | 强制软渲染，等效于设置 `WIND_SOFT_RENDER=1` |

子命令 `install` / `uninstall` 分别对应安装与卸载模式。

打包工具 `wind-packer` 提供 `pack`、`bundle`、`build`、`inspect` 四个子命令，其中 `build` 为 `pack` 加 `bundle` 的合并操作，`inspect` 用于读取已生成安装程序中嵌入的清单摘要。

## 卸载

安装过程中每个有系统副作用的步骤都会写入一条回执，卸载时读取回执逐条撤销，不重新解析清单。因此用户升级过多个版本、清单已经变化时，卸载撤销的仍是当初安装的内容。

卸载器是独立的二进制 `uninstall.exe`，安装时写进注册表 ARP 键：

| 值 | 内容 | 行为 |
|---|---|---|
| `UninstallString` | `"<安装目录>\uninstall.exe"` | 交互式向导（控制面板 / 设置里点「卸载」走这条） |
| `QuietUninstallString` | `"<安装目录>\uninstall.exe" --silent` | 无人值守，无任何界面（winget / SCCM 走这条） |

`uninstall.exe` 认两个参数：`--silent`、`--keep-user-data`（静默时保留 `%APPDATA%\<app.id>`）。
不认识的参数**只跳过它自己**，后面的照常解析 —— 存量机器的 ARP 条目是老版本安装器写的，
里面有一个已经废弃的 `--uninstall`，新卸载器必须能被那些条目正常调起。

## 文件占用与重启

升级时旧文件常被占用。此时安装器将其改名让路，并用 `MoveFileEx(DELAY_UNTIL_REBOOT)` 排队删除，新版本照常安装。这种情况下安装已完成，但需要重启才能清理干净：交互式向导会显示提示，`--silent` 模式以 3010 退出。

安装器只提示，不会自动重启系统。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 失败 |
| 5 | 什么都没做：需要管理员权限（`ERROR_ACCESS_DENIED`）。**只有静默卸载**会返回它 |
| 1618 | 什么都没做：另一个安装/卸载实例正在运行（`ERROR_INSTALL_ALREADY_RUNNING`） |
| 3010 | 成功，但需重启以完成清理（`ERROR_SUCCESS_REBOOT_REQUIRED`） |

3010 表示安装成功，取值与 MSI、NSIS 一致。调用方若只判断 `exit == 0`，会把需要重启的情况误判为安装失败。

5 同样表示**本次没有做任何改动**。`--silent` 的语义是「不产生任何 UI」，而 UAC 提示框
就是 UI —— 静默调用未提权时弹它，等于把一个无人值守的部署任务挂在那里等人点。

它的适用范围是**静默卸载**（`uninstall.exe --silent` 与 `wind-installer.exe uninstall --silent`），
不含静默**安装**：`wind-installer.exe --silent` 未提权时照旧弹 UAC 并以 0 退出。那不是遗漏——
应用内自动升级依赖这个行为（`request_elevation` 会转发原始参数给提权后的新实例），
改成 5 会当场打断升级。交互式路径全都照常弹 UAC。

1618 表示**本次没有做任何改动**，重试即可。它取代了从前那个「被单实例锁挡住就以 0 退出」的行为
——那个行为会让批量部署脚本把「什么都没干」记成一次成功的安装。被挡时的详情
（占着锁的进程号、锁文件路径）写在 `%TEMP%\wind_installer_args.log`。

## 构建与测试

```
cargo build
cargo test --tests
cargo clippy --lib
```

`cargo test` 包含 doctest 时会有一个既有失败：`src/archive/format.rs` 的文档注释中有一张 ASCII 结构图，rustdoc 会将其当作代码块编译。用 `--tests` 跳过。

Windows 目标通过 `.cargo/config.toml` 静态链接 MSVC CRT，产物不依赖 VC++ 运行库。

## 发布产物

推送 `v*` tag 触发 CI 构建并创建 Release：

| 资产 | 说明 |
|---|---|
| `wind-installer-windows-x64.exe` | stub，打包时由 packer 追加归档 |
| `wind-uninstaller-windows-x64.exe` | 卸载器，打包前注入源目录，随安装包解压到安装目录 |
| `wind-packer-windows-x64.exe` | 打包工具（Windows） |
| `wind-packer-linux-x64` | 打包工具（Linux） |
| `SHA256SUMS` | 上述文件的校验和 |

资产名不含版本号，版本由 tag 表达：

```
gh release download v0.1.0 -p 'wind-installer-windows-x64.exe'
```

stub 与卸载器运行在用户机器上，静态链接 MSVC CRT，CI 在发布前会检查产物确认这一点。打包工具只在构建机运行，不作此要求。

## 贡献

欢迎 Bug 报告、功能建议与代码贡献，开发环境、自检命令与硬性约束见 [CONTRIBUTING.md](CONTRIBUTING.md)。首次提交 PR 前需签署 [CLA](CLA.md)。

提功能建议前请留意本仓库的通用性定位：新能力应当表现为一个新的清单字段或段，且遵循「缺省即不执行」——不声明它的应用行为完全不受影响。只服务于单一应用的需求，通常更适合在调用方解决。

使用清风输入法时遇到的问题（候选、编码、词库、界面）请到 [WindInput](https://github.com/huanfeng/WindInput/issues) 反馈，除非能确认问题出在安装或卸载过程本身。

## 安全

安装器以管理员权限运行并改动系统状态。发现安全问题请勿公开提 Issue，改用 GitHub 的私密漏洞报告渠道，详见 [SECURITY.md](SECURITY.md)。

## 许可

MIT，见 [LICENSE](LICENSE)。

GUI 使用 [windui](https://github.com/huanfeng/wind-ui-rust)。

`vendor/editpe` 是 [editpe](https://crates.io/crates/editpe) 0.2.3 的本地修补副本，BSD-2-Clause，许可证见 `vendor/editpe/LICENSE`。上游的 `VersionInfo::build()` 按 UTF-8 字节数计算版本资源头部长度，含中文时结构错乱导致 Windows 读不到版本信息，副本已改为 UTF-16 码元数。

第三方组件的完整声明见 [NOTICE.md](NOTICE.md)。
