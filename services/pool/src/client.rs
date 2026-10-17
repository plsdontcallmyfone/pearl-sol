//! Miner side of the pool protocol (HTTP JSON on :10663, see src/http.rs):
//!
//! | route | body | result |
//! |---|---|---|
//! | POST /register | `{"payout": base58}` | MinerRec (id, nonce_start, nonce_end, ...) |
//! | POST /work | `{"payout": base58, "fallback"?: bool}` | work (below) |
//! | POST /share | `{"payout", "work_id", "nonce", "proof": base64}` | `{"accepted", "reason"?, "stale"?, "block"?}` |
//! | GET /stats, GET /miner?payout=, GET /health | | |
//!
//! Work = a getBlockTemplate-shaped object (`header` hex with the POOL key as miner,
//! `job`, `fallback`, `template_id`) plus `work_id`, `share_bits`, `nonce_start`,
//! `nonce_end`. Mine nonces in your range; a proof that meets `share_bits` is a share.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use pearl_node::{b58, num, Template};

use crate::{Pool, ShareOutcome};

#[derive(Debug, Clone)]
pub struct PoolWork {
    pub work_id: String,
    pub template: Template,
    pub share_bits: u32,
    pub nonce_start: u64,
    pub nonce_end: u64,
}

impl PoolWork {
    pub fn from_json(v: &Value) -> Result<Self> {
        let s = |k: &str| v.get(k).filter(|x| !x.is_null()).ok_or_else(|| anyhow!("work has no {k}"));
        Ok(Self {
            work_id: s("work_id")?.as_str().ok_or_else(|| anyhow!("work_id not a string"))?.to_string(),
            template: Template::from_json(v)?,
            share_bits: num(s("share_bits")?)? as u32,
            nonce_start: num(s("nonce_start")?)?,
            nonce_end: num(s("nonce_end")?)?,
        })
    }
}

pub fn share_from_json(v: &Value) -> Result<ShareOutcome> {
    if v["accepted"].as_bool() == Some(true) {
        let block = match v.get("block").filter(|b| !b.is_null()) {
            Some(b) => Some(crate::BlockResult {
                accepted: b["accepted"].as_bool().unwrap_or(false),
                hash: b["hash"].as_str().unwrap_or("").to_string(),
                height: b["height"].as_u64().unwrap_or(0),
                reason: b["reason"].as_str().map(String::from),
            }),
            None => None,
        };
        Ok(ShareOutcome::Accepted { block })
    } else if v.get("accepted").is_some() {
        Ok(ShareOutcome::Rejected { reason: v["reason"].as_str().unwrap_or("").to_string(), stale: v["stale"].as_bool().unwrap_or(false) })
    } else {
        bail!("bad share reply: {v}")
    }
}

/// What a pool-mode miner needs from a pool.
pub trait PoolApi: Send + Sync {
    fn register(&self, payout: &[u8; 32]) -> Result<Value>;
    fn work(&self, payout: &[u8; 32], fallback: bool) -> Result<PoolWork>;
    fn submit_share(&self, payout: &[u8; 32], work_id: &str, nonce: u64, proof: &[u8]) -> Result<ShareOutcome>;
}

/// In process (tests): same JSON shapes as the HTTP server.
impl PoolApi for Pool {
    fn register(&self, payout: &[u8; 32]) -> Result<Value> {
        Ok(serde_json::to_value(Pool::register(self, payout)?)?)
    }
    fn work(&self, payout: &[u8; 32], fallback: bool) -> Result<PoolWork> {
        PoolWork::from_json(&Pool::work(self, payout, fallback)?)
    }
    fn submit_share(&self, payout: &[u8; 32], work_id: &str, nonce: u64, proof: &[u8]) -> Result<ShareOutcome> {
        Pool::submit_share(self, payout, work_id, nonce, proof)
    }
}

pub struct HttpPool {
    pub url: String,
    agent: ureq::Agent,
}

impl HttpPool {
    pub fn new(url: &str) -> Self {
        Self { url: url.trim_end_matches('/').to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build() }
    }

    fn post(&self, path: &str, body: Value) -> Result<Value> {
        let v: Value = match self.agent.post(&format!("{}{path}", self.url)).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = r.into_json().unwrap_or(json!({}));
                bail!("pool {path}: HTTP {code}: {}", v["error"].as_str().unwrap_or(""))
            }
            Err(e) => bail!("pool {path}: {e}"),
        };
        Ok(v)
    }
}

impl PoolApi for HttpPool {
    fn register(&self, payout: &[u8; 32]) -> Result<Value> {
        self.post("/register", json!({"payout": b58(payout)}))
    }
    fn work(&self, payout: &[u8; 32], fallback: bool) -> Result<PoolWork> {
        PoolWork::from_json(&self.post("/work", json!({"payout": b58(payout), "fallback": fallback}))?)
    }
    fn submit_share(&self, payout: &[u8; 32], work_id: &str, nonce: u64, proof: &[u8]) -> Result<ShareOutcome> {
        let body = json!({"payout": b58(payout), "work_id": work_id, "nonce": nonce,
                          "proof": base64::engine::general_purpose::STANDARD.encode(proof)});
        share_from_json(&self.post("/share", body)?)
    }
}
