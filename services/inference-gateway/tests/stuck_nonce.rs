//! Server nonce robustness (devnet box 0, 2026-10-22): a round the chain had started was resumed
//! restacked with later requests (m 1712 against the chain's 1280). Every PostSegments of it
//! failed at the tip, the mempool kept them (templates skip a failing tx, nothing evicts it), and
//! every later settle reused the pooled nonce ("a tx with this nonce is pooled with a fee >= this
//! one"): rounds_posted stuck at 45, scratch at the 64 GiB cap, paid AI refused.
//!
//! The chain here is a small simulation of the parts that matter: confirmed nonces, a mempool
//! with the node's replacement rule (10% fee bump), templates that include the sender's txs in
//! nonce order and stop at the first that fails, and the PostSegments round rules (SPEC 4.5).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Result};
use base64::Engine;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::{data, standing};
use pearl_node::mock::MockNode;
use pearl_node::NodeApi;
use pearl_node::tx::{self, Body, Tx};

const CHAIN_ID: u32 = 7;
const MIN_FEE: u64 = 1000;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwsn-{tag}-{}", std::process::id()));
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

/// What the worker writes with pad false: A (rows x k) and C = A B per matrix.
fn write_outputs(dir: &Path, model: &ModelInfo, rows: u32, seed: u64) {
    let b = bts(model);
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("c")).unwrap();
    for (i, (k, n)) in model.matrices.iter().enumerate() {
        let (a, _) = data::random_matrices(rows as usize, 1, *k as usize, Some(seed * 100 + i as u64));
        let c = pearl_job::freivalds::matmul_bt(&a, &b[i], rows as usize, *n as usize, *k as usize);
        std::fs::write(dir.join(format!("a/{i}.i8")), pearl_job::job::as_u8(&a)).unwrap();
        std::fs::write(dir.join(format!("c/{i}.i32")), pearl_job::freivalds::c_to_bytes(&c)).unwrap();
    }
}

struct NoSf;
impl Storefront for NoSf {
    fn post_job(&self, _b: &Value, _x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        bail!("not in this test")
    }
}
struct NoWorker;
impl Worker for NoWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        bail!("not in this test")
    }
    fn chat_ids(&self, _m: &Value, _t: Option<&Value>) -> Result<Vec<u32>> {
        bail!("not in this test")
    }
    fn generate(&self, _i: &[u32], _n: u32, _o: &Path, _g: &GenOpts, _f: &mut dyn FnMut(&str)) -> Result<Value> {
        bail!("not in this test")
    }
}
struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        bail!("not in this test")
    }
}

// ------------------------------------------------------------------ chain simulation

#[derive(Default, Clone)]
struct SimTicket {
    rounds: u16,
    round_rows: u32,
}

#[derive(Default)]
struct SimState {
    height: u64,
    nonces: HashMap<[u8; 32], u64>,
    /// (sender, nonce) -> (hash, wire)
    pool: BTreeMap<([u8; 32], u64), (String, Vec<u8>)>,
    included: HashMap<String, u64>,
    tickets: HashMap<[u8; 32], SimTicket>,
    /// segment id -> (m, hash_a, a_uri)
    segments: HashMap<[u8; 32], (u32, [u8; 32], String)>,
    /// tickets whose PostSegments the chain refuses (a validity failure the gateway can not see)
    refuse: HashSet<[u8; 32]>,
    /// blocks include nothing until the gateway first reads the mempool
    hold: bool,
}

struct Sim {
    s: Mutex<SimState>,
    blobs: MockNode,
}

struct ParsedSeg {
    ticket: [u8; 32],
    round: u16,
    segs: Vec<(u16, u32, [u8; 32], String)>,
}

fn parse_post_segments(b: &[u8]) -> ParsedSeg {
    let ticket: [u8; 32] = b[..32].try_into().unwrap();
    let round = u16::from_le_bytes(b[32..34].try_into().unwrap());
    let mut o = 34 + 32 + 2 + 6 + 6;
    let count = b[o] as usize;
    o += 1;
    let mut segs = vec![];
    for _ in 0..count {
        let matrix = u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        let m = u32::from_le_bytes(b[o + 2..o + 6].try_into().unwrap());
        let hash_a: [u8; 32] = b[o + 18..o + 50].try_into().unwrap();
        o += 18 + 96;
        let ul = u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as usize;
        let uri = String::from_utf8(b[o + 2..o + 2 + ul].to_vec()).unwrap();
        o += 2 + ul;
        segs.push((matrix, m, hash_a, uri));
    }
    ParsedSeg { ticket, round, segs }
}

impl SimState {
    /// Apply one tx at the tip (Err: the template skips it).
    fn apply(&mut self, wire: &[u8]) -> Result<()> {
        let body = &wire[54..wire.len() - 64];
        match wire[53] {
            tx::kind::TRANSFER => Ok(()),
            tx::kind::POST_SEGMENTS => {
                let p = parse_post_segments(body);
                ensure!(!self.refuse.contains(&p.ticket), "refused");
                let mut t = self.tickets.get(&p.ticket).cloned().ok_or_else(|| anyhow!("unknown ticket"))?;
                let m0 = p.segs[0].1;
                if t.rounds > 0 && p.round == t.rounds - 1 {
                    ensure!(m0 == t.round_rows, "round {} has m {}, not {m0}", p.round, t.round_rows);
                } else if p.round == t.rounds {
                    t.rounds += 1;
                    t.round_rows = m0;
                } else {
                    bail!("round {}: the ticket is at round {}", p.round, t.rounds as i64 - 1);
                }
                for (mx, m, ha, uri) in &p.segs {
                    let id = standing::segment_id(&p.ticket, *mx, p.round);
                    ensure!(!self.segments.contains_key(&id), "round already has a segment for matrix {mx}");
                    self.segments.insert(id, (*m, *ha, uri.clone()));
                }
                self.tickets.insert(p.ticket, t);
                Ok(())
            }
            k => bail!("kind {k} not simulated"),
        }
    }

    /// One block: each sender's pooled txs in nonce order from its confirmed nonce, up to the
    /// first that fails (it stays pooled, like the node's mempool).
    fn mine(&mut self) {
        self.height += 1;
        if self.hold {
            return;
        }
        let senders: HashSet<[u8; 32]> = self.pool.keys().map(|k| k.0).collect();
        for s in senders {
            loop {
                let n = self.nonces.get(&s).copied().unwrap_or(0);
                let Some((h, w)) = self.pool.get(&(s, n)).cloned() else { break };
                if self.apply(&w).is_err() {
                    break;
                }
                self.pool.remove(&(s, n));
                self.included.insert(h, self.height);
                self.nonces.insert(s, n + 1);
            }
        }
    }

    /// The node's mempool insert (chain/mempool): nonce window, replacement by fee.
    fn insert(&mut self, wire: &[u8]) -> Result<String> {
        let (t, _) = tx::decode(wire)?;
        let h = hex::encode(blake3::hash(wire).as_bytes());
        let confirmed = self.nonces.get(&t.from).copied().unwrap_or(0);
        ensure!(t.chain_id == CHAIN_ID, "wrong chain id");
        ensure!(t.fee >= MIN_FEE, "fee {} below min_fee {MIN_FEE}", t.fee);
        ensure!(t.nonce >= confirmed, "nonce {} already used (account nonce {confirmed})", t.nonce);
        ensure!(t.nonce < confirmed + 64, "nonce {} too far ahead of account nonce {confirmed}", t.nonce);
        if let Some((oh, ow)) = self.pool.get(&(t.from, t.nonce)) {
            ensure!(*oh != h, "already pooled");
            let old = tx::decode(ow)?.0.fee;
            let floor = old + (old / 10).max(1);
            if t.fee < floor {
                if old >= t.fee {
                    bail!("a tx with this nonce is pooled with a fee >= this one");
                }
                bail!("replacement fee {} below {floor} (10% above the pooled tx)", t.fee);
            }
        }
        self.pool.insert((t.from, t.nonce), (h.clone(), wire.to_vec()));
        Ok(h)
    }
}

#[derive(Clone)]
struct SimNode(Arc<Sim>);

impl NodeApi for SimNode {
    fn block_template(&self, _p: &[u8; 32]) -> Result<pearl_node::Template> {
        bail!("not in this test")
    }
    fn submit_block(&self, _t: &pearl_node::Template, _b: &pearl_job::pow::MinedBlock) -> Result<pearl_node::SubmitOutcome> {
        bail!("not in this test")
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<pearl_node::JobInfo>> {
        let s = self.0.s.lock().unwrap();
        Ok(s.segments.get(id).map(|(m, ha, uri)| pearl_node::JobInfo {
            job_id: *id, header_id: [0; 32], poster: [0; 32], terms_hash: [0; 32], created_height: 0, share_target: 0,
            config: MiningConfig::localnet_default(), m: *m, n: 0, hash_a: *ha, hash_b: [0; 32], data_uri: uri.clone(), status: "Open".into(),
            deliverer: None, result_hash: None, result_uri: String::new(), evaluators: vec![], threshold: 1, deadline_height: 1_000_000,
            kind: "segment".into(), standing: None,
        }))
    }
    fn get_account(&self, a: &[u8; 32]) -> Result<pearl_node::Account> {
        Ok(pearl_node::Account { balance: u64::MAX / 4, nonce: self.0.s.lock().unwrap().nonces.get(a).copied().unwrap_or(0) })
    }
    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        self.0.s.lock().unwrap().insert(wire).map_err(|e| anyhow!("sendTx: {e}"))
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.0.blobs.get_blob(id)
    }
    fn put_blob(&self, d: &[u8]) -> Result<String> {
        self.0.blobs.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        let s = self.0.s.lock().unwrap();
        match method {
            "getParams" => Ok(json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": MIN_FEE.to_string()})),
            "getChainInfo" => Ok(json!({"chain_id": CHAIN_ID, "height": s.height})),
            "getTicket" => {
                let id: [u8; 32] = hex::decode(p["ticket_id"].as_str().unwrap_or(""))?.try_into().map_err(|_| anyhow!("ticket id"))?;
                let t = s.tickets.get(&id).cloned().ok_or_else(|| anyhow!("unknown ticket"))?;
                Ok(json!({"status": "open", "deadline_height": 1_000_000, "max_blocks": 100_000,
                    "standing": {"rounds": t.rounds, "round_rows": t.round_rows, "request_hash": "00"}}))
            }
            "getMempool" => {
                drop(s);
                self.0.s.lock().unwrap().hold = false;
                let s = self.0.s.lock().unwrap();
                let txs: Vec<&String> = s.pool.values().map(|(h, _)| h).collect();
                Ok(json!({"size": txs.len(), "txs": txs}))
            }
            "getTx" => {
                let h = p["tx_hash"].as_str().unwrap_or("");
                if let Some(height) = s.included.get(h) {
                    return Ok(json!({"tx_hash": h, "status": "included", "height": height}));
                }
                match s.pool.iter().find(|(_, (ph, _))| ph == h) {
                    Some(((from, nonce), (_, w))) => Ok(json!({"tx_hash": h, "status": "pending", "from": pearl_node::b58(from), "nonce": nonce,
                        "fee": tx::decode(w)?.0.fee.to_string(), "tx": base64::engine::general_purpose::STANDARD.encode(w)})),
                    None => Ok(Value::Null),
                }
            }
            "getBlobStats" => bail!("unknown method getBlobStats"),
            m => bail!("method not found: {m}"),
        }
    }
}

/// Mines a block every few milliseconds until dropped.
struct Miner(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
impl Miner {
    fn start(sim: Arc<Sim>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let h = std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                sim.s.lock().unwrap().mine();
                std::thread::sleep(Duration::from_millis(15));
            }
        });
        Miner(stop, Some(h))
    }
}
impl Drop for Miner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
        if let Some(h) = self.1.take() {
            let _ = h.join();
        }
    }
}

fn server_key() -> SigningKey {
    SigningKey::from_bytes(&[3; 32])
}

struct Env {
    gw: Arc<Gateway>,
    sim: Arc<Sim>,
    md: ModelInfo,
    events: Arc<Mutex<Vec<Value>>>,
    dir: PathBuf,
}

fn env(tag: &str, cfg: impl FnOnce(&mut Config)) -> Env {
    let md = model();
    let sim = Arc::new(Sim { s: Mutex::new(SimState::default()), blobs: MockNode::new(CHAIN_ID, 0x207f_ffff) });
    let dir = tmp(tag);
    let mut c = Config { work_dir: dir.join("gw"), poll: Duration::from_millis(5), round_every: Duration::from_millis(0), min_free_disk_bytes: 0,
        stall_blocks: 4, ..Config::default() };
    cfg(&mut c);
    let events = Arc::new(Mutex::new(vec![]));
    let ev = events.clone();
    let gw = Arc::new(Gateway::new(Box::new(SimNode(sim.clone())), Box::new(NoSf), Box::new(NoWorker), Box::new(NoEval), server_key(), md.clone(), c,
        Box::new(move |v| { if std::env::var("SN_DEBUG").is_ok() { eprintln!("{v}"); } ev.lock().unwrap().push(v) })));
    Env { gw, sim, md, events, dir }
}

impl Env {
    /// A balance ticket of `rows` rows in the ledger and on the chain.
    fn ticket(&self, seed: u8, rows: u64) -> [u8; 32] {
        let id = [seed; 32];
        self.sim.s.lock().unwrap().tickets.insert(id, SimTicket::default());
        let t = TicketState { ticket_id: hex::encode(id), order: None, wallet: "w".into(), rows, lamports_per_row: None, reserved: 0, pending: vec![],
            rounds: vec![], state: "open".into(), single: false, created_ms: 0, error: None, deadline_height: None, round_secs: None, max_rounds: None, posting: None };
        self.gw.ledger.lock().unwrap().tickets.insert(hex::encode(id), t);
        id
    }

    /// A served request of `rows` rows waiting in `ticket`'s next round.
    fn serve(&self, ticket: &[u8; 32], name: &str, rows: u32, at_ms: u64) -> Served {
        let d = self.dir.join("req").join(name);
        write_outputs(&d, &self.md, rows, at_ms + rows as u64);
        let r = Served { request_id: name.into(), wallet: "w".into(), ticket_id: hex::encode(ticket), dir: d, rows, prompt_tokens: rows, completion_tokens: 1,
            body_sha256: String::new(), output_sha256: String::new(), text_sha256: String::new(), charged_lamports: None, at_ms, round: None, row_offset: None,
            ttft_s: 0.0, decode_tokens_per_s: None, prompt_ids: vec![], output_ids: vec![], max_new_tokens: 1, stop: vec![], glue: String::new() };
        let mut l = self.gw.ledger.lock().unwrap();
        l.requests.insert(name.into(), r.clone());
        l.tickets.get_mut(&hex::encode(ticket)).unwrap().pending.push(name.into());
        r
    }

    /// A PostSegments of `ticket`'s round `round`, matrix `i`, built from `reqs` padded to `m`.
    fn post_body(&self, ticket: &[u8; 32], round: u16, i: usize, reqs: &[&Served], m: u32) -> Body {
        let dirs: Vec<(PathBuf, u32)> = reqs.iter().map(|r| (r.dir.clone(), r.rows)).collect();
        let seg = prepare_matrix(&self.md, &dirs, m, i, 1).unwrap();
        self.sim.blobs.put_blob(&seg.a_blob).unwrap();
        Body::PostSegments { ticket_id: *ticket, round, head: self.md.head(), segments: vec![seg.spec] }
    }

    /// Sign at `nonce` with the server key and put it straight into the simulated mempool.
    fn pool(&self, body: Body, nonce: u64, fee: u64) -> String {
        let me = server_key().verifying_key().to_bytes();
        let w = tx::sign(&Tx { chain_id: CHAIN_ID, from: me, nonce, fee, body }, &server_key()).unwrap();
        self.sim.s.lock().unwrap().insert(&w).unwrap()
    }

    /// Apply a tx at the tip directly (an earlier attempt's tx that was mined).
    fn mined(&self, body: Body) {
        let me = server_key().verifying_key().to_bytes();
        let mut s = self.sim.s.lock().unwrap();
        let n = s.nonces.get(&me).copied().unwrap_or(0);
        let w = tx::sign(&Tx { chain_id: CHAIN_ID, from: me, nonce: n, fee: MIN_FEE, body }, &server_key()).unwrap();
        s.apply(&w).unwrap();
        s.nonces.insert(me, n + 1);
        s.height += 1;
    }

    fn has_event(&self, name: &str) -> bool {
        self.events.lock().unwrap().iter().any(|e| e["event"] == name)
    }
}

fn round_up(x: u32, p: u32) -> u32 {
    x.div_ceil(p) * p
}

#[test]
fn a_resumed_round_keeps_its_requests_and_replaces_the_stuck_txs() {
    let e = env("resume", |_| {});
    let p = e.md.period();
    let t = e.ticket(0x61, 4096);
    let r1 = e.serve(&t, "r1", 5, 1);
    let r2 = e.serve(&t, "r2", 6, 2);
    let r3 = e.serve(&t, "r3", 3 * p, 3); // served after the round started
    let m_chain = round_up(11, p);
    let m_all = round_up(11 + 3 * p, p);
    assert_ne!(m_chain, m_all, "the restacked round names another m");
    // round 0 started on the chain with r1 + r2 (matrix 0 mined), then the gateway restarted
    // before the ledger recorded it
    e.mined(e.post_body(&t, 0, 0, &[&r1, &r2], m_chain));
    // what the old gateway left: matrix 1 restacked with r3 (refused at the tip) holding the
    // server's next nonce, and a second leftover after it
    let me = server_key().verifying_key().to_bytes();
    let n0 = e.sim.s.lock().unwrap().nonces[&me];
    let stuck = e.pool(e.post_body(&t, 0, 1, &[&r1, &r2, &r3], m_all), n0, MIN_FEE);
    e.pool(e.post_body(&t, 0, 1, &[&r1, &r2, &r3], m_all), n0 + 1, MIN_FEE + 1);
    {
        let mut s = e.sim.s.lock().unwrap();
        s.mine();
        s.mine();
        assert_eq!(s.nonces[&me], n0, "the stuck tx is never included and holds the nonce");
        assert!(s.insert(&tx::sign(&Tx { chain_id: CHAIN_ID, from: me, nonce: n0, fee: MIN_FEE, body: Body::Transfer { to: me, amount: 0 } }, &server_key()).unwrap())
            .unwrap_err().to_string().contains("pooled with a fee >= this one"), "the error box 0 logged");
    }
    let _m = Miner::start(e.sim.clone());
    e.gw.settle_pass();
    let tid = hex::encode(t);
    {
        let l = e.gw.ledger.lock().unwrap();
        let tk = &l.tickets[&tid];
        assert_eq!(tk.rounds.len(), 1, "{:?}", e.events.lock().unwrap());
        assert_eq!((tk.rounds[0].round, tk.rounds[0].m), (0, m_chain), "the round keeps the chain's m");
        assert_eq!(tk.rounds[0].requests, vec!["r1", "r2"], "with exactly the requests it started with");
        assert_eq!(tk.pending, vec!["r3"]);
        assert!(tk.posting.is_none());
    }
    {
        let s = e.sim.s.lock().unwrap();
        assert_eq!(s.segments[&standing::segment_id(&t, 1, 0)].0, m_chain);
        assert!(!s.included.contains_key(&stuck), "the stuck tx was replaced, not mined");
        assert!(s.pool.keys().all(|k| k.0 != me), "no tx of the server left in the mempool: {:?}", s.pool.keys().collect::<Vec<_>>());
    }
    assert!(e.has_event("round_recovered") && e.has_event("tx_replaced") && e.has_event("tx_cancelled"));
    let st = e.gw.stats_json();
    assert_eq!(st["settle"]["tx_replaced"], 1);
    assert_eq!(st["settle"]["tx_cancelled"], 1);
    // the next pass posts r3 as round 1
    e.gw.settle_pass();
    let l = e.gw.ledger.lock().unwrap();
    assert_eq!(l.tickets[&tid].rounds.len(), 2);
    assert_eq!(l.tickets[&tid].rounds[1].requests, vec!["r3"]);
    drop(l);
    assert_eq!(e.sim.s.lock().unwrap().tickets[&t].rounds, 2);
    assert_eq!(e.gw.stats_json()["rounds_posted"], 2);
}

#[test]
fn a_recorded_posting_round_is_resumed_as_recorded_and_its_pooled_tx_reused() {
    let e = env("posting", |_| {});
    let p = e.md.period();
    let t = e.ticket(0x62, 4096);
    let r1 = e.serve(&t, "q1", 2 * p, 1);
    let r2 = e.serve(&t, "q2", p, 2);
    // the gateway recorded round 0 = [q1] before sending; matrix 0 mined, matrix 1 still pooled
    // at the replacement fee of an earlier attempt
    e.gw.ledger.lock().unwrap().tickets.get_mut(&hex::encode(t)).unwrap().posting = Some(PostingRound { round: 0, m: 2 * p, requests: vec!["q1".into()] });
    e.mined(e.post_body(&t, 0, 0, &[&r1], 2 * p));
    let me = server_key().verifying_key().to_bytes();
    let n0 = e.sim.s.lock().unwrap().nonces[&me];
    let pooled = e.pool(e.post_body(&t, 0, 1, &[&r1], 2 * p), n0, 1100);
    let _ = r2;
    e.sim.s.lock().unwrap().hold = true; // not mined before the gateway looks
    let _m = Miner::start(e.sim.clone());
    e.gw.settle_pass();
    let l = e.gw.ledger.lock().unwrap();
    let tk = &l.tickets[&hex::encode(t)];
    assert_eq!(tk.rounds.len(), 1);
    assert_eq!(tk.rounds[0].requests, vec!["q1"]);
    assert_eq!(tk.rounds[0].txs, vec![pooled.clone()], "the pooled tx is the same tx: reused, not replaced");
    assert_eq!(tk.pending, vec!["q2"]);
    assert!(!e.has_event("tx_replaced") && !e.has_event("round_recovered"));
    assert!(e.sim.s.lock().unwrap().included.contains_key(&pooled));
}

#[test]
fn a_tx_the_chain_refuses_stalls_one_ticket_not_the_pipeline() {
    let e = env("stall", |c| c.settle_backoff = Duration::from_secs(600));
    let p = e.md.period();
    let a = e.ticket(0x41, 4096);
    let b = e.ticket(0x42, 4096);
    e.serve(&a, "a1", p, 1); // older: settled first
    e.serve(&b, "b1", p, 2);
    e.sim.s.lock().unwrap().refuse.insert(a); // a validity failure the gateway can not see
    let _m = Miner::start(e.sim.clone());
    let t0 = Instant::now();
    e.gw.settle_pass();
    assert!(t0.elapsed() < Duration::from_secs(30), "stopped after a few skipped blocks, not the 600 s timeout");
    let l = e.gw.ledger.lock().unwrap();
    assert!(l.tickets[&hex::encode(a)].rounds.is_empty());
    assert_eq!(l.tickets[&hex::encode(b)].rounds.len(), 1, "the next ticket posted its round in the same pass, on the stuck nonce");
    drop(l);
    let st = e.gw.stats_json();
    assert_eq!(st["settle"]["tx_stalls"], 1);
    assert!(st["settle"]["tx_replaced"].as_u64().unwrap() >= 1);
    let bo = st["settle"]["backoff"].as_array().unwrap();
    assert_eq!(bo.len(), 1);
    assert_eq!(bo[0]["ticket"], hex::encode(a));
    assert!(bo[0]["error"].as_str().unwrap().contains("blocks skipped it"), "{bo:?}");
    // in its backoff the failing ticket is not tried again
    e.gw.settle_pass();
    assert_eq!(e.gw.stats_json()["settle"]["tx_stalls"], 1);
    let me = server_key().verifying_key().to_bytes();
    assert!(e.sim.s.lock().unwrap().pool.keys().all(|k| k.0 != me), "the refused ticket's leftovers were replaced or cancelled");
}

#[test]
fn settle_order_puts_a_started_round_first_then_drains_the_most_rows_under_pressure() {
    let e = env("order", |c| c.max_request_data_bytes = 1 << 40);
    let p = e.md.period();
    let (x, y, z) = (e.ticket(0x11, 4096), e.ticket(0x12, 4096), e.ticket(0x13, 4096));
    e.serve(&x, "x1", p, 10);
    e.serve(&y, "y1", 4 * p, 20);
    e.serve(&z, "z1", 2 * p, 30);
    let h = |t: &[u8; 32]| hex::encode(t);
    assert_eq!(e.gw.settle_order(), vec![h(&x), h(&y), h(&z)], "no pressure: oldest waiting first");
    let bpr = e.gw.bytes_per_row();
    let e2 = env("order2", |c| c.max_request_data_bytes = 10 * p as u64 * bpr);
    let (x, y, z) = (e2.ticket(0x11, 4096), e2.ticket(0x12, 4096), e2.ticket(0x13, 4096));
    e2.serve(&x, "x1", p, 10);
    e2.serve(&y, "y1", 4 * p, 20);
    e2.serve(&z, "z1", 2 * p, 30);
    assert_eq!(e2.gw.settle_order(), vec![h(&y), h(&z), h(&x)], "over half the cap: most waiting rows first");
    e2.gw.ledger.lock().unwrap().tickets.get_mut(&h(&x)).unwrap().posting = Some(PostingRound { round: 0, m: p, requests: vec!["x1".into()] });
    assert_eq!(e2.gw.settle_order()[0], h(&x), "a round in progress first");
}

#[test]
fn replacement_fee_and_errors() {
    assert_eq!(replacement_fee(1000), 1100);
    assert_eq!(replacement_fee(5), 6);
    assert!(is_nonce_pooled("send PostSegments 0: sendTx: a tx with this nonce is pooled with a fee >= this one"));
    assert!(is_nonce_pooled("sendTx: replacement fee 1050 below 1100 (10% above the pooled tx)"));
    assert!(!is_nonce_pooled("sendTx: already pooled"));
}
