//! Sidebar profile list: rebuild, select, edit, delete (with an
//! `AdwAlertDialog` confirmation, replacing the old native `confirm()`).

use std::rc::Rc;

use gtk::Builder;
use gtk::prelude::*;
use libadwaita::prelude::*;

use crate::app::AppState;
use crate::i18n::{error_text, fill};
use crate::ui::toast::toast;
use crate::ui::window::Ui;

/// Handles for the sidebar widgets.
pub struct ListUi {
    /// Builder of `config_list.ui`.
    pub builder: Builder,
    pub root: gtk::Box,
    pub btn_new: gtk::Button,
    pub cfg_list: gtk::ListBox,
    pub list_hint: gtk::Label,
}

impl ListUi {
    /// Load `config_list.ui`.
    pub fn new(builder: Builder) -> Self {
        Self {
            root: builder.object::<gtk::Box>("list_root").expect("list_root"),
            btn_new: builder.object::<gtk::Button>("btn_new").expect("btn_new"),
            cfg_list: builder
                .object::<gtk::ListBox>("cfg_list")
                .expect("cfg_list"),
            list_hint: builder
                .object::<gtk::Label>("list_hint")
                .expect("list_hint"),
            builder,
        }
    }
}

/// The profile name a row stands for (set as the row's widget name).
pub fn row_name(row: &gtk::ListBoxRow) -> Option<String> {
    let name = row.widget_name();
    (!name.is_empty()).then(|| name.to_string())
}

/// Rebuild the list from disk, mark the active profile, keep the current
/// selection, and toggle the empty-state hint.
pub fn rebuild(ui: &Rc<Ui>, state: &Rc<AppState>) {
    state.refresh_names();
    let scanned = state.store.scan();
    let active = state.active_config().map(|s| s.cfg_name);
    let selected = state.selected.borrow().clone();
    let s = state.strings();

    while let Some(row) = ui.list.cfg_list.row_at_index(0) {
        ui.list.cfg_list.remove(&row);
    }

    ui.list.list_hint.set_visible(scanned.is_empty());

    let mut select_idx: Option<u32> = None;
    for (i, entry) in scanned.iter().enumerate() {
        let name = &entry.name;
        let broken = entry.error.as_ref();
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(name);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&label);

        if let Some(err) = broken {
            // Corrupt file: show the error inline on the row (GOAL §4.1);
            // the row stays deletable but cannot be selected.
            label.add_css_class("destructive-action");
            let badge = gtk::Label::new(Some(&error_text(err, s)));
            badge.add_css_class("caption");
            badge.add_css_class("dim-label");
            badge.set_ellipsize(gtk::pango::EllipsizeMode::End);
            badge.set_tooltip_text(Some(&error_text(err, s)));
            hbox.append(&badge);
        } else if active.as_deref() == Some(name.as_str()) {
            label.add_css_class("success");
            let badge = gtk::Label::new(Some(s.cfg_list_active));
            badge.add_css_class("caption");
            badge.add_css_class("dim-label");
            hbox.append(&badge);
        }

        let btn_edit = gtk::Button::from_icon_name("document-edit-symbolic");
        btn_edit.add_css_class("flat");
        btn_edit.set_tooltip_text(Some(s.common_edit));
        let btn_del = gtk::Button::from_icon_name("user-trash-symbolic");
        btn_del.add_css_class("flat");
        btn_del.add_css_class("destructive-action");
        btn_del.set_tooltip_text(Some(s.common_delete));

        hbox.append(&btn_edit);
        hbox.append(&btn_del);
        row.set_child(Some(&hbox));
        ui.list.cfg_list.append(&row);

        if selected.as_deref() == Some(name.as_str()) && broken.is_none() {
            select_idx = Some(i as u32);
        }

        // Edit button → open the form prefilled.
        {
            let name = name.clone();
            let ui = ui.clone();
            let state = state.clone();
            btn_edit.connect_clicked(move |_| {
                super::window::open_form(&ui, &state, Some(&name));
            });
        }
        // Delete button → confirmation dialog → remove file → rebuild.
        {
            let name = name.clone();
            let ui = ui.clone();
            let state = state.clone();
            btn_del.connect_clicked(move |_| {
                confirm_delete(&ui, &state, &name);
            });
        }
    }

    ui.update_empty_state(state);

    if let Some(idx) = select_idx
        && let Some(row) = ui.list.cfg_list.row_at_index(idx as i32)
    {
        ui.list.cfg_list.select_row(Some(&row));
    }
}

/// `AdwAlertDialog` confirmation → delete the file → refresh.
/// Returns the presented dialog so QA hooks can emit `response`.
pub(crate) fn confirm_delete(
    ui: &Rc<Ui>,
    state: &Rc<AppState>,
    name: &str,
) -> libadwaita::AlertDialog {
    let s = state.strings();
    let dialog = libadwaita::AlertDialog::new(
        Some(&fill(s.cfg_list_delete_confirm, &[("name", name)])),
        Some(name),
    );
    dialog.add_response("cancel", s.common_cancel);
    dialog.add_response("delete", s.common_delete);
    dialog.set_response_appearance("delete", libadwaita::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let ui_cb = ui.clone();
    let state = state.clone();
    let name = name.to_string();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let s = state.strings();
        match state.store.delete(&name) {
            Ok(()) => {
                if state.selected.borrow().as_deref() == Some(name.as_str()) {
                    *state.selected.borrow_mut() = None;
                    // The remembered row is gone too — forget it, otherwise
                    // the next launch would look for a deleted profile.
                    state.save_selected();
                    super::window::show_empty(&ui_cb, &state);
                }
                toast(&ui_cb.toast_overlay, s.cfg_list_delete_success);
                rebuild(&ui_cb, &state);
                super::window::update_status(&ui_cb, &state);
            }
            Err(e) => {
                let msg = format!("{}: {}", s.cfg_list_delete_failed, error_text(&e, s));
                toast(&ui_cb.toast_overlay, &msg);
            }
        }
    });
    dialog.present(Some(&ui.window));
    dialog
}
