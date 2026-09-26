//! On-disk config model — the ssr-n JSON schema (`ARCHITECTURE.md` §6.3).
//!
//! **External contract**: `ssr-client-rs::config_json::config_from_json`
//! parses the same files, so field names and nesting must not change.
//!
//! Deliberately *not* modelled:
//! - `cfg_name` — it is the file name, never part of the file contents
//!   (ARCHITECTURE §6.2 `insert_cfg_file` strips it).
//! - `client_settings.ssr_service_port` — legacy second port of the old
//!   Tauri build; unknown keys are ignored on read and never written back
//!   (GOAL §6.1 / D5: single exposed port only).

use serde::{Deserialize, Serialize};

/// One SSR node configuration as stored in `~/.ssr/<cfg_name>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowsocksConfig {
    /// Shared password for the remote server.
    pub password: String,
    /// Encryption method name, e.g. `aes-256-cfb` (must be a `CipherType`).
    pub method: String,
    /// Protocol name, e.g. `auth_aes128_sha1` (must be a `ProtocolType`).
    pub protocol: String,
    /// Protocol parameter string (`user:pass` list), usually empty.
    pub protocol_param: String,
    /// Obfuscation name, e.g. `tls1.2_ticket_auth` (must be an `ObfsType`).
    pub obfs: String,
    /// Obfuscation parameter string, usually empty.
    pub obfs_param: String,
    /// Enable UDP relay on the same port as the TCP SOCKS5 listener.
    pub udp: bool,
    /// Connection idle timeout in seconds.
    pub idle_timeout: u32,
    /// TCP connect timeout in seconds.
    pub connect_timeout: u32,
    /// UDP timeout in seconds.
    pub udp_timeout: u32,
    /// Local/remote endpoint settings.
    pub client_settings: ClientSettings,
}

/// `client_settings` block of the config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSettings {
    /// Remote SSR server host (name or IP).
    pub server: String,
    /// Remote SSR server port.
    pub server_port: u16,
    /// Local bind address — always `0.0.0.0` (GOAL §6.3 / D2, written on save).
    pub listen_address: String,
    /// Local SOCKS5 port; the only port the app ever exposes (GOAL §6 N1).
    pub listen_port: u16,
}

/// Field defaults matching the old form defaults (`ARCHITECTURE.md` §5.4).
impl Default for ShadowsocksConfig {
    fn default() -> Self {
        Self {
            password: String::new(),
            method: "aes-128-ctr".into(),
            protocol: "auth_aes128_md5".into(),
            protocol_param: String::new(),
            obfs: "tls1.2_ticket_auth".into(),
            obfs_param: String::new(),
            udp: true,
            idle_timeout: 300,
            connect_timeout: 6,
            udp_timeout: 6,
            client_settings: ClientSettings::default(),
        }
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            server: String::new(),
            server_port: 443,
            // GOAL D2: listen_address is fixed at 0.0.0.0 — never exposed
            // in the UI, always rewritten on save.
            listen_address: "0.0.0.0".into(),
            listen_port: 1080,
        }
    }
}
