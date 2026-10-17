//! pearl-miner v2: a block producer for the agent chain (SPEC v2 section 3).
//!
//! Loop: `getBlockTemplate(payout)` -> the template names a job (load A, B from its
//! `data_uri`, `file://` in M1) or a fallback block -> iterate the header nonce on
//! all threads (each nonce is a new `pow_digest` and a full noised GEMM) ->
//! `submitBlock`. A stats line is logged every `stats_secs`.
//!
//! `deliver`: for each job this miner got a block accepted on, compute C = AB, send
//! SubmitResult, wait until the job is Delivered to us, ask the attester for the
//! Result signature (SPEC 4.1), send CompleteJob.

pub mod pool;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_job::freivalds::result_hash;
use pearl_job::pow::{mine_header_nonce, BlockWork, MinedBlock};
use pearl_job::{data, PreparedJob};
use pearl_node::tx::{self, Body, Tx};
use pearl_node::{b58, NodeApi, SubmitOutcome, Template};

/// Signs Result messages for CompleteJob (the attester service, or an in-process one).
pub trait ResultAttester: Send + Sync {
    /// `(attester pubkey, signature over SPEC 4.1 result message)`.
    fn attest(&self, job_id: &[u8; 32], deliverer: &[u8; 32], result_uri: &str) -> Result<([u8; 32], [u8; 64])>;
}

/// POST {url}/result to pearl-attester.
pub struct HttpAttester {
    pub url: String,
    agent: ureq::Agent,
}

impl HttpAttester {
    pub fn new(url: &str) -> Self {
        Self { url: url.trim_end_matches('/').to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).build() }
    }
}

/// Parse an attester response (`attester` base58, `signature_hex`).
pub fn parse_attestation(v: &Value) -> Result<([u8; 32], [u8; 64])> {
    if v["ok"].as_bool() != Some(true) {
        bail!("attester refused: {}", v["error"]);
    }
    let att = pearl_job::spec::parse_key32(v["attester"].as_str().ok_or_else(|| anyhow!("no attester field"))?)?;
    let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap_or(""))?
        .try_into()
        .map_err(|_| anyhow!("signature is not 64 bytes"))?;
    Ok((att, sig))
}

impl ResultAttester for HttpAttester {
    fn attest(&self, job_id: &[u8; 32], deliverer: &[u8; 32], result_uri: &str) -> Result<([u8; 32], [u8; 64])> {
        let body = json!({"job": hex::encode(job_id), "deliverer": b58(deliverer), "result_uri": result_uri});
        let v: Value = match self.agent.post(&format!("{}/result", self.url)).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(_, r)) => r.into_json()?,
            Err(e) => bail!("attester: {e}"),
        };
        parse_attestation(&v)
    }
}

#[derive(Debug, Clone)]
pub struct MinerConfig {
    pub threads: usize,
    /// Re-fetch the template at least this often while mining.
    pub refresh: Duration,
    pub stats_every: Duration,
    pub deliver: bool,
    pub results_dir: PathBuf,
    /// Tx fee (raised to the template's min_fee).
    pub fee: u64,
    /// Overrides the template's chain_id.
    pub chain_id: Option<u32>,
    /// Stop after this many accepted blocks.
    pub max_blocks: Option<u64>,
    /// Retry a delivery step after this long without progress.
    pub deliver_retry: Duration,
    /// Deliver C as a `blob:` (putBlob to the node) instead of a local `file://` URI.
    /// Falls back to file:// if the node refuses the blob.
    pub result_blob: bool,
}

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8),
            refresh: Duration::from_secs(2),
            stats_every: Duration::from_secs(10),
            deliver: false,
            results_dir: PathBuf::from("results"),
            fee: 0,
            chain_id: None,
            max_blocks: None,
            deliver_retry: Duration::from_secs(30),
            result_blob: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Needs SubmitResult.
    Submit,
    /// SubmitResult sent at this time.
    Submitted(Instant),
    /// CompleteJob sent at this time.
    Completing(Instant),
}

struct Delivery {
    stage: Stage,
    result_hash: [u8; 32],
    uri: String,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub nonces: u64,
    pub found: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub fallback_accepted: u64,
    pub completed: u64,
}

pub type Logger = Box<dyn Fn(Value) + Send + Sync>;

pub struct Miner<'a> {
    pub node: &'a dyn NodeApi,
    pub attester: Option<&'a dyn ResultAttester>,
    pub key: SigningKey,
    pub cfg: MinerConfig,
    pub stats: Stats,
    log: Logger,
    prepared: HashMap<[u8; 32], Arc<PreparedJob>>,
    /// Jobs whose data failed to load: skip until this time.
    bad_jobs: HashMap<[u8; 32], Instant>,
    deliveries: BTreeMap<[u8; 32], Delivery>,
    nonce: AtomicU64,
    nonces_done: AtomicU64,
    ticker: (Instant, u64),
}

fn stats_tick(ticker: &mut (Instant, u64), stats: &mut Stats, done: &AtomicU64, cfg: &MinerConfig, log: &Logger, height: u64) {
    if ticker.0.elapsed() < cfg.stats_every {
        return;
    }
    let n = done.load(Ordering::Relaxed);
    let dt = ticker.0.elapsed().as_secs_f64();
    stats.nonces = n;
    log(json!({"event": "stats", "height": height, "nonces_per_s": (n - ticker.1) as f64 / dt,
               "nonces": n, "found": stats.found, "accepted": stats.accepted,
               "rejected": stats.rejected, "completed": stats.completed, "threads": cfg.threads}));
    *ticker = (Instant::now(), n);
}

impl<'a> Miner<'a> {
    pub fn new(node: &'a dyn NodeApi, attester: Option<&'a dyn ResultAttester>, key: SigningKey, cfg: MinerConfig, log: Logger) -> Self {
        // Start nonces at a random point so restarts do not redo the same headers.
        let start = u64::from_le_bytes(blake3_seed()[..8].try_into().unwrap()) >> 1;
        Self {
            node,
            attester,
            key,
            cfg,
            stats: Stats::default(),
            log,
            prepared: HashMap::new(),
            bad_jobs: HashMap::new(),
            deliveries: BTreeMap::new(),
            nonce: AtomicU64::new(start),
            nonces_done: AtomicU64::new(0),
            ticker: (Instant::now(), 0),
        }
    }

    pub fn me(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    fn log(&self, v: Value) {
        (self.log)(v)
    }

    /// Work for a template: the job's prepared matrices (cached) or a fallback block.
    fn work_for(&mut self, t: &Template) -> Result<Option<Arc<PreparedJob>>> {
        if t.header.is_fallback() {
            return Ok(None);
        }
        let id = t.header.job_id;
        if let Some(p) = self.prepared.get(&id) {
            return Ok(Some(p.clone()));
        }
        let job = t.job.as_ref().ok_or_else(|| anyhow!("template names a job but carries no job object"))?;
        let d = pearl_node::load_job_data(self.node, &job.data_uri).with_context(|| format!("load job data {}", job.data_uri))?;
        let prep = Arc::new(PreparedJob::new(job.to_job(), &d.a, &d.b)?);
        self.log(json!({"event": "job_loaded", "job": hex::encode(id), "m": job.m, "n": job.n, "data_uri": job.data_uri}));
        self.prepared.insert(id, prep.clone());
        Ok(Some(prep))
    }

    /// Mine `t` on all threads until a block is found or `until` passes.
    pub fn mine_template(&mut self, t: &Template, prep: Option<&PreparedJob>, until: Instant) -> Option<MinedBlock> {
        let work = match prep {
            Some(p) => BlockWork::Job(p),
            None => BlockWork::Fallback(t.fallback),
        };
        let stop = AtomicBool::new(false);
        let found: Mutex<Option<MinedBlock>> = Mutex::new(None);
        let err: Mutex<Option<String>> = Mutex::new(None);
        let header = t.header;
        let (nonce, done) = (&self.nonce, &self.nonces_done);
        let (work, stop, found, err) = (&work, &stop, &found, &err);
        std::thread::scope(|s| {
            for _ in 0..self.cfg.threads.max(1) {
                s.spawn(move || {
                    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
                        let n = nonce.fetch_add(1, Ordering::Relaxed);
                        let r = mine_header_nonce(&header, work, n);
                        done.fetch_add(1, Ordering::Relaxed);
                        match r {
                            Ok(Some(b)) => {
                                found.lock().unwrap().get_or_insert(b);
                                stop.store(true, Ordering::Relaxed);
                            }
                            Ok(None) => {}
                            Err(e) => {
                                *err.lock().unwrap() = Some(format!("{e:#}"));
                                stop.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                });
            }
            while !stop.load(Ordering::Relaxed) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
                stats_tick(&mut self.ticker, &mut self.stats, done, &self.cfg, &self.log, header.height);
            }
        });
        if let Some(e) = err.lock().unwrap().take() {
            self.log(json!({"event": "mine_error", "job": (!t.header.is_fallback()).then(|| hex::encode(t.header.job_id)), "error": e}));
            // A job that cannot be mined must not stall block production: skip it for a
            // while (the next template falls back, see `step`).
            if !t.header.is_fallback() {
                self.bad_jobs.insert(t.header.job_id, Instant::now() + Duration::from_secs(60));
            }
        }
        stats_tick(&mut self.ticker, &mut self.stats, &self.nonces_done, &self.cfg, &self.log, header.height);
        let b = found.lock().unwrap().take();
        if b.is_some() {
            self.stats.found += 1;
        }
        b
    }

    /// One round: template, mine until found or refresh, submit. Returns the outcome
    /// when a block was submitted.
    pub fn step(&mut self) -> Result<Option<SubmitOutcome>> {
        let me = self.me();
        let mut t = self.node.block_template(&me)?;
        let skip = |m: &Self, t: &Template| !t.header.is_fallback() && m.bad_jobs.get(&t.header.job_id).is_some_and(|u| Instant::now() < *u);
        if !skip(self, &t) {
            if let Err(e) = self.work_for(&t) {
                self.log(json!({"event": "job_unusable", "job": hex::encode(t.header.job_id), "error": format!("{e:#}")}));
                self.bad_jobs.insert(t.header.job_id, Instant::now() + Duration::from_secs(60));
            }
        }
        if skip(self, &t) {
            // The node chose a job whose data we cannot load: ask for a fallback block.
            t = self.node.fallback_template(&me)?;
        }
        if t.header.miner != me {
            bail!("template pays {} not {}", b58(&t.header.miner), b58(&me));
        }
        let prep = self.work_for(&t)?;
        let until = Instant::now() + self.cfg.refresh;
        let Some(b) = self.mine_template(&t, prep.as_deref(), until) else { return Ok(None) };
        let out = self.node.submit_block(&t, &b)?;
        let job = (!b.header.is_fallback()).then(|| hex::encode(b.header.job_id));
        match &out {
            SubmitOutcome::Accepted { block_hash } => {
                self.stats.accepted += 1;
                if b.header.is_fallback() {
                    self.stats.fallback_accepted += 1;
                }
                self.log(json!({"event": "block_accepted", "height": b.header.height, "block_hash": hex::encode(block_hash),
                                "nonce": b.header.nonce, "job": job, "proof_len": b.proof.len()}));
                if self.cfg.deliver && !b.header.is_fallback() && !self.deliveries.contains_key(&b.header.job_id) {
                    if let Some(p) = &prep {
                        self.queue_delivery(b.header.job_id, p)?;
                    }
                }
            }
            SubmitOutcome::Rejected { reason } => {
                self.stats.rejected += 1;
                self.log(json!({"event": "block_rejected", "height": b.header.height, "nonce": b.header.nonce, "job": job, "reason": reason}));
            }
        }
        Ok(Some(out))
    }

    /// `id` is the chain job id (the block header's job_id); the prepared job's Pearl header
    /// id differs from it for storefront orders.
    fn queue_delivery(&mut self, id: [u8; 32], prep: &PreparedJob) -> Result<()> {
        let c = prep.product();
        let rh = result_hash(&prep.job_key, &c);
        std::fs::create_dir_all(&self.cfg.results_dir)?;
        let path = std::fs::canonicalize(&self.cfg.results_dir)?.join(format!("{}.i32", hex::encode(id)));
        data::write_i32(&path, &c)?;
        let mut uri = format!("file://{}", path.display());
        if self.cfg.result_blob {
            match self.node.put_blob(&pearl_job::freivalds::c_to_bytes(&c)) {
                Ok(u) => uri = u,
                Err(e) => self.log(json!({"event": "result_blob_failed", "job": hex::encode(id), "error": format!("{e:#}")})),
            }
        }
        self.log(json!({"event": "result_computed", "job": hex::encode(id), "result_hash": hex::encode(rh), "result_uri": uri}));
        self.deliveries.insert(id, Delivery { stage: Stage::Submit, result_hash: rh, uri });
        Ok(())
    }

    fn send(&self, chain_id: u32, fee: u64, body: Body) -> Result<String> {
        let me = self.me();
        let acct = self.node.get_account(&me)?;
        let wire = tx::sign(&Tx { chain_id, from: me, nonce: acct.nonce, fee, body }, &self.key)?;
        self.node.send_tx(&wire)
    }

    /// Advance every pending delivery by at most one step.
    pub fn deliver_step(&mut self, t: &Template) -> Result<()> {
        if self.deliveries.is_empty() {
            return Ok(());
        }
        let chain_id = self.cfg.chain_id.or(t.chain_id).ok_or_else(|| anyhow!("no chain_id (template has none; pass --chain-id)"))?;
        let fee = self.cfg.fee.max(t.min_fee);
        let me = self.me();
        let ids: Vec<[u8; 32]> = self.deliveries.keys().copied().collect();
        for id in ids {
            let jhex = hex::encode(id);
            let Some(job) = self.node.get_job(&id)? else {
                self.log(json!({"event": "deliver_dropped", "job": jhex, "reason": "job not found"}));
                self.deliveries.remove(&id);
                continue;
            };
            let d = self.deliveries.get(&id).unwrap();
            let (stage, rh, uri) = (d.stage, d.result_hash, d.uri.clone());
            let stale = |since: Instant| since.elapsed() >= self.cfg.deliver_retry;
            if job.is("Completed") || job.is("Expired") {
                if job.is("Completed") && job.deliverer == Some(me) {
                    self.stats.completed += 1;
                    self.log(json!({"event": "completed", "job": jhex}));
                } else {
                    self.log(json!({"event": "deliver_dropped", "job": jhex, "reason": format!("job is {}", job.status)}));
                }
                self.deliveries.remove(&id);
            } else if job.is("Open") {
                if matches!(stage, Stage::Submit) || matches!(stage, Stage::Submitted(s) if stale(s)) {
                    let body = Body::SubmitResult { job_id: id, result_hash: rh, uri: uri.clone() };
                    match self.send(chain_id, fee, body) {
                        Ok(h) => {
                            self.log(json!({"event": "result_submitted", "job": jhex, "tx": h, "result_hash": hex::encode(rh)}));
                            self.deliveries.get_mut(&id).unwrap().stage = Stage::Submitted(Instant::now());
                        }
                        Err(e) => self.log(json!({"event": "submit_result_failed", "job": jhex, "error": format!("{e:#}")})),
                    }
                }
            } else if job.is("Delivered") {
                if job.deliverer != Some(me) {
                    self.log(json!({"event": "deliver_dropped", "job": jhex, "reason": "delivered by someone else"}));
                    self.deliveries.remove(&id);
                    continue;
                }
                if job.result_hash.is_some_and(|h| h != rh) {
                    self.log(json!({"event": "deliver_dropped", "job": jhex, "reason": "on-chain result_hash differs from ours"}));
                    self.deliveries.remove(&id);
                    continue;
                }
                if matches!(stage, Stage::Completing(s) if !stale(s)) {
                    continue;
                }
                let Some(att) = self.attester else {
                    self.log(json!({"event": "deliver_blocked", "job": jhex, "reason": "no attester configured"}));
                    continue;
                };
                let r = att.attest(&id, &me, &uri).and_then(|(pk, sig)| {
                    self.send(chain_id, fee, Body::CompleteJob { job_id: id, sigs: vec![(pk, sig)] }).map(|h| (pk, h))
                });
                match r {
                    Ok((pk, h)) => {
                        self.log(json!({"event": "complete_sent", "job": jhex, "tx": h, "attester": b58(&pk)}));
                        self.deliveries.get_mut(&id).unwrap().stage = Stage::Completing(Instant::now());
                    }
                    Err(e) => self.log(json!({"event": "complete_failed", "job": jhex, "error": format!("{e:#}")})),
                }
            }
        }
        Ok(())
    }

    /// Header nonces tried so far (all threads).
    pub fn nonces_done(&self) -> u64 {
        self.nonces_done.load(Ordering::Relaxed)
    }

    pub fn pending_deliveries(&self) -> usize {
        self.deliveries.len()
    }

    /// Main loop until `max_blocks` accepted blocks and all pending deliveries are
    /// done, or `stop` returns true.
    pub fn run(&mut self, stop: &dyn Fn() -> bool) -> Result<()> {
        self.log(json!({"event": "start", "miner": b58(&self.me()), "threads": self.cfg.threads, "deliver": self.cfg.deliver,
                        "pid": std::process::id()}));
        // Keep mining while deliveries are pending: their txs need blocks.
        while !stop() {
            if self.cfg.max_blocks.is_some_and(|m| self.stats.accepted >= m) && self.deliveries.is_empty() {
                break;
            }
            if let Err(e) = self.step() {
                self.log(json!({"event": "step_failed", "error": format!("{e:#}")}));
                std::thread::sleep(Duration::from_millis(500));
            }
            if self.cfg.deliver && !self.deliveries.is_empty() {
                let me = self.me();
                match self.node.block_template(&me) {
                    Ok(t) => {
                        if let Err(e) = self.deliver_step(&t) {
                            self.log(json!({"event": "deliver_failed", "error": format!("{e:#}")}));
                        }
                    }
                    Err(e) => self.log(json!({"event": "template_failed", "error": format!("{e:#}")})),
                }
            }
        }
        self.stats.nonces = self.nonces_done.load(Ordering::Relaxed);
        self.log(json!({"event": "stop", "accepted": self.stats.accepted, "rejected": self.stats.rejected,
                        "completed": self.stats.completed, "nonces": self.stats.nonces}));
        Ok(())
    }
}

fn blake3_seed() -> [u8; 32] {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut v = t.to_le_bytes().to_vec();
    v.extend_from_slice(&std::process::id().to_le_bytes());
    pearl_job::merkle::blake3_digest(&v, None)
}
