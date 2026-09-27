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

pub mod env;
pub mod gnome;
pub mod kde;
pub mod snapshot;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

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

/// Desktops whose proxy settings live in `org.gnome.system.proxy` (the
/// GIO/GSettings proxy resolver every GTK/GNOME-family app reads).
///
/// Tokens are matched against the upper-cased `XDG_CURRENT_DESKTOP` value
/// (`:`-separated lists included), so `ubuntu:GNOME`, `X-Cinnamon` and
/// `Pop:GNOME` all land here. `DDE` is deepin/UOS's `XDG_CURRENT_DESKTOP`
/// value — the session name (`deepin`, `uos`) is the fallback that catches
/// it when the XDG variable says something else.
const GNOME_TOKENS: [&str; 10] = [
    "GNOME", "UNITY", "CINNAMON", "MATE", "BUDGIE", "DEEPIN", "UOS", "DDE", "PANTHEON", "UKUI",
];

/// Classify a raw desktop string (pure; unit-tested against the tokens).
pub fn classify_desktop(raw: &str) -> Desktop {
    let upper = raw.to_uppercase();
    if upper.contains("KDE") || upper.contains("PLASMA") {
        Desktop::Kde
    } else if GNOME_TOKENS.iter().any(|k| upper.contains(k)) {
        Desktop::Gnome
    } else if raw.is_empty() {
        Desktop::Unsupported("unknown".to_string())
    } else {
        Desktop::Unsupported(raw.to_string())
    }
}

/// Detect the desktop from the environment (old `whoami::desktop_env()`
/// behaviour, but table-driven).
pub fn detect_desktop() -> Desktop {
    let raw = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default();
    classify_desktop(&raw)
}

/// *How* the system proxy should be switched on (user choice in the GUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SysProxyPref {
    /// Write the desktop's own proxy settings (KDE `kioslaverc` /
    /// GNOME-family gsettings).
    #[default]
    Desktop,
    /// Write the shell rc file (`http_proxy` & friends) instead — for
    /// desktops with no proxy settings at all, and for shells-only setups.
    EnvVar,
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
    /// Test hook: write the env-var backend here instead of `$HOME`.
    #[cfg(test)]
    env_rc: Option<std::path::PathBuf>,
}

impl SysProxy {
    /// Production instance: detect desktop, run real commands.
    pub fn system() -> Self {
        Self::with_runner(detect_desktop(), Arc::new(CommandRunner))
    }

    /// Instance with an injected runner (tests).
    pub fn with_runner(desktop: Desktop, runner: Arc<dyn Runner>) -> Self {
        Self {
            desktop,
            runner,
            #[cfg(test)]
            env_rc: None,
        }
    }

    /// Where the env-var backend should write (tests only).
    #[cfg(test)]
    pub fn with_env_rc(mut self, path: std::path::PathBuf) -> Self {
        self.env_rc = Some(path);
        self
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

    /// Write the system proxy for a local SOCKS5 on `port`, using the
    /// strategy the user picked (`pref`).
    pub fn enable(&self, port: u16, pref: SysProxyPref) -> AppResult<EnableOutcome> {
        if pref == SysProxyPref::EnvVar {
            return env::enable(self, port).map(EnableOutcome::Applied);
        }
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
            "env" => env::restore(self, snap),
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
        // The real process env is KDE here...
        assert_eq!(detect_desktop(), Desktop::Kde);
        // ...and the table itself is exercised through the pure classifier.
        for token in [
            "GNOME",
            "ubuntu:GNOME",
            "Pop:GNOME",
            "X-Cinnamon",
            "MATE",
            "Budgie:GNOME",
            "Pantheon",
            "DDE",    // deepin / UOS — user's real-machine test: gsettings
            "deepin", // DESKTOP_SESSION fallback
            "uos",
            "UKUI",
        ] {
            assert_eq!(
                classify_desktop(token),
                Desktop::Gnome,
                "{token} must map to the GNOME strategy"
            );
        }
        for token in ["KDE", "plasma", "KDE:GNOME"] {
            assert_eq!(classify_desktop(token), Desktop::Kde, "{token}");
        }
        // COSMIC ships no proxy settings (cosmic-settings: 0 hits for
        // "proxy"), so writing gsettings there would be a silent no-op.
        for token in ["XFCE", "LXQt", "sway", "Sway", "X-COSMIC", "cosmic"] {
            assert!(
                matches!(classify_desktop(token), Desktop::Unsupported(_)),
                "{token} must stay unsupported (no proxy settings there)"
            );
        }
        assert!(matches!(classify_desktop(""), Desktop::Unsupported(_)));
    }

    #[test]
    fn kde_enable_writes_proxy_and_snapshots_original() {
        let mock = kde_mock();
        let sp = kde_sysproxy(&mock);
        let outcome = sp.enable(1082, SysProxyPref::Desktop).expect("enable");
        let EnableOutcome::Applied(snap) = outcome else {
            panic!("expected Applied");
        };
        assert_eq!(snap.backend, "kde");
        assert_eq!(
            snap.settings,
            vec![
                crate::sysproxy::Setting::set("socksProxy", "127.0.0.1 1080"),
                crate::sysproxy::Setting::set("NoProxyFor", "localhost,127.0.0.1,::1"),
                crate::sysproxy::Setting::set("ProxyType", "1"),
            ]
        );

        let calls = mock.calls();
        assert!(calls.contains(&"kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy 127.0.0.1 1082".to_string()));
        // Writes: address first, `ProxyType` (the mode) **last**, then the
        // KIO reload — the desktop never sees "manual" with a stale address.
        let writes: Vec<&String> = calls
            .iter()
            .filter(|c| c.starts_with("kwriteconfig5"))
            .collect();
        assert_eq!(
            writes.last().map(|c| c.as_str()),
            Some("kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1"),
            "ProxyType must be the final write: {writes:?}"
        );
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
        let EnableOutcome::Applied(snap) = sp.enable(1082, SysProxyPref::Desktop).unwrap() else {
            panic!("expected Applied");
        };
        let before_disable = mock.calls().len();
        sp.disable(&snap).expect("disable");
        let calls = &mock.calls()[before_disable..];

        // Restore order mirrors enable: address first, mode last.
        assert_eq!(
            calls[0],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy 127.0.0.1 1080"
        );
        assert_eq!(
            calls[1],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost,127.0.0.1,::1"
        );
        assert_eq!(
            calls[2],
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1"
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
        let EnableOutcome::Applied(snap) = sp.enable(1090, SysProxyPref::Desktop).unwrap() else {
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

        let EnableOutcome::Applied(snap) = sp.enable(1082, SysProxyPref::Desktop).unwrap() else {
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
        match sp.enable(1082, SysProxyPref::Desktop).unwrap() {
            EnableOutcome::Unsupported { desktop } => assert_eq!(desktop, "XFCE"),
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert!(mock.calls().is_empty(), "must not touch the system");
    }

    #[test]
    fn failing_command_surfaces_as_sysproxy_error() {
        // A *failed* command (not a missing binary) — no `*6` fallback here.
        let mock = kde_mock().on(
            "kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1",
            Err("kwriteconfig5 exited with exit status: 1: permission denied"),
        );
        let sp = kde_sysproxy(&mock);
        let err = sp.enable(1082, SysProxyPref::Desktop).unwrap_err();
        match err {
            AppError::SysProxy(msg) => assert!(msg.contains("permission denied"), "{msg}"),
            other => panic!("unexpected error: {other:?}"),
        }
        // …and because the write failed, the originals went back: the
        // desktop must never be left pointing at a port we then close.
        let calls = mock.calls();
        // The failure is recorded as the command line (stderr is not).
        let after = calls
            .iter()
            .position(|c| {
                c == "kwriteconfig5 --file kioslaverc --group Proxy Settings --key ProxyType 1"
            })
            .expect("failing write recorded");
        assert!(
            calls[after..]
                .iter()
                .any(|c| c.contains("socksProxy 127.0.0.1 1080")),
            "rollback must restore the original socksProxy: {calls:?}"
        );
        assert!(
            calls[after..].iter().any(|c| c.contains("ProxyType 1")),
            "rollback must restore the original ProxyType: {calls:?}"
        );
    }

    /// A Plasma 6-only machine: the `*5` KConfig tools are not installed,
    /// so every command must fall through to the `*6` spelling.
    struct Plasma6Only {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Runner for Plasma6Only {
        fn output(&self, program: &str, args: &[&str]) -> Result<String, String> {
            let line = std::iter::once(program.to_string())
                .chain(args.iter().map(|a| a.to_string()))
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.lock().unwrap().push(line);
            if program.ends_with('5') {
                return Err(format!("{program}: No such file or directory (os error 2)"));
            }
            Ok(String::new()) // `kreadconfig6`: key absent
        }
    }

    #[test]
    fn kde_falls_back_to_the_kf6_tools_when_5_is_absent() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let sp = SysProxy::with_runner(
            Desktop::Kde,
            Arc::new(Plasma6Only {
                calls: Arc::clone(&calls),
            }),
        );
        let EnableOutcome::Applied(snap) = sp.enable(1082, SysProxyPref::Desktop).unwrap() else {
            panic!("expected Applied");
        };
        assert_eq!(snap.backend, "kde");

        let calls = calls.lock().unwrap().clone();
        assert!(
            calls.iter().any(|c| c.starts_with("kreadconfig6 ")),
            "reads must retry with kreadconfig6: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.starts_with("kwriteconfig6 ")),
            "writes must retry with kwriteconfig6: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .filter(|c| c.starts_with("kwriteconfig6"))
                .count()
                >= 3,
            "all three keys must be written through the KF6 tool: {calls:?}"
        );
    }

    /// `SysProxyPref::EnvVar` must bypass the desktop strategy entirely.
    #[test]
    fn env_var_preference_writes_the_shell_rc_instead_of_the_desktop() {
        if env::detect().is_err() {
            eprintln!("no shell detectable here — env dispatch skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("ssr-env-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rc = dir.join("bashrc");
        std::fs::write(&rc, "# rc\n").unwrap();

        let sp = SysProxy::with_runner(Desktop::Unsupported("XFCE".into()), Arc::new(Mock::new()))
            .with_env_rc(rc.clone());
        let EnableOutcome::Applied(snap) = sp.enable(1080, SysProxyPref::EnvVar).unwrap() else {
            panic!("expected Applied");
        };
        assert_eq!(snap.backend, "env");
        let text = std::fs::read_to_string(&rc).unwrap();
        assert!(text.contains("socks5h://127.0.0.1:1080"), "{text}");

        sp.disable(&snap).unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "# rc\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The desktop preference on an unsupported desktop still degrades to
    /// a hint — the env backend only runs when it is chosen.
    #[test]
    fn desktop_preference_on_unknown_desktop_stays_unsupported() {
        let mock = Mock::new();
        let sp = SysProxy::with_runner(Desktop::Unsupported("sway".into()), Arc::new(mock.clone()));
        match sp.enable(1082, SysProxyPref::Desktop).unwrap() {
            EnableOutcome::Unsupported { .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert!(mock.calls().is_empty());
    }

    #[test]
    fn gnome_failed_write_rolls_back_to_the_snapshot() {
        let mock = Mock::new()
            .on("gsettings get org.gnome.system.proxy mode", Ok("'none'"))
            .on("gsettings get org.gnome.system.proxy.socks host", Ok("''"))
            .on("gsettings get org.gnome.system.proxy.socks port", Ok("0"))
            .on(
                "gsettings set org.gnome.system.proxy.socks port 1082",
                Err("gsettings exited with exit status: 1: schema unavailable"),
            );
        let sp = SysProxy::with_runner(Desktop::Gnome, Arc::new(mock.clone()));
        let err = sp.enable(1082, SysProxyPref::Desktop).unwrap_err();
        assert!(matches!(err, AppError::SysProxy(_)), "{err:?}");

        let calls = mock.calls();
        let after = calls
            .iter()
            .rposition(|c| c == "gsettings set org.gnome.system.proxy.socks port 1082")
            .expect("failing write recorded");
        // The host written a moment earlier must be restored to '' …
        assert!(
            calls[after..]
                .iter()
                .any(|c| c == "gsettings set org.gnome.system.proxy.socks host ''"),
            "rollback must restore the original host: {calls:?}"
        );
        // …and the mode must never have flipped to 'manual'.
        assert!(
            !calls[..after].iter().any(|c| c.ends_with("mode 'manual'")),
            "mode must be written last: {calls:?}"
        );
    }
}
