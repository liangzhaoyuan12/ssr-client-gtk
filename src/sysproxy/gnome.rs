//! GNOME (and derivatives) system proxy via `gsettings`.
//!
//! Keys: `org.gnome.system.proxy mode` plus the SOCKS host/port in
//! `org.gnome.system.proxy.socks`. Values are kept as raw GVariant text so
//! `gsettings set` can take them back verbatim on restore.

use crate::error::AppResult;

use super::{Setting, Snapshot, SysProxy};

const SCHEMA_MODE: &str = "org.gnome.system.proxy";
const SCHEMA_SOCKS: &str = "org.gnome.system.proxy.socks";

fn get(sp: &SysProxy, schema: &str, key: &str) -> AppResult<Option<String>> {
    sp.read("gsettings", &["get", schema, key])
}

fn set(sp: &SysProxy, schema: &str, key: &str, value: &str) -> AppResult<()> {
    sp.run("gsettings", &["set", schema, key, value])?;
    Ok(())
}

fn reset(sp: &SysProxy, schema: &str, key: &str) -> AppResult<()> {
    sp.run("gsettings", &["reset", schema, key])?;
    Ok(())
}

/// Snapshot mode/host/port, point GNOME at our SOCKS5.
///
/// Ordering: host → port → `mode = 'manual'` last, so GNOME never sees a
/// `manual` mode with a half-written address. A failing write rolls the
/// keys already written back to the snapshot, so a broken `gsettings` can
/// never leave the desktop pointed at a port we are about to close.
pub(crate) fn enable(sp: &SysProxy, port: u16) -> AppResult<Snapshot> {
    let host = get(sp, SCHEMA_SOCKS, "host")?;
    let socks_port = get(sp, SCHEMA_SOCKS, "port")?;
    let mode = get(sp, SCHEMA_MODE, "mode")?;

    let snap = Snapshot {
        backend: "gnome".into(),
        settings: vec![
            Setting {
                group: SCHEMA_SOCKS.to_string(),
                key: "host".to_string(),
                value: host,
            },
            Setting {
                group: SCHEMA_SOCKS.to_string(),
                key: "port".to_string(),
                value: socks_port,
            },
            Setting {
                group: SCHEMA_MODE.to_string(),
                key: "mode".to_string(),
                value: mode,
            },
        ],
    };

    let writes: [(&str, &str, String); 3] = [
        (SCHEMA_SOCKS, "host", "'127.0.0.1'".to_string()),
        (SCHEMA_SOCKS, "port", port.to_string()),
        (SCHEMA_MODE, "mode", "'manual'".to_string()),
    ];
    for (schema, key, value) in writes {
        if let Err(e) = set(sp, schema, key, &value) {
            let _ = restore(sp, &snap); // all-or-nothing
            return Err(e);
        }
    }
    Ok(snap)
}

/// Restore host → port → mode (mode last, mirroring `enable`); settings
/// that did not exist before are `gsettings reset` rather than blanked.
pub(crate) fn restore(sp: &SysProxy, snap: &Snapshot) -> AppResult<()> {
    for setting in &snap.settings {
        match &setting.value {
            Some(value) => set(sp, &setting.group, &setting.key, value)?,
            None => reset(sp, &setting.group, &setting.key)?,
        }
    }
    Ok(())
}
