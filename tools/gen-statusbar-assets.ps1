param(
    [string]$OutDir = "D:\迅音输入法\xunyin-tsf\src\assets"
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
# 悬停胶囊：原来只有 13/255 的黑（≈5% 压暗，肉眼几乎看不出，用户反馈"不支持悬停"），
# 改成与主题蓝同族的淡蓝灰，悬停时一眼能看出高亮
$HOVER = [System.Drawing.Color]::FromArgb(255, 232, 240, 254)    # 悬停淡蓝 #F0E8FE(BGR)
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

# ── 文字蒙版：中 / 英 / 全 / 半 / 标点（白色 + alpha，运行时 tint）──────────
# 标点与全半角也用文字而不是图形：圆环/实心圆/弯月的隐喻太含糊（实测两个圆
# 挨在一起分不清哪个是标点哪个是全角），文字最直观。
function Make-Glyph([string]$text, [string]$name, [double]$fontPx = 15.0) {
    # 先画到临时画布量出**墨迹包围盒**，再按墨迹居中贴进方格。
    # 必要性：全角标点（，。）的墨迹天生挤在字框左下角，直接居中排版会显得
    # 又小又偏下；中文汉字的墨迹也不是严格居中。按墨迹居中后整排图标才对得齐。
    $tmpSize = 160
    $tmp = New-Object System.Drawing.Bitmap($tmpSize, $tmpSize, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $tg = [System.Drawing.Graphics]::FromImage($tmp)
    $tg.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $f = New-Object System.Drawing.Font("Microsoft YaHei UI", ($fontPx * $S), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $tg.DrawString($text, $f, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0, 0, $tmpSize, $tmpSize)), $sf)
    $f.Dispose()
    # 量墨迹
    $minX = $tmpSize; $maxX = -1; $minY = $tmpSize; $maxY = -1
    for ($y = 0; $y -lt $tmpSize; $y++) {
        for ($x = 0; $x -lt $tmpSize; $x++) {
            if ($tmp.GetPixel($x, $y).A -gt 20) {
                if ($x -lt $minX) { $minX = $x }
                if ($x -gt $maxX) { $maxX = $x }
                if ($y -lt $minY) { $minY = $y }
                if ($y -gt $maxY) { $maxY = $y }
            }
        }
    }
    $ctx = New-Bmp $Tile $Tile
    if ($maxX -ge $minX -and $maxY -ge $minY) {
        $inkW = $maxX - $minX + 1
        $inkH = $maxY - $minY + 1
        $dx = [int](($Tile - $inkW) / 2) - $minX
        $dy = [int](($Tile - $inkH) / 2) - $minY
        $ctx.G.DrawImage($tmp, (New-Object System.Drawing.Rectangle($dx, $dy, $tmpSize, $tmpSize)))
    }
    $tg.Dispose(); $tmp.Dispose()
    Save-Bgra $ctx $name
}

Make-Glyph "中" "glyph_zhong"
Make-Glyph "英" "glyph_ying"
Make-Glyph "※" "glyph_symbol"
Make-Glyph "全" "glyph_quan"
Make-Glyph "半" "glyph_ban"

# 把单个字符画到临时画布并按墨迹裁成紧致位图（返回位图与尺寸）。
function Render-Ink([string]$text, [double]$fontPx) {
    $sz = 160
    $tmp = New-Object System.Drawing.Bitmap($sz, $sz, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($tmp)
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $f = New-Object System.Drawing.Font("Microsoft YaHei UI", ($fontPx * $S), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $g.DrawString($text, $f, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0, 0, $sz, $sz)), $sf)
    $f.Dispose(); $g.Dispose()
    $minX = $sz; $maxX = -1; $minY = $sz; $maxY = -1
    for ($y = 0; $y -lt $sz; $y++) {
        for ($x = 0; $x -lt $sz; $x++) {
            if ($tmp.GetPixel($x, $y).A -gt 20) {
                if ($x -lt $minX) { $minX = $x }
                if ($x -gt $maxX) { $maxX = $x }
                if ($y -lt $minY) { $minY = $y }
                if ($y -gt $maxY) { $maxY = $y }
            }
        }
    }
    if ($maxX -lt $minX) { $tmp.Dispose(); return @{ Bmp = $null; W = 0; H = 0 } }
    $w = $maxX - $minX + 1
    $h = $maxY - $minY + 1
    $tight = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $tg = [System.Drawing.Graphics]::FromImage($tight)
    $tg.DrawImage(
        $tmp,
        (New-Object System.Drawing.Rectangle(0, 0, $w, $h)),
        (New-Object System.Drawing.Rectangle($minX, $minY, $w, $h)),
        [System.Drawing.GraphicsUnit]::Pixel)
    $tg.Dispose(); $tmp.Dispose()
    return @{ Bmp = $tight; W = $w; H = $h }
}

# 标点格：两枚标点**按墨迹并排**，间距固定（全角标点自带全角字宽，直接排版会离得老远）
function Make-PunctGlyph([string]$a, [string]$b, [string]$name, [double]$fontPx = 18.0, [int]$gap = 4) {
    $ia = Render-Ink $a $fontPx
    $ib = Render-Ink $b $fontPx
    if ($ia.W -eq 0 -or $ib.W -eq 0) { throw "标点字形渲染失败: $a / $b" }
    $totalW = $ia.W + $gap + $ib.W
    $totalH = [Math]::Max($ia.H, $ib.H)
    $x0 = [int](($Tile - $totalW) / 2)
    $y0 = [int](($Tile - $totalH) / 2)
    $ctx = New-Bmp $Tile $Tile
    $ctx.G.DrawImage($ia.Bmp, $x0, $y0 + [int](($totalH - $ia.H) / 2))
    $ctx.G.DrawImage($ib.Bmp, $x0 + $ia.W + $gap, $y0 + [int](($totalH - $ib.H) / 2))
    $ia.Bmp.Dispose(); $ib.Bmp.Dispose()
    Save-Bgra $ctx $name
}

Make-PunctGlyph "，" "。" "glyph_punct_cn"
Make-PunctGlyph "," "." "glyph_punct_en"

Get-ChildItem $out -Filter *.bin | Select-Object Name, Length
Write-Output "assets generated -> $out"
