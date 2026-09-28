# 三平台验证清单（GOAL §11 Phase 8.9）

每台主机照抄对应段落，把命令输出贴回来即可。**没贴真实输出的项一律不打勾。**
版本单一来源 = `Cargo.toml`；三段里的 `$VER` 就是它。

---

## Linux（Deepin / LoongArch，本机）

```bash
cd /path/to/ssr-client-gtk
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)

# 1) 门禁四连
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --quiet                 # 期望 ≥ 120 passed / 0 failed
cargo doc --no-deps 2>&1 | grep -ci warning   # 期望 0

# 2) 跨平台审计：每一行命中都必须落在 #[cfg(unix)] 里
grep -rn 'libc::\|std::os::unix\|/proc/' src --include='*.rs'

# 3) 代理 e2e（系统为 KDE 或 GNOME；路由用「绕开局域网」让环回直连）
#    选中配置 → 启用 → 下面三条全绿 → 停用 → 还原 byte-identical
ss -tlnp | grep ssr-client-gtk                # 恰好 1 个监听口
curl -sS -x socks5h://127.0.0.1:1080 https://api.ipify.org   # 出口 IP = 服务端
curl -sS -x http://127.0.0.1:1080  http://example.com -o /dev/null -w '%{http_code}\n'  # HTTP 前端同样可用
# 停用后：端口消失、系统代理回基线、~/.config/ssr-client-gtk/sysproxy-snapshot.json 删除

# 4) 四件套（需先补齐 packaging/PKGBUILD —— 见 GOAL 表 C 备注）
./build.sh --check
ls -lh packaging/*.deb packaging/*.rpm packaging/*.pkg.tar.zst packaging/*.tar.gz
```

---

## Windows 10/11（x86_64）

前置（MSYS2 MinGW64 shell 里装一次）：

```bash
pacman -S mingw-w64-x86_64-{gtk4,libadwaita,pkgconf,binutils,ntldd,gcc}
```
外加 Rust（rustup，默认脚本会 `rustup target add x86_64-pc-windows-gnu`），
可选 ImageMagick（只为 exe 图标）。

```powershell
cd C:\path\to\ssr-client-gtk
# 1) 打包（内含 fmt/clippy/test 门禁；-SkipTests 可跳过）
powershell -ExecutionPolicy Bypass -File .\build-win.ps1
# 期望结尾：产物路径 + sha256 + "zip 里有 exe/DLL/launcher" 断言全过

# 2) 单元测试单独跑（若上面用了 -SkipTests）
cargo test --quiet            # 期望 ≥ 120 passed / 0 failed（/proc 相关已按 cfg(unix) 关掉）

# 3) 运行时：解压 zip，双击 ssr-client-gtk.cmd
#    - 窗口出现、中文界面（系统 zh-CN 时）
#    - 首次监听会弹防火墙授权 → 允许
#    - 窗口图标显示不出来 → 记下来：gdk-pixbuf loaders 的 cache 仍指向构建机
#      的 C:\msys64，换机即失效（脚本已带上 lib\gdk-pixbuf-2.0，需再补 cache 重写）

# 4) 系统代理 = HTTP 前端（决定 D10：Windows 无原生 SOCKS）
#    - 「系统代理方式」下拉：桌面行读「桌面设置（系统代理）」，第二行读
#      「环境变量（不支持）」且**置灰不可选**（决定 D14：Windows 只用桌面代理）
#    启用代理后，另开 PowerShell：
curl.exe -sS -x http://127.0.0.1:1080 https://api.ipify.org    # 出口 IP = 服务端
Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' |
    Select-Object ProxyEnable, ProxyServer                      # 1 / 127.0.0.1:1080
# Edge 打开 https://api.ipify.org → 同一个出口 IP（证明 WinINET 认这个代理）
# 停用后：ProxyEnable/ProxyServer/ProxyOverride 精确回原值（缺失的键被删除）、快照文件删除

# 5) 退出清理：启用中 Ctrl+C / 关闭控制台 / 注销 →
#    进程退出 + 代理还原 + 关窗通知 1 条（PowerShell 气泡）

# 6) 单实例：连开两个 ssr-client-gtk.cmd → 第二个把窗口带到前台，不出现第二个进程
```

---

## macOS 12+（arm64 / x86_64）

前置（一次）：

```bash
brew install gtk4 libadwaita dylibbundler
```

```bash
cd /path/to/ssr-client-gtk
SKIP_GATE=0 ./build-mac.sh
# 期望：门禁过 + 产出 packaging/macos/ssr-client-gtk.app + dylib 数量/版本/otool 断言全过

# 1) 运行（未签名/未公证时：右键 → 打开）
open packaging/macos/ssr-client-gtk.app

# 2) 系统代理 = SOCKS（决定 D11：M1 networksetup）
#    若启用时报 "networksetup: command not found" 或权限错误 → 贴回来：
#    Finder 启动的 app PATH 可能没有 /usr/sbin（代码已给子进程补上
#    /usr/sbin:/sbin），真报权限错则说明这台机器要管理员组。
#    启用代理后：
networksetup -getsocksfirewallproxy Wi-Fi     # Enabled: Yes / Server: 127.0.0.1 / Port: 1080
scutil --proxy | grep -E 'SOCKSEnable|SOCKSProxy|SOCKSPort'
curl -sS --socks5-hostname 127.0.0.1:1080 https://api.ipify.org   # 出口 IP = 服务端
curl -sS -x http://127.0.0.1:1080 http://example.com -o /dev/null -w '%{http_code}\n'
# 停用后：每个网络服务的 SOCKS 状态与启用前逐字一致（含原本是关的）

# 3) 单实例 / 中英切换 / 关窗零残留 —— 与 Linux 同一套手工检查
#    已知限制（贴结果时注明是否踩到）：schemas / gdk-pixbuf loaders 仍按
#    Homebrew 编译期前缀解析，在构建这台机器上没问题；挪到没装 Homebrew
#    的机器会缺数据文件（dylib 本身已随包）。
```

---

## 回归底线（每台机器都必须成立）

1. `cargo test` 0 failed（数字可比 120 多，不许少）。
2. 启用期间**监听口恰好 1 个**；停用/关窗后为 0。
3. 系统代理写入前后**byte-identical 还原**（含"原本不存在的键被删掉"）。
4. HTTP 前端（Windows 主用）与 SOCKS5 前端（Linux/macOS 主用）**同一端口同时可用**。
5. 打包产物版本 == `Cargo.toml` 版本。
