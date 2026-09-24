# 只同步上游的**中文词库**：cn_dicts / cn_dicts_cell。其它文件一律不碰。
#
# 用法：tools\update-upstream-dicts.ps1 -Source D:\path\to\rime-frost
param(
    [Parameter(Mandatory = $true)][string]$Source,
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot)
)
$ErrorActionPreference = "Stop"
$data = Join-Path $RepoRoot "xunyin-ime\rime\data"
if (-not (Test-Path $Source)) { throw "源目录不存在: $Source" }
foreach ($d in "cn_dicts", "cn_dicts_cell") {
    $src = Join-Path $Source $d
    if (-not (Test-Path $src)) { throw "上游缺目录: $src" }
    $dst = Join-Path $data $d
    Remove-Item $dst -Recurse -Force -ErrorAction SilentlyContinue
    Copy-Item $src $dst -Recurse -Force
    $n = (Get-ChildItem $dst -File).Count
    Write-Host ("{0}: 已更新 {1} 个文件" -f $d, $n)
}
Write-Host "上游中文词库同步完成（我们自己的 schema / 标点表 / 英文与 opencc 未改动）"