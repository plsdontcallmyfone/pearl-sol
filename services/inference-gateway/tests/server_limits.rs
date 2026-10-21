//! Security pass V-7: the gateway's HTTP front caps requests in flight (503), cuts off
//! silent or slow clients with a socket read timeout (headers and body), and answers 413
//! for bodies over the cap instead of truncating them.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use std::time::{Duration, Instant};

use serde_json::json;

use cadre_gateway::server::{self, Limits};
use cadre_gateway::{HttpResponse, Payload};

/// A server on a free port with `limits`; the handler echoes the body length after `delay`.
fn start(limits: Limits, delay: Duration) -> (String, Arc<AtomicUsize>) {
    let srv = server::bind("127.0.0.1:0", &limits).unwrap();
    let addr = srv.server_addr().to_ip().unwrap().to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    std::thread::spawn(move || {
        server::serve(
            srv,
            limits,
            Arc::new(move |_m: &str, _p: &str, _h: &[(String, String)], b: &[u8]| {
                c2.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(delay);
                HttpResponse { status: 200, body: Payload::Json(json!({"len": b.len()})), headers: vec![] }
            }),
        )
    });
    (addr, calls)
}

/// Send raw bytes, return the status code of the reply (0 if the server closed without one).
fn raw(addr: &str, req: &[u8], wait: Duration) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(wait)).unwrap();
    s.write_all(req).unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.windows(4).any(|w| w == b"\r\n\r\n") && String::from_utf8_lossy(&out).contains("}") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let code = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, text)
}

#[test]
fn small_request_is_served() {
    let (addr, calls) = start(Limits::default(), Duration::ZERO);
    let (code, text) = raw(&addr, b"POST /v1/x HTTP/1.1\r\nHost: a\r\nContent-Length: 2\r\n\r\n{}", Duration::from_secs(5));
    assert_eq!(code, 200, "{text}");
    assert!(text.contains("\"len\":2"), "{text}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn body_over_the_cap_is_413_not_truncated() {
    let limits = Limits { max_body: 1024, ..Limits::default() };
    let (addr, calls) = start(limits, Duration::ZERO);
    // declared too large: refused before reading
    let (code, _) = raw(&addr, b"POST /v1/x HTTP/1.1\r\nHost: a\r\nContent-Length: 5000\r\n\r\n", Duration::from_secs(5));
    assert_eq!(code, 413);
    // chunked, no length: refused once the cap is passed
    let mut req = b"POST /v1/x HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..3 {
        req.extend_from_slice(b"400\r\n");
        req.extend_from_slice(&[b'a'; 1024]);
        req.extend_from_slice(b"\r\n");
    }
    req.extend_from_slice(b"0\r\n\r\n");
    let (code, _) = raw(&addr, &req, Duration::from_secs(5));
    assert_eq!(code, 413);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the handler never saw an oversized body");
}

#[test]
fn requests_over_the_in_flight_cap_get_503() {
    let limits = Limits { max_in_flight: 2, ..Limits::default() };
    let (addr, _) = start(limits, Duration::from_millis(1500));
    let codes = Arc::new(Mutex::new(vec![]));
    let go = Arc::new(Barrier::new(4));
    let mut hs = vec![];
    for _ in 0..4 {
        let (a, c, g) = (addr.clone(), codes.clone(), go.clone());
        hs.push(std::thread::spawn(move || {
            g.wait();
            let (code, _) = raw(&a, b"GET /v1/x HTTP/1.1\r\nHost: a\r\n\r\n", Duration::from_secs(10));
            c.lock().unwrap().push(code);
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let mut v = codes.lock().unwrap().clone();
    v.sort();
    assert_eq!(v, vec![200, 200, 503, 503], "{v:?}");
}

#[test]
fn the_listener_outlives_idle_periods_longer_than_the_read_timeout() {
    // SO_RCVTIMEO also times out accept(); the ticker keeps tiny_http's accept thread alive
    let limits = Limits { read_timeout: Duration::from_secs(1), ..Limits::default() };
    let (addr, _) = start(limits, Duration::ZERO);
    std::thread::sleep(Duration::from_millis(3500));
    let (code, text) = raw(&addr, b"GET /v1/x HTTP/1.1\r\nHost: a\r\n\r\n", Duration::from_secs(5));
    assert_eq!(code, 200, "{text}");
}

#[test]
fn a_silent_client_is_cut_off_by_the_read_timeout() {
    let limits = Limits { read_timeout: Duration::from_secs(1), ..Limits::default() };
    let (addr, calls) = start(limits, Duration::ZERO);
    // headers promise a body that never comes: 408 (or a close) within the timeout, not a hang
    let t = Instant::now();
    let (code, _) = raw(&addr, b"POST /v1/x HTTP/1.1\r\nHost: a\r\nContent-Length: 100\r\n\r\n{", Duration::from_secs(10));
    assert!(code == 408 || code == 0, "got {code}");
    assert!(t.elapsed() < Duration::from_secs(5), "took {:?}", t.elapsed());
    // half a request line, then silence (slowloris on the headers): the socket is closed
    let t = Instant::now();
    let (code, _) = raw(&addr, b"GET /v1/x HTT", Duration::from_secs(10));
    assert!(code == 408 || code == 400 || code == 0, "got {code}");
    assert!(t.elapsed() < Duration::from_secs(5), "took {:?}", t.elapsed());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
