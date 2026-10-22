//! Pre-confirmations from weak blocks (docs/SPEED.md 2.4): the weak blocks this node holds per
//! parent, which txs they list, and the status a tx gets from them. Node policy, not consensus.

use std::collections::{HashMap, HashSet, VecDeque};

use agent_types::{Hash, Header, Pubkey, Tx};
use commonware_runtime::Spawner;
use consensus_pow::{PowVerifier, WeakBlock, WeakReject, WEAK_PARENT_DEPTH};
use serde_json::{json, Value};

use crate::core::{Core, Outbound};
use crate::dispatch::{b58, RpcError, R};

/// Weak blocks kept per parent (more drop the oldest).
pub const MAX_WEAK_PER_PARENT: usize = 256;

#[derive(Debug, Clone)]
pub struct WeakEntry {
    pub hash: Hash,
    pub header: Header,
    pub tx_hashes: Vec<Hash>,
    pub received_ms: u64,
    pub size: usize,
    /// Arrival order across all parents.
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreconfInfo {
    pub weak_count: u64,
    pub weak_miners: u64,
    pub weak_share: f64,
    pub first_seen_ms: u64,
    pub latest_age_ms: u64,
    pub confidence: &'static str,
}

impl PreconfInfo {
    pub fn json(&self) -> Value {
        json!({"weak_count": self.weak_count, "weak_miners": self.weak_miners, "weak_share": self.weak_share,
               "first_seen_ms": self.first_seen_ms, "latest_age_ms": self.latest_age_ms, "confidence": self.confidence})
    }
}

/// `low` (one weak block or one miner), `medium` (2+ weak blocks from 2+ miners), `high` (4+
/// weak blocks, 2+ miners, share >= 0.9).
pub fn confidence(count: u64, miners: u64, share: f64) -> &'static str {
    if count >= 4 && miners >= 2 && share >= 0.9 {
        "high"
    } else if count >= 2 && miners >= 2 {
        "medium"
    } else {
        "low"
    }
}

#[derive(Default)]
pub struct Preconf {
    by_parent: HashMap<Hash, VecDeque<WeakEntry>>,
    seen: HashSet<Hash>,
    seq: u64,
    /// Counters for getWeakBlocks / measurement.
    pub received: u64,
    pub invalid: u64,
}

impl Preconf {
    pub fn knows(&self, h: &Hash) -> bool {
        self.seen.contains(h)
    }

    /// Store a checked weak block. False if it was known.
    pub fn insert(&mut self, w: &WeakBlock, received_ms: u64, size: usize) -> bool {
        let hash = w.hash();
        if !self.seen.insert(hash) {
            return false;
        }
        self.seq += 1;
        self.received += 1;
        let q = self.by_parent.entry(w.header.prev_hash).or_default();
        q.push_back(WeakEntry { hash, header: w.header, tx_hashes: w.tx_hashes().to_vec(), received_ms, size, seq: self.seq });
        while q.len() > MAX_WEAK_PER_PARENT {
            if let Some(old) = q.pop_front() {
                self.seen.remove(&old.hash);
            }
        }
        true
    }

    /// Keep only the parents for which `keep` holds.
    pub fn retain_parents(&mut self, keep: impl Fn(&Hash) -> bool) {
        let seen = &mut self.seen;
        self.by_parent.retain(|p, q| {
            let k = keep(p);
            if !k {
                for e in q.iter() {
                    seen.remove(&e.hash);
                }
            }
            k
        });
    }

    pub fn entries(&self, parent: &Hash) -> impl Iterator<Item = &WeakEntry> {
        self.by_parent.get(parent).into_iter().flatten()
    }

    pub fn parents(&self) -> usize {
        self.by_parent.len()
    }

    /// Pre-confirmation of `tx` on `parent` (None: no weak block on it lists the tx).
    pub fn status(&self, tx: &Hash, parent: &Hash, now_ms: u64) -> Option<PreconfInfo> {
        let q = self.by_parent.get(parent)?;
        let listing: Vec<&WeakEntry> = q.iter().filter(|e| e.tx_hashes.contains(tx)).collect();
        let first = listing.first()?;
        let miners: HashSet<Pubkey> = listing.iter().map(|e| e.header.miner).collect();
        let since = q.iter().filter(|e| e.seq >= first.seq).count().max(1);
        let count = listing.len() as u64;
        let share = count as f64 / since as f64;
        let latest = listing.iter().map(|e| e.received_ms).max().unwrap_or(first.received_ms);
        Some(PreconfInfo {
            weak_count: count,
            weak_miners: miners.len() as u64,
            weak_share: share,
            first_seen_ms: listing.iter().map(|e| e.received_ms).min().unwrap_or(first.received_ms),
            latest_age_ms: now_ms.saturating_sub(latest),
            confidence: confidence(count, miners.len() as u64, share),
        })
    }

    pub fn is_preconfirmed(&self, tx: &Hash, parent: &Hash) -> bool {
        self.by_parent.get(parent).is_some_and(|q| q.iter().any(|e| e.tx_hashes.contains(tx)))
    }
}

/// docs/SPEED.md 2.6: a pooled tx listed by a weak block on the tip cannot be replaced by a
/// same-nonce tx. `pooled` = the hash pooled at `tx`'s sender and nonce, if any.
pub fn check_replacement(preconf: &Preconf, tip: &Hash, tx: &Tx, pooled: Option<Hash>) -> Result<(), String> {
    match pooled {
        Some(old) if old != tx.hash() && preconf.is_preconfirmed(&old, tip) => {
            Err(format!("a tx with this nonce ({}) is preconfirmed by a weak block on the tip and cannot be replaced", hex::encode(old)))
        }
        _ => Ok(()),
    }
}

/// Result of taking a weak message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeakOutcome {
    Added(Hash),
    Known,
    Ignored(String),
    /// The sender should be blocked.
    Invalid(String),
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    /// A weak message from a peer or a miner: check, store, relay (once).
    pub async fn add_weak(&mut self, bytes: &[u8], now_ms: u64) -> WeakOutcome {
        let w = match WeakBlock::decode(bytes) {
            Ok(w) => w,
            Err(e) => {
                self.preconf.invalid += 1;
                return WeakOutcome::Invalid(format!("undecodable weak block: {e}"));
            }
        };
        self.add_weak_block(w, bytes.len(), now_ms).await
    }

    pub async fn add_weak_block(&mut self, w: WeakBlock, size: usize, now_ms: u64) -> WeakOutcome {
        let hash = w.hash();
        if self.preconf.knows(&hash) {
            return WeakOutcome::Known;
        }
        let t0 = std::time::Instant::now();
        match self.chain.check_weak(&w, now_ms).await {
            Ok(()) => {}
            Err(WeakReject::Ignore(m)) => return WeakOutcome::Ignored(m),
            Err(WeakReject::Invalid(m)) => {
                self.preconf.invalid += 1;
                return WeakOutcome::Invalid(m);
            }
        }
        let verify_us = t0.elapsed().as_micros() as u64;
        self.preconf.insert(&w, now_ms, size);
        tracing::debug!(weak = %hex::encode(&hash[..6]), height = w.header.height, txs = w.tx_hashes().len(), size, verify_us, "weak block");
        self.outbox.push(Outbound::Weak(w.encode()));
        self.notify(
            "weak",
            json!({"hash": hex::encode(hash), "parent": hex::encode(w.header.prev_hash), "height": w.header.height,
                   "miner": b58(&w.header.miner), "job_id": hex::encode(w.header.job_id), "tx_count": w.tx_hashes().len(),
                   "received_ms": now_ms}),
        );
        WeakOutcome::Added(hash)
    }

    /// Drop weak state for parents more than WEAK_PARENT_DEPTH below the tip or no longer live.
    pub fn prune_weak(&mut self) {
        let tip = self.chain.tip_height();
        let chain = &self.chain;
        self.preconf.retain_parents(|p| chain.live_node(p).is_some_and(|n| n.header.height + WEAK_PARENT_DEPTH >= tip));
    }

    /// Pre-confirmation of a tx on the current tip.
    pub fn preconf_of(&self, tx: &Hash, now_ms: u64) -> Option<PreconfInfo> {
        self.preconf.status(tx, &self.chain.tip().hash, now_ms)
    }

    /// getTx / getTxStatus annotation for a pending (or unknown) tx.
    pub fn annotate_pending(&self, v: &mut Value, tx: &Hash, now_ms: u64) {
        match self.preconf_of(tx, now_ms) {
            Some(p) => {
                v["status"] = json!("preconfirmed");
                v["preconf"] = p.json();
            }
            None => v["status"] = json!("pending"),
        }
        v["final"] = json!(false);
    }

    /// RPC methods of this module (None: not one of them).
    pub async fn preconf_dispatch(&mut self, method: &str, p: &Value, now_ms: u64) -> Option<R> {
        Some(match method {
            "submitWeakBlock" => self.submit_weak(p, now_ms).await,
            "getWeakBlocks" => {
                let parent = match p.get("parent").and_then(|v| v.as_str()) {
                    Some(s) => match crate::dispatch::parse32(s) {
                        Ok(h) => h,
                        Err(e) => return Some(Err(e)),
                    },
                    None => self.chain.tip().hash,
                };
                let list: Vec<Value> = self
                    .preconf
                    .entries(&parent)
                    .map(|e| {
                        json!({"hash": hex::encode(e.hash), "height": e.header.height, "miner": b58(&e.header.miner),
                               "job_id": hex::encode(e.header.job_id), "tx_count": e.tx_hashes.len(), "received_ms": e.received_ms,
                               "size": e.size, "timestamp_ms": e.header.timestamp_ms,
                               "tx_hashes": e.tx_hashes.iter().map(hex::encode).collect::<Vec<_>>()})
                    })
                    .collect();
                Ok(json!({"parent": hex::encode(parent), "weak_shift": self.chain.weak_shift_for(&parent),
                          "count": list.len(), "weak_blocks": list, "received_total": self.preconf.received, "invalid_total": self.preconf.invalid}))
            }
            "getTxStatus" => {
                let h = match crate::dispatch::p_key(p, &["tx_hash", "hash"]) {
                    Ok(h) => h,
                    Err(e) => return Some(Err(e)),
                };
                Ok(self.tx_status(&h, now_ms).await)
            }
            _ => return None,
        })
    }

    /// `{status, preconf?, block_hash?, height?, confirmations?, final}`; status "unknown" when
    /// this node knows nothing of the tx.
    pub async fn tx_status(&self, h: &Hash, now_ms: u64) -> Value {
        if let Some((bh, _)) = self.chain.tx_location(h) {
            let height = match self.chain.live_node(&bh) {
                Some(n) => Some(n.header.height),
                None => self.chain.get_block(&bh).await.ok().flatten().map(|b| b.header.height),
            };
            if let Some(height) = height {
                let fin = self.chain.is_final(&bh, height);
                return json!({"tx_hash": hex::encode(h), "status": if fin { "final" } else { "included" }, "block_hash": hex::encode(bh),
                              "height": height, "confirmations": self.chain.confirmations(&bh, height), "final": fin});
            }
        }
        let mut v = json!({"tx_hash": hex::encode(h)});
        if self.mempool.contains(h) || self.preconf_of(h, now_ms).is_some() {
            self.annotate_pending(&mut v, h, now_ms);
        } else {
            v["status"] = json!("unknown");
            v["final"] = json!(false);
        }
        v
    }

    async fn submit_weak(&mut self, p: &Value, now_ms: u64) -> R {
        let hs = p.get("header").and_then(|v| v.as_str()).ok_or_else(|| RpcError::params("missing header"))?;
        let hb = hex::decode(hs.trim_start_matches("0x")).map_err(|e| RpcError::params(e.to_string()))?;
        let header = Header::decode(&hb).map_err(|e| RpcError::params(e.to_string()))?;
        let proof = crate::dispatch::p_bytes(p, "proof")?;
        let tid = p.get("template_id").and_then(|v| v.as_str());
        let m = |t: &consensus_pow::Template| t.header.prev_hash == header.prev_hash && t.header.tx_root == header.tx_root;
        let t = tid
            .and_then(|id| self.templates.iter().find(|(i, t)| i == id && m(t)))
            .or_else(|| self.templates.iter().rev().find(|(_, t)| m(t)))
            .map(|(_, t)| t.clone());
        let Some(t) = t else { return Ok(json!({"accepted": false, "reason": "no template with this parent and tx_root"})) };
        let w = WeakBlock::from_template(&t, header, proof);
        let size = w.encode().len();
        let r = self.add_weak_block(w, size, now_ms).await;
        Ok(match r {
            WeakOutcome::Added(h) => json!({"accepted": true, "weak_hash": hex::encode(h), "size": size}),
            WeakOutcome::Known => json!({"accepted": true, "weak_hash": hex::encode(header.hash()), "reason": "already known"}),
            WeakOutcome::Ignored(m) => json!({"accepted": false, "reason": format!("ignored: {m}")}),
            WeakOutcome::Invalid(m) => json!({"accepted": false, "reason": format!("invalid: {m}")}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(parent: u8, miner: u8, nonce: u64) -> Header {
        Header {
            version: agent_types::HEADER_VERSION,
            height: 5,
            prev_hash: [parent; 32],
            timestamp_ms: 1,
            target_bits: 0x207fffff,
            job_id: [0; 32],
            miner: [miner; 32],
            tx_root: [0; 32],
            state_root: [0; 32],
            withdraw_root: [0; 32],
            proof_hash: [0; 32],
            nonce,
        }
    }

    fn weak(parent: u8, miner: u8, nonce: u64, txs: &[Hash]) -> WeakBlock {
        let mut items = txs.to_vec();
        items.push([0xcb; 32]);
        WeakBlock { header: hdr(parent, miner, nonce), proof: vec![], items, has_contracts: false, uncle_items: 0 }
    }

    #[test]
    fn counting_share_confidence_and_pruning() {
        let (x, y) = ([1u8; 32], [2u8; 32]);
        let p = [9u8; 32];
        let mut c = Preconf::default();
        // a weak block without x first: it does not count against x's share (before x appeared)
        assert!(c.insert(&weak(9, 1, 0, &[y]), 100, 10));
        assert!(c.status(&x, &p, 200).is_none());
        assert!(c.insert(&weak(9, 1, 1, &[x, y]), 1000, 10));
        let s = c.status(&x, &p, 1500).unwrap();
        assert_eq!((s.weak_count, s.weak_miners, s.first_seen_ms, s.latest_age_ms, s.confidence), (1, 1, 1000, 500, "low"));
        assert_eq!(s.weak_share, 1.0);
        assert!(!c.insert(&weak(9, 1, 1, &[x, y]), 1100, 10), "duplicate");
        assert!(c.insert(&weak(9, 2, 2, &[x]), 2000, 10));
        assert_eq!(c.status(&x, &p, 2000).unwrap().confidence, "medium");
        // a weak block after x appeared that leaves x out lowers the share
        assert!(c.insert(&weak(9, 3, 3, &[y]), 2100, 10));
        let s = c.status(&x, &p, 2100).unwrap();
        assert_eq!(s.weak_count, 2);
        assert!((s.weak_share - 2.0 / 3.0).abs() < 1e-9);
        for n in 4..10 {
            c.insert(&weak(9, (n % 3) as u8 + 1, n, &[x]), 3000 + n, 10);
        }
        let s = c.status(&x, &p, 4000).unwrap();
        assert_eq!(s.weak_count, 8);
        assert!(s.weak_share >= 0.88 && s.weak_share < 0.9, "{}", s.weak_share);
        assert_eq!(s.confidence, "medium");
        c.insert(&weak(9, 1, 10, &[x]), 4000, 10);
        assert_eq!(c.status(&x, &p, 4000).unwrap().confidence, "high");
        // other parents do not count
        assert!(c.status(&x, &[8; 32], 4000).is_none());
        c.insert(&weak(8, 1, 11, &[x]), 4000, 10);
        assert_eq!(c.parents(), 2);
        c.retain_parents(|h| *h == [8; 32]);
        assert_eq!(c.parents(), 1);
        assert!(c.status(&x, &p, 4000).is_none());
        assert!(!c.knows(&weak(9, 1, 1, &[x, y]).hash()), "pruned entries are forgotten");
    }

    #[test]
    fn per_parent_cap_drops_oldest() {
        let mut c = Preconf::default();
        for n in 0..(MAX_WEAK_PER_PARENT as u64 + 5) {
            c.insert(&weak(9, 1, n, &[[n as u8; 32]]), n, 1);
        }
        assert_eq!(c.entries(&[9; 32]).count(), MAX_WEAK_PER_PARENT);
        assert!(!c.knows(&weak(9, 1, 0, &[[0; 32]]).hash()));
    }

    #[test]
    fn preconfirmed_tx_cannot_be_replaced() {
        let seed = [7u8; 32];
        let a = Tx::signed(&seed, 1, 0, 1000, agent_types::TxBody::Transfer { to: [3; 32], amount: 1 });
        let b = Tx::signed(&seed, 1, 0, 5000, agent_types::TxBody::Transfer { to: [4; 32], amount: 1 });
        let mut pool = mempool::Mempool::new(1, 100);
        pool.insert(a.clone(), 0, 1000).unwrap();
        let mut c = Preconf::default();
        let tip = [9u8; 32];
        // not preconfirmed: the fee bump rule applies as before
        assert!(check_replacement(&c, &tip, &b, pool.pooled_at(&b.from, b.nonce)).is_ok());
        c.insert(&weak(9, 1, 0, &[a.hash()]), 1, 1);
        let e = check_replacement(&c, &tip, &b, pool.pooled_at(&b.from, b.nonce)).unwrap_err();
        assert!(e.contains("preconfirmed"), "{e}");
        // a weak block on another parent does not lock it
        assert!(check_replacement(&c, &[1; 32], &b, pool.pooled_at(&b.from, b.nonce)).is_ok());
        // the same tx again is not a replacement
        assert!(check_replacement(&c, &tip, &a, pool.pooled_at(&a.from, a.nonce)).is_ok());
    }

    #[test]
    fn confidence_labels() {
        assert_eq!(confidence(1, 1, 1.0), "low");
        assert_eq!(confidence(3, 1, 1.0), "low");
        assert_eq!(confidence(2, 2, 0.1), "medium");
        assert_eq!(confidence(4, 2, 0.9), "high");
        assert_eq!(confidence(4, 2, 0.89), "medium");
    }
}
