//! Minimal Solana JSON-RPC client (blocking, ureq).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::key::Pubkey;

pub struct Rpc {
    pub url: String,
    agent: ureq::Agent,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub lamports: u64,
    pub owner: Pubkey,
    pub data: Vec<u8>,
}

fn b64d(s: &str) -> Result<Vec<u8>> {
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}

fn parse_account(v: &Value) -> Result<Account> {
    let data = v["data"][0].as_str().ok_or_else(|| anyhow!("account data missing"))?;
    Ok(Account {
        lamports: v["lamports"].as_u64().unwrap_or(0),
        owner: Pubkey::from_b58(v["owner"].as_str().unwrap_or("11111111111111111111111111111111"))?,
        data: b64d(data)?,
    })
}

impl Rpc {
    pub fn new(url: &str) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
        Self { url: url.to_string(), agent }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let resp: Value = self
            .agent
            .post(&self.url)
            .send_json(body)
            .map_err(|e| anyhow!("rpc {method} to {}: {e}", self.url))?
            .into_json()
            .context("rpc response json")?;
        if let Some(err) = resp.get("error") {
            bail!("rpc {method} error: {err}");
        }
        Ok(resp["result"].clone())
    }

    pub fn get_slot(&self) -> Result<u64> {
        self.call("getSlot", json!([{"commitment": "confirmed"}]))?.as_u64().ok_or_else(|| anyhow!("bad slot"))
    }

    pub fn get_account(&self, pk: &Pubkey) -> Result<Option<Account>> {
        let r = self.call("getAccountInfo", json!([pk.to_b58(), {"encoding": "base64", "commitment": "confirmed"}]))?;
        if r["value"].is_null() {
            return Ok(None);
        }
        Ok(Some(parse_account(&r["value"])?))
    }

    /// `memcmp` filters as (offset, bytes).
    pub fn get_program_accounts(&self, program: &Pubkey, memcmp: &[(usize, Vec<u8>)]) -> Result<Vec<(Pubkey, Account)>> {
        let filters: Vec<Value> = memcmp
            .iter()
            .map(|(o, b)| json!({"memcmp": {"offset": o, "bytes": bs58::encode(b).into_string()}}))
            .collect();
        let r = self.call(
            "getProgramAccounts",
            json!([program.to_b58(), {"encoding": "base64", "commitment": "confirmed", "filters": filters}]),
        )?;
        let arr = r.as_array().ok_or_else(|| anyhow!("getProgramAccounts: not an array"))?;
        arr.iter().map(|e| Ok((Pubkey::from_b58(e["pubkey"].as_str().unwrap_or(""))?, parse_account(&e["account"])?))).collect()
    }

    pub fn latest_blockhash(&self) -> Result<[u8; 32]> {
        let r = self.call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))?;
        let s = r["value"]["blockhash"].as_str().ok_or_else(|| anyhow!("no blockhash"))?;
        Ok(Pubkey::from_b58(s)?.0)
    }

    /// Send with preflight, then wait for `confirmed`. Returns the signature.
    pub fn send_and_confirm(&self, tx: &[u8]) -> Result<String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(tx);
        let sig = self
            .call("sendTransaction", json!([b64, {"encoding": "base64", "preflightCommitment": "confirmed"}]))?
            .as_str()
            .ok_or_else(|| anyhow!("sendTransaction: no signature"))?
            .to_string();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            let r = self.call("getSignatureStatuses", json!([[sig]]))?;
            let st = &r["value"][0];
            if !st.is_null() {
                if !st["err"].is_null() {
                    bail!("transaction {sig} failed: {}", st["err"]);
                }
                let c = st["confirmationStatus"].as_str().unwrap_or("");
                if c == "confirmed" || c == "finalized" {
                    return Ok(sig);
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        bail!("transaction {sig} not confirmed in 30 s")
    }
}
