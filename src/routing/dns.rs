//! Local DNS resolution — "系统 DNS" vs "自定义 DNS".
//!
//! Scope (documented in the README): the resolver answers questions the
//! *client* has to answer locally — routing decisions (is this domain a
//! China address? a LAN address?) and the address of a **direct**
//! connection. Domains that go through the SSR tunnel are handed to the
//! server as domains, so proxied connections use the server's DNS and are
//! unaffected by this setting.

use std::net::IpAddr;

use hickory_resolver::Resolver;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use serde::{Deserialize, Serialize};

/// Which resolver the client uses for its own lookups — persisted as
/// `{"mode":"system"}` / `{"mode":"custom","servers":["223.5.5.5"]}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", content = "servers", rename_all = "kebab-case")]
pub enum DnsConfig {
    /// The operating system resolver (`getaddrinfo` — /etc/resolv.conf,
    /// nsswitch, systemd-resolved… whatever the machine is configured as).
    #[default]
    System,
    /// One or more explicit servers, queried on UDP/TCP port 53.
    Custom(Vec<IpAddr>),
    /// 阿里云 AliDNS — dropdown preset, queried as UDP/TCP 53 like `Custom`.
    Ali,
    /// 腾讯云 DNSPod Public DNS+ — dropdown preset, same transport.
    Tencent,
}

/// Why a custom-DNS setting was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsSettingsError(pub String);

impl std::fmt::Display for DnsSettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl DnsConfig {
    /// AliDNS public resolvers (`223.5.5.5` / `223.6.6.6` + Anycast v6).
    pub const ALI_SERVERS: [IpAddr; 4] = [
        IpAddr::V4(std::net::Ipv4Addr::new(223, 5, 5, 5)),
        IpAddr::V4(std::net::Ipv4Addr::new(223, 6, 6, 6)),
        IpAddr::V6(std::net::Ipv6Addr::new(0x2400, 0x3200, 0, 0, 0, 0, 0, 1)),
        IpAddr::V6(std::net::Ipv6Addr::new(
            0x2400, 0x3200, 0xbaba, 0, 0, 0, 0, 1,
        )),
    ];

    /// DNSPod Public DNS+ (`119.29.29.29` / `119.28.28.28` + `182.254.*`
    /// and the `2402:4e00::` anycast pair).
    pub const TENCENT_SERVERS: [IpAddr; 6] = [
        IpAddr::V4(std::net::Ipv4Addr::new(119, 29, 29, 29)),
        IpAddr::V4(std::net::Ipv4Addr::new(119, 28, 28, 28)),
        IpAddr::V4(std::net::Ipv4Addr::new(182, 254, 116, 116)),
        IpAddr::V4(std::net::Ipv4Addr::new(182, 254, 118, 118)),
        IpAddr::V6(std::net::Ipv6Addr::new(0x2402, 0x4e00, 0, 0, 0, 0, 0, 0)),
        IpAddr::V6(std::net::Ipv6Addr::new(0x2402, 0x4e00, 1, 0, 0, 0, 0, 0)),
    ];

    /// The servers this configuration queries — empty means "ask the OS".
    /// The presets are just well-known lists, so they share `Custom`'s code
    /// paths (validation, resolver building, the settings field).
    pub fn servers(&self) -> Vec<IpAddr> {
        match self {
            DnsConfig::System => Vec::new(),
            DnsConfig::Custom(servers) => servers.clone(),
            DnsConfig::Ali => Self::ALI_SERVERS.to_vec(),
            DnsConfig::Tencent => Self::TENCENT_SERVERS.to_vec(),
        }
    }

    /// Map a parsed server list back onto the preset it *is* — so editing
    /// the settings box back to AliDNS' exact list re-selects the preset,
    /// and `parse(to_text(x)) == x` keeps holding for every variant.
    pub fn from_servers(servers: Vec<IpAddr>) -> DnsConfig {
        if servers.is_empty() {
            return DnsConfig::System; // nothing to ask → the OS resolver
        }
        if servers == Self::ALI_SERVERS.to_vec() {
            DnsConfig::Ali
        } else if servers == Self::TENCENT_SERVERS.to_vec() {
            DnsConfig::Tencent
        } else {
            DnsConfig::Custom(servers)
        }
    }
    /// Parse the free-text DNS field: empty → system, otherwise a
    /// comma/space separated list of IP literals (`223.5.5.5, 8.8.8.8`).
    ///
    /// Hostnames are rejected on purpose: resolving the resolver itself
    /// through the system would make "custom DNS" mean "system DNS" on the
    /// first query, which is exactly what the user is trying to avoid.
    pub fn parse(text: &str) -> Result<DnsConfig, DnsSettingsError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(DnsConfig::System);
        }
        let mut servers = Vec::new();
        for part in trimmed.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let ip: IpAddr = part.parse().map_err(|_| {
                DnsSettingsError(format!(
                    "'{part}' is not an IP address (expected e.g. 223.5.5.5 or 2001:4860:4860::8888)"
                ))
            })?;
            if !servers.contains(&ip) {
                servers.push(ip);
            }
        }
        if servers.is_empty() {
            return Ok(DnsConfig::System);
        }
        Ok(DnsConfig::Custom(servers))
    }

    /// Human-readable form for the settings field (round-trips through
    /// [`DnsConfig::parse`]).
    pub fn to_text(&self) -> String {
        self.servers()
            .iter()
            .map(|ip| ip.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A built resolver: system or custom.
///
/// Built once per `enable()` inside the proxy runtime (hickory spawns its
/// queries on the ambient tokio runtime), then shared by every connection
/// task through an `Arc`.
pub struct DnsResolver {
    inner: Inner,
}

enum Inner {
    System,
    Custom(Box<Resolver<TokioRuntimeProvider>>),
}

impl DnsResolver {
    /// Build the resolver for `config`. Custom servers are contacted with
    /// two attempts / 5s timeout each — short enough that a dead server
    /// falls back to "cannot decide" instead of stalling a connection.
    pub fn build(config: &DnsConfig) -> Result<DnsResolver, DnsSettingsError> {
        match config {
            DnsConfig::System => Ok(DnsResolver {
                inner: Inner::System,
            }),
            _ => {
                let servers = config.servers();
                if servers.is_empty() {
                    return Err(DnsSettingsError(
                        "custom DNS selected but no server given".into(),
                    ));
                }
                let nameservers = servers
                    .iter()
                    .map(|ip| {
                        // UDP first, TCP as the truncation fallback — the
                        // same shape as hickory's own `udp()` helper plus
                        // the fallback it leaves out.
                        NameServerConfig::new(
                            *ip,
                            true,
                            vec![ConnectionConfig::udp(), ConnectionConfig::tcp()],
                        )
                    })
                    .collect();
                let mut opts = ResolverOpts::default();
                opts.timeout = std::time::Duration::from_secs(5);
                opts.attempts = 2;
                let resolver = Resolver::builder_with_config(
                    ResolverConfig::from_name_servers(nameservers),
                    TokioRuntimeProvider::default(),
                )
                .with_options(opts)
                .build()
                .map_err(|e| DnsSettingsError(format!("could not build resolver: {e}")))?;
                Ok(DnsResolver {
                    inner: Inner::Custom(Box::new(resolver)),
                })
            }
        }
    }

    /// Resolve `host` to every A/AAAA address. Never fails: an empty vector
    /// means "unknown", which the router treats as "cannot prove bypass →
    /// go through the proxy" (fail-safe for reachability).
    pub async fn resolve(&self, host: &str) -> Vec<IpAddr> {
        if host.is_empty() {
            return Vec::new();
        }
        // An IP literal needs no query at all (and getaddrinfo would not
        // consult the custom server for it either).
        if let Ok(ip) = host.parse::<IpAddr>() {
            return vec![ip];
        }
        match &self.inner {
            Inner::System => tokio::net::lookup_host((host, 0u16))
                .await
                .map(|addrs| {
                    let mut seen: Vec<IpAddr> = Vec::new();
                    for addr in addrs {
                        let ip = addr.ip();
                        if !seen.contains(&ip) {
                            seen.push(ip);
                        }
                    }
                    seen
                })
                .unwrap_or_default(),
            Inner::Custom(resolver) => resolver
                .lookup_ip(host)
                .await
                .map(|lookup| {
                    let mut seen: Vec<IpAddr> = Vec::new();
                    for ip in lookup.iter() {
                        if !seen.contains(&ip) {
                            seen.push(ip);
                        }
                    }
                    seen
                })
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn empty_text_means_system() {
        assert_eq!(DnsConfig::parse("").unwrap(), DnsConfig::System);
        assert_eq!(DnsConfig::parse("   ").unwrap(), DnsConfig::System);
        assert_eq!(DnsConfig::System.to_text(), "");
    }

    #[test]
    fn parses_list_and_dedupes() {
        let cfg = DnsConfig::parse("223.5.5.5, 8.8.8.8 ;223.5.5.5").unwrap();
        assert_eq!(
            cfg,
            DnsConfig::Custom(vec![
                IpAddr::V4(Ipv4Addr::new(223, 5, 5, 5)),
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            ])
        );
        assert_eq!(cfg.to_text(), "223.5.5.5, 8.8.8.8");
        // Round-trip through the same parser the settings field uses.
        assert_eq!(DnsConfig::parse(&cfg.to_text()).unwrap(), cfg);
    }

    #[test]
    fn accepts_ipv6_literals() {
        let cfg = DnsConfig::parse("2001:4860:4860::8888").unwrap();
        assert_eq!(
            cfg,
            DnsConfig::Custom(vec![IpAddr::V6(Ipv6Addr::new(
                0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
            ))])
        );
    }

    #[test]
    fn rejects_hostnames_and_junk() {
        for bad in ["dns.example.com", "8.8.8.8:53", "1.2.3.4/8", "not an ip"] {
            let err = DnsConfig::parse(bad).unwrap_err();
            assert!(err.0.contains("not an IP address"), "{bad} → {err}");
        }
    }

    #[tokio::test]
    async fn literal_addresses_resolve_without_a_query() {
        let resolver = DnsResolver::build(&DnsConfig::System).unwrap();
        assert_eq!(
            resolver.resolve("127.0.0.1").await,
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        );
        assert!(resolver.resolve("").await.is_empty());
    }

    #[tokio::test]
    #[ignore = "waits for the 5s x 2 query timeout against a blackholed server"]
    async fn custom_resolver_reports_empty_on_unreachable_server() {
        // 198.51.100.1 is TEST-NET-2: nothing answers, so the query times
        // out and yields "unknown" rather than an error/panic.
        let cfg = DnsConfig::Custom(vec![IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))]);
        let resolver = DnsResolver::build(&cfg).unwrap();
        let got = resolver.resolve("example.com").await;
        assert!(got.is_empty(), "expected no answer, got {got:?}");
    }

    #[test]
    fn presets_hold_the_published_servers() {
        // AliDNS: 223.5.5.5 / 223.6.6.6 + 2400:3200::1 / 2400:3200:baba::1
        assert_eq!(
            DnsConfig::Ali.to_text(),
            "223.5.5.5, 223.6.6.6, 2400:3200::1, 2400:3200:baba::1"
        );
        // DNSPod Public DNS+: 119.29.29.29 / 119.28.28.28 / 182.254.116.116 /
        // 182.254.118.118 + 2402:4e00:: / 2402:4e00:1::
        assert_eq!(
            DnsConfig::Tencent.to_text(),
            "119.29.29.29, 119.28.28.28, 182.254.116.116, 182.254.118.118, 2402:4e00::, 2402:4e00:1::"
        );
        assert_eq!(DnsConfig::Ali.servers().len(), 4);
        assert_eq!(DnsConfig::Tencent.servers().len(), 6);
        // A preset is a first-class configuration, never "empty servers".
        assert!(!DnsConfig::Ali.servers().is_empty());
        assert!(!DnsConfig::Tencent.servers().is_empty());
    }

    #[test]
    fn presets_round_trip_through_the_settings_field() {
        for cfg in [
            DnsConfig::System,
            DnsConfig::Ali,
            DnsConfig::Tencent,
            DnsConfig::parse("1.1.1.1, 9.9.9.9").unwrap(),
        ] {
            let text = cfg.to_text();
            let back = DnsConfig::from_servers(DnsConfig::parse(&text).unwrap().servers());
            assert_eq!(back, cfg, "round trip of {text}");
        }
        // The box keeps an edited list in `Custom`, but an exact preset list
        // snaps back onto the preset.
        let ali: Vec<IpAddr> = DnsConfig::parse(&DnsConfig::Ali.to_text())
            .unwrap()
            .servers();
        assert_eq!(DnsConfig::from_servers(ali), DnsConfig::Ali);
        let mut edited = DnsConfig::Ali.servers();
        edited[0] = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        assert_eq!(
            DnsConfig::from_servers(edited),
            DnsConfig::Custom(vec![
                IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
                IpAddr::V4(Ipv4Addr::new(223, 6, 6, 6)),
                IpAddr::V6(std::net::Ipv6Addr::new(0x2400, 0x3200, 0, 0, 0, 0, 0, 1)),
                IpAddr::V6(std::net::Ipv6Addr::new(
                    0x2400, 0x3200, 0xbaba, 0, 0, 0, 0, 1
                )),
            ])
        );
    }

    #[test]
    fn presets_survive_settings_json() {
        for cfg in [DnsConfig::Ali, DnsConfig::Tencent] {
            let json = serde_json::to_string(&cfg).unwrap();
            let back: DnsConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(back, cfg, "{json}");
        }
        // Old files still load: {"mode":"system"} / {"mode":"custom",...}.
        assert_eq!(
            serde_json::from_str::<DnsConfig>(r#"{"mode":"system"}"#).unwrap(),
            DnsConfig::System
        );
        assert_eq!(
            serde_json::from_str::<DnsConfig>(r#"{"mode":"custom","servers":["1.1.1.1"]}"#)
                .unwrap(),
            DnsConfig::Custom(vec![IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))])
        );
    }

    #[tokio::test]
    #[ignore = "needs the real internet: proves the presets answer queries"]
    async fn preset_resolvers_answer_on_the_real_internet() {
        for cfg in [DnsConfig::Ali, DnsConfig::Tencent] {
            let resolver = DnsResolver::build(&cfg).unwrap();
            let got = resolver.resolve("www.baidu.com").await;
            assert!(!got.is_empty(), "{cfg:?} must answer, got {got:?}");
            assert!(got.iter().any(|ip| !ip.is_loopback()), "{got:?}");
            println!("{cfg:?} → {got:?}");
        }
    }

    #[test]
    fn preset_resolvers_build() {
        for cfg in [DnsConfig::Ali, DnsConfig::Tencent] {
            assert!(DnsResolver::build(&cfg).is_ok(), "{cfg:?} must build");
        }
        assert!(DnsResolver::build(&DnsConfig::Custom(Vec::new())).is_err());
    }
}
