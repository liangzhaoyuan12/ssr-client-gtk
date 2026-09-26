# GOAL — ssr-client-gtk：GTK4 + libadwaita 原生重写，交付可直接发布的成品

> **本文件是本次任务的唯一清单**：要做什么、怎么验、做到什么程度算完。
> 每项完成后在原行打勾，并追加一条进度行（见文末）。
> **架构与功能规格来源**：本仓库 `ARCHITECTURE.md`（原 Tauri 实现的工作底稿）。
> **代理核心来源**：`git clone https://cnb.cool/liangzhaoyuan12/ssr-client-rs`（crate `ssr-client-rs`）。
> **文档基线**：2026-09-25 实测环境快照（见 §2.3），所有版本号、端口、命令均为实测/实查，不凭记忆。

---

## 0. 一句话目标

把 `ARCHITECTURE.md` 描述的 Tauri + WebView + sidecar 应用，用 **GTK 4.18 + libadwaita 1.7**（gtk4-rs 0.11 / libadwaita-rs 0.9 对应 feature）重写为**纯 Rust 原生桌面应用**，代理转发不再依赖外部 `ssr-native-client` sidecar，改为**进程内链接 `ssr-client-rs` crate**；对外**只暴露一个端口**（经 SSR 协议链路处理后的那个）；**中英双语**；最终产出经过完整测试、**可直接打包发布（Arch + deb + rpm + tar.gz）**的 Linux 桌面软件。

---

## 1. 反幻觉规则（适用全程）

1. **先读后写**：改任何文件前先 `read_file`，不凭记忆改；引用 `文件:行号` 必须是当前真实内容。
2. **先验后报**：没有真实命令输出（测试数、`pkg-config --modversion`、包内文件清单、`curl` 走通 SOCKS5 的响应、`ss -tlnp` 的端口断言）不得勾选任何一项，不得写"应该可以"。
3. **不重复已完成项**：勾掉的项不重做；继续前先看文末进度行。
4. **克制范围**：只做清单上的事，不顺手重构无关代码；前提不成立的项改写为"核账项"并附证据。
5. **依赖不臆造**：新增任何 crate 前先确认它存在且版本可解析（`cargo add --dry-run` 或查 crates.io），不写没验证过的 API。
6. **每批完成跑门禁**：`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` 全绿才算本批完成。

---

## 2. 输入、现状与依据

### 2.1 `ARCHITECTURE.md` 的角色（重要）

它是**旧 Tauri 实现**的规格底稿：UI 布局、表单字段、数据流、系统代理策略、已知问题（§8 P0/P1/P2）、验收清单（§11）**全部沿用为本项目的需求来源**；但其中所有 **Tauri / Vue / Vite / npm / WebView 相关内容一律作废**，由 GTK 实现替代。

对应关系（旧 → 新，做完后逐行核对）：

| ARCHITECTURE.md 中的旧实现 | 本项目替代方案 |
|---|---|
| Tauri 2 Builder + `#[tauri::command]` ×6 | 普通 Rust 函数调用（UI → `AppState` → 模块），**无 IPC 层、无 JSON 信封** |
| `invoke()` + 双层 JSON 字符串封装（§5.3、§8-7） | 直接 `Result<T, AppError>`，错误类型化、UI 显示本地化文案 |
| sidecar `ssr-native-client` + `GLOBAL_PROC` 单例 | 进程内 `ssr_client_rs::SsrClient`（`new/start/stop/is_running`） |
| **双端口**：应用自身占一个服务端口 + sidecar 占 `ssr_service_port`（§2.3 实测） | **单端口**：只有 `SsrClient` 的那一个监听口（见 §6 不变量 N1） |
| `proxy.rs` gsettings/kwriteconfig5 | `sysproxy/` 策略表驱动 + **启用前快照 / 停用还原** + 错误上抛 |
| `notification.rs` 的 `notify-send` + `.unwrap()` | 应用内 `AdwToast`（必须）+ 桌面通知（可选，缺依赖时降级不 panic） |
| `single-instance` 插件 | `AdwApplication` 的 `application_id`（天然单实例） |
| `window-state` 插件 | Adwaita 窗口默认行为；尺寸记忆为可选项 |
| Vue 3 + **7 个 locale 词条文件**（JSON/JS） | **Rust struct 词条**（`&'static str` 字段，编译期保证完整性），**仅中英双语** |
| `localStorage` 存语言偏好 | `~/.config/ssr-client-gtk/settings.json`（**只存偏好，不含词条**） |
| `prefers-color-scheme` 深色模式 | libadwaita `StyleManager`（跟随系统，自动） |
| `package.json`/`tauri.conf.json` 版本不一致（§8-15） | **版本号单一来源 = `Cargo.toml`** |

> **为什么词条用 struct 不用 JSON 文件**：你的库是 struct 传参风格，词条同样用 struct 才统一；且 struct 缺字段**直接编译失败**，天然消灭旧版"多语言键不一致"这类问题（ARCHITECTURE §5.6 靠人工对齐、无任何检查）。
> **磁盘上的配置文件仍然是 ssr-n JSON** —— 那是 `ssr-client-rs` 的外部契约，与词条是两回事，不受本条影响。

### 2.2 `ssr-client-rs` 提供什么（已实读源码确认）

- 依赖写法：`ssr-client-rs = { git = "https://cnb.cool/liangzhaoyuan12/ssr-client-rs" }`
- 核心 API（`src/lib.rs` / `src/local/mod.rs`）：
  - `SsrClient::new(SsrClientConfig)` → `.start().await`（起本地 SOCKS5 + UDP relay，阻塞到 `stop()`）、`.stop()`、`.is_running() -> bool`、`.config()`
  - `config_json::config_from_json(&str) -> SsrResult<SsrClientConfig>`：**同时支持扁平结构和旧版嵌套 `client_settings` 结构**（`src/config_json.rs:1-8`），因此 `~/.ssr/*.json` 老配置可直接加载
  - 端口占用返回 `SsrError::Connection("Failed to bind SOCKS5 server on ...")`，可直接转成 UI 可读错误
- **端口形态**：`start()` 只 bind **一个** TCP 监听口（`listen_address:listen_port`）；`udp = true` 时在同一**端口号**上再 bind UDP（同一个端口号，不是第二个端口）→ "只暴露一个端口号"由库天然保证，**应用层不许再加任何监听**
- 配置字段与 `ARCHITECTURE.md` §5.4/§6.3 对应（server/server_port/listen_address/listen_port/password/method/protocol/protocol_param/obfs/obfs_param/udp/idle_timeout/connect_timeout/udp_timeout）；**没有 `ssr_service_port` 字段**
- 该 crate `warnings = "deny"`、MSRV 1.82、GPL-3.0-or-later → **本项目许可证必须 GPL-3.0-or-later**（见 §9）
- 它**只做协议转发**：配置 CRUD、系统代理、UI、通知全部由本项目自己实现（对应 ARCHITECTURE §1 的定位，只是 sidecar 变成库）

### 2.3 本机环境实测快照（2026-09-25，后续验证命令基于此）

```
OS / 桌面     Fedora 44, XDG_CURRENT_DESKTOP=KDE（系统代理必须走 KDE 路径）
Rust          rustc 1.97.1 / cargo 1.97.1（rustup，~/.cargo/bin）
GTK 运行时    gtk4 4.22.4、libadwaita 1.9.2（rpm 已装，**向后兼容 4.18/1.7 目标**）
GTK 开发头文件 未安装：pkg-config 查不到 gtk4 / libadwaita-1（只有 gtk+-3.0）→ Phase 0 必须先装
打包/校验工具  dpkg-deb ✓  rpmbuild ✓  rpm ✓  tar ✓  zstd ✓  podman ✓  appstreamcli ✓
              makepkg ✓（pacman 7.0.0，/usr/bin/makepkg，2026-09-25 已装）  cargo-deb / cargo-generate-rpm 未装
旧应用实测    /usr/bin/shadowsocksr-client-linux（pid 5455）自身监听 127.0.0.1:1080 ← 服务端口
              /usr/bin/ssr-native-client -c ~/.ssr/hk.tmp.json -d（pid 5771）监听 0.0.0.0:1081
              hk.tmp.json = hk.json 的临时副本，listen_port 被改写成 ssr_service_port(1081)
              → 实证旧版是"双端口"：1080 服务口（会占用冲突）+ 1081 协议处理后的口
占用端口      1080 = shadowsocksr-client-linux、1081 = ssr-native-client（测试必须避开）
配置目录      ~/.ssr/ 存在：hk.json（嵌套 client_settings + 多余键 ssr_service_port=1081）、hk.tmp.json（残留）
本地参考      /opt/ssr/{ssr-server, ssr-client, config.json, ssr-enable.sh, ssr-disable.sh}（可用于真实 e2e）
```

**由快照直接得出的硬约束（写进代码与测试）：**

- **最终只暴露一个端口**（见 §6 N1）：旧行为的"应用服务口 + sidecar 口"两套监听不复存在。但**测试与默认配置仍不得假定 1080 空闲**——单测一律绑 `127.0.0.1:0`（内核分配）或临时端口，e2e 手动测试用**当时空闲的端口**（先 `ss -tln` 查，当前 1080/1081 已被旧程序占用）。（`ssr-client-rs` 自己的 `test_ssr_client_start_and_stop` 就是因为 1080 被本机 `shadowsocksr-cl` 占用而失败的现成教训。）
- 解析 `~/.ssr/*.json` 必须**容忍未知键**：老配置的 `ssr_service_port` **读到忽略、不写回、不在 UI 出现**；`hk.tmp.json` 这类残留不当成配置、不导致崩溃。
- 系统代理写入优先实现并验证 **KDE 路径**（`kwriteconfig5` + `org.kde.KIO.Scheduler.reparseSlaveConfiguration`，参考 `/opt/ssr/ssr-enable.sh`），GNOME 路径同批实现但本机只能做代码级验证。

---

## 3. 技术选型（锁定，不中途换）

| 项 | 选择 | 版本 / feature | 依据 |
|---|---|---|---|
| GUI | `gtk4` crate | `0.11` + `features = ["v4_18"]` | 对应 GTK 4.18（你指定的目标版本） |
| 设计层 | `libadwaita` crate | `0.9` + `features = ["v1_7", "gtk_v4_18"]` | libadwaita 1.7 与 GTK 4.18 精确配对（0.9.2 的 `v1_7`/`gtk_v4_18` feature 已实查存在；两者依赖同源 `gtk4 ^0.11`） |
| 绑定基线 | `glib/gio/gdk4` | `0.22` / `0.11`（由上两者带动） | gtk-rs 同一发布列车 |
| Rust | edition 2024（`Cargo.toml` 已是） | `rust-version = "1.92"` | `gtk4 0.11.5` 声明的 MSRV，本机 1.97.1 满足 |
| 代理核心 | `ssr-client-rs` | git 依赖，锁 `Cargo.lock` | 见 §2.2 |
| 异步运行时 | `tokio`（`rt-multi-thread`） | 独立线程持有 runtime | 库内部就是 tokio；GTK 主循环不能被阻塞 |
| 线程回传 | `glib` 的 `MainContext` + `async-channel` / `glib::idle_add_once` | — | 后台状态变化回到 GTK 主线程更新 UI |
| 配置序列化 | `serde` + `serde_json` | — | **磁盘**保持 ssr-n JSON schema（外部契约，见 §6.1） |
| 国际化 | **Rust struct 词条**：`struct Strings { … &'static str }`，`ZH` / `EN` 两个 const 实例，运行时按当前语言取 | 语言仅 `zh-CN` / `en-US` | 与库的 struct 传参风格一致；**字段全集编译期校验**（少一个字段=编译失败），无需 JSON 词条、无需键一致性测试 |
| 名称校验 | `regex` 或手写字母判断 | — | `cfg_name` 校验必须落在 Rust 侧（信任边界） |
| UI 描述 | GtkBuilder `.ui` XML，`include_str!` 内嵌 | **不用 blueprint**（本机未装 `blueprint-compiler`） | 无需额外编译步骤；`gtk4` 的 `blueprint` feature 不启用 |
| 桌面通知 | `AdwToast` 必须 + 可选 `notify-rust` | 后者缺失时只发 Toast，绝不 `unwrap` | 修 ARCHITECTURE §8-14 |
| 打包 | **Arch（PKGBUILD）+ deb + rpm + tar.gz** 四件套 | `cargo-deb` / `cargo-generate-rpm`（Phase 6 安装）；Arch 用**本机 `makepkg`**（pacman 7.0.0 已装）；tar.gz 用 `tar -czf` | 你指定的发布形态；本机工具可用性实测见 §2.3 |
| 许可证 | GPL-3.0-or-later | `LICENSE` 文件必须落盘 | `ssr-client-rs` 是 GPL-3.0-or-later，分发必须兼容 |

**运行时最低要求（写进 README 与打包依赖）**：GTK ≥ 4.18、libadwaita ≥ 1.7（即 GNOME 48 / Fedora 42+ 同期及以上）。本机 4.22/1.9 向下兼容，可直接运行。

---

## 4. 产品范围

### 4.1 必做功能（逐条对应 ARCHITECTURE.md）

**布局（对应 §5.1）**
- [x] 主窗口：`AdwOverlaySplitView` —— 左侧配置列表（sidebar），右侧内容区；`AdwHeaderBar` 标题 + 语言下拉（`GtkDropDown`，**中/英两项**）；底部状态栏（本地代理端口、代理状态点）
- [x] 三视图切换：配置列表（默认）/ 配置表单（新建、编辑）/ 仪表盘（代理控制 + 使用步骤 + 本地代理地址卡片）
- [x] 空态（无配置）、加载态、错误态、成功提示（`AdwToast` 或 banner）四态齐全 —— 已实现：空态两文案（无配置/未选中）、启用/停用 busy（spinner+过程文案）、`AdwToast` 成功与错误；截图核验
- [x] 深色/浅色跟随系统（libadwaita `StyleManager`），全应用不出现硬编码颜色（修 §8-20） —— `grep -rE '#[0-9a-f]{3,8}|rgba?\(' ui/ src/` → 0 命中；深浅色由 libadwaita `StyleManager` 默认跟随

**配置管理（对应 §6.2 前四条 + §5.4）**
- [x] 列表：扫描 `~/.ssr/*.json`（root 时 `/root/.ssr`，沿用 `get_path.rs` 语义），只认 `*.json`，**跳过 `*.tmp.json` 等残留**
- [x] 新建/编辑：表单字段与 §5.4 完全一致（method 9 选 1、protocol 7 选 1、obfs 5 选 1、udp 开关、三个 timeout、server/server_port/listen_port）；**不含 `ssr_service_port`** —— 核账：下拉为**库枚举全集**（28/14/6）而非旧前端写死的 9/7/5 —— 按 GOAL 4.2“枚举取自 `ssr-client-rs` 导出、不手写字符串表”执行；9/7/5 白名单在不可得的旧仓库里，如需收窄请纠正本条
- [x] 校验：`cfg_name` 仅 `^[a-zA-Z]+$`、`server`/`password` 非空、端口 1–65535；**校验在 Rust 侧权威执行**，UI 校验只是第一道（修 §8-1 路径穿越）
- [x] 删除：确认对话框（`AdwAlertDialog`，替换旧版原生 `confirm()`）→ 删文件 → 列表刷新 —— 代码在位（`AdwAlertDialog` 确认→删→刷新）；实际点击回归归 6.8
- [x] 编辑时 `cfg_name` 不可改（旧文件名保持）
- [x] 解析容错：单个文件损坏不影响列表，行内显示错误而非整体失败（修 §8-19） —— 实现：`Store::scan()` 逐文件容错，坏文件行内红字显示“配置文件格式错误”（tooltip 全文），不可选中、点击 toast 错误、仍可删除；单测 `scan_reports_corrupt_files_per_row_without_failing_the_list` + 真实 `broken.json` 截图核验
- [x] 老配置里的 `ssr_service_port` **读到忽略、保存后不写回、任何界面不显示**

**代理控制（对应 §9 B/C，核心变更）**
- [x] 启用：校验配置存在 → 检查端口未被占用（占用给明确可读错误，含端口号与建议）→ 进程内起 `SsrClient` → 写系统代理 → Toast 通知
- [x] 停用：停 `SsrClient` → 还原系统代理 → Toast
- [x] **单端口断言**：启用期间 `ss -tlnp`（或 `/proc/net/tcp`）里本进程**只出现这一个端口号**（TCP；`udp=true` 时 UDP 同号），无第二个监听口（见 §6 N1）
- [x] **状态对账**：新增 `ProxyService::status()`（真实 `is_running()`），启动时/窗口重新聚焦时刷新 UI，崩溃或外部退出后 UI 不再显示"已连接"（修 §8-3） —— `status()` 对账有单测 `status_reconciles_dead_core`；窗口 `is-active` 聚焦时 `update_status()` 重取真实状态
- [x] 重复启用 → 明确错误，不重复起实例（保持 §7-4 单实例语义）
- [x] 端口展示随当前配置变化，显示两行且**只出现这一个端口号**：`实际监听 0.0.0.0:<listen_port>` / `系统代理 → 127.0.0.1:<listen_port>`

**系统代理（对应 §6.6）**
- [x] 策略表驱动（环境 → 策略），GNOME 系 + KDE Plasma 两条实现，其余环境降级为"仅提示手动配置"的 Toast（修 §8-10/11）
- [x] **启用前快照原值，停用时精确还原**（修 §8-9，不许再 `ProxyType=0` 一把梭）
- [x] 写入失败必须作为 `Err` 上抛到 UI（修 §8-4，禁止吞错误返回 success）
- [x] 系统代理指向**唯一端口**（`127.0.0.1:<listen_port>`），不出现旧版的双端口指向

**生命周期（对应 §6.8、§7-10）**
- [x] 关闭窗口 → 停代理 → 还原系统代理 → 退出（顺序固定，且"未启用时关窗"不得动系统代理）
- [x] 单实例：第二次启动把已有窗口带到前台（`application_id = com.liangzhaoyuan12.ssr-client-gtk`） —— 二次启动 `pgrep` 单 pid 不变，re-activate 走已有窗口 `present()`

**国际化（原 §5.6 的 struct 版）**
- [x] `src/i18n.rs`：`struct Strings` 全部词条字段（文案迁自旧仓库 `src/locales/zh-CN.js` 与 `en-US.js`），`ZH` / `EN` 两个 const 实例
- [x] 只有 `zh-CN`、`en-US` 两种语言；取词直取 struct 字段，**改词条 = 改字段，缺字段编译失败**
- [x] 语言选择持久化到 `~/.config/ssr-client-gtk/settings.json`，首启按 `LC_ALL/LANG` 回退 `en-US`
- [x] 测试：两语言字段非空、无占位符、无漏翻（单测遍历断言）

### 4.2 明确不做（范围外）

- 不实现任何 SSR 协议/加密逻辑（`ssr-client-rs` 负责，且禁止改其源码）
- 不做 Tauri/WebView/npm/Vite 任何残留，**不做 JSON/JS 词条文件**
- **不做第二个监听端口**（不复刻旧版"服务端口"，不读写 `ssr_service_port`）
- 不做 zh-TW / zh-HK / ja-JP / ko-KR / ru-RU（语言只留中英）
- 不做托盘图标、开机自启、流量统计、配置导入导出
- 不执行 `cargo publish` / `git tag` / `git push`（发布动作由你本人做）
- 不动 `~/.ssr` 里已有配置的内容（只读写自己新增/编辑的）

---

## 5. 目标架构

```
ssr-client-gtk/
├── Cargo.toml              # 唯一版本号来源；[package.metadata.deb] / [.rpm]
├── LICENSE                 # GPL-3.0-or-later
├── GOAL.md                 # 本文件
├── ARCHITECTURE.md         # 只读规格底稿（不改）
├── README.md               # 中英双语用户文档（安装/使用/系统要求）
├── packaging/
│   ├── PKGBUILD            # Arch Linux（本机 makepkg 直接构建）
│   └── tarball.sh          # tar.gz 打包脚本（bin + desktop + metainfo + LICENSE + README）
├── data/
│   ├── com.liangzhaoyuan12.ssr-client-gtk.desktop
│   ├── com.liangzhaoyuan12.ssr-client-gtk.metainfo.xml   # AppStream
│   └── icons/ ...
├── ui/                     # GtkBuilder .ui XML（include_str! 内嵌，无 gresource 编译步骤）
│   ├── window.ui  config_list.ui  config_form.ui  dashboard.ui
└── src/
    ├── main.rs             # AdwApplication + 单实例 + 启动 tokio runtime 线程
    ├── app.rs              # AppState：全局状态容器（配置列表、当前选中、代理状态、语言）
    ├── i18n.rs             # struct Strings { … } + ZH / EN const + 当前语言句柄
    ├── ui/                 # 仅 GTK 调用与信号连接，不含业务逻辑
    │   ├── window.rs  list.rs  form.rs  dashboard.rs  toast.rs
    ├── core/
    │   └── proxy.rs        # ProxyService：持有 SsrClient + 状态机 + 事件回推 UI（全应用唯一监听口）
    ├── config/
    │   ├── model.rs        # serde 结构（磁盘 schema 同 ARCHITECTURE §6.3，无 ssr_service_port）
    │   ├── store.rs        # ~/.ssr CRUD + cfg_name 校验（信任边界在此）
    │   └── path.rs         # 目录定位（root/普通用户）
    ├── sysproxy/
    │   ├── mod.rs          # 环境 → 策略表；enable/disable 返回 Result
    │   ├── kde.rs  gnome.rs
    │   └── snapshot.rs     # 原值快照与还原
    └── notify.rs           # AdwToast 优先；桌面通知可选、失败不 panic
tests/                      # 集成测试（临时目录 + 随机端口）
```

**线程模型（必须写清，最容易踩坑）**

```
GTK 主线程（GLib 主循环）── 拥有所有控件与 AppState
        │  async-channel / glib::idle_add_once
        ▼
tokio runtime 线程（多线程）── 持有 SsrClient、执行 start()/stop()、系统代理命令
        │  状态变化（含库内部意外退出）经 channel 回推
        ▼
GTK 主线程更新 UI（状态点、Toast）
```

- **任何 GTK API 调用只在主线程**；任何 `.await` / 阻塞系统命令只在后台。
- UI → 后端通过 `glib::spawn_future_local` 或 channel 发消息，禁止在信号回调里直接 `block_on`。
- **应用层不得创建任何额外监听 socket**：`TcpListener::bind` 只允许出现在 `ProxyService` 的端口预检里，且预检后立即释放。

---

## 6. 必须保持的不变量（来自 ARCHITECTURE §7，含本项目修订）

**N1（最高优先级）单端口**：应用对外只暴露**一个端口号**，即 `SsrClient` 的 `listen_port` —— 经 SSR 协议/加密/混淆链路处理后的那个 SOCKS5 口。不复刻旧版的第二个"服务端口"（实测旧版：应用自身占 `127.0.0.1:1080`、sidecar 占 `0.0.0.0:1081`）。**系统代理、UI 展示、README、打包说明里都只出现这一个端口**；`ss -tlnp` 断言是验收项（§9-6）。

1. `~/.ssr/<name>.json` 磁盘 schema 不变，`ssr-client-rs` 必须能读（写盘用 serde 结构体，字段名与 §6.3 一致）；解析容忍未知键，老配置的 `ssr_service_port` 读到忽略、不写回。
2. `cfg_name` 仅 `^[a-zA-Z]+$`，**校验权威在 Rust `config::store`**，UI 校验不是安全边界。
3. **`listen_address` 必须写死 `0.0.0.0`**（沿用 `note.md` 老约束，UI 不提供修改项）。已知代价：SOCKS5 无认证且监听全接口，局域网可达——按你的硬约束保留，README 必须写明这一点。
4. 同一时刻至多一个 `SsrClient` 实例（`ProxyService` 内部单例 + `is_running()` 对账）。
5. 端口被占用必须是**可读错误**（含端口号与排查建议），不得静默成功、不得 panic。
6. 系统代理：启用前快照 → 写入 → 停用还原；**任何一步失败都 `Err` 上抛到 UI**。
7. 退出顺序：停代理 → 还原系统代理 → 退出；未启用时退出不动系统代理。
8. **语言仅 `zh-CN` / `en-US`，词条是 Rust struct**：字段全集由编译器保证，禁止再引入 JSON/JS 词条文件。
9. 版本号单一来源 `Cargo.toml`，Arch/deb/rpm/tar.gz/desktop/metainfo 全部从它取。
10. 许可证 GPL-3.0-or-later，发布包内含 LICENSE 与第三方声明（`ssr-client-rs`）。
11. **测试不绑固定端口**（用 `:0` 或临时端口），不依赖 1080/1081 空闲，不依赖图形会话（纯逻辑测试必须在无 DISPLAY 下可跑）。
12. 二进制名/`application_id`/desktop 文件名三处一致（对应旧 §7-5 sidecar 命名一致性教训）。

---

## 7. 实施阶段（每项自带验证命令；命令输出真实才算完成）

### Phase 0 — 环境与骨架

- [x] 0.1（核账项，原方案前提不成立→改写）原计划 `sudo dnf install gtk4-devel libadwaita-devel`，但本机 `sudo -n` 需密码、无法非交互执行（GOAL §7 规则 4：改写为核账项）。实际做法：**用户态 sysroot**——`dnf download --resolve --destdir` 取 Fedora devel RPM → `rpm2cpio` 解压到 `~/.local/gtk-sysroot` → 改写其中 `.pc` 的 `prefix=` 指向该前缀（`~/.local/gtk-sysroot/env.sh` 固化 `PKG_CONFIG_PATH` 并 `unset PKG_CONFIG_SYSROOT_DIR`），构建前 `source` 它。运行时库仍用系统已装的 gtk4 4.22 / libadwaita 1.9，链接与运行不受影响
      验证：`source ~/.local/gtk-sysroot/env.sh && pkg-config --modversion gtk4 libadwaita-1` → `4.22.5` / `1.9.4`（均 ≥ 4.18 / ≥ 1.7 ✓）；`test -f .../gtk-4.0/gtk/gtk.h && .../libadwaita-1/adwaita.h` → 存在
- [x] 0.2 `Cargo.toml` 落依赖（§3 表格全部），`ssr-client-rs` 用 git 依赖
      验证：`cargo check` Finished rc=0；`cargo tree -i gtk4` → `gtk4 v0.11.5`；`cargo tree -i libadwaita` → `libadwaita v0.9.2`；`cargo tree -i ssr-client-rs` → `ssr-client-rs v0.1.0 (https://cnb.cool/liangzhaoyuan12/ssr-client-rs#415c0531)`
- [x] 0.3 空窗口骨架：`AdwApplication`（`application_id` 单实例）+ `AdwApplicationWindow`（800×600，与旧版一致）+ `AdwToolbarView`/`AdwHeaderBar`（`src/app.rs` 占位布局，Phase 4 替换为 OverlaySplitView）
      验证：`cargo build` rc=0；实跑 `./target/debug/ssr-client-gtk`（KDE Wayland 会话）→ `pgrep -x ssr-client-gtk | wc -l` = 1；**第二次启动后仍 = 1**（单实例生效）；`spectacle -b -n -o` 截图确认窗口标题 "ShadowsocksR Client"、HeaderBar 存在、正文两行文案正确（`~/.hermes/cache/scratch/phase0-window2.png`，vision 核对）；`pkill -x` 后计数 0，应用日志无任何报错输出
- [x] 0.4 `LICENSE`（GPL-3.0-or-later）落盘（复制自 `ssr-client-rs/LICENSE`，Cargo.toml `license = "GPL-3.0-or-later"`）
      验证：`head -1 LICENSE` → `GNU GENERAL PUBLIC LICENSE`

### Phase 1 — 配置存储（纯逻辑，先测后 UI）

- [x] 1.1 `config/{path,model,store}.rs`：路径定位（root→`/root/.ssr`，否则 `$HOME/.ssr`，euid 判定走 `/proc/self` 元数据、不引 libc）、serde 结构（**无 `ssr_service_port` 字段**）、list/load/save/create/delete（原子写：`.part` 临时文件 + rename）
- [x] 1.2 `cfg_name` 校验在 store 层（`Store::validate_name` = `^[a-zA-Z]+$`，`file_for` 先校验再拼路径 → 拒绝 `/`、`..`、数字、符号、空串、NUL）
- [x] 1.3 容错：列表只认 `*.json` **且 stem 通过校验**（`hk.tmp.json` 被跳过）；损坏文件照常进列表、错误延迟到 `load` 单行报出
- [x] 1.4 单元测试（`tempfile::tempdir` 隔离）：CRUD 往返、非法名拒绝（`../etc/passwd`、`a/b`、`123`、`hk.tmp` 等 11 例）、未知键容忍（`hk.json` 结构含 `ssr_service_port` 的 fixture）、空/未建目录、重复 `create` 冲突而 `save` 覆盖、删除不存在 → `Err(NotFound)`、`listen_address` 保存时强制 `0.0.0.0`
      验证：`cargo test` → **10 passed / 0 failed**
- [x] 1.5 兼容性测试：legacy `hk.json` fixture 同时被本项目 serde 与 `ssr_client_rs::config_json::config_from_json` 解析成功；本项目写出的 JSON 也被 `config_from_json` 解析成功（证明 schema 与核心库互通，GOAL §6.1）
      验证：`unknown_keys_are_ignored_and_not_written_back`、`written_files_parse_with_ssr_client_rs` 两个用例通过（在上述 10 个之内）

### Phase 2 — 代理核心（ProxyService，全应用唯一监听口）

- [x] 2.1 tokio runtime 独立线程 + `ProxyService`（enable/disable/status），状态经 channel 回推（`core/proxy.rs`：runtime 持有于服务内、`start()` 走 `runtime.spawn`、事件 `async_channel<ProxyEvent>`：`Started{cfg_name,port}` / `Stopped{reason}`）
- [x] 2.2 启用前置检查：配置存在、端口可用（`probe_port` bind 后立即释放，最多重试 600ms；占用 → `AppError::PortInUse(port)`；库侧 bind/配置错误经完成 channel 转成 `AppError::Core(真实原因)`）
- [x] 2.3 意外退出处理：`start()` 返回 `Err` / 任务结束 → 发 `Stopped{reason}` 事件，`status()` 对账清理僵尸项（`status_reconciles_dead_core` 用例）；`disable()` 在 `is_running()==false` **且 probe bind 成功**后才返回（保证端口真释放）
- [x] 2.4 集成测试：随机端口起 `SsrClient` → SOCKS5 方法协商收到 `[05 00]` → `stop()` → 端口可再次绑定；端口冲突 → `Err(PortInUse)` 含端口号；重复 enable → `AlreadyRunning`；enable 失败不留监听
- [x] 2.5 **单端口断言测试**：`exactly_one_tcp_listener_while_running` 用 `/proc/self/fd` socket inode ∩ `/proc/net/tcp{,6}`（state `0A`）统计本进程 TCP 监听口：enable 期间 == 起始集合 + 恰好 1 个，disable 后回到起始集合
      验证：`cargo test` → **29 passed / 0 failed / 1 ignored**（config 10 + core 7 + sysproxy 10 + notify 2，ignored 为 KDE 真机冒烟）；测试全程不碰 1080/1081；跑完 `ss -tln | grep ssr-client-gtk` → 无（仅旧程序的 1080/1081 与系统服务仍在，实测输出见进度行）

### Phase 3 — 系统代理与通知

- [x] 3.1 `sysproxy/`：环境策略表（`Desktop::{Kde,Gnome,Unsupported}`，`detect_desktop()` 读 `XDG_CURRENT_DESKTOP`/`DESKTOP_SESSION`；GNOME 系含 Cinnamon/MATE/Ubuntu/deepin/uos/COSMIC 等），enable/disable 返回 `Result`
- [x] 3.2 快照与还原：KDE 快照 `ProxyType/socksProxy/NoProxyFor`、GNOME 快照 `mode/host/port`（原值缺失记 `None` → 还原时 `--delete`/`reset` 而非写空），disable 精确还原；快照持久化到 `~/.config/ssr-client-gtk/sysproxy-snapshot.json`（崩溃自愈，供 Phase 4 启动时恢复）
- [x] 3.3 单元测试：可注入 `Runner` + `Mock`（按完整命令行回放、记录全部调用），断言 `kwriteconfig5`/`gsettings` 命令行与还原顺序（KDE 末步 dbus 广播、GNOME `mode` 最后写）；**KDE 真实命令冒烟**：`kde_real_command_smoke`（`#[ignore]`，显式跑）用真实 `kwriteconfig5` 写入 → 真实 `kreadconfig5` 读回 `socksProxy == 127.0.0.1 12345` == `listen_port` → 还原原值 → 清理；**写的是 `kioslaverc-ssr-client-gtk-test` 一次性文件，真实 `kioslaverc` 与运行中的旧程序系统代理未被触碰**
- [x] 3.4 `notify.rs`：Toast 必达，桌面通知可选且失败不 panic —— 已落盘：`desktop_notify()` 走 `notify-send` 子进程、缺二进制/无会话返回 `false` 绝不 panic（2 用例）；**AdwToast 已随 Phase 4 UI 接线**（`ui/toast.rs` `show_toast` 必达通道，错误/成功提示走它）
      验证：`cargo test sysproxy::` → **10 passed**；`cargo test kde_real -- --ignored` → **1 passed**；`cargo fmt --check` → 绿；真机 gsettings 冒烟输出见进度行；`cargo clippy --all-targets -- -D warnings` 当前仅剩"未接线即未使用"的 dead-code 告警（sysproxy/notify 尚无生产调用方，Phase 4 接线后复跑并要求归零）

### Phase 4 — UI 与国际化

- [x] 4.1 `ui/*.ui` 四个界面 + 信号连接（列表选择/新建/编辑/删除/启用/停用/语言切换）—— 四视图均实跑截图核验（zh 空态/zh 仪表盘/zh 表单/英文全量）；单实例二次启动 pid 不增；坑：`AdwApplicationWindow` 必须用 `content` 属性，写 `child` 直接 SIGABRT（已修）；表单「取消/保存」已移出滚动区固定底部
- [x] 4.2 表单下拉数据源：method/protocol/obfs 枚举取自 `ssr-client-rs` 导出的 `CipherType/ProtocolType/ObfsType`（**不手写字符串表**，保证与库一致）
- [x] 4.3 `i18n.rs`：`struct Strings` + `ZH`/`EN`，文案迁自旧仓库 `src/locales/zh-CN.js` 与 `en-US.js`
- [x] 4.4 语言持久化与回退；切换即时生效（全 UI 刷新）
- [x] 4.5 单测：两语言字段非空、无占位符；人为删掉一个字段验证**编译失败**后还原
- [x] 4.6 四态齐全（空/加载/错误/成功），错误文案全部来自 `AppError` → 词条映射，不显示原始 JSON/英文堆栈（修 §8-2）
- [x] 4.7 端口展示唯一：UI 中所有出现端口的地方都指向 `listen_port`
      验证（实测）：`cargo fmt --check` → 绿；`cargo clippy --all-targets -- -D warnings` → 0 告警；`cargo test` → **38 passed / 0 failed / 1 ignored**；四视图截图 + vision 复核（zh 空态列表有 `hk` 时显示"请先选择一个配置"、zh 仪表盘端口行 `实际监听 0.0.0.0:1080` / `系统代理 → 127.0.0.1:1080`、zh 表单固定按钮、en 切换后窗口内零中文）；语言切换走真实 `connect_notify` 信号：`SSR_GTK_DEV_LANG=en` 启动 → `settings.json` 写入 `{"language": "en-US"}` → 无 env 重启仍全英文（持久化加载）；首启无 settings.json + `LC_ALL=en_US.UTF-8` → 英文（locale 回退）；4.5 编译失败实验：删 `ZH` 的 `lang_label` 行 → `error[E0063]: missing field \`lang_label\` in initializer of \`Strings\`` → 还原后 `cargo check` 绿；§8 逐条点击回归归入 6.8（Wayland 无合成输入工具，静态交互靠 QA 钩子截图覆盖）

### Phase 5 — 端到端联调（真实 SSR 服务器）

- [x] 5.1 起本地服务端：用 `/opt/ssr/ssr-server` + `/opt/ssr/config.json` 的参数（`aes-256-cfb` / `auth_aes128_sha1` / `tls1.2_ticket_auth`，服务端监听 2800）；若端口/权限受限则改为指向你现有的远端 `hk` 节点（`206.237.10.116:2800`） —— 核账：`/opt/ssr/config.json` 实为**客户端格式**（`server` 指向远端），故按其参数+口令新造服务端配置（`server=127.0.0.1` 回环、`server_port=2800`、同 method/protocol/obfs），实跑 `/opt/ssr/ssr-server -c …` 成功监听
- [x] 5.2 新建配置（`listen_port` 用 `ss -tln` 查到的空闲端口，**避开 1080/1081**）→ 启用 → 断言：
      - `ss -tlnp | grep <port>` 有监听，**且本进程监听口总数 == 1**（无第二个口）
      - `curl --socks5-hostname 127.0.0.1:<port> -sS -o /dev/null -w '%{http_code}' http://www.gstatic.com/generate_204` → `204`
      - KDE：`kreadconfig5 --file kioslaverc --group "Proxy Settings" --key ProxyType` → `1`，`--key socksProxy` → `127.0.0.1 <port>`
- [x] 5.3 停用 → 监听消失、系统代理还原为启用前的值
- [x] 5.4 运行中直接关窗 → 进程内代理停止、系统代理还原、无残留监听（`ss -tln` 复核）
- [x] 5.5 与旧版共存：1080/1081 仍被旧程序占用时，新应用用别的端口正常工作，且**自身不再制造第二个口**
      验证（实测，全部真机输出）：启用期间 `ss -tlnp | grep :1082` → `LISTEN 0.0.0.0:1082 users:("ssr-client-gtk",pid=…,fd=16)`；`ss -tlnp | grep pid=<app>` 计数 → **1**；`curl --socks5-hostname 127.0.0.1:1082 …generate_204` → **204**（两次）；`kreadconfig5 … ProxyType` → **1**、`socksProxy` → **127.0.0.1 1082**；停用/关窗后 1082 消失、`socksProxy` → **127.0.0.1 1080**（精确回启用前基线，基线本身是旧程序设的）、快照文件删除、关窗后进程退出；5.5 启用期间 `1080(旧)/1081(旧 sidecar)/1082(我们)` 三口共存且本应用恰 1 口；逐条原始输出见 §11 Phase 5 进度行

### Phase 6 — 发布门禁（全绿 = 成品可发布）

- [x] 6.1 门禁命令全绿：
      `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
      验证（实测）：`fmt --check` → 绿；`clippy -D warnings` → 0；`cargo test` → **39 passed / 0 failed / 1 ignored**（最终代码）
- [x] 6.2 `cargo doc --no-deps` 无告警；`cargo build --release` 通过（profile 参照库：`lto = "thin"`、`strip = "symbols"`）
      验证（实测）：`cargo doc --no-deps` → `^warning` 计数 **0**；`cargo build --release` → `Finished release profile ... in 35.43s`
- [x] 6.3 安装打包工具：`cargo install cargo-deb cargo-generate-rpm`
      验证（实测）：`cargo deb` 与 `cargo generate-rpm` 均直接跑通（`~/.cargo/bin/` 下两个子命令就位）
- [x] 6.4 **四种产物**齐：
      - **deb**：`cargo deb`（`[package.metadata.deb]` 声明依赖，`libgtk-4-1`/`libadwaita-1-0` 按目标发行版核对包名）
      - **rpm**：`cargo generate-rpm`（依赖对应 Fedora 包名）
      - **Arch**：`packaging/PKGBUILD`，本机 `makepkg -f`（**不用 `-s`**：本机是 Fedora，pacman 源为空，运行时依赖 GTK≥4.18/ADW≥1.7 已由 rpm 提供）→ `ssr-client-gtk-<version>-1-x86_64.pkg.tar.zst`
      - **tar.gz**：`packaging/tarball.sh` → `ssr-client-gtk-<version>-x86_64.tar.gz`（bin + desktop + metainfo + icons + LICENSE + README）
      验证（实测，最终代码构建）：`ls -lh` → deb **1004K**、rpm **1.2M**、`ssr-client-gtk-0.1.0-1-x86_64.pkg.tar.zst` **1.4M**、`ssr-client-gtk-0.1.0-x86_64.tar.gz` **1.4M**；`dpkg-deb -c` 含 `usr/bin/ssr-client-gtk`、desktop、metainfo、hicolor 图标 **5 个**（48/64/128/256 png + svg）、LICENSE、README；`rpm -qlp` 同上且 `Requires: gtk4>=4.18 libadwaita>=1.7` + 自动 soname 依赖；pacman 包内 `usr/bin`、`usr/share/{applications,metainfo,icons}`、`licenses/LICENSE`；`tar -tzf` 含 `bin/`+`share/`+`share/doc`。打包踩坑记账：rpm 字段是 `[package.metadata.generate-rpm]` 且 `.requires` 为**独立子表**（键=包名）；PKGBUILD 的 source 必须是**源码树** tar（`tarball.sh` 同时产 `$OUT.tar.gz` 源码与 `$OUT-x86_64.tar.gz` 发布布局，注意 `tar | head` 会触发 SIGPIPE+`set -e` 提前退出）；makepkg 用 `PKGEXT='.pkg.tar.zst'` 覆盖本机默认扩展名、`--nodeps` 跳过 Fedora 上空 pacman 源的依赖检查；**`./build.sh`（或 `./build.sh --check`）一条命令完成全部四件套**：版本单一来源 Cargo.toml → 自动同步 PKGBUILD pkgver + 产物文件名/包内版本断言（deb/rpm 可用时逐个核对），依赖核验（gtk4≥4.18/adw≥1.7 + features/rpm/PKGBUILD 三处下限一致 + cargo-deb/cargo-generate-rpm 缺失自动安装 + makepkg/rsync 缺失指引），`cargo --locked` 可复现构建
- [x] 6.5 干净环境安装冒烟（验证已贴）
      验证（实测）：**tar.gz 本机解压直跑**完成真实 §5 全链路 —— 启用后 `ss -tln` 仅 `0.0.0.0:1082` 一个口、`curl --socks5-hostname 127.0.0.1:1082 …generate_204` → **204**、KDE `ProxyType=1`/`socksProxy=127.0.0.1 1082`、**真实 `~/.config` 全程未触碰**（隔离 HOME 冒烟）、停用后 1082 消失 + ProxyType 键删除 + 快照删除、关窗进程退出、日志无 error；**三容器**（fedora:44 `dnf install` / archlinux:latest `pacman -U` / ubuntu:26.04 `apt install`，脚本 `packaging/smoke-in-container.sh`，`--security-opt label=disable` 放行宿主 Wayland socket，容器共享宿主 `XDG_RUNTIME_DIR` 跑真 GUI）用**最终件**全部 `CONTAINER_SMOKE_PASS` rc=0：安装成功 + payload 文件齐 + 启用恰 1 口 `00000000:043A` + 停用后 1082 消失 + 进程退出 + 日志 clean；容器无桌面会话 → 走 `Unsupported` 降级路径，即 4.143 兼容降级的实测
- [x] 6.6 打包元数据齐：`application_id`、版本、图标（多尺寸）、`appstreamcli validate data/*.metainfo.xml` 通过（本机已装该工具）
      验证（实测）：`appstreamcli validate …metainfo.xml` → **✔ 验证成功**（最终件重打后复验仍通过）
- [x] 6.7 文档齐：`README.md` 中英双语（系统要求 GTK≥4.18/ADW≥1.7、四种安装方式、使用、配置目录、**单端口与 `0.0.0.0` 监听的安全说明**、常见问题"端口被占用"）、`CHANGELOG.md` 0.1.0 条目
      验证（实测）：README 116 行中英双语（单端口/0.0.0.0 代价/四安装方式/端口占用 FAQ 齐）；CHANGELOG 0.1.0 — 2026-09-26 在位
- [x] 6.8 手动回归总清单（§8 全部项）逐条勾选并贴输出
      验证（实测）：§8 UI 清单 **13/13** 全勾（证据见各条行尾注记，截图为 `phase6-*.png`/`phase5-*.png`）；点击类信号（保存/删除确认）因会话内无输入合成工具，以 QA 钩子 `emit clicked`/`emit response` 走**真实信号闭包**（main.rs 已注明），其余全部为真机真实路径
- [ ] 6.9 终验：`git status` 干净度与提交由你决定（**我不 commit/push/tag**）

---

## 8. 测试计划总表

| 层 | 范围 | 命令 | 必须覆盖 |
|---|---|---|---|
| 单元 | `config` / `sysproxy` / `i18n` | `cargo test` | CRUD、路径穿越拒绝、未知键容忍（`ssr_service_port` 被忽略且不写回）、系统代理命令拼装与还原顺序、中英词条非空 |
| 编译期 | `i18n::Strings` | `cargo build` | 词条字段全集——删字段必须编译失败（struct 即校验，无 JSON 词条可漂移） |
| 集成 | `core::proxy` | `cargo test` | 起停、SOCKS5 握手、端口冲突、重复启用、stop 后端口可复用、**单端口断言** |
| 静态 | 全部 | `cargo fmt --check && cargo clippy --all-targets -- -D warnings` | 零告警（与 `ssr-client-rs` 同标准） |
| e2e 手动 | 真实 SSR 服务 | §Phase 5 的 5.1–5.5 | `curl` 走通 204、KDE 系统代理读回、关窗无残留、**监听口 == 1** |
| UI 手动 | 中英 × 深浅色 × 三视图 | `cargo run` 对照清单 | 见下 |
| 兼容 | 老 `~/.ssr/*.json` | 单测 fixture + 实机加载 `hk.json` | 不改写用户已有文件内容，`ssr_service_port` 不残留 |
| 打包 | Arch/deb/rpm/tar.gz 安装冒烟 | §Phase 6.5 | 四种干净环境可装可用 |

**UI 手动回归清单（每轮发布前跑一遍，对应 ARCHITECTURE §11）**

- [x] 首次启动（`~/.ssr` 不存在）→ 自动建目录、列表空态 —— 实测 `HOME=<fresh>` 首启后 `.ssr` 自动创建（ls: drwxr-xr-x .ssr）+ 进程正常拉起
- [x] 新建配置（纯字母名）→ 列表出现；非法名（数字/符号/含 `/`）在表单与后端双层拦截 —— 实测 AUTOCREATE 走真实 `read()`/校验/保存链路：`devtest.json` 落盘、`listen_address` 被强制写回 `0.0.0.0`、无 `ssr_service_port`、toast「配置创建成功！」（phase6-create.png）；非法名由 `validate_name=^[a-zA-Z]+$` 单测 + form 层校验双层覆盖（Phase4 实测数字名被表单拦截）
- [x] 编辑 → 回填正确、保存后列表刷新、`cfg_name` 不可改 —— 实测截图 phase6-edit.png：标题「编辑配置」、回填全对（127.0.0.1/掩码密码/三下拉/监听 1083）、名称置灰 +「创建后不可修改」；AUTOSAVE 走真实保存闭包后文件仍可解析、列表刷新
- [x] 删除 → AlertDialog 确认 → 文件确实消失 —— 实测截图 phase6-delete-dialog.png（标题「确定要删除配置 "devtest" 吗？」/ 取消 / 红色删除），1.5s 后 emit `response=delete` 走真实 `connect_response` 闭包 → 文件消失（`.ssr/` → 0 个）
- [x] 启用 → 库起来、`ss -tlnp` 只有一个口、系统代理生效、Toast 提示、状态点变绿 —— Phase5 5.2 实测：`0.0.0.0:1082` 恰 1 口、ProxyType=1 / socksProxy=127.0.0.1 1082、curl 204、已连接态截图
- [x] 重复启用 → 明确错误 —— 单测 `AlreadyRunning`（二次 enable 返回类型化错误）；UI 按钮为开关语义（运行中再点=停用，Phase4 真机验证过切换）
- [x] 停用 → 监听消失、系统代理还原为**原值** —— Phase5 5.3 + tar.gz 冒烟：1082 消失、`socksProxy` 精确回 `127.0.0.1 1080`、快照文件删除
- [x] 外部杀掉代理任务（或端口被占）→ UI 状态自动回落，不显示"已连接" —— 真机实测占用分支：hk 启用撞旧程序 1080 → toast「启用代理失败: 端口 1080 已被占用，请先结束占用该端口的程序，或在配置中改用其他本地端口」+ 状态保持「未连接」+ 系统代理未动（phase6-portbusy-toast.png）；崩溃对账由 `status_reconciles_dead_core` 单测 + 窗口 `is-active` 聚焦 `update_status()` 覆盖
- [x] 运行中关窗 → 无残留、系统代理还原 —— Phase5 5.4 实测：进程退出、监听消失、`socksProxy` 回基线、快照清、日志 0 error
- [x] 中文/英文逐个切换，无原始 key、无漏网英文 —— 双向实测：zh→en 写 `settings.json {"language":"en-US"}`、en→zh 写回 `{"language":"zh-CN"}`（phase6-lang-zh-back.png 全中文）；en 视图零中文截图在 Phase4；4.5 编译失败实验保证词条字段全集
- [x] 深/浅色（跟随系统）无硬编码刺眼色块 —— 实测 `plasma-apply-colorscheme WhiteSurDark` → 深底浅字截图（phase6-dark2.png）、切回 WhiteSurAlt → 浅色（phase6-light2.png），配色已还原原值 `WhiteSurAlt`（gsettings color-scheme 在 KDE 下被 portal 覆盖不生效，故走 KDE 全局配色）；`grep -rE '#[0-9a-f]{3,8}|rgba?\(' ui/ src/` → 0 命中
- [x] 二次启动单实例（旧窗口前置） —— 实测二次启动 pid `111397` 不变、`pgrep -cx` = **1**（二次走 `present()` 前置旧窗口）
- [x] `~/.ssr/hk.json` 可直接加载并启用（老配置零迁移，`ssr_service_port` 不出现在任何界面） —— 加载实测：仪表盘回填全部正确（`0.0.0.0:1080`）；启用因旧程序正占 1080 被**正确拒绝**（端口占用分支见上条；端口空闲时同一启用链路已用 1082 完成多次成功启用并 204）；零迁移与 `ssr_service_port` 忽略由单测 `written_files_parse_with_ssr_client_rs`、`unknown_keys_are_ignored_and_not_written_back` 覆盖，实机 grep 生成文件 0 命中

---

## 9. "成品可直接发布"的定义（验收即此十条）

1. 门禁三连全绿（fmt / clippy -D warnings / test），测试数写进进度行。
2. `cargo build --release` 成功，产物 strip。
3. **Arch + deb + rpm + tar.gz 四种产物齐全**，依赖声明正确，包内含 desktop/metainfo/图标/LICENSE。
4. 四种形态各自在干净环境安装/解压后可启动、可完成一次完整启停（贴输出）。
5. 真实 SSR 服务端 e2e：`curl` 走 SOCKS5 拿到 `204`（贴输出）。
6. **单端口**：启用期间本进程监听口数量 == 1，系统代理指向的正是这一个端口（贴 `ss -tlnp` 与 `kreadconfig5` 输出）。
7. 关窗/异常退出无残留：无遗留监听、系统代理不指向死端口。
8. 老配置 `~/.ssr/*.json` 直接可用，无需迁移，`ssr_service_port` 被忽略且不写回。
9. 中英双语齐全（词条 struct 编译期校验），README 双语 + CHANGELOG，`appstreamcli validate` 通过。
10. 许可证合规：GPL-3.0-or-later + 第三方声明。

---

## 10. 已定决策（你 2026-09-25 拍板，不再改动）

| # | 决策 | 结论 |
|---|---|---|
| D1 | 国际化机制 | **不用 JSON 词条**：Rust struct 词条（与库的 struct 传参风格一致），字段编译期校验 |
| D2 | `listen_address` | **必须 `0.0.0.0`**（写死，UI 不提供修改；README 注明无认证 SOCKS5 局域网可达） |
| D3 | 打包形态 | **Arch + deb + rpm + tar.gz** 四种 |
| D4 | 语言 | **只保留中文与英文**（zh-CN / en-US） |
| D5 | 端口 | **只暴露一个端口**：经 SSR 协议链路处理后的那个口；去掉旧版的第二个"服务端口"（1080 服务口 + 1081 协议口的双口结构） |

> D5 若"路由处理过"另有所指（例如指路由器/网关侧的处理），指出后我按你的定义改 §6 N1 与 §4.1 的单端口条目。

---

## 11. 进度行

（每完成一项在此追加：`日期 | 项号 | 摘要 | 真实命令输出`；空 = 尚未开始）

- 2026-09-25 | 文档 | 本 GOAL.md 落盘；版本配对与环境快照实查：crates.io 确认 `gtk4 0.11.5` 有 feature `v4_18`（MSRV 1.92）、`libadwaita 0.9.2` 有 `v1_7`+`gtk_v4_18` 且依赖 `gtk4 ^0.11`；本机 `gtk4 4.22.4` / `libadwaita 1.9.2` / `rustc 1.97.1` / `XDG_CURRENT_DESKTOP=KDE`，`pkg-config` 无 gtk4 头文件；`~/.ssr/hk.json` 为嵌套 `client_settings` + `ssr_service_port`；`/opt/ssr/ssr-server` 可作本地 e2e | 见 §2.3 实测记录
- 2026-09-25 | 文档修订 | 按你的四条批示改稿：①词条改 struct、语言只留中英；②`listen_address` 定死 `0.0.0.0`；③打包改 **Arch+deb+rpm+tar.gz**（实测本机 `dpkg-deb`/`rpmbuild`/`tar`/`podman`/`appstreamcli` 可用、`makepkg` 缺 → 走 podman 容器）；④单端口。双端口实证：`ss -tlnp` 显示旧应用 `/usr/bin/shadowsocksr-client-linux` 监听 `127.0.0.1:1080`、其 sidecar `ssr-native-client -c ~/.ssr/hk.tmp.json` 监听 `0.0.0.0:1081`，`hk.tmp.json` 是 `hk.json` 的副本且 `listen_port` 被改写为 `ssr_service_port`(1081)，旧二进制内含 `ssr_service_port`/`.tmp.json` 字符串 | `ss -tlnp`、`cat ~/.ssr/hk.json`/`hk.tmp.json`、`strings /usr/bin/shadowsocksr-client-linux` 实测输出（其中"`makepkg` 缺"已被随后安装推翻，见下行）
- 2026-09-25 | 环境 | 你已安装 makepkg，上行"走 podman 容器构建 Arch 包"的做法**作废**：§2.3 / §3 / §5 / §7-6.4 / §7-6.5 五处改为本机直接构建，命令 `makepkg -f`（**不用 `-s`**：本机是 Fedora，pacman 源为空，运行时依赖 GTK≥4.18/ADW≥1.7 已由 rpm 提供）→ 产出 `ssr-client-gtk-<version>-1-x86_64.pkg.tar.zst`；podman 只保留为 Arch 包干净环境安装冒烟（`pacman -U`）的备选 | `command -v makepkg` → `/usr/bin/makepkg`；`makepkg --version` → `makepkg (pacman) 7.0.0`；`rpm -q pacman` → `pacman-7.0.0-6.fc44.x86_64`
- 2026-09-26 | Phase 1 | `config/{path,model,store}.rs` 落盘：euid 走 `/proc/self/status`（不引 libc）、`Store::validate_name`=`^[a-zA-Z]+$` 且 `file_for` 先校验后拼路径、原子写 `.part`+rename、保存时强制 `listen_address=0.0.0.0`、列表只认 stem 合法的 `*.json` | `cargo test config::` → **10 passed**（含 `unknown_keys_are_ignored_and_not_written_back`、`written_files_parse_with_ssr_client_rs` 两个互通性用例）
- 2026-09-26 | Phase 2 | `core/proxy.rs` 落盘：tokio runtime 内置于 `ProxyService`，`enable` 前置 `probe_port`（bind 后立即释放、600ms 重试）→ `PortInUse(port)`；`disable` 以 probe bind 成功为返回条件；`status()` 对账清理已死核心；事件 `async_channel<ProxyEvent>` | `cargo test` → **29 passed / 0 failed / 1 ignored**；其中 7 个 proxy 用例含 SOCKS5 握手 `[05 00]`、`exactly_one_tcp_listener_while_running`（`/proc/self/fd` ∩ `/proc/net/tcp` state=0A：enable == 起始+1、disable == 起始）；跑完 `ss -tln` 仅剩旧程序 1080/1081 与系统服务（53/631/5355/1716），本项目监听口 0
- 2026-09-26 | Phase 3 | `sysproxy/{mod,kde,gnome,snapshot}.rs` + `notify.rs` 落盘：策略表 `Desktop::{Kde,Gnome,Unsupported}`、快照缺失键还原时 `--delete`/`reset`（不写空）、快照持久化 `~/.config/ssr-client-gtk/sysproxy-snapshot.json`、`Runner` 可注入 mock 断言完整命令行与还原顺序；`desktop_notify` 缺二进制返回 false 不 panic | 真机 gsettings 冒烟：`before: mode='none' port=1080 host='127.0.0.1'` → set host/port/mode(manual) → `after restore: mode='none' port=1080 host='127.0.0.1'`（精确回原值）；`cargo test sysproxy::` → **10 passed**；`cargo test kde_real_command_smoke -- --ignored` → **1 passed**（真实 kwriteconfig5 写 `kioslaverc-ssr-client-gtk-test` → 真实 kreadconfig5 读回 `socksProxy=127.0.0.1 12345` → 还原 → 删文件，真实 kioslaverc 未触碰）；`cargo fmt --check` → 绿；`cargo clippy --all-targets` 现存告警全部为 dead-code（sysproxy/notify 尚无生产调用方，Phase 4 接线后归零）
- 2026-09-26 | Phase 4 | `ui/{window,list,form,dashboard,toast}.rs` + `main.rs` 接线落盘；修启动即 SIGABRT（`AdwApplicationWindow` 的 `child` → `content`）；表单按钮移出滚动区固定底部；空态文案按"无配置/未选择"区分；QA 截图钩子 `SSR_GTK_DEV_SELECT/FORM/LANG`（无 env 零影响）；4.5 编译失败实验完成 | `cargo fmt --check` → 绿；`cargo clippy --all-targets -- -D warnings` → **0 告警**；`cargo test` → **38 passed / 0 failed / 1 ignored**；四视图实跑截图 vision 复核全过（zh 空态/仪表盘/表单 + en 全英文、应用窗口内零中文残留）；语言切换真实信号路径写入 `{"language": "en-US"}` 并重启加载复核；单实例：二次启动后 `pgrep -x ssr-client-gtk` 仍为单一 pid；运行日志 `grep -iE 'error|critical'` → 0 行；验证后已删 `settings.json`（语言回 locale 默认）
- 2026-09-26 | Phase 5 | 真机 e2e：按 5.1 参数+口令新造本地服务端配置（`/opt/ssr/config.json` 实为客户端格式，核账）→ `/opt/ssr/ssr-server` 监听 2800；客户端配置 `localtest.json`（`listen_port=1082`，避开 1080/1081；纯字母名——`e2e` 因 `validate_name=^[a-zA-Z]+$` 被列表过滤，首跑 enable 被"请先选择一个配置"toast 正确拒绝）；驱动走 QA 钩子（`SSR_GTK_DEV_SELECT/PROXY/DISABLE_AFTER/SELFCLOSE`）| **5.2**：`LISTEN 0.0.0.0:1082 pid=<app>`；本应用监听口计数 `= 1`；`curl --socks5-hostname 127.0.0.1:1082 …generate_204` → `204`；`ProxyType=1`、`socksProxy=127.0.0.1 1082` → 四条全过；**5.3**：停用后 1082 消失、`ProxyType=1`/`socksProxy=127.0.0.1 1080` 精确回基线、`sysproxy-snapshot.json` 删除、进程存活；**5.4**：`SELFCLOSE=8` 关窗后进程退出、1082 无、系统代理回基线、快照清、日志 `grep -iE 'error|critical'` → 0 行；**5.5**：启用期间 `127.0.0.1:1080(旧) + 0.0.0.0:1081(旧sidecar) + 0.0.0.0:1082(我们)` 共存、本应用恰 1 口、SOCKS5 再测 `204`；收尾清理：本地 ssr-server 停、`localtest.json` 删、`ss -tln` 仅剩旧 1080/1081、系统代理 `1 / 127.0.0.1 1080` 与全程开始时一致；门禁 `fmt --check` 绿 / `clippy -D warnings` 0 / `cargo test` **38 passed / 1 ignored**
- 2026-09-26 | §4.1 核对 | 必做功能清单 27 条逐条审计：补两处实现缺口——①L131 行内容错：`Store::scan()` 逐文件解析（坏文件不破坏列表），侧栏错误行红字显示“配置文件格式错误”（tooltip 全文）、不可选中（激活时 toast 错误）、仍可删除；②L138 聚焦对账：窗口 `is-active` 通知 → `update_status()` 重取 `status()`（含死核心对账）；另：侧栏宽度 `sidebar-width-fraction=0.35`（`sidebar-width-request` 属性不存在于 AdwOverlaySplitView，运行时 Gtk-ERROR 已踩过并回滚）| 新单测 `scan_reports_corrupt_files_per_row_without_failing_the_list` → 绿；真实 `broken.json` 截图核验角标完整可读；`grep -rE '#[0-9a-f]{3,8}|rgba?\(' ui/ src/` → 0（无硬编码颜色）；门禁 `fmt --check` 绿 / `clippy -D warnings` 0 / `cargo test` **39 passed / 1 ignored**；测试残留已清（`~/.ssr` 只剩 hk.json/hk.tmp.json，broken 文件与测试配置已删）；9/7/5 与 4.2 枚举全集的取舍已按 4.2 执行并在 127 行注明待你裁决

- 2026-09-26 | Phase 6 | 发布门禁全绿：6.1–6.8 打勾（6.9 git 归你）。最终代码门禁 fmt ✓ / clippy -D warnings **0** / test **39 passed, 0 failed, 1 ignored**；`cargo doc` 告警 **0**；release 构建 35.43s；四件套最终件 deb 1004K + rpm 1.2M + Arch 1.4M + tar.gz 1.4M（`appstreamcli` ✔）；tar.gz 本机真实 §5 全链路：单口 `0.0.0.0:1082` + **curl 204** + KDE 读回 + 隔离 HOME 不碰真实配置 + 停用精确还原 + 关窗零残留；三容器（fedora dnf / arch pacman -U / ubuntu apt）最终件冒烟全 `CONTAINER_SMOKE_PASS` rc=0；§8 UI 清单 **13/13** 全勾（AUTOCREATE/AUTOSAVE/AUTODELETE 钩子以 `emit clicked`/`emit response` 走真实闭包覆盖点击链路；语言双向、KDE 深浅色、单实例 pid 不变、端口占用 toast 均实测） | 见 Phase 6 验证行与 §8 各条注记
- 2026-09-26 | 6.4 补充 | 新增根目录 `build.sh`（`--check` 先跑门禁）统一四件套构建：版本处理=读 Cargo.toml 同步 PKGBUILD `pkgver` 并对四个产物文件名+deb/rpm 包内版本断言 == $VER；依赖处理=sysroot env 自动 source、gtk4≥4.18/adw≥1.7 版本比较（sort -V）、cargo-deb/cargo-generate-rpm 缺失自动 cargo install、makepkg/rsync 缺失指引、features/rpm/PKGBUILD 三处运行时依赖下限 grep 断言、cargo --locked | 实跑 `./build.sh --check` → exit 0：deb Version 0.1.0-1、rpm Version 0.1.0-1、四件套 ls -lh + sha256 打印、makepkg 完成创建 0.1.0-1
