#!/usr/bin/env bash
# 构建 ssr-client-gtk 的四种发布包（GOAL D3）：
#   deb + rpm + Arch(.pkg.tar.zst) + tar.gz
#
# 用法：
#   ./build.sh            # 构建四件套
#   ./build.sh --check    # 先跑门禁（fmt/clippy/test）再构建
#
# 版本问题：版本单一来源 = Cargo.toml（GOAL §10 D 版本约定）。本脚本读取后
#   1) 同步 packaging/PKGBUILD 的 pkgver（含 source 文件名）；
#   2) 构建完成后断言四个产物文件名内嵌的版本 == Cargo.toml 版本，
#      任一漂移即失败退出，杜绝"包上写的版本不是 Cargo.toml 的版本"。
#
# 依赖问题：
#   - 构建库：优先 source 用户态 sysroot（~/.local/gtk-sysroot/env.sh，
#     本机无 sudo 装 devel 的方案），否则用系统 pkg-config；随后核验
#     gtk4 >= 4.18、libadwaita-1 >= 1.7（与 Cargo features v4_18/v1_7 一致）。
#   - 打包工具：cargo-deb / cargo-generate-rpm 缺失时自动 cargo install；
#     makepkg / rsync 缺失给出安装指引后退出（需 sudo，脚本不代做）。
#   - 运行时依赖声明：核验三处（Cargo features、rpm requires、PKGBUILD
#     depends）的版本下限一致（4.18 / 1.7）。
#   - 可复现构建：cargo --locked（Cargo.lock 须存在且与 Cargo.toml 匹配）。
set -euo pipefail
cd "$(dirname "$0")"

NAME=ssr-client-gtk
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
command -v pkg-config >/dev/null 2>&1 || die "找不到 pkg-config"
pkg-config --exists gtk4 || die "pkg-config 找不到 gtk4：需要 gtk4-devel，或 source ~/.local/gtk-sysroot/env.sh"
pkg-config --exists libadwaita-1 || die "pkg-config 找不到 libadwaita-1：需要 libadwaita-devel"

# 版本下限比较（sort -V：$1 >= $2 ?）
version_ge() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$2" ]; }
GTK_VER=$(pkg-config --modversion gtk4)
ADW_VER=$(pkg-config --modversion libadwaita-1)
version_ge "$GTK_VER" 4.18 || die "gtk4 $GTK_VER < 4.18（Cargo features 要求 v4_18）"
version_ge "$ADW_VER" 1.7 || die "libadwaita $ADW_VER < 1.7（Cargo features 要求 v1_7）"
log "构建环境: gtk4 $GTK_VER / libadwaita $ADW_VER"

# ---------- 2. 打包工具 ----------
if ! command -v cargo-deb >/dev/null 2>&1 && ! cargo deb --help >/dev/null 2>&1; then
  log "安装 cargo-deb（缺失）"
  cargo install cargo-deb
fi
if ! cargo generate-rpm --help >/dev/null 2>&1; then
  log "安装 cargo-generate-rpm（缺失）"
  cargo install cargo-generate-rpm
fi
command -v makepkg >/dev/null 2>&1 \
  || die "找不到 makepkg：Arch 包需要 pacman 工具链（dnf install pacman，需 sudo）"
command -v rsync >/dev/null 2>&1 || die "找不到 rsync（tarball.sh 需要，dnf install rsync）"

# ---------- 3. 门禁（可选） ----------
if [ "$CHECK" = 1 ]; then
  log "门禁: fmt / clippy / test"
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test --quiet
fi

# ---------- 4. 版本：单一来源 Cargo.toml，同步 PKGBUILD ----------
[ -f Cargo.lock ] || die "缺少 Cargo.lock（--locked 可复现构建需要）"
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$VER" ] || die "无法从 Cargo.toml 读取 version"
PKGVER_LINE=$(grep -n '^pkgver=' packaging/PKGBUILD | head -1)
if [ -z "$PKGVER_LINE" ]; then
  die "packaging/PKGBUILD 缺少 pkgver= 行"
fi
if ! grep -q "^pkgver=$VER\$" packaging/PKGBUILD; then
  log "同步 PKGBUILD pkgver: -> $VER"
  sed -i "s/^pkgver=.*/pkgver=$VER/" packaging/PKGBUILD
fi
# 运行时依赖声明三处一致性（features / rpm / PKGBUILD）
grep -q '"v4_18"' Cargo.toml || die "Cargo.toml features 缺 v4_18"
grep -q '"v1_7"' Cargo.toml || die "Cargo.toml features 缺 v1_7"
grep -q 'gtk4 = ">= 4.18"' Cargo.toml || die "rpm requires 的 gtk4 下限不是 >= 4.18"
grep -q 'libadwaita = ">= 1.7"' Cargo.toml || die "rpm requires 的 libadwaita 下限不是 >= 1.7"
grep -q "'gtk4>=4.18'" packaging/PKGBUILD || die "PKGBUILD depends 的 gtk4 下限不是 >=4.18"
grep -q "'libadwaita>=1.7'" packaging/PKGBUILD || die "PKGBUILD depends 的 libadwaita 下限不是 >=1.7"
grep -q 'libgtk-4-1 (>= 4.18)' Cargo.toml || die "deb depends 的 libgtk-4-1 下限不是 (>= 4.18)"
grep -q 'libadwaita-1-0 (>= 1.7)' Cargo.toml || die "deb depends 的 libadwaita-1-0 下限不是 (>= 1.7)"
log "版本: $VER（已核验 features/rpm/PKGBUILD 依赖下限一致）"

# ---------- 5. release 构建（--locked） ----------
log "cargo build --release --locked"
cargo build --release --locked

# ---------- 6. 四种产物 ----------
log "deb: cargo deb"
cargo deb

log "rpm: cargo generate-rpm"
cargo generate-rpm

log "tar.gz: packaging/tarball.sh（发布布局 + PKGBUILD 源码 tar）"
./packaging/tarball.sh

log "Arch: makepkg（Fedora 主机: --nodeps 跳过空 pacman 源；PKGEXT=zst）"
cp "dist/$NAME-$VER.tar.gz" packaging/
(
  cd packaging
  PKGEXT='.pkg.tar.zst' makepkg -f --nodeps
)

# ---------- 7. 产物核验：文件存在 + 版本一致 ----------
log "产物核验（版本必须 == $VER）"
DEB="target/debian/${NAME}_${VER}-1_amd64.deb"
RPM="target/generate-rpm/${NAME}-${VER}-1.x86_64.rpm"
ARCH="packaging/${NAME}-${VER}-1-x86_64.pkg.tar.zst"
TARBALL="dist/${NAME}-${VER}-x86_64.tar.gz"
for f in "$DEB" "$RPM" "$ARCH" "$TARBALL"; do
  [ -f "$f" ] || die "缺少产物: $f（文件名版本与 Cargo.toml 不一致也会落到这里）"
done
if command -v dpkg-deb >/dev/null 2>&1; then
  DV=$(dpkg-deb -f "$DEB" Version)
  case "$DV" in
    "$VER"*) ;;
    *) die "deb 内部版本 $DV != Cargo.toml $VER" ;;
  esac
  # 产物级依赖断言：GUI 运行库必须带版本下限（用户要求 deb 标明 >= 4.18）
  DEB_DEPS=$(dpkg-deb -f "$DEB" Depends)
  case "$DEB_DEPS" in
    *"libgtk-4-1 (>= 4.18)"*) ;;
    *) die "deb Depends 缺 'libgtk-4-1 (>= 4.18)'：实际 = [$DEB_DEPS]" ;;
  esac
  case "$DEB_DEPS" in
    *"libadwaita-1-0 (>= 1.7)"*) ;;
    *) die "deb Depends 缺 'libadwaita-1-0 (>= 1.7)'：实际 = [$DEB_DEPS]" ;;
  esac
  # $auto = dpkg-shlibdeps 解析 ELF 自动依赖；仅在 Debian 系（有系统库
  # shlibs 记录）构建机上展开。非 Debian 系会静默跳过（cargo-deb 只发
  # warning），这里显式提示：显式声明的 GUI 依赖始终在，libc6/libgcc-s1
  # 等基础库在 Debian/Ubuntu 上由 Essential/传递依赖保证。
  case "$DEB_DEPS" in
    *libc6*) ;;
    *) log "提示: \$auto 未展开（本机非 Debian 系，dpkg-shlibdeps 无系统库 shlibs 记录）；在 Debian/Ubuntu 构建机上重跑 build.sh 会自动补齐 libc6 等 ELF 依赖" ;;
  esac
  log "deb Version: $DV | Depends: $DEB_DEPS"
fi
if command -v rpm >/dev/null 2>&1; then
  RV=$(rpm -qp --queryformat '%{VERSION}-%{RELEASE}' "$RPM")
  case "$RV" in
    "$VER"*) ;;
    *) die "rpm 内部版本 $RV != Cargo.toml $VER" ;;
  esac
  log "rpm Version: $RV"
fi

log "四件套齐备（$VER）"
ls -lh "$DEB" "$RPM" "$ARCH" "$TARBALL"
sha256sum "$DEB" "$RPM" "$ARCH" "$TARBALL"
log "打包完成"
