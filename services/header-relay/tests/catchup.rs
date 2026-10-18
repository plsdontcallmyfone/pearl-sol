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
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use header_relay::chain::{ChainHeader, Src};
use header_relay::ledger::{Ledger, TxState};
use header_relay::relay::{Opts, Relay};
use lightclient_litesvm_tests::*;
use litesvm::LiteSVM;
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
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
    fn blockhash_valid(&self, _: &Hash) -> Result<bool> {
        Ok(true)
    }
    fn submit(&mut self, tx: Transaction) -> Result<Signature> {
        let size = bincode_size(&tx);
        assert!(size <= 1232, "tx too large: {size}");
        self.max_tx = self.max_tx.max(size);
        let sig = tx.signatures[0];
        self.queue.push(tx);
        Ok(sig)
    }
    fn states(&self, sigs: &[Signature]) -> Result<Vec<TxState>> {
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
    blocks: Rc<Vec<Blk>>,
    visible: Rc<Cell<usize>>,
    fetches: Rc<RefCell<u64>>,
}

impl Src for Chain {
    fn tip(&self) -> Result<(u64, [u8; 32])> {
        let n = self.visible.get();
        Ok(((n - 1) as u64, self.blocks[n - 1].hash))
    }
    fn header_at(&self, h: u64) -> Result<Option<ChainHeader>> {
        *self.fetches.borrow_mut() += 1;
        if h as usize >= self.visible.get() {
            return Ok(None);
        }
        let b = &self.blocks[h as usize];
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
    max_tx: usize,
}

/// `backlog` blocks visible at the start, then one more every `every` slots up to
/// `total`, then `tail` slots to finish.
fn run(blocks: &Rc<Vec<Blk>>, parallel: usize, backlog: usize, every: u64, max_slots: u64) -> Run {
    let p = small_params();
    let env = Env::new(p, &blocks[0]);
    let mut sim = Sim { svm: env.svm, payer: env.payer, queue: vec![], results: HashMap::new(), slot: 0, rng: 0x9e3779b97f4a7c15, max_tx: 0 };
    let total = blocks.len();
    let chain = Chain { blocks: blocks.clone(), visible: Rc::new(Cell::new(backlog + 1)), fetches: Rc::new(RefCell::new(0)) };
    let visible = chain.visible.clone();
    let works = (0..parallel).map(|_| Keypair::new()).collect();
    let mut r = Relay::new(chain, &mut sim, works, Opts { parallel, exact_cu: true });
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
    Run { lag, slots: slot, lc_tip: c.tip_height, lc_final: c.final_height, acc_next: acc.0 + acc.1, txs: r.stats.txs, failed: r.stats.failed_txs, max_tx: r.ledger.max_tx }
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
