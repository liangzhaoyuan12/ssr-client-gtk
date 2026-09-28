<#
.SYNOPSIS
  ssr-client-gtk — Windows packaging (GOAL §11 Phase 8.8, decision D9: zip only).

.DESCRIPTION
  Run on a Windows host. Needs:
    * MSYS2 with the mingw-w64 toolchain (default C:\msys64, override -Msys64)
        pacman -S mingw-w64-x86_64-{gcc,binutils,pkgconf,ntldd,gtk4,libadwaita}
    * Rust with the GNU target (the script runs `rustup target add`)
    * ImageMagick (optional — only for the exe icon)

  Produces:
    packaging\windows\ssr-client-gtk-<version>-windows-x86_64.zip

  No installer (decision D9): the zip is the deliverable; it is unpacked
  anywhere and started with `ssr-client-gtk.cmd`.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\build-win.ps1
#>
[CmdletBinding()]
param(
    # MSYS2 root (contains mingw64\).
    [string]$Msys64 = "C:\msys64",
    # Rust target — MSYS2's mingw-w64 GCC is a GNU toolchain, not MSVC.
    [string]$Target = "x86_64-pc-windows-gnu",
    # Switches for CI-style reruns.
    [switch]$SkipBuild,
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
# Scripts live at the repo root next to build.sh (the packaging/ folder is
# git-ignored and holds build *products* only).
$RepoRoot = $PSScriptRoot
Set-Location $RepoRoot
# …so every product this script produces goes under packaging\windows\.
$OutDir = Join-Path $RepoRoot "packaging\windows"
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

function Fail([string]$msg) {
    Write-Host "错误: $msg" -ForegroundColor Red
    exit 1
}
function Log([string]$msg) { Write-Host "==> $msg" }

# ---------- 0. version — single source Cargo.toml ----------
$verMatch = Select-String -Path (Join-Path $RepoRoot "Cargo.toml") -Pattern '^version = "(.+)"' |
    Select-Object -First 1
if (-not $verMatch) { Fail "cannot read version from Cargo.toml" }
$Ver = $verMatch.Matches[0].Groups[1].Value
Log "版本: $Ver"

# ---------- 1. toolchain ----------
$MingwRoot = Join-Path $Msys64 "mingw64"
$MingwBin = Join-Path $MingwRoot "bin"
foreach ($need in @("gcc.exe", "pkg-config.exe", "windres.exe", "ntldd.exe")) {
    if (-not (Test-Path (Join-Path $MingwBin $need))) {
        Fail "$need 不在 $MingwBin —— MSYS2 里执行: pacman -S mingw-w64-x86_64-{gcc,pkgconf,binutils,ntldd,gtk4,libadwaita}"
    }
}
$env:PATH = "$MingwBin;$env:PATH"
# pkg-config wants forward slashes.
$env:PKG_CONFIG_PATH = ((Join-Path $MingwRoot "lib\pkgconfig") -replace '\\', '/')
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "gcc.exe"

& (Join-Path $MingwBin "pkg-config.exe") --exists gtk4 libadwaita-1
if ($LASTEXITCODE -ne 0) {
    Fail "pkg-config 找不到 gtk4 / libadwaita-1 —— MSYS2 里执行: pacman -S mingw-w64-x86_64-gtk4 mingw-w64-x86_64-libadwaita"
}
$GtkVer = (& (Join-Path $MingwBin "pkg-config.exe") --modversion gtk4)
$AdwVer = (& (Join-Path $MingwBin "pkg-config.exe") --modversion libadwaita-1)
Log "构建环境: gtk4 $GtkVer / libadwaita $AdwVer"

rustup target add $Target
if ($LASTEXITCODE -ne 0) { Fail "rustup target add $Target 失败" }
if (-not $SkipTests) {
    Log "门禁: cargo fmt --check / clippy / test"
    # A fresh Rust toolchain has no rustfmt/clippy; the gate needs both.
    try { rustup component add rustfmt clippy 2>$null } catch { }
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) { Fail "cargo fmt --check 失败" }
    cargo clippy --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { Fail "cargo clippy 失败" }
    cargo test --quiet
    if ($LASTEXITCODE -ne 0) { Fail "cargo test 失败" }
}

# ---------- 2. icon + version resource (optional) ----------
$IconSrc = Join-Path $RepoRoot "icon.png"
$IcoPath = Join-Path $OutDir "app.ico"
$RcPath = Join-Path $OutDir "app.rc"
$magick = Get-Command magick -ErrorAction SilentlyContinue
if ($magick -and (Test-Path $IconSrc)) {
    # One .ico with the usual sizes; icon:auto-resize picks them for us.
    & magick $IconSrc -define icon:auto-resize=256,128,64,48,32,16 $IcoPath
    if ($LASTEXITCODE -ne 0) { Fail "icon.png → app.ico 转换失败" }
    $parts = @($Ver.Split('.'))
    while ($parts.Count -lt 4) { $parts += "0" }
    $fileVer = (@($parts)[0..3]) -join ','
    $rc = @"
#include <winres.h>
IDI_ICON1 ICON "$($IcoPath -replace '\\', '/')"
VS_VERSION_INFO VERSIONINFO
 FILEVERSION $fileVer
 PRODUCTVERSION $fileVer
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "liangzhaoyuan12"
            VALUE "FileDescription", "ShadowsocksR client (GTK4 + libadwaita)"
            VALUE "FileVersion", "$Ver"
            VALUE "InternalName", "ssr-client-gtk"
            VALUE "OriginalFilename", "ssr-client-gtk.exe"
            VALUE "ProductName", "ssr-client-gtk"
            VALUE "ProductVersion", "$Ver"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"@
    Set-Content -Path $RcPath -Value $rc -Encoding Ascii
    Log "图标与版本资源: app.ico + app.rc"
} else {
    Log "跳过 exe 图标/版本资源（未找到 magick 或 icon.png）—— 只影响资源管理器显示"
    if (Test-Path $RcPath) { Remove-Item $RcPath -Force }
}

# ---------- 3. build ----------
if (-not $SkipBuild) {
    Log "cargo build --release --target $Target"
    cargo build --release --target $Target
    if ($LASTEXITCODE -ne 0) { Fail "cargo build 失败" }
}
$Exe = Join-Path $RepoRoot "target\$Target\release\ssr-client-gtk.exe"
if (-not (Test-Path $Exe)) { Fail "找不到 $Exe" }

# ---------- 4. stage ----------
$StageRoot = Join-Path $OutDir "stage"
$Stage = Join-Path $StageRoot "ssr-client-gtk"
if (Test-Path $StageRoot) { Remove-Item $StageRoot -Recurse -Force }
New-Item -ItemType Directory -Path $Stage -Force | Out-Null
Copy-Item $Exe (Join-Path $Stage "ssr-client-gtk.exe")

# 4a. DLL closure (transitive), from the MSYS2 tree only.
$ntlddOut = & (Join-Path $MingwBin "ntldd.exe") -R $Exe 2>$null
$dlls = @{}
$rootPrefix = ($MingwRoot -replace '/', '\').ToLower()
foreach ($line in $ntlddOut) {
    # ntldd indents dependency lines — a bare `-split '\s+'` then returns an
    # EMPTY first token and every DLL would be skipped, tripping the count
    # assertion below. Drop empties and normalise slashes before comparing.
    $tokens = @($line -split '\s+' | Where-Object { $_ -ne '' })
    if ($tokens.Count -lt 1) { continue }
    $first = ($tokens[0] -replace '/', '\')
    $lower = $first.ToLower()
    if ($lower.EndsWith(".dll") -and $lower.StartsWith($rootPrefix)) {
        $dlls[$first] = $true
    }
}
if ($dlls.Count -lt 10) { Fail "ntldd 只解析到 $($dlls.Count) 个 DLL —— 依赖树不完整" }
foreach ($dll in $dlls.Keys) { Copy-Item $dll $Stage }
foreach ($must in @("libgtk-4-1.dll", "libadwaita-1-1.dll")) {
    if (-not (Test-Path (Join-Path $Stage $must))) { Fail "依赖树缺少 $must" }
}
Log "DLL: $($dlls.Count) 个"

# 4b. GTK runtime data (schemas, icon themes, GTK's own translations,
#     fontconfig + the GTK loader cache). `lib\gio` and
#     `lib\gdk-pixbuf-2.0` are optional on purpose: their absence only
#     costs a warning + the app's own icon, and Test-Path skips them.
foreach ($dir in @("share\glib-2.0", "share\icons", "share\libadwaita-1", "lib\gtk-4.0", "share\gtk-4.0", "lib\gio", "lib\gdk-pixbuf-2.0")) {
    $src = Join-Path $MingwRoot $dir
    if (Test-Path $src) {
        $dst = Join-Path $Stage $dir
        New-Item -ItemType Directory -Path (Split-Path $dst) -Force | Out-Null
        Copy-Item $src $dst -Recurse -Force
    }
}
# Chinese catalogues only — English is the source language, everything else
# would just bloat the zip (GOAL D4: zh-CN / en-US only).
$zh = Join-Path $MingwRoot "share\locale\zh_CN"
if (Test-Path $zh) {
    New-Item -ItemType Directory -Path (Join-Path $Stage "share\locale") -Force | Out-Null
    Copy-Item $zh (Join-Path $Stage "share\locale\zh_CN") -Recurse -Force
}
# fontconfig: FONTCONFIG_PATH is set by the launcher below.
$fonts = Join-Path $MingwRoot "etc\fonts"
if (Test-Path $fonts) {
    New-Item -ItemType Directory -Path (Join-Path $Stage "etc") -Force | Out-Null
    Copy-Item $fonts (Join-Path $Stage "etc\fonts") -Recurse -Force
}

# 4c. launcher — puts the bundled data dirs in front and pins a renderer
#     that works without the GPU drivers GTK would probe on a foreign box.
@"
@echo off
setlocal
set "FONTCONFIG_PATH=%~dp0etc\fonts"
set "XDG_DATA_DIRS=%~dp0share"
set "GSK_RENDERER=cairo"
start "" "%~dp0ssr-client-gtk.exe" %*
"@ | Set-Content -Path (Join-Path $Stage "ssr-client-gtk.cmd") -Encoding Ascii

Copy-Item (Join-Path $RepoRoot "LICENSE") $Stage -ErrorAction SilentlyContinue
Copy-Item (Join-Path $RepoRoot "README.md") $Stage -ErrorAction SilentlyContinue

# ---------- 5. zip ----------
$ZipName = "ssr-client-gtk-$Ver-windows-x86_64.zip"
$ZipPath = Join-Path $OutDir $ZipName
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path (Join-Path $StageRoot "*") -DestinationPath $ZipPath

# ---------- 6. assertions ----------
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
try {
    $names = $zip.Entries | ForEach-Object { $_.FullName }
    foreach ($must in @("ssr-client-gtk/ssr-client-gtk.exe", "ssr-client-gtk/ssr-client-gtk.cmd", "ssr-client-gtk/libgtk-4-1.dll")) {
        if (-not ($names -contains $must)) { Fail "zip 缺少 $must" }
    }
    $exeEntry = $names | Where-Object { $_ -like "*/ssr-client-gtk.exe" }
    if (-not $exeEntry) { Fail "zip 里没有 ssr-client-gtk.exe" }
} finally { $zip.Dispose() }

$hash = (Get-FileHash -Algorithm SHA256 $ZipPath).Hash
$size = [math]::Round((Get-Item $ZipPath).Length / 1KB)
Log "产物: $ZipPath ($size KB)"
Log "sha256: $hash"
Log "解压后双击 ssr-client-gtk.cmd 启动（无安装器 —— 决定 D9）"
Write-Host ""
Write-Host "下一步验证见 VERIFY.md 的 Windows 段" -ForegroundColor Green
exit 0
