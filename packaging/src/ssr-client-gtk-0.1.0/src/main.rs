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

        // QA hooks for screenshot verification (no effect unless set):
        //   SSR_GTK_DEV_SELECT=<profile> → preselect + open its dashboard
        //   SSR_GTK_DEV_FORM=new|edit    → open the config form
        if let Ok(name) = std::env::var("SSR_GTK_DEV_SELECT")
            && state.names.borrow().contains(&name)
        {
            *state.selected.borrow_mut() = Some(name);
            ui::window::show_dashboard(&ui, &state);
        }
        if let Ok(lang) = std::env::var("SSR_GTK_DEV_LANG") {
            let idx = u32::from(lang != "zh");
            ui.lang_dropdown.set_selected(idx);
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

        *existing2.borrow_mut() = Some(ui);
    });

    app.run();
}
