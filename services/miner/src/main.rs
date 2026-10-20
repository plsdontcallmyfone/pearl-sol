//! pearl-miner v2 (CPU block producer). The loop is in src/lib.rs; the proof format
//! is crates/pearl-job/PROOF-FORMAT.md section 7. One JSON log line per event on stdout.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use pearl_miner::pool::{PoolMiner, PoolMinerConfig};
use pearl_miner::{HttpAttester, Miner, MinerConfig, ResultAttester};
use pearl_pool::client::HttpPool;
use pearl_node::{read_key_file, FailoverNode, DEFAULT_RPC};

#[derive(Parser)]
#[command(name = "pearl-miner", about = "v2 agent-chain block producer (useful PoW on posted jobs, CPU)")]
struct Args {
    /// Node JSON-RPC endpoint (pool mode: used only to fetch `blob:` job data). Repeat it, or
    /// give a comma-separated list, for several nodes of the same chain: the miner stays on one
    /// and moves to the next when it cannot be reached (an `rpc_failover` log line).
    #[arg(long, default_value = DEFAULT_RPC, value_delimiter = ',', action = clap::ArgAction::Append)]
    rpc: Vec<String>,
    /// Payout key file (Solana JSON array, or base58 / hex of the 64-byte keypair or 32-byte seed).
    /// Also signs SubmitResult / CompleteJob with --deliver.
    #[arg(long, default_value = "keys/localnet/miner-keypair.json")]
    payout: PathBuf,
    #[arg(long, default_value_t = MinerConfig::default().threads)]
    threads: usize,
    /// Re-fetch the block template at least this often (ms).
    #[arg(long, default_value_t = 2000)]
    refresh_ms: u64,
    /// Stats line interval (s).
    #[arg(long, default_value_t = 10)]
    stats_secs: u64,
    /// For jobs this miner mined a block on: SubmitResult, attester signature, CompleteJob.
    #[arg(long)]
    deliver: bool,
    #[arg(long, default_value = "http://127.0.0.1:10661")]
    attester: String,
    #[arg(long, default_value = "results")]
    results_dir: PathBuf,
    /// Deliver C as a local file:// URI instead of a blob: put to the node.
    #[arg(long)]
    result_file: bool,
    /// Tx fee (raised to the template's min_fee).
    #[arg(long, default_value_t = 0)]
    fee: u64,
    /// Overrides the template's chain_id.
    #[arg(long)]
    chain_id: Option<u32>,
    /// Exit after this many accepted blocks (and their deliveries).
    #[arg(long)]
    blocks: Option<u64>,
    /// Pool mode: get work from a pearl-pool (e.g. http://127.0.0.1:10663) and submit
    /// shares there; --payout is the key the pool pays. Without it: solo mode.
    #[arg(long)]
    pool: Option<String>,
    /// Model store for standing jobs (SPEC 4.5): <dir>/<model_id hex>/w/<i>.i8; matrices
    /// not there are rebuilt from the node's blobs.
    #[arg(long, default_value = "models")]
    models: PathBuf,
    /// Prepared jobs kept in memory.
    #[arg(long, default_value_t = 8)]
    max_prepared: usize,
    /// Pool mode: exit after this many accepted shares.
    #[arg(long)]
    shares: Option<u64>,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let key = read_key_file(&a.payout)?;
    if let Some(url) = &a.pool {
        let pool = HttpPool::new(url);
        let cfg = PoolMinerConfig {
            threads: a.threads.max(1),
            refresh: Duration::from_millis(a.refresh_ms.max(50)),
            stats_every: Duration::from_secs(a.stats_secs.max(1)),
            max_shares: a.shares,
        };
        let data_node = nodes(&a.rpc)?;
        let mut m = PoolMiner::new(&pool, key.verifying_key().to_bytes(), cfg, Box::new(|v| println!("{v}")));
        m.data_node = Some(&data_node);
        return m.run(&|| false);
    }
    let node = nodes(&a.rpc)?;
    let attester = HttpAttester::new(&a.attester);
    let cfg = MinerConfig {
        threads: a.threads.max(1),
        refresh: Duration::from_millis(a.refresh_ms.max(50)),
        stats_every: Duration::from_secs(a.stats_secs.max(1)),
        deliver: a.deliver,
        results_dir: a.results_dir,
        fee: a.fee,
        chain_id: a.chain_id,
        max_blocks: a.blocks,
        result_blob: !a.result_file,
        models_dir: a.models,
        max_prepared: a.max_prepared,
        ..MinerConfig::default()
    };
    let att: Option<&dyn ResultAttester> = if a.deliver { Some(&attester) } else { None };
    let mut miner = Miner::new(&node, att, key, cfg, Box::new(|v| println!("{v}")));
    miner.run(&|| false)
}

fn nodes(urls: &[String]) -> Result<FailoverNode> {
    let urls: Vec<String> = urls.iter().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect();
    Ok(FailoverNode::new(&urls)?.with_log(Box::new(|v| println!("{v}"))))
}
