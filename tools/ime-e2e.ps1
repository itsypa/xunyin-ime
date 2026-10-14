# End-to-end smoke test for the IME: open a fresh host process (Notepad), type with
# synthetic keystrokes, then read back what actually landed in the edit control.
#
# Why: the DLL is loaded per host process. A change to the DLL only shows up in a
# process started AFTER the deploy, so this script starts one and drives it.
# It also prints the DLL/service log lines produced by that new pid.
#
# Usage:
#   powershell -File tools\ime-e2e.ps1
#   powershell -File tools\ime-e2e.ps1 -Text "nihao" -Expect "你好"
#   powershell -File tools\ime-e2e.ps1 -Keys "N,I,H,A,O,SPACE"   # raw virtual keys
#
# IMPORTANT: this needs an interactive, unlocked desktop. On a locked / headless
# session the injected keys are accepted by the API but never reach the window, so
# the edit box stays empty and the result is FAIL even when the IME is fine.
# Rule of thumb: if the very first letters do not even appear, it is the harness,
# not the IME.
param(
    [string]$Text = "nihao",
    [int]$SettleMs = 1500,
    [string]$Expect = "",
    [switch]$KeepOpen
)

$ErrorActionPreference = "Stop"

Add-Type -Namespace Ime -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int c);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowTextW(System.IntPtr h, System.Text.StringBuilder s, int n);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetClassNameW(System.IntPtr h, System.Text.StringBuilder s, int n);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern System.IntPtr FindWindowExW(System.IntPtr parent, System.IntPtr after, string cls, string title);

[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public System.IntPtr dwExtraInfo; }
[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct INPUT { public uint type; public KEYBDINPUT ki; public int pad1; public int pad2; }
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] p, int size);

public static void Key(ushort vk) {
    INPUT[] a = new INPUT[2];
    a[0].type = 1; a[0].ki.wVk = vk;
    a[1].type = 1; a[1].ki.wVk = vk; a[1].ki.dwFlags = 2; // KEYEVENTF_KEYUP
    SendInput(2, a, System.Runtime.InteropServices.Marshal.SizeOf(typeof(INPUT)));
}
'@

function Get-ChildEdit([IntPtr]$parent) {
    foreach ($cls in @('Edit', 'RichEditD2DPT', 'RICHEDIT50W', 'EDIT')) {
        $h = [Ime.Native]::FindWindowExW($parent, [IntPtr]::Zero, $cls, $null)
        if ($h -ne [IntPtr]::Zero) { return $h }
    }
    return [IntPtr]::Zero
}

function Get-Text([IntPtr]$h) {
    if ($h -eq [IntPtr]::Zero) { return "" }
    $sb = New-Object System.Text.StringBuilder 4096
    [Ime.Native]::GetWindowTextW($h, $sb, 4096) | Out-Null
    return $sb.ToString()
}

$logPath = Join-Path $env:APPDATA "com.xunyin.ime\logs\tsf.log"
$before = if (Test-Path -LiteralPath $logPath) { (Get-Item -LiteralPath $logPath).Length } else { 0 }

$proc = Start-Process -FilePath (Join-Path $env:WINDIR "System32\notepad.exe") -PassThru
try {
    for ($i = 0; $i -lt 30 -and $proc.MainWindowHandle -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 200
        $proc.Refresh()
    }
    $hwnd = $proc.MainWindowHandle
    if ($hwnd -eq [IntPtr]::Zero) { throw "记事本窗口没出现" }
    [Ime.Native]::ShowWindow($hwnd, 9) | Out-Null   # SW_RESTORE
    [Ime.Native]::SetForegroundWindow($hwnd) | Out-Null
    Start-Sleep -Milliseconds $SettleMs
    "记事本 pid=$($proc.Id) hwnd=$hwnd 前台=$([Ime.Native]::GetForegroundWindow())"

    $edit = Get-ChildEdit $hwnd
    "编辑框 hwnd=$edit"

    # Type lowercase letters of $Text, then Space (that is what selects the candidate).
    foreach ($ch in $Text.ToCharArray()) {
        $vk = [int][char]([string]$ch).ToUpper()
        [Ime.Native]::Key([uint16]$vk)
        Start-Sleep -Milliseconds 60
    }
    Start-Sleep -Milliseconds 250
    [Ime.Native]::Key(0x20)   # SPACE
    Start-Sleep -Milliseconds $SettleMs

    $got = Get-Text $edit
    "上屏内容: [$got]"
    if ($Expect -ne "") {
        if ($got -eq $Expect) { "结果: PASS（等于 [$Expect]）" } else { "结果: FAIL（期望 [$Expect]）" }
    }
}
finally {
    if (-not $KeepOpen) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    "---- tsf.log 新增 ----"
    if (Test-Path -LiteralPath $logPath) {
        $fs = [System.IO.File]::Open($logPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        try {
            $fs.Seek($before, [System.IO.SeekOrigin]::Begin) | Out-Null
            $sr = New-Object System.IO.StreamReader($fs)
            $n = 0
            while (-not $sr.EndOfStream -and $n -lt 60) { Write-Host $sr.ReadLine(); $n++ }
        }
        finally { $fs.Dispose() }
    }
}
