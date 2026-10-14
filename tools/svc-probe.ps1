# 服务探针：直接跟 xunyin-svc.exe 说话（命名管道协议：4 字节小端长度 + JSON）。
#
# 用途：开发期在不依赖宿主应用（Edge/ChatGPT）的情况下驱动"输入逻辑",
# 复现服务侧问题并把结果打回控制台。
#
# 例：
#   powershell -File tools\svc-probe.ps1 -Keys "N,I,H,A,O,SPACE" -Times 5
#   powershell -File tools\svc-probe.ps1 -Keys "N,I" -Env @{XUNYIN_SVC_RAW_KEY="1"}
param(
    [string[]]$Keys = @("N", "I", "H", "A", "O", "SPACE"),
    [int]$Times = 1,
    # 额外环境变量，形如 "XUNYIN_SVC_LEVEL=1;XUNYIN_SVC_NO_ASSOC=1"
    [string]$EnvVars = "",
    [int]$Port = 0,
    [switch]$KeepAlive,
    [string]$Exe = ""
)

$ErrorActionPreference = "Stop"
# 允许 -Keys "N,I,H" 这种写法（PowerShell 会把整串塞成一个元素）
if ($Keys.Count -ge 1) { $Keys = @($Keys | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne "" }) }
$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) {
    $Exe = Join-Path $root "build\dev-deploy\tsf\xunyin-svc.exe"
}
if (-not (Test-Path -LiteralPath $Exe)) { throw "找不到服务：$Exe" }

$logPath = Join-Path $env:APPDATA "com.xunyin.ime\logs\svc.log"
$before = if (Test-Path -LiteralPath $logPath) { (Get-Item $logPath).Length } else { 0 }

Get-Process xunyin-svc -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

# 注意：宿主进程里的 TSF DLL 会在服务消失后自动把它拉起来（不带我们的调试环境变量）。
# 所以这里"起了再连"必须核对 hello_ack 里的 pid 是不是我们起的那个，不是就重来。
$VK = @{}
foreach ($c in [char[]]("ABCDEFGHIJKLMNOPQRSTUVWXYZ")) { $VK["$c"] = [int][char]$c }
foreach ($d in 0..9) { $VK["$d"] = 0x30 + $d }
$VK["SPACE"] = 0x20; $VK["ENTER"] = 0x0D; $VK["BACK"] = 0x08
$VK["ESC"] = 0x1B; $VK["LEFT"] = 0x25; $VK["RIGHT"] = 0x27
$VK["SEMI"] = 0xBA; $VK["COMMA"] = 0xBC; $VK["PERIOD"] = 0xBE
$VK["SLASH"] = 0xBF; $VK["QUOTE"] = 0xDE; $VK["MINUS"] = 0xBD
$VK["TAB"] = 0x09; $VK["SHIFT"] = 0x10

function Send-Msg($pipe, $Text) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $len = [BitConverter]::GetBytes([uint32]$bytes.Length)
    $pipe.Write($len, 0, 4)
    $pipe.Write($bytes, 0, $bytes.Length)
    $pipe.Flush()
}
function Read-Msg($pipe) {
    $lenBuf = New-Object byte[] 4
    $n = 0
    while ($n -lt 4) {
        $r = $pipe.Read($lenBuf, $n, 4 - $n)
        if ($r -le 0) { return $null }
        $n += $r
    }
    $len = [BitConverter]::ToUInt32($lenBuf, 0)
    $buf = New-Object byte[] $len
    $n = 0
    while ($n -lt $len) {
        $r = $pipe.Read($buf, $n, $len - $n)
        if ($r -le 0) { return $null }
        $n += $r
    }
    return [System.Text.Encoding]::UTF8.GetString($buf)
}

$proc = $null
$pipe = $null
for ($attempt = 1; $attempt -le 4; $attempt++) {
    Get-Process xunyin-svc -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 400
    $env:XUNYIN_SVC_INPUT = "1"
    foreach ($pair in @($EnvVars -split '[;,]' | Where-Object { $_ -match '=' })) {
        $kv = $pair.Split('=', 2)
        Set-Item -Path "Env:$($kv[0])" -Value $kv[1]
    }
    $proc = Start-Process -FilePath $Exe -PassThru -WindowStyle Hidden
    Start-Sleep -Milliseconds 600
    $pipeShort = if ($env:XUNYIN_SVC_PIPE) { $env:XUNYIN_SVC_PIPE -replace '^\\\\\.\\pipe\\', '' } else { 'xunyin-ime-svc-v1' }
    $p = New-Object System.IO.Pipes.NamedPipeClientStream(".", $pipeShort, [System.IO.Pipes.PipeDirection]::InOut)
    try { $p.Connect(3000) } catch { Write-Host "第 $attempt 次：连不上服务，重试"; continue }
    Send-Msg $p ('{"t":"hello","proto":1,"pid":' + $PID + ',"dll_tag":"probe"}')
    $ack = Read-Msg $p
    $ackPid = 0
    if ($ack) { $ackPid = ([regex]::Match($ack, '"pid":(\d+)')).Groups[1].Value }
    if ("$ackPid" -ne "$($proc.Id)") {
        # 连到的是宿主 DLL 顺手拉起来的服务（没有我们的调试环境变量），重来
        Write-Host "第 $attempt 次：连到 pid=$ackPid（不是我们起的 $($proc.Id)），重试"
        $p.Dispose()
        continue
    }
    Write-Host "已连上我们启动的服务 pid=$($proc.Id) 环境变量=$EnvVars"
    $pipe = $p
    break
}
if (-not $pipe) { throw "连不上自己启动的服务实例" }

$died = 0
for ($i = 1; $i -le $Times; $i++) {
    if ($proc.HasExited) { $died = 1; break }
    foreach ($k in $Keys) {
        # 特殊动作：SELECT:<n> 点击第 n 个候选；CMD:<name> 发命令（toggle_mode 等）
        if ($k -like "WARMUP") {
            Send-Msg $pipe '{"t":"warmup"}'
            $reply = Read-Msg $pipe
            if ($null -eq $reply) { Write-Host "[$i] WARMUP 时连接中断（服务已崩）"; $died = 1; break }
            Write-Host "[$i] WARMUP -> $reply"
            continue
        }
        if ($k -like "SELECT:*") {
            $idx = [int]($k -replace "SELECT:", "")
            Send-Msg $pipe ('{"t":"select","index":' + $idx + '}')
            $reply = Read-Msg $pipe
            if ($null -eq $reply) { Write-Host "[$i] SELECT 时连接中断（服务已崩）"; $died = 1; break }
            Write-Host "[$i] SELECT:$idx -> $reply"
            continue
        }
        if ($k -like "CMD:*") {
            $name = $k -replace "CMD:", ""
            Send-Msg $pipe ('{"t":"command","name":"' + $name + '"}')
            $reply = Read-Msg $pipe
            if ($null -eq $reply) { Write-Host "[$i] CMD 时连接中断（服务已崩）"; $died = 1; break }
            Write-Host "[$i] CMD:$name -> $reply"
            continue
        }
        $up = $k.ToUpper()
        if (-not $VK.ContainsKey($up)) { Write-Host "跳过未知键 $k"; continue }
        $payload = '{"t":"key","vk":' + $VK[$up] + ',"mask":' + $(if ($up -eq "SHIFT") { 1 } else { 0 }) + ',"shift":' + $(if ($up -eq "SHIFT") { "true" } else { "false" }) + '}'
        Send-Msg $pipe $payload
        $reply = Read-Msg $pipe
        if ($null -eq $reply) { Write-Host "[$i] 连接中断（服务已崩）"; $died = 1; break }
        Write-Host "[$i] $up -> $reply"
    }
    if ($died) { break }
}
if ($KeepAlive -and -not $died) {
    Write-Host "保持连接中，按回车退出…"
    Read-Host | Out-Null
}
$pipe.Dispose()
Start-Sleep -Milliseconds 200
if ($proc.HasExited) { $died = 1 }
if ($died) {
    Write-Host "== 服务进程已退出，退出码=$($proc.ExitCode) =="
} else {
    Write-Host "== 服务存活 =="
}

# 打印本次新增的服务日志
if (Test-Path -LiteralPath $logPath) {
    Write-Host "---- svc.log 新增内容 ----"
    $fs = [System.IO.File]::Open($logPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    try {
        $fs.Seek($before, [System.IO.SeekOrigin]::Begin) | Out-Null
        $sr = New-Object System.IO.StreamReader($fs)
        while (-not $sr.EndOfStream) { Write-Host $sr.ReadLine() }
    } finally { $fs.Dispose() }
}
