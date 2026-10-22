//! Inference backpressure (SPEC 5.4 "Worker concurrency", security pass V-7): at most
//! `max_inference` requests at the worker; one more answers 503 with Retry-After before any rows
//! are reserved, and a worker whose queue is full (its own 503) is a 503 too, with the
//! reservation released. Concurrent requests reach the worker together (the batching worker
//! serves them in one batch), not one at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::standing;
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwbp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn model() -> ModelInfo {
    ModelInfo {
        model_id: standing::model_id(&[1; 32], b"m"),
        name: "shape-model".into(),
        base: MiningConfig::localnet_default(),
        matrices: vec![(2048, 8); 4],
        hash_b: vec![[0; 32]; 4],
        macs_per_row: 2048 * 8 * 4,
    }
}

struct FakeSf;
impl Storefront for FakeSf {
    fn post_job(&self, body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        let mb = body["spec"]["max_blocks"].as_u64().unwrap_or(0);
        Ok(match x {
            None => (402, json!({"x402Version": 1, "accepts": [{"maxAmountRequired": "409600"}]}), None),
            Some(_) => (201, json!({"id": "1", "job_id": hex::encode([mb as u8; 32]), "status": "pending",
                "payment": {"payer": bs58::encode(SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()).into_string(), "amount": "409600"}}), None),
        })
    }
}

/// Holds every request until `gate` releases it; counts how many are inside at once.
struct SlowWorker {
    m: ModelInfo,
    inside: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    gate: Arc<Barrier>,
    busy: bool,
}
impl Worker for SlowWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn chat_ids(&self, _m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path, _o: &GenOpts, _on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        if self.busy {
            anyhow::bail!("{}: {{\"error\": \"busy: 64 requests waiting\"}}", transport::WORKER_BUSY);
        }
        let n = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(n, Ordering::SeqCst);
        self.gate.wait();
        let rows = ids.len() as u32 + max_new - 1;
        std::fs::create_dir_all(out.join("a"))?;
        std::fs::create_dir_all(out.join("c"))?;
        for (i, (k, n)) in self.m.matrices.iter().enumerate() {
            std::fs::write(out.join(format!("a/{i}.i8")), vec![0u8; rows as usize * *k as usize])?;
            std::fs::write(out.join(format!("c/{i}.i32")), vec![0u8; rows as usize * *n as usize * 4])?;
        }
        self.inside.fetch_sub(1, Ordering::SeqCst);
        Ok(json!({"output_ids": vec![5; max_new as usize], "text": "ok", "rows": rows, "rows_padded": rows,
            "prompt_tokens": ids.len(), "completion_tokens": max_new, "finish_reason": "length", "timing": {}}))
    }
}

struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

struct ParamsNode(MockNode);
impl pearl_node::NodeApi for ParamsNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<pearl_node::Template> {
        self.0.block_template(payout)
    }
    fn submit_block(&self, t: &pearl_node::Template, b: &pearl_job::pow::MinedBlock) -> Result<pearl_node::SubmitOutcome> {
        self.0.submit_block(t, b)
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<pearl_node::JobInfo>> {
        self.0.get_job(id)
    }
    fn get_account(&self, a: &[u8; 32]) -> Result<pearl_node::Account> {
        self.0.get_account(a)
    }
    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        self.0.send_tx(wire)
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.0.get_blob(id)
    }
    fn put_blob(&self, d: &[u8]) -> Result<String> {
        self.0.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        if method == "getParams" {
            return Ok(json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": 1000}));
        }
        self.0.call(method, p)
    }
}

fn signed(sk: &SigningKey, method: &str, path: &str, body: &[u8]) -> Vec<(String, String)> {
    let ts = now_ms();
    let msg = format!("cadre-auth-v1\n{method}\n{path}\n{ts}\n{}", hex::encode(<sha2::Sha256 as sha2::Digest>::digest(body)));
    let sig = sk.sign(msg.as_bytes());
    vec![
        ("X-Cadre-Wallet".into(), bs58::encode(sk.verifying_key().to_bytes()).into_string()),
        ("X-Cadre-Timestamp".into(), ts.to_string()),
        ("X-Cadre-Signature".into(), hex::encode(sig.to_bytes())),
    ]
}

fn gateway(tag: &str, max_inference: usize, w: SlowWorker) -> Arc<Gateway> {
    let md = model();
    let cfg = Config { work_dir: tmp(tag), ticket_wait: Duration::from_millis(200), poll: Duration::from_millis(50), balance_rows: 1024, max_new_tokens: 32,
        max_inference, ..Config::default() };
    Arc::new(Gateway::new(Box::new(ParamsNode(MockNode::new(1, 0x207fffff))), Box::new(FakeSf), Box::new(w), Box::new(NoEval),
        SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})))
}

fn topup(gw: &Arc<Gateway>) -> String {
    let r = gw.handle("POST", "/v1/balance/topup", &[("X-PAYMENT".into(), "pay".into())], br#"{"rows": 1024}"#);
    assert_eq!(r.status, 201, "{}", r.json());
    r.json()["ticket_id"].as_str().unwrap().to_string()
}

#[test]
fn requests_over_the_cap_get_503_before_any_rows_are_reserved_and_the_rest_reach_the_worker_together() {
    let inside = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    // 3 requests are admitted (the cap) and wait at the worker until the test releases them
    let gate = Arc::new(Barrier::new(4));
    let gw = gateway("cap", 3, SlowWorker { m: model(), inside: inside.clone(), peak: peak.clone(), gate: gate.clone(), busy: false });
    let ticket = topup(&gw);
    let buyer = SigningKey::from_bytes(&[5; 32]);
    // distinct bodies: the gateway accepts each signature once (security pass V-3)
    let body = |i: u32| format!(r#"{{"prompt": "x{i}", "max_tokens": 4}}"#).into_bytes();
    let mut hs = vec![];
    for i in 0..3 {
        let gw = gw.clone();
        let b = body(i);
        let h = signed(&buyer, "POST", "/v1/completions", &b);
        hs.push(std::thread::spawn(move || {
            let r = gw.handle("POST", "/v1/completions", &h, &b);
            (r.status, r.json().clone())
        }));
    }
    let t0 = std::time::Instant::now();
    while inside.load(Ordering::SeqCst) < 3 {
        assert!(t0.elapsed() < Duration::from_secs(30), "3 requests never reached the worker together");
        std::thread::sleep(Duration::from_millis(10));
    }
    let reserved = gw.ledger.lock().unwrap().tickets[&ticket].reserved;
    assert_eq!(reserved, 3 * 6, "3 admitted requests of 3 + 4 - 1 rows each");
    // the 4th: 503 overloaded with Retry-After, nothing more reserved
    let b4 = body(3);
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", &b4), &b4);
    assert_eq!(r.status, 503, "{}", r.json());
    assert_eq!(r.json()["error"]["type"], "overloaded");
    assert!(r.headers.iter().any(|(k, v)| k == "retry-after" && v == "2"));
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].reserved, reserved);
    assert_eq!(gw.stats_json()["inference"]["at_worker"], 3);
    gate.wait();
    for h in hs {
        let (st, v) = h.join().unwrap();
        assert_eq!(st, 200, "{v}");
    }
    assert_eq!(peak.load(Ordering::SeqCst), 3, "the admitted requests were at the worker at the same time");
    assert_eq!(gw.stats_json()["inference"]["at_worker"], 0, "slots are freed when requests end");
    assert_eq!(gw.stats_json()["inference"]["refused_busy"], 1);
    // after the batch ends a new request is admitted again
    let g2 = gate.clone();
    let gw2 = gw.clone();
    let b5 = body(4);
    let h = signed(&buyer, "POST", "/v1/completions", &b5);
    let t = std::thread::spawn(move || gw2.handle("POST", "/v1/completions", &h, &b5).status);
    let t0 = std::time::Instant::now();
    while inside.load(Ordering::SeqCst) < 1 {
        assert!(t0.elapsed() < Duration::from_secs(30), "the next request never reached the worker");
        std::thread::sleep(Duration::from_millis(10));
    }
    // release with three extra parties standing in for the other barrier slots
    let extra: Vec<_> = (0..2).map(|_| {
        let g = g2.clone();
        std::thread::spawn(move || {
            g.wait();
        })
    }).collect();
    g2.wait();
    for e in extra {
        e.join().unwrap();
    }
    assert_eq!(t.join().unwrap(), 200);
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].reserved, 0);
}

#[test]
fn a_full_worker_queue_is_a_503_with_the_reservation_released() {
    let w = SlowWorker { m: model(), inside: Arc::new(AtomicUsize::new(0)), peak: Arc::new(AtomicUsize::new(0)), gate: Arc::new(Barrier::new(1)), busy: true };
    let gw = gateway("wbusy", 32, w);
    let ticket = topup(&gw);
    let buyer = SigningKey::from_bytes(&[5; 32]);
    let body = br#"{"prompt": "x", "max_tokens": 4}"#;
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", body), body);
    assert_eq!(r.status, 503, "{}", r.json());
    assert!(r.json()["error"]["message"].as_str().unwrap().contains(transport::WORKER_BUSY));
    assert!(r.headers.iter().any(|(k, _)| k == "retry-after"));
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].reserved, 0);
    assert_eq!(gw.stats_json()["inference"]["at_worker"], 0);
}
