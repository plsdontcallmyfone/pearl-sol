//! Request data retention (SPEC 5.4 "Request data retention", box 0 disk-full incident
//! 2026-10-22): a request's A/C scratch files are deleted as soon as its round is posted; a dead
//! ticket's and an orphan's are swept; a paid request that would pass the data cap or the disk
//! reserve answers 503 insufficient_storage before any rows are reserved; over half the cap the
//! waiting rounds are posted early instead of waiting for the round period. Since the audit
//! follow-up of 2026-10-23 a posted round's data is kept until its PostSegments are final.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwret-{tag}-{}", std::process::id()));
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

/// MockNode plus the reads settlement makes: the ticket is on the chain, and getTx answers the
/// test's value for every tx (default final; "included" or null to test retention until final).
struct ChainNode(MockNode, Arc<std::sync::Mutex<Value>>);
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
        self.0.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "getParams" => Ok(json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": 1000})),
            "getTicket" => Ok(json!({"standing": {"rounds": 0}, "deadline_height": 1_000_000, "max_blocks": 100_000})),
            "getChainInfo" => Ok(json!({"chain_id": 1, "height": 10})),
            "getTx" => Ok(self.1.lock().unwrap().clone()),
            _ => self.0.call(method, p),
        }
    }
}

fn gateway(tag: &str, cfg: impl FnOnce(&mut Config)) -> (Arc<Gateway>, Arc<AtomicUsize>) {
    let (gw, calls, _) = gateway_tx(tag, json!({"status": "final", "final": true}), cfg);
    (gw, calls)
}

/// A gateway whose node answers getTx with the returned handle's value.
fn gateway_tx(tag: &str, tx: Value, cfg: impl FnOnce(&mut Config)) -> (Arc<Gateway>, Arc<AtomicUsize>, Arc<std::sync::Mutex<Value>>) {
    let tx = Arc::new(std::sync::Mutex::new(tx));
    let md = model();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut c = Config { work_dir: tmp(tag), ticket_wait: Duration::from_secs(60), poll: Duration::from_millis(10), balance_rows: 1024, max_new_tokens: 32,
        min_free_disk_bytes: 0, ..Config::default() };
    cfg(&mut c);
    let gw = Arc::new(Gateway::new(Box::new(ChainNode(MockNode::new(1, 0x207fffff), tx.clone())), Box::new(FakeSf), Box::new(FakeWorker(md.clone(), bts(&md), calls.clone())),
        Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, c, Box::new(|_| {})));
    (gw, calls, tx)
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

fn req_dir(gw: &Gateway, id: &str) -> PathBuf {
    gw.cfg.work_dir.join("requests").join(id)
}

#[test]
fn a_posted_round_deletes_its_requests_data() {
    let (gw, _) = gateway("posted", |c| c.round_every = Duration::from_millis(0));
    let ticket = topup(&gw);
    let (s1, _, id1) = complete(&gw, 1);
    let (s2, _, id2) = complete(&gw, 2);
    assert_eq!((s1, s2), (200, 200));
    let (id1, id2) = (id1.unwrap(), id2.unwrap());
    assert!(req_dir(&gw, &id1).join("a/0.i8").exists() && req_dir(&gw, &id2).join("c/1.i32").exists(), "served data waits for its round");
    assert_eq!(gw.request_data_bytes(), 12 * gw.bytes_per_row());
    assert_eq!(gw.bytes_per_row(), 2048 + 4 * 16 + 2048 + 4 * 8);
    gw.settle_pass();
    let l = gw.ledger.lock().unwrap();
    assert_eq!(l.tickets[&ticket].rounds.len(), 1, "the round was posted");
    assert_eq!(l.requests[&id1].round, Some(0));
    drop(l);
    assert!(!req_dir(&gw, &id1).exists() && !req_dir(&gw, &id2).exists(), "posted: A is on the node, C committed, scratch deleted");
    assert_eq!(gw.request_data_bytes(), 0);
    assert_eq!(gw.stats_json()["request_data"]["deleted"], 2);
}

#[test]
fn a_full_data_cap_or_disk_reserve_is_a_503_before_anything_is_reserved() {
    let bpr = 2048 + 4 * 16 + 2048 + 4 * 8;
    // room for one request of 6 rows waiting for its round, not two (the round period is long)
    let (gw, calls) = gateway("cap", |c| {
        c.round_every = Duration::from_secs(3600);
        c.max_request_data_bytes = 10 * bpr;
    });
    let ticket = topup(&gw);
    assert_eq!(complete(&gw, 1).0, 200);
    let (st, v, _) = complete(&gw, 2);
    assert_eq!(st, 503, "{v}");
    assert_eq!(v["error"]["type"], "insufficient_storage");
    assert!(v["error"]["message"].as_str().unwrap().contains("cap"), "{v}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the refused request never reached the worker");
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].reserved, 0);
    assert_eq!(gw.stats_json()["request_data"]["refused_storage"], 1);
    // over half the cap: the next pass posts the round early (the period is an hour), frees the
    // data, and the refused request goes through on retry
    gw.settle_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].rounds.len(), 1, "posted early under data pressure");
    assert_eq!(gw.request_data_bytes(), 0);
    assert_eq!(complete(&gw, 3).0, 200);

    // the disk reserve: a gateway that must keep more free than any disk has refuses at once
    let (gw2, calls2) = gateway("disk", |c| c.min_free_disk_bytes = u64::MAX / 4);
    topup(&gw2);
    let (st, v, _) = complete(&gw2, 1);
    assert_eq!(st, 503, "{v}");
    assert_eq!(v["error"]["type"], "insufficient_storage");
    assert!(v["error"]["message"].as_str().unwrap().contains("disk"), "{v}");
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
    assert!(disk_free_bytes(&gw2.cfg.work_dir).unwrap() > 0, "statvfs reads the work dir's filesystem");
}

#[test]
fn the_sweep_removes_dead_tickets_data_and_old_orphans_only() {
    let (gw, _) = gateway("sweep", |c| c.round_every = Duration::from_secs(3600));
    let ticket = topup(&gw);
    let (_, _, id1) = complete(&gw, 1);
    let id1 = id1.unwrap();
    // a fresh orphan (a request still being booked, or one that crashed a moment ago) is kept
    let orphan = gw.cfg.work_dir.join("requests").join("req_orphan");
    std::fs::create_dir_all(orphan.join("a")).unwrap();
    assert_eq!(gw.sweep_request_data(), 0, "pending data of an open ticket and a fresh orphan stay");
    assert!(req_dir(&gw, &id1).exists() && orphan.exists());
    // the ticket ends without posting (failed, completed elsewhere): its waiting data goes
    gw.ledger.lock().unwrap().tickets.get_mut(&ticket).unwrap().state = "completed".into();
    assert_eq!(gw.sweep_request_data(), 1);
    assert!(!req_dir(&gw, &id1).exists());
    assert!(orphan.exists(), "a fresh orphan is still within its grace");
}

/// Audit follow-up 2026-10-23: a posted round's data is kept until its last PostSegments is
/// final (finality_k blocks), counted in the cap but not in data pressure, then deleted.
#[test]
fn a_posted_round_keeps_its_data_until_final() {
    let bpr = 2048 + 4 * 16 + 2048 + 4 * 8;
    let (gw, _, tx) = gateway_tx("final", json!({"status": "included", "final": false}), |c| {
        c.round_every = Duration::from_millis(0);
        c.max_request_data_bytes = 16 * bpr;
    });
    let ticket = topup(&gw);
    let (_, _, id1) = complete(&gw, 1);
    let (_, _, id2) = complete(&gw, 2);
    let (id1, id2) = (id1.unwrap(), id2.unwrap());
    gw.settle_pass();
    {
        let l = gw.ledger.lock().unwrap();
        assert_eq!(l.tickets[&ticket].rounds.len(), 1, "posted");
        assert!(l.tickets[&ticket].rounds[0].awaiting_final);
    }
    assert!(req_dir(&gw, &id1).join("c/0.i32").exists() && req_dir(&gw, &id2).join("a/1.i8").exists(), "included, not final: kept");
    assert_eq!(gw.request_data_bytes(), 0, "nothing waits for a round (no data pressure from it)");
    assert_eq!(gw.final_wait_bytes(), 12 * bpr);
    let st = gw.stats_json();
    assert_eq!(st["request_data"]["bytes_awaiting_final"], 12 * bpr);
    assert_eq!(st["request_data"]["rounds_awaiting_final"], 1);
    assert_eq!(st["request_data"]["delete_on"], "final");
    // the sweep keeps it too, and the cap counts it: 12 kept + 6 new rows > 16
    assert_eq!(gw.sweep_request_data(), 0);
    let (s3, v3, _) = complete(&gw, 3);
    assert_eq!(s3, 503, "{v3}");
    assert_eq!(v3["error"]["type"], "insufficient_storage");
    // final: the next pass deletes it, and the cap has room again
    *tx.lock().unwrap() = json!({"status": "final", "final": true});
    gw.settle_pass();
    assert!(!req_dir(&gw, &id1).exists() && !req_dir(&gw, &id2).exists(), "final: deleted");
    assert_eq!(gw.final_wait_bytes(), 0);
    assert!(!gw.ledger.lock().unwrap().tickets[&ticket].rounds[0].awaiting_final);
    let st = gw.stats_json();
    assert_eq!(st["request_data"]["rounds_final"], 1);
    assert_eq!(st["request_data"]["deleted"], 2);
    assert_eq!(complete(&gw, 4).0, 200);
}

/// A reorg that drops a posted round's txs (neither in a block nor pooled on three checks in a
/// row) puts the round back, and the next pass posts it again from the kept data with the same
/// requests; one miss alone (a tx read between disconnect and reinsert) changes nothing.
#[test]
fn a_round_dropped_by_a_reorg_is_posted_again_from_the_kept_data() {
    let (gw, _, tx) = gateway_tx("reorg", json!({"status": "included", "final": false}), |c| c.round_every = Duration::from_millis(0));
    let ticket = topup(&gw);
    let (_, _, id1) = complete(&gw, 1);
    let (_, _, id2) = complete(&gw, 2);
    let (id1, id2) = (id1.unwrap(), id2.unwrap());
    gw.settle_pass();
    let first = gw.ledger.lock().unwrap().tickets[&ticket].rounds[0].clone();
    assert_eq!(first.requests, vec![id1.clone(), id2.clone()]);
    *tx.lock().unwrap() = Value::Null;
    gw.finality_pass();
    *tx.lock().unwrap() = json!({"status": "pending"}); // reinserted into the pool
    gw.finality_pass();
    *tx.lock().unwrap() = Value::Null;
    gw.finality_pass();
    gw.finality_pass();
    assert_eq!(gw.ledger.lock().unwrap().tickets[&ticket].rounds.len(), 1, "two misses in a row are not yet a drop");
    gw.finality_pass();
    {
        let l = gw.ledger.lock().unwrap();
        let t = &l.tickets[&ticket];
        assert!(t.rounds.is_empty(), "the dropped round left the ledger");
        assert_eq!(t.pending, vec![id1.clone(), id2.clone()]);
        assert_eq!(t.posting.as_ref().map(|p| (p.round, p.m, p.requests.clone())), Some((0, first.m, first.requests.clone())));
        assert_eq!(l.requests[&id1].round, None);
    }
    assert!(req_dir(&gw, &id1).exists() && req_dir(&gw, &id2).exists(), "the data is still there");
    assert_eq!(gw.stats_json()["request_data"]["rounds_reorged"], 1);
    // the chain answers again: the round is posted again, same rows, then final and deleted
    *tx.lock().unwrap() = json!({"status": "included", "final": false});
    gw.settle_pass();
    {
        let l = gw.ledger.lock().unwrap();
        let t = &l.tickets[&ticket];
        assert_eq!(t.rounds.len(), 1);
        assert_eq!((t.rounds[0].round, t.rounds[0].m, &t.rounds[0].requests), (0, first.m, &first.requests));
        assert_eq!(t.rounds[0].txs.len(), first.txs.len(), "every segment sent again");
        assert!(t.pending.is_empty() && t.posting.is_none());
    }
    *tx.lock().unwrap() = json!({"status": "final", "final": true});
    gw.settle_pass();
    assert!(!req_dir(&gw, &id1).exists());
}

/// --delete-on-post: the behaviour before the change (delete as soon as the round is included).
#[test]
fn delete_on_post_deletes_at_inclusion() {
    let (gw, _, _) = gateway_tx("onpost", json!({"status": "included", "final": false}), |c| {
        c.round_every = Duration::from_millis(0);
        c.delete_on_post = true;
    });
    let ticket = topup(&gw);
    let (_, _, id1) = complete(&gw, 1);
    gw.settle_pass();
    assert!(!gw.ledger.lock().unwrap().tickets[&ticket].rounds[0].awaiting_final);
    assert!(!req_dir(&gw, &id1.unwrap()).exists());
    assert_eq!(gw.stats_json()["request_data"]["delete_on"], "posted");
}
