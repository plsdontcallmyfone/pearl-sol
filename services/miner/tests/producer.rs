//! Miner against the in-process mock node (no sockets), plus one JSON-RPC-over-HTTP
//! run on a free port in 10690-10699 (outside the demo stack's 10600-10669) (checked with lsof first).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use pearl_attester::{Attester, JobSource};
use pearl_job::freivalds::{matmul, result_hash};
use pearl_job::{data, job_key, JobHeader, MiningConfig};
use pearl_miner::{Miner, MinerConfig, ResultAttester};
use pearl_node::mock::MockNode;
use pearl_node::{HttpNode, JobInfo, NodeApi};

/// About 0.6 wins per nonce on 64 x 64 (CLI.md).
const EASY: u32 = 0x1e040000;
const CHAIN: u32 = 4242;

fn miner_key() -> SigningKey {
    SigningKey::from_bytes(&[0x4d; 32])
}
fn attester_key() -> SigningKey {
    SigningKey::from_bytes(&[0x5a; 32])
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("miner-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 64 x 64 job with data on disk, evaluated by `attester_key()`.
fn make_job(dir: &std::path::Path, id: [u8; 32], seed: u64) -> (JobInfo, Vec<i32>) {
    make_job_with(dir, id, id, seed)
}

/// `header_id` = the Pearl header's first field (the poster for a storefront order).
fn make_job_with(dir: &std::path::Path, id: [u8; 32], header_id: [u8; 32], seed: u64) -> (JobInfo, Vec<i32>) {
    let (m, n, k) = (64u32, 64u32, 2048u32);
    let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, Some(seed));
    let mp = data::save(&dir.join(hex::encode(&id[..4])), m, n, k, &a, &b).unwrap();
    let config = MiningConfig::localnet_default();
    let header = JobHeader { job_pubkey: header_id, terms_hash: [0x33; 32], created_slot: 1, target_bits: 0x1d010000 };
    let key = job_key(&header, &config);
    let (ha, hb) = pearl_job::commit(&key, &a, &b, k as usize, n as usize);
    let c = matmul(&a, &b, m as usize, n as usize, k as usize);
    let info = JobInfo {
        job_id: id,
        header_id,
        poster: [0x10; 32],
        terms_hash: [0x33; 32],
        created_height: 1,
        share_target: 0x1d010000,
        config,
        m,
        n,
        hash_a: ha,
        hash_b: hb,
        data_uri: format!("file://{}", mp.display()),
        status: "Open".into(),
        deliverer: None,
        result_hash: None,
        result_uri: String::new(),
        evaluators: vec![attester_key().verifying_key().to_bytes()],
        threshold: 1,
        deadline_height: 1000,
        kind: "job".into(),
        standing: None,
    };
    (info, c)
}

struct InProc(Attester);
impl ResultAttester for InProc {
    fn attest(&self, job_id: &[u8; 32], deliverer: &[u8; 32], result_uri: &str) -> anyhow::Result<([u8; 32], [u8; 64])> {
        let body = serde_json::json!({"job": hex::encode(job_id), "deliverer": bs58::encode(deliverer).into_string(), "result_uri": result_uri});
        let (_, v) = self.0.route("POST", "/result", body.to_string().as_bytes());
        pearl_miner::parse_attestation(&v)
    }
}

fn cfg(dir: &std::path::Path, blocks: u64, deliver: bool) -> MinerConfig {
    MinerConfig {
        threads: 2,
        refresh: Duration::from_millis(300),
        stats_every: Duration::from_millis(1),
        deliver,
        results_dir: dir.join("results"),
        max_blocks: Some(blocks),
        deliver_retry: Duration::from_secs(5),
        ..MinerConfig::default()
    }
}

fn collecting_log() -> (Arc<Mutex<Vec<Value>>>, pearl_miner::Logger) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let l2 = logs.clone();
    (logs, Box::new(move |v| l2.lock().unwrap().push(v)))
}

fn events(logs: &Arc<Mutex<Vec<Value>>>, e: &str) -> usize {
    logs.lock().unwrap().iter().filter(|v| v["event"] == e).count()
}

#[test]
fn mines_job_blocks_and_delivers() {
    let dir = tmp("deliver");
    let node = MockNode::new(CHAIN, EASY);
    let (job, c) = make_job(&dir, [0x0a; 32], 5);
    node.add_job(job.clone());
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 2, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, Some(&att), miner_key(), cfg(&dir, 3, true), log);
    m.run(&|| false).unwrap();

    let s = node.state.lock().unwrap();
    assert!(s.blocks.len() >= 3, "{} blocks", s.blocks.len());
    assert!(s.blocks.iter().all(|b| b.header.miner == miner_key().verifying_key().to_bytes()));
    assert!(s.blocks.iter().any(|b| b.header.job_id == [0x0a; 32]), "no job block");
    assert!(s.blocks.iter().all(|b| b.proof_len > 12_000));
    drop(s);
    let j = node.job(&[0x0a; 32]).unwrap();
    assert!(j.is("Completed"), "job is {}", j.status);
    assert_eq!(j.deliverer, Some(miner_key().verifying_key().to_bytes()));
    assert_eq!(j.result_hash, Some(result_hash(&job.to_job().job_key(), &c)));
    assert_eq!(m.stats.completed, 1);
    assert_eq!(m.pending_deliveries(), 0);
    assert!(events(&logs, "result_submitted") == 1 && events(&logs, "complete_sent") == 1);
    assert!(events(&logs, "stats") > 0, "no stats line");
    // Account nonce advanced by SubmitResult + CompleteJob.
    assert_eq!(node.get_account(&miner_key().verifying_key().to_bytes()).unwrap().nonce, 2);
}

/// Storefront order: the job's Pearl header id (the poster) differs from its chain id.
#[test]
fn delivers_order_job_whose_header_id_is_the_poster() {
    let dir = tmp("order");
    let node = MockNode::new(CHAIN, EASY);
    let (job, c) = make_job_with(&dir, [0x0b; 32], [0x10; 32], 6);
    node.add_job(job.clone());
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 2, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, Some(&att), miner_key(), cfg(&dir, 2, true), log);
    m.run(&|| false).unwrap();
    assert!(node.state.lock().unwrap().blocks.iter().any(|b| b.header.job_id == [0x0b; 32]), "no job block");
    let j = node.job(&[0x0b; 32]).unwrap();
    assert!(j.is("Completed"), "job is {}", j.status);
    assert_eq!(j.result_hash, Some(result_hash(&job.to_job().job_key(), &c)));
    assert_eq!(events(&logs, "mine_error"), 0);
    assert_eq!(m.stats.completed, 1);
}

#[test]
fn fallback_blocks_without_jobs() {
    let dir = tmp("fallback");
    let node = MockNode::new(CHAIN, EASY);
    let (_logs, log) = collecting_log();
    let mut m = Miner::new(&node, None, miner_key(), cfg(&dir, 3, false), log);
    m.run(&|| false).unwrap();
    let s = node.state.lock().unwrap();
    assert_eq!(s.blocks.len(), 3);
    assert!(s.blocks.iter().all(|b| b.header.is_fallback()));
    assert!(s.rejected.is_empty(), "{:?}", s.rejected);
    // Chain links.
    for w in s.blocks.windows(2) {
        assert_eq!(w[1].header.prev_hash, w[0].header.block_hash());
    }
    assert_eq!(m.stats.fallback_accepted, 3);
}

/// The job gets one block, then the node moves to fallback blocks.
#[test]
fn job_then_fallback() {
    let dir = tmp("mixed");
    let node = MockNode::new(CHAIN, EASY);
    node.state.lock().unwrap().max_blocks_per_job = 1;
    let (job, _) = make_job(&dir, [0x0b; 32], 6);
    node.add_job(job);
    let (_logs, log) = collecting_log();
    let mut m = Miner::new(&node, None, miner_key(), cfg(&dir, 3, false), log);
    m.run(&|| false).unwrap();
    let s = node.state.lock().unwrap();
    assert_eq!(s.blocks.iter().filter(|b| !b.header.is_fallback()).count(), 1);
    assert_eq!(s.blocks.iter().filter(|b| b.header.is_fallback()).count(), 2);
}

#[test]
fn stale_templates_are_rejected_not_fatal() {
    let dir = tmp("stale");
    let node = MockNode::new(CHAIN, EASY);
    node.state.lock().unwrap().steal_templates = true;
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, None, miner_key(), cfg(&dir, 1, false), log);
    let mut rejected = 0;
    for _ in 0..4 {
        if let Some(pearl_node::SubmitOutcome::Rejected { .. }) = m.step().unwrap() {
            rejected += 1;
        }
    }
    assert!(rejected >= 2, "rejected {rejected}");
    assert_eq!(m.stats.accepted, 0);
    assert_eq!(events(&logs, "block_rejected"), rejected);
    assert!(node.state.lock().unwrap().rejected.iter().all(|r| r.contains("stale")));
}

#[test]
fn unusable_job_data_is_skipped() {
    let dir = tmp("baddata");
    let node = MockNode::new(CHAIN, EASY);
    let (mut job, _) = make_job(&dir, [0x0c; 32], 7);
    job.hash_a[0] ^= 1; // data no longer matches the commitment
    node.add_job(job);
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, None, miner_key(), cfg(&dir, 2, false), log);
    m.run(&|| false).unwrap();
    assert_eq!(events(&logs, "job_unusable"), 1, "retried too early");
    // It mined fallback blocks instead of idling.
    let s = node.state.lock().unwrap();
    assert_eq!(s.blocks.len(), 2);
    assert!(s.blocks.iter().all(|b| b.header.is_fallback()));
}

/// The wrong attester key (not an evaluator) gets no signature; the job stays Delivered.
#[test]
fn attester_must_be_an_evaluator() {
    let dir = tmp("noeval");
    let node = MockNode::new(CHAIN, EASY);
    let (job, _) = make_job(&dir, [0x0d; 32], 8);
    node.add_job(job);
    let att = InProc(Attester { key: SigningKey::from_bytes(&[0x77; 32]), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 1, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut c = cfg(&dir, 2, true);
    c.max_blocks = Some(2);
    let mut m = Miner::new(&node, Some(&att), miner_key(), c, log);
    for _ in 0..12 {
        let _ = m.step();
        let t = node.block_template(&miner_key().verifying_key().to_bytes()).unwrap();
        m.deliver_step(&t).unwrap();
    }
    let j = node.job(&[0x0d; 32]).unwrap();
    assert!(j.is("Delivered"), "{}", j.status);
    assert!(events(&logs, "complete_failed") > 0);
    let errs: Vec<String> = logs.lock().unwrap().iter().filter(|v| v["event"] == "complete_failed").map(|v| v["error"].to_string()).collect();
    assert!(errs[0].contains("not an evaluator"), "{errs:?}");
}

fn free_test_port() -> Option<u16> {
    (10690..=10699).find(|p| {
        let out = std::process::Command::new("lsof").args(["-ti", &format!(":{p}")]).output();
        matches!(out, Ok(o) if o.stdout.is_empty())
    })
}

/// Same miner, through HttpNode (JSON-RPC 2.0) to the mock behind tiny_http.
#[test]
fn over_http_jsonrpc() {
    let Some(port) = free_test_port() else {
        eprintln!("no free port in 10690-10699, skipping");
        return;
    };
    let dir = tmp("http");
    let node = MockNode::new(CHAIN, EASY);
    let (job, _) = make_job(&dir, [0x0e; 32], 9);
    node.add_job(job);
    let server = Arc::new(tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("bind test port"));
    let (srv, mock) = (server.clone(), node.clone());
    let handle = std::thread::spawn(move || {
        for mut req in srv.incoming_requests() {
            let mut body = Vec::new();
            let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
            let v = mock.handle_jsonrpc(&body);
            let _ = req.respond(tiny_http::Response::from_string(v.to_string()));
        }
    });
    let http = HttpNode::new(&format!("http://127.0.0.1:{port}"));
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(HttpNode::new(&format!("http://127.0.0.1:{port}")))), freivalds_rounds: 2, models_dir: "models".into() });
    let (_logs, log) = collecting_log();
    let mut m = Miner::new(&http, Some(&att), miner_key(), cfg(&dir, 2, true), log);
    m.run(&|| false).unwrap();
    server.unblock();
    handle.join().unwrap();
    assert!(node.height() >= 2);
    assert!(node.job(&[0x0e; 32]).unwrap().is("Completed"));
}

/// `cargo test --release -p pearl-miner --test producer -- --ignored --nocapture bench`
/// Nonce rate on job and fallback templates with a target nothing meets.
#[test]
#[ignore]
fn bench_rates() {
    let dir = tmp("bench");
    for (label, with_job, threads) in [("job 64x64", true, 1), ("job 64x64", true, 8), ("fallback 64x64", false, 1), ("fallback 64x64", false, 8)] {
        let node = MockNode::new(CHAIN, 0x03000001);
        if with_job {
            node.add_job(make_job(&dir, [0x1f; 32], 3).0);
        }
        let (_l, log) = collecting_log();
        let mut c = cfg(&dir, 1, false);
        c.threads = threads;
        c.refresh = Duration::from_secs(4);
        let mut m = Miner::new(&node, None, miner_key(), c, log);
        let _ = m.step(); // loads the job, one round
        let (n0, t0) = (m.nonces_done(), std::time::Instant::now());
        let _ = m.step();
        let rate = (m.nonces_done() - n0) as f64 / t0.elapsed().as_secs_f64();
        println!("{label} threads={threads}: {rate:.0} nonces/s ({:.2} ms per nonce per thread)", 1e3 * threads as f64 / rate);
    }
}

/// M2 data availability: the job's data is a `blob:` on the node; C is delivered as a
/// `blob:` (putBlob) and the attester reads both through the node.
#[test]
fn blob_job_data_and_blob_result() {
    let dir = tmp("blob");
    let node = MockNode::new(CHAIN, EASY);
    let (mut job, c) = make_job(&dir, [0x0b; 32], 9);
    let d = data::load(&job.data_uri).unwrap();
    let blob = data::encode_job_blob(job.m, job.n, job.config.common_dim, &d.a, &d.b).unwrap();
    let uri = node.put_blob(&blob).unwrap();
    assert_eq!(uri, data::blob_uri(&data::blob_id(&blob)));
    job.data_uri = uri;
    std::fs::remove_dir_all(dir.join(hex::encode(&[0x0b; 4]))).unwrap(); // no local copy
    node.add_job(job.clone());
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 2, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, Some(&att), miner_key(), cfg(&dir, 2, true), log);
    m.run(&|| false).unwrap();
    assert!(events(&logs, "job_loaded") >= 1);
    let j = node.job(&[0x0b; 32]).unwrap();
    assert!(j.is("Completed"), "job is {}", j.status);
    assert!(j.result_uri.starts_with("blob:"), "result uri {}", j.result_uri);
    let id = data::parse_blob_uri(&j.result_uri).unwrap();
    assert_eq!(node.get_blob(&id).unwrap().unwrap(), pearl_job::freivalds::c_to_bytes(&c));
    assert_eq!(j.result_hash, Some(result_hash(&job.to_job().job_key(), &c)));
    // a blob the node does not hold: the job is skipped, fallback blocks continue
    let node2 = MockNode::new(CHAIN, EASY);
    let mut job2 = job.clone();
    job2.data_uri = data::blob_uri(&[7; 32]);
    node2.add_job(job2);
    let (logs2, log2) = collecting_log();
    let mut m2 = Miner::new(&node2, None, miner_key(), cfg(&dir, 1, false), log2);
    m2.run(&|| false).unwrap();
    assert!(node2.state.lock().unwrap().blocks.iter().all(|b| b.header.is_fallback()));
    drop(logs2);
}

/// MockNode plus the two calls the orphan sweep makes (`listJobs`, `getChainInfo`): it
/// reports every mock job as mined out by someone else (`blocks_mined = max_blocks = 2`).
struct MinedOutNode(MockNode);
impl NodeApi for MinedOutNode {
    fn block_template(&self, payout: &[u8; 32]) -> anyhow::Result<pearl_node::Template> { self.0.block_template(payout) }
    fn submit_block(&self, t: &pearl_node::Template, b: &pearl_job::pow::MinedBlock) -> anyhow::Result<pearl_node::SubmitOutcome> { self.0.submit_block(t, b) }
    fn get_job(&self, id: &[u8; 32]) -> anyhow::Result<Option<JobInfo>> { self.0.get_job(id) }
    fn get_account(&self, a: &[u8; 32]) -> anyhow::Result<pearl_node::Account> { self.0.get_account(a) }
    fn send_tx(&self, wire: &[u8]) -> anyhow::Result<String> { self.0.send_tx(wire) }
    fn call(&self, method: &str, _p: Value) -> anyhow::Result<Value> {
        let s = self.0.state.lock().unwrap();
        match method {
            "getChainInfo" => Ok(serde_json::json!({"height": s.blocks.len()})),
            "listJobs" => {
                let jobs: Vec<Value> = s.jobs.iter().filter(|j| j.is("Open")).map(|j| {
                    let mut v = j.to_json();
                    v["max_blocks"] = 2.into();
                    v["blocks_mined"] = 2.into();
                    v
                }).collect();
                Ok(serde_json::json!({"total": jobs.len(), "jobs": jobs}))
            }
            m => anyhow::bail!("method not found: {m}"),
        }
    }
}

/// A job other miners mined out without delivering (no `--deliver` there): the node no
/// longer offers it, so a delivering miner finds it with the orphan sweep and closes it.
#[test]
fn delivers_orphaned_mined_out_job() {
    let dir = tmp("orphan");
    let mock = MockNode::new(CHAIN, EASY);
    mock.state.lock().unwrap().max_blocks_per_job = 0; // templates are fallback only
    let (job, c) = make_job(&dir, [0x0e; 32], 9);
    mock.add_job(job.clone());
    let node = MinedOutNode(mock.clone());
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(mock.clone())), freivalds_rounds: 2, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, Some(&att), miner_key(), cfg(&dir, 3, true), log);
    m.run(&|| false).unwrap();
    let j = mock.job(&[0x0e; 32]).unwrap();
    assert!(j.is("Completed"), "job is {}", j.status);
    assert_eq!(j.deliverer, Some(miner_key().verifying_key().to_bytes()));
    assert_eq!(j.result_hash, Some(result_hash(&job.to_job().job_key(), &c)));
    assert_eq!(events(&logs, "orphan_found"), 1);
    assert!(mock.state.lock().unwrap().blocks.iter().all(|b| b.header.job_id != [0x0e; 32]), "the sweep must not mine the job");

    // without --deliver nothing is swept
    let mock2 = MockNode::new(CHAIN, EASY);
    mock2.state.lock().unwrap().max_blocks_per_job = 0;
    mock2.add_job(make_job(&dir, [0x0f; 32], 10).0);
    let node2 = MinedOutNode(mock2.clone());
    let (logs2, log2) = collecting_log();
    let mut m2 = Miner::new(&node2, None, miner_key(), cfg(&dir, 2, false), log2);
    m2.run(&|| false).unwrap();
    assert!(mock2.job(&[0x0f; 32]).unwrap().is("Open"));
    assert_eq!(events(&logs2, "orphan_found"), 0);
}

/// A node with the job index (2026-10-22): listJobs filters by `kind` and pages by cursor
/// (`after` / `next_cursor`), one job per page here; it records every listJobs query.
struct CursorNode(MockNode, std::sync::Mutex<Vec<Value>>);
impl NodeApi for CursorNode {
    fn block_template(&self, payout: &[u8; 32]) -> anyhow::Result<pearl_node::Template> { self.0.block_template(payout) }
    fn submit_block(&self, t: &pearl_node::Template, b: &pearl_job::pow::MinedBlock) -> anyhow::Result<pearl_node::SubmitOutcome> { self.0.submit_block(t, b) }
    fn get_job(&self, id: &[u8; 32]) -> anyhow::Result<Option<JobInfo>> { self.0.get_job(id) }
    fn get_account(&self, a: &[u8; 32]) -> anyhow::Result<pearl_node::Account> { self.0.get_account(a) }
    fn send_tx(&self, wire: &[u8]) -> anyhow::Result<String> { self.0.send_tx(wire) }
    fn call(&self, method: &str, p: Value) -> anyhow::Result<Value> {
        let s = self.0.state.lock().unwrap();
        match method {
            "getChainInfo" => Ok(serde_json::json!({"height": s.blocks.len()})),
            "listJobs" => {
                self.1.lock().unwrap().push(p.clone());
                let open: Vec<(u64, Value)> = s.jobs.iter().enumerate().filter(|(_, j)| j.is("Open")).map(|(i, j)| {
                    let mut v = j.to_json();
                    v["max_blocks"] = 2.into();
                    v["blocks_mined"] = 2.into();
                    v["index"] = (i as u64).into();
                    (i as u64, v)
                }).collect();
                let start = p.get("after").and_then(Value::as_u64).map_or(0, |a| a + 1);
                let rest: Vec<&(u64, Value)> = open.iter().filter(|(i, _)| *i >= start).collect();
                let page: Vec<Value> = rest.iter().take(1).map(|(_, v)| v.clone()).collect();
                let next = if rest.len() > 1 { Value::from(rest[0].0) } else { Value::Null };
                Ok(serde_json::json!({"total": open.len(), "jobs": page, "next_cursor": next}))
            }
            m => anyhow::bail!("method not found: {m}"),
        }
    }
}

#[test]
fn orphan_sweep_pages_by_cursor_and_asks_for_plain_jobs_only() {
    let dir = tmp("orphan-cursor");
    let mock = MockNode::new(CHAIN, EASY);
    mock.state.lock().unwrap().max_blocks_per_job = 0;
    for (id, seed) in [([0x1a; 32], 21u64), ([0x1b; 32], 22), ([0x1c; 32], 23)] {
        mock.add_job(make_job(&dir, id, seed).0);
    }
    let node = CursorNode(mock.clone(), Default::default());
    let att = InProc(Attester { key: attester_key(), source: JobSource::Node(Box::new(mock.clone())), freivalds_rounds: 2, models_dir: "models".into() });
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, Some(&att), miner_key(), cfg(&dir, 3, true), log);
    m.run(&|| false).unwrap();
    assert_eq!(events(&logs, "orphan_found"), 3, "every page of the cursor walk was read");
    for id in [[0x1a; 32], [0x1b; 32], [0x1c; 32]] {
        assert!(mock.job(&id).unwrap().is("Completed"));
    }
    let qs = node.1.lock().unwrap();
    assert!(qs.iter().all(|q| q["kind"] == "job" && q["status"] == "open"), "{qs:?}");
    assert!(qs[0].get("after").is_none() && qs[1]["after"].is_u64(), "first page by offset, then by cursor: {qs:?}");
}

/// `--payout-address` (the Cadre Wallet's miner): blocks pay the given account, the key in the
/// struct never signs, and no tx is sent (no delivery).
#[test]
fn payout_address_mode_pays_the_address_and_signs_nothing() {
    let dir = tmp("payout-address");
    let node = MockNode::new(CHAIN, EASY);
    let (job, _) = make_job(&dir, [0x0c; 32], 7);
    node.add_job(job);
    let payout = [0x77; 32];
    let (logs, log) = collecting_log();
    let mut m = Miner::new(&node, None, SigningKey::from_bytes(&[0u8; 32]), cfg(&dir, 2, false), log);
    m.payout = Some(payout);
    m.run(&|| false).unwrap();

    let s = node.state.lock().unwrap();
    assert!(s.blocks.len() >= 2, "{} blocks", s.blocks.len());
    assert!(s.blocks.iter().all(|b| b.header.miner == payout), "a block paid someone else");
    drop(s);
    assert_eq!(node.job(&[0x0c; 32]).unwrap().deliverer, None, "delivered without a key");
    assert_eq!(events(&logs, "result_submitted") + events(&logs, "complete_sent"), 0);
    let start = logs.lock().unwrap().iter().find(|v| v["event"] == "start").cloned().unwrap();
    assert_eq!(start["miner"], bs58::encode(payout).into_string());
}
