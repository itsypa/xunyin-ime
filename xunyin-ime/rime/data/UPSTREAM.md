# 词库分区规则（升级时务必遵守）

本目录里有两类东西，**升级上游时只碰第一类**。

## 一、上游区（可整体替换）
只来自上游 [rime-frost](https://github.com/gaboolic/rime-frost) 的**中文词库**：

- `cn_dicts/` —— 字表与基础/扩展词库
- `cn_dicts_cell/` —— 细胞词库（专业词库）

升级方式：`tools/update-upstream-dicts.ps1 -Source <解压后的 rime-frost 目录>`
该脚本只会覆盖上面两个目录，其它文件一律不碰。

## 二、我们自己的（升级上游时**严禁覆盖**）
这些文件直接决定输入行为，被上游版本覆盖就会出现"标点要选词""英文词库被换掉"之类的问题：

- `xunyin_rich.schema.yaml` —— 方案定义（方案名：迅音输入）
- `rime_xunyin.dict.yaml` —— 主词表挂载清单（决定引用哪些上游词库）
- `symbols.yaml` —— 标点表（**曾因被上游覆盖导致顿号要选词**）
- `default.yaml` —— 方案列表 / 候选数 / 按键绑定
- `melt_eng.dict.yaml`、`emoji.dict.yaml` —— 中英混输与 emoji 词表
- `en_dicts/`、`opencc/`、`essay.txt`、`luna_pinyin.dict.yaml`、`custom_phrase.txt`（随包示例）

## 三、用户自己产生的（与程序数据分开存放，永不随包更新）
全部在 `%APPDATA%\com.xunyin.ime\`：

- `*.userdb/` —— 打字过程中学习到的词（**用户词库**）
- `custom_phrase.txt` —— 用户自定义短语
- `backup/` —— 用户词库备份
- `config.json` —— 用户设置

所以：**上游词库 = 随包的中文词库；用户词库 = %APPDATA% 下的 userdb/短语**，
两者物理分离，升级上游不会影响用户学到的词。