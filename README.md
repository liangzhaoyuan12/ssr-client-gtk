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

支持三平台（GOAL §11）/ three platforms:

| 平台 Platform | 要求 Requirement |
| --- | --- |
| **Linux** | GTK ≥ 4.18、libadwaita ≥ 1.5（GNOME 47 / Deepin 25 同期及以上）；系统代理支持 **KDE Plasma** 与 **GNOME 系**（Cinnamon / MATE / Ubuntu / deepin / COSMIC 等），其他桌面提示手动配置 |
| **Windows** | Windows 10+（x86_64）；系统代理写 **HTTP**（Windows 无原生 SOCKS，见下"单端口"一节），首次监听会弹**防火墙授权** |
| **macOS** | macOS 12+（arm64 / x86_64）；系统代理走 `networksetup` 的 **SOCKS**；未公证的 `.app` 首次打开需**右键 → 打开** |

三平台都需要一个可用的 ShadowsocksR 服务端地址 / a working ShadowsocksR server to connect to.

## 安装 / Install

### Linux：四种发行产物，任选其一 / pick one of four artifacts

```bash
# 架构段取自构建机（x86_64 构建 = amd64/x86_64，龙芯 = loong64/loongarch64）：
#   deb 用 dpkg --print-architecture，rpm 用 rpm --eval '%{_arch}'，其余用 uname -m

# Debian / Ubuntu
sudo dpkg -i ssr-client-gtk_0.1.0-1_<deb架构>.deb   # 或 apt install ./<deb>

# Fedora / RPM 系
sudo dnf install ./ssr-client-gtk_0.1.0-1.<rpm架构>.rpm

# Arch Linux
pacman -U ssr-client-gtk_0.1.0-1-<arch>.pkg.tar.zst

# 免安装 tar.gz / portable tarball
tar -xzf ssr-client-gtk-0.1.0-<arch>.tar.gz
./ssr-client-gtk-0.1.0-<arch>/bin/ssr-client-gtk
```

### Windows：绿色 zip（无安装器，决定 D9）

解压到任意目录，双击 `ssr-client-gtk.cmd`（它会把随包的 GTK 运行时、
字体配置与渲染器设好）。首次启动在防火墙授权窗口点「允许」。

Unpack the zip anywhere and start `ssr-client-gtk.cmd` — no installer
(decision D9). Allow the firewall prompt on first launch.

### macOS：`.app`（决定 D11 交付形态）

把 `ssr-client-gtk.app` 拖到「应用程序」，首次打开**右键 → 打开**（未做
Apple 公证）。/ Drag the app into Applications; first launch needs
**right-click → Open** (not notarised).

### 从源码构建 / Build from source

```bash
# Linux
cargo build --release        # 需要系统 gtk4-devel / libadwaita-devel
./target/release/ssr-client-gtk

# macOS（需 brew install gtk4 libadwaita dylibbundler）
./build-mac.sh

# Windows（需 MSYS2 + mingw-w64 的 gtk4/libadwaita/ntldd，见 VERIFY.md）
powershell -ExecutionPolicy Bypass -File .\build-win.ps1
```

三平台的逐条验证命令见 [VERIFY.md](VERIFY.md) /
per-platform verification steps live in `VERIFY.md`.

### 打包四件套 / Build all four packages

```bash
./build.sh            # 构建 deb + rpm + Arch(.pkg.tar.zst) + tar.gz
./build.sh --check    # 先跑门禁（fmt/clippy/test）再构建
```

**打包只直接调用系统工具，绝不 `cargo install`**：deb → `dpkg-deb`
（`Depends` 的 ELF 部分由系统 `dpkg-shlibdeps` 补齐）、rpm → `rpmbuild`
（脚本现写 spec）、Arch → `makepkg`（脚本现写 PKGBUILD，`--nodeps`）、
tar.gz → `tar -czf`；工具缺失直接给安装指引退出（`dpkg` / `rpm` /
`makepkg` / `libarchive-tools` / `tar`）。

版本单一来源 = `Cargo.toml`：脚本读取后用于四个产物的文件名与包内版本断言
（deb `Version` / rpm `%{VERSION}` / Arch `.PKGINFO`），不一致即失败退出；
架构一律从本机系统读取（`dpkg --print-architecture` / `rpm --eval %{_arch}` /
`uname -m` / `makepkg.conf` 的 `CARCH`），不硬编码 x86_64。依赖处理：核验
构建环境 gtk4 ≥ 4.18 / libadwaita ≥ 1.5（与 Cargo features `v4_18`/`v1_5`
一致，运行时下限在 `build.sh` 里只声明一处）、`cargo --locked` 可复现构建。
四件套统一落在 **`packaging/`** 这一处（脚本断言：不出现 `dist/`，仓库别处
不许有包）：

```text
packaging/ssr-client-gtk_<ver>-1_<deb架构>.deb       # loong64 / amd64
packaging/ssr-client-gtk_<ver>-1.<rpm架构>.rpm       # loongarch64 / x86_64
packaging/ssr-client-gtk_<ver>-1-<arch>.pkg.tar.zst
packaging/ssr-client-gtk_<ver>-<arch>.tar.gz
```

## 使用 / Usage

1. 点击「新建配置」，填写服务器地址、端口、密码，选择加密 / 协议 / 混淆
   （下拉选项与 `ssr-client-rs` 的枚举完全一致）。
2. 选中配置后点击「启用代理」：本地起**唯一一个**监听口（SOCKS5 + HTTP
   双协议），并把系统代理指向它（Linux/macOS 写 SOCKS，Windows 写 HTTP）。
3. 「停用代理」精确还原系统代理原值；直接关窗同样会先停代理、再还原系统代理。

1. Create a profile (server, port, password, cipher / protocol / obfs).
2. Select it and hit **Enable**: one listener starts (SOCKS5 + HTTP) and the
   system proxy is pointed at it (SOCKS on Linux/macOS, HTTP on Windows).
3. **Disable** restores the system proxy to its exact previous values — closing
   the window while enabled does the same, in that order.

### 配置目录 / Where data lives

| 路径 Path | 内容 Contents |
| --- | --- |
| `~/.ssr/*.json` | 配置文件（文件名 = 配置名，仅 `^[a-zA-Z]+$`） |
| `~/.config/ssr-client-gtk/settings.json` | 语言偏好 |
| `~/.config/ssr-client-gtk/sysproxy-snapshot.json` | 系统代理快照（异常退出后下次启动自动还原） |

Windows 上 `~` = `%USERPROFILE%`（即 `%USERPROFILE%\.ssr` 与
`%USERPROFILE%\.config\ssr-client-gtk\`）；三平台路径同构，整个目录可以
直接拷走（决定 D12）。/ On Windows `~` is `%USERPROFILE%`; the layout is
identical on all three platforms, so the folders are portable as-is.

`ssr_service_port` 等旧版遗留键会被读取时忽略、保存时不写回。

Legacy keys such as `ssr_service_port` are ignored on read and never written back.

## 单端口与监听地址 / Single port & listener address

启用期间**本应用只暴露一个 TCP 监听口**：经 SSR 协议链路处理后的那个口
（没有旧版的第二个"服务端口"）。同一个口按**首字节嗅探**讲两种协议：

- `0x05` → **SOCKS5**（Linux / macOS 的系统代理走这条）；
- 其余 → **HTTP 代理**（`CONNECT` 与绝对 URI 两种请求行，路由与 SSR
  链路完全复用）——**仅 Windows 需要**：WinINET、Windows 设置页与
  Chromium 系浏览器都不认 SOCKS，系统代理因此指向 `127.0.0.1:<端口>`
  的 HTTP 形态（决定 D10）。

While enabled the app exposes **exactly one TCP listening port**, speaking
both dialects: SOCKS5 (`0x05`) and an HTTP proxy (`CONNECT` / absolute-URI).
Windows needs the HTTP side — its system proxy does not understand SOCKS.

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
应用照常启动本地代理，只提示你手动把系统代理指向
`127.0.0.1:<listen_port>`，不会报错退出。

**Windows 防火墙弹窗 / Windows firewall prompt**
第一次监听时 Windows 会问是否允许本程序联网——点「允许」，否则局域网
机器连不上这个口（本机 `127.0.0.1` 的流量不受影响）。

**macOS 打不开 / macOS says the app can't be opened**
`.app` 只做了 ad-hoc 签名、未做 Apple 公证：**右键 → 打开 → 打开**，
或 `xattr -dr com.apple.quarantine ssr-client-gtk.app`。

**与旧版共存 / Running next to the old client**
互不影响：旧版占 `1080`/`1081` 时，本应用用你配置的其他端口正常工作，
且自身始终只有一个监听口。

## 许可证 / License

GPL-3.0-or-later（与 `ssr-client-rs` 一致）。见 [LICENSE](LICENSE)。

GPL-3.0-or-later, same as `ssr-client-rs`. See [LICENSE](LICENSE).
