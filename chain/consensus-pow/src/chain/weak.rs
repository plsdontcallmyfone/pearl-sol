//! Weak blocks (docs/SPEED.md 2): a valid child header of a live block whose Pearl proof meets
//! the weak target `target << weak_shift` instead of the header's own target. Not consensus:
//! nodes gossip them so a tx a miner is working on shows as pre-confirmed seconds after it is
//! sent. They ride the tips channel (a tip announcement is exactly 40 bytes; nodes from before
//! weak blocks ignore any other length there).
//!
//! ```text
//! weak msg = "CWB1" || header [256] || proof_len u32 || proof || item_count u32
//!            || item hash [32] * item_count || flags u8 (bit 0: a contracts record item)
//! items    = tx_root leaf items in block order: tx hashes, contracts record hash (flag),
//!            coinbase receipt hash (last)
//! ```

use agent_types::codec::{R, W};
use agent_types::{merkle, DecodeError, Hash, Header, HEADER_VERSION};
use commonware_runtime::Spawner;
use state::schema::{self, JobRecord};

use super::{Chain, Template};
use crate::verifier::PowVerifier;

pub const WEAK_MAGIC: &[u8; 4] = b"CWB1";
/// Largest weak message accepted.
pub const MAX_WEAK_MSG: usize = 4 << 20;
/// Weak blocks are kept for parents at most this far below the tip.
pub const WEAK_PARENT_DEPTH: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeakBlock {
    pub header: Header,
    pub proof: Vec<u8>,
    /// tx_root leaf items (tx hashes, contracts record hash, coinbase receipt hash).
    pub items: Vec<Hash>,
    pub has_contracts: bool,
}

/// Why a weak block was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeakReject {
    /// Not the sender's fault (unknown or old parent, weak blocks off): dropped silently.
    Ignore(String),
    /// Invalid: the sender is misbehaving.
    Invalid(String),
}

impl std::fmt::Display for WeakReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WeakReject::Ignore(m) => write!(f, "ignored: {m}"),
            WeakReject::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}

/// Is this tips-channel payload a weak message (not a 40-byte tip)?
pub fn is_weak_msg(b: &[u8]) -> bool {
    b.len() != 40 && b.len() >= 4 && &b[..4] == WEAK_MAGIC
}

/// Weak shift for a target block time (pearl_job::pow::weak_shift).
pub fn weak_shift(target_block_time_ms: u64) -> u32 {
    pearl_job::pow::weak_shift(target_block_time_ms)
}

impl WeakBlock {
    /// The tx hashes the weak block lists.
    pub fn tx_hashes(&self) -> &[Hash] {
        let extra = 1 + self.has_contracts as usize;
        &self.items[..self.items.len().saturating_sub(extra)]
    }

    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    /// The weak block of a mined `header` + `proof` on `t` (the node's own template).
    pub fn from_template(t: &Template, header: Header, proof: Vec<u8>) -> Self {
        let items = agent_types::block::leaf_hashes_full(&t.txs, t.summary.contracts.as_ref(), Some(&t.summary.coinbase));
        Self { header, proof, items, has_contracts: t.summary.contracts.is_some() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.bytes(WEAK_MAGIC).bytes(&self.header.encode()).u32(self.proof.len() as u32).bytes(&self.proof);
        w.u32(self.items.len() as u32);
        for i in &self.items {
            w.bytes(i);
        }
        w.u8(self.has_contracts as u8);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() > MAX_WEAK_MSG {
            return Err(DecodeError::TooLarge("weak block"));
        }
        let mut r = R::new(b);
        if r.take(4)? != &WEAK_MAGIC[..] {
            return Err(DecodeError::Invalid("weak magic"));
        }
        let header = Header::decode(r.take(Header::SIZE)?)?;
        let plen = r.u32()? as usize;
        if plen > agent_types::block::MAX_PROOF_LEN {
            return Err(DecodeError::TooLarge("weak proof"));
        }
        let proof = r.take(plen)?.to_vec();
        let n = r.u32()? as usize;
        if n == 0 || n > agent_types::block::MAX_BLOCK_TXS + 2 {
            return Err(DecodeError::Invalid("weak item count"));
        }
        let mut items = Vec::with_capacity(n);
        for _ in 0..n {
            items.push(r.take(32)?.try_into().unwrap());
        }
        let has_contracts = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Invalid("weak flags")),
        };
        if has_contracts && n < 2 {
            return Err(DecodeError::Invalid("weak item count"));
        }
        r.finish()?;
        Ok(Self { header, proof, items, has_contracts })
    }
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Chain<E, V> {
    /// Weak target (compact) for a child of `parent`, or None when weak blocks are off.
    pub fn weak_bits_for(&self, parent: &Hash) -> Option<u32> {
        let p = self.live.get(parent)?;
        let shift = weak_shift(p.config.target_block_time_ms);
        if shift == 0 {
            return None;
        }
        Some(pearl_job::pow::weak_bits(self.required_bits(parent), shift, p.config.pow_limit_bits))
    }

    /// Weak shift for a child of `parent` (0 = off).
    pub fn weak_shift_for(&self, parent: &Hash) -> u32 {
        self.live.get(parent).map(|p| weak_shift(p.config.target_block_time_ms)).unwrap_or(0)
    }

    /// docs/SPEED.md 2.2 checks 2-6 (decoding is check 1).
    pub async fn check_weak(&self, w: &WeakBlock, now_ms: u64) -> Result<(), WeakReject> {
        let h = &w.header;
        let Some(parent) = self.live.get(&h.prev_hash) else { return Err(WeakReject::Ignore("parent is not a live block".into())) };
        if parent.header.height + WEAK_PARENT_DEPTH < self.tip_height() {
            return Err(WeakReject::Ignore("parent is too far below the tip".into()));
        }
        let Some(wbits) = self.weak_bits_for(&h.prev_hash) else { return Err(WeakReject::Ignore("weak blocks are off at this block time".into())) };
        let bad = |m: &str| Err(WeakReject::Invalid(m.to_string()));
        if merkle::root(&w.items) != h.tx_root {
            return bad("weak items do not match tx_root");
        }
        if h.version != HEADER_VERSION {
            return bad("header version");
        }
        if h.height != parent.header.height + 1 {
            return bad("height is not parent + 1");
        }
        if h.target_bits != self.required_bits(&h.prev_hash) {
            return bad("target_bits is not the required target");
        }
        if h.timestamp_ms < self.min_timestamp(&h.prev_hash) {
            return bad("timestamp not after median time past");
        }
        if h.timestamp_ms > now_ms.saturating_add(parent.config.max_future_ms) {
            return Err(WeakReject::Ignore(super::TIME_TOO_NEW.into()));
        }
        let pcfg = parent.config.clone();
        let job = if h.is_fallback() {
            None
        } else {
            let v = self.store.get(&parent.snapshot, &schema::job_key(&h.job_id)).await.map_err(|e| WeakReject::Ignore(e.to_string()))?;
            let Some(v) = v else { return bad("weak block names a job its parent does not have") };
            let j = JobRecord::decode(&v).map_err(|e| WeakReject::Ignore(e.to_string()))?;
            Some(j.pearl_job().map_err(WeakReject::Invalid)?)
        };
        self.verifier
            .verify_at(h, job.as_ref(), &w.proof, &pcfg.fallback_params(), wbits)
            .map_err(|e| WeakReject::Invalid(format!("bad weak proof of work: {e}")))
    }
}
