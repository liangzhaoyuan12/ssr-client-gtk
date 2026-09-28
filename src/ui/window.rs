//! Main window: assembles the four `.ui` views, owns every signal
//! connection and the lifecycle handling (close → stop proxy → restore
//! system proxy, GOAL invariant 7).
//!
//! No business logic here beyond dispatching into `state` / `core` /
//! `sysproxy`; all blocking work runs on worker threads (§5 thread model).

use std::rc::Rc;

use gtk::Builder;
use gtk::prelude::*;
use libadwaita::AboutDialog;
use libadwaita::prelude::*;
use libadwaita::{
    Application, ApplicationWindow, HeaderBar, NavigationPage, StatusPage, ToastOverlay,
    ToolbarView, WindowTitle,
};

use crate::app::{AppState, View};
use crate::core::proxy::ProxyEvent;
use crate::error::AppError;
use crate::i18n::{Lang, Strings, error_text};
use crate::sysproxy::snapshot::Snapshot;
use crate::ui::dashboard::{self, DashUi};
use crate::ui::form::{self, FormUi};
use crate::ui::list::{self, ListUi};
use crate::ui::toast::toast;

pub use dashboard::update_status;

const WINDOW_UI: &str = include_str!("../../ui/window.ui");
const LIST_UI: &str = include_str!("../../ui/config_list.ui");
const FORM_UI: &str = include_str!("../../ui/config_form.ui");
const DASH_UI: &str = include_str!("../../ui/dashboard.ui");

/// Static label binding: (widget id, string getter).
type LabelBinding = (&'static str, fn(&Strings) -> &'static str);

const LIST_LABELS: &[LabelBinding] = &[
    ("btn_new", |s| s.cf_add_title),
    ("list_hint", |s| s.cfg_list_empty_hint),
];

const FORM_LABELS: &[LabelBinding] = &[
    ("lbl_name", |s| s.cf_profile_name),
    ("name_hint", |s| s.cf_profile_name_hint),
    ("lbl_server", |s| s.cf_server_address),
    ("lbl_server_port", |s| s.cf_server_port),
    ("lbl_password", |s| s.cf_password),
    ("lbl_method", |s| s.cf_method),
    ("lbl_protocol", |s| s.cf_protocol),
    ("lbl_obfs", |s| s.cf_obfs),
    ("lbl_protocol_param", |s| s.cf_protocol_param),
    ("lbl_obfs_param", |s| s.cf_obfs_param),
    ("lbl_udp", |s| s.cf_enable_udp),
    ("lbl_idle_timeout", |s| s.cf_idle_timeout),
    ("lbl_connect_timeout", |s| s.cf_connect_timeout),
    ("lbl_udp_timeout", |s| s.cf_udp_timeout),
    ("lbl_listen_port", |s| s.cf_local_port),
    ("listen_note", |s| s.cf_listen_note),
    ("btn_save", |s| s.common_save),
    ("btn_cancel", |s| s.common_cancel),
];

const FORM_PLACEHOLDERS: &[LabelBinding] = &[
    ("entry_name", |s| s.cf_profile_name_placeholder),
    ("entry_server", |s| s.cf_server_address_placeholder),
    ("entry_password", |s| s.cf_password_placeholder),
    ("entry_protocol_param", |s| s.cf_protocol_param_placeholder),
    ("entry_obfs_param", |s| s.cf_obfs_param_placeholder),
];

const DASH_LABELS: &[LabelBinding] = &[
    ("dash_proxy_title", |s| s.dash_local_proxy),
    ("dash_steps_title", |s| s.dash_how_to_use),
    ("dash_route_title", |s| s.dash_route_title),
    ("lbl_route_mode", |s| s.route_mode_label),
    ("lbl_dns", |s| s.dns_label),
    ("lbl_dns_note", |s| s.dns_note),
    ("lbl_udp_note", |s| s.route_udp_note),
    ("lbl_restart_hint", |s| s.route_restart_hint),
    ("lbl_sysproxy_mode", |s| s.sysproxy_mode_label),
    ("btn_acl_pick", |s| s.route_acl_pick),
];

/// Everything the UI layer touches.
pub struct Ui {
    pub window: ApplicationWindow,
    pub toast_overlay: ToastOverlay,
    pub win_title: WindowTitle,
    /// Bottom status bar: connection text.
    pub status_state: gtk::Label,
    /// Bottom status bar: the single listening port line.
    pub status_port: gtk::Label,
    pub sidebar_page: NavigationPage,
    pub content_page: NavigationPage,
    pub stack: gtk::Stack,
    pub empty_page: StatusPage,
    pub lang_dropdown: gtk::DropDown,
    /// Top-right 关于 button (opens [`open_about`]).
    pub about_btn: gtk::Button,
    pub list: ListUi,
    pub form: FormUi,
    pub dash: DashUi,
}

impl Ui {
    /// Load the four `.ui` files, assemble the shell, apply the current
    /// language.
    pub fn new(app: &Application, state: &AppState) -> Self {
        // Icon search path for source-tree runs. Must happen here, not in
        // `main()`: the GDK display is only opened when the application starts,
        // so `IconTheme::for_display` has nothing to attach to before that.
        // The path lists directories that CONTAIN icon themes (same shape as
        // `$XDG_DATA_DIRS/icons`) and a theme dir only counts with an
        // index.theme — `data/icons/hicolor/index.theme` is that file. Without
        // both, `cargo run` cannot resolve `APP_ID` and the 关于 dialog falls
        // back to the theme's `image-missing` art instead of icon.png.
        if let Some(display) = gtk::gdk::Display::default() {
            let theme = gtk::IconTheme::for_display(&display);
            theme.add_search_path("data/icons");
            #[cfg(debug_assertions)]
            theme.add_search_path(format!("{}/data/icons", env!("CARGO_MANIFEST_DIR")));
        }

        let builder = Builder::from_string(WINDOW_UI);
        let window = builder
            .object::<ApplicationWindow>("window")
            .expect("window");
        window.set_application(Some(app));

        let toast_overlay = builder
            .object::<ToastOverlay>("toast_overlay")
            .expect("toast_overlay");
        let toolbar = builder.object::<ToolbarView>("toolbar").expect("toolbar");
        let sidebar_page = builder
            .object::<NavigationPage>("sidebar_page")
            .expect("sidebar_page");
        let content_page = builder
            .object::<NavigationPage>("content_page")
            .expect("content_page");
        let stack = builder.object::<gtk::Stack>("stack").expect("stack");
        let empty_page = builder
            .object::<StatusPage>("empty_page")
            .expect("empty_page");
        let sidebar_holder = builder
            .object::<gtk::Box>("sidebar_holder")
            .expect("sidebar_holder");

        // Sub-views from their own builders.
        let list = ListUi::new(Builder::from_string(LIST_UI));
        let form = FormUi::new(Builder::from_string(FORM_UI));
        let dash = DashUi::new(
            Builder::from_string(DASH_UI),
            state.strings(),
            state.sys.desktop(),
        );
        sidebar_holder.append(&list.root);
        stack.add_titled(&form.root, Some("form"), "form");
        stack.add_titled(&dash.root, Some("dashboard"), "dashboard");

        // Header: title + language dropdown.
        let s = state.strings();
        let win_title = WindowTitle::new(s.app_title, s.footer_text);
        // Same icon name as the .desktop entry and the 关于 dialog.
        window.set_icon_name(Some(crate::APP_ID));
        let header = HeaderBar::new();
        header.set_title_widget(Some(&win_title));
        // 关于 — an icon button (the glyph, not the word), packed first so it
        // ends up rightmost of the two: language dropdown → 关于 → window
        // controls. `pack_end` places earlier children closer to the end.
        let about_btn = gtk::Button::from_icon_name("help-about-symbolic");
        about_btn.set_tooltip_text(Some(s.about_label));
        about_btn.add_css_class("flat");
        header.pack_end(&about_btn);
        let lang_dropdown = gtk::DropDown::from_strings(&[s.lang_zh, s.lang_en]);
        lang_dropdown.set_tooltip_text(Some(s.lang_label));
        lang_dropdown.set_selected(match state.lang.get() {
            Lang::ZhCn => 0,
            Lang::EnUs => 1,
        });
        header.pack_end(&lang_dropdown);
        toolbar.add_top_bar(&header);

        // Bottom status bar: state dot text + the one port line.
        let status_state = gtk::Label::new(Some(s.pc_disconnected));
        status_state.set_xalign(0.0);
        status_state.set_hexpand(true);
        status_state.add_css_class("dim-label");
        let status_port = gtk::Label::new(None);
        status_port.set_xalign(1.0);
        status_port.add_css_class("caption");
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status_bar.set_margin_start(12);
        status_bar.set_margin_end(12);
        status_bar.set_margin_top(4);
        status_bar.set_margin_bottom(6);
        status_bar.append(&status_state);
        status_bar.append(&status_port);
        toolbar.add_bottom_bar(&status_bar);

        Self {
            window,
            toast_overlay,
            win_title,
            status_state,
            status_port,
            sidebar_page,
            content_page,
            stack,
            empty_page,
            lang_dropdown,
            about_btn,
            list,
            form,
            dash,
        }
    }

    /// Empty page wording depends on whether the list has configs:
    /// none → "no profile yet" + hint; configs exist but none is
    /// selected → "select one first".
    pub fn update_empty_state(&self, state: &AppState) {
        let s = state.strings();
        if state.names.borrow().is_empty() {
            self.empty_page.set_title(s.cfg_list_empty);
            self.empty_page.set_description(Some(s.cfg_list_empty_hint));
        } else {
            self.empty_page.set_title(s.pc_select_first);
            self.empty_page.set_description(None);
        }
    }

    /// Re-apply every visible string (language switch, first paint).
    pub fn apply_strings(self: &Rc<Self>, state: &AppState) {
        let s = state.strings();
        self.win_title.set_title(s.app_title);
        self.win_title.set_subtitle(s.footer_text);
        self.sidebar_page.set_title(s.cfg_list_title);
        self.update_empty_state(state);
        self.lang_dropdown.set_tooltip_text(Some(s.lang_label));
        // Icon button: only the tooltip (and the accessible name) are text.
        self.about_btn.set_tooltip_text(Some(s.about_label));
        self.about_btn
            .update_property(&[gtk::accessible::Property::Label(s.about_label)]);

        let set_labels = |builder: &Builder, table: &[LabelBinding]| {
            for (id, get) in table {
                if let Some(l) = builder.object::<gtk::Label>(*id) {
                    l.set_label(get(s));
                } else if let Some(b) = builder.object::<gtk::Button>(*id) {
                    b.set_label(get(s));
                }
            }
        };
        set_labels(&self.list.builder, LIST_LABELS);
        set_labels(&self.form.builder, FORM_LABELS);
        for (id, get) in FORM_PLACEHOLDERS {
            if let Some(e) = self.form.builder.object::<gtk::Entry>(*id) {
                e.set_placeholder_text(Some(get(s)));
            } else if let Some(p) = self.form.builder.object::<gtk::PasswordEntry>(*id) {
                p.set_placeholder_text(Some(get(s)));
            }
        }
        set_labels(&self.dash.builder, DASH_LABELS);
        if let Some(steps) = self.dash.builder.object::<gtk::Label>("dash_steps") {
            steps.set_label(&s.steps.join("\n\n"));
        }
        crate::ui::dashboard::rebuild_routing_text(self, state);

        // Mode-dependent titles.
        let (form_title, page_title) = match state.view.get() {
            View::Form => {
                if state.form_editing.borrow().is_some() {
                    (s.cf_edit_title, s.cf_edit_title)
                } else {
                    (s.cf_add_title, s.cf_add_title)
                }
            }
            View::Dashboard => (s.pc_title, s.pc_title),
            View::Empty => ("", s.cfg_list_title),
        };
        self.form.title.set_label(form_title);
        self.content_page.set_title(page_title);
        if let Some(heading) = self.form.builder.object::<gtk::Label>("form_title") {
            heading.set_label(form_title);
        }

        update_status(self, state);
    }
}

/// Switch to the empty status page.
pub fn show_empty(ui: &Rc<Ui>, state: &AppState) {
    state.view.set(View::Empty);
    ui.stack.set_visible_child_name("empty");
    ui.content_page.set_title(state.strings().cfg_list_title);
    update_status(ui, state);
}

/// Switch to the dashboard for the current selection.
pub fn show_dashboard(ui: &Rc<Ui>, state: &AppState) {
    state.view.set(View::Dashboard);
    ui.stack.set_visible_child_name("dashboard");
    ui.content_page.set_title(state.strings().pc_title);
    update_status(ui, state);
}

/// Select a profile the way a sidebar click does: remember it (persisted, so
/// the next launch reopens the same row — GOAL 7.12), highlight it in the
/// list and open its dashboard. Shared by the click handler, the start-up
/// restore and the QA `SSR_GTK_DEV_SELECT` hook.
pub fn select_profile(ui: &Rc<Ui>, state: &Rc<AppState>, name: &str) {
    *state.selected.borrow_mut() = Some(name.to_string());
    state.save_selected();
    highlight_row(ui, name);
    show_dashboard(ui, state);
}

/// Highlight `name`'s sidebar row without rebuilding the list: the rows may
/// already exist (start-up restore) and rebuilding from inside
/// `row_activated` would yank the row out from under the running signal.
fn highlight_row(ui: &Ui, name: &str) {
    let mut i = 0;
    while let Some(row) = ui.list.cfg_list.row_at_index(i) {
        if list::row_name(&row).as_deref() == Some(name) {
            ui.list.cfg_list.select_row(Some(&row));
            return;
        }
        i += 1;
    }
}

/// Open the form: `Some(name)` = edit mode (fields prefilled, name frozen).
pub fn open_form(ui: &Rc<Ui>, state: &Rc<AppState>, name: Option<&str>) {
    let s = state.strings();
    match name {
        Some(name) => match state.store.load(name) {
            Ok(cfg) => {
                ui.form.fill(name, &cfg);
                *state.form_editing.borrow_mut() = Some(name.to_string());
                ui.form.title.set_label(s.cf_edit_title);
                ui.content_page.set_title(s.cf_edit_title);
            }
            Err(e) => {
                toast(&ui.toast_overlay, &error_text(&e, s));
                return;
            }
        },
        None => {
            ui.form.reset();
            *state.form_editing.borrow_mut() = None;
            ui.form.title.set_label(s.cf_add_title);
            ui.content_page.set_title(s.cf_add_title);
        }
    }
    state.view.set(View::Form);
    ui.stack.set_visible_child_name("form");
}

/// Save the form (create or overwrite) and jump back to the dashboard.
fn save_form(ui: &Rc<Ui>, state: &Rc<AppState>) {
    let s = state.strings();
    ui.form.hide_error();
    let input = match ui.form.read() {
        Ok(input) => input,
        Err(e) => {
            ui.form.show_error(form::form_error_text(e, s));
            return;
        }
    };
    let editing = state.form_editing.borrow().clone();
    let result = match &editing {
        Some(_) => state.store.save(&input.name, &input.cfg),
        None => state.store.create(&input.name, &input.cfg),
    };
    match result {
        Ok(()) => {
            let msg = if editing.is_some() {
                s.cf_success_updated
            } else {
                s.cf_success_created
            };
            toast(&ui.toast_overlay, msg);
            *state.selected.borrow_mut() = Some(input.name.clone());
            state.save_selected();
            state.refresh_names();
            list::rebuild(ui, state);
            show_dashboard(ui, state);
        }
        Err(e) => {
            ui.form.show_error(&error_text(&e, s));
        }
    }
}

/// Connect every signal (GOAL 4.1) and start the event/cleanup helpers.
pub fn wire(ui: Rc<Ui>, state: Rc<AppState>) {
    // --- header: 关于 ---------------------------------------------------
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn = ui.about_btn.clone();
        btn.connect_clicked(move |_| open_about(&ui, &state));
    }

    // --- sidebar -------------------------------------------------------
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn_new = ui.list.btn_new.clone();
        btn_new.connect_clicked(move |_| open_form(&ui, &state, None));
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let cfg_list = ui.list.cfg_list.clone();
        cfg_list.connect_row_activated(move |_, row| {
            if let Some(name) = list::row_name(row) {
                // Corrupt file: toast the error inline instead of selecting
                // (GOAL §4.1 — broken rows never become the active profile).
                if let Err(e) = state.store.load(&name) {
                    let s = state.strings();
                    toast(&ui.toast_overlay, &error_text(&e, s));
                    return;
                }
                select_profile(&ui, &state, &name);
            }
        });
    }

    // --- focus → reconcile status (GOAL §4.1 状态对账: a core that died
    //     while the window was unfocused must not keep showing "connected")
    {
        let ui = ui.clone();
        let state = state.clone();
        let window = ui.window.clone();
        window.connect_notify_local(Some("is-active"), move |win, _| {
            if win.is_active() {
                update_status(&ui, &state);
            }
        });
    }

    // --- form ----------------------------------------------------------
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn_cancel = ui.form.btn_cancel.clone();
        btn_cancel.connect_clicked(move |_| {
            if state.selected.borrow().is_some() {
                show_dashboard(&ui, &state);
            } else {
                show_empty(&ui, &state);
            }
        });
    }
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn_save = ui.form.btn_save.clone();
        btn_save.connect_clicked(move |_| save_form(&ui, &state));
    }

    // --- dashboard toggle ----------------------------------------------
    {
        let ui = ui.clone();
        let state = state.clone();
        let btn_toggle = ui.dash.btn_toggle.clone();
        btn_toggle.connect_clicked(move |_| toggle_proxy(&ui, &state));
    }

    // --- language switch ------------------------------------------------
    {
        let ui = ui.clone();
        let state = state.clone();
        let lang_dropdown = ui.lang_dropdown.clone();
        lang_dropdown.connect_notify_local(Some("selected"), move |dd, _| {
            let lang = if dd.selected() == 0 {
                Lang::ZhCn
            } else {
                Lang::EnUs
            };
            state.lang.set(lang);
            ui.apply_strings(&state);
            list::rebuild(&ui, &state);
        });
    }

    // --- window close: stop proxy → restore system proxy → quit ---------
    {
        let state = state.clone();
        ui.window.connect_close_request(move |win| {
            if state.cleaned_up.get() {
                return glib::Propagation::Proceed;
            }
            let owned = state.snapshot.borrow_mut().take();
            let leftover = Snapshot::load().ok().flatten();
            let snapshot = owned.or(leftover);
            let running = state.proxy.status().is_some();

            if !running && snapshot.is_none() {
                // Not enabled → nothing to touch (invariant 7).
                state.cleaned_up.set(true);
                return glib::Propagation::Proceed;
            }

            let proxy = state.proxy.clone();
            let sys = state.sys.clone();
            let s = state.strings();
            let (tx, rx) = async_channel::unbounded::<()>();
            std::thread::spawn(move || {
                let _ = proxy.disable();
                if let Some(snap) = &snapshot {
                    let _ = sys.disable(snap);
                }
                let _ = Snapshot::remove();
                // Closing the window must not be silent: announce it the way
                // the stop button does. Sent here, before tx fires, so the
                // main loop only destroys the window once notify-send has
                // finished — otherwise the app could exit underneath it.
                crate::notify::desktop_notify(s.app_title, s.pc_close_stopped);
                let _ = tx.send_blocking(());
            });

            let state = state.clone();
            let win = win.clone();
            glib::spawn_future_local(async move {
                let _ = rx.recv().await;
                state.cleaned_up.set(true);
                win.close(); // re-enters close_request → now Proceed
            });
            glib::Propagation::Stop
        });
    }

    // --- proxy lifecycle events from the core ---------------------------
    spawn_event_loop(ui.clone(), state.clone());

    // --- routing / DNS card ---------------------------------------------
    dashboard::wire_routing(&ui, &state);

    // --- initial paint ---------------------------------------------------
    ui.apply_strings(&state);
    list::rebuild(&ui, &state);
    show_empty(&ui, &state);
}

/// Enable or disable the proxy depending on current state — shared by
/// the dashboard toggle button and the QA hooks in `main`.
/// The 关于 dialog: project address, author, version, licence and icon.
/// `AdwAboutDialog` is the libadwaita 1.5+ replacement for the deprecated
/// `AdwAboutWindow`.
pub fn open_about(ui: &Rc<Ui>, state: &AppState) {
    let s = state.strings();
    let lang = state.lang.get();
    // Build and present inside the locale flip: the dialog's stock row labels
    // (Website / Credits / Legal information) are gettext strings created here.
    with_messages_locale(lang, || {
        let dialog = AboutDialog::new();
        dialog.set_application_name(s.app_title);
        dialog.set_application_icon(crate::APP_ID);
        dialog.set_version(env!("CARGO_PKG_VERSION"));
        // The main page shows icon, name, version and this developer name;
        // project address / license / credits live one click away (Details,
        // Legal, Credits) — libadwaita's standard About layout.
        dialog.set_developer_name(crate::AUTHOR);
        dialog.set_comments(s.about_comments);
        dialog.set_website(crate::PROJECT_URL);
        dialog.set_developers(&[crate::AUTHOR]);
        // Free text, not gtk::License::Gpl30: the project is
        // GPL-3.0-**or-later** and the enum only prints "GPL-3.0".
        dialog.set_license("GPL-3.0-or-later");
        dialog.present(Some(&ui.window));
    });
}

/// GTK's own strings — the 关于 dialog's `Website` / `Credits` / `Legal
/// information` rows — are gettext-translated from `LC_MESSAGES`, which
/// ignores the app's language switch. Those labels are created while the
/// dialog is built, so point `LC_MESSAGES` at the UI language, build and
/// present, then restore: the labels keep the language they were built with
/// and no other code runs with the flipped locale.
///
/// Windows has no `LC_MESSAGES` category (GOAL §11 A3): the non-unix build
/// just runs the closure — GTK's own labels then follow the system locale
/// instead of the in-app switch (Phase 8.5 probe decides whether that is
/// worth fixing via `LANGUAGE`/`LC_ALL`).
#[cfg(unix)]
fn with_messages_locale(lang: crate::i18n::Lang, f: impl FnOnce()) {
    // zh → the Chinese catalog; English → the C locale, which gettext leaves
    // untranslated (English).
    let candidates: &[&str] = if lang == crate::i18n::Lang::ZhCn {
        &["zh_CN.UTF-8", "zh_CN.utf8"]
    } else {
        &["en_US.UTF-8", "en_US.utf8", "C.UTF-8", "C"]
    };
    unsafe {
        let prev = libc::setlocale(libc::LC_MESSAGES, std::ptr::null());
        for cand in candidates {
            let applied = std::ffi::CString::new(*cand)
                .map(|c| !libc::setlocale(libc::LC_MESSAGES, c.as_ptr()).is_null())
                .unwrap_or(false);
            if applied {
                break;
            }
        }
        f();
        if !prev.is_null() {
            libc::setlocale(libc::LC_MESSAGES, prev);
        }
    }
}

#[cfg(not(unix))]
fn with_messages_locale(_lang: crate::i18n::Lang, f: impl FnOnce()) {
    f();
}

pub fn toggle_proxy(ui: &Rc<Ui>, state: &Rc<AppState>) {
    if state.proxy.is_running() {
        dashboard::start_disable(ui, state);
    } else {
        dashboard::start_enable(ui, state);
    }
}

/// Consume [`ProxyEvent`]s pushed by the core (including unexpected
/// exits) and reconcile the UI (GOAL 2.3 / ARCHITECTURE §8-5).
pub fn spawn_event_loop(ui: Rc<Ui>, state: Rc<AppState>) {
    let rx = state.events.clone();
    glib::spawn_future_local(async move {
        while let Ok(event) = rx.recv().await {
            let s = state.strings();
            match event {
                ProxyEvent::Started { .. } => {
                    update_status(&ui, &state);
                    list::rebuild(&ui, &state);
                }
                ProxyEvent::Stopped { reason } => {
                    if let Some(reason) = reason {
                        // Core died on its own: the system proxy must not
                        // keep pointing at a dead port (GOAL 9-7).
                        let snapshot = state
                            .snapshot
                            .borrow_mut()
                            .take()
                            .or_else(|| Snapshot::load().ok().flatten());
                        if let Some(snap) = snapshot {
                            let sys = state.sys.clone();
                            std::thread::spawn(move || {
                                let _ = sys.disable(&snap);
                                let _ = Snapshot::remove();
                            });
                        }
                        let msg = error_text(&AppError::Core(reason), s);
                        toast(&ui.toast_overlay, &msg);
                    }
                    update_status(&ui, &state);
                    list::rebuild(&ui, &state);
                }
            }
        }
    });
}

/// Crash self-heal at startup: a leftover system-proxy snapshot (previous
/// run died without cleanup) is restored immediately (GOAL 9-7).
pub fn startup_self_heal(ui: Rc<Ui>, state: Rc<AppState>) {
    if state.proxy.is_running() {
        return;
    }
    let Some(snapshot) = Snapshot::load().ok().flatten() else {
        return;
    };
    let sys = state.sys.clone();
    let (tx, rx) = async_channel::unbounded::<Result<(), AppError>>();
    std::thread::spawn(move || {
        let result = sys.disable(&snapshot).and_then(|()| Snapshot::remove());
        let _ = tx.send_blocking(result);
    });
    glib::spawn_future_local(async move {
        if let Ok(Err(e)) = rx.recv().await {
            let msg = error_text(&e, state.strings());
            toast(&ui.toast_overlay, &msg);
        }
    });
}
