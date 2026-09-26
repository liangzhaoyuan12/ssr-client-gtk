//! CRUD over `<cfg_dir>/<cfg_name>.json`.
//!
//! Trust boundary: **all** `cfg_name` validation happens here (GOAL §6.2) —
//! the UI's own checks are only UX, never security. A name must match
//! `^[a-zA-Z]+$`, which by construction rules out `/`, `..`, NUL and every
//! other path-traversal shape.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use super::model::ShadowsocksConfig;
use super::path::config_dir;
use crate::error::{AppError, AppResult};

/// Config store rooted at a directory (the real one, or a tempdir in tests).
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

/// One scanned profile: file name plus a parse error when the JSON is
/// corrupt (GOAL §4.1 — a broken file must not break the list; its row
/// shows the error instead of failing the whole scan).
pub struct Scanned {
    /// Profile name (file stem, already `validate_name`-checked).
    pub name: String,
    /// `Some(e)` when the file exists but cannot be parsed.
    pub error: Option<crate::error::AppError>,
}

impl Store {
    /// Store at the user's real config dir (`~/.ssr` or `/root/.ssr`).
    pub fn new() -> Self {
        Self { dir: config_dir() }
    }

    /// Store at an arbitrary directory — used by tests.
    #[cfg(test)]
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Directory this store reads/writes.
    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Create the config directory if missing (old `create_dir_all` behaviour).
    pub fn ensure_dir(&self) -> AppResult<()> {
        fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    /// Authoritative `cfg_name` validation: `^[a-zA-Z]+$`.
    pub fn validate_name(name: &str) -> AppResult<()> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"^[a-zA-Z]+$").expect("static regex"));
        if re.is_match(name) {
            Ok(())
        } else {
            Err(AppError::InvalidName(name.to_string()))
        }
    }

    /// Absolute path of a config file (validates the name first).
    fn file_for(&self, name: &str) -> AppResult<PathBuf> {
        Self::validate_name(name)?;
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// List config names (file stems), sorted alphabetically.
    ///
    /// Only `*.json` files whose stem passes [`Store::validate_name`] are
    /// returned: `hk.tmp.json` residue and unrelated JSON never show up, and
    /// a *corrupt* file still lists — its error surfaces on `load` instead
    /// (GOAL §4.1: 单文件损坏不影响列表).
    pub fn list(&self) -> AppResult<Vec<String>> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if Self::validate_name(stem).is_ok() {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Scan the config dir: never fails as a whole. Non-`.json` files and
    /// invalid stems are skipped (same rule as [`Store::list`]); a corrupt
    /// file is reported per row instead of aborting the scan.
    pub fn scan(&self) -> Vec<Scanned> {
        self.list()
            .unwrap_or_default()
            .into_iter()
            .map(|name| {
                let error = self.load(&name).err();
                Scanned { name, error }
            })
            .collect()
    }

    /// Read one config; unknown keys (e.g. legacy `ssr_service_port`) are
    /// ignored by serde and dropped on the next save (GOAL §6.1).
    pub fn load(&self, name: &str) -> AppResult<ShadowsocksConfig> {
        let path = self.file_for(name)?;
        if !path.exists() {
            return Err(AppError::NotFound(name.to_string()));
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(AppError::json)
    }

    /// Create a new config; fails if the name is taken.
    pub fn create(&self, name: &str, cfg: &ShadowsocksConfig) -> AppResult<()> {
        let path = self.file_for(name)?;
        if path.exists() {
            return Err(AppError::AlreadyExists(name.to_string()));
        }
        self.write(&path, cfg)
    }

    /// Save (create or overwrite) a config — the edit path of the form.
    pub fn save(&self, name: &str, cfg: &ShadowsocksConfig) -> AppResult<()> {
        let path = self.file_for(name)?;
        self.write(&path, cfg)
    }

    /// Delete a config file.
    pub fn delete(&self, name: &str) -> AppResult<()> {
        let path = self.file_for(name)?;
        if !path.exists() {
            return Err(AppError::NotFound(name.to_string()));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    /// Whether a config file exists.
    #[cfg(test)]
    pub fn exists(&self, name: &str) -> bool {
        self.file_for(name).map(|p| p.exists()).unwrap_or(false)
    }

    /// Serialize + write atomically (temp file in the same dir, then rename).
    ///
    /// Also enforces GOAL §6.3: `listen_address` is forced to `0.0.0.0` on
    /// every write, so no config this app saves can bind anything else.
    fn write(&self, path: &Path, cfg: &ShadowsocksConfig) -> AppResult<()> {
        let mut cfg = cfg.clone();
        cfg.client_settings.listen_address = "0.0.0.0".to_string();

        self.ensure_dir()?;
        let text = serde_json::to_string_pretty(&cfg).map_err(AppError::json)?;
        let tmp = self.dir.join(format!(
            ".{}.json.part",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("cfg")
        ));
        fs::write(&tmp, text)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::ClientSettings;

    /// Minimal valid config pointing at a dummy server.
    fn sample() -> ShadowsocksConfig {
        ShadowsocksConfig {
            password: "secret".into(),
            client_settings: ClientSettings {
                server: "example.com".into(),
                server_port: 8388,
                listen_address: "0.0.0.0".into(),
                listen_port: 1080,
            },
            ..ShadowsocksConfig::default()
        }
    }

    #[test]
    fn scan_reports_corrupt_files_per_row_without_failing_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        store.create("good", &sample()).unwrap();
        fs::write(dir.path().join("broken.json"), "{ not json").unwrap();
        fs::write(dir.path().join("hk.tmp.json"), "{}").unwrap(); // skipped

        // The scan never fails as a whole; each row carries its own state.
        let scanned = store.scan();
        assert_eq!(scanned.len(), 2, "good + broken, tmp skipped");
        let good = scanned.iter().find(|e| e.name == "good").unwrap();
        assert!(good.error.is_none());
        let broken = scanned.iter().find(|e| e.name == "broken").unwrap();
        assert!(broken.error.is_some(), "corrupt file must be flagged");

        // list() (names only) is unaffected by a corrupt file.
        assert_eq!(store.list().unwrap(), vec!["broken", "good"]);
    }

    #[test]
    fn crud_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());

        assert!(store.list().unwrap().is_empty());
        store.create("alpha", &sample()).unwrap();
        assert_eq!(store.list().unwrap(), vec!["alpha".to_string()]);
        assert!(store.exists("alpha"));

        let mut cfg = store.load("alpha").unwrap();
        cfg.password = "changed".into();
        store.save("alpha", &cfg).unwrap();
        assert_eq!(store.load("alpha").unwrap().password, "changed");

        store.delete("alpha").unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn rejects_path_traversal_and_bad_names() {
        for bad in [
            "../etc/passwd",
            "a/b",
            "a\\b",
            "..",
            "123",
            "hk.tmp",
            "",
            "a-b",
            "a b",
            "a\u{0}b",
            ".hidden",
        ] {
            assert!(
                Store::validate_name(bad).is_err(),
                "name {bad:?} must be rejected"
            );
            let store = Store::with_dir("/nonexistent");
            assert!(store.file_for(bad).is_err(), "path for {bad:?} must fail");
        }
        for good in ["a", "ABC", "hk", "nodeOne"] {
            Store::validate_name(good).unwrap();
        }
    }

    #[test]
    fn delete_missing_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        assert!(matches!(store.delete("nope"), Err(AppError::NotFound(_))));
        assert!(matches!(store.load("nope"), Err(AppError::NotFound(_))));
    }

    #[test]
    fn create_twice_conflicts_save_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        store.create("dup", &sample()).unwrap();
        assert!(matches!(
            store.create("dup", &sample()),
            Err(AppError::AlreadyExists(_))
        ));
        // save() is the edit path: overwrite is allowed.
        let mut cfg = sample();
        cfg.password = "v2".into();
        store.save("dup", &cfg).unwrap();
        assert_eq!(store.load("dup").unwrap().password, "v2");
    }

    #[test]
    fn list_skips_tmp_json_and_non_json_but_keeps_corrupt_configs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        store.create("good", &sample()).unwrap();
        // Legacy residue of the old Tauri build + unrelated files.
        fs::write(dir.path().join("hk.tmp.json"), "{}").unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        fs::write(dir.path().join("stray.json"), "not json at all").unwrap();

        let names = store.list().unwrap();
        // "stray" has a valid stem, so it lists — but loading it errors.
        assert_eq!(names, vec!["good".to_string(), "stray".to_string()]);
        assert!(store.load("good").is_ok());
        assert!(matches!(store.load("stray"), Err(AppError::Json(_))));
        // Corrupt file did not hide the healthy one.
        assert!(store.load("good").is_ok());
    }

    #[test]
    fn unknown_keys_are_ignored_and_not_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        // Structure of the real ~/.ssr/hk.json, incl. legacy ssr_service_port.
        let legacy = r#"{
            "password": "1234567890",
            "method": "aes-256-cfb",
            "protocol": "auth_aes128_sha1",
            "protocol_param": "",
            "obfs": "tls1.2_ticket_auth",
            "obfs_param": "",
            "udp": true,
            "idle_timeout": 300,
            "connect_timeout": 6,
            "udp_timeout": 6,
            "client_settings": {
                "server": "206.237.10.116",
                "server_port": 2800,
                "listen_address": "0.0.0.0",
                "listen_port": 1081,
                "ssr_service_port": 1081
            }
        }"#;
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(dir.path().join("hk.json"), legacy).unwrap();

        let cfg = store.load("hk").unwrap();
        assert_eq!(cfg.client_settings.server_port, 2800);
        assert_eq!(cfg.client_settings.listen_port, 1081);

        // GOAL 1.5: the real legacy file must also load in the ssr core lib.
        let core = ssr_client_rs::config_json::config_from_json(legacy)
            .expect("ssr-client-rs must parse the legacy ~/.ssr/hk.json shape");
        assert_eq!(core.server, "206.237.10.116");
        assert_eq!(core.listen_port, 1081);

        // Re-save: ssr_service_port must be gone from the file.
        store.save("hk", &cfg).unwrap();
        let text = fs::read_to_string(dir.path().join("hk.json")).unwrap();
        assert!(
            !text.contains("ssr_service_port"),
            "legacy key leaked back: {text}"
        );
        assert!(text.contains("\"listen_port\": 1081"));
    }

    #[test]
    fn listen_address_is_forced_to_0_0_0_0_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        let mut cfg = sample();
        cfg.client_settings.listen_address = "127.0.0.1".into();
        store.save("loop", &cfg).unwrap();
        let text = fs::read_to_string(dir.path().join("loop.json")).unwrap();
        assert!(text.contains("\"listen_address\": \"0.0.0.0\""), "{text}");
        assert_eq!(
            store.load("loop").unwrap().client_settings.listen_address,
            "0.0.0.0"
        );
    }

    #[test]
    fn written_files_parse_with_ssr_client_rs() {
        // GOAL §6.1: our serde schema must stay readable by the core lib.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path());
        store.create("compat", &sample()).unwrap();
        let text = fs::read_to_string(dir.path().join("compat.json")).unwrap();
        let core = ssr_client_rs::config_json::config_from_json(&text)
            .expect("ssr-client-rs must parse what we write");
        assert_eq!(core.server, "example.com");
        assert_eq!(core.server_port, 8388);
        assert_eq!(core.listen_port, 1080);
        assert_eq!(core.listen_address, "0.0.0.0");
    }

    #[test]
    fn empty_and_missing_dirs_list_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_dir(dir.path().join("not-created-yet"));
        assert!(store.list().unwrap().is_empty());
        store.ensure_dir().unwrap();
        assert!(store.dir().is_dir());
        assert!(store.list().unwrap().is_empty());
    }
}
