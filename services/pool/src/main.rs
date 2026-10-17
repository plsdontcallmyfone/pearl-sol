//! pearl-pool: mining pool server (src/lib.rs has the rules, src/client.rs the routes).
//! One JSON log line per event on stdout.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

use pearl_node::{read_key_file, HttpNode, DEFAULT_RPC};
use pearl_pool::{Pool, PoolConfig};

fn parse_bits(s: &str) -> Result<u32, String> {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).map_err(|e| e.to_string()),
        None => s.parse().map_err(|e: std::num::ParseIntError| e.to_string()),
    }
}

#[derive(Parser)]
#[command(name = "pearl-pool", about = "Agent-chain mining pool: shares at a lower target, PPLNS payouts after maturity")]
struct Args {
    /// Node JSON-RPC endpoint.
    #[arg(long, default_value = DEFAULT_RPC)]
    rpc: String,
    /// Pool key file: the header's miner (payout) key and the signer of payout Transfers.
    #[arg(long)]
    key: PathBuf,
    #[arg(long, default_value = "127.0.0.1:10663")]
    listen: String,
    #[arg(long, default_value = ".state/pool.json")]
    state: PathBuf,
    /// Pool fee, basis points of each block reward.
    #[arg(long, default_value_t = 100)]
    fee_bps: u32,
    /// PPLNS window (shares).
    #[arg(long, default_value_t = 1000)]
    pplns_n: usize,
    /// Blocks until a reward is paid out (default: node finality_k + 1).
    #[arg(long)]
    maturity: Option<u64>,
    /// Fixed share target, compact (e.g. 0x1e040000). Default: block target x --share-factor.
    #[arg(long, value_parser = parse_bits)]
    share_bits: Option<u32>,
    /// Share target = block target x this (see pearl_pool::DEFAULT_SHARE_FACTOR).
    #[arg(long, default_value_t = pearl_pool::DEFAULT_SHARE_FACTOR)]
    share_factor: u64,
    /// Smallest payout amount (base units, after the tx fee).
    #[arg(long, default_value_t = 1_000_000)]
    min_payout: u64,
    /// Payout tx fee (default: the template's min_fee).
    #[arg(long)]
    payout_fee: Option<u64>,
    #[arg(long, default_value_t = 32)]
    max_batch: usize,
    #[arg(long, default_value_t = 40)]
    nonce_range_bits: u32,
    #[arg(long)]
    chain_id: Option<u32>,
    #[arg(long, default_value_t = 5000)]
    template_refresh_ms: u64,
    #[arg(long, default_value_t = 20000)]
    resend_ms: u64,
    /// Background tick (tip, maturity, payouts).
    #[arg(long, default_value_t = 500)]
    poll_ms: u64,
    #[arg(long, default_value_t = 4)]
    http_threads: usize,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let key = read_key_file(&a.key)?;
    let cfg = PoolConfig {
        fee_bps: a.fee_bps,
        pplns_n: a.pplns_n,
        maturity: a.maturity,
        share_bits: a.share_bits,
        share_factor: a.share_factor,
        min_payout: a.min_payout,
        payout_fee: a.payout_fee,
        max_batch: a.max_batch,
        nonce_range_bits: a.nonce_range_bits,
        chain_id: a.chain_id,
        template_refresh: Duration::from_millis(a.template_refresh_ms),
        resend_after: Duration::from_millis(a.resend_ms),
        state_path: a.state,
    };
    let node = Arc::new(HttpNode::new(&a.rpc));
    let pool = Arc::new(Pool::open(node, key, cfg, Box::new(|v| println!("{v}"))).context("open pool")?);
    println!("{}", serde_json::json!({"event": "listen", "addr": a.listen, "rpc": a.rpc, "pid": std::process::id()}));
    let ticker = pool.clone();
    let poll = Duration::from_millis(a.poll_ms.max(50));
    std::thread::spawn(move || loop {
        if let Err(e) = ticker.tick() {
            println!("{}", serde_json::json!({"event": "tick_failed", "error": format!("{e:#}")}));
        }
        std::thread::sleep(poll);
    });
    pearl_pool::http::serve(pool, &a.listen, a.http_threads)
}
