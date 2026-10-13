param(
    # 把内置的界面资源导出成「皮肤文件」，供手工修改。改完在状态栏齿轮菜单点
    # 「重载配置」即可生效，**不用重启任何应用**（资源从磁盘读，内嵌只作兜底）。
    [string]$SrcDir = (Join-Path (Split-Path -Parent $PSScriptRoot) "xunyin-tsf\src\assets"),
    [string]$SkinDir = (Join-Path $env:APPDATA "com.xunyin.ime\skins"),
    # 删掉皮肤文件，回到内置资源
    [switch]$Remove
)

$ErrorActionPreference = "Stop"

if ($Remove) {
    if (Test-Path -LiteralPath $SkinDir) {
        $n = 0
        foreach ($f in Get-ChildItem -LiteralPath $SkinDir -Filter *.bin -File) {
            Remove-Item -LiteralPath $f.FullName -Force
            $n++
        }
        Write-Host "已删除 $n 个皮肤文件（回到内置资源）：$SkinDir"
    } else {
        Write-Host "皮肤目录不存在，无需删除：$SkinDir"
    }
    Write-Host "提示：删除后到状态栏齿轮菜单点「重载配置」即可恢复内置图标。"
    exit 0
}

if (-not (Test-Path -LiteralPath $SrcDir)) { throw "找不到资源目录：$SrcDir" }
New-Item -ItemType Directory -Force -Path $SkinDir | Out-Null

$copied = 0
foreach ($f in Get-ChildItem -LiteralPath $SrcDir -Filter *.bin -File) {
    Copy-Item -LiteralPath $f.FullName -Destination (Join-Path $SkinDir $f.Name) -Force
    $copied++
}

$readme = @'
# 迅音输入法 皮肤目录

这里的 `*.bin` 会**覆盖**内置界面资源（状态栏图标、条底、格子高亮等）。
改完文件后：**状态栏齿轮菜单 → 重载配置**，所有正在运行的应用立刻生效（不用重启）。

## 文件格式

每个文件 = 8 字节头 + BGRA 像素（自顶向下，非预乘 alpha）：

    偏移 0 : u32 宽（小端）
    偏移 4 : u32 高（小端）
    偏移 8 : 宽 × 高 × 4 字节，每像素 B,G,R,A

尺寸必须与内置一致，否则该文件会被忽略并回退内置：

    bar_bg.bin        324 x 60   整条背景（拖动柄 + 5 格底）
    其它 *.bin         60 x 60   单格贴图

## 各文件含义

| 文件 | 用途 |
|---|---|
| bar_bg.bin | 状态栏整条底色、圆角、边框、拖动柄点阵 |
| cell_hover.bin | 鼠标悬停格的高亮胶囊 |
| cell_active.bin | 激活格（中文模式 / 符号面板已打开）的蓝底胶囊 |
| glyph_zhong.bin / glyph_ying.bin | 中 / 英 |
| glyph_punct_cn.bin / glyph_punct_en.bin | 中文标点「，。」/ 英文标点「,.」 |
| glyph_quan.bin / glyph_ban.bin | 全角=满月 / 半角=半月 |
| glyph_symbol.bin | 符号（※） |
| icon_gear.bin | 设置齿轮 |

图标类文件只用到 **alpha 通道**（颜色由主题 tint 决定），所以做单色蒙版即可：
要显示的地方 alpha 给 255，不要的地方给 0，边缘用灰度做抗锯齿。

## 复原

    .\tools\export-skin-assets.ps1 -Remove     # 删掉皮肤文件，回到内置资源

注意：皮肤文件**优先级高于内置资源**，所以升级输入法后，如果内置图标更新了，
这里的旧副本会把它遮住——想要新内置图标就先 -Remove 或同步更新文件。
'@

Set-Content -LiteralPath (Join-Path $SkinDir "README.md") -Value $readme -Encoding UTF8

Write-Host "已导出 $copied 个皮肤文件 -> $SkinDir"
Write-Host "改完资源后在状态栏齿轮菜单点「重载配置」即可生效（不用重启应用）。"
