param(
    [string]$Version = "",
    [ValidateSet("lzma", "zstd")]
    [string]$Compression = "lzma",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$installerDir = Join-Path $root "installer"
$payloadDir = Join-Path $root "build\installer-payload"
$distDir = Join-Path $root "build\installer-dist"

if (-not $Version) {
    $conf = Join-Path $root "lingxi-ime\src-tauri\tauri.conf.json"
    $Version = (Get-Content $conf -Raw | ConvertFrom-Json).version
}
Write-Host "版本: $Version  压缩: $Compression"

# ── 1. 构建 TSF（x64 / x86 release）────────────────────────────────────────
if (-not $SkipBuild) {
    Push-Location (Join-Path $root "lingxi-tsf")
    try {
        cargo build --release
        cargo build --release --target i686-pc-windows-msvc
    } finally { Pop-Location }

    # 设置中心（前端 + Tauri 后端 release）
    Push-Location (Join-Path $root "lingxi-ime")
    try {
        npm run build
        Push-Location "src-tauri"
        try { cargo build --release --features custom-protocol } finally { Pop-Location }
    } finally { Pop-Location }

    # 安装器 stub / 卸载器 / 打包器
    Push-Location $installerDir
    try { cargo build --release --features packer } finally { Pop-Location }
}

# ── 2. 组装 payload ────────────────────────────────────────────────────────
if (Test-Path $payloadDir) { Remove-Item -LiteralPath $payloadDir -Recurse -Force }
New-Item -ItemType Directory -Force $payloadDir, (Join-Path $payloadDir "tsf"), (Join-Path $payloadDir "tsf-x86") | Out-Null

$tsfX64 = Join-Path $root "lingxi-tsf\target\release\lingxi_tsf.dll"
$tsfX86 = Join-Path $root "lingxi-tsf\target\i686-pc-windows-msvc\release\lingxi_tsf.dll"
$imeExe = Join-Path $root "lingxi-ime\src-tauri\target\release\lingxi-ime.exe"
$rimeX64 = Join-Path $root "lingxi-ime\src-tauri\rime\rime.dll"
$rimeX86 = Join-Path $root "lingxi-ime\src-tauri\rime-x86\rime.dll"
$rimeData = Join-Path $root "lingxi-ime\src-tauri\rime\data"
$dictMgr = Join-Path $root "lingxi-ime\src-tauri\rime\rime_dict_manager.exe"
$uninstaller = Join-Path $installerDir "target\release\wind-uninstaller.exe"

foreach ($f in @($tsfX64, $tsfX86, $imeExe, $rimeX64, $rimeX86, $dictMgr, $uninstaller)) {
    if (-not (Test-Path $f)) { throw "缺少构建产物: $f" }
}

# 设置中心 + 引擎（rime.dll 需与 exe 同目录）
Copy-Item $imeExe (Join-Path $payloadDir "lingxi-ime.exe") -Force
Copy-Item $rimeX64 (Join-Path $payloadDir "rime.dll") -Force
Copy-Item $dictMgr (Join-Path $payloadDir "rime_dict_manager.exe") -Force
Copy-Item $uninstaller (Join-Path $payloadDir "uninstall.exe") -Force

# 共享词库
Copy-Item $rimeData (Join-Path $payloadDir "rime\data") -Recurse -Force

# TSF（x64 与其 rime.dll；x86 与其 32 位 rime.dll）
Copy-Item $tsfX64 (Join-Path $payloadDir "tsf\lingxi_tsf_x64.dll") -Force
Copy-Item $rimeX64 (Join-Path $payloadDir "tsf\rime.dll") -Force
Copy-Item $tsfX86 (Join-Path $payloadDir "tsf-x86\lingxi_tsf_x86.dll") -Force
Copy-Item $rimeX86 (Join-Path $payloadDir "tsf-x86\rime.dll") -Force

$size = [math]::Round(((Get-ChildItem $payloadDir -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 1)
Write-Host "payload 组装完成: $payloadDir ($size MB)"

# 组装后校验：这些文件缺一不可（曾出现过复制后缺失导致安装包不完整）
$required = @(
    "lingxi-ime.exe", "rime.dll", "uninstall.exe",
    "rime\data\default.yaml",
    "tsf\lingxi_tsf_x64.dll", "tsf\rime.dll",
    "tsf-x86\lingxi_tsf_x86.dll", "tsf-x86\rime.dll"
)
$missing = $required | Where-Object { -not (Test-Path (Join-Path $payloadDir $_)) }
if ($missing) { throw "payload 缺少必要文件: $($missing -join ', ')" }
Write-Host "payload 校验通过（$($required.Count) 项必要文件齐备）"

# ── 3. 生成安装包 ──────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force $distDir | Out-Null
$packer = Join-Path $installerDir "target\release\wind-packer.exe"
$stub = Join-Path $installerDir "target\release\wind-installer.exe"
$config = Join-Path $installerDir "app.toml"
$output = Join-Path $distDir "LingxiIME-Setup-$Version.exe"

& $packer build --config $config --version $Version --source-dir $payloadDir --stub $stub `
    --compression $Compression --output $output
if ($LASTEXITCODE -ne 0) { throw "wind-packer 失败 (exit $LASTEXITCODE)" }

$pkg = Get-Item $output
Write-Host ("安装包: {0} ({1:N1} MB)" -f $pkg.FullName, ($pkg.Length / 1MB))
