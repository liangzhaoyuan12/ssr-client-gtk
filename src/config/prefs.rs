//! `~/.config/ssr-client-gtk/settings.json` — **preferences only**
//! (never profile data, never message tables; GOAL §2.1).
//!
//! The file used to hold the language alone; routing and DNS are
//! preferences too, so the whole document is loaded and written as one
//! unit — partial writes would drop the other fields.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::i18n::Lang;
use crate::routing::RoutingSettings;
use crate::sysproxy::SysProxyPref;

/// The persisted preference document.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Prefs {
    /// UI language (`zh-CN` / `en-US`).
    #[serde(default)]
    pub language: Option<Lang>,
    /// Routing mode + ACL path + DNS choice.
    #[serde(default)]
    pub routing: RoutingSettings,
    /// Desktop settings vs. shell-rc environment variables.
    #[serde(default)]
    pub sysproxy: SysProxyPref,
    /// Sidebar profile picked last session — restored on the next launch so
    /// the user doesn't have to click the same row again (GOAL 7.12).
    #[serde(default)]
    pub selected_profile: Option<String>,
}

/// `~/.config/ssr-client-gtk/settings.json`.
pub fn path() -> PathBuf {
    crate::sysproxy::snapshot::Snapshot::config_dir().join("settings.json")
}

/// Read the document; a missing or corrupt file means "defaults".
pub fn load() -> Prefs {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Prefs::default();
    };
    serde_json::from_str::<Prefs>(&text).unwrap_or_default()
}

/// Write the document atomically (`.part` + rename).
pub fn save(prefs: &Prefs) -> AppResult<()> {
    let dir = crate::sysproxy::snapshot::Snapshot::config_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(prefs).map_err(AppError::json)?;
    let tmp = dir.join(".settings.json.part");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_empty_documents_fall_back_to_defaults() {
        assert!(serde_json::from_str::<Prefs>("{}").is_ok());
        let legacy = r#"{"language":"zh-CN"}"#;
        let prefs: Prefs = serde_json::from_str(legacy).unwrap();
        assert_eq!(prefs.language, Some(Lang::ZhCn));
        assert_eq!(prefs.routing, RoutingSettings::default());
        assert_eq!(prefs.sysproxy, SysProxyPref::Desktop);
        assert_eq!(prefs.selected_profile, None, "old document, no memory");
        assert!(serde_json::from_str::<Prefs>("not json").is_err());
    }

    #[test]
    fn round_trips_language_and_routing_together() {
        let prefs = Prefs {
            language: Some(Lang::EnUs),
            routing: RoutingSettings {
                mode: crate::routing::RouteMode::BypassLanCn,
                acl_path: "/tmp/a.acl".into(),
                dns: crate::routing::dns::DnsConfig::parse("223.5.5.5").unwrap(),
            },
            sysproxy: SysProxyPref::EnvVar,
            selected_profile: Some("hk".into()),
        };
        let text = serde_json::to_string(&prefs).unwrap();
        let back: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(back.language, Some(Lang::EnUs));
        assert_eq!(back.routing.mode, crate::routing::RouteMode::BypassLanCn);
        assert_eq!(back.routing.acl_path, "/tmp/a.acl");
        assert_eq!(back.sysproxy, SysProxyPref::EnvVar);
        assert_eq!(back.selected_profile.as_deref(), Some("hk"));
    }
}
