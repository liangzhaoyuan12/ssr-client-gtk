#!/usr/bin/env bash
# 构建 ssr-client-gtk 的四种发布包（GOAL D3）：
#   deb + rpm + Arch(.pkg.tar.zst) + tar.gz
#
# 用法：
#   ./build.sh            # 构建四件套
#   ./build.sh --check    # 先跑门禁（fmt/clippy/test）再构建
#
# 打包只直接调用系统工具，绝不 cargo install 任何东西：
#   deb  → dpkg-deb（Depends 的 ELF 部分由系统 dpkg-shlibdeps 补齐，失败则只留显式依赖）
#   rpm  → rpmbuild（本脚本现写 spec，_topdir 指向临时目录）
#   Arch → makepkg（本脚本现写 PKGBUILD，--nodeps 跳过空 pacman 源）
#   tar  → tar -czf（便携布局 bin/ + share/，解压直跑）
# 架构一律从本机系统工具读取（dpkg / rpm / uname / makepkg.conf），不硬编码
# x86_64 —— 本机是 loongarch64，硬编码过的名字会直接找不到产物。
#
# 版本问题：版本单一来源 = Cargo.toml（GOAL §10 D 版本约定）。本脚本读取后
#   用于四个产物的文件名与包内版本断言，任一漂移即失败退出。
#
# 依赖问题：
#   - 构建库：优先 source 用户态 sysroot（~/.local/gtk-sysroot/env.sh），
#     否则用系统 pkg-config；随后核验 gtk4 >= 4.18、libadwaita-1 >= 1.5
#     （与 Cargo features v4_18/v1_5 一致）。
#   - 打包工具：dpkg-deb / rpmbuild / makepkg / bsdtar / tar 缺失时直接给出
#     安装指引后退出（需 sudo，脚本不代做，也绝不 cargo install）。
#   - 运行时依赖下限只写在本脚本一处（DEB_DEPENDS / RPM_REQUIRES /
#     PKG_DEPENDS 三个变量），并同时与 Cargo features、pkg-config 实测版本核对。
#   - 可复现构建：cargo --locked（Cargo.lock 须存在）。
set -euo pipefail
cd "$(dirname "$0")"

NAME=ssr-client-gtk
APP_ID=com.liangzhaoyuan12.ssr-client-gtk
RELEASE=1                                # deb/rpm/Arch 共用的 release 号
HOMEPAGE=https://github.com/liangzhaoyuan12/ssr-client-gtk
SUMMARY='GTK4/libadwaita ShadowsocksR client (single-port SOCKS5)'
DESC_LINES=(
  'A native GTK4 + libadwaita desktop client for ShadowsocksR, linking the'
  'ssr-client-rs core in-process. One click to start/stop the proxy; while'
  'enabled exactly one TCP port is listening (the routed SOCKS5 port) and the'
  'desktop system proxy follows it. UI in Chinese and English.'
)
# 运行时依赖下限的唯一声明处（三种包格式 + Cargo features 都要对齐它）
DEP_GTK=4.18
DEP_ADW=1.5
DEB_DEPENDS="libgtk-4-1 (>= $DEP_GTK), libadwaita-1-0 (>= $DEP_ADW)"
RPM_REQUIRES="gtk4 >= $DEP_GTK, libadwaita >= $DEP_ADW"
PKG_DEPENDS=("gtk4>=$DEP_GTK" "libadwaita>=$DEP_ADW")

log() { printf '\n==> %s\n' "$*"; }
die() { printf '错误: %s\n' "$*" >&2; exit 1; }

# ---------- 0. 参数 ----------
CHECK=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK=1 ;;
    *) die "未知参数: $arg（仅支持 --check）" ;;
  esac
done

# ---------- 1. 构建环境（GTK 依赖） ----------
if [ -f "$HOME/.local/gtk-sysroot/env.sh" ]; then
  # shellcheck source=/dev/null
  . "$HOME/.local/gtk-sysroot/env.sh"
fi
command -v cargo >/dev/null 2>&1 || die "找不到 cargo（安装 rustup/rust 后重试）"
command -v pkg-config >/dev/null 2>&1 || die "找不到 pkg-config（apt install pkg-config）"
pkg-config --exists gtk4 || die "pkg-config 找不到 gtk4：需要 gtk4-devel，或 source ~/.local/gtk-sysroot/env.sh"
pkg-config --exists libadwaita-1 || die "pkg-config 找不到 libadwaita-1：需要 libadwaita-devel"

# 版本下限比较（sort -V：$1 >= $2 ?）
version_ge() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$2" ]; }
GTK_VER=$(pkg-config --modversion gtk4)
ADW_VER=$(pkg-config --modversion libadwaita-1)
version_ge "$GTK_VER" "$DEP_GTK" || die "gtk4 $GTK_VER < $DEP_GTK（Cargo features 要求 v4_18）"
version_ge "$ADW_VER" "$DEP_ADW" || die "libadwaita $ADW_VER < $DEP_ADW（Cargo features 要求 v1_5）"
log "构建环境: gtk4 $GTK_VER / libadwaita $ADW_VER"

# ---------- 2. 系统打包工具（缺了给安装指引，不代装、绝不 cargo install） ----------
need() { # $1=命令 $2=安装指引
  command -v "$1" >/dev/null 2>&1 || die "缺少系统工具 $1：$2"
}
need file       "apt install file"
need tar        "apt install tar"
need dpkg-deb   "apt install dpkg"
need rpmbuild   "apt install rpm"
need makepkg    "apt install makepkg"
need bsdtar     "apt install libarchive-tools（makepkg 用它生成 .MTREE 并打包装箱）"
log "系统工具: $(dpkg-deb --version | head -1 | sed 's/,.*//') / $(rpmbuild --version) / $(makepkg --version | head -1) / $(bsdtar --version | awk '{print $1, $2}')"

# ---------- 3. 门禁（可选） ----------
if [ "$CHECK" = 1 ]; then
  log "门禁: fmt / clippy / test"
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test --quiet
fi

# ---------- 4. 版本与架构：单一来源 Cargo.toml，架构来自本机系统 ----------
[ -f Cargo.lock ] || die "缺少 Cargo.lock（--locked 可复现构建需要）"
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$VER" ] || die "无法从 Cargo.toml 读取 version"
grep -q "\"v4_18\"" Cargo.toml || die "Cargo.toml features 缺 v4_18（依赖下限 $DEP_GTK 与之对应）"
grep -q "\"v1_5\"" Cargo.toml  || die "Cargo.toml features 缺 v1_5（依赖下限 $DEP_ADW 与之对应）"
case "$DEB_DEPENDS" in *"libgtk-4-1 (>= $DEP_GTK)"*) ;; *) die "deb Depends 缺 libgtk-4-1 (>= $DEP_GTK)" ;; esac
case "$DEB_DEPENDS" in *"libadwaita-1-0 (>= $DEP_ADW)"*) ;; *) die "deb Depends 缺 libadwaita-1-0 (>= $DEP_ADW)" ;; esac
case "$RPM_REQUIRES" in *"gtk4 >= $DEP_GTK"*"libadwaita >= $DEP_ADW"*) ;; *) die "rpm Requires 下限与 $DEP_GTK/$DEP_ADW 不符" ;; esac
case "${PKG_DEPENDS[*]}" in *"gtk4>=$DEP_GTK"*"libadwaita>=$DEP_ADW"*) ;; *) die "PKGBUILD depends 下限与 $DEP_GTK/$DEP_ADW 不符" ;; esac

DEB_ARCH=$(dpkg --print-architecture)                    # loong64
RPM_ARCH=$(rpm --eval '%{_arch}')                        # loongarch64
TAR_ARCH=$(uname -m)                                     # loongarch64
[ -f /etc/makepkg.conf ] || die "缺少 /etc/makepkg.conf（makepkg 装配坏了）"
CARCH=$( (source /etc/makepkg.conf >/dev/null 2>&1 || true; printf '%s' "${CARCH:-}") )
[ -n "$CARCH" ] || die "无法从 /etc/makepkg.conf 读 CARCH"
log "版本: $VER（release $RELEASE）| 架构: deb=$DEB_ARCH rpm=$RPM_ARCH arch=$CARCH tar=$TAR_ARCH"

# ---------- 5. 图标：唯一来源 = 仓库根目录 icon.png ----------
# 桌面图标（.desktop 的 Icon=）、窗口图标、关于对话框、四件套包内的
# hicolor 图标全部由 icon.png 派生；每次构建重新生成，杜绝“包里装的是
# 另一张图”。512x512 是图标原尺寸（直接拷贝，逐字节等于源图）。
ICON=icon.png
[ -f "$ICON" ] || die "缺少图标源文件 icon.png（应用图标的唯一来源）"
case "$(file -b "$ICON")" in
  "PNG image data, 512 x 512"*) ;;
  *) die "icon.png 必须是 512x512 的 PNG：实际 = $(file -b "$ICON")" ;;
esac
if command -v magick >/dev/null 2>&1; then
  icon_resize() { magick "$ICON" -resize "$1x$1" "png32:$2"; }
  ICON_TOOL=magick
elif command -v convert >/dev/null 2>&1; then
  icon_resize() { convert "$ICON" -resize "$1x$1" "png32:$2"; }
  ICON_TOOL=convert
elif command -v gm >/dev/null 2>&1; then
  icon_resize() { gm convert "$ICON" -resize "$1x$1" "$2"; }
  ICON_TOOL=gm
elif command -v ffmpeg >/dev/null 2>&1; then
  # netpbm（pngtopam/pnmscale）会把 alpha 丢成黑底，不用；ffmpeg 保 RGBA
  icon_resize() { ffmpeg -loglevel error -y -i "$ICON" -vf "scale=$1:$1:flags=lanczos" "$2"; }
  ICON_TOOL=ffmpeg
else
  die "找不到图标缩放工具：需要 ImageMagick（magick/convert）、GraphicsMagick（gm）或 ffmpeg 之一"
fi
for s in 48 64 128 256 512; do
  out="data/icons/hicolor/${s}x${s}/apps/$APP_ID.png"
  mkdir -p "$(dirname "$out")"
  if [ "$s" = 512 ]; then
    cp -f "$ICON" "$out"                       # 512 不缩放：与源图逐字节一致
  else
    icon_resize "$s" "$out"
  fi
  [ -f "$out" ] || die "图标生成失败: $out"
  case "$(file -b "$out")" in
    "PNG image data, $s x $s"*) ;;
    *) die "生成的 $out 尺寸不对: $(file -b "$out")" ;;
  esac
done
# desktop 文件的 Icon= 必须指向同一个图标名
grep -q "^Icon=$APP_ID$" "data/$APP_ID.desktop" \
  || die "data/$APP_ID.desktop 的 Icon= 不是 $APP_ID"
# 1) 512 产物必须与 icon.png 逐字节一致（不是“另一张图”）
cmp -s "$ICON" "data/icons/hicolor/512x512/apps/$APP_ID.png" \
  || die "512x512 图标与 icon.png 不一致（512 是直接拷贝的，不一致即有别的东西动过它）"
# 2) 源码树必须能被 GTK 当成图标主题：没有 index.theme 就认不出 data/icons/hicolor
[ -f "data/icons/hicolor/index.theme" ] \
  || die "缺少 data/icons/hicolor/index.theme（cargo run 时 GTK 认不出这个主题目录）"
# 3) 除 icon.png 派生的 PNG 外不许有第二种图（历史 SVG 是另一套美术，必须绝迹）
if find data/icons -name "*.svg" | grep -q .; then
  die "data/icons 下存在 SVG：$(find data/icons -name '*.svg')（图标唯一来源是 icon.png）"
fi
log "图标: icon.png (512x512) → hicolor 48/64/128/256/512（$APP_ID，缩放工具=$ICON_TOOL），512 逐字节一致 ✓，index.theme 在 ✓，无 SVG ✓"

# ---------- 6. release 构建（--locked） ----------
log "cargo build --release --locked"
cargo build --release --locked
[ -x "target/release/$NAME" ] || die "构建产物 target/release/$NAME 不存在"

# ---------- 7. 一次 staging，四种包共用同一份文件清单 ----------
# 便携布局（bin/ + share/）= tar.gz 的内容；安装布局（usr/…）= deb/rpm/Arch。
# 四件套因此绝不可能出现“这个包里有图、那个包里没有”的漂移。
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
TAR_DIR="${NAME}-${VER}-${TAR_ARCH}"
STAGE="$WORK/tarroot/$TAR_DIR"     # tar.gz 顶层目录
ROOTFS="$WORK/rootfs"              # usr/ 布局
mkdir -p "$STAGE"
install -Dm755 "target/release/$NAME" "$STAGE/bin/$NAME"
install -Dm644 "data/$APP_ID.desktop"      "$STAGE/share/applications/$APP_ID.desktop"
install -Dm644 "data/$APP_ID.metainfo.xml" "$STAGE/share/metainfo/$APP_ID.metainfo.xml"
for s in 48 64 128 256 512; do
  install -Dm644 "data/icons/hicolor/${s}x${s}/apps/$APP_ID.png" \
                 "$STAGE/share/icons/hicolor/${s}x${s}/apps/$APP_ID.png"
done
for f in README.md CHANGELOG.md LICENSE; do
  install -Dm644 "$f" "$STAGE/share/doc/$NAME/$f"
done
# index.theme 只服务于源码树运行，装机时必须由 hicolor-icon-theme 包提供，
# 我们带一份会盖掉系统 hicolor 主题定义 —— 四件套都不许带它。
[ ! -e "$STAGE/share/icons/hicolor/index.theme" ] \
  || die "staging 里混进了 index.theme（会覆盖系统 hicolor 主题定义）"
mkdir -p "$ROOTFS/usr"
cp -a "$STAGE"/. "$ROOTFS/usr/"
log "staging: 便携布局 $TAR_DIR/{bin,share} + 安装布局 usr/（同一份文件清单）"

# ---------- 8. 四种产物（统一落在 packaging/，一处交付） ----------
rm -rf dist
mkdir -p packaging

log "tar.gz: tar -czf（便携布局，解压直跑）"
TARBALL="packaging/${TAR_DIR}.tar.gz"
tar -czf "$TARBALL" -C "$WORK/tarroot" "$TAR_DIR"

# --- 8a. deb：dpkg-deb ---
log "deb: dpkg-deb（Depends 的 ELF 部分由 dpkg-shlibdeps 补齐）"
DEB="packaging/${NAME}_${VER}-${RELEASE}_${DEB_ARCH}.deb"
DEBROOT="$WORK/debroot"
cp -a "$ROOTFS" "$DEBROOT"
mkdir -p "$DEBROOT/DEBIAN"
DEPS="$DEB_DEPENDS"
if command -v dpkg-shlibdeps >/dev/null 2>&1; then
  SH="$WORK/shlibdeps"
  mkdir -p "$SH/debian"
  cat > "$SH/debian/control" <<'EOF'
Source: ssr-client-gtk
Section: net
Priority: optional
Maintainer: liangzhaoyuan12
Build-Depends: debhelper-compat (= 13)

Package: ssr-client-gtk
Architecture: any
Depends: ${shlibs:Depends}
Description: placeholder for dpkg-shlibdeps
 placeholder
EOF
  install -Dm755 "$ROOTFS/usr/bin/$NAME" "$SH/debian/$NAME/usr/bin/$NAME"
  if AUTO=$(cd "$SH" && dpkg-shlibdeps -e "debian/$NAME/usr/bin/$NAME" -O 2>"$SH/err.log"); then
    AUTO=${AUTO#shlibs:Depends=}
    AUTO=${AUTO#shlibs:Depends: }
    # 显式声明的 GUI 依赖带版本下限，优先于 shlibdeps 算出的同名条目
    AUTO=$(printf '%s\n' "$AUTO" | tr ',' '\n' \
             | sed -e 's/^ *//' -e 's/ *$//' \
             | grep -v -e '^libgtk-4-1' -e '^libadwaita-1-0' -e '^$' \
             | paste -sd, - || true)
    if [ -n "$AUTO" ]; then
      AUTO=${AUTO//,/, }
      DEPS="$DEPS, $AUTO"
      log "deb 自动依赖（dpkg-shlibdeps）: $AUTO"
    fi
  else
    log "提示: dpkg-shlibdeps 未产出依赖（日志: $SH/err.log），deb 只带显式 GUI 依赖"
  fi
fi
{
  printf 'Package: %s\n' "$NAME"
  printf 'Version: %s-%s\n' "$VER" "$RELEASE"
  printf 'Section: net\n'
  printf 'Priority: optional\n'
  printf 'Architecture: %s\n' "$DEB_ARCH"
  printf 'Maintainer: liangzhaoyuan12\n'
  printf 'Depends: %s\n' "$DEPS"
  printf 'Homepage: %s\n' "$HOMEPAGE"
  printf 'Description: %s\n' "$SUMMARY"
  printf ' %s\n' "${DESC_LINES[@]}"
} > "$DEBROOT/DEBIAN/control"
chmod 755 "$DEBROOT/DEBIAN"
chmod 644 "$DEBROOT/DEBIAN/control"
dpkg-deb --build --root-owner-group "$DEBROOT" "$DEB" >/dev/null

# --- 8b. rpm：rpmbuild ---
log "rpm: rpmbuild（现写 spec，_topdir 用临时目录）"
RPMDIR="$WORK/rpmbuild"
mkdir -p "$RPMDIR"/BUILD "$RPMDIR"/BUILDROOT "$RPMDIR"/RPMS "$RPMDIR"/SPECS "$RPMDIR"/SRPMS
SPEC="$RPMDIR/SPECS/$NAME.spec"
# %files 直接取 rootfs 实际内容（只列文件，父目录 rpm 自带），杜绝漏打/多打
(cd "$ROOTFS" && find . -mindepth 1 -type f | sed 's|^\./|/|' | LC_ALL=C sort) > "$WORK/rpm-files.list"
cat > "$SPEC" <<EOF
Name: $NAME
Version: $VER
Release: $RELEASE
Summary: $SUMMARY
License: GPL-3.0-or-later
URL: $HOMEPAGE
Requires: $RPM_REQUIRES

%description
$(printf '%s\n' "${DESC_LINES[@]}")

%global debug_package %{nil}
%global _build_id_links none

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}
cp -a %{stagedir}/. %{buildroot}/

%files
%defattr(-,root,root,-)
EOF
cat "$WORK/rpm-files.list" >> "$SPEC"
if ! rpmbuild -bb --define "_topdir $RPMDIR" --define "stagedir $ROOTFS" "$SPEC" \
     > "$WORK/rpmbuild.log" 2>&1; then
  cat "$WORK/rpmbuild.log" >&2
  die "rpmbuild 失败（完整日志见上）"
fi
RPM="packaging/${NAME}-${VER}-${RELEASE}.${RPM_ARCH}.rpm"
mv -f "$RPMDIR/RPMS/$RPM_ARCH/${NAME}-${VER}-${RELEASE}.$RPM_ARCH.rpm" "$RPM"

# --- 8c. Arch：makepkg ---
log "Arch: makepkg（现写 PKGBUILD，--nodeps 跳过空 pacman 源，PKGEXT=zst）"
PKGBUILD_DIR="$WORK/pkgbuild"
mkdir -p "$PKGBUILD_DIR"
cp -f LICENSE "$PKGBUILD_DIR/LICENSE"
PKGDEP_QUOTED=$(printf "'%s' " "${PKG_DEPENDS[@]}")
# makepkg 用 $PACMAN 填 .BUILDINFO 的 pkginfo。本机 PATH 里的 pacman 可能是
# 同名的 X11 游戏（Deepin 的 `pacman` 包 = /usr/games/pacman），调它会连 X 打出
# X Error。不是真 pacman 就指向 /bin/true（pkginfo 留空，只影响 BUILDINFO 这条
# 元信息；依赖检查本来就被 --nodeps 跳过）。
PACMAN_BIN=$(type -P pacman 2>/dev/null || true)
MAKEPKG_PACMAN=/bin/true
case "$PACMAN_BIN" in
  */games/*) ;;                       # 同名游戏，不是包管理器
  ?*) MAKEPKG_PACMAN="$PACMAN_BIN" ;;  # 真 pacman
esac
cat > "$PKGBUILD_DIR/PKGBUILD" <<EOF
pkgname=$NAME
pkgver=$VER
pkgrel=$RELEASE
pkgdesc='$SUMMARY'
arch=('$CARCH')
url='$HOMEPAGE'
license=('GPL-3.0-or-later')
depends=($PKGDEP_QUOTED)
source=()
sha256sums=()

package() {
  cp -a $ROOTFS/. "\$pkgdir"/
  install -Dm644 $PKGBUILD_DIR/LICENSE "\$pkgdir/usr/share/licenses/$NAME/LICENSE"
}
EOF
(
  cd "$PKGBUILD_DIR"
  PACMAN="$MAKEPKG_PACMAN" PKGEXT='.pkg.tar.zst' makepkg -f --nodeps --nosign
)
ARCH_PKG="packaging/${NAME}-${VER}-${RELEASE}-${CARCH}.pkg.tar.zst"
mv -f "$PKGBUILD_DIR/${NAME}-${VER}-${RELEASE}-${CARCH}.pkg.tar.zst" "$ARCH_PKG"

# ---------- 9. 产物核验：文件存在 + 版本一致 + 图标/index.theme ----------
log "产物核验（版本必须 == $VER）"
for f in "$DEB" "$RPM" "$ARCH_PKG" "$TARBALL"; do
  [ -f "$f" ] || die "缺少产物: $f（本机架构 deb=$DEB_ARCH rpm=$RPM_ARCH arch=$CARCH tar=$TAR_ARCH；文件名里的版本与 Cargo.toml 不一致也会落到这里）"
done
# 交付物只允许出现在 packaging/：不许再冒出 dist/，仓库别处也不许有包
[ ! -d dist ] || die "出现了 dist/ 目录（四件套必须统一放 packaging/）"
STRAY=$(find . -path ./packaging -prune -o -path ./target -prune -o -path ./.git -prune -o \
        -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.pkg.tar.*' \) -print)
[ -z "$STRAY" ] || die "packaging/ 之外出现包文件：$STRAY（交付物只允许在 packaging/）"
log "产物只在 packaging/ ✓（无 dist/，仓库别处无包）"

DV=$(dpkg-deb -f "$DEB" Version)
case "$DV" in "$VER"*) ;; *) die "deb 内部版本 $DV != Cargo.toml $VER" ;; esac
[ "$(dpkg-deb -f "$DEB" Architecture)" = "$DEB_ARCH" ] \
  || die "deb Architecture != $DEB_ARCH：$(dpkg-deb -f "$DEB" Architecture)"
DEB_DEPS=$(dpkg-deb -f "$DEB" Depends)
case "$DEB_DEPS" in *"libgtk-4-1 (>= $DEP_GTK)"*) ;; *) die "deb Depends 缺 'libgtk-4-1 (>= $DEP_GTK)'：实际 = [$DEB_DEPS]" ;; esac
case "$DEB_DEPS" in *"libadwaita-1-0 (>= $DEP_ADW)"*) ;; *) die "deb Depends 缺 'libadwaita-1-0 (>= $DEP_ADW)'：实际 = [$DEB_DEPS]" ;; esac
case "$DEB_DEPS" in
  *libc6*) ;;
  *) log "提示: 未见 libc6 自动依赖（dpkg-shlibdeps 在本机没产出 ELF 依赖）；在 Debian/Ubuntu 构建机上重跑会补齐" ;;
esac
log "deb Version: $DV | Depends: $DEB_DEPS"

# --dbpath 指向空目录：只查包文件不查已装库，绕开本机 /var/lib/rpm 的权限报错
RPM_Q="rpm -qp --dbpath $WORK/rpmdb"
RV=$($RPM_Q --queryformat '%{VERSION}-%{RELEASE}' "$RPM")
case "$RV" in "$VER"*) ;; *) die "rpm 内部版本 $RV != Cargo.toml $VER" ;; esac
RPM_REQ=$($RPM_Q --requires "$RPM")
case "$RPM_REQ" in *"gtk4 >= $DEP_GTK"*) ;; *) die "rpm Requires 缺 'gtk4 >= $DEP_GTK'：实际 = [$RPM_REQ]" ;; esac
case "$RPM_REQ" in *"libadwaita >= $DEP_ADW"*) ;; *) die "rpm Requires 缺 'libadwaita >= $DEP_ADW'：实际 = [$RPM_REQ]" ;; esac
log "rpm Version: $RV | Requires: $(printf '%s' "$RPM_REQ" | tr '\n' ' ')"

PKGINFO=$(bsdtar -xOf "$ARCH_PKG" .PKGINFO)
case "$PKGINFO" in *"depend = gtk4>=$DEP_GTK"*) ;; *) die "Arch .PKGINFO 缺 depend = gtk4>=$DEP_GTK" ;; esac
case "$PKGINFO" in *"depend = libadwaita>=$DEP_ADW"*) ;; *) die "Arch .PKGINFO 缺 depend = libadwaita>=$DEP_ADW" ;; esac
log "Arch .PKGINFO depend: $(printf '%s' "$PKGINFO" | grep '^depend =' | tr '\n' ' ')"

# 图标必须真的进包（用变量承接输出，避免 pipefail + grep -q 的 SIGPIPE 误判）
ICON_PATH="hicolor/512x512/apps/$APP_ID.png"
BIN_PATH="bin/$NAME"
DEB_LIST=$(dpkg-deb -c "$DEB")
RPM_LIST=$($RPM_Q --list "$RPM")
TAR_LIST=$(tar -tzf "$TARBALL")
if ARCH_LIST=$(tar --zstd -tf "$ARCH_PKG" 2>/dev/null); then :; else ARCH_LIST=""; fi
case "$DEB_LIST"  in *"$ICON_PATH"*) ;; *) die "deb 内缺 512x512 图标 $ICON_PATH" ;; esac
case "$RPM_LIST"  in *"$ICON_PATH"*) ;; *) die "rpm 内缺 512x512 图标 $ICON_PATH" ;; esac
case "$TAR_LIST"  in *"$ICON_PATH"*) ;; *) die "tar.gz 内缺 512x512 图标 $ICON_PATH" ;; esac
if [ -n "$ARCH_LIST" ]; then
  case "$ARCH_LIST" in *"$ICON_PATH"*) ;; *) die "Arch 包内缺 512x512 图标 $ICON_PATH" ;; esac
else
  die "无法列出 Arch 包内容（tar --zstd 失败）"
fi
case "$DEB_LIST" in *"usr/$BIN_PATH"*) ;; *) die "deb 内缺可执行文件 usr/$BIN_PATH" ;; esac
case "$RPM_LIST" in *"usr/$BIN_PATH"*) ;; *) die "rpm 内缺可执行文件 usr/$BIN_PATH" ;; esac
case "$TAR_LIST" in *"$TAR_DIR/$BIN_PATH"*) ;; *) die "tar.gz 内缺 $TAR_DIR/$BIN_PATH" ;; esac
case "$ARCH_LIST" in *"usr/$BIN_PATH"*) ;; *) die "Arch 包内缺可执行文件 usr/$BIN_PATH" ;; esac
# index.theme 不许进任何包
case "$TAR_LIST" in *"share/icons/hicolor/index.theme"*) die "tar.gz 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;; esac
case "$DEB_LIST" in *"share/icons/hicolor/index.theme"*) die "deb 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;; esac
case "$RPM_LIST" in *"share/icons/hicolor/index.theme"*) die "rpm 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;; esac
case "$ARCH_LIST" in *"share/icons/hicolor/index.theme"*) die "Arch 包内含 index.theme（会覆盖系统 hicolor 主题定义）" ;; esac
log "四包内容一致 ✓（可执行文件 + 512 图标在，index.theme 不在）"

log "四件套齐备（$VER，release $RELEASE）"
ls -lh "$DEB" "$RPM" "$ARCH_PKG" "$TARBALL"
sha256sum "$DEB" "$RPM" "$ARCH_PKG" "$TARBALL"
log "打包完成（全部由系统工具产出: dpkg-deb / rpmbuild / makepkg / tar）"
