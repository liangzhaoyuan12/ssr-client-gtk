//! Persisted system-proxy snapshot (self-heal after a crash, GOAL §9-7).
//!
//! On enable we save what the desktop *already had* to
//! `~/.config/ssr-client-gtk/sysproxy-snapshot.json`; on disable we restore
//! it and delete the file. If the app dies in between, the next start finds
//! the file and restores the originals, so no dead `127.0.0.1:<port>` proxy
//! is left behind.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// One desktop setting captured before we overwrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    /// Backend-specific scope: `""` for KDE (file/group are fixed),
    /// the gsettings schema name for GNOME.
    pub group: String,
    /// Key inside that scope.
    pub key: String,
    /// Raw original value; `None` = the key did not exist (restore deletes
    /// / resets it instead of writing a blank).
    pub value: Option<String>,
}

#[cfg(test)]
impl Setting {
    /// A setting that had an explicit value.
    pub fn set(key: &str, value: &str) -> Self {
        Self {
            group: String::new(),
            key: key.to_string(),
            value: Some(value.to_string()),
        }
    }

    /// A setting that was absent before we touched it.
    pub fn absent(key: &str) -> Self {
        Self {
            group: String::new(),
            key: key.to_string(),
            value: None,
        }
    }
}

/// Captured pre-change state of the system proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// `"kde"` or `"gnome"` — selects the restore strategy.
    pub backend: String,
    /// Settings captured, in restore order.
    pub settings: Vec<Setting>,
}

impl Snapshot {
    /// `~/.config/ssr-client-gtk` (or `$XDG_CONFIG_HOME/…`).
    pub fn config_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/tmp"));
                home.join(".config")
            });
        base.join("ssr-client-gtk")
    }

    /// Persist the snapshot (atomic write).
    pub fn save(&self) -> AppResult<()> {
        self.save_to(&Self::config_dir()).map(|_| ())
    }

    /// Persist into an arbitrary directory (tests).
    pub fn save_to(&self, dir: &Path) -> AppResult<PathBuf> {
        fs::create_dir_all(dir)?;
        let path = dir.join("sysproxy-snapshot.json");
        let tmp = dir.join(".sysproxy-snapshot.json.part");
        let text = serde_json::to_string_pretty(self).map_err(AppError::json)?;
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Load a persisted snapshot, if any.
    pub fn load() -> AppResult<Option<Snapshot>> {
        Self::load_from(&Self::config_dir())
    }

    /// Load from an arbitrary directory (tests). A corrupt file is an
    /// error the caller may choose to ignore — it never panics.
    pub fn load_from(dir: &Path) -> AppResult<Option<Snapshot>> {
        let path = dir.join("sysproxy-snapshot.json");
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(AppError::json)
    }

    /// Delete the persisted snapshot (after a successful restore).
    pub fn remove() -> AppResult<()> {
        Self::remove_from(&Self::config_dir())
    }

    /// Delete from an arbitrary directory (tests).
    pub fn remove_from(dir: &Path) -> AppResult<()> {
        let path = dir.join("sysproxy-snapshot.json");
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            backend: "kde".into(),
            settings: vec![
                Setting::set("ProxyType", "0"),
                Setting::absent("socksProxy"),
                Setting::set("NoProxyFor", "localhost,127.0.0.1,::1"),
            ],
        }
    }

    #[test]
    fn roundtrip_save_load_remove() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Snapshot::load_from(dir.path()).unwrap().is_none());

        let snap = sample();
        let path = snap.save_to(dir.path()).unwrap();
        assert!(path.is_file());
        assert_eq!(Snapshot::load_from(dir.path()).unwrap().unwrap(), snap);

        Snapshot::remove_from(dir.path()).unwrap();
        assert!(Snapshot::load_from(dir.path()).unwrap().is_none());
        // Removing twice is not an error.
        Snapshot::remove_from(dir.path()).unwrap();
    }

    #[test]
    fn corrupt_snapshot_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sysproxy-snapshot.json"), "{not json").unwrap();
        assert!(matches!(
            Snapshot::load_from(dir.path()),
            Err(AppError::Json(_))
        ));
    }

    #[test]
    fn absent_value_is_preserved_distinctly_from_empty_string() {
        let dir = tempfile::tempdir().unwrap();
        let snap = Snapshot {
            backend: "gnome".into(),
            settings: vec![Setting::set("host", "''"), Setting::absent("host2")],
        };
        snap.save_to(dir.path()).unwrap();
        let back = Snapshot::load_from(dir.path()).unwrap().unwrap();
        assert_eq!(back.settings[0].value.as_deref(), Some("''"));
        assert_eq!(back.settings[1].value, None);
    }
}
