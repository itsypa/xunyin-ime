param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    # 原生设置中心（Win32，无 WebView）：debug 版窗口出现也只要 ~130ms，
    # release 版更小更快（~30ms）。安装包一律用 release。
    # 默认出 release（更小更快）；日常改设置界面用 -Debug 迭代
    [switch]$Debug,
    [switch]$SkipBuild,
    [switch]$SkipSign,
    [switch]$SkipLaunch
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$nativeDir = Join-Path $root "xunyin-ime\native"
$certThumb = "109B04E710355C66C85B025C8E939B0F778580CB"

$targetArg = @()
$outSub = if ($Debug) { "debug" } else { "release" }
if ($Arch -eq "x86") {
    $targetArg = @("--target", "i686-pc-windows-msvc")
    $outSub = if ($Debug) { "i686-pc-windows-msvc\debug" } else { "i686-pc-windows-msvc\release" }
}

if (-not $SkipBuild) {
    Push-Location $nativeDir
    try {
        # native 命令写 stderr 时不要抛错（$ErrorActionPreference=Stop 会把它当终止错误）
        $ErrorActionPreference = "Continue"
        # 注意：空数组用 @var 展开时会多传一个空参数，cargo 会报 “unexpected argument '-'”。
        # 用参数数组拼接，只在实际有内容时才带上。
        $cargoArgs = @("build")
        if ($targetArg.Count -gt 0) { $cargoArgs += $targetArg }
        if (-not $Debug) { $cargoArgs += "--release" }
        & cargo @cargoArgs 2>&1 | Out-Host
        $code = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        if ($code -ne 0) { throw "cargo build failed ($Arch)" }
    } finally {
        Pop-Location
    }
}

$exe = Join-Path $nativeDir "target\$outSub\xunyin-ime.exe"
if (-not (Test-Path $exe)) { throw "settings exe not found: $exe" }

if (-not $SkipSign) {
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Thumbprint -eq $certThumb }
    if (-not $cert) { throw "signing cert not found: $certThumb" }
    Set-AuthenticodeSignature -FilePath $exe -Certificate $cert | Out-Null
    $sig = Get-AuthenticodeSignature $exe
    Write-Host "[$Arch] signed: $($sig.Status) ($exe)"
}

# 数据目录不用拷到 exe 旁：程序从 exe 目录逐级向上找 rime\data
# （开发布局 xunyin-ime\rime\data；安装布局 <安装目录>\rime\data）。

if (-not $SkipLaunch) {
    Get-Process xunyin-ime -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 600
    Start-Process -FilePath $exe
    Write-Host "[$Arch] settings center launched: $exe"
}

Write-Host "deploy-settings done: $Arch -> $exe"
