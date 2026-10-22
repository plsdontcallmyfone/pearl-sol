//! Speed upgrade fields in the RPC (docs/SPEED.md 3): uncles in getBlock, the speed height in
//! getParams and getChainInfo.

use agent_types::Block;
use serde_json::{json, Value};

/// getBlock `uncles`: `[{hash, height, miner, job_id, reward, nephew, mature_height, path}]`
/// (`path` = the record's tx_root path, 33-byte entries, hex).
pub fn uncles_json(b: &Block) -> Value {
    Value::Array(
        b.uncles
            .iter()
            .enumerate()
            .map(|(i, u)| {
                let r = &u.record;
                json!({
                    "hash": hex::encode(r.uncle_hash), "height": r.uncle_height, "miner": bs58::encode(r.miner).into_string(),
                    "job_id": hex::encode(r.job_id), "reward": r.reward.to_string(), "nephew": r.nephew.to_string(),
                    "mature_height": r.mature_height, "proof_len": u.proof.len(), "record": hex::encode(r.encode()),
                    "path": hex::encode(b.leaf_path(b.uncle_index(i))),
                })
            })
            .collect(),
    )
}

/// `{speed_height, speed_migration_height}` for a chain id.
pub fn speed_fields(chain_id: u32) -> (u64, Option<u64>) {
    (agent_types::speed::speed_upgrade_height(chain_id), agent_types::speed::speed_migration_height(chain_id))
}
