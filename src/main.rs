//! ssr-client-gtk — GTK4 + libadwaita native client for ShadowsocksR,
//! linking `ssr-client-rs` in-process (no sidecar, exactly one port).
//!
//! Threading model (GOAL §5): the GTK main loop owns every widget; worker
//! threads run the proxy and blocking system commands; results travel back
//! over channels. The application_id gives us single-instance behaviour —
//! a second launch presents the existing window.

mod app;
mod config;
mod core;
mod error;
mod i18n;
mod notify;
mod routing;
mod sysproxy;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use app::AppState;
use libadwaita::Application;
use libadwaita::prelude::*;
use ui::window::Ui;

/// application_id — also the binary and desktop-file name basis
/// (invariant 12: three names must match).
pub const APP_ID: &str = "com.liangzhaoyuan12.ssr-client-gtk";

/// Project home — shown by the 关于 dialog (`PROJECT_URL`).
pub const PROJECT_URL: &str = "https://github.com/liangzhaoyuan12/ssr-client-gtk";

/// Author shown in the 关于 dialog.
pub const AUTHOR: &str = "liangzhaoyuan12";

/// Set by [`record_exit_signal`], read by the 100 ms poll started in
/// `connect_activate`. Non-zero means "the session asked us to go away".
static EXIT_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

fn exit_signal_pending() -> bool {
    EXIT_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) != 0
}

/// Async-signal-safe: a single atomic store, nothing else.
#[cfg(unix)]
extern "C" fn record_exit_signal(sig: libc::c_int) {
    EXIT_SIGNAL.store(sig, std::sync::atomic::Ordering::SeqCst);
}

/// SIGHUP / SIGINT / SIGTERM / SIGQUIT would otherwise terminate the process
/// **before** `close-request` runs: the SOCKS listener dies with the process,
/// but the desktop proxy stays enabled and points at a port nobody listens on
/// (实测 SIGTERM → 退出码 143、`ProxyType` 仍是 1)。 The handler only records
/// the signal; the main loop then closes the window, so the regular cleanup
/// (stop proxy → restore system proxy → drop snapshot) runs exactly as it does
/// for a window close. SIGKILL cannot be caught — the leftover snapshot plus
/// `startup_self_heal` covers that case on the next launch.
#[cfg(unix)]
fn install_exit_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = record_exit_signal as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM, libc::SIGQUIT] {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// Windows equivalent of [`install_exit_signal_handlers`]: the console ctrl
/// handler records "go away" for Ctrl-C / Ctrl-Break / closing the console /
/// system shutdown, and the same 100 ms poll turns it into a normal
/// `window.close()` so the desktop proxy is restored (GOAL §11 A2 / Phase 8.6).
/// The value stored is `1` — `exit_signal_pending()` only checks non-zero.
#[cfg(windows)]
fn install_exit_signal_handlers() {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_SHUTDOWN_EVENT,
        SetConsoleCtrlHandler,
    };

    unsafe extern "system" fn on_ctrl_event(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_SHUTDOWN_EVENT => {
                EXIT_SIGNAL.store(1, std::sync::atomic::Ordering::SeqCst);
                1 // TRUE — we handled it; the poll then closes the window
            }
            _ => 0,
        }
    }

    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl_event), 1);
    }
}

/// Turn a recorded exit signal into a normal window close. Runs once — a
/// second activation (single-instance) must not add a second poller.
fn hook_exit_signal_to_close(ui: &Rc<Ui>) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let ui_sig = ui.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        if exit_signal_pending() {
            ui_sig.window.close(); // → close-request → 还原系统代理与快照
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn main() {
    let app = Application::builder().application_id(APP_ID).build();
    // Single window shared across activations.
    let existing: Rc<RefCell<Option<Rc<Ui>>>> = Rc::new(RefCell::new(None));

    let existing2 = existing.clone();
    app.connect_activate(move |app| {
        // Second launch → bring the existing window to the front.
        if let Some(ui) = existing2.borrow().as_ref() {
            ui.window.present();
            return;
        }

        // Startup message-table self-check (reads every Strings field).
        let state = AppState::new();
        if !i18n::self_check(state.strings()) {
            eprintln!(
                "i18n: empty message detected in {}",
                state.lang.get().code()
            );
        }

        let ui = Rc::new(Ui::new(app, &state));
        ui::window::wire(ui.clone(), state.clone());
        ui::window::startup_self_heal(ui.clone(), state.clone());

        // GOAL 7.12 — remember the last selection: reopen the profile the
        // previous run had picked instead of the empty status page, so a
        // returning user never has to click the same row again. A stale or
        // unparseable name falls back to the empty state (only parseable
        // profiles are in `names`).
        if let Some(name) = state.remembered_selection()
            && state.names.borrow().contains(&name)
        {
            ui::window::select_profile(&ui, &state, &name);
        }

        // QA hooks for screenshot verification (no effect unless set):
        //   SSR_GTK_DEV_SELECT=<profile> → preselect + open its dashboard
        //   SSR_GTK_DEV_FORM=new|edit    → open the config form
        if let Ok(name) = std::env::var("SSR_GTK_DEV_SELECT")
            && state.names.borrow().contains(&name)
        {
            ui::window::select_profile(&ui, &state, &name);
        }
        if let Ok(lang) = std::env::var("SSR_GTK_DEV_LANG") {
            let idx = u32::from(lang != "zh");
            ui.lang_dropdown.set_selected(idx);
        }
        //   SSR_GTK_DEV_ROUTE=<id>     → routing dropdown (real notify signal)
        //   SSR_GTK_DEV_DNS=custom|system / SSR_GTK_DEV_SYSPROXY=desktop|env
        if let Ok(mode) = std::env::var("SSR_GTK_DEV_ROUTE") {
            let parsed = serde_json::from_str::<routing::RouteMode>(&format!("\"{mode}\""));
            if let Ok(wanted) = parsed
                && let Some(idx) = routing::RouteMode::ALL.iter().position(|m| *m == wanted)
            {
                ui.dash.route_mode.set_selected(idx as u32);
            }
        }
        if let Ok(dns) = std::env::var("SSR_GTK_DEV_DNS").as_deref() {
            let idx = match dns {
                "system" => 0,
                "ali" => 2,
                "tencent" => 3,
                _ => 1, // custom
            };
            ui.dash.dns_mode.set_selected(idx);
        }
        if let Ok(sp) = std::env::var("SSR_GTK_DEV_SYSPROXY").as_deref() {
            ui.dash.sysproxy_mode.set_selected(u32::from(sp == "env"));
        }
        //   SSR_GTK_DEV_MAXIMIZE=1 / SSR_GTK_DEV_SCROLL=end → fit more of the
        //   dashboard into a verification screenshot (windowing only).
        if std::env::var("SSR_GTK_DEV_MAXIMIZE").is_ok() {
            ui.window.maximize();
        }
        if std::env::var("SSR_GTK_DEV_SCROLL").as_deref() == Ok("end")
            && let Some(sw) = ui
                .dash
                .builder
                .object::<gtk::ScrolledWindow>("dashboard_root")
        {
            let adj = sw.vadjustment();
            glib::timeout_add_local(std::time::Duration::from_millis(1800), move || {
                adj.set_value((adj.upper() - adj.page_size()).max(0.0));
                glib::ControlFlow::Break
            });
        }
        //   SSR_GTK_DEV_PROXY=enable|disable  → drive the real toggle path
        //   SSR_GTK_DEV_DISABLE_AFTER=<secs>  → scheduled disable (real signal path)
        //   SSR_GTK_DEV_SELFCLOSE=<secs>      → close() → real close-request cleanup
        match std::env::var("SSR_GTK_DEV_PROXY").as_deref() {
            Ok("enable") if !state.proxy.is_running() => ui::window::toggle_proxy(&ui, &state),
            Ok("disable") if state.proxy.is_running() => ui::window::toggle_proxy(&ui, &state),
            _ => {}
        }
        if let Some(secs) = std::env::var("SSR_GTK_DEV_DISABLE_AFTER")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            let ui_t = ui.clone();
            let state_t = state.clone();
            glib::timeout_add_local(std::time::Duration::from_secs(secs), move || {
                if state_t.proxy.is_running() {
                    ui::window::toggle_proxy(&ui_t, &state_t);
                }
                glib::ControlFlow::Break
            });
        }
        if let Some(secs) = std::env::var("SSR_GTK_DEV_SELFCLOSE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            let win = ui.window.clone();
            glib::timeout_add_local(std::time::Duration::from_secs(secs), move || {
                win.close();
                glib::ControlFlow::Break
            });
        }
        if let Ok(mode) = std::env::var("SSR_GTK_DEV_FORM") {
            let target = if mode == "edit" {
                state.selected.borrow().clone()
            } else {
                None
            };
            ui::window::open_form(&ui, &state, target.as_deref());
        }

        // Click-chain QA hooks — the button press itself cannot be
        // synthesised from outside this session (no input tool), so the
        // handlers are invoked the way GTK would: `clicked`/`response`
        // signals through the real connected closures.
        //   SSR_GTK_DEV_AUTOCREATE=<name> → fill new form + click Save
        //   SSR_GTK_DEV_AUTOSAVE=1        → in edit mode, click Save
        //   SSR_GTK_DEV_AUTODELETE=1      → confirm dialog "delete" response
        if let Ok(name) = std::env::var("SSR_GTK_DEV_AUTOCREATE") {
            ui::window::open_form(&ui, &state, None);
            ui.form.entry_name.set_text(&name);
            ui.form.entry_server.set_text("127.0.0.1");
            ui.form.spin_server_port.set_value(2800.0);
            ui.form.entry_password.set_text("devpass");
            ui.form.spin_listen.set_value(1083.0);
            ui.form.btn_save.emit_by_name::<()>("clicked", &[]);
        }
        if std::env::var("SSR_GTK_DEV_AUTOSAVE").is_ok() {
            ui.form.btn_save.emit_by_name::<()>("clicked", &[]);
        }
        if std::env::var("SSR_GTK_DEV_AUTODELETE").is_ok() {
            let selected = state.selected.borrow().clone();
            if let Some(name) = selected {
                let dialog = ui::list::confirm_delete(&ui, &state, &name);
                // Delay the response so a screenshot can catch the dialog.
                glib::timeout_add_local(std::time::Duration::from_millis(1500), move || {
                    dialog.emit_by_name::<()>("response", &[&"delete" as &dyn ToValue]);
                    glib::ControlFlow::Break
                });
            }
        }

        ui.window.present();

        // SIGHUP / SIGINT / SIGTERM → normal close, so the proxy is stopped
        // and the desktop proxy restored instead of being left dangling.
        install_exit_signal_handlers();
        hook_exit_signal_to_close(&ui);

        //   SSR_GTK_DEV_ABOUT=1 → open the 关于 dialog, but only after the
        //   window is mapped (a dialog shown first has no parent to grab).
        if std::env::var("SSR_GTK_DEV_ABOUT").is_ok() {
            let ui_about = ui.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(600), move || {
                // Fire the real button's `clicked` signal so this exercises the
                // button → dialog wiring, not a second copy of it.
                ui_about.about_btn.emit_by_name::<()>("clicked", &[]);
                glib::ControlFlow::Break
            });
        }

        *existing2.borrow_mut() = Some(ui);
    });

    app.run();
}
