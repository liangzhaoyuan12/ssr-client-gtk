//! Desktop notifications — the *optional* channel (GOAL 3.4).
//!
//! Policy: [`AdwToast`](libadwaita::Toast) inside the app is the
//! mandatory, always-reachable path (see `ui/toast.rs`, Phase 4); this
//! module only adds an OS-level notification when `notify-send` happens to
//! exist. **No `unwrap`, no panic** — any failure just returns `false`
//! and the caller falls back to the Toast (ARCHITECTURE §8-14).

use std::process::Command;

/// Send an OS notification via `notify-send` if it is installed.
///
/// Returns `true` only when the command was spawned *and* exited 0.
/// Missing binary, bad DISPLAY, D-Bus errors → `false`, never a panic.
pub fn desktop_notify(title: &str, body: &str) -> bool {
    notify_with("notify-send", "--app-name=ssr-client-gtk", title, body)
}

/// Spawn `program` with the given notification arguments (test seam).
fn notify_with(program: &str, app_flag: &str, title: &str, body: &str) -> bool {
    Command::new(program)
        .arg(app_flag)
        .arg("--urgency=normal")
        .arg(title)
        .arg(body)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_returns_false_without_panicking() {
        assert!(
            !notify_with("ssr-client-gtk-definitely-not-a-real-bin", "x", "t", "b"),
            "absent notify-send must degrade, not panic"
        );
    }

    #[test]
    fn real_notify_send_never_panics() {
        // With or without notify-send / a display: must return a bool.
        let _ = desktop_notify("ssr-client-gtk", "test notification");
    }
}
