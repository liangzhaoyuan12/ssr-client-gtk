//! Traffic routing: mode selection, address sets, ACL files and local DNS.
//!
//! The `ssr-client-rs` core only knows how to push bytes through one SSR
//! server — it has no routing at all. Everything in this module is the
//! decision layer the app puts in front of it: for every SOCKS5 target we
//! decide **direct** (connect from this machine) or **proxy** (open an SSR
//! session), so the five user-facing modes work:
//!
//! | mode | direct when |
//! |---|---|
//! | 全局 global | only loopback / mDNS names |
//! | 绕开局域网 bypass LAN | target inside the LAN set |
//! | 绕开中国大陆 bypass CN | target inside the vendored China set |
//! | 绕开局域网及大陆 bypass LAN + CN | inside either set |
//! | 自定义 ACL | the ACL file says so |
//!
//! Fail-safe rule: when a decision needs a local DNS answer and the answer
//! is unknown, the target goes **through the proxy** — bypassing is an
//! optimisation, reachability is not.

pub mod acl;
pub mod cidr;
pub mod dns;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use ssr_client_rs::socks5::TargetAddress;

use crate::error::{AppError, AppResult};
use cidr::{CidrSet, cn_set, lan_set};
use dns::{DnsConfig, DnsResolver};

/// Where a single connection is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Connect directly from this machine.
    Direct,
    /// Tunnel through the SSR server.
    Proxy,
}

/// The five routing modes (persisted to `settings.json`, shown in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteMode {
    /// Everything through the SSR server (historical behaviour).
    #[default]
    Global,
    /// Private / special-purpose addresses go direct.
    BypassLan,
    /// Mainland-China addresses go direct.
    BypassCn,
    /// Both of the above go direct.
    BypassLanCn,
    /// Rules come from a user-supplied ACL file.
    Acl,
}

impl RouteMode {
    /// Every mode, in UI order.
    pub const ALL: [RouteMode; 5] = [
        RouteMode::Global,
        RouteMode::BypassLan,
        RouteMode::BypassCn,
        RouteMode::BypassLanCn,
        RouteMode::Acl,
    ];
}

/// Everything the routing card stores in `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingSettings {
    /// Active mode.
    #[serde(default)]
    pub mode: RouteMode,
    /// ACL file path, only consulted when `mode == Acl`.
    #[serde(default)]
    pub acl_path: String,
    /// Local resolver (system or custom).
    #[serde(default)]
    pub dns: DnsConfig,
}

impl Default for RoutingSettings {
    fn default() -> Self {
        Self {
            mode: RouteMode::Global,
            acl_path: String::new(),
            dns: DnsConfig::System,
        }
    }
}

impl RoutingSettings {
    /// Validate cheaply, without building a [`Router`] (used by the UI on
    /// save so the user gets an immediate error instead of at enable time).
    pub fn validate(&self) -> AppResult<()> {
        if self.mode == RouteMode::Acl {
            let path = self.acl_path.trim();
            if path.is_empty() {
                return Err(AppError::Acl("no ACL file selected".into()));
            }
            acl::Acl::load(std::path::Path::new(path))?;
        }
        if let DnsConfig::Custom(servers) = &self.dns
            && servers.is_empty()
        {
            return Err(AppError::Dns(
                "custom DNS selected but no server given".into(),
            ));
        }
        Ok(())
    }
}

/// Hostnames that are **always** direct, whatever the mode: loopback and
/// link-local mDNS names. The SSR server sits on the other side of the
/// internet — it can neither reach our `127.0.0.1` nor resolve `*.local`.
pub fn is_always_direct_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".localdomain")
        || h == "ip6-localhost"
        || h == "ip6-loopback"
}

/// Hostnames treated as LAN by the bypass-LAN modes (in addition to the
/// always-direct set above).
pub fn is_lan_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    is_always_direct_host(h.as_str())
        || h.ends_with(".lan")
        || h.ends_with(".home")
        || h.ends_with(".internal")
        || h.ends_with(".intranet")
        || h.ends_with(".corp")
        || h.ends_with(".home.arpa")
        || h.ends_with(".local")
}

/// Loopback *and* unspecified addresses are never worth tunneling: the SSR
/// server can reach neither our `127.0.0.1` nor `0.0.0.0`/`::`.
fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
}

/// Resolved, built routing state for one enable cycle: the mode, the ACL
/// (if any) and the local resolver.
///
/// Shared by every connection task through an `Arc`; nothing inside is
/// mutated after `build`.
pub struct Router {
    mode: RouteMode,
    acl: Option<acl::Acl>,
    dns: DnsResolver,
    lan: &'static CidrSet,
    cn: &'static CidrSet,
}

impl Router {
    /// Build the router for `settings`, validating the ACL file and the
    /// DNS configuration up front so a bad setting fails *before* anything
    /// is bound or written.
    pub fn build(settings: &RoutingSettings) -> AppResult<Router> {
        let dns = DnsResolver::build(&settings.dns).map_err(|e| AppError::Dns(e.0))?;
        let acl = match settings.mode {
            RouteMode::Acl => Some(acl::Acl::load(std::path::Path::new(&settings.acl_path))?),
            _ => None,
        };
        let lan = lan_set();
        let cn = cn_set();
        log::debug!(
            "routing built: mode={:?}, {} LAN rules, {} mainland-CN rules",
            settings.mode,
            lan.len(),
            cn.len()
        );
        Ok(Router {
            mode: settings.mode,
            acl,
            dns,
            lan,
            cn,
        })
    }

    /// Router configured for tests (mode + system DNS, optional ACL text).
    #[cfg(test)]
    pub fn for_test(mode: RouteMode) -> Router {
        Router {
            mode,
            acl: None,
            dns: DnsResolver::build(&DnsConfig::System).expect("system resolver"),
            lan: lan_set(),
            cn: cn_set(),
        }
    }

    /// Resolve a hostname with the configured local resolver.
    pub async fn resolve(&self, host: &str) -> Vec<IpAddr> {
        self.dns.resolve(host).await
    }

    /// Decide where `target` goes.
    pub async fn decide(&self, target: &TargetAddress) -> Route {
        let host: Option<String> = match target {
            TargetAddress::IPv4(oct) => {
                return self.decide_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                    oct[0], oct[1], oct[2], oct[3],
                )));
            }
            TargetAddress::IPv6(oct) => {
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(oct);
                return self.decide_ip(IpAddr::V6(bytes.into()));
            }
            TargetAddress::Domain(raw) => Some(String::from_utf8_lossy(raw).into_owned()),
        };
        let Some(host) = host else {
            return Route::Proxy;
        };
        self.decide_host(&host).await
    }

    /// Decide for a hostname (separated out so the domain rules are unit
    /// testable without a socket).
    pub async fn decide_host(&self, host: &str) -> Route {
        if is_always_direct_host(host) {
            return Route::Direct;
        }
        match self.mode {
            RouteMode::Global => Route::Proxy,
            RouteMode::Acl => {
                let acl = self.acl.as_ref().expect("acl mode always has a rule set");
                match acl.decide_host(host) {
                    Some(route) => route,
                    // No domain rule: consult the IP rules after a local
                    // lookup — but only when they exist, otherwise the mode
                    // default answers without a DNS round-trip (ss-rust
                    // `check_target_bypassed`).
                    None if !acl.needs_ip_lookup() => acl.default_route(),
                    None => {
                        let ips = self.resolve(host).await;
                        acl.decide_ips(&ips)
                    }
                }
            }
            _ => {
                // LAN/CN modes must see addresses: resolve locally, then
                // require *every* answer to fall in the bypassed set.
                if self.mode == RouteMode::BypassLan && is_lan_host(host) {
                    return Route::Direct;
                }
                let ips = self.resolve(host).await;
                self.decide_ips(&ips)
            }
        }
    }

    /// Decide for a single IP literal (no DNS involved).
    pub fn decide_ip(&self, ip: IpAddr) -> Route {
        if is_loopback(ip) {
            return Route::Direct;
        }
        match self.mode {
            RouteMode::Global => Route::Proxy,
            RouteMode::BypassLan => self.bypass(self.lan, ip),
            RouteMode::BypassCn => self.bypass(self.cn, ip),
            RouteMode::BypassLanCn => {
                if self.lan.contains(ip) || self.cn.contains(ip) {
                    Route::Direct
                } else {
                    Route::Proxy
                }
            }
            RouteMode::Acl => self
                .acl
                .as_ref()
                .map(|acl| acl.decide_ip(ip))
                .unwrap_or(Route::Proxy),
        }
    }

    /// Decide for a resolved set: direct only when **all** answers are
    /// bypassed (a half-Chinese, half-foreign answer is treated as
    /// "not provably bypassable" and goes through the proxy).
    pub fn decide_ips(&self, ips: &[IpAddr]) -> Route {
        if ips.is_empty() {
            return Route::Proxy;
        }
        if ips.iter().all(|ip| is_loopback(*ip)) {
            return Route::Direct;
        }
        match self.mode {
            RouteMode::Global => Route::Proxy,
            RouteMode::BypassLan => {
                if ips.iter().all(|ip| self.lan.contains(*ip)) {
                    Route::Direct
                } else {
                    Route::Proxy
                }
            }
            RouteMode::BypassCn => {
                if ips.iter().all(|ip| self.cn.contains(*ip)) {
                    Route::Direct
                } else {
                    Route::Proxy
                }
            }
            RouteMode::BypassLanCn => {
                if ips
                    .iter()
                    .all(|ip| self.lan.contains(*ip) || self.cn.contains(*ip))
                {
                    Route::Direct
                } else {
                    Route::Proxy
                }
            }
            RouteMode::Acl => self
                .acl
                .as_ref()
                .map(|acl| acl.decide_ips(ips))
                .unwrap_or(Route::Proxy),
        }
    }

    fn bypass(&self, set: &'static CidrSet, ip: IpAddr) -> Route {
        if set.contains(ip) {
            Route::Direct
        } else {
            Route::Proxy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d))
    }

    fn target_v4(a: u8, b: u8, c: u8, d: u8) -> TargetAddress {
        TargetAddress::IPv4([a, b, c, d])
    }

    fn target_host(host: &str) -> TargetAddress {
        TargetAddress::Domain(host.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn global_mode_proxies_everything_except_loopback() {
        let r = Router::for_test(RouteMode::Global);
        assert_eq!(r.decide(&target_v4(8, 8, 8, 8)).await, Route::Proxy);
        assert_eq!(r.decide(&target_v4(192, 168, 1, 1)).await, Route::Proxy);
        assert_eq!(r.decide(&target_v4(114, 114, 114, 114)).await, Route::Proxy);
        assert_eq!(r.decide(&target_v4(127, 0, 0, 1)).await, Route::Direct);
        assert_eq!(
            r.decide(&TargetAddress::IPv6([0; 16])).await,
            Route::Direct,
            ":: is loopback-ish only via ::1; :: is 'unspecified' → direct is safe"
        );
        assert_eq!(r.decide(&target_host("localhost")).await, Route::Direct);
        assert_eq!(r.decide(&target_host("printer.local")).await, Route::Direct);
        assert_eq!(r.decide(&target_host("example.com")).await, Route::Proxy);
    }

    #[tokio::test]
    async fn bypass_lan_mode_splits_private_and_public() {
        let r = Router::for_test(RouteMode::BypassLan);
        for lan in [
            target_v4(10, 1, 2, 3),
            target_v4(172, 16, 0, 1),
            target_v4(192, 168, 0, 1),
            target_v4(127, 0, 0, 1),
            target_v4(169, 254, 10, 10),
        ] {
            assert_eq!(r.decide(&lan).await, Route::Direct, "{lan:?}");
        }
        assert_eq!(r.decide(&target_v4(1, 2, 4, 1)).await, Route::Proxy);
        assert_eq!(r.decide(&target_v4(172, 32, 0, 1)).await, Route::Proxy);
        assert_eq!(
            r.decide(&target_host("nas.lan")).await,
            Route::Direct,
            "LAN hostname suffix bypassed without DNS"
        );
    }

    #[tokio::test]
    async fn bypass_cn_mode_uses_the_vendored_list() {
        let r = Router::for_test(RouteMode::BypassCn);
        assert_eq!(r.decide(&target_v4(223, 5, 5, 5)).await, Route::Direct);
        assert_eq!(
            r.decide(&target_v4(114, 114, 114, 114)).await,
            Route::Direct
        );
        assert_eq!(r.decide(&target_v4(8, 8, 8, 8)).await, Route::Proxy);
        // Private space is *not* China: bypass-CN does not imply bypass-LAN.
        assert_eq!(r.decide(&target_v4(192, 168, 1, 1)).await, Route::Proxy);
        // Loopback stays direct in every mode.
        assert_eq!(r.decide(&target_v4(127, 0, 0, 1)).await, Route::Direct);
    }

    #[tokio::test]
    async fn bypass_lan_cn_is_the_union() {
        let r = Router::for_test(RouteMode::BypassLanCn);
        assert_eq!(r.decide(&target_v4(192, 168, 1, 1)).await, Route::Direct);
        assert_eq!(r.decide(&target_v4(223, 5, 5, 5)).await, Route::Direct);
        assert_eq!(r.decide(&target_v4(1, 1, 1, 1)).await, Route::Proxy);
        assert_eq!(r.decide(&target_v4(8, 8, 8, 8)).await, Route::Proxy);
    }

    #[test]
    fn ip_sets_never_cross_families() {
        let r = Router::for_test(RouteMode::BypassCn);
        // An IPv6 Google address must not be treated as Chinese just
        // because its low bytes match something.
        let v6: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert_eq!(r.decide_ip(v6), Route::Proxy);
        let cn_v6: IpAddr = "2400:3200::1".parse().unwrap(); // Aliyun CN block
        assert_eq!(r.decide_ip(cn_v6), Route::Direct);
    }

    #[test]
    fn decide_ips_requires_all_answers_to_be_bypassed() {
        let r = Router::for_test(RouteMode::BypassCn);
        assert_eq!(
            r.decide_ips(&[v4(223, 5, 5, 5), v4(114, 114, 114, 114)]),
            Route::Direct
        );
        assert_eq!(
            r.decide_ips(&[v4(223, 5, 5, 5), v4(8, 8, 8, 8)]),
            Route::Proxy,
            "mixed answer must not be bypassed"
        );
        assert_eq!(
            r.decide_ips(&[]),
            Route::Proxy,
            "unknown DNS answer fails safe to the proxy"
        );
    }

    #[test]
    fn hostname_rules_are_case_and_dot_insensitive() {
        assert!(is_always_direct_host("LocalHost."));
        assert!(is_always_direct_host("X.LocalHost"));
        assert!(is_always_direct_host("mypi.local"));
        assert!(!is_always_direct_host("local.example.com"));
        assert!(is_lan_host("nas.lan"));
        assert!(is_lan_host("router.home"));
        assert!(!is_lan_host("example.com"));
    }

    #[test]
    fn settings_validate_acl_and_dns_before_anything_starts() {
        let mut s = RoutingSettings::default();
        assert!(s.validate().is_ok(), "defaults must validate");

        s.mode = RouteMode::Acl;
        assert!(
            matches!(s.validate(), Err(AppError::Acl(_))),
            "acl mode without a file must be rejected"
        );

        s.mode = RouteMode::Global;
        assert!(s.validate().is_ok());
        s.dns = dns::DnsConfig::Custom(Vec::new());
        assert!(
            matches!(s.validate(), Err(AppError::Dns(_))),
            "custom DNS without a server must be rejected"
        );
    }

    #[test]
    fn settings_serialise_with_stable_keys() {
        // `env::temp_dir()` instead of a literal `/tmp` — GOAL §11 A5.
        let acl = std::env::temp_dir().join("x.acl").display().to_string();
        let s = RoutingSettings {
            mode: RouteMode::BypassLanCn,
            acl_path: acl.clone(),
            dns: dns::DnsConfig::parse("223.5.5.5").unwrap(),
        };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"bypass-lan-cn\""), "{text}");
        assert!(text.contains("\"custom\""), "{text}");
        let back: RoutingSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        // Old settings files (language only) must still parse.
        let legacy: RoutingSettings = serde_json::from_str("{}").expect("all fields defaulted");
        assert_eq!(legacy, RoutingSettings::default());
    }
}
