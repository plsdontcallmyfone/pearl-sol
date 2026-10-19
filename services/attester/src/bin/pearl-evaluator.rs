//! pearl-evaluator: the reference evaluator anyone can run (docs/EVALUATORS.md).
//!
//!   pearl-evaluator --keypair my-evaluator.json --rpc http://127.0.0.1:10630
//!
//! Polls the node for Delivered jobs naming this key as an evaluator, checks each
//! delivery, and sends CompleteJob (accept) or RejectResult (provable failure) once its
//! signatures meet the job's threshold. HTTP (127.0.0.1 by default):
//!   GET /health, GET /verdicts, GET /signatures/<job_id hex>, POST /result

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;

use pearl_attester::evaluator::{EvalConfig, Evaluator};
use pearl_attester::{Attester, JobSource};

#[derive(Parser)]
#[command(name = "pearl-evaluator")]
struct Args {
    /// Evaluator key file (Solana JSON, base58 or hex). Its public key is what buyers name.
    #[arg(long)]
    keypair: PathBuf,
    /// Agent-chain node RPC.
    #[arg(long, default_value = pearl_node::DEFAULT_RPC)]
    rpc: String,
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// HTTP port (0 = no HTTP server).
    #[arg(long, default_value_t = 10667)]
    port: u16,
    /// Seconds between passes over the node's Delivered jobs.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    #[arg(long, default_value_t = 2)]
    freivalds_rounds: usize,
    /// Do not send CompleteJob / RejectResult; only sign and publish verdicts.
    #[arg(long)]
    no_submit: bool,
    /// Reject a delivery whose result data stays unavailable this many blocks (0 = never).
    #[arg(long, default_value_t = 20)]
    unavailable_blocks: u64,
    /// Another evaluator's base URL, for threshold > 1 jobs (repeatable).
    #[arg(long)]
    peer: Vec<String>,
    /// Model store for inference tickets.
    #[arg(long, default_value = "models")]
    models: PathBuf,
    /// One pass, print the verdicts as JSON, exit.
    #[arg(long)]
    once: bool,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let att = Attester {
        key: pearl_node::read_key_file(&a.keypair)?,
        source: JobSource::Node(Box::new(pearl_node::HttpNode::new(&a.rpc))),
        freivalds_rounds: a.freivalds_rounds,
        models_dir: a.models,
    };
    let cfg = EvalConfig { submit: !a.no_submit, unavailable_blocks: a.unavailable_blocks, peers: a.peer };
    let ev = Arc::new(Evaluator::new(att, cfg)?);
    let me = bs58::encode(ev.pubkey()).into_string();
    if a.once {
        let v = ev.round()?;
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if a.port != 0 {
        let addr = format!("{}:{}", a.host, a.port);
        let server = tiny_http::Server::http(&addr).map_err(|e| anyhow!("bind {addr}: {e} (is the port in use? lsof -ti :{})", a.port))?;
        eprintln!("pearl-evaluator {me} on http://{addr} (node {}, pid {})", a.rpc, std::process::id());
        let ev2 = ev.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = Vec::new();
                let _ = req.as_reader().take(64 << 20).read_to_end(&mut body);
                let method = req.method().as_str().to_uppercase();
                let path = req.url().split('?').next().unwrap_or("").to_string();
                let (status, v) = ev2.route(&method, &path, &body);
                let resp = tiny_http::Response::from_string(v.to_string())
                    .with_status_code(status)
                    .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
                let _ = req.respond(resp);
            }
        });
    } else {
        eprintln!("pearl-evaluator {me} (node {}, no HTTP, pid {})", a.rpc, std::process::id());
    }
    loop {
        match ev.round() {
            Ok(v) => {
                for s in v {
                    eprintln!("{} {} {} by {}: {}{}", match s.decision { pearl_attester::evaluator::Decision::Accept => "ACCEPT", _ => "REJECT" },
                              s.job_id, s.result_hash, s.deliverer, s.reason, s.tx_hash.map(|h| format!(" (tx {h})")).unwrap_or_default());
                }
            }
            Err(e) => eprintln!("round: {e:#}"),
        }
        std::thread::sleep(Duration::from_secs(a.interval.max(1)));
    }
}
