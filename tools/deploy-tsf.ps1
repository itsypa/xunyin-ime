param(
    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",
    # 暂存布局根目录：DLL 复制到这里再注册。默认 build\dev-deploy（已被 .gitignore 忽略）。
    # 注意：该目录下可能有一个指向仓库词库的 rime 目录联接，不要对它做递归删除。
    [string]$StageRoot = "",
    # 注册用的 DLL 文件名。默认按架构固定：lingxi_tsf_x64.dll / lingxi_tsf_x86.dll。
    [string]$DllName = "",
    [switch]$SkipBuild,
    [switch]$SkipSign,
    # 保留 target 下历史遗留的 lingxi_tsf_v*.dll（默认清理，被进程占用的会跳过）
    [switch]$KeepLegacyDlls
)

# 为什么要"固定路径 + 固定文件名"，而不是直接注册 target\debug 里的产物：
#
#   TSF 会把输入法 DLL 加载进**每一个正在输入的进程**（explorer、浏览器、编辑器…），
#   而 cargo 每次构建都会重写 target\debug。注册表一旦指向那里，就会出现
#   「已打开的进程持着旧版本 DLL，注册表/磁盘已是新版本」的错位；再叠加旧版脚本的
#   「强杀 ctfmon / TextInputHost」，会话里的输入法状态也被打断。现象就是
#   **所有输入法都打不出中文**，而且只有重开应用 / 注销重登才恢复。
#
#   所以本脚本一律：
#     * 把 DLL 与同位数 rime.dll 复制到固定路径的暂存布局（默认 build\dev-deploy\，
#       与安装包布局一致：tsf\lingxi_tsf_x64.dll、tsf-x86\lingxi_tsf_x86.dll）；
#     * 用固定文件名注册（带 v66 这类版本号会让注册表路径每次都变）；
#     * 不杀 ctfmon / TextInputHost，改为在末尾列出仍占用旧 DLL 的进程。
#
#   改完 DLL 后请**完全退出并重开**已打开的应用（或注销重登），新进程才会加载新版本。

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tsfDir = Join-Path $root "lingxi-tsf"
$certThumb = "109B04E710355C66C85B025C8E939B0F778580CB"
$clsid = "{F3A2D6E1-4C5B-4E8A-9C1D-2A6B7C8D9E0F}"

if (-not $StageRoot) { $StageRoot = Join-Path $root "build\dev-deploy" }

if ($Arch -eq "x64") {
    $target = "x86_64-pc-windows-msvc"
    $targetSub = "debug"
    $stageSub = "tsf"
    $regFlag = "/reg:64"
    $regsvr = (Join-Path $env:WINDIR "System32\regsvr32.exe")
    if (-not $DllName) { $DllName = "lingxi_tsf_x64.dll" }
} else {
    $target = "i686-pc-windows-msvc"
    $targetSub = "i686-pc-windows-msvc\debug"
    $stageSub = "tsf-x86"
    $regFlag = "/reg:32"
    $regsvr = (Join-Path $env:WINDIR "SysWOW64\regsvr32.exe")
    if (-not $DllName) { $DllName = "lingxi_tsf_x86.dll" }
}

# 发布单个文件：目标可能正被已打开的进程加载。
# Windows 允许把**已加载**的 DLL 改名让路（但不允许直接删除），所以占用时先改名再写新文件；
# 改名后的旧文件尽量立即删除，删不掉就排队到下次重启（与安装器的做法一致）。
function Publish-StagedFile {
    param([string]$Source, [string]$Dest)
    try {
        Copy-Item -LiteralPath $Source -Destination $Dest -Force -ErrorAction Stop
        return
    } catch {
        if (-not (Test-Path -LiteralPath $Dest)) { throw }
    }
    $aside = "$Dest.stale-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    Move-Item -LiteralPath $Dest -Destination $aside -Force
    Copy-Item -LiteralPath $Source -Destination $Dest -Force
    Write-Host "  [占用] 旧文件已改名让路: $(Split-Path $aside -Leaf)"
    try {
        Remove-Item -LiteralPath $aside -Force -ErrorAction Stop
    } catch {
        try {
            if (-not ("Win32.FileOps" -as [type])) {
                Add-Type -Namespace Win32 -Name FileOps -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern bool MoveFileEx(string lpExistingFileName, string lpNewFileName, int dwFlags);
'@
            }
            # MOVEFILE_DELAY_UNTIL_REBOOT = 4
            [Win32.FileOps]::MoveFileEx($aside, $null, 4) | Out-Null
        } catch {
            Write-Host "  [占用] 旧文件仍被持有，未能删除: $aside"
        }
    }
}

if (-not $SkipBuild) {
    Push-Location $tsfDir
    try {
        # native 命令写 stderr（cargo/链接器警告）时不要抛错
        $ErrorActionPreference = "Continue"
        if ($Arch -eq "x64") {
            cargo build 2>&1 | Out-Host
        } else {
            cargo build --target $target 2>&1 | Out-Host
        }
        $code = $LASTEXITCODE
        $ErrorActionPreference = "Stop"
        if ($code -ne 0) { throw "cargo build failed ($Arch)" }
    } finally {
        Pop-Location
    }
}

# 从 cargo 产物复制到**固定路径**的暂存布局（与安装包布局一致），注册表只认这里。
$outDir = Join-Path $tsfDir "target\$targetSub"
$built = Join-Path $outDir "lingxi_tsf.dll"
if (-not (Test-Path -LiteralPath $built)) { throw "build output not found: $built" }

$stageDir = Join-Path $StageRoot $stageSub
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
$dst = Join-Path $stageDir $DllName
Publish-StagedFile -Source $built -Dest $dst

# rime.dll 必须与 TSF DLL 同目录、同位数：宿主进程加载本 DLL 时会从本 DLL 目录找依赖。
$rimeSrc = Join-Path $outDir "rime.dll"
if (-not (Test-Path -LiteralPath $rimeSrc)) { throw "rime.dll not found: $rimeSrc" }
Publish-StagedFile -Source $rimeSrc -Dest (Join-Path $stageDir "rime.dll")
Write-Host "[$Arch] staged: $dst"

# 词库自检：DLL 依次找 <dll>\rime\data → <dll>\..\rime\data → 上溯 3 级的 lingxi-ime\rime\data
$dataCandidates = @(
    (Join-Path $stageDir "rime\data\default.yaml"),
    (Join-Path $StageRoot "rime\data\default.yaml"),
    (Join-Path $stageDir "..\..\..\lingxi-ime\rime\data\default.yaml")
)
if (-not ($dataCandidates | Where-Object { Test-Path -LiteralPath $_ })) {
    # 兜底：让 <StageRoot>\rime 指向仓库词库，与安装布局 <安装目录>\rime\data 对齐
    $link = Join-Path $StageRoot "rime"
    $repoRime = Join-Path $root "lingxi-ime\rime"
    if (Test-Path -LiteralPath $link) {
        throw "找不到 rime 词库数据，且 $link 已存在但不可用：请手工处理后再部署。"
    }
    New-Item -ItemType Junction -Path $link -Target $repoRime | Out-Null
    Write-Host "[$Arch] 已创建词库目录联接: $link -> $repoRime"
}

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

# 语言栏/状态栏图标：蓝底白字「迅」——拷到 DLL 旁并登记到 TIP 语言档案
$ico = Join-Path $root "lingxi-ime\icons\xunyin.ico"
if (Test-Path $ico) {
    Copy-Item $ico (Join-Path (Split-Path -Parent $dst) "xunyin.ico") -Force
    $tipKey = "HKLM\SOFTWARE\Microsoft\CTF\TIP\{F3A2D6E1-4C5B-4E8A-9C1D-2A6B7C8D9E0F}\LanguageProfile\0x00000804\{B2E4C9A1-7D3F-4B62-9E11-5C8A0F2D6E31}"
    reg add $tipKey /v IconFile /t REG_SZ /d $ico /f | Out-Null
    reg add $tipKey /v IconIndex /t REG_DWORD /d 0 /f | Out-Null
    Write-Host "[$Arch] 语言栏图标已登记: $ico"
}

# 清理开发期遗留的版本化 DLL（当年把注册表指向 target\debug 时留下的产物）。
# 只删 lingxi_tsf_v*.dll，不动 lingxi_tsf.dll / rime.dll。
if (-not $KeepLegacyDlls) {
    $legacyDirs = @(
        (Join-Path $tsfDir "target\debug"),
        (Join-Path $tsfDir "target\i686-pc-windows-msvc\debug")
    )
    $legacy = @()
    foreach ($d in $legacyDirs) {
        if (-not (Test-Path -LiteralPath $d)) { continue }
        $legacy += Get-ChildItem -LiteralPath $d -Filter "lingxi_tsf_v*.dll" -File -ErrorAction SilentlyContinue
    }
    if ($legacy) {
        $removed = 0
        $locked = @()
        foreach ($f in $legacy) {
            try {
                Remove-Item -LiteralPath $f.FullName -Force -ErrorAction Stop
                $removed++
            } catch {
                $locked += $f.FullName
            }
        }
        Write-Host "[$Arch] 清理历史 DLL: 删除 $removed / 共 $($legacy.Count)"
        foreach ($p in $locked) {
            Write-Host "  跳过（仍被进程持有；重开应用或重启后再跑一次）: $p"
        }
    }
}

# 收尾：不再强杀 ctfmon，改为列出仍占用 lingxi_tsf*.dll 的进程
$holders = @()
foreach ($proc in Get-Process -ErrorAction SilentlyContinue) {
    try {
        $mods = $proc.Modules | Where-Object { $_.ModuleName -like "lingxi_tsf*" }
        foreach ($m in $mods) { $holders += "$($proc.ProcessName)($($proc.Id)) -> $($m.FileName)" }
    } catch {
        # 受保护或已退出的进程读不到模块列表，忽略
    }
}

Write-Host ""
if ($holders) {
    Write-Host "以下进程仍在使用**旧版本** DLL，需完全退出并重开（或注销重登）后才会加载新版本："
    $holders | Sort-Object -Unique | ForEach-Object { Write-Host "  $_" }
} else {
    Write-Host "当前没有进程占用 lingxi_tsf*.dll。"
}
Write-Host "提示：本脚本不再强杀 ctfmon / TextInputHost —— 强杀会让已打开应用的输入法状态半死，"
Write-Host "      现象是「所有输入法都打不出中文」，只有重开应用或注销重登才恢复。"
Write-Host "deploy-tsf done: $Arch -> $dst"
