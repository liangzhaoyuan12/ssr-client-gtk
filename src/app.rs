//! Global application state (GOAL §5 tree: the single state container).
//!
//! Everything here is owned by the GTK main thread; the two members that
//! cross into worker threads ([`ProxyService`], [`SysProxy`]) are wrapped
//! in `Arc` and only expose `&self` APIs with interior mutability.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use crate::config::model::ShadowsocksConfig;
use crate::config::prefs;
use crate::config::store::Store;
use crate::core::proxy::{ProxyEvent, ProxyService, ProxyState};
use crate::i18n::{LangHandle, Strings};
use crate::routing::RoutingSettings;
use crate::sysproxy::SysProxy;
use crate::sysproxy::SysProxyPref;
use crate::sysproxy::snapshot::Snapshot;

/// What the content area currently shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Nothing selected yet (status page).
    Empty,
    /// Create/edit form.
    Form,
    /// Dashboard for the selected profile.
    Dashboard,
}

pub struct AppState {
    /// Config CRUD (path + name validation live inside).
    pub store: Store,
    /// Proxy lifecycle; the app's only listener (invariant N1).
    pub proxy: Arc<ProxyService>,
    /// Receiver for lifecycle events pushed by [`ProxyService`].
    pub events: async_channel::Receiver<ProxyEvent>,
    /// System-proxy backends (KDE/GNOME strategy table).
    pub sys: Arc<SysProxy>,
    /// Pre-change system-proxy snapshot, `Some` while we own the change.
    pub snapshot: RefCell<Option<Snapshot>>,
    /// Cached `~/.ssr` listing.
    pub names: RefCell<Vec<String>>,
    /// Currently selected profile name.
    pub selected: RefCell<Option<String>>,
    /// Form mode: `Some(name)` = editing that profile.
    pub form_editing: RefCell<Option<String>>,
    /// Current content view.
    pub view: Cell<View>,
    /// An enable/disable operation is in flight.
    pub busy: Cell<bool>,
    /// Close-request cleanup already performed.
    pub cleaned_up: Cell<bool>,
    /// Language handle (persists on change).
    pub lang: LangHandle,
    /// Routing mode + ACL file + DNS choice (applied on the next enable).
    pub routing: RefCell<RoutingSettings>,
    /// Desktop settings vs. environment variables (applied on enable).
    pub sysproxy_pref: Cell<SysProxyPref>,
}

impl AppState {
    /// Build the state container: config store, proxy service + event
    /// channel, system-proxy strategy for this desktop, language handle.
    pub fn new() -> std::rc::Rc<Self> {
        let (tx, rx) = async_channel::unbounded();
        let store = Store::new();
        let _ = store.ensure_dir(); // first launch: create ~/.ssr
        let names = store.list().unwrap_or_default();
        std::rc::Rc::new(Self {
            store,
            proxy: Arc::new(ProxyService::new(tx)),
            events: rx,
            sys: Arc::new(SysProxy::system()),
            snapshot: RefCell::new(None),
            names: RefCell::new(names),
            selected: RefCell::new(None),
            form_editing: RefCell::new(None),
            view: Cell::new(View::Empty),
            busy: Cell::new(false),
            cleaned_up: Cell::new(false),
            lang: LangHandle::load(),
            routing: RefCell::new(prefs::load().routing),
            sysproxy_pref: Cell::new(prefs::load().sysproxy),
        })
    }

    /// Persist the routing/DNS settings (non-critical on failure: the
    /// in-memory value still applies to the next enable).
    pub fn save_routing(&self) {
        let mut prefs = prefs::load();
        prefs.routing = self.routing.borrow().clone();
        if let Err(e) = prefs::save(&prefs) {
            eprintln!("failed to persist routing settings: {e}");
        }
    }

    /// Persist the system-proxy strategy (same rules as [`Self::save_routing`]).
    pub fn save_sysproxy_pref(&self) {
        let mut prefs = prefs::load();
        prefs.sysproxy = self.sysproxy_pref.get();
        if let Err(e) = prefs::save(&prefs) {
            eprintln!("failed to persist system-proxy preference: {e}");
        }
    }

    /// Persist the sidebar selection so the next launch can restore it
    /// (non-critical on failure: same rules as [`Self::save_routing`]).
    pub fn save_selected(&self) {
        let mut prefs = prefs::load();
        prefs.selected_profile = self.selected.borrow().clone();
        if let Err(e) = prefs::save(&prefs) {
            eprintln!("failed to persist the selected profile: {e}");
        }
    }

    /// What the previous run had selected. The caller checks the name is
    /// still selectable — `names` only holds parseable profiles.
    pub fn remembered_selection(&self) -> Option<String> {
        prefs::load().selected_profile
    }

    /// Active string table for the current language.
    pub fn strings(&self) -> &'static Strings {
        self.lang.get().strings()
    }

    /// Re-scan the config directory (only parseable profiles count as
    /// selectable; corrupt files show up as error rows in the sidebar).
    pub fn refresh_names(&self) {
        let names: Vec<String> = self
            .store
            .scan()
            .into_iter()
            .filter(|e| e.error.is_none())
            .map(|e| e.name)
            .collect();
        *self.names.borrow_mut() = names;
    }

    /// Load the selected profile (if any).
    pub fn load_selected(&self) -> Result<ShadowsocksConfig, crate::error::AppError> {
        let name = self
            .selected
            .borrow()
            .clone()
            .ok_or_else(|| crate::error::AppError::NotFound(String::new()))?;
        self.store.load(&name)
    }

    /// Port to *display*: the running port while enabled, otherwise the
    /// selected profile's `listen_port` (invariant N1 — one port only).
    pub fn display_port(&self) -> Option<u16> {
        if let Some(state) = self.proxy.status() {
            return Some(state.port);
        }
        self.load_selected()
            .ok()
            .map(|c| c.client_settings.listen_port)
    }

    /// Name of the profile the proxy is serving, if running.
    pub fn active_config(&self) -> Option<ProxyState> {
        self.proxy.status()
    }
}
