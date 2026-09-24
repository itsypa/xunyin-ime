param(
    [string]$OutDir = "D:\灵犀输入法\lingxi-tsf\src\assets"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

# 状态栏资源生成（参照 WindInput 工具栏规格）：
#   条厚 30dp、左侧拖动柄 12dp、每格 30dp；白底 245 alpha、圆角 = 厚度×0.3、1dp 同色边框；
#   格内高亮胶囊内缩（横条：左右各 inset/2、上下各 inset，inset=4）；
#   中文模式格 = 主题蓝底 + 白字；悬停 = 极淡黑（alpha 13）。
#   图标为单色 alpha 蒙版（运行时按主题/激活色 tint），文字同理。
# 资源按 2 倍分辨率渲染（逻辑 132x30 @96dpi → 264x60），运行时按显示器 DPI 缩放。

$S = 2
$CELL = 30
$GRIP = 12
$H = 30
$W = $GRIP + $CELL * 5          # 162：拖动柄 + 中/英 + 标点 + 全半角 + 符号 + 设置
$AW = $W * $S                    # 264
$AH = $H * $S                    # 60
$Tile = $CELL * $S               # 60（单格 tile 尺寸）

$out = $OutDir
New-Item -ItemType Directory -Force -Path $out | Out-Null

# 颜色（对齐 WindInput toolbar.rs 默认主题）
$BG    = [System.Drawing.Color]::FromArgb(245, 255, 255, 255)   # 白色半透明底
$SEP   = [System.Drawing.Color]::FromArgb(255, 214, 214, 220)   # 边框/分隔线
$GRIPC = [System.Drawing.Color]::FromArgb(255, 186, 186, 194)   # 拖动柄点
$HL_BG = [System.Drawing.Color]::FromArgb(255, 66, 133, 244)    # 激活蓝
$HOVER = [System.Drawing.Color]::FromArgb(13, 0, 0, 0)          # 悬停极淡黑
$CLEAR = [System.Drawing.Color]::FromArgb(0, 0, 0, 0)

function New-Bmp([int]$bw, [int]$bh) {
    $b = New-Object System.Drawing.Bitmap($bw, $bh, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    return @{ Bmp = $b; G = $g }
}

function Save-Bgra($ctx, [string]$name) {
    $b = $ctx.Bmp
    $bw = $b.Width; $bh = $b.Height
    $rect = New-Object System.Drawing.Rectangle(0, 0, $bw, $bh)
    $data = $b.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $bytes = New-Object byte[] ($data.Stride * $bh)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
    $b.UnlockBits($data)
    $fs = [System.IO.File]::Create((Join-Path $out "$name.bin"))
    $w = New-Object System.IO.BinaryWriter($fs)
    $w.Write([uint32]$bw); $w.Write([uint32]$bh)
    $w.Write($bytes)
    $w.Close()
    $ctx.G.Dispose(); $b.Dispose()
}

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    if ($d -gt $w) { $d = $w }
    if ($d -gt $h) { $d = $h }
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# ── 整条背景：白底圆角 + 1px 边框 + 左侧拖动柄点阵 ──────────────────────────
function Make-BarBg() {
    $ctx = New-Bmp $AW $AH
    $g = $ctx.G
    $radius = $AH * 0.30
    $body = RoundRect 0.5 0.5 ($AW - 1) ($AH - 1) $radius
    $g.FillPath((New-Object System.Drawing.SolidBrush($BG)), $body)
    $pen = New-Object System.Drawing.Pen($SEP, 1)
    $g.DrawPath($pen, $body)
    $pen.Dispose(); $body.Dispose()
    # 拖动柄：2 列 × 3 行，点径 2dp、间距 4dp，居中于 12dp 宽的柄区
    $dot = 2.0 * $S
    $gap = 4.0 * $S
    $cx = ($GRIP * $S) / 2.0
    $cy = $AH / 2.0
    $brush = New-Object System.Drawing.SolidBrush($GRIPC)
    for ($row = 0; $row -lt 3; $row++) {
        for ($col = 0; $col -lt 2; $col++) {
            $x = $cx + ($col - 0.5) * $gap - $dot / 2
            $y = $cy + ($row - 1) * $gap - $dot / 2
            $g.FillEllipse($brush, $x, $y, $dot, $dot)
        }
    }
    $brush.Dispose()
    Save-Bgra $ctx "bar_bg"
}

# ── 单格高亮 tile（悬停 / 激活）：横条胶囊内缩 inset=4 ──────────────────────
function Make-CellTile([string]$name, [System.Drawing.Color]$color) {
    $ctx = New-Bmp $Tile $Tile
    $g = $ctx.G
    $inset = 4 * $S
    $x = $inset / 2.0
    $y = [float]$inset
    $w = $Tile - $inset
    $hh = $Tile - $inset * 2
    $r = [Math]::Min($w, $hh) * 0.30
    $p = RoundRect $x $y $w $hh $r
    $g.FillPath((New-Object System.Drawing.SolidBrush($color)), $p)
    $p.Dispose()
    Save-Bgra $ctx $name
}

# ── 图标蒙版（白色 + alpha，运行时 tint）────────────────────────────────────
function Make-Icon([string]$name, [scriptblock]$draw) {
    $ctx = New-Bmp $Tile $Tile
    $g = $ctx.G
    $size = 12.0 * $S
    $cx = $Tile / 2.0
    $cy = $Tile / 2.0
    $white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
    & $draw $g $cx $cy $size $white
    $white.Dispose()
    Save-Bgra $ctx $name
}

Make-BarBg
Make-CellTile "cell_hover" $HOVER
Make-CellTile "cell_active" $HL_BG

# 全角：实心圆 ●
Make-Icon "icon_width_full" {
    param($g, $cx, $cy, $size, $brush)
    $r = $size / 2.0
    $g.FillEllipse($brush, $cx - $r, $cy - $r, $size, $size)
}

# 半角：弯月（实心圆挖掉偏移圆）
Make-Icon "icon_width_half" {
    param($g, $cx, $cy, $size, $brush)
    $r = $size / 2.0
    $g.FillEllipse($brush, $cx - $r, $cy - $r, $size, $size)
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $er = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(0, 0, 0, 0))
    $r2 = $r * 0.86
    $off = $r * 0.42
    $g.FillEllipse($er, $cx - $r2 + $off, $cy - $r2 - $off * 0.35, $r2 * 2, $r2 * 2)
    $er.Dispose()
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
}

# 全角标点：圆环 。（中文句号）
Make-Icon "icon_punct_full" {
    param($g, $cx, $cy, $size, $brush)
    $r = $size * 0.40
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, (2.2 * $S))
    $g.DrawEllipse($pen, $cx - $r, $cy - $r, $r * 2, $r * 2)
    $pen.Dispose()
}

# 半角标点：实心小圆点 .（英文句号）
Make-Icon "icon_punct_half" {
    param($g, $cx, $cy, $size, $brush)
    $r = $size * 0.22
    $g.FillEllipse($brush, $cx - $r, $cy - $r + $size * 0.14, $r * 2, $r * 2)
}

# 设置：齿轮（8 齿 + 中心镂空）
Make-Icon "icon_gear" {
    param($g, $cx, $cy, $size, $brush)
    $r = $size * 0.34
    $g.FillEllipse($brush, $cx - $r, $cy - $r, $r * 2, $r * 2)
    $toothW = $size * 0.20
    $toothH = $size * 0.26
    for ($i = 0; $i -lt 8; $i++) {
        $ang = $i * 45.0 * [Math]::PI / 180.0
        $tx = $cx + [Math]::Cos($ang) * ($r * 1.12)
        $ty = $cy + [Math]::Sin($ang) * ($r * 1.12)
        $state = $g.Save()
        $g.TranslateTransform([float]$tx, [float]$ty)
        $g.RotateTransform([float]($i * 45.0))
        $g.FillRectangle($brush, [float](-$toothW / 2), [float](-$toothH / 2), [float]$toothW, [float]$toothH)
        $g.Restore($state)
    }
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $er = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(0, 0, 0, 0))
    $hr = $size * 0.16
    $g.FillEllipse($er, $cx - $hr, $cy - $hr, $hr * 2, $hr * 2)
    $er.Dispose()
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
}

# ── 文字蒙版：中 / 英（白色 + alpha，15dp 字号）──────────────────────────────
function Make-Glyph([string]$text, [string]$name) {
    $ctx = New-Bmp $Tile $Tile
    $g = $ctx.G
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $f = New-Object System.Drawing.Font("Microsoft YaHei UI", (15.0 * $S), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $g.DrawString($text, $f, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0, 0, $Tile, $Tile)), $sf)
    $f.Dispose()
    Save-Bgra $ctx $name
}

Make-Glyph "中" "glyph_zhong"
Make-Glyph "英" "glyph_ying"
Make-Glyph "※" "glyph_symbol"

Get-ChildItem $out -Filter *.bin | Select-Object Name, Length
Write-Output "assets generated -> $out"
