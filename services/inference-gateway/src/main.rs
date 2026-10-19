//! cadre-gateway HTTP server (SPEC 5.4). 127.0.0.1:10664 by default.
//!
//!   GET  /health, GET /v1/models
//!   POST /v1/completions {prompt | prompt_ids, max_tokens, recipient?}   (x402: X-PAYMENT header)
//!   GET  /v1/requests/<ticket_id>

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::json;

use cadre_gateway::{Config, Gateway, HttpEvaluator, HttpStorefront, HttpWorker, ModelInfo};
use pearl_node::{read_key_file, HttpNode};

#[derive(Parser)]
#[command(name = "cadre-gateway")]
struct Args {
    /// Server key (the tickets' server: posts segments, delivers, receives W).
    #[arg(long)]
    key: PathBuf,
    /// Registered model id (hex); or --model-dir with a registered manifest.json.
    #[arg(long)]
    model_id: Option<String>,
    #[arg(long)]
    model_dir: Option<PathBuf>,
    #[arg(long, default_value = pearl_node::DEFAULT_RPC)]
    rpc: String,
    #[arg(long, default_value = "http://127.0.0.1:10662")]
    storefront: String,
    #[arg(long, default_value = "http://127.0.0.1:10665")]
    worker: String,
    #[arg(long, default_value = "http://127.0.0.1:10661")]
    attester: String,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 10664)]
    port: u16,
    #[arg(long, default_value = ".cadre-gateway")]
    work_dir: PathBuf,
    /// Ticket deadline, blocks after the unlock (the segments are mineable until then).
    #[arg(long, default_value_t = 400)]
    deadline_blocks: u64,
    #[arg(long, default_value_t = 1)]
    blocks_per_segment: u32,
    #[arg(long, default_value_t = 64)]
    max_new_tokens: u32,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let node = HttpNode::new(&a.rpc);
    let model_id = match (&a.model_id, &a.model_dir) {
        (Some(id), _) => id.clone(),
        (None, Some(d)) => pearl_job::standing::Manifest::load(d)?.model_id,
        _ => return Err(anyhow!("pass --model-id or --model-dir")),
    };
    let model = ModelInfo::from_json(&node.call("getModel", json!({"model_id": model_id}))?)?;
    let cfg = Config {
        work_dir: a.work_dir.clone(),
        deadline_blocks: a.deadline_blocks,
        blocks_per_segment: a.blocks_per_segment,
        max_new_tokens: a.max_new_tokens,
        ..Config::default()
    };
    std::fs::create_dir_all(&cfg.work_dir)?;
    let gw = Arc::new(Gateway::new(
        Box::new(HttpNode::new(&a.rpc)),
        Box::new(HttpStorefront { url: a.storefront.clone() }),
        Box::new(HttpWorker { url: a.worker.clone() }),
        Box::new(HttpEvaluator { url: a.attester.clone() }),
        read_key_file(&a.key)?,
        model,
        cfg,
        Box::new(|v| println!("{v}")),
    ));
    let addr = format!("{}:{}", a.host, a.port);
    let server = tiny_http::Server::http(&addr).map_err(|e| anyhow!("bind {addr}: {e} (is the port in use? lsof -ti :{})", a.port))?;
    println!("{}", json!({"msg": "cadre-gateway started", "url": format!("http://{addr}"), "server": pearl_node::b58(&gw.server()),
        "model_id": hex::encode(gw.model.model_id), "matrices": gw.model.matrices.len(), "pid": std::process::id()}));
    for mut req in server.incoming_requests() {
        let gw = Arc::clone(&gw);
        std::thread::spawn(move || {
            let mut body = Vec::new();
            let _ = req.as_reader().take(1 << 20).read_to_end(&mut body);
            let method = req.method().as_str().to_uppercase();
            let path = req.url().split('?').next().unwrap_or("").to_string();
            let headers: Vec<(String, String)> = req.headers().iter().map(|h| (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())).collect();
            let r = gw.handle(&method, &path, &headers, &body);
            let mut resp = tiny_http::Response::from_string(r.body.to_string())
                .with_status_code(r.status)
                .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
            for (k, v) in r.headers {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    resp = resp.with_header(h);
                }
            }
            let _ = req.respond(resp);
        });
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
