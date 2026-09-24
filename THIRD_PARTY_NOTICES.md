# 第三方组件声明

本项目使用以下第三方组件/数据：

## Rime / librime

- 项目：[Rime/librime](https://github.com/rime/librime)
- 许可：LGPL-3.0（运行时以独立 DLL 形式分发）
- 说明：`xunyin-ime/src-tauri/rime/rime.dll`（x64）与
  `xunyin-ime/src-tauri/rime-x86/rime.dll`（x86）为 librime 构建产物。

## Rime 词库数据

- 来源：Rime 官方与社区词库（cn_dicts、cn_dicts_cell、en_dicts、luna_pinyin
  词表、opencc 等），位于 `xunyin-ime/src-tauri/rime/data`。
- 许可：GPL/LGPL 与开放词库许可，具体以各文件头注明的版权声明为准。
- 范围：本输入法只做拼音。随包数据仅保留迅音拼音所需的两个方案
  （`xunyin_rich`、`xunyin_pinyin`）及其依赖词表；五笔/仓颉/笔画/双拼等
  无关方案与词表（stroke、cangjie5、double_pinyin*、luna_pinyin 方案定义）
  已从随包数据中移除，`luna_pinyin.dict.yaml` 仅作为词表被
  `xunyin_common` 引用。

## 词库取材规则（重要）

升级上游词库时**只取中文词库**：`cn_dicts/`、`cn_dicts_cell/`。
英文词库（`en_dicts/`）、简繁与符号表（`opencc/`）、语句模型（`essay.txt`）、
标点表（`symbols.yaml`）一律**保留我们自己的版本**——它们会直接改变输入行为
（例如 symbols.yaml 的 full_shape `'/' : [ ／, ÷ ]` 会让斜杠变成"选词"，
而不是直接上屏顿号）。

## 界面参考

- 状态栏视觉风格参考搜狗输入法默认状态栏（仅借鉴外观，无代码/素材复用）。

## wind-installer（安装器）

- 项目：[huanfeng/wind-installer](https://github.com/huanfeng/wind-installer)
- 许可：MIT
- 说明：`installer/` 目录为其源码副本（清单驱动的 Windows 安装器生成器）。
  本项目用它生成 `XunyinIME-Setup-<版本>.exe`（含卸载器），清单见
  `installer/app.toml`。原项目的 LICENSE 与 NOTICE 保留在 `installer/` 内。

## windui

- 项目：[huanfeng/wind-ui-rust](https://github.com/huanfeng/wind-ui-rust)
- 许可：MIT
- 说明：`installer/` 的安装/卸载向导 GUI 依赖（经 crates.io 引入，未随仓库分发源码）。
