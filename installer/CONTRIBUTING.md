# 贡献指南

感谢您对 wind-installer 的关注！我们欢迎所有形式的贡献，包括 Bug 报告、功能建议和代码提交。

## 先理解本仓库的定位

**wind-installer 是一个「通用 Windows 安装器生成器」，不是某个特定应用的专用安装器。**

同一个预编译的 stub 二进制配上不同的清单 (`app.toml`) 即可为任意应用打出安装包，无需重新编译。清单在打包时嵌入归档头部，安装器与卸载器在**运行期**读取它。因此有一条贯穿全仓的原则：

> **一切与具体应用相关的东西都必须写在清单里，绝不能硬编码进 Rust 代码。**

清风输入法 (WindInput) 只是本项目的第一个使用者，不是特权使用者。任何"因为调用方是输入法所以直接写死"的改动，都是在把通用安装器退化成定制安装器，不会被接受。这条约束由测试 `tests/ui_defaults.rs::defaults_are_domain_neutral` 守护。

完整的架构说明见 [`docs/DESIGN.md`](docs/DESIGN.md)，代码级的硬性规则见 [`AGENTS.md`](AGENTS.md)——**提交代码前请务必读一遍后者**，它记录了若干反直觉的约定及其原因。

## 签署 CLA（必须）

**所有贡献者在首次提交 Pull Request 前必须签署贡献者许可协议 (CLA)。**

这是为了确保项目的许可证管理和知识产权的一致性。流程如下：

1. 提交您的 Pull Request
2. CLA Assistant 机器人会自动在 PR 中发起签署请求
3. 在 PR 评论中回复：`I have read the CLA Document and I hereby sign the CLA`
4. 签署完成后，CLA 检查将自动通过

未签署 CLA 的 PR 将无法合并。完整协议内容请参阅 [CLA.md](CLA.md)。

## Bug 报告

请通过 [GitHub Issues](../../issues) 的 **Bug 反馈** 模板提交。安装器的问题高度依赖清单内容与系统状态，请尽量包含：

- 操作系统与版本（如 Windows 11 24H2）、是否以管理员身份运行
- 触发问题的**清单片段**（`app.toml` 中相关的段落，敏感信息可脱敏）
- 运行模式：交互式向导 / `--quiet` / `--silent`，以及完整命令行
- **退出码**——尤其请注意 `3010` 表示"安装成功但需重启"，不是失败
- `%TEMP%\wind_installer_args.log`（`--silent` 模式会写入）
- 是首次安装、升级覆盖，还是卸载时出现

`wind-packer inspect <安装包.exe>` 可以读出已生成安装程序中嵌入的清单摘要，定位"打包时写了什么"与"运行期读到什么"不一致的问题时很有用，请一并附上。

> 使用清风输入法时遇到的问题（候选、编码、词库、界面），请到
> [WindInput 主仓库](https://github.com/huanfeng/WindInput/issues) 反馈——除非您能确认问题出在安装/卸载过程本身。

## 功能建议

欢迎通过 [GitHub Issues](../../issues) 的 **功能建议** 模板提交。

由于本仓库是通用工具，请在描述中说明该功能对**非输入法类应用**同样成立的理由，或者它如何以清单字段的形式表达（缺省即不启用）。只服务于单一应用的需求，通常应当在调用方解决而非在安装器中特判。

## 代码贡献

### 开发环境

- Rust stable 工具链（rustup 安装，含 `clippy` 组件）
- **Windows 本机构建**：Visual Studio 2022 生成工具（MSVC 链接器）

`src/installer/`、`src/uninstaller/`、`src/ui/`、`src/util/` 基本都是 `cfg(windows)` 代码，在非 Windows 主机上根本不参与编译。而 `src/archive/`、`src/manifest.rs` 与 `wind-packer` 是纯 IO，可在 Linux 上原生构建——CI 用一个独立的 ubuntu job 守着这条能力（消费方的全 Linux 打包流水线依赖它）。

因此：**改动 archive / manifest / meta 时请留意不要引入 Windows-only 依赖**，否则会打断 Linux 侧的打包能力，而 Windows job 不会报错。

### 构建

```powershell
# 三个二进制。packer 走 packer feature（editpe 是可选依赖），
# 只编 stub 不会暴露 packer 侧的编译错误
cargo build --bin wind-installer --bin wind-uninstaller
cargo build --bin wind-packer --features packer

# 打完整安装包（编译 + 打包，输出到 app.toml 的 [package] output_dir）
.\scripts\pack.ps1
```

Windows 目标通过 `.cargo/config.toml` 静态链接 MSVC CRT，产物不依赖 VC++ 运行库。stub 与卸载器运行在最终用户机器上，**这一点是发布门禁**（`scripts/verify-static-crt.ps1`），不要修改相关链接参数。打包工具只在构建机运行，不受此约束。

### 提 PR 前的自检

CI（`.github/workflows/ci.yml`）跑以下几项，请在本地先跑一遍：

```powershell
cargo fmt --all -- --check
cargo build --locked --bin wind-installer --bin wind-uninstaller
cargo build --locked --bin wind-packer --features packer
cargo test --locked --tests
cargo clippy --locked --lib -- -D warnings
```

两点说明：

- **`cargo test` 要带 `--tests`。** 不带时会跑 doctest，`src/archive/format.rs` 的文档注释里有一张 ASCII 结构图，rustdoc 会把它当代码块编译并报错。这是既有问题，与业务逻辑无关，**不要在 PR 里顺手"修好"**。
- **clippy 的 `-D warnings` 目前只覆盖 `--lib`。** `--all-targets` 下 bin 与 tests 尚有几处警告（`items after a test module` 等）未清，清理它们需要挪动代码块，留待单独收拾。若您愿意专门提一个 PR 做这件事（**且不夹带任何逻辑修改**），我们很欢迎，届时会同步把 CI 的范围扩到 `--all-targets --features packer`。

### Git Hooks（首次克隆后建议激活）

仓库自带 `.githooks/pre-commit`（提交前自动跑 `cargo fmt --all --check`，避免未格式化代码被提交后才在 CI 里暴露），默认不生效，需一次性激活：

```bash
git config core.hooksPath .githooks
```

仅影响本地 clone，不会随仓库自动传播——**每个 clone 与每个 git worktree 都要各自执行一次**。

### 测试

Windows 副作用（注册表、字体安装、TSF 注册、快捷方式、文件占用）无法在 CI 中验证，涉及这些路径的改动请在 Windows 设备上实测并在 PR 中说明。

可自动化的部分请补测试，`tests/` 下已有的几类是很好的范本：

| 测试文件 | 守护的性质 |
|---|---|
| `ui_defaults.rs` | 默认文案的**领域中性**——不含任何具体产品的词汇 |
| `step_plan.rs` | **能力段缺省 = 该步骤不入计划**（`datadir_conf_is_opt_in` 等） |
| `wizard_data_dir.rs` | 向导初值、卸载提示、实际删除**三方取值一致** |
| `receipt_roundtrip.rs` | 回执的读写往返 |
| `archive_roundtrip.rs` / `format_serde.rs` | 归档格式与序列化兼容性 |

### 新增一种能力时

请按既有套路走完五步，缺一步都会留下坑：

1. `src/manifest.rs`：给对应段加字段，用 `#[serde(default)]` 保证旧清单兼容
2. `src/installer/<capability>.rs`：实现逻辑，参数只从 `meta`/传入的清单结构取
3. `src/installer/steps.rs`：加一个 `impl Step<InstallCtx>`；**有系统副作用就必须 push 回执**
4. `src/installer/plan.rs`：加门控，按清单字段决定是否入计划
5. `tests/step_plan.rs`：加断言——缺省不入计划、声明才入计划

三条最容易被忽略的硬性要求：

- **有副作用的步骤必须写回执**（`ctx.receipt`）。卸载是回执驱动的，不重新读清单——没有回执就撤销不掉，且用户升级过多个版本后无从补救。
- **新增的、会改系统（尤其注册表）的行为默认关闭**，由打包器在清单里显式 opt-in。通用安装器不擅自动系统。
- **可能删不掉文件的新代码，失败分支必须记账**（`util::reboot::schedule_delete_on_reboot` / `record_pending`），否则"需要重启"的结论会漏判。不要只 `eprintln!`——安装器是 `windows_subsystem = "windows"` 的无控制台进程，那等于丢弃。

### 提交规范

本项目使用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/) 规范：

```
<类型>(<范围>): <描述>

[可选的正文]
```

类型包括：

| 类型 | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `docs` | 文档变更 |
| `refactor` | 代码重构（不改变行为） |
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建/工具变更 |
| `style` | 格式化 |

范围示例：`archive`、`manifest`、`installer`、`uninstaller`、`packer`、`ui`、`ime`、`font`、`shortcut`、`receipt`、`reboot`、`ci`

### 代码风格

- 复杂决策请在代码注释中写明**为什么**。本项目大量涉及 Win32 的非直觉行为（文件占用改名让路、`MoveFileEx(DELAY_UNTIL_REBOOT)`、3010 退出码语义、AV 误报对策），现有的 CI/release workflow 注释与 `AGENTS.md` 即是范例——那些"为什么不这么做"的说明，价值往往高于代码本身。
- 界面显示的路径必须等于实际生效的路径，不得在 UI 里按默认模板重新推导一遍。

### Pull Request 流程

1. Fork 本仓库并从 `main` 分支创建您的分支
2. 完成修改后运行上面「提 PR 前的自检」的命令
3. 涉及 Windows 副作用的改动，在真实设备上实测
4. 按 PR 模板填写变更说明、测试情况与检查清单
5. 提交 PR 并等待 CLA 检查和代码审查

## 项目结构

| 文件 / 目录 | 说明 |
|---|---|
| `src/main.rs` / `src/uninstaller_main.rs` | 安装器与卸载器入口 |
| `src/manifest.rs` | `app.toml` 清单的数据结构与解析 |
| `src/meta.rs` | 运行期清单访问（`meta::manifest()`） |
| `src/archive/` | 归档格式：读写、头部、压缩（纯 IO，Linux 可构建） |
| `src/installer/plan.rs` | "装什么、按什么顺序装"的唯一真相，GUI 与静默路径共用 |
| `src/installer/steps.rs` / `step.rs` | 步骤定义与执行框架 |
| `src/installer/receipt.rs` | 回执：记录系统副作用供卸载撤销 |
| `src/installer/{ime,font,shortcut,registry,acl,…}.rs` | 各能力的具体实现 |
| `src/uninstaller/` | 卸载计划、清理与自删除 |
| `src/ui/` | 安装/卸载向导与主题 |
| `src/util/reboot.rs` | "需要重启"账本 |
| `tools/packer/` | 打包工具 `wind-packer` |
| `scripts/` | 构建、打包与发布门禁脚本 |
| `vendor/` | 第三方 crate 的本地修补副本（见 [NOTICE.md](NOTICE.md)） |
| `docs/DESIGN.md` | 架构与归档格式设计文档 |

## 许可证

提交贡献即表示您同意您的贡献将按照项目的 [MIT 许可证](LICENSE) 进行授权。
