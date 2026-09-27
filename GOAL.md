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
| 设计层 | `libadwaita` crate | `0.9` + `features = ["v1_5", "gtk_v4_18"]` | 目标系统 libadwaita 只有 1.5（Deepin 25），故下限取 `v1_5`；0.9.2 的 `v1_5`/`gtk_v4_18` feature 已实查存在，两者依赖同源 `gtk4 ^0.11` |
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

**运行时最低要求（写进 README 与打包依赖）**：GTK ≥ 4.18、libadwaita ≥ 1.5（与目标系统 Deepin 25 的 1.5.0 对齐）。本机 4.22/1.9 向下兼容，可直接运行。

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
- [x] 端口展示随当前配置变化，**本地代理设置卡片只有一行端口信息**：`实际监听 0.0.0.0:<listen_port>`（2026-09-26 拍板：只说明正在代理哪个端口；删掉 `系统代理 → …`、`在浏览器或终端中将代理指向该地址即可` 与 `SOCKS5 代理` 副标题三条）

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
      验证（实测，最终代码构建）：`ls -lh` → deb **1004K**、rpm **1.2M**、`ssr-client-gtk-0.1.0-1-x86_64.pkg.tar.zst` **1.4M**、`ssr-client-gtk-0.1.0-x86_64.tar.gz` **1.4M**；`dpkg-deb -c` 含 `usr/bin/ssr-client-gtk`、desktop、metainfo、hicolor 图标 **5 个**（48/64/128/256 png + svg）、LICENSE、README；`dpkg-deb -f Depends` → **`libadwaita-1-0 (>= 1.7), libgtk-4-1 (>= 4.18)`**（GUI 运行库带版本下限，build.sh 产物级断言强制校验）；`$auto`（dpkg-shlibdeps 自动 ELF 依赖）在本机非 Debian 系不展开——build.sh 显式提示此限制，在 Debian/Ubuntu 构建机上重跑会自动补齐 libc6 等，显式 GUI 依赖 + Ubuntu 26.04 容器 `apt install` 实测（版本下限被检查）+ 完整冒烟 `CONTAINER_SMOKE_PASS` rc=0 兜底；`rpm -qlp` 同上且 `Requires: gtk4>=4.18 libadwaita>=1.7` + 自动 soname 依赖；pacman 包内 `usr/bin`、`usr/share/{applications,metainfo,icons}`、`licenses/LICENSE`；`tar -tzf` 含 `bin/`+`share/`+`share/doc`。打包踩坑记账：rpm 字段是 `[package.metadata.generate-rpm]` 且 `.requires` 为**独立子表**（键=包名）；PKGBUILD 的 source 必须是**源码树** tar（`tarball.sh` 同时产 `$OUT.tar.gz` 源码与 `$OUT-x86_64.tar.gz` 发布布局，注意 `tar | head` 会触发 SIGPIPE+`set -e` 提前退出）；makepkg 用 `PKGEXT='.pkg.tar.zst'` 覆盖本机默认扩展名、`--nodeps` 跳过 Fedora 上空 pacman 源的依赖检查；**`./build.sh`（或 `./build.sh --check`）一条命令完成全部四件套**：版本单一来源 Cargo.toml → 自动同步 PKGBUILD pkgver + 产物文件名/包内版本断言（deb/rpm 可用时逐个核对），依赖核验（gtk4≥4.18/adw≥1.7 + features/rpm/PKGBUILD 三处下限一致 + cargo-deb/cargo-generate-rpm 缺失自动安装 + makepkg/rsync 缺失指引），`cargo --locked` 可复现构建
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

### Phase 7 — 流量路由、DNS 与系统代理扩展（2026-09-26）

- [x] 7.1 路由模块 `src/routing/{cidr,dns,acl,mod}.rs` + vendored `src/routing/china_cidrs.txt`（IPv4 **6206** 行 + IPv6 **3410** 行，源自 `gaoyifan/china-operator-ip` commit `75f2eb0…`，MIT，文件头含许可全文与两段 sha256）
      验证（实测）：`cargo test cidr::` → **6 passed / 0 failed**（含 `china_set_is_non_empty_and_sane`：`223.5.5.5`/`114.114.114.114`/`1.2.4.0` 与 v6 `2400:3200::1`/`2408:8000::1`/`240e::1` 命中，`8.8.8.8`/`1.1.1.1`/私网 与 v6 `2001:4860:4860::8888`/`2606:4700:4700::1111`/`fe80::1` 不命中）；`awk '!/^#/ && /:/' china_cidrs.txt | wc -l` → **3410**、`awk '!/^#/ && !/:/'` → **6206**
- [x] 7.2 唯一 SOCKS5 入口分流 `src/core/socks5.rs` + `src/core/proxy.rs`：监听口数仍为 1（TCP accept → `Router::decide` → Direct / 经 SSR；UDP ASSOCIATE 跟 profile 的 `udp` 开关，不参与分流），启用前先 `Router::build` 校验 ACL/DNS，坏文件在绑定前报错
      验证（实测）：真机 e2e 两批（见 §11）——全局模式出口 IP `206.237.10.116`；启用期间本进程监听口 `== 1`
- [x] 7.3 DNS 二选一（系统 DNS / 自定义，`hickory-resolver` 本地解析，只服务本地路由判定与直连解析）
      验证（实测）：`cargo test routing::` → **30 passed / 0 failed / 1 ignored**；真机对照 `bypass-cn` + 黑洞自定义 DNS `192.0.2.1` → `204` 但 `time_total=15.34s`（2×5s 超时），同模式系统 DNS → `204` / `0.072s`（差 200 倍，证明自定义解析器确实被查）
- [x] 7.4 UI 卡片「路由、DNS 与系统代理」（`ui/dashboard.ui` + `src/ui/dashboard.rs` + `src/i18n.rs`）：五个路由模式下拉、ACL 文件选择、DNS 二选一、系统代理方式二选一，改任何一项都提示"重新启用生效"
      验证（实测）：`xmllint --noout ui/dashboard.ui` → OK；中英双语截图 vision 复核逐行读出无截断/重叠（zh：`路由规则=全局代理（全部流量走代理）`、`DNS 解析=系统 DNS`、`系统代理方式=桌面设置`；en：`Routing mode=Global (route everything)`、`System proxy=Desktop settings`），截图 `~/.hermes/cache/scratch/ui_{zh,en}.png`
- [x] 7.5 环境变量代理后端 `src/sysproxy/env.rs`：按 `$SHELL`（回退父进程链）决定写 `~/.bashrc` / `~/.zshrc` / `~/.config/fish/conf.d/ssr-client-gtk.fish`，块内 `socks5h://127.0.0.1:<port>` 的 `http_proxy/https_proxy/all_proxy/no_proxy`（小写+大写），`no_proxy=localhost,127.0.0.1,::1`；GUI「系统代理方式」二选一（桌面设置 / 环境变量），选环境变量时先探测 shell，探测不到当场 toast 并回退
      验证（实测）：`cargo test env::` → **7 passed / 0 failed**（含 `apply_then_restore_is_byte_identical`、`file_created_by_us_is_removed_on_restore`、`enabling_twice_does_not_duplicate_the_block`、`user_edits_made_while_the_proxy_was_on_survive`、fish 语法分支）；真机 e2e：启用后 `~/.bashrc` 出现 8 行 `export …="socks5h://127.0.0.1:1080"`、SOCKS5 问候回 `0500`、**`kioslaverc` 全程 `ProxyType=0` 未被触碰**、关窗后 `~/.bashrc` sha256 `19fd3189…` 前后相同（`BASHRC RESTORED byte-identical`）、app 日志 0 行 error
- [x] 7.6 桌面策略修订：**COSMIC 移出 GNOME 策略组**（cosmic-settings 无代理项，写 gsettings 是静默空操作 → `Unsupported` + 手动提示）；DDE/UKUI 保留（实测走 gsettings）；KDE 增加 KF5/KF6 工具名双写（`kwriteconfig5` 缺失才回退 `6`）、写入顺序=地址在前 `ProxyType` 在后、**失败先按快照回滚再报错**；GNOME 同样 enable 失败按快照回滚
      验证（实测）：`cargo test sysproxy::` → **21 passed / 0 failed / 1 ignored**（`classify_desktop` 逐 token：`X-COSMIC`/`cosmic` 进 `Unsupported`）；`cargo test kde_real -- --ignored` → **1 passed**
- [x] 7.7 ACL 小节名与 `shadowsocks-rust` 实测规范对拍：`[bypass_all]`/`[proxy_all]`（最后一个 mode 头生效）、`[bypass_list]`≡`[black_list]`、`[proxy_list]`≡`[white_list]`、域名规则在 proxy_list 优先、IP 规则在 bypass_list 优先、未命中按 mode 默认 —— `src/routing/acl.rs` 已按此实现；真机 ACL 用例：`domain:api.ipify.org` → 强制走代理 `206.237.10.116`，对照 `example.com` → 直连 `200`（两者可区分，同址直连本机不可达）

- [x] 7.8 DNS 下拉预设（阿里云 / 腾讯云）+ 输入框可见性 bug 修复：`DnsConfig` 增 `Ali`/`Tencent` 两个变体，配 `servers()`（预设=已知服务器列表，复用 Custom 的校验与解析器构建）与 `from_servers()`（列表与预设完全一致则吸附回预设，改一个 IP 就退回自定义）；下拉 4 项 = 系统 DNS / 自定义 DNS / 阿里云 DNS / 腾讯云 DNSPod，选中预设即自动填入输入框（可改），说明行按预设追加 DoT/DoH 信息并注明"本客户端仍按 UDP/TCP 53 直连查询，不走 DoT/DoH"。**顺带修掉同批次的真 bug**：`sync_dns`（下拉切换路径）不更新 `entry_dns` 可见性，导致点「自定义 DNS」看不到输入框 —— 改为下拉/输入框两条路径都经 `apply_dns` 同步（下拉可重写输入框内容，输入框路径永不重写，避免打字时光标跳动）
      验证（实测）：`cargo test` → **95 passed / 0 failed / 3 ignored**（新增 `presets_hold_the_published_servers`、`presets_round_trip_through_the_settings_field`、`presets_survive_settings_json`、`preset_resolvers_build`、`preset_resolvers_answer_on_the_real_internet`）；`cargo test preset_resolvers_answer -- --ignored` → 真实网络下 `Ali → [2409:8c54:…, 183.240.99.224, 111.45.11.5]`、`Tencent → […]` 均解析出 www.baidu.com；持久化：`{"dns":{"mode":"ali"}}` 重启（不带任何钩子）后下拉=阿里云 DNS、输入框 `223.5.5.5, 223.6.6.6, 2400:3200::1, 2400:3200:baba::1`，英文界面 `DNSPod Public DNS+ (119.29.29.29)` + 6 个 IP，截图 vision 复核（`dns_restart_ali.png` / `dns_restart_tencent_en.png`）；三 DNS 对照真机 e2e（system / ali / tencent 各一轮，bypass-cn 模式）：`www.baidu.com` 直连 **200**、`api.ipify.org` 走代理 **206.237.10.116**、`1.1.1.1` **301**、`example.com` **200**，每轮关窗 `kioslaverc` byte-identical、日志 0 error

- [x] 7.9 右上角「关于」按钮 + `icon.png` 图标打包：标题栏 `pack_end` 加「关于」**图标按钮**（`Button::from_icon_name("help-about-symbolic")`，flat、tooltip 与无障碍 Label 用中英 `about_label`；`pack_end` 先入者靠右 → 实际顺序 **语言下拉 → 关于图标 → 窗口三键**），点击弹 `AdwAboutDialog`（libadwaita 1.6+，取代已废弃的 `AdwAboutWindow`）—— 应用名=标题栏名、图标=`APP_ID` 主题图标、版本=`env!(CARGO_PKG_VERSION)`、`developer_name`=`liangzhaoyuan12`（**主页显示**）、`website`=项目地址、`comments` 三行 `项目地址 / 开源协议 / 作者`（中英各一套）、`license` 用**字符串** `GPL-3.0-or-later`（不用 `gtk::License::Gpl30`，那只会打印 `GPL-3.0`、丢掉 or-later）。**GTK 自带文案不认应用内语言开关**（gettext 读 `LC_MESSAGES`）→ `with_messages_locale()` 在建对话框前把 `LC_MESSAGES` 切到 UI 语言（zh→`zh_CN.UTF-8`、en→`en_US.UTF-8`/`C`，任何一步失败就保持原样），建完立即还原，除建窗外不暴露切换后的 locale。图标以根目录 `icon.png`（512×512）为**唯一来源**：`build.sh` 新增 4b 步每次用 ImageMagick 重生成 `data/icons/hicolor/{48,64,128,256,512}/apps/<APP_ID>.png`，`file -b` 断言 512×512 源与各尺寸产物、断言 `.desktop` 的 `Icon=`；窗口 `set_icon_name(APP_ID)`。**图标解析踩了三个坑（已修，你报的"cargo run 图标不是 icon.png"就出在这里）**：① `add_search_path` 的入参必须是**主题目录的父目录**（形如 `$XDG_DATA_DIRS/icons`），原来传 `data/icons/hicolor` 根本不生效；② GTK 只认带 `index.theme` 的主题目录，新增 `data/icons/hicolor/index.theme`，但它**不进包**（装机由 hicolor-icon-theme 提供，我们带一份会盖掉系统 hicolor 定义）；③ 这段必须放 `Ui::new`——`main()` 里 GTK 尚未初始化，`Display::default()` 返回 `None`、整段静默跳过。修好前 GTK 解析不到图标名，回退到当前主题的 `image-missing` 占位图（Win11-black-dark 的蓝色图片图标），所以看到的压根不是我们的图；同时**删掉旧美术 `hicolor/scalable/…svg`**（修好后 scalable 会抢在 PNG 前面显示）并同步删掉 Cargo.toml / PKGBUILD / 容器冒烟脚本里对它的三处引用。**顺带修掉真 bug**：cargo-deb 的资产通配符把 `hicolor/*/apps/*.png` 全部拍平成 `hicolor/<文件名>`（四尺寸互相覆盖，deb 里只剩一张 12KB 图），改成文件名位通配 `hicolor/*x*/apps/*.png` 后保留 `48x48/apps/` 目录结构；rpm 资产与 PKGBUILD 循环补 `512x512`；`Cargo.toml repository` / `PKGBUILD url` / `metainfo <homepage>` 指向 GitHub 项目地址（原为 cnb.cool 旧地址）
      验证（实测）：`./build.sh` → **rc=0**，日志 `==> 图标: icon.png (512x512) → hicolor 48/64/128/256/512（com.liangzhaoyuan12.ssr-client-gtk）` + `==> 四件套齐备（0.1.0）`；deb 解包后 `find …/usr/share/icons` → **5 个尺寸各就各位**（修复前只有 `hicolor/<文件名>` 一张），`dpkg-deb -c` / `rpm -qlp` / `tar -tzf` / `tar --zstd -tf` 四处断言都含 `hicolor/512x512/apps/com.liangzhaoyuan12.ssr-client-gtk.png`；deb 内二进制 `strings` 命中 `https://github.com/liangzhaoyuan12/ssr-client-gtk` 与 `GPL-3.0-or-later`；QA 钩子 `SSR_GTK_DEV_ABOUT=1` **emit 按钮真实 `clicked` 信号**（验证按钮→对话框接线，不是绕过按钮开窗），中英截图 vision 复核（`about_zh.png` / `about_en.png`）：主页自上而下 = icon + 应用名 + `liangzhaoyuan12` + `0.1.0` + 三行（zh=`详细信息/鸣谢/法律信息`、en=`Details/Credits/Legal`，en 窗口**零中文**），右上角顺序 语言下拉 → 关于（`help-about-symbolic` 图标按钮）→ 窗口三键，无截断重叠；临时控件树 dump（验完已删）证实「详细信息」页 GtkLabel = `项目地址：https://github.com/liangzhaoyuan12/ssr-client-gtk\n开源协议：GPL-3.0-or-later\n作者：liangzhaoyuan12`、「法律信息」页 GtkLabel = `GPL-3.0-or-later`。**图标解析实测**：`strace -e trace=openat` 抓到 `cargo run` 实际打开 `data/icons/hicolor/{48,128,256}x…/com.liangzhaoyuan12.ssr-client-gtk.png`（修前抓到的是 `~/.local/share/icons/Win11-black-dark/…/image-missing.svg`，即占位图）；中英截图 vision 复核对话框图标 = 粉色圆底 + 白色纸飞机 + "Live"（与 `icon.png` 一致）且与任务栏图标同一画面；模拟装机（release 二进制 + 假 `$XDG_DATA_DIRS`、cwd 下没有 `data/icons`）同样打开包内 4 个尺寸 PNG、日志 0 error；`./build.sh` rc=0 并新增四条断言 `512 逐像素一致 ✓`、`index.theme 在 ✓`、`无 SVG ✓`、`index.theme 未进包 ✓`；门禁 `cargo fmt --check` 绿 / `cargo clippy --all-targets -- -D warnings` **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / `xmllint` OK

- [x] 7.10 你报的两个逻辑 bug（改服务器端口带动本地端口 / 关闭软件不关代理）
      **根因①（端口联动）**：`ui/config_form.ui` 里 `spin_server_port` 和 `spin_listen_port` **共用同一个 `adj_port`** —— 两个 SpinButton 挂同一个 `GtkAdjustment` 就是写穿彼此，改一个另一个跟着变；同类问题还有 `spin_connect_timeout` 与 `spin_udp_timeout` 共用 `adj_timeout_short`。修 = 每个 spin 各自的 adjustment（新增 `adj_listen_port`、`adj_timeout_udp`），并加单测 `every_spin_button_owns_its_adjustment`（解析 `.ui`：任两个 spin 不得共用 adjustment、引用的 adjustment 必须已声明），把这类 bug 挡在 CI 里。
      **根因②（关代理）**：只有"正常关窗"会走 `close-request`（停代理 → 还原系统代理 → 删快照）；**SIGTERM / SIGINT / SIGHUP / SIGQUIT 默认直接终止进程**，实测 `kill -TERM` → 退出码 **143**、端口随进程消失但 `ProxyType` 仍 = **1**、快照残留 —— 桌面代理从此指向没人监听的口。修 = 装 `sigaction` 处理器（handler 只做一次原子写，async-signal-safe），主循环 100 ms 轮询到信号后 `window.close()`，复用同一条清理链（单实例二次激活不会重复挂轮询）。SIGKILL 拦不住 → 由残留快照 + `startup_self_heal` 在下次启动兜底。
      验证（实测）：**before/after 探针**（临时代码，验完删除，`grep -r PORT_PROBE src/` → 0）—— 把 listen 临时改回共用 `adj_port` 复现 `PORT_PROBE server=2800 listen=2800`（= 你看到的现象），恢复独立 adjustment 后 `server=2800 listen=1080`；截图 `port_probe_zh.png`（编辑表单，服务器端口 2800、状态栏 `0.0.0.0:1080`）。**三条退出路径实测**（`signal_test.sh` / `close_while_enabled.sh`，均为启用中退出）：`kill -TERM` → 进程 0、退出码 **0**（原 143）、1080 无监听、`ProxyType` **0**、快照 0、日志 0 error；`kill -INT`（后台任务默认忽略 SIGINT，我们已接管）→ 同样全绿；**正常关窗**回归 → 同样全绿；另实测泄漏后的 `startup_self_heal` 能消费残留快照。门禁 `cargo fmt --check` 绿 / `cargo clippy --all-targets -- -D warnings` **0** / `cargo test` **96 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / `xmllint` 两个 .ui 均 OK

- [x] 7.11 关窗时弹通知（点击关闭按钮 / 信号退出都要有）
      **需求**：关窗虽然把代理关了，但不该悄无声息 —— 要像平时手动停止代理那样弹一条桌面通知。
      **实现**：复用既有 `notify::desktop_notify(title, body)`（`notify-send`，title = 应用标题）——在 `close-request` 的清理线程里、`tx.send()` **之前**发送（先发通知再允许主循环销毁窗口，避免进程先退把 `notify-send` 掐掉）。新增 i18n 词条 `pc_close_stopped`：zh `窗口已关闭，代理已停用！` / en `Window closed — proxy disabled!`。**只在真有东西要清**（代理在跑或残留快照）时才发 —— 空闲关窗不打扰。SIGTERM/SIGINT 那条信号路径最终也走 `window.close()`，所以同样会弹。
      验证（实测，`dbus-monitor --session "interface='org.freedesktop.Notifications'"` 抓总线）：**启用中关窗** → 抓到 `Notify(app="ssr-client-gtk", app_name="ShadowsocksR 客户端", body="窗口已关闭，代理已停用！")`，且 `ProxyType` 归 0；**未启用关窗（对照）** → 总线上没有这条（不打扰）；**`SSR_GTK_DEV_LANG=en`** → 抓到 `Window closed — proxy disabled!`；**`kill -TERM`** → 退出码 0、`ProxyType=0`、关窗通知 **1 条**。门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **96 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / 两个 `.ui` `xmllint` OK

- [x] 7.12 GUI 记住用户上次选择的 item（不必每次重点）
      **缺口**：`Prefs` 只存了 `language` / `routing` / `sysproxy`，**侧栏选中的配置从不落盘** —— `state.selected` 每次启动都是 `None`，回回都得点一下。三个下拉（流量路由 / DNS / 系统代理方式）其实已有记忆，这次补的是**配置列表项**。
      **实现**：`Prefs` 加 `selected_profile: Option<String>`（`#[serde(default)]`，老文档照常解析）；`AppState::save_selected()` / `remembered_selection()`（非关键失败只 `eprintln`，与 `save_routing` 同规则）；新增 `ui::window::select_profile()` —— 与侧栏点击**同一条路径**：写 `state.selected` → 落盘 → `highlight_row()`（只 `select_row` 高亮，不 `rebuild`：启动时行已建好，且在 `row_activated` 信号里 `rebuild` 会把正在用的行拆掉）→ `show_dashboard()`。**四个写入点全覆盖**：侧栏点击、新建/编辑保存成功、QA `SSR_GTK_DEV_SELECT` 钩子、以及**删除当前选中项时清空记忆**（否则下次启动找一个已删的名字）。启动时在 `wire()` 之后、QA 钩子之前恢复：`remembered_selection()` 命中 `names`（只装得下可解析配置）才恢复，否则回落空状态页；QA 钩子仍可覆盖。
      验证（实测，四用例 + 截图）：①模拟点击 `SELECT=hk` → `settings.json` 出现 `"selected_profile": "hk"`（全文干净，其他字段不丢）；②**无任何钩子重启** → 截图 `mem_restore.png`：侧栏 `hk` 高亮 + 仪表盘（未连接 / 启用代理 / 实际监听 0.0.0.0:1080）；③对照 `mem_nomem.png`（删掉 settings.json）→ `hk` 不高亮 + 空状态页「请先选择一个配置」，证明恢复确实来自记忆；④记忆指向已删配置 `ghost`（`mem_stale.png`）→ 回落空状态页、无报错。**同一次无钩子重启**（`memB.png`）三项下拉也全部复原：路由规则=绕开中国大陆、DNS=阿里云 DNS（223.5.5.5, 223.6.6.6, 2400:3200::1, 2400:3200:baba::1）、系统代理方式=环境变量；`settings.json` 实测 `"mode": "bypass-cn"` 重启后仍在。门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **96 passed, 0 failed, 3 ignored**（prefs 往返测试覆盖 `selected_profile`、老文档断言 `None`）/ `cargo doc` 告警 **0** / 两个 `.ui` `xmllint` OK

- [x] 7.13 首次启动动态检测宿主机系统语言（非简中一律英语）
      **规则（按你拍板，含追加裁定：繁中电脑也看简中）**：没有保存的 `language` 时读宿主消息语言变量 → **任何 `zh*` locale → 中文**（简中繁中同看这一门简体界面：`zh` / `zh_CN` / `zh_TW` / `zh_HK` / `zh_MO` / `zh_Hans` / `zh_Hant` / `zh_SG`，含 codeset/modifier、`-`/`_` 两种写法）；`ja`/`fr`/`de`/`ko`、`C`/`POSIX`、一个变量都没设 → **英语**。变量优先级照 gettext：`LC_ALL` > `LC_MESSAGES` > `LANG`（`C`/`POSIX` 视为"无翻译"继续往下找），且 locale 非 C 时 **`LANGUAGE` 语言列表优先**（`de_DE + LANGUAGE=zh_CN` → 中文；`C + LANGUAGE=zh_CN` → 英语，gettext 本就忽略）。判定逻辑抽成纯函数 `is_simplified_chinese()` / `is_posix_locale()` / `locale_head()`，单测不碰进程环境、无并发污染；已保存的语言**永远压过**系统检测（记忆功能不变）。
      验证（实测，`lang_matrix.sh` 12 用例，每个用例都先删 `settings.json` 真·首次启动，临时 `SSR_GTK_DEV_PRINTLANG` 打印 `state.lang`，验完删除，`grep -r PRINTLANG src/` → 0）：
      矩阵终值（`zh_TW` 一行随 7.13 调整改回 zh-CN）：`zh_CN` → zh-CN；`zh_TW` → **zh-CN**（繁中看简中）；`LC_ALL=zh_TW`（覆盖 `LANG=zh_CN`）→ zh-CN；`en_US`/`ja_JP`/`fr_FR`/`C`/`C.UTF-8`/未设置 → en-US；`C + LANGUAGE=zh_CN` → en-US（gettext 忽略 LANGUAGE）；`de_DE + LANGUAGE=zh_CN` → zh-CN（认识 GNU 语言列表）；`LANGUAGE=zh_CN:en + LANG=zh_CN` → zh-CN。
      截图四张（首次启动、无钩子）：`langshot_zh_CN.png` → 中文界面 + 下拉选中「中文」；`langshot_ja_JP.png` → 英文界面 + 下拉选中 `English`；`langshot_zh_TW.png` → **繁中宿主也是中文界面 + 下拉「中文」**；`langshot_saved.png`（已存 `language=zh-CN`、宿主切日语）→ 仍中文 + 下拉「中文」，即**记忆压过系统检测**；四次运行错误行均 0。门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **98 passed, 0 failed, 3 ignored**（新增 `only_simplified_chinese_counts_as_chinese`、`posix_locales_never_decide_the_language`）/ `cargo doc` 告警 **0** / 两个 `.ui` `xmllint` OK

**Phase 7 门禁（实测）**：`cargo fmt --check` → 绿；`cargo clippy --all-targets -- -D warnings` → **0 告警**；`cargo test` → **96 passed / 0 failed / 3 ignored**；`cargo doc --no-deps` → 告警 **0**；`xmllint --noout ui/dashboard.ui ui/config_form.ui` → OK；UI 冒烟 `SSR_GTK_DEV_SELECT=hk SSR_GTK_DEV_SELFCLOSE=3` → exit 0、日志空。

---

## 8. 测试计划总表

| 层 | 范围 | 命令 | 必须覆盖 |
|---|---|---|---|
| 单元 | `config` / `sysproxy` / `i18n` | `cargo test` | CRUD、路径穿越拒绝、未知键容忍（`ssr_service_port` 被忽略且不写回）、系统代理命令拼装与还原顺序、中英词条非空 |
| 单元 | `routing`（cidr/dns/acl）+ `sysproxy::env` | `cargo test routing:: && cargo test env::` | 五模式判定、大陆 CIDR v4+v6、LAN/保留段、ACL 小节名与优先级、系统/自定义 DNS 解析、shell rc 块的写入/去重/精确还原 |
| e2e 手动 | 路由与 DNS 真机对照 | §Phase 7.2/7.3 | 五模式真机出口 IP、ACL 强制走代理 vs 直连、自定义 DNS 黑洞计时对照、监听口仍 == 1 |
| 编译期 | `i18n::Strings` | `cargo build` | 词条字段全集——删字段必须编译失败（struct 即校验，无 JSON 词条可漂移） |
| 集成 | `core::proxy` | `cargo test` | 起停、SOCKS5 握手、端口冲突、重复启用、stop 后端口可复用、**单端口断言** |
| 静态 | 全部 | `cargo fmt --check && cargo clippy --all-targets -- -D warnings` | 零告警（与 `ssr-client-rs` 同标准） |
| e2e 手动 | 真实 SSR 服务 | §Phase 5 的 5.1–5.5 | `curl` 走通 204、KDE 系统代理读回、关窗无残留、**监听口 == 1** |
| UI 手动 | 中英 × 深浅色 × 三视图 | `cargo run` 对照清单 | 见下 |
| 兼容 | 老 `~/.ssr/*.json` | 单测 fixture + 实机加载 `hk.json` | 不改写用户已有文件内容，`ssr_service_port` 不残留 |
| 打包 | Arch/deb/rpm/tar.gz 安装冒烟 | §Phase 6.5 | 四种干净环境可装可用 |
| 打包 | 图标 `icon.png` → 四件套 | `./build.sh`（内置断言） | 5 个 hicolor 尺寸各就位（文件名位通配，避免 deb 拍平）、`.desktop` 的 `Icon=`、四包都含 `512x512` |

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
- [x] 「路由、DNS 与系统代理」卡片三行齐全、双语说明无截断 —— 实测 `xmllint --noout ui/dashboard.ui` → OK；`SSR_GTK_DEV_LANG=zh/en SSR_GTK_DEV_SELECT=hk` 截图 vision 逐行复核：zh 读出 `路由规则=全局代理（全部流量走代理）` / `DNS 解析=系统 DNS` / `系统代理方式=桌面设置` + 两行说明，en 读出 `Routing mode=Global (route everything)` / `System proxy=Desktop settings` + caption，均无截断、重叠、异常空白（`~/.hermes/cache/scratch/ui_{zh,en}.png`）

- [x] 右上角「关于」按钮 → 弹出关于对话框，项目地址/协议/作者/版本/图标齐全且随语言切换 —— 实测 `SSR_GTK_DEV_ABOUT=1`（发按钮真实 `clicked`）中英截图 vision 复核（`about_zh.png`/`about_en.png`）：主页 icon + 应用名 + `liangzhaoyuan12` + `0.1.0` + 三行（zh=`详细信息/鸣谢/法律信息`、en=`Details/Credits/Legal`，en 窗口零中文）、右上角 语言下拉 → 关于图标（`help-about-symbolic`）→ 窗口三键、无截断重叠；控件树 dump 证实「详细信息」页含 `项目地址：https://github.com/liangzhaoyuan12/ssr-client-gtk` + `开源协议：GPL-3.0-or-later` + `作者：liangzhaoyuan12`、「法律信息」页含 `GPL-3.0-or-later`
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

| # | 决策 | 结论 |
|---|---|---|
| D6 | 流量路由（2026-09-26） | **五模式**：全局 / 绕开局域网 / 绕开中国大陆 / 绕开局域网+大陆 / 自定义 ACL 文件；只在 GTK 客户端做（核心库不加路由），仍是**唯一 SOCKS5 监听口**内部分流；大陆名单 vendored 自 `gaoyifan/china-operator-ip`（MIT，IPv4+IPv6） |
| D7 | DNS（2026-09-26） | **二选一**：系统 DNS / 自定义 DNS；只用于本地解析与路由判定，**走代理的域名交 SSR 服务端解析**，不本地解 |
| D8 | 系统代理写入方式（2026-09-26） | GUI 二选一：**桌面代理**（KDE 家族 / GNOME 家族及衍生）或 **环境变量**（按 `$SHELL`/父进程链写 `~/.bashrc`、`~/.zshrc`、fish `conf.d`，只对新终端生效）。纯 SOCKS5，**不写 `httpProxy`/`httpsProxy` 键**；**COSMIC 移出 GNOME 策略组**（实测无代理设置项），DDE/UKUI 保留 gsettings；**README 不写 libproxy 限制说明**（实测 https 流量同样走 SOCKS 代理） |

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

- 2026-09-26 | Phase 7 | 流量路由五模式 + DNS 二选一 + 系统代理两条路（桌面 / 环境变量）全部落盘：`src/routing/{cidr,dns,acl,mod}.rs` 与 vendored `china_cidrs.txt`（IPv4 **6206** + IPv6 **3410** 行、MIT、文件头含许可与两段 sha256，**IPv6 部分已有 v6 命中断言**）、`src/core/{socks5,proxy}.rs` 在唯一监听口内分流（启用前先校验 ACL/DNS）、`src/sysproxy/env.rs` 按 `$SHELL`/父进程链写 `~/.bashrc`、`~/.zshrc` 或 fish `conf.d`、GUI 卡片「路由、DNS 与系统代理」新增「系统代理方式」二选一（探测不到 shell 当场 toast 并回退）、**COSMIC 移出 GNOME 策略组**（→`Unsupported` 手动提示）、DDE/UKUI 保留 gsettings；**README 不写 libproxy 限制说明**（按你实测 https 流量同样走 SOCKS）。门禁：`cargo fmt --check` 绿 / `cargo clippy --all-targets -- -D warnings` **0** / `cargo test` **91 passed, 0 failed, 2 ignored** / `cargo doc --no-deps` 告警 **0**；分项 `routing::` 30 passed、`sysproxy::` 21 passed、`env::` 7 passed、`cidr::` 6 passed、`cargo test kde_real -- --ignored` 1 passed。真机 e2e 四批：①KDE `ProxyType 0→1`、SOCKS5 回 `0500`、关窗 `kioslaverc` 三键 byte-identical；②全局模式 `curl --socks5-hostname 127.0.0.1:1080 https://api.ipify.org` → **206.237.10.116**、generate_204→204、1.1.1.1→301、还原 byte-identical；③`bypass-cn` + 黑洞自定义 DNS `192.0.2.1` → 204 / **15.34s**（2×5s 超时）对照系统 DNS → 204 / **0.072s**，ACL 强制 `domain:api.ipify.org` → **206.237.10.116**（走代理）而 `example.com` → 200（直连）；④环境变量模式 → `~/.bashrc` 出现 8 行 `socks5h://127.0.0.1:1080`、**`kioslaverc` 全程 `ProxyType=0` 未被触碰**、关窗 `~/.bashrc` sha256 `19fd3189…` 前后一致（`BASHRC RESTORED byte-identical`）、日志 0 行 error。UI：`xmllint` OK，zh/en 截图 vision 逐行复核三行齐全无截断。产物：`./build.sh` rc=0 重新出包 deb **1.5M** / rpm **1.7M** / Arch **2.0M** / tar.gz **2.1M**（版本与依赖断言内置），`target/release/ssr-client-gtk` 与 tar.gz 内二进制冒烟均 **rc=0 且日志 0 error**；容器安装冒烟未复跑（Phase 6 已做过，本次未改打包脚本） | 见 Phase 7 验证行与 §8 各条注记
- 2026-09-26 | Phase 7.8 | DNS 下拉加**阿里云 / 腾讯云**预设（服务器照你给的清单：阿里 `223.5.5.5` `223.6.6.6` `2400:3200::1` `2400:3200:baba::1`；腾讯 `119.29.29.29` `119.28.28.28` `182.254.116.116` `182.254.118.118` `2402:4e00::` `2402:4e00:1::`），DoT/DoH 写进说明行并标注本客户端仍走 UDP/TCP 53；`DnsConfig` 加 `Ali`/`Tencent` 变体（`servers()`/`from_servers()`，设置文件存 `{"mode":"ali"}`，旧文件兼容）。**修复同批发现的真 bug**：切「自定义 DNS」不显示输入框（`sync_dns` 没同步可见性），下拉与输入框两条路径统一走 `apply_dns`（下拉可改写输入框、输入框路径永不改写，打字不跳光标）；QA 钩子 `SSR_GTK_DEV_DNS=system|custom|ali|tencent` 走真实 notify 信号 | 门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / doc 告警 **0**；`cargo test preset_resolvers_answer -- --ignored` 真实网络解析 www.baidu.com 成功（Ali 与 Tencent 各返回 A+AAAA）；重启持久化截图 vision 复核（zh 阿里云 4 个 IP + 完整说明、en DNSPod 6 个 IP）；三 DNS 真机对照 e2e 全绿：baidu **200** 直连 / ipify **206.237.10.116** 走代理 / 1.1.1.1 **301** / example.com **200**，`kioslaverc` 每轮 byte-identical、日志 0 error
- 2026-09-26 | 6.4 补充 | 新增根目录 `build.sh`（`--check` 先跑门禁）统一四件套构建：版本处理=读 Cargo.toml 同步 PKGBUILD `pkgver` 并对四个产物文件名+deb/rpm 包内版本断言 == $VER；依赖处理=sysroot env 自动 source、gtk4≥4.18/adw≥1.7 版本比较（sort -V）、cargo-deb/cargo-generate-rpm 缺失自动 cargo install、makepkg/rsync 缺失指引、features/rpm/PKGBUILD 三处运行时依赖下限 grep 断言、cargo --locked | 实跑 `./build.sh --check` → exit 0：deb Version 0.1.0-1、rpm Version 0.1.0-1、四件套 ls -lh + sha256 打印、makepkg 完成创建 0.1.0-1
- 2026-09-26 | 依赖补强 | deb `depends` 改为 `$auto, libgtk-4-1 (>= 4.18), libadwaita-1-0 (>= 1.7)`（GUI 运行库标明版本下限，与 rpm requires / PKGBUILD depends 三处一致）；build.sh 新增产物级断言（`dpkg-deb -f Depends` 必须含两条带版本依赖，缺则 die）+ `$auto` 未展开提示；实测 `./build.sh` → exit 0，`Depends: libadwaita-1-0 (>= 1.7), libgtk-4-1 (>= 4.18)`；Ubuntu 26.04 容器用新 deb 复验 `apt install` + 完整 GUI 冒烟 → **CONTAINER_SMOKE_PASS** rc=0（版本下限通过 apt 检查）
- 2026-09-27 | 文案精简 | 按你拍板精简两处 UI 文案：①**本地代理设置卡片**只剩标题 + 一行端口 `实际监听 0.0.0.0:1080`（删除 `SOCKS5 代理`、`系统代理 → 127.0.0.1:1080`、`在浏览器或终端中将代理指向该地址即可` 三个子项及其词条 `dash_socks5`/`dash_sysproxy_line`/`dash_copy_hint`，`DashUi.sysproxy` 字段一并移除）；②**使用说明**只保留两条 —— `火狐有自己的代理设置，需要在火狐浏览器的设置中自行设定代理。` + `开源协议：GPL-3.0-or-later`（删掉单端口/GNOME-KDE 跟随、proxychains 两条，以及 FoxyProxy/Chromium 推荐话术；中英同步，`steps` 4→2）；窗口底部状态栏的端口行不动 | 门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / `xmllint` OK；新增 QA 钩子 `SSR_GTK_DEV_MAXIMIZE=1` + `SSR_GTK_DEV_SCROLL=end` 以便截到滚动区底部；最大化+滚底截图 vision 逐字复核（`ui2_zh.png` / `ui2_en.png`）：本地代理设置 2 行、使用说明恰好 2 行（火狐 + GPL）、路由卡片三行标签不变，无截断重叠、en 窗口内零中文
- 2026-09-27 | Phase 7.9 | **右上角「关于」按钮 + `icon.png` 打包**：标题栏 `pack_end` 加「关于」flat 按钮（位于语言下拉左侧）→ 点击弹 `AdwAboutDialog`（libadwaita 1.6+ 的新 API，取代已废弃 `AdwAboutWindow`）：应用名、`APP_ID` 图标、版本 `0.1.0`、`developer_name=liangzhaoyuan12`（主页可见）、`website` 项目地址、`comments` 三行（项目地址/开源协议/作者，中英各一套）、`license` 取字符串 `GPL-3.0-or-later`（不用 `gtk::License::Gpl30`——它只会打 `GPL-3.0`）。**GTK 自带行文案（详细信息/鸣谢/法律信息 等）走 gettext/LC_MESSAGES、不认应用内语言开关**，`with_messages_locale()` 建对话框前把 `LC_MESSAGES` 切到 UI 语言（zh→`zh_CN.UTF-8`、en→`en_US.UTF-8`/`C`）建完即还原；新增依赖仅 `libc`。图标：根目录 `icon.png`（512×512）为唯一来源，`build.sh` 新增 4b 步每次 ImageMagick 重生成 `data/icons/hicolor/{48,64,128,256,512}` 并 `file -b` 断言尺寸 + `.desktop` 的 `Icon=`；窗口 `set_icon_name(APP_ID)` 并运行时加 `data/icons/hicolor` 搜索路径（源码直跑也有图标）。**修掉真 bug**：cargo-deb 资产通配 `hicolor/*/apps/*.png` 把四尺寸拍平成一张互相覆盖（deb 里只剩 1 张 12KB），改文件名位通配 `hicolor/*x*/apps/*.png` 保住 `48x48/apps/` 目录；rpm 资产与 PKGBUILD 循环补 512；`Cargo.toml repository`/`PKGBUILD url`/`metainfo homepage` 统一为 GitHub 项目地址 | `./build.sh` **rc=0**：日志 `==> 图标: icon.png (512x512) → hicolor 48/64/128/256/512` + `==> 四件套齐备（0.1.0）`，deb/rpm/Arch/tar.gz 四处断言都含 `hicolor/512x512/apps/com.liangzhaoyuan12.ssr-client-gtk.png`，deb 解包后 5 个尺寸目录齐全，`strings` 命中项目地址与 `GPL-3.0-or-later`；`SSR_GTK_DEV_ABOUT=1` **emit 按钮真实 `clicked`**（走按钮接线）中英截图 vision 复核（`about_zh.png`/`about_en.png`）：主页 icon + 名 + `liangzhaoyuan12` + `0.1.0` + 三行（zh 中文/en 英文、en 窗口零中文），右上角 关于→语言下拉→窗口三键，无截断重叠；临时控件树 dump（验完删除，`grep -r TEMP-VERIFY src/` → 0）证实详情页 `项目地址：https://github.com/liangzhaoyuan12/ssr-client-gtk\n开源协议：GPL-3.0-or-later\n作者：liangzhaoyuan12` 与法律页 `GPL-3.0-or-later` 真实渲染；门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / doc 告警 **0** / `xmllint` OK
- 2026-09-27 | 7.9 修复 | **"cargo run 图标不是 icon.png"的根因定位与修复**：实测发现显示的是 Win11-black-dark 主题的 `image-missing` 占位图（strace 抓到 `~/.local/share/icons/Win11-black-dark/apps@2x/scalable/image-missing.svg`），根本不是我们的任何一张图 —— 三个叠加原因：① `IconTheme::add_search_path("data/icons/hicolor")` 传成了主题目录本身，应传它的**父目录** `data/icons`；② `data/icons/hicolor` 没有 `index.theme`，GTK 不把它当主题；③ 设置搜索路径的代码写在 `main()`，此时 GDK display 还没打开、`Display::default()` 返回 `None`，整段**静默跳过**（改放进 `Ui::new`）。修复 = 父目录路径 + 新增源码树专用 `data/icons/hicolor/index.theme`（build.sh 断言其不进四件套，避免装机盖掉系统 hicolor 定义）+ 代码搬家；另**删掉旧美术 `hicolor/scalable/…svg`**（修好后 scalable 会优先于 PNG 被选中），并同步删掉 Cargo.toml 资产、PKGBUILD 安装、容器冒烟脚本三处引用 | 修后 `strace -e trace=openat` 抓到 `cargo run` 打开 `data/icons/hicolor/{48,128,256}x…/<ID>.png`；中英截图 vision：对话框图标 = 粉色圆底 + 白色纸飞机 + "Live"（与 icon.png 一致）且与任务栏同一画面（`about_zh.png`/`about_en.png`）；模拟装机（release 二进制 + 假 `$XDG_DATA_DIRS`、cwd 无 `data/icons`）打开包内 4 个尺寸 PNG、日志 **0 error**；`./build.sh` **rc=0** 新断言全过（`512 逐像素一致 ✓`/`index.theme 在 ✓`/`无 SVG ✓`/`index.theme 未进包 ✓`）；门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / doc 告警 **0** / xmllint OK
- 2026-09-27 | 7.9 调整 | 按你拍板改「关于」入口两点：① 由文字按钮改为**图标按钮** `gtk::Button::from_icon_name("help-about-symbolic")`（圆圈 i 图标，flat；文案不再做按钮文字，`about_label` 改作 tooltip 与无障碍 `Property::Label`，语言切换时同步更新）；② 位置改到**语言按钮右边** —— `pack_end` 先入者靠右，故把关于按钮的创建与 pack 挪到语言下拉之前，最终顺序 **语言下拉 → 关于图标 → 最小化/最大化/关闭** | 中英截图 vision 复核（`about_zh.png`/`about_en.png` 09:33）：右上角从左到右 = `中文` 文字下拉 → 圆圈 i 图标 → `_` `□` `×` 三键，间距正常无重叠，图标按 tooltip/无障碍名随语言切换；`strace -e trace=openat` 抓到它打开 `~/.local/share/icons/Win11-black-dark/actions@2x/16/help-about-symbolic.svg`（主题里真实存在，不是 image-missing 占位）；对话框内粉色纸飞机图标不变；门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **95 passed, 0 failed, 3 ignored** / doc 告警 **0** / xmllint OK
- 2026-09-27 | Phase 7.10 | **你报的两个逻辑 bug**：①**改服务器端口带动本地端口** —— `ui/config_form.ui` 里 `spin_server_port` 与 `spin_listen_port` 共用同一个 `adj_port`（`GtkAdjustment` 被两个 SpinButton 挂就是写穿彼此），同类的 `spin_connect_timeout`/`spin_udp_timeout` 也共用了 `adj_timeout_short`；修复 = 每个 spin 独立 adjustment（新增 `adj_listen_port`、`adj_timeout_udp`）+ 新单测 `every_spin_button_owns_its_adjustment`（解析 .ui：禁止共用、引用必须已声明）。②**关闭软件不关代理** —— 只有正常关窗走 `close-request`；SIGTERM/SIGINT/SIGHUP/SIGQUIT 默认直接杀进程，实测 `kill -TERM` 退出码 **143**、端口随进程消失但 `ProxyType` 残留 **1**、快照残留（桌面代理指向无人监听的口）；修复 = `sigaction` 记录信号（async-signal-safe 原子写）+ 主循环 100 ms 轮询后 `window.close()` 复用同一条清理链，单实例二次激活不重复挂轮询；SIGKILL 由残留快照 + `startup_self_heal` 兜底 | before/after 探针（临时代码已删，`grep -r PORT_PROBE src/` → 0）：共用 adjustment 时 `PORT_PROBE server=2800 listen=2800`（复现你的现象），独立后 `server=2800 listen=1080`；三条退出路径启用中实测全绿 —— `kill -TERM` → 进程 0 / 退出码 **0** / 1080 无监听 / `ProxyType=0` / 快照 0 / 日志 0 error，`kill -INT` 同样全绿，正常关窗回归同样全绿；门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **96 passed, 0 failed, 3 ignored** / doc **0** / 两个 .ui xmllint OK；`./build.sh` **rc=0**（四件套 + 图标断言全过）
- 2026-09-27 | Phase 7.11 | **关窗弹通知**：关窗虽然停了代理但悄无声息 → 复用既有 `notify::desktop_notify(title, body)`（`notify-send`，title=应用标题），在 `close-request` 清理线程里 `tx.send()` **之前**发送（先发完通知再允许主循环销毁窗口，免得进程先退把 `notify-send` 掐掉）；新增 i18n 词条 `pc_close_stopped`（zh `窗口已关闭，代理已停用！` / en `Window closed — proxy disabled!`）；**只在真有东西要清**（代理在跑或残留快照）时发，空闲关窗不打扰；SIGTERM/SIGINT 信号路径最终也走 `window.close()`，同样弹 | `dbus-monitor --session "interface='org.freedesktop.Notifications'"` 实测：启用中关窗 → 抓到 `Notify(app=ssr-client-gtk, app_name=ShadowsocksR 客户端, body=窗口已关闭，代理已停用！)` 且 `ProxyType` 归 0；未启用关窗对照 → 无此通知；`LANG=en` → 抓到 `Window closed — proxy disabled!`；`kill -TERM` → 退出码 0、`ProxyType=0`、通知 1 条；门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **96 passed, 0 failed, 3 ignored** / doc **0** / 两个 .ui xmllint OK；`./build.sh` **rc=0**
- 2026-09-27 | Phase 7.12 | **GUI 记住上次选择的 item**：三个下拉（流量路由/DNS/系统代理方式）本就有记忆，真正缺的是**侧栏配置项** —— `state.selected` 每次启动都是 `None`。修复：`Prefs` 加 `selected_profile: Option<String>`（serde default，老文档兼容）+ `AppState::save_selected()/remembered_selection()` + 新增 `ui::window::select_profile()`（与点击同一条路径：选中→落盘→`highlight_row()` 只做 `select_row` 不 `rebuild`，避免在 `row_activated` 信号里拆行→开仪表盘）；写入点全覆盖 = 侧栏点击 / 新建·编辑保存 / QA SELECT 钩子 / **删除当前项时清空记忆**；启动恢复放在 `wire()` 之后、QA 钩子之前，命中 `names` 才恢复否则回落空状态页 | 实测四用例：①`SELECT=hk` 后 `settings.json` 出现 `"selected_profile": "hk"`；②无钩子重启 → `mem_restore.png` 侧栏 hk 高亮 + 仪表盘（未连接/启用代理/实际监听 0.0.0.0:1080）；③对照 `mem_nomem.png`（无 settings.json）→ hk 不高亮 + 「请先选择一个配置」空页；④记忆指向已删配置 `ghost` → `mem_stale.png` 空页无报错；同一次无钩子重启 `memB.png` 三项下拉全复原（路由=绕开中国大陆、DNS=阿里云 DNS、系统代理=环境变量），`"mode": "bypass-cn"` 重启后仍在 | 门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **96 passed, 0 failed, 3 ignored** / doc **0** / 两个 .ui xmllint OK；`./build.sh` **rc=0**
- 2026-09-27 | Phase 7.13 | **首启动态检测宿主系统语言（非简中→英语）**：语言记忆本就有，缺的是首启动规则的"精准"——`Lang::detect()` 改造：简中判定收窄为 `zh`/`zh_CN`/`zh_Hans`/`zh_SG`（含 codeset/modifier 与 `-`/`_`），**`zh_TW`/`zh_HK`/`zh_MO`/`zh_Hant` 按"除简中外"判英语**；`ja`/`fr`/`de`、`C`/`POSIX`、未设置 → 英语；变量优先级照 gettext（`LC_ALL` > `LC_MESSAGES` > `LANG`，`C` 视为无翻译继续找），并在 locale 非 C 时认 **`LANGUAGE` 语言列表**；判定抽成纯函数 `is_simplified_chinese()/is_posix_locale()/locale_head()` 便于无环境副作用的单测；已保存语言永远压过系统检测 | `lang_matrix.sh` 12 用例（每例先删 settings.json 真·首次启动，临时 PRINTLANG 打印后删除、`grep -r PRINTLANG src/` → 0）修复前→后：`zh_CN`→zh-CN 不变；**`zh_TW` zh-CN→en-US**；**`LC_ALL=zh_TW`（覆盖 zh_CN）zh-CN→en-US**；`en_US/ja_JP/fr_FR/C/C.UTF-8/未设置`→en-US 不变；`C+LANGUAGE=zh_CN`→en-US 不变；**`de_DE+LANGUAGE=zh_CN` en-US→zh-CN**；`LANGUAGE=zh_CN:en+LANG=zh_CN`→zh-CN；截图 `langshot_zh_CN.png`（中文界面+下拉「中文」）、`langshot_ja_JP.png`（英文界面+下拉 English）、`langshot_saved.png`（已存 zh-CN 而宿主日语 → 仍中文，记忆压过系统），错误行均 0 | 门禁 fmt ✓ / clippy -D warnings **0** / `cargo test` **98 passed, 0 failed, 3 ignored**（新增 2 条纯函数单测）/ doc **0** / 两个 .ui xmllint OK；`./build.sh` **rc=0**
- 2026-09-27 | 7.13 调整 | 追加裁定：**繁中电脑也看简中**（上一条按字面把 `zh_TW`/`zh_HK`/`zh_MO`/`zh_Hant` 判成了英语）。`is_simplified_chinese()` 改名并放宽为 `is_chinese()`：`zh` 或 `zh_*` 一律中文（唯一那门中文 = 简体），单测 `only_simplified_chinese_counts_as_chinese` → `every_chinese_locale_gets_the_chinese_ui`（繁中四项挪进中文列表，另补 `ko_KR` 进英语列表）；`detect()` 文档同步。矩阵终值：`zh_TW` **en-US → zh-CN**、`LC_ALL=zh_TW`（覆盖 `LANG=zh_CN`）**en-US → zh-CN**，其余 10 用例不变（`zh_CN`/`de_DE+LANGUAGE=zh_CN`/`LANGUAGE=zh_CN:en` → zh-CN；`en_US`/`ja_JP`/`fr_FR`/`C`/`C.UTF-8`/未设置/`C+LANGUAGE=zh_CN` → en-US）；新增截图 `langshot_zh_TW.png`：繁中宿主首启 → 中文界面 + 下拉「中文」，错误行 0；临时 PRINTLANG 钩子验完删除（`grep -r PRINTLANG src/` → 0）。门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **98 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / 两个 `.ui` `xmllint` OK；`./build.sh` **rc=0**
- 2026-09-27 | Phase 6.4 调整 | **四件套产物统一到 packaging/ 一处**（你指出 build.sh 散落三处：`dist/`、`packaging/`、`target/`）。改动：①`packaging/tarball.sh` —— staging 从 `dist/` 改到 `mktemp -d` 临时目录（`trap` 自动清），两个 tar（发布布局 `*-x86_64.tar.gz`、PKGBUILD 源码 `$NAME-$VER.tar.gz`）**直接写 packaging/**，仓库里不再产生 `dist/`；②`build.sh` —— 打包前 `rm -rf target/debian target/generate-rpm dist`，`cargo deb` / `cargo generate-rpm` 完成后 `mv` 进 `packaging/`，`DEB/RPM/ARCH/TARBALL` 四个变量全指 `packaging/`；③新增三条断言：**`dist/` 不许存在**、**`target/debian|generate-rpm` 不许留 `.deb`/`.rpm`**、**源码 tar 不许混进 `.deb`/`.rpm`/`.pkg.tar.*`**（变量承接 `tar -tzf` 再 `case`，避开 `pipefail`+`grep -q` 的 SIGPIPE 误判）；④README 产物路径四行同步改到 `packaging/` 并写明"一处交付"。**踩坑记账**：deb/rpm 先搬进 `packaging/` 再跑 `tarball.sh`，rsync 的 `--exclude 'packaging/*.tar.gz'` 挡不住 `.deb`/`.rpm` —— 首跑源码 tar 从 441K 涨到 **3.7M**（里面打进了 1.6M deb + 1.7M rpm），补 `--exclude 'packaging/*.deb' 'packaging/*.rpm'` 后回到 **441K、76 条目、零包文件**，并由断言把这类回归挡死。 | 实测：`./build.sh` **rc=0 两轮**，日志含 `deb/rpm 移入 packaging/（交付物只放这里）`、`产物只在 packaging/ ✓（无 dist/，target/ 无包）`、`源码 tar 未混入包 ✓`、`四件套齐备（0.1.0）`；`ls packaging/` → deb 1.6M / rpm 1.7M / Arch `*.pkg.tar.zst` 2.1M / 发布 tar 2.1M / 源码 tar 441K；`[ ! -d dist ]` ✓；`target/debian`、`target/generate-rpm` 文件数均 0；门禁 `cargo fmt --check` 绿 / `clippy -D warnings` **0** / `cargo test` **98 passed, 0 failed, 3 ignored** / `cargo doc` 告警 **0** / 两个 `.ui` `xmllint` OK / `bash -n build.sh packaging/tarball.sh` OK
