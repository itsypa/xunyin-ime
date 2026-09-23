#Requires -Version 5.1
<#
.SYNOPSIS
    通用安装程序打包脚本（由 app.toml 驱动）
.DESCRIPTION
    编译 stub 二进制 → 注入卸载器到源目录 → 调用 wind-packer build 生成安装程序。
    应用身份、源目录、压缩、品牌（logo/icon）全部由 app.toml 描述，与本脚本无关。
.PARAMETER Config
    app.toml 路径，默认项目根目录下的 app.toml
.PARAMETER SkipBuild
    跳过 cargo 编译，使用已有 release 二进制
.PARAMETER PrepOnly
    只做到「注入并加工卸载器」为止就返回，且把 uninstall.exe 留在源目录。
    给代码签名腾位置用：卸载器一旦签名就不能再改，而 build 会把它封进压缩块，
    补签够不着。调用方在两次调用之间签它：
        pack.ps1 -PrepOnly          → 加工 uninstall.exe
        <签 uninstall.exe>
        pack.ps1 -SkipPrep          → 打包
.PARAMETER SkipPrep
    跳过「注入并加工卸载器」，直接打包。**只能与前一次 -PrepOnly 配对使用** ——
    源目录里没有加工好的卸载器时直接报错退出，不会打出一个不含卸载器的包
    （那种包装完没有卸载入口，是比失败更糟的结果）。
.EXAMPLE
    .\scripts\pack.ps1
    .\scripts\pack.ps1 -Config app.toml -SkipBuild
#>

param(
    [string]$Config = "",
    [switch]$SkipBuild,
    [switch]$PrepOnly,
    [switch]$SkipPrep
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$ProjectRoot = Split-Path -Parent $ScriptDir
if ($Config -eq "") { $Config = Join-Path $ProjectRoot "app.toml" }

function Write-Step { param([string]$m) Write-Host "`n>>> $m" -ForegroundColor Cyan }
function Write-OK   { param([string]$m) Write-Host "    [OK] $m" -ForegroundColor Green }
function Write-Err  { param([string]$m) Write-Host "    [ERROR] $m" -ForegroundColor Red }

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Wind Installer 打包（app.toml 驱动）" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

if (-not (Test-Path $Config)) { Write-Err "配置不存在: $Config"; exit 1 }

# 两个开关是同一次打包的前后两段，同时给出没有任何合理语义。不校验的话 -SkipPrep
# 会胜出、-PrepOnly 被静默忽略并直接打包 —— 调用方以为只做了 prep, 实际包已经出了。
if ($PrepOnly -and $SkipPrep) {
    Write-Err "-PrepOnly 与 -SkipPrep 互斥: 它们是同一次打包的前后两段, 请分两次调用。"
    exit 1
}

# --- 从 app.toml 解析 source_dir（用于注入卸载器；相对 app.toml 所在目录）---
$ConfigDir = Split-Path -Parent (Resolve-Path $Config)
$tomlText = Get-Content $Config -Raw
if ($tomlText -notmatch 'source_dir\s*=\s*"([^"]+)"') {
    Write-Err "app.toml 缺少 [package] source_dir"; exit 1
}
$SourceDir = $Matches[1]
if (-not [System.IO.Path]::IsPathRooted($SourceDir)) {
    $SourceDir = Join-Path $ConfigDir $SourceDir
}
if (-not (Test-Path $SourceDir)) { Write-Err "源目录不存在: $SourceDir"; exit 1 }
Write-OK "源目录: $SourceDir"

# --- Step 1: 编译 stub 二进制 ---
$StubExe        = Join-Path $ProjectRoot "target\release\wind-installer.exe"
$PackerExe      = Join-Path $ProjectRoot "target\release\wind-packer.exe"
$UninstallerExe = Join-Path $ProjectRoot "target\release\wind-uninstaller.exe"

if (-not $SkipBuild) {
    Write-Step "编译 wind-installer / wind-uninstaller / wind-packer..."
    Push-Location $ProjectRoot
    try {
        cargo build --release --bin wind-installer --bin wind-uninstaller
        if ($LASTEXITCODE -ne 0) { Write-Err "stub 编译失败"; exit 1 }
        # wind-packer 需要 packer feature（启用 editpe 写图标）
        cargo build --release --bin wind-packer --features packer
        if ($LASTEXITCODE -ne 0) { Write-Err "packer 编译失败"; exit 1 }
        Write-OK "编译完成"
    }
    finally { Pop-Location }
}

foreach ($exe in @($StubExe, $PackerExe, $UninstallerExe)) {
    if (-not (Test-Path $exe)) { Write-Err "缺少二进制: $exe（去掉 -SkipBuild 重新编译）"; exit 1 }
}

# --- Step 2: 注入卸载器到源目录（打包时一并压缩，安装后解压到安装目录）---
# 每次都从 target\ 重新复制未加工的 stub 覆盖上一轮的产物 —— 加工是不可逆的
# （写完版本信息又追加了 overlay），已加工的文件不能就地更新: 重写 PE 会毁掉可能已经
# 打上的签名。不复制的话, 打包器会发现 overlay 与本轮配置对不上而报错退出。
$UninstallerDest = Join-Path $SourceDir "uninstall.exe"
if (-not $SkipPrep) {
    Copy-Item -Path $UninstallerExe -Destination $UninstallerDest -Force
    Write-OK "已注入卸载器: $UninstallerDest"

    # --- Step 2.5: 加工卸载器（版本信息 + 图标 + 清单 overlay）---
    # 必须在 pack 之前完成：pack 会把它封进压缩块，之后再改就够不着了。
    # 这一步之后卸载器即为终态，可以签名——签名也只能夹在这里与 Step 3 之间。
    Write-Step "加工卸载器（wind-packer prep-uninstaller）..."
    & $PackerExe prep-uninstaller --config $Config
    if ($LASTEXITCODE -ne 0) { Write-Err "卸载器加工失败"; exit 1 }

    if ($PrepOnly) {
        # 有意不删 uninstall.exe：调用方接下来要签它，然后带 -SkipPrep 回来打包。
        Write-Host "`n============================================" -ForegroundColor Green
        Write-Host "  卸载器已就绪（-PrepOnly）: $UninstallerDest" -ForegroundColor Green
        Write-Host "  下一步: 签名该文件, 再以 -SkipPrep 重新调用本脚本打包" -ForegroundColor Green
        Write-Host "============================================" -ForegroundColor Green
        exit 0
    }
} elseif (-not (Test-Path $UninstallerDest)) {
    Write-Err "-SkipPrep 要求源目录内已有加工好的卸载器, 但未找到: $UninstallerDest"
    Write-Err "请先以 -PrepOnly 调用一次本脚本。"
    exit 1
}

try {
    # --- Step 3: pack + bundle（含写图标）---
    Write-Step "打包（wind-packer build）..."
    # -SkipPrep 时要求卸载器【已经加工过】, 而不只是「文件在」。差别在于: 源目录里躺着
    # 一个未加工的裸 stub 时, 打包器默认会就地补加工 —— 而签名夹在 prep 与打包之间,
    # 补加工出来的卸载器【没签名】, 调用方却以为签过了。上面那个 Test-Path 只挡得住
    # 「文件不在」, 挡不住「文件在但没加工」。
    # 整段参数走一个数组再 splat, 而不是只把开关单独 splat: 条件追加更直观, 且带空格的
    # 路径经数组传参不需要手工加引号。
    $packerArgs = @("build", "--config", $Config, "--stub", $StubExe)
    if ($SkipPrep) { $packerArgs += "--require-prepared-uninstaller" }
    & $PackerExe @packerArgs
    if ($LASTEXITCODE -ne 0) { Write-Err "打包失败"; exit 1 }
}
finally {
    # 保持源目录干净
    Remove-Item -Path $UninstallerDest -Force -ErrorAction SilentlyContinue
}

Write-Host "`n============================================" -ForegroundColor Green
Write-Host "  打包完成!" -ForegroundColor Green
Write-Host "============================================" -ForegroundColor Green
