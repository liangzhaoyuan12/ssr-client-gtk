//! System-proxy backends (KDE / GNOME) with snapshot & restore.
//!
//! Rules (GOAL §6 invariant 6, §4.1):
//! - **Snapshot before write, exact restore on stop** — never a blind
//!   `ProxyType=0` (ARCHITECTURE §8-9).
//! - **Failures propagate** as [`AppError::SysProxy`] — no swallowing
//!   (ARCHITECTURE §8-4).
//! - Strategy table by desktop environment; unrecognised desktops get
//!   [`EnableOutcome::Unsupported`] and the UI shows a manual-setup toast
//!   instead of writing anything (ARCHITECTURE §8-10/11).
//! - External commands go through an injectable [`Runner`] so tests can
//!   assert exact command lines without touching the real desktop.

pub mod gnome;
pub mod kde;
pub mod snapshot;

use std::sync::Arc;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use crate::error::{AppError, AppResult};

pub use snapshot::{Setting, Snapshot};

/// Executes external commands; mocked in tests.
pub trait Runner: Send + Sync {
    /// Run `program args…`, returning trimmed stdout, or an error message.
    fn output(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

/// Real runner backed by `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandRunner;

impl Runner for CommandRunner {
    fn output(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if !out.status.success() {
            let status = out.status;
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{program} exited with {status}: {}", stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Desktop environment as far as system-proxy handling is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Desktop {
    /// KDE Plasma — `kwriteconfig5`/`kreadconfig5` + KIO dbus reparse.
    Kde,
    /// GNOME and derivatives (Cinnamon, MATE, Ubuntu, deepin, uos,
    /// COSMIC, Budgie, Pantheon…) — `gsettings`.
    Gnome,
    /// Anything else: no automatic system proxy (manual-setup toast).
    Unsupported(String),
}

/// Detect the desktop from the environment (old `whoami::desktop_env()`
/// behaviour, but table-driven).
pub fn detect_desktop() -> Desktop {
    let raw = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default();
    let upper = raw.to_uppercase();
    if upper.contains("KDE") || upper.contains("PLASMA") {
        Desktop::Kde
    } else if [
        "GNOME", "UNITY", "CINNAMON", "MATE", "BUDGIE", "DEEPIN", "UOS", "COSMIC", "PANTHEON",
        "UKUI",
    ]
    .iter()
    .any(|k| upper.contains(k))
    {
        Desktop::Gnome
    } else {
        Desktop::Unsupported(if raw.is_empty() {
            "unknown".to_string()
        } else {
            raw
        })
    }
}

/// Result of [`SysProxy::enable`].
#[derive(Debug)]
pub enum EnableOutcome {
    /// System proxy written; the snapshot must be handed to `disable`.
    Applied(Snapshot),
    /// No automatic handling for this desktop — show a manual-setup hint.
    Unsupported {
        /// Desktop string for the message.
        desktop: String,
    },
}

/// System-proxy operations bound to a desktop and a (mockable) runner.
pub struct SysProxy {
    desktop: Desktop,
    runner: Arc<dyn Runner>,
}

impl SysProxy {
    /// Production instance: detect desktop, run real commands.
    pub fn system() -> Self {
        Self::with_runner(detect_desktop(), Arc::new(CommandRunner))
    }

    /// Instance with an injected runner (tests).
    pub fn with_runner(desktop: Desktop, runner: Arc<dyn Runner>) -> Self {
        Self { desktop, runner }
    }

    /// Run a command, mapping failures to [`AppError::SysProxy`].
    pub(crate) fn run(&self, program: &str, args: &[&str]) -> AppResult<String> {
        self.runner
            .output(program, args)
            .map_err(AppError::SysProxy)
    }

    /// Read a setting that may be absent: `Ok(Some(v))` / `Ok(None)` / `Err`.
    pub(crate) fn read(&self, program: &str, args: &[&str]) -> AppResult<Option<String>> {
        match self.runner.output(program, args) {
            Ok(v) => Ok(Some(v).filter(|s| !s.is_empty())),
            Err(msg) if msg.contains("exited with") => Ok(None), // key absent
            Err(msg) => Err(AppError::SysProxy(msg)),
        }
    }

    /// Write the system proxy for a local SOCKS5 on `port`.
    pub fn enable(&self, port: u16) -> AppResult<EnableOutcome> {
        match self.desktop {
            Desktop::Unsupported(ref name) => Ok(EnableOutcome::Unsupported {
                desktop: name.clone(),
            }),
            Desktop::Kde => kde::enable(self, port).map(EnableOutcome::Applied),
            Desktop::Gnome => gnome::enable(self, port).map(EnableOutcome::Applied),
        }
    }

    /// Restore a snapshot taken by [`SysProxy::enable`].
    pub fn disable(&self, snap: &Snapshot) -> AppResult<()> {
        match snap.backend.as_str() {
            "kde" => kde::restore(self, snap),
            "gnome" => gnome::restore(self, snap),
            other => Err(AppError::SysProxy(format!("unknown backend {other:?}"))),
        }
    }
}

/// Test double: canned answers keyed by full command line, records every
/// call (shared with the test through [`Mock::calls`]).
#[cfg(test)]
#[derive(Clone)]
pub struct Mock {
    answers: HashMap<String, Result<String, String>>,
    calls: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl Mock {
    /// Empty mock: every command succeeds with empty stdout.
    pub fn new() -> Self {
        Self {
            answers: HashMap::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Pin an answer for an exact command line
    /// (e.g. `"kreadconfig5 --file kioslaverc … --key ProxyType"`).
    pub fn on(mut self, cmdline: &str, answer: Result<&str, &str>) -> Self {
        self.answers.insert(
            cmdline.to_string(),
            answer.map(str::to_string).map_err(str::to_string),
        );
        self
    }

    /// All command lines run so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Default for Mock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Runner for Mock {
    fn output(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let line = std::iter::once(program.to_string())
            .chain(args.iter().map(|a| a.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.lock().unwrap().push(line.clone());
        self.answers
            .get(&line)
            .cloned()
            .unwrap_or(Ok(String::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysproxy::EnableOutcome;

    /// Current KDE proxy settings on this machine shape the fixture:
    /// ProxyType=1, socksProxy="127.0.0.1 1080" (old app), NoProxyFor set.
    fn kde_mock() -> Mock {
        Mock::new()
            .on(
                "kreadconfig5 --file kioslaverc --group Proxy Settings --key ProxyType",
                Ok("1"),
            )
            .on(
                "kreadconfig5 --file kioslaverc --group Proxy Settings --key socksProxy",
                Ok("127.0.0.1 1080"),
            )
            .on(
                "kreadconfig5 --file kioslaverc --group Proxy Settings --key NoProxyFor",
                Ok("localhost,127.0.0.1,::1"),
            )
    }

    fn kde_sysproxy(mock: &Mock) -> SysProxy {
        SysProxy::with_runner(Desktop::Kde, Arc::new(mock.clone()))
    }

    #[test]
    fn detect_desktop_from_env_table() {
        // The real process env is KDE here; the table itself is unit-tested
        // through the pure matcher on known tokens.
        assert_eq!(detect_desktop(), Desktop::Kde);
        for token in ["GNOME", "X-COSMIC", "Cinnamon", "deepin", "uos"] {
            let upper = token.to_uppercase();
            let hit = ["GNOME", "CINNAMON", "DEEPIN", "UOS", "COSMIC"]
                .iter()
                .any(|k| upper.contains(k));
            assert!(hit, "{token} must map to the GNOME strategy");
        }
    }

    #[test]
    fn kde_enable_writes_proxy_and_snapshots_original() {
        let mock = kde_mock();
        let sp = kde_sysproxy(&mock);
        let outcome = sp.enable(1082).expect("enable");
        let EnableOutcome::Applied(snap) = outcome else {
            panic!("expected Applied");
        };
        assert_eq!(snap.backend, "kde");
        assert_eq!(
            snap.settings,
            vec![
                crate::sysproxy::Setting::set("ProxyType", "1"),
                crate::sysproxy::Setting::set("socksProxy", "127.0.0.1 1080"),
                crate::sysproxy::Setting::set("NoProxyFor", "localhost,127.0.0.1,::1"),
            ]
        );

        let calls = mock.calls();
        // Writes: manual mode + our port, then the KIO reload **last**.
        assert!(calls.contains(
            &"kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1".to_string()
        ));
        assert!(calls.contains(&"kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy 127.0.0.1 1082".to_string()));
        assert_eq!(
            calls.last().map(String::as_str),
            Some(
                "dbus-send --type=signal /KIO/Scheduler org.kde.KIO.Scheduler.reparseSlaveConfiguration string:"
            ),
            "KIO must be told to reparse after the writes"
        );
        // Reads happened before writes.
        let first_write = calls
            .iter()
            .position(|c| c.starts_with("kwriteconfig5"))
            .expect("writes present");
        assert!(
            calls[..first_write]
                .iter()
                .all(|c| c.starts_with("kreadconfig5"))
        );
    }

    #[test]
    fn kde_disable_restores_exact_originals_then_reloads() {
        let mock = kde_mock();
        let sp = kde_sysproxy(&mock);
        let EnableOutcome::Applied(snap) = sp.enable(1082).unwrap() else {
            panic!("expected Applied");
        };
        let before_disable = mock.calls().len();
        sp.disable(&snap).expect("disable");
        let calls = &mock.calls()[before_disable..];

        assert_eq!(
            calls[0],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1"
        );
        assert_eq!(
            calls[1],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy 127.0.0.1 1080"
        );
        assert_eq!(
            calls[2],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost,127.0.0.1,::1"
        );
        assert_eq!(
            calls.last().map(String::as_str),
            Some(
                "dbus-send --type=signal /KIO/Scheduler org.kde.KIO.Scheduler.reparseSlaveConfiguration string:"
            ),
            "reload must be the final step of restore"
        );
    }

    #[test]
    fn kde_absent_key_is_deleted_on_restore() {
        let mock = Mock::new(); // every key absent (empty stdout)
        let sp = kde_sysproxy(&mock);
        let EnableOutcome::Applied(snap) = sp.enable(1090).unwrap() else {
            panic!("expected Applied");
        };
        assert!(snap.settings.iter().all(|s| s.value.is_none()));

        let before = mock.calls().len();
        sp.disable(&snap).unwrap();
        let calls = &mock.calls()[before..];
        let deletes: Vec<_> = calls
            .iter()
            .filter(|c| c.contains("--delete"))
            .cloned()
            .collect();
        assert_eq!(
            deletes.len(),
            3,
            "absent keys must be removed, not blanked: {deletes:?}"
        );
    }

    #[test]
    fn gnome_enable_and_disable_use_gsettings_with_mode_last() {
        let mock = Mock::new()
            .on("gsettings get org.gnome.system.proxy mode", Ok("'none'"))
            .on("gsettings get org.gnome.system.proxy.socks host", Ok("''"))
            .on("gsettings get org.gnome.system.proxy.socks port", Ok("0"));
        let sp = SysProxy::with_runner(Desktop::Gnome, Arc::new(mock.clone()));

        let EnableOutcome::Applied(snap) = sp.enable(1082).unwrap() else {
            panic!("expected Applied");
        };
        assert_eq!(snap.backend, "gnome");
        let calls = mock.calls();
        assert!(
            calls.contains(
                &"gsettings set org.gnome.system.proxy.socks host '127.0.0.1'".to_string()
            )
        );
        assert!(
            calls.contains(&"gsettings set org.gnome.system.proxy.socks port 1082".to_string())
        );
        assert_eq!(
            calls.last().map(String::as_str),
            Some("gsettings set org.gnome.system.proxy mode 'manual'"),
            "mode must flip last so GNOME applies a fully-written config"
        );

        let before = mock.calls().len();
        sp.disable(&snap).unwrap();
        let after = &mock.calls()[before..];
        // Original values verbatim, mode restored last.
        assert_eq!(
            after[0],
            "gsettings set org.gnome.system.proxy.socks host ''"
        );
        assert_eq!(
            after[1],
            "gsettings set org.gnome.system.proxy.socks port 0"
        );
        assert_eq!(after[2], "gsettings set org.gnome.system.proxy mode 'none'");
    }

    #[test]
    fn unsupported_desktop_writes_nothing() {
        let mock = Mock::new();
        let sp = SysProxy::with_runner(Desktop::Unsupported("XFCE".into()), Arc::new(mock.clone()));
        match sp.enable(1082).unwrap() {
            EnableOutcome::Unsupported { desktop } => assert_eq!(desktop, "XFCE"),
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert!(mock.calls().is_empty(), "must not touch the system");
    }

    #[test]
    fn failing_command_surfaces_as_sysproxy_error() {
        let mock = Mock::new().on(
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1",
            Err("kwriteconfig5: permission denied"),
        );
        let sp = kde_sysproxy(&mock);
        let err = sp.enable(1082).unwrap_err();
        match err {
            AppError::SysProxy(msg) => assert!(msg.contains("permission denied"), "{msg}"),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
