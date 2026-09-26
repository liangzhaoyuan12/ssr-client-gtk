# ssr-client-gtk

原生 **GTK4 + libadwaita** 的 ShadowsocksR 桌面客户端，进程内链接
[`ssr-client-rs`](https://cnb.cool/liangzhaoyuan12/ssr-client-rs) 协议核心
（无 sidecar、无 WebView）。中英双语界面，一键启停，自动跟随桌面系统代理。

A native **GTK4 + libadwaita** desktop client for ShadowsocksR, linking the
[`ssr-client-rs`](https://cnb.cool/liangzhaoyuan12/ssr-client-rs) core
in-process — no sidecar, no WebView. zh-CN / en-US UI, one click to start or
stop the proxy, with automatic system-proxy handling.

---

## 系统要求 / Requirements

- GTK ≥ 4.18、libadwaita ≥ 1.7（即 GNOME 48 / Fedora 42+ 同期及以上）
- 一个可用的 ShadowsocksR 服务端地址
- 系统代理自动设置目前支持 **KDE Plasma** 与 **GNOME 系**（含 Cinnamon /
  MATE / Ubuntu / deepin / COSMIC 等）；其他桌面会提示手动配置

- GTK ≥ 4.18 and libadwaita ≥ 1.7 (the GNOME 48 / Fedora 42+ generation or newer)
- A working ShadowsocksR server to connect to
- Automatic system proxy currently supports **KDE Plasma** and **GNOME-based**
  desktops; elsewhere the app tells you how to configure it manually

## 安装 / Install

四种发行产物，任选其一 / pick one of four artifacts:

```bash
# Debian / Ubuntu
sudo dpkg -i ssr-client-gtk_0.1.0-1_amd64.deb      # 或 apt install ./<deb>

# Fedora / RPM 系
sudo dnf install ./ssr-client-gtk-0.1.0-1.x86_64.rpm

# Arch Linux
pacman -U ssr-client-gtk-0.1.0-1-x86_64.pkg.tar.zst

# 免安装 tar.gz / portable tarball
tar -xzf ssr-client-gtk-0.1.0-x86_64.tar.gz
./ssr-client-gtk-0.1.0-x86_64/bin/ssr-client-gtk
```

从源码构建 / Build from source:

```bash
cargo build --release        # 需要系统 gtk4-devel / libadwaita-devel
./target/release/ssr-client-gtk
```

### 打包四件套 / Build all four packages

```bash
./build.sh            # 构建 deb + rpm + Arch(.pkg.tar.zst) + tar.gz
./build.sh --check    # 先跑门禁（fmt/clippy/test）再构建
```

版本单一来源 = `Cargo.toml`：脚本读取后自动同步 `packaging/PKGBUILD` 的
`pkgver`，构建完断言四个产物文件名与包内版本（deb `Version` / rpm
`%{VERSION}`）== `Cargo.toml` 版本，不一致即失败。依赖处理：核验构建环境
gtk4 ≥ 4.18 / libadwaita ≥ 1.7（与 Cargo features `v4_18`/`v1_7` 一致）、
缺失的 `cargo-deb`/`cargo-generate-rpm` 自动 `cargo install`、`makepkg`/`rsync`
缺失给出安装指引、核验 features/rpm requires/PKGBUILD depends 三处运行时
依赖下限一致、`cargo --locked` 保证可复现构建。产物路径：

```text
target/debian/ssr-client-gtk_<ver>-1_amd64.deb
target/generate-rpm/ssr-client-gtk-<ver>-1.x86_64.rpm
packaging/ssr-client-gtk-<ver>-1-x86_64.pkg.tar.zst
dist/ssr-client-gtk-<ver>-x86_64.tar.gz
```

## 使用 / Usage

1. 点击「新建配置」，填写服务器地址、端口、密码，选择加密 / 协议 / 混淆
   （下拉选项与 `ssr-client-rs` 的枚举完全一致）。
2. 选中配置后点击「启用代理」：本地起 SOCKS5 监听，并把桌面系统代理指向它。
3. 「停用代理」精确还原系统代理原值；直接关窗同样会先停代理、再还原系统代理。

1. Create a profile (server, port, password, cipher / protocol / obfs).
2. Select it and hit **Enable**: the SOCKS5 listener starts and the desktop
   system proxy is pointed at it.
3. **Disable** restores the system proxy to its exact previous values — closing
   the window while enabled does the same, in that order.

### 配置目录 / Where data lives

| 路径 Path | 内容 Contents |
| --- | --- |
| `~/.ssr/*.json` | 配置文件（文件名 = 配置名，仅 `^[a-zA-Z]+$`） |
| `~/.config/ssr-client-gtk/settings.json` | 语言偏好 |
| `~/.config/ssr-client-gtk/sysproxy-snapshot.json` | 系统代理快照（异常退出后下次启动自动还原） |

`ssr_service_port` 等旧版遗留键会被读取时忽略、保存时不写回。

Legacy keys such as `ssr_service_port` are ignored on read and never written back.

## 单端口与监听地址 / Single port & listener address

启用期间**本应用只暴露一个 TCP 监听口**：经 SSR 协议链路处理后的 SOCKS5 口
（没有旧版的第二个"服务端口"）。

While enabled the app exposes **exactly one TCP listening port**: the SOCKS5
port produced by the SSR protocol pipeline (the old second "service port" is
gone).

**监听地址固定为 `0.0.0.0`**，即同一局域网内的机器也能直接连上这个**无认证**
SOCKS5 口。若你不需要局域网访问，请用防火墙限制该端口，或只在可信网络中启用。

**The listener is fixed to `0.0.0.0`** — anyone on the same LAN can reach this
**unauthenticated** SOCKS5 port. Firewall it if you don't need LAN access, and
only enable the proxy on networks you trust.

## 常见问题 / FAQ

**端口被占用 / Port already in use**
启用时报「端口被占用」并给出具体端口号：换一个 `listen_port`（1–65535）再启用；
本应用不会自动挑端口，也不会制造第二个监听口。

**界面语言 / UI language**
标题栏右上角下拉即时切换中 / 英，选择会持久化；首次启动按系统
`LC_ALL`/`LANG` 回退，非中文环境默认英文。

**切换系统代理的桌面不支持 / Unsupported desktop**
应用照常启动本地 SOCKS5，只提示你手动把系统代理指向
`127.0.0.1:<listen_port>`，不会报错退出。

**与旧版共存 / Running next to the old client**
互不影响：旧版占 `1080`/`1081` 时，本应用用你配置的其他端口正常工作，
且自身始终只有一个监听口。

## 许可证 / License

GPL-3.0-or-later（与 `ssr-client-rs` 一致）。见 [LICENSE](LICENSE)。

GPL-3.0-or-later, same as `ssr-client-rs`. See [LICENSE](LICENSE).
