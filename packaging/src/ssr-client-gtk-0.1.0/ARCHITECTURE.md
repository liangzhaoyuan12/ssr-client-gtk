# Shadowsocksr-Client-Linux 软件架构与布局说明（重构工作底稿）

> **本文档的用途**：把它整体粘贴给 AI / 新同事，即可在不读源码的情况下理解本项目的完整结构、数据流、接口契约和已知问题，并直接开始重构工作。
> **文档基线**：仓库 `main` 分支 @ `94e1c64`（版本 0.2.0），文档中所有 `文件:行号` 均对应此基线。
> **验证状态**：`npm run build` 在此基线通过（vite 6.4.2，46 modules，560ms）。

---

## 1. 项目一句话概述

一个 **Linux 桌面 ShadowsocksR 客户端**：Tauri 2 做桌面壳与 Rust 后端，Vue 3 做前端 UI，真正跑代理转发的是外部 sidecar 二进制 `ssr-native-client`（来自 shadowsocksr-native 项目）。本项目自身**不实现任何代理协议**，只负责：配置文件 CRUD、拉起/杀死 sidecar 进程、写系统代理设置、发通知。

### 技术栈与版本

| 层 | 技术 | 版本 | 备注 |
|---|---|---|---|
| 桌面框架 | Tauri | 2.x | Rust 后端 + WebView 前端壳 |
| 前端框架 | Vue | 3.5 | `<script setup>` Composition API，**无 TypeScript** |
| 构建工具 | Vite | 6.x | `type: module`，端口 1420 固定 |
| 国际化 | vue-i18n | 9.x | `legacy: false`（Composition 模式） |
| 后端语言 | Rust | edition 2021 | lib 名 `shadowsocksr_linux_lib` |
| 代理核心 | shadowsocksr-native | 外部预编译 | 以 sidecar 形式分发，**源码不在本仓库** |
| 包管理 | npm | — | 有 `package-lock.json` |

**关键点：本仓库无 TypeScript、无测试、无 Linter/Formatter、无 CI（无 `.github/`）。**

---

## 2. 目录结构（真实文件清单）

```
Shadowsocksr-Client-Linux/
├── index.html                    # Vite 入口，挂载点 #app
├── package.json                  # version 0.1.0（与 tauri/Cargo 的 0.2.0 不一致 ⚠️）
├── vite.config.js                # 端口 1420 strictPort，忽略 src-tauri 监听
├── note.md                       # 两条硬约束（见 §7）
├── README.md                     # 中英双语用户文档
│
├── src/                          # ===== 前端（Vue 3，约 1900 行）=====
│   ├── main.js                   #   7 行：createApp → use(i18n) → mount
│   ├── i18n.js                   #   i18n 实例 + 语言探测（localStorage → navigator）
│   ├── App.vue                   #   560 行：根组件 = 全局状态 + 页面布局 + 全局样式
│   ├── components/
│   │   ├── ConfigList.vue        #   249 行：左侧配置列表（读/删/选）
│   │   ├── ConfigForm.vue        #   552 行：新建/编辑配置表单
│   │   └── ProxyControl.vue      #   259 行：启用/停用代理开关面板
│   ├── utils/
│   │   └── api.js                #   68 行：全部 6 个 invoke 封装（唯一 IPC 出口）
│   ├── locales/                  #   7 个语言文件，各 100 行，结构必须完全一致
│   │   ├── zh-CN.js  zh-TW.js  zh-HK.js  en-US.js
│   │   ├── ja-JP.js  ko-KR.js  ru-RU.js
│   └── assets/vue.svg
│
├── src-tauri/                    # ===== 后端（Rust，约 750 行源码）=====
│   ├── tauri.conf.json           #   应用元信息、窗口、bundle、externalBin
│   ├── Cargo.toml                #   依赖清单
│   ├── capabilities/
│   │   ├── default.json          #   主窗口权限（shell/dialog/notification/store/opener）
│   │   └── desktop.json          #   linux 平台 window-state 权限
│   ├── binaries/                 #   ⚠️ 3 个预编译 ELF sidecar（约 7.4MB，已入 git）
│   │   ├── ssr-native-client-x86_64-unknown-linux-gnu
│   │   ├── ssr-native-client-i686-unknown-linux-gnu
│   │   └── ssr-native-client-aarch64-unknown-linux-gnu
│   ├── src/
│   │   ├── main.rs               #   12 行：设 WEBKIT_DISABLE_DMABUF_RENDERER=1 → run()
│   │   ├── lib.rs                #   215 行：Tauri Builder + 插件 + 6 个 command 定义
│   │   ├── model.rs              #   82 行：全部数据结构（传输模型 + 文件模型）
│   │   ├── get_path.rs           #   21 行：配置目录路径（~/.ssr 或 /root/.ssr）
│   │   ├── insert_cfg_file.rs    #   58 行：写配置文件
│   │   ├── remove_cfg_file.rs    #   17 行：删配置文件
│   │   ├── get_cfg.rs            #   74 行：列配置 / 读单个配置
│   │   ├── ssr_process.rs        #   58 行：sidecar 进程生命周期（全局单例）
│   │   ├── proxy.rs              #   198 行：桌面环境识别 + 写系统代理
│   │   └── notification.rs       #   24 行：notify-send 发系统通知
│   ├── icons/                    #   各尺寸图标
│   ├── target/                   #   构建产物（gitignore）
│   └── gen/                      #   Tauri 生成的 schema（gitignore）
│
├── dist/                         # vite 构建产物（gitignore，本地存在）
└── node_modules/
```

**代码量分布**（不含生成物/图标/lock）：

| 位置 | 行数 | 说明 |
|---|---|---|
| `src/*.vue` + `src/utils` + `src/i18n` | ~1750 | 前端逻辑+样式（其中样式约一半） |
| `src/locales/*` | 700 | 7 × 100 行 |
| `src-tauri/src/*.rs` | ~750 | 后端全部逻辑 |

---

## 3. 分层架构与运行模型

```
┌─────────────────────────── Tauri 进程 ────────────────────────────┐
│                                                                    │
│  ┌─── WebView（前端）──────────────────────────────────────────┐   │
│  │  main.js → App.vue（全局状态容器）                          │   │
│  │     ├── ConfigList.vue   ──┐                                │   │
│  │     ├── ConfigForm.vue   ──┼──→ utils/api.js ── invoke() ───┼──┐│
│  │     └── ProxyControl.vue ──┘                                │││
│  │  状态：activeConfig / proxyEnabled / showForm 等（纯内存 ref）│││
│  └─────────────────────────────────────────────────────────────┘││
│                                                                  ││
│  ┌─── Rust 后端 ────────────────────────────────────────────────┤│
│  │  lib.rs 的 6 个 #[tauri::command]  ←────────────────────────┘│
│  │     ├── get_cfg.rs / insert_cfg_file.rs / remove_cfg_file.rs │
│  │     │      └── get_path.rs → ~/.ssr/<name>.json  （磁盘）     │
│  │     └── ssr_process.rs ── GLOBAL_PROC 单例                    │
│  │            ├── shell().sidecar("ssr-native-client")          │
│  │            │        └── spawn: ssr-native-client -c <cfg> -d │
│  │            └── proxy.rs → gsettings / kwriteconfig5 / dbus   │
│  │                   └── notification.rs → notify-send          │
│  └───────────────────────────────────────────────────────────────┘
│                          │
└──────────────────────────┼────────────────────────────────────────┘
                           ▼
              外部进程 ssr-native-client（sidecar）
              监听 0.0.0.0:<listen_port> 提供 SOCKS5
```

### 进程与状态的真实归属（重构时最容易踩的点）

| 状态 | 存在哪 | 生命周期 | 是否可恢复 |
|---|---|---|---|
| 代理是否在运行 | 后端 `GLOBAL_PROC` 静态变量 | 进程内 | **否**，前端重启后只能靠猜 |
| 当前选中的配置 | 前端 `App.vue` 的 `activeConfig` ref | 页面内存 | **否**，刷新即丢 |
| 系统代理设置 | GNOME gsettings / KDE kioslaverc | **系统级，跨进程持久** | 是（但也因此可能残留脏状态） |
| 配置文件 | `~/.ssr/*.json` | 磁盘 | 是 |
| 语言偏好 | 浏览器 `localStorage['ssr-client-language']` | WebView 持久 | 是 |
| 窗口位置/尺寸 | `tauri-plugin-window-state` | 持久 | 是 |
| 代理运行状态查询 | **不存在此接口** | — | ⚠️ 缺失 |

---

## 4. 构建 / 运行 / 调试命令

```bash
npm install                 # 装依赖（node 22 / npm 10 已验证）
npm run dev                 # 只起前端 vite（127.0.0.1:1420，strictPort）
npm run build               # 只构建前端 → dist/（已验证通过）
npm run tauri dev           # 完整开发模式（先 npm run dev，再编译 Rust）
npm run tauri build         # 完整发布构建 → src-tauri/target/release/bundle/{deb,rpm}
```

交叉编译（README 中的既定流程）：

```bash
rustup target add i686-unknown-linux-gnu       # 或 aarch64-unknown-linux-gnu
npm run tauri build -- --target i686-unknown-linux-gnu
```

**环境要求**
- Linux + `gtk3` + `webkit2gtk4.1`（运行时依赖）
- Rust toolchain（cargo/rustc 已在 `~/.cargo/bin`）
- NVIDIA 驱动下需要 `WEBKIT_DISABLE_DMABUF_RENDERER=1`，已在 `src-tauri/src/main.rs:8` 硬编码
- 无 `cargo test` / `npm test` 可跑——**目前没有自动化测试**

**Tauri 配置要点**（`src-tauri/tauri.conf.json`）
- productName `shadowsocksr-client-linux`，identifier `com.liangzhaoyuan12.shadowsocksr-client-linux`
- 窗口 800×600，单窗口 `main`
- `bundle.targets: ["deb","rpm"]`，`bundle.externalBin: ["binaries/ssr-native-client"]`
- `beforeBuildCommand: npm run build`，`frontendDist: ../dist`
- `security.csp: null`（未设 CSP）

---

## 5. 前端架构详解

### 5.1 组件树与职责

```
App.vue  ← 唯一的状态容器（无 Vuex/Pinia、无 vue-router）
├── header：标题 + 语言下拉框（7 种语言，点击外部关闭）
├── aside.sidebar
│   ├── ConfigList   props: activeConfig
│   │                emits: select / edit / refresh
│   │                父组件通过 ref 调用 .refresh()（defineExpose）
│   └── "+ 新建配置" 按钮
├── section.content-area
│   ├── [showForm=true]  ConfigForm  props: editMode, configName
│   │                                emits: saved / cancelled
│   └── [showForm=false] dashboard
│       ├── ProxyControl props: activeConfig, proxyEnabled
│       │                emits: status-changed(bool)
│       └── 两张 info card：使用步骤 + 本地代理地址 127.0.0.1:port
└── footer
```

### 5.2 全局状态（全部集中在 `src/App.vue:11-17`）

| ref | 类型 | 含义 | 被谁改 |
|---|---|---|---|
| `activeConfig` | `string\|null` | 当前选中的配置名 | handleSelectConfig / disableProxyAndSelect / handleProxyStatusChanged(false) |
| `proxyEnabled` | `boolean` | 代理是否启用（**仅前端记忆**） | handleProxyStatusChanged |
| `showForm` | `boolean` | 是否显示表单而非仪表盘 | handleAddNew / handleEditConfig / handleFormSaved / handleFormCancelled |
| `editMode` | `boolean` | 表单是编辑还是新建 | 同上 |
| `editingConfig` | `string` | 正在编辑的配置名 | 同上 |
| `configListRef` | template ref | 调子组件 `refresh()` | handleFormSaved |
| `activeConfigPort` | `number` | 展示用本地端口，默认 1080 | updateActiveConfigPort |

子组件**自带**局部状态（loading / error / success / 自己的数据列表），父子只通过 props + emits 通信，**没有共享 store**。跨组件刷新靠 `defineExpose({refresh})` 和 `emit("refresh")`（后者父组件接的是空函数 `() => {}`，见 §8 问题 12）。

### 5.3 前端 ↔ 后端 IPC 契约（`src/utils/api.js`）

**唯一出口**是 `invoke()`。共 6 个命令，全部是 **JSON 字符串进、JSON 字符串出** 的双层封装：

| 前端函数 | invoke 命令 | 入参 | 成功返回（已 parse 一层） | 失败行为 |
|---|---|---|---|---|
| `insertConfig(config)` | `insert_cfg_file` | `{ data: JSON.stringify(config) }` | `{status_type,msg}` | **reject 原始 JSON 字符串** ⚠️ |
| `removeConfig(cfgName)` | `remove_cfg_file` | `{ cfgName }` | 同上 | 同上 ⚠️ |
| `getConfigList()` | `get_cfg_list` | — | `JSON.parse(msg).cfg_list` → `string[]` | 抛 `Error("Failed to get config list")` |
| `getConfigInfo(cfgName)` | `get_cfg_info` | `{ cfgName }` | `JSON.parse(msg)` → 配置对象 | 抛 `Error("Failed to get config info")` |
| `enableProxy(cfgName)` | `enable_proxy` | `{ cfgName }` | `{status_type,msg}` | **reject 原始 JSON 字符串** ⚠️ |
| `disableProxy()` | `disable_proxy` | — | 同上 | 同上 ⚠️ |

**双层 JSON 封装示意**（成功路径）：

```jsonc
// Rust 侧实际发出的字符串
"{\"status_type\":\"success\",\"msg\":\"{\\\"cfg_list\\\":[\\\"a\\\",\\\"b\\\"]}\"}"
//                         ↑ 外层信封                          ↑ msg 本身又是一个 JSON 字符串
```

前端 `getConfigList` 做 `JSON.parse(response)` 再 `JSON.parse(data.msg)`，共两次。
**失败路径**：Rust `Err(字符串)`，`invoke` reject 出来的 `err.message` 就是那串未解析的 JSON——组件里 `error.value = err.message` 会把 `{"status_type":"failure",...}` 原样显示给用户。

### 5.4 配置表单字段（`ConfigForm.vue` 的 `formData` 结构，即后端入参）

```js
{
  cfg_name: "",                 // 只允许 /^[a-zA-Z]+$/，编辑时禁用
  password: "",
  method: "aes-128-ctr",        // 9 选 1（写死数组 methods）
  protocol: "auth_aes128_md5",  // 7 选 1（写死数组 protocols）
  protocol_param: "",
  obfs: "tls1.2_ticket_auth",   // 5 选 1（写死数组 obfsList）
  obfs_param: "",
  udp: true,
  idle_timeout: 300,
  connect_timeout: 6,
  udp_timeout: 6,
  client_settings: {
    server: "",
    server_port: 443,
    listen_address: "0.0.0.0",  // ⚠️ 前端写死，后端不校验
    listen_port: 1080
  }
}
```

前端校验（`ConfigForm.vue:98-111`）：`cfg_name` 匹配字母正则、`server` 非空、`password` 非空。其余字段无校验，直接透传。

### 5.5 样式与主题约定

- **设计变量**全部定义在 `App.vue` 的非 scoped `<style>` 的 `:root` 中（`--primary-color`、`--card-bg`、`--text-primary`、`--btn-*` 等约 30 个 CSS 变量）
- 深色模式：`@media (prefers-color-scheme: dark)` 覆盖 `:root`（`App.vue:283-303`），**跟随系统，无手动切换**
- 布局：`.main-layout` 为 `grid-template-columns: 320px 1fr`，`max-width: 1400px`；`≤768px` 降级为单列
- 各组件 `<style scoped>` 只写自己局部，引用全局 CSS 变量
- 所有颜色都必须走 `var(--xxx)`，禁止硬编码（现有代码里 error/success 提示条是硬编码的 `#fee/#efe`）

### 5.6 国际化

- 7 个 locale 文件结构**必须逐键一致**（`app / common / configList / configForm / proxyControl / dashboard / footer` 7 个顶层命名空间，各文件 100 行）
- `i18n.js`：优先读 localStorage，再按 `navigator.language` 映射，`fallbackLocale: 'en-US'`
- 数组型文案用 `tm('dashboard.steps')` 取原始对象
- 语言切换在 `App.vue:changeLanguage`，同时写 localStorage
- **改文案时必须同步 7 个文件，目前没有任何一致性检查**

---

## 6. 后端架构详解

### 6.1 模块与依赖关系

```
main.rs ──→ shadowsocksr_linux_lib::run()
                 │
lib.rs ──┬─ model.rs            (数据结构，被所有模块 use)
         ├─ get_path.rs         (home_dir ← nix::Uid / dirs::home_dir)
         ├─ insert_cfg_file.rs ─→ get_path
         ├─ remove_cfg_file.rs ─→ get_path
         ├─ get_cfg.rs ─────────→ get_path, model
         ├─ ssr_process.rs ─────→ get_path, model, proxy, tauri_plugin_shell
         ├─ proxy.rs ───────────→ notification
         └─ notification.rs     (notify-send)
```

### 6.2 6 个 Tauri 命令（`lib.rs:51-57` 注册）

| 命令 | 签名 | 实际逻辑 | 返回 |
|---|---|---|---|
| `insert_cfg_file` | `(data: &str) -> Result<String,String>` | 解析 JSON → 剥掉 `cfg_name` → 写 `~/.ssr/<name>.json`（覆盖） | Ok(信封) / Err(信封字符串) |
| `remove_cfg_file` | `(cfg_name: &str) -> Result<String,String>` | 删文件，不存在则 Err | 同上 |
| `get_cfg_list` | `() -> Result<String,String>` | 扫 `~/.ssr/` 下所有 `.json`，去掉 5 字符后缀 → `{"cfg_list":[...]}` | 同上 |
| `get_cfg_info` | `(cfg_name: &str) -> Result<String,String>` | 读文件 → 反序列化 → 补回 `cfg_name` 字段 → 序列化 | 同上 |
| `enable_proxy` | `(app: AppHandle, cfg_name: &str) -> Result<String,String>` | 校验文件存在 → 确认无实例 → 读 `listen_port` → spawn sidecar → 写系统代理 | 同上 |
| `disable_proxy` | `() -> Result<String,String>` | kill sidecar → 关系统代理 | 同上 |

**所有命令都是「同步逻辑包在 async fn 里」**，且错误信息是写死的英文常量，后端 `anyhow` 的具体错误（`the config file doesn't exist` 等）**从未传到前端**，被 `Err("failed to ...")` 覆盖。

### 6.3 数据模型（`model.rs`）

```rust
status_response { status_type: status_type, msg: String }   // 通用信封
enum status_type { success, failure }

ShadowsocksConfigReceive   // 传输模型：多一个 cfg_name 字段（前端 ↔ 后端）
ShadowsocksConfig          // 文件模型：写进磁盘的 JSON（无 cfg_name）
ClientSettings { server, server_port, listen_address, listen_port }
ShadowSocksConfigList { cfg_list: Vec<String> }
```

**磁盘上的配置文件格式**（`~/.ssr/<cfg_name>.json`，`to_string_pretty`）：

```json
{
  "password": "…",
  "method": "aes-128-ctr",
  "protocol": "auth_aes128_md5",
  "protocol_param": "",
  "obfs": "tls1.2_ticket_auth",
  "obfs_param": "",
  "udp": true,
  "idle_timeout": 300,
  "connect_timeout": 6,
  "udp_timeout": 6,
  "client_settings": {
    "server": "1.2.3.4",
    "server_port": 443,
    "listen_address": "0.0.0.0",
    "listen_port": 1080
  }
}
```

> ⚠️ **这个文件同时是 `ssr-native-client -c <file>` 的输入**，即它必须符合 shadowsocksr-native 期望的 schema。重构时**不能随意改字段名/结构**，除非同步升级/patch sidecar。

### 6.4 配置存储位置（`get_path.rs`）

```rust
root 用户  → /root/.ssr
普通用户   → $HOME/.ssr
```

用 `nix::unistd::Uid::effective().is_root()` 判断。目录不存在时在 `insert`/`get_cfg_list` 中 `create_dir_all` 创建。

### 6.5 sidecar 进程管理（`ssr_process.rs`）

```rust
static GLOBAL_PROC: OnceCell<Mutex<Option<CommandChild>>>   // 全局单例，仅允许 1 个实例
```

- `enable(cfg_name, app)`：
  1. 校验 `~/.ssr/<cfg_name>.json` 存在，否则 `Err("the config file doesn't exist")`
  2. 拿锁，若已有实例 → `Err("ssr client has been already run")`
  3. 读配置拿 `client_settings.listen_port`
  4. `app.shell().sidecar("ssr-native-client")?.arg("-c").arg(path).arg("-d").spawn()`
  5. 存 child 到全局 → `proxy::enable(port).await`
- `disable()`：锁 → 无实例则 `Err("ssr client isn't running")` → `child.kill()` → `proxy::disable().await`
- `_rx`（stdout/stderr 接收器）被立即丢弃并绑定为 `_rx`，**sidecar 的输出与退出事件完全没人监听**——进程异常退出时后端不知情。

参数说明：`-c <配置文件路径>` 指定配置，`-d` 为守护/后台模式（shadowsocksr-native 的参数）。

### 6.6 系统代理写入（`proxy.rs`）

按 `whoami::desktop_env()` 分派：

| 桌面环境 | 处理方式 | 使用的外部命令 |
|---|---|---|
| Cinnamon / Gnome / Ubuntu / Mate / Cosmic | GNOME 路径 | `gsettings set org.gnome.system.proxy mode manual`、`…proxy.socks host 127.0.0.1`、`…port <p>` |
| Plasma / `Unknown("KDE")` | KDE 路径 | `kwriteconfig5 --file kioslaverc --group "Proxy Settings" --key ProxyType 1`、`socksProxy "127.0.0.1 <p>"`、`NoProxyFor "localhost,127.0.0.1,::1"`，再 `dbus-send … org.kde.KIO.Scheduler.reparseSlaveConfiguration` |
| `Unknown("deepin")` / `Unknown("uos")` | 走 GNOME 路径 | 同 GNOME |
| 其他 / 检测不到 | 仅发通知提示手动配置 | — |

关闭时：GNOME `mode=none` + `reset` host/port；KDE `ProxyType=0` + dbus 广播。

**关键行为**：`enable()` / `disable()` 返回类型是 `()`（无错误），成败只通过 `notification::send(...)` 告知用户。因此 `enable_proxy` 命令在「sidecar 起来了但系统代理写失败」时仍返回 success。

### 6.7 通知（`notification.rs`）

`tokio::process::Command::new("notify-send") --app-name shadowsocksr-client-linux <title> <body>`，`tokio::spawn` 异步发出（`.status().await.unwrap()`，**notify-send 不存在时会在 spawned task 里 panic**）。非 Linux 平台是空实现。

> 注意：后端注册了 `tauri_plugin_notification`，但实际用的是外部 `notify-send` 命令；两者并存。

### 6.8 生命周期钩子（`lib.rs:20-43`）

拦截主窗口 `CloseRequested` → `api.prevent_close()` → 异步执行 `ssr_process::disable()`（错误忽略）→ `drop(window)` → `app_handle.exit(0)`。
即：**关闭窗口时会杀 sidecar 并关系统代理**（`disable()` 在无实例时返回 Err，被 `let _` 忽略，此时 `proxy::disable` 不会执行——所以「没开代理就关窗」不会动系统代理设置）。

已注册插件：`window-state`、`store`、`single-instance`、`notification`、`dialog`、`shell`、`opener`。
**前端 `src/` 中没有任何 `@tauri-apps/plugin-*` 的 import** —— `dialog`（用的是原生 `confirm()`）、`notification`、`store`（用的是 `localStorage`）、`opener` 对前端而言都是**已授权但未使用**的权限。

---

## 7. 关键约束与不变量（重构时不可破坏）

1. **配置文件 schema 是外部契约**：`~/.ssr/<name>.json` 必须保持 shadowsocksr-native 可解析的字段（§6.3），改字段名需同步改 sidecar 侧。
2. **`cfg_name` 只允许大小写字母**（前端正则 `^[a-zA-Z]+$`，`ConfigForm.vue:98`），它直接拼进文件路径。
3. **`listen_address` 必须写死 `0.0.0.0`**（`note.md` + `insert_cfg_file.rs:22` 注释：由前端保证，后端不校验）。
4. **sidecar 同一时刻只允许一个实例**（`GLOBAL_PROC` 单例）。
5. **sidecar 二进制名固定为 `ssr-native-client`**，必须与 `tauri.conf.json` 的 `bundle.externalBin: ["binaries/ssr-native-client"]` 及 `src-tauri/binaries/ssr-native-client-<target-triple>` 三处一致；改名会破坏打包。
6. **仅支持自动系统代理的桌面环境**：GNOME 系（含 Cinnamon/MATE/Cosmic/deepin/uos）与 KDE Plasma；其余（XFCE/LXQt）只发通知要求手动配置。
7. **本地 SOCKS5 地址恒为 `127.0.0.1`**，端口取配置的 `listen_port`（默认 1080），UI 展示用 `activeConfigPort`。
8. **窗口只有一个 `main`**，capabilities 里 `windows: ["main"]`；新增窗口要同步改 capabilities。
9. **7 个 locale 文件键结构必须完全一致**，否则 vue-i18n 会静默 fallback 到 en-US。
10. **退出时必须清理**：关窗 → 杀 sidecar + 关系统代理（否则系统代理会指向一个死端口）。
11. 构建时 `externalBin` 要求当前 target 三元组对应的二进制存在，否则 `tauri build` 失败。

---

## 8. 已知问题与重构热点（按优先级）

> 以下均为在源码中确认的问题，可直接作为重构 backlog。标注 `文件:行号`。

**P0 — 正确性 / 安全**

1. **后端无 `cfg_name` 校验 → 路径穿越**。`insert_cfg_file.rs:35` 与 `remove_cfg_file.rs:10-11` 直接 `format!("{}.json", name)` 拼路径，后端信任前端正则。恶意/异常入参可写删任意 `.json`。**信任边界应在 Rust 侧。**
2. **错误信息格式不统一，UI 显示原始 JSON**。`api.js` 的 `insertConfig`/`removeConfig`/`enableProxy`/`disableProxy`（8-11/18-21/56-59/65-68 行）不做错误处理，失败时 reject 的是未解析的 JSON 字符串，组件 `err.message` 会原样展示 `{"status_type":"failure","msg":"failed to ..."}`。
3. **无「查询代理运行状态」命令**。`proxyEnabled` 只在前端内存里（`App.vue:12`），刷新页面即与真实状态脱钩；崩溃/强杀后系统代理仍指向失效端口。**建议新增 `get_proxy_status` 命令并在启动时对账。**
4. **`proxy::enable/disable` 吞掉错误**（`proxy.rs:6,53` 返回 `()`）。系统代理写失败时 `enable_proxy` 仍返回 success，UI 显示「连接成功」但系统代理没生效。
5. **sidecar 的 stdout/stderr/退出事件无人监听**（`ssr_process.rs:40`，`_rx` 立即丢弃）。sidecar 崩溃 → UI 仍显示已连接。应订阅 `rx` 的 exit 事件并回推状态到前端（Tauri event）。

**P1 — 架构**

6. **状态全部堆在 `App.vue`（560 行）**，没有 store。配置列表、表单、代理状态三者通过 props/emits/ref 手工接力（`configListRef.refresh()` 这种命令式调用）。重构建议引入 Pinia 或至少 composable（`useConfigs` / `useProxy`）。
7. **`utils/api.js` 的双层 JSON 字符串封装可去掉**。Rust command 直接 `Result<T, E>` 返回 serde 结构体即可，Tauri 2 自动编解码，省掉两层 `JSON.parse` 和信封结构。
8. **错误被常量覆盖**：`lib.rs:99,117,141,162,197,213` 一律 `Err("failed to …")`，后端 `anyhow` 的真实原因（`the config file doesn't exist` / `ssr client has been already run`）全部丢失，前端无法区分「文件不存在」和「已在运行」。
9. **系统代理配置不保存原值**：关闭时只是 `mode=none` / `ProxyType=0`，若用户原本已有代理设置会被破坏。建议 enable 前快照、disable 时还原。
10. **`proxy.rs` 有 4 处重复的 match 分支**（`proxy.rs:11-48` 与 `56-93` 几乎相同），应抽成「环境 → 策略」表驱动。
11. **桌面环境识别脆弱**：`Unknown(val) if val=="KDE"` / `"deepin"` / `"uos"` 字符串硬匹配，未识别环境静默降级为只发通知。

**P2 — 代码质量 / 工程**

12. **`ConfigList` 的 `refresh` emit 是空函数**：`App.vue:189` 传 `@refresh="() => {}"`，实际刷新靠 `configListRef.refresh()`，两条路径冗余。
13. **注册但未使用的插件与权限**：`store`、`dialog`、`opener`、`notification`（前端）在 `src/` 里零引用，但都在 `capabilities/default.json` 授权。可精简，或改用它们替代 `confirm()`/`localStorage`/`notify-send`。
14. **`notification.rs:16` 的 `.unwrap()`** 会在 `notify-send` 缺失的发行版上让 spawned task panic。
15. **版本号不一致**：`package.json` = 0.1.0，`tauri.conf.json` / `Cargo.toml` = 0.2.0。
16. **无测试、无 Lint、无 CI**。重构前建议先补：Rust 侧对 `get_cfg`/`insert`/`remove` 的单元测试（可 tempdir 隔离），前端至少加 ESLint + 一个 `api.js` 的 mock 测试。
17. **7 份手写 locale 无一致性校验**，加一条 CI 脚本比对键集合成本极低。
18. **`status_response::to_string`（`model.rs:29`）用 `unwrap()`**；`insert_cfg_file.rs:56` 的 `File::create` 后 `write` 未 flush 检查长度。
19. **`get_cfg_list` 会把 `~/.ssr` 下任意 `.json` 当配置**（`get_cfg.rs:31`），反序列化失败时 `get_cfg_info` 报错但列表仍显示。
20. **样式硬编码**：`error-message`/`success-message` 用 `#fee/#c33/#efe/#3c3`，绕过了 CSS 变量体系；深色模式下刺眼。
21. **前端无 loading 骨架 / 无请求并发控制**，快速切换配置可能产生竞态（`updateActiveConfigPort` 异步覆盖端口）。

---

## 9. 三条核心数据流（重构后应保持等价）

**A. 新建/编辑配置**
```
用户填表 → ConfigForm.handleSubmit（本地校验）
  → insertConfig(formData) → invoke("insert_cfg_file", {data: JSON.stringify(...)})
  → Rust: serde 解析 → 剥 cfg_name → 写 ~/.ssr/<name>.json（覆盖）
  → 信封 success → 前端 1s 后 emit("saved") → App.handleFormSaved
  → configListRef.refresh() + updateActiveConfigPort()
```

**B. 启用代理**
```
用户点「启用代理」→ ProxyControl.handleEnable
  → enableProxy(activeConfig) → invoke("enable_proxy", {cfgName})
  → Rust ssr_process::enable：
      校验文件存在 → 拿全局锁（已有实例则 Err）→ 读 listen_port
      → spawn sidecar: ssr-native-client -c ~/.ssr/<name>.json -d
      → 存 GLOBAL_PROC → proxy::enable(port)
          → 识别桌面环境 → gsettings / kwriteconfig5+dbus → notify-send
  → 信封 success → emit("status-changed", true) → proxyEnabled = true
```

**C. 停用代理 / 关闭窗口**
```
用户点「停用」 → disableProxy() → invoke("disable_proxy")
  → ssr_process::disable：拿锁 → child.kill() → proxy::disable()（还原/关闭系统代理）→ 通知
  → emit("status-changed", false)，activeConfig 置 null，端口回落 1080

关闭窗口 → CloseRequested → prevent_close → spawn：
  ssr_process::disable()（错误忽略）→ drop(window) → app_handle.exit(0)
```

---

## 10. 重构方向建议（供起点，非指令）

若做分层重构，可按此目标结构推进（保持 §7 全部不变量与 §9 三条流程等价）：

```
src/
├── App.vue                    # 只留布局与路由式切换（拆掉状态与大段样式）
├── stores/                    # Pinia：configs.js / proxy.js / settings.js
├── composables/               # 可选：useConfigs / useProxy / useI18nLocale
├── components/
│   ├── layout/                # AppHeader / Sidebar / Footer
│   ├── config/                # ConfigList / ConfigForm / ConfigCard
│   ├── proxy/                 # ProxyControl / StatusIndicator
│   └── common/                # ErrorBanner / ConfirmDialog / LoadingState
├── services/
│   ├── ipc.js                 # invoke 薄封装，统一信封解析与错误类型
│   └── errors.js              # 后端错误 → 本地化文案映射
├── styles/                    # variables.css（拆出 App.vue 的 :root）
└── locales/                   # 建议改单文件 per locale + 一致性校验脚本

src-tauri/src/
├── commands/                  # 每个 command 一个文件，统一 Result<T, AppError>
├── error.rs                   # thiserror 定义，序列化后直接透传给前端
├── config/                    # repo/store：路径 + 读写 + cfg_name 校验
├── process/                   # ssr sidecar：状态机 + 退出事件 → emit 到前端
├── sysproxy/                  # 策略表驱动 GNOME/KDE/其他 + 原值快照还原
├── notify.rs
└── model.rs
```

**推荐顺序**：① 先加错误类型统一 + `get_proxy_status`（修 P0）→ ② 再拆 Rust 模块（`sysproxy` 表驱动 + 快照）→ ③ 引入 Pinia 拆 `App.vue` → ④ 去掉双层 JSON 封装 → ⑤ 补测试与 CI。

---

## 11. 验收 / 回归检查清单（每轮重构后跑一遍）

```bash
npm run build                 # 前端可构建
cargo check                   # 在 src-tauri/ 下，Rust 可编译
npm run tauri dev             # 能启动、窗口 800x600、中文/英文切换正常
```

手动回归：
- [ ] 首次启动（`~/.ssr` 不存在）→ 自动建目录、列表显示空态
- [ ] 新建配置（字母名）→ 列表出现；非法名（数字/符号）被前端拦截
- [ ] 编辑配置 → 表单回填正确、保存后列表刷新、`cfg_name` 不可改
- [ ] 删除配置 → confirm → 消失；文件确实从 `~/.ssr` 删除
- [ ] 选中配置 → 启用代理 → sidecar 进程存在（`pgrep -f ssr-native-client`）
- [ ] GNOME：`gsettings get org.gnome.system.proxy mode` = `manual`；KDE：`kreadconfig5` 检查 ProxyType=1
- [ ] 停用代理 → 进程消失 + 系统代理还原
- [ ] 代理运行中直接关窗 → 进程被杀、系统代理关闭、无残留
- [ ] 重复启用 → 返回明确错误（而非重复 spawn）
- [ ] 7 种语言逐个切换，无 key 缺失显示为原始 key
- [ ] 深色/浅色主题（跟随系统）下无刺眼硬编码色块
- [ ] `npm run tauri build` → `deb`/`rpm` 产物内含 sidecar 且可运行

---

## 12. 术语表

| 术语 | 含义 |
|---|---|
| 配置 / cfg | 一个 SSR 节点配置，磁盘上是 `~/.ssr/<cfg_name>.json` |
| `cfg_name` | 配置名，也是文件名（不含 `.json`），仅字母 |
| sidecar | 随应用打包的外部可执行文件，此处为 `ssr-native-client` |
| 信封 / envelope | `{status_type, msg}` 二层 JSON 包装格式 |
| 系统代理 | 桌面环境级 SOCKS 代理设置（gsettings / kioslaverc） |
| `listen_port` | 本地 SOCKS5 监听端口，由 sidecar 打开，默认 1080 |
| `server_port` | 远端 SSR 服务器端口 |
| `GLOBAL_PROC` | 后端保存 sidecar `CommandChild` 的全局单例锁 |
