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

# ---------- 4b. 图标：唯一来源 = 仓库根目录 icon.png ----------
# 桌面图标（.desktop 的 Icon=）、窗口图标、关于对话框、四件套包内的
# hicolor 图标全部由 icon.png 派生；每次构建重新生成，杜绝"包里装的是
# 另一张图"。512x512 是图标原尺寸，其余尺寸由它缩放。
ICON=icon.png
ICON_ID=com.liangzhaoyuan12.ssr-client-gtk
[ -f "$ICON" ] || die "缺少图标源文件 icon.png（应用图标的唯一来源）"
ICON_DESC=$(file -b "$ICON")
case "$ICON_DESC" in
  "PNG image data, 512 x 512"*) ;;
  *) die "icon.png 必须是 512x512 的 PNG：实际 = $ICON_DESC" ;;
esac
if command -v magick >/dev/null 2>&1; then
  icon_resize() { magick "$ICON" -resize "$1" "png32:$2"; }
elif command -v convert >/dev/null 2>&1; then
  icon_resize() { convert "$ICON" -resize "$1" "png32:$2"; }
else
  die "找不到 ImageMagick（magick/convert）：需要它从 icon.png 生成各尺寸图标"
fi
for s in 48 64 128 256 512; do
  out="data/icons/hicolor/${s}x${s}/apps/$ICON_ID.png"
  mkdir -p "$(dirname "$out")"
  icon_resize "${s}x${s}" "$out"
  [ -f "$out" ] || die "图标生成失败: $out"
  case "$(file -b "$out")" in
    "PNG image data, $s x $s"*) ;;
    *) die "生成的 $out 尺寸不对: $(file -b "$out")" ;;
  esac
done
# desktop 文件的 Icon= 必须指向同一个图标名
grep -q "^Icon=$ICON_ID$" "data/$ICON_ID.desktop" \
  || die "data/$ICON_ID.desktop 的 Icon= 不是 $ICON_ID"
# 1) 512 产物必须与 icon.png 逐像素一致（不是"另一张图"）
if command -v compare >/dev/null 2>&1; then
  # `compare -metric AE` prints e.g. `0 (0)`; take the leading count.
  AE=$(compare -metric AE "$ICON" \
        "data/icons/hicolor/512x512/apps/$ICON_ID.png" null: 2>&1) || true
  AE=${AE%% *}
  [ "$AE" = "0" ] || die "512x512 图标与 icon.png 不一致（差异像素=$AE）"
else
  log "提示: 找不到 ImageMagick 的 compare，跳过 512 与 icon.png 的逐像素比对"
fi
# 2) 源码树必须能被 GTK 当成图标主题：没有 index.theme 就认不出 data/icons/hicolor
[ -f "data/icons/hicolor/index.theme" ] \
  || die "缺少 data/icons/hicolor/index.theme（cargo run 时 GTK 认不出这个主题目录）"
# 3) 除 icon.png 派生的 PNG 外不许有第二种图（历史 SVG 是另一套美术，必须绝迹）
if find data/icons -name "*.svg" | grep -q .; then
  die "data/icons 下存在 SVG：$(find data/icons -name '*.svg')（图标唯一来源是 icon.png）"
fi
log "图标: icon.png (512x512) → hicolor 48/64/128/256/512（$ICON_ID），512 逐像素一致 ✓，index.theme 在 ✓，无 SVG ✓"

# ---------- 5. release 构建（--locked） ----------
log "cargo build --release --locked"
cargo build --release --locked

# ---------- 6. 四种产物（统一落在 packaging/，一处交付） ----------
# cargo deb / cargo generate-rpm 默认把包写进 target/，tarball.sh 以前写
# dist/ —— 交付物因此散在三个地方。这里先清旧件，打完立刻把 deb/rpm 搬进
# packaging/，tarball.sh 直接写 packaging/，仓库里不再出现 dist/。
rm -rf target/debian target/generate-rpm dist

log "deb: cargo deb"
cargo deb

log "rpm: cargo generate-rpm"
cargo generate-rpm

log "deb/rpm 移入 packaging/（交付物只放这里）"
mv -f "target/debian/${NAME}_${VER}-1_amd64.deb" packaging/
mv -f "target/generate-rpm/${NAME}-${VER}-1.x86_64.rpm" packaging/

log "tar.gz: packaging/tarball.sh（发布布局 + PKGBUILD 源码 tar，直接写 packaging/）"
./packaging/tarball.sh

log "Arch: makepkg（Fedora 主机: --nodeps 跳过空 pacman 源；PKGEXT=zst）"
(
  cd packaging
  PKGEXT='.pkg.tar.zst' makepkg -f --nodeps
)

# ---------- 7. 产物核验：文件存在 + 版本一致 ----------
log "产物核验（版本必须 == $VER）"
DEB="packaging/${NAME}_${VER}-1_amd64.deb"
RPM="packaging/${NAME}-${VER}-1.x86_64.rpm"
ARCH="packaging/${NAME}-${VER}-1-x86_64.pkg.tar.zst"
TARBALL="packaging/${NAME}-${VER}-x86_64.tar.gz"
for f in "$DEB" "$RPM" "$ARCH" "$TARBALL"; do
  [ -f "$f" ] || die "缺少产物: $f（文件名版本与 Cargo.toml 不一致也会落到这里）"
done
# 交付物只允许出现在 packaging/：不许再冒出 dist/，也不许把包留在 target/。
[ ! -d dist ] || die "出现了 dist/ 目录（四件套必须统一放 packaging/）"
for stale in target/debian/*.deb target/generate-rpm/*.rpm; do
  [ -e "$stale" ] || continue
  die "包残留在 target/ 下：$stale（交付物只允许在 packaging/）"
done
log "产物只在 packaging/ ✓（无 dist/，target/ 无包）"
# 源码 tar 是 PKGBUILD 的输入，绝不能把 packaging/ 下的交付包自己打进去
# （rsync 按目录排除，packaging/ 里新增的 deb/rpm 必须显式排除）。
SRC_TAR="packaging/${NAME}-${VER}.tar.gz"
[ -f "$SRC_TAR" ] || die "缺少源码 tar: $SRC_TAR"
SRC_LIST=$(tar -tzf "$SRC_TAR")
case "$SRC_LIST" in
  *.deb*|*.rpm*|*.pkg.tar.*)
    die "源码 tar 混进了包文件：$SRC_TAR（packaging/ 下的 deb/rpm 必须被排除）" ;;
esac
log "源码 tar 未混入包 ✓（$SRC_TAR）"
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

# 图标必须真的进包（用变量承接输出，避免 pipefail + grep -q 的 SIGPIPE 误判）
ICON_PATH="hicolor/512x512/apps/$ICON_ID.png"
if command -v dpkg-deb >/dev/null 2>&1; then
  DEB_LIST=$(dpkg-deb -c "$DEB")
  case "$DEB_LIST" in *"$ICON_PATH"*) ;; *) die "deb 内缺 512x512 图标 $ICON_PATH" ;; esac
fi
if command -v rpm >/dev/null 2>&1; then
  RPM_LIST=$(rpm -qlp "$RPM")
  case "$RPM_LIST" in *"$ICON_PATH"*) ;; *) die "rpm 内缺 512x512 图标 $ICON_PATH" ;; esac
fi
TAR_LIST=$(tar -tzf "$TARBALL")
case "$TAR_LIST" in *"$ICON_PATH"*) ;; *) die "tar.gz 内缺 512x512 图标 $ICON_PATH" ;; esac
if ARCH_LIST=$(tar --zstd -tf "$ARCH" 2>/dev/null); then
  case "$ARCH_LIST" in *"$ICON_PATH"*) ;; *) die "Arch 包内缺 512x512 图标 $ICON_PATH" ;; esac
else
  log "提示: 当前 tar 不支持 zstd，跳过 Arch 包内容断言（图标已由源码侧断言保证）"
fi
# index.theme 只服务于源码树运行，装机时必须由 hicolor-icon-theme 包提供，
# 我们带一份会盖掉系统 hicolor 主题定义 —— 四件套都不许带它。
case "$TAR_LIST" in
  *"share/icons/hicolor/index.theme"*) die "tar.gz 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;;
esac
if command -v dpkg-deb >/dev/null 2>&1; then
  case "$DEB_LIST" in
    *"share/icons/hicolor/index.theme"*) die "deb 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;;
  esac
fi
if command -v rpm >/dev/null 2>&1; then
  case "$RPM_LIST" in
    *"share/icons/hicolor/index.theme"*) die "rpm 内含 index.theme（会覆盖系统 hicolor 主题定义）" ;;
  esac
fi
if [ -n "${ARCH_LIST:-}" ]; then
  case "$ARCH_LIST" in
    *"usr/share/icons/hicolor/index.theme"*) die "Arch 包内含 index.theme（会覆盖系统 hicolor 主题定义）" ;;
  esac
fi
log "index.theme 未进包 ✓（源码树有、包里无）"
log "四件套齐备（$VER）"
ls -lh "$DEB" "$RPM" "$ARCH" "$TARBALL"
sha256sum "$DEB" "$RPM" "$ARCH" "$TARBALL"
log "打包完成"
