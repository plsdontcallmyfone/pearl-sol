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

use crate::brand::{self, TICKER};
use crate::ctx::Ctx;
use crate::fmt::{self, parse_amount, short, short_n, tokens, CHAIN_DP, SPL_DP};
use crate::snapshot::{self, s, u, Snapshot};
use crate::solana::{self, b58};
use crate::term::{status_tone, Block, Panel, Table, Tone, View};

#[derive(Subcommand, Debug, Clone)]
pub enum Cmd {
    /// Open the full-screen dashboard.
    #[command(alias = "tui")]
    Dashboard,
    /// Everything the dashboard reads, as one object.
    Status,
    /// Agent-chain balance (tip) and pending rewards; SPL and SOL on Solana.
    Balance,
    /// The connected addresses.
    Address,
    /// Transfer LODE on the agent chain: `send <to> <amount>`.
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
    /// Storefront buyback: share of order SOL, destination, vault, totals and the latest
    /// buys, all read from Solana (config, vault, the program's BuybackExecuted events).
    Buyback {
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
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
    /// One-line form (the dashboard's shell strip).
    pub text: String,
    /// Text mode: the info box(es) the command ends in. None = print `text`.
    pub view: Option<View>,
}

impl Out {
    fn new(json: Value, text: impl Into<String>) -> Self {
        Self { json, text: text.into(), view: None }
    }

    fn view(mut self, v: View) -> Self {
        self.view = Some(v);
        self
    }
}

pub const HELP: &str = "\
send <to> <amount>             transfer LODE  on the agent chain
bridge in <amount>             Solana SPL -> chain (bridge deposit)
bridge out <amount> [--to X]   chain -> Solana (BridgeLock, released when K deep)
bridge status                  escrow, vault, in flight, invariant
job post --pay W --fee P [--m 64 --n 64 --k 2048 --deadline 500 --max-blocks 10]
job show <id>                  one job (a unique id prefix is enough)
jobs [--status open]           job list
blocks [n] | activity          recent blocks | your transactions
lien set <sol-recipient> <bps> [--until H --cap X --release-key ADDR]
lien clear --release-key-file F [--account A] | lien show
register <name> [uri]          RegisterAgent
agent [address]                registration + lien
buyback [--limit 10]           storefront buyback: share, vault, totals, latest buys
mine start [--threads 2] | mine stop | mine status
balance | address | status | tx <hash> | dashboard | quit";

/// Common commands, shown under the no-command overview.
pub const OVERVIEW_HINT: &str = "\
pearl balance    pearl send <to> <amount>    pearl jobs    pearl job post --pay W --fee P
pearl bridge out <amount>    pearl bridge status    pearl mine start    pearl dashboard";

fn tok(units: u64) -> String {
    format!("{} {TICKER}", tokens(units as u128))
}

/// All 9 decimals (fees are a few base units).
fn tok_exact(units: u64) -> String {
    format!("{} {TICKER}", fmt::amount(units as u128, CHAIN_DP, CHAIN_DP))
}

/// Next nonce for the connected account: past the tip nonce, past any of its txs still
/// in the node's mempool (another process may have sent them), and past the last nonce
/// this process used (back-to-back sends before inclusion).
fn next_nonce(ctx: &Ctx, me: &str) -> Result<u64> {
    let acc = ctx.call("getAccount", json!({"account": me}))?;
    let mut n = u(&acc, "nonce");
    // getMempool lists tx hashes; getTx gives each one's sender and nonce
    if let Ok(mp) = ctx.call("getMempool", json!({})) {
        for h in mp["txs"].as_array().into_iter().flatten().filter_map(|h| h.as_str()) {
            let Ok(t) = ctx.call("getTx", json!({"tx_hash": h})) else { continue };
            if s(&t, "from") == me && s(&t, "status") == "pending" && u(&t, "nonce") >= n {
                n = u(&t, "nonce") + 1;
            }
        }
    }
    let mut last = NONCE.lock().unwrap();
    if let Some((who, l)) = &*last {
        if who == me && l + 1 > n {
            n = l + 1;
        }
    }
    *last = Some((me.to_string(), n));
    Ok(n)
}

static NONCE: std::sync::Mutex<Option<(String, u64)>> = std::sync::Mutex::new(None);

pub struct Sent {
    pub tx_hash: String,
    pub nonce: u64,
    pub fee: u64,
    pub from: String,
    pub height: Option<u64>,
    /// Seen K deep (text mode follows the tx there).
    pub is_final: bool,
}

impl Sent {
    fn state(&self) -> &'static str {
        match (self.height, self.is_final) {
            (_, true) => "final",
            (Some(_), false) => "included",
            (None, _) => "pending",
        }
    }
}

/// Sign `body` with the connected key and send it, as steps in text mode. Waits for
/// inclusion with --wait or in text mode (60 s timeout), then, when `to_final` and
/// text mode, for K-deep finality.
pub fn send_body(ctx: &Ctx, what: &str, body: TxBody, fee: Option<u64>, nonce: Option<u64>, to_final: bool) -> Result<Sent> {
    let key = ctx.key()?;
    let me = b58(&key.verifying_key().to_bytes());
    let st = ctx.ui.step(format!("Signing {what}"));
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
    st.ok(format!("Signed {what} (nonce {nonce}, fee {})", tok_exact(fee)));
    let st = ctx.ui.step("Submitting to the node");
    let tx_hash = match pearl_node::NodeApi::send_tx(&ctx.node, &wire) {
        Ok(h) => h,
        Err(e) => {
            // forget the nonce so the next try re-reads it
            *NONCE.lock().unwrap() = None;
            st.fail(&e);
            return Err(e);
        }
    };
    let r = (|| {
        ensure!(tx_hash == local, "node tx hash {tx_hash} != local {local}");
        Ok(())
    })();
    st.wrap(r, |_| format!("Submitted tx {}", short_n(&tx_hash, 8)))?;
    let mut sent = Sent { tx_hash, nonce, fee, from: me, height: None, is_final: false };
    if ctx.wait_inclusion() {
        sent.height = Some(wait_included(ctx, &sent.tx_hash)?);
    }
    if to_final && ctx.follow() {
        sent.height = Some(wait_final(ctx, &sent.tx_hash)?);
        sent.is_final = true;
    }
    Ok(sent)
}

pub fn wait_included(ctx: &Ctx, tx_hash: &str) -> Result<u64> {
    let st = ctx.ui.step("Waiting for inclusion in a block");
    let t0 = Instant::now();
    let r = (|| loop {
        let v = ctx.call("getTx", json!({"tx_hash": tx_hash}))?;
        if s(&v, "status") == "included" {
            return Ok(u(&v, "height"));
        }
        ensure!(t0.elapsed() < Duration::from_secs(60), "tx {tx_hash} not included after 60 s");
        std::thread::sleep(Duration::from_millis(300));
    })();
    st.wrap(r, |h| format!("Included in block {h}"))
}

/// Follow an included tx until its block is K deep (`finalized_height >= height`).
/// Returns the final height (a reorg may move the tx).
pub fn wait_final(ctx: &Ctx, tx_hash: &str) -> Result<u64> {
    let st = ctx.ui.step("Waiting for finality");
    let t0 = Instant::now();
    let mut h = 0;
    let mut k = 0;
    let r = (|| loop {
        let t = ctx.call("getTx", json!({"tx_hash": tx_hash}))?;
        let info = ctx.call("getChainInfo", json!({}))?;
        k = u(&info, "finality_k");
        if s(&t, "status") == "included" {
            h = u(&t, "height");
            let depth = u(&info, "height").saturating_sub(h).min(k);
            st.label(format!("Finality {depth}/{k} blocks deep"));
            st.progress(depth, k);
            if u(&info, "finalized_height") >= h {
                return Ok(h);
            }
        } else {
            st.label("Reorged out, waiting to be included again");
            st.progress(0, k);
        }
        ensure!(t0.elapsed() < Duration::from_secs(300), "tx {tx_hash} not final after 300 s");
        std::thread::sleep(Duration::from_millis(400));
    })();
    st.wrap(r, |h| format!("Final, {k} blocks deep (block {h})"))
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

/// The tx rows every write box starts with.
fn tx_panel(chip: &str, s: &Sent) -> Panel {
    Panel::new(chip).kv("Hash", &s.tx_hash).kv("From", &s.from)
}

fn tx_tail(p: Panel, s: &Sent) -> Panel {
    p.kv("Fee", tok_exact(s.fee)).kv("Nonce", s.nonce.to_string()).kv("Block", s.height.map(|h| h.to_string()).unwrap_or("-".into())).state("State", s.state())
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

fn final_word(v: &Value) -> &'static str {
    if v["final"] == true {
        "final"
    } else {
        "pending"
    }
}

fn cell(s: impl Into<String>) -> (String, Tone) {
    (s.into(), Tone::Plain)
}

fn state_cell(s: &str) -> (String, Tone) {
    (s.to_string(), status_tone(s))
}

pub fn exec(ctx: &Ctx, cmd: &Cmd, shell: bool) -> Result<Out> {
    match cmd {
        Cmd::Dashboard => bail!("already in the dashboard"),
        Cmd::Help => Ok(Out::new(json!({"help": HELP}), HELP)),
        Cmd::Quit => Ok(Out::new(json!({"quit": true}), "bye")),
        Cmd::Address => {
            let j = json!({"address": ctx.me.as_ref().map(b58), "solana_address": ctx.sol_me.as_ref().map(b58), "has_key": ctx.has_key()});
            let t = format!("chain {}  solana {}", s(&j, "address"), s(&j, "solana_address"));
            let p = Panel::new("Address").kv("Agent chain", s(&j, "address")).kv("Solana", s(&j, "solana_address")).kv("Key", if ctx.has_key() { "loaded (never printed)" } else { "none (watching)" });
            Ok(Out::new(j, t).view(View::panel(p)))
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
            Ok(Out::new(j, t).view(View::panel(overview(&sn, true))))
        }
        Cmd::Balance => balance(ctx),
        Cmd::Send { to, amount, fee } => {
            let to_k = key32(to)?;
            ensure!(!agent_types::is_reserved(&to_k), "{to} is a reserved address (escrow or pool): nobody can spend from it");
            let amt = parse_amount(amount, CHAIN_DP)?;
            ensure!(amt > 0, "amount must be positive");
            let s = send_body(ctx, "Transfer", TxBody::Transfer { to: to_k, amount: amt }, opt_tokens(fee)?, None, true)?;
            let mut j = sent_json(&s);
            j["to"] = json!(b58(&to_k));
            j["amount"] = json!(amt.to_string());
            let p = tx_tail(tx_panel("Transfer", &s).kv("To", b58(&to_k)).kv("Amount", tok(amt)), &s);
            Ok(Out::new(j, sent_text(&format!("send {} to {}", tok(amt), short(&b58(&to_k))), &s)).view(View::panel(p)))
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
            let v = View(vec![Block::Table(jobs_table(&jobs))]).hint(format!(
                "{} of {} jobs{}  ·  pearl job show <id> (a unique id prefix is enough)",
                jobs.len(),
                total,
                status.as_deref().map(|x| format!(" ({x})")).unwrap_or_default()
            ));
            Ok(Out::new(json!({"total": total, "jobs": jobs}), text).view(v))
        }
        Cmd::Job(JobCmd::Show { id }) => {
            let id = resolve_job_id(ctx, id)?;
            let v = ctx.call("getJob", json!({"job_id": id}))?;
            ensure!(!v.is_null(), "no job {id}");
            let t = job_detail(&v);
            let view = View::panel(job_panel(&v));
            Ok(Out::new(v, t).view(view))
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
            let who = |m: &str| if Some(m.to_string()) == mine { "you".to_string() } else { short(m) };
            let job = |b: &Value| if s(b, "job_id").chars().all(|c| c == '0') { "fallback".to_string() } else { short(s(b, "job_id")) };
            let t = out
                .iter()
                .map(|b| format!("{:>7}  {:<13} {:<9} {:>16}  {}", u(b, "height"), who(s(b, "miner")), job(b), tok(u(b, "reward")), final_word(b)))
                .collect::<Vec<_>>()
                .join("\n");
            let table = Table {
                chip: Some("Blocks".into()),
                head: ["HEIGHT", "MINER", "JOB", "TXS", "REWARD", "STATE"].map(String::from).to_vec(),
                right: vec![true, false, false, true, true, false],
                rows: out
                    .iter()
                    .map(|b| vec![cell(u(b, "height").to_string()), cell(who(s(b, "miner"))), cell(job(b)), cell(u(b, "txs").to_string()), cell(tok(u(b, "reward"))), state_cell(final_word(b))])
                    .collect(),
                empty: "no blocks".into(),
            };
            Ok(Out::new(json!({"blocks": out}), t).view(View(vec![Block::Table(table)])))
        }
        Cmd::Activity { limit } => {
            let v = ctx.call("getAccountTxs", json!({"account": me(ctx)?, "limit": (*limit).clamp(1, 200)}))?;
            let txs = v["txs"].as_array().cloned().unwrap_or_default();
            let t = if txs.is_empty() {
                "no transactions".into()
            } else {
                txs.iter()
                    .map(|x| format!("{:>7}  {:<13} {:<9} {}  {}", u(x, "height"), s(x, "kind"), s(x, "role"), short(s(x, "tx_hash")), final_word(x)))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let table = Table {
                chip: Some("Activity".into()),
                head: ["HEIGHT", "KIND", "ROLE", "TX", "FEE", "STATE"].map(String::from).to_vec(),
                right: vec![true, false, false, false, true, false],
                rows: txs
                    .iter()
                    .map(|x| vec![cell(u(x, "height").to_string()), cell(s(x, "kind")), cell(s(x, "role")), cell(short_n(s(x, "tx_hash"), 8)), cell(tok(u(x, "fee"))), state_cell(final_word(x))])
                    .collect(),
                empty: "no transactions yet".into(),
            };
            Ok(Out::new(v, t).view(View(vec![Block::Table(table)])))
        }
        Cmd::Bridge(b) => bridge(ctx, b),
        Cmd::Buyback { limit } => buyback(ctx, *limit),
        Cmd::Lien(l) => lien(ctx, l),
        Cmd::Register { name, uri } => {
            ensure!(!name.is_empty() && name.len() <= agent_types::tx::MAX_NAME, "name must be 1..64 bytes");
            let s = send_body(ctx, "RegisterAgent", TxBody::RegisterAgent { name: name.as_bytes().to_vec(), uri: uri.as_bytes().to_vec() }, None, None, true)?;
            let mut view = View::panel(tx_tail(tx_panel("RegisterAgent", &s).kv("Name", name.as_str()).kv("URI", uri.as_str()), &s));
            if ctx.ui.is_plain() {
                let a = ctx.call("getAgent", json!({"account": s.from}))?;
                view = view.with(Block::Panel(agent_panel(&s.from, &a, &Value::Null, false)));
            }
            Ok(Out::new(sent_json(&s), sent_text(&format!("register {name:?}"), &s)).view(view))
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
            let view = View::panel(agent_panel(&a, &agent, &route, true));
            Ok(Out::new(json!({"account": a, "agent": agent, "route": route}), t).view(view))
        }
        Cmd::Mine(m) => mine(ctx, m, shell),
        Cmd::Tx { hash } => {
            let v = ctx.call("getTx", json!({"tx_hash": hash}))?;
            ensure!(!v.is_null(), "no tx {hash}");
            let t = format!("{} {} from {} fee {}  {} {}", s(&v, "kind"), short(hash), short(s(&v, "from")), tok(u(&v, "fee")), s(&v, "status"), v.get("height").map(|h| format!("at {h}")).unwrap_or_default());
            let fin = ctx.call("getChainInfo", json!({})).map(|i| v.get("height").is_some() && u(&i, "finalized_height") >= u(&v, "height")).unwrap_or(false);
            let p = Panel::new("Transaction")
                .kv("Hash", hash.as_str())
                .kv("Kind", s(&v, "kind"))
                .kv("From", s(&v, "from"))
                .kv("Nonce", u(&v, "nonce").to_string())
                .kv("Fee", tok_exact(u(&v, "fee")))
                .kv("Block", v.get("height").map(|h| h.to_string()).unwrap_or("-".into()))
                .kv("Confirmations", v.get("confirmations").map(|h| h.to_string()).unwrap_or("-".into()))
                .state("State", if fin { "final" } else { s(&v, "status") });
            Ok(Out::new(v, t).view(View::panel(p)))
        }
        Cmd::Snapshot { out, width, height, tab } => {
            let sn = snapshot::gather(ctx);
            let tab = crate::ui::Tab::parse(tab)?;
            let (html, text) = crate::html::render_snapshot(&sn, *width, *height, tab)?;
            std::fs::write(out, html).with_context(|| format!("write {}", out.display()))?;
            let txt = out.with_extension("txt");
            std::fs::write(&txt, text)?;
            let msg = format!("wrote {} and {}", out.display(), txt.display());
            let v = View(vec![Block::Step(true, format!("Rendered the dashboard ({width} x {height}, {})", tab.name())), Block::Step(true, msg.clone())]);
            Ok(Out::new(json!({"html": out, "text": txt, "errors": sn.errors}), msg).view(v))
        }
    }
}

/// `pearl` with no command: the overview box and a hint line of common commands.
/// JSON: the same object as `status`.
pub fn overview_out(ctx: &Ctx) -> Out {
    let sn = snapshot::gather(ctx);
    let view = View::panel(overview(&sn, false)).hint(OVERVIEW_HINT);
    Out::new(sn.to_json(), String::new()).view(view)
}

/// The no-command screen and `status`: wallet, chain, bridge, agent (and with `full`,
/// mining, market and read errors). Every figure is a Snapshot field.
pub fn overview(sn: &Snapshot, full: bool) -> Panel {
    let mut p = Panel::new(&format!("{} overview", brand::NAME)).section("Wallet");
    match &sn.me {
        Some(me) => {
            p = p.kv("Address", me.as_str()).kv("Balance", sn.balance().map(tok).unwrap_or("-".into()));
            p = p.kv("Pending", format!("+{} (not yet K deep)", tok(sn.pending_rewards())));
        }
        None => p = p.kt("Address", "not connected (pass --key FILE)", Tone::Dim),
    }
    if let Some(sm) = &sn.sol_me {
        if sn.me.as_deref() != Some(sm.as_str()) {
            p = p.kv("Solana", sm.as_str());
        }
        let spl = sn.sol.my_spl.map(|x| format!("{} {TICKER} (SPL)", fmt::spl(x as u128))).unwrap_or("-".into());
        let sol = sn.sol.my_sol.map(|x| format!("{} SOL", fmt::sol(x as u128))).unwrap_or("-".into());
        p = p.kv("On Solana", format!("{spl}  ·  {sol}"));
    }

    p = p.section("Chain");
    if sn.chain_ok() {
        p = p
            .state("Node", if sn.info["syncing"] == true { "syncing" } else { "synced" })
            .kv("Height", format!("{}  (final {}, K {})", sn.height(), u(&sn.info, "finalized_height"), sn.finality_k()))
            .kv("Peers", u(&sn.info, "peers").to_string())
            .kv("Jobs", format!("{} posted", sn.jobs_total));
    } else {
        p = p.kt("Node", "OFFLINE", Tone::Bad);
    }

    p = p.section("Bridge");
    let supply = u(&sn.escrow, "total_supply") as u128;
    let outside = u(&sn.escrow, "outside_escrow") as u128;
    if sn.escrow.is_object() {
        let pct = if supply > 0 { format!(" ({:.2}% of supply)", outside as f64 * 100.0 / supply as f64) } else { String::new() };
        p = p.kv("Bridged out", format!("{}{pct}", tok(outside as u64)));
    }
    p = p.kv("Vault", sn.sol.vault_spl.map(|x| format!("{} {TICKER} (SPL)", fmt::spl(x as u128))).unwrap_or("-".into()));
    p = p.kv("In flight", format!("{} deposits, {} withdrawals unreleased", sn.sol.inflight_dep_count, sn.sol.released.iter().filter(|(_, r)| !r).count()));
    p = invariant_row(p, sn);

    p = p.section("Agent");
    p = p.kv(
        "Registration",
        if sn.agent.is_object() { format!("{} since block {}", s(&sn.agent, "name"), u(&sn.agent, "registered_height")) } else { "not registered".into() },
    );
    p = p.kv("Payout lien", if sn.route.is_object() { route_text(&sn.route) } else { "none".into() });
    p = p.state("Agent Credit", &sn.credit);

    if full {
        let (earned, mined) = sn.window_rewards();
        p = p
            .section("Mining")
            .kv("Your blocks", format!("{mined} of the last {}  ({})", sn.blocks.len(), tok(earned)))
            .kv("Job subsidy", sn.job_subsidy().map(|x| format!("up to {} per job block", tok(x))).unwrap_or("-".into()))
            .kv("Mining pool", tok(u(&sn.escrow, "pool_balance")))
            .state("Pool service", if sn.pool.is_some() { "connected" } else { "not connected" })
            .kv("Miner", sn.miner_pid.map(|p| format!("pid {p}")).unwrap_or("not started here".into()));
        p = p.section("Market");
        p = match &sn.sol.store {
            Some(c) => p.kv("Storefront", format!("jobs from {} SOL, {} orders", fmt::sol(c.min_sol_in as u128), c.orders)),
            None => p.kv("Storefront", "-"),
        };
        p = p.kt("Price", "no price source", Tone::Dim);
    }
    if !sn.errors.is_empty() {
        p.rows.push(crate::term::Row::Blank);
        for e in &sn.errors {
            p = p.text(e.clone(), Tone::Bad);
        }
    }
    p
}

fn invariant_word(sn: &Snapshot) -> &'static str {
    match sn.invariant() {
        Some(i) if i.strict => "OK",
        Some(i) if i.ok => "OK (in flight)",
        Some(_) => "BROKEN",
        None => "n/a",
    }
}

fn invariant_row(p: Panel, sn: &Snapshot) -> Panel {
    let w = invariant_word(sn);
    let tone = if w == "n/a" { Tone::Dim } else { status_tone(w) };
    let tone = if w == "OK (in flight)" { Tone::Good } else { tone };
    p.kt("Invariant", w, tone)
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
    let mut p = Panel::new("Balance")
        .kv("Address", a.as_str())
        .kv("Balance", tok(u(&acc, "balance")))
        .kv("Pending", format!("+{} (blocks not yet {k} deep)", tok(pending)))
        .kv("Height", tip.to_string());
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
        p = p.section("Solana");
        if b58(sm) != a {
            p = p.kv("Address", b58(sm));
        }
        match r {
            Ok((sol, spl)) => {
                j["solana"] = json!({"address": b58(sm), "sol_lamports": sol.to_string(), "spl": spl.map(|x| x.to_string())});
                t += &format!("\nsolana {}  {} SOL  {}", short(&b58(sm)), fmt::sol(sol as u128), spl.map(|x| format!("{} {TICKER} (SPL)", fmt::spl(x as u128))).unwrap_or("no bridge mint".into()));
                p = p.kv("SPL", spl.map(|x| format!("{} {TICKER}", fmt::spl(x as u128))).unwrap_or("no bridge mint".into())).kv("SOL", fmt::sol(sol as u128));
            }
            Err(e) => {
                p = p.kt("Error", e.to_string(), Tone::Bad);
                j["solana"] = json!({"error": e.to_string()});
            }
        }
    }
    Ok(Out::new(j, t).view(View::panel(p)))
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

/// Light job table: 12-char id prefix (enough for `job show`), status colored.
pub fn jobs_table(jobs: &[Value]) -> Table {
    Table {
        chip: Some("Jobs".into()),
        head: ["ID", "STATUS", "W", "P", "SHAPE", "BLOCKS", "DEADLINE"].map(String::from).to_vec(),
        right: vec![false, false, true, true, false, true, true],
        rows: jobs
            .iter()
            .map(|j| {
                vec![
                    cell(s(j, "job_id").chars().take(12).collect::<String>()),
                    state_cell(s(j, "status")),
                    cell(tokens(u(j, "worker_pay") as u128)),
                    cell(tokens(u(j, "protocol_fee") as u128)),
                    cell(format!("{}x{}x{}", u(j, "m"), u(j, "n"), u(j, "k"))),
                    cell(format!("{}/{}", u(j, "blocks_mined"), u(j, "max_blocks"))),
                    cell(u(j, "deadline_height").to_string()),
                ]
            })
            .collect(),
        empty: "no jobs yet (pearl job post --pay W --fee P)".into(),
    }
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

/// `job show` box, title chip " Job info ".
pub fn job_panel(j: &Value) -> Panel {
    let opt = |k: &str| -> String { s(j, k).to_string() };
    let closed = u(j, "closed_height");
    Panel::new("Job info")
        .kv("ID", opt("job_id"))
        .state("Status", s(j, "status"))
        .kv("Poster", opt("poster"))
        .kv("Shape", format!("{} x {} x {}", u(j, "m"), u(j, "n"), u(j, "k")))
        .kv("Worker pay", tok(u(j, "worker_pay")))
        .kv("Protocol fee", tok(u(j, "protocol_fee")))
        .kv("Subsidy paid", tok(u(j, "subsidy_paid")))
        .kv("Cap remaining", tok(u(j, "cap_remaining")))
        .kv("Blocks mined", format!("{} of {}", u(j, "blocks_mined"), u(j, "max_blocks")))
        .kv("Posted", format!("block {}", u(j, "posted_height")))
        .kv("Deadline", format!("block {}", u(j, "deadline_height")))
        .kv("Closed", if closed > 0 { format!("block {closed}") } else { "-".into() })
        .kv("Data", opt("data_uri"))
        .kv("Evaluators", format!("{} of {}", u(j, "threshold"), j["evaluators"].as_array().map(|a| a.len()).unwrap_or(0)))
        .kv("Deliverer", opt("deliverer"))
        .kv("Result hash", opt("result_hash"))
        .kv("Result", opt("result_uri"))
}

/// A full 64-hex job id, or the one job whose id starts with `id`.
fn resolve_job_id(ctx: &Ctx, id: &str) -> Result<String> {
    let want = id.trim().to_ascii_lowercase();
    ensure!(!want.is_empty() && want.chars().all(|c| c.is_ascii_hexdigit()), "job id {id:?} is not hex");
    if want.len() == 64 {
        return Ok(want);
    }
    let total = u(&ctx.call("listJobs", json!({"limit": 1}))?, "total");
    let mut hits = vec![];
    let mut off = 0;
    while off < total {
        let page = ctx.call("listJobs", json!({"offset": off, "limit": 200}))?;
        for j in page["jobs"].as_array().into_iter().flatten() {
            if s(j, "job_id").starts_with(&want) {
                hits.push(s(j, "job_id").to_string());
            }
        }
        off += 200;
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => bail!("no job id starts with {want}"),
        n => bail!("{n} jobs start with {want}: give more of the id"),
    }
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

/// Payout lien box (getRoute).
pub fn lien_panel(account: &str, r: &Value) -> Panel {
    let p = Panel::new("Payout lien").kv("Account", account);
    if r.is_null() {
        return p.state("State", "none");
    }
    let state = if r["cleared"] == true {
        "cleared"
    } else if r["active"] == true {
        "active"
    } else {
        "ended"
    };
    let cap = u(r, "cap");
    p.state("State", state)
        .kv("Share", format!("{}% of income", u(r, "route_bps") as f64 / 100.0))
        .kv("Recipient", s(r, "solana_recipient"))
        .kv("Until", format!("block {}", u(r, "until_height")))
        .kv("Cap", if cap == agent_types::TOTAL_SUPPLY { "whole supply".into() } else { tok(cap) })
        .kv("Routed", tok(u(r, "routed")))
        .kv("Withdrawn", tok(u(r, "withdrawn")))
        .kv("Pending", tok(u(r, "pending")))
        .kv("Release key", r["release_key"].as_str().unwrap_or("none"))
        .kv("Set at", format!("block {}", u(r, "set_height")))
        .kv("Route nonce", u(r, "route_nonce").to_string())
}

/// Agent box (getAgent, plus the lien line when `with_route`).
pub fn agent_panel(account: &str, a: &Value, route: &Value, with_route: bool) -> Panel {
    let mut p = Panel::new("Agent").kv("Account", account);
    if a.is_object() {
        p = p
            .state("State", "registered")
            .kv("Name", s(a, "name"))
            .kv("URI", s(a, "uri"))
            .kv("Registered", format!("block {}", u(a, "registered_height")))
            .kv("Updated", format!("block {}", u(a, "updated_height")));
    } else {
        p = p.state("State", "not registered");
    }
    if with_route {
        p = p.kv("Payout lien", route_text(route));
    }
    p
}

fn job_post(ctx: &Ctx, p: &JobCmd) -> Result<Out> {
    let JobCmd::Post { m, n, k, pay, fee, deadline, max_blocks, evaluator, threshold, seed, data, tx_fee } = p else { unreachable!() };
    let ui = &ctx.ui;
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
    let st = ui.step("Preparing job data");
    let (m, n, k, a, b) = match data {
        Some(path) => {
            let d = pearl_job::data::decode_job_blob(&std::fs::read(path).with_context(|| format!("read {}", path.display()))?)?;
            st.ok(format!("Read job blob {} ({} x {} x {})", path.display(), d.manifest.m, d.manifest.n, d.manifest.k));
            (d.manifest.m, d.manifest.n, d.manifest.k, d.a, d.b)
        }
        None => {
            let (a, b) = pearl_job::data::random_matrices(*m as usize, *n as usize, *k as usize, *seed);
            st.ok(format!("Generated {m} x {n} x {k} int7 matrices{}", seed.map(|s| format!(" (seed {s})")).unwrap_or_default()));
            (*m, *n, *k, a, b)
        }
    };
    let config = MiningConfig { common_dim: k, ..MiningConfig::localnet_default() };
    pearl_job::job::check_shape(&config, m as usize, n as usize)?;
    let blob = pearl_job::data::encode_job_blob(m, n, k, &a, &b)?;
    let st = ui.step("Storing the data blob on the node");
    let uri = st.wrap(pearl_node::NodeApi::put_blob(&ctx.node, &blob), |u| format!("Stored data blob {} ({} bytes)", short_n(u, 12), blob.len()))?;

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
    let job_id = hex::encode(commit.job_id);
    ui.done(&format!("Committed to the data: hash_a {}, hash_b {}", short_n(&hex::encode(job.hash_a), 6), short_n(&hex::encode(job.hash_b), 6)));
    let s = send_body(ctx, "PostJob", TxBody::PostJob(job), Some(fee_units), Some(nonce), false)?;
    let mut j = sent_json(&s);
    j["job_id"] = json!(job_id);
    j["data_uri"] = json!(uri);
    j["deadline_height"] = json!(deadline_height);
    j["worker_pay"] = json!(w.to_string());
    j["protocol_fee"] = json!(pf.to_string());
    let text = sent_text(&format!("job {} {m}x{n}x{k} W {} P {}", short(&job_id), tok(w), tok(pf)), &s);
    if !ui.is_plain() {
        return Ok(Out::new(j, text));
    }
    let mut view = View(vec![]);
    if let (true, Some(h)) = (ctx.follow(), s.height) {
        ui.hint(&format!("Job {} is on the chain. Following it to completion (Ctrl-C stops waiting, the job stays).", short_n(&job_id, 8)));
        follow_job(ctx, &job_id, h)?;
    }
    let jv = ctx.call("getJob", json!({"job_id": job_id}))?;
    if jv.is_object() {
        view = view.with(Block::Panel(job_panel(&jv)));
    }
    view = view.hint(format!("tx {}  ·  pearl job show {}", short_n(&s.tx_hash, 8), &job_id[..12]));
    Ok(Out::new(j, text).view(view))
}

/// Follow a posted job through its real events: each block mined on it (scanned from
/// the canonical chain), delivery (result hash), completion by the evaluator.
fn follow_job(ctx: &Ctx, job_id: &str, posted: u64) -> Result<()> {
    let st = ctx.ui.step("Waiting for a miner");
    st.progress(0, 3);
    let mut scanned = posted;
    let mut mined = 0u64;
    let mut delivered = false;
    let t0 = Instant::now();
    let r = (|| loop {
        let j = ctx.call("getJob", json!({"job_id": job_id}))?;
        ensure!(j.is_object(), "job {job_id} not found");
        let tip = u(&ctx.call("getChainInfo", json!({}))?, "height");
        while scanned < tip {
            scanned += 1;
            let b = ctx.call("getBlock", json!({"height": scanned}))?;
            if b["header_fields"]["job_id"].as_str() == Some(job_id) {
                mined += 1;
                st.event(&format!("Mined in block {scanned} by {} (subsidy {})", short(b["header_fields"]["miner"].as_str().unwrap_or("")), tok(u(&b, "subsidy"))));
            }
        }
        let status = s(&j, "status");
        if !delivered && (status == "delivered" || status == "completed") {
            delivered = true;
            st.event(&format!("Delivered by {}, result {}", short(s(&j, "deliverer")), short_n(s(&j, "result_hash"), 6)));
        }
        let stage = match status {
            "completed" => 3,
            "delivered" => 2,
            _ if mined > 0 || u(&j, "blocks_mined") > 0 => 1,
            _ => 0,
        };
        st.progress(stage, 3);
        st.label(match stage {
            0 => format!("Waiting for a miner (deadline block {}, now {tip})", u(&j, "deadline_height")),
            1 => format!("Waiting for delivery ({} of {} blocks mined)", u(&j, "blocks_mined"), u(&j, "max_blocks")),
            _ => "Waiting for the evaluator".to_string(),
        });
        match status {
            "completed" => return Ok(u(&j, "closed_height")),
            "expired" => bail!("job expired at block {}", u(&j, "closed_height").max(u(&j, "deadline_height"))),
            _ => {}
        }
        ensure!(t0.elapsed() < Duration::from_secs(900), "job still {status} after 15 min; pearl job show {}", &job_id[..12]);
        std::thread::sleep(Duration::from_millis(500));
    })();
    st.wrap(r, |h| format!("Completed at block {h}, attested by the evaluator")).map(|_| ())
}

fn bridge(ctx: &Ctx, b: &BridgeCmd) -> Result<Out> {
    let ui = &ctx.ui;
    match b {
        BridgeCmd::In { amount } => {
            let amt = parse_amount(amount, SPL_DP)?;
            ensure!(amt > 0, "amount must be positive");
            let to = ctx.me.ok_or_else(|| anyhow!("no chain account"))?;
            let st = ui.step("Signing the Solana bridge deposit");
            let (tx, nonce) = st.wrap(solana::sign_deposit(&ctx.sol, ctx.sol_key()?, amt, &to), |(_, n)| format!("Signed bridge deposit #{n} ({} {TICKER} SPL)", fmt::spl(amt as u128)))?;
            let st = ui.step("Submitting to Solana");
            let sig = st.wrap(ctx.sol.send(&tx), |s| format!("Submitted Solana tx {}", short_n(s, 8)))?;
            let st = ui.step("Waiting for Solana confirmation");
            let slot = st.wrap(ctx.sol.confirm(&sig, Duration::from_secs(60)), |s| format!("Solana deposit confirmed in slot {s}"))?;
            let mut j = json!({"signature": sig, "deposit_nonce": nonce, "slot": slot, "amount_spl": amt.to_string(), "recipient": b58(&to)});
            let mut t = format!("bridge in {} {TICKER}: Solana deposit {nonce} confirmed (slot {slot})", fmt::spl(amt as u128));
            let mut unlocked: Option<Value> = None;
            if ctx.wait_inclusion() {
                let st = ui.step("Waiting for the relayer to unlock it on the chain");
                st.progress(0, 1);
                let t0 = Instant::now();
                let r = (|| loop {
                    let d = ctx.call("getDeposit", json!({"solana_nonce": nonce}))?;
                    if d["processed"] == true {
                        return Ok(d);
                    }
                    ensure!(t0.elapsed() < Duration::from_secs(180), "deposit {nonce} not unlocked on the chain after 180 s (is the relayer running?)");
                    std::thread::sleep(Duration::from_millis(500));
                })();
                let d = st.wrap(r, |d| format!("Relayer unlocked it on the chain in block {}", u(d, "height")))?;
                t += &format!(", unlocked on the chain at height {}", u(&d, "height"));
                j["unlocked"] = d.clone();
                unlocked = Some(d);
            } else {
                t += "; the relayer unlocks it on the chain";
            }
            if let (true, Some(d)) = (ctx.follow(), &unlocked) {
                let fin = wait_height_final(ctx, u(d, "height"))?;
                if fin {
                    unlocked = ctx.call("getDeposit", json!({"solana_nonce": nonce})).ok();
                }
            }
            let mut p = Panel::new("Bridge in")
                .kv("Signature", sig.as_str())
                .kv("Deposit", format!("#{nonce}"))
                .kv("Amount", format!("{} {TICKER} (SPL) on Solana, {} on the chain", fmt::spl(amt as u128), tok(amt * agent_types::SPL_SCALE)))
                .kv("Recipient", b58(&to))
                .kv("Solana slot", slot.to_string());
            p = match &unlocked {
                Some(d) => p.kv("Unlocked", format!("block {}", u(d, "height"))).state("State", final_word(d)),
                None => p.kv("Unlocked", "-").state("State", "locked"),
            };
            let view = if ui.is_plain() { View::panel(bridge_section(p, &snapshot::gather(ctx))) } else { View::default() };
            Ok(Out::new(j, t).view(view))
        }
        BridgeCmd::Out { amount, to } => {
            let spl_amt = parse_amount(amount, SPL_DP)?;
            ensure!(spl_amt > 0, "amount must be positive");
            let recipient = match to {
                Some(t) => key32(t)?,
                None => ctx.sol_me.ok_or_else(|| anyhow!("no Solana recipient: pass --to or --sol-key"))?,
            };
            let units = spl_amt * agent_types::SPL_SCALE;
            let s = send_body(ctx, "BridgeLock", TxBody::BridgeLock { solana_recipient: recipient, amount: units }, None, None, true)?;
            let mut j = sent_json(&s);
            j["amount"] = json!(units.to_string());
            j["solana_recipient"] = json!(b58(&recipient));
            let text = sent_text(&format!("bridge out {} to {} (released when K deep)", tok(units), short(&b58(&recipient))), &s);
            if !ui.is_plain() {
                return Ok(Out::new(j, text));
            }
            let mut p = tx_panel("Bridge out", &s).kv("Amount", format!("{} on the chain, {} {TICKER} (SPL) on Solana", tok(units), fmt::spl(spl_amt as u128))).kv("Recipient", b58(&recipient));
            let mut released = None;
            if let Some(h) = s.height {
                let st = ui.step("Finding the withdrawal in the log");
                let w = st.wrap(find_withdrawal(ctx, &s.from, &b58(&recipient), spl_amt, h), |w| format!("Withdrawal #{} in the log at block {h}", u(w, "nonce")))?;
                let wn = u(&w, "nonce");
                p = p.kv("Withdrawal", format!("#{wn}"));
                if s.is_final {
                    let st = ui.step("Waiting for the release on Solana");
                    let t0 = Instant::now();
                    let pda = solana::processed_pda(solana::CHAIN_AGENT, wn);
                    // 0 of 1 until the bridge's Processed record for this nonce exists
                    st.progress(0, 1);
                    let r = (|| loop {
                        if ctx.sol.account(&pda)?.is_some() {
                            return Ok(());
                        }
                        ensure!(t0.elapsed() < Duration::from_secs(900), "withdrawal #{wn} not released on Solana after 900 s (relayer / header-relay running?)");
                        std::thread::sleep(Duration::from_millis(700));
                    })();
                    st.wrap(r, |_| format!("Released on Solana to {}", short(&b58(&recipient))))?;
                    released = Some(true);
                } else {
                    released = ctx.sol.account(&solana::processed_pda(solana::CHAIN_AGENT, wn)).ok().map(|a| a.is_some());
                }
            }
            p = tx_tail(p, &s);
            p = match released {
                Some(true) => p.state("Solana", "released"),
                Some(false) => p.state("Solana", "pending"),
                None => p.kt("Solana", "-", Tone::Dim),
            };
            let view = View::panel(bridge_section(p, &snapshot::gather(ctx)));
            Ok(Out::new(j, text).view(view))
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
            let t = match &inv {
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
            let mut p = Panel::new("Bridge status")
                .kv("Escrow", tok(u(&sn.escrow, "escrow_balance")))
                .kv("Mining pool", tok(u(&sn.escrow, "pool_balance")))
                .kv("Deposits", format!("{} in ({} in flight, {} {TICKER})", sn.deposit_count, sn.sol.inflight_dep_count, sn.sol.inflight_dep_spl.map(|x| fmt::spl(x as u128)).unwrap_or("-".into())))
                .kv("Withdrawals", format!("{} out ({} unreleased, {} {TICKER})", sn.withdrawal_count, sn.sol.released.iter().filter(|(_, r)| !r).count(), fmt::spl(sn.inflight_wd_spl() as u128)));
            if let Some(c) = &sn.sol.bridge {
                p = p.kv("Release path", match c.release_modes {
                    1 => "signers".to_string(),
                    2 => "light-client proof".to_string(),
                    3 => "signers or light-client proof".to_string(),
                    m => format!("mode {m}"),
                });
            }
            for e in &sn.errors {
                p = p.text(e.clone(), Tone::Bad);
            }
            Ok(Out::new(j, t).view(View::panel(bridge_section(p, &sn)).hint("vault SPL x 1000 == 1e18 - escrow, less transfers in flight")))
        }
    }
}

/// `pearl buyback`: storefront Config buyback fields + vault balance + recent events.
fn buyback(ctx: &Ctx, limit: usize) -> Result<Out> {
    let d = ctx.sol.account(&solana::storefront_config_pda())?.ok_or_else(|| anyhow!("storefront config not found on Solana ({})", ctx.sol.url))?;
    let b = solana::Buyback::parse(&d)?;
    let vault = solana::buyback_vault_pda();
    let lamports = ctx.sol.balance(&vault)?;
    let available = lamports.saturating_sub(890_880).saturating_sub(b.locked);
    let supply = ctx.sol.account(&b.mint)?.map(|m| solana::mint_supply(&m)).transpose()?;
    let dest_account = if b.dest == 0 { solana::reserve_ata(&b.reserve_program, &b.mint) } else { b.mint };
    // events: newest txs touching the vault (orders pay into it, buys spend from it)
    let mut buys = vec![];
    let mut accrued = 0usize;
    let mut accrued_lamports = 0u64;
    let sigs = ctx.sol.call("getSignaturesForAddress", json!([b58(&vault), {"limit": 100, "commitment": "confirmed"}]))?;
    for sg in sigs.as_array().into_iter().flatten() {
        if buys.len() >= limit {
            break;
        }
        if !sg["err"].is_null() {
            continue;
        }
        let sig = sg["signature"].as_str().unwrap_or_default();
        let tx = ctx.sol.call("getTransaction", json!([sig, {"commitment": "confirmed", "encoding": "json", "maxSupportedTransactionVersion": 0}]))?;
        for l in tx["meta"]["logMessages"].as_array().into_iter().flatten().filter_map(|l| l.as_str()) {
            let Some(p) = l.strip_prefix("Program data: ") else { continue };
            let Ok(raw) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, p) else { continue };
            match solana::decode_store_event(&raw) {
                Some(solana::StoreEvent::Executed(e)) => buys.push((sig.to_string(), sg["slot"].as_u64().unwrap_or(0), sg["blockTime"].as_i64(), e)),
                Some(solana::StoreEvent::Accrued(a)) => {
                    accrued += 1;
                    accrued_lamports += a.lamports;
                }
                None => {}
            }
        }
    }
    let dest_label = |d: u8| if d == 0 { "reserve" } else if d == 1 { "burn" } else { "unknown" };
    let jb: Vec<Value> = buys
        .iter()
        .map(|(sig, slot, t, e)| {
            json!({"signature": sig, "slot": slot, "block_time": t, "buy": e.buy, "venue": if e.venue == 1 { "pumpswap" } else { "curve" },
                   "sol_in_lamports": e.sol_in.to_string(), "tokens_out_spl": e.tokens_out.to_string(), "routed_spl": e.routed.to_string(),
                   "destination": dest_label(e.destination), "destination_account": b58(&e.destination_account),
                   "orders": e.orders, "first_nonce": if e.orders > 0 { Some(e.first_nonce) } else { None }, "last_nonce": if e.orders > 0 { Some(e.last_nonce) } else { None },
                   "available_before_lamports": e.available_before.to_string(), "carry_lamports": e.carry.to_string()})
        })
        .collect();
    let zero = [0u8; 32];
    let j = json!({
        "vault": b58(&vault), "vault_lamports": lamports.to_string(), "available_lamports": available.to_string(), "held_lamports": b.locked.to_string(),
        "buyback_bps": b.bps, "destination": b.dest_name(), "destination_account": b58(&dest_account), "reserve_program": b58(&b.reserve_program),
        "cranker": if b.cranker == zero { None } else { Some(b58(&b.cranker)) },
        "max_per_call_lamports": b.max_per_call.to_string(), "min_buy_lamports": b.min_buy.to_string(), "min_interval_secs": b.min_interval_secs, "last_buy_ts": b.last_ts,
        "totals": {"refunded_lamports": b.refunded.to_string(), "accrued_lamports": b.accrued.to_string(), "spent_lamports": b.spent.to_string(), "tokens_bought_spl": b.tokens.to_string(),
                   "to_reserve_spl": b.to_reserve.to_string(), "burned_spl": b.burned.to_string(), "buys": b.buys},
        "pending": {"orders": b.pending_orders, "first_nonce": if b.pending_orders > 0 { Some(b.pending_first_nonce) } else { None },
                    "last_nonce": if b.pending_orders > 0 { Some(b.pending_last_nonce) } else { None }},
        "mint_supply_spl": supply.map(|x| x.to_string()),
        "recent_buys": jb, "recent_accruals_seen": accrued, "recent_accrued_lamports": accrued_lamports.to_string(),
    });
    let t = format!(
        "buyback {} bps -> {}  vault {} SOL  bought {} {TICKER} for {} SOL in {} buys (reserve {}, burned {})  pending orders {}",
        b.bps,
        b.dest_name(),
        fmt::sol(available as u128),
        fmt::spl(b.tokens as u128),
        fmt::sol(b.spent as u128),
        b.buys,
        fmt::spl(b.to_reserve as u128),
        fmt::spl(b.burned as u128),
        b.pending_orders
    );
    let mut p = Panel::new("Buyback")
        .kv("Share of order SOL", format!("{:.2}% ({} bps)", b.bps as f64 / 100.0, b.bps))
        .kv("Destination", if b.dest == 0 { format!("mining reserve {}", short(&b58(&dest_account))) } else { "burn".to_string() })
        .kv("Vault", format!("{} SOL to spend ({})", fmt::sol(available as u128), short(&b58(&vault))))
        .kv("Held for open orders", format!("{} SOL (refunded if the order is)", fmt::sol(b.locked as u128)))
        .kv("Cranker", if b.cranker == zero { "anyone".to_string() } else { short(&b58(&b.cranker)) })
        .kv("Per call", if b.max_per_call == 0 { "no cap".to_string() } else { format!("{} SOL max", fmt::sol(b.max_per_call as u128)) })
        .section("Totals")
        .kv("Shares released", format!("{} SOL (finalized orders)", fmt::sol(b.accrued as u128)))
        .kv("Shares refunded", format!("{} SOL", fmt::sol(b.refunded as u128)))
        .kv("Spent on buys", format!("{} SOL in {} buys", fmt::sol(b.spent as u128), b.buys))
        .kv("Bought", format!("{} {TICKER}", fmt::spl(b.tokens as u128)))
        .kv("To the reserve", format!("{} {TICKER}", fmt::spl(b.to_reserve as u128)))
        .kv("Burned", format!("{} {TICKER}", fmt::spl(b.burned as u128)));
    if let Some(sp) = supply {
        p = p.kv("Mint supply", format!("{} {TICKER}", fmt::spl(sp as u128)));
    }
    p = p.kv(
        "Pending orders",
        if b.pending_orders == 0 { "none".to_string() } else { format!("{} (nonces {}..{})", b.pending_orders, b.pending_first_nonce, b.pending_last_nonce) },
    );
    let table = Table {
        chip: Some("Latest buys".into()),
        head: ["#", "venue", "SOL in", "TOKEN out", "to", "orders"].iter().map(|x| x.replace("TOKEN", TICKER)).collect(),
        right: vec![true, false, true, true, false, false],
        rows: buys
            .iter()
            .map(|(_, _, _, e)| {
                vec![
                    (e.buy.to_string(), Tone::Plain),
                    ((if e.venue == 1 { "pumpswap" } else { "curve" }).to_string(), Tone::Plain),
                    (fmt::sol(e.sol_in as u128), Tone::Plain),
                    (fmt::spl(e.tokens_out as u128), Tone::Good),
                    (dest_label(e.destination).to_string(), Tone::Plain),
                    (if e.orders == 0 { "carry only".to_string() } else { format!("{} ({}..{})", e.orders, e.first_nonce, e.last_nonce) }, Tone::Plain),
                ]
            })
            .collect(),
        empty: "no buys yet".into(),
    };
    Ok(Out::new(j, t).view(View::panel(p).with(Block::Table(table)).hint("every figure is a Solana read: storefront config, vault balance, BuybackExecuted events")))
}

/// The invariant section every bridge box ends with.
fn bridge_section(p: Panel, sn: &Snapshot) -> Panel {
    let mut p = p.section("Invariant");
    p = p.kv("Vault", sn.sol.vault_spl.map(|x| format!("{} {TICKER} (SPL)", fmt::spl(x as u128))).unwrap_or("-".into()));
    p = p.kv("Outside escrow", tok(u(&sn.escrow, "outside_escrow")));
    if let Some(i) = sn.invariant() {
        p = p.kv("Delta", format!("{} units ({} in flight)", i.delta, i.in_flight));
    }
    invariant_row(p, sn)
}

/// The withdrawal log entry a BridgeLock included at `height` wrote.
fn find_withdrawal(ctx: &Ctx, from: &str, recipient: &str, amount_spl: u64, height: u64) -> Result<Value> {
    let count = u(&ctx.call("getBridgeEvents", json!({"limit": 1}))?, "withdrawal_count");
    let from_nonce = count.saturating_sub(snapshot::BRIDGE_WINDOW);
    let v = ctx.call("getBridgeEvents", json!({"from_nonce": from_nonce, "limit": snapshot::BRIDGE_WINDOW}))?;
    v["withdrawals"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|w| u(w, "height") == height && s(w, "from") == from && s(w, "solana_recipient") == recipient && u(w, "amount_spl") == amount_spl)
        .cloned()
        .ok_or_else(|| anyhow!("no withdrawal entry for this BridgeLock at block {height}"))
}

/// Progress until block `h` is K deep; false if text mode is off.
fn wait_height_final(ctx: &Ctx, h: u64) -> Result<bool> {
    let st = ctx.ui.step("Waiting for finality");
    let t0 = Instant::now();
    let mut k = 0;
    let r = (|| loop {
        let info = ctx.call("getChainInfo", json!({}))?;
        k = u(&info, "finality_k");
        let depth = u(&info, "height").saturating_sub(h).min(k);
        st.label(format!("Finality {depth}/{k} blocks deep"));
        st.progress(depth, k);
        if u(&info, "finalized_height") >= h {
            return Ok(true);
        }
        ensure!(t0.elapsed() < Duration::from_secs(300), "block {h} not final after 300 s");
        std::thread::sleep(Duration::from_millis(400));
    })();
    st.wrap(r, |_| format!("Final, {k} blocks deep (block {h})"))
}

fn lien(ctx: &Ctx, l: &LienCmd) -> Result<Out> {
    match l {
        LienCmd::Show => {
            let a = me(ctx)?;
            let r = ctx.call("getRoute", json!({"account": a}))?;
            let t = route_text(&r);
            let view = View::panel(lien_panel(&a, &r));
            Ok(Out::new(r, t).view(view))
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
            let s = send_body(ctx, "SetRoute", body, None, None, true)?;
            let view = lien_after(ctx, "SetRoute", &s, &s.from.clone())?;
            Ok(Out::new(sent_json(&s), sent_text(&format!("lien set {}% to {}", *bps as f64 / 100.0, short(recipient)), &s)).view(view))
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
            ctx.ui.done(&format!("Release key signed the clear of route nonce {route_nonce}"));
            let s = send_body(ctx, "ClearRoute", TxBody::ClearRoute { account, route_nonce, sig }, None, None, true)?;
            let view = lien_after(ctx, "ClearRoute", &s, &b58(&account))?;
            Ok(Out::new(sent_json(&s), sent_text(&format!("lien clear for {}", short(&b58(&account))), &s)).view(view))
        }
    }
}

fn lien_after(ctx: &Ctx, what: &str, s: &Sent, account: &str) -> Result<View> {
    let mut v = View::panel(tx_tail(tx_panel(what, s), s));
    if ctx.ui.is_plain() {
        let r = ctx.call("getRoute", json!({"account": account}))?;
        v = v.with(Block::Panel(lien_panel(account, &r)));
    }
    Ok(v)
}

fn miner_bin(ctx: &Ctx) -> Result<PathBuf> {
    if let Some(p) = &ctx.opts.miner_bin {
        return Ok(p.clone());
    }
    let exe = std::env::current_exe()?;
    for anc in exe.ancestors().skip(1).take(6) {
        for c in [anc.join("pearl-miner"), anc.join("target/release/pearl-miner"), anc.join("crates/target/release/pearl-miner")] {
            if c.is_file() {
                return Ok(c);
            }
        }
    }
    bail!("pearl-miner not found (build it: cd crates && cargo build --release -p pearl-miner, or pass --miner-bin)")
}

/// One pearl-miner JSON log line, as text mode shows it.
#[derive(Debug, PartialEq, Eq)]
pub enum MinerLine {
    /// `✓` line.
    Good(String),
    /// `✗` line.
    Bad(String),
    /// Update the live line.
    Status(String),
    Ignore,
}

pub fn miner_line(v: &Value) -> MinerLine {
    let job = |v: &Value| match v["job"].as_str() {
        Some(j) => format!("job {}", short_n(j, 6)),
        None => "fallback".into(),
    };
    let err = |v: &Value| s(v, "error").to_string();
    match s(v, "event") {
        "start" => MinerLine::Good(format!("Miner started, {} threads, payout {}", u(v, "threads"), short(s(v, "miner")))),
        "block_accepted" => MinerLine::Good(format!("Block {} found ({}), {}", u(v, "height"), job(v), short_n(s(v, "block_hash"), 6))),
        "pool_block" if v["accepted"] == true => MinerLine::Good(format!("Pool block {} found, {}", u(v, "height"), short_n(s(v, "hash"), 6))),
        "pool_block" => MinerLine::Bad(format!("Pool block {} rejected: {}", u(v, "height"), s(v, "reason"))),
        "block_rejected" => MinerLine::Bad(format!("Block {} rejected ({}): {}", u(v, "height"), job(v), s(v, "reason"))),
        "job_loaded" => MinerLine::Good(format!("Loaded {} ({} x {})", job(v), u(v, "m"), u(v, "n"))),
        "result_submitted" => MinerLine::Good(format!("Delivered {} (result {})", job(v), short_n(s(v, "result_hash"), 6))),
        "complete_sent" => MinerLine::Good(format!("Completion sent for {} by attester {}", job(v), short(s(v, "attester")))),
        "completed" => MinerLine::Good(format!("{} completed", job(v))),
        "stats" => MinerLine::Status(format!("Mining at height {}, {:.0} nonces/s", u(v, "height"), v["nonces_per_s"].as_f64().unwrap_or(0.0))),
        "mine_error" | "step_failed" | "template_failed" | "deliver_failed" | "submit_result_failed" | "complete_failed" | "result_blob_failed" => {
            MinerLine::Bad(format!("{}: {}", s(v, "event").replace('_', " "), err(v)))
        }
        "job_unusable" => MinerLine::Bad(format!("{} unusable: {}", job(v), err(v))),
        _ => MinerLine::Ignore,
    }
}

fn mine(ctx: &Ctx, m: &MineCmd, shell: bool) -> Result<Out> {
    match m {
        MineCmd::Start { threads } => {
            let key = ctx.opts.key.clone().ok_or_else(|| anyhow!("mine start needs --key (the payout key file)"))?;
            let bin = miner_bin(ctx)?;
            let mut cmd = std::process::Command::new(&bin);
            cmd.args(["--rpc", &ctx.opts.rpc, "--threads", &threads.to_string()]).arg("--payout").arg(&key);
            if !shell && !ctx.ui.is_plain() {
                // --json: foreground, its JSON log lines on our stdout
                let st = cmd.status().with_context(|| format!("run {}", bin.display()))?;
                return Ok(Out::new(json!({"exit": st.code()}), format!("pearl-miner exited: {st}")));
            }
            if !shell {
                return mine_foreground(ctx, cmd, &bin, *threads);
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
            let p = Panel::new("Mining")
                .kv("Your blocks", format!("{mined} of the last {}", sn.blocks.len()))
                .kv("Rewards", tok(earned))
                .kv("Pending", format!("+{} (not yet K deep)", tok(sn.pending_rewards())))
                .kv("Job subsidy", sn.job_subsidy().map(|x| format!("up to {} per job block", tok(x))).unwrap_or("-".into()))
                .state("Pool service", if sn.pool.is_some() { "connected" } else { "not connected" })
                .kv("Height", sn.height().to_string());
            Ok(Out::new(j, t).view(View::panel(p)))
        }
    }
}

/// Text mode `mine start`: run pearl-miner, one line per block found, until it exits
/// (Ctrl-C reaches it too: same process group).
fn mine_foreground(ctx: &Ctx, mut cmd: std::process::Command, bin: &std::path::Path, threads: usize) -> Result<Out> {
    use std::io::BufRead;
    cmd.stdout(std::process::Stdio::piped()).stdin(std::process::Stdio::null());
    let mut child = cmd.spawn().with_context(|| format!("run {}", bin.display()))?;
    let pid = child.id();
    ctx.ui.hint(&format!("pearl-miner pid {pid}, {threads} threads, payout {}. Ctrl-C stops it.", ctx.me.as_ref().map(b58).map(|a| short(&a)).unwrap_or_default()));
    let st = ctx.ui.step("Mining");
    let (mut found, mut rejected) = (0u64, 0u64);
    let out = child.stdout.take().ok_or_else(|| anyhow!("no miner stdout"))?;
    for line in std::io::BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match miner_line(&v) {
            MinerLine::Good(t) => {
                if s(&v, "event") == "block_accepted" || (s(&v, "event") == "pool_block" && v["accepted"] == true) {
                    found += 1;
                }
                st.event(&t)
            }
            MinerLine::Bad(t) => {
                if s(&v, "event").ends_with("rejected") || s(&v, "event") == "pool_block" {
                    rejected += 1;
                }
                st.event_bad(&t)
            }
            MinerLine::Status(t) => st.label(t),
            MinerLine::Ignore => {}
        }
    }
    let status = child.wait()?;
    st.ok(format!("pearl-miner stopped ({status})"));
    let p = Panel::new("Mining")
        .kv("Miner", format!("pid {pid}, {threads} threads"))
        .kv("Blocks found", found.to_string())
        .kv("Rejected", rejected.to_string())
        .kv("Exit", status.to_string());
    Ok(Out::new(json!({"exit": status.code(), "blocks_found": found, "rejected": rejected}), format!("pearl-miner exited: {status}")).view(View::panel(p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Style;

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
        assert!(matches!(parse_line("dashboard").unwrap(), Cmd::Dashboard));
        assert!(matches!(parse_line("tui").unwrap(), Cmd::Dashboard));
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
        assert!(job_detail(&j).contains("worker_pay      5 LODE"));
        assert_eq!(route_text(&Value::Null), "none");
        let r = json!({"active": true, "route_bps": 5000, "solana_recipient": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                       "until_height": 99, "routed": "1000000500", "withdrawn": "1000000000", "pending": "500"});
        assert_eq!(route_text(&r), "active 50% to 7xKX...gAsU until 99 (routed 1 LODE, withdrawn 1 LODE, pending 0 LODE)");
        assert!(HELP.lines().all(|l| l.len() <= 90));
    }

    fn sample_job(status: &str) -> Value {
        json!({"job_id": "ab".repeat(32), "status": status, "poster": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "worker_pay": "5000000000",
               "protocol_fee": "1000000000", "m": 64, "n": 64, "k": 2048, "blocks_mined": 1, "max_blocks": 10, "deadline_height": 900,
               "posted_height": 400, "closed_height": 0, "evaluators": ["x"], "threshold": 1, "data_uri": "blob:00ff", "deliverer": null,
               "result_hash": null, "result_uri": "", "cap_remaining": "790000000", "subsidy_paid": "10000000"})
    }

    #[test]
    fn job_views() {
        let p = job_panel(&sample_job("completed"));
        assert_eq!(p.chip.as_deref(), Some("Job info"));
        let plain = crate::term::render_panel(&p, &Style::plain());
        assert!(plain.contains("[ Job info ]"));
        assert!(plain.contains("Status           COMPLETED"), "{plain}");
        assert!(plain.contains("Worker pay       5 LODE"));
        assert!(plain.contains("Deliverer        -"));
        let col = crate::term::render_panel(&p, &Style::colored());
        assert!(col.contains("\x1b[32mCOMPLETED\x1b[0m"));
        let t = jobs_table(&[sample_job("open"), sample_job("expired")]);
        let r = crate::term::render_table(&t, &Style::colored());
        assert!(r.contains("\x1b[33mopen\x1b[0m") && r.contains("\x1b[31mexpired\x1b[0m"));
        assert!(r.contains("abababababab"), "12-char id prefix");
        assert!(crate::term::render_table(&jobs_table(&[]), &Style::plain()).contains("no jobs yet"));
    }

    #[test]
    fn lien_and_agent_views() {
        let r = json!({"active": true, "cleared": false, "route_bps": 2500, "solana_recipient": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
                       "until_height": 99, "cap": agent_types::TOTAL_SUPPLY.to_string(), "routed": "0", "withdrawn": "0", "pending": "0",
                       "release_key": null, "set_height": 5, "route_nonce": 0});
        let s = crate::term::render_panel(&lien_panel("me", &r), &Style::plain());
        assert!(s.contains("ACTIVE") && s.contains("25% of income") && s.contains("whole supply") && s.contains("Release key    none"), "{s}");
        assert!(crate::term::render_panel(&lien_panel("me", &Value::Null), &Style::plain()).contains("NONE"));
        let a = crate::term::render_panel(&agent_panel("me", &Value::Null, &Value::Null, true), &Style::plain());
        assert!(a.contains("NOT REGISTERED") && a.contains("Payout lien    none"), "{a}");
    }

    #[test]
    fn overview_from_sample() {
        let sn = crate::snapshot::sample::snapshot();
        let p = overview(&sn, false);
        let s = crate::term::render_panel(&p, &Style::plain());
        for want in ["Wallet", "Chain", "Bridge", "Agent", "Balance", "Invariant", "Height"] {
            assert!(s.contains(want), "{want} missing in\n{s}");
        }
        assert!(!s.contains("Mining") && !s.contains("Market"));
        let full = crate::term::render_panel(&overview(&sn, true), &Style::plain());
        assert!(full.contains("Mining") && full.contains("no price source"));
        assert!(!full.contains('\u{2014}') && !OVERVIEW_HINT.contains('\u{2014}'));
        let mut off = sn.clone();
        off.info = Value::Null;
        assert!(crate::term::render_panel(&overview(&off, false), &Style::plain()).contains("OFFLINE"));
    }

    #[test]
    fn sent_states() {
        let mut s = Sent { tx_hash: "aa".repeat(32), nonce: 3, fee: 1000, from: "me".into(), height: None, is_final: false };
        assert_eq!(s.state(), "pending");
        s.height = Some(9);
        assert_eq!(s.state(), "included");
        s.is_final = true;
        let p = tx_tail(tx_panel("Transfer", &s), &s);
        let r = crate::term::render_panel(&p, &Style::colored());
        assert!(r.contains("\x1b[32mFINAL\x1b[0m") && r.contains(&"aa".repeat(32)));
    }

    #[test]
    fn miner_lines() {
        let v = json!({"event": "block_accepted", "height": 812, "block_hash": "ab".repeat(32), "job": null});
        assert_eq!(miner_line(&v), MinerLine::Good("Block 812 found (fallback), abab...abab".replace("abab...abab", &short_n(&"ab".repeat(32), 6))));
        let v = json!({"event": "block_rejected", "height": 9, "job": "cd".repeat(32), "reason": "stale"});
        assert!(matches!(miner_line(&v), MinerLine::Bad(t) if t.contains("rejected") && t.contains("stale")));
        assert!(matches!(miner_line(&json!({"event": "stats", "height": 5, "nonces_per_s": 1234.4})), MinerLine::Status(t) if t == "Mining at height 5, 1234 nonces/s"));
        assert_eq!(miner_line(&json!({"event": "whatever"})), MinerLine::Ignore);
    }

    #[test]
    fn buyback_read_from_config_vault_and_events() {
        let url = crate::stub::serve();
        let c = crate::stub::ctx(&url, false);
        let o = crate::stub::run(&c, "buyback --limit 5");
        let j = &o.json;
        assert_eq!(j["buyback_bps"], 1000);
        assert_eq!(j["destination"], "reserve");
        let reserve = solana::pk("4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p");
        assert_eq!(j["destination_account"], b58(&solana::reserve_ata(&reserve, &solana::pk(crate::stub::OTHER))));
        assert_eq!(j["vault"], b58(&solana::buyback_vault_pda()));
        assert_eq!(j["available_lamports"], "50000000", "held shares are not counted");
        assert_eq!(j["held_lamports"], "20000000");
        assert_eq!(j["totals"]["refunded_lamports"], "5000000");
        assert_eq!(j["cranker"], Value::Null);
        assert_eq!(j["totals"]["accrued_lamports"], "300000000");
        assert_eq!(j["totals"]["spent_lamports"], "250000000");
        assert_eq!(j["totals"]["to_reserve_spl"], "9000000000");
        assert_eq!(j["totals"]["buys"], 1);
        assert_eq!(j["pending"]["orders"], 1);
        assert_eq!(j["pending"]["first_nonce"], 12);
        assert_eq!(j["mint_supply_spl"], "999000000000000");
        let b = &j["recent_buys"][0];
        assert_eq!(b["signature"], "5sig");
        assert_eq!(b["sol_in_lamports"], "250000000");
        assert_eq!(b["tokens_out_spl"], "9000000000");
        assert_eq!(b["destination"], "reserve");
        assert_eq!(b["orders"], 3);
        assert_eq!(b["first_nonce"], 3);
        assert_eq!(b["last_nonce"], 9);
        assert_eq!(b["carry_lamports"], "50000000");
        assert!(o.text.contains("1000 bps -> reserve"), "{}", o.text);
        let t = crate::stub::ctx(&url, true);
        let plain = crate::stub::run(&t, "buyback").view.unwrap().render(&Style::plain());
        assert!(plain.contains("Latest buys ]") && plain.contains("mining reserve") && plain.contains("3 (3..9)"), "{plain}");
        // a config from before the buyback fields reads as share 0
        let old = &crate::stub::store_config_bytes()[..8 + 32 * 3 + 1 + 2 + 2 + 8 + 8 + 4 + 4 + 8 + 1 + 4 + 32 + 24 + 3 + 32];
        assert_eq!(solana::Buyback::parse(old).unwrap().bps, 0);
    }

    #[test]
    fn every_command_output_path_text_and_json() {
        let url = crate::stub::serve();
        let text = crate::stub::ctx(&url, true);
        let json_ctx = crate::stub::ctx(&url, false);
        let id12 = "ab".repeat(6);
        let cases: Vec<(String, &str)> = vec![
            ("balance".into(), "Balance"),
            ("address".into(), "Address"),
            ("status".into(), "overview"),
            ("jobs".into(), "Jobs"),
            (format!("job show {id12}"), "Job info"),
            ("blocks 3".into(), "Blocks"),
            ("activity".into(), "Activity"),
            ("agent".into(), "Agent"),
            ("lien show".into(), "Payout lien"),
            (format!("tx {}", "ef".repeat(32)), "Transaction"),
            ("bridge status".into(), "Bridge status"),
            ("mine status".into(), "Mining"),
            (format!("send {} 1.5", crate::stub::OTHER), "Transfer"),
            ("register demo-agent https://x".into(), "RegisterAgent"),
            (format!("lien set {} 2500", crate::stub::OTHER), "SetRoute"),
            ("buyback".into(), "Buyback"),
        ];
        for (line, chip) in &cases {
            let out = crate::stub::run(&text, line);
            let v = out.view.unwrap_or_else(|| panic!("{line}: no view"));
            let plain = v.render(&Style::plain());
            assert!(plain.contains(&format!("{chip} ]")), "{line}: chip {chip} missing in\n{plain}");
            assert!(!plain.contains('\x1b') && !plain.contains('\u{2014}'), "{line}");
            let col = v.render(&Style::colored());
            assert!(col.contains("\x1b[30;107m ") && col.contains(&format!("{chip} \x1b[0m")), "{line}: colored chip");
            // JSON mode: same command, no view printed, JSON object as before
            let j = crate::stub::run(&json_ctx, line);
            assert!(j.json.is_object() || j.json.is_null(), "{line}");
        }
        // text mode follows the send to K-deep final; JSON mode returns at submit
        let t = crate::stub::run(&text, &format!("send {} 1", crate::stub::OTHER)).view.unwrap().render(&Style::plain());
        assert!(t.contains("FINAL") && t.contains("Block     10"), "{t}");
        let j = crate::stub::run(&json_ctx, &format!("send {} 1", crate::stub::OTHER)).json;
        assert_eq!(j["included_height"], Value::Null);
        assert!(j.get("final").is_none(), "JSON shape unchanged");
        // job show by prefix resolves to the full id
        assert_eq!(crate::stub::run(&json_ctx, "job show abab").json["job_id"], json!("ab".repeat(32)));
        // overview: the no-command screen
        let o = overview_out(&text).view.unwrap().render(&Style::plain());
        assert!(o.contains("Wallet") && o.contains("pearl dashboard") && !o.contains("Market"), "{o}");
    }

}
