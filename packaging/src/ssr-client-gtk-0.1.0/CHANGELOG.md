# Changelog

## 0.1.0 — 2026-09-26

首个发布 / first release.

### 新增 / Added

- GTK4 + libadwaita 原生界面，进程内链接 `ssr-client-rs`（无 sidecar）
  / native UI with the `ssr-client-rs` core linked in-process (no sidecar)
- 配置管理：新建 / 编辑 / 删除（确认对话框），配置名仅 `^[a-zA-Z]+$`，
  编辑时名称冻结；损坏的配置文件在列表行内显示错误、不影响其余行
  / profile CRUD with `AdwAlertDialog` confirmation, name frozen on edit,
  corrupt files show an inline error row instead of breaking the list
- 一键启用 / 停用：单端口 SOCKS5 监听（`0.0.0.0:<listen_port>`），
  启用期本进程只有一个 TCP 监听口
  / one-click enable & disable with exactly one TCP listener while running
- 系统代理快照与精确还原（KDE Plasma / GNOME 系），快照持久化，
  异常退出下次启动自动还原；关窗顺序固定为停代理 → 还原系统代理 → 退出
  / system-proxy snapshot with exact restore (KDE / GNOME), persisted across
  crashes; close = stop proxy → restore system proxy → quit
- 中英双语界面，语言选择持久化，首启按 `LC_ALL`/`LANG` 回退
  / zh-CN & en-US UI, persisted choice, locale-based fallback on first run
- 四种打包：deb、rpm、Arch（PKGBUILD）、tar.gz

### 变更 / Changed

- `ssr_service_port`（旧版双端口遗留键）读取时忽略、保存时不写回、UI 不显示
  / legacy `ssr_service_port` is ignored on read, never written back, never shown
- 监听地址写死 `0.0.0.0`，UI 不提供修改（代价说明见 README）
  / listener address is fixed to `0.0.0.0` by design (see README)

### 修复 / Fixed

- 相对旧 Tauri 版（ARCHITECTURE.md §8）：路径穿越、双端口、错误吞掉、
  快照不还原、`confirm()` 原生弹窗等问题均不存在于本实现
  / the Tauri-era issues in ARCHITECTURE.md §8 (path traversal, double port,
  swallowed errors, unrestored snapshot, native `confirm()`) do not exist here
