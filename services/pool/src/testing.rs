//! Test node: pearl-node's [`MockNode`] plus what a pool needs to see money move.
//! Templates report `fees` = a settable reward; an accepted block's reward is credited
//! to its header's miner `k` blocks later (like the chain's coinbase maturity); Transfer
//! txs are checked (signature, chain id, strict nonce, balance) and applied when the
//! next block lands. [`RewardNode::advance`] adds blocks mined by someone else, and
//! [`RewardNode::reorg_at`] replaces a canonical block (orphaning it).

use std::sync::{Arc, Mutex};

use anyhow::{bail, ensure, Result};
use serde_json::json;

use pearl_job::pow::{BlockHeader, MinedBlock};
use pearl_node::mock::{MockBlock, MockNode};
use pearl_node::tx::{self, Body};
use pearl_node::{Account, JobInfo, NodeApi, SubmitOutcome, Template};

use crate::PoolNode;

#[derive(Default)]
struct Ledger {
    /// (credit at height, miner, amount)
    immature: Vec<(u64, [u8; 32], u64)>,
    mempool: Vec<Vec<u8>>,
    reward: u64,
    /// Rewards credited so far, per miner.
    credited: Vec<([u8; 32], u64)>,
}

#[derive(Clone)]
pub struct RewardNode {
    pub mock: MockNode,
    pub k: u64,
    ledger: Arc<Mutex<Ledger>>,
}

impl RewardNode {
    pub fn new(chain_id: u32, target_bits: u32, k: u64, reward: u64) -> Self {
        let n = Self { mock: MockNode::new(chain_id, target_bits), k, ledger: Arc::default() };
        n.ledger.lock().unwrap().reward = reward;
        n
    }

    pub fn set_reward(&self, r: u64) {
        self.ledger.lock().unwrap().reward = r;
    }

    pub fn set_target(&self, bits: u32) {
        self.mock.state.lock().unwrap().target_bits = bits;
    }

    pub fn balance(&self, a: &[u8; 32]) -> u64 {
        self.mock.state.lock().unwrap().accounts.get(a).map(|x| x.balance).unwrap_or(0)
    }

    pub fn mempool_len(&self) -> usize {
        self.ledger.lock().unwrap().mempool.len()
    }

    /// Forget pending txs (a node restart): the sender must re-send them.
    pub fn drop_mempool(&self) {
        self.ledger.lock().unwrap().mempool.clear();
    }

    pub fn add_job(&self, j: JobInfo) {
        self.mock.add_job(j)
    }

    /// Called after a block at `height` lands: credit matured rewards, apply mempool.
    fn on_block(&self, height: u64, miner: [u8; 32], reward: u64) {
        let mut l = self.ledger.lock().unwrap();
        if reward > 0 {
            l.immature.push((height + self.k, miner, reward));
        }
        let mut s = self.mock.state.lock().unwrap();
        let (due, keep): (Vec<_>, Vec<_>) = l.immature.drain(..).partition(|(h, _, _)| *h <= height);
        l.immature = keep;
        for (_, m, a) in due {
            s.accounts.entry(m).or_default().balance += a;
            l.credited.push((m, a));
        }
        drop(s);
        drop(l);
        self.apply_pending();
    }

    fn apply_pending(&self) {
        let mut l = self.ledger.lock().unwrap();
        let mut s = self.mock.state.lock().unwrap();
        loop {
            let mut progressed = false;
            let pool = std::mem::take(&mut l.mempool);
            let mut left = Vec::new();
            for w in pool {
                let (t, _) = tx::decode(&w).unwrap();
                let from = s.accounts.entry(t.from).or_default();
                match t.body {
                    Body::Transfer { to, amount } if t.nonce == from.nonce && from.balance >= amount + t.fee => {
                        from.balance -= amount + t.fee;
                        from.nonce += 1;
                        s.accounts.entry(to).or_default().balance += amount;
                        progressed = true;
                    }
                    _ if t.nonce >= from.nonce => left.push(w),
                    _ => {}
                }
            }
            l.mempool = left;
            if !progressed || l.mempool.is_empty() {
                break;
            }
        }
    }

    /// `n` blocks by an outside miner (no reward to the pool).
    pub fn advance(&self, n: u64) {
        for _ in 0..n {
            let h = {
                let mut s = self.mock.state.lock().unwrap();
                let (prev, h) = s.tip();
                let header = BlockHeader { version: 1, height: h + 1, prev_hash: prev, miner: [0xee; 32], nonce: 0xbeef, ..Default::default() };
                s.blocks.push(MockBlock { header, proof_len: 0, txs: 0 });
                h + 1
            };
            self.on_block(h, [0xee; 32], 0);
        }
    }

    /// Replace the canonical block at `height` with a different one (its successors stay;
    /// the mock does not check links). The old block's pending reward is dropped.
    pub fn reorg_at(&self, height: u64) {
        let mut s = self.mock.state.lock().unwrap();
        let b = s.blocks.iter_mut().find(|b| b.header.height == height).expect("no block at height");
        let old = b.header.miner;
        b.header.nonce ^= 0x5555;
        b.header.miner = [0xdd; 32];
        drop(s);
        self.ledger.lock().unwrap().immature.retain(|(h, m, _)| !(*h == height + self.k && *m == old));
    }
}

impl NodeApi for RewardNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<Template> {
        let mut t = self.mock.block_template(payout)?;
        t.raw = json!({"fees": self.ledger.lock().unwrap().reward.to_string(), "subsidy": "0"});
        Ok(t)
    }

    fn fallback_template(&self, payout: &[u8; 32]) -> Result<Template> {
        let mut t = self.mock.fallback_template(payout)?;
        t.raw = json!({"fees": self.ledger.lock().unwrap().reward.to_string(), "subsidy": "0"});
        Ok(t)
    }

    fn submit_block(&self, t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        let out = self.mock.submit_block(t, b)?;
        if matches!(out, SubmitOutcome::Accepted { .. }) {
            let r = self.ledger.lock().unwrap().reward;
            self.on_block(b.header.height, b.header.miner, r);
        }
        Ok(out)
    }

    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>> {
        self.mock.get_job(job_id)
    }

    fn get_account(&self, account: &[u8; 32]) -> Result<Account> {
        Ok(self.mock.state.lock().unwrap().accounts.get(account).copied().unwrap_or_default())
    }

    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        let (from, _) = tx::verify(wire)?;
        let (t, _) = tx::decode(wire)?;
        ensure!(t.chain_id == self.mock.state.lock().unwrap().chain_id, "wrong chain_id");
        let acct = self.get_account(&from)?;
        if t.nonce < acct.nonce {
            bail!("nonce {} below account nonce {}", t.nonce, acct.nonce);
        }
        let mut l = self.ledger.lock().unwrap();
        if l.mempool.iter().any(|w| w == wire) {
            bail!("already in mempool");
        }
        if l.mempool.iter().any(|w| tx::decode(w).map(|(x, _)| x.from == from && x.nonce == t.nonce).unwrap_or(false)) {
            bail!("another tx with nonce {} is pending", t.nonce);
        }
        l.mempool.push(wire.to_vec());
        Ok(hex::encode(blake3::hash(wire).as_bytes()))
    }
}

impl PoolNode for RewardNode {
    fn tip(&self) -> Result<([u8; 32], u64)> {
        Ok(self.mock.state.lock().unwrap().tip())
    }

    fn canonical_hash(&self, height: u64) -> Result<Option<[u8; 32]>> {
        let s = self.mock.state.lock().unwrap();
        Ok(s.blocks.iter().find(|b| b.header.height == height).map(|b| b.header.block_hash()))
    }

    fn finality_k(&self) -> Result<u64> {
        Ok(self.k)
    }
}
