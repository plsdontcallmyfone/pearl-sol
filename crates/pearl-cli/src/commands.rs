//! Commands, shared by the plain CLI (`pearl send ...`) and the dashboard's shell
//! line (`$ send ...`): one clap definition, one executor, so scripts, agents and
//! the TUI run the same code. Every command returns JSON plus a one-screen text form.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use agent_types::tx::{route_clear_message, sign_plain, Tx, TxBody};
use agent_types::PostJob;
use pearl_job::job::{commit_a, commit_b, job_key, JobHeader, MiningConfig};

use crate::brand::TICKER;
use crate::ctx::Ctx;
use crate::fmt::{self, parse_amount, short, tokens, CHAIN_DP, SPL_DP};
use crate::snapshot::{self, s, u};
use crate::solana::{self, b58};

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Open the dashboard (the default with no command).
    Tui,
    /// Everything the dashboard reads, as one object.
    Status,
    /// Agent-chain balance (tip) and pending rewards; SPL and SOL on Solana.
    Balance,
    /// The connected addresses.
    Address,
    /// Transfer TOKEN on the agent chain: `send <to> <amount>`.
    Send {
        to: String,
        /// Decimal tokens, e.g. 1.5
        amount: String,
        /// Fee in tokens (default: the chain's min_fee).
        #[arg(long)]
        fee: Option<String>,
    },
    /// Jobs on the agent chain (newest first).
    Jobs {
        /// open | delivered | completed | expired | mineable
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
    #[command(subcommand)]
    Job(JobCmd),
    /// Recent blocks: miner, job or fallback, reward.
    Blocks {
        #[arg(default_value_t = 10)]
        n: u64,
    },
    /// Transactions touching the connected account.
    Activity {
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
    #[command(subcommand)]
    Bridge(BridgeCmd),
    #[command(subcommand)]
    Lien(LienCmd),
    /// RegisterAgent: `register <name> [uri]`.
    Register {
        name: String,
        #[arg(default_value = "")]
        uri: String,
    },
    /// Agent registration and payout lien of an address (default: connected).
    Agent { address: Option<String> },
    #[command(subcommand)]
    Mine(MineCmd),
    /// Look up a transaction by hash.
    Tx { hash: String },
    /// Render the dashboard from a live read into an HTML (and .txt) file.
    Snapshot {
        /// Output path; `.html` gets a styled page, the text grid goes next to it as .txt.
        out: PathBuf,
        #[arg(long, default_value_t = 140)]
        width: u16,
        #[arg(long, default_value_t = 46)]
        height: u16,
        /// jobs | mining | bridge | agent | activity
        #[arg(long, default_value = "jobs")]
        tab: String,
    },
    /// Command list.
    Help,
    /// Leave the dashboard.
    #[command(alias = "exit", alias = "q")]
    Quit,
}

#[derive(Subcommand, Debug, Clone)]
pub enum JobCmd {
    /// One job by id (hex).
    Show { id: String },
    /// Post a matmul job: random int7 data (or --data PSD1 blob) put to the node's
    /// blob store, commitments computed here, PostJob signed and sent.
    Post {
        #[arg(long, default_value_t = 64)]
        m: u32,
        #[arg(long, default_value_t = 64)]
        n: u32,
        /// Common dimension (rank 128, 4x8 tile: the chain's default config).
        #[arg(long, default_value_t = 2048)]
        k: u32,
        /// Worker pay W, tokens.
        #[arg(long)]
        pay: String,
        /// Protocol fee P, tokens (burned into the pool; at least min_protocol_fee_effective).
        #[arg(long)]
        fee: String,
        /// Deadline, blocks from now.
        #[arg(long, default_value_t = 500)]
        deadline: u64,
        #[arg(long, default_value_t = 10)]
        max_blocks: u32,
        /// Evaluator addresses (default: the chain's first attester).
        #[arg(long)]
        evaluator: Vec<String>,
        #[arg(long, default_value_t = 1)]
        threshold: u8,
        /// Seed for the random matrices (reproducible data).
        #[arg(long)]
        seed: Option<u64>,
        /// Use this PSD1 job blob (pearl-job gen's job.blob) instead of random data.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Tx fee in tokens (default: min_fee).
        #[arg(long)]
        tx_fee: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum BridgeCmd {
    /// Solana -> chain: lock SPL in the bridge vault for the connected chain address.
    In { amount: String },
    /// Chain -> Solana: BridgeLock; released to the Solana key by the relayer or the
    /// light-client proof path once K deep.
    Out {
        amount: String,
        /// Solana recipient (default: the connected Solana key).
        #[arg(long)]
        to: Option<String>,
    },
    /// Escrow, vault, in-flight transfers and the invariant.
    Status,
}

#[derive(Subcommand, Debug, Clone)]
pub enum LienCmd {
    /// SetRoute: send `bps` of this account's income to a Solana recipient.
    Set {
        recipient: String,
        /// Share in basis points (1..=10000).
        bps: u16,
        /// Last height (default: tip + 10000).
        #[arg(long)]
        until: Option<u64>,
        /// Cap in tokens (default: the whole supply).
        #[arg(long)]
        cap: Option<String>,
        /// Address allowed to clear the lien early (default: none).
        #[arg(long)]
        release_key: Option<String>,
    },
    /// ClearRoute, signed by the lien's release key (a key file).
    Clear {
        #[arg(long)]
        release_key_file: PathBuf,
        /// Account whose lien ends (default: connected).
        #[arg(long)]
        account: Option<String>,
    },
    /// The connected account's lien.
    Show,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MineCmd {
    /// Run pearl-miner with the connected key as payout (dashboard: in the background,
    /// stopped with `mine stop` or on quit; plain mode: in the foreground).
    Start {
        #[arg(long, default_value_t = 2)]
        threads: usize,
    },
    /// Stop the miner this dashboard started.
    Stop,
    /// Blocks mined in the recent window, pending rewards, pool.
    Status,
}

/// The shell line parser (no binary name; global options come from the session).
#[derive(Parser, Debug)]
#[command(no_binary_name = true, disable_help_subcommand = true, disable_help_flag = true)]
pub struct Line {
    #[command(subcommand)]
    pub cmd: Cmd,
}

pub fn parse_line(line: &str) -> Result<Cmd> {
    let words = fmt::split_words(line)?;
    ensure!(!words.is_empty(), "empty command");
    Line::try_parse_from(words).map(|l| l.cmd).map_err(|e| anyhow!("{}", e.to_string().lines().next().unwrap_or("bad command").trim_start_matches("error: ")))
}

pub struct Out {
    pub json: Value,
    pub text: String,
}

impl Out {
    fn new(json: Value, text: impl Into<String>) -> Self {
        Self { json, text: text.into() }
    }
}

pub const HELP: &str = "\
send <to> <amount>             transfer TOKEN on the agent chain
bridge in <amount>             Solana SPL -> chain (bridge deposit)
bridge out <amount> [--to X]   chain -> Solana (BridgeLock, released when K deep)
bridge status                  escrow, vault, in flight, invariant
job post --pay W --fee P [--m 64 --n 64 --k 2048 --deadline 500 --max-blocks 10]
job show <id>                  one job
jobs [--status open]           job list
blocks [n] | activity          recent blocks | your transactions
lien set <sol-recipient> <bps> [--until H --cap X --release-key ADDR]
lien clear --release-key-file F [--account A] | lien show
register <name> [uri]          RegisterAgent
agent [address]                registration + lien
mine start [--threads 2] | mine stop | mine status
balance | address | status | tx <hash> | quit";

fn tok(units: u64) -> String {
    format!("{} {TICKER}", tokens(units as u128))
}

/// Next nonce for the connected account: the tip nonce, or one past the last nonce
/// this process used if that is higher (back-to-back sends before inclusion).
fn next_nonce(ctx: &Ctx, me: &str) -> Result<u64> {
    let acc = ctx.call("getAccount", json!({"account": me}))?;
    let tip = u(&acc, "nonce");
    let mut last = NONCE.lock().unwrap();
    let n = match &*last {
        Some((who, l)) if who == me && l + 1 > tip => l + 1,
        _ => tip,
    };
    *last = Some((me.to_string(), n));
    Ok(n)
}

static NONCE: std::sync::Mutex<Option<(String, u64)>> = std::sync::Mutex::new(None);

pub struct Sent {
    pub tx_hash: String,
    pub nonce: u64,
    pub height: Option<u64>,
}

/// Sign `body` with the connected key and send it. With --wait, returns once the tx is
/// in a block (60 s timeout).
pub fn send_body(ctx: &Ctx, body: TxBody, fee: Option<u64>, nonce: Option<u64>) -> Result<Sent> {
    let key = ctx.key()?;
    let me = b58(&key.verifying_key().to_bytes());
    let info = ctx.call("getChainInfo", json!({}))?;
    let params = ctx.call("getParams", json!({}))?;
    let chain_id = u(&info, "chain_id") as u32;
    let fee = fee.unwrap_or(0).max(u(&params, "min_fee"));
    let nonce = match nonce {
        Some(n) => n,
        None => next_nonce(ctx, &me)?,
    };
    let tx = Tx::signed(&key.to_bytes(), chain_id, nonce, fee, body);
    let wire = tx.encode();
    let local = hex::encode(tx.hash());
    let r = pearl_node::NodeApi::send_tx(&ctx.node, &wire);
    let tx_hash = match r {
        Ok(h) => h,
        Err(e) => {
            // forget the nonce so the next try re-reads it
            *NONCE.lock().unwrap() = None;
            return Err(e);
        }
    };
    ensure!(tx_hash == local, "node tx hash {tx_hash} != local {local}");
    let height = if ctx.opts.wait { Some(wait_included(ctx, &tx_hash)?) } else { None };
    Ok(Sent { tx_hash, nonce, height })
}

pub fn wait_included(ctx: &Ctx, tx_hash: &str) -> Result<u64> {
    let t0 = Instant::now();
    loop {
        let v = ctx.call("getTx", json!({"tx_hash": tx_hash}))?;
        if s(&v, "status") == "included" {
            return Ok(u(&v, "height"));
        }
        ensure!(t0.elapsed() < Duration::from_secs(60), "tx {tx_hash} not included after 60 s");
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn sent_json(s: &Sent) -> Value {
    json!({"tx_hash": s.tx_hash, "nonce": s.nonce, "included_height": s.height})
}

fn sent_text(what: &str, s: &Sent) -> String {
    match s.height {
        Some(h) => format!("{what}: tx {} included at height {h}", short(&s.tx_hash)),
        None => format!("{what}: tx {} sent (nonce {})", short(&s.tx_hash), s.nonce),
    }
}

fn key32(s: &str) -> Result<[u8; 32]> {
    pearl_job::spec::parse_key32(s).with_context(|| format!("{s:?} is not a 32-byte address (base58 or hex)"))
}

fn me(ctx: &Ctx) -> Result<String> {
    ctx.me.as_ref().map(b58).ok_or_else(|| anyhow!("no account: pass --key FILE or --address ADDR"))
}

fn opt_tokens(s: &Option<String>) -> Result<Option<u64>> {
    s.as_deref().map(|x| parse_amount(x, CHAIN_DP)).transpose()
}

pub fn exec(ctx: &Ctx, cmd: &Cmd, shell: bool) -> Result<Out> {
    match cmd {
        Cmd::Tui => bail!("already in the dashboard"),
        Cmd::Help => Ok(Out::new(json!({"help": HELP}), HELP)),
        Cmd::Quit => Ok(Out::new(json!({"quit": true}), "bye")),
        Cmd::Address => {
            let j = json!({"address": ctx.me.as_ref().map(b58), "solana_address": ctx.sol_me.as_ref().map(b58), "has_key": ctx.has_key()});
            let t = format!("chain {}  solana {}", s(&j, "address"), s(&j, "solana_address"));
            Ok(Out::new(j, t))
        }
        Cmd::Status => {
            let sn = snapshot::gather(ctx);
            let j = sn.to_json();
            let inv = sn.invariant();
            let t = format!(
                "height {} (final {}, K {})  balance {}  pending {}  jobs {}  invariant {}{}",
                sn.height(),
                u(&sn.info, "finalized_height"),
                sn.finality_k(),
                sn.balance().map(tok).unwrap_or("-".into()),
                tok(sn.pending_rewards()),
                sn.jobs_total,
                inv.map(|i| if i.ok { "OK" } else { "BROKEN" }).unwrap_or("n/a"),
                if sn.errors.is_empty() { String::new() } else { format!("\nerrors: {}", sn.errors.join("; ")) }
            );
            Ok(Out::new(j, t))
        }
        Cmd::Balance => balance(ctx),
        Cmd::Send { to, amount, fee } => {
            let to_k = key32(to)?;
            ensure!(!agent_types::is_reserved(&to_k), "{to} is a reserved address (escrow or pool): nobody can spend from it");
            let amt = parse_amount(amount, CHAIN_DP)?;
            ensure!(amt > 0, "amount must be positive");
            let s = send_body(ctx, TxBody::Transfer { to: to_k, amount: amt }, opt_tokens(fee)?, None)?;
            let mut j = sent_json(&s);
            j["to"] = json!(b58(&to_k));
            j["amount"] = json!(amt.to_string());
            Ok(Out::new(j, sent_text(&format!("send {} to {}", tok(amt), short(&b58(&to_k))), &s)))
        }
        Cmd::Jobs { status, limit } => {
            let mut p = json!({"limit": 1});
            if let Some(st) = status {
                p["status"] = json!(st);
            }
            let total = u(&ctx.call("listJobs", p.clone())?, "total");
            p["limit"] = json!((*limit).min(200));
            p["offset"] = json!(total.saturating_sub(*limit));
            let mut jobs = ctx.call("listJobs", p)?["jobs"].as_array().cloned().unwrap_or_default();
            jobs.reverse();
            let text = if jobs.is_empty() { "no jobs".to_string() } else { jobs.iter().map(job_line).collect::<Vec<_>>().join("\n") };
            Ok(Out::new(json!({"total": total, "jobs": jobs}), text))
        }
        Cmd::Job(JobCmd::Show { id }) => {
            let v = ctx.call("getJob", json!({"job_id": id}))?;
            ensure!(!v.is_null(), "no job {id}");
            let t = job_detail(&v);
            Ok(Out::new(v, t))
        }
        Cmd::Job(post @ JobCmd::Post { .. }) => job_post(ctx, post),
        Cmd::Blocks { n } => {
            let tip = u(&ctx.call("getChainInfo", json!({}))?, "height");
            let mut out = vec![];
            for h in (tip.saturating_sub(n.saturating_sub(1).min(199))..=tip).rev() {
                let b = ctx.call("getBlock", json!({"height": h}))?;
                if b.is_object() {
                    out.push(json!({"height": h, "hash": b["hash"], "miner": b["header_fields"]["miner"], "job_id": b["header_fields"]["job_id"],
                                    "fees": b["fees"], "subsidy": b["subsidy"], "reward": b["reward"], "final": b["final"],
                                    "txs": b["txs"].as_array().map(|a| a.len())}));
                }
            }
            let mine = ctx.me.as_ref().map(b58);
            let t = out
                .iter()
                .map(|b| {
                    let m = s(b, "miner");
                    format!(
                        "{:>7}  {:<13} {:<9} {:>16}  {}",
                        u(b, "height"),
                        if Some(m.to_string()) == mine { "you".to_string() } else { short(m) },
                        if s(b, "job_id").chars().all(|c| c == '0') { "fallback".to_string() } else { short(s(b, "job_id")) },
                        tok(u(b, "reward")),
                        if b["final"] == true { "final" } else { "pending" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Out::new(json!({"blocks": out}), t))
        }
        Cmd::Activity { limit } => {
            let v = ctx.call("getAccountTxs", json!({"account": me(ctx)?, "limit": (*limit).clamp(1, 200)}))?;
            let txs = v["txs"].as_array().cloned().unwrap_or_default();
            let t = if txs.is_empty() {
                "no transactions".into()
            } else {
                txs.iter()
                    .map(|x| format!("{:>7}  {:<13} {:<9} {}  {}", u(x, "height"), s(x, "kind"), s(x, "role"), short(s(x, "tx_hash")), if x["final"] == true { "final" } else { "pending" }))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Out::new(v, t))
        }
        Cmd::Bridge(b) => bridge(ctx, b),
        Cmd::Lien(l) => lien(ctx, l),
        Cmd::Register { name, uri } => {
            ensure!(!name.is_empty() && name.len() <= agent_types::tx::MAX_NAME, "name must be 1..64 bytes");
            let s = send_body(ctx, TxBody::RegisterAgent { name: name.as_bytes().to_vec(), uri: uri.as_bytes().to_vec() }, None, None)?;
            Ok(Out::new(sent_json(&s), sent_text(&format!("register {name:?}"), &s)))
        }
        Cmd::Agent { address } => {
            let a = match address {
                Some(a) => b58(&key32(a)?),
                None => me(ctx)?,
            };
            let agent = ctx.call("getAgent", json!({"account": a}))?;
            let route = ctx.call("getRoute", json!({"account": a}))?;
            let t = format!(
                "{}: {}\nlien: {}",
                short(&a),
                if agent.is_null() { "not registered".into() } else { format!("{} ({}) since height {}", s(&agent, "name"), s(&agent, "uri"), u(&agent, "registered_height")) },
                route_text(&route)
            );
            Ok(Out::new(json!({"account": a, "agent": agent, "route": route}), t))
        }
        Cmd::Mine(m) => mine(ctx, m, shell),
        Cmd::Tx { hash } => {
            let v = ctx.call("getTx", json!({"tx_hash": hash}))?;
            ensure!(!v.is_null(), "no tx {hash}");
            let t = format!("{} {} from {} fee {}  {} {}", s(&v, "kind"), short(hash), short(s(&v, "from")), tok(u(&v, "fee")), s(&v, "status"), v.get("height").map(|h| format!("at {h}")).unwrap_or_default());
            Ok(Out::new(v, t))
        }
        Cmd::Snapshot { out, width, height, tab } => {
            let sn = snapshot::gather(ctx);
            let tab = crate::ui::Tab::parse(tab)?;
            let (html, text) = crate::html::render_snapshot(&sn, *width, *height, tab)?;
            std::fs::write(out, html).with_context(|| format!("write {}", out.display()))?;
            let txt = out.with_extension("txt");
            std::fs::write(&txt, text)?;
            Ok(Out::new(json!({"html": out, "text": txt, "errors": sn.errors}), format!("wrote {} and {}", out.display(), txt.display())))
        }
    }
}

fn balance(ctx: &Ctx) -> Result<Out> {
    let a = me(ctx)?;
    let acc = ctx.call("getAccount", json!({"account": a}))?;
    let info = ctx.call("getChainInfo", json!({}))?;
    // pending = coinbase of this account's blocks not yet K deep (credited at K)
    let (tip, k) = (u(&info, "height"), u(&info, "finality_k"));
    let mut pending = 0u64;
    for h in tip.saturating_sub(k)..=tip {
        let b = ctx.call("getBlock", json!({"height": h}))?;
        if b["header_fields"]["miner"].as_str() == Some(a.as_str()) && b["final"] != true {
            pending += u(&b, "reward");
        }
    }
    let mut j = json!({"address": a, "balance": acc["balance"], "nonce": acc["nonce"], "pending_rewards": pending.to_string(),
                       "height": tip, "finality_k": k});
    let mut t = format!("{}  {}  (+{} pending, not yet K deep)", short(&a), tok(u(&acc, "balance")), tok(pending));
    if let Some(sm) = &ctx.sol_me {
        let r = (|| -> Result<(u64, Option<u64>)> {
            let sol = ctx.sol.balance(sm)?;
            let cfg = ctx.sol.account(&solana::bridge_config_pda())?.map(|d| solana::BridgeConfig::parse(&d)).transpose()?;
            let spl = match cfg {
                Some(c) => Some(ctx.sol.token_balance(&solana::ata(sm, &c.mint, &c.token_program))?.unwrap_or(0)),
                None => None,
            };
            Ok((sol, spl))
        })();
        match r {
            Ok((sol, spl)) => {
                j["solana"] = json!({"address": b58(sm), "sol_lamports": sol.to_string(), "spl": spl.map(|x| x.to_string())});
                t += &format!("\nsolana {}  {} SOL  {}", short(&b58(sm)), fmt::sol(sol as u128), spl.map(|x| format!("{} {TICKER} (SPL)", fmt::spl(x as u128))).unwrap_or("no bridge mint".into()));
            }
            Err(e) => j["solana"] = json!({"error": e.to_string()}),
        }
    }
    Ok(Out::new(j, t))
}

pub fn job_line(j: &Value) -> String {
    format!(
        "{}  {:<9} W {:>14}  P {:>12}  {}x{}x{}  blocks {}/{}  deadline {}",
        short(s(j, "job_id")),
        s(j, "status"),
        tokens(u(j, "worker_pay") as u128),
        tokens(u(j, "protocol_fee") as u128),
        u(j, "m"),
        u(j, "n"),
        u(j, "k"),
        u(j, "blocks_mined"),
        u(j, "max_blocks"),
        u(j, "deadline_height")
    )
}

pub fn job_detail(j: &Value) -> String {
    let mut lines = vec![];
    for k in [
        "job_id", "status", "poster", "m", "n", "k", "worker_pay", "protocol_fee", "cap_remaining", "subsidy_paid", "blocks_mined", "max_blocks",
        "posted_height", "deadline_height", "data_uri", "deliverer", "result_hash", "result_uri",
    ] {
        let v = &j[k];
        let shown = match k {
            "worker_pay" | "protocol_fee" | "cap_remaining" | "subsidy_paid" => tok(u(j, k)),
            _ => match v {
                Value::String(x) => x.clone(),
                Value::Null => "-".into(),
                o => o.to_string(),
            },
        };
        lines.push(format!("{k:<16}{shown}"));
    }
    lines.join("\n")
}

pub fn route_text(r: &Value) -> String {
    if r.is_null() {
        return "none".into();
    }
    format!(
        "{} {}% to {} until {} (routed {}, withdrawn {}, pending {}){}",
        if r["active"] == true { "active" } else { "ended" },
        u(r, "route_bps") as f64 / 100.0,
        short(s(r, "solana_recipient")),
        u(r, "until_height"),
        tok(u(r, "routed")),
        tok(u(r, "withdrawn")),
        tok(u(r, "pending")),
        if r["cleared"] == true { " cleared" } else { "" }
    )
}

fn job_post(ctx: &Ctx, p: &JobCmd) -> Result<Out> {
    let JobCmd::Post { m, n, k, pay, fee, deadline, max_blocks, evaluator, threshold, seed, data, tx_fee } = p else { unreachable!() };
    let key = ctx.key()?;
    let poster = key.verifying_key().to_bytes();
    let info = ctx.call("getChainInfo", json!({}))?;
    let params = ctx.call("getParams", json!({}))?;
    let chain_id = u(&info, "chain_id") as u32;
    let w = parse_amount(pay, CHAIN_DP)?;
    let pf = parse_amount(fee, CHAIN_DP)?;
    let floor = u(&params, "min_protocol_fee_effective");
    ensure!(pf >= floor, "protocol fee {} is below the chain's min_protocol_fee_effective {}", tok(pf), tok(floor));

    // data: a PSD1 blob from a file, or random int7 matrices
    let (m, n, k, a, b) = match data {
        Some(path) => {
            let d = pearl_job::data::decode_job_blob(&std::fs::read(path).with_context(|| format!("read {}", path.display()))?)?;
            (d.manifest.m, d.manifest.n, d.manifest.k, d.a, d.b)
        }
        None => {
            let (a, b) = pearl_job::data::random_matrices(*m as usize, *n as usize, *k as usize, *seed);
            (*m, *n, *k, a, b)
        }
    };
    let config = MiningConfig { common_dim: k, ..MiningConfig::localnet_default() };
    pearl_job::job::check_shape(&config, m as usize, n as usize)?;
    let blob = pearl_job::data::encode_job_blob(m, n, k, &a, &b)?;
    let uri = pearl_node::NodeApi::put_blob(&ctx.node, &blob)?;

    let evaluators = if evaluator.is_empty() {
        let a = params["attesters"].as_array().and_then(|a| a.first()).and_then(|x| x.as_str()).ok_or_else(|| anyhow!("chain has no attesters; pass --evaluator"))?;
        vec![key32(a)?]
    } else {
        evaluator.iter().map(|e| key32(e)).collect::<Result<_>>()?
    };
    let deadline_height = u(&info, "height") + deadline;
    let mut job = PostJob {
        m,
        n,
        k,
        config: config.to_bytes(),
        hash_a: [0; 32],
        hash_b: [0; 32],
        data_uri: uri.as_bytes().to_vec(),
        worker_pay: w,
        protocol_fee: pf,
        deadline_height,
        max_blocks: *max_blocks,
        evaluators,
        threshold: *threshold,
    };
    let nonce = next_nonce(ctx, &b58(&poster))?;
    let fee_units = opt_tokens(tx_fee)?.unwrap_or(0).max(u(&params, "min_fee"));
    // job id, terms hash and job header from the encoded tx, by the light client's own
    // reader (crates/pearl-verify-sbf): the same rule the chain executes
    let draft = Tx::signed(&key.to_bytes(), chain_id, nonce, fee_units, TxBody::PostJob(job.clone())).encode();
    let commit = pearl_verify_sbf::chaintx::job_from_tx(&pearl_verify_sbf::chaintx::TxView::new(&draft, chain_id).map_err(|e| anyhow!("{e:?}"))?, |parts| {
        agent_types::sha256(parts)
    })
    .map_err(|e| anyhow!("job_from_tx: {e:?}"))?;
    let jk = job_key(&JobHeader::from_bytes(&commit.header76)?, &config);
    job.hash_a = commit_a(&jk, &a);
    job.hash_b = commit_b(&jk, &b, k as usize, n as usize);
    let s = send_body(ctx, TxBody::PostJob(job), Some(fee_units), Some(nonce))?;
    let job_id = hex::encode(commit.job_id);
    let mut j = sent_json(&s);
    j["job_id"] = json!(job_id);
    j["data_uri"] = json!(uri);
    j["deadline_height"] = json!(deadline_height);
    j["worker_pay"] = json!(w.to_string());
    j["protocol_fee"] = json!(pf.to_string());
    Ok(Out::new(j, sent_text(&format!("job {} {m}x{n}x{k} W {} P {}", short(&job_id), tok(w), tok(pf)), &s)))
}

fn bridge(ctx: &Ctx, b: &BridgeCmd) -> Result<Out> {
    match b {
        BridgeCmd::In { amount } => {
            let amt = parse_amount(amount, SPL_DP)?;
            ensure!(amt > 0, "amount must be positive");
            let to = ctx.me.ok_or_else(|| anyhow!("no chain account"))?;
            let (sig, nonce, slot) = solana::bridge_deposit(&ctx.sol, ctx.sol_key()?, amt, &to)?;
            let mut j = json!({"signature": sig, "deposit_nonce": nonce, "slot": slot, "amount_spl": amt.to_string(), "recipient": b58(&to)});
            let mut t = format!("bridge in {} {TICKER}: Solana deposit {nonce} confirmed (slot {slot})", fmt::spl(amt as u128));
            if ctx.opts.wait {
                let t0 = Instant::now();
                loop {
                    let d = ctx.call("getDeposit", json!({"solana_nonce": nonce}))?;
                    if d["processed"] == true {
                        t += &format!(", unlocked on the chain at height {}", u(&d, "height"));
                        j["unlocked"] = d;
                        break;
                    }
                    ensure!(t0.elapsed() < Duration::from_secs(180), "deposit {nonce} not unlocked on the chain after 180 s (is the relayer running?)");
                    std::thread::sleep(Duration::from_millis(500));
                }
            } else {
                t += "; the relayer unlocks it on the chain";
            }
            Ok(Out::new(j, t))
        }
        BridgeCmd::Out { amount, to } => {
            let spl_amt = parse_amount(amount, SPL_DP)?;
            ensure!(spl_amt > 0, "amount must be positive");
            let recipient = match to {
                Some(t) => key32(t)?,
                None => ctx.sol_me.ok_or_else(|| anyhow!("no Solana recipient: pass --to or --sol-key"))?,
            };
            let units = spl_amt * agent_types::SPL_SCALE;
            let s = send_body(ctx, TxBody::BridgeLock { solana_recipient: recipient, amount: units }, None, None)?;
            let mut j = sent_json(&s);
            j["amount"] = json!(units.to_string());
            j["solana_recipient"] = json!(b58(&recipient));
            Ok(Out::new(j, sent_text(&format!("bridge out {} to {} (released when K deep)", tok(units), short(&b58(&recipient))), &s)))
        }
        BridgeCmd::Status => {
            let sn = snapshot::gather(ctx);
            let inv = sn.invariant();
            let j = json!({"escrow": sn.escrow, "vault_spl": sn.sol.vault_spl.map(|x| x.to_string()),
                           "inflight_deposits_spl": sn.sol.inflight_dep_spl.map(|x| x.to_string()),
                           "inflight_withdrawals_spl": sn.inflight_wd_spl().to_string(),
                           "withdrawal_count": sn.withdrawal_count, "deposit_count": sn.deposit_count,
                           "invariant": inv.as_ref().map(|i| json!({"ok": i.ok, "strict": i.strict, "delta": i.delta.to_string(), "in_flight": i.in_flight.to_string()})),
                           "errors": sn.errors});
            let t = match inv {
                Some(i) => format!(
                    "vault {} SPL x 1000 vs 1e18 - escrow {}: delta {} in flight {} -> {}",
                    fmt::spl(sn.sol.vault_spl.unwrap_or(0) as u128),
                    tokens(i.outside_units as u128),
                    i.delta,
                    i.in_flight,
                    if i.ok { "OK" } else { "BROKEN" }
                ),
                None => format!("invariant not available: {}", sn.errors.join("; ")),
            };
            Ok(Out::new(j, t))
        }
    }
}

fn lien(ctx: &Ctx, l: &LienCmd) -> Result<Out> {
    match l {
        LienCmd::Show => {
            let r = ctx.call("getRoute", json!({"account": me(ctx)?}))?;
            let t = route_text(&r);
            Ok(Out::new(r, t))
        }
        LienCmd::Set { recipient, bps, until, cap, release_key } => {
            ensure!((1..=10_000).contains(bps), "bps must be 1..=10000");
            let tip = u(&ctx.call("getChainInfo", json!({}))?, "height");
            let body = TxBody::SetRoute {
                solana_recipient: key32(recipient)?,
                route_bps: *bps,
                until_height: until.unwrap_or(tip + 10_000),
                cap: opt_tokens(cap)?.unwrap_or(agent_types::TOTAL_SUPPLY),
                release_key: release_key.as_deref().map(key32).transpose()?.unwrap_or([0; 32]),
            };
            let s = send_body(ctx, body, None, None)?;
            Ok(Out::new(sent_json(&s), sent_text(&format!("lien set {}% to {}", *bps as f64 / 100.0, short(recipient)), &s)))
        }
        LienCmd::Clear { release_key_file, account } => {
            let account = match account {
                Some(a) => key32(a)?,
                None => ctx.me.ok_or_else(|| anyhow!("no account"))?,
            };
            let r = ctx.call("getRoute", json!({"account": b58(&account)}))?;
            ensure!(!r.is_null(), "{} has no lien", short(&b58(&account)));
            let route_nonce = u(&r, "route_nonce");
            let chain_id = u(&ctx.call("getChainInfo", json!({}))?, "chain_id") as u32;
            let rk = pearl_node::read_key_file(release_key_file)?;
            let sig = sign_plain(&rk.to_bytes(), &route_clear_message(chain_id, &account, route_nonce));
            let s = send_body(ctx, TxBody::ClearRoute { account, route_nonce, sig }, None, None)?;
            Ok(Out::new(sent_json(&s), sent_text(&format!("lien clear for {}", short(&b58(&account))), &s)))
        }
    }
}

fn miner_bin(ctx: &Ctx) -> Result<PathBuf> {
    if let Some(p) = &ctx.opts.miner_bin {
        return Ok(p.clone());
    }
    let exe = std::env::current_exe()?;
    let mut tried = vec![];
    for anc in exe.ancestors().skip(1).take(6) {
        for c in [anc.join("pearl-miner"), anc.join("target/release/pearl-miner"), anc.join("crates/target/release/pearl-miner")] {
            if c.is_file() {
                return Ok(c);
            }
            tried.push(c);
        }
    }
    bail!("pearl-miner not found (build it: cd crates && cargo build --release -p pearl-miner, or pass --miner-bin)")
}

fn mine(ctx: &Ctx, m: &MineCmd, shell: bool) -> Result<Out> {
    match m {
        MineCmd::Start { threads } => {
            let key = ctx.opts.key.clone().ok_or_else(|| anyhow!("mine start needs --key (the payout key file)"))?;
            let bin = miner_bin(ctx)?;
            let mut cmd = std::process::Command::new(&bin);
            cmd.args(["--rpc", &ctx.opts.rpc, "--threads", &threads.to_string()]).arg("--payout").arg(&key);
            if !shell {
                // plain mode: foreground, its JSON log lines on our stdout
                let st = cmd.status().with_context(|| format!("run {}", bin.display()))?;
                return Ok(Out::new(json!({"exit": st.code()}), format!("pearl-miner exited: {st}")));
            }
            ensure!(ctx.miner_pid().is_none(), "a miner is already running (pid {})", ctx.miner_pid().unwrap());
            let log = std::env::temp_dir().join(format!("pearl-cli-miner-{}.log", std::process::id()));
            let f = std::fs::File::create(&log)?;
            cmd.stdout(f.try_clone()?).stderr(f).stdin(std::process::Stdio::null());
            let child = cmd.spawn().with_context(|| format!("spawn {}", bin.display()))?;
            let pid = child.id();
            *ctx.miner.lock().unwrap() = Some(child);
            Ok(Out::new(json!({"pid": pid, "log": log}), format!("miner started (pid {pid}, {threads} threads), log {}", log.display())))
        }
        MineCmd::Stop => {
            let mut g = ctx.miner.lock().unwrap();
            match g.take() {
                Some(mut c) => {
                    let pid = c.id();
                    c.kill()?;
                    let _ = c.wait();
                    Ok(Out::new(json!({"stopped": pid}), format!("miner {pid} stopped")))
                }
                None => bail!("no miner started from this dashboard (other miners are left alone)"),
            }
        }
        MineCmd::Status => {
            let sn = snapshot::gather(ctx);
            let (earned, mined) = sn.window_rewards();
            let j = json!({"window_blocks": sn.blocks.len(), "blocks_mined": mined, "rewards": earned.to_string(),
                           "pending_rewards": sn.pending_rewards().to_string(), "miner_pid": sn.miner_pid, "pool": sn.pool, "pool_miner": sn.pool_miner});
            let t = format!(
                "{mined} of the last {} blocks are yours: {} ({} pending)  miner {}  pool {}",
                sn.blocks.len(),
                tok(earned),
                tok(sn.pending_rewards()),
                sn.miner_pid.map(|p| format!("running pid {p}")).unwrap_or("not started here".into()),
                if sn.pool.is_some() { "connected" } else { "not connected" }
            );
            Ok(Out::new(j, t))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_lines_parse() {
        assert!(matches!(parse_line("send 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU 1.5").unwrap(), Cmd::Send { amount, .. } if amount == "1.5"));
        assert!(matches!(parse_line("bridge in 10").unwrap(), Cmd::Bridge(BridgeCmd::In { amount }) if amount == "10"));
        assert!(matches!(parse_line("bridge out 2 --to abc").unwrap(), Cmd::Bridge(BridgeCmd::Out { to: Some(t), .. }) if t == "abc"));
        assert!(matches!(parse_line("job post --pay 5 --fee 1 --m 8 --n 16").unwrap(), Cmd::Job(JobCmd::Post { m: 8, n: 16, k: 2048, .. })));
        assert!(matches!(parse_line("job show ab12").unwrap(), Cmd::Job(JobCmd::Show { id }) if id == "ab12"));
        assert!(matches!(parse_line("lien set abc 5000 --until 99").unwrap(), Cmd::Lien(LienCmd::Set { bps: 5000, until: Some(99), .. })));
        assert!(matches!(parse_line("lien clear --release-key-file k.json").unwrap(), Cmd::Lien(LienCmd::Clear { .. })));
        assert!(matches!(parse_line("mine start --threads 4").unwrap(), Cmd::Mine(MineCmd::Start { threads: 4 })));
        assert!(matches!(parse_line("register 'my agent' https://x").unwrap(), Cmd::Register { name, .. } if name == "my agent"));
        assert!(matches!(parse_line("q").unwrap(), Cmd::Quit));
        assert!(matches!(parse_line("exit").unwrap(), Cmd::Quit));
        assert!(matches!(parse_line("help").unwrap(), Cmd::Help));
        assert!(matches!(parse_line("blocks").unwrap(), Cmd::Blocks { n: 10 }));
    }

    #[test]
    fn shell_errors_are_one_line() {
        let e = parse_line("send onlyone").unwrap_err().to_string();
        assert!(!e.contains('\n'), "{e}");
        assert!(e.contains("required"), "{e}");
        assert!(parse_line("frobnicate").unwrap_err().to_string().contains("unrecognized"));
        assert!(parse_line("").is_err());
    }

    #[test]
    fn text_forms() {
        let j = json!({"job_id": "ab".repeat(32), "status": "open", "worker_pay": "5000000000", "protocol_fee": "1000000000",
                       "m": 64, "n": 64, "k": 2048, "blocks_mined": 1, "max_blocks": 10, "deadline_height": 900});
        assert_eq!(job_line(&j), "abab...abab  open      W              5  P            1  64x64x2048  blocks 1/10  deadline 900");
        assert!(job_detail(&j).contains("worker_pay      5 TOKEN"));
        assert_eq!(route_text(&Value::Null), "none");
        let r = json!({"active": true, "route_bps": 5000, "solana_recipient": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                       "until_height": 99, "routed": "1000000500", "withdrawn": "1000000000", "pending": "500"});
        assert_eq!(route_text(&r), "active 50% to 7xKX...gAsU until 99 (routed 1 TOKEN, withdrawn 1 TOKEN, pending 0 TOKEN)");
        assert!(HELP.lines().all(|l| l.len() <= 90));
    }
}
