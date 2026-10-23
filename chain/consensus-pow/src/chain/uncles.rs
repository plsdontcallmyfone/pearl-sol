//! Uncles (docs/SPEED.md 3.3): stale job blocks a later block credits so their useful work
//! still pays. Fork choice is unchanged (the heaviest chain by its own headers' work); uncle
//! work is never counted.
//!
//! Validity of uncle U in block B at height H on branch b (B's ancestry):
//! 1. U's version is the header version and U.job_id is not zero;
//! 2. 1 <= H - U.height <= MAX_UNCLE_DEPTH;
//! 3. U.prev_hash is b's block at U.height - 1;
//! 4. U is not b's block at U.height, uncles in B are distinct, and `uncle(U.hash)` is absent
//!    in B's parent state (not credited earlier on b);
//! 5. U.target_bits = target_bits of b's block at U.height (siblings share a parent), and
//!    U.timestamp_ms > the median time past of U's parent;
//! 6. the job U.job_id exists in B's parent state with created_height < U.height, and U's
//!    proof verifies against it at U's own target.
//! 7. (from uncle_job_rule_height) the job was mineable at U.height.
//! 8. (from uncle_digest_rule_height) U.pow_digest differs from the branch block's at U.height,
//!    from the other uncles in B, and from the uncles of b's blocks above U.height: a header
//!    with a second valid proof (another tile under the target) is the same block, not a sibling.
//! Rewards are computed by the executor (executor/src/uncles.rs); the body's records must equal
//! the ones execution produced.

use agent_types::speed::{speed_upgrade_height, uncle_digest_rule_height, uncle_share, Uncle, MAX_UNCLES, MAX_UNCLE_DEPTH};
use agent_types::{Hash, Header, HEADER_VERSION};
use commonware_runtime::Spawner;
use executor::UncleInput;
use state::schema::{self, JobRecord};
use state::Snapshot;

use super::{Chain, ChainError, MTP_SPAN};
use crate::verifier::PowVerifier;

/// One block of a branch as uncle checks need it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BranchEntry {
    pub hash: Hash,
    pub timestamp_ms: u64,
    pub target_bits: u32,
}

fn invalid<T>(m: impl Into<String>) -> Result<T, ChainError> {
    Err(ChainError::Invalid(m.into()))
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Chain<E, V> {
    /// The block at `height` on the branch ending at live block `tip` (live blocks above the
    /// finalized one, the final canonical chain below it).
    pub(crate) fn branch_at(&self, tip: &Hash, height: u64) -> Option<BranchEntry> {
        let mut cur = *tip;
        loop {
            let n = self.live.get(&cur)?;
            if n.header.height == height {
                return Some(BranchEntry { hash: n.hash, timestamp_ms: n.header.timestamp_ms, target_bits: n.header.target_bits });
            }
            if n.header.height < height {
                return None;
            }
            if cur == self.finalized {
                let c = self.canon.get(height as usize)?;
                return Some(BranchEntry { hash: c.hash, timestamp_ms: c.timestamp_ms, target_bits: c.target_bits });
            }
            cur = n.header.prev_hash;
        }
    }

    /// Median time past of the branch block at `height` (itself and up to 10 ancestors).
    fn branch_mtp(&self, tip: &Hash, height: u64) -> Option<u64> {
        let mut ts = Vec::with_capacity(MTP_SPAN);
        let mut h = height;
        loop {
            ts.push(self.branch_at(tip, h)?.timestamp_ms);
            if ts.len() == MTP_SPAN || h == 0 {
                break;
            }
            h -= 1;
        }
        ts.sort_unstable();
        Some(ts[ts.len() / 2])
    }

    /// Header of a branch block (live, else from the archive).
    async fn branch_header(&self, h: &Hash) -> Result<Option<Header>, ChainError> {
        if let Some(n) = self.live.get(h) {
            return Ok(Some(n.header));
        }
        Ok(self.get_block(h).await?.map(|b| b.header))
    }

    /// Rule 8 against the branch: `u`'s pow_digest is not the branch block's at `u.height` and
    /// not an uncle's of a branch block between `u.height + 1` and `height - 1` (so a second
    /// proof of a block, or of an uncle credited earlier, is not paid again).
    async fn check_uncle_digest(&self, parent: &Hash, height: u64, u: &Header, sib: &Hash) -> Result<(), ChainError> {
        let d = u.pow_digest();
        let Some(sh) = self.branch_header(sib).await? else { return Err(ChainError::Internal("branch block missing".into())) };
        if sh.pow_digest() == d {
            return invalid("uncle has the same pow_digest as the branch block at its height (a second proof of that block)");
        }
        for h in u.height + 1..height {
            let Some(e) = self.branch_at(parent, h) else { return invalid("uncle ancestry unavailable") };
            let Some(b) = self.get_block(&e.hash).await? else { return Err(ChainError::Internal("branch block missing".into())) };
            if b.uncles.iter().any(|x| x.header.pow_digest() == d) {
                return invalid("uncle has the same pow_digest as an uncle already included on this branch");
            }
        }
        Ok(())
    }

    /// Rules 1-8 for one uncle of a child of `parent` (a live block) at `height`, against the
    /// parent's state. Returns the job (for the reward ranking of templates).
    async fn check_uncle(&self, parent: &Hash, snap: &Snapshot, height: u64, u: &Header, proof: &[u8]) -> Result<JobRecord, ChainError> {
        if u.version != HEADER_VERSION {
            return invalid("uncle header version");
        }
        if u.job_id == [0; 32] {
            return invalid("an uncle must be a job block (fallback uncles earn nothing)");
        }
        let depth = height.checked_sub(u.height).unwrap_or(0);
        if depth < 1 || depth > MAX_UNCLE_DEPTH {
            return invalid(format!("uncle depth {depth} (allowed 1..={MAX_UNCLE_DEPTH})"));
        }
        if u.height == 0 {
            return invalid("uncle parent is not on the branch"); // same refusal as before, without the u64 wrap
        }
        let Some(up) = self.branch_at(parent, u.height - 1) else { return invalid("uncle parent is not on the branch") };
        if u.prev_hash != up.hash {
            return invalid("uncle parent is not on the branch");
        }
        let Some(sib) = self.branch_at(parent, u.height) else { return invalid("no branch block at the uncle's height") };
        if sib.hash == u.hash() {
            return invalid("an uncle cannot be on the branch");
        }
        if self.store.get(snap, &schema::uncle_key(&u.hash())).await.map_err(|e| ChainError::Internal(e.to_string()))?.is_some() {
            return invalid("uncle already included on this branch");
        }
        if height >= uncle_digest_rule_height(self.genesis.chain_id) {
            self.check_uncle_digest(parent, height, u, &sib.hash).await?;
        }
        if u.target_bits != sib.target_bits {
            return invalid("uncle target_bits differ from its sibling's");
        }
        let Some(mtp) = self.branch_mtp(parent, u.height - 1) else { return invalid("uncle ancestry unavailable") };
        if u.timestamp_ms <= mtp {
            return invalid("uncle timestamp not after median time past");
        }
        let Some(v) = self.store.get(snap, &schema::job_key(&u.job_id)).await.map_err(|e| ChainError::Internal(e.to_string()))? else {
            return invalid("uncle names an unknown job");
        };
        let job = JobRecord::decode(&v).map_err(|e| ChainError::Internal(e.to_string()))?;
        if job.created_height >= u.height {
            return invalid("uncle names a job posted after its parent");
        }
        // the job must have been mineable at the uncle's height (audit 2026-10-23, own height)
        if height >= agent_types::speed::uncle_job_rule_height(self.genesis.chain_id) {
            if let Err(e) = executor::check_mineable(&job, u.height) {
                return invalid(format!("uncle names a job that was not mineable: {e}"));
            }
        }
        let pj = job.pearl_job().map_err(ChainError::Invalid)?;
        let cfg = self.live.get(parent).map(|n| n.config.clone()).ok_or_else(|| ChainError::Internal("parent not live".into()))?;
        if let Err(e) = self.verifier.verify(u, Some(&pj), proof, &cfg.fallback_params()) {
            return invalid(format!("bad uncle proof of work: {e}"));
        }
        Ok(job)
    }

    /// Check a block's uncles (rules 1-8) and return them as executor inputs, in body order.
    pub(crate) async fn check_block_uncles(&self, b: &agent_types::Block, snap: &Snapshot) -> Result<Vec<UncleInput>, ChainError> {
        if b.uncles.is_empty() {
            return Ok(Vec::new());
        }
        let h = b.header.height;
        let act = speed_upgrade_height(self.genesis.chain_id);
        if h < act {
            return invalid(format!("uncles are valid from height {act}"));
        }
        if b.uncles.len() > MAX_UNCLES {
            return invalid("too many uncles");
        }
        let digest_rule = h >= uncle_digest_rule_height(self.genesis.chain_id);
        let mut out = Vec::with_capacity(b.uncles.len());
        for (i, u) in b.uncles.iter().enumerate() {
            if b.uncles[..i].iter().any(|x| x.header.hash() == u.header.hash()) {
                return invalid("duplicate uncle");
            }
            if digest_rule && b.uncles[..i].iter().any(|x| x.header.pow_digest() == u.header.pow_digest()) {
                return invalid("duplicate uncle (same pow_digest: two proofs of one block)");
            }
            self.check_uncle(&b.header.prev_hash, snap, h, &u.header, &u.proof).await?;
            out.push(UncleInput { hash: u.header.hash(), height: u.header.height, miner: u.header.miner, job_id: u.header.job_id });
        }
        Ok(out)
    }

    /// Uncles a template on `parent` should carry: live, non-canonical-on-this-branch job blocks
    /// this node connected that pass rules 1-8, larger expected reward first (shallower depth,
    /// then the job's remaining cap), at most MAX_UNCLES. Proofs come from the archive.
    pub(crate) async fn template_uncles(&self, parent: &Hash) -> Result<Vec<Uncle>, ChainError> {
        let Some(p) = self.live.get(parent) else { return Ok(Vec::new()) };
        let height = p.header.height + 1;
        if height < speed_upgrade_height(self.genesis.chain_id) {
            return Ok(Vec::new());
        }
        let snap = p.snapshot.clone();
        let lo = height.saturating_sub(MAX_UNCLE_DEPTH).max(1);
        let cands: Vec<Header> = self
            .live
            .values()
            .filter(|n| n.header.height >= lo && n.header.height < height && n.header.job_id != [0; 32])
            .map(|n| n.header)
            .collect();
        let mut ranked = Vec::new();
        for c in cands {
            let Some(b) = self.get_block(&c.hash()).await? else { continue };
            if let Ok(job) = self.check_uncle(parent, &snap, height, &c, &b.proof).await {
                let depth = height - c.height;
                let (n, d) = uncle_share(depth).unwrap_or((0, 1));
                ranked.push((depth, std::cmp::Reverse(job.cap_remaining as u128 * n as u128 / d as u128), c.hash(), Uncle {
                    header: c,
                    proof: b.proof,
                    record: Default::default(),
                }));
            }
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let digest_rule = height >= uncle_digest_rule_height(self.genesis.chain_id);
        let mut out: Vec<Uncle> = Vec::new();
        for r in ranked {
            if out.len() == MAX_UNCLES {
                break;
            }
            // rule 8: two proofs of one block are one uncle (keep the first by rank)
            if digest_rule && out.iter().any(|x| x.header.pow_digest() == r.3.header.pow_digest()) {
                continue;
            }
            out.push(r.3);
        }
        Ok(out)
    }
}
