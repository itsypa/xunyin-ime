# 把状态栏资源（.bin = 8 字节头「宽 + 高」+ 宽×高×4 字节 BGRA）的 alpha 通道
# 打成 ASCII 点阵，用来"看"图标长什么样（没有看图工具时用）。
#
# 用法：powershell -File tools\render-asset.ps1 -Path xunyin-tsf\src\assets\glyph_ban.bin
param(
    [Parameter(Mandatory = $true)][string]$Path,
    [int]$Threshold = 40
)

$ErrorActionPreference = "Stop"
$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path).Path)
$w = [BitConverter]::ToUInt32($bytes, 0)
$h = [BitConverter]::ToUInt32($bytes, 4)
Write-Output "$Path  ${w}x${h}"
for ($y = 0; $y -lt $h; $y++) {
    $line = New-Object System.Text.StringBuilder
    for ($x = 0; $x -lt $w; $x++) {
        $i = 8 + ($y * $w + $x) * 4
        $a = $bytes[$i + 3]
        $ch = if ($a -ge 200) { "#" } elseif ($a -ge $Threshold) { "+" } elseif ($a -gt 0) { "." } else { " " }
        [void]$line.Append($ch)
    }
    Write-Output $line.ToString()
}
