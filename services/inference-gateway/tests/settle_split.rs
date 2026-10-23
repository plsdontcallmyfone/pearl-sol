//! Settlement split (box 0, 2026-10-23): request data waits for its round, and rounds used to
//! wait behind ticket closes, whose evaluator replay takes minutes (480-565 s for 1,663-1,732
//! rows on box 0). Closes now run on their own pass: round posting goes on while one is
//! attested. Also: served rows a ticket can no longer post (deadline passed, closed on the
//! chain) are dropped and their data deleted instead of waiting forever, and early posting
//! starts at `data_pressure_bytes` whatever the cap.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::{data, standing};
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwsplit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn model() -> ModelInfo {
    let mut md = ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "test-model".into(), base: MiningConfig::localnet_default(),
        matrices: vec![(2048, 16), (2048, 8)], hash_b: vec![], macs_per_row: 2048 * 24 };
    let b = bts(&md);
    md.hash_b = (0..2).map(|i| standing::commit_bt(&md.job_key(i as u16), &b[i])).collect();
    md
}

fn bts(model: &ModelInfo) -> Vec<Vec<i8>> {
    model.matrices.iter().enumerate().map(|(i, (k, n))| {
        let (_, b) = data::random_matrices(1, *n as usize, *k as usize, Some(i as u64));
        pearl_job::job::transpose(&b, *k as usize, *n as usize)
    }).collect()
}

/// A storefront that hands out a new ticket id per paid order.
struct FakeSf(AtomicUsize);
impl Storefront for FakeSf {
    fn post_job(&self, _body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        Ok(match x {
            None => (402, json!({"x402Version": 1, "accepts": [{"maxAmountRequired": "409600"}]}), None),
            Some(_) => {
                let n = self.0.fetch_add(1, Ordering::SeqCst) as u8 + 1;
                (201, json!({"id": n.to_string(), "job_id": hex::encode([n; 32]), "status": "pending",
                    "payment": {"payer": bs58::encode(SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()).into_string(), "amount": "409600"}}), None)
            }
        })
    }
}

struct FakeWorker(ModelInfo, Vec<Vec<i8>>);
impl Worker for FakeWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn chat_ids(&self, _m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path, _o: &GenOpts, _on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        let rows = ids.len() as u32 + max_new - 1;
        std::fs::create_dir_all(out.join("a"))?;
        std::fs::create_dir_all(out.join("c"))?;
        for (i, (k, n)) in self.0.matrices.iter().enumerate() {
            let (a, _) = data::random_matrices(rows as usize, 1, *k as usize, Some(rows as u64 * 100 + i as u64));
            let c = pearl_job::freivalds::matmul_bt(&a, &self.1[i], rows as usize, *n as usize, *k as usize);
            std::fs::write(out.join(format!("a/{i}.i8")), pearl_job::job::as_u8(&a))?;
            std::fs::write(out.join(format!("c/{i}.i32")), pearl_job::freivalds::c_to_bytes(&c))?;
        }
        Ok(json!({"output_ids": vec![5; max_new as usize], "text": "ok", "rows": rows, "rows_padded": rows,
            "prompt_tokens": ids.len(), "completion_tokens": max_new, "finish_reason": "length", "glue": "cadre-glue-v1", "timing": {}}))
    }
}

/// An evaluator that holds every attestation until released (a replay that takes minutes).
struct SlowEval {
    release: Arc<AtomicBool>,
    entered: Arc<AtomicUsize>,
}
impl Evaluator for SlowEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let t = Instant::now();
        while !self.release.load(Ordering::SeqCst) {
            anyhow::ensure!(t.elapsed() < Duration::from_secs(20), "never released");
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(([7; 32], [0; 64]))
    }
}

/// MockNode plus the settlement reads; the tip and the tickets' deadline are settable.
struct ChainNode {
    node: MockNode,
    tip: Arc<AtomicU64>,
    deadline: u64,
}
impl pearl_node::NodeApi for ChainNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<pearl_node::Template> {
        self.node.block_template(payout)
    }
    fn submit_block(&self, t: &pearl_node::Template, b: &pearl_job::pow::MinedBlock) -> Result<pearl_node::SubmitOutcome> {
        self.node.submit_block(t, b)
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<pearl_node::JobInfo>> {
        self.node.get_job(id)
    }
    fn get_account(&self, a: &[u8; 32]) -> Result<pearl_node::Account> {
        self.node.get_account(a)
    }
    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        self.node.send_tx(wire)
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.node.get_blob(id)
    }
    fn put_blob(&self, d: &[u8]) -> Result<String> {
        self.node.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "getParams" => Ok(json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": 1000})),
            "getTicket" => Ok(json!({"standing": {"rounds": 0}, "deadline_height": self.deadline, "max_blocks": 100_000})),
            "getChainInfo" => Ok(json!({"chain_id": 1, "height": self.tip.load(Ordering::SeqCst)})),
            "getTx" => Ok(json!({"status": "included"})),
            _ => self.node.call(method, p),
        }
    }
}

struct Env {
    gw: Arc<Gateway>,
    release: Arc<AtomicBool>,
    entered: Arc<AtomicUsize>,
    tip: Arc<AtomicU64>,
}

fn env(tag: &str, deadline: u64, cfg: impl FnOnce(&mut Config)) -> Env {
    let md = model();
    let (release, entered, tip) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicU64::new(10)));
    let mut c = Config { work_dir: tmp(tag), ticket_wait: Duration::from_secs(60), poll: Duration::from_millis(10), balance_rows: 1024, max_new_tokens: 32,
        min_free_disk_bytes: 0, round_every: Duration::from_millis(0), settle_backoff: Duration::from_millis(0), ..Config::default() };
    cfg(&mut c);
    let node = ChainNode { node: MockNode::new(1, 0x207fffff), tip: tip.clone(), deadline };
    let gw = Arc::new(Gateway::new(Box::new(node), Box::new(FakeSf(AtomicUsize::new(0))), Box::new(FakeWorker(md.clone(), bts(&md))),
        Box::new(SlowEval { release: release.clone(), entered: entered.clone() }), SigningKey::from_bytes(&[3; 32]), md, c, Box::new(|_| {})));
    Env { gw, release, entered, tip }
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

fn topup(gw: &Arc<Gateway>) -> String {
    let r = gw.handle("POST", "/v1/balance/topup", &[("X-PAYMENT".into(), "pay".into())], br#"{"rows": 1024}"#);
    assert_eq!(r.status, 201, "{}", r.json());
    r.json()["ticket_id"].as_str().unwrap().to_string()
}

/// One served completion of 6 rows; returns the request id.
fn complete(gw: &Arc<Gateway>, i: u32) -> String {
    let buyer = SigningKey::from_bytes(&[5; 32]);
    let b = format!(r#"{{"prompt": "p{i}", "max_tokens": 4}}"#).into_bytes();
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", &b), &b);
    assert_eq!(r.status, 200, "{}", r.json());
    r.headers.iter().find(|(k, _)| k == "x-cadre-request").map(|(_, v)| v.clone()).unwrap()
}

fn dir(gw: &Gateway, id: &str) -> PathBuf {
    gw.cfg.work_dir.join("requests").join(id)
}

#[test]
fn rounds_post_while_a_close_waits_for_the_evaluator() {
    let e = env("split", 1_000_000, |_| {});
    let gw = &e.gw;
    // ticket A: one round posted, then the wallet closes it
    let a = topup(gw);
    let r1 = complete(gw, 1);
    gw.post_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].rounds.len(), 1);
    assert!(!dir(gw, &r1).exists());
    gw.ledger.lock().unwrap().tickets.get_mut(&a).unwrap().state = "closing".into();
    // ticket B: a request served after A started closing
    let b = topup(gw);
    assert_ne!(a, b);
    let r2 = complete(gw, 2);
    assert_eq!(gw.ledger.lock().unwrap().requests[&r2].ticket_id, b);
    // the close thread enters the evaluator and stays there
    let g2 = Arc::clone(gw);
    let closer = std::thread::spawn(move || g2.close_pass());
    let t = Instant::now();
    while e.entered.load(Ordering::SeqCst) == 0 {
        assert!(t.elapsed() < Duration::from_secs(10), "the close never reached the evaluator");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(gw.stats_json()["settle"]["closing"]["ticket"], a, "stats name the ticket being closed");
    // meanwhile B's round posts and its data goes: posting does not wait for A's attestation
    gw.post_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&b].rounds.len(), 1, "B posted while A was being attested");
    assert!(!dir(gw, &r2).exists(), "B's data deleted at once");
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].state, "closing", "A still waits for its evaluator");
    e.release.store(true, Ordering::SeqCst);
    closer.join().unwrap();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].state, "completed");
    assert!(gw.stats_json()["settle"]["closing"].is_null());
    // the counters measure the backlog: everything written was freed
    let rd = &gw.stats_json()["request_data"];
    assert!(rd["written_bytes"].as_u64().unwrap() > 0);
    assert_eq!(rd["written_bytes"], rd["freed_bytes"]);
    assert_eq!(rd["written_bytes"].as_u64().unwrap(), 12 * gw.bytes_per_row(), "two 6-row requests, measured on disk");
}

#[test]
fn served_rows_past_the_tickets_deadline_are_dropped_with_their_data() {
    let e = env("expired", 100, |c| c.round_every = Duration::from_secs(3600));
    let gw = &e.gw;
    let a = topup(gw);
    let r1 = complete(gw, 1);
    gw.post_pass();
    assert!(dir(gw, &r1).exists(), "before the deadline the data waits for its round");
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].pending.len(), 1);
    // the chain passes the deadline: PostSegments would be refused forever
    e.tip.store(101, Ordering::SeqCst);
    gw.post_pass();
    let l = gw.ledger.lock().unwrap();
    let t = &l.tickets[&a];
    assert!(t.pending.is_empty());
    assert_eq!(t.state, "failed", "no round was ever posted");
    assert!(t.error.as_deref().unwrap().contains("deadline"), "{:?}", t.error);
    assert!(l.requests.contains_key(&r1), "the receipt stays");
    drop(l);
    assert!(!dir(gw, &r1).exists(), "its data is deleted");
    assert_eq!(gw.request_data_bytes(), 0);
    assert_eq!(gw.stats_json()["request_data"]["pending_dropped"], 1);
}

#[test]
fn early_posting_starts_at_the_pressure_threshold_not_half_a_raised_cap() {
    let bpr = 2048 + 4 * 16 + 2048 + 4 * 8;
    // a cap raised far above the waiting data (the stopgap) and a pressure threshold of 10 rows
    let e = env("pressure", 1_000_000, |c| {
        c.round_every = Duration::from_secs(3600);
        c.max_request_data_bytes = 1 << 40;
        c.data_pressure_bytes = 10 * bpr;
    });
    let gw = &e.gw;
    assert_eq!(gw.pressure_bytes(), 10 * bpr);
    let a = topup(gw);
    complete(gw, 1);
    gw.post_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].rounds.len(), 0, "6 rows waiting: under the threshold, the round period rules");
    complete(gw, 2);
    gw.post_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&a].rounds.len(), 1, "12 rows waiting: posted early");
    assert_eq!(gw.request_data_bytes(), 0);

    // without the threshold the old rule holds: half of the (huge) cap
    let e2 = env("pressure0", 1_000_000, |c| {
        c.round_every = Duration::from_secs(3600);
        c.max_request_data_bytes = 1 << 40;
        c.data_pressure_bytes = 0;
    });
    assert_eq!(e2.gw.pressure_bytes(), 1 << 39);
    let a2 = topup(&e2.gw);
    complete(&e2.gw, 1);
    complete(&e2.gw, 2);
    e2.gw.post_pass();
    assert_eq!(e2.gw.ledger.lock().unwrap().tickets[&a2].rounds.len(), 0);
    let _ = e.tip;
}
