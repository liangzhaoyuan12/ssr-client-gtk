//! Runtime services (proxy lifecycle).

pub(crate) mod http_proxy;
pub mod proxy;
pub(crate) mod socks5;

/// Test-only helpers shared by the modules that open listeners.
///
/// Two hazards are solved here instead of per test module:
///
/// 1. **Port reuse races.** `free_port()` binds `:0`, notes the number and
///    releases it — any *other* test running at the same time can be handed
///    the same number by the kernel (all suites calling `bind(0)` pull from
///    one ephemeral pool), which surfaces as a flaky `PortInUse` / bind
///    failure. The candidates therefore come from `20000..30000`, a range
///    *below* the ephemeral floor on every target we ship on (Linux
///    32768+, Windows/macOS 49152+), so `bind(0)` in a neighbouring test
///    can never collide.
/// 2. **Cross-suite overlap.** Two tests that each pick a *free* number and
///    then release it can still pick the *same* one; every test that takes a
///    non-ephemeral port therefore holds [`port_lock()`] first.
#[cfg(test)]
pub(crate) mod test_sync {
    use std::net::TcpListener;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static PORT: Mutex<()> = Mutex::new(());

    /// Hold across "pick a port → bind it → use it".
    pub(crate) fn port_lock() -> MutexGuard<'static, ()> {
        PORT.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A port nothing listens on right now, drawn from `20000..30000`
    /// (below every target's ephemeral range — see the module docs).
    pub(crate) fn free_port() -> u16 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as usize;
        for i in 0usize..4000 {
            let candidate = 20_000 + ((nanos.wrapping_add(i.wrapping_mul(7_919))) % 10_000) as u16;
            if let Ok(sock) = TcpListener::bind(("0.0.0.0", candidate)) {
                let port = sock.local_addr().expect("local_addr").port();
                drop(sock);
                return port;
            }
        }
        panic!("no free port in 20000..30000");
    }
}
