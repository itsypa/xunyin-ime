param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    # 出 release 版：启动快得多（实测窗口出现 60~100ms vs 调试版 ~2.1s），
    # 状态栏齿轮拉起的就是它；开发迭代用默认的 debug 即可。
    [switch]$Release,
    [switch]$SkipFrontend,
    [switch]$SkipBuild,
    [switch]$SkipSign,
    [switch]$SkipLaunch
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$imeDir = Join-Path $root "lingxi-ime"
$certThumb = "109B04E710355C66C85B025C8E939B0F778580CB"

if (-not $SkipFrontend) {
    Push-Location $imeDir
    try {
        # native 命令写 stderr 时不要抛错（$ErrorActionPreference=Stop 会把它当终止错误）
        $ErrorActionPreference = "Continue"
        npm run build 2>&1 | Out-Host
        $code = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        if ($code -ne 0) { throw "npm run build failed" }
    } finally {
        Pop-Location
    }
}

$targetArg = @()
$outSub = if ($Release) { "release" } else { "debug" }
if ($Arch -eq "x86") {
    $targetArg = @("--target", "i686-pc-windows-msvc")
    $outSub = if ($Release) { "i686-pc-windows-msvc\release" } else { "i686-pc-windows-msvc\debug" }
}

if (-not $SkipBuild) {
    Push-Location (Join-Path $imeDir "src-tauri")
    try {
        # 注意：必须启用 custom-protocol 特性，否则 exe 会去找 Vite 开发服务器
        # （devUrl=http://localhost:1420），导致设置中心"localhost 拒绝连接"。
        $ErrorActionPreference = "Continue"
        $profileArg = if ($Release) { @("--release") } else { @() }
        cargo build @targetArg @profileArg --features custom-protocol 2>&1 | Out-Host
        $code = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        if ($code -ne 0) { throw "cargo build failed ($Arch)" }
    } finally {
        Pop-Location
    }
}

$exe = Join-Path $imeDir "src-tauri\target\$outSub\lingxi-ime.exe"
if (-not (Test-Path $exe)) { throw "settings exe not found: $exe" }

if (-not $SkipSign) {
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Thumbprint -eq $certThumb }
    if (-not $cert) { throw "signing cert not found: $certThumb" }
    Set-AuthenticodeSignature -FilePath $exe -Certificate $cert | Out-Null
    $sig = Get-AuthenticodeSignature $exe
    Write-Host "[$Arch] signed: $($sig.Status) ($exe)"
}

# 确保 rime/data 随 exe 就位（开发布局：exe 旁 rime/data）
$dataSrc = Join-Path $imeDir "src-tauri\rime\data"
$dataDst = Join-Path (Split-Path $exe) "rime\data"
if (-not (Test-Path (Join-Path $dataDst "default.yaml")) -and (Test-Path (Join-Path $dataSrc "default.yaml"))) {
    New-Item -ItemType Directory -Path $dataDst -Force | Out-Null
    Copy-Item -Path (Join-Path $dataSrc "*") -Destination $dataDst -Recurse -Force
    Write-Host "[$Arch] rime/data copied next to exe"
}

if (-not $SkipLaunch) {
    Get-Process lingxi-ime -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 600
    Start-Process -FilePath $exe
    Write-Host "[$Arch] settings center launched: $exe"
}

Write-Host "deploy-settings done: $Arch -> $exe"
