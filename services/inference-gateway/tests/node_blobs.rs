//! Node blob backpressure (SPEC 5.4 "Request data retention", 2026-10-22 blob store full on
//! devnet box 0): a paid request whose activation rows (plus every row waiting for a round)
//! would not fit in the node's blob headroom (getBlobStats rpc_headroom_bytes) answers 503
//! insufficient_storage before any rows are reserved; a settle pass the node refuses with
//! "blob store nearly full" refuses new paid work at once; an older node without the field
//! falls back to max_total_bytes - total_bytes; a node without getBlobStats never blocks.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    model_of(vec![(2048, 16), (2048, 8)])
}

fn model_of(matrices: Vec<(u32, u32)>) -> ModelInfo {
    let macs = matrices.iter().map(|(k, n)| *k as u64 * *n as u64).sum();
    let mut md = ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "test-model".into(), base: MiningConfig::localnet_default(),
        matrices, hash_b: vec![], macs_per_row: macs };
    let b = bts(&md);
    md.hash_b = (0..md.matrices.len()).map(|i| standing::commit_bt(&md.job_key(i as u16), &b[i])).collect();
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

/// Writes real A and C = A B, like the worker with pad false; counts calls. Matrices with the
/// same input group (the 4th field, one per matrix) get byte-identical A, like Llama's gate and
/// up projections or the lm_head column shards.
struct FakeWorker(ModelInfo, Vec<Vec<i8>>, Arc<AtomicUsize>, Vec<u64>);
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
            let (a, _) = data::random_matrices(rows as usize, 1, *k as usize, Some(rows as u64 * 100 + self.3[i]));
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
/// Content-addressed like the node's: a blob it already holds costs no room again.
struct ChainNode(MockNode, Arc<AtomicU64>, bool, Mutex<HashSet<[u8; 32]>>);
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
        if h != u64::MAX && !self.3.lock().unwrap().contains(&data::blob_id(d)) {
            anyhow::ensure!(d.len() as u64 <= h, "putBlob: blob store nearly full (1 of 2 bytes used; RPC puts stop at 2 so the rest stays for blobs the chain references); retry after the prune pass frees room");
            self.1.store(h - d.len() as u64, Ordering::SeqCst);
            self.3.lock().unwrap().insert(data::blob_id(d));
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
    gateway_with(tag, model(), vec![0, 1], headroom, legacy, cfg)
}

fn gateway_with(tag: &str, md: ModelInfo, groups: Vec<u64>, headroom: u64, legacy: bool, cfg: impl FnOnce(&mut Config)) -> (Arc<Gateway>, Arc<AtomicUsize>, Arc<AtomicU64>) {
    let room = Arc::new(AtomicU64::new(headroom));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut c = Config { work_dir: tmp(tag), ticket_wait: Duration::from_secs(60), poll: Duration::from_millis(10), balance_rows: 1024, max_new_tokens: 32,
        min_free_disk_bytes: 0, ..Config::default() };
    cfg(&mut c);
    let gw = Arc::new(Gateway::new(Box::new(ChainNode(MockNode::new(1, 0x207fffff), room.clone(), legacy, Mutex::new(HashSet::new()))), Box::new(FakeSf), Box::new(FakeWorker(md.clone(), bts(&md), calls.clone(), groups)),
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
    // a request is 6 rows, posted padded to the row period 4: m 8, two A blobs of 12 + 8 x 2048
    // bytes = 32,792 bytes
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
    room.store(40_000, Ordering::SeqCst);
    let (gw, calls, _room) = gateway("full2", 40_000, false, |c| c.round_every = Duration::from_secs(3600));
    topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
    assert_eq!(gw.node_blob_need(0), 32_792);
    // the waiting round counts: with a second request it would be 32,792 + 32,792 = 65,584 bytes
    let (st, v, _) = complete(&gw, 2);
    assert_eq!(st, 503, "{v}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_settle_refused_for_space_blocks_new_paid_work_at_once() {
    // room for the first request's check (32,792) but not for its round's A blobs on the node
    let (gw, _, room) = gateway("settle", 35_000, false, |c| c.round_every = Duration::from_millis(0));
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

/// Model with shared inputs: matrices 0 and 1 read the same activations (gate/up), matrix 2 a
/// narrower one. Per row: upper bound 2048 + 2048 + 1024 = 5,120 bytes, real blobs 2048 + 1024.
fn shared() -> (ModelInfo, Vec<u64>) {
    (model_of(vec![(2048, 16), (2048, 8), (1024, 8)]), vec![0, 0, 2])
}

#[test]
fn the_estimate_is_the_bytes_the_round_puts_on_the_node() {
    let (md, g) = shared();
    let (gw, _, room) = gateway_with("exact", md, g, 10_000_000, false, |c| {
        c.round_every = Duration::from_secs(3600);
        c.settle_backoff = Duration::ZERO;
    });
    assert_eq!(gw.a_bytes_per_row(), 5_120, "layout unknown: one row per matrix");
    let ticket = topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
    assert_eq!(complete(&gw, 2).0, 200);
    // learned from the served request's A files: two distinct blobs
    assert_eq!(gw.a_bytes_per_row(), 3_072);
    assert_eq!(gw.stats_json()["request_data"]["a_blobs_per_round"], 2);
    // 12 rows in one round, m 12: 2 x 12 header + 12 x 3,072 = 36,888 (the old per-matrix
    // estimate: 12 x 5,120 = 61,440)
    let before = gw.node_blob_need(0);
    assert_eq!(before, 36_888);
    assert_eq!(gw.stats_json()["request_data"]["node_blob_need_bytes"], 36_888);
    // what the node's store actually grows by when the round posts
    let r0 = room.load(Ordering::SeqCst);
    gw.ledger.lock().unwrap().tickets.get_mut(&ticket).unwrap().round_secs = Some(0);
    gw.post_pass();
    let t = gw.ledger.lock().unwrap().tickets[&ticket].clone();
    assert_eq!(t.rounds.len(), 1, "{t:?}");
    assert_eq!(t.rounds[0].m, 12);
    assert_eq!(r0 - room.load(Ordering::SeqCst), before, "estimate == bytes put");
    assert_eq!(gw.node_blob_need(0), 0, "a posted round needs nothing more");
}

#[test]
fn a_half_posted_round_counts_only_the_blobs_still_to_put() {
    let (md, g) = shared();
    // before any request is served the layout is unknown: the check takes the per-matrix bound
    // (3 blobs, 40,996 bytes for m 8)
    let (gw, _, room) = gateway_with("half", md, g, 50_000, false, |c| {
        c.round_every = Duration::from_millis(0);
        c.settle_backoff = Duration::ZERO;
    });
    let ticket = topup(&gw);
    // 6 rows, m 8: blob of matrix 0/1 12 + 8 x 2048 = 16,396, blob of matrix 2 12 + 8 x 1024 = 8,204
    assert_eq!(complete(&gw, 1).0, 200);
    assert_eq!(gw.node_blob_need(0), 24_600);
    // the node takes the first blob, then is full
    room.store(20_000, Ordering::SeqCst);
    gw.post_pass();
    let t = gw.ledger.lock().unwrap().tickets[&ticket].clone();
    assert!(t.rounds.is_empty() && t.posting.is_some(), "{t:?}");
    assert_eq!(room.load(Ordering::SeqCst), 20_000 - 16_396);
    // the blob already on the node is not counted again while the round's rows are pending
    assert_eq!(gw.node_blob_need(0), 8_204);
    room.store(1_000_000, Ordering::SeqCst);
    gw.post_pass();
    let t = gw.ledger.lock().unwrap().tickets[&ticket].clone();
    assert_eq!(t.rounds.len(), 1, "{t:?}");
    assert_eq!(1_000_000 - room.load(Ordering::SeqCst), 8_204, "the resume put only the missing blob");
    assert_eq!(gw.node_blob_need(0), 0);
}

fn served(id: &str, ticket: &str, rows: u32) -> Served {
    serde_json::from_value(json!({"request_id": id, "wallet": "w", "ticket_id": ticket, "dir": "/nonexistent", "rows": rows, "prompt_tokens": 1,
        "completion_tokens": 1, "body_sha256": "", "output_sha256": "", "charged_lamports": null, "at_ms": 0, "round": null, "row_offset": null,
        "ttft_s": 0.0, "decode_tokens_per_s": null})).unwrap()
}

fn ticket(id: &str, pending: &[&str]) -> TicketState {
    serde_json::from_value(json!({"ticket_id": id, "order": null, "wallet": "w", "rows": 8192, "lamports_per_row": null, "reserved": 0,
        "pending": pending, "rounds": [], "state": "open", "single": false, "created_ms": 0})).unwrap()
}

/// Recorded on devnet box 0, 2026-10-23 ~10:40 UTC (gateway ledger.json, read only): the
/// pending rows of the three open tickets with served rows waiting ([355], [411] and
/// [346, 278, 286, 325, 396]); every request dir there has 144 A files of which 113 are
/// distinct (md5), k 2048 each; posted rounds are padded to the row period 4 (pad 0-3 rows over
/// 337 rounds); round cap (4 MiB - 12) / 2048 rounded down to 4 = 2044 rows. getBlobStats
/// segment_a 35,349,877,236 bytes in 27,007 blobs = 27,007 x 12 + 2048 x 17,260,524 exactly,
/// the `12 + m x k` layout.
#[test]
fn box0_recorded_backlog() {
    let ks = vec![2048u32; 113];
    assert_eq!(ks.iter().map(|k| *k as u64).sum::<u64>(), 231_424);
    assert_eq!(a_round_bytes(&ks, 236), 113 * (12 + 236 * 2048));
    assert_eq!(27_007u64 * 12 + 2048 * 17_260_524, 35_349_877_236);
    let mut l = Ledger::default();
    let rows: [(&str, &[u32]); 3] = [("09c7", &[355]), ("1c92", &[411]), ("a069", &[346, 278, 286, 325, 396])];
    for (t, rs) in rows {
        let ids: Vec<String> = (0..rs.len()).map(|i| format!("{t}-{i}")).collect();
        for (i, r) in rs.iter().enumerate() {
            l.requests.insert(ids[i].clone(), served(&ids[i], t, *r));
        }
        l.tickets.insert(t.into(), ticket(t, &ids.iter().map(|s| s.as_str()).collect::<Vec<_>>()));
    }
    let waiting = waiting_a_bytes(&l, &ks, 4, 2044, &BTreeMap::new());
    // rounds m 356, 412, 1632 (1,631 rows fit one round): 3 x 113 x 12 + 2,400 x 231,424
    assert_eq!(waiting, 3 * 113 * 12 + 2_400 * 231_424);
    assert_eq!(waiting, 555_421_668);
    // the old estimate: 2,397 rows x 144 x 2048
    assert_eq!(2_397u64 * 294_912, 706_904_064);
    // a round half put (as the posting thread leaves it): only the rest counts
    let mut posting = ticket("a069", &["a069-0", "a069-1", "a069-2", "a069-3", "a069-4"]);
    posting.posting = Some(PostingRound { round: 3, m: 1632, requests: posting.pending.clone() });
    l.tickets.insert("a069".into(), posting);
    let mut put = BTreeMap::new();
    put.insert("a069".to_string(), (3u16, 60 * (12 + 1632 * 2048)));
    assert_eq!(waiting_a_bytes(&l, &ks, 4, 2044, &put), waiting - 60 * (12 + 1632 * 2048));
    // a stale entry for another round counts nothing off
    put.insert("a069".to_string(), (2u16, 1 << 30));
    assert_eq!(waiting_a_bytes(&l, &ks, 4, 2044, &put), waiting);
}
