//! HTTP JSON server for the pool (routes in src/client.rs). tiny_http, a few worker
//! threads sharing one [`Pool`]; share checks run outside the pool lock.

use std::io::Read;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};

use crate::{decode_proof, parse_payout, Pool};

/// Answer one request: `(status, body)`.
pub fn route(pool: &Pool, method: &str, url: &str, body: &[u8]) -> (u16, Value) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let q = |k: &str| query.split('&').filter_map(|kv| kv.split_once('=')).find(|(a, _)| *a == k).map(|(_, b)| b.to_string());
    let req: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(body).unwrap_or(Value::Null) };
    let payout = || -> Result<[u8; 32]> {
        let s = req.get("payout").and_then(|p| p.as_str()).map(String::from).or_else(|| q("payout")).ok_or_else(|| anyhow::anyhow!("payout missing"))?;
        parse_payout(&s)
    };
    let r: Result<Value> = (|| match (method, path) {
        ("GET", "/health") => Ok(json!("ok")),
        ("GET", "/stats") => Ok(pool.stats()),
        ("GET", "/miner") => Ok(pool.miner_json(&payout()?)),
        ("POST", "/register") => Ok(serde_json::to_value(pool.register(&payout()?)?)?),
        ("POST", "/work") | ("GET", "/work") => {
            let fb = req.get("fallback").and_then(|f| f.as_bool()).unwrap_or(false) || q("fallback").as_deref() == Some("true");
            pool.work(&payout()?, fb)
        }
        ("POST", "/share") => {
            let work_id = req.get("work_id").and_then(|w| w.as_str()).ok_or_else(|| anyhow::anyhow!("work_id missing"))?;
            let nonce = pearl_node::num(req.get("nonce").ok_or_else(|| anyhow::anyhow!("nonce missing"))?)?;
            let proof = decode_proof(&req)?;
            Ok(pool.submit_share(&payout()?, work_id, nonce, &proof)?.to_json())
        }
        _ => Err(anyhow::anyhow!("not found: {method} {path}")),
    })();
    match r {
        Ok(v) => (200, v),
        Err(e) => {
            let msg = format!("{e:#}");
            let code = if msg.starts_with("not found") { 404 } else { 400 };
            (code, json!({"error": msg}))
        }
    }
}

/// Serve until the process exits. Binds `addr` (e.g. 127.0.0.1:10663).
pub fn serve(pool: Arc<Pool>, addr: &str, threads: usize) -> Result<()> {
    let server = Arc::new(tiny_http::Server::http(addr).map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?);
    let mut hs = Vec::new();
    for _ in 0..threads.max(1) {
        let (server, pool) = (server.clone(), pool.clone());
        hs.push(std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = Vec::new();
                let _ = req.as_reader().take(4 << 20).read_to_end(&mut body);
                let method = req.method().as_str().to_uppercase();
                let (code, v) = route(&pool, &method, req.url(), &body);
                let resp = tiny_http::Response::from_string(v.to_string())
                    .with_status_code(code)
                    .with_header("content-type: application/json".parse::<tiny_http::Header>().unwrap());
                let _ = req.respond(resp);
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    Ok(())
}
