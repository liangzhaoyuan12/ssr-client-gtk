//! CIDR primitives: parse, match and the two built-in address sets.
//!
//! Everything here is pure and dependency-free so routing decisions can be
//! unit-tested without a network. The mainland-China set is vendored data
//! (`china_cidrs.txt`, see the file header for its source); the LAN set is
//! the usual RFC 1918 / loopback / link-local / special-purpose space.

use std::net::IpAddr;
use std::sync::OnceLock;

/// One IPv4 or IPv6 network in prefix form.
///
/// Addresses are kept in their natural integer width (`u32` values live in
/// the low bits of a `u128`), so a match is two shifts and an XOR — no
/// allocation, no parsing at decision time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    /// Network address (low 32 bits for IPv4, full width for IPv6).
    net: u128,
    /// Prefix length: 0–32 for IPv4, 0–128 for IPv6.
    prefix: u8,
    /// `true` for IPv6 networks (IPv4 never matches them and vice versa).
    v6: bool,
}

impl Cidr {
    /// Parse `"10.0.0.0/8"`, `"192.168.1.1"` (implicit `/32`), `"::1/128"`.
    ///
    /// Returns `None` on anything malformed — callers treat that as a
    /// configuration error, never as "match nothing silently".
    pub fn parse(s: &str) -> Option<Cidr> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (s, None),
        };
        let ip: IpAddr = addr.parse().ok()?;
        match ip {
            IpAddr::V4(v4) => {
                let prefix = match prefix {
                    None => 32,
                    Some(p) => p.parse::<u8>().ok()?,
                };
                if prefix > 32 {
                    return None;
                }
                Some(Cidr {
                    net: u128::from(u32::from(v4)),
                    prefix,
                    v6: false,
                })
            }
            IpAddr::V6(v6) => {
                let prefix = match prefix {
                    None => 128,
                    Some(p) => p.parse::<u8>().ok()?,
                };
                if prefix > 128 {
                    return None;
                }
                Some(Cidr {
                    net: u128::from(v6),
                    prefix,
                    v6: true,
                })
            }
        }
    }

    /// `true` when `ip` falls inside this network (family must match).
    ///
    /// Both the address and the network are low-aligned in their natural
    /// width (32 or 128 bits), so discarding the host bits with a right
    /// shift and comparing the remainders is exactly "do the first
    /// `prefix` bits agree?" — no masking, no sign issues.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let (value, width) = match ip {
            IpAddr::V4(v4) if !self.v6 => (u128::from(u32::from(v4)), 32u32),
            IpAddr::V6(v6) if self.v6 => (u128::from(v6), 128u32),
            _ => return false,
        };
        if self.prefix == 0 {
            return true;
        }
        let shift = width - u32::from(self.prefix);
        (value >> shift) == (self.net >> shift)
    }

    /// Prefix length (test helper / diagnostics).
    #[cfg(test)]
    pub fn prefix(self) -> u8 {
        self.prefix
    }
}

/// A set of networks, split by address family and scanned linearly.
///
/// The China list has a few thousand IPv4 entries and a decision happens
/// once per connection (never per packet), so a linear scan is well inside
/// the noise; keeping it allocation-free beats a fancier index here.
#[derive(Debug, Clone, Default)]
pub struct CidrSet {
    v4: Vec<Cidr>,
    v6: Vec<Cidr>,
}

impl CidrSet {
    /// Parse newline- and comma-separated networks; invalid lines are an
    /// error so a corrupt data file can never silently disable bypassing.
    pub fn parse(text: &str) -> Result<CidrSet, String> {
        let mut set = CidrSet::default();
        for (idx, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let cidr = Cidr::parse(line).ok_or_else(|| format!("line {}: {:?}", idx + 1, line))?;
            set.push(cidr);
        }
        Ok(set)
    }

    /// Add one network to the right family bucket.
    pub fn push(&mut self, cidr: Cidr) {
        if cidr.v6 {
            self.v6.push(cidr);
        } else {
            self.v4.push(cidr);
        }
    }

    /// `true` when any network in the set contains `ip`.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let bucket = if ip.is_ipv4() { &self.v4 } else { &self.v6 };
        bucket.iter().any(|c| c.contains(ip))
    }

    /// Number of networks (both families).
    pub fn len(&self) -> usize {
        self.v4.len() + self.v6.len()
    }

    /// `true` when the set holds no networks.
    pub fn is_empty(&self) -> bool {
        self.v4.is_empty() && self.v6.is_empty()
    }
}

/// LAN / non-routable space — the "绕开局域网" set.
///
/// RFC 1918 + loopback + link-local + CGNAT + multicast/reserved plus the
/// IPv6 ULA/link-local/multicast equivalents. Special-purpose ranges
/// (TEST-NET etc.) are included: sending them through a proxy can never
/// work, so bypassing them is strictly safer.
pub fn lan_set() -> &'static CidrSet {
    static LAN: OnceLock<CidrSet> = OnceLock::new();
    LAN.get_or_init(|| {
        const LAN_V4: &[&str] = &[
            "0.0.0.0/8",
            "10.0.0.0/8",
            "100.64.0.0/10",
            "127.0.0.0/8",
            "169.254.0.0/16",
            "172.16.0.0/12",
            "192.0.0.0/24",
            "192.0.2.0/24",
            "192.168.0.0/16",
            "198.18.0.0/15",
            "198.51.100.0/24",
            "203.0.113.0/24",
            "224.0.0.0/4",
            "240.0.0.0/4",
        ];
        const LAN_V6: &[&str] = &[
            "::/128",
            "::1/128",
            "64:ff9b::/96",
            "fc00::/7",
            "fe80::/10",
            "ff00::/8",
        ];
        let mut set = CidrSet::default();
        for line in LAN_V4.iter().chain(LAN_V6.iter()) {
            match Cidr::parse(line) {
                Some(cidr) => set.push(cidr),
                None => panic!("built-in LAN CIDR {line} is malformed"),
            }
        }
        set
    })
}

/// Mainland-China IPv4/IPv6 space — the "绕开中国大陆" set.
///
/// Data file: `china_cidrs.txt` (vendored, source and checksum recorded in
/// its header comment). Parsed once per process; a parse failure panics
/// rather than silently routing Chinese traffic through the proxy.
pub fn cn_set() -> &'static CidrSet {
    static CN: OnceLock<CidrSet> = OnceLock::new();
    CN.get_or_init(|| {
        const CN_DATA: &str = include_str!("china_cidrs.txt");
        CidrSet::parse(CN_DATA).expect("vendored china_cidrs.txt must parse")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parses_v4_with_and_without_prefix() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert_eq!(c.prefix(), 8);
        assert!(c.contains(ip("10.1.2.3")));
        assert!(!c.contains(ip("11.0.0.1")));

        let host = Cidr::parse("192.168.1.5").unwrap();
        assert_eq!(host.prefix(), 32);
        assert!(host.contains(ip("192.168.1.5")));
        assert!(!host.contains(ip("192.168.1.6")));
    }

    #[test]
    fn parses_v6_and_rejects_cross_family() {
        let ula = Cidr::parse("fd00::/8").unwrap();
        assert!(ula.contains(ip("fd12:3456::1")));
        assert!(!ula.contains(ip("fe80::1")));
        assert!(
            !ula.contains(ip("10.0.0.1")),
            "IPv4 must not match a v6 net"
        );
        assert!(!lan_set().contains(ip("8.8.8.8")));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_silent_skip() {
        assert!(Cidr::parse("not-an-ip/8").is_none());
        assert!(Cidr::parse("10.0.0.0/33").is_none());
        assert!(Cidr::parse("::/129").is_none());
        let err = CidrSet::parse("10.0.0.0/8\nbogus\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn lan_set_covers_the_expected_space() {
        let lan = lan_set();
        for hit in [
            "10.1.2.3",
            "172.16.5.5",
            "172.31.255.254",
            "192.168.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(lan.contains(ip(hit)), "{hit} must be LAN");
        }
        for miss in ["8.8.8.8", "172.32.0.1", "1.1.1.1", "2001:4860:4860::8888"] {
            assert!(!lan.contains(ip(miss)), "{miss} must not be LAN");
        }
        // Boundary: 172.16/12 ends at 172.31.255.255.
        assert!(lan.contains(ip("172.31.255.255")));
        assert!(!lan.contains(ip("172.32.0.0")));
    }

    #[test]
    fn china_set_is_non_empty_and_sane() {
        let cn = cn_set();
        assert!(
            cn.len() > 1000,
            "expected a real China list, got {}",
            cn.len()
        );
        // Well-known Chinese blocks must be in; public resolvers must not.
        for hit in ["223.5.5.5", "114.114.114.114", "1.2.4.0"] {
            assert!(cn.contains(ip(hit)), "{hit} should be a China address");
        }
        for miss in ["8.8.8.8", "1.1.1.1", "10.0.0.1", "172.16.0.1"] {
            assert!(
                !cn.contains(ip(miss)),
                "{miss} must not be classified as China"
            );
        }
        // The IPv6 half of the same list (china6.txt) — "绕开中国大陆"
        // must hold for v6 targets too.
        for hit in ["2400:3200::1", "2408:8000::1", "240e::1"] {
            assert!(cn.contains(ip(hit)), "{hit} should be a China address");
        }
        for miss in ["2001:4860:4860::8888", "2606:4700:4700::1111", "fe80::1"] {
            assert!(
                !cn.contains(ip(miss)),
                "{miss} must not be classified as China"
            );
        }
        assert!(!cn.is_empty());
    }

    #[test]
    fn explicit_loops_over_real_types() {
        // Keep the constructors honest for the exact families used in prod.
        let v4: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();
        let v6: IpAddr = Ipv6Addr::LOCALHOST.into();
        assert!(lan_set().contains(v4));
        assert!(lan_set().contains(v6));
    }
}
