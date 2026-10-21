//! cadre-gateway HTTP server (SPEC 5.4, API v1). 127.0.0.1:10664 by default.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::json;

use cadre_gateway::{server, Config, Gateway, HttpEvaluator, HttpStorefront, HttpWorker, ModelInfo};
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
    /// Deadline of a one-request ticket, blocks after the unlock.
    #[arg(long, default_value_t = 400)]
    deadline_blocks: u64,
    /// Deadline of a prepaid balance ticket.
    #[arg(long, default_value_t = 10_000)]
    balance_deadline_blocks: u64,
    #[arg(long, default_value_t = 4096)]
    balance_rows: u64,
    #[arg(long, default_value_t = 1)]
    blocks_per_segment: u32,
    #[arg(long, default_value_t = 256)]
    max_new_tokens: u32,
    /// Post a round when the oldest served request is this old.
    #[arg(long, default_value_t = 60)]
    round_secs: u64,
    /// Round period of balances above 4096 rows (large house tickets, M1.5); a top-up's own
    /// `round_secs` overrides both.
    #[arg(long, default_value_t = 10_800)]
    big_round_secs: u64,
    /// Largest balance a top-up may ask for (also bounded by the chain's max_balance_rows).
    #[arg(long, default_value_t = 4096)]
    max_balance_rows: u64,
    /// Most rows one round stacks (0 = what one 4 MiB A blob holds at the model's widest k).
    #[arg(long, default_value_t = 0)]
    max_round_rows: u32,
    #[arg(long, default_value_t = 64)]
    min_free_rows: u64,
    #[arg(long, default_value_t = 50)]
    close_margin_blocks: u64,
    /// Largest max_blocks a balance ticket asks for: keep it at or below the storefront's
    /// on-chain max_blocks_cap (10,000 at initialize). A ticket's rounds are this / (matrices x
    /// blocks per segment), cut further by its deadline and round period (SPEC 5.4).
    #[arg(long, default_value_t = 10_000)]
    max_ticket_blocks: u32,
    #[arg(long, default_value = "http://127.0.0.1:10670")]
    explorer_url: String,
    #[arg(long)]
    public_url: Option<String>,
    /// Public URL of this gateway's /v1 root behind a proxy that mounts it under another path,
    /// e.g. https://api.example/v1/openai (cadre-api). Status links use it instead of --public-url.
    #[arg(long)]
    public_api_root: Option<String>,
    /// Requests handled at once; more answer 503 (security pass V-7).
    #[arg(long, default_value_t = 64)]
    max_in_flight: usize,
    /// Socket read timeout per connection, seconds (headers, body, idle keep-alive).
    #[arg(long, default_value_t = 30)]
    read_timeout_secs: u64,
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
        balance_deadline_blocks: a.balance_deadline_blocks,
        balance_rows: a.balance_rows,
        blocks_per_segment: a.blocks_per_segment,
        max_new_tokens: a.max_new_tokens,
        round_every: Duration::from_secs(a.round_secs),
        big_round_every: Duration::from_secs(a.big_round_secs),
        max_balance_rows: a.max_balance_rows,
        max_round_rows: a.max_round_rows,
        min_free_rows: a.min_free_rows,
        close_margin_blocks: a.close_margin_blocks,
        max_ticket_blocks: a.max_ticket_blocks,
        explorer_url: a.explorer_url.clone(),
        public_url: a.public_url.clone().unwrap_or(format!("http://{}:{}", a.host, a.port)),
        public_api_root: a.public_api_root.clone(),
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
    let limits = server::Limits { max_in_flight: a.max_in_flight.max(1), read_timeout: Duration::from_secs(a.read_timeout_secs.max(1)), ..server::Limits::default() };
    let srv = server::bind(&addr, &limits).map_err(|e| anyhow!("bind {addr}: {e} (is the port in use? lsof -ti :{})", a.port))?;
    println!("{}", json!({"msg": "cadre-gateway started", "url": format!("http://{addr}"), "server": pearl_node::b58(&gw.server()),
        "model_id": hex::encode(gw.model.model_id), "model": gw.model.name, "matrices": gw.model.matrices.len(), "pid": std::process::id(),
        "max_in_flight": limits.max_in_flight, "read_timeout_secs": limits.read_timeout.as_secs()}));
    let g2 = Arc::clone(&gw);
    std::thread::spawn(move || g2.run_rounds());
    let g3 = Arc::clone(&gw);
    server::serve(srv, limits, Arc::new(move |m: &str, p: &str, h: &[(String, String)], b: &[u8]| g3.handle(m, p, h, b)));
    // only reached if the listener stopped: exit non-zero so systemd restarts the gateway
    Err(anyhow!("the HTTP listener on {addr} stopped"))
}
