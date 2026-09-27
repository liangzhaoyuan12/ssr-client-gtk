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
use crate::i18n::{Strings, error_text, fill};
use crate::routing::RouteMode;
use crate::routing::dns::DnsConfig;
use crate::sysproxy::{EnableOutcome, SysProxyPref, snapshot::Snapshot};
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
    /// Routing mode chooser (index order = [`RouteMode::ALL`]).
    pub route_mode: gtk::DropDown,
    /// DNS chooser: system / custom.
    pub dns_mode: gtk::DropDown,
    /// Backing models — kept so a language switch can *splice* new items
    /// in place (replacing the model would reset the selection).
    pub route_model: gtk::StringList,
    pub dns_model: gtk::StringList,
    /// Custom DNS server list.
    pub entry_dns: gtk::Entry,
    /// Row with the chosen ACL path (visible in ACL mode only).
    pub acl_row: gtk::Box,
    pub acl_path: gtk::Label,
    pub btn_acl_pick: gtk::Button,
    /// Desktop settings vs. environment variables.
    pub sysproxy_mode: gtk::DropDown,
    pub sysproxy_model: gtk::StringList,
    /// Caption under the chooser (where the setting is written).
    pub sysproxy_hint: gtk::Label,
    /// Caption under the DNS row (scope + preset DoT/DoH details).
    pub dns_note: gtk::Label,
}

/// Dropdown item lists for the routing card, in current language.
fn route_mode_items(s: &Strings) -> Vec<&'static str> {
    vec![
        s.route_global,
        s.route_bypass_lan,
        s.route_bypass_cn,
        s.route_bypass_lan_cn,
        s.route_acl,
    ]
}

fn dns_mode_items(s: &Strings) -> Vec<&'static str> {
    vec![s.dns_system, s.dns_custom, s.dns_ali, s.dns_tencent]
}

/// Dropdown positions in the DNS combo (`dns_mode_items` order).
const DNS_IDX_SYSTEM: u32 = 0;
const DNS_IDX_CUSTOM: u32 = 1;
const DNS_IDX_ALI: u32 = 2;
const DNS_IDX_TENCENT: u32 = 3;

/// Index order matches [`SysProxyPref`]: desktop first, env vars second.
fn sysproxy_items(s: &Strings) -> Vec<&'static str> {
    vec![s.sysproxy_desktop, s.sysproxy_env]
}

impl DashUi {
    /// Load `dashboard.ui` and build the two routing dropdowns (they are
    /// created in code because their items are translated).
    pub fn new(builder: Builder, s: &'static Strings) -> Self {
        let l = |id: &str| builder.object::<gtk::Label>(id).expect(id);
        let holder = |id: &str| builder.object::<gtk::Box>(id).expect(id);

        let route_model = gtk::StringList::new(&route_mode_items(s));
        let route_mode = gtk::DropDown::new(Some(route_model.clone()), None::<&gtk::Expression>);
        holder("route_mode_holder").append(&route_mode);
        let dns_model = gtk::StringList::new(&dns_mode_items(s));
        let dns_mode = gtk::DropDown::new(Some(dns_model.clone()), None::<&gtk::Expression>);
        holder("dns_mode_holder").append(&dns_mode);
        let sysproxy_model = gtk::StringList::new(&sysproxy_items(s));
        let sysproxy_mode =
            gtk::DropDown::new(Some(sysproxy_model.clone()), None::<&gtk::Expression>);
        holder("sysproxy_mode_holder").append(&sysproxy_mode);

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
            route_mode,
            dns_mode,
            route_model,
            dns_model,
            sysproxy_mode,
            sysproxy_model,
            sysproxy_hint: l("lbl_sysproxy_hint"),
            dns_note: l("lbl_dns_note"),
            entry_dns: builder
                .object::<gtk::Entry>("entry_dns_servers")
                .expect("entry_dns_servers"),
            acl_row: holder("acl_row"),
            acl_path: l("lbl_acl_path"),
            btn_acl_pick: builder
                .object::<gtk::Button>("btn_acl_pick")
                .expect("btn_acl_pick"),
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
            ui.status_port.set_label(&listen_line(p));
        }
        None => {
            ui.dash.listen.set_label("—");
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

    // Routing/DNS must validate before any progress UI or binding.
    if let Err(e) = state.routing.borrow().validate() {
        state.busy.set(false);
        toast(&ui.toast_overlay, &error_text(&e, s));
        return;
    }

    ui.dash.busy_note.set_label(s.pc_connecting);
    ui.dash.busy_note.set_visible(true);
    update_status(ui, state);

    let proxy = state.proxy.clone();
    let sys = state.sys.clone();
    let routing = state.routing.borrow().clone();
    let syspref = state.sysproxy_pref.get();
    let port = cfg.client_settings.listen_port;
    let (tx, rx) = async_channel::unbounded::<Result<EnableUi, AppError>>();

    // Worker thread: everything blocking lives here (GOAL §5 thread model).
    std::thread::spawn(move || {
        let outcome = (|| -> Result<EnableUi, AppError> {
            proxy.enable(&name, &cfg, &routing)?;
            match sys.enable(port, syspref)? {
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

// ---------------------------------------------------------------------------
// Routing / DNS card (GOAL §3.8 路由与 DNS)
// ---------------------------------------------------------------------------

/// Push `state.routing` into the routing card widgets.
pub fn refresh_routing(ui: &Ui, state: &AppState) {
    let routing = state.routing.borrow().clone();
    let s = state.strings();

    let mode_idx = RouteMode::ALL
        .iter()
        .position(|m| *m == routing.mode)
        .unwrap_or(0) as u32;
    if ui.dash.route_mode.selected() != mode_idx {
        ui.dash.route_mode.set_selected(mode_idx);
    }

    let acl_on = routing.mode == RouteMode::Acl;
    ui.dash.acl_row.set_visible(acl_on);
    ui.dash
        .acl_path
        .set_label(if routing.acl_path.trim().is_empty() {
            s.route_acl_none
        } else {
            routing.acl_path.as_str()
        });

    let dns = routing.dns.clone();
    let idx = dns_index_for(&dns);
    if ui.dash.dns_mode.selected() != idx {
        ui.dash.dns_mode.set_selected(idx);
    }
    ui.dash
        .entry_dns
        .set_visible(!matches!(dns, DnsConfig::System));
    let text = dns.to_text();
    if ui.dash.entry_dns.text().as_str() != text {
        ui.dash.entry_dns.set_text(&text);
    }
    let incomplete = matches!(&dns, DnsConfig::Custom(v) if v.is_empty());
    ui.dash
        .entry_dns
        .set_css_classes(if incomplete { &["error"] } else { &[] });
    ui.dash.dns_note.set_label(&dns_note_text(s, &dns));
}

/// Dropdown position for a configuration (`dns_mode_items` order).
fn dns_index_for(dns: &DnsConfig) -> u32 {
    match dns {
        DnsConfig::System => DNS_IDX_SYSTEM,
        DnsConfig::Custom(_) => DNS_IDX_CUSTOM,
        DnsConfig::Ali => DNS_IDX_ALI,
        DnsConfig::Tencent => DNS_IDX_TENCENT,
    }
}

/// Selection → configuration. The dropdown owns this direction; "自定义"
/// seeds the box with the servers already configured so the user edits
/// what they had instead of starting from an empty line.
fn dns_from_selection(sel: u32, current: &DnsConfig) -> DnsConfig {
    match sel {
        DNS_IDX_SYSTEM => DnsConfig::System,
        DNS_IDX_ALI => DnsConfig::Ali,
        DNS_IDX_TENCENT => DnsConfig::Tencent,
        _ => match current {
            DnsConfig::System => DnsConfig::Custom(Vec::new()),
            other => DnsConfig::Custom(other.servers()),
        },
    }
}

/// Scope note, plus the selected preset's DoT/DoH names (informational:
/// the client itself always queries plain UDP/TCP 53).
fn dns_note_text(s: &Strings, dns: &DnsConfig) -> String {
    match dns {
        DnsConfig::Ali => format!("{} {}", s.dns_note, s.dns_note_ali),
        DnsConfig::Tencent => format!("{} {}", s.dns_note, s.dns_note_tencent),
        _ => s.dns_note.to_string(),
    }
}

/// Push a DNS configuration into state and the card. `rewrite_text` is true
/// only for the dropdown (it owns the box contents); the entry handler passes
/// `false` so a caret mid-edit is never moved under the user.
fn apply_dns(ui: &Rc<Ui>, state: &Rc<AppState>, next: DnsConfig, rewrite_text: bool) {
    let s = state.strings();
    let changed = {
        let mut routing = state.routing.borrow_mut();
        if routing.dns == next {
            false
        } else {
            routing.dns = next.clone();
            true
        }
    };
    ui.dash
        .entry_dns
        .set_visible(!matches!(next, DnsConfig::System));
    if rewrite_text {
        let text = next.to_text();
        if ui.dash.entry_dns.text().as_str() != text {
            ui.dash.entry_dns.set_text(&text);
        }
    }
    let incomplete = matches!(&next, DnsConfig::Custom(v) if v.is_empty());
    ui.dash
        .entry_dns
        .set_css_classes(if incomplete { &["error"] } else { &[] });
    let idx = dns_index_for(&next);
    if ui.dash.dns_mode.selected() != idx {
        ui.dash.dns_mode.set_selected(idx);
    }
    ui.dash.dns_note.set_label(&dns_note_text(s, &next));
    if changed {
        state.save_routing();
        hint_restart(ui, state);
    }
}

/// Sync the system-proxy chooser and its caption with the stored choice.
pub fn refresh_sysproxy_pref(ui: &Ui, state: &AppState) {
    let pref = state.sysproxy_pref.get();
    let s = state.strings();
    let idx = u32::from(pref == SysProxyPref::EnvVar);
    if ui.dash.sysproxy_mode.selected() != idx {
        ui.dash.sysproxy_mode.set_selected(idx);
    }
    let hint = match pref {
        SysProxyPref::Desktop => s.sysproxy_desktop_hint.to_string(),
        SysProxyPref::EnvVar => match crate::sysproxy::env::detect() {
            Ok(shell) => fill(
                s.sysproxy_env_hint,
                &[("rc", &shell.rc_path().display().to_string())],
            ),
            Err(_) => s.sysproxy_env_unknown.to_string(),
        },
    };
    ui.dash.sysproxy_hint.set_label(&hint);
}

/// Re-translate the routing card after a language switch. The dropdown
/// *models* are spliced in place so the current selection survives.
pub fn rebuild_routing_text(ui: &Ui, state: &AppState) {
    let s = state.strings();
    let modes = route_mode_items(s);
    ui.dash
        .route_model
        .splice(0, ui.dash.route_model.n_items(), &modes);
    let dns = dns_mode_items(s);
    ui.dash
        .dns_model
        .splice(0, ui.dash.dns_model.n_items(), &dns);
    ui.dash
        .entry_dns
        .set_placeholder_text(Some(s.dns_servers_placeholder));
    let modes = sysproxy_items(s);
    ui.dash
        .sysproxy_model
        .splice(0, ui.dash.sysproxy_model.n_items(), &modes);
    refresh_routing(ui, state);
    refresh_sysproxy_pref(ui, state);
}

/// Tell the user the card only applies from the next enable.
fn hint_restart(ui: &Rc<Ui>, state: &Rc<AppState>) {
    if state.proxy.is_running() {
        toast(&ui.toast_overlay, state.strings().route_restart_hint);
    }
}

fn mode_index(mode: RouteMode) -> u32 {
    RouteMode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as u32
}

/// Persist the chosen mode and refresh the card.
fn apply_mode(ui: &Rc<Ui>, state: &Rc<AppState>, mode: RouteMode) {
    {
        let mut routing = state.routing.borrow_mut();
        if routing.mode == mode {
            return;
        }
        routing.mode = mode;
    }
    state.save_routing();
    refresh_routing(ui, state);
    hint_restart(ui, state);
}

/// Ask for an ACL file; cancel puts the dropdown back where it was.
fn pick_acl_file(ui: &Rc<Ui>, state: &Rc<AppState>) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title(state.strings().route_acl_pick);
    dialog.set_modal(true);
    let ui2 = ui.clone();
    let state2 = state.clone();
    let parent = ui2.window.clone();
    dialog.open(Some(&parent), None::<&gtk::gio::Cancellable>, move |res| {
        let s = state2.strings();
        let path = match res {
            Ok(file) => file.path(),
            Err(_) => None, // cancelled
        };
        let Some(path) = path else {
            // Cancelled: restore the previous selection.
            ui2.dash
                .route_mode
                .set_selected(mode_index(state2.routing.borrow().mode));
            return;
        };
        match crate::routing::acl::Acl::load(&path) {
            Ok(_) => {
                {
                    let mut routing = state2.routing.borrow_mut();
                    routing.acl_path = path.to_string_lossy().to_string();
                    routing.mode = RouteMode::Acl;
                }
                state2.save_routing();
                refresh_routing(&ui2, &state2);
                hint_restart(&ui2, &state2);
            }
            Err(e) => {
                toast(&ui2.toast_overlay, &error_text(&e, s));
                ui2.dash
                    .route_mode
                    .set_selected(mode_index(state2.routing.borrow().mode));
            }
        }
    });
}

/// Rebuild the DNS setting from the dropdown + entry (never saves junk:
/// an unparseable list only marks the entry as invalid).
/// The entry owns text edits: parse what is typed, keep the last valid value
/// on junk (marking the box red), and sync everything *except* the text —
/// `rewrite_text = false` so the caret never moves under the user.
fn sync_dns(ui: &Rc<Ui>, state: &Rc<AppState>) {
    // "系统 DNS" hides the box; apply_dns clears it, which lands here too.
    if ui.dash.dns_mode.selected() == DNS_IDX_SYSTEM {
        apply_dns(ui, state, DnsConfig::System, false);
        return;
    }
    let text = ui.dash.entry_dns.text().to_string();
    if text.trim().is_empty() {
        // Custom/preset selected but the box is empty → keep the selection,
        // mark the entry, and refuse to enable until it is filled.
        apply_dns(ui, state, DnsConfig::Custom(Vec::new()), false);
        return;
    }
    match DnsConfig::parse(&text) {
        // A box that still spells out a preset stays on that preset; any
        // other valid list is a plain Custom configuration.
        Ok(parsed) => apply_dns(ui, state, DnsConfig::from_servers(parsed.servers()), false),
        Err(_) => {
            ui.dash.entry_dns.add_css_class("error"); // keep the last valid value
        }
    }
}

/// Connect the routing card (call once, after the first refresh).
pub fn wire_routing(ui: &Rc<Ui>, state: &Rc<AppState>) {
    refresh_routing(ui, state);
    refresh_sysproxy_pref(ui, state);

    {
        let ui = ui.clone();
        let state = state.clone();
        let dd = ui.dash.route_mode.clone();
        dd.connect_notify_local(Some("selected"), move |dd, _| {
            let Some(&mode) = RouteMode::ALL.get(dd.selected() as usize) else {
                return;
            };
            if state.routing.borrow().mode == mode {
                return;
            }
            if mode == RouteMode::Acl && state.routing.borrow().acl_path.trim().is_empty() {
                pick_acl_file(&ui, &state);
                return;
            }
            apply_mode(&ui, &state, mode);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let dd = ui.dash.dns_mode.clone();
        dd.connect_notify_local(Some("selected"), move |dd, _| {
            // The dropdown owns this direction: system / custom / presets.
            let current = state.routing.borrow().dns.clone();
            let next = dns_from_selection(dd.selected(), &current);
            apply_dns(&ui, &state, next, true);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let entry = ui.dash.entry_dns.clone();
        entry.connect_changed(move |_| sync_dns(&ui, &state));
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let dd = ui.dash.sysproxy_mode.clone();
        dd.connect_notify_local(Some("selected"), move |dd, _| {
            let pref = if dd.selected() == 1 {
                SysProxyPref::EnvVar
            } else {
                SysProxyPref::Desktop
            };
            if state.sysproxy_pref.get() == pref {
                return;
            }
            if pref == SysProxyPref::EnvVar
                && let Err(msg) = crate::sysproxy::env::detect()
            {
                // No shell we know how to write: say why and stay on the
                // desktop strategy instead of failing at enable time.
                toast(&ui.toast_overlay, &msg);
                dd.set_selected(0);
                return;
            }
            state.sysproxy_pref.set(pref);
            state.save_sysproxy_pref();
            refresh_sysproxy_pref(&ui, &state);
            hint_restart(&ui, &state);
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn = ui.dash.btn_acl_pick.clone();
        btn.connect_clicked(move |btn| {
            // Only reachable while ACL mode is active (row is hidden
            // otherwise); re-picking keeps the mode.
            let _ = btn;
            pick_acl_file(&ui, &state);
        });
    }
}
