#!/usr/bin/env bash
# In-container install smoke (GOAL 6.5), run as:
#   smoke-in-container.sh /pkg.<ext>
# Installs the mounted package, verifies payload files, then runs the
# full enable→disable→close cycle with the QA hooks (no desktop session
# inside a container, so the desktop is "Unsupported" — the code's
# degraded path: proxy runs, system-proxy step is skipped with a toast).
set -e
PKG="$1"

case "$PKG" in
  *.rpm)
    dnf install -y "$PKG" 2>&1 | tail -4
    ;;
  *.deb)
    apt-get update -qq 2>&1 | tail -2
    DEBIAN_FRONTEND=noninteractive apt-get install -y "$PKG" 2>&1 | tail -4
    ;;
  *.pkg.tar.zst)
    sed -i 's|^Server = .*|Server = https://mirrors.tuna.tsinghua.edu.cn/archlinux/$repo/os/$arch|' \
      /etc/pacman.d/mirrorlist
    pacman -Sy --noconfirm >/dev/null 2>&1 || true
    pacman -U --noconfirm "$PKG" 2>&1 | tail -5
    ;;
  *)
    echo "unknown package type: $PKG"; exit 2
    ;;
esac

echo "== payload files =="
command -v ssr-client-gtk
test -f /usr/share/applications/com.liangzhaoyuan12.ssr-client-gtk.desktop && echo DESKTOP_OK
test -f /usr/share/metainfo/com.liangzhaoyuan12.ssr-client-gtk.metainfo.xml && echo METAINFO_OK
test -f /usr/share/icons/hicolor/256x256/apps/com.liangzhaoyuan12.ssr-client-gtk.png \
  -o -f /usr/share/icons/hicolor/scalable/apps/com.liangzhaoyuan12.ssr-client-gtk.svg && echo ICON_OK

mkdir -p "$HOME/.config" "$HOME/.ssr"
# profile: server unreachable is fine — enable only binds the local port
cat > "$HOME/.ssr/localtest.json" <<'JSON'
{
 "password": "smoke",
 "method": "aes-256-cfb",
 "protocol": "auth_aes128_sha1",
 "protocol_param": "",
 "obfs": "tls1.2_ticket_auth",
 "obfs_param": "",
 "udp": true,
 "idle_timeout": 300,
 "connect_timeout": 6,
 "udp_timeout": 6,
 "client_settings": {
  "server": "127.0.0.1",
  "server_port": 2800,
  "listen_address": "0.0.0.0",
  "listen_port": 1082
 }
}
JSON

echo "== display env =="
echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR WAYLAND_DISPLAY=$WAYLAND_DISPLAY DISPLAY=$DISPLAY"
ls "$XDG_RUNTIME_DIR" 2>/dev/null | grep wayland || echo "no wayland socket visible"
command -v kwriteconfig5 gsettings 2>/dev/null || echo "(no desktop config tools -> Unsupported desktop path)"

env GTK_A11Y=none \
    SSR_GTK_DEV_SELECT=localtest \
    SSR_GTK_DEV_PROXY=enable \
    SSR_GTK_DEV_DISABLE_AFTER=8 \
    SSR_GTK_DEV_SELFCLOSE=11 \
    ssr-client-gtk >/tmp/app.log 2>&1 &
APP=$!
sleep 5
if ! kill -0 $APP 2>/dev/null; then
  echo "== app died early; log: =="
  cat /tmp/app.log
  exit 1
fi
echo "== app alive (pid $APP); log so far =="
grep -viE 'gdk-debug' /tmp/app.log | head -10 || true

echo "== t=5s enabled: 1082 (043A) listeners =="
grep -c ':043A' /proc/net/tcp || { echo "1082 MISSING (BAD)"; exit 1; }
echo "all LISTEN sockets: $(awk '$4=="0A"{print $2}' /proc/net/tcp | tr '\n' ' ')"

sleep 7
echo "== t=12s after scheduled disable + close =="
if grep -q ':043A' /proc/net/tcp; then echo "1082 STILL UP (BAD)"; exit 1; fi
echo "1082 gone"
if kill -0 $APP 2>/dev/null; then echo "PROCESS STILL ALIVE (BAD)"; exit 1; fi
echo "process exited"
echo "== log hits (all) =="
grep -iE 'error|critical' /tmp/app.log | head -20
BAD_HITS=$(grep -iE 'error|critical' /tmp/app.log \
  | grep -viE 'gdk|mesa|zink|vulkan|no such backend|at-spi|accessibility' || true)
if [ -n "$BAD_HITS" ]; then
  echo "log has errors (BAD):"
  echo "$BAD_HITS" | head -3
  exit 1
fi
echo "log clean"
echo "CONTAINER_SMOKE_PASS"
