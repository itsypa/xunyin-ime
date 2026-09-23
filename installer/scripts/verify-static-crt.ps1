#Requires -Version 5.1
<#
.SYNOPSIS
    校验 Windows 产物静态链接了 MSVC CRT（发布门禁）。

.DESCRIPTION
    stub 与卸载器都在**用户机器**上运行，且安装器往往是用户在那台机器上跑的第一个
    程序。动态链接 CRT 的话，没装 VC++ 运行库的干净机器会直接报「找不到
    VCRUNTIME140.dll」或 0xc000007b，用户完全无从自救。

    +crt-static 由 .cargo/config.toml 提供，但 RUSTFLAGS 环境变量会**覆盖**而非合并
    那一节——这个保证可能被外部因素静默掀翻。故本脚本不看构建配置，直接在产物二进制
    里找动态 CRT 的导入名。

    独立成脚本而非内联进 workflow：内联逻辑没法在本地跑，只能靠"照抄一份"来验证，
    而照抄的副本会连 bug 一起抄走（这个脚本的前身就是这么翻车的——相对路径遇上
    .NET CurrentDirectory 与 PowerShell location 不同步，读不到文件却判定通过）。

.PARAMETER Path
    待校验的可执行文件，可多个。

.EXAMPLE
    .\scripts\verify-static-crt.ps1 -Path target\release\wind-installer.exe, target\release\wind-uninstaller.exe
#>
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path
)

# 任何异常都必须终止:读不到文件要报错退出,绝不能落到"没找到动态 CRT 引用"
# 的分支上被判定为合格——那是把"没数据"当成"数据是 0"。
$ErrorActionPreference = 'Stop'

# 动态 CRT 的两组导入名:VC++ 运行时本体与 UCRT 转发 DLL
$DynamicCrtMarkers = @('VCRUNTIME140', 'api-ms-win-crt-runtime')

$failed = @()

foreach ($p in $Path) {
    # Resolve-Path 走 PowerShell 的 location,且文件不存在时直接抛错
    # （不能把相对路径交给 .NET API,它认的是进程 CurrentDirectory）
    $resolved = (Resolve-Path -LiteralPath $p).ProviderPath
    $name = Split-Path $resolved -Leaf

    $text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($resolved))

    $hits = @($DynamicCrtMarkers | Where-Object { $text -match $_ })
    if ($hits.Count -gt 0) {
        # ::error:: 前缀让 GitHub Actions 把它渲染成注解;本地运行原样打印
        Write-Host "::error::$name 动态链接了 $($hits -join ', ') —— 干净机器上无法启动。检查 .cargo/config.toml 与 RUSTFLAGS。"
        $failed += $name
    }
    else {
        Write-Host "  OK $name 自包含"
    }
}

if ($failed.Count -gt 0) {
    Write-Host "静态 CRT 校验失败: $($failed -join ', ')"
    exit 1
}

Write-Host "静态 CRT 校验通过($($Path.Count) 个产物)"
exit 0
