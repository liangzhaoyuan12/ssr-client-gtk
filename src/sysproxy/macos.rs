//! macOS system proxy via `networksetup` (GOAL §11 B3, decision D11).
//!
//! macOS speaks SOCKS natively (`-setsocksfirewallproxy`), so the desktop
//! keeps pointing at our SOCKS5 listener — no HTTP conversion needed on this
//! platform (that is Windows-only, see [`crate::core::http_proxy`]).
//!
//! Decision D11 = **M1 `networksetup` is the primary path** (this module),
//! **M2 = `~/.zshrc` env block** stays available through the GUI's
//! "环境变量" option (`super::env`), which macOS supports because it is
//! POSIX like Linux.
//!
//! Every command goes through the injectable [`SysProxy::run`] /
//! [`SysProxy::read`] seam, so tests assert exact command lines without
//! touching a real network configuration.

use crate::error::AppResult;

use super::{Setting, Snapshot, SysProxy};

/// Backend id stored in [`Snapshot::backend`].
pub(crate) const BACKEND: &str = "macos";

/// `networksetup -listallnetworkservices`:
/// first line is the legend, disabled services are prefixed with `*`.
fn services(sp: &SysProxy) -> AppResult<Vec<String>> {
    let out = sp.run("networksetup", &["-listallnetworkservices"])?;
    Ok(out
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('*'))
        .map(str::to_string)
        .collect())
}

/// `-getsocksfirewallproxy` reply → one storable line: `enabled|server|port`.
fn parse_socks_state(out: &str) -> String {
    let mut enabled = "0";
    let mut server = String::new();
    let mut port = String::from("0");
    for line in out.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("Enabled:") {
            enabled = if v.trim().eq_ignore_ascii_case("yes") {
                "1"
            } else {
                "0"
            };
        } else if let Some(v) = line.strip_prefix("Server:") {
            server = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Port:") {
            port = v.trim().to_string();
        }
    }
    format!("{enabled}|{server}|{port}")
}

/// Inverse of [`parse_socks_state`].
fn split_socks_state(stored: &str) -> Option<(bool, String, String)> {
    let mut parts = stored.splitn(3, '|');
    let enabled = parts.next()?;
    let server = parts.next()?.to_string();
    let port = parts.next()?.to_string();
    Some((enabled == "1", server, port))
}

fn set_proxy(sp: &SysProxy, service: &str, host: &str, port: &str) -> AppResult<()> {
    sp.run(
        "networksetup",
        &["-setsocksfirewallproxy", service, host, port],
    )?;
    Ok(())
}

fn set_enabled(sp: &SysProxy, service: &str, on: bool) -> AppResult<()> {
    let flag = if on {
        "-setsocksfirewallon"
    } else {
        "-setsocksfirewalloff"
    };
    sp.run("networksetup", &[flag, service])?;
    Ok(())
}

/// Snapshot every service's SOCKS state, then point them all at us.
///
/// Ordering per service: set the address first, flip the switch last — the
/// desktop never sees "enabled" pointing at a stale address. A failing
/// write rolls the already-configured services back to the snapshot.
pub(crate) fn enable(sp: &SysProxy, port: u16) -> AppResult<Snapshot> {
    let services = services(sp)?;
    let mut settings = Vec::with_capacity(services.len());
    for service in &services {
        let state = sp.read("networksetup", &["-getsocksfirewallproxy", service])?;
        settings.push(Setting {
            group: service.clone(),
            key: "socks".to_string(),
            value: state.map(|out| parse_socks_state(&out)),
        });
    }
    let snap = Snapshot {
        backend: BACKEND.into(),
        settings,
    };

    for service in &services {
        if let Err(e) = set_proxy(sp, service, "127.0.0.1", &port.to_string())
            .and_then(|()| set_enabled(sp, service, true))
        {
            let _ = restore(sp, &snap); // all-or-nothing
            return Err(e);
        }
    }
    Ok(snap)
}

/// Put every service back: original address (when it had one) and the
/// original on/off switch; a service we could not read is switched off so
/// no `127.0.0.1:<port>` pointing at a closed port survives.
pub(crate) fn restore(sp: &SysProxy, snap: &Snapshot) -> AppResult<()> {
    for setting in &snap.settings {
        let service = setting.group.as_str();
        let stored = setting.value.as_deref();
        match stored.and_then(split_socks_state) {
            Some((enabled, server, port)) => {
                // `Enabled: Yes` with no usable address (`1||0`) is a state
                // networksetup can report: re-enabling it would leave
                // *whatever address is installed right now* — ours — behind
                // after we exit, pointing the machine at a closed port.
                let addressable = !server.is_empty() && !port.is_empty() && port != "0";
                if !addressable {
                    set_enabled(sp, service, false)?;
                    continue;
                }
                set_proxy(sp, service, &server, &port)?;
                set_enabled(sp, service, enabled)?;
            }
            None => set_enabled(sp, service, false)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysproxy::Mock;
    use std::sync::Arc;

    /// Two active services, one disabled (`*Bluetooth PAN` must be skipped).
    fn mac_mock() -> Mock {
        Mock::new()
            .on(
                "networksetup -listallnetworkservices",
                Ok("An asterisk (*) denotes that a network service is disabled.\nWi-Fi\nEthernet\n*Bluetooth PAN"),
            )
            .on(
                "networksetup -getsocksfirewallproxy Wi-Fi",
                Ok("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0"),
            )
            .on(
                "networksetup -getsocksfirewallproxy Ethernet",
                Ok("Enabled: Yes\nServer: old.proxy\nPort: 1080\nAuthenticated Proxy Enabled: 0"),
            )
    }

    fn sp_with(mock: Mock) -> SysProxy {
        SysProxy::with_runner(crate::sysproxy::Desktop::Macos, Arc::new(mock))
    }

    #[test]
    fn parses_networksetup_replies() {
        assert_eq!(parse_socks_state("Enabled: No\nServer: \nPort: 0"), "0||0");
        assert_eq!(
            parse_socks_state("Enabled: Yes\nServer: old.proxy\nPort: 1080"),
            "1|old.proxy|1080"
        );
        assert_eq!(
            split_socks_state("1|old.proxy|1080"),
            Some((true, "old.proxy".into(), "1080".into()))
        );
        assert_eq!(split_socks_state("garbage"), None);
    }

    #[test]
    fn enable_points_every_active_service_at_us_and_snapshots_them() {
        let mock = mac_mock();
        let sp = sp_with(mock.clone());

        let snap = enable(&sp, 1082).expect("enable");

        assert_eq!(snap.backend, "macos");
        assert_eq!(
            snap.settings,
            vec![
                Setting {
                    group: "Wi-Fi".into(),
                    key: "socks".into(),
                    value: Some("0||0".into()),
                },
                Setting {
                    group: "Ethernet".into(),
                    key: "socks".into(),
                    value: Some("1|old.proxy|1080".into()),
                },
            ]
        );

        let calls = mock.calls();
        assert!(
            !calls.iter().any(|c| c.contains("Bluetooth")),
            "disabled services must be skipped: {calls:?}"
        );
        for svc in ["Wi-Fi", "Ethernet"] {
            assert!(
                calls.contains(&format!(
                    "networksetup -setsocksfirewallproxy {svc} 127.0.0.1 1082"
                )),
                "{calls:?}"
            );
            assert!(
                calls.contains(&format!("networksetup -setsocksfirewallon {svc}")),
                "{calls:?}"
            );
        }
        // Reads before writes — the snapshot must exist first.
        let first_write = calls
            .iter()
            .position(|c| c.contains("-setsocksfirewallproxy"))
            .expect("a write");
        assert!(
            calls[..first_write]
                .iter()
                .all(|c| c.contains("-listallnetworkservices")
                    || c.contains("-getsocksfirewallproxy"))
        );
        // Address before switch, per service.
        let pos = |needle: &str| calls.iter().position(|c| c == needle).unwrap();
        assert!(
            pos("networksetup -setsocksfirewallproxy Wi-Fi 127.0.0.1 1082")
                < pos("networksetup -setsocksfirewallon Wi-Fi")
        );
    }

    #[test]
    fn restore_puts_back_original_state_per_service() {
        let mock = mac_mock();
        let sp = sp_with(mock.clone());
        let snap = enable(&sp, 1082).expect("enable");
        let after_enable = mock.calls().len();

        restore(&sp, &snap).expect("restore");
        let calls = mock.calls()[after_enable..].to_vec();

        // Wi-Fi was off before → left off, no stale address written back.
        assert!(
            calls
                .iter()
                .any(|c| c == "networksetup -setsocksfirewalloff Wi-Fi"),
            "{calls:?}"
        );
        // Ethernet was on with old.proxy:1080 → exactly that comes back.
        assert!(
            calls
                .iter()
                .any(|c| c == "networksetup -setsocksfirewallproxy Ethernet old.proxy 1080"),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c == "networksetup -setsocksfirewallon Ethernet"),
            "{calls:?}"
        );
        let pos = |needle: &str| calls.iter().position(|c| c == needle).unwrap();
        assert!(
            pos("networksetup -setsocksfirewallproxy Ethernet old.proxy 1080")
                < pos("networksetup -setsocksfirewallon Ethernet"),
            "address must be restored before the switch is flipped on"
        );
    }

    #[test]
    fn an_unreadable_service_is_switched_off_on_restore() {
        let snap = Snapshot {
            backend: BACKEND.into(),
            settings: vec![Setting {
                group: "Ghost".into(),
                key: "socks".into(),
                value: None,
            }],
        };
        let mock = Mock::new();
        let sp = sp_with(mock.clone());

        restore(&sp, &snap).expect("restore");
        assert_eq!(
            mock.calls(),
            vec!["networksetup -setsocksfirewalloff Ghost".to_string()],
            "unknown state must never be left pointing at a closed port"
        );
    }

    #[test]
    fn enabled_but_addressless_state_is_restored_as_off() {
        // `Enabled: Yes` with an empty server/port is what networksetup
        // reports for a half-configured SOCKS entry. Re-enabling it would
        // keep OUR 127.0.0.1:<port> installed after we quit.
        let snap = Snapshot {
            backend: BACKEND.into(),
            settings: vec![Setting {
                group: "Wi-Fi".into(),
                key: "socks".into(),
                value: Some("1||0".into()),
            }],
        };
        let mock = Mock::new();
        let sp = sp_with(mock.clone());

        restore(&sp, &snap).expect("restore");
        assert_eq!(
            mock.calls(),
            vec!["networksetup -setsocksfirewalloff Wi-Fi".to_string()],
            "an empty address must come back as OFF, never re-enabled"
        );
    }
}
