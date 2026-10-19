//! RPC reads of RejectResult records (SPEC 4.1, docs/RPC.md): getJobRejects,
//! getRejectStats, getJob.reject_count.

use commonware_runtime::Spawner;
use consensus_pow::PowVerifier;
use serde_json::{json, Value};
use state::reject::{self, Reject, RejectStats};

use crate::core::Core;
use crate::dispatch::{b58, RpcError};

fn corrupt(e: impl std::fmt::Display) -> RpcError {
    RpcError::server(e)
}

pub fn reject_json(job_id: &[u8; 32], index: u32, r: &Reject) -> Value {
    json!({
        "job_id": hex::encode(job_id), "index": index, "deliverer": b58(&r.deliverer),
        "result_hash": hex::encode(r.result_hash), "height": r.height, "protocol_fee": r.protocol_fee.to_string(),
        "signers": r.signers.iter().map(|s| b58(s)).collect::<Vec<_>>(),
    })
}

pub fn reject_stats_json(account: &[u8; 32], s: &RejectStats) -> Value {
    json!({
        "account": b58(account), "rejected": s.rejected, "rejected_fee": s.rejected_fee.to_string(),
        "first_height": s.first_height, "last_height": s.last_height,
    })
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    pub(crate) async fn reject_count(&self, job_id: &[u8; 32]) -> Result<u32, RpcError> {
        match self.chain.read_tip(&reject::reject_count_key(job_id)).await.map_err(corrupt)? {
            Some(b) => reject::decode_count(&b).map_err(corrupt),
            None => Ok(0),
        }
    }

    pub(crate) async fn job_rejects(&self, job_id: &[u8; 32]) -> Result<Value, RpcError> {
        let n = self.reject_count(job_id).await?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Some(b) = self.chain.read_tip(&reject::reject_key(job_id, i)).await.map_err(corrupt)? {
                out.push(reject_json(job_id, i, &Reject::decode(&b).map_err(corrupt)?));
            }
        }
        Ok(json!({"job_id": hex::encode(job_id), "count": n, "rejects": out}))
    }

    pub(crate) async fn reject_stats(&self, a: &[u8; 32]) -> Result<Value, RpcError> {
        let s = match self.chain.read_tip(&reject::reject_stats_key(a)).await.map_err(corrupt)? {
            Some(b) => RejectStats::decode(&b).map_err(corrupt)?,
            None => RejectStats::default(),
        };
        Ok(reject_stats_json(a, &s))
    }
}
