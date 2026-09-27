//! Application-wide error type.
//!
//! Every fallible operation in the app reports an `AppError`; the UI maps
//! it to a localized string (`i18n`), never to a raw JSON envelope or a
//! Rust debug dump (ARCHITECTURE §8-2).

/// Errors surfaced by config storage, the proxy service and system-proxy
/// integration.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `cfg_name` failed the `^[a-zA-Z]+$` rule (path-traversal guard).
    #[error("invalid config name: {0}")]
    InvalidName(String),

    /// The config file does not exist on disk.
    #[error("config file doesn't exist: {0}")]
    NotFound(String),

    /// The config name is already taken (create-only operation).
    #[error("config already exists: {0}")]
    AlreadyExists(String),

    /// The local SOCKS5 port is already bound by another process.
    #[error("port {0} is already in use")]
    PortInUse(u16),

    /// The proxy is already running (single-instance invariant).
    #[error("ssr client is already running")]
    AlreadyRunning,

    /// The proxy is not running (stop called while disabled).
    #[error("ssr client isn't running")]
    NotRunning,

    /// `ssr-client-rs` reported an error (config parse, bind, relay…).
    #[error("ssr core: {0}")]
    Core(String),

    /// Reading/writing files under the config directory failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A config file is not valid ssr-n JSON.
    #[error("invalid config json: {0}")]
    Json(String),

    /// A system-proxy command (gsettings / kwriteconfig5 / dbus…) failed.
    #[error("system proxy: {0}")]
    SysProxy(String),

    /// The custom ACL file is missing, unreadable or malformed.
    #[error("acl: {0}")]
    Acl(String),

    /// The custom DNS setting is invalid (bad server list, empty selection).
    #[error("dns: {0}")]
    Dns(String),
}

impl AppError {
    /// Convert a `serde_json` error into [`AppError::Json`].
    pub fn json(err: serde_json::Error) -> Self {
        Self::Json(err.to_string())
    }
}

/// Convenience alias used across the crate.
pub type AppResult<T> = Result<T, AppError>;
