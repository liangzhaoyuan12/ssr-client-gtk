//! Custom ACL editor — a notepad-style dialog that edits the rules
//! **in place** instead of importing an external file (the old
//! `gtk::FileDialog` chooser).
//!
//! Everything the user types is written to the managed file
//! [`managed_path`] (`~/.ssr/custom.acl`), so `routing.acl_path` always
//! points at a file the app owns and can rewrite. An ACL that a previous
//! version picked from elsewhere is loaded into the editor on open and
//! migrated into the managed file on the first save.
//!
//! The dialog shows the shadowsocks-rust ACL syntax (translated, from
//! `i18n`) right above the editor, validates the text on save through
//! [`Acl::parse`] and refuses to persist a file the router would reject —
//! the same "never save junk" rule the DNS box follows.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};

use crate::app::AppState;
use crate::error::{AppError, AppResult};
use crate::i18n::{Strings, error_text, fill};
use crate::routing::RouteMode;
use crate::routing::acl::Acl;
use crate::ui::window::Ui;

/// The ACL file the editor owns, inside the config directory.
pub fn managed_path() -> PathBuf {
    crate::config::path::config_dir().join("custom.acl")
}

/// Starting text: the previously picked file if readable (an external ACL
/// gets edited in place, then migrated on save), else the managed file,
/// else the localized template.
fn initial_text(s: &Strings, state: &AppState) -> String {
    let picked = state.routing.borrow().acl_path.trim().to_string();
    let candidates: Vec<PathBuf> = [(!picked.is_empty()).then(|| PathBuf::from(&picked))]
        .into_iter()
        .flatten()
        .chain([managed_path()])
        .collect();
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return text;
        }
    }
    s.route_acl_template.to_string()
}

/// Atomically write the rules to the managed file (temp file + rename,
/// same shape as the language preference write).
fn save_managed(path: &Path, text: &str) -> AppResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Acl("managed ACL path has no directory".into()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".custom.acl.part");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Open the editor on top of the dashboard. Closing it (Esc, ✕ or 取消)
/// re-syncs the routing card, so a mode dropdown left on "自定义 ACL"
/// snaps back to the stored mode when the user cancels without saving.
pub fn open(ui: &Rc<Ui>, state: &Rc<AppState>) {
    let s = state.strings();

    let win = gtk::Window::new();
    win.set_transient_for(Some(&ui.window));
    win.set_modal(true);
    win.set_title(Some(s.route_acl_editor_title));
    win.set_default_size(740, 740);

    let header = gtk::HeaderBar::new();
    let title = gtk::Label::new(Some(s.route_acl_editor_title));
    title.add_css_class("heading");
    header.set_title_widget(Some(&title));
    win.set_titlebar(Some(&header));

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(10);
    vbox.set_margin_bottom(12);

    // --- 编写规则 (always visible, own scroll area) ---------------------
    let help_title = gtk::Label::new(Some(s.route_acl_help_title));
    help_title.set_xalign(0.0);
    help_title.add_css_class("heading");
    vbox.append(&help_title);

    let help = gtk::Label::new(Some(s.route_acl_help));
    help.set_xalign(0.0);
    help.set_wrap(true);
    help.set_wrap_mode(pango::WrapMode::WordChar);
    help.set_selectable(true);
    help.add_css_class("monospace");
    help.add_css_class("dim-label");
    let help_scroll = gtk::ScrolledWindow::new();
    help_scroll.set_child(Some(&help));
    help_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    // Grow to the whole help text when it fits, cap it (and scroll) when
    // it does not — the rules list must not be silently cut in half.
    help_scroll.set_propagate_natural_height(true);
    help_scroll.set_max_content_height(400);
    vbox.append(&help_scroll);

    // --- the editor ----------------------------------------------------
    let editor = gtk::TextView::new();
    editor.set_monospace(true);
    editor.set_wrap_mode(gtk::WrapMode::None);
    editor.set_top_margin(6);
    editor.set_bottom_margin(6);
    editor.set_left_margin(10);
    editor.set_right_margin(10);
    let buffer = editor.buffer();
    buffer.set_text(&initial_text(s, state));
    let edit_scroll = gtk::ScrolledWindow::new();
    edit_scroll.set_child(Some(&editor));
    edit_scroll.set_vexpand(true);
    edit_scroll.set_min_content_height(220);
    vbox.append(&edit_scroll);

    // --- inline error (parse failure / write failure) -------------------
    let err_label = gtk::Label::new(None);
    err_label.set_xalign(0.0);
    err_label.set_wrap(true);
    err_label.set_visible(false);
    err_label.add_css_class("error");
    vbox.append(&err_label);

    // --- buttons --------------------------------------------------------
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let btn_cancel = gtk::Button::with_label(s.common_cancel);
    let btn_save = gtk::Button::with_label(s.common_save);
    btn_save.add_css_class("suggested-action");
    buttons.append(&btn_cancel);
    buttons.append(&btn_save);
    vbox.append(&buttons);

    win.set_child(Some(&vbox));

    // Cancel: just close; the close-request handler re-syncs the card.
    {
        let win = win.clone();
        btn_cancel.connect_clicked(move |b| {
            let _ = b;
            win.close();
        });
    }

    // Save: validate → write the managed file → point the settings at it.
    {
        let win = win.clone();
        let ui = ui.clone();
        let state = state.clone();
        let buffer = buffer.clone();
        let err_label = err_label.clone();
        btn_save.connect_clicked(move |_| {
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if let Err(e) = Acl::parse(&text) {
                err_label.set_label(&fill(s.route_acl_parse_error, &[("error", &e)]));
                err_label.set_visible(true);
                return;
            }
            let path = managed_path();
            if let Err(e) = save_managed(&path, &text) {
                err_label.set_label(&error_text(&e, s));
                err_label.set_visible(true);
                return;
            }
            err_label.set_visible(false);
            {
                let mut routing = state.routing.borrow_mut();
                routing.acl_path = path.to_string_lossy().to_string();
                routing.mode = RouteMode::Acl;
            }
            state.save_routing();
            win.close(); // close-request re-syncs the routing card
            crate::ui::dashboard::refresh_routing(&ui, &state);
            crate::ui::toast::toast(&ui.toast_overlay, s.route_acl_saved);
            crate::ui::dashboard::hint_restart(&ui, &state);
        });
    }

    // Esc closes; closing re-syncs the routing card (undo a half-changed
    // mode dropdown when the user cancels without saving).
    {
        let win = win.clone();
        let ui = ui.clone();
        let state = state.clone();
        win.connect_close_request(move |_| {
            crate::ui::dashboard::refresh_routing(&ui, &state);
            glib::Propagation::Proceed
        });
        let win_key = win.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                win_key.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        win.add_controller(key);
    }

    win.present();
    editor.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{EN, ZH};

    /// The managed file lives in the config directory — that is what
    /// `routing.acl_path` ends up pointing at after a save.
    #[test]
    fn managed_file_lives_in_the_config_dir() {
        let p = managed_path();
        assert_eq!(p.file_name().unwrap(), "custom.acl");
        assert!(p.starts_with(crate::config::path::config_dir()));
    }

    /// The write path must land atomically: content in the target file,
    /// no `.part` left behind, and a second save overwrites cleanly.
    #[test]
    fn save_managed_writes_and_overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.acl");

        save_managed(&path, "[proxy_all]\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[proxy_all]\n");
        assert!(
            !dir.path().join(".custom.acl.part").exists(),
            "the temp file must be renamed away"
        );

        save_managed(&path, "[bypass_all]\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[bypass_all]\n");
        // The parser guards the save button, so whatever reached disk is
        // loadable by the router at enable time.
        Acl::load(&path).expect("saved file must load");
    }

    /// The seed text must pass the very parser that guards the save
    /// button, in both languages — a template the app itself rejects
    /// would make the editor unusable.
    #[test]
    fn template_parses_in_both_languages() {
        for s in [&ZH, &EN] {
            Acl::parse(s.route_acl_template).expect("template must be valid ACL");
        }
    }

    /// The help text is the editor's documentation: it must cover the
    /// section headers and every rule spelling the parser understands.
    #[test]
    fn help_text_documents_sections_and_rule_spellings() {
        for s in [&ZH, &EN] {
            for token in [
                "[proxy_all]",
                "[bypass_all]",
                "[bypass_list]",
                "[proxy_list]",
                "||",
                "|example.com",
                "10.0.0.0/8",
            ] {
                assert!(s.route_acl_help.contains(token), "{token} missing in help");
            }
        }
    }
}
