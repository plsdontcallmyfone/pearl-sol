//! Transaction pool: per-sender queues ordered by nonce, block selection by fee.
//!
//! Selection only takes a sender's txs in contiguous nonce order starting at the
//! account's current nonce; among the heads of all senders' queues the highest fee goes
//! first (ties: lower hash, so the order is deterministic). A tx with the same sender and
//! nonce as a pooled one replaces it only with a strictly higher fee.

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
}

pub struct Mempool {
    pub chain_id: u32,
    pub max_txs: usize,
    pub max_ahead: u64,
    txs: HashMap<Hash, Tx>,
    by_sender: HashMap<Pubkey, BTreeMap<u64, Hash>>,
}

impl Mempool {
    pub fn new(chain_id: u32, max_txs: usize) -> Self {
        Self { chain_id, max_txs, max_ahead: 64, txs: HashMap::new(), by_sender: HashMap::new() }
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
    pub fn senders(&self) -> Vec<Pubkey> {
        self.by_sender.keys().copied().collect()
    }

    /// Admit a tx given the sender's current account nonce and the min fee.
    pub fn insert(&mut self, tx: Tx, account_nonce: u64, min_fee: u64) -> Result<Hash, MempoolError> {
        let h = tx.hash();
        if self.txs.contains_key(&h) {
            return Err(MempoolError::Known);
        }
        if tx.chain_id != self.chain_id {
            return Err(MempoolError::WrongChain);
        }
        if tx.body.kind() != TxKind::BridgeUnlock && tx.fee < min_fee {
            return Err(MempoolError::LowFee(tx.fee, min_fee));
        }
        if tx.nonce < account_nonce {
            return Err(MempoolError::StaleNonce(tx.nonce, account_nonce));
        }
        if tx.nonce >= account_nonce + self.max_ahead {
            return Err(MempoolError::FutureNonce(tx.nonce, account_nonce));
        }
        if !tx.verify_sig() {
            return Err(MempoolError::BadSignature);
        }
        if let Some(old) = self.by_sender.get(&tx.from).and_then(|q| q.get(&tx.nonce)).copied() {
            if self.txs[&old].fee >= tx.fee {
                return Err(MempoolError::Underpriced);
            }
            self.txs.remove(&old);
        } else if self.txs.len() >= self.max_txs {
            return Err(MempoolError::Full);
        }
        self.by_sender.entry(tx.from).or_default().insert(tx.nonce, h);
        self.txs.insert(h, tx);
        Ok(h)
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
            self.txs.remove(&old);
        }
        self.by_sender.entry(tx.from).or_default().insert(tx.nonce, h);
        self.txs.insert(h, tx);
    }

    pub fn remove(&mut self, h: &Hash) -> Option<Tx> {
        let tx = self.txs.remove(h)?;
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
}
