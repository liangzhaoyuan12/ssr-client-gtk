#!/usr/bin/env bash
# Build the portable tar.gz artifact:
#   dist/ssr-client-gtk-<ver>-x86_64.tar.gz
# containing bin + desktop + metainfo + hicolor icons + LICENSE + README.
set -euo pipefail
cd "$(dirname "$0")/.."

VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
NAME=ssr-client-gtk
# Staging dir doubles as PKGBUILD's `source=(...)` tarball (no arch suffix);
# the published artifact gets the -x86_64 name required by GOAL 6.4.
OUT="$NAME-$VER"

echo "==> cargo build --release"
cargo build --release

echo "==> staging dist/$OUT"
rm -rf "dist/$OUT"
mkdir -p "dist/$OUT/bin" \
         "dist/$OUT/share/applications" \
         "dist/$OUT/share/metainfo" \
         "dist/$OUT/share/doc/$NAME"

cp target/release/ssr-client-gtk "dist/$OUT/bin/"
cp data/com.liangzhaoyuan12.ssr-client-gtk.desktop "dist/$OUT/share/applications/"
cp data/com.liangzhaoyuan12.ssr-client-gtk.metainfo.xml "dist/$OUT/share/metainfo/"
cp -r data/icons "dist/$OUT/share/icons"
cp LICENSE README.md CHANGELOG.md "dist/$OUT/share/doc/$NAME/"

echo "==> dist/$OUT.tar.gz"
# Published artifact: portable layout (bin + share …).
tar -C dist -czf "dist/$OUT-x86_64.tar.gz" "$OUT"
ls -lh "dist/$OUT-x86_64.tar.gz"
(tar -tzf "dist/$OUT-x86_64.tar.gz" | head -20) || true

# Source tarball for packaging/PKGBUILD's `source=(...)` (top-level dir
# must be exactly $OUT so makepkg's build()/package() can cd into it).
SRCSTAGE="dist/$OUT-srcstage"
rm -rf "$SRCSTAGE"
mkdir -p "$SRCSTAGE"
rsync -a \
  --exclude target/ --exclude dist/ --exclude .git/ \
  --exclude 'packaging/src/' --exclude 'packaging/pkg/' \
  --exclude 'packaging/*.tar.gz' --exclude 'packaging/*.pkg.tar.*' \
  ./ "$SRCSTAGE/"
tar -C dist --transform "s,^$OUT-srcstage,$OUT," -czf "dist/$OUT.tar.gz" "$OUT-srcstage"
rm -rf "$SRCSTAGE"
ls -lh "dist/$OUT.tar.gz"
(tar -tzf "dist/$OUT.tar.gz" | head -8) || true
