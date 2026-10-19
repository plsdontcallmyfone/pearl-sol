//! Transports: storefront-x402, the model worker, the evaluator. Traits so tests run the
//! gateway in process.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Value};

use pearl_node::b58;

/// storefront-x402 `POST /jobs`: `(status, body, x-payment-response header)`.
pub trait Storefront: Send + Sync {
    fn post_job(&self, body: &Value, x_payment: Option<&str>) -> Result<(u16, Value, Option<String>)>;
}

/// Generation options the worker takes.
#[derive(Debug, Clone, Default)]
pub struct GenOpts {
    pub stop: Vec<String>,
    /// false: A/C keep exactly `rows` rows (requests are stacked into rounds by the gateway).
    pub pad: bool,
}

/// The model worker (tools/cadre/worker.py).
pub trait Worker: Send + Sync {
    fn tokenize(&self, text: &str) -> Result<Vec<u32>>;
    /// Prompt ids for chat messages (the model's chat template).
    fn chat_ids(&self, messages: &Value, tools: Option<&Value>) -> Result<Vec<u32>>;
    /// Runs the request; writes `out_dir/a/<i>.i8`, `out_dir/c/<i>.i32`. `on_text` gets every
    /// new piece of text as it is generated (streaming); the result is the worker's summary.
    fn generate(&self, prompt_ids: &[u32], max_new: u32, out_dir: &Path, opts: &GenOpts, on_text: &mut dyn FnMut(&str)) -> Result<Value>;
}

/// The evaluator (services/attester `POST /result`): `(attester pubkey, signature)`.
pub trait Evaluator: Send + Sync {
    fn attest(&self, ticket: &[u8; 32], deliverer: &[u8; 32]) -> Result<([u8; 32], [u8; 64])>;
}

fn agent(timeout_s: u64) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout_s)).build()
}

pub struct HttpStorefront {
    pub url: String,
}

impl Storefront for HttpStorefront {
    fn post_job(&self, body: &Value, x_payment: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        let mut req = agent(180).post(&format!("{}/jobs", self.url.trim_end_matches('/'))).set("content-type", "application/json");
        if let Some(p) = x_payment {
            req = req.set("x-payment", p);
        }
        let r = match req.send_string(&body.to_string()) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => bail!("storefront-x402: {e}"),
        };
        let status = r.status();
        let xpr = r.header("x-payment-response").map(String::from);
        let v: Value = r.into_json().unwrap_or(Value::Null);
        Ok((status, v, xpr))
    }
}

pub struct HttpWorker {
    pub url: String,
}

impl Worker for HttpWorker {
    fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let v: Value = agent(60).post(&format!("{}/tokenize", self.url)).send_json(json!({"text": text}))?.into_json()?;
        ids_of(&v)
    }

    fn chat_ids(&self, messages: &Value, tools: Option<&Value>) -> Result<Vec<u32>> {
        let mut b = json!({"messages": messages});
        if let Some(t) = tools {
            b["tools"] = t.clone();
        }
        let v: Value = match agent(60).post(&format!("{}/tokenize", self.url)).send_json(b) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(_, r)) => bail!("worker /tokenize: {}", r.into_string().unwrap_or_default()),
            Err(e) => bail!("worker /tokenize: {e}"),
        };
        ids_of(&v)
    }

    fn generate(&self, prompt_ids: &[u32], max_new: u32, out_dir: &Path, opts: &GenOpts, on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        let body = json!({"prompt_ids": prompt_ids, "max_new_tokens": max_new, "out_dir": out_dir.display().to_string(),
            "stop": opts.stop, "pad": opts.pad, "stream": true});
        let r = match agent(3600).post(&format!("{}/generate", self.url)).send_json(body) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => bail!("worker /generate: {}", r.into_string().unwrap_or_default()),
            Err(e) => bail!("worker /generate: {e}"),
        };
        for line in BufReader::new(r.into_reader()).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&line)?;
            if v["done"] == json!(true) {
                return Ok(v);
            }
            if let Some(t) = v["text"].as_str() {
                if !t.is_empty() {
                    on_text(t);
                }
            }
        }
        bail!("worker stream ended without a result")
    }
}

fn ids_of(v: &Value) -> Result<Vec<u32>> {
    v["ids"].as_array().ok_or_else(|| anyhow!("worker /tokenize: {v}"))?.iter().map(|x| x.as_u64().map(|y| y as u32).ok_or_else(|| anyhow!("bad id"))).collect()
}

pub struct HttpEvaluator {
    pub url: String,
}

impl Evaluator for HttpEvaluator {
    fn attest(&self, ticket: &[u8; 32], deliverer: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        let body = json!({"job": hex::encode(ticket), "deliverer": b58(deliverer)});
        let v: Value = match agent(1800).post(&format!("{}/result", self.url)).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(_, r)) => r.into_json()?,
            Err(e) => bail!("attester: {e}"),
        };
        ensure!(v["ok"] == json!(true), "attester refused: {}", v["error"]);
        let pk = pearl_job::spec::parse_key32(v["attester"].as_str().unwrap_or(""))?;
        let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap_or(""))?.try_into().map_err(|_| anyhow!("signature is not 64 bytes"))?;
        Ok((pk, sig))
    }
}
