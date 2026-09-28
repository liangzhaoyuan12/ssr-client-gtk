//! Windows system proxy through the WinINET registry keys
//! (GOAL §11 B2, decision D10).
//!
//! Windows — WinINET, the Settings proxy page and the Chromium family —
//! does **not** understand SOCKS, so what we publish is our **HTTP**
//! front-end on the single exposed port:
//! `ProxyServer = 127.0.0.1:<port>` with **no scheme prefix**, which those
//! consumers treat as an HTTP proxy. The HTTP/`CONNECT` conversion itself
//! HTTP/`CONNECT` conversion itself lives in [`crate::core::http_proxy`]; this module only points Windows
//! at it. The GUI's "环境变量" row is greyed out on Windows (decision
//! `D14`) — there is no shell-rc strategy there, and a stale preference
//! naming one falls back to this registry backend.
//!
//! Everything goes through the injectable [`SysProxy::run`] /
//! [`SysProxy::read`] seam, so tests assert exact `reg` command lines
//! without touching a real registry.

use crate::error::{AppError, AppResult};

use super::{Setting, Snapshot, SysProxy};

/// Backend id stored in [`Snapshot::backend`].
pub(crate) const BACKEND: &str = "windows";

/// Per-user WinINET settings.
const INTERNET_SETTINGS: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// Values we manage, in snapshot/restore order (address first, `ProxyEnable`
/// last — same ordering rule as the KDE/GNOME backends: Windows must never
/// see "enabled" with a stale address).
const KEYS: [(&str, &str); 3] = [
    ("ProxyServer", "REG_SZ"),
    ("ProxyOverride", "REG_SZ"),
    ("ProxyEnable", "REG_DWORD"),
];

/// Read one value. `reg query` exits non-zero for a missing value, which
/// [`SysProxy::read`] already maps to `Ok(None)`.
fn get(sp: &SysProxy, name: &str) -> AppResult<Option<String>> {
    let raw = sp.read("reg", &["query", INTERNET_SETTINGS, "/v", name])?;
    Ok(raw.and_then(|out| parse_value(&out, name)))
}

/// Pull the value out of `reg query` output:
/// `<value name>  <REG_TYPE>  <value…>` (the value may contain spaces).
fn parse_value(out: &str, name: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .find(|line| line.split_whitespace().next() == Some(name))
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            parts.next()?; // value name
            parts.next()?; // REG_SZ / REG_DWORD
            let value = parts.collect::<Vec<_>>().join(" ");
            (!value.is_empty()).then_some(value)
        })
}

fn set(sp: &SysProxy, name: &str, kind: &str, value: &str) -> AppResult<()> {
    sp.run(
        "reg",
        &[
            "add",
            INTERNET_SETTINGS,
            "/v",
            name,
            "/t",
            kind,
            "/d",
            value,
            "/f",
        ],
    )?;
    Ok(())
}

/// Delete a value that did not exist before we wrote it. A value that is
/// already gone (`exited with`) is not an error — the desired state is
/// reached either way.
fn delete(sp: &SysProxy, name: &str) -> AppResult<()> {
    match sp.run("reg", &["delete", INTERNET_SETTINGS, "/v", name, "/f"]) {
        Ok(_) => Ok(()),
        Err(AppError::SysProxy(msg)) if msg.contains("exited with") => Ok(()),
        Err(e) => Err(e),
    }
}

/// Ask WinINET to re-read its settings
/// (`INTERNET_OPTION_SETTINGS_CHANGED` = 39, `INTERNET_OPTION_REFRESH` = 37).
/// Best effort: without it the new proxy still applies on the next refresh
/// (or next logon), so a missing PowerShell must not fail the enable path.
fn broadcast(sp: &SysProxy) {
    let script = "$sig='[DllImport(\"wininet.dll\",SetLastError=true)] public static extern bool InternetSetOption(int h,int o,int b,int l);';$t=Add-Type -MemberDefinition $sig -Name WinInet -Namespace Win32 -PassThru;$t::InternetSetOption(0,39,0,0)|Out-Null;$t::InternetSetOption(0,37,0,0)|Out-Null";
    let _ = sp.run("powershell", &["-NoProfile", "-Command", script]);
}

/// Snapshot the three values, then point Windows at our HTTP front-end.
///
/// Ordering: `ProxyServer` → `ProxyOverride` → `ProxyEnable` last, and a
/// failing write rolls back to the snapshot (all-or-nothing, like GNOME).
pub(crate) fn enable(sp: &SysProxy, port: u16) -> AppResult<Snapshot> {
    let mut settings = Vec::with_capacity(KEYS.len());
    for (name, kind) in KEYS {
        settings.push(Setting {
            group: kind.to_string(),
            key: name.to_string(),
            value: get(sp, name)?,
        });
    }
    let snap = Snapshot {
        backend: BACKEND.into(),
        settings,
    };

    let writes = [
        ("ProxyServer", "REG_SZ", format!("127.0.0.1:{port}")),
        (
            "ProxyOverride",
            "REG_SZ",
            "<local>;127.0.0.1;localhost".to_string(),
        ),
        ("ProxyEnable", "REG_DWORD", "1".to_string()),
    ];
    for (name, kind, value) in writes {
        if let Err(e) = set(sp, name, kind, &value) {
            let _ = restore(sp, &snap); // all-or-nothing
            return Err(e);
        }
    }
    broadcast(sp);
    Ok(snap)
}

/// Put the three values back exactly (values that were absent are
/// deleted), then refresh WinINET.
pub(crate) fn restore(sp: &SysProxy, snap: &Snapshot) -> AppResult<()> {
    for setting in &snap.settings {
        match &setting.value {
            Some(value) => set(sp, &setting.key, &setting.group, value)?,
            None => delete(sp, &setting.key)?,
        }
    }
    broadcast(sp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysproxy::Mock;

    const KEY_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    /// Existing settings on the "machine": address set, override absent.
    fn win_mock() -> Mock {
        Mock::new()
            .on(
                &format!("reg query {KEY_PATH} /v ProxyServer"),
                Ok("HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyServer    REG_SZ    127.0.0.1:1080\r\n"),
            )
            .on(
                &format!("reg query {KEY_PATH} /v ProxyEnable"),
                Ok("HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x1\r\n"),
            )
            .on(
                &format!("reg query {KEY_PATH} /v ProxyOverride"),
                Err("reg exited with exit code: 1."),
            )
    }

    #[test]
    fn parses_reg_query_output() {
        let out = "HKEY_CURRENT_USER\\…\r\n    ProxyServer    REG_SZ    127.0.0.1:1080\r\n";
        assert_eq!(
            parse_value(out, "ProxyServer").as_deref(),
            Some("127.0.0.1:1080")
        );
        assert_eq!(parse_value(out, "ProxyEnable"), None);
        let out = "    ProxyOverride    REG_SZ    <local>;127.0.0.1\r\n";
        assert_eq!(
            parse_value(out, "ProxyOverride").as_deref(),
            Some("<local>;127.0.0.1"),
            "values with spaces must survive"
        );
    }

    #[test]
    fn enable_writes_the_http_proxy_and_snapshots_the_originals() {
        let mock = win_mock();
        let sp = SysProxy::with_runner(
            crate::sysproxy::Desktop::Windows,
            std::sync::Arc::new(mock.clone()),
        );

        let snap = enable(&sp, 1082).expect("enable");

        assert_eq!(snap.backend, "windows");
        assert_eq!(
            snap.settings,
            vec![
                Setting {
                    group: "REG_SZ".into(),
                    key: "ProxyServer".into(),
                    value: Some("127.0.0.1:1080".into()),
                },
                Setting {
                    group: "REG_SZ".into(),
                    key: "ProxyOverride".into(),
                    value: None,
                },
                Setting {
                    group: "REG_DWORD".into(),
                    key: "ProxyEnable".into(),
                    value: Some("0x1".into()),
                },
            ]
        );

        let calls = mock.calls();
        // Reads come first — nothing may be written before the snapshot.
        let first_write = calls
            .iter()
            .position(|c| c.starts_with("reg add"))
            .expect("at least one reg add");
        assert!(
            calls[..first_write]
                .iter()
                .all(|c| c.starts_with("reg query"))
        );
        // No scheme prefix: this is the HTTP proxy, not a SOCKS one (D10).
        assert!(
            calls.contains(&format!(
                "reg add {KEY_PATH} /v ProxyServer /t REG_SZ /d 127.0.0.1:1082 /f"
            )),
            "{calls:?}"
        );
        assert!(
            calls.contains(&format!(
                "reg add {KEY_PATH} /v ProxyEnable /t REG_DWORD /d 1 /f"
            )),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("powershell -NoProfile -Command")),
            "WinINET must be told to refresh: {calls:?}"
        );
        // Order: address → bypass → enable (never enabled with a stale address).
        let pos = |needle: &str| calls.iter().position(|c| c.contains(needle)).unwrap();
        assert!(pos("ProxyServer /t REG_SZ /d 127.0.0.1:1082") < pos("ProxyEnable /t"));
    }

    #[test]
    fn restore_puts_back_originals_and_deletes_absent_values() {
        let mock = win_mock();
        let sp = SysProxy::with_runner(
            crate::sysproxy::Desktop::Windows,
            std::sync::Arc::new(mock.clone()),
        );
        let snap = enable(&sp, 1082).expect("enable");
        let after_enable = mock.calls().len();

        restore(&sp, &snap).expect("restore");
        let calls = mock.calls()[after_enable..].to_vec();

        assert!(
            calls.contains(&format!(
                "reg add {KEY_PATH} /v ProxyServer /t REG_SZ /d 127.0.0.1:1080 /f"
            )),
            "original address must come back: {calls:?}"
        );
        assert!(
            calls.contains(&format!(
                "reg add {KEY_PATH} /v ProxyEnable /t REG_DWORD /d 0x1 /f"
            )),
            "original ProxyEnable must come back: {calls:?}"
        );
        assert!(
            calls.contains(&format!("reg delete {KEY_PATH} /v ProxyOverride /f")),
            "a value that did not exist must be deleted, not blanked: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with("powershell -NoProfile -Command")),
            "WinINET must be refreshed after restore: {calls:?}"
        );
    }

    #[test]
    fn a_failed_write_rolls_back_to_the_snapshot() {
        let mock = Mock::new()
            .on(
                &format!("reg query {KEY_PATH} /v ProxyServer"),
                Err("reg exited with exit code: 1."),
            )
            .on(
                &format!("reg query {KEY_PATH} /v ProxyOverride"),
                Err("reg exited with exit code: 1."),
            )
            .on(
                &format!("reg query {KEY_PATH} /v ProxyEnable"),
                Err("reg exited with exit code: 1."),
            )
            .on(
                &format!("reg add {KEY_PATH} /v ProxyServer /t REG_SZ /d 127.0.0.1:1082 /f"),
                Err("reg exited with exit code: 5."),
            );
        let sp = SysProxy::with_runner(
            crate::sysproxy::Desktop::Windows,
            std::sync::Arc::new(mock.clone()),
        );

        let err = enable(&sp, 1082).expect_err("write must fail");
        assert!(matches!(err, AppError::SysProxy(_)), "{err:?}");

        let calls = mock.calls();
        // Rollback deleted everything we had created (all three were absent).
        assert!(
            calls
                .iter()
                .any(|c| c.starts_with(&format!("reg delete {KEY_PATH}"))),
            "rollback must run: {calls:?}"
        );
        // …and never wrote ProxyEnable after the failure.
        assert!(
            !calls.iter().any(|c| c.contains("/v ProxyEnable /t")),
            "no half-enabled state may be left: {calls:?}"
        );
    }
}
