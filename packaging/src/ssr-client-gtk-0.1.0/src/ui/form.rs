//! Config form: option lists sourced from `ssr-client-rs` enums (GOAL 4.2
//! — never a hand-written string table), field validation and the
//! form ⇄ [`ShadowsocksConfig`] mapping (ARCHITECTURE §5.4).

use gtk::prelude::*;
use gtk::{Builder, DropDown, StringList};

use ssr_client_rs::{CipherType, ObfsType, ProtocolType};

use crate::config::model::{ClientSettings, ShadowsocksConfig};
use crate::config::store::Store;
use crate::i18n::Strings;

/// Every cipher the core library supports (28).
pub const CIPHERS: &[CipherType] = &[
    CipherType::None,
    CipherType::Table,
    CipherType::RC4,
    CipherType::RC4Md56,
    CipherType::RC4Md5,
    CipherType::AES128CFB,
    CipherType::AES192CFB,
    CipherType::AES256CFB,
    CipherType::AES128CTR,
    CipherType::AES192CTR,
    CipherType::AES256CTR,
    CipherType::BFCFB,
    CipherType::Camellia128CFB,
    CipherType::Camellia192CFB,
    CipherType::Camellia256CFB,
    CipherType::CAST5CFB,
    CipherType::DESCFB,
    CipherType::IDEACFB,
    CipherType::RC2CFB,
    CipherType::SeedCFB,
    CipherType::Salsa20,
    CipherType::ChaCha20,
    CipherType::ChaCha20IETF,
    CipherType::AES128GCM,
    CipherType::AES192GCM,
    CipherType::AES256GCM,
    CipherType::ChaCha20Poly1305IETF,
    CipherType::XChaCha20Poly1305IETF,
];

/// Every protocol the core library supports (14).
pub const PROTOCOLS: &[ProtocolType] = &[
    ProtocolType::Origin,
    ProtocolType::VerifySimple,
    ProtocolType::AuthSimple,
    ProtocolType::AuthSHA1,
    ProtocolType::AuthSHA1V2,
    ProtocolType::AuthSHA1V4,
    ProtocolType::AuthAES128MD5,
    ProtocolType::AuthAES128SHA1,
    ProtocolType::AuthChainA,
    ProtocolType::AuthChainB,
    ProtocolType::AuthChainC,
    ProtocolType::AuthChainD,
    ProtocolType::AuthChainE,
    ProtocolType::AuthChainF,
];

/// Every obfuscation the core library supports (6).
pub const OBFS: &[ObfsType] = &[
    ObfsType::Plain,
    ObfsType::HTTPSimple,
    ObfsType::HTTPPost,
    ObfsType::HTTPMix,
    ObfsType::TLS12TicketAuth,
    ObfsType::TLS12TicketFastAuth,
];

/// Dropdown entries for the three option lists (names come from the lib).
pub fn option_names() -> (Vec<&'static str>, Vec<&'static str>, Vec<&'static str>) {
    (
        CIPHERS.iter().map(|c| c.name()).collect(),
        PROTOCOLS.iter().map(|p| p.name()).collect(),
        OBFS.iter().map(|o| o.name()).collect(),
    )
}

/// Index of `name` in a name list (dropdown selection helper).
pub fn index_of(names: &[&'static str], name: &str) -> u32 {
    names
        .iter()
        .position(|n| *n == name)
        .map(|i| i as u32)
        .unwrap_or(0)
}

/// What the user typed in the form (validated).
#[derive(Debug, Clone, PartialEq)]
pub struct FormInput {
    /// Profile name (becomes `<name>.json`).
    pub name: String,
    /// Fully-built file model.
    pub cfg: ShadowsocksConfig,
}

/// Form-level validation failure (mapped to localized text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormError {
    /// `cfg_name` failed `^[a-zA-Z]+$`.
    Name,
    /// Server address empty.
    Server,
    /// Password empty.
    Password,
    /// A port outside 1–65535.
    Port,
}

/// Localized message for a form error.
pub fn form_error_text(err: FormError, s: &Strings) -> &'static str {
    match err {
        FormError::Name => s.cf_validation_name,
        FormError::Server => s.cf_validation_server,
        FormError::Password => s.cf_validation_password,
        FormError::Port => s.cf_validation_port,
    }
}

/// Validate raw field values into a [`FormInput`].
///
/// Port bounds and the `cfg_name` rule are enforced here **and** again by
/// [`Store`] (GOAL 4.1: the store is the authoritative boundary).
#[allow(clippy::too_many_arguments)]
pub fn build_input(
    name: &str,
    server: &str,
    server_port: u16,
    password: &str,
    method: &str,
    protocol: &str,
    obfs: &str,
    protocol_param: &str,
    obfs_param: &str,
    udp: bool,
    idle_timeout: u32,
    connect_timeout: u32,
    udp_timeout: u32,
    listen_port: u16,
) -> Result<FormInput, FormError> {
    if Store::validate_name(name).is_err() {
        return Err(FormError::Name);
    }
    if server.trim().is_empty() {
        return Err(FormError::Server);
    }
    if password.is_empty() {
        return Err(FormError::Password);
    }
    if server_port == 0 || listen_port == 0 {
        return Err(FormError::Port);
    }
    Ok(FormInput {
        name: name.to_string(),
        cfg: ShadowsocksConfig {
            password: password.to_string(),
            method: method.to_string(),
            protocol: protocol.to_string(),
            protocol_param: protocol_param.to_string(),
            obfs: obfs.to_string(),
            obfs_param: obfs_param.to_string(),
            udp,
            idle_timeout,
            connect_timeout,
            udp_timeout,
            client_settings: ClientSettings {
                server: server.trim().to_string(),
                server_port,
                listen_address: "0.0.0.0".to_string(), // GOAL D2 (store also enforces)
                listen_port,
            },
        },
    })
}

/// Handles for the interactive form widgets.
pub struct FormUi {
    /// Builder of `config_form.ui` (label lookups for language switches).
    pub builder: Builder,
    pub root: gtk::Box,
    pub title: gtk::Label,
    pub error: gtk::Label,
    pub entry_name: gtk::Entry,
    pub entry_server: gtk::Entry,
    pub spin_server_port: gtk::SpinButton,
    pub entry_password: gtk::PasswordEntry,
    pub dd_method: DropDown,
    pub dd_protocol: DropDown,
    pub dd_obfs: DropDown,
    pub entry_protocol_param: gtk::Entry,
    pub entry_obfs_param: gtk::Entry,
    pub sw_udp: gtk::Switch,
    pub spin_idle: gtk::SpinButton,
    pub spin_connect: gtk::SpinButton,
    pub spin_udp: gtk::SpinButton,
    pub spin_listen: gtk::SpinButton,
    pub btn_save: gtk::Button,
    pub btn_cancel: gtk::Button,
}

impl FormUi {
    /// Load `config_form.ui`, fill the three dropdown models from the
    /// library enums.
    pub fn new(builder: Builder) -> Self {
        let b = |id: &str| builder.object::<gtk::Box>(id).expect(id);
        let lbl = |id: &str| builder.object::<gtk::Label>(id).expect(id);
        let entry = |id: &str| builder.object::<gtk::Entry>(id).expect(id);
        let spin = |id: &str| builder.object::<gtk::SpinButton>(id).expect(id);

        let dd_method = builder.object::<DropDown>("dd_method").expect("dd_method");
        let dd_protocol = builder
            .object::<DropDown>("dd_protocol")
            .expect("dd_protocol");
        let dd_obfs = builder.object::<DropDown>("dd_obfs").expect("dd_obfs");

        let (methods, protocols, obfs_names) = option_names();
        dd_method.set_model(Some(&StringList::new(&methods)));
        dd_protocol.set_model(Some(&StringList::new(&protocols)));
        dd_obfs.set_model(Some(&StringList::new(&obfs_names)));

        // sensible defaults (old app's formData defaults)
        dd_method.set_selected(index_of(&methods, "aes-128-ctr"));
        dd_protocol.set_selected(index_of(&protocols, "auth_aes128_md5"));
        dd_obfs.set_selected(index_of(&obfs_names, "tls1.2_ticket_auth"));

        Self {
            root: b("form_root"),
            title: lbl("form_title"),
            error: builder
                .object::<gtk::Label>("form_error")
                .expect("form_error"),
            entry_name: entry("entry_name"),
            entry_server: entry("entry_server"),
            spin_server_port: spin("spin_server_port"),
            entry_password: builder
                .object::<gtk::PasswordEntry>("entry_password")
                .expect("entry_password"),
            dd_method,
            dd_protocol,
            dd_obfs,
            entry_protocol_param: entry("entry_protocol_param"),
            entry_obfs_param: entry("entry_obfs_param"),
            sw_udp: builder.object::<gtk::Switch>("sw_udp").expect("sw_udp"),
            spin_idle: spin("spin_idle_timeout"),
            spin_connect: spin("spin_connect_timeout"),
            spin_udp: spin("spin_udp_timeout"),
            spin_listen: spin("spin_listen_port"),
            btn_save: builder.object::<gtk::Button>("btn_save").expect("btn_save"),
            btn_cancel: builder
                .object::<gtk::Button>("btn_cancel")
                .expect("btn_cancel"),
            builder,
        }
    }

    /// Reset the form for a fresh profile (defaults from §5.4).
    pub fn reset(&self) {
        self.entry_name.set_text("");
        self.entry_name.set_sensitive(true);
        self.entry_server.set_text("");
        self.spin_server_port.set_value(443.0);
        self.entry_password.set_text("");
        let (methods, protocols, obfs_names) = option_names();
        self.dd_method
            .set_selected(index_of(&methods, "aes-128-ctr"));
        self.dd_protocol
            .set_selected(index_of(&protocols, "auth_aes128_md5"));
        self.dd_obfs
            .set_selected(index_of(&obfs_names, "tls1.2_ticket_auth"));
        self.entry_protocol_param.set_text("");
        self.entry_obfs_param.set_text("");
        self.sw_udp.set_active(true);
        self.spin_idle.set_value(300.0);
        self.spin_connect.set_value(6.0);
        self.spin_udp.set_value(6.0);
        self.spin_listen.set_value(1080.0);
        self.hide_error();
    }

    /// Fill the form from an existing profile (edit mode).
    pub fn fill(&self, name: &str, cfg: &ShadowsocksConfig) {
        let (methods, protocols, obfs_names) = option_names();
        self.reset();
        self.entry_name.set_text(name);
        self.entry_name.set_sensitive(false); // edit: name is frozen (§4.1)
        self.entry_server.set_text(&cfg.client_settings.server);
        self.spin_server_port
            .set_value(f64::from(cfg.client_settings.server_port));
        self.entry_password.set_text(&cfg.password);
        self.dd_method.set_selected(index_of(&methods, &cfg.method));
        self.dd_protocol
            .set_selected(index_of(&protocols, &cfg.protocol));
        self.dd_obfs.set_selected(index_of(&obfs_names, &cfg.obfs));
        self.entry_protocol_param.set_text(&cfg.protocol_param);
        self.entry_obfs_param.set_text(&cfg.obfs_param);
        self.sw_udp.set_active(cfg.udp);
        self.spin_idle.set_value(f64::from(cfg.idle_timeout));
        self.spin_connect.set_value(f64::from(cfg.connect_timeout));
        self.spin_udp.set_value(f64::from(cfg.udp_timeout));
        self.spin_listen
            .set_value(f64::from(cfg.client_settings.listen_port));
    }

    /// Selected option names for method/protocol/obfs.
    fn selected_names(&self) -> (String, String, String) {
        let pick = |dd: &DropDown| -> String {
            dd.selected_item()
                .and_then(|o| o.downcast::<gtk::StringObject>().ok())
                .map(|s| s.string().to_string())
                .unwrap_or_default()
        };
        (
            pick(&self.dd_method),
            pick(&self.dd_protocol),
            pick(&self.dd_obfs),
        )
    }

    /// Validate the current field values.
    pub fn read(&self) -> Result<FormInput, FormError> {
        let (method, protocol, obfs) = self.selected_names();
        let server_port = self.spin_server_port.value().round() as u16;
        let listen_port = self.spin_listen.value().round() as u16;
        build_input(
            &self.entry_name.text(),
            &self.entry_server.text(),
            server_port,
            &self.entry_password.text(),
            &method,
            &protocol,
            &obfs,
            &self.entry_protocol_param.text(),
            &self.entry_obfs_param.text(),
            self.sw_udp.is_active(),
            self.spin_idle.value().round() as u32,
            self.spin_connect.value().round() as u32,
            self.spin_udp.value().round() as u32,
            listen_port,
        )
    }

    /// Show an inline error under the title (never a raw stack trace).
    pub fn show_error(&self, text: &str) {
        self.error.set_label(text);
        self.error.set_visible(true);
    }

    /// Hide the inline error.
    pub fn hide_error(&self) {
        self.error.set_visible(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GOAL 4.2: dropdown data comes from the library enums; every listed
    /// name parses back through the library itself (no drift possible
    /// between what we show and what the core accepts).
    #[test]
    fn option_lists_roundtrip_through_the_library() {
        let (methods, protocols, obfs) = option_names();
        assert_eq!(methods.len(), CIPHERS.len());
        assert_eq!(protocols.len(), PROTOCOLS.len());
        assert_eq!(obfs.len(), OBFS.len());
        for (i, name) in methods.iter().enumerate() {
            let parsed = CipherType::from_name(name).expect(name);
            assert_eq!(parsed, CIPHERS[i], "cipher name mismatch for {name}");
        }
        for (i, name) in protocols.iter().enumerate() {
            let parsed = ProtocolType::from_name(name).expect(name);
            assert_eq!(parsed, PROTOCOLS[i], "protocol name mismatch for {name}");
        }
        for (i, name) in obfs.iter().enumerate() {
            let parsed = ObfsType::from_name(name).expect(name);
            assert_eq!(parsed, OBFS[i], "obfs name mismatch for {name}");
        }
    }

    /// Old defaults survive the round trip (ARCHITECTURE §5.4).
    #[test]
    fn defaults_match_the_old_form() {
        let input = build_input(
            "hk",
            "206.237.10.116",
            2800,
            "secret",
            "aes-128-ctr",
            "auth_aes128_md5",
            "tls1.2_ticket_auth",
            "",
            "",
            true,
            300,
            6,
            6,
            1080,
        )
        .expect("valid input");
        assert_eq!(input.cfg.method, "aes-128-ctr");
        assert_eq!(input.cfg.client_settings.listen_address, "0.0.0.0");
        assert_eq!(input.cfg.client_settings.listen_port, 1080);
        assert_eq!(input.cfg.client_settings.server_port, 2800);
    }

    /// Validation catches the three §5.4 rules plus port bounds.
    #[test]
    fn validation_rules() {
        let base = |name: &str, server: &str, pass: &str, lport: u16, sport: u16| {
            build_input(
                name,
                server,
                sport,
                pass,
                "aes-128-ctr",
                "origin",
                "plain",
                "",
                "",
                true,
                300,
                6,
                6,
                lport,
            )
        };
        assert_eq!(base("../etc", "s", "p", 1080, 443), Err(FormError::Name));
        assert_eq!(base("abc1", "s", "p", 1080, 443), Err(FormError::Name));
        assert_eq!(base("abc", "", "p", 1080, 443), Err(FormError::Server));
        assert_eq!(base("abc", "s", "", 1080, 443), Err(FormError::Password));
        assert_eq!(base("abc", "s", "p", 0, 443), Err(FormError::Port));
        assert_eq!(base("abc", "s", "p", 1080, 0), Err(FormError::Port));
        assert!(base("abc", "s", "p", 1080, 443).is_ok());
    }
}
