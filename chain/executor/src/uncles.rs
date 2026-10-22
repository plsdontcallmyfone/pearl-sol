//! Chain speed upgrade in the executor (docs/SPEED.md 3.1-3.3): the one-time 60 s -> 30 s
//! config migration and uncle rewards.
//!
//! Uncles are validated by the chain (consensus-pow chain/uncles.rs: branch, target, proof);
//! here they only pay: `base = min(emission(pool), job.cap_remaining)`, `reward = base x
//! share(depth)`, `nephew = reward / 32`, both from the pool and the job's cap, maturing at
//! `height + K` to the uncle's miner and to this block's miner. `job.blocks_mined` is not
//! touched (max_blocks counts canonical job blocks).

use agent_types::speed::{speed_migration_height, uncle_reward, UncleRecord, MAX_UNCLES};
use agent_types::{pool_address, Hash, Pubkey, Tx};
use state::schema::{self, Immature};
use state::{StateError, StateRead, WriteSet};

use crate::{BlockEnv, BlockExecutor, BlockSummary, ExecError};

/// One uncle as the executor sees it (the chain checked everything else).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UncleInput {
    pub hash: Hash,
    pub height: u64,
    pub miner: Pubkey,
    pub job_id: Hash,
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Uncles this block credits, in body order (call before `finish`).
    pub fn set_uncles(&mut self, uncles: Vec<UncleInput>) {
        self.uncles = uncles;
    }

    /// docs/SPEED.md 3.1: block `S` of a migrating chain halves the target time, doubles K and
    /// sets `emission_ppb` to half of `emission_ppm` (in ppb). Runs once, at exactly that height.
    pub(crate) async fn speed_migration(&mut self) -> Result<(), ExecError> {
        if speed_migration_height(self.env.chain_id) != Some(self.env.height) {
            return Ok(());
        }
        let mut c = self.cfg.clone();
        c.target_block_time_ms /= 2;
        c.finality_k = c.finality_k.saturating_mul(2);
        c.emission_ppb = c.emission_ppm * 1000 / 2;
        c.validate().map_err(|e| ExecError::Block(format!("speed migration: {e}")))?;
        self.ov.put(schema::config_key(), Some(c.encode()));
        self.cfg = c;
        self.ov.commit_tx();
        Ok(())
    }

    /// Pay this block's uncles (from `finish`, after the block's own subsidy).
    pub(crate) async fn credit_uncles(&mut self) -> Result<(), ExecError> {
        if self.uncles.is_empty() {
            return Ok(());
        }
        let h = self.env.height;
        let blk = |e: crate::TxError| ExecError::Block(e.to_string());
        if self.uncles.len() > MAX_UNCLES {
            return Err(ExecError::Block("too many uncles".into()));
        }
        let mature_height = h + self.cfg.finality_k as u64;
        let uncles = std::mem::take(&mut self.uncles);
        for u in &uncles {
            let depth = h.checked_sub(u.height).unwrap_or(0);
            let ukey = schema::uncle_key(&u.hash);
            if self.ov.get(&ukey).await?.is_some() {
                return Err(ExecError::Block("uncle already included".into()));
            }
            let mut j = self.ov.job(&u.job_id).await?.ok_or_else(|| ExecError::Block("uncle names an unknown job".into()))?;
            let pool = self.ov.account(&pool_address()).await?.balance;
            let base = self.cfg.emission_of(pool).min(j.cap_remaining);
            let (reward, nephew) = uncle_reward(base, depth).ok_or_else(|| ExecError::Block(format!("uncle depth {depth} out of range")))?;
            let total = reward + nephew;
            j.cap_remaining -= total;
            j.subsidy_paid += total;
            self.ov.put_job(&j);
            self.ov.debit(&pool_address(), total).await.map_err(blk)?;
            let k = schema::immature_key(mature_height);
            let mut imm = match self.ov.get(&k).await? {
                Some(b) => Immature::decode(&b).map_err(|e| StateError::Corrupt(e.to_string()))?,
                None => Immature::default(),
            };
            if reward > 0 {
                imm.entries.push((u.miner, reward, h));
            }
            if nephew > 0 {
                imm.entries.push((self.env.miner, nephew, h));
            }
            if !imm.entries.is_empty() {
                self.ov.put(k, Some(imm.encode()));
            }
            self.ov.put(ukey, Some(schema::encode_uncle_value(h)));
            self.summary.uncles.push(UncleRecord {
                height: h,
                uncle_hash: u.hash,
                uncle_height: u.height,
                miner: u.miner,
                job_id: u.job_id,
                reward,
                nephew,
                mature_height,
            });
        }
        Ok(())
    }
}

/// [`crate::execute_block`] with uncles: every tx must succeed, then the uncles are paid.
pub async fn execute_block_with_uncles<S: StateRead>(
    base: &S,
    env: BlockEnv,
    txs: &[Tx],
    uncles: Vec<UncleInput>,
) -> Result<(WriteSet, BlockSummary), ExecError> {
    let mut ex = BlockExecutor::begin(base, env).await?;
    if txs.len() > ex.cfg.max_block_txs as usize {
        return Err(ExecError::Block("too many txs".into()));
    }
    for (index, tx) in txs.iter().enumerate() {
        ex.apply_tx(tx).await.map_err(|e| ExecError::Tx { index, err: e.to_string() })?;
    }
    ex.set_uncles(uncles);
    ex.finish(true).await
}
