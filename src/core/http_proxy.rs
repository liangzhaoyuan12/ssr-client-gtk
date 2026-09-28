//! HTTP proxy front-end on the **same** listener as SOCKS5 (GOAL §11 B11,
//! decision D10).
//!
//! Windows has no native SOCKS support: WinINET, the Windows Settings proxy
//! page and the Chromium family only speak **HTTP proxies** (`CONNECT` for
//! HTTPS, absolute-URI requests for plain HTTP). Pointing the Windows system
//! proxy at our SOCKS5 port would silently do nothing, so the single exposed
//! port (invariant N1 — still exactly one port) now accepts both protocols:
//!
//! - first byte `0x05` → the existing SOCKS5 dialogue (`socks5::handle`),
//! - anything else      → this module.
//!
//! The adapter does **not** re-implement routing: it dials back into our own
//! SOCKS5 front-end over loopback and speaks the client side of that
//! handshake, so every HTTP connection gets exactly the same
//! `Router::decide` + direct/SSR treatment (and the same DNS policy) as a
//! SOCKS5 one. Linux/macOS keep writing SOCKS to their desktop settings;
//! their SOCKS5 path is untouched (sniffing only *adds* the HTTP branch).

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::core::socks5::{self, Frontend};

/// Largest request head (request line + headers) we are willing to parse.
const MAX_HEAD: usize = 16 * 1024;

/// What the client asked for, after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// `CONNECT host:port` → raw byte tunnel to `host:port`.
    Connect { host: String, port: u16 },
    /// Absolute-form request → forward with `origin` (path + query) as the
    /// request target, to `host:port`.
    Forward {
        host: String,
        port: u16,
        origin: String,
    },
}

/// Read exactly one request head (up to and including `\r\n\r\n`).
///
/// Returns the head plus any bytes already read past it — those belong to
/// the client payload and must be forwarded, never dropped.
async fn read_head(stream: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(pos) = head_end(&buf) {
            let head = buf[..pos + 4].to_vec();
            let rest = buf[pos + 4..].to_vec();
            return Ok((head, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err("request head too large".into());
        }
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("client closed before sending a request".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Offset of the `\r\n\r\n` terminator, if present.
fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse a request head (pure — unit-tested without sockets).
fn parse_head(head: &[u8]) -> Result<Request, String> {
    let text = String::from_utf8_lossy(head);
    let line = text
        .split("\r\n")
        .next()
        .ok_or_else(|| "empty request head".to_string())?;
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(format!("malformed request line: {line:?}"));
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(target)?;
        return Ok(Request::Connect { host, port });
    }

    // Absolute-form: `http://host[:port]/path` (RFC 7230 §5.3.2).
    let (scheme, rest) = target
        .split_once("://")
        .ok_or_else(|| format!("unsupported request target {target:?} (expected absolute URI)"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("unsupported scheme {scheme:?}"));
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    let (authority, path_query) = match rest.split_once('/') {
        Some((auth, tail)) => (auth, format!("/{tail}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = split_authority(authority, default_port)?;
    Ok(Request::Forward {
        host,
        port,
        origin: path_query,
    })
}

/// Absolute-form authority: `host[:port]`, `[v6]`, `[v6]:port` — the port
/// is optional here (defaults by scheme), unlike CONNECT where it is
/// mandatory. Without this, `http://[::1]/path` used to be rejected.
fn split_authority(s: &str, default_port: u16) -> Result<(String, u16), String> {
    if let Ok(hp) = split_host_port(s) {
        return Ok(hp);
    }
    if s.is_empty() {
        return Err("empty authority".into());
    }
    if !s.contains(':') {
        return Ok((s.to_string(), default_port));
    }
    if s.starts_with('[') && s.ends_with(']') && s.len() > 2 {
        let inner = &s[1..s.len() - 1];
        if inner.contains(':') {
            return Ok((inner.to_string(), default_port));
        }
    }
    Err(format!("missing or bad port in authority {s:?}"))
}

/// `host:port`, `[v6]:port` → (host, port).
fn split_host_port(s: &str) -> Result<(String, u16), String> {
    if let Some(rest) = s.strip_prefix('[') {
        // Bracketed IPv6.
        let (addr, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("unterminated IPv6 literal in {s:?}"))?;
        let port = tail
            .strip_prefix(':')
            .ok_or_else(|| format!("missing port in {s:?}"))?
            .parse::<u16>()
            .map_err(|_| format!("bad port in {s:?}"))?;
        return Ok((addr.to_string(), port));
    }
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("missing port in {s:?}"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("bad port in {s:?}"))?;
    if host.is_empty() {
        return Err(format!("missing host in {s:?}"));
    }
    Ok((host.to_string(), port))
}

/// Rebuild the head that goes upstream: origin-form target, no proxy headers.
fn forward_head(head: &[u8], req: &Request) -> Result<Vec<u8>, String> {
    let Request::Forward { origin, .. } = req else {
        return Err("forward_head on a CONNECT request".into());
    };
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default();
    let _target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if version.is_empty() {
        return Err("truncated request line".into());
    }
    let mut out = format!("{method} {origin} {version}\r\n");
    for header in lines {
        if header.is_empty() {
            break; // end of headers
        }
        let lower = header.to_ascii_lowercase();
        // Hop-by-hop / proxy-only headers must not reach the origin.
        if lower.starts_with("proxy-connection:") || lower.starts_with("proxy-authorization:") {
            continue;
        }
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    Ok(out.into_bytes())
}

/// Dial our own SOCKS5 front-end over loopback and complete the handshake
/// for `host:port` — the routing decision then happens exactly where it
/// happens for SOCKS5 clients (in `socks5::handle`).
async fn dial(local_port: u16, host: &str, port: u16) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", local_port))
        .await
        .map_err(|e| format!("local front-end connect: {e}"))?;

    stream
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|e| e.to_string())?;
    let mut ack = [0u8; 2];
    stream
        .read_exact(&mut ack)
        .await
        .map_err(|e| e.to_string())?;
    if ack != [0x05, 0x00] {
        return Err(format!("local front-end greeting rejected: {ack:?}"));
    }

    let mut req = vec![0x05u8, 0x01, 0x00];
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        req.push(0x01);
        req.extend_from_slice(&v4.octets());
    } else if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        req.push(0x04);
        req.extend_from_slice(&v6.octets());
    } else {
        let bytes = host.as_bytes();
        if bytes.len() > 255 {
            return Err("host name longer than 255 bytes".into());
        }
        req.push(0x03);
        req.push(bytes.len() as u8);
        req.extend_from_slice(bytes);
    }
    req.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&req).await.map_err(|e| e.to_string())?;

    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| e.to_string())?;
    if head[1] != 0x00 {
        return Err(format!(
            "local front-end reply 0x{:02x} for {host}:{port}",
            head[1]
        ));
    }
    // Consume the bound address so the tunnel starts at the payload.
    let tail = match head[3] {
        0x01 => 4 + 2,
        0x04 => 16 + 2,
        0x03 => {
            let mut len = [0u8; 1];
            stream
                .read_exact(&mut len)
                .await
                .map_err(|e| e.to_string())?;
            1 + usize::from(len[0]) + 2
        }
        other => return Err(format!("bad address type 0x{other:02x} in reply")),
    };
    let mut rest = vec![0u8; tail];
    stream
        .read_exact(&mut rest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(stream)
}

/// One HTTP-proxy connection.
pub(crate) async fn handle(
    mut client: TcpStream,
    ctx: &Frontend,
    local_port: u16,
) -> Result<(), String> {
    let (head, leftovers) = read_head(&mut client).await?;
    let req = match parse_head(&head) {
        Ok(req) => req,
        Err(reason) => return reject(client, &reason).await,
    };

    let (host, port) = match &req {
        Request::Connect { host, port } => (host.clone(), *port),
        Request::Forward { host, port, .. } => (host.clone(), *port),
    };

    let mut tunnel = match dial(local_port, &host, port).await {
        Ok(t) => t,
        Err(e) => {
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Err(e);
        }
    };

    match req {
        Request::Connect { .. } => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .map_err(|e| e.to_string())?;
            relay(client, tunnel, ctx, leftovers).await
        }
        Request::Forward { .. } => {
            let rewritten = forward_head(&head, &req)?;
            tunnel
                .write_all(&rewritten)
                .await
                .map_err(|e| e.to_string())?;
            if !leftovers.is_empty() {
                tunnel
                    .write_all(&leftovers)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            relay(client, tunnel, ctx, Vec::new()).await
        }
    }
}

/// Answer `400` to anything that is not a proxy request (never hang, never
/// swallow the bytes) and close — "非代理流量不吞" from GOAL Phase 8.3.
pub(crate) async fn reject(mut client: TcpStream, reason: &str) -> Result<(), String> {
    let body = format!("bad request: {reason}\n");
    let response = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = client.write_all(response.as_bytes()).await;
    let _ = client.shutdown().await;
    Ok(())
}

async fn relay(
    client: TcpStream,
    tunnel: TcpStream,
    ctx: &Frontend,
    _pending: Vec<u8>,
) -> Result<(), String> {
    let idle = ctx.idle_timeout;
    if idle.is_zero() {
        let mut client = client;
        let mut tunnel = tunnel;
        tokio::io::copy_bidirectional(&mut client, &mut tunnel)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        socks5::pump(client, tunnel, idle).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{ClientSettings, ShadowsocksConfig};
    use crate::core::proxy::{ProxyEvent, ProxyService};
    use crate::routing::{RouteMode, RoutingSettings};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream as StdStream};
    use std::time::Duration;

    /// Not `bind(:0)` — see `core::test_sync::free_port`.
    fn free_port() -> u16 {
        crate::core::test_sync::free_port()
    }

    /// Every test below binds a *concrete* port, so it holds the shared
    /// lock first: the `proxy` tests (and their own listeners) run in
    /// parallel threads and used to be able to grab the same number.
    fn take_port_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::core::test_sync::port_lock()
    }

    fn cfg_on(port: u16) -> ShadowsocksConfig {
        ShadowsocksConfig {
            password: "secret".into(),
            method: "aes-256-cfb".into(),
            protocol: "auth_aes128_sha1".into(),
            obfs: "tls1.2_ticket_auth".into(),
            client_settings: ClientSettings {
                server: "127.0.0.1".into(),
                server_port: 1, // unreachable: only bypassed traffic may work
                listen_address: "0.0.0.0".into(),
                listen_port: port,
            },
            ..ShadowsocksConfig::default()
        }
    }

    /// Loopback targets must be routed directly (no live SSR server here).
    fn bypass_routing() -> RoutingSettings {
        RoutingSettings {
            mode: RouteMode::BypassLan,
            ..RoutingSettings::default()
        }
    }

    fn service() -> (ProxyService, async_channel::Receiver<ProxyEvent>) {
        let (tx, rx) = async_channel::unbounded();
        (ProxyService::new(tx), rx)
    }

    /// Blocking echo server on 127.0.0.1 (returns its port).
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

    /// Minimal HTTP origin that answers with its own first request line as
    /// the body — lets the test prove the absolute-URI rewrite happened.
    fn origin_server() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut conn, _)) = listener.accept() {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match conn.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let first_line = String::from_utf8_lossy(&buf)
                    .split("\r\n")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let body = format!("seen:{first_line}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = conn.write_all(response.as_bytes());
            }
        });
        port
    }

    fn read_until_close(stream: &mut StdStream) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn parses_connect_and_absolute_forms() {
        assert_eq!(
            parse_head(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"),
            Ok(Request::Connect {
                host: "example.com".into(),
                port: 443
            })
        );
        assert_eq!(
            parse_head(b"CONNECT [2001:db8::1]:8443 HTTP/1.1\r\n\r\n"),
            Ok(Request::Connect {
                host: "2001:db8::1".into(),
                port: 8443
            })
        );
        assert_eq!(
            parse_head(b"GET http://example.com/a/b?x=1 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Ok(Request::Forward {
                host: "example.com".into(),
                port: 80,
                origin: "/a/b?x=1".into()
            })
        );
        assert_eq!(
            parse_head(b"GET http://example.com HTTP/1.1\r\n\r\n"),
            Ok(Request::Forward {
                host: "example.com".into(),
                port: 80,
                origin: "/".into()
            })
        );
        assert_eq!(
            parse_head(b"GET https://example.com:8443/ HTTP/1.1\r\n\r\n"),
            Ok(Request::Forward {
                host: "example.com".into(),
                port: 8443,
                origin: "/".into()
            })
        );
        assert!(parse_head(b"NOT-A-REQUEST\r\n\r\n").is_err());
        assert!(parse_head(b"GET /relative HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_head(b"GET ftp://host/x HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_head(b"CONNECT nohost HTTP/1.1\r\n\r\n").is_err());
        // Bracketed IPv6 authority, with and without an explicit port.
        assert_eq!(
            parse_head(b"GET http://[::1]/x HTTP/1.1\r\nHost: [::1]\r\n\r\n"),
            Ok(Request::Forward {
                host: "::1".into(),
                port: 80,
                origin: "/x".into()
            })
        );
        assert_eq!(
            parse_head(b"GET http://[fe80::1]:8080/x HTTP/1.1\r\n\r\n"),
            Ok(Request::Forward {
                host: "fe80::1".into(),
                port: 8080,
                origin: "/x".into()
            })
        );
    }

    #[test]
    fn forward_head_strips_proxy_headers_and_rewrites_target() {
        let head = b"GET http://example.com/p HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic zzz\r\nX-Keep: 1\r\n\r\n";
        let req = Request::Forward {
            host: "example.com".into(),
            port: 80,
            origin: "/p".into(),
        };
        let out = forward_head(head, &req).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("GET /p HTTP/1.1\r\n"), "{text}");
        assert!(text.contains("X-Keep: 1\r\n"), "{text}");
        assert!(!text.to_lowercase().contains("proxy-"), "{text}");
        assert!(text.ends_with("\r\n\r\n"), "{text}");
    }

    #[test]
    fn connect_tunnel_carries_bytes_both_ways() {
        let _port_guard = take_port_lock();
        let origin = echo_server();
        let port = free_port();
        let (svc, _rx) = service();
        svc.enable("http", &cfg_on(port), &bypass_routing())
            .expect("enable");

        let mut client =
            StdStream::connect(("127.0.0.1", port)).expect("connect to the proxy port");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .write_all(
                format!("CONNECT 127.0.0.1:{origin} HTTP/1.1\r\nHost: 127.0.0.1:{origin}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();

        // Read the 200 head, then everything else is tunnel payload.
        let mut buf = [0u8; 512];
        let mut got = Vec::new();
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = client.read(&mut buf).expect("read 200 response");
            assert!(n > 0, "proxy closed before answering CONNECT");
            got.extend_from_slice(&buf[..n]);
        }
        let head = String::from_utf8_lossy(&got);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "CONNECT must be accepted: {head:?}"
        );

        client.write_all(b"hello-http-connect").unwrap();
        client.read_exact(&mut buf[..18]).unwrap();
        assert_eq!(&buf[..18], b"hello-http-connect", "tunnel must echo");

        drop(client);
        svc.disable().expect("disable");
    }

    #[test]
    fn absolute_uri_request_is_forwarded_in_origin_form() {
        let _port_guard = take_port_lock();
        let origin = origin_server();
        let port = free_port();
        let (svc, _rx) = service();
        svc.enable("http2", &cfg_on(port), &bypass_routing())
            .expect("enable");

        let mut client =
            StdStream::connect(("127.0.0.1", port)).expect("connect to the proxy port");
        client
            .write_all(
                format!("GET http://127.0.0.1:{origin}/hello?q=1 HTTP/1.1\r\nHost: 127.0.0.1:{origin}\r\nProxy-Connection: keep-alive\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let response = read_until_close(&mut client);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.contains("seen:GET /hello?q=1 HTTP/1.1"),
            "origin must see the origin-form target: {response}"
        );

        svc.disable().expect("disable");
    }

    #[test]
    fn non_proxy_traffic_gets_400_and_is_not_swallowed() {
        let _port_guard = take_port_lock();
        let port = free_port();
        let (svc, _rx) = service();
        svc.enable("http3", &cfg_on(port), &bypass_routing())
            .expect("enable");

        let mut client =
            StdStream::connect(("127.0.0.1", port)).expect("connect to the proxy port");
        client
            .write_all(b"hello, not a proxy request\r\n\r\n")
            .unwrap();
        let response = read_until_close(&mut client);
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "garbage must be rejected, not hung: {response:?}"
        );

        svc.disable().expect("disable");
    }

    #[test]
    fn socks5_still_works_on_the_same_port_as_http() {
        // Sniffing must not disturb the SOCKS5 path (GOAL §11 B11).
        let _port_guard = take_port_lock();
        let origin = echo_server();
        let port = free_port();
        let (svc, _rx) = service();
        svc.enable("mixed", &cfg_on(port), &bypass_routing())
            .expect("enable");

        let mut s = StdStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(&[0x05, 0x01, 0x00]).unwrap();
        let mut ack = [0u8; 2];
        s.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [0x05, 0x00]);

        let mut req = vec![0x05u8, 0x01, 0x00, 0x01];
        req.extend_from_slice(&[127, 0, 0, 1]);
        req.extend_from_slice(&origin.to_be_bytes());
        s.write_all(&req).unwrap();
        let mut head = [0u8; 10];
        s.read_exact(&mut head[..4]).unwrap();
        assert_eq!(head[1], 0x00, "SOCKS5 CONNECT must still succeed");
        let tail = if head[3] == 0x01 { 6 } else { 0 };
        s.read_exact(&mut head[4..4 + tail]).unwrap();
        s.write_all(b"still-socks5").unwrap();
        let mut echo = [0u8; 12];
        s.read_exact(&mut echo).unwrap();
        assert_eq!(&echo, b"still-socks5");

        svc.disable().expect("disable");
    }

    /// Real-client check with the actual `curl` binary — the same shape as
    /// the Windows probe in GOAL Phase 8.3 (`curl -x http://…`), run here
    /// against a local origin. `#[ignore]` because it shells out; run with
    /// `cargo test http_proxy -- --ignored`.
    #[test]
    #[ignore = "shells out to curl"]
    fn curl_x_talks_to_the_http_front_end() {
        let _port_guard = take_port_lock();
        let origin = origin_server();
        let port = free_port();
        let (svc, _rx) = service();
        svc.enable("curl", &cfg_on(port), &bypass_routing())
            .expect("enable");

        let out = std::process::Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "10",
                "-x",
                &format!("http://127.0.0.1:{port}"),
                &format!("http://127.0.0.1:{origin}/via-curl"),
            ])
            .output()
            .expect("run curl");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "curl failed: {stderr}");
        assert!(
            stdout.contains("seen:GET /via-curl HTTP/1.1"),
            "curl must reach the origin through the proxy: {stdout} / {stderr}"
        );

        svc.disable().expect("disable");
    }
}
