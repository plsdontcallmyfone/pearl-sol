//! What the pool needs from a node beyond [`NodeApi`]: the tip, canonical block hashes
//! by height (orphan check at maturity) and `finality_k` (when coinbase matures).

use anyhow::{anyhow, Result};
use serde_json::json;

use pearl_node::mock::MockNode;
use pearl_node::{key32, num, HttpNode, NodeApi, Template};

pub trait PoolNode: NodeApi {
    /// `(tip hash, tip height)`.
    fn tip(&self) -> Result<([u8; 32], u64)>;
    /// Hash of the canonical block at `height`, None if the chain is shorter.
    fn canonical_hash(&self, height: u64) -> Result<Option<[u8; 32]>>;
    /// Coinbase (fees + subsidy) is credited to the header's miner this many blocks later.
    fn finality_k(&self) -> Result<u64>;
    /// What a block on this template pays its miner: `fees + subsidy` (docs/RPC.md
    /// getBlockTemplate). Missing fields count as zero.
    fn template_reward(&self, t: &Template) -> u64 {
        let g = |k: &str| t.raw.get(k).filter(|x| !x.is_null()).and_then(|x| num(x).ok()).unwrap_or(0);
        g("fees").saturating_add(g("subsidy"))
    }
}

impl PoolNode for HttpNode {
    fn tip(&self) -> Result<([u8; 32], u64)> {
        let v = self.call("getChainInfo", json!({}))?;
        Ok((key32(&v["tip"])?, num(&v["height"])?))
    }

    fn canonical_hash(&self, height: u64) -> Result<Option<[u8; 32]>> {
        let v = self.call("getHeader", json!({"height": height}))?;
        if v.is_null() {
            return Ok(None);
        }
        if v.get("canonical").and_then(|c| c.as_bool()) == Some(false) {
            return Ok(None);
        }
        Ok(Some(key32(v.get("hash").ok_or_else(|| anyhow!("getHeader has no hash"))?)?))
    }

    fn finality_k(&self) -> Result<u64> {
        let v = self.call("getParams", json!({}))?;
        num(v.get("finality_k").ok_or_else(|| anyhow!("getParams has no finality_k"))?)
    }
}

/// The mock pays nothing and has no finality parameter: tests wrap it (see
/// tests/pool.rs) or set `PoolConfig::maturity`.
impl PoolNode for MockNode {
    fn tip(&self) -> Result<([u8; 32], u64)> {
        Ok(self.state.lock().unwrap().tip())
    }

    fn canonical_hash(&self, height: u64) -> Result<Option<[u8; 32]>> {
        let s = self.state.lock().unwrap();
        Ok(s.blocks.iter().find(|b| b.header.height == height).map(|b| b.header.block_hash()))
    }

    fn finality_k(&self) -> Result<u64> {
        Err(anyhow!("MockNode has no finality_k; set PoolConfig::maturity"))
    }
}
