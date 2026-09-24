# windui 本地修补说明

本目录是 [windui](https://github.com/huanfeng/wind-ui-rust) 0.16.1 的本地副本，
仅为让**32 位（i686-pc-windows-msvc）**安装器能编译而做了最小修补，其余代码与上游一致。

## 修补内容

| 文件 | 位置 | 问题 | 处理 |
|---|---|---|---|
| `src/platform/win32/mod.rs` | `WM_NCCREATE` ×2 | `SetWindowLongPtrW` 第三参在 32 位目标是 `i32`、64 位是 `isize`，上游固定传 `isize` → E0308 | 改为 `as isize as _` / `as _`，按目标推导类型 |
| `src/platform/win32/tray.rs` | `copy_wide(&mut nid.szXXX, …)` ×4 | `NOTIFYICONDATAW` 是 1 字节对齐的 packed 结构，`&mut 字段` 触发 E0793「reference to field of packed struct is unaligned」 | 新增 `set_wide_field()`：先填临时数组，再用 `addr_of_mut!` + `copy_nonoverlapping` 整块写入 |

## 升级上游时

替换为新版 windui 后，先跑一次

```
cargo build --release --features packer --target i686-pc-windows-msvc
```

若上游已修掉上述问题，可删除本目录与 `Cargo.toml` 里的 `[patch.crates-io] windui` 条目。
