param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    [string]$DllName = "lingxi_tsf_v53.dll",
    [switch]$SkipBuild,
    [switch]$SkipSign,
    [switch]$SkipRestart
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tsfDir = Join-Path $root "lingxi-tsf"
$certThumb = "109B04E710355C66C85B025C8E939B0F778580CB"
$clsid = "{F3A2D6E1-4C5B-4E8A-9C1D-2A6B7C8D9E0F}"

if ($Arch -eq "x64") {
    $target = "x86_64-pc-windows-msvc"
    $targetSub = "debug"
    $regFlag = "/reg:64"
    $regsvr = (Join-Path $env:WINDIR "System32\regsvr32.exe")
} else {
    $target = "i686-pc-windows-msvc"
    $targetSub = "i686-pc-windows-msvc\debug"
    $regFlag = "/reg:32"
    $regsvr = (Join-Path $env:WINDIR "SysWOW64\regsvr32.exe")
}

if (-not $SkipBuild) {
    Push-Location $tsfDir
    try {
        if ($Arch -eq "x64") {
            cargo build 2>&1 | Out-Host
        } else {
            cargo build --target $target 2>&1 | Out-Host
        }
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($Arch)" }
    } finally {
        Pop-Location
    }
}

$outDir = Join-Path $tsfDir "target\$targetSub"
$src = Join-Path $outDir "lingxi_tsf.dll"
$dst = Join-Path $outDir $DllName
if (-not (Test-Path $src)) { throw "build output not found: $src" }
Copy-Item -LiteralPath $src -Destination $dst -Force

if (-not $SkipSign) {
    $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Thumbprint -eq $certThumb }
    if (-not $cert) { throw "signing cert not found: $certThumb" }
    Set-AuthenticodeSignature -FilePath $dst -Certificate $cert | Out-Null
    $sig = Get-AuthenticodeSignature $dst
    Write-Host "[$Arch] signed: $($sig.Status) ($dst)"
}

# 注册 COM InprocServer32（按 WOW64 视图写入，保证 32/64 位应用各自加载对应 DLL）
$paths = @(
    "HKCU\Software\Classes\CLSID\$clsid\InprocServer32",
    "HKLM\SOFTWARE\Classes\CLSID\$clsid\InprocServer32"
)
foreach ($p in $paths) {
    reg add $p /ve /d $dst /f $regFlag | Out-Null
    reg add $p /v ThreadingModel /t REG_SZ /d Apartment /f $regFlag | Out-Null
}
Write-Host "[$Arch] InprocServer32 registered (both HKCU/HKLM, $regFlag): $dst"

# 走一次 DllRegisterServer，刷新 TIP 语言档案/类别注册（HKCU，无需管理员）
Start-Process -FilePath $regsvr -ArgumentList "/s `"$dst`"" -Wait -WindowStyle Hidden
Write-Host "[$Arch] regsvr32 DllRegisterServer done"

if (-not $SkipRestart) {
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    Stop-Process -Name TextInputHost -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
    Write-Host "[$Arch] ctfmon/TextInputHost restarted"
}

Write-Host "deploy-tsf done: $Arch -> $dst"
