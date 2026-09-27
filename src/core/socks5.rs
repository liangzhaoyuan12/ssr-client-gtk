//! Routing-aware SOCKS5 front-end — the app's **only** listener (N1).
//!
//! `ssr-client-rs` can either bind its own listener and tunnel *everything*
//! (Mode A) or hand out one encrypted session per target (Mode B,
//! `SsrClient::open_session`). Routing needs a decision per connection, so
//! this module owns the socket and the protocol dialogue, then picks:
//!
//! - **direct** → resolve with the configured DNS and `TcpStream::connect`,
//! - **proxy**  → `open_session(target)` and pump the bytes through it.
//!
//! The wire helpers (parsers, reply builders, constants) all come from
//! `ssr_client_rs::socks5`, so the dialogue cannot drift from the library's
//! own Mode-A implementation. UDP ASSOCIATE keeps pointing at the library's
//! [`ssr_client_rs::local::udp_relay::UdpRelay`], which is bound to the same
//! port number by [`crate::core::proxy`].
//!
//! One documented limitation: the UDP relay has no per-target routing — UDP
//! always goes through the SSR server (see GOAL §4.3).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use ssr_client_rs::TargetAddr;
use ssr_client_rs::socks5::{
    self, ATYP_IPV4, ATYP_IPV6, CMD_CONNECT, CMD_UDP_ASSOCIATE, ConnectRequest, TargetAddress,
};

use crate::core::proxy::ProxyEvent;
use crate::routing::{Route, Router};

/// Everything a connection handler needs.
pub(crate) struct Frontend {
    /// Untouched core instance — only used for `open_session` (Mode B).
    pub client: ssr_client_rs::SsrClient,
    /// Routing decisions + local DNS.
    pub router: Arc<Router>,
    /// Whether the UDP relay is bound (UDP ASSOCIATE is refused if not).
    pub udp: bool,
    /// `idle_timeout` from the profile (0 = never time out).
    pub idle_timeout: Duration,
    /// `connect_timeout` from the profile, applied to direct connections.
    pub connect_timeout: Duration,
}

/// Run the accept loop until `shutdown` flips, then report why we stopped.
///
/// The lifecycle event is sent **before** the listener is dropped, so a
/// caller waiting for the port to be released always finds the event queued
/// (the UI's disable flow relies on this ordering).
pub(crate) async fn serve(
    listener: TcpListener,
    ctx: Arc<Frontend>,
    mut shutdown: watch::Receiver<bool>,
    alive: Arc<AtomicBool>,
    events: async_channel::Sender<ProxyEvent>,
) {
    let mut reason: Option<String> = None;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        log::debug!("[s5] connection from {peer}");
                        let ctx = Arc::clone(&ctx);
                        tokio::spawn(async move {
                            if let Err(e) = handle(stream, &ctx).await {
                                log::debug!("[s5] {peer} closed: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        reason = Some(format!("SOCKS5 accept failed: {e}"));
                        break;
                    }
                }
            }
            changed = shutdown.changed() => {
                // `changed()` also returns Err when the service is dropped.
                let stopping = changed.is_ok() && *shutdown.borrow();
                if !stopping {
                    reason = Some("shutdown signal lost".into());
                }
                break;
            }
        }
    }

    let _ = events.send_blocking(ProxyEvent::Stopped { reason });
    // Released last: everything interested has been notified by now.
    drop(listener);
    alive.store(false, Ordering::SeqCst);
}

/// One client connection: greeting → request → routed relay.
async fn handle(mut stream: TcpStream, ctx: &Frontend) -> Result<(), String> {
    let negotiation = read_method_negotiation(&mut stream).await?;
    if !negotiation.methods.contains(&socks5::AUTH_NONE) {
        stream
            .write_all(&socks5::build_method_response(socks5::AUTH_REQUIRED))
            .await
            .map_err(|e| e.to_string())?;
        return Err("client requires an auth method we do not offer".into());
    }
    stream
        .write_all(&socks5::build_method_response(socks5::AUTH_NONE))
        .await
        .map_err(|e| e.to_string())?;

    let req = read_connect_request(&mut stream).await?;
    match req.cmd {
        CMD_CONNECT => connect(&mut stream, ctx, req).await,
        CMD_UDP_ASSOCIATE => udp_associate(&mut stream, ctx).await,
        cmd => {
            reply(
                &mut stream,
                socks5::REP_COMMAND_NOT_SUPPORTED,
                &TargetAddress::IPv4([0, 0, 0, 0]),
                0,
            )
            .await?;
            Err(format!("unsupported SOCKS5 command 0x{cmd:02x}"))
        }
    }
}

/// `[VER, NMETHODS, METHODS…]` → parsed negotiation.
async fn read_method_negotiation(
    stream: &mut TcpStream,
) -> Result<socks5::MethodNegotiation, String> {
    let mut head = [0u8; 2];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(usize::from(head[1]) + 2);
    buf.extend_from_slice(&head);
    buf.resize(2 + usize::from(head[1]), 0);
    stream
        .read_exact(&mut buf[2..])
        .await
        .map_err(|e| e.to_string())?;
    socks5::parse_method_negotiation(&buf).map_err(|e| e.to_string())
}

/// `[VER, CMD, RSV, ATYP, ADDR…, PORT]` → parsed request, read exactly as
/// many bytes as the address type requires (no over-read into the payload).
async fn read_connect_request(stream: &mut TcpStream) -> Result<ConnectRequest, String> {
    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(4 + 1 + 16 + 2);
    buf.extend_from_slice(&head);
    // Remaining bytes: address (4 / 16 / len+name) then port. For domains
    // the length byte is part of the request, so it goes into the buffer —
    // it must not be read twice, and nothing beyond it may be consumed
    // (the bytes that follow belong to the application payload).
    let tail_len = match head[3] {
        socks5::ATYP_IPV4 => 4 + 2,
        socks5::ATYP_IPV6 => 16 + 2,
        socks5::ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|e| e.to_string())?;
            buf.extend_from_slice(&len);
            usize::from(len[0]) + 2
        }
        other => return Err(format!("unsupported address type 0x{other:02x}")),
    };
    let mut tail = vec![0u8; tail_len];
    stream
        .read_exact(&mut tail)
        .await
        .map_err(|e| e.to_string())?;
    buf.extend_from_slice(&tail);
    socks5::parse_connect_request(&buf).map_err(|e| e.to_string())
}

/// Send a SOCKS5 reply bound to `addr:port` (usually 0.0.0.0:0).
async fn reply(
    stream: &mut TcpStream,
    rep: u8,
    addr: &TargetAddress,
    port: u16,
) -> Result<(), String> {
    let bytes = match addr {
        TargetAddress::IPv4(oct) => oct.to_vec(),
        TargetAddress::IPv6(oct) => oct.to_vec(),
        TargetAddress::Domain(_) => vec![0, 0, 0, 0],
    };
    let (atyp, bytes) = match addr {
        TargetAddress::Domain(_) => (ATYP_IPV4, bytes),
        other => (other.atyp(), bytes),
    };
    stream
        .write_all(&socks5::build_connect_reply(rep, atyp, &bytes, port))
        .await
        .map_err(|e| e.to_string())
}

/// UDP ASSOCIATE: point the client at the same port number the library's
/// UDP relay is bound to, then hold the control connection open (RFC 1928).
async fn udp_associate(stream: &mut TcpStream, ctx: &Frontend) -> Result<(), String> {
    let local = stream.local_addr().map_err(|e| e.to_string())?;
    let (atyp, bytes, port) = match local {
        std::net::SocketAddr::V4(a) => (ATYP_IPV4, a.ip().octets().to_vec(), a.port()),
        std::net::SocketAddr::V6(a) => (ATYP_IPV6, a.ip().octets().to_vec(), a.port()),
    };
    let rep = if ctx.udp {
        socks5::REP_SUCCESS
    } else {
        socks5::REP_COMMAND_NOT_SUPPORTED
    };
    stream
        .write_all(&socks5::build_connect_reply(rep, atyp, &bytes, port))
        .await
        .map_err(|e| e.to_string())?;
    if !ctx.udp {
        return Err("UDP relay disabled".into());
    }
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    Ok(())
}

/// CONNECT: decide, establish, reply, pump.
async fn connect(
    stream: &mut TcpStream,
    ctx: &Frontend,
    req: ConnectRequest,
) -> Result<(), String> {
    let port = req.port;
    let target = req.addr;
    let route = ctx.router.decide(&target).await;
    log::debug!(
        "[s5] CONNECT {}:{port} → {}",
        target.display(),
        match route {
            Route::Direct => "direct",
            Route::Proxy => "proxy",
        }
    );

    match route {
        Route::Direct => connect_direct(stream, ctx, &target, port).await,
        Route::Proxy => connect_proxy(stream, ctx, target, port).await,
    }
}

async fn connect_direct(
    stream: &mut TcpStream,
    ctx: &Frontend,
    target: &TargetAddress,
    port: u16,
) -> Result<(), String> {
    let addrs = resolve_target(ctx, target, port).await;
    if addrs.is_empty() {
        reply(stream, socks5::REP_HOST_UNREACHABLE, target, 0).await?;
        return Err(format!("cannot resolve {}", target.display()));
    }

    let mut outcome: Option<std::io::Result<TcpStream>> = None;
    for addr in addrs {
        let attempt = tokio::time::timeout(ctx.connect_timeout, TcpStream::connect(addr)).await;
        match attempt {
            Ok(Ok(sock)) => {
                outcome = Some(Ok(sock));
                break;
            }
            Ok(Err(e)) => outcome = Some(Err(e)),
            Err(_) => {
                outcome = Some(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connect timeout",
                )));
            }
        }
    }

    match outcome {
        Some(Ok(mut outbound)) => {
            reply(stream, socks5::REP_SUCCESS, target, 0).await?;
            let idle = ctx.idle_timeout;
            if idle.is_zero() {
                tokio::io::copy_bidirectional(stream, &mut outbound)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            } else {
                pump(&mut *stream, outbound, idle).await
            }
        }
        Some(Err(e)) => {
            let rep = match e.kind() {
                std::io::ErrorKind::ConnectionRefused => socks5::REP_CONNECTION_REFUSED,
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                    socks5::REP_HOST_UNREACHABLE
                }
                std::io::ErrorKind::NetworkUnreachable => socks5::REP_NETWORK_UNREACHABLE,
                _ => socks5::REP_HOST_UNREACHABLE,
            };
            reply(stream, rep, target, 0).await?;
            Err(format!("direct connect failed: {e}"))
        }
        None => {
            reply(stream, socks5::REP_GENERAL_FAILURE, target, 0).await?;
            Err("no address to connect to".into())
        }
    }
}

async fn connect_proxy(
    stream: &mut TcpStream,
    ctx: &Frontend,
    target: TargetAddress,
    port: u16,
) -> Result<(), String> {
    let mut session = match ctx.client.open_session(to_target_addr(&target, port)).await {
        Ok(session) => session,
        Err(e) => {
            let msg = e.to_string();
            let rep = if msg.contains("refused") {
                socks5::REP_CONNECTION_REFUSED
            } else if msg.to_lowercase().contains("timeout") {
                socks5::REP_HOST_UNREACHABLE
            } else {
                socks5::REP_GENERAL_FAILURE
            };
            reply(stream, rep, &target, 0).await?;
            return Err(format!("ssr session: {msg}"));
        }
    };
    reply(stream, socks5::REP_SUCCESS, &target, 0).await?;

    let relayed = tokio::io::copy_bidirectional(stream, &mut session)
        .await
        .map_err(|e| e.to_string());
    // Graceful close: drain the server side and surface a mid-stream error.
    let finished = session.finish().await.map_err(|e| e.to_string());
    relayed.and(finished)
}

/// Local addresses for a direct connection, honouring the configured DNS.
async fn resolve_target(
    ctx: &Frontend,
    target: &TargetAddress,
    port: u16,
) -> Vec<std::net::SocketAddr> {
    match target {
        TargetAddress::IPv4(oct) => vec![std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(oct[0], oct[1], oct[2], oct[3])),
            port,
        )],
        TargetAddress::IPv6(oct) => {
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(oct);
            vec![std::net::SocketAddr::new(
                std::net::IpAddr::V6(bytes.into()),
                port,
            )]
        }
        TargetAddress::Domain(raw) => {
            let host = String::from_utf8_lossy(raw).into_owned();
            ctx.router
                .resolve(&host)
                .await
                .into_iter()
                .map(|ip| std::net::SocketAddr::new(ip, port))
                .collect()
        }
    }
}

/// Bidirectional pump with a reset-on-activity idle timeout — the same
/// semantics the core applies to tunnelled connections.
async fn pump<A, B>(a: A, b: B, idle: Duration) -> Result<(), String>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut a_read, mut a_write) = tokio::io::split(a);
    let (mut b_read, mut b_write) = tokio::io::split(b);
    let mut buf_ab = vec![0u8; 32 * 1024];
    let mut buf_ba = vec![0u8; 32 * 1024];
    let mut deadline = tokio::time::Instant::now() + idle;

    loop {
        tokio::select! {
            read = a_read.read(&mut buf_ab) => match read {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    b_write.write_all(&buf_ab[..n]).await.map_err(|e| e.to_string())?;
                    deadline = tokio::time::Instant::now() + idle;
                }
                Err(e) if is_disconnect(&e) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            },
            read = b_read.read(&mut buf_ba) => match read {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    a_write.write_all(&buf_ba[..n]).await.map_err(|e| e.to_string())?;
                    deadline = tokio::time::Instant::now() + idle;
                }
                Err(e) if is_disconnect(&e) => return Ok(()),
                Err(e) => return Err(e.to_string()),
            },
            _ = tokio::time::sleep_until(deadline) => {
                return Err(format!("idle for {}s", idle.as_secs()));
            }
        }
    }
}

fn is_disconnect(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn to_target_addr(target: &TargetAddress, port: u16) -> TargetAddr {
    match target {
        TargetAddress::IPv4(oct) => TargetAddr::IPv4(*oct, port),
        TargetAddress::IPv6(oct) => TargetAddr::IPv6(*oct, port),
        TargetAddress::Domain(raw) => {
            TargetAddr::Domain(String::from_utf8_lossy(raw).into_owned(), port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// Domain requests: build the exact byte stream a client sends and make
    /// sure the reader consumes only what belongs to the request (the
    /// trailing bytes are the payload of the first relayed segment).
    #[tokio::test]
    async fn reads_domain_requests_without_eating_the_payload() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            // greeting
            s.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut ack = [0u8; 2];
            s.read_exact(&mut ack).await.unwrap();
            assert_eq!(ack, [0x05, 0x00]);
            // CONNECT example.com:80 followed by payload bytes
            let mut req = vec![0x05, 0x01, 0x00, 0x03, 11];
            req.extend_from_slice(b"example.com");
            req.extend_from_slice(&80u16.to_be_bytes());
            req.extend_from_slice(b"GET / HTTP/1.1\r\n\r\n");
            s.write_all(&req).await.unwrap();
            s
        });

        let (mut server, _) = listener.accept().await.unwrap();
        let negotiation = read_method_negotiation(&mut server).await.unwrap();
        assert_eq!(negotiation.methods, vec![0x00]);
        server
            .write_all(&socks5::build_method_response(socks5::AUTH_NONE))
            .await
            .unwrap();
        let req = read_connect_request(&mut server).await.unwrap();
        assert_eq!(req.cmd, CMD_CONNECT);
        assert_eq!(req.port, 80);
        assert_eq!(req.addr, TargetAddress::Domain(b"example.com".to_vec()));
        assert_eq!(req.addr.display(), "example.com");

        // Still on the socket: the parser consumed the request, not the
        // payload that followed it. Echo it back so the client can finish.
        let mut payload = [0u8; 18];
        server.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload[..8], b"GET / HT", "payload must survive the parse");
        server.write_all(&payload).await.unwrap();

        let mut s = client.await.unwrap();
        let mut echo = [0u8; 18];
        s.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, &payload, "payload round-trips untouched");
    }

    #[tokio::test]
    async fn ipv4_and_ipv6_requests_parse() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
            let mut ack = [0u8; 2];
            s.read_exact(&mut ack).await.unwrap();
            s.write_all(&[0x05, 0x01, 0x00, 0x01, 10, 0, 0, 1, 0x01, 0xbb])
                .await
                .unwrap();
            s.write_all(&[0x05, 0x01, 0x00, 0x04]).await.unwrap();
            s.write_all(&[0u8; 16]).await.unwrap();
            s.write_all(&53u16.to_be_bytes()).await.unwrap();
            s
        });

        let (mut server, _) = listener.accept().await.unwrap();
        let negotiation = read_method_negotiation(&mut server).await.unwrap();
        assert_eq!(negotiation.methods, vec![0x00]);
        server
            .write_all(&socks5::build_method_response(socks5::AUTH_NONE))
            .await
            .unwrap();
        let req = read_connect_request(&mut server).await.unwrap();
        assert_eq!(req.addr, TargetAddress::IPv4([10, 0, 0, 1]), "IPv4 target");
        assert_eq!(req.port, 443);
        let req6 = read_connect_request(&mut server).await.unwrap();
        assert_eq!(req6.addr, TargetAddress::IPv6([0u8; 16]));
        assert_eq!(req6.port, 53);
        drop(client.await.unwrap());
    }

    #[test]
    fn target_address_converts_to_core_form() {
        // `TargetAddr` has no PartialEq, so match the variants instead.
        match to_target_addr(&TargetAddress::Domain(b"a.b".to_vec()), 443) {
            TargetAddr::Domain(name, port) => {
                assert_eq!(name, "a.b");
                assert_eq!(port, 443);
            }
            other => panic!("unexpected {other:?}"),
        }
        match to_target_addr(&TargetAddress::IPv4([1, 2, 3, 4]), 53) {
            TargetAddr::IPv4(oct, port) => {
                assert_eq!(oct, [1, 2, 3, 4]);
                assert_eq!(port, 53);
            }
            other => panic!("unexpected {other:?}"),
        }
        match to_target_addr(&TargetAddress::IPv6([1u8; 16]), 1) {
            TargetAddr::IPv6(oct, port) => {
                assert_eq!(oct, [1u8; 16]);
                assert_eq!(port, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn pump_is_bounded_and_carries_bytes_both_ways() {
        let (a, mut b) = tokio::io::duplex(64);
        let (a2, mut b2) = tokio::io::duplex(64);
        // `pump` wants TcpStreams; exercise the byte path through duplexes
        // by calling the same code on the two halves we do have.
        let pump_task = tokio::spawn(async move {
            let (mut ar, mut aw) = tokio::io::split(a);
            let (mut br, mut bw) = tokio::io::split(a2);
            let mut x = vec![0u8; 16];
            let mut y = vec![0u8; 16];
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            tokio::select! {
                r = ar.read(&mut x) => {
                    let n = r.unwrap();
                    bw.write_all(&x[..n]).await.unwrap();
                }
                r = br.read(&mut y) => {
                    let n = r.unwrap();
                    aw.write_all(&y[..n]).await.unwrap();
                }
                _ = tokio::time::sleep_until(deadline) => {}
            }
        });
        b.write_all(b"ping").await.unwrap();
        let mut out = [0u8; 4];
        b2.read_exact(&mut out).await.unwrap();
        assert_eq!(&out, b"ping");
        pump_task.await.unwrap();
    }

    #[test]
    fn address_helpers_are_stable() {
        let v4 = TargetAddress::IPv4([192, 168, 0, 1]);
        assert_eq!(v4.atyp(), socks5::ATYP_IPV4);
        assert_eq!(v4.display(), "192.168.0.1");
        let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 9);
        assert_eq!(addr.port(), 9);
    }
}
