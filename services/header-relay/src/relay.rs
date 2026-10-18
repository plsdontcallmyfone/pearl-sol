//! The relay pipeline: keeps the Solana light client in step with the agent chain.
//!
//! Why a pipeline: verifying one header is a chain of dependent transactions on one
//! work account (submit, begin + proof upload, then 7-9 verify steps), each of which
//! must be confirmed before the next is sent. One header at a time therefore costs
//! about 12-14 confirmation rounds, and a relay doing that falls behind whenever a
//! round takes longer than block time / 14 (SPEC 5.3 "parallel relay"). The light
//! client accepts a header whose parent is merely submitted, verifies its work on its
//! own work account, and marks it Verified only when the parent is Verified (else
//! WorkDone, promoted later by `connect`). So this relay keeps up to `parallel` headers
//! in flight, each on its own work account, and advances all of them every round:
//!
//! - submit frontier: the next unsubmitted canonical headers, two per tx (a child may
//!   be submitted on a pending parent);
//! - flights: begin + first chunk, the remaining chunks as one batch, then the planned
//!   verify steps one tx at a time (the jackpot step passes the parent, so a header
//!   whose parent is already Verified is Verified at once);
//! - global ops (fire and forget, one batch in flight at a time): `connect` runs of
//!   WorkDone headers, reorg to a heavier verified branch, `mark_final`, `accumulate`,
//!   `close_header` of our old final headers, job registration for new canonical blocks.
//!
//! All progress lives on Solana, so restarts resume (a half-verified header is simply
//! begun again on a fresh pass). Every status poll and account read is batched.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lightclient::client::{self, Step};
#[allow(deprecated)]
use solana_sdk::system_instruction;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
};

use crate::chain::{self, ChainHeader, Src};
use crate::ledger::{self, Ledger, TxState, MAX_CU};

const CANON_LEN: usize = lightclient::CANON_LEN;
/// CU asked for by the small txs (measured in LiteSVM: submit 31.7k each, begin + first
/// chunk 24.9k, a 950-byte chunk 1.5k; with headroom).
const CU_SUBMIT2: u32 = 90_000;
const CU_BEGIN: u32 = 45_000;
const CU_WRITE: u32 = 6_000;
const CU_CONNECT_EACH: u32 = 25_000;
const CU_FINAL_EACH: u32 = 15_000;
const CU_ACC: u32 = 150_000;
const CU_CLOSE_EACH: u32 = 15_000;
const CONNECTS_PER_TX: usize = 6;
const FINALS_PER_TX: usize = 8;
const CLOSES_PER_TX: usize = 6;
const ACC_PER_TX: usize = 24;

pub struct Opts {
    /// Headers verified at the same time (one work account each).
    pub parallel: usize,
    /// Ask for the planned CU (+25%) instead of 1.4M per tx (priority fees are charged
    /// on the limit). Falls back to 1.4M after a compute-budget failure.
    pub exact_cu: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self { parallel: 8, exact_cu: true }
    }
}

/// One outstanding transaction (re-sent with a fresh blockhash if it expires).
struct Tx {
    ixs: Vec<Instruction>,
    cu: u32,
    sig: Option<Signature>,
    bh: Hash,
    sent: Instant,
    done: bool,
}

impl Tx {
    fn new(ixs: Vec<Instruction>, cu: u32) -> Self {
        Self { ixs, cu, sig: None, bh: Hash::default(), sent: Instant::now(), done: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Begin,
    Writes,
    Step(usize),
    /// Last step confirmed; the status read decides.
    Finished,
}

struct Flight {
    hd: ChainHeader,
    slot: usize,
    stage: Stage,
    txs: Vec<Tx>,
    plan: Vec<(Vec<Step>, u64)>,
    job: Option<[u8; 32]>,
    started: Instant,
    sent: u32,
    fails: u32,
    failed: Option<String>,
}

/// Lag and throughput, for the log line and the tests.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub node_tip: u64,
    pub lc_tip: u64,
    pub lc_final: u64,
    pub inflight: usize,
    /// Headers this relay verified (own work steps completed).
    pub verified: u64,
    pub connected: u64,
    pub txs: u64,
    pub failed_txs: u64,
}

impl Stats {
    pub fn lag(&self) -> u64 {
        self.node_tip.saturating_sub(self.lc_tip)
    }
}

/// Header record fields the relay needs: (status, is_final, relayer, cum_work LE).
type Rec = (u8, bool, Pubkey, [u8; 32]);

fn rec_of(d: &[u8]) -> Option<Rec> {
    use lightclient::layout::*;
    if d.len() < LEN {
        return None;
    }
    Some((d[STATUS], d[IS_FINAL] == 1, Pubkey::new_from_array(d[424..456].try_into().unwrap()), d[CUM_WORK..CUM_WORK + 32].try_into().unwrap()))
}

fn u256(b: &[u8; 32]) -> pearl_verify_sbf::U256 {
    pearl_verify_sbf::difficulty::u256_from_le(b)
}

pub struct Relay<'a, L: Ledger, S: Src> {
    pub src: S,
    pub ledger: &'a mut L,
    works: Vec<Keypair>,
    free: Vec<usize>,
    flights: Vec<Flight>,
    /// Node canonical headers by height (window + recent history).
    cache: BTreeMap<u64, ChainHeader>,
    submit: Option<Tx>,
    submit_retry: Option<Instant>,
    global: Vec<Tx>,
    /// Headers we submitted that left the node's canonical chain: closed below the floor.
    abandoned: Vec<(u64, [u8; 32])>,
    skip: HashMap<[u8; 32], Instant>,
    closed_upto: Option<u64>,
    jobs_upto: Option<u64>,
    opts: Opts,
    full_cu: bool,
    pub stats: Stats,
    last_tip_check: Instant,
}

impl<'a, L: Ledger, S: Src> Relay<'a, L, S> {
    /// `works`: one keypair per parallel slot (accounts created by `ensure_works`).
    pub fn new(src: S, ledger: &'a mut L, works: Vec<Keypair>, opts: Opts) -> Self {
        let free = (0..works.len()).rev().collect();
        Self {
            src,
            ledger,
            works,
            free,
            flights: vec![],
            cache: BTreeMap::new(),
            submit: None,
            submit_retry: None,
            global: vec![],
            abandoned: vec![],
            skip: HashMap::new(),
            closed_upto: None,
            jobs_upto: None,
            opts,
            full_cu: false,
            stats: Stats::default(),
            last_tip_check: Instant::now(),
        }
    }

    pub fn config(&self) -> Result<lightclient::Config> {
        use anchor_lang::AccountDeserialize;
        let d = self.ledger.account(&client::config_pda())?.context("light client not initialized")?;
        Ok(lightclient::Config::try_deserialize(&mut d.as_slice())?)
    }

    fn canon(&self) -> Result<Vec<[u8; 32]>> {
        let d = self.ledger.account(&client::canon_pda())?.context("no canon account")?;
        Ok(d[8..].chunks(32).map(|c| c.try_into().unwrap()).collect())
    }

    /// Create and init the work accounts that do not exist yet.
    pub fn ensure_works(&mut self) -> Result<()> {
        let size = client::work_size_for(&self.config()?.params);
        let keys: Vec<Pubkey> = self.works.iter().map(|k| k.pubkey()).collect();
        let have = self.ledger.accounts(&keys)?;
        for (i, h) in have.iter().enumerate() {
            if h.is_some() {
                continue;
            }
            let rent = self.ledger.rent(size)?;
            let w = self.works[i].insecure_clone();
            let pk = self.ledger.pk();
            let create = system_instruction::create_account(&pk, &w.pubkey(), rent, size as u64, &lightclient::ID);
            ledger::send_wait(self.ledger, &[create, client::init_work(pk, w.pubkey(), 32 * 1024)], &[&w], 200_000)?;
        }
        Ok(())
    }

    fn cu(&self, est: u32) -> u32 {
        if self.opts.exact_cu && !self.full_cu {
            est.min(MAX_CU)
        } else {
            MAX_CU
        }
    }

    fn fire(ledger: &mut L, t: &mut Tx, bh: Hash, stats: &mut Stats) -> Result<()> {
        let tx = ledger::build(ledger, &t.ixs, &[], t.cu, bh);
        t.sig = Some(ledger.submit(tx)?);
        t.bh = bh;
        t.sent = Instant::now();
        stats.txs += 1;
        Ok(())
    }

    fn node_hash(&mut self, h: u64) -> Result<Option<[u8; 32]>> {
        if let Some(c) = self.cache.get(&h) {
            return Ok(Some(c.hash));
        }
        match self.src.header_at(h)? {
            Some(hd) => {
                let hash = hd.hash;
                self.cache.insert(h, hd);
                Ok(Some(hash))
            }
            None => Ok(None),
        }
    }

    /// Fork point: highest height <= the light-client tip where the node agrees with it.
    fn base(&mut self, cfg: &lightclient::Config, canon: &[[u8; 32]]) -> Result<u64> {
        let mut h = cfg.tip_height;
        loop {
            if h <= cfg.final_floor || cfg.tip_height - h >= CANON_LEN as u64 - 1 {
                return Ok(h);
            }
            if self.node_hash(h)? == Some(canon[(h as usize) % CANON_LEN]) {
                return Ok(h);
            }
            h -= 1;
        }
    }

    /// Fill the header cache for (base, top]; drop it when the node reorged under it.
    fn refresh(&mut self, base: u64, top: u64, node_tip: (u64, [u8; 32])) -> Result<()> {
        let mut h = base + 1;
        while h <= top {
            if !self.cache.contains_key(&h) {
                match self.src.header_at(h)? {
                    Some(hd) => {
                        self.cache.insert(h, hd);
                    }
                    None => break,
                }
            }
            h += 1;
        }
        // prev links inside the window, and the node's tip (cheap) or the window top
        // (every few seconds while behind) must match what we hold.
        let mut stale = false;
        for h in base + 1..=top {
            if let (Some(c), Some(p)) = (self.cache.get(&h), self.cache.get(&(h - 1))) {
                if c.prev() != p.hash {
                    stale = true;
                    break;
                }
            }
        }
        if !stale {
            if let Some(c) = self.cache.get(&node_tip.0) {
                stale = c.hash != node_tip.1;
            } else if top > base && self.last_tip_check.elapsed() > Duration::from_secs(5) {
                self.last_tip_check = Instant::now();
                if let (Some(c), Some(n)) = (self.cache.get(&top), self.src.header_at(top)?) {
                    stale = c.hash != n.hash;
                }
            }
        }
        if stale {
            eprintln!("relay: node chain changed under the cache (reorg); refetching");
            self.cache.clear();
        }
        let keep = base.saturating_sub(CANON_LEN as u64 + 64);
        self.cache = self.cache.split_off(&keep);
        Ok(())
    }

    /// One pass: poll, refresh, submit, start/advance flights, global ops. Never blocks
    /// on a confirmation except for job registration and work-account setup.
    /// Returns true while there is work in flight or a backlog.
    pub fn pass(&mut self) -> Result<bool> {
        let bh = self.ledger.blockhash()?;
        self.poll(bh)?;
        let cfg = self.config()?;
        let canon = self.canon()?;
        let node_tip = self.src.tip()?;
        let base = self.base(&cfg, &canon)?;
        let lookahead = (self.works.len() * 3).clamp(8, 96) as u64;
        let top = node_tip.0.min(base + lookahead);
        self.refresh(base, top, node_tip)?;
        let base = self.base(&cfg, &canon)?;
        let want: Vec<ChainHeader> = (base + 1..=top).map_while(|h| self.cache.get(&h).cloned()).collect();
        let mut addrs: Vec<Pubkey> = want.iter().map(|h| client::header_addr(&h.hash)).collect();
        addrs.push(client::header_addr(&self.cache.get(&base).map(|c| c.hash).unwrap_or(canon[(base as usize) % CANON_LEN])));
        let mut recs: Vec<Option<Rec>> = self.ledger.accounts(&addrs)?.into_iter().map(|d| d.and_then(|d| rec_of(&d))).collect();
        let base_rec = recs.pop().flatten();
        self.skip.retain(|_, t| t.elapsed() < Duration::from_secs(30));

        // finished / failed / abandoned flights
        let status_of = |hash: &[u8; 32]| want.iter().position(|w| w.hash == *hash).map(|i| recs[i]);
        let inflight = self.flights.len();
        let mut i = 0;
        while i < self.flights.len() {
            let f = &mut self.flights[i];
            let st = status_of(&f.hd.hash);
            let done = match st {
                None => {
                    // left the node's canonical chain (or the window)
                    self.abandoned.push((f.hd.height, f.hd.hash));
                    true
                }
                Some(Some((s, ..))) if s != lightclient::STATUS_PENDING => {
                    if s == lightclient::STATUS_WORK_DONE || s == lightclient::STATUS_VERIFIED {
                        self.stats.verified += 1;
                        let label = if s == lightclient::STATUS_VERIFIED { "verified" } else { "work-verified" };
                        println!(
                            "{label} {} {} (txs {}, {} ms, in flight {inflight})",
                            f.hd.height,
                            hex::encode(f.hd.hash),
                            f.sent,
                            f.started.elapsed().as_millis(),
                        );
                    }
                    true
                }
                _ => {
                    if let Some(e) = f.failed.take() {
                        f.fails += 1;
                        eprintln!("relay: header {} attempt {} failed: {e}", f.hd.height, f.fails);
                        if f.fails >= 3 {
                            self.skip.insert(f.hd.hash, Instant::now());
                            true
                        } else {
                            f.stage = Stage::Begin;
                            f.txs.clear();
                            false
                        }
                    } else if f.stage == Stage::Finished {
                        f.stage = Stage::Begin; // the jackpot did not land: redo
                        f.txs.clear();
                        false
                    } else {
                        false
                    }
                }
            };
            if done {
                let f = self.flights.swap_remove(i);
                self.free.push(f.slot);
            } else {
                i += 1;
            }
        }

        // submit frontier: the first unsubmitted headers, two per tx
        if self.submit.is_none() && base_rec.is_some() && self.submit_retry.map_or(true, |t| Instant::now() >= t) {
            if let Some(i) = recs.iter().position(|r| r.is_none()) {
                let pk = self.ledger.pk();
                let mut ixs = vec![client::submit_header(pk, want[i].bytes)];
                if i + 1 < want.len() && recs[i + 1].is_none() {
                    ixs.push(client::submit_header(pk, want[i + 1].bytes));
                }
                let mut t = Tx::new(ixs, self.cu(CU_SUBMIT2));
                Self::fire(self.ledger, &mut t, bh, &mut self.stats)?;
                self.submit = Some(t);
            }
        }

        // start flights on pending headers, lowest first
        let p = cfg.params;
        for (i, hd) in want.iter().enumerate() {
            if self.free.is_empty() {
                break;
            }
            if !matches!(recs[i], Some((lightclient::STATUS_PENDING, ..))) || self.skip.contains_key(&hd.hash) || self.flights.iter().any(|f| f.hd.hash == hd.hash) {
                continue;
            }
            let (job, config) = if hd.is_fallback() {
                (None, p.fallback_config)
            } else {
                let j = hd.job_id();
                let Some(data) = self.ledger.account(&client::job_addr(&j))? else {
                    continue; // its job is registered once the posting block is canonical
                };
                let jc = <lightclient::JobCommit as anchor_lang::AccountDeserialize>::try_deserialize(&mut data.as_slice())?;
                (Some(j), jc.config)
            };
            let shape = client::Shape::new(hd.is_fallback(), p.fallback_m, p.fallback_n, &config, hd.proof.len());
            let slot = self.free.pop().unwrap();
            self.flights.push(Flight {
                hd: hd.clone(),
                slot,
                stage: Stage::Begin,
                txs: vec![],
                plan: client::plan_costed(&shape, client::TX_CU_BUDGET),
                job,
                started: Instant::now(),
                sent: 0,
                fails: 0,
                failed: None,
            });
        }

        // advance every flight whose current stage is confirmed
        let pk = self.ledger.pk();
        for fi in 0..self.flights.len() {
            if self.flights[fi].txs.iter().any(|t| !t.done) || self.flights[fi].stage == Stage::Finished {
                continue;
            }
            let f = &self.flights[fi];
            let w = self.works[f.slot].pubkey();
            let hash = f.hd.hash;
            let (next, txs): (Stage, Vec<Tx>) = match (f.stage, f.txs.is_empty()) {
                (Stage::Begin, true) => {
                    let first = client::FIRST_CHUNK.min(f.hd.proof.len());
                    let ixs = vec![client::begin_verify(pk, w, &hash, f.job.as_ref(), f.hd.proof.len() as u32), client::write_proof(pk, w, 0, f.hd.proof[..first].to_vec())];
                    (Stage::Begin, vec![Tx::new(ixs, self.cu(CU_BEGIN))])
                }
                (Stage::Begin, false) => {
                    let mut off = client::FIRST_CHUNK.min(f.hd.proof.len());
                    let mut txs = vec![];
                    while off < f.hd.proof.len() {
                        let n = client::WRITE_CHUNK.min(f.hd.proof.len() - off);
                        txs.push(Tx::new(vec![client::write_proof(pk, w, off as u32, f.hd.proof[off..off + n].to_vec())], self.cu(CU_WRITE)));
                        off += n;
                    }
                    if txs.is_empty() {
                        (Stage::Step(0), vec![self.step_tx(fi, 0)])
                    } else {
                        (Stage::Writes, txs)
                    }
                }
                (Stage::Writes, _) => (Stage::Step(0), vec![self.step_tx(fi, 0)]),
                (Stage::Step(s), _) if s + 1 < f.plan.len() => (Stage::Step(s + 1), vec![self.step_tx(fi, s + 1)]),
                (Stage::Step(_), _) => (Stage::Finished, vec![]),
                (Stage::Finished, _) => unreachable!(),
            };
            let f = &mut self.flights[fi];
            f.stage = next;
            f.txs = txs;
            f.sent += f.txs.len() as u32;
            for t in f.txs.iter_mut() {
                Self::fire(self.ledger, t, bh, &mut self.stats)?;
            }
        }

        if self.global.iter().all(|t| t.done) {
            self.global.clear();
            self.global_ops(&cfg, &canon, base, &want, &recs, bh)?;
        }

        self.stats.node_tip = node_tip.0;
        self.stats.lc_tip = cfg.tip_height;
        self.stats.lc_final = cfg.final_height;
        self.stats.inflight = self.flights.len();
        Ok(!self.flights.is_empty() || self.submit.is_some() || !self.global.is_empty() || cfg.tip_height < node_tip.0)
    }

    fn step_tx(&self, fi: usize, s: usize) -> Tx {
        let f = &self.flights[fi];
        let pk = self.ledger.pk();
        let w = self.works[f.slot].pubkey();
        let (steps, est) = &f.plan[s];
        let ixs: Vec<Instruction> = steps.iter().map(|st| st.instruction_with_parent(pk, w, &f.hd.hash, &f.hd.prev(), pk)).collect();
        Tx::new(ixs, self.cu((*est as u32).saturating_mul(5) / 4 + 20_000))
    }

    /// Poll every outstanding signature in one batch; re-send expired ones.
    fn poll(&mut self, bh: Hash) -> Result<()> {
        let mut sigs = vec![];
        for f in &self.flights {
            sigs.extend(f.txs.iter().filter(|t| !t.done).filter_map(|t| t.sig));
        }
        sigs.extend(self.submit.iter().filter_map(|t| t.sig));
        sigs.extend(self.global.iter().filter(|t| !t.done).filter_map(|t| t.sig));
        if sigs.is_empty() {
            return Ok(());
        }
        let states: HashMap<Signature, TxState> = sigs.iter().copied().zip(self.ledger.states(&sigs)?).collect();
        let mut cu_fail = false;
        let mut check = |t: &mut Tx, ledger: &mut L, stats: &mut Stats| -> Result<Option<String>> {
            let Some(sig) = t.sig else { return Ok(None) };
            match states.get(&sig) {
                Some(TxState::Ok) => t.done = true,
                Some(TxState::Failed(e)) => {
                    stats.failed_txs += 1;
                    if e.contains("ComputationalBudgetExceeded") || e.contains("exceeded CUs") {
                        cu_fail = true;
                    }
                    return Ok(Some(e.clone()));
                }
                _ => {
                    if t.sent.elapsed() > Duration::from_secs(20) && !ledger.blockhash_valid(&t.bh)? {
                        Self::fire(ledger, t, bh, stats)?; // expired unconfirmed: send again
                    }
                }
            }
            Ok(None)
        };
        for f in self.flights.iter_mut() {
            for t in f.txs.iter_mut().filter(|t| !t.done) {
                if let Some(e) = check(t, self.ledger, &mut self.stats)? {
                    f.failed.get_or_insert(e);
                }
            }
            if f.failed.is_some() {
                f.txs.clear();
            }
        }
        if let Some(t) = self.submit.as_mut() {
            let r = check(t, self.ledger, &mut self.stats)?;
            if r.is_some() || t.done {
                if let Some(e) = r {
                    eprintln!("relay: submit failed: {e}");
                    self.submit_retry = Some(Instant::now() + Duration::from_secs(2));
                }
                self.submit = None;
            }
        }
        for t in self.global.iter_mut().filter(|t| !t.done) {
            if let Some(e) = check(t, self.ledger, &mut self.stats)? {
                eprintln!("relay: {} failed: {e}", describe(&t.ixs));
                t.done = true;
            }
        }
        if cu_fail && !self.full_cu {
            eprintln!("relay: a tx ran out of compute units; asking for {MAX_CU} from now on");
            self.full_cu = true;
        }
        Ok(())
    }

    fn global_ops(&mut self, cfg: &lightclient::Config, canon: &[[u8; 32]], base: u64, want: &[ChainHeader], recs: &[Option<Rec>], _bh: Hash) -> Result<()> {
        let pk = self.ledger.pk();
        let mut txs: Vec<Tx> = vec![];
        // 1. connect the first run of WorkDone headers whose parent is Verified
        let mut ixs = vec![];
        let mut parent_ok = true;
        for (i, hd) in want.iter().enumerate() {
            match recs[i] {
                Some((lightclient::STATUS_VERIFIED, ..)) => parent_ok = true,
                Some((lightclient::STATUS_WORK_DONE, _, relayer, _)) if parent_ok => {
                    ixs.push(client::connect(&hd.prev(), &hd.hash, relayer));
                    self.stats.connected += 1;
                    if ixs.len() == CONNECTS_PER_TX {
                        break;
                    }
                }
                _ => {
                    if !ixs.is_empty() {
                        break;
                    }
                    parent_ok = false;
                }
            }
        }
        if !ixs.is_empty() {
            let n = ixs.len() as u32;
            txs.push(Tx::new(ixs, self.cu(CU_CONNECT_EACH * n + 10_000)));
        }
        // 2. reorg when the node's branch (diverging at base, below our tip) is verified
        //    and heavier than our tip
        if base < cfg.tip_height {
            let mut best = None;
            for (i, r) in recs.iter().enumerate() {
                match r {
                    Some((lightclient::STATUS_VERIFIED, _, _, cw)) => {
                        if u256(cw) > u256(&cfg.tip_work) {
                            best = Some(i);
                        }
                    }
                    _ => break,
                }
            }
            if let Some(i) = best {
                if i < 30 {
                    let mut path: Vec<[u8; 32]> = want[..i].iter().rev().map(|h| h.hash).collect();
                    path.push(canon[(base as usize) % CANON_LEN]);
                    println!("reorg to {} at {} (fork at {base})", hex::encode(want[i].hash), want[i].height);
                    txs.push(Tx::new(vec![client::reorg(&want[i].hash, &path)], self.cu(200_000)));
                }
            }
        }
        // 3. mark_final up to the final floor
        let lo = cfg.final_height.max(cfg.tip_height.saturating_sub(CANON_LEN as u64 - 1)) + 1;
        if lo <= cfg.final_floor {
            let hashes: Vec<[u8; 32]> = (lo..=cfg.final_floor).map(|h| canon[(h as usize) % CANON_LEN]).collect();
            let addrs: Vec<Pubkey> = hashes.iter().map(client::header_addr).collect();
            let st = self.ledger.accounts(&addrs)?;
            let todo: Vec<Instruction> = hashes
                .iter()
                .zip(st)
                .filter(|(_, d)| matches!(d.as_deref().and_then(rec_of), Some((lightclient::STATUS_VERIFIED, false, ..))))
                .map(|(h, _)| client::mark_final(h))
                .collect();
            for c in todo.chunks(FINALS_PER_TX) {
                txs.push(Tx::new(c.to_vec(), self.cu(CU_FINAL_EACH * c.len() as u32 + 10_000)));
            }
        }
        // 4. accumulate final headers in order (one tx in flight: order matters)
        let acc = self.ledger.account(&client::acc_addr())?.and_then(|d| client::acc_state(&d));
        let acc_next = match acc {
            None => {
                if cfg.admin == pk {
                    ledger::send_wait(self.ledger, &[client::init_accumulator(pk)], &[], 200_000)?;
                    println!("accumulator started at final height {}", cfg.final_height);
                }
                0
            }
            Some((start, count, _)) => {
                let next = start + count;
                let mut hashes = vec![];
                let mut h = next;
                while h <= cfg.final_floor && hashes.len() < ACC_PER_TX {
                    let hash = if cfg.tip_height - h < CANON_LEN as u64 { Some(canon[(h as usize) % CANON_LEN]) } else { self.node_hash(h)? };
                    let Some(hash) = hash else { break };
                    hashes.push(hash);
                    h += 1;
                }
                if !hashes.is_empty() {
                    txs.push(Tx::new(vec![client::accumulate(&hashes)], self.cu(CU_ACC)));
                }
                next
            }
        };
        // 5. close our final headers retain_final below the floor (and accumulated),
        //    and abandoned side-branch headers at or below the floor
        let upto = cfg.final_floor.saturating_sub(cfg.params.retain_final + 1).min(acc_next);
        let from = self.closed_upto.unwrap_or(upto.saturating_sub(512)).max(cfg.checkpoint_height + 1);
        let end = upto.min(from + 64).max(from);
        let mut cands: Vec<[u8; 32]> = vec![];
        for h in from..end {
            let hash = if cfg.tip_height - h < CANON_LEN as u64 { Some(canon[(h as usize) % CANON_LEN]) } else { self.node_hash(h)? };
            cands.extend(hash);
        }
        let n_final = cands.len();
        let (dead, keep): (Vec<_>, Vec<_>) = self.abandoned.drain(..).partition(|(h, _)| *h <= cfg.final_floor);
        self.abandoned = keep;
        cands.extend(dead.iter().map(|(_, x)| *x));
        if !cands.is_empty() {
            let st = self.ledger.accounts(&cands.iter().map(client::header_addr).collect::<Vec<_>>())?;
            let mut todo: Vec<Instruction> = vec![];
            for (i, (x, d)) in cands.iter().zip(st).enumerate() {
                let Some((_, fin, relayer, _)) = d.as_deref().and_then(rec_of) else { continue };
                // our final headers in range; abandoned (non-canonical) headers we submitted
                let ok = if i < n_final { fin } else { canon.iter().all(|c| c != x) };
                if ok && relayer == pk && *x != cfg.final_hash && *x != cfg.tip_hash {
                    todo.push(client::close_header(x, relayer));
                }
            }
            for c in todo.chunks(CLOSES_PER_TX) {
                txs.push(Tx::new(c.to_vec(), self.cu(CU_CLOSE_EACH * c.len() as u32 + 10_000)));
            }
        }
        let next_closed = end;
        self.closed_upto = Some(next_closed);
        // 6. register jobs posted in new canonical blocks (rare; blocking)
        let from = self.jobs_upto.unwrap_or(cfg.tip_height);
        let end = cfg.tip_height.min(from + 32);
        for h in from + 1..=end {
            let hash = if cfg.tip_height - h < CANON_LEN as u64 { Some(canon[(h as usize) % CANON_LEN]) } else { self.node_hash(h)? };
            if let Some(hash) = hash {
                self.register_jobs(h, &hash)?;
            }
        }
        self.jobs_upto = Some(end.max(from));
        // a blocking send above (jobs, accumulator start) may have outlived `bh`
        let bh = self.ledger.blockhash()?;
        for t in txs.iter_mut() {
            Self::fire(self.ledger, t, bh, &mut self.stats)?;
        }
        self.global = txs;
        Ok(())
    }

    fn register_jobs(&mut self, height: u64, hash: &[u8; 32]) -> Result<()> {
        let (txs, cb) = self.src.block_body(hash)?;
        let jobs = chain::jobs_in(&txs);
        if jobs.is_empty() {
            return Ok(());
        }
        let (_, paths) = chain::tx_root_paths(&txs, cb.as_deref());
        for (job_id, i) in jobs {
            if self.ledger.account(&client::job_addr(&job_id))?.is_some() {
                continue;
            }
            let pk = self.ledger.pk();
            match ledger::send_wait(self.ledger, &[client::register_job(pk, hash, job_id, txs[i].clone(), paths[i].clone())], &[], MAX_CU) {
                Ok(_) => println!("registered job {} (posted at {height})", hex::encode(job_id)),
                Err(e) => eprintln!("register job {}: {e:#}", hex::encode(job_id)),
            }
        }
        Ok(())
    }

    /// One JSON line: node tip, light-client tip and final, lag in blocks, in flight.
    pub fn lag_line(&self, headers_per_min: f64) -> String {
        let s = &self.stats;
        serde_json::json!({
            "node_tip": s.node_tip, "lc_tip": s.lc_tip, "lc_final": s.lc_final, "lag_blocks": s.lag(),
            "inflight": s.inflight, "parallel": self.works.len(), "headers_per_min": (headers_per_min * 10.0).round() / 10.0,
            "verified": s.verified, "connected": s.connected, "txs": s.txs, "failed_txs": s.failed_txs,
        })
        .to_string()
    }
}

fn describe(ixs: &[Instruction]) -> &'static str {
    let Some(i) = ixs.first() else { return "tx" };
    let d = &i.data[..8.min(i.data.len())];
    use anchor_lang::Discriminator;
    let is = |x: &[u8]| d == x;
    if is(lightclient::instruction::Connect::DISCRIMINATOR) {
        "connect"
    } else if is(lightclient::instruction::MarkFinal::DISCRIMINATOR) {
        "mark_final"
    } else if is(lightclient::instruction::Accumulate::DISCRIMINATOR) {
        "accumulate"
    } else if is(lightclient::instruction::CloseHeader::DISCRIMINATOR) {
        "close_header"
    } else if is(lightclient::instruction::Reorg::DISCRIMINATOR) {
        "reorg"
    } else {
        "tx"
    }
}
