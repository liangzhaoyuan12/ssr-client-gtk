//! Proxy lifecycle: the routing-aware SOCKS5 front-end plus the
//! `ssr-client-rs` core (Mode B sessions).
//!
//! Design notes (GOAL §5 / §6):
//! - **Single listener (N1)**: the only socket this app ever *listens* on is
//!   the front-end TCP socket on `listen_port`; when the profile has UDP
//!   enabled the core's `UdpRelay` binds the **same port number** on UDP
//!   (one port number, two protocols — exactly what Mode A did).
//!   `probe_port`/`probe_udp` bind and immediately drop their sockets, so
//!   they can never become a second listener.
//! - `listen_address` is forced to `0.0.0.0` (GOAL §6.3 / D2).
//! - A multi-thread tokio runtime lives inside the service; everything
//!   blocking happens on the caller's worker thread, never in a GTK
//!   signal handler.
//! - Lifecycle facts are pushed to `events` so the UI can reconcile even
//!   when the front-end dies on its own (ARCHITECTURE §8-5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::config::model::ShadowsocksConfig;
use crate::core::socks5::{self, Frontend};
use crate::error::{AppError, AppResult};
use crate::routing::{Router, RoutingSettings};

/// Lifecycle notifications pushed to subscribers (the UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyEvent {
    /// Proxy confirmed running on `port` under config `cfg_name`.
    Started {
        /// Name of the config that was enabled.
        cfg_name: String,
        /// Local SOCKS5 port (the only exposed port).
        port: u16,
    },
    /// Proxy is no longer running.
    Stopped {
        /// `None` on a deliberate stop, `Some(reason)` on a real failure.
        reason: Option<String>,
    },
}

/// Snapshot of the running proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyState {
    /// Config currently serving.
    pub cfg_name: String,
    /// Local SOCKS5 port.
    pub port: u16,
}

struct Running {
    state: ProxyState,
    /// Flipped to stop the accept loop and the UDP relay.
    shutdown: watch::Sender<bool>,
    /// Cleared by the accept loop when it exits (crash reconciliation).
    alive: Arc<AtomicBool>,
    /// Whether the UDP relay owns the port number on UDP.
    udp: bool,
}

/// Owns the tokio runtime and the (at most one) running front-end.
pub struct ProxyService {
    runtime: Runtime,
    running: Mutex<Option<Running>>,
    events: async_channel::Sender<ProxyEvent>,
}

impl ProxyService {
    /// Build the service and the event channel subscribers should use.
    pub fn new(events: async_channel::Sender<ProxyEvent>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            runtime,
            running: Mutex::new(None),
            events,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Probe whether `port` can be bound on 0.0.0.0, retrying for `wait`
    /// (sockets released by a just-stopped listener linger for a moment).
    ///
    /// The probe listener is dropped immediately — it never becomes a live
    /// second listener (GOAL §6 N1).
    fn probe_port(port: u16, wait: Duration) -> AppResult<()> {
        let deadline = Instant::now() + wait;
        loop {
            match std::net::TcpListener::bind(("0.0.0.0", port)) {
                Ok(_) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                    if Instant::now() >= deadline {
                        return Err(AppError::PortInUse(port));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(AppError::Io(err)),
            }
        }
    }

    /// Same idea for the UDP half of the port number (`true` when free).
    fn probe_udp(port: u16) -> bool {
        std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
    }

    /// Current state; also reconciles a front-end that died on its own by
    /// clearing the stale entry (ARCHITECTURE §8-3 状态对账).
    pub fn status(&self) -> Option<ProxyState> {
        let mut guard = self.lock();
        let stale = guard
            .as_ref()
            .is_some_and(|r| !r.alive.load(Ordering::SeqCst));
        if stale {
            *guard = None;
            return None;
        }
        guard.as_ref().map(|r| r.state.clone())
    }

    /// `true` while the SOCKS5 listener is actually up.
    pub fn is_running(&self) -> bool {
        self.status().is_some()
    }

    /// Start the proxy for `cfg_name` under `routing`.
    ///
    /// Fails with [`AppError::PortInUse`] when the port is taken,
    /// [`AppError::AlreadyRunning`] when a proxy is already up,
    /// [`AppError::Acl`] / [`AppError::Dns`] when the routing settings are
    /// invalid (**before** anything is bound), and [`AppError::Core`] for
    /// core/bind failures (ARCHITECTURE §8-8: errors are never masked).
    pub fn enable(
        &self,
        cfg_name: &str,
        cfg: &ShadowsocksConfig,
        routing: &RoutingSettings,
    ) -> AppResult<()> {
        // Single-instance invariant, checked under the lock below.
        let mut guard = self.lock();
        if guard.is_some() {
            return Err(AppError::AlreadyRunning);
        }

        // GOAL §6.3: never bind anything but 0.0.0.0.
        let mut cfg = cfg.clone();
        cfg.client_settings.listen_address = "0.0.0.0".to_string();
        let port = cfg.client_settings.listen_port;

        // Port pre-check (probe socket dropped immediately).
        Self::probe_port(port, Duration::from_millis(600))?;

        // Build the core config through the exact same JSON path the files
        // take, so method/protocol/obfs names are validated by the library.
        let text = serde_json::to_string(&cfg).map_err(AppError::json)?;
        let core_cfg = ssr_client_rs::config_json::config_from_json(&text)
            .map_err(|e| AppError::Core(e.to_string()))?;

        // Routing (ACL + DNS) validates *before* we bind anything: a typo
        // in the ACL must not leave a half-started proxy behind. Built
        // inside the runtime context because the custom resolver holds a
        // tokio handle.
        let router = {
            let _enter = self.runtime.enter();
            Arc::new(Router::build(routing)?)
        };
        // Everything from here to the task spawning below runs inside the
        // runtime context: `UdpRelay::spawn` and the accept loop call
        // `tokio::spawn`, which panics without it.

        let client = ssr_client_rs::SsrClient::new(core_cfg.clone());
        let udp = cfg.udp;
        let idle_timeout = Duration::from_secs(u64::from(cfg.idle_timeout));
        let connect_timeout = Duration::from_secs(u64::from(cfg.connect_timeout.max(1)));

        // Bind the one TCP port plus (optionally) the UDP relay on the same
        // port number. Both are dropped on error before we return.
        let bound = self.runtime.block_on(async {
            let listener = TcpListener::bind(("0.0.0.0", port)).await.map_err(|e| {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    AppError::PortInUse(port)
                } else {
                    AppError::Core(format!("SOCKS5 bind on 0.0.0.0:{port}: {e}"))
                }
            })?;
            let relay = if udp {
                Some(
                    ssr_client_rs::local::udp_relay::UdpRelay::bind(core_cfg.clone())
                        .await
                        .map_err(|e| AppError::Core(e.to_string()))?,
                )
            } else {
                None
            };
            Ok::<_, AppError>((listener, relay))
        });
        let (listener, relay) = bound?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let alive = Arc::new(AtomicBool::new(true));
        {
            let _enter = self.runtime.enter();
            if let Some(relay) = relay {
                relay.spawn(shutdown_rx.clone());
            }

            let ctx = Arc::new(Frontend {
                client: client.clone(),
                router,
                udp,
                idle_timeout,
                connect_timeout,
            });
            self.runtime.spawn(socks5::serve(
                listener,
                ctx,
                shutdown_rx,
                Arc::clone(&alive),
                self.events.clone(),
            ));
        }

        let state = ProxyState {
            cfg_name: cfg_name.to_string(),
            port,
        };
        *guard = Some(Running {
            state: state.clone(),
            shutdown: shutdown_tx,
            alive,
            udp,
        });
        drop(guard);

        let _ = self.events.send_blocking(ProxyEvent::Started {
            cfg_name: state.cfg_name,
            port: state.port,
        });
        Ok(())
    }

    /// Stop the front-end and the UDP relay, then release the port.
    ///
    /// Returns only once both halves of the port number are *actually*
    /// free, so callers can immediately rebind or restore the system proxy
    /// without racing socket teardown. Bounded at 2s.
    pub fn disable(&self) -> AppResult<()> {
        let mut guard = self.lock();
        let Some(running) = guard.take() else {
            return Err(AppError::NotRunning);
        };
        let port = running.state.port;
        let udp = running.udp;

        let _ = running.shutdown.send(true);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let tcp_free = Self::probe_port(port, Duration::ZERO).is_ok();
            let udp_free = !udp || Self::probe_udp(port);
            if tcp_free && udp_free {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(AppError::Core(format!("port {port} was not released")));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ProxyService {
    fn drop(&mut self) {
        // Process exit path: make sure nothing keeps listening.
        if let Some(running) = self.lock().take() {
            let _ = running.shutdown.send(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{ClientSettings, ShadowsocksConfig};
    use crate::routing::RouteMode;
    use std::collections::HashSet;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::time::Duration;

    /// Tests count process-wide listeners, and several of them run proxies
    /// at once — so every proxy test takes this lock (they are all fast).
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A local port nothing is listening on right now.
    fn free_port() -> u16 {
        let sock = TcpListener::bind(("0.0.0.0", 0)).expect("bind ephemeral");
        sock.local_addr().unwrap().port()
    }

    fn cfg_on(port: u16) -> ShadowsocksConfig {
        ShadowsocksConfig {
            password: "secret".into(),
            method: "aes-256-cfb".into(),
            protocol: "auth_aes128_sha1".into(),
            obfs: "tls1.2_ticket_auth".into(),
            client_settings: ClientSettings {
                // Unreachable on purpose: only a *bypassed* connection may
                // succeed without a live SSR server.
                server: "127.0.0.1".into(),
                server_port: 1,
                listen_address: "0.0.0.0".into(),
                listen_port: port,
            },
            ..ShadowsocksConfig::default()
        }
    }

    fn routing(mode: RouteMode) -> RoutingSettings {
        RoutingSettings {
            mode,
            ..RoutingSettings::default()
        }
    }

    fn service() -> (ProxyService, async_channel::Receiver<ProxyEvent>) {
        let (tx, rx) = async_channel::unbounded();
        (ProxyService::new(tx), rx)
    }

    /// Local ports of *listening* TCP sockets (state 0A) owned by this
    /// process — derived from /proc so no external tools are needed.
    fn self_listen_tcp_ports() -> HashSet<u16> {
        let mut ours: HashSet<String> = HashSet::new();
        if let Ok(fds) = fs::read_dir("/proc/self/fd") {
            for fd in fds.flatten() {
                if let Ok(link) = fs::read_link(fd.path()) {
                    let s = link.to_string_lossy();
                    if let Some(inode) =
                        s.strip_prefix("socket:[").and_then(|x| x.strip_suffix(']'))
                    {
                        ours.insert(inode.to_string());
                    }
                }
            }
        }
        let mut ports = HashSet::new();
        for file in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 10 || cols[3] != "0A" || !ours.contains(cols[9]) {
                    continue; // 0A = TCP_LISTEN, cols[9] = inode
                }
                if let Ok(port) = u16::from_str_radix(cols[1].rsplit(':').next().unwrap(), 16) {
                    ports.insert(port);
                }
            }
        }
        ports
    }

    /// Speak SOCKS5 against `port` and return (stream, reply). `body` is
    /// everything after `[VER]`: `CMD, RSV, ATYP, ...`.
    fn socks5_connect(port: u16, body: &[u8]) -> (TcpStream, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to SOCKS5");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut ack = [0u8; 2];
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [0x05, 0x00], "method negotiation");

        let mut request = vec![0x05];
        request.extend_from_slice(body);
        stream.write_all(&request).unwrap();

        let mut reply = vec![0u8; 4];
        stream.read_exact(&mut reply).unwrap();
        let extra = match reply[3] {
            0x01 => 4 + 2,
            0x04 => 16 + 2,
            0x03 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).unwrap();
                reply.push(len[0]);
                usize::from(len[0]) + 2
            }
            other => panic!("bad atyp in reply {other}"),
        };
        let mut tail = vec![0u8; extra];
        stream.read_exact(&mut tail).unwrap();
        reply.extend_from_slice(&tail);
        (stream, reply)
    }

    /// `[CMD, RSV, ATYP, addr, port]` for a CONNECT to `ip:port`.
    fn connect_v4(ip: Ipv4Addr, port: u16) -> Vec<u8> {
        let mut v = vec![0x01, 0x00, 0x01];
        v.extend_from_slice(&ip.octets());
        v.extend_from_slice(&port.to_be_bytes());
        v
    }

    /// A one-shot echo server (returns its port).
    fn echo_server() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { break };
                std::thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    loop {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if conn.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        port
    }

    #[test]
    fn start_socks5_handshake_then_stop_frees_port() {
        let _g = serial();
        let (svc, rx) = service();
        let port = free_port();
        let cfg = cfg_on(port);

        svc.enable("test", &cfg, &routing(RouteMode::Global))
            .expect("enable");
        assert!(svc.is_running());
        assert_eq!(
            svc.status(),
            Some(ProxyState {
                cfg_name: "test".into(),
                port
            })
        );
        assert!(self_listen_tcp_ports().contains(&port));

        // GOAL 2.4: SOCKS5 method negotiation answered by the listener.
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to local SOCKS5");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [0x05, 0x00], "SOCKS5 greeting not accepted");
        drop(stream);

        svc.disable().expect("disable");
        assert!(!svc.is_running());
        assert_eq!(svc.status(), None);

        // Port is free again — disable() only returns once it really is.
        drop(TcpListener::bind(("0.0.0.0", port)).expect("port must be reusable"));
        assert!(!self_listen_tcp_ports().contains(&port));

        // Events: exactly one Started and exactly one clean Stopped.
        let mut started = 0;
        let mut stopped = 0;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                ProxyEvent::Started { .. } => started += 1,
                ProxyEvent::Stopped { reason } => {
                    stopped += 1;
                    assert_eq!(reason, None, "deliberate stop must carry no reason");
                }
            }
        }
        assert_eq!(started, 1);
        assert_eq!(stopped, 1);
    }

    #[test]
    fn duplicate_enable_reports_already_running() {
        let _g = serial();
        let (svc, _rx) = service();
        let port = free_port();
        svc.enable("a", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap();
        let err = svc
            .enable("b", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap_err();
        assert!(matches!(err, AppError::AlreadyRunning), "{err:?}");
        svc.disable().unwrap();
        let err = svc.disable().unwrap_err();
        assert!(matches!(err, AppError::NotRunning), "{err:?}");
    }

    #[test]
    fn busy_port_reports_port_in_use() {
        let _g = serial();
        let (svc, _rx) = service();
        let holder = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        let err = svc
            .enable("busy", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap_err();
        assert!(
            matches!(err, AppError::PortInUse(p) if p == port),
            "{err:?}"
        );
        // The probe must not have disturbed the real owner.
        assert_eq!(holder.local_addr().unwrap().port(), port);
    }

    #[test]
    fn exactly_one_tcp_listener_while_running() {
        // GOAL 2.5 / §6 N1: single exposed port.
        let _g = serial();
        let (svc, _rx) = service();
        let before = self_listen_tcp_ports();
        let port = free_port();
        assert!(!before.contains(&port));
        svc.enable("single", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap();
        let during = self_listen_tcp_ports();
        svc.disable().unwrap();
        let after = self_listen_tcp_ports();

        let mut expected = before.clone();
        expected.insert(port);
        assert_eq!(during, expected, "proxy must add exactly one listener");
        assert_eq!(after, before, "stop must remove the listener again");
    }

    #[test]
    fn invalid_method_fails_with_real_reason_and_no_listener_left() {
        let _g = serial();
        let (svc, _rx) = service();
        let before = self_listen_tcp_ports();
        let port = free_port();
        let mut cfg = cfg_on(port);
        cfg.method = "no-such-cipher".into();
        let err = svc
            .enable("bad", &cfg, &routing(RouteMode::Global))
            .unwrap_err();
        match err {
            AppError::Core(msg) | AppError::Json(msg) => {
                assert!(!msg.is_empty(), "error must carry a reason");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        assert!(svc.status().is_none());
        assert_eq!(
            self_listen_tcp_ports(),
            before,
            "failed enable must leave no listener behind"
        );
        drop(TcpListener::bind(("0.0.0.0", port)).expect("port must stay free"));
    }

    #[test]
    fn status_reconciles_dead_front_end() {
        // Simulates the front-end dying without us calling disable().
        let _g = serial();
        let (svc, _rx) = service();
        let port = free_port();
        svc.enable("reconcile", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap();
        {
            let mut guard = svc.lock();
            let running = guard.as_mut().expect("running");
            // Die behind the service's back: flip the stop signal but keep
            // the `Running` entry (as a crash would leave it).
            let _ = running.shutdown.send(true);
        }
        // status() must notice the accept loop is gone and forget the entry.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if svc.status().is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "front-end never released its alive flag"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // ...which means a fresh enable is possible again (its probe waits
        // out the teardown of the dead listener).
        svc.enable("reconcile", &cfg_on(port), &routing(RouteMode::Global))
            .unwrap();
        svc.disable().unwrap();
    }

    #[test]
    fn probe_socket_does_not_leak_as_listener() {
        // The port pre-check binds+drops; after a failed enable the only
        // listener left on that port must be the foreign owner's.
        let _g = serial();
        let (svc, _rx) = service();
        let holder = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        assert!(
            svc.enable("x", &cfg_on(port), &routing(RouteMode::Global))
                .is_err()
        );
        assert_eq!(
            self_listen_tcp_ports()
                .iter()
                .filter(|p| **p == port)
                .count(),
            1,
            "probe must not have left a second listener on {port}"
        );
    }

    #[test]
    fn invalid_routing_fails_before_binding_anything() {
        let _g = serial();
        let (svc, _rx) = service();
        let before = self_listen_tcp_ports();
        let port = free_port();
        let bad = RoutingSettings {
            mode: RouteMode::Acl,
            ..RoutingSettings::default()
        };
        let err = svc.enable("acl", &cfg_on(port), &bad).unwrap_err();
        assert!(matches!(err, AppError::Acl(_)), "{err:?}");
        assert_eq!(self_listen_tcp_ports(), before, "nothing may be bound");

        let bad_dns = RoutingSettings {
            dns: crate::routing::dns::DnsConfig::Custom(Vec::new()),
            ..RoutingSettings::default()
        };
        let err = svc.enable("dns", &cfg_on(port), &bad_dns).unwrap_err();
        assert!(matches!(err, AppError::Dns(_)), "{err:?}");
        assert_eq!(self_listen_tcp_ports(), before, "nothing may be bound");
    }

    #[test]
    fn bypass_lan_routes_around_a_dead_core() {
        // The core's server (127.0.0.1:1) refuses every connection, so a
        // proxied attempt cannot work — but a bypassed target must still
        // get through: that is the whole point of routing.
        let _g = serial();
        let echo = echo_server();
        let (svc, _rx) = service();
        let port = free_port();
        svc.enable("bypass", &cfg_on(port), &routing(RouteMode::BypassLan))
            .expect("enable");

        let (mut stream, reply) = socks5_connect(port, &connect_v4(Ipv4Addr::LOCALHOST, echo));
        assert_eq!(
            reply[1], 0x00,
            "loopback must be routed directly: {reply:?}"
        );
        stream.write_all(b"hello-routing").unwrap();
        let mut buf = [0u8; 13];
        stream.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello-routing", "direct path really carried bytes");

        svc.disable().unwrap();
    }

    #[test]
    fn global_mode_reports_a_failed_tunnel_instead_of_hanging() {
        // A non-loopback target in global mode must go through the (dead)
        // core and get a failure reply — never a stall, never a success.
        let _g = serial();
        let (svc, _rx) = service();
        let port = free_port();
        svc.enable("global", &cfg_on(port), &routing(RouteMode::Global))
            .expect("enable");

        let (stream, reply) = socks5_connect(port, &connect_v4(Ipv4Addr::new(192, 0, 2, 1), 80));
        assert_ne!(
            reply[1], 0x00,
            "unreachable SSR server must not yield success: {reply:?}"
        );
        drop(stream);
        svc.disable().unwrap();
    }

    #[test]
    fn udp_associate_follows_the_profile_flag() {
        let _g = serial();
        let (svc, _rx) = service();
        let port = free_port();
        let mut cfg = cfg_on(port);
        cfg.udp = false;
        svc.enable("noudp", &cfg, &routing(RouteMode::Global))
            .expect("enable");

        // UDP ASSOCIATE (CMD 0x03) to 0.0.0.0:0
        let (stream, reply) = socks5_connect(port, &[0x03, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            reply[1], 0x07,
            "UDP ASSOCIATE must be refused when udp=false: {reply:?}"
        );
        drop(stream);
        svc.disable().unwrap();
    }
}
