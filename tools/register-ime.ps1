# 便携安装/卸载：对**已就位**的 TSF DLL 注册或反注册（不复制文件、不建快捷方式）。
#
# 用途：
#   * 便携解压使用（含 32 位 Windows —— 官方安装器只出 x64）；
#   * 排查输入法注册问题时手工重注册。
#
# 用法（需管理员）：
#   .\register-ime.ps1                  # 注册脚本同目录下的 tsf\ 与 tsf-x86\
#   .\register-ime.ps1 -Root <目录>      # 指定目录（其下应有 tsf\ 与 tsf-x86\）
#   .\register-ime.ps1 -Unregister       # 反注册

param(
    [string]$Root = "",
    [switch]$Unregister
)

$ErrorActionPreference = "Stop"

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw "请以管理员身份运行：注册/反注册输入法需要写注册表。" }

if (-not $Root) { $Root = $PSScriptRoot }
$Root = (Resolve-Path -LiteralPath $Root).Path
$windir = $env:WINDIR
$is64 = Test-Path (Join-Path $windir "SysWOW64")

function Invoke-Regsvr([string]$dll, [bool]$wow64) {
    if (-not (Test-Path -LiteralPath $dll)) { return }
    $dir = if ($wow64 -and $is64) { "SysWOW64" } else { "System32" }
    $regsvr = Join-Path $windir "$dir\regsvr32.exe"
    $args = @("/s")
    if ($Unregister) { $args += "/u" }
    $args += $dll
    $p = Start-Process -FilePath $regsvr -ArgumentList $args -Wait -PassThru -WindowStyle Hidden
    $verb = if ($Unregister) { "反注册" } else { "注册" }
    $ok = if ($p.ExitCode -eq 0) { "OK" } else { "失败(退出码 $($p.ExitCode))" }
    Write-Host ("{0} {1}: {2}" -f $verb, (Split-Path $dll -Leaf), $ok)
}

$x64 = Join-Path $Root "tsf\lingxi_tsf_x64.dll"
$x86 = Join-Path $Root "tsf-x86\lingxi_tsf_x86.dll"

if (-not (Test-Path $x64) -and -not (Test-Path $x86)) {
    throw "在 $Root 下没找到 tsf\lingxi_tsf_x64.dll 或 tsf-x86\lingxi_tsf_x86.dll"
}

# 32 位 Windows 上没有 x64 视图可言，只注册 x86
if ($is64) { Invoke-Regsvr $x64 $false }
Invoke-Regsvr $x86 $true

Write-Host ""
Write-Host "完成。切换输入法：Win+Space 或任务栏语言栏选择「中文（简体）- 灵犀输入法」。"
Write-Host "（注册后需完全退出并重开已打开的应用，新进程才会加载输入法。）"
