param(
    # 应用 / 语言栏图标：蓝底白字「迅」，多尺寸 ICO（语言栏、任务栏、安装器共用）。
    [string]$OutFile = (Join-Path (Split-Path -Parent $PSScriptRoot) "xunyin-ime\icons\xunyin.ico"),
    # 安装向导里显示的那张 logo（PNG）。同一份底图渲染，避免"图标换了、向导还顶着旧图"。
    [string]$LogoOut = (Join-Path (Split-Path -Parent $PSScriptRoot) "xunyin-ime\icons\logo.png"),
    [int[]]$Sizes = @(16, 24, 32, 48, 64, 128, 256)
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

# 说明：Windows 语言栏（CTF）只认真正的多尺寸 ICO。之前仓库里的 xunyin.ico 是
# 生成脚本写坏的（只有 91 字节、5 个目录项全是空的图像数据），系统加载失败后
# 就退回显示语言默认的「简体」二字 —— 这就是「语言栏图标一直是简体」的原因。
# 本脚本按 ICO 规范手工拼 ICONDIR + BMP 条目（32bpp BGRA + AND 掩码），
# 尺寸齐全、带 alpha，重新生成一次即可。

$BlueTop = [System.Drawing.Color]::FromArgb(255, 48, 122, 232) # #307AE8
$BlueBottom = [System.Drawing.Color]::FromArgb(255, 32, 96, 200) # #2060C8
$White = [System.Drawing.Color]::FromArgb(255, 255, 255, 255)

function New-RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
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

function Add-IconImage([int]$size, [System.IO.MemoryStream]$sink) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::Transparent)

    # 圆角方块底（Win11 风格圆角 ≈ 22%，边缘内缩 0.5px 防锯齿出界）
    $inset = [Math]::Max(0.5, $size * 0.02)
    $body = New-RoundRect $inset $inset ($size - 2 * $inset) ($size - 2 * $inset) ($size * 0.22)
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF(0, 0)),
        (New-Object System.Drawing.PointF(0, $size)),
        $BlueTop, $BlueBottom)
    $g.FillPath($grad, $body)
    $body.Dispose(); $grad.Dispose()

    # 白色「迅」（微软雅黑 UI Bold）
    $em = [Math]::Max(7.0, $size * 0.74)
    $font = New-Object System.Drawing.Font("Microsoft YaHei UI", $em, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $brush = New-Object System.Drawing.SolidBrush($White)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = [System.Drawing.StringAlignment]::Center
    $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
    # 汉字字形在 em 框里重心偏上，整体下移 2% 视觉更居中
    $rect = New-Object System.Drawing.RectangleF(0, ($size * 0.02), $size, $size)
    $g.DrawString("迅", $font, $brush, $rect, $fmt)

    # 取 32bpp BGRA（非预乘，ICO 的 BMP 条目要求）
    $rectI = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
    $data = $bmp.LockBits($rectI, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $stride = $data.Stride
    $buf = New-Object byte[] ($stride * $size)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $buf, 0, $buf.Length)
    $bmp.UnlockBits($data)
    $fmt.Dispose(); $brush.Dispose(); $font.Dispose(); $g.Dispose(); $bmp.Dispose()

    # 组装 BMP 条目：BITMAPINFOHEADER(40) + XOR（自下而上）+ AND 掩码（全 0）
    $w = New-Object System.IO.BinaryWriter($sink)
    $maskRow = [int]([Math]::Floor(($size + 31) / 32) * 4)
    $xorLen = $stride * $size
    $andLen = $maskRow * $size
    $w.Write([uint32]40); $w.Write([int32]$size); $w.Write([int32]($size * 2))
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]0)
    $w.Write([uint32]($xorLen + $andLen))
    $w.Write([int32]0); $w.Write([int32]0); $w.Write([uint32]0); $w.Write([uint32]0)
    for ($y = $size - 1; $y -ge 0; $y--) {
        $w.Write($buf, $y * $stride, $stride)
    }
    $w.Write((New-Object byte[] $andLen))
    $w.Flush()
    $w.Dispose()
}

$streams = @()
foreach ($s in $Sizes) {
    $img = New-Object System.IO.MemoryStream
    Add-IconImage -size $s -sink $img
    $streams += , $img
}
$images = @()
foreach ($st in $streams) { $images += , $st.ToArray() }

$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($ms)
# ICONDIR
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
for ($i = 0; $i -lt $Sizes.Count; $i++) {
    $s = $Sizes[$i]
    $dim = if ($s -ge 256) { 0 } else { $s }
    $w.Write([byte]$dim); $w.Write([byte]$dim)   # width / height（0 表示 256）
    $w.Write([byte]0); $w.Write([byte]0)         # 调色板数 / 保留
    $w.Write([uint16]1); $w.Write([uint16]32)    # 平面数 / 位深
    $w.Write([uint32]$images[$i].Length); $w.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $w.Write($img) }
$w.Flush()

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $OutFile) | Out-Null
[System.IO.File]::WriteAllBytes($OutFile, $ms.ToArray())
$w.Dispose(); $ms.Dispose()

$len = (Get-Item -LiteralPath $OutFile).Length
Write-Host "已生成 $OutFile（$($Sizes -join '/')，共 $len 字节）"

# ── 顺带出一张 PNG，给安装向导当 logo ────────────────────────────────────────
# 为什么要脚本一起生成：从前向导的 logo 是另外手放的图片（还是 Tauri 时代的旧图），
# 换图标时没人想起它 —— 安装界面就一直顶着旧 logo。同一个来源就不会再对不上。
if ($LogoOut) {
    $bmp = New-Object System.Drawing.Bitmap(256, 256, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::Transparent)
    $rect = New-Object System.Drawing.RectangleF(0, 0, 256, 256)
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $rect, $BlueTop, $BlueBottom, [System.Drawing.Drawing2D.LinearGradientMode]::Vertical)
    $path = New-RoundRect 1 1 254 254 56
    $g.FillPath($grad, $path)
    $font = New-Object System.Drawing.Font("Microsoft YaHei UI", 150, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $brush = New-Object System.Drawing.SolidBrush($White)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = [System.Drawing.StringAlignment]::Center
    $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
    $textRect = New-Object System.Drawing.RectangleF(0, 6, 256, 256)
    $g.DrawString("迅", $font, $brush, $textRect, $fmt)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogoOut) | Out-Null
    $bmp.Save($LogoOut, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose(); $grad.Dispose(); $path.Dispose(); $font.Dispose(); $brush.Dispose()
    Write-Host "已生成 $LogoOut（256×256 PNG，供安装向导显示）"
}
