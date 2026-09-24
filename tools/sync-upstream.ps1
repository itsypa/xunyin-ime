# 同步上游最新：librime（预发布 latest 的 master 构建）+ rime-frost 词库（主分支）
#
# 只覆盖"上游产物"：rime.dll / rime.lib / 头文件，以及 cn_dicts、cn_dicts_cell、
# en_dicts、opencc、essay.txt、symbols.yaml。我们自己的 schema / 词表 / default.yaml
# / custom_phrase.txt / build 缓存一律不动。
#
# 为什么不用 release：上游词库每天更新但很少发 release（release 常落后几个月），
# librime 的 master 构建也只挂在预发布 tag "latest" 上。
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [switch]$SkipLibrime,
    [switch]$SkipDicts
)

$ErrorActionPreference = "Stop"
$imeDir = Join-Path $RepoRoot "lingxi-ime"
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("lingxi-sync-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $stage | Out-Null

function Get-Remote([string]$url, [string]$out) {
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers["Authorization"] = "Bearer $($env:GITHUB_TOKEN)" }
    Write-Host "  GET $url"
    Invoke-WebRequest -Uri $url -OutFile $out -Headers $headers -TimeoutSec 900
}

function Find-7z {
    $candidates = @(
        "$env:ProgramFiles\7-Zip\7z.exe",
        "${env:ProgramFiles(x86)}\7-Zip\7z.exe",
        "C:\Program Files\7-Zip\7z.exe"
    ) | Where-Object { $_ -and (Test-Path $_) }
    if ($candidates) { return $candidates[0] }
    $cmd = Get-Command 7z -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "找不到 7z.exe（解压 librime 压缩包需要）"
}

# ── 1. librime：取预发布 tag "latest" 的 Windows-msvc x64/x86 构建 ──────────────
if (-not $SkipLibrime) {
    Write-Host "== librime（latest 预发布，master 构建）=="
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers["Authorization"] = "Bearer $($env:GITHUB_TOKEN)" }
    $rel = Invoke-RestMethod "https://api.github.com/repos/rime/librime/releases/tags/latest" -Headers $headers -TimeoutSec 60
    Write-Host "  tag=$($rel.tag_name) published=$($rel.published_at)"
    $sevenZip = Find-7z
    foreach ($arch in @("x64", "x86")) {
        $asset = $rel.assets | Where-Object { $_.name -match "-Windows-msvc-$arch\.7z$" } | Select-Object -First 1
        if (-not $asset) { throw "librime latest 里找不到 $arch 资产" }
        $pkg = Join-Path $stage $asset.name
        Get-Remote $asset.browser_download_url $pkg
        $dst = Join-Path $stage "lib-$arch"
        & $sevenZip x $pkg "-o$dst" -y | Out-Null
        $dll = Get-ChildItem $dst -Recurse -File -Filter rime.dll | Select-Object -First 1
        $lib = Get-ChildItem $dll.Directory -File -Filter rime.lib | Select-Object -First 1
        if (-not $dll -or -not $lib) { throw "$arch 包里缺 rime.dll / rime.lib" }
        $target = if ($arch -eq "x64") { Join-Path $imeDir "rime" } else { Join-Path $imeDir "rime-x86" }
        Copy-Item $dll.FullName (Join-Path $target "rime.dll") -Force
        Copy-Item $lib.FullName (Join-Path $target "rime.lib") -Force
        Get-ChildItem $dll.Directory -File -Filter "*.h" | Copy-Item -Destination $target -Force
        Write-Host ("  {0}: rime.dll {1:N0} 字节（{2}）" -f $arch, $dll.Length, $asset.name)
    }
}

# ── 2. 词库：rime-frost 主分支 ────────────────────────────────────────────────
if (-not $SkipDicts) {
    Write-Host "== 词库（rime-frost 主分支）=="
    $tar = Join-Path $stage "rime-frost-master.tar.gz"
    Get-Remote "https://codeload.github.com/gaboolic/rime-frost/tar.gz/refs/heads/master" $tar
    $ex = Join-Path $stage "frost"
    New-Item -ItemType Directory -Force $ex | Out-Null
    # 仓库里有非 ASCII 文件名，tar 会往 stderr 报 "Invalid empty pathname"；
    # 那只是 others/ 里的杂项，我们需要的目录已正常解出，故这里不让它中断脚本
    $ErrorActionPreference = "Continue"
    & tar -xf $tar -C $ex 2>&1 | Out-Null
    $ErrorActionPreference = "Stop"
    $root = Get-ChildItem $ex -Directory | Select-Object -First 1
    if (-not $root) { throw "解压 rime-frost 失败" }
    $data = Join-Path $imeDir "rime\data"
    foreach ($d in "cn_dicts", "cn_dicts_cell", "en_dicts", "opencc") {
        $src = Join-Path $root.FullName $d
        if (-not (Test-Path $src)) { continue }
        Remove-Item (Join-Path $data $d) -Recurse -Force -ErrorAction SilentlyContinue
        Copy-Item $src (Join-Path $data $d) -Recurse -Force
        $sz = (Get-ChildItem (Join-Path $data $d) -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
        Write-Host ("  {0}: {1:N1} MB" -f $d, $sz)
    }
    foreach ($f in "essay.txt", "symbols.yaml") {
        $src = Join-Path $root.FullName $f
        if (Test-Path $src) {
            Copy-Item $src (Join-Path $data $f) -Force
            Write-Host "  $f 已更新"
        }
    }
}

Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "上游同步完成（librime 与词库均为最新）"
