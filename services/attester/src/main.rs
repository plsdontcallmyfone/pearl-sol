//! pearl-attester HTTP server (M1, 1-of-1). Port 10661 by default, 127.0.0.1 only.
//!
//!   GET  /health
//!   POST /share   {job, miner, proof_b64[, job_spec]}
//!   POST /result  {job, deliverer, result_b64 | result_uri[, job_spec]}

use std::io::Read;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::Parser;

use pearl_attester::{Attester, JobSource};
use pearl_chain::ix::Programs;
use pearl_chain::{read_keypair, Pubkey, Rpc};

#[derive(Parser)]
#[command(name = "pearl-attester")]
struct Args {
    /// Attester keypair (Solana JSON). Localnet key: keys/localnet/attester-keypair.json
    #[arg(long, default_value = "keys/localnet/attester-keypair.json")]
    keypair: PathBuf,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 10661)]
    port: u16,
    /// Agent-chain RPC.
    #[arg(long, default_value = pearl_chain::DEFAULT_RPC)]
    rpc: String,
    /// `chain` (read jobs from the agent chain) or `request` (trust job_spec in requests; dev only).
    #[arg(long, default_value = "chain")]
    mode: String,
    #[arg(long, default_value = pearl_chain::JOBS_PROGRAM)]
    jobs_program: String,
    #[arg(long, default_value = pearl_chain::MINING_PROGRAM)]
    mining_program: String,
    #[arg(long, default_value_t = 2)]
    freivalds_rounds: usize,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let source = match a.mode.as_str() {
        "chain" => JobSource::Chain {
            rpc: Rpc::new(&a.rpc),
            programs: Programs { jobs: Pubkey::from_b58(&a.jobs_program)?, mining: Pubkey::from_b58(&a.mining_program)? },
        },
        "request" => JobSource::Request,
        m => return Err(anyhow!("unknown --mode {m}")),
    };
    let att = Attester { key: read_keypair(&a.keypair)?, source, freivalds_rounds: a.freivalds_rounds };
    let addr = format!("{}:{}", a.host, a.port);
    // Binding fails if anything already holds the port; we never take it over.
    let server = tiny_http::Server::http(&addr).map_err(|e| anyhow!("bind {addr}: {e} (is the port in use? lsof -ti :{})", a.port))?;
    eprintln!("pearl-attester {} listening on http://{addr} (mode {}, pid {})", att.pubkey_b58(), a.mode, std::process::id());
    for mut req in server.incoming_requests() {
        let mut body = Vec::new();
        let limit = 64 << 20;
        let _ = req.as_reader().take(limit).read_to_end(&mut body);
        let method = req.method().as_str().to_uppercase();
        let path = req.url().split('?').next().unwrap_or("").to_string();
        let (status, v) = att.route(&method, &path, &body);
        eprintln!("{method} {path} -> {status}{}", v.get("error").map(|e| format!(" {e}")).unwrap_or_default());
        let resp = tiny_http::Response::from_string(v.to_string())
            .with_status_code(status)
            .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
        let _ = req.respond(resp);
    }
    Ok(())
}
