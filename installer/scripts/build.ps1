#Requires -Version 5.1
<#
.SYNOPSIS
    编译 wind-installer 项目
.PARAMETER Release
    Release 模式编译
.PARAMETER Clean
    清理后重新编译
#>

param(
    [switch]$Release,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$ProjectRoot = Split-Path -Parent $ScriptDir

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  Wind Installer 构建" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

Push-Location $ProjectRoot
try {
    $profile = if ($Release) { "--release" } else { "" }

    if ($Clean) {
        Write-Host "`n>>> 清理构建目录..." -ForegroundColor Cyan
        cargo clean
        Write-Host "    [OK] 清理完成" -ForegroundColor Green
    }

    Write-Host "`n>>> 编译项目..." -ForegroundColor Cyan
    $env:CARGO_TERM_COLOR = "always"

    cargo build $profile 2>&1 | ForEach-Object {
        if ($_ -match "^error") {
            Write-Host $_ -ForegroundColor Red
        }
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n[ERROR] 编译失败" -ForegroundColor Red
        exit 1
    }

    Write-Host "`n>>> 编译完成!" -ForegroundColor Green

    # 显示输出文件
    $targetDir = if ($Release) { "target\release" } else { "target\debug" }
    $binaries = @("wind-installer.exe", "wind-packer.exe")

    Write-Host "`n输出文件:" -ForegroundColor Cyan
    foreach ($bin in $binaries) {
        $path = Join-Path $targetDir $bin
        if (Test-Path $path) {
            $size = [math]::Round((Get-Item $path).Length / 1KB, 1)
            Write-Host "    $bin ($size KB)" -ForegroundColor Green
        }
    }
}
finally {
    Pop-Location
}
