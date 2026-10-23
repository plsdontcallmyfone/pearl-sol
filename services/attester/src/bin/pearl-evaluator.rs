//! pearl-evaluator: the reference evaluator anyone can run (docs/EVALUATORS.md).
//!
//!   pearl-evaluator --keypair my-evaluator.json --rpc http://127.0.0.1:10630
//!
//! Polls the node for Delivered jobs naming this key as an evaluator, checks each
//! delivery, and sends CompleteJob (accept) or RejectResult (provable failure) once its
//! signatures meet the job's threshold. HTTP (127.0.0.1 by default):
//!   GET /health, GET /verdicts, GET /signatures/<job_id hex>, POST /result
//!
//! LLM-judge mode (docs/EVALUATORS.md "LLM judge", docs/MARKET.md 5): judges delivered hires
//! on the services market contract that name this key, with a paid Cadre thought:
//!   pearl-evaluator --keypair judge.json --rpc <node> --judge-market <contract> \
//!       --gateway https://api.<host>/v1/openai --solana-rpc <url> --port 0

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
    /// LLM-judge mode: the services market contract to judge hires on (base58).
    #[arg(long)]
    judge_market: Option<String>,
    /// Inference gateway (or cadre-api's /v1/openai pass-through) for the judge's paid thoughts.
    #[arg(long, default_value = "https://api.64-95-12-116.sslip.io/v1/openai")]
    gateway: String,
    /// Model id for the judge (default: the gateway's default model).
    #[arg(long)]
    judge_model: Option<String>,
    #[arg(long, default_value_t = 120)]
    judge_max_tokens: u32,
    /// Solana RPC for the judge's x402 top-ups of its own prepaid balance.
    #[arg(long, default_value = "https://api.devnet.solana.com")]
    solana_rpc: String,
    #[arg(long, default_value_t = 2048)]
    topup_rows: u64,
    /// Top up below this many free rows on one ticket: a judge request (prompt + judge_max_tokens) is
    /// about 520 rows on devnet, reserved on a single ticket, so 400 let it run dry (2026-10-23).
    #[arg(long, default_value_t = 1024)]
    min_rows: u64,
    /// Daily cap on the judge's own top-up spend (lamports).
    #[arg(long, default_value_t = 150_000_000)]
    max_topup_lamports_per_day: u64,
    /// With --judge-market: run the recorded cases in this JSON file through the paid gateway,
    /// print what the model wrote as JSON and exit. Never votes (judge.rs `probe`).
    #[arg(long)]
    judge_probe: Option<PathBuf>,
}

fn run_judge(a: &Args, market: &str) -> Result<()> {
    use pearl_attester::judge::{Judge, JudgeConfig};
    let cfg = JudgeConfig {
        market: market.to_string(), gateway: a.gateway.clone(), model: a.judge_model.clone(), max_tokens: a.judge_max_tokens,
        solana_rpc: a.solana_rpc.clone(), topup_rows: a.topup_rows, min_rows: a.min_rows,
        max_topup_lamports_per_day: a.max_topup_lamports_per_day, submit: !a.no_submit,
    };
    let mut j = Judge::new(pearl_node::read_key_file(&a.keypair)?, &a.rpc, cfg)?;
    if let Some(p) = &a.judge_probe {
        let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(p)?)?;
        println!("{}", serde_json::to_string_pretty(&j.probe(&cases)?)?);
        return Ok(());
    }
    eprintln!("pearl-evaluator judge {} on market {market} (node {}, pid {})", j.address(), a.rpc, std::process::id());
    loop {
        match j.round() {
            Ok(v) => {
                if a.once {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                    return Ok(());
                }
                for s in v {
                    eprintln!("{} hire {} thought {}: {}{}", if s.accept { "ACCEPT" } else { "REJECT" }, s.hire, s.thought_ref, s.reason, s.tx_hash.map(|h| format!(" (tx {h})")).unwrap_or_default());
                }
            }
            Err(e) => eprintln!("judge round: {e:#}"),
        }
        if a.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(a.interval.max(1)));
    }
}

fn main() -> Result<()> {
    let a = Args::parse();
    if let Some(m) = a.judge_market.clone() {
        return run_judge(&a, &m);
    }
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
