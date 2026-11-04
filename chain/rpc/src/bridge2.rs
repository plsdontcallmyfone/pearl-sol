//! RPC for the two-side bridge (docs/BRIDGE-V2.md, docs/RPC.md "Bridge v2"):
//! getDeposit / getWithdrawalProof / getBridgeEvents with `domain` >= 2, getBridgeCounters,
//! and the bridge v2 fields of getParams. Without `domain` (or with domain 1) the original
//! methods answer exactly as before.

use agent_types::bridge2::{WithdrawalV2, DOMAIN_SOLANA, EXT_SCALE};
use agent_types::{escrow_address, TOTAL_SUPPLY};
use commonware_runtime::Spawner;
use consensus_pow::{Chain, PowVerifier};
use serde_json::{json, Value};
use state::bridge2::{self as b2, BridgeParams, DepositV2, DomainCfg, DomainStats};

use crate::core::Core;
use crate::dispatch::{b58, b64, p_u64, srv, RpcError, R};

/// An EVM recipient (20 bytes left-padded) as 0x hex, else None.
fn evm_addr(r: &[u8; 32]) -> Option<String> {
    (r[..12] == [0; 12]).then(|| format!("0x{}", hex::encode(&r[12..])))
}

pub fn withdrawal_v2_json<E: commonware_storage::Context + Spawner, V: PowVerifier>(c: &Chain<E, V>, w: &WithdrawalV2) -> Value {
    let bh = c.canonical_hash(w.height);
    let conf = bh.map(|h| c.confirmations(&h, w.height)).unwrap_or(0);
    json!({
        "version": 2, "domain": w.domain, "nonce": w.nonce, "from": b58(&w.from),
        "recipient": hex::encode(w.recipient), "recipient_evm": evm_addr(&w.recipient),
        "amount_ext": w.amount_ext.to_string(), "amount": (w.amount_ext as u128 * EXT_SCALE as u128).to_string(),
        "height": w.height, "block_hash": bh.map(hex::encode), "confirmations": conf,
        "entry": hex::encode(w.log_entry()),
        "final": c.withdrawal_v2_location(w.domain, w.nonce).is_some(),
    })
}

pub fn domain_json(d: &DomainCfg) -> Value {
    json!({"domain": d.domain, "ext_chain_id": d.ext_chain_id, "bridge_addr": format!("0x{}", hex::encode(&d.bridge_addr)),
           "evm": d.is_evm(), "paused": d.paused})
}

/// getParams additions.
pub fn params_fields(v: &mut Value, p: &BridgeParams, next_height: u64) {
    v["bridge_v2_height"] = if p.activation_height == u64::MAX { Value::Null } else { json!(p.activation_height) };
    v["bridge_v2_active"] = json!(p.is_active(next_height));
    v["bridge_domains"] = Value::Array(p.domains.iter().map(domain_json).collect());
    v["keys"]["bridge_v2_height"] = json!(b2::keys::BRIDGE_V2_HEIGHT);
    v["keys"]["bridge_domain"] = json!(b2::keys::BRIDGE_DOMAIN);
}

fn p_domain(p: &Value) -> Result<Option<u8>, RpcError> {
    match p.get("domain") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().filter(|d| *d <= u8::MAX as u64).map(|d| Some(d as u8)).ok_or_else(|| RpcError::params("domain must be 0..=255")),
    }
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    pub(crate) async fn bridge_params(&self) -> Result<BridgeParams, RpcError> {
        Ok(match srv(self.chain.read_tip(&b2::params_key()).await)? {
            Some(b) => srv(BridgeParams::decode(&b))?,
            None => BridgeParams::default(),
        })
    }

    async fn domain_stats(&self, d: u8) -> Result<DomainStats, RpcError> {
        Ok(match srv(self.chain.read_tip(&b2::domain_stats_key(d)).await)? {
            Some(b) => srv(DomainStats::decode(&b))?,
            None => DomainStats::default(),
        })
    }

    /// Answers the bridge v2 methods; `None` hands the call to the original methods.
    pub(crate) async fn bridge2_dispatch(&self, method: &str, p: &Value) -> Option<R> {
        match method {
            "getBridgeCounters" => Some(self.bridge_counters().await),
            "getDeposit" | "getWithdrawalProof" | "getBridgeEvents" => match p_domain(p) {
                Err(e) => Some(Err(e)),
                Ok(None) | Ok(Some(DOMAIN_SOLANA)) => None,
                Ok(Some(d)) => Some(match method {
                    "getDeposit" => self.deposit_v2(d, p).await,
                    "getWithdrawalProof" => self.withdrawal_v2_proof(d, p).await,
                    _ => self.bridge_events_v2(d, p).await,
                }),
            },
            _ => None,
        }
    }

    async fn bridge_counters(&self) -> R {
        let p = self.bridge_params().await?;
        let esc = srv(self.account(&escrow_address()).await)?.balance;
        let mut ext_sum: u64 = 0;
        let mut domains = Vec::new();
        for d in &p.domains {
            let s = self.domain_stats(d.domain).await?;
            ext_sum = ext_sum.saturating_add(s.outstanding());
            domains.push(json!({
                "domain": d.domain, "ext_chain_id": d.ext_chain_id, "paused": d.paused,
                "unlocked": s.unlocked.to_string(), "locked": s.locked.to_string(), "outstanding": s.outstanding().to_string(),
                "outstanding_ext": (s.outstanding() / EXT_SCALE).to_string(),
                "deposit_count": s.deposit_count, "withdrawal_count": s.withdrawal_count,
            }));
        }
        let c = srv(self.chain.read_tip(&state::schema::counters_key()).await)?
            .map(|b| state::schema::Counters::decode(&b))
            .transpose()
            .map_err(RpcError::server)?
            .unwrap_or_default();
        let sol = TOTAL_SUPPLY.saturating_sub(esc).saturating_sub(ext_sum);
        let mut all = vec![json!({
            "domain": DOMAIN_SOLANA, "derived": true, "outstanding": sol.to_string(), "outstanding_ext": (sol / EXT_SCALE).to_string(),
            "deposit_count": c.deposit_count, "withdrawal_count": c.withdrawal_count,
            "rule": "TOTAL_SUPPLY - escrow - sum(outstanding of domains >= 2)",
        })];
        all.extend(domains);
        Ok(json!({
            "height": self.chain.tip_height(), "escrow_balance": esc.to_string(), "total_supply": TOTAL_SUPPLY.to_string(),
            "bridge_v2_height": if p.activation_height == u64::MAX { Value::Null } else { json!(p.activation_height) },
            "bridge_v2_active": p.is_active(self.chain.tip_height() + 1), "domains": all,
        }))
    }

    async fn deposit_v2(&self, d: u8, p: &Value) -> R {
        let n = p_u64(p, "nonce").or_else(|| p_u64(p, "ext_nonce")).ok_or_else(|| RpcError::params("missing nonce"))?;
        match srv(self.chain.read_tip(&b2::deposit_v2_key(d, n)).await)? {
            None => Ok(json!({"processed": false, "domain": d, "nonce": n})),
            Some(b) => {
                let x = srv(DepositV2::decode(&b))?;
                let bh = self.chain.canonical_hash(x.height);
                let conf = bh.map(|h| self.chain.confirmations(&h, x.height)).unwrap_or(0);
                Ok(json!({"processed": true, "domain": d, "nonce": n, "height": x.height, "recipient": b58(&x.recipient),
                          "amount_ext": x.amount_ext.to_string(), "confirmations": conf,
                          "final": x.height <= self.chain.finalized().header.height}))
            }
        }
    }

    async fn withdrawal_v2(&self, d: u8, n: u64) -> Result<Option<WithdrawalV2>, RpcError> {
        match srv(self.chain.read_tip(&b2::withdrawal_v2_key(d, n)).await)? {
            None => Ok(None),
            Some(b) => Ok(Some(srv(b2::decode_withdrawal_v2(&b))?)),
        }
    }

    async fn bridge_events_v2(&self, d: u8, p: &Value) -> R {
        let from = p_u64(p, "from_nonce").unwrap_or(0);
        let limit = p_u64(p, "limit").unwrap_or(100).min(1000);
        let s = self.domain_stats(d).await?;
        let mut out = Vec::new();
        for n in from..s.withdrawal_count.min(from.saturating_add(limit)) {
            if let Some(w) = self.withdrawal_v2(d, n).await? {
                out.push(withdrawal_v2_json(&self.chain, &w));
            }
        }
        Ok(json!({"domain": d, "withdrawal_count": s.withdrawal_count, "deposit_count": s.deposit_count, "withdrawals": out}))
    }

    async fn withdrawal_v2_proof(&self, d: u8, p: &Value) -> R {
        let n = p_u64(p, "nonce").ok_or_else(|| RpcError::params("missing nonce"))?;
        let Some(w) = self.withdrawal_v2(d, n).await? else { return Ok(Value::Null) };
        let wj = withdrawal_v2_json(&self.chain, &w);
        if let Some(at) = p_u64(p, "at_height") {
            if at < w.height {
                return Err(RpcError::params(format!("at_height {at} is below the withdrawal's block {}", w.height)));
            }
            if at > self.chain.finalized().header.height {
                return Err(RpcError::params(format!("at_height {at} is not final (final height {})", self.chain.finalized().header.height)));
            }
            return match srv(self.chain.withdrawal_v2_proof_at(d, n, at).await)? {
                None => Ok(json!({"withdrawal": wj, "final": false, "proof": null})),
                Some((entry, loc, proof, root, hash)) => Ok(json!({"withdrawal": wj, "final": true, "entry": hex::encode(entry),
                    "location": loc, "proof": b64(&proof), "withdraw_root": hex::encode(root),
                    "root_height": at, "root_block_hash": hex::encode(hash)})),
            };
        }
        match srv(self.chain.withdrawal_v2_proof(d, n).await)? {
            None => Ok(json!({"withdrawal": wj, "final": false, "proof": null})),
            Some((entry, loc, proof, root)) => {
                let f = self.chain.finalized();
                Ok(json!({"withdrawal": wj, "final": true, "entry": hex::encode(entry), "location": loc,
                          "proof": b64(&proof), "withdraw_root": hex::encode(root),
                          "root_height": f.header.height, "root_block_hash": hex::encode(f.hash)}))
            }
        }
    }
}
