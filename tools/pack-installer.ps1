param(
    [string]$Version = "",
    # x64 = 64 位 Windows；x86 = 32 位 Windows（也可在 64 位系统上跑，注册走 Sysnative）。
    # 两者都用同一份 payload，只有安装器 stub 的架构不同。
    [ValidateSet("x64", "x86", "both")]
    [string]$Arch = "both",
    [ValidateSet("lzma", "zstd")]
    [string]$Compression = "lzma",
    [switch]$Sign,
    [string]$PfxPath = "",
    [string]$PfxPassword = "",
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

    # 安装器 stub / 卸载器 / 打包器（x64 打包器通吃两种架构的产物）
    Push-Location $installerDir
    try {
        cargo build --release --features packer
        cargo build --release --features packer --target i686-pc-windows-msvc
    } finally { Pop-Location }
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
$config = Join-Path $installerDir "app.toml"
if (-not (Test-Path $packer)) { throw "缺少打包器: $packer" }

# 找 signtool（Windows SDK 自带，按版本从高到低取）。
function Get-SignTool {
    $kits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path $kits) {
        $found = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
            Where-Object { Test-Path $_ } |
            Select-Object -First 1
        if ($found) { return $found }
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

# 对单个文件签名（SHA256 + RFC3161 时间戳）。
function Sign-File([string]$path) {
    if (-not $path -or -not (Test-Path $path)) { throw "待签名文件不存在: $path" }
    $signtool = Get-SignTool
    if (-not $signtool) { throw "未找到 signtool.exe（需要 Windows SDK）" }
    $toolArgs = @("sign", "/fd", "SHA256", "/f", $PfxPath)
    if ($PfxPassword) { $toolArgs += @("/p", $PfxPassword) }
    $toolArgs += @("/tr", "http://timestamp.digicert.com", "/td", "SHA256", $path)
    & $signtool @toolArgs | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "签名失败: $path (exit $LASTEXITCODE)" }
    Write-Host "已签名: $path"
}

function Pack-One([string]$arch) {
    $sub = if ($arch -eq "x86") { "i686-pc-windows-msvc\release" } else { "release" }
    $stub = Join-Path $installerDir "target\$sub\wind-installer.exe"
    if (-not (Test-Path $stub)) { throw "缺少 $arch 安装器 stub: $stub" }
    # 卸载器必须与安装器同架构：32 位系统跑不了 x64 卸载器。
    # 同时这也把上一轮"已加工"的卸载器换回未加工的 stub，避免打包器报"上一轮产物"。
    $un = Join-Path $installerDir "target\$sub\wind-uninstaller.exe"
    if (-not (Test-Path $un)) { throw "缺少 $arch 卸载器: $un" }
    Copy-Item $un (Join-Path $payloadDir "uninstall.exe") -Force
    $output = Join-Path $distDir "LingxiIME-Setup-$Version-$arch.exe"
    if ($Sign) {
        if (-not (Test-Path $PfxPath)) { throw "签名证书不存在: $PfxPath" }
        # 官方推荐顺序：先加工卸载器（写版本信息/图标）→ 签名 → 打包 → 签名安装包。
        # 卸载器一旦签名就不能再改，所以加工必须在签名之前完成。
        & $packer prep-uninstaller --config $config --version $Version --source-dir $payloadDir
        if ($LASTEXITCODE -ne 0) { throw "wind-packer prep-uninstaller 失败 ($arch)" }
        Sign-File (Join-Path $payloadDir "uninstall.exe")
        $archive = Join-Path $distDir "LingxiIME-$Version-$arch.bin"
        & $packer pack --config $config --version $Version --source-dir $payloadDir `
            --compression $Compression --output $archive --require-prepared-uninstaller
        if ($LASTEXITCODE -ne 0) { throw "wind-packer pack 失败 ($arch)" }
        & $packer bundle --config $config --version $Version --stub $stub --archive $archive --output $output
        if ($LASTEXITCODE -ne 0) { throw "wind-packer bundle 失败 ($arch)" }
        Sign-File $output
    } else {
        & $packer build --config $config --version $Version --source-dir $payloadDir --stub $stub `
            --compression $Compression --output $output
        if ($LASTEXITCODE -ne 0) { throw "wind-packer 失败 ($arch, exit $LASTEXITCODE)" }
    }
    $pkg = Get-Item $output
    Write-Host ("安装包({0}): {1} ({2:N1} MB)" -f $arch, $pkg.FullName, ($pkg.Length / 1MB))
}

if ($Arch -eq "both") {
    Pack-One "x64"
    Pack-One "x86"
} else {
    Pack-One $Arch
}
