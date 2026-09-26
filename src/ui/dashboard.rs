//! Dashboard: proxy status rendering, the enable/disable flow (all blocking
//! work happens on a worker thread, results come back via channel) and the
//! single-port address card.

use std::rc::Rc;

use gtk::Builder;
use gtk::prelude::*;

use crate::app::AppState;
use crate::config::model::ShadowsocksConfig;
use crate::core::proxy::ProxyState;
use crate::error::AppError;
use crate::i18n::{error_text, fill};
use crate::sysproxy::{EnableOutcome, snapshot::Snapshot};
use crate::ui::toast::toast;
use crate::ui::window::Ui;

/// Handles for the dashboard widgets.
pub struct DashUi {
    /// Builder of `dashboard.ui`.
    pub builder: Builder,
    pub root: gtk::ScrolledWindow,
    pub config_name: gtk::Label,
    pub state_label: gtk::Label,
    pub dot: gtk::Image,
    pub spinner: gtk::Spinner,
    pub btn_toggle: gtk::Button,
    pub busy_note: gtk::Label,
    pub listen: gtk::Label,
    pub sysproxy: gtk::Label,
}

impl DashUi {
    /// Load `dashboard.ui`.
    pub fn new(builder: Builder) -> Self {
        let l = |id: &str| builder.object::<gtk::Label>(id).expect(id);
        Self {
            root: builder
                .object::<gtk::ScrolledWindow>("dashboard_root")
                .expect("dashboard_root"),
            config_name: l("dash_config_name"),
            state_label: l("dash_state"),
            dot: builder
                .object::<gtk::Image>("status_dot")
                .expect("status_dot"),
            spinner: builder
                .object::<gtk::Spinner>("dash_spinner")
                .expect("dash_spinner"),
            btn_toggle: builder
                .object::<gtk::Button>("btn_toggle")
                .expect("btn_toggle"),
            busy_note: l("dash_busy_note"),
            listen: l("lbl_listen"),
            sysproxy: l("lbl_sysproxy"),
            builder,
        }
    }
}

/// Result of the background enable operation.
enum EnableUi {
    /// System proxy written; keep the snapshot for restore.
    Enabled(Snapshot),
    /// Desktop unsupported — proxy runs, user must configure manually.
    Unsupported(String, u16),
}

/// Refresh status-dependent labels (state text, dot, toggle button, the
/// two port lines, status bar, busy spinner). Main thread only.
pub fn update_status(ui: &Rc<Ui>, state: &AppState) {
    let s = state.strings();
    let running: Option<ProxyState> = state.proxy.status();
    let (text, btn, port) = match &running {
        Some(ps) => (s.pc_connected, s.pc_disable, Some(ps.port)),
        None => (s.pc_disconnected, s.pc_enable, state.display_port()),
    };

    ui.dash.state_label.set_label(text);
    ui.dash.dot.set_visible(running.is_some());
    ui.dash.btn_toggle.set_label(btn);
    ui.dash
        .config_name
        .set_label(state.selected.borrow().as_deref().unwrap_or("—"));

    let listen_line = |p: u16| fill(s.dash_listen_line, &[("port", &p.to_string())]);
    match port {
        Some(p) => {
            ui.dash.listen.set_label(&listen_line(p));
            ui.dash
                .sysproxy
                .set_label(&fill(s.dash_sysproxy_line, &[("port", &p.to_string())]));
            ui.status_port.set_label(&listen_line(p));
        }
        None => {
            ui.dash.listen.set_label("—");
            ui.dash.sysproxy.set_label("—");
            ui.status_port.set_label("");
        }
    }
    ui.status_state.set_label(text);

    let busy = state.busy.get();
    ui.dash.btn_toggle.set_sensitive(!busy);
    if busy {
        ui.dash.spinner.start();
        ui.dash.spinner.set_visible(true);
    } else {
        ui.dash.spinner.stop();
        ui.dash.spinner.set_visible(false);
    }
}

/// Start the enable flow: proxy first, then system proxy (background).
pub fn start_enable(ui: &Rc<Ui>, state: &Rc<AppState>) {
    let s = state.strings();
    if state.busy.replace(true) {
        return;
    }

    let Some(name) = state.selected.borrow().clone() else {
        state.busy.set(false);
        toast(&ui.toast_overlay, s.pc_select_first);
        return;
    };
    let cfg: ShadowsocksConfig = match state.store.load(&name) {
        Ok(c) => c,
        Err(e) => {
            state.busy.set(false);
            toast(&ui.toast_overlay, &error_text(&e, s));
            return;
        }
    };

    ui.dash.busy_note.set_label(s.pc_connecting);
    ui.dash.busy_note.set_visible(true);
    update_status(ui, state);

    let proxy = state.proxy.clone();
    let sys = state.sys.clone();
    let port = cfg.client_settings.listen_port;
    let (tx, rx) = async_channel::unbounded::<Result<EnableUi, AppError>>();

    // Worker thread: everything blocking lives here (GOAL §5 thread model).
    std::thread::spawn(move || {
        let outcome = (|| -> Result<EnableUi, AppError> {
            proxy.enable(&name, &cfg)?;
            match sys.enable(port)? {
                EnableOutcome::Applied(snap) => {
                    snap.save()?; // crash self-heal (GOAL 9-7)
                    Ok(EnableUi::Enabled(snap))
                }
                EnableOutcome::Unsupported { desktop } => Ok(EnableUi::Unsupported(desktop, port)),
            }
        })();
        if outcome.is_err() {
            // Roll back the half-started state: never leave the proxy
            // running when the system-proxy step failed (GOAL §6.6).
            let _ = proxy.disable();
        }
        let _ = tx.send_blocking(outcome);
    });

    let ui = ui.clone();
    let state = state.clone();
    glib::spawn_future_local(async move {
        let s = state.strings();
        state.busy.set(false);
        ui.dash.busy_note.set_visible(false);
        match rx.recv().await {
            Ok(Ok(EnableUi::Enabled(snap))) => {
                *state.snapshot.borrow_mut() = Some(snap);
                toast(&ui.toast_overlay, s.pc_success_enabled);
                crate::notify::desktop_notify(s.app_title, s.pc_success_enabled);
            }
            Ok(Ok(EnableUi::Unsupported(desktop, port))) => {
                toast(
                    &ui.toast_overlay,
                    &fill(
                        s.pc_unsupported_desktop,
                        &[("desktop", &desktop), ("port", &port.to_string())],
                    ),
                );
            }
            Ok(Err(e)) => {
                let msg = format!("{}: {}", s.pc_error_enable, error_text(&e, s));
                toast(&ui.toast_overlay, &msg);
            }
            Err(_) => toast(&ui.toast_overlay, s.pc_error_enable),
        }
        update_status(&ui, &state);
        super::list::rebuild(&ui, &state);
    });
}

/// Start the disable flow: proxy first, then restore the system proxy.
pub fn start_disable(ui: &Rc<Ui>, state: &Rc<AppState>) {
    let s = state.strings();
    if state.busy.replace(true) {
        return;
    }
    ui.dash.busy_note.set_label(s.pc_disconnecting);
    ui.dash.busy_note.set_visible(true);
    update_status(ui, state);

    let snapshot = state.snapshot.borrow_mut().take();
    let proxy = state.proxy.clone();
    let sys = state.sys.clone();
    let (tx, rx) = async_channel::unbounded::<Result<(), AppError>>();

    std::thread::spawn(move || {
        // Invariant 7 order: stop the proxy, then restore the system proxy.
        let result = proxy.disable().and_then(|()| {
            if let Some(snap) = &snapshot {
                sys.disable(snap)?;
            }
            Snapshot::remove()
        });
        let _ = tx.send_blocking(result);
    });

    let ui = ui.clone();
    let state = state.clone();
    glib::spawn_future_local(async move {
        let s = state.strings();
        state.busy.set(false);
        ui.dash.busy_note.set_visible(false);
        match rx.recv().await {
            Ok(Ok(())) => {
                toast(&ui.toast_overlay, s.pc_success_disabled);
                crate::notify::desktop_notify(s.app_title, s.pc_success_disabled);
            }
            Ok(Err(e)) => {
                let msg = format!("{}: {}", s.pc_error_disable, error_text(&e, s));
                toast(&ui.toast_overlay, &msg);
            }
            Err(_) => toast(&ui.toast_overlay, s.pc_error_disable),
        }
        update_status(&ui, &state);
        super::list::rebuild(&ui, &state);
    });
}
