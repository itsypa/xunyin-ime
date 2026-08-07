# 第三方组件声明

本项目使用以下第三方组件/数据：

## Rime / librime

- 项目：[Rime/librime](https://github.com/rime/librime)
- 许可：LGPL-3.0（运行时以独立 DLL 形式分发）
- 说明：`lingxi-ime/src-tauri/rime/rime.dll`（x64）与
  `lingxi-ime/src-tauri/rime-x86/rime.dll`（x86）为 librime 构建产物。

## Rime 词库数据

- 来源：Rime 官方与社区词库（luna_pinyin、stroke、cangjie5、cn_dicts、
  cn_dicts_cell、en_dicts、opencc 等），位于 `lingxi-ime/src-tauri/rime/data`。
- 许可：GPL/LGPL 与开放词库许可，具体以各文件头注明的版权声明为准。

## 界面参考

- 状态栏视觉风格参考搜狗输入法默认状态栏（仅借鉴外观，无代码/素材复用）。
