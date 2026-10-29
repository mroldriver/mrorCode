# bootstrap-tools.ps1
# 为本机残缺的 GNU 工具链补齐 windows-gnu 所需的 binutils（as/dlltool）。
# 零系统改动：仅解压到项目本地 .tools/mingw，并生成 .cargo/config.toml。
# 用法：在项目根目录执行  powershell -ExecutionPolicy Bypass -File scripts/bootstrap-tools.ps1
#
# 背景：rustc 在 windows-gnu 上生成 raw-dylib 导入库时调用 dlltool，
# 而 dlltool 需要同目录的 as.exe（本机 ~/.cargo/bin 中是残缺副本，缺 as/cc1）。
# MSYS2 mingw64 官方包：https://repo.msys2.org/mingw/x86_64/
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ===== 可调参数（集中顶部） =====
$BaseUrl   = 'https://repo.msys2.org/mingw/x86_64'
$Packages  = @(
    'mingw-w64-x86_64-binutils-2.47-3-any.pkg.tar.zst',
    'mingw-w64-x86_64-zlib-ng-compat-2.3.3-2-any.pkg.tar.zst',
    'mingw-w64-x86_64-libiconv-1.19-1-any.pkg.tar.zst',
    'mingw-w64-x86_64-gettext-runtime-1.0-1-any.pkg.tar.zst',
    'mingw-w64-x86_64-libwinpthread-14.0.0.r98.g19f5121a2-1-any.pkg.tar.zst',
    'mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst',
    'mingw-w64-x86_64-zstd-1.5.7-2-any.pkg.tar.zst'
)
# ================================

$Root      = (Resolve-Path "$PSScriptRoot/..").Path
$ToolsDir  = Join-Path $Root '.tools'
$PkgDir    = Join-Path $ToolsDir 'pkg'
$MingwDir  = Join-Path $ToolsDir 'mingw'   # 解压后 bin 位于 $MingwDir\bin
$BinDir    = Join-Path $MingwDir 'bin'
$Dlltool   = Join-Path $BinDir 'dlltool.exe'
$As        = Join-Path $BinDir 'as.exe'
# dlltool/ld/rustc 的临时文件统一落到项目内，不依赖系统 %TEMP%
# （企业沙箱/杀软收紧 Temp 权限时会报 Permission denied）。
$TmpDir    = Join-Path $ToolsDir 'tmp'

New-Item -ItemType Directory -Force -Path $PkgDir, $MingwDir, $TmpDir | Out-Null

# 每个包幂等处理：缺则下载，每次运行都重新解压（tar 覆盖，安全可重入）
foreach ($pkg in $Packages) {
    $dest = Join-Path $PkgDir $pkg
    if (-not (Test-Path $dest)) {
        Write-Host "downloading $pkg ..."
        Invoke-WebRequest -UseBasicParsing "$BaseUrl/$pkg" -OutFile $dest
    }
    # 包内统一前缀 mingw64/，剥掉后直接落到 $MingwDir
    & tar.exe -xf $dest -C $MingwDir --strip-components=1
    if ($LASTEXITCODE -ne 0) { throw "extract failed: $pkg" }
}

# 验证工具可运行（缺 DLL 时会在此报错）
$dv = & $Dlltool --version 2>&1; if ($LASTEXITCODE -ne 0) { throw "dlltool failed to start: $dv" }
$av = & $As --version 2>&1;       if ($LASTEXITCODE -ne 0) { throw "as failed to start: $av" }
Write-Host $dv[0]
Write-Host $av[0]

# 生成 .cargo/config.toml（绝对路径；本机专用，不入库）
$CargoDir = Join-Path $Root '.cargo'
New-Item -ItemType Directory -Force -Path $CargoDir | Out-Null
$ConfigPath = Join-Path $CargoDir 'config.toml'
$DlltoolEsc = $Dlltool -replace '\\', '\\'
$TmpRel = '.tools/tmp'
@"
# 由 scripts/bootstrap-tools.ps1 生成：指向项目本地完整 binutils
[target.x86_64-pc-windows-gnu]
rustflags = ["-C", "dlltool=$DlltoolEsc"]

# dlltool/ld 的临时文件重定向到项目内，摆脱系统 %TEMP% 权限依赖。
[env]
TMPDIR = { value = "$TmpRel", relative = true, force = true }
TEMP   = { value = "$TmpRel", relative = true, force = true }
TMP    = { value = "$TmpRel", relative = true, force = true }
"@ | Set-Content -Encoding UTF8 $ConfigPath

Write-Host "bootstrap done: $Dlltool"
Write-Host "config written: $ConfigPath"

# 自检：真跑一次链接（kernel 是最轻的底层 crate），确认 dlltool/as 真能工作。
Write-Host "running cargo check -p mrorcode-kernel (sanity)..."
Push-Location $Root
try {
    cargo check -p mrorcode-kernel
    if ($LASTEXITCODE -ne 0) { throw "cargo check -p mrorcode-kernel failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}
Write-Host "bootstrap sanity check OK."
