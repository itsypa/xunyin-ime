# 第三方声明

wind-installer 使用了以下第三方组件，在此表示感谢并声明其许可证信息。

本项目自身采用 MIT 许可证，见 [LICENSE](LICENSE)。

## 仓库内收录的第三方代码

### editpe（本地修补副本）

- **用途**：`wind-packer` 写入 EXE 的图标与版本资源
- **目录**：`vendor/editpe`
- **上游**：[editpe](https://crates.io/crates/editpe) 0.2.3，作者 Christian Sdunek 及 editpe contributors
- **许可证**：BSD-2-Clause，全文见 [`vendor/editpe/LICENSE`](vendor/editpe/LICENSE)
- **修改说明**：上游的 `VersionInfo::build()` 按 UTF-8 字节数计算版本资源头部长度。
  版本信息含中文时结构错乱，导致 Windows 读不到版本信息。副本已改为按 UTF-16 码元数计算。

  该修补通过 `Cargo.toml` 的 `[patch.crates-io]` 段生效。向 `vendor/` 提交改动时请同步更新本节。

## 主要依赖

以下依赖以 crates.io 包的形式引入，不包含在本仓库中，各自适用其原项目的许可证条款。完整清单及其许可证可用
[`cargo-license`](https://github.com/onur/cargo-license) 或 [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) 生成：

```
cargo license --features packer
```

| 依赖 | 用途 | 许可证 |
|---|---|---|
| [windui](https://github.com/huanfeng/wind-ui-rust) | 安装/卸载向导的 GUI 框架 | MIT |
| [windows](https://github.com/microsoft/windows-rs) | Win32 API 绑定 | MIT OR Apache-2.0 |
| [zstd](https://github.com/gyscos/zstd-rs) / [xz2](https://github.com/alexcrichton/xz2-rs) | 归档压缩 | MIT / MIT OR Apache-2.0 |
| [serde](https://serde.rs/) / [toml](https://github.com/toml-rs/toml) | 清单解析 | MIT OR Apache-2.0 |
| [clap](https://github.com/clap-rs/clap) | 命令行参数解析 | MIT OR Apache-2.0 |
| [crc32fast](https://github.com/srijs/rust-crc32fast) | 归档校验 | MIT OR Apache-2.0 |
| [rand](https://github.com/rust-random/rand) | 临时文件名生成 | MIT OR Apache-2.0 |

## 权利声明

如您是上述任一资源的权利人，且认为本项目的使用方式不当，请通过
[Issue](../../issues) 联系我们，我们将及时处理。
