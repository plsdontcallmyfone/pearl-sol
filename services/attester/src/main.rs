//! pearl-attester HTTP server (M1, 1-of-1). Port 10661 by default, 127.0.0.1 only.
//!
//!   GET  /health
//!   POST /share   {job, miner, proof_b64[, job_spec]}   (v1 modes only)
//!   POST /result  {job, deliverer, result_b64 | result_uri[, job_spec]}

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::Parser;

use pearl_attester::{Attester, JobSource};
use pearl_chain::ix::Programs;
use pearl_chain::{Pubkey, Rpc};

#[derive(Parser)]
#[command(name = "pearl-attester")]
struct Args {
    /// Attester key file (Solana JSON, base58 or hex). Localnet key: keys/localnet/attester-keypair.json
    #[arg(long, default_value = "keys/localnet/attester-keypair.json")]
    keypair: PathBuf,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 10661)]
    port: u16,
    /// Agent-chain node RPC.
    #[arg(long, default_value = pearl_node::DEFAULT_RPC)]
    rpc: String,
    /// `node` (read jobs from the v2 node), `chain` (v1 SVM jobs program, superseded)
    /// or `request` (trust job_spec in requests; dev only).
    #[arg(long, default_value = "node")]
    mode: String,
    #[arg(long, default_value = pearl_chain::JOBS_PROGRAM)]
    jobs_program: String,
    #[arg(long, default_value = pearl_chain::MINING_PROGRAM)]
    mining_program: String,
    #[arg(long, default_value_t = 2)]
    freivalds_rounds: usize,
    /// Model store for inference tickets: <dir>/<model_id hex>/w/<i>.i8 (rebuilt from the
    /// node's blobs when missing).
    #[arg(long, default_value = "models")]
    models: PathBuf,
    /// Threads handling /result and /share (each a full replay or Freivalds check; requests
    /// beyond them wait in arrival order). /health never waits for them.
    #[arg(long, default_value_t = 1)]
    workers: usize,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let source = match a.mode.as_str() {
        "node" => JobSource::Node(Box::new(pearl_node::HttpNode::new(&a.rpc))),
        "chain" => JobSource::Chain {
            rpc: Rpc::new(&a.rpc),
            programs: Programs { jobs: Pubkey::from_b58(&a.jobs_program)?, mining: Pubkey::from_b58(&a.mining_program)? },
        },
        "request" => JobSource::Request,
        m => return Err(anyhow!("unknown --mode {m}")),
    };
    let att = Attester { key: pearl_node::read_key_file(&a.keypair)?, source, freivalds_rounds: a.freivalds_rounds, models_dir: a.models };
    let addr = format!("{}:{}", a.host, a.port);
    // Binding fails if anything already holds the port; we never take it over.
    let server = tiny_http::Server::http(&addr).map_err(|e| anyhow!("bind {addr}: {e} (is the port in use? lsof -ti :{})", a.port))?;
    eprintln!("pearl-attester {} listening on http://{addr} (mode {}, pid {})", att.pubkey_b58(), a.mode, std::process::id());
    // /health answers at once; /result and /share run on --workers threads (a glue replay takes
    // minutes, and the health check used to wait behind it)
    let att = std::sync::Arc::new(att);
    let a2 = std::sync::Arc::clone(&att);
    pearl_attester::serve(server, a.workers, move || att.route("GET", "/health", &[]).1, move |m, p, b| a2.route(m, p, b), Default::default(), true);
    Ok(())
}
