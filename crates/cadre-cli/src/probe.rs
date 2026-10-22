//! Small HTTP helpers for `status`, `ping` and the read commands: a GET or a JSON-RPC
//! call with a timeout, the HTTP status and the latency. Nothing here writes anywhere.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub const TIMEOUT: Duration = Duration::from_secs(12);

/// One HTTP answer.
#[derive(Debug, Clone)]
pub struct Hit {
    pub url: String,
    /// HTTP status (0 = no answer).
    pub status: u16,
    pub ms: u64,
    /// The body as JSON (Null when it is not JSON).
    pub json: Value,
    /// Body bytes read (for non-JSON pages).
    pub bytes: usize,
    /// Transport error, if no answer.
    pub error: Option<String>,
}

impl Hit {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// One short reason for a failed hit.
    pub fn why(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        let msg = self.json.pointer("/error/message").or_else(|| self.json.get("error")).and_then(|v| v.as_str()).unwrap_or("");
        if msg.is_empty() {
            format!("HTTP {}", self.status)
        } else {
            format!("HTTP {} {}", self.status, msg)
        }
    }
}

fn agent(t: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(t).user_agent(concat!("cadre-cli/", env!("CARGO_PKG_VERSION"))).build()
}

fn short_err(e: &ureq::Error) -> String {
    let s = e.to_string();
    // ureq repeats the URL; keep the cause
    match s.rsplit_once(": ") {
        Some((_, c)) if !c.is_empty() => c.to_string(),
        _ => s,
    }
}

fn finish(url: &str, t0: Instant, r: std::result::Result<ureq::Response, ureq::Error>) -> Hit {
    let ms = t0.elapsed().as_millis() as u64;
    let (status, resp) = match r {
        Ok(r) => (r.status(), Some(r)),
        Err(ureq::Error::Status(c, r)) => (c, Some(r)),
        Err(e) => return Hit { url: url.into(), status: 0, ms, json: Value::Null, bytes: 0, error: Some(short_err(&e)) },
    };
    let mut body = String::new();
    if let Some(r) = resp {
        use std::io::Read;
        let _ = r.into_reader().take(16 << 20).read_to_string(&mut body);
    }
    let json = serde_json::from_str(&body).unwrap_or(Value::Null);
    Hit { url: url.into(), status, ms: t0.elapsed().as_millis().max(ms as u128) as u64, json, bytes: body.len(), error: None }
}

pub fn get(url: &str) -> Hit {
    get_t(url, TIMEOUT)
}

pub fn get_t(url: &str, t: Duration) -> Hit {
    let t0 = Instant::now();
    finish(url, t0, agent(t).get(url).call())
}

/// JSON-RPC 2.0 POST; the hit's `json` is the whole envelope.
pub fn rpc_hit(url: &str, method: &str, params: Value) -> Hit {
    let t0 = Instant::now();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    finish(url, t0, agent(TIMEOUT).post(url).send_json(body))
}

/// JSON-RPC result (or the node's error message).
pub fn rpc(url: &str, method: &str, params: Value) -> Result<(Value, u64)> {
    let h = rpc_hit(url, method, params);
    if let Some(e) = &h.error {
        return Err(anyhow!("{method}: {e}"));
    }
    if let Some(e) = h.json.get("error").filter(|e| !e.is_null()) {
        return Err(anyhow!("{method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(&e.to_string())));
    }
    if !h.ok() {
        return Err(anyhow!("{method}: HTTP {}", h.status));
    }
    Ok((h.json.get("result").cloned().unwrap_or(Value::Null), h.ms))
}

/// GET returning JSON on a 2xx, else an error with the reason.
pub fn get_json(url: &str) -> Result<Value> {
    let h = get(url);
    if h.ok() {
        Ok(h.json)
    } else {
        Err(anyhow!("{url}: {}", h.why()))
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// "3m 20s ago" style age.
pub fn age(ms_ago: u64) -> String {
    let s = ms_ago / 1000;
    if s < 90 {
        format!("{s}s ago")
    } else if s < 90 * 60 {
        format!("{}m ago", s / 60)
    } else if s < 48 * 3600 {
        format!("{}h {}m ago", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d ago", s / 86400)
    }
}
