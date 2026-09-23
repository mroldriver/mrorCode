# install.ps1
# 把 release 二进制安装到用户 PATH 中的 ~/.cargo/bin（无需管理员权限）。
# 用法（在项目根目录）：pwsh -File scripts/install.ps1
# 更新代码后：先 cargo build --release，再重跑本脚本即可。
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ===== 可调参数（集中顶部） =====
$Root       = (Resolve-Path "$PSScriptRoot/..").Path
$BinaryName = 'mrorcode.exe'
$Source     = Join-Path $Root "target\release\$BinaryName"
$DestDir    = Join-Path $env:USERPROFILE '.cargo\bin'
# ================================

if (-not (Test-Path $Source)) {
    Write-Host "未找到 $Source" -ForegroundColor Yellow
    Write-Host "请先在项目根目录执行：cargo build --release" -ForegroundColor Yellow
    exit 1
}

New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
Copy-Item $Source (Join-Path $DestDir $BinaryName) -Force
Write-Host "已安装: $(Join-Path $DestDir $BinaryName)" -ForegroundColor Green

if (($env:PATH -split ';') -notcontains $DestDir) {
    Write-Host "注意: $DestDir 不在当前 PATH 中，请新开一个终端窗口。" -ForegroundColor Yellow
} else {
    Write-Host "完成。任意目录新开终端即可直接运行: mrorcode" -ForegroundColor Green
}
