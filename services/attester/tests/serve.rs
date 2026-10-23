//! pearl-attester serving (audit follow-up 2026-10-23): /health answers while a /result is being
//! handled. Box 0 2026-10-23 07:33-08:47 UTC: the health check FAILed "timed out reading
//! response" because one loop served both and a glue replay takes 480-565 s; the queued /health
//! requests were answered in the same second the /result finished (attester journal).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use pearl_attester::{serve, ServeStats};

#[test]
fn health_answers_while_a_result_is_being_replayed() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let stats = Arc::new(ServeStats::default());
    let st = Arc::clone(&stats);
    std::thread::spawn(move || {
        serve(server, 1, || json!({"ok": true, "mode": "node"}), |m, p, _b| {
            if m == "POST" && p == "/result" {
                std::thread::sleep(Duration::from_millis(1500)); // a replay
                return (200, json!({"ok": true}));
            }
            (404, json!({"ok": false}))
        }, st, false)
    });
    let base = format!("http://{addr}");
    let b2 = base.clone();
    let slow = std::thread::spawn(move || {
        let t0 = Instant::now();
        let r = ureq::post(&format!("{b2}/result")).send_string("{}").unwrap();
        assert_eq!(r.status(), 200);
        t0.elapsed()
    });
    let t = Instant::now();
    while stats.running.load(Ordering::SeqCst) == 0 {
        assert!(t.elapsed() < Duration::from_secs(5), "the result never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(500)).build();
    let t0 = Instant::now();
    let h: serde_json::Value = agent.get(&format!("{base}/health")).call().expect("health must not wait for the replay").into_json().unwrap();
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(h["ok"], true);
    assert_eq!(h["busy"]["running"], 1, "{h}");
    assert!(h["busy"]["oldest_running_s"].is_u64());
    // a second result queues behind the first (one worker), in arrival order
    let b3 = base.clone();
    let second = std::thread::spawn(move || ureq::post(&format!("{b3}/result")).send_string("{}").unwrap().status());
    let t = Instant::now();
    while stats.queued.load(Ordering::SeqCst) == 0 && stats.done.load(Ordering::SeqCst) == 0 {
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    let h: serde_json::Value = agent.get(&format!("{base}/health")).call().unwrap().into_json().unwrap();
    assert!(h["busy"]["queued"].as_u64().unwrap() + h["busy"]["done"].as_u64().unwrap() >= 1, "{h}");
    assert!(slow.join().unwrap() >= Duration::from_millis(1500));
    assert_eq!(second.join().unwrap(), 200);
    assert_eq!(stats.done.load(Ordering::SeqCst), 2);
    let r = ureq::get(&format!("{base}/nope")).call();
    assert!(matches!(r, Err(ureq::Error::Status(404, _))));
}
