//! Reference evaluator (docs/EVALUATORS.md) on a mock node: accept -> CompleteJob,
//! provable failure -> RejectResult (exact bytes), transient problems -> hold, threshold
//! jobs collect a peer's signature, verdicts published once per delivery.

use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use ed25519_dalek::{Signature, SigningKey, Verifier};
use serde_json::{json, Value};

use pearl_attester::evaluator::{reject_body, reject_message, Decision, EvalConfig, Evaluator, Verdict, KIND_REJECT_RESULT};
use pearl_attester::{Attester, JobSource};
use pearl_job::freivalds::{matmul, result_hash};
use pearl_job::messages::result_message;
use pearl_job::{data, JobHeader, MiningConfig};
use pearl_node::mock::MockNode;
use pearl_job::pow::MinedBlock;
use pearl_node::{tx as ntx, Account, JobInfo, NodeApi, SubmitOutcome, Template};

const EASY: u32 = 0x1e040000;
const CHAIN: u32 = 7;

/// MockNode plus the RPC reads the evaluator makes (listJobs, getChainInfo, getParams).
#[derive(Clone)]
struct Node {
    inner: MockNode,
    height: Arc<Mutex<u64>>,
}

impl NodeApi for Node {
    fn block_template(&self, p: &[u8; 32]) -> Result<Template> {
        self.inner.block_template(p)
    }
    fn submit_block(&self, t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        self.inner.submit_block(t, b)
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<JobInfo>> {
        self.inner.get_job(id)
    }
    fn get_account(&self, a: &[u8; 32]) -> Result<Account> {
        self.inner.get_account(a)
    }
    fn send_tx(&self, w: &[u8]) -> Result<String> {
        self.inner.send_tx(w)
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.inner.get_blob(id)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "getChainInfo" => Ok(json!({"chain_id": CHAIN, "height": *self.height.lock().unwrap()})),
            "getParams" => Ok(json!({"min_fee": "1000"})),
            "listJobs" => {
                assert_eq!(p["status"], "delivered");
                let jobs: Vec<Value> = self.inner.state.lock().unwrap().jobs.iter().filter(|j| j.is("Delivered")).map(|j| j.to_json()).collect();
                Ok(json!({"total": jobs.len(), "jobs": jobs}))
            }
            m => bail!("mock has no {m}"),
        }
    }
}

struct Fx {
    node: Node,
    job_id: [u8; 32],
    deliverer: [u8; 32],
    c: Vec<i32>,
    key: [u8; 32],
    dir: std::path::PathBuf,
}

/// A 64x64x2048 job with real data on disk, delivered as `c_override` (or the true C).
fn fixture(tag: &str, evaluators: Vec<[u8; 32]>, threshold: u8) -> Fx {
    let (m, n, k) = (64u32, 64u32, 2048u32);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("evaluator-{tag}"));
    let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, Some(17));
    let mp = data::save(&dir, m, n, k, &a, &b).unwrap();
    let job_id = [0x31u8; 32];
    let config = MiningConfig::localnet_default();
    let header = JobHeader { job_pubkey: job_id, terms_hash: [0x33; 32], created_slot: 5, target_bits: EASY };
    let key = pearl_job::job_key(&header, &config);
    let (ha, hb) = pearl_job::commit(&key, &a, &b, k as usize, n as usize);
    let c = matmul(&a, &b, 64, 64, 2048);
    let inner = MockNode::new(CHAIN, EASY);
    let deliverer = [0x66; 32];
    let cpath = dir.join("C.i32");
    data::write_i32(&cpath, &c).unwrap();
    inner.add_job(JobInfo {
        job_id,
        header_id: job_id,
        poster: [1; 32],
        terms_hash: [0x33; 32],
        created_height: 5,
        share_target: EASY,
        config,
        m,
        n,
        hash_a: ha,
        hash_b: hb,
        data_uri: format!("file://{}", mp.display()),
        status: "Delivered".into(),
        deliverer: Some(deliverer),
        result_hash: Some(result_hash(&key, &c)),
        result_uri: format!("file://{}", cpath.display()),
        evaluators,
        threshold,
        deadline_height: 1000,
        kind: "job".into(),
        standing: None,
    });
    Fx { node: Node { inner, height: Arc::new(Mutex::new(10)) }, job_id, deliverer, c, key, dir }
}

fn sk(i: u8) -> SigningKey {
    SigningKey::from_bytes(&[i; 32])
}
fn pk(i: u8) -> [u8; 32] {
    sk(i).verifying_key().to_bytes()
}

fn evaluator(node: &Node, i: u8, cfg: EvalConfig) -> Evaluator {
    let att = Attester { key: sk(i), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 2, models_dir: "models".into() };
    Evaluator::new(att, cfg).unwrap()
}

fn fund(node: &Node, who: [u8; 32]) {
    node.inner.state.lock().unwrap().accounts.insert(who, Account { balance: 1_000_000, nonce: 3 });
}

fn mempool(node: &Node) -> Vec<(ntx::Tx, [u8; 64])> {
    node.inner.state.lock().unwrap().mempool.iter().map(|w| ntx::decode(w).unwrap()).collect()
}

fn write_c(fx: &Fx, c: &[i32]) {
    data::write_i32(&fx.dir.join("C.i32"), c).unwrap();
}

#[test]
fn accepts_a_correct_delivery_and_sends_complete_job() {
    let fx = fixture("accept", vec![pk(5)], 1);
    fund(&fx.node, pk(5));
    let ev = evaluator(&fx.node, 5, EvalConfig::default());
    let job = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    assert_eq!(ev.check(&job, 10), Verdict::Accept);
    let out = ev.round().unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].decision, Decision::Accept);
    assert!(out[0].tx_hash.is_some());
    let txs = mempool(&fx.node);
    assert_eq!(txs.len(), 1);
    let (t, _) = &txs[0];
    assert_eq!((t.chain_id, t.from, t.nonce, t.fee), (CHAIN, pk(5), 3, 1000));
    let rh = result_hash(&fx.key, &fx.c);
    let msg = result_message(&fx.job_id, &fx.deliverer, &rh);
    match &t.body {
        ntx::Body::CompleteJob { job_id, sigs } => {
            assert_eq!(*job_id, fx.job_id);
            assert_eq!(sigs.len(), 1);
            assert_eq!(sigs[0].0, pk(5));
            sk(5).verifying_key().verify(&msg, &Signature::from_bytes(&sigs[0].1)).unwrap();
        }
        b => panic!("expected CompleteJob, got {b:?}"),
    }
    // once per delivery: the next pass sends nothing
    assert!(ev.round().unwrap().is_empty());
    assert_eq!(mempool(&fx.node).len(), 1);
    // published
    let (st, v) = ev.route("GET", &format!("/signatures/{}", hex::encode(fx.job_id)), b"");
    assert_eq!(st, 200);
    assert_eq!(v[0]["decision"], "accept");
    assert_eq!(ev.route("GET", "/health", b"").1["mode"], "evaluator");
}

#[test]
fn rejects_a_wrong_c_with_exact_reject_result_bytes() {
    let fx = fixture("reject", vec![pk(5)], 1);
    fund(&fx.node, pk(5));
    // The deliverer committed to (and published) a wrong C: hashes match, Freivalds fails.
    let mut bad = fx.c.clone();
    bad[7] += 1;
    write_c(&fx, &bad);
    let bad_rh = result_hash(&fx.key, &bad);
    fx.node.inner.state.lock().unwrap().jobs[0].result_hash = Some(bad_rh);
    let ev = evaluator(&fx.node, 5, EvalConfig::default());
    let out = ev.round().unwrap();
    assert_eq!(out[0].decision, Decision::Reject);
    assert!(out[0].reason.contains("Freivalds"), "{}", out[0].reason);
    let txs = mempool(&fx.node);
    let (t, _) = &txs[0];
    let msg = reject_message(&fx.job_id, &fx.deliverer, &bad_rh);
    let mut expect_msg = b"pearl-sol:reject:v1".to_vec();
    expect_msg.extend_from_slice(&fx.job_id);
    expect_msg.extend_from_slice(&fx.deliverer);
    expect_msg.extend_from_slice(&bad_rh);
    assert_eq!(msg, expect_msg);
    match &t.body {
        ntx::Body::Raw { kind, body } => {
            assert_eq!(*kind, KIND_REJECT_RESULT);
            let sig: [u8; 64] = body[129..193].try_into().unwrap();
            sk(5).verifying_key().verify(&msg, &Signature::from_bytes(&sig)).unwrap();
            assert_eq!(*body, reject_body(&fx.job_id, &fx.deliverer, &bad_rh, &[(pk(5), sig)]));
            assert_eq!(body.len(), 32 * 3 + 1 + 96);
        }
        b => panic!("expected RejectResult, got {b:?}"),
    }
}

#[test]
fn rejects_c_that_does_not_hash_to_result_hash_and_wrong_sizes() {
    let fx = fixture("hash", vec![pk(5)], 1);
    let ev = evaluator(&fx.node, 5, EvalConfig { submit: false, ..Default::default() });
    let mut j = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    j.result_hash = Some([9; 32]);
    assert!(matches!(ev.check(&j, 10), Verdict::Reject(r) if r.contains("result_hash")));
    write_c(&fx, &fx.c[..100]);
    let j = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    assert!(matches!(ev.check(&j, 10), Verdict::Reject(r) if r.contains("bytes")));
}

#[test]
fn holds_when_it_is_not_the_deliverers_fault_and_rejects_unavailable_after_grace() {
    let fx = fixture("hold", vec![pk(5)], 1);
    fund(&fx.node, pk(5));
    let ev = evaluator(&fx.node, 5, EvalConfig { unavailable_blocks: 5, ..Default::default() });
    // poster's data gone: hold forever
    let mut j = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    j.data_uri = "file:///nonexistent/manifest.json".into();
    assert!(matches!(ev.check(&j, 10), Verdict::Hold(r) if r.contains("job data")));
    // poster's data does not match its commitments: hold
    let mut j2 = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    j2.hash_a = [0; 32];
    assert!(matches!(ev.check(&j2, 10), Verdict::Hold(r) if r.contains("poster")));
    // the deliverer's blob is missing: hold, then reject after 5 blocks
    fx.node.inner.state.lock().unwrap().jobs[0].result_uri = format!("blob:{}", hex::encode([0xabu8; 32]));
    assert!(ev.round().unwrap().is_empty());
    assert!(mempool(&fx.node).is_empty());
    *fx.node.height.lock().unwrap() = 15;
    let out = ev.round().unwrap();
    assert_eq!(out[0].decision, Decision::Reject);
    assert!(out[0].reason.contains("unavailable"), "{}", out[0].reason);
}

#[test]
fn skips_jobs_that_do_not_name_it_and_tickets_are_never_auto_rejected() {
    let fx = fixture("skip", vec![pk(6)], 1);
    let ev = evaluator(&fx.node, 5, EvalConfig::default());
    assert!(ev.round().unwrap().is_empty());
    let mut j = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    j.kind = "ticket".into();
    assert!(matches!(ev.check(&j, 10), Verdict::Hold(_)));
}

#[test]
fn threshold_two_collects_a_peer_signature() {
    let fx = fixture("peer", vec![pk(5), pk(6)], 2);
    fund(&fx.node, pk(5));
    // Peer 6 runs its own evaluator without submitting; its HTTP is served on a free port.
    let peer = Arc::new(evaluator(&fx.node, 6, EvalConfig { submit: false, ..Default::default() }));
    let mine = peer.round().unwrap();
    assert_eq!(mine.len(), 1);
    assert!(mine[0].tx_hash.is_none());
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let p2 = peer.clone();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let path = req.url().to_string();
            let (st, v) = p2.route("GET", &path, b"");
            let _ = req.respond(tiny_http::Response::from_string(v.to_string()).with_status_code(st));
        }
    });
    // alone, 5 cannot meet the threshold
    let solo = evaluator(&fx.node, 5, EvalConfig::default());
    let out = solo.round().unwrap();
    assert!(out[0].tx_hash.is_none());
    assert!(mempool(&fx.node).is_empty());
    // with the peer, it sends CompleteJob carrying both signatures
    let ev = evaluator(&fx.node, 5, EvalConfig { peers: vec![format!("http://127.0.0.1:{port}")], ..Default::default() });
    let out = ev.round().unwrap();
    assert!(out[0].tx_hash.is_some(), "{:?}", out[0]);
    let txs = mempool(&fx.node);
    match &txs[0].0.body {
        ntx::Body::CompleteJob { sigs, .. } => {
            let keys: Vec<[u8; 32]> = sigs.iter().map(|s| s.0).collect();
            assert_eq!(keys, vec![pk(5), pk(6)]);
        }
        b => panic!("{b:?}"),
    }
}

#[test]
fn unfunded_evaluator_publishes_but_does_not_send() {
    let fx = fixture("unfunded", vec![pk(5)], 1);
    let ev = evaluator(&fx.node, 5, EvalConfig::default());
    let out = ev.round().unwrap();
    assert_eq!(out[0].decision, Decision::Accept);
    assert!(out[0].tx_hash.is_none());
    assert!(mempool(&fx.node).is_empty());
    // a correct C on another fixture path still verifies with the published signature
    let sig: [u8; 64] = hex::decode(&out[0].signature_hex).unwrap().try_into().unwrap();
    let msg = hex::decode(&out[0].message_hex).unwrap();
    sk(5).verifying_key().verify(&msg, &Signature::from_bytes(&sig)).unwrap();
}

#[test]
fn file_result_uri_naming_a_device_is_rejected_without_reading_it() {
    // security pass V-2b: a chain-sourced file:// URI must not make the evaluator read
    // /dev/zero (or a FIFO) until it is killed.
    let fx = fixture("device", vec![pk(5)], 1);
    let ev = evaluator(&fx.node, 5, EvalConfig { submit: false, ..Default::default() });
    let mut j = fx.node.get_job(&fx.job_id).unwrap().unwrap();
    j.result_uri = "file:///dev/zero".into();
    let t = std::time::Instant::now();
    assert!(matches!(ev.check(&j, 10), Verdict::Reject(r) if r.contains("regular file")));
    assert!(t.elapsed() < std::time::Duration::from_secs(5));
}
