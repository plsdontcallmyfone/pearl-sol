//! Node blob backpressure (SPEC 5.4 "Request data retention", 2026-10-22 blob store full on
//! devnet box 0): a paid request whose activation rows (plus every row waiting for a round)
//! would not fit in the node's blob headroom (getBlobStats rpc_headroom_bytes) answers 503
//! insufficient_storage before any rows are reserved; a settle pass the node refuses with
//! "blob store nearly full" refuses new paid work at once; an older node without the field
//! falls back to max_total_bytes - total_bytes; a node without getBlobStats never blocks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::{data, standing};
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwnb-{tag}-{}", std::process::id()));
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

struct FakeSf;
impl Storefront for FakeSf {
    fn post_job(&self, _body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        Ok(match x {
            None => (402, json!({"x402Version": 1, "accepts": [{"maxAmountRequired": "409600"}]}), None),
            Some(_) => (201, json!({"id": "1", "job_id": hex::encode([9u8; 32]), "status": "pending",
                "payment": {"payer": bs58::encode(SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()).into_string(), "amount": "409600"}}), None),
        })
    }
}

/// Writes real A and C = A B, like the worker with pad false; counts calls.
struct FakeWorker(ModelInfo, Vec<Vec<i8>>, Arc<AtomicUsize>);
impl Worker for FakeWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn chat_ids(&self, _m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path, _o: &GenOpts, _on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        self.2.fetch_add(1, Ordering::SeqCst);
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

struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

/// MockNode plus the reads settlement makes, and a blob store with `headroom` bytes of RPC room
/// (u64::MAX: no getBlobStats at all, like an older node API; `legacy`: the old fields only).
struct ChainNode(MockNode, Arc<AtomicU64>, bool);
impl pearl_node::NodeApi for ChainNode {
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
        let h = self.1.load(Ordering::SeqCst);
        if h != u64::MAX {
            anyhow::ensure!(d.len() as u64 <= h, "putBlob: blob store nearly full (1 of 2 bytes used; RPC puts stop at 2 so the rest stays for blobs the chain references); retry after the prune pass frees room");
            self.1.store(h - d.len() as u64, Ordering::SeqCst);
        }
        self.0.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "getParams" => Ok(json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": 1000})),
            "getTicket" => Ok(json!({"standing": {"rounds": 0}, "deadline_height": 1_000_000, "max_blocks": 100_000})),
            "getChainInfo" => Ok(json!({"chain_id": 1, "height": 10})),
            "getTx" => Ok(json!({"status": "included"})),
            "getBlobStats" => {
                let h = self.1.load(Ordering::SeqCst);
                anyhow::ensure!(h != u64::MAX, "unknown method getBlobStats");
                if self.2 {
                    Ok(json!({"total_bytes": 1_000_000_000u64 - h, "max_total_bytes": 1_000_000_000u64}))
                } else {
                    Ok(json!({"rpc_headroom_bytes": h, "total_bytes": 5, "max_total_bytes": 1_000_000_000u64}))
                }
            }
            _ => self.0.call(method, p),
        }
    }
}

fn gateway(tag: &str, headroom: u64, legacy: bool, cfg: impl FnOnce(&mut Config)) -> (Arc<Gateway>, Arc<AtomicUsize>, Arc<AtomicU64>) {
    let room = Arc::new(AtomicU64::new(headroom));
    let md = model();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut c = Config { work_dir: tmp(tag), ticket_wait: Duration::from_secs(60), poll: Duration::from_millis(10), balance_rows: 1024, max_new_tokens: 32,
        min_free_disk_bytes: 0, ..Config::default() };
    cfg(&mut c);
    let gw = Arc::new(Gateway::new(Box::new(ChainNode(MockNode::new(1, 0x207fffff), room.clone(), legacy)), Box::new(FakeSf), Box::new(FakeWorker(md.clone(), bts(&md), calls.clone())),
        Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, c, Box::new(|_| {})));
    (gw, calls, room)
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

/// One served completion (3 prompt rows + 4 tokens - 1 = 6 rows); returns (status, request id).
fn complete(gw: &Arc<Gateway>, i: u32) -> (u16, Value, Option<String>) {
    let buyer = SigningKey::from_bytes(&[5; 32]);
    let b = format!(r#"{{"prompt": "p{i}", "max_tokens": 4}}"#).into_bytes();
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", &b), &b);
    let id = r.headers.iter().find(|(k, _)| k == "x-cadre-request").map(|(_, v)| v.clone());
    (r.status, r.json().clone(), id)
}



#[test]
fn a_nearly_full_node_blob_store_is_a_503_before_anything_is_reserved() {
    // a request is 6 rows; A is 2 x 2048 bytes per row = 24,576 bytes
    let (gw, calls, room) = gateway("full", 20_000, false, |c| c.round_every = Duration::from_secs(3600));
    assert_eq!(gw.a_bytes_per_row(), 4096);
    let ticket = topup(&gw);
    let (st, v, _) = complete(&gw, 1);
    assert_eq!(st, 503, "{v}");
    assert_eq!(v["error"]["type"], "insufficient_storage");
    assert!(v["error"]["message"].as_str().unwrap().contains("blob store is nearly full"), "{v}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "never reached the worker");
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].reserved, 0, "nothing reserved");
    assert_eq!(gw.stats_json()["request_data"]["refused_node_storage"], 1);
    // 20,000 less the balance descriptor blob the top-up put
    let h = gw.stats_json()["request_data"]["node_blob_headroom_bytes"].as_u64().unwrap();
    assert!(h < 20_000 && h > 19_000, "{h}");
    // the node prunes: room for one request (the read is cached for NODE_BLOB_TTL, so start over)
    room.store(30_000, Ordering::SeqCst);
    let (gw, calls, _room) = gateway("full2", 30_000, false, |c| c.round_every = Duration::from_secs(3600));
    topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
    // the waiting round's rows count: a second request would need 12 rows of A = 49,152 bytes
    let (st, v, _) = complete(&gw, 2);
    assert_eq!(st, 503, "{v}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_settle_refused_for_space_blocks_new_paid_work_at_once() {
    // room for the first request's check (24,576) but not for its round's A blobs on the node
    let (gw, _, room) = gateway("settle", 25_000, false, |c| c.round_every = Duration::from_millis(0));
    let ticket = topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
    room.store(100, Ordering::SeqCst); // the node filled up meanwhile (the cached read still says 25,000)
    gw.settle_pass();
    assert!(gw.ledger.lock().unwrap().tickets[&ticket].rounds.is_empty(), "the round could not be posted");
    let (st, v, _) = complete(&gw, 2);
    assert_eq!(st, 503, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("nearly full"), "{v}");
    assert!(is_blob_store_full("putBlob: blob store full (7999925132 of 8000000000 bytes used)"));
}

#[test]
fn an_older_node_uses_quota_minus_total_and_no_blob_stats_never_blocks() {
    let (gw, _, _) = gateway("legacy", 10_000, true, |c| c.round_every = Duration::from_secs(3600));
    assert_eq!(gw.node_blob_headroom(), Some(10_000));
    topup(&gw);
    assert_eq!(complete(&gw, 1).0, 503);
    let (gw, _, _) = gateway("none", u64::MAX, false, |c| c.round_every = Duration::from_secs(3600));
    assert_eq!(gw.node_blob_headroom(), None);
    topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
}
