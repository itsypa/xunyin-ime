# 用消息驱动设置中心窗口做"点击 + 输入"的端到端验证（没有鼠标时用来测按钮逻辑）。
#
# 读 XUNYIN_UI_DUMP 导出的 hits.txt 拿到控件矩形，往窗口 PostMessage：
# 先点输入框（得到焦点）、再发 WM_CHAR 输入文字、最后点按钮。
#
# 用法：powershell -File tools\ui-click-test.ps1 -PageExe <设置中心 exe> -Dump <dump 目录>
param(
    [string]$Exe = "xunyin-ime\native\target\release\xunyin-ime.exe",
    [string]$Dump = "build\ui-dump",
    # 走一遍「填编码 + 填词条 + 点添加」，验证设置中心的按钮逻辑
    [string]$Code = "xy",
    [string]$Phrase = "迅音输入法",
    [int]$WaitMs = 700,
    # 打完字后为真就不再点"添加"（用来观察输入框收到了什么）
    [switch]$SkipClick,
    # 改走删除流程：点第 N 行（0 起）再点"删除所选"
    [int]$DeleteRow = -1
)

$ErrorActionPreference = "Stop"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Msg
{
    [DllImport("user32.dll")] private static extern bool SetProcessDPIAware();
    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "PostMessageW")]
    private static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);

    public static void MakeDpiAware() { SetProcessDPIAware(); }
    // 参数走 int，转换在 C# 里做：PowerShell 直接传 IntPtr 时会丢高位（实测 0x4E2D 变成 0x2D）
    public static void Click(IntPtr h, int x, int y)
    {
        IntPtr lp = (IntPtr)((y << 16) | (x & 0xFFFF));
        PostMessageW(h, 0x0201, (IntPtr)1, lp);
        PostMessageW(h, 0x0202, IntPtr.Zero, lp);
    }
    public static void TypeChar(IntPtr h, int code) { PostMessageW(h, 0x0102, (IntPtr)code, IntPtr.Zero); }
}
'@
[Msg]::MakeDpiAware()

$exe = (Resolve-Path $Exe).Path
$dump = (Resolve-Path $Dump).Path
$env:XUNYIN_UI_DUMP = $dump
$env:XUNYIN_UI_PAGE = "2"

Get-Process xunyin-ime -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item (Join-Path $dump "hits.txt") -ErrorAction SilentlyContinue
$proc = Start-Process -FilePath $exe -PassThru
Start-Sleep -Milliseconds 1800
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) { throw "拿不到窗口句柄" }

function Read-Hits($dump) {
    $map = @{}
    foreach ($line in (Get-Content (Join-Path $dump "hits.txt") -ErrorAction SilentlyContinue)) {
        if ($line -match '^(\S+) (\d+) (\d+) (\d+) (\d+)$') {
            $map[$matches[1]] = @([int]$matches[2], [int]$matches[3], [int]$matches[4], [int]$matches[5])
        }
    }
    return $map
}
function Post-Click($hwnd, $r) {
    $x = [int](($r[0] + $r[2]) / 2); $y = [int](($r[1] + $r[3]) / 2)
    [Msg]::Click($hwnd, $x, $y)
}
function Post-Text($hwnd, $s) {
    foreach ($ch in $s.ToCharArray()) {
        $code = [int][char]$ch; [Msg]::TypeChar($hwnd, $code)
        Start-Sleep -Milliseconds 40
    }
}

$hits = Read-Hits $dump
if ($DeleteRow -ge 0) {
    $rowName = "PhraseRow($DeleteRow)"
    if (-not $hits.ContainsKey($rowName)) { throw "hits.txt 里没有 $rowName" }
    Post-Click $hwnd $hits[$rowName]
    Start-Sleep -Milliseconds $WaitMs
    if (-not $hits.ContainsKey("PhraseDel")) { throw "hits.txt 里没有按钮 PhraseDel" }
    Post-Click $hwnd $hits["PhraseDel"]
    Start-Sleep -Milliseconds 1200
    Write-Host "已选中 $rowName 并点击删除"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    exit 0
}
foreach ($item in @(@("PhraseCode", $Code), @("PhraseText", $Phrase))) {
    $name = $item[0]
    if (-not $hits.ContainsKey($name)) { throw "hits.txt 里没有控件 $name" }
    Post-Click $hwnd $hits[$name]
    Start-Sleep -Milliseconds $WaitMs
    Post-Text $hwnd $item[1]
    Start-Sleep -Milliseconds $WaitMs
    Write-Host "已填写 $name = $($item[1])"
}
if ($SkipClick) {
    Write-Host "（已跳过点击添加）"
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    exit 0
}
if (-not $hits.ContainsKey("PhraseAdd")) { throw "hits.txt 里没有按钮 PhraseAdd" }
Post-Click $hwnd $hits["PhraseAdd"]
Start-Sleep -Milliseconds 1200
Write-Host "已点击 $Click"
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
