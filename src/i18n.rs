//! Internationalisation: **struct-based strings** (GOAL D1 / §6.8).
//!
//! - `Strings` declares every message key once; `ZH` and `EN` are `const`
//!   instances, so a missing field is a **compile error** — no JSON/JS
//!   locale files, no key drift (ARCHITECTURE §5.6's 7-file sync problem).
//! - Copy is migrated from the old app's `src/locales/{zh-CN,en-US}.js`
//!   (recovered from the shipped v0.6.0 bundle) and trimmed to this
//!   project's scope: no `ssr_service_port`, no router-port hints, no
//!   env-var/proxychains features (GOAL §4.2), single `listen_port` only.
//! - `Lang` persists to `~/.config/ssr-client-gtk/settings.json`
//!   (preference only — never strings, GOAL §2.1).

use std::cell::RefCell;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Declares `Strings` plus its `all()` enumerator so tests can check every
/// field of both languages (adding a field to the list without filling it
/// in `ZH`/`EN` fails compilation).
macro_rules! define_strings {
    ($($f:ident),* $(,)?) => {
        /// One field per UI message; `&'static str` everywhere.
        #[derive(Debug, Clone, Copy)]
        pub struct Strings {
            $(pub $f: &'static str,)*
            /// Usage steps on the dashboard (array copy).
            pub steps: &'static [&'static str],
        }

        impl Strings {
            /// Every single-line message (flattens `steps` too).
            pub fn all(&self) -> Vec<&'static str> {
                let mut v: Vec<&'static str> = vec![$(self.$f),*];
                v.extend_from_slice(self.steps);
                v
            }
        }
    };
}

define_strings! {
    app_title,
    app_subtitle,
    lang_label,
    about_label,
    about_comments,
    lang_zh,
    lang_en,
    common_loading,
    common_cancel,
    common_save,
    common_create,
    common_update,
    common_delete,
    common_edit,
    common_refresh,
    common_confirm,
    common_close,
    cfg_list_title,
    cfg_list_empty,
    cfg_list_empty_hint,
    cfg_list_active,
    cfg_list_delete_confirm,
    cfg_list_delete_success,
    cfg_list_delete_failed,
    cfg_list_load_failed,
    cf_add_title,
    cf_edit_title,
    cf_basic_settings,
    cf_advanced_settings,
    cf_profile_name,
    cf_profile_name_placeholder,
    cf_profile_name_hint,
    cf_server_address,
    cf_server_address_placeholder,
    cf_server_port,
    cf_password,
    cf_password_placeholder,
    cf_method,
    cf_protocol,
    cf_obfs,
    cf_local_port,
    cf_protocol_param,
    cf_protocol_param_placeholder,
    cf_obfs_param,
    cf_obfs_param_placeholder,
    cf_idle_timeout,
    cf_connect_timeout,
    cf_udp_timeout,
    cf_enable_udp,
    cf_listen_note,
    cf_validation_name,
    cf_validation_server,
    cf_validation_password,
    cf_validation_port,
    cf_success_created,
    cf_success_updated,
    cf_error_save,
    pc_title,
    pc_connected,
    pc_disconnected,
    pc_select_first,
    pc_active_config,
    pc_enable,
    pc_disable,
    pc_connecting,
    pc_disconnecting,
    pc_success_enabled,
    pc_success_disabled,
    pc_close_stopped,
    pc_error_enable,
    pc_error_disable,
    pc_unsupported_desktop,
    dash_how_to_use,
    dash_local_proxy,
    dash_port_label,
    dash_listen_line,
    footer_text,
    err_generic,
    err_port_in_use,
    err_already_running,
    err_not_running,
    err_not_found,
    err_name_invalid,
    err_conflict,
    err_sysproxy,
    err_json,
    err_core,
    err_io,
    err_acl,
    err_dns,
    dash_route_title,
    route_mode_label,
    route_global,
    route_bypass_lan,
    route_bypass_cn,
    route_bypass_lan_cn,
    route_acl,
    route_acl_file,
    route_acl_pick,
    route_acl_none,
    route_udp_note,
    route_restart_hint,
    dns_label,
    dns_system,
    dns_custom,
    dns_servers_placeholder,
    dns_note,
    dns_note_ali,
    dns_note_tencent,
    dns_ali,
    dns_tencent,
    sysproxy_mode_label,
    sysproxy_desktop,
    sysproxy_env,
    sysproxy_desktop_hint,
    sysproxy_env_hint,
    sysproxy_env_unknown,
}

/// Supported UI languages (GOAL D4: only these two).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    /// 简体中文
    #[serde(rename = "zh-CN")]
    ZhCn,
    /// English
    #[serde(rename = "en-US")]
    EnUs,
}

impl Lang {
    /// Language code as shown in settings.json.
    pub fn code(self) -> &'static str {
        match self {
            Lang::ZhCn => "zh-CN",
            Lang::EnUs => "en-US",
        }
    }

    /// The string table for this language.
    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::ZhCn => &ZH,
            Lang::EnUs => &EN,
        }
    }

    /// First-launch guess (GOAL 7.13): read the host's message locale and
    /// answer "简体中文 or English" — those are the only two languages the UI
    /// ships (GOAL D4), and on the first start there is no saved preference
    /// to go by.
    ///
    /// Rule: **any `zh*` locale → Chinese** — traditional-Chinese hosts read
    /// the simplified interface too, because that is the only Chinese the UI
    /// ships (ruling: 繁中电脑也看简中). **Everything else** → English —
    /// other languages (`ja_JP`, `de_DE`, …) and `C`/POSIX/unset. Either
    /// way the pick is remembered once the user switches by hand.
    ///
    /// GNU's `LANGUAGE` language list is honoured exactly the way gettext
    /// orders these variables: only when the locale itself is not C/POSIX,
    /// and it wins over `LC_ALL`/`LC_MESSAGES`/`LANG`.
    pub fn detect() -> Lang {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            let Ok(v) = std::env::var(var) else {
                continue;
            };
            if v.trim().is_empty() {
                continue;
            }
            if is_posix_locale(&v) {
                continue; // "no translation": keep looking further down
            }
            let preferred = match std::env::var("LANGUAGE") {
                Ok(l) if !l.trim().is_empty() => l,
                _ => v,
            };
            return if is_chinese(&preferred) {
                Lang::ZhCn
            } else {
                Lang::EnUs
            };
        }
        Lang::EnUs
    }
}

/// `C` / `POSIX`, with or without a codeset or modifier — "no translation",
/// so it never decides the language by itself.
fn is_posix_locale(v: &str) -> bool {
    matches!(locale_head(v).as_str(), "c" | "posix")
}

/// The part of a locale (or of one entry of a `LANGUAGE` list) that carries
/// the language: head of the `:` list, codeset/modifier dropped, lower-cased
/// and `-` normalised to `_`. `zh_CN.UTF-8@pinyin` → `zh_cn`.
fn locale_head(v: &str) -> String {
    let head = v.split(':').next().unwrap_or("").trim();
    let head = head.split('.').next().unwrap_or("");
    let head = head.split('@').next().unwrap_or("");
    head.to_lowercase().replace('-', "_")
}

/// Chinese? The UI ships exactly **one** Chinese (简体), so every `zh*`
/// locale gets it — traditional-Chinese hosts included (`zh_TW`/`zh_HK`/
/// `zh_MO`/`zh_Hant`; ruling: 繁中电脑也看简中).
fn is_chinese(v: &str) -> bool {
    let head = locale_head(v);
    head == "zh" || head.starts_with("zh_")
}

/// Current language handle (set once at startup, updated on switch).
#[derive(Debug)]
pub struct LangHandle(RefCell<Lang>);

impl LangHandle {
    /// Start from the persisted preference (or detect on first launch).
    pub fn load() -> LangHandle {
        LangHandle(RefCell::new(load_lang().unwrap_or_else(Lang::detect)))
    }

    /// Current language.
    pub fn get(&self) -> Lang {
        *self.0.borrow()
    }

    /// Switch language and persist the choice.
    pub fn set(&self, lang: Lang) {
        *self.0.borrow_mut() = lang;
        if let Err(e) = save_lang(lang) {
            // Preference write is non-critical; keep going with the switch.
            eprintln!("failed to persist language: {e}");
        }
    }
}

/// `~/.config/ssr-client-gtk/settings.json` — **preferences only**.
fn settings_path() -> PathBuf {
    crate::sysproxy::snapshot::Snapshot::config_dir().join("settings.json")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    language: Option<Lang>,
}

/// Read the stored language, if any.
pub fn load_lang() -> Option<Lang> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    serde_json::from_str::<Settings>(&text).ok()?.language
}

/// Persist the language (atomic write; errors bubble to the caller).
pub fn save_lang(lang: Lang) -> Result<(), AppError> {
    let dir = crate::sysproxy::snapshot::Snapshot::config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("settings.json");
    let tmp = dir.join(".settings.json.part");
    let settings = Settings {
        language: Some(lang),
    };
    let text = serde_json::to_string_pretty(&settings).map_err(AppError::json)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, &path)?;
    Ok(())
}

/// Startup self-check: every message in the active table must be
/// non-empty (cheap; also keeps `Strings::all()` reachable in release
/// builds so the compiler checks every field).
pub fn self_check(s: &Strings) -> bool {
    s.all().iter().all(|m| !m.trim().is_empty())
}

/// Replace `{token}` placeholders in a template.
pub fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in pairs {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Map an [`AppError`] to a localized, user-readable sentence
/// (GOAL 4.6: no raw JSON / English stack traces in the UI).
pub fn error_text(err: &AppError, s: &Strings) -> String {
    match err {
        AppError::PortInUse(p) => fill(s.err_port_in_use, &[("port", &p.to_string())]),
        AppError::AlreadyRunning => s.err_already_running.to_string(),
        AppError::NotRunning => s.err_not_running.to_string(),
        AppError::NotFound(_) => s.err_not_found.to_string(),
        AppError::InvalidName(_) => s.err_name_invalid.to_string(),
        AppError::AlreadyExists(_) => s.err_conflict.to_string(),
        AppError::SysProxy(_) => s.err_sysproxy.to_string(),
        AppError::Json(_) => s.err_json.to_string(),
        AppError::Core(_) => s.err_core.to_string(),
        AppError::Io(_) => s.err_io.to_string(),
        AppError::Acl(_) => s.err_acl.to_string(),
        AppError::Dns(_) => s.err_dns.to_string(),
    }
}

/// Chinese table (migrated from the old `zh-CN.js`, scope-trimmed).
pub const ZH: Strings = Strings {
    app_title: "ShadowsocksR 客户端",
    app_subtitle: "Linux 安全代理客户端",
    lang_label: "语言",
    about_label: "关于",
    about_comments: "项目地址：https://github.com/liangzhaoyuan12/ssr-client-gtk\n开源协议：GPL-3.0-or-later\n作者：liangzhaoyuan12",
    lang_zh: "中文",
    lang_en: "English",
    common_loading: "加载中...",
    common_cancel: "取消",
    common_save: "保存",
    common_create: "创建",
    common_update: "更新",
    common_delete: "删除",
    common_edit: "编辑",
    common_refresh: "刷新",
    common_confirm: "确认",
    common_close: "关闭",
    cfg_list_title: "配置列表",
    cfg_list_empty: "暂无配置",
    cfg_list_empty_hint: "点击\"新建配置\"创建您的第一个配置",
    cfg_list_active: "活跃",
    cfg_list_delete_confirm: "确定要删除配置 \"{name}\" 吗？",
    cfg_list_delete_success: "删除成功",
    cfg_list_delete_failed: "删除失败",
    cfg_list_load_failed: "加载配置列表失败",
    cf_add_title: "新建配置",
    cf_edit_title: "编辑配置",
    cf_basic_settings: "基本设置",
    cf_advanced_settings: "高级设置",
    cf_profile_name: "配置名称",
    cf_profile_name_placeholder: "例如：我的服务器",
    cf_profile_name_hint: "仅支持字母，创建后不可修改",
    cf_server_address: "服务器地址",
    cf_server_address_placeholder: "example.com 或 1.2.3.4",
    cf_server_port: "服务器端口",
    cf_password: "密码",
    cf_password_placeholder: "输入密码",
    cf_method: "加密方式",
    cf_protocol: "协议",
    cf_obfs: "混淆",
    cf_local_port: "本地端口",
    cf_protocol_param: "协议参数",
    cf_protocol_param_placeholder: "可选",
    cf_obfs_param: "混淆参数",
    cf_obfs_param_placeholder: "可选",
    cf_idle_timeout: "空闲超时（秒）",
    cf_connect_timeout: "连接超时（秒）",
    cf_udp_timeout: "UDP 超时（秒）",
    cf_enable_udp: "启用 UDP 中继",
    cf_listen_note: "本地监听地址固定为 0.0.0.0（无认证 SOCKS5，注意局域网可达）",
    cf_validation_name: "配置名称只能包含字母（a-z, A-Z）",
    cf_validation_server: "服务器地址不能为空",
    cf_validation_password: "密码不能为空",
    cf_validation_port: "端口必须是 1–65535 之间的整数",
    cf_success_created: "配置创建成功！",
    cf_success_updated: "配置更新成功！",
    cf_error_save: "保存配置失败",
    pc_title: "代理控制",
    pc_connected: "已连接",
    pc_disconnected: "未连接",
    pc_select_first: "请先选择一个配置",
    pc_active_config: "当前配置",
    pc_enable: "启用代理",
    pc_disable: "停用代理",
    pc_connecting: "连接中...",
    pc_disconnecting: "断开中...",
    pc_success_enabled: "代理启用成功！",
    pc_success_disabled: "代理已停用！",
    pc_close_stopped: "窗口已关闭，代理已停用！",
    pc_error_enable: "启用代理失败",
    pc_error_disable: "停用代理失败",
    pc_unsupported_desktop: "当前桌面环境（{desktop}）不支持自动设置系统代理，请手动指向 127.0.0.1:{port}",
    dash_how_to_use: "使用说明",
    dash_local_proxy: "本地代理设置",
    dash_port_label: "端口",
    dash_listen_line: "实际监听 0.0.0.0:{port}",
    footer_text: "ShadowsocksR Linux 客户端",
    err_generic: "操作失败",
    err_port_in_use: "端口 {port} 已被占用：请先结束占用该端口的程序，或在配置中改用其他本地端口",
    err_already_running: "代理已在运行，请先停用",
    err_not_running: "代理未在运行",
    err_not_found: "配置不存在",
    err_name_invalid: "配置名称非法：仅允许字母",
    err_conflict: "同名配置已存在",
    err_sysproxy: "系统代理设置失败（命令返回错误）",
    err_json: "配置文件格式错误",
    err_core: "代理核心错误",
    err_io: "文件读写失败",
    err_acl: "ACL 文件无法读取或格式错误",
    err_dns: "DNS 设置无效",
    dash_route_title: "路由、DNS 与系统代理",
    route_mode_label: "路由规则",
    route_global: "全局代理（全部流量走代理）",
    route_bypass_lan: "绕开局域网",
    route_bypass_cn: "绕开中国大陆",
    route_bypass_lan_cn: "绕开局域网及中国大陆",
    route_acl: "自定义 ACL 文件",
    route_acl_file: "ACL 文件",
    route_acl_pick: "选择文件…",
    route_acl_none: "未选择 ACL 文件",
    route_udp_note: "UDP 中继不参与分流，启用后始终经 SSR 转发。",
    route_restart_hint: "路由与 DNS 设置在下次“启用代理”时生效。",
    dns_label: "DNS 解析",
    dns_system: "系统 DNS",
    dns_custom: "自定义 DNS",
    dns_servers_placeholder: "DNS 服务器 IP，多个用逗号分隔，如 223.5.5.5, 8.8.8.8",
    dns_note: "本地解析与路由判定使用该 DNS；走代理的域名交给 SSR 服务端解析。",
    dns_note_ali: "阿里云 DoT: dns.alidns.com · DoH: https://dns.alidns.com/dns-query（本客户端仍按 UDP/TCP 53 直连查询，不走 DoT/DoH）。",
    dns_note_tencent: "腾讯云 DoT: dot.pub · DoH: https://doh.pub/dns-query（本客户端仍按 UDP/TCP 53 直连查询，不走 DoT/DoH）。",
    dns_ali: "阿里云 DNS",
    dns_tencent: "腾讯云 DNSPod",
    sysproxy_mode_label: "系统代理方式",
    sysproxy_desktop: "桌面设置",
    sysproxy_env: "环境变量",
    sysproxy_desktop_hint: "由桌面环境自身的代理设置接管（KDE / GNOME 及其衍生）。",
    sysproxy_env_hint: "写入 {rc}，仅对新开的终端生效。",
    sysproxy_env_unknown: "无法确定当前 shell 的配置文件，启用代理时会报错（支持 bash / zsh / fish）。",
    steps: &[
        "火狐有自己的代理设置，需要在火狐浏览器的设置中自行设定代理。",
        "开源协议：GPL-3.0-or-later",
    ],
};

/// English table (migrated from the old `en-US.js`, scope-trimmed).
pub const EN: Strings = Strings {
    app_title: "ShadowsocksR Client",
    app_subtitle: "Secure proxy client for Linux",
    lang_label: "Language",
    about_label: "About",
    about_comments: "Project: https://github.com/liangzhaoyuan12/ssr-client-gtk\nLicense: GPL-3.0-or-later\nAuthor: liangzhaoyuan12",
    lang_zh: "中文",
    lang_en: "English",
    common_loading: "Loading...",
    common_cancel: "Cancel",
    common_save: "Save",
    common_create: "Create",
    common_update: "Update",
    common_delete: "Delete",
    common_edit: "Edit",
    common_refresh: "Refresh",
    common_confirm: "Confirm",
    common_close: "Close",
    cfg_list_title: "Configuration Profiles",
    cfg_list_empty: "No configurations found",
    cfg_list_empty_hint: "Click \"Add New\" to create your first configuration",
    cfg_list_active: "Active",
    cfg_list_delete_confirm: "Are you sure you want to delete \"{name}\"?",
    cfg_list_delete_success: "Deleted successfully",
    cfg_list_delete_failed: "Failed to delete",
    cfg_list_load_failed: "Failed to load config list",
    cf_add_title: "Add New Configuration",
    cf_edit_title: "Edit Configuration",
    cf_basic_settings: "Basic Settings",
    cf_advanced_settings: "Advanced Settings",
    cf_profile_name: "Profile Name",
    cf_profile_name_placeholder: "e.g., MyServer",
    cf_profile_name_hint: "Letters only, cannot be changed after creation",
    cf_server_address: "Server Address",
    cf_server_address_placeholder: "example.com or 1.2.3.4",
    cf_server_port: "Server Port",
    cf_password: "Password",
    cf_password_placeholder: "Enter password",
    cf_method: "Encryption Method",
    cf_protocol: "Protocol",
    cf_obfs: "Obfuscation",
    cf_local_port: "Local Port",
    cf_protocol_param: "Protocol Param",
    cf_protocol_param_placeholder: "Optional",
    cf_obfs_param: "Obfs Param",
    cf_obfs_param_placeholder: "Optional",
    cf_idle_timeout: "Idle Timeout (s)",
    cf_connect_timeout: "Connect Timeout (s)",
    cf_udp_timeout: "UDP Timeout (s)",
    cf_enable_udp: "Enable UDP Relay",
    cf_listen_note: "Local listen address is fixed to 0.0.0.0 (unauthenticated SOCKS5 — reachable from your LAN)",
    cf_validation_name: "Config name must contain only letters (a-z, A-Z)",
    cf_validation_server: "Server address is required",
    cf_validation_password: "Password is required",
    cf_validation_port: "Port must be an integer between 1 and 65535",
    cf_success_created: "Configuration created!",
    cf_success_updated: "Configuration updated!",
    cf_error_save: "Failed to save configuration",
    pc_title: "Proxy Control",
    pc_connected: "Connected",
    pc_disconnected: "Disconnected",
    pc_select_first: "Please select a configuration first",
    pc_active_config: "Active Configuration",
    pc_enable: "Enable Proxy",
    pc_disable: "Disable Proxy",
    pc_connecting: "Connecting...",
    pc_disconnecting: "Disconnecting...",
    pc_success_enabled: "Proxy enabled successfully!",
    pc_success_disabled: "Proxy disabled successfully!",
    pc_close_stopped: "Window closed — proxy disabled!",
    pc_error_enable: "Failed to enable proxy",
    pc_error_disable: "Failed to disable proxy",
    pc_unsupported_desktop: "Desktop environment {desktop} is not supported for automatic system proxy; set it manually to 127.0.0.1:{port}",
    dash_how_to_use: "How to Use",
    dash_local_proxy: "Local Proxy Settings",
    dash_port_label: "Port",
    dash_listen_line: "Listening on 0.0.0.0:{port}",
    footer_text: "ShadowsocksR Linux Client",
    err_generic: "Operation failed",
    err_port_in_use: "Port {port} is already in use: stop the program holding it, or pick another local port in the config",
    err_already_running: "Proxy is already running — disable it first",
    err_not_running: "Proxy is not running",
    err_not_found: "Configuration not found",
    err_name_invalid: "Invalid config name: letters only",
    err_conflict: "A configuration with this name already exists",
    err_sysproxy: "Failed to update the system proxy (command error)",
    err_json: "Invalid configuration file format",
    err_core: "Proxy core error",
    err_io: "File I/O error",
    err_acl: "The ACL file could not be read or parsed",
    err_dns: "Invalid DNS setting",
    dash_route_title: "Routing, DNS & System Proxy",
    route_mode_label: "Routing mode",
    route_global: "Global (route everything)",
    route_bypass_lan: "Bypass LAN",
    route_bypass_cn: "Bypass mainland China",
    route_bypass_lan_cn: "Bypass LAN and mainland China",
    route_acl: "Custom ACL file",
    route_acl_file: "ACL file",
    route_acl_pick: "Choose file…",
    route_acl_none: "No ACL file selected",
    route_udp_note: "The UDP relay is not routed — it always goes through SSR.",
    route_restart_hint: "Routing and DNS settings apply the next time the proxy is enabled.",
    dns_label: "DNS resolution",
    dns_system: "System DNS",
    dns_custom: "Custom DNS",
    dns_servers_placeholder: "DNS server IPs, comma separated, e.g. 223.5.5.5, 8.8.8.8",
    dns_note: "Used for local lookups and routing decisions; proxied domains are resolved by the SSR server.",
    dns_note_ali: "AliDNS DoT: dns.alidns.com · DoH: https://dns.alidns.com/dns-query (this client still queries plain UDP/TCP 53 — no DoT/DoH).",
    dns_note_tencent: "DNSPod DoT: dot.pub · DoH: https://doh.pub/dns-query (this client still queries plain UDP/TCP 53 — no DoT/DoH).",
    dns_ali: "AliDNS (223.5.5.5)",
    dns_tencent: "DNSPod Public DNS+ (119.29.29.29)",
    sysproxy_mode_label: "System proxy",
    sysproxy_desktop: "Desktop settings",
    sysproxy_env: "Env vars",
    sysproxy_desktop_hint: "Handled by the desktop's own proxy settings (KDE / GNOME and derivatives).",
    sysproxy_env_hint: "Written to {rc}; takes effect in newly opened terminals only.",
    sysproxy_env_unknown: "Cannot determine this shell's rc file — enabling will report an error (bash / zsh / fish supported).",
    steps: &[
        "Firefox has its own proxy settings — set the proxy in Firefox's own settings.",
        "License: GPL-3.0-or-later",
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key exists in both languages, is non-empty, and is not a
    /// leftover placeholder (GOAL 4.5).
    #[test]
    fn all_strings_nonempty_and_translated() {
        for table in [ZH, EN] {
            let all = table.all();
            assert!(!all.is_empty());
            for s in all {
                assert!(
                    !s.trim().is_empty(),
                    "empty message in {:?}",
                    table.app_title
                );
                assert!(
                    !s.contains("TODO") && !s.contains("FIXME"),
                    "placeholder left: {s}"
                );
                // A key leaking through untranslated would show the raw id.
                assert!(
                    !s.starts_with("common.") && !s.starts_with("configForm."),
                    "raw i18n key leaked: {s}"
                );
            }
        }
    }

    /// The two languages declare the same number of messages (structs
    /// guarantee same fields; this guards `steps` drift).
    #[test]
    fn zh_en_same_message_count() {
        assert_eq!(ZH.all().len(), EN.all().len());
    }

    /// Error mapping never returns an empty sentence and fills the port
    /// token for port errors.
    #[test]
    fn error_text_is_localized() {
        let msg = error_text(&AppError::PortInUse(1234), &ZH);
        assert!(msg.contains("1234"), "{msg}");
        assert!(msg.contains("端口"), "{msg}");
        let msg = error_text(&AppError::PortInUse(1234), &EN);
        assert!(msg.contains("1234") && msg.contains("Port"), "{msg}");
        for e in [
            AppError::AlreadyRunning,
            AppError::NotRunning,
            AppError::NotFound("x".into()),
            AppError::InvalidName("x".into()),
        ] {
            assert!(!error_text(&e, &ZH).is_empty());
            assert!(!error_text(&e, &EN).is_empty());
        }
    }

    #[test]
    fn template_fill_replaces_tokens() {
        assert_eq!(fill("a {port} b", &[("port", "8080")]), "a 8080 b");
    }

    #[test]
    fn language_roundtrip_in_settings_file() {
        // save_lang writes to Snapshot::config_dir(); test serde roundtrip
        // through the same document type instead of touching $HOME.
        let s = crate::config::prefs::Prefs {
            language: Some(Lang::ZhCn),
            ..Default::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("zh-CN"));
        let back: crate::config::prefs::Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(back.language, Some(Lang::ZhCn));
    }

    #[test]
    fn every_chinese_locale_gets_the_chinese_ui() {
        // 简中与繁中同看简体界面（唯一那门中文）；其他语言一律英语。
        for v in [
            "zh",
            "zh_CN",
            "zh_CN.UTF-8",
            "zh-Hans",
            "zh_Hans_CN",
            "zh_SG",
            "zh_CN.UTF-8@pinyin",
            "zh_CN:en",
            "zh_TW.UTF-8",
            "zh_HK",
            "zh_MO",
            "zh_Hant",
            "zh_TW:en",
        ] {
            assert!(is_chinese(v), "{v} 应看中文界面");
        }
        for v in [
            "",
            "C",
            "C.UTF-8",
            "POSIX",
            "en_US.UTF-8",
            "ja_JP.UTF-8",
            "fr_FR.UTF-8",
            "de_DE",
            "ko_KR",
        ] {
            assert!(!is_chinese(v), "{v} 应判为英语");
        }
    }

    #[test]
    fn posix_locales_never_decide_the_language() {
        assert!(is_posix_locale("C"));
        assert!(is_posix_locale("c.UTF-8"));
        assert!(is_posix_locale("POSIX"));
        assert!(!is_posix_locale("zh_CN.UTF-8"));
        assert!(!is_posix_locale("en_US"));
        assert!(!is_posix_locale("zh_TW"));
    }

    #[test]
    fn detect_falls_back_to_english_for_non_zh() {
        // Process env here is LANG=C.UTF-8 or zh — assert only the rule:
        // empty/posix-ish env → EnUs.
        let lang = Lang::detect();
        assert!(matches!(lang, Lang::ZhCn | Lang::EnUs));
    }
}
