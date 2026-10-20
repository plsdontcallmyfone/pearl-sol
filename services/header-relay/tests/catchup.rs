//! Catch-up test: the real relay pipeline (src/relay.rs) against the real light-client
//! program in LiteSVM, with simulated slots. Every `pause` is one slot: the queued txs
//! land (in a shuffled order) and are confirmed. A producer adds one agent-chain block
//! every `every` slots. The sequential relay (parallel 1) needs about 12 rounds per
//! header, so it falls behind at one block per 4 slots; the pipelined relay drains a
//! backlog and then keeps the lag bounded.
//!
//! Needs token/target/deploy/lightclient.so built from this tree
//! (`cd token && anchor build -p lightclient`). Run: `cargo test --release --test catchup -- --nocapture`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use header_relay::chain::{ChainHeader, Src};
use header_relay::ledger::{Ledger, TxState};
use header_relay::relay::{Opts, Relay};
use lightclient_litesvm_tests::*;
use litesvm::LiteSVM;
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    transaction::Transaction,
};

struct Sim {
    svm: LiteSVM,
    payer: Keypair,
    queue: Vec<Transaction>,
    results: HashMap<Signature, TxState>,
    slot: u64,
    rng: u64,
    max_tx: usize,
    /// Status-cache loss (0 = off): every Nth tx that lands OK drops out of the recent
    /// status cache at once, so `states` reports it unknown forever (on an RPC node that
    /// happens once its blockhash ages out, while a slow pass has not polled yet).
    lose_every: u64,
    landed_ok: u64,
    lost: HashSet<Signature>,
    /// `history` finds lost txs (an RPC node's searchTransactionHistory). Off: it answers
    /// like `states`, which is what the relay effectively had before it asked history.
    history_finds: bool,
    /// Blockhashes are valid this many slots after first use (0 = forever).
    expiry: u64,
    bh_slot: HashMap<Hash, u64>,
}

impl Sim {
    fn new(svm: LiteSVM, payer: Keypair, rng: u64) -> Self {
        Self { svm, payer, queue: vec![], results: HashMap::new(), slot: 0, rng, max_tx: 0, lose_every: 0, landed_ok: 0, lost: HashSet::new(), history_finds: true, expiry: 0, bh_slot: HashMap::new() }
    }
}

impl Sim {
    fn next(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }
}

impl Ledger for Sim {
    fn payer(&self) -> &Keypair {
        &self.payer
    }
    fn cu_price(&self) -> u64 {
        0
    }
    fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Vec<u8>>>> {
        Ok(keys.iter().map(|k| self.svm.get_account(k).filter(|a| a.lamports > 0).map(|a| a.data)).collect())
    }
    fn blockhash(&self) -> Result<Hash> {
        Ok(self.svm.latest_blockhash())
    }
    fn blockhash_valid(&self, h: &Hash) -> Result<bool> {
        Ok(self.expiry == 0 || self.bh_slot.get(h).is_some_and(|s| self.slot <= s + self.expiry))
    }
    fn submit(&mut self, tx: Transaction) -> Result<Signature> {
        self.bh_slot.entry(tx.message.recent_blockhash).or_insert(self.slot);
        let size = bincode_size(&tx);
        assert!(size <= 1232, "tx too large: {size}");
        self.max_tx = self.max_tx.max(size);
        let sig = tx.signatures[0];
        self.queue.push(tx);
        Ok(sig)
    }
    fn states(&self, sigs: &[Signature]) -> Result<Vec<TxState>> {
        Ok(sigs.iter().map(|s| if self.lost.contains(s) { TxState::Pending } else { self.results.get(s).cloned().unwrap_or(TxState::Pending) }).collect())
    }
    fn history(&self, sigs: &[Signature]) -> Result<Vec<TxState>> {
        if !self.history_finds {
            return self.states(sigs);
        }
        Ok(sigs.iter().map(|s| self.results.get(s).cloned().unwrap_or(TxState::Pending)).collect())
    }
    fn rent(&self, size: usize) -> Result<u64> {
        Ok(self.svm.minimum_balance_for_rent_exemption(size))
    }
    /// One slot: everything queued lands, in a shuffled order (independent txs only:
    /// the relay never sends a tx before the one it depends on is confirmed).
    fn pause(&mut self, _ms: u64) {
        let mut q = std::mem::take(&mut self.queue);
        for i in (1..q.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            q.swap(i, j);
        }
        for tx in q {
            let sig = tx.signatures[0];
            let r = match self.svm.send_transaction(tx) {
                Ok(_) => TxState::Ok,
                Err(f) => TxState::Failed(format!("{:?}", f.err)),
            };
            if r == TxState::Ok && self.lose_every > 0 {
                self.landed_ok += 1;
                if self.landed_ok % self.lose_every == 0 {
                    self.lost.insert(sig);
                }
            }
            self.results.insert(sig, r);
        }
        self.svm.expire_blockhash();
        self.slot += 1;
    }
}

fn bincode_size(tx: &Transaction) -> usize {
    let msg = tx.message.serialize();
    1 + 64 * tx.signatures.len() + msg.len()
}

/// The agent chain as seen by the relay: the first `visible` mined blocks.
#[derive(Clone)]
struct Chain {
    blocks: Rc<RefCell<Vec<Blk>>>,
    visible: Rc<Cell<usize>>,
    fetches: Rc<RefCell<u64>>,
}

impl Src for Chain {
    fn tip(&self) -> Result<(u64, [u8; 32])> {
        let n = self.visible.get();
        Ok(((n - 1) as u64, self.blocks.borrow()[n - 1].hash))
    }
    fn header_at(&self, h: u64) -> Result<Option<ChainHeader>> {
        *self.fetches.borrow_mut() += 1;
        if h as usize >= self.visible.get() {
            return Ok(None);
        }
        let b = &self.blocks.borrow()[h as usize];
        Ok(Some(ChainHeader { bytes: b.bytes, hash: b.hash, height: h, proof: b.proof.clone(), cum_work: None }))
    }
    fn block_body(&self, _: &[u8; 32]) -> Result<(Vec<Vec<u8>>, Option<Vec<u8>>)> {
        Ok((vec![], None))
    }
}

fn small_params() -> lightclient::Params {
    let mut p = params(6);
    p.fallback_m = 4;
    p.fallback_n = 8;
    p.retain_final = 2;
    p
}

fn mine(p: &lightclient::Params, n: usize) -> Vec<Blk> {
    let mut out = vec![genesis(0x1e100000)];
    for _ in 0..n {
        let parent = out.last().unwrap().clone();
        out.push(child(p, &parent, &Work::Fallback, ChildOpts::default()));
    }
    out
}

struct Run {
    /// lag (node tip - light-client tip) after each slot
    lag: Vec<u64>,
    slots: u64,
    lc_tip: u64,
    lc_final: u64,
    acc_next: u64,
    txs: u64,
    failed: u64,
    verified: u64,
    max_tx: usize,
    resends: u64,
    dup_sends: u64,
    /// distinct (error) strings of the txs that landed with an error
    errors: Vec<String>,
}

/// `backlog` blocks visible at the start, then one more every `every` slots up to
/// `total`, then `tail` slots to finish.
fn run(blocks: &Rc<Vec<Blk>>, parallel: usize, backlog: usize, every: u64, max_slots: u64) -> Run {
    run_with(blocks, parallel, backlog, every, max_slots, |_| {})
}

fn run_with(blocks: &Rc<Vec<Blk>>, parallel: usize, backlog: usize, every: u64, max_slots: u64, cfg: impl Fn(&mut Sim)) -> Run {
    let p = small_params();
    let env = Env::new(p, &blocks[0]);
    let mut sim = Sim::new(env.svm, env.payer, 0x9e3779b97f4a7c15);
    cfg(&mut sim);
    let total = blocks.len();
    let chain = Chain { blocks: Rc::new(RefCell::new(blocks.to_vec())), visible: Rc::new(Cell::new(backlog + 1)), fetches: Rc::new(RefCell::new(0)) };
    let visible = chain.visible.clone();
    let works = (0..parallel).map(|_| Keypair::new()).collect();
    let mut r = Relay::new(chain, &mut sim, works, Opts { parallel, exact_cu: true, resend_after: Duration::ZERO });
    r.ensure_works().unwrap();
    let mut lag = vec![];
    let mut slot = 0u64;
    loop {
        if slot > 0 && slot % every == 0 && visible.get() < total {
            visible.set(visible.get() + 1);
        }
        r.pass().unwrap();
        r.ledger.pause(0);
        slot += 1;
        let c = r.config().unwrap();
        lag.push((visible.get() as u64 - 1).saturating_sub(c.tip_height));
        let done = visible.get() == total && c.tip_height as usize == total - 1 && c.final_height + 6 >= c.tip_height;
        if done || slot >= max_slots {
            break;
        }
    }
    // a few more passes so finals / accumulate / close settle
    for _ in 0..12 {
        r.pass().unwrap();
        r.ledger.pause(0);
    }
    let c = r.config().unwrap();
    let acc = r.ledger.account(&lightclient::client::acc_addr()).unwrap().and_then(|d| lightclient::client::acc_state(&d)).unwrap();
    let mut errors: Vec<String> = r.ledger.results.values().filter_map(|s| if let TxState::Failed(e) = s { Some(e.clone()) } else { None }).collect();
    errors.sort();
    errors.dedup();
    Run {
        lag,
        slots: slot,
        lc_tip: c.tip_height,
        lc_final: c.final_height,
        acc_next: acc.0 + acc.1,
        txs: r.stats.txs,
        failed: r.stats.failed_txs,
        verified: r.stats.verified,
        max_tx: r.ledger.max_tx,
        resends: r.stats.resends,
        dup_sends: r.stats.dup_sends,
        errors,
    }
}

#[test]
fn pipelined_relay_drains_a_backlog_and_keeps_up() {
    let p = small_params();
    let blocks = Rc::new(mine(&p, 70));
    // 30-block backlog, then one block every 4 slots (40 more)
    let every = 4;
    let r = run(&blocks, 8, 30, every, 1000);
    let max_lag = *r.lag.iter().max().unwrap();
    let drained = r.lag.iter().position(|&l| l <= 2).expect("never caught up");
    let after = r.lag[drained..].iter().max().copied().unwrap();
    println!(
        "parallel 8: backlog 30 drained (lag <= 2) after {drained} slots; max lag after that {after}; done at slot {} (tip {}, final {}, accumulated to {}); {} txs ({} failed), largest tx {} bytes",
        r.slots, r.lc_tip, r.lc_final, r.acc_next, r.txs, r.failed, r.max_tx
    );
    println!("lag by slot (every 10th): {:?}", r.lag.iter().step_by(10).collect::<Vec<_>>());
    assert_eq!(r.lc_tip, 70);
    assert_eq!(r.verified, 70, "every header counted once as verified by this relay");
    assert!(max_lag >= 30);
    // 30 blocks drained while 1 block / 4 slots kept arriving
    assert!(drained < 160, "drain took {drained} slots");
    assert!(after <= 4, "lag after catching up: {after}");
    assert!(r.lc_final >= 70 - 6 - 1, "final {}", r.lc_final);
    assert!(r.acc_next >= r.lc_final, "accumulator at {} behind final {}", r.acc_next, r.lc_final);
    // per header: about 37 txs (submit pair, begin, 26 chunks, 9 steps) plus global ops
    assert!(r.txs < 70 * 45, "txs {}", r.txs);
}

#[test]
fn sequential_relay_falls_behind_at_the_same_rate() {
    let p = small_params();
    let blocks = Rc::new(mine(&p, 24));
    let r = run(&blocks, 1, 0, 4, 96);
    let last = *r.lag.last().unwrap();
    println!("parallel 1, one block / 4 slots: lag after {} slots = {last} (tip {})", r.slots, r.lc_tip);
    // 24 blocks in 96 slots; one at a time is about 12 slots per header
    assert!(last >= 10, "sequential relay kept up: lag {last}");
}

/// The node switches to a heavier branch that forks below the light client's tip: the
/// relay abandons its flights on the old branch, verifies the new one (children of a
/// Verified non-tip parent are Verified at once), and reorgs the light client to it.
#[test]
fn follows_a_node_reorg() {
    let p = small_params();
    let a = mine(&p, 20);
    let mut b: Vec<Blk> = a[..16].to_vec();
    for _ in 0..10 {
        let parent = b.last().unwrap().clone();
        b.push(child(&p, &parent, &Work::Fallback, ChildOpts { miner: 0x77, ..Default::default() }));
    }
    let env = Env::new(p, &a[0]);
    let mut sim = Sim::new(env.svm, env.payer, 7);
    let blocks = Rc::new(RefCell::new(a.clone()));
    let chain = Chain { blocks: blocks.clone(), visible: Rc::new(Cell::new(a.len())), fetches: Rc::new(RefCell::new(0)) };
    let visible = chain.visible.clone();
    let works = (0..6).map(|_| Keypair::new()).collect();
    let mut r = Relay::new(chain, &mut sim, works, Opts { parallel: 6, exact_cu: true, resend_after: Duration::ZERO });
    r.ensure_works().unwrap();
    for _ in 0..200 {
        r.pass().unwrap();
        r.ledger.pause(0);
        if r.config().unwrap().tip_height == 20 {
            break;
        }
    }
    let c = r.config().unwrap();
    assert_eq!((c.tip_height, c.tip_hash), (20, a[20].hash));
    assert!(c.final_floor <= 15, "floor {} would refuse the fork at 15", c.final_floor);
    // the node reorgs to b (fork after height 15, tip 25)
    *blocks.borrow_mut() = b.clone();
    visible.set(b.len());
    let mut slots = 0;
    for _ in 0..300 {
        r.pass().unwrap();
        r.ledger.pause(0);
        slots += 1;
        let c = r.config().unwrap();
        if c.tip_hash == b[25].hash && c.final_height + 7 >= c.tip_height {
            break;
        }
    }
    let c = r.config().unwrap();
    println!("reorg 20 -> 25 (fork at 15) followed in {slots} slots; final {}", c.final_height);
    assert_eq!((c.tip_height, c.tip_hash), (25, b[25].hash));
    assert!(c.final_height >= 18, "final {}", c.final_height);
    // the final headers are b's, not a's
    let canon = r.ledger.account(&lightclient::client::canon_pda()).unwrap().unwrap();
    let at = |h: usize| -> [u8; 32] { canon[8 + 32 * h..8 + 32 * h + 32].try_into().unwrap() };
    assert_eq!(at(17), b[17].hash);
}

/// Devnet 2026-10-19/20 (docs/HANDOFF.md item 19): headers failed once with BadStep (6015) or
/// NotPending (6008) and passed on retry. A step tx landed, but the relay's next poll came
/// after its blockhash aged out (passes of about 2 min under 429 backoff), so the RPC
/// node's recent status cache no longer had it and getSignatureStatuses said unknown; the
/// blockhash read expired, so the relay sent the same step again with a new blockhash, and
/// the copy ran a finished step a second time. Here every 9th tx that lands drops out of
/// the status cache and blockhashes expire after 2 slots.
fn lossy(history_finds: bool) -> Run {
    let p = small_params();
    let blocks = Rc::new(mine(&p, 24));
    run_with(&blocks, 6, 12, 4, 600, |s| {
        s.lose_every = 9;
        s.expiry = 2;
        s.history_finds = history_finds;
    })
}

/// Reproduction: without a history lookup the expired-and-unknown tx is sent again and
/// its copy fails with BadStep / NotPending (or AccountAlreadyInUse for a submit), and
/// that header starts over.
#[test]
fn resending_a_landed_tx_runs_the_step_twice() {
    let r = lossy(false);
    println!("no history: {} txs, {} failed, {} resends; errors {:?}", r.txs, r.failed, r.resends, r.errors);
    assert!(r.resends > 0);
    assert!(r.failed > 0, "the copies of landed txs should fail");
    let custom = |c: u32| format!("Custom({c})");
    assert!(
        r.errors.iter().any(|e| e.contains(&custom(lightclient::LcError::BadStep.into())) || e.contains(&custom(lightclient::LcError::NotPending.into())) || e.contains("Custom(0)")),
        "errors {:?}",
        r.errors
    );
}

/// Regression: the relay asks the ledger history before sending an expired tx again, so
/// a landed tx counts as done and nothing runs twice: no failed tx, every header
/// verified once, the light client reaches the tip.
#[test]
fn expired_but_landed_txs_are_not_sent_again() {
    let r = lossy(true);
    println!("history: {} txs, {} failed, {} resends, {} dup sends, tip {}; errors {:?}", r.txs, r.failed, r.resends, r.dup_sends, r.lc_tip, r.errors);
    assert_eq!(r.lc_tip, 24);
    assert_eq!(r.verified, 24, "every header verified once (no restart)");
    assert_eq!(r.failed, 0, "errors {:?}", r.errors);
    assert!(r.errors.is_empty(), "errors {:?}", r.errors);
    assert_eq!(r.resends, 0, "every lost tx had landed");
}
