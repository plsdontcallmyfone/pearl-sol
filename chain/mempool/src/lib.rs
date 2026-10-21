//! Transaction pool: per-sender queues ordered by nonce, block selection by fee.
//!
//! Selection only takes a sender's txs in contiguous nonce order starting at the
//! account's current nonce; among the heads of all senders' queues the highest fee goes
//! first (ties: lower hash, so the order is deterministic). A tx with the same sender and
//! nonce as a pooled one replaces it only with a fee at least 10% (and at least 1) higher.
//!
//! Admission limits (security pass 2026-10-21): the sender's balance must cover this tx's
//! fee plus the fees of its pooled txs before it (a key with no funds cannot fill the
//! pool for free; BridgeUnlock pays no fee and is checked against the attester set by
//! the caller instead), the pool is capped by count and by encoded bytes, and when it is
//! full a higher-fee tx evicts the lowest-fee tail of some sender's queue (tails only, so
//! no queue gets a nonce gap).

use std::collections::{BTreeMap, BinaryHeap, HashMap};

use agent_types::{Hash, Pubkey, Tx, TxKind};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum MempoolError {
    #[error("bad signature")]
    BadSignature,
    #[error("wrong chain id")]
    WrongChain,
    #[error("fee {0} below min_fee {1}")]
    LowFee(u64, u64),
    #[error("nonce {0} already used (account nonce {1})")]
    StaleNonce(u64, u64),
    #[error("nonce {0} too far ahead of account nonce {1}")]
    FutureNonce(u64, u64),
    #[error("a tx with this nonce is pooled with a fee >= this one")]
    Underpriced,
    #[error("already pooled")]
    Known,
    #[error("pool full")]
    Full,
    #[error("balance {0} cannot pay {1} in fees (this tx and the sender's pooled txs before it)")]
    Unfunded(u64, u64),
    #[error("replacement fee {0} below {1} (10% above the pooled tx)")]
    ReplacementUnderpriced(u64, u64),
}

/// Default cap on the pool's encoded bytes.
pub const DEFAULT_MAX_BYTES: usize = 64 << 20;

/// Fee a replacement of a pooled tx paying `old` must reach.
pub fn replacement_floor(old: u64) -> u64 {
    old.saturating_add((old / 10).max(1))
}

pub struct Mempool {
    pub chain_id: u32,
    pub max_txs: usize,
    pub max_ahead: u64,
    /// Cap on the sum of the pooled txs' encoded sizes.
    pub max_bytes: usize,
    bytes: usize,
    txs: HashMap<Hash, Tx>,
    by_sender: HashMap<Pubkey, BTreeMap<u64, Hash>>,
}

fn size_of(tx: &Tx) -> usize {
    tx.encode().len()
}

impl Mempool {
    pub fn new(chain_id: u32, max_txs: usize) -> Self {
        Self { chain_id, max_txs, max_ahead: 64, max_bytes: DEFAULT_MAX_BYTES, bytes: 0, txs: HashMap::new(), by_sender: HashMap::new() }
    }

    /// Encoded bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
    pub fn contains(&self, h: &Hash) -> bool {
        self.txs.contains_key(h)
    }
    pub fn get(&self, h: &Hash) -> Option<&Tx> {
        self.txs.get(h)
    }
    pub fn hashes(&self) -> Vec<Hash> {
        let mut v: Vec<Hash> = self.txs.keys().copied().collect();
        v.sort();
        v
    }
    /// Hash of the pooled tx of `from` at `nonce`, if any.
    pub fn pooled_at(&self, from: &Pubkey, nonce: u64) -> Option<Hash> {
        self.by_sender.get(from).and_then(|q| q.get(&nonce)).copied()
    }
    pub fn senders(&self) -> Vec<Pubkey> {
        self.by_sender.keys().copied().collect()
    }

    /// Admit a tx given the sender's current account nonce and the min fee, without a
    /// balance check (tests and tools; the node uses [`Mempool::insert_funded`]).
    pub fn insert(&mut self, tx: Tx, account_nonce: u64, min_fee: u64) -> Result<Hash, MempoolError> {
        self.insert_funded(tx, account_nonce, u64::MAX, min_fee)
    }

    /// Admit a tx given the sender's account nonce and balance and the min fee.
    pub fn insert_funded(&mut self, tx: Tx, account_nonce: u64, balance: u64, min_fee: u64) -> Result<Hash, MempoolError> {
        let h = tx.hash();
        if self.txs.contains_key(&h) {
            return Err(MempoolError::Known);
        }
        if tx.chain_id != self.chain_id {
            return Err(MempoolError::WrongChain);
        }
        let unlock = tx.body.kind() == TxKind::BridgeUnlock;
        if !unlock && tx.fee < min_fee {
            return Err(MempoolError::LowFee(tx.fee, min_fee));
        }
        if tx.nonce < account_nonce {
            return Err(MempoolError::StaleNonce(tx.nonce, account_nonce));
        }
        if tx.nonce >= account_nonce.saturating_add(self.max_ahead) {
            return Err(MempoolError::FutureNonce(tx.nonce, account_nonce));
        }
        if !unlock {
            // this tx's fee plus every pooled fee of the sender between the account nonce and it
            let before: u64 = self
                .by_sender
                .get(&tx.from)
                .map(|q| q.range(account_nonce..tx.nonce).map(|(_, h)| self.txs[h].fee).fold(0u64, |a, f| a.saturating_add(f)))
                .unwrap_or(0);
            let need = before.saturating_add(tx.fee);
            if balance < need {
                return Err(MempoolError::Unfunded(balance, need));
            }
        }
        if !tx.verify_sig() {
            return Err(MempoolError::BadSignature);
        }
        let size = size_of(&tx);
        if let Some(old) = self.by_sender.get(&tx.from).and_then(|q| q.get(&tx.nonce)).copied() {
            let floor = replacement_floor(self.txs[&old].fee);
            if tx.fee < floor {
                return Err(if self.txs[&old].fee >= tx.fee { MempoolError::Underpriced } else { MempoolError::ReplacementUnderpriced(tx.fee, floor) });
            }
            let replaced = self.remove(&old).expect("pooled");
            if let Err(e) = self.make_room(size, &tx) {
                self.reinsert(replaced);
                return Err(e);
            }
        } else {
            self.make_room(size, &tx)?;
        }
        self.bytes += size;
        self.by_sender.entry(tx.from).or_default().insert(tx.nonce, h);
        self.txs.insert(h, tx);
        Ok(h)
    }

    fn full_for(&self, size: usize) -> bool {
        self.txs.len() >= self.max_txs || self.bytes + size > self.max_bytes
    }

    /// Evict lowest-fee queue tails (of other senders) paying less than `tx` until `size`
    /// more bytes and one more tx fit; `Full` if that is not possible.
    fn make_room(&mut self, size: usize, tx: &Tx) -> Result<(), MempoolError> {
        if size > self.max_bytes {
            return Err(MempoolError::Full);
        }
        while self.full_for(size) {
            let victim = self
                .by_sender
                .iter()
                .filter(|(s, _)| **s != tx.from)
                .filter_map(|(_, q)| q.iter().next_back().map(|(_, h)| *h))
                .min_by(|a, b| self.txs[a].fee.cmp(&self.txs[b].fee).then(b.cmp(a)));
            match victim {
                Some(v) if self.txs[&v].fee < tx.fee => {
                    self.remove(&v);
                }
                _ => return Err(MempoolError::Full),
            }
        }
        Ok(())
    }

    /// Re-add a tx from a disconnected block (no fee/nonce-window checks; pruned later).
    pub fn reinsert(&mut self, tx: Tx) {
        let h = tx.hash();
        if self.txs.contains_key(&h) || tx.chain_id != self.chain_id {
            return;
        }
        if let Some(old) = self.by_sender.get(&tx.from).and_then(|q| q.get(&tx.nonce)).copied() {
            if self.txs[&old].fee >= tx.fee {
                return;
            }
            self.remove(&old);
        }
        let size = size_of(&tx);
        if self.full_for(size) {
            return;
        }
        self.bytes += size;
        self.by_sender.entry(tx.from).or_default().insert(tx.nonce, h);
        self.txs.insert(h, tx);
    }

    pub fn remove(&mut self, h: &Hash) -> Option<Tx> {
        let tx = self.txs.remove(h)?;
        self.bytes -= size_of(&tx);
        if let Some(q) = self.by_sender.get_mut(&tx.from) {
            if q.get(&tx.nonce) == Some(h) {
                q.remove(&tx.nonce);
            }
            if q.is_empty() {
                self.by_sender.remove(&tx.from);
            }
        }
        Some(tx)
    }

    /// Drop txs whose nonce is below the sender's account nonce.
    pub fn prune(&mut self, nonces: &HashMap<Pubkey, u64>) {
        let mut gone = Vec::new();
        for (s, q) in &self.by_sender {
            let n = nonces.get(s).copied().unwrap_or(0);
            gone.extend(q.range(..n).map(|(_, h)| *h));
        }
        for h in gone {
            self.remove(&h);
        }
    }

    /// Up to `max` txs in block order.
    pub fn select(&self, nonces: &HashMap<Pubkey, u64>, max: usize) -> Vec<Tx> {
        #[derive(PartialEq, Eq)]
        struct Head {
            fee: u64,
            hash: Hash,
            sender: Pubkey,
            nonce: u64,
        }
        impl Ord for Head {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                self.fee.cmp(&o.fee).then(o.hash.cmp(&self.hash))
            }
        }
        impl PartialOrd for Head {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        let mut heap = BinaryHeap::new();
        for (s, q) in &self.by_sender {
            let n = nonces.get(s).copied().unwrap_or(0);
            if let Some(h) = q.get(&n) {
                heap.push(Head { fee: self.txs[h].fee, hash: *h, sender: *s, nonce: n });
            }
        }
        let mut out = Vec::new();
        while let Some(hd) = heap.pop() {
            if out.len() >= max {
                break;
            }
            out.push(self.txs[&hd.hash].clone());
            if let Some(h) = self.by_sender[&hd.sender].get(&(hd.nonce + 1)) {
                heap.push(Head { fee: self.txs[h].fee, hash: *h, sender: hd.sender, nonce: hd.nonce + 1 });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_types::TxBody;

    fn tx(s: u8, nonce: u64, fee: u64) -> Tx {
        Tx::signed(&[s; 32], 1, nonce, fee, TxBody::Transfer { to: [0; 32], amount: 1 })
    }

    #[test]
    fn fee_priority_nonce_order() {
        let mut m = Mempool::new(1, 100);
        let a0 = tx(1, 0, 10);
        let a1 = tx(1, 1, 500);
        let b0 = tx(2, 0, 100);
        let a2 = tx(1, 2, 1);
        for t in [&a1, &a0, &b0, &a2] {
            m.insert(t.clone(), 0, 1).unwrap();
        }
        let nonces = HashMap::new();
        let sel = m.select(&nonces, 10);
        // b0 (100) beats a0 (10); a1 is only reachable after a0.
        assert_eq!(sel, vec![b0.clone(), a0.clone(), a1.clone(), a2.clone()]);
        assert_eq!(m.select(&nonces, 2).len(), 2);
        // gap: without a0 nothing from sender 1 is selectable
        m.remove(&a0.hash());
        assert_eq!(m.select(&nonces, 10), vec![b0.clone()]);
        // prune by account nonce
        let mut n = HashMap::new();
        n.insert(a0.from, 2);
        m.prune(&n);
        assert!(!m.contains(&a1.hash()));
        assert_eq!(m.select(&n, 10), vec![b0, a2]);
    }

    #[test]
    fn admission_rules() {
        let mut m = Mempool::new(1, 2);
        assert_eq!(m.insert(tx(1, 0, 5), 0, 10), Err(MempoolError::LowFee(5, 10)));
        assert_eq!(m.insert(tx(1, 0, 10), 1, 10), Err(MempoolError::StaleNonce(0, 1)));
        assert_eq!(m.insert(tx(1, 100, 10), 0, 10), Err(MempoolError::FutureNonce(100, 0)));
        let mut bad = tx(1, 0, 10);
        bad.sig[0] ^= 1;
        assert_eq!(m.insert(bad, 0, 10), Err(MempoolError::BadSignature));
        m.insert(tx(1, 0, 10), 0, 10).unwrap();
        assert_eq!(m.insert(tx(1, 0, 10), 0, 10), Err(MempoolError::Known));
        assert_eq!(m.insert(Tx::signed(&[1; 32], 1, 0, 10, TxBody::Transfer { to: [1; 32], amount: 1 }), 0, 10), Err(MempoolError::Underpriced));
        m.insert(tx(1, 0, 20), 0, 10).unwrap();
        assert_eq!(m.len(), 1);
        m.insert(tx(2, 0, 10), 0, 10).unwrap();
        assert_eq!(m.insert(tx(3, 0, 10), 0, 10), Err(MempoolError::Full));
    }

    /// Security pass: a key with no funds could fill the pool for free (txs that can never
    /// pay are never mined, so they cost nothing and stay until the pool is full).
    #[test]
    fn unfunded_sender_refused() {
        let mut m = Mempool::new(1, 100);
        assert_eq!(m.insert_funded(tx(1, 0, 10), 0, 0, 10), Err(MempoolError::Unfunded(0, 10)));
        assert_eq!(m.insert_funded(tx(1, 0, 10), 0, 9, 10), Err(MempoolError::Unfunded(9, 10)));
        // fees add up over the sender's queue
        m.insert_funded(tx(1, 0, 10), 0, 25, 10).unwrap();
        m.insert_funded(tx(1, 1, 10), 0, 25, 10).unwrap();
        assert_eq!(m.insert_funded(tx(1, 2, 10), 0, 25, 10), Err(MempoolError::Unfunded(25, 30)));
        // BridgeUnlock pays no fee (the caller checks its attester signatures)
        let u = Tx::signed(&[5; 32], 1, 0, 0, TxBody::BridgeUnlock { solana_nonce: 1, recipient: [1; 32], amount_spl: 1, job_order: None, sigs: vec![] });
        m.insert_funded(u, 0, 0, 10).unwrap();
    }

    /// Security pass: a full pool takes a higher-fee tx by evicting the cheapest queue tail.
    #[test]
    fn full_pool_evicts_cheapest_tail() {
        let mut m = Mempool::new(1, 3);
        let a0 = tx(1, 0, 10);
        let a1 = tx(1, 1, 50);
        let b0 = tx(2, 0, 20);
        for t in [&a0, &a1, &b0] {
            m.insert(t.clone(), 0, 1).unwrap();
        }
        // tails are a1 (50) and b0 (20): b0 goes, a0 (10, not a tail) stays
        let c0 = tx(3, 0, 30);
        m.insert(c0.clone(), 0, 1).unwrap();
        assert!(!m.contains(&b0.hash()) && m.contains(&a0.hash()) && m.contains(&c0.hash()));
        // nothing cheaper than 25 left at a tail: refused
        assert_eq!(m.insert(tx(4, 0, 25), 0, 1), Err(MempoolError::Full));
        // byte cap
        let mut m = Mempool::new(1, 100);
        let one = tx(1, 0, 10).encode().len();
        m.max_bytes = 2 * one;
        m.insert(tx(1, 0, 10), 0, 1).unwrap();
        m.insert(tx(2, 0, 10), 0, 1).unwrap();
        assert_eq!(m.bytes(), 2 * one);
        assert_eq!(m.insert(tx(3, 0, 10), 0, 1), Err(MempoolError::Full));
        m.insert(tx(3, 0, 11), 0, 1).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m.bytes(), 2 * one);
    }

    #[test]
    fn replacement_needs_ten_percent() {
        let mut m = Mempool::new(1, 10);
        m.insert(tx(1, 0, 100), 0, 1).unwrap();
        assert_eq!(m.insert(tx(1, 0, 105), 0, 1), Err(MempoolError::ReplacementUnderpriced(105, 110)));
        m.insert(tx(1, 0, 110), 0, 1).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.bytes(), tx(1, 0, 110).encode().len());
    }
}
