param(
    [string]$OutDir = "D:\灵犀输入法\lingxi-tsf\src\assets"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

# 搜狗默认配色 + 现代药丸：浅灰底 #F7F7F7、灰边框、两端半圆、橙色圆 Logo、
# 蓝色文字按钮（中/英、全/半、标点）+ 软键盘 + 菜单。全部从零绘制，无重叠。
# 资源按 2 倍分辨率渲染（逻辑 170x34 @96dpi → 340x68），运行时按实际 DPI 缩放，
# 保证 125%/150%/200% 缩放下文字依然清晰（解决"像素抠图/字体难看"）。
$S = 2
$W = 170 * $S   # 340
$H = 34 * $S    # 68
$R = 17 * $S    # 半圆端
$out = $OutDir
New-Item -ItemType Directory -Force -Path $out | Out-Null

$Blue = [System.Drawing.Color]::FromArgb(255, 76, 163, 224)
$Dark = [System.Drawing.Color]::FromArgb(255, 51, 51, 51)

function New-Bmp() {
    $b = New-Object System.Drawing.Bitmap($W, $H, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
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
    $bw2 = New-Object System.IO.BinaryWriter($fs)
    $bw2.Write([uint32]$bw); $bw2.Write([uint32]$bh)
    $bw2.Write($bytes)
    $bw2.Close()
    $ctx.G.Dispose(); $b.Dispose()
}

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function Draw-Bar($g, $hoverId) {
    # 柔和投影（两层）
    foreach ($shd in @(@{dx=0; dy=2; a=26}, @{dx=1; dy=3; a=14})) {
        $sh = RoundRect (0.5 + $shd.dx) (0.5 + $shd.dy) ($W - 1) ($H - 1) $R
        $sb = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb($shd.a, 30, 45, 70))
        $g.FillPath($sb, $sh)
        $sb.Dispose(); $sh.Dispose()
    }
    # 药丸：浅灰底 + 灰边框（半圆端）
    $body = RoundRect 0.5 0.5 ($W - 1) ($H - 1) $R
    $g.FillPath((New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 247, 247, 247))), $body)
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 189, 189, 189), 1)
    $g.DrawPath($pen, $body)
    $pen.Dispose(); $body.Dispose()

    # 悬停：浅蓝圆角高亮（仅 Logo/中英/全半/菜单 四项）
    $btns = @(
        @{ id = 0; x = 14; w = 22 },
        @{ id = 1; x = 48; w = 36 },
        @{ id = 2; x = 88; w = 34 },
        @{ id = 3; x = 134; w = 22 }
    )
    foreach ($b in $btns) {
        if ($hoverId -eq $b.id) {
            $hl = RoundRect (($b.x - 1) * $S) (4 * $S) (($b.w + 2) * $S) ($H - 8 * $S) (11 * $S)
            $hb = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 217, 233, 247))
            $g.FillPath($hb, $hl)
            $hb.Dispose()
            $hp = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(255, 183, 211, 236), 1)
            $g.DrawPath($hp, $hl)
            $hp.Dispose(); $hl.Dispose()
        }
    }

    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center

    # 橙色圆 Logo + 白色「灵」
    $logoRect = New-Object System.Drawing.RectangleF((14 * $S), (6 * $S), (22 * $S), (22 * $S))
    $lg = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF((14 * $S), (6 * $S))),
        (New-Object System.Drawing.PointF((36 * $S), (28 * $S))),
        [System.Drawing.Color]::FromArgb(255, 255, 138, 8),
        [System.Drawing.Color]::FromArgb(255, 247, 77, 24))
    $g.FillEllipse($lg, $logoRect)
    $lg.Dispose()
    $fLogo = New-Object System.Drawing.Font("Microsoft YaHei UI", (13 * $S), [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $g.DrawString("灵", $fLogo, [System.Drawing.Brushes]::White, $logoRect, $sf)
    $fLogo.Dispose()

    # 菜单 ≡（三条横杠 = 设置入口）
    $fMenu = New-Object System.Drawing.Font("Segoe UI Symbol", (13 * $S), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $mb = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 110, 120, 132))
    $g.DrawString("≡", $fMenu, $mb, (New-Object System.Drawing.RectangleF((134 * $S), (3 * $S), (22 * $S), (28 * $S))), $sf)
    $mb.Dispose(); $fMenu.Dispose()
}

function Draw-Glyph([string]$text, [string]$name, [int]$gx, [int]$gw) {
    $ctx = New-Bmp
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $f = New-Object System.Drawing.Font("Microsoft YaHei UI", (14 * $S), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $b = New-Object System.Drawing.SolidBrush($Blue)
    $ctx.G.DrawString($text, $f, $b, (New-Object System.Drawing.RectangleF(($gx * $S), (3 * $S), ($gw * $S), (28 * $S))), $sf)
    $b.Dispose(); $f.Dispose()
    Save-Bgra $ctx $name
}

# 背景：normal + 悬停（Logo/中/全/菜单）
$c = New-Bmp; Draw-Bar $c.G -1; Save-Bgra $c "bg_normal"
foreach ($hid in 0..3) {
    $c = New-Bmp; Draw-Bar $c.G $hid
    $tag = @("logo","mode","width","menu")[$hid]
    Save-Bgra $c "bg_hover_$tag"
}

# 动态字形
Draw-Glyph "中" "glyph_zhong" 48 36
Draw-Glyph "英" "glyph_ying" 48 36
Draw-Glyph "全" "glyph_quan" 88 34
Draw-Glyph "半" "glyph_ban" 88 34

# 预览
function Read-Bgra([string]$name) {
    $fs = [System.IO.File]::OpenRead((Join-Path $out "$name.bin"))
    $br = New-Object System.IO.BinaryReader($fs)
    $w0 = $br.ReadUInt32(); $h0 = $br.ReadUInt32()
    $px = $br.ReadBytes([int]($w0 * $h0 * 4))
    $br.Close()
    return @{ W = [int]$w0; H = [int]$h0; Px = $px }
}
function Blend-Over([byte[]]$dst, [byte[]]$src) {
    for ($i = 0; $i -lt $src.Length; $i += 4) {
        $sa = $src[$i + 3]
        if ($sa -eq 0) { continue }
        if ($sa -eq 255) { $dst[$i]=$src[$i]; $dst[$i+1]=$src[$i+1]; $dst[$i+2]=$src[$i+2]; $dst[$i+3]=255; continue }
        $da = $dst[$i+3]; $oa = $sa + $da * (255 - $sa) / 255
        if ($oa -gt 0) {
            $dst[$i]   = [byte](($src[$i]*$sa + $dst[$i]*$da*(255-$sa)/255)/$oa)
            $dst[$i+1] = [byte](($src[$i+1]*$sa + $dst[$i+1]*$da*(255-$sa)/255)/$oa)
            $dst[$i+2] = [byte](($src[$i+2]*$sa + $dst[$i+2]*$da*(255-$sa)/255)/$oa)
            $dst[$i+3] = [byte]$oa
        }
    }
}
$bg = Read-Bgra "bg_normal"
$frame = [byte[]]($bg.Px.Clone())
Blend-Over $frame (Read-Bgra "glyph_zhong").Px
Blend-Over $frame (Read-Bgra "glyph_quan").Px
$previewSrc = New-Object System.Drawing.Bitmap($bg.W, $bg.H, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$rect = New-Object System.Drawing.Rectangle(0, 0, $bg.W, $bg.H)
$data = $previewSrc.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
[System.Runtime.InteropServices.Marshal]::Copy($frame, 0, $data.Scan0, $frame.Length)
$previewSrc.UnlockBits($data)
$pw = $W * 3; $ph = $H * 3
$preview = New-Object System.Drawing.Bitmap($pw, $ph, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$pg = [System.Drawing.Graphics]::FromImage($preview)
$pg.Clear([System.Drawing.Color]::White)
$pg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$pg.DrawImage($previewSrc, 0, 0, $pw, $ph)
$pg.Dispose()
$preview.Save("D:\灵犀输入法\tools\statusbar_preview.png", [System.Drawing.Imaging.ImageFormat]::Png)
$preview.Dispose(); $previewSrc.Dispose()

Get-ChildItem $out -Filter *.bin | Select-Object Name, Length
Write-Output "assets generated -> $out"
