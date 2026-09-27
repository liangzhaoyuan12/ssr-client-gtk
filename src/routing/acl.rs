//! Custom ACL file — the shadowsocks-rust (`ss-rust`) file format.
//!
//! The format is taken verbatim from
//! `crates/shadowsocks-service/src/acl/mod.rs` of
//! <https://github.com/shadowsocks/shadowsocks-rust> so that users can drop
//! an existing `.acl` in unchanged:
//!
//! ```text
//! # comment (also accepted: ! comment — ss-rust treats it as an inert rule)
//! [proxy_all]          # or [bypass_all] — default action for unmatched targets
//! [bypass_list]        # or [black_list] — targets that connect directly
//! 10.0.0.0/8
//! |exact.example.com    # | = this domain only
//! [proxy_list]         # or [white_list] — targets that go through the proxy
//! ||example.com         # || = this domain and all subdomains
//! (^|\.)gmail\.com$     # anything else is a regular expression on the host
//! ```
//!
//! Semantics mirrored from ss-rust (not re-invented):
//! - host rules: `[proxy_list]` wins over `[bypass_list]`;
//! - IP rules: `[bypass_list]` wins over `[proxy_list]`;
//! - unmatched target: `[proxy_all]`/`[accept_all]` → proxy,
//!   `[bypass_all]`/`[reject_all]` → direct, no mode header → proxy;
//! - a domain with no host rule is resolved locally **only** when the
//!   relevant list actually contains IP rules, then any matching answer
//!   decides (ss-rust `check_target_bypassed`).
//!
//! Deliberate differences (both fail *louder* than ss-rust):
//! - a line containing `/` that is not a valid CIDR is an **error**
//!   (ss-rust silently turns it into a regex that never matches);
//! - an invalid regular expression is an **error** with its line number.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

use regex::RegexBuilder;

use crate::error::{AppError, AppResult};
use crate::routing::Route;
use crate::routing::cidr::{Cidr, CidrSet};

/// Default action for targets no rule mentions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `[proxy_all]` / `[accept_all]` — unmatched targets are proxied.
    BlackList,
    /// `[bypass_all]` / `[reject_all]` — unmatched targets go direct.
    WhiteList,
}

/// One side of the ACL: IP rules plus host rules.
#[derive(Debug, Default)]
struct Rules {
    ips: CidrSet,
    /// `|domain` — exact host.
    exact: HashSet<String>,
    /// `||domain` — domain and every subdomain.
    suffix: HashSet<String>,
    /// Anything else, compiled as a regex against the lowercase host.
    regexes: Vec<regex::Regex>,
}

impl Rules {
    fn is_ip_empty(&self) -> bool {
        self.ips.is_empty()
    }

    fn matches_ip(&self, ip: IpAddr) -> bool {
        self.ips.contains(ip)
    }

    fn matches_host(&self, host: &str) -> bool {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        if self.exact.contains(&host) {
            return true;
        }
        // `||domain` → walk the host's own suffixes ("a.b.c", "b.c", "c"):
        // O(labels) lookups instead of scanning every rule in the file.
        let mut start = 0usize;
        loop {
            if self.suffix.contains(&host[start..]) {
                return true;
            }
            match host[start..].find('.') {
                Some(dot) => start += dot + 1,
                None => break,
            }
        }
        self.regexes.iter().any(|re| re.is_match(&host))
    }
}

/// A parsed ACL file.
#[derive(Debug)]
pub struct Acl {
    mode: Mode,
    bypass: Rules,
    proxy: Rules,
}

impl Acl {
    /// Read and parse `path`.
    pub fn load(path: &Path) -> AppResult<Acl> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| AppError::Acl(format!("{}: {e}", path.display())))?;
        Acl::parse(&text).map_err(AppError::Acl)
    }

    /// Parse ACL text (exposed for tests and for validating before save).
    pub fn parse(text: &str) -> Result<Acl, String> {
        let mut mode = Mode::BlackList;
        let mut bypass = Rules::default();
        let mut proxy = Rules::default();
        // `[outbound_*]` sections are server-side only: their rules must not
        // leak into the client's bypass/proxy lists.
        let mut outbound = Rules::default();
        let mut outbound_active = false;
        let mut section = "before any section header";
        for (idx, raw) in text.lines().enumerate() {
            let lineno = idx + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            // ss-rust skips non-ASCII lines too (IDN must be punycoded).
            if !line.is_ascii() {
                continue;
            }

            match line {
                "[reject_all]" | "[bypass_all]" => {
                    mode = Mode::WhiteList;
                    section = line;
                    continue;
                }
                "[accept_all]" | "[proxy_all]" => {
                    mode = Mode::BlackList;
                    section = line;
                    continue;
                }
                "[black_list]" | "[bypass_list]" => {
                    outbound_active = false;
                    section = line;
                    continue;
                }
                "[white_list]" | "[proxy_list]" => {
                    outbound_active = false;
                    section = line;
                    continue;
                }
                "[outbound_block_list]" | "[outbound_allow_list]" => {
                    outbound_active = true;
                    section = line;
                    continue;
                }
                _ => {}
            }
            if line.starts_with('[') && line.ends_with(']') {
                // Unknown section (server-side names such as
                // `[white_list]` synonyms are handled above): ignore its
                // rules instead of filing them under the wrong list.
                outbound_active = true;
                section = line;
                continue;
            }

            let rules = if outbound_active {
                &mut outbound
            } else if is_proxy_section(section) {
                &mut proxy
            } else {
                &mut bypass
            };
            add_rule(rules, line).map_err(|e| format!("line {lineno}: {e}"))?;
        }

        let acl = Acl {
            mode,
            bypass,
            proxy,
        };
        let (bypassed, proxied) = acl.counts();
        log::debug!("acl loaded: {bypassed} bypass rules, {proxied} proxy rules");
        Ok(acl)
    }

    /// Host rule lookup: `Some(Proxy)` / `Some(Direct)` when a domain rule
    /// matched, `None` when the decision needs the target's IPs.
    pub fn decide_host(&self, host: &str) -> Option<Route> {
        if self.proxy.matches_host(host) {
            return Some(Route::Proxy);
        }
        if self.bypass.matches_host(host) {
            return Some(Route::Direct);
        }
        None
    }

    /// IP rule lookup (bypass list wins, then proxy list, then the mode
    /// default) — same order as ss-rust `check_ip_in_proxy_list`.
    pub fn decide_ip(&self, ip: IpAddr) -> Route {
        if self.bypass.matches_ip(ip) {
            return Route::Direct;
        }
        if self.proxy.matches_ip(ip) {
            return Route::Proxy;
        }
        self.default_route()
    }

    /// Whether a host without a domain rule is worth resolving at all
    /// (ss-rust skips DNS when the relevant list has no IP rules).
    pub fn needs_ip_lookup(&self) -> bool {
        match self.mode {
            Mode::BlackList => !self.bypass.is_ip_empty(),
            Mode::WhiteList => !self.proxy.is_ip_empty(),
        }
    }

    /// Decide from a set of resolved addresses: **any** answer matching the
    /// relevant list decides, otherwise the mode default (ss-rust
    /// `check_target_bypassed`).
    pub fn decide_ips(&self, ips: &[IpAddr]) -> Route {
        if ips.is_empty() {
            return self.default_route();
        }
        match self.mode {
            Mode::BlackList => {
                if self.bypass.is_ip_empty() {
                    self.default_route()
                } else if ips.iter().any(|ip| self.bypass.matches_ip(*ip)) {
                    Route::Direct
                } else {
                    self.default_route()
                }
            }
            Mode::WhiteList => {
                if self.proxy.is_ip_empty() {
                    self.default_route()
                } else if ips.iter().any(|ip| self.proxy.matches_ip(*ip)) {
                    Route::Proxy
                } else {
                    self.default_route()
                }
            }
        }
    }

    /// Action for a target no rule mentions (`[proxy_all]` → proxy,
    /// `[bypass_all]` → direct, no mode header → proxy).
    pub fn default_route(&self) -> Route {
        match self.mode {
            Mode::BlackList => Route::Proxy,
            Mode::WhiteList => Route::Direct,
        }
    }

    /// Number of rules on each side (diagnostics / tests).
    pub fn counts(&self) -> (usize, usize) {
        (
            self.bypass.ips.len() + self.bypass.exact.len() + self.bypass.suffix.len(),
            self.proxy.ips.len() + self.proxy.exact.len() + self.proxy.suffix.len(),
        )
    }
}

/// Section currently being filled decides which list a rule lands in.
fn is_proxy_section(section: &str) -> bool {
    matches!(section, "[white_list]" | "[proxy_list]")
}

fn add_rule(rules: &mut Rules, line: &str) -> Result<(), String> {
    if let Some(domain) = line.strip_prefix("||") {
        let domain = normalize_domain(domain)?;
        rules.suffix.insert(domain);
        return Ok(());
    }
    if let Some(domain) = line.strip_prefix('|') {
        let domain = normalize_domain(domain)?;
        rules.exact.insert(domain);
        return Ok(());
    }
    // A line that looks like a network must *be* one — never silently turn
    // a typo (`10.0.0.0/33`) into a rule that matches nothing.
    if line.contains('/') {
        let cidr = Cidr::parse(line).ok_or_else(|| format!("invalid CIDR `{line}`"))?;
        rules.ips.push(cidr);
        return Ok(());
    }
    if let Ok(ip) = line.parse::<IpAddr>() {
        let cidr = Cidr::parse(line).ok_or_else(|| format!("invalid address `{line}`"))?;
        debug_assert!(cidr.contains(ip));
        rules.ips.push(cidr);
        return Ok(());
    }
    let re = RegexBuilder::new(line)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("invalid regex `{line}`: {e}"))?;
    rules.regexes.push(re);
    Ok(())
}

fn normalize_domain(domain: &str) -> Result<String, String> {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return Err("empty domain rule".into());
    }
    if !domain.is_ascii() {
        return Err(format!("non-ASCII domain `{domain}` (use punycode)"));
    }
    Ok(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blacklist_mode_proxies_by_default_and_bypasses_listed_targets() {
        let acl = Acl::parse(
            "[proxy_all]\n\
             [bypass_list]\n\
             10.0.0.0/8\n\
             192.168.1.1\n\
             |router.example\n\
             ||lan.example\n",
        )
        .unwrap();

        assert_eq!(acl.decide_ip(ip("10.1.2.3")), Route::Direct);
        assert_eq!(acl.decide_ip(ip("192.168.1.1")), Route::Direct);
        assert_eq!(acl.decide_ip(ip("8.8.8.8")), Route::Proxy);
        assert_eq!(acl.decide_host("router.example"), Some(Route::Direct));
        assert_eq!(
            acl.decide_host("a.lan.example"),
            Some(Route::Direct),
            "|| matches subdomains"
        );
        assert_eq!(
            acl.decide_host("notlan.example"),
            None,
            "no domain rule → fall through to IP lookup"
        );
        assert_eq!(acl.default_route(), Route::Proxy);
        assert!(acl.needs_ip_lookup(), "bypass list has IP rules");
    }

    #[test]
    fn whitelist_mode_directs_by_default_and_proxies_listed_targets() {
        let acl = Acl::parse(
            "[bypass_all]\n\
             [proxy_list]\n\
             ||blocked.example\n\
             203.0.113.0/24\n",
        )
        .unwrap();

        assert_eq!(acl.decide_host("x.blocked.example"), Some(Route::Proxy));
        assert_eq!(acl.decide_host("example.com"), None);
        assert_eq!(acl.decide_ip(ip("203.0.113.7")), Route::Proxy);
        assert_eq!(acl.decide_ip(ip("1.2.3.4")), Route::Direct);
        assert_eq!(acl.default_route(), Route::Direct);
        assert!(acl.needs_ip_lookup(), "proxy list has IP rules");
    }

    #[test]
    fn host_rules_win_in_proxy_list_and_ip_rules_prefer_bypass() {
        let acl = Acl::parse(
            "[proxy_all]\n\
             [bypass_list]\n\
             ||example.com\n\
             [proxy_list]\n\
             ||www.example.com\n",
        )
        .unwrap();
        // Domain lookup: proxy list first.
        assert_eq!(acl.decide_host("www.example.com"), Some(Route::Proxy));
        assert_eq!(acl.decide_host("mail.example.com"), Some(Route::Direct));
        // IP lookup: bypass list first (the mirror of ss-rust's order).
        assert_eq!(acl.decide_ip(ip("192.0.2.1")), Route::Proxy);
    }

    #[test]
    fn exact_and_subdomain_rules_do_not_leak() {
        let acl = Acl::parse("[proxy_list]\n|example.com\n").unwrap();
        assert_eq!(acl.decide_host("example.com"), Some(Route::Proxy));
        assert_eq!(acl.decide_host("EXAMPLE.COM."), Some(Route::Proxy));
        assert_eq!(
            acl.decide_host("sub.example.com"),
            None,
            "| is exact-match only"
        );
        assert_eq!(acl.decide_host("notexample.com"), None);
    }

    #[test]
    fn regex_rules_are_matched_against_the_host() {
        let acl = Acl::parse("[proxy_list]\n(^|\\.)gmail\\.com$\n").unwrap();
        assert_eq!(acl.decide_host("gmail.com"), Some(Route::Proxy));
        assert_eq!(acl.decide_host("mail.gmail.com"), Some(Route::Proxy));
        assert_eq!(acl.decide_host("googlemail.com"), None);
        assert_eq!(acl.decide_host("gmail.com.evil.net"), None);
    }

    #[test]
    fn comments_and_outbound_sections_are_handled() {
        let acl = Acl::parse(
            "# a comment\n\
             ! another comment style\n\
             \n\
             [bypass_list]\n\
             10.0.0.0/8\n\
             [outbound_block_list]\n\
             1.1.1.1\n\
             [proxy_list]\n\
             ||go.example\n",
        )
        .unwrap();
        assert_eq!(acl.decide_ip(ip("10.0.0.1")), Route::Direct);
        assert_eq!(
            acl.decide_ip(ip("1.1.1.1")),
            Route::Proxy,
            "server-side outbound rules must not become bypass rules"
        );
        assert_eq!(acl.decide_host("go.example"), Some(Route::Proxy));
    }

    #[test]
    fn malformed_cidr_and_regex_are_errors_with_a_line_number() {
        let err = Acl::parse("[bypass_list]\n10.0.0.0/33\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
        assert!(err.contains("invalid CIDR"), "{err}");

        let err = Acl::parse("[bypass_list]\n([broken\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
        assert!(err.contains("invalid regex"), "{err}");

        let err = Acl::parse("[bypass_list]\n||\n").unwrap_err();
        assert!(err.contains("empty domain"), "{err}");
    }

    #[test]
    fn ip_lookup_is_skipped_when_the_relevant_list_has_no_ips() {
        let acl = Acl::parse("[bypass_all]\n||only.example\n").unwrap();
        assert!(!acl.needs_ip_lookup(), "proxy list has no IP rules");
        // …so decide_ips must return the default, not "proxy".
        assert_eq!(acl.decide_ips(&[ip("8.8.8.8")]), Route::Direct);
    }

    #[test]
    fn decide_ips_any_match_decides() {
        let acl = Acl::parse("[proxy_all]\n[bypass_list]\n223.5.5.0/24\n").unwrap();
        assert_eq!(
            acl.decide_ips(&[ip("8.8.8.8"), ip("223.5.5.5")]),
            Route::Direct
        );
        assert_eq!(acl.decide_ips(&[ip("8.8.8.8")]), Route::Proxy);
        assert_eq!(
            acl.decide_ips(&[]),
            Route::Proxy,
            "empty answer falls back to the mode default"
        );
    }

    #[test]
    fn missing_file_is_an_acl_error() {
        let err = Acl::load(Path::new("/nonexistent/nope.acl")).unwrap_err();
        assert!(matches!(err, AppError::Acl(_)), "{err:?}");
    }
}
