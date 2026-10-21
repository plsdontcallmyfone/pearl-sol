//! The gateway's HTTP front (security pass V-7): a bounded number of requests in flight
//! (503 above it), a socket read timeout on every connection (headers and body: a silent
//! or slow-drip client is cut off), and 413 for bodies over the cap instead of silently
//! truncating them. `main.rs` serves `Gateway::handle` through it; tests serve a closure.

use std::io::Read;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use serde_json::json;

use crate::{HttpResponse, Payload};

/// Largest request body (4 MiB, one A blob).
pub const MAX_BODY: u64 = 4 << 20;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Requests handled at once; more answer 503 at once.
    pub max_in_flight: usize,
    /// Socket read timeout for every connection (set on the listener, inherited by
    /// accepted sockets): bounds header reads, body reads and idle keep-alive.
    pub read_timeout: Duration,
    pub max_body: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_in_flight: 64, read_timeout: Duration::from_secs(30), max_body: MAX_BODY }
    }
}

pub type Handler = dyn Fn(&str, &str, &[(String, String)], &[u8]) -> HttpResponse + Send + Sync;

// CORS: browsers (the Cadre site) call the gateway directly or through cadre-api
const CORS: [&str; 4] = [
    "Access-Control-Allow-Origin: *",
    "Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS",
    "Access-Control-Allow-Headers: content-type, authorization, x-payment, x-cadre-wallet, x-cadre-timestamp, x-cadre-signature",
    "Access-Control-Expose-Headers: x-cadre-request, x-cadre-ticket, x-cadre-order, x-cadre-rows, x-cadre-charged-lamports, x-cadre-receipt, x-cadre-explorer, x-payment-response",
];

/// Set SO_RCVTIMEO on the listening socket; accepted sockets inherit it (Linux and macOS),
/// which covers tiny_http's own header reads too. The same timeout also applies to accept()
/// itself, and tiny_http's accept thread stops on an accept error, so `bind` starts a ticker
/// that connects to the listener well inside the timeout (an empty connection tiny_http drops)
/// and `serve` returns if the listener stops anyway (main exits non-zero; systemd restarts it).
#[cfg(unix)]
pub fn set_read_timeout(l: &TcpListener, d: Duration) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let tv = libc::timeval { tv_sec: d.as_secs() as libc::time_t, tv_usec: d.subsec_micros() as libc::suseconds_t };
    // SAFETY: a valid fd and a correctly sized timeval.
    let r = unsafe {
        libc::setsockopt(l.as_raw_fd(), libc::SOL_SOCKET, libc::SO_RCVTIMEO, &tv as *const libc::timeval as *const libc::c_void, std::mem::size_of::<libc::timeval>() as libc::socklen_t)
    };
    if r == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
pub fn set_read_timeout(_l: &TcpListener, _d: Duration) -> std::io::Result<()> {
    Ok(())
}

/// Bind `addr` with the read timeout set; the tiny_http server on it, plus the accept ticker.
pub fn bind(addr: &str, limits: &Limits) -> std::io::Result<tiny_http::Server> {
    let l = TcpListener::bind(addr)?;
    set_read_timeout(&l, limits.read_timeout)?;
    let mut local = l.local_addr()?;
    if local.ip().is_unspecified() {
        local.set_ip(if local.is_ipv4() { std::net::Ipv4Addr::LOCALHOST.into() } else { std::net::Ipv6Addr::LOCALHOST.into() });
    }
    let srv = tiny_http::Server::from_listener(l, None).map_err(|e| std::io::Error::other(e.to_string()))?;
    // keep accept() from ever waiting a whole read timeout (a timed-out accept ends tiny_http's
    // accept thread): an empty local connection at most every timeout / 8, at least every 2 s
    let every = (limits.read_timeout / 8).clamp(Duration::from_millis(50), Duration::from_secs(2));
    std::thread::spawn(move || loop {
        std::thread::sleep(every);
        if let Ok(s) = std::net::TcpStream::connect_timeout(&local, Duration::from_secs(1)) {
            drop(s);
        }
    });
    Ok(srv)
}

/// Feeds an SSE channel to tiny_http as a Read.
struct ChanReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(b) => {
                    self.buf = b;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn simple(req: tiny_http::Request, status: u16, kind: &str, msg: &str) {
    let body = json!({"error": {"message": msg, "type": kind, "code": status}}).to_string();
    let mut resp = tiny_http::Response::from_string(body).with_status_code(status);
    resp = resp.with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
    resp = resp.with_header("Connection: close".parse::<tiny_http::Header>().unwrap());
    for c in CORS {
        resp = resp.with_header(c.parse::<tiny_http::Header>().unwrap());
    }
    let _ = req.respond(resp);
}

/// Releases one in-flight slot when the handler thread ends.
struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Serve until the server stops (returns then: the caller should exit non-zero). Each admitted
/// request runs on its own thread.
pub fn serve(server: tiny_http::Server, limits: Limits, handler: Arc<Handler>) {
    let in_flight = Arc::new(AtomicUsize::new(0));
    for req in server.incoming_requests() {
        if in_flight.fetch_add(1, Ordering::SeqCst) >= limits.max_in_flight {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            simple(req, 503, "overloaded", "too many requests in flight; retry shortly");
            continue;
        }
        let slot = Slot(in_flight.clone());
        let handler = handler.clone();
        std::thread::spawn(move || {
            let _slot = slot;
            handle_one(req, &limits, &*handler);
        });
    }
}

fn handle_one(mut req: tiny_http::Request, limits: &Limits, handler: &Handler) {
    let method = req.method().as_str().to_uppercase();
    if req.body_length().is_some_and(|n| n as u64 > limits.max_body) {
        return simple(req, 413, "payload_too_large", &format!("request body is over {} bytes", limits.max_body));
    }
    let mut body = Vec::new();
    match req.as_reader().take(limits.max_body + 1).read_to_end(&mut body) {
        Ok(_) if body.len() as u64 > limits.max_body => {
            return simple(req, 413, "payload_too_large", &format!("request body is over {} bytes", limits.max_body));
        }
        Ok(_) => {}
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
            return simple(req, 408, "request_timeout", "request body not received in time");
        }
        Err(_) => return simple(req, 400, "bad_request", "could not read the request body"),
    }
    let path = req.url().to_string();
    let headers: Vec<(String, String)> = req.headers().iter().map(|h| (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())).collect();
    if method == "OPTIONS" {
        let mut resp = tiny_http::Response::empty(204);
        for c in CORS {
            resp = resp.with_header(c.parse::<tiny_http::Header>().unwrap());
        }
        let _ = req.respond(resp);
        return;
    }
    let r = handler(&method, &path, &headers, &body);
    let mut hs: Vec<tiny_http::Header> = r.headers.iter().filter_map(|(k, v)| tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()).ok()).collect();
    for c in CORS {
        hs.push(c.parse().unwrap());
    }
    match r.body {
        Payload::Json(v) => {
            if !r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                hs.push("Content-Type: application/json".parse().unwrap());
            }
            let mut resp = tiny_http::Response::from_string(v.to_string()).with_status_code(r.status);
            for h in hs {
                resp = resp.with_header(h);
            }
            let _ = req.respond(resp);
        }
        Payload::Stream(rx) => {
            let resp = tiny_http::Response::new(tiny_http::StatusCode(r.status), hs, ChanReader { rx, buf: vec![], pos: 0 }, None, None);
            let _ = req.respond(resp);
        }
    }
}
