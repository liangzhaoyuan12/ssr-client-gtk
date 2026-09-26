//! KDE Plasma system proxy via `kreadconfig5` / `kwriteconfig5` on
//! `kioslaverc`, followed by a KIO `reparseSlaveConfiguration` broadcast.
//!
//! Verified against this machine's tooling (GOAL §2.3): `kwriteconfig5`
//! takes the value as a trailing positional argument.
//!
//! The file name is a parameter of the internal helpers so the real-command
//! smoke test can write to `kioslaverc-ssr-client-gtk-test` instead of the
//! live `kioslaverc` — production entry points always pass [`FILE`].

use crate::error::AppResult;

use super::{Setting, Snapshot, SysProxy};

/// Keys we manage (the same trio the old `ssr-enable.sh` wrote).
const KEYS: [&str; 3] = ["ProxyType", "socksProxy", "NoProxyFor"];

/// Production config file (relative to `~/.config`).
const FILE: &str = "kioslaverc";
const GROUP: &str = "Proxy Settings";

fn read_key(sp: &SysProxy, file: &str, key: &str) -> AppResult<Option<String>> {
    sp.read(
        "kreadconfig5",
        &["--file", file, "--group", GROUP, "--key", key],
    )
}

fn write_key(sp: &SysProxy, file: &str, key: &str, value: &str) -> AppResult<()> {
    sp.run(
        "kwriteconfig5",
        &["--file", file, "--group", GROUP, "--key", key, value],
    )?;
    Ok(())
}

fn delete_key(sp: &SysProxy, file: &str, key: &str) -> AppResult<()> {
    sp.run(
        "kwriteconfig5",
        &["--file", file, "--group", GROUP, "--key", key, "--delete"],
    )?;
    Ok(())
}

/// Ask KIO to re-read its configuration (must be the last step).
fn reparse(sp: &SysProxy) -> AppResult<()> {
    sp.run(
        "dbus-send",
        &[
            "--type=signal",
            "/KIO/Scheduler",
            "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
            "string:",
        ],
    )?;
    Ok(())
}

/// Snapshot `file`'s current settings and point it at our SOCKS5
/// (**without** triggering a KIO reparse — callers add that).
fn apply(sp: &SysProxy, file: &str, port: u16) -> AppResult<Snapshot> {
    let mut settings = Vec::with_capacity(KEYS.len());
    for key in KEYS {
        settings.push(Setting {
            group: String::new(),
            key: key.to_string(),
            value: read_key(sp, file, key)?,
        });
    }

    write_key(sp, file, "ProxyType", "1")?;
    write_key(sp, file, "socksProxy", &format!("127.0.0.1 {port}"))?;
    write_key(sp, file, "NoProxyFor", "localhost,127.0.0.1,::1")?;

    Ok(Snapshot {
        backend: "kde".into(),
        settings,
    })
}

/// Write the snapshot back exactly (deleting keys that did not exist),
/// **without** a KIO reparse — callers add that.
fn undo(sp: &SysProxy, file: &str, snap: &Snapshot) -> AppResult<()> {
    for setting in &snap.settings {
        match &setting.value {
            Some(value) => write_key(sp, file, &setting.key, value)?,
            None => delete_key(sp, file, &setting.key)?,
        }
    }
    Ok(())
}

/// Snapshot current settings, point KIO at our SOCKS5, reload KIO.
pub(crate) fn enable(sp: &SysProxy, port: u16) -> AppResult<Snapshot> {
    let snap = apply(sp, FILE, port)?;
    reparse(sp)?;
    Ok(snap)
}

/// Write the snapshot back exactly, then reload KIO.
pub(crate) fn restore(sp: &SysProxy, snap: &Snapshot) -> AppResult<()> {
    undo(sp, FILE, snap)?;
    reparse(sp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysproxy::{CommandRunner, Desktop};
    use std::sync::Arc;

    /// Real-command smoke (GOAL 3.3): write → read back → restore → read
    /// back, against a **throwaway** kwriteconfig5 file so the live
    /// `kioslaverc` and the user's running proxy are never touched.
    ///
    /// Ignored in the default suite to keep it hermetic (needs KDE's
    /// kwriteconfig5/kreadconfig5). Run: `cargo test kde_real -- --ignored`
    #[test]
    #[ignore = "needs kwriteconfig5/kreadconfig5 on the machine"]
    fn kde_real_command_smoke() {
        let sp = SysProxy::with_runner(Desktop::Kde, Arc::new(CommandRunner));
        let file = "kioslaverc-ssr-client-gtk-test";

        // Tooling present?
        if sp.run("kwriteconfig5", &["--help"]).is_err() {
            eprintln!("kwriteconfig5 unavailable — smoke skipped");
            return;
        }

        // Seed known originals (also cleans leftovers from a prior run).
        for key in KEYS {
            let _ = delete_key(&sp, file, key);
        }
        write_key(&sp, file, "ProxyType", "0").unwrap();
        write_key(&sp, file, "socksProxy", "127.0.0.1 19999").unwrap();
        write_key(&sp, file, "NoProxyFor", "example.com").unwrap();

        // enable path (minus the broadcast) and read back through the real
        // kreadconfig5: the written socks port must equal listen_port.
        let listen_port = 12345u16;
        let snap = apply(&sp, file, listen_port).expect("apply");
        assert_eq!(
            read_key(&sp, file, "socksProxy").unwrap().as_deref(),
            Some("127.0.0.1 12345"),
            "real kreadconfig5 must read back the configured listen_port"
        );
        assert_eq!(
            read_key(&sp, file, "ProxyType").unwrap().as_deref(),
            Some("1")
        );
        // Snapshot captured the originals before overwriting.
        assert_eq!(
            snap.settings,
            vec![
                Setting::set("ProxyType", "0"),
                Setting::set("socksProxy", "127.0.0.1 19999"),
                Setting::set("NoProxyFor", "example.com"),
            ]
        );

        // restore path: originals back, byte for byte.
        undo(&sp, file, &snap).expect("undo");
        assert_eq!(
            read_key(&sp, file, "ProxyType").unwrap().as_deref(),
            Some("0")
        );
        assert_eq!(
            read_key(&sp, file, "socksProxy").unwrap().as_deref(),
            Some("127.0.0.1 19999")
        );
        assert_eq!(
            read_key(&sp, file, "NoProxyFor").unwrap().as_deref(),
            Some("example.com")
        );

        // Cleanup: remove keys and the throwaway file itself.
        for key in KEYS {
            delete_key(&sp, file, key).unwrap();
        }
        let _ = std::fs::remove_file(crate::sysproxy::snapshot::Snapshot::config_dir().join(file));
        assert!(
            !crate::sysproxy::snapshot::Snapshot::config_dir()
                .join(file)
                .exists(),
            "throwaway file must be gone"
        );
    }
}
