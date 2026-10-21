//! Contract RPC (docs/RPC.md "Contracts", docs/CONTRACTS.md): getContract, getCode,
//! listContracts, listCodes, queryContract, simulateCall, getContractReceipt,
//! getContractEvents, getContractStorage, getContractStateProof.

use agent_types::contracts::{Event, Receipt};
use agent_types::Pubkey;
use commonware_runtime::Spawner;
use consensus_pow::chain::TipRead;
use consensus_pow::PowVerifier;
use serde_json::{json, Value};
use state::contracts::{self as sc, CodeInfo, Contract, ContractCounters, StoredReceipt};
use state::View;

use crate::core::Core;
use crate::dispatch::{b58, p_key, p_str, p_u64, parse32, RpcError};

/// Gas a query gets when the caller names none (and the most it may ask for).
pub const QUERY_GAS_DEFAULT: u64 = 20_000_000;
pub const QUERY_GAS_MAX: u64 = 50_000_000;

fn srv<T, E: std::fmt::Display>(r: Result<T, E>) -> Result<T, RpcError> {
    r.map_err(RpcError::server)
}

fn hex_param(p: &Value, keys: &[&str]) -> Result<Vec<u8>, RpcError> {
    match p_str(p, keys) {
        None => Ok(Vec::new()),
        Some(s) => hex::decode(s.trim().trim_start_matches("0x")).map_err(|e| RpcError::params(format!("{}: {e}", keys[0]))),
    }
}

fn opt_key(k: &[u8; 32]) -> Value {
    if *k == [0; 32] { Value::Null } else { json!(b58(k)) }
}

pub fn contract_json(c: &Contract) -> Value {
    json!({
        "address": b58(&c.address), "code_hash": hex::encode(c.code_hash), "admin": opt_key(&c.admin),
        "upgradable": c.admin != [0; 32],
        "creator": b58(&c.creator), "label": String::from_utf8_lossy(&c.label), "created_height": c.created_height,
        "migrated_height": if c.migrated_height == 0 { Value::Null } else { json!(c.migrated_height) },
        "storage_bytes": c.storage_bytes, "storage_items": c.storage_items, "deposit": c.deposit.to_string(),
        "index": c.index, "calls": c.calls, "events": c.events,
    })
}

pub fn code_json(c: &CodeInfo) -> Value {
    json!({
        "code_hash": hex::encode(c.code_hash), "deployer": b58(&c.deployer), "height": c.height, "size": c.size,
        "deposit": c.deposit.to_string(), "instances": c.instances, "index": c.index,
    })
}

pub fn event_json(e: &Event) -> Value {
    json!({
        "contract": b58(&e.contract), "tx_hash": hex::encode(e.tx_hash), "index": e.index,
        "topics": e.topics.iter().map(hex::encode).collect::<Vec<_>>(), "data": hex::encode(&e.data),
        "bytes": hex::encode(e.encode()), "hash": hex::encode(e.hash()),
    })
}

pub fn receipt_json(r: &Receipt) -> Value {
    json!({
        "tx_hash": hex::encode(r.tx_hash), "kind": r.kind, "status": r.status.name(), "ok": r.status == agent_types::contracts::Status::Ok,
        "gas_limit": r.gas_limit.to_string(), "gas_used": r.gas_used.to_string(), "fee_charged": r.fee_charged.to_string(),
        "contract": opt_key(&r.contract), "code_hash": hex::encode(r.code_hash), "event_count": r.event_count,
        "return": hex::encode(&r.ret), "error": String::from_utf8_lossy(&r.error),
        "bytes": hex::encode(r.encode()), "hash": hex::encode(r.hash()),
    })
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    async fn crec(&self, k: &state::Key) -> Result<Option<Vec<u8>>, RpcError> {
        srv(self.chain.read_tip(k).await)
    }

    async fn contract_at(&self, a: &Pubkey) -> Result<Option<Contract>, RpcError> {
        match self.crec(&sc::contract_key(a)).await? {
            Some(b) => Ok(Some(srv(Contract::decode(&b))?)),
            None => Ok(None),
        }
    }

    async fn ccounters(&self) -> Result<ContractCounters, RpcError> {
        Ok(match self.crec(&sc::counters_key()).await? {
            Some(b) => srv(ContractCounters::decode(&b))?,
            None => ContractCounters::default(),
        })
    }

    /// Contract methods; `None` = not a contract method.
    pub(crate) async fn contract_rpc(&mut self, method: &str, p: &Value) -> Option<Result<Value, RpcError>> {
        let r = match method {
            "getContract" => self.get_contract(p).await,
            "getCode" => self.get_code(p).await,
            "listContracts" => self.list_index(p, false).await,
            "listCodes" => self.list_index(p, true).await,
            "queryContract" => self.query(p).await,
            "simulateCall" => self.simulate(p).await,
            "getContractReceipt" => self.get_receipt(p).await,
            "getContractEvents" => self.get_events(p).await,
            "getContractStorage" => self.get_storage(p).await,
            "getContractStateProof" => self.state_proof(p).await,
            _ => return None,
        };
        Some(r)
    }

    async fn get_contract(&self, p: &Value) -> Result<Value, RpcError> {
        let a = p_key(p, &["address", "contract"])?;
        let Some(c) = self.contract_at(&a).await? else { return Ok(Value::Null) };
        let mut v = contract_json(&c);
        v["balance"] = json!(srv(self.account(&a).await)?.balance.to_string());
        if let Some(b) = self.crec(&sc::code_key(&c.code_hash)).await? {
            let ci = srv(CodeInfo::decode(&b))?;
            v["code_size"] = json!(ci.size);
        }
        if let Some(b) = self.crec(&sc::code_bytes_key(&c.code_hash)).await? {
            if let Ok(code) = sc::decode_code_bytes(&b) {
                if let Ok((_, info)) = executor_vm().validate(code, usize::MAX) {
                    v["entries"] = json!(info.entries);
                    v["imports"] = json!(info.imports);
                }
            }
        }
        Ok(v)
    }

    async fn get_code(&self, p: &Value) -> Result<Value, RpcError> {
        let h = p_key(p, &["code_hash", "hash"])?;
        let Some(b) = self.crec(&sc::code_key(&h)).await? else { return Ok(Value::Null) };
        let mut v = code_json(&srv(CodeInfo::decode(&b))?);
        if p.get("include_code").and_then(|x| x.as_bool()).unwrap_or(false) {
            if let Some(b) = self.crec(&sc::code_bytes_key(&h)).await? {
                v["code"] = json!(hex::encode(srv(sc::decode_code_bytes(&b))?));
            }
        }
        Ok(v)
    }

    async fn list_index(&self, p: &Value, codes: bool) -> Result<Value, RpcError> {
        let c = self.ccounters().await?;
        let total = if codes { c.codes } else { c.contracts };
        let offset = p_u64(p, "offset").unwrap_or(0);
        let limit = p_u64(p, "limit").unwrap_or(50).min(200);
        // newest first
        let mut out = Vec::new();
        let mut i = total.saturating_sub(offset);
        while i > 0 && (out.len() as u64) < limit {
            i -= 1;
            let (k, t) = if codes { (sc::code_index_key(i), sc::tag::CODE_INDEX) } else { (sc::contract_index_key(i), sc::tag::CONTRACT_INDEX) };
            let Some(b) = self.crec(&k).await? else { continue };
            let id = srv(sc::decode_index(t, &b))?;
            if codes {
                if let Some(b) = self.crec(&sc::code_key(&id)).await? {
                    out.push(code_json(&srv(CodeInfo::decode(&b))?));
                }
            } else if let Some(c) = self.contract_at(&id).await? {
                out.push(contract_json(&c));
            }
        }
        Ok(json!({"total": total, "offset": offset, "items": out}))
    }

    fn tip_read(&self) -> TipRead<'_, E> {
        TipRead { view: View { store: self.chain.store(), snap: &self.chain.tip().snapshot } }
    }

    async fn query(&self, p: &Value) -> Result<Value, RpcError> {
        let a = p_key(p, &["address", "contract"])?;
        let args = hex_param(p, &["args", "input"])?;
        let gas = p_u64(p, "gas").unwrap_or(QUERY_GAS_DEFAULT).min(QUERY_GAS_MAX);
        let height = self.chain.tip_height() + 1;
        let base = self.tip_read();
        let r = executor::query_contract(&base, self.chain.chain_id(), height, &a, args, gas).await.map_err(RpcError::params)?;
        Ok(json!({"ok": r.ok, "return": hex::encode(&r.ret), "error": String::from_utf8_lossy(&r.error),
                  "gas_used": r.gas_used.to_string(), "height": height - 1}))
    }

    async fn simulate(&self, p: &Value) -> Result<Value, RpcError> {
        let from = p_key(p, &["from"])?;
        let a = p_key(p, &["address", "contract"])?;
        let args = hex_param(p, &["args", "input"])?;
        let value = p_u64(p, "value").unwrap_or(0);
        let tip_cfg = self.chain.tip().config.clone();
        let gas = p_u64(p, "gas").unwrap_or(tip_cfg.tx_gas_limit).min(tip_cfg.tx_gas_limit).min(QUERY_GAS_MAX);
        let height = self.chain.tip_height() + 1;
        let base = self.tip_read();
        let r = executor::simulate_call(&base, self.chain.chain_id(), height, &from, &a, args, value, gas).await.map_err(RpcError::params)?;
        let fee = executor::contract_fee_floor(&tip_cfg, 160 + p_str(p, &["args", "input"]).map(|s| s.len() / 2).unwrap_or(0), r.gas_used);
        Ok(json!({"ok": r.ok, "return": hex::encode(&r.ret), "error": String::from_utf8_lossy(&r.error),
                  "gas_used": r.gas_used.to_string(), "log": String::from_utf8_lossy(&r.log),
                  "events": r.events.iter().map(event_json).collect::<Vec<_>>(),
                  "fee_estimate": fee.to_string(), "height": height - 1}))
    }

    async fn get_receipt(&self, p: &Value) -> Result<Value, RpcError> {
        let h = p_key(p, &["tx_hash", "hash"])?;
        let Some(b) = self.crec(&sc::receipt_key(&h)).await? else { return Ok(Value::Null) };
        let s = srv(StoredReceipt::decode(&b))?;
        let mut v = receipt_json(&s.receipt);
        v["height"] = json!(s.height);
        v["events"] = Value::Array(s.events.iter().map(event_json).collect());
        if let Some(bh) = self.chain.canonical_hash(s.height) {
            v["block_hash"] = json!(hex::encode(bh));
            v["final"] = json!(self.chain.is_final(&bh, s.height));
        }
        Ok(v)
    }

    async fn get_events(&self, p: &Value) -> Result<Value, RpcError> {
        let a = p_key(p, &["address", "contract"])?;
        let Some(c) = self.contract_at(&a).await? else { return Ok(Value::Null) };
        let offset = p_u64(p, "offset").unwrap_or(0);
        let limit = p_u64(p, "limit").unwrap_or(50).min(200);
        let topic = match p_str(p, &["topic"]) {
            Some(t) => Some(parse32(t)?),
            None => None,
        };
        let mut out = Vec::new();
        let mut n = c.events.saturating_sub(offset);
        let mut scanned = 0u64;
        while n > 0 && (out.len() as u64) < limit && scanned < 2000 {
            n -= 1;
            scanned += 1;
            let Some(b) = self.crec(&sc::event_index_key(&a, n)).await? else { continue };
            let (height, txh, idx) = srv(sc::decode_event_ref(&b))?;
            let Some(rb) = self.crec(&sc::receipt_key(&txh)).await? else { continue };
            let r = srv(StoredReceipt::decode(&rb))?;
            let Some(e) = r.events.iter().find(|e| e.index == idx) else { continue };
            if topic.is_some_and(|t| e.topics.first() != Some(&t)) {
                continue;
            }
            let mut v = event_json(e);
            v["height"] = json!(height);
            v["n"] = json!(n);
            out.push(v);
        }
        Ok(json!({"address": b58(&a), "total": c.events, "events": out, "next_offset": c.events - n}))
    }

    async fn get_storage(&self, p: &Value) -> Result<Value, RpcError> {
        let a = p_key(p, &["address", "contract"])?;
        let key = hex_param(p, &["key"])?;
        let Some(b) = self.crec(&sc::store_key(&a, &key)).await? else { return Ok(json!({"value": Value::Null})) };
        let (_, v) = srv(sc::decode_store(&b))?;
        Ok(json!({"value": hex::encode(v)}))
    }

    /// Committed (final) state proof of a contract storage item against the finalized
    /// header's `state_root` (QMDB current-value proof; docs/CONTRACTS.md "Proofs").
    async fn state_proof(&self, p: &Value) -> Result<Value, RpcError> {
        let a = p_key(p, &["address", "contract"])?;
        let key = hex_param(p, &["key"])?;
        let k = sc::store_key(&a, &key);
        let store = self.chain.store();
        let snap = store.committed();
        let value = srv(store.get(snap, &k).await)?;
        let proof = match &value {
            Some(_) => Some(srv(store.state_proof(&k).await)?),
            None => None,
        };
        let fin = self.chain.finalized();
        Ok(json!({
            "state_key": hex::encode(k), "value": value.as_ref().map(hex::encode), "proof": proof.map(hex::encode),
            "state_root": hex::encode(snap.state_root), "height": fin.header.height, "block_hash": hex::encode(fin.hash),
            "header_state_root": hex::encode(fin.header.state_root),
        }))
    }
}

fn executor_vm() -> &'static ::contracts::Vm {
    executor::vm()
}
