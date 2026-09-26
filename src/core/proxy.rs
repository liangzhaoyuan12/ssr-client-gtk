//! In-process proxy lifecycle around `ssr_client_rs::SsrClient`.
//!
//! Design notes (GOAL §5 / §6):
//! - **Single listener (N1)**: the only socket this app ever opens is the
//!   `SsrClient` SOCKS5 listener on `listen_port` (UDP shares the same port
//!   number). The port pre-check binds and *immediately drops* its probe
//!   socket, so it can never become a second listener.
//! - `listen_address` is forced to `0.0.0.0` (GOAL §6.3 / D2).
//! - A multi-thread tokio runtime lives inside the service; `start()` runs
//!   on it, `stop()` is synchronous on `SsrClient`. Everything here is
//!   blocking-but-bounded and is called from a worker thread by the UI
//!   layer, never from a GTK signal handler.
//! - Lifecycle facts are pushed to `events` so the UI can reconcile even
//!   when the core dies on its own (ARCHITECTURE §8-5).

use std::sync::mpsc as std_mpsc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;

use crate::config::model::ShadowsocksConfig;
use crate::error::{AppError, AppResult};

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
        /// `None` on a deliberate stop, `Some(reason)` on a core failure.
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
    client: ssr_client_rs::SsrClient,
    state: ProxyState,
}

/// Owns the tokio runtime and the (at most one) running `SsrClient`.
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

    /// Current state; also reconciles a core that died on its own by
    /// clearing the stale entry (ARCHITECTURE §8-3 状态对账).
    pub fn status(&self) -> Option<ProxyState> {
        let mut guard = self.lock();
        let stale = guard.as_ref().is_some_and(|r| !r.client.is_running());
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

    /// Start the proxy for `cfg_name`.
    ///
    /// Fails with [`AppError::PortInUse`] when the port is taken and
    /// [`AppError::AlreadyRunning`] when a proxy is already up; any
    /// `ssr-client-rs` failure surfaces as [`AppError::Core`] with the
    /// real cause (ARCHITECTURE §8-8: errors are never masked).
    pub fn enable(&self, cfg_name: &str, cfg: &ShadowsocksConfig) -> AppResult<()> {
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

        let client = ssr_client_rs::SsrClient::new(core_cfg);
        let task_client = client.clone();
        let state = ProxyState {
            cfg_name: cfg_name.to_string(),
            port,
        };

        // Diagnostic channel: the start task reports its outcome so
        // enable() can fail fast instead of waiting blind.
        let (done_tx, done_rx) = std_mpsc::channel::<Result<(), String>>();
        let events = self.events.clone();
        self.runtime.spawn(async move {
            let outcome = task_client.start().await;
            let reason = outcome.as_ref().err().map(|e| e.to_string());
            let _ = done_tx.send(outcome.map(|_| ()).map_err(|e| e.to_string()));
            let _ = events.send_blocking(ProxyEvent::Stopped { reason });
        });

        // Wait (bounded) for the listener to come up or the task to fail.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if client.is_running() {
                break;
            }
            match done_rx.try_recv() {
                Ok(Err(reason)) => return Err(AppError::Core(reason)),
                Ok(Ok(())) => return Err(AppError::Core("start returned immediately".into())),
                Err(std_mpsc::TryRecvError::Empty) => {}
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    return Err(AppError::Core("start task vanished".into()));
                }
            }
            if Instant::now() >= deadline {
                return Err(AppError::Core("SOCKS5 listener did not come up".into()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        *guard = Some(Running {
            client,
            state: state.clone(),
        });
        drop(guard);

        let _ = self.events.send_blocking(ProxyEvent::Started {
            cfg_name: state.cfg_name,
            port: state.port,
        });
        Ok(())
    }

    /// Stop the proxy and release the port.
    ///
    /// Returns only once the listener is *actually* gone (probe bind
    /// succeeds), so callers can immediately rebind or restore the system
    /// proxy without racing the socket teardown. Bounded at 2s.
    pub fn disable(&self) -> AppResult<()> {
        let mut guard = self.lock();
        let Some(running) = guard.take() else {
            return Err(AppError::NotRunning);
        };
        let port = running.state.port;

        running.client.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stopped = !running.client.is_running();
            if stopped && Self::probe_port(port, Duration::ZERO).is_ok() {
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
            running.client.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{ClientSettings, ShadowsocksConfig};
    use std::collections::HashSet;
    use std::fs;
    use std::net::{TcpListener, TcpStream};
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
                server: "127.0.0.1".into(),
                server_port: 1, // never dialed in these tests
                listen_address: "0.0.0.0".into(),
                listen_port: port,
            },
            ..ShadowsocksConfig::default()
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

    #[test]
    fn start_socks5_handshake_then_stop_frees_port() {
        let _g = serial();
        let (svc, rx) = service();
        let port = free_port();
        let cfg = cfg_on(port);

        svc.enable("test", &cfg).expect("enable");
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
        use std::io::{Read, Write};
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
        svc.enable("a", &cfg_on(port)).unwrap();
        let err = svc.enable("b", &cfg_on(port)).unwrap_err();
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
        let err = svc.enable("busy", &cfg_on(port)).unwrap_err();
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
        svc.enable("single", &cfg_on(port)).unwrap();
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
        let err = svc.enable("bad", &cfg).unwrap_err();
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
    fn status_reconciles_dead_core() {
        // Simulates the core dying without us calling disable().
        let _g = serial();
        let (svc, _rx) = service();
        let port = free_port();
        svc.enable("reconcile", &cfg_on(port)).unwrap();
        {
            let mut guard = svc.lock();
            let running = guard.as_mut().expect("running");
            running.client.stop(); // die behind the service's back
        }
        // status() must notice and forget the stale entry.
        assert_eq!(svc.status(), None);
        // ...which means a fresh enable is possible again (its probe waits
        // out the teardown of the dead listener).
        svc.enable("reconcile", &cfg_on(port)).unwrap();
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
        assert!(svc.enable("x", &cfg_on(port)).is_err());
        assert_eq!(
            self_listen_tcp_ports()
                .iter()
                .filter(|p| **p == port)
                .count(),
            1,
            "probe must not have left a second listener on {port}"
        );
    }
}
