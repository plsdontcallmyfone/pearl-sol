//! Pool mode (`pearl-miner --pool http://127.0.0.1:10663`): work comes from a
//! pearl-pool, not the node. The header pays the pool key; this miner registers its
//! payout key with the pool and mines nonces in the range the pool assigned it. Every
//! nonce whose proof meets the pool's share target is sent as a share; the pool checks
//! it and submits it to the node if it also meets the block target.
//!
//! A nonce's first winning tile at the share target may miss the block target while a
//! later tile hits it, so such a nonce is rescanned at the block target: pool mode
//! finds every block solo mode would.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use pearl_job::pow::{block_proof_to_bytes, fallback_job, verify_block_proof, BlockHeader, FallbackParams};
use pearl_job::{data, PreparedJob};
use pearl_node::b58;
use pearl_pool::client::{PoolApi, PoolWork};
use pearl_pool::ShareOutcome;

use crate::Logger;

#[derive(Debug, Default, Clone, Copy)]
pub struct PoolStats {
    pub nonces: u64,
    pub shares: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub stale: u64,
    pub blocks: u64,
}

#[derive(Debug, Clone)]
pub struct PoolMinerConfig {
    pub threads: usize,
    /// Re-fetch work at least this often.
    pub refresh: Duration,
    pub stats_every: Duration,
    /// Stop after this many accepted shares.
    pub max_shares: Option<u64>,
}

impl Default for PoolMinerConfig {
    fn default() -> Self {
        Self {
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8),
            refresh: Duration::from_secs(2),
            stats_every: Duration::from_secs(10),
            max_shares: None,
        }
    }
}

/// One nonce at the share target: `Some(proof bytes)` if it is a share. Rescans at the
/// block target when the share tile is not a block.
pub fn mine_share_nonce(header: &BlockHeader, prep: Option<&PreparedJob>, fallback: &FallbackParams, share_bits: u32, nonce: u64) -> Result<Option<Vec<u8>>> {
    let mut h = *header;
    h.nonce = nonce;
    h.proof_hash = [0; 32];
    let pd = h.pow_digest();
    let fb;
    let p: &PreparedJob = match prep {
        Some(p) => p,
        None => {
            fb = fallback_job(&pd, fallback)?;
            &fb
        }
    };
    let Some(proof) = p.mine_block(&pd, share_bits, nonce) else { return Ok(None) };
    if share_bits != h.target_bits && verify_block_proof(&p.job, &proof, &pd, h.target_bits, nonce).is_err() {
        if let Some(block) = p.mine_block(&pd, h.target_bits, nonce) {
            return Ok(Some(block_proof_to_bytes(&block)));
        }
    }
    Ok(Some(block_proof_to_bytes(&proof)))
}

pub struct PoolMiner<'a> {
    pub pool: &'a dyn PoolApi,
    pub payout: [u8; 32],
    pub cfg: PoolMinerConfig,
    pub stats: PoolStats,
    log: Logger,
    prepared: HashMap<[u8; 32], Arc<PreparedJob>>,
    bad_jobs: HashMap<[u8; 32], Instant>,
    /// Next nonce per work id (resumes when the same work comes back).
    cursor: HashMap<String, u64>,
    nonces_done: AtomicU64,
    ticker: Instant,
    ticker_nonces: u64,
}

impl<'a> PoolMiner<'a> {
    pub fn new(pool: &'a dyn PoolApi, payout: [u8; 32], cfg: PoolMinerConfig, log: Logger) -> Self {
        Self {
            pool,
            payout,
            cfg,
            stats: PoolStats::default(),
            log,
            prepared: HashMap::new(),
            bad_jobs: HashMap::new(),
            cursor: HashMap::new(),
            nonces_done: AtomicU64::new(0),
            ticker: Instant::now(),
            ticker_nonces: 0,
        }
    }

    fn log(&self, v: serde_json::Value) {
        (self.log)(v)
    }

    fn prep_for(&mut self, w: &PoolWork) -> Result<Option<Arc<PreparedJob>>> {
        let t = &w.template;
        if t.header.is_fallback() {
            return Ok(None);
        }
        let id = t.header.job_id;
        if let Some(p) = self.prepared.get(&id) {
            return Ok(Some(p.clone()));
        }
        let job = t.job.as_ref().ok_or_else(|| anyhow!("work names a job but carries no job object"))?;
        let d = data::load(&job.data_uri).with_context(|| format!("load job data {}", job.data_uri))?;
        let prep = Arc::new(PreparedJob::new(job.to_job(), &d.a, &d.b)?);
        self.log(json!({"event": "job_loaded", "job": hex::encode(id), "m": job.m, "n": job.n, "data_uri": job.data_uri}));
        self.prepared.insert(id, prep.clone());
        Ok(Some(prep))
    }

    /// One round: fetch work, mine shares until refresh or stale, submitting each.
    pub fn step(&mut self) -> Result<()> {
        let mut w = self.pool.work(&self.payout, false)?;
        let bad = |m: &Self, w: &PoolWork| !w.template.header.is_fallback() && m.bad_jobs.get(&w.template.header.job_id).is_some_and(|u| Instant::now() < *u);
        if !bad(self, &w) {
            if let Err(e) = self.prep_for(&w) {
                self.log(json!({"event": "job_unusable", "job": hex::encode(w.template.header.job_id), "error": format!("{e:#}")}));
                self.bad_jobs.insert(w.template.header.job_id, Instant::now() + Duration::from_secs(60));
            }
        }
        if bad(self, &w) {
            w = self.pool.work(&self.payout, true)?;
        }
        let prep = self.prep_for(&w)?;
        let start = *self.cursor.get(&w.work_id).unwrap_or(&w.nonce_start);
        let next = AtomicU64::new(start.max(w.nonce_start));
        let stop = AtomicBool::new(false);
        let until = Instant::now() + self.cfg.refresh;
        let tally = Mutex::new(PoolStats::default());
        let err: Mutex<Option<String>> = Mutex::new(None);
        let (pool, payout, log, done) = (self.pool, self.payout, &self.log, &self.nonces_done);
        let max_more = self.cfg.max_shares.map(|m| m.saturating_sub(self.stats.accepted));
        let (wr, prep_ref, next_r, stop_r, tally_r, err_r) = (&w, prep.as_deref(), &next, &stop, &tally, &err);
        std::thread::scope(|s| {
            for _ in 0..self.cfg.threads.max(1) {
                s.spawn(move || {
                    while !stop_r.load(Ordering::Relaxed) && Instant::now() < until {
                        let n = next_r.fetch_add(1, Ordering::Relaxed);
                        if n > wr.nonce_end {
                            stop_r.store(true, Ordering::Relaxed);
                            break;
                        }
                        let r = mine_share_nonce(&wr.template.header, prep_ref, &wr.template.fallback, wr.share_bits, n);
                        done.fetch_add(1, Ordering::Relaxed);
                        let proof = match r {
                            Ok(Some(p)) => p,
                            Ok(None) => continue,
                            Err(e) => {
                                *err_r.lock().unwrap() = Some(format!("{e:#}"));
                                stop_r.store(true, Ordering::Relaxed);
                                break;
                            }
                        };
                        let out = pool.submit_share(&payout, &wr.work_id, n, &proof);
                        let mut t = tally_r.lock().unwrap();
                        t.shares += 1;
                        match out {
                            Ok(ShareOutcome::Accepted { block }) => {
                                t.accepted += 1;
                                if let Some(b) = block {
                                    if b.accepted {
                                        t.blocks += 1;
                                    }
                                    log(json!({"event": "pool_block", "height": b.height, "hash": b.hash, "accepted": b.accepted, "reason": b.reason, "nonce": n}));
                                    // A new tip: this work is now stale.
                                    stop_r.store(true, Ordering::Relaxed);
                                }
                                if max_more.is_some_and(|m| t.accepted >= m) {
                                    stop_r.store(true, Ordering::Relaxed);
                                }
                            }
                            Ok(ShareOutcome::Rejected { reason, stale }) => {
                                t.rejected += 1;
                                if stale {
                                    t.stale += 1;
                                    stop_r.store(true, Ordering::Relaxed);
                                }
                                log(json!({"event": "share_rejected", "nonce": n, "reason": reason, "stale": stale}));
                            }
                            Err(e) => {
                                t.rejected += 1;
                                log(json!({"event": "share_failed", "nonce": n, "error": format!("{e:#}")}));
                            }
                        }
                    }
                });
            }
        });
        self.cursor.retain(|k, _| k == &w.work_id);
        self.cursor.insert(w.work_id.clone(), next.load(Ordering::Relaxed));
        let t = *tally.lock().unwrap();
        self.stats.shares += t.shares;
        self.stats.accepted += t.accepted;
        self.stats.rejected += t.rejected;
        self.stats.stale += t.stale;
        self.stats.blocks += t.blocks;
        self.stats.nonces = self.nonces_done.load(Ordering::Relaxed);
        if let Some(e) = err.lock().unwrap().take() {
            self.log(json!({"event": "mine_error", "work_id": w.work_id, "error": e}));
            if !w.template.header.is_fallback() {
                self.bad_jobs.insert(w.template.header.job_id, Instant::now() + Duration::from_secs(60));
            }
        }
        if self.ticker.elapsed() >= self.cfg.stats_every {
            let dt = self.ticker.elapsed().as_secs_f64();
            self.log(json!({"event": "stats", "mode": "pool", "height": w.template.header.height, "nonces_per_s": (self.stats.nonces - self.ticker_nonces) as f64 / dt,
                            "nonces": self.stats.nonces, "shares": self.stats.shares, "accepted": self.stats.accepted,
                            "rejected": self.stats.rejected, "stale": self.stats.stale, "blocks": self.stats.blocks, "share_bits": w.share_bits}));
            self.ticker = Instant::now();
            self.ticker_nonces = self.stats.nonces;
        }
        Ok(())
    }

    pub fn run(&mut self, stop: &dyn Fn() -> bool) -> Result<()> {
        let reg = self.pool.register(&self.payout)?;
        self.log(json!({"event": "start", "mode": "pool", "payout": b58(&self.payout), "threads": self.cfg.threads,
                        "registered": reg, "pid": std::process::id()}));
        while !stop() {
            if self.cfg.max_shares.is_some_and(|m| self.stats.accepted >= m) {
                break;
            }
            if let Err(e) = self.step() {
                self.log(json!({"event": "step_failed", "error": format!("{e:#}")}));
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        self.log(json!({"event": "stop", "mode": "pool", "shares": self.stats.shares, "accepted": self.stats.accepted,
                        "rejected": self.stats.rejected, "blocks": self.stats.blocks, "nonces": self.stats.nonces}));
        Ok(())
    }
}
