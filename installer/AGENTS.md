# AGENTS.md — wind-installer

## 本仓库的定位（不可动摇的目标）

**wind-installer 是一个「通用 Windows 安装器生成器」，不是清风输入法的专用安装器。**

同一个 stub 二进制（`wind-installer.exe`）配上不同的清单（`app.toml`）即可为**任意应用**打出安装包，无需重新编译：

```
wind-packer build --config <app.toml> --stub wind-installer.exe
```

清单被嵌入归档头部，由安装/卸载器**运行期读取**。因此：

> **一切与具体应用相关的东西都必须写在清单里，绝不能硬编码进 Rust 代码。**

清风输入法只是**第一个使用者**，不是特权使用者。任何"因为我们是输入法所以直接写死"的改动，都是在把通用安装器退化成定制安装器——这是明确要避免的方向。

## 硬性规则

1. **禁止在 Rust 代码里出现**：产品名（"清风输入法"）、具体 GUID/CLSID、DLL 名、"输入法/词库"等领域词、固定安装路径。这些一律从 `meta::manifest()` 取。
   - 领域中性由测试守护：`tests/ui_defaults.rs::defaults_are_domain_neutral`。
2. **能力段缺省 = 该能力不执行**。清单不声明某段（`[ime]`/`[[font]]`/`[autostart]`/…）→ 对应 `Step` 不入计划。一个只有 `[app]` 身份的普通应用，计划里只剩"解压 + 可卸载"。由 `tests/step_plan.rs::minimal_manifest_plans_no_capability_steps` 守护。
3. **有系统副作用的步骤必须写回执**（`ctx.receipt`），否则卸载撤销不掉。卸载是回执驱动的，不重新读清单。
4. **保守默认**：新增的、会改系统（尤其注册表）的行为，默认关闭，由打包器在清单里显式 opt-in。通用安装器不擅自动系统。
5. **界面显示的路径 = 实际生效的路径**，不得在 UI 里按默认模板重新推导一遍。默认模板只是「没有既有状态时用什么」，一旦状态已落盘（如 `datadir.conf`），显示与执行必须出自同一次解析。
   - 「控件置灰」只表达「本次不改这个值」，不表达「这个值是对的」——只读展示的取值来源必须是状态本身。二者恰好在首次安装时重合，故此类缺陷只在二次安装/卸载时暴露。
   - 提示文案与不可逆动作（`remove_dir_all`、备份拷贝）之间尤其不能有第二份推导：用户是照着提示按下按钮的。
   - 由 `tests/wizard_data_dir.rs` 守护（向导初值、卸载提示、实际删除三方一致）。

## 新增一种能力的标准套路

1. `src/manifest.rs`：给对应段/结构加字段（`#[serde(default)]` 保证旧清单兼容）。
2. `src/installer/<capability>.rs`：实现具体逻辑，只从 `meta`/传入的清单结构取参数。
3. `src/installer/steps.rs`：加一个 `impl Step<InstallCtx>`，有副作用就 push 回执。
4. `src/installer/plan.rs`：加一行门控，按清单字段决定是否入计划。
5. `tests/step_plan.rs`：加断言——「缺省不入计划、声明才入计划」（参考 `datadir_conf_is_opt_in`、`ime_residue_sweep_is_opt_in`）。

`plan::plan_install` 是"装什么、按什么顺序装"的唯一真相；GUI 与静默路径共用它，仅 Reporter 不同。不要在步骤内部再按模式/产品分支。

## 锁定文件与「需要重启」

升级时旧文件常被占用（典型是仍被 `ctfmon` 加载的 TSF DLL）。安装器不会因此失败：解压走「改名 `.old_xxxxxxxx` 让路 + `MoveFileEx(DELAY_UNTIL_REBOOT)` 排队」，新版照常就位。但**这个事实必须一路传到用户面前**。

- **记账入口**：`util::reboot::schedule_delete_on_reboot` / `record_pending`。任何「当前删不掉」的路径都要经它们过一遍——它维护一本进程级账本。底层 IO 处没有 `Step` 上下文也没有 `Reporter`，逐层改签名会污染 `extract_entry` 等无关 API，故用账本承接。
- **汇总点**：`step::run_plan` 收尾时读账本，与「步骤失败 + `needs_reboot_on_failure()`」合并成 `RunOutcome::need_reboot`。步骤自身不要去读账本。
- **新增删除逻辑的义务**：凡是可能删不掉文件的新代码，失败分支必须记账，否则「需要重启」的结论会漏判。**不要**只 `eprintln!`——安装器是 `windows_subsystem = "windows"` 的无控制台 GUI 进程，那等于丢弃。

`need_reboot` 的三种去向，一处都不能少：

| 路径 | 行为 |
|---|---|
| 交互式向导 / `--quiet` | 完成页显示 warning 色提示；**quiet 模式放弃自动退出**，窗口留给用户亲手关闭 |
| `--silent`（无界面） | 以 **3010**（`ERROR_SUCCESS_REBOOT_REQUIRED`）退出，并写 `%TEMP%\wind_installer_args.log` |
| 卸载（两条路径同上） | 同上 |

> **调用方契约**：`--silent` 的 `3010` 是**成功**，不是失败。宿主（wind-setting）若只判 `exit == 0` 会把「装好了但请重启」误报成安装失败。
>
> 另有 **1618**（`ERROR_INSTALL_ALREADY_RUNNING`）：被单实例锁挡住，**什么都没做**，重试即可。
> 以及 **5**（`ERROR_ACCESS_DENIED`）：**静默卸载**未提权。它**不弹 UAC**——那是 UI，
> 会把无人值守的部署挂在那里等人点；从前这条路是「弹 UAC + `exit(0)`」，一次连开始都没
> 开始的卸载被记成了成功。
>
> ⚠️ **静默安装不在此列**：`wind-installer.exe --silent` 未提权时仍弹 UAC 并以 0 退出，
> 因为 wind-setting 的应用内自动升级依赖它（`request_elevation` 转发原始 argv 给新实例）。
> 这个不对称是有意的，改动前先看 `uninstaller::require_admin_or_exit` 的注释。
> 从前这条路是 `exit(0)`——静默、无提示、退出码还在说「成功」，于是一个残留的锁能让
> 批量部署把「一次都没装上」记成全部成功。

安装器**只提示、不代劳重启**：它无从判断用户手头有没有没保存的工作。提示用 `theme::warning()` 而非 `error()`——红色会让用户以为装失败而去重装，而重装解决不了任何问题。

## 测试不许碰真的 `PendingFileRenameOperations`

`schedule_delete_on_reboot` / `schedule_dir_on_reboot` 写的是
`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`
——**进程外的全局状态**，开机时由会话管理器**无条件执行**。测试里调到它们，等于在跑
测试的人机器上留下真实的开机删除指令。

这不是假想。2026-09-14 在编译机上实测：一轮 `cargo test --bins` 往注册表写 2 对条目，
**攒到 36 对才被发现**。排的都是 `%TEMP%\wind_guard_<pid>_…`，而 PID 会被系统复用。
污染完全静默，不会有任何东西变红。

**凡是可能走到排队分支的代码，排队动作必须可注入。** 现有的注入口：

| 入口 | 可注入版本 |
|---|---|
| `cleanup::remove_path_with` | 收 `schedule_file` / `schedule_dir` 两个 `&dyn Fn` |
| `cleanup::remove_local_data_entries_with` | 同上 |
| `cleanup::delete_install_files_with` | 同上 |
| `reboot::schedule_dir_on_reboot_impl` | 收 `is_self` + `schedule` |
| `reboot::schedule_dir_on_reboot_with_queue` | 只换排队、保留真 `is_current_exe`（给 `tests/` 用） |
| `font::remove_or_schedule_with` | 收 `schedule` |

**注入不损失任何测试强度**：这些测试断言的是**账本**，而 `schedule_delete_on_reboot`
里是 `record_pending(path, result.is_ok())` —— 无论 `MoveFileExW` 成没成功都记账。
所以断言从来不依赖那次真实调用（非提权环境下它本来就会失败，测试照样绿），
假的排队函数自己 `record_pending` 一笔就够了。

新增删除逻辑时：先开注入口，**再**写测试。顺序反过来，中间那段时间测试已经在写真
注册表而没人注意。

## 静默卸载与 ARP

`uninstall.exe` 是独立二进制（`src/uninstaller_main.rs`），**不用 clap**。参数解析在
`uninstaller::args`，静默路径的退出码收场在 `uninstaller::run_silent`——后者与
`wind-installer.exe uninstall --silent` 共用同一份，退出码是对外契约，分两处写会分叉。

改 ARP 那两条命令行时注意：

- 字符串由 `installer::registry::{uninstall_command, quiet_uninstall_command}` 构造，
  `registry.rs` 的 `arp_command_tests` 把生成的命令行**原样切回 argv 喂给解析器**再断言。
  只断言「字符串长得对」是不够的——从前那条 `"…" --uninstall --silent` 长得完全正确，
  却一次都没静默过。
- **别往里加未定义的 flag**。安装器那边 clap 开着 `ignore_errors = true`，语义是
  **从出错处截断**：一个不认识的参数会把它自己和它之后的全部丢掉。历史上的
  `--uninstall` 正是这么把 `--silent` 吃掉的。
- 新加 flag 要同时管两条路：`uninstaller::args`（`uninstall.exe`）与 `main.rs` 的 clap
  `Args`（`wind-installer.exe`）。

## 变体隔离（dev / release）

输入法有 dev（CLSID `{99C2DEB0-…}`）与 release（CLSID `{99C2EE30-…}`）两套 GUID，设计上要能共存。凡是按 CLSID/profile 操作系统的逻辑（注册、反注册、残留清扫），**只用清单里的 GUID 推导目标键**——这样正式版包只碰 EE 系列、dev 包只碰 DEB 系列，隔离性来自"配置即身份"，代码里**不写任何变体判断分支**。

## 配置文件的位置（重要）

- **生产清单**：`../WindInput/config/app.toml`（归属 **WindInput 仓库**）。这才是清风输入法真正打包用的配置。
- **本仓库的 `app.toml`**：只是**示例/参考**，演示清单写法，不是产品配置。改产品行为要改 WindInput 那份。

## 构建 / 测试

```
cargo build
cargo test --tests      # 跳过 doctest；集成测试是纯函数断言，无需管理员权限
cargo clippy --lib
```

注：`cargo test`（含 doctest）目前会因 `src/archive/format.rs` 文档注释里的 ASCII 图而报一个既有 doctest 失败，与业务逻辑无关，用 `--tests` 规避。

## Windows 产物必须静态链接 CRT（不可删）

`.cargo/config.toml` 给 MSVC 目标注入 `-C target-feature=+crt-static`。**删掉它，产出的安装器在没装 VC++ 运行库的干净机器上直接起不来**——`VCRUNTIME140.dll` 不是 Windows 内置组件，缺它会报「找不到 VCRUNTIME140.dll」或 `0xc000007b`。而安装器往往是用户在那台机器上运行的第一个程序，它起不来，用户无从自救、也不会知道为什么。

这条约束原先只写在调用方（WindInput 仓的 `dev.ps1` / `pack-installer.sh`）的 `RUSTFLAGS` 注入里，本仓直接 `cargo build --release` 出来的 stub 其实**不能发布**。约束属于产物本身，故已固化到本仓。

注意 `RUSTFLAGS` 环境变量会**覆盖**而非合并 `.cargo/config.toml` 的 target 小节——调用方注入的恰好是同一个 flag 故结果一致，但这也意味着这个保证可能被外部环境静默掀翻。因此 `release.yml` 不信任构建配置，而是**在发布前实测产物本身**有无动态 CRT 引用，命中即失败。改动构建配置或发布流程时不要摘掉这个校验。

## CI

- `ci.yml`（push/PR）：Windows 上 build + `test --tests` + `clippy --lib`；另有一个 ubuntu job 只构建 `wind-packer`，守着「packer 能在 Linux 原生构建」这条能力（`pack-installer.sh` 的全 Linux 流水线依赖它）。往 `archive`/`manifest`/`meta` 里引入 Windows-only 依赖时，只有这个 job 会红。
- `release.yml`（push tag `v*`）：产出 `wind-installer-windows-x64.exe`、`wind-uninstaller-windows-x64.exe`、`wind-packer-windows-x64.exe`、`wind-packer-linux-x64` 与 `SHA256SUMS`。`workflow_dispatch` 触发时只产 artifact 不发 Release。资产名不带版本号——版本由 tag 表达，下载方写死文件名即可。

门禁强度是**按代码库现状**定的：未启用 `-D warnings`（既有 11 处 clippy 警告）、未加 `cargo fmt --check`（现有代码未按 rustfmt 格式化）。红着的门禁没人会认真看，所以先让它保持绿。清理干净后再收紧，两处都在 `ci.yml` 里留了注释说明怎么改。
