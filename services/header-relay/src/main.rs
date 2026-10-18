//! header-relay: keeps the Solana light client (token/programs/lightclient) in step
//! with the agent chain. Permissionless (any funded Solana key) and resumable: all
//! progress lives on Solana (header accounts, tip, canonical ring), so a restart just
//! picks up at the light client's tip. See README.md.
//!
//!   header-relay init     --node URL --solana URL --payer KEY [--height H]
//!   header-relay run      --node URL --solana URL --payer KEY [--work KEYFILE] [--once]
//!   header-relay seed     --node URL   (devnet only: unlock, lock, post + mine a job block)
//!   header-relay mine     --node URL N (devnet only: N fallback blocks)
//!   header-relay harvest  --node URL --out FILE (headers, proofs, txs, withdrawal proofs)

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer as _, SigningKey};
use lightclient::client;
use pearl_node::tx::{self, Body};
use pearl_node::NodeApi;
use serde_json::{json, Value};
use solana_sdk::signature::{read_keypair_file, write_keypair_file, Keypair};

use header_relay::chain::{self, Node};
use header_relay::relay::{Opts, Relay};
use header_relay::settle;
use header_relay::sol::Sol;

#[derive(Parser)]
struct Cli {
    /// Agent-chain node RPC.
    #[arg(long, global = true, default_value = "http://127.0.0.1:10630")]
    node: String,
    /// Solana RPC (token-chain localnet by default).
    #[arg(long, global = true, default_value = "http://127.0.0.1:10600")]
    solana: String,
    /// Solana fee payer (relayer key).
    #[arg(long, global = true, default_value = "keys/localnet/payer-keypair.json")]
    payer: PathBuf,
    /// Priority fee, micro-lamports per CU.
    #[arg(long, global = true, default_value_t = 0)]
    cu_price: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize the light client from a node header (default: the node's final header).
    Init {
        #[arg(long)]
        height: Option<u64>,
        #[arg(long, default_value_t = 0)]
        relay_reward: u64,
        #[arg(long, default_value_t = 64)]
        retain_final: u64,
    },
    /// Follow the node and relay every canonical header (pipelined: see src/relay.rs).
    Run {
        /// Work account keypair of slot 0 (created on first use); slot i uses the same
        /// path with `-i` before the extension.
        #[arg(long, default_value = ".header-relay/work-keypair.json")]
        work: PathBuf,
        /// Idle poll interval when caught up (busy passes poll every 25 ms).
        #[arg(long, default_value_t = 2000)]
        poll_ms: u64,
        /// Headers verified at the same time, one work account (about 0.49 SOL rent,
        /// refundable with close-works) each. 1 = the old one-at-a-time relay.
        #[arg(long, default_value_t = 8)]
        parallel: usize,
        /// Ask for 1.4M CU on every tx instead of the planned amount.
        #[arg(long)]
        full_cu: bool,
        /// Seconds between lag log lines.
        #[arg(long, default_value_t = 10)]
        lag_secs: u64,
        /// Relay what is there, then exit.
        #[arg(long)]
        once: bool,
        /// Also settle by proof: bridge release_proof for final withdrawals and storefront
        /// finalize/expire_order_proof for closed orders, when those programs have proof
        /// mode on (src/settle.rs).
        #[arg(long)]
        settle: bool,
    },
    /// Devnet seeding: BridgeUnlock to a user, BridgeLock back, PostJob, mine a job block.
    Seed {
        #[arg(long, default_value = "keys/localnet/attester-keypair.json")]
        attester: PathBuf,
        /// Solana recipient of the seeded withdrawal.
        #[arg(long)]
        withdraw_to: Option<String>,
    },
    /// Light-client state as one JSON line (tip, final, floor, and the lag behind the
    /// node when it answers; initialized false if none).
    Status,
    /// Close this relay's work accounts (rent back to the payer).
    CloseWorks {
        #[arg(long, default_value = ".header-relay/work-keypair.json")]
        work: PathBuf,
        #[arg(long, default_value_t = 8)]
        parallel: usize,
    },
    /// Devnet: mine N fallback blocks via getBlockTemplate / submitBlock.
    Mine { n: u64 },
    /// R5: accumulator proof of the final header at HEIGHT (short form, current size) as one
    /// JSON line; `--restore` also re-creates its light-client record if it was closed.
    AccProof {
        height: u64,
        #[arg(long)]
        restore: bool,
    },
    /// Dump a header chain with proofs, txs and final withdrawal proofs as JSON.
    Harvest {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        to: Option<u64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let node = Node::new(&cli.node);
    match cli.cmd {
        Cmd::Init { height, relay_reward, retain_final } => {
            let mut s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            init(&node, &mut s, height, relay_reward, retain_final)
        }
        Cmd::Run { work, poll_ms, once, settle, parallel, full_cu, lag_secs } => {
            let mut s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            let works = (0..parallel.max(1)).map(|i| load_or_create_key(&work_path(&work, i))).collect::<Result<Vec<_>>>()?;
            let settle_node = Node::new(&cli.node);
            let mut r = Relay::new(node, &mut s, works, Opts { parallel, exact_cu: !full_cu });
            r.ensure_works()?;
            let mut st = settle::Settle::new();
            let (mut last_settle, mut last_lag) = (std::time::Instant::now() - Duration::from_secs(60), std::time::Instant::now());
            let (mut lag_verified, mut lag_t) = (0u64, std::time::Instant::now());
            loop {
                let busy = match r.pass() {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("relay: {e:#}");
                        true
                    }
                };
                if settle && last_settle.elapsed() > Duration::from_secs(2) {
                    last_settle = std::time::Instant::now();
                    if let Err(e) = st.tick(&settle_node, r.ledger) {
                        eprintln!("settle: {e:#}");
                    }
                }
                if last_lag.elapsed() > Duration::from_secs(lag_secs) {
                    let per_min = (r.stats.verified - lag_verified) as f64 * 60.0 / lag_t.elapsed().as_secs_f64();
                    println!("lag {}", r.lag_line(per_min));
                    (lag_verified, lag_t, last_lag) = (r.stats.verified, std::time::Instant::now(), std::time::Instant::now());
                }
                if once && !busy {
                    let c = r.config()?;
                    println!("tip {} {} final_floor {} final {} (txs {})", c.tip_height, hex::encode(c.tip_hash), c.final_floor, c.final_height, r.stats.txs);
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(if busy { 25 } else { poll_ms }));
            }
        }
        Cmd::CloseWorks { work, parallel } => {
            let mut s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            for i in 0..parallel.max(1) {
                let p = work_path(&work, i);
                if !p.exists() {
                    continue;
                }
                let w = read_key(&p)?;
                if s.account(&solana_sdk::signer::Signer::pubkey(&w))?.is_none() {
                    continue;
                }
                s.send(&[client::close_work(s.pk(), solana_sdk::signer::Signer::pubkey(&w))], &[])?;
                println!("closed work account {}", p.display());
            }
            Ok(())
        }
        Cmd::Status => {
            let s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            match s.config() {
                Ok(c) => {
                    let acc = s.account(&client::acc_addr())?.and_then(|d| client::acc_state(&d));
                    let node_h = node.info().ok().and_then(|i| chain::num(&i["height"]).ok());
                    println!("{}", json!({"initialized": true, "tip_height": c.tip_height, "tip_hash": hex::encode(c.tip_hash),
                        "final_height": c.final_height, "final_floor": c.final_floor, "checkpoint_height": c.checkpoint_height,
                        "node_height": node_h, "lag_blocks": node_h.map(|h| h.saturating_sub(c.tip_height)),
                        "accumulator": acc.map(|(st, n, last)| json!({"start_height": st, "count": n, "next_height": st + n, "last_hash": hex::encode(last)}))}))
                }
                Err(_) => println!("{}", json!({"initialized": false})),
            }
            Ok(())
        }
        Cmd::Seed { attester, withdraw_to } => seed(&node, &attester, withdraw_to),
        Cmd::Mine { n } => {
            let miner = [0x4du8; 32];
            for _ in 0..n {
                mine_fallback(&node, &miner)?;
            }
            Ok(())
        }
        Cmd::Harvest { out, to } => harvest(&node, &out, to),
        Cmd::AccProof { height, restore } => {
            let mut s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            acc_proof(&node, &mut s, height, restore)
        }
    }
}

fn sha_parts(parts: &[&[u8]]) -> [u8; 32] {
    chain::sha256(parts)
}

/// Short-form accumulator proof of the node's canonical header at `height`, against the
/// light client's current accumulator (leaf hashes rebuilt from the node's headers).
fn acc_proof(node: &Node, s: &mut Sol, height: u64, restore: bool) -> Result<()> {
    let d = s.account(&client::acc_addr())?.context("no accumulator (header-relay init / run starts one)")?;
    let (start, count, last) = client::acc_state(&d).context("accumulator layout")?;
    if height < start || height >= start + count {
        anyhow::bail!("height {height} is not in the accumulator (heights {start}..{})", start + count);
    }
    let mut hashes = Vec::with_capacity(count as usize);
    for h in start..start + count {
        hashes.push(node.header_at(h)?.context("node header")?.hash);
    }
    if *hashes.last().unwrap() != last {
        anyhow::bail!("the node's chain does not match the accumulator's last hash");
    }
    let hd = node.header_at(height)?.context("node header")?;
    let proof = pearl_verify_sbf::acc::prove_short(sha_parts, &hashes, height - start).context("proof")?;
    let mut restored = false;
    if restore && s.account(&client::header_addr(&hd.hash))?.is_none() {
        s.send(&[client::restore_header(s.pk(), hd.bytes, count, proof.clone())], &[])?;
        restored = true;
    }
    println!("{}", json!({"height": height, "hash": hex::encode(hd.hash), "header": hex::encode(hd.bytes), "count": count,
        "start_height": start, "proof": hex::encode(&proof), "form": "short (siblings to the current peak)", "restored": restored}));
    Ok(())
}

/// Work keypair file of slot `i`: the given path for 0, `<stem>-<i>.<ext>` otherwise.
fn work_path(p: &PathBuf, i: usize) -> PathBuf {
    if i == 0 {
        return p.clone();
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = match p.extension() {
        Some(e) => format!("{stem}-{i}.{}", e.to_string_lossy()),
        None => format!("{stem}-{i}"),
    };
    p.with_file_name(name)
}

fn read_key(p: &PathBuf) -> Result<Keypair> {
    read_keypair_file(p).map_err(|e| anyhow!("read {}: {e}", p.display()))
}

fn load_or_create_key(p: &PathBuf) -> Result<Keypair> {
    if p.exists() {
        return read_key(p);
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let k = Keypair::new();
    write_keypair_file(&k, p).map_err(|e| anyhow!("write {}: {e}", p.display()))?;
    Ok(k)
}

// ---------------------------------------------------------------- init

fn init(node: &Node, s: &mut Sol, height: Option<u64>, relay_reward: u64, retain_final: u64) -> Result<()> {
    let info = node.info()?;
    let h = height.unwrap_or(chain::num(&info["finalized_height"]).unwrap_or(0));
    let cp = node.header_at(h)?.ok_or_else(|| anyhow!("no header at {h}"))?;
    let mut ts = vec![cp.timestamp_ms()];
    let mut cur = h;
    while ts.len() < 11 && cur > 0 {
        cur -= 1;
        ts.push(node.header_at(cur)?.context("ancestor")?.timestamp_ms());
    }
    let cw = chain::u256_hex_le(cp.cum_work.as_deref().context("cumulative_work")?)?;
    let p = chain::lc_params(node, relay_reward, retain_final)?;
    s.send(&[client::initialize(s.pk(), p, cp.bytes, cw, ts)], &[])?;
    // R5: the final-header accumulator starts at the checkpoint (admin = this key).
    s.send(&[client::init_accumulator(s.pk())], &[])?;
    println!("light client initialized at height {h} ({}), accumulator started", hex::encode(cp.hash));
    Ok(())
}

// ---------------------------------------------------------------- devnet seeding

fn seed_of(path: &PathBuf) -> Result<SigningKey> {
    let v: Vec<u8> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    Ok(SigningKey::from_bytes(&v[..32].try_into()?))
}

fn send_signed(node: &Node, key: &SigningKey, fee: u64, body: Body) -> Result<String> {
    let info = node.info()?;
    let from = key.verifying_key().to_bytes();
    let acct = node.rpc.get_account(&from)?;
    let t = tx::Tx { chain_id: chain::num(&info["chain_id"])? as u32, from, nonce: acct.nonce, fee, body };
    let wire = tx::sign(&t, key)?;
    node.rpc.send_tx(&wire)
}

fn mine_fallback(node: &Node, miner: &[u8; 32]) -> Result<[u8; 32]> {
    let t = node.rpc.fallback_template(miner)?;
    let b = pearl_job::pow::mine_header(&t.header, &pearl_job::BlockWork::Fallback(t.fallback), 0..u64::MAX)?.context("no nonce")?;
    match node.rpc.submit_block(&t, &b)? {
        pearl_node::SubmitOutcome::Accepted { block_hash } => Ok(block_hash),
        pearl_node::SubmitOutcome::Rejected { reason } => bail!("block rejected: {reason}"),
    }
}

fn wait_included(node: &Node, tx_hash: &str, miner: &[u8; 32]) -> Result<u64> {
    for _ in 0..20 {
        mine_fallback(node, miner)?;
        let v = node.rpc.call("getTx", json!({"tx_hash": tx_hash}))?;
        if v["status"] == "included" {
            return chain::num(&v["height"]);
        }
    }
    bail!("tx {tx_hash} not included")
}

fn seed(node: &Node, attester_path: &PathBuf, withdraw_to: Option<String>) -> Result<()> {
    use rand::{Rng, SeedableRng};
    let miner = [0x4du8; 32];
    let attester = seed_of(attester_path)?;
    let user = SigningKey::from_bytes(&[0x5au8; 32]);
    let upk = user.verifying_key().to_bytes();
    let min_fee = chain::num(&node.params()?["min_fee"])?;
    // 1. BridgeUnlock 10 tokens (10e6 SPL units) to the user.
    let sol_nonce: u64 = 900_000 + rand::rng().random_range(0..100_000);
    let amount_spl = 10_000_000u64;
    let mut msg = b"pearl-sol:deposit:v1".to_vec();
    msg.extend_from_slice(&sol_nonce.to_le_bytes());
    msg.extend_from_slice(&upk);
    msg.extend_from_slice(&amount_spl.to_le_bytes());
    let sig = attester.sign(&msg).to_bytes();
    let mut body = sol_nonce.to_le_bytes().to_vec();
    body.extend_from_slice(&upk);
    body.extend_from_slice(&amount_spl.to_le_bytes());
    body.push(0);
    body.push(1);
    body.extend_from_slice(&attester.verifying_key().to_bytes());
    body.extend_from_slice(&sig);
    let h = send_signed(node, &attester, 0, Body::Raw { kind: tx::kind::BRIDGE_UNLOCK, body })?;
    println!("unlock at {}", wait_included(node, &h, &miner)?);
    // 2. BridgeLock 2 tokens back to a Solana recipient.
    let to: [u8; 32] = match withdraw_to {
        Some(s) => bs58::decode(s).into_vec()?.try_into().map_err(|_| anyhow!("bad pubkey"))?,
        None => [0x77; 32],
    };
    let mut body = to.to_vec();
    body.extend_from_slice(&2_000_000_000u64.to_le_bytes());
    let h = send_signed(node, &user, min_fee, Body::Raw { kind: tx::kind::BRIDGE_LOCK, body: body.clone() })?;
    println!("lock at {}", wait_included(node, &h, &miner)?);
    // A second lock in a later block, so the withdraw_root changes between them (the
    // fixture then holds a historical proof against the older root).
    let h = send_signed(node, &user, min_fee, Body::Raw { kind: tx::kind::BRIDGE_LOCK, body })?;
    println!("lock 2 at {}", wait_included(node, &h, &miner)?);
    // 3. PostJob with real matrices, then mine one block on it.
    let mut rng = rand::rngs::StdRng::seed_from_u64(11);
    let (m, n) = (64u32, 64u32);
    let cfg = pearl_job::MiningConfig::localnet_default();
    let k = cfg.common_dim;
    let a: Vec<i8> = (0..m as usize * k as usize).map(|_| rng.random_range(-64i8..=64)).collect();
    let b: Vec<i8> = (0..k as usize * n as usize).map(|_| rng.random_range(-64i8..=64)).collect();
    let nonce = node.rpc.get_account(&upk)?.nonce;
    let height = chain::num(&node.info()?["height"])?;
    let (w, p, deadline, max_blocks) = (1_000_000_000u64, 1_000_000_000u64, height + 1000, 5u32);
    let job_id = pv_hash_parts(&[&upk, &nonce.to_le_bytes()]);
    let mut t = b"pearl-sol:terms:v2".to_vec();
    t.extend_from_slice(&upk);
    t.extend_from_slice(&job_id);
    for x in [m, n, k] {
        t.extend_from_slice(&x.to_le_bytes());
    }
    for x in [w, p, deadline] {
        t.extend_from_slice(&x.to_le_bytes());
    }
    t.extend_from_slice(&max_blocks.to_le_bytes());
    let terms = pv_hash_parts(&[&t]);
    let jh = pearl_job::JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
    let prep = pearl_job::PreparedJob::with_computed_commitments(pearl_job::Job { header: jh, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] }, &a, &b)?;
    let mut body = Vec::new();
    for x in [m, n, k] {
        body.extend_from_slice(&x.to_le_bytes());
    }
    body.extend_from_slice(&cfg.to_bytes());
    body.extend_from_slice(&prep.job.hash_a);
    body.extend_from_slice(&prep.job.hash_b);
    // Job data as a PSD1 blob on this node (the node refuses malformed blob URIs).
    let mut psd = b"PSD1".to_vec();
    for x in [m, n, k] {
        psd.extend_from_slice(&x.to_le_bytes());
    }
    psd.extend(a.iter().map(|&x| x as u8));
    psd.extend(b.iter().map(|&x| x as u8));
    let put = node.rpc.call("putBlob", json!({"data_hex": hex::encode(&psd)}))?;
    let uri_s = put["uri"].as_str().context("putBlob uri")?.to_string();
    let uri = uri_s.as_bytes();
    body.extend_from_slice(&(uri.len() as u16).to_le_bytes());
    body.extend_from_slice(uri);
    for x in [w, p, deadline] {
        body.extend_from_slice(&x.to_le_bytes());
    }
    body.extend_from_slice(&max_blocks.to_le_bytes());
    body.push(1);
    body.extend_from_slice(&upk);
    body.push(1);
    let h = send_signed(node, &user, min_fee, Body::Raw { kind: tx::kind::POST_JOB, body })?;
    println!("job {} posted at {}", hex::encode(job_id), wait_included(node, &h, &miner)?);
    let v = node.rpc.call("getBlockTemplate", json!({"miner": bs58::encode(miner).into_string(), "job_id": hex::encode(job_id)}))?;
    let tpl = pearl_node::Template::from_json(&v)?;
    let mined = pearl_job::pow::mine_header(&tpl.header, &pearl_job::BlockWork::Job(&prep), 0..u64::MAX)?.context("no nonce")?;
    match node.rpc.submit_block(&tpl, &mined)? {
        pearl_node::SubmitOutcome::Accepted { block_hash } => println!("job block {} at {}", hex::encode(block_hash), mined.header.height),
        pearl_node::SubmitOutcome::Rejected { reason } => bail!("job block rejected: {reason}"),
    }
    // 4. Bury everything K deep.
    let k = chain::num(&node.params()?["finality_k"])?;
    for _ in 0..k + 2 {
        mine_fallback(node, &miner)?;
    }
    println!("seeded; tip {}", chain::num(&node.info()?["height"])?);
    Ok(())
}

fn pv_hash_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p);
    }
    *h.finalize().as_bytes()
}

// ---------------------------------------------------------------- harvest

fn harvest(node: &Node, out: &PathBuf, to: Option<u64>) -> Result<()> {
    let info = node.info()?;
    let tip = to.unwrap_or(chain::num(&info["height"])?);
    let mut headers = vec![];
    for h in 0..=tip {
        let hd = node.header_at(h)?.context("header")?;
        let (txs, cb) = node.block_body(&hd.hash)?;
        headers.push(json!({
            "coinbase": cb.as_ref().map(hex::encode),
            "height": h,
            "hash": hex::encode(hd.hash),
            "header": hex::encode(hd.bytes),
            "proof": chain::b64e(&hd.proof),
            "cumulative_work": hd.cum_work,
            "txs": txs.iter().map(|t| chain::b64e(t)).collect::<Vec<_>>(),
        }));
    }
    let ev = node.rpc.call("getBridgeEvents", json!({"from_nonce": 0, "limit": 100}))?;
    let mut withdrawals = vec![];
    let mut withdrawals_at = vec![];
    for w in ev["withdrawals"].as_array().cloned().unwrap_or_default() {
        let n = chain::num(&w["nonce"])?;
        let p = node.withdrawal_proof(n)?;
        if p["final"] == true {
            withdrawals.push(p);
            // Historical proof against the root of the withdrawal's own block (getWithdrawalProof
            // at_height), older than the current root once a later block adds withdrawals.
            let at = chain::num(&w["height"])?;
            let h = node.withdrawal_proof_at(n, at)?;
            withdrawals_at.push(h);
        }
    }
    let p = chain::lc_params(node, 10_000, 4)?;
    let doc: Value = json!({
        "chain_id": p.chain_id,
        "params": {
            "finality_k": p.finality_k, "target_block_time_ms": p.target_block_time_ms, "half_life_ms": p.half_life_ms,
            "pow_limit_bits": p.pow_limit_bits, "min_difficulty_bits": p.min_difficulty_bits, "max_future_ms": p.max_future_ms,
            "fallback_m": p.fallback_m, "fallback_n": p.fallback_n, "fallback_config": hex::encode(p.fallback_config),
        },
        "headers": headers,
        "withdrawals": withdrawals,
        "withdrawals_at_own_block": withdrawals_at,
    });
    std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
    println!("wrote {} headers, {} final withdrawals to {}", tip + 1, doc["withdrawals"].as_array().unwrap().len(), out.display());
    Ok(())
}
