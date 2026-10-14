# 检查设置中心的布局导出（XUNYIN_UI_DUMP）：卡片是否重叠 / 越界 / 与窗口边缘打架。
#
# 用法：
#   $env:XUNYIN_UI_DUMP="$PWD\build\ui-dump"; $env:XUNYIN_UI_PAGE="0"
#   .\xunyin-ime\native\target\debug\xunyin-ime.exe      # 跑一下让它重绘
#   powershell -File tools\check-ui-dump.ps1 -Dir build\ui-dump
param(
    [string]$Dir = "build\ui-dump",
    [int]$Sidebar = 318,   # 侧栏右边界（物理像素，150% 缩放时 212*1.5）
    [int]$Margin = 8
)

$ErrorActionPreference = "Stop"
$paint = Join-Path $Dir "paint.log"
$cards = Join-Path $Dir "cards.txt"
if (-not (Test-Path $cards)) { throw "没有 $cards，先带 XUNYIN_UI_DUMP 跑一次设置中心" }

$last = (Get-Content $paint -Tail 1)
$m = [regex]::Match($last, "client=(\d+)x(\d+)")
$cw = [int]$m.Groups[1].Value
$ch = [int]$m.Groups[2].Value
Write-Output "最后一次重绘：$last"

$rects = @()
foreach ($line in (Get-Content $cards | Select-Object -Unique)) {
    if ($line -match '^(\d+) (\d+) (\d+) (\d+)$') {
        $rects += [pscustomobject]@{
            L = [int]$matches[1]; T = [int]$matches[2]; R = [int]$matches[3]; B = [int]$matches[4]
            Idx = $rects.Count
        }
    }
}
Write-Output "卡片数：$($rects.Count)"

$bad = 0
foreach ($r in $rects) {
    if ($r.L -lt ($Sidebar + $Margin)) { Write-Output "❌ 卡片 $($r.Idx) 左边压到侧栏：L=$($r.L)"; $bad++ }
    if ($r.R -gt ($cw - $Margin)) { Write-Output "❌ 卡片 $($r.Idx) 右越界：R=$($r.R) > $cw"; $bad++ }
    if ($r.R -le $r.L -or $r.B -le $r.T) { Write-Output "❌ 卡片 $($r.Idx) 尺寸非法：$($r.L),$($r.T),$($r.R),$($r.B)"; $bad++ }
}
for ($i = 0; $i -lt $rects.Count; $i++) {
    for ($j = $i + 1; $j -lt $rects.Count; $j++) {
        $a = $rects[$i]; $b = $rects[$j]
        if ($a.L -eq $b.L -and $a.T -eq $b.T -and $a.R -eq $b.R -and $a.B -eq $b.B) { continue }
        $overlap = -not ($a.R -le $b.L -or $b.R -le $a.L -or $a.B -le $b.T -or $b.B -le $a.T)
        if ($overlap) {
            Write-Output "❌ 卡片 $($a.Idx) 与 $($b.Idx) 重叠：[$($a.L),$($a.T),$($a.R),$($a.B)] / [$($b.L),$($b.T),$($b.R),$($b.B)]"
            $bad++
        }
    }
}
if ($bad -eq 0) { Write-Output "✅ 没有重叠、越界或压到侧栏的卡片" } else { Write-Output "共 $bad 处问题" }

# ── 控件是否都在某张卡片里 ────────────────────────────────────────────
# 这条能抓住"控件画到卡片外面去了"（历史上出现过：按钮溢出卡片右边缘、设置项
# 被预览挡住）。侧栏导航不在卡片里，跳过；容差 2px 是给 1px 描边留的。
$hits = Join-Path $Dir "hits.txt"
if (Test-Path $hits) {
    $outside = 0
    foreach ($line in (Get-Content $hits | Select-Object -Unique)) {
        if ($line -notmatch '^(\S+) (\d+) (\d+) (\d+) (\d+)$') { continue }
        $id = $matches[1]; $l = [int]$matches[2]; $t = [int]$matches[3]; $r = [int]$matches[4]; $b = [int]$matches[5]
        if ($id -like "Nav(*") { continue }
        $inside = $false
        foreach ($c in $rects) {
            if ($l -ge ($c.L - 2) -and $t -ge ($c.T - 2) -and $r -le ($c.R + 2) -and $b -le ($c.B + 2)) { $inside = $true; break }
        }
        if (-not $inside) {
            Write-Output "❌ 控件 $id 不在任何卡片内：[$l,$t,$r,$b]"
            $outside++
        }
    }
    if ($outside -eq 0) { Write-Output "✅ 所有控件都在卡片内" } else { Write-Output "共 $outside 个控件画到了卡片外" }
}
