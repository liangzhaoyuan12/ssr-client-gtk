//! In-app notifications: `AdwToast` is the mandatory channel (GOAL 3.4).

use libadwaita::{Toast, ToastOverlay};

/// Show a toast; always delivered, never panics.
pub fn toast(overlay: &ToastOverlay, message: &str) {
    overlay.add_toast(Toast::new(message));
}
