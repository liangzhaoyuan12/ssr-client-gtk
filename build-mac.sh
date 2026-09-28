#!/usr/bin/env bash
# ssr-client-gtk — macOS packaging (GOAL §11 Phase 8.8, decision D11/D9:
# deliver a .app, no dmg/zip installer).
#
# Run on a macOS host:
#   brew install gtk4 libadwaita dylibbundler
#   ./build-mac.sh
#
# Produces:
#   packaging/macos/ssr-client-gtk.app          <- the deliverable
#   (structure asserted: Contents/MacOS + Frameworks + Resources/AppIcon.icns)
set -euo pipefail
# Scripts live at the repo root next to build.sh — packaging/ is
# git-ignored and holds build *products* only (user rule).
cd "$(dirname "$0")"

fail() { printf '错误: %s\n' "$*" >&2; exit 1; }
log()  { printf '\n==> %s\n' "$*"; }

# ---------- 0. version — single source Cargo.toml ----------
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$VER" ] || fail "无法从 Cargo.toml 读取 version"
ARCH=$(uname -m)   # arm64 | x86_64
log "版本: $VER  架构: $ARCH"

# ---------- 1. toolchain ----------
command -v cargo >/dev/null 2>&1 || fail "找不到 cargo"
command -v pkg-config >/dev/null 2>&1 || fail "找不到 pkg-config"
pkg-config --exists gtk4 libadwaita-1 \
  || fail "pkg-config 找不到 gtk4 / libadwaita-1 —— brew install gtk4 libadwaita"
log "构建环境: gtk4 $(pkg-config --modversion gtk4) / libadwaita $(pkg-config --modversion libadwaita-1)"
command -v dylibbundler >/dev/null 2>&1 \
  || fail "找不到 dylibbundler —— brew install dylibbundler（把 Homebrew 的 dylib 收进 .app 并改 rpath）"
command -v iconutil >/dev/null 2>&1 || fail "找不到 iconutil（macOS 自带，PATH 异常）"
command -v sips >/dev/null 2>&1 || fail "找不到 sips（macOS 自带，PATH 异常）"

# ---------- 2. gate + build ----------
if [ "${SKIP_GATE:-0}" != "1" ]; then
  log "门禁: fmt / clippy / test"
  # A fresh toolchain may lack rustfmt/clippy — the gate needs both.
  rustup component add rustfmt clippy >/dev/null 2>&1 || true
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test --quiet
fi
log "cargo build --release"
cargo build --release
BIN=target/release/ssr-client-gtk
[ -x "$BIN" ] || fail "构建产物不存在: $BIN"

# ---------- 3. bundle skeleton ----------
OUT="$PWD/packaging/macos"
APP="$OUT/ssr-client-gtk.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp "$BIN" "$APP/Contents/MacOS/ssr-client-gtk"
chmod +x "$APP/Contents/MacOS/ssr-client-gtk"

# ---------- 4. Info.plist (version == Cargo.toml) ----------
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ssr-client-gtk</string>
  <key>CFBundleDisplayName</key><string>ssr-client-gtk</string>
  <key>CFBundleIdentifier</key><string>com.liangzhaoyuan12.ssr-client-gtk</string>
  <key>CFBundleExecutable</key><string>ssr-client-gtk</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VER</string>
  <key>CFBundleShortVersionString</key><string>$VER</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST

# ---------- 5. icon: icon.png → AppIcon.icns ----------
if [ ! -f icon.png ]; then
  fail "icon.png 不存在 —— 图标源必须在仓库根目录（sips 会因此中断）"
fi
ICONSET="$APP/Contents/Resources/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 64 128 256 512; do
  sips -z "$size" "$size" icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  dbl=$((size * 2))
  sips -z "$dbl" "$dbl" icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

# ---------- 6. vendor the Homebrew dylibs and rewrite their rpaths ----------
# -b bundle, -d destination, -p install path prefix inside the bundle.
dylibbundler -b \
  -x "$APP/Contents/MacOS/ssr-client-gtk" \
  -d "$APP/Contents/Frameworks" \
  -p '@executable_path/../Frameworks' >/dev/null
FRAMEWORK_COUNT=$(find "$APP/Contents/Frameworks" -name '*.dylib' | wc -l | tr -d ' ')
[ "$FRAMEWORK_COUNT" -gt 0 ] || fail "dylibbundler 没有收进任何 dylib"

# ---------- 7. ad-hoc signature (no developer certificate needed) ----------
codesign --force --deep --sign - "$APP" 2>/dev/null \
  || fail "ad-hoc codesign 失败"

# ---------- 8. assertions ----------
[ -x "$APP/Contents/MacOS/ssr-client-gtk" ] || fail "可执行文件缺失"
grep -q "<string>$VER</string>" "$APP/Contents/Info.plist" \
  || fail "Info.plist 版本 != Cargo.toml ($VER)"
[ -s "$APP/Contents/Resources/AppIcon.icns" ] || fail ".icns 缺失"
if otool -L "$APP/Contents/MacOS/ssr-client-gtk" | grep -q '/opt/homebrew\|/usr/local/opt'; then
  fail "二进制仍指向 Homebrew 路径 —— dylibbundler 没有改写干净"
fi
codesign --verify --deep "$APP" || fail "codesign 校验失败"

log "产物: $APP"
printf '    Frameworks dylib: %s 个\n' "$FRAMEWORK_COUNT"
printf '    版本: %s\n' "$VER"
printf '    sha256(可执行文件): %s\n' "$(shasum -a 256 "$APP/Contents/MacOS/ssr-client-gtk" | cut -d' ' -f1)"
printf '\n下一步验证见 VERIFY.md 的 macOS 段\n'
