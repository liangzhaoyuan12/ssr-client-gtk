//! Desktop notifications — the *optional* channel (GOAL 3.4).
//!
//! Policy: [`AdwToast`](libadwaita::Toast) inside the app is the
//! mandatory, always-reachable path (see `ui/toast.rs`, Phase 4); this
//! module only adds an OS-level notification when the platform offers one.
//! **No `unwrap`, no panic** — any failure just returns `false`
//! and the caller falls back to the Toast (ARCHITECTURE §8-14).
//!
//! Backends (GOAL §11 B5):
//! - Unix (Linux/BSD): `notify-send`
//! - macOS: `osascript -e 'display notification …'`
//! - Windows: a detached PowerShell `NotifyIcon` balloon — spawned without
//!   waiting, because waiting out the balloon would block the GTK loop.

use std::process::Command;

#[cfg(not(windows))]
fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Send an OS notification if the platform's tooling is present.
///
/// Returns `true` only when the notification was actually handed to the
/// OS. Missing binary / bad display / no D-Bus → `false`, never a panic.
#[cfg(not(windows))]
pub fn desktop_notify(title: &str, body: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title {}",
            osascript_string(body),
            osascript_string(title)
        );
        return run("osascript", &["-e", &script]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        notify_with("notify-send", "--app-name=ssr-client-gtk", title, body)
    }
}

/// Quote for an `osascript` `-e` script (`"…"` with `\` and `"` escaped).
#[cfg(target_os = "macos")]
fn osascript_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Windows: show a balloon through `System.Windows.Forms.NotifyIcon`.
///
/// Detached on purpose (see the module docs) — `true` means "the balloon
/// process was started", which is the best on offer without pulling in the
/// whole WinRT notification stack.
#[cfg(windows)]
pub fn desktop_notify(title: &str, body: &str) -> bool {
    use std::process::Stdio;

    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms;Add-Type -AssemblyName System.Drawing;\
         $n=New-Object System.Windows.Forms.NotifyIcon;\
         $n.Icon=[System.Drawing.SystemIcons]::Information;$n.Visible=$true;\
         $n.ShowBalloonTip(5000,{}, {}, 'Info');Start-Sleep 6;$n.Dispose()",
        powershell_string(title),
        powershell_string(body)
    );
    Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// Quote for a PowerShell single-quoted literal (`''` escapes a quote).
#[cfg(windows)]
fn powershell_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Spawn `program` with the given notification arguments (test seam).
#[cfg(not(windows))]
fn notify_with(program: &str, app_flag: &str, title: &str, body: &str) -> bool {
    run(program, &[app_flag, "--urgency=normal", title, body])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn missing_binary_returns_false_without_panicking() {
        assert!(
            !notify_with("ssr-client-gtk-definitely-not-a-real-bin", "x", "t", "b"),
            "absent notify-send must degrade, not panic"
        );
    }

    #[test]
    fn real_notify_send_never_panics() {
        // With or without the platform tooling / a display: must return a bool.
        let _ = desktop_notify("ssr-client-gtk", "test notification");
    }
}
