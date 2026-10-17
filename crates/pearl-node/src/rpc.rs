//! JSON-RPC 2.0 over HTTP POST to the node (`--rpc`, default http://127.0.0.1:10630).
//!
//! Methods and params used (keep in sync with docs/RPC.md):
//!
//! | method | params | result |
//! |---|---|---|
//! | getBlockTemplate | `{"miner": <base58>}` | template object (`Template::from_json`) |
//! | submitBlock | `{"header": <hex256>, "proof": <base64>, "template_id"?}` | `{"accepted", "block_hash"?, "reason"?}` or a block hash string |
//! | getJob | `{"job_id": <hex>}` | job object (`JobInfo::from_json`) or null |
//! | getAccount | `{"account": <base58>}` | `{"balance", "nonce"}` (null = empty account) |
//! | sendTx | `{"tx": <base64>}` | `{"tx_hash"}` or a hash string |

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use pearl_job::pow::MinedBlock;

use crate::{b58, key32, num, Account, JobInfo, NodeApi, SubmitOutcome, Template};

pub struct HttpNode {
    pub url: String,
    agent: ureq::Agent,
    id: AtomicU64,
}

impl HttpNode {
    pub fn new(url: &str) -> Self {
        Self { url: url.to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build(), id: AtomicU64::new(1) }
    }

    /// One JSON-RPC call; returns `result` or the node's error as an Err.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_raw(method, params)?.map_err(|m| anyhow!("{method}: {m}"))
    }

    /// Outer Err: transport failure. Inner Err: the node's JSON-RPC error message.
    pub fn call_raw(&self, method: &str, params: Value) -> Result<std::result::Result<Value, String>> {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let v: Value = match self.agent.post(&self.url).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => r.into_json().unwrap_or_else(|_| json!({"error": {"message": format!("HTTP {code}")}})),
            Err(e) => bail!("{method}: {e}"),
        };
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            let msg = e.get("message").and_then(|m| m.as_str()).map(String::from).unwrap_or_else(|| e.to_string());
            return Ok(Err(msg));
        }
        Ok(Ok(v.get("result").cloned().unwrap_or(Value::Null)))
    }
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// submitBlock params for a mined block.
pub fn submit_params(t: &Template, b: &MinedBlock) -> Value {
    let mut p = json!({"header": hex::encode(b.header.to_bytes()), "proof": b64(&b.proof)});
    if let Some(id) = &t.template_id {
        p["template_id"] = json!(id);
    }
    p
}

/// Interpret a submitBlock result.
pub fn parse_submit(v: &Value, b: &MinedBlock) -> SubmitOutcome {
    match v {
        Value::String(s) => match crate::key32(&json!(s)) {
            Ok(h) => SubmitOutcome::Accepted { block_hash: h },
            Err(_) => SubmitOutcome::Rejected { reason: s.clone() },
        },
        Value::Bool(true) | Value::Null => SubmitOutcome::Accepted { block_hash: b.header.block_hash() },
        Value::Object(o) => {
            let ok = o.get("accepted").and_then(|a| a.as_bool()).unwrap_or(true);
            if ok {
                let h = o.get("block_hash").and_then(|h| key32(h).ok()).unwrap_or_else(|| b.header.block_hash());
                SubmitOutcome::Accepted { block_hash: h }
            } else {
                SubmitOutcome::Rejected { reason: o.get("reason").map(|r| r.as_str().map(String::from).unwrap_or(r.to_string())).unwrap_or_default() }
            }
        }
        other => SubmitOutcome::Rejected { reason: other.to_string() },
    }
}

impl NodeApi for HttpNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<Template> {
        let v = self.call("getBlockTemplate", json!({"miner": b58(payout)}))?;
        let mut t = Template::from_json(&v)?;
        if t.job.is_none() && !t.header.is_fallback() {
            t.job = self.get_job(&t.header.job_id)?;
            if t.job.is_none() {
                bail!("template names job {} but getJob has none", hex::encode(t.header.job_id));
            }
        }
        Ok(t)
    }

    fn submit_block(&self, t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        // A JSON-RPC error on submit is a rejection, not a transport failure.
        Ok(match self.call_raw("submitBlock", submit_params(t, b))? {
            Ok(v) => parse_submit(&v, b),
            Err(reason) => SubmitOutcome::Rejected { reason },
        })
    }

    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>> {
        let v = self.call("getJob", json!({"job_id": hex::encode(job_id)}))?;
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(JobInfo::from_json(&v)?))
    }

    fn get_account(&self, account: &[u8; 32]) -> Result<Account> {
        let v = self.call("getAccount", json!({"account": b58(account)}))?;
        if v.is_null() {
            return Ok(Account::default());
        }
        let g = |k: &str| v.get(k).filter(|x| !x.is_null()).map(num).transpose().map(|x| x.unwrap_or(0));
        Ok(Account { balance: g("balance")?, nonce: g("nonce")? })
    }

    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        let v = self.call("sendTx", json!({"tx": b64(wire)}))?;
        Ok(match &v {
            Value::String(s) => s.clone(),
            Value::Object(o) => o
                .get("tx_hash")
                .or_else(|| o.get("hash"))
                .and_then(|h| h.as_str())
                .map(String::from)
                .ok_or_else(|| anyhow!("sendTx result has no tx_hash: {v}"))?,
            _ => v.to_string(),
        })
    }
}
