//! JSON-RPC methods (docs/RPC.md).

use agent_types::{block, Block, Hash, Header, Tx, SPL_SCALE, TOTAL_SUPPLY};
use base64::Engine as _;
use commonware_runtime::Spawner;
use consensus_pow::{Chain, JobChoice, PowVerifier};
use serde_json::{json, Value};
use state::config::{keys, Config};
use state::schema::{self, Agent, Counters, Deposit, JobRecord, Route, Withdrawal};

use crate::core::Core;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn params(m: impl Into<String>) -> Self {
        Self { code: -32602, message: m.into() }
    }
    pub fn server(m: impl std::fmt::Display) -> Self {
        Self { code: -32000, message: m.to_string() }
    }
    pub fn method(m: &str) -> Self {
        Self { code: -32601, message: format!("unknown method {m}") }
    }
}

pub(crate) type R = Result<Value, RpcError>;

/// Most segments getTicket lists in one call (each is a state read on the node actor).
pub const MAX_TICKET_SEGMENTS: usize = 4096;

pub fn b58(k: &[u8]) -> String {
    bs58::encode(k).into_string()
}

pub fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn opt_key(k: &[u8; 32]) -> Value {
    if *k == [0; 32] { Value::Null } else { json!(b58(k)) }
}

fn opt_hash(k: &[u8; 32]) -> Value {
    if *k == [0; 32] { Value::Null } else { json!(hex::encode(k)) }
}

/// A 32-byte value from hex (64 chars) or base58.
pub fn parse32(s: &str) -> Result<[u8; 32], RpcError> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(t).map(|v| v.try_into().unwrap()).map_err(|e| RpcError::params(e.to_string()));
    }
    let v = bs58::decode(t).into_vec().map_err(|_| RpcError::params(format!("{s:?} is not hex or base58")))?;
    v.try_into().map_err(|_| RpcError::params(format!("{s:?} is not 32 bytes")))
}

pub(crate) fn p_str<'a>(p: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| p.get(*k).and_then(|v| v.as_str()))
}

pub(crate) fn p_key(p: &Value, keys: &[&str]) -> Result<[u8; 32], RpcError> {
    parse32(p_str(p, keys).ok_or_else(|| RpcError::params(format!("missing {}", keys[0])))?)
}

pub(crate) fn p_u64(p: &Value, k: &str) -> Option<u64> {
    p.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// Binary from base64 (`key`) or hex (`key_hex`).
pub(crate) fn p_bytes(p: &Value, key: &str) -> Result<Vec<u8>, RpcError> {
    if let Some(s) = p.get(key).and_then(|v| v.as_str()) {
        return base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| RpcError::params(format!("{key}: {e}")));
    }
    if let Some(s) = p.get(format!("{key}_hex")).and_then(|v| v.as_str()) {
        return hex::decode(s.trim().trim_start_matches("0x")).map_err(|e| RpcError::params(format!("{key}_hex: {e}")));
    }
    Err(RpcError::params(format!("missing {key}")))
}

pub fn header_fields(h: &Header) -> Value {
    json!({
        "version": h.version, "height": h.height, "prev_hash": hex::encode(h.prev_hash),
        "timestamp_ms": h.timestamp_ms, "target_bits": h.target_bits, "job_id": hex::encode(h.job_id),
        "miner": b58(&h.miner), "tx_root": hex::encode(h.tx_root), "state_root": hex::encode(h.state_root),
        "withdraw_root": hex::encode(h.withdraw_root), "proof_hash": hex::encode(h.proof_hash), "nonce": h.nonce,
    })
}

fn u256_hex(x: primitive_types::U256) -> String {
    format!("{x:x}")
}

pub fn job_json(j: &JobRecord, next_height: u64) -> Value {
    json!({
        "job_id": hex::encode(j.job_id), "poster": b58(&j.poster), "status": j.status.name(),
        "mineable": executor::check_mineable(j, next_height).is_ok(),
        "m": j.m, "n": j.n, "k": j.k, "config": hex::encode(j.config),
        "hash_a": hex::encode(j.hash_a), "hash_b": hex::encode(j.hash_b),
        "data_uri": String::from_utf8_lossy(&j.data_uri),
        "worker_pay": j.worker_pay.to_string(), "protocol_fee": j.protocol_fee.to_string(),
        "deadline_height": j.deadline_height, "max_blocks": j.max_blocks, "blocks_mined": j.blocks_mined,
        "cap_remaining": j.cap_remaining.to_string(), "subsidy_paid": j.subsidy_paid.to_string(),
        "evaluators": j.evaluators.iter().map(|e| b58(e)).collect::<Vec<_>>(), "threshold": j.threshold,
        "terms_hash": hex::encode(j.terms_hash),
        "job_key": j.job_key().map(hex::encode).unwrap_or_default(),
        "job_header": hex::encode(j.pearl_header().to_bytes()),
        "header_id": hex::encode(j.header_id),
        "created_height": 0, "share_target": 0, "posted_height": j.created_height,
        "deliverer": opt_key(&j.deliverer), "result_hash": opt_hash(&j.result_hash),
        "result_uri": String::from_utf8_lossy(&j.result_uri),
        "closed_height": j.closed_height, "index": j.index,
        "kind": j.kind_name(), "standing": standing_json(j),
    })
}

/// Ticket / segment fields of a job (SPEC 4.5), or null for a plain job.
pub fn standing_json(j: &JobRecord) -> Value {
    match &j.ext {
        None => Value::Null,
        Some(schema::JobExt::Ticket(t)) => json!({
            "model_id": hex::encode(t.model_id), "server": opt_key(&t.server), "rows": t.rows,
            "prompt_tokens": j.m, "max_new_tokens": j.n, "request_hash": hex::encode(j.hash_a),
            "cap_total": t.cap_total.to_string(), "cap_unallocated": j.cap_remaining.to_string(),
            "segments": t.segments, "blocks_allocated": t.blocks_allocated, "blocks_mined": t.blocks_mined,
            "subsidy_paid": t.subsidy_paid.to_string(),
            "rounds": t.rounds, "round_rows": t.round_rows, "rows_used": t.rows_used, "rows_free": t.rows - t.rows_used.min(t.rows),
        }),
        Some(schema::JobExt::Segment { ticket_id, model_id, matrix }) => json!({
            "ticket_id": hex::encode(ticket_id), "model_id": hex::encode(model_id), "matrix": matrix,
            "c_hash": hex::encode(j.result_hash),
        }),
    }
}

/// A registered model (SPEC 4.5).
pub fn model_json(m: &schema::Model) -> Value {
    let base = m.base_config().ok();
    let matrices: Vec<Value> = m
        .matrices
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let key = base.map(|b| hex::encode(pearl_job::standing::matrix_job_key(&m.model_id, i as u16, x.k, x.n, &b)));
            json!({"index": i, "k": x.k, "n": x.n, "macs": (x.k as u64 * x.n as u64).to_string(),
                   "hash_b": hex::encode(x.hash_b), "job_key": key})
        })
        .collect();
    json!({
        "model_id": hex::encode(m.model_id), "registrant": b58(&m.registrant), "name": String::from_utf8_lossy(&m.name),
        "manifest_uri": String::from_utf8_lossy(&m.manifest_uri), "recipe": m.recipe,
        "recipe_name": if m.recipe == pearl_job::standing::RECIPE_W7A7_SQ_ASYM { pearl_job::standing::RECIPE_NAME } else { "unknown" },
        "config": hex::encode(m.config), "row_period": base.map(|b| b.rows_pattern.period()),
        "macs_per_row": m.macs_per_row.to_string(), "registered_height": m.registered_height, "index": m.index,
        "tickets": m.tickets, "segments": m.segments, "blocks_mined": m.blocks_mined, "subsidy_paid": m.subsidy_paid.to_string(),
        "matrix_count": m.matrices.len(), "matrices": matrices,
    })
}

pub fn withdrawal_json<E: commonware_storage::Context + Spawner, V: PowVerifier>(c: &Chain<E, V>, w: &Withdrawal) -> Value {
    let bh = c.canonical_hash(w.height);
    let conf = bh.map(|h| c.confirmations(&h, w.height)).unwrap_or(0);
    json!({
        "nonce": w.nonce, "from": b58(&w.from), "solana_recipient": b58(&w.solana_recipient),
        "amount_spl": w.amount_spl.to_string(), "amount": (w.amount_spl as u128 * SPL_SCALE as u128).to_string(),
        "height": w.height, "block_hash": bh.map(hex::encode), "confirmations": conf,
        "final": c.withdrawal_location(w.nonce).is_some(),
    })
}

pub fn config_json(c: &Config) -> Value {
    let mut keymap = serde_json::Map::new();
    for k in 0..=keys::LAST {
        keymap.insert(keys::name(k).into(), json!(k));
    }
    json!({
        "admin": b58(&c.admin), "min_fee": c.min_fee.to_string(), "target_block_time_ms": c.target_block_time_ms,
        "wtema_half_life_ms": c.wtema_half_life_ms, "pow_limit_bits": c.pow_limit_bits, "finality_k": c.finality_k,
        "emission_ppm": c.emission_ppm, "alpha_ppm": c.alpha_ppm,
        "attesters": c.attesters.iter().map(|a| b58(a)).collect::<Vec<_>>(),
        "attester_threshold": c.attester_threshold, "max_block_txs": c.max_block_txs, "max_future_ms": c.max_future_ms,
        "fallback_m": c.fallback_m, "fallback_n": c.fallback_n, "fallback_config": hex::encode(c.fallback_config),
        "min_protocol_fee": c.min_protocol_fee.to_string(),
        "min_protocol_fee_effective": executor::effective_min_protocol_fee(c).to_string(), "bridge_paused": c.bridge_paused,
        "max_job_blocks": c.max_job_blocks, "min_difficulty_bits": c.min_difficulty_bits,
        "social_min_fee": c.social_min_fee.to_string(), "social_byte_fee": c.social_byte_fee.to_string(),
        "house_operators": c.house_operators.iter().map(|a| b58(a)).collect::<Vec<_>>(),
        "post_inline_max": c.post_inline_max, "max_balance_rows": c.max_balance_rows,
        "price_base_lamports": c.price_base_lamports.map(|x| x.to_string()),
        "price_mlamports_per_gflop": c.price_mlamports_per_gflop.map(|x| x.to_string()),
        "price_overhead_lamports": c.price_overhead_lamports.map(|x| x.to_string()),
        "gas_price": c.gas_price.to_string(), "gas_price_unit": state::config::GAS_PRICE_UNIT.to_string(),
        "block_gas_limit": c.block_gas_limit.to_string(), "tx_gas_limit": c.tx_gas_limit.to_string(),
        "contract_base_fee": c.contract_base_fee.to_string(), "contract_byte_fee": c.contract_byte_fee.to_string(),
        "storage_byte_deposit": c.storage_byte_deposit.to_string(), "max_code_size": c.max_code_size, "max_call_depth": c.max_call_depth,
        "keys": keymap,
    })
}

fn tx_summary(t: &Tx) -> Value {
    json!({"tx_hash": hex::encode(t.hash()), "kind": t.body.kind().name(), "from": b58(&t.from),
           "nonce": t.nonce, "fee": t.fee.to_string(), "tx": b64(&t.encode())})
}

/// A block's coinbase receipt (R1) with its tx_root path (the last leaf).
fn coinbase_json(b: &Block) -> Value {
    let Some(c) = &b.coinbase else { return Value::Null };
    let index = b.coinbase_index();
    json!({
        "record": hex::encode(c.encode()), "receipt_hash": hex::encode(c.hash()),
        "height": c.height, "miner": b58(&c.miner), "fees": c.fees.to_string(), "subsidy": c.subsidy.to_string(),
        "reward": (c.fees as u128 + c.subsidy as u128).to_string(), "job_id": hex::encode(c.job_id),
        "mature_height": c.mature_height, "leaf_index": index,
        "path": hex::encode(b.leaf_path(index)),
    })
}

/// A block's contracts record (docs/CONTRACTS.md) with its tx_root path.
fn contracts_json(b: &Block) -> Value {
    let Some(c) = &b.contracts else { return Value::Null };
    let index = b.contracts_index();
    json!({
        "record": hex::encode(c.encode()), "record_hash": hex::encode(c.hash()), "height": c.height,
        "receipt_count": c.receipt_count, "receipts_root": hex::encode(c.receipts_root),
        "event_count": c.event_count, "events_root": hex::encode(c.events_root), "gas_used": c.gas_used.to_string(),
        "leaf_index": index, "path": hex::encode(b.leaf_path(index)),
    })
}

/// A payout lien (R2) as of `height` (the next block's height for "active").
pub fn route_json(account: &[u8; 32], r: &Route, height: u64) -> Value {
    json!({
        "account": b58(account), "solana_recipient": b58(&r.solana_recipient), "route_bps": r.route_bps,
        "until_height": r.until_height, "cap": r.cap.to_string(),
        "release_key": if r.release_key == [0; 32] { Value::Null } else { json!(b58(&r.release_key)) },
        "route_nonce": r.route_nonce, "set_height": r.set_height, "routed": r.routed.to_string(),
        "pending": r.pending.to_string(), "withdrawn": r.withdrawn.to_string(), "cleared": r.cleared,
        "active": r.is_active(height), "remaining_cap": (r.cap - r.routed).to_string(),
    })
}

fn srv<T, Er: std::fmt::Display>(r: Result<T, Er>) -> Result<T, RpcError> {
    r.map_err(RpcError::server)
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    async fn tip_record(&self, key: &schema::Key) -> Result<Option<Vec<u8>>, RpcError> {
        srv(self.chain.read_tip(key).await)
    }

    async fn route(&self, a: &[u8; 32]) -> Result<Value, RpcError> {
        let Some(b) = self.tip_record(&schema::route_key(a)).await? else { return Ok(Value::Null) };
        let r = srv(Route::decode(&b))?;
        let mut v = route_json(a, &r, self.chain.tip_height() + 1);
        // R8: where the current SetRoute and the last ClearRoute sit (canonical chain).
        let set = srv(self.chain.route_set_tx(a, r.set_height, r.route_nonce).await)?;
        v["set_block"] = set.map(|e| self.route_tx_json(&e)).unwrap_or(Value::Null);
        v["last_clear"] = self.chain.last_clear(a).map(|e| self.route_tx_json(&e)).unwrap_or(Value::Null);
        Ok(v)
    }

    fn route_tx_json(&self, e: &consensus_pow::RouteTxEntry) -> Value {
        json!({"block_hash": hex::encode(e.block_hash), "height": e.height, "index": e.index, "route_nonce": e.route_nonce,
               "confirmations": self.chain.confirmations(&e.block_hash, e.height), "final": self.chain.is_final(&e.block_hash, e.height)})
    }

    async fn counters(&self) -> Result<Counters, RpcError> {
        Ok(match self.tip_record(&schema::counters_key()).await? {
            Some(b) => srv(Counters::decode(&b))?,
            None => Counters::default(),
        })
    }

    async fn job(&self, id: &Hash) -> Result<Option<JobRecord>, RpcError> {
        match self.tip_record(&schema::job_key(id)).await? {
            Some(b) => Ok(Some(srv(JobRecord::decode(&b))?)),
            None => Ok(None),
        }
    }

    async fn block_by(&self, p: &Value) -> Result<Option<Block>, RpcError> {
        let hash = if let Some(h) = p_str(p, &["hash", "block_hash"]) {
            parse32(h)?
        } else if let Some(h) = p_u64(p, "height") {
            match self.chain.canonical_hash(h) {
                Some(x) => x,
                None => return Ok(None),
            }
        } else {
            return Err(RpcError::params("need hash or height"));
        };
        srv(self.chain.get_block(&hash).await)
    }

    fn block_meta(&self, b: &Block) -> Value {
        let h = b.hash();
        let height = b.header.height;
        let cw = self
            .chain
            .live_node(&h)
            .map(|n| n.cum_work)
            .or_else(|| self.chain.canon_entry(height).filter(|c| c.hash == h).map(|c| c.cum_work));
        json!({
            "hash": hex::encode(h), "header": hex::encode(b.header.encode()), "header_fields": header_fields(&b.header),
            "cumulative_work": cw.map(u256_hex), "canonical": self.chain.is_canonical(&h, height),
            "confirmations": self.chain.confirmations(&h, height), "final": self.chain.is_final(&h, height),
            "proof_len": b.proof.len(),
        })
    }

    /// Handle one JSON-RPC call. `now_ms` is the node clock.
    pub async fn dispatch(&mut self, method: &str, p: &Value, now_ms: u64) -> R {
        let p = if p.is_null() { &json!({}) } else { p };
        match method {
            "getChainInfo" => {
                let tip = self.chain.tip();
                let f = self.chain.finalized();
                Ok(json!({
                    "chain_id": self.chain.chain_id(), "genesis_hash": hex::encode(self.chain.genesis_hash()),
                    "height": tip.header.height, "tip": hex::encode(tip.hash), "cumulative_work": u256_hex(tip.cum_work),
                    "finalized_height": f.header.height, "finalized_hash": hex::encode(f.hash),
                    "target_bits": tip.header.target_bits, "next_target_bits": self.chain.required_bits(&tip.hash),
                    "timestamp_ms": tip.header.timestamp_ms,
                    "mempool_size": self.mempool.len(), "peers": self.peers, "finality_k": tip.config.finality_k,
                    "syncing": self.syncing, "live_blocks": self.chain.live_count(),
                    "blocks_accepted": self.blocks_accepted, "txs_accepted": self.txs_accepted,
                }))
            }
            "getParams" => Ok(config_json(&self.chain.tip().config)),
            "getBlockTemplate" => {
                let miner = p_key(p, &["miner", "payout"])?;
                let choice = match p_str(p, &["job_id"]) {
                    None => JobChoice::Auto,
                    Some("fallback") => JobChoice::Fallback,
                    Some(s) => JobChoice::Job(parse32(s)?),
                };
                let (id, t) = srv(self.template(miner, choice, now_ms).await)?;
                let f = &t.fallback;
                Ok(json!({
                    "template_id": id, "header": hex::encode(t.header.encode()), "header_fields": header_fields(&t.header),
                    "height": t.header.height, "target_bits": t.header.target_bits, "chain_id": self.chain.chain_id(),
                    "job": t.job.as_ref().map(|j| job_json(j, t.header.height)),
                    "fallback": {"m": f.m, "n": f.n, "config": hex::encode(f.config.to_bytes())},
                    "tx_count": t.txs.len(), "fees": t.summary.fees.to_string(), "subsidy": t.summary.subsidy.to_string(),
                    "min_fee": t.config.min_fee.to_string(),
                    "coinbase": hex::encode(t.summary.coinbase.encode()), "mature_height": t.summary.coinbase.mature_height,
                    "weak_target_bits": self.chain.weak_bits_for(&t.header.prev_hash), "weak_shift": self.chain.weak_shift_for(&t.header.prev_hash),
                }))
            }
            "submitBlock" => {
                let block = if p.get("block").is_some() || p.get("block_hex").is_some() {
                    srv(Block::decode(&p_bytes(p, "block")?))?
                } else {
                    let hb = hex::decode(p_str(p, &["header"]).ok_or_else(|| RpcError::params("missing header"))?.trim_start_matches("0x"))
                        .map_err(|e| RpcError::params(e.to_string()))?;
                    let header = srv(Header::decode(&hb))?;
                    let proof = p_bytes(p, "proof")?;
                    let Some((txs, coinbase, contracts)) = self.template_txs(&header, p_str(p, &["template_id"])) else {
                        return Ok(json!({"accepted": false, "reason": "no template with this parent and tx_root"}));
                    };
                    Block { header, proof, txs, coinbase: Some(coinbase), contracts }
                };
                let r = self.add_block(block, now_ms, true).await;
                tracing::debug!(block = %hex::encode(&r.hash[..6]), height = r.height, submitted_ms = now_ms, accepted = r.accepted, "submitted block");
                let mut v = json!({"accepted": r.accepted, "block_hash": hex::encode(r.hash), "height": r.height, "tip": r.tip});
                if let Some(reason) = r.reason {
                    v["reason"] = json!(reason);
                }
                Ok(v)
            }
            "sendTx" => {
                let tx = srv(Tx::decode(&p_bytes(p, "tx")?))?;
                let h = self.add_tx(tx, true).await.map_err(RpcError::server)?;
                Ok(json!({"tx_hash": hex::encode(h)}))
            }
            "getTx" => {
                let h = p_key(p, &["tx_hash", "hash"])?;
                if let Some(t) = self.mempool.get(&h) {
                    let mut v = tx_summary(t);
                    self.annotate_pending(&mut v, &h, now_ms);
                    return Ok(v);
                }
                let Some((bh, idx)) = self.chain.tx_location(&h) else {
                    // listed by a weak block on the tip but not pooled here (docs/SPEED.md 2.4)
                    return Ok(if self.preconf_of(&h, now_ms).is_some() { self.tx_status(&h, now_ms).await } else { Value::Null });
                };
                let Some(b) = srv(self.chain.get_block(&bh).await)? else { return Ok(Value::Null) };
                let t = &b.txs[idx as usize];
                let mut v = tx_summary(t);
                let fin = self.chain.is_final(&bh, b.header.height);
                v["status"] = json!(if fin { "final" } else { "included" });
                v["final"] = json!(fin);
                v["block_hash"] = json!(hex::encode(bh));
                v["height"] = json!(b.header.height);
                v["index"] = json!(idx);
                v["confirmations"] = json!(self.chain.confirmations(&bh, b.header.height));
                if t.body.kind().is_contract() {
                    if let Some(Ok(r)) = self.contract_rpc("getContractReceipt", &json!({"tx_hash": hex::encode(h)})).await {
                        v["receipt"] = r;
                    }
                }
                Ok(v)
            }
            "getAccount" => {
                let a = p_key(p, &["account", "address"])?;
                let acc = srv(self.account(&a).await)?;
                let route = self.route(&a).await?;
                Ok(json!({"account": b58(&a), "balance": acc.balance.to_string(), "nonce": acc.nonce, "route": route}))
            }
            "getRoute" => {
                let a = p_key(p, &["account", "address"])?;
                self.route(&a).await
            }
            "getAgent" => {
                let a = p_key(p, &["account", "address"])?;
                match self.tip_record(&schema::agent_key(&a)).await? {
                    None => Ok(Value::Null),
                    Some(b) => {
                        let g = srv(Agent::decode(&b))?;
                        Ok(json!({"account": b58(&a), "name": String::from_utf8_lossy(&g.name), "uri": String::from_utf8_lossy(&g.uri),
                                  "registered_height": g.registered_height, "updated_height": g.updated_height}))
                    }
                }
            }
            "getJob" => {
                let id = p_key(p, &["job_id", "id"])?;
                let next = self.chain.tip_height() + 1;
                let Some(j) = self.job(&id).await? else { return Ok(Value::Null) };
                let mut v = job_json(&j, next);
                v["reject_count"] = json!(self.reject_count(&id).await?);
                Ok(v)
            }
            "getProfile" => {
                let a = p_key(p, &["account", "address"])?;
                self.get_profile(&a).await
            }
            "getHandle" => {
                let h = p_str(p, &["handle"]).ok_or_else(|| RpcError::params("missing handle"))?;
                self.get_handle(h).await
            }
            "listProfiles" => {
                let offset = p_u64(p, "offset").unwrap_or(0);
                let limit = p_u64(p, "limit").unwrap_or(50).min(200);
                self.list_profiles(offset, limit).await
            }
            "getPost" => {
                let id = p_key(p, &["post_id", "id"])?;
                self.get_post(&id).await
            }
            "getFollowCounts" => {
                let a = p_key(p, &["account", "address"])?;
                let s = self.social_stats(&a).await?;
                Ok(json!({"account": b58(&a), "followers": s.followers, "following": s.following}))
            }
            "isFollowing" => {
                let f = p_key(p, &["follower"])?;
                let t = p_key(p, &["target"])?;
                self.is_following(&f, &t).await
            }
            "getJobRejects" => {
                let id = p_key(p, &["job_id", "id"])?;
                self.job_rejects(&id).await
            }
            "getRejectStats" => {
                let a = p_key(p, &["account", "address", "deliverer"])?;
                self.reject_stats(&a).await
            }
            "listJobs" => {
                let status = p_str(p, &["status"]).map(|s| s.to_ascii_lowercase());
                let offset = p_u64(p, "offset").unwrap_or(0) as usize;
                let limit = p_u64(p, "limit").unwrap_or(50).min(200) as usize;
                let c = self.counters().await?;
                let next = self.chain.tip_height() + 1;
                let mut all = Vec::new();
                for i in c.job_count.saturating_sub(10_000)..c.job_count {
                    let Some(b) = self.tip_record(&schema::job_index_key(i)).await? else { continue };
                    let id = srv(schema::decode_job_index(&b))?;
                    if let Some(j) = self.job(&id).await? {
                        let keep = match status.as_deref() {
                            None => true,
                            Some("mineable") => executor::check_mineable(&j, next).is_ok(),
                            Some(s) => j.status.name() == s,
                        };
                        if keep {
                            all.push(j);
                        }
                    }
                }
                let total = all.len();
                let jobs: Vec<Value> = all.iter().skip(offset).take(limit).map(|j| job_json(j, next)).collect();
                Ok(json!({"total": total, "jobs": jobs}))
            }
            "getModel" => {
                let id = p_key(p, &["model_id", "id"])?;
                Ok(match self.tip_record(&schema::model_key(&id)).await? {
                    Some(b) => {
                        let m = srv(schema::Model::decode(&b))?;
                        let mut v = model_json(&m);
                        self.add_manifest_state(&mut v, &m);
                        v
                    }
                    None => Value::Null,
                })
            }
            "listModels" => {
                let models = srv(self.models().await)?;
                let list: Vec<Value> = models
                    .iter()
                    .map(|m| {
                        let mut v = model_json(m);
                        v.as_object_mut().unwrap().remove("matrices");
                        self.add_manifest_state(&mut v, m);
                        v
                    })
                    .collect();
                Ok(json!({"total": list.len(), "models": list}))
            }
            "getTicket" => {
                let id = p_key(p, &["ticket_id", "job_id", "id"])?;
                let next = self.chain.tip_height() + 1;
                let Some(t) = self.job(&id).await? else { return Ok(Value::Null) };
                let Some(te) = t.ticket().copied() else { return Err(RpcError::params("not an inference ticket")) };
                let count = match self.tip_record(&schema::model_key(&te.model_id)).await? {
                    Some(b) => srv(schema::Model::decode(&b))?.matrices.len(),
                    None => 0,
                };
                let mut segs = Vec::new();
                // cap the state reads one call makes on the node actor
                let mut truncated = false;
                for (round, i) in (0..te.rounds).flat_map(|r| (0..count).map(move |i| (r, i))) {
                    if segs.len() >= MAX_TICKET_SEGMENTS {
                        truncated = true;
                        break;
                    }
                    let sid = pearl_job::standing::segment_id(&id, i as u16, round);
                    if let Some(sj) = self.job(&sid).await? {
                        segs.push(json!({"segment_id": hex::encode(sid), "round": round, "matrix": i, "m": sj.m, "k": sj.k, "n": sj.n,
                            "status": sj.status.name(), "mineable": executor::check_mineable(&sj, next).is_ok(),
                            "max_blocks": sj.max_blocks, "blocks_mined": sj.blocks_mined,
                            "cap": (sj.cap_remaining + sj.subsidy_paid).to_string(), "subsidy_paid": sj.subsidy_paid.to_string(),
                            "data_uri": String::from_utf8_lossy(&sj.data_uri), "c_hash": hex::encode(sj.result_hash)}));
                    }
                }
                let mut v = job_json(&t, next);
                v["segment_list"] = Value::Array(segs);
                v["segment_list_truncated"] = json!(truncated);
                Ok(v)
            }
            "getBlock" => {
                let Some(b) = self.block_by(p).await? else { return Ok(Value::Null) };
                let mut v = self.block_meta(&b);
                v["txs"] = Value::Array(
                    b.txs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let mut s = tx_summary(t);
                            if b.coinbase.is_some() {
                                s["path"] = json!(hex::encode(b.leaf_path(i)));
                            }
                            s
                        })
                        .collect(),
                );
                v["coinbase"] = coinbase_json(&b);
                v["contracts"] = contracts_json(&b);
                match srv(self.chain.block_rewards(&b.hash()).await)? {
                    Some(r) => {
                        v["fees"] = json!(r.fees.to_string());
                        v["subsidy"] = json!(r.subsidy.to_string());
                        v["reward"] = json!((r.fees as u128 + r.subsidy as u128).to_string());
                        v["matured"] = json!(r.matured.to_string());
                    }
                    None => {
                        let fees: u128 = b.txs.iter().map(|t| t.fee as u128).sum();
                        v["fees"] = json!(fees.to_string());
                        v["subsidy"] = Value::Null;
                        v["reward"] = Value::Null;
                        v["matured"] = Value::Null;
                    }
                }
                if p.get("raw").and_then(|r| r.as_bool()) == Some(true) {
                    v["block"] = json!(b64(&b.encode()));
                }
                Ok(v)
            }
            "getHeader" => {
                let Some(b) = self.block_by(p).await? else { return Ok(Value::Null) };
                let mut v = self.block_meta(&b);
                v["proof"] = json!(b64(&b.proof));
                Ok(v)
            }
            "getEscrow" => {
                let esc = srv(self.account(&agent_types::escrow_address()).await)?.balance;
                let pool = srv(self.account(&agent_types::pool_address()).await)?.balance;
                let outside = TOTAL_SUPPLY - esc;
                Ok(json!({
                    "escrow": b58(&agent_types::escrow_address()), "escrow_balance": esc.to_string(),
                    "pool": b58(&agent_types::pool_address()), "pool_balance": pool.to_string(),
                    "total_supply": TOTAL_SUPPLY.to_string(), "outside_escrow": outside.to_string(),
                    "outside_escrow_spl": (outside / SPL_SCALE).to_string(), "height": self.chain.tip_height(),
                }))
            }
            "getBridgeEvents" => {
                let from = p_u64(p, "from_nonce").unwrap_or(0);
                let limit = p_u64(p, "limit").unwrap_or(100).min(1000);
                let c = self.counters().await?;
                let mut out = Vec::new();
                for n in from..c.withdrawal_count.min(from.saturating_add(limit)) {
                    if let Some(b) = self.tip_record(&schema::withdrawal_key(n)).await? {
                        out.push(withdrawal_json(&self.chain, &srv(Withdrawal::decode(&b))?));
                    }
                }
                Ok(json!({"withdrawal_count": c.withdrawal_count, "deposit_count": c.deposit_count, "withdrawals": out}))
            }
            "getDeposit" => {
                let n = p_u64(p, "solana_nonce").ok_or_else(|| RpcError::params("missing solana_nonce"))?;
                match self.tip_record(&schema::deposit_key(n)).await? {
                    None => Ok(json!({"processed": false})),
                    Some(b) => {
                        let d = srv(Deposit::decode(&b))?;
                        let bh = self.chain.canonical_hash(d.height);
                        let conf = bh.map(|h| self.chain.confirmations(&h, d.height)).unwrap_or(0);
                        Ok(json!({"processed": true, "height": d.height, "recipient": b58(&d.recipient),
                                  "amount_spl": d.amount_spl.to_string(), "confirmations": conf,
                                  "final": d.height <= self.chain.finalized().header.height}))
                    }
                }
            }
            "getWithdrawalProof" => {
                let n = p_u64(p, "nonce").ok_or_else(|| RpcError::params("missing nonce"))?;
                let Some(b) = self.tip_record(&schema::withdrawal_key(n)).await? else { return Ok(Value::Null) };
                let w = srv(Withdrawal::decode(&b))?;
                let wj = withdrawal_json(&self.chain, &w);
                if let Some(at) = p_u64(p, "at_height") {
                    // Historical proof against the withdraw_root of the final header at `at`.
                    if at < w.height {
                        return Err(RpcError::params(format!("at_height {at} is below the withdrawal's block {}", w.height)));
                    }
                    if at > self.chain.finalized().header.height {
                        return Err(RpcError::params(format!("at_height {at} is not final (final height {})", self.chain.finalized().header.height)));
                    }
                    return match srv(self.chain.withdrawal_proof_at(n, at).await)? {
                        None => Ok(json!({"withdrawal": wj, "final": false, "proof": null})),
                        Some((entry, loc, proof, root, hash)) => Ok(json!({"withdrawal": wj, "final": true, "entry": hex::encode(entry),
                            "location": loc, "proof": b64(&proof), "withdraw_root": hex::encode(root),
                            "root_height": at, "root_block_hash": hex::encode(hash)})),
                    };
                }
                match srv(self.chain.withdrawal_proof(n).await)? {
                    None => Ok(json!({"withdrawal": wj, "final": false, "proof": null})),
                    Some((entry, loc, proof, root)) => {
                        let f = self.chain.finalized();
                        Ok(json!({"withdrawal": wj, "final": true, "entry": hex::encode(entry), "location": loc,
                                  "proof": b64(&proof), "withdraw_root": hex::encode(root),
                                  "root_height": f.header.height, "root_block_hash": hex::encode(f.hash)}))
                    }
                }
            }
            "putBlob" => {
                let data = p_bytes(p, "data")?;
                let push = p.get("push").and_then(|v| v.as_bool()).unwrap_or(true);
                let (id, new) = self.put_blob_opt(&data, now_ms, push).await.map_err(RpcError::server)?;
                Ok(json!({"blob_id": hex::encode(id), "uri": blobs::blob_uri(&id), "size": data.len(), "new": new}))
            }
            "getBlob" | "hasBlob" => {
                let s = p_str(p, &["blob_id", "id", "uri"]).ok_or_else(|| RpcError::params("missing blob_id"))?;
                let id = blobs::parse_id(s).ok_or_else(|| RpcError::params("blob_id must be 64 hex or blob:<64 hex>"))?;
                if self.blobs.is_none() {
                    return Err(RpcError::server("this node has no blob store"));
                }
                let meta = self.blobs.as_ref().unwrap().has(&id);
                if method == "hasBlob" {
                    return Ok(json!({"blob_id": hex::encode(id), "present": meta.is_some(),
                                     "size": meta.map(|m| m.size), "fetching": self.blob_wanted.contains_key(&id)}));
                }
                match self.serve_blob(&id).await {
                    Some(d) => Ok(json!({"blob_id": hex::encode(id), "uri": blobs::blob_uri(&id), "size": d.len(), "data": b64(&d)})),
                    None => {
                        // not here: fetch it from peers if a job on the chain references it
                        let refs = srv(self.blob_refs_cached().await)?;
                        if refs.contains_key(&id) || self.model_pinned().await.contains(&id) {
                            self.want_blob(id);
                        }
                        Ok(Value::Null)
                    }
                }
            }
            "getBlobStats" => {
                let Some(b) = self.blobs.as_ref() else { return Err(RpcError::server("this node has no blob store")) };
                let c = b.config();
                Ok(json!({"count": b.len(), "total_bytes": b.total_bytes(), "fetching": self.blob_wanted.len(), "fetch_in_flight": self.blob_fetch.in_flight(),
                          "max_blob_bytes": c.max_blob_bytes, "max_total_bytes": c.max_total_bytes, "grace_blocks": c.grace_blocks,
                          "put_rate_bytes_per_s": c.put_rate_bytes_per_s, "last_prune_height": self.last_blob_prune}))
            }
            "getAccountTxs" => {
                let a = p_key(p, &["account", "address"])?;
                let limit = p_u64(p, "limit").unwrap_or(50).clamp(1, 200) as usize;
                let before = match p_str(p, &["before", "cursor"]) {
                    None | Some("") => None,
                    Some(c) => {
                        let (h, i) = c.split_once(':').ok_or_else(|| RpcError::params("cursor is height:index"))?;
                        Some((h.parse().map_err(|_| RpcError::params("cursor height"))?, i.parse().map_err(|_| RpcError::params("cursor index"))?))
                    }
                };
                let (entries, more) = self.chain.account_txs(&a, before, limit);
                let mut out = Vec::with_capacity(entries.len());
                let mut cache: Option<Block> = None;
                for e in &entries {
                    if cache.as_ref().map(|b| b.hash()) != Some(e.block_hash) {
                        cache = srv(self.chain.get_block(&e.block_hash).await)?;
                    }
                    let Some(b) = &cache else { continue };
                    let Some(t) = b.txs.get(e.index as usize) else { continue };
                    let mut v = tx_summary(t);
                    v["block_hash"] = json!(hex::encode(e.block_hash));
                    v["height"] = json!(e.height);
                    v["index"] = json!(e.index);
                    let lien = matches!(&t.body, agent_types::TxBody::ClearRoute { account, .. } if *account == a);
                    let rejected = matches!(&t.body, agent_types::TxBody::RejectResult { deliverer, .. } if *deliverer == a);
                    let followed = matches!(&t.body, agent_types::TxBody::Follow { items } if t.from != a && items.iter().any(|x| x.target == a));
                    let linked = matches!(&t.body, agent_types::TxBody::OperatorLink { agent, .. } if *agent == a);
                    let messaged = matches!(&t.body, agent_types::TxBody::SendMessage(m) if t.from != a && m.to == a);
                    let membered = matches!(&t.body, agent_types::TxBody::ChannelMember { member, .. } if t.from != a && *member == a);
                    v["role"] = json!(if lien {
                        "lien"
                    } else if rejected {
                        "rejected"
                    } else if followed {
                        "follow"
                    } else if linked {
                        "operator"
                    } else if messaged {
                        "message"
                    } else if membered {
                        "member"
                    } else if t.from == a {
                        "sender"
                    } else {
                        "recipient"
                    });
                    v["confirmations"] = json!(self.chain.confirmations(&e.block_hash, e.height));
                    v["final"] = json!(self.chain.is_final(&e.block_hash, e.height));
                    out.push(v);
                }
                let next = if more { entries.last().map(|e| format!("{}:{}", e.height, e.index)) } else { None };
                Ok(json!({"account": b58(&a), "txs": out, "next": next, "tip_height": self.chain.tip_height(),
                          "finalized_height": self.chain.finalized().header.height}))
            }
            "getPeers" => match &self.peer_info {
                Some(f) => Ok(f()),
                None => Ok(json!({"mode": "static", "count": self.static_peers.len(),
                                  "peers": self.static_peers.iter().map(|k| json!({"key": k})).collect::<Vec<_>>()})),
            },
            "getMempool" => Ok(json!({"size": self.mempool.len(), "txs": self.mempool.hashes().iter().map(hex::encode).collect::<Vec<_>>()})),
            "subscribe" => Err(RpcError::server("subscribe is only available over the WebSocket endpoint")),
            m => match self.contract_rpc(m, p).await {
                Some(r) => r,
                None => match self.dm_dispatch(m, p).await {
                    Some(r) => r,
                    None => match self.preconf_dispatch(m, p, now_ms).await {
                        Some(r) => r,
                        None => Err(RpcError::method(m)),
                    },
                },
            },
        }
    }
}

/// For tests and tools: the tx root of a list.
pub fn tx_root(txs: &[Tx]) -> Hash {
    block::tx_root(txs)
}
