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

mod chain;
mod sol;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signer as _, SigningKey};
use lightclient::client::{self, Step};
use pearl_node::tx::{self, Body};
use pearl_node::NodeApi;
use serde_json::{json, Value};
use solana_sdk::signature::{read_keypair_file, write_keypair_file, Keypair, Signer};
use solana_sdk::system_instruction;

use chain::{ChainHeader, Node};
use sol::Sol;

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
    /// Follow the node and relay every canonical header.
    Run {
        /// Work account keypair (created on first use).
        #[arg(long, default_value = ".header-relay/work-keypair.json")]
        work: PathBuf,
        #[arg(long, default_value_t = 2000)]
        poll_ms: u64,
        /// Relay what is there, then exit.
        #[arg(long)]
        once: bool,
    },
    /// Devnet seeding: BridgeUnlock to a user, BridgeLock back, PostJob, mine a job block.
    Seed {
        #[arg(long, default_value = "keys/localnet/attester-keypair.json")]
        attester: PathBuf,
        /// Solana recipient of the seeded withdrawal.
        #[arg(long)]
        withdraw_to: Option<String>,
    },
    /// Devnet: mine N fallback blocks via getBlockTemplate / submitBlock.
    Mine { n: u64 },
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
        Cmd::Run { work, poll_ms, once } => {
            let mut s = Sol::new(&cli.solana, read_key(&cli.payer)?, cli.cu_price);
            let work = load_or_create_key(&work)?;
            let mut r = Relay { node, sol: &mut s, work, closed_upto: 0 };
            r.ensure_work()?;
            loop {
                match r.step() {
                    Ok(n) if n > 0 => continue,
                    Ok(_) => {}
                    Err(e) => eprintln!("relay: {e:#}"),
                }
                if once {
                    let c = r.sol.config()?;
                    println!("tip {} {} final_floor {} final {} (txs {})", c.tip_height, hex::encode(c.tip_hash), c.final_floor, c.final_height, r.sol.txs);
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(poll_ms));
            }
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
    }
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
    println!("light client initialized at height {h} ({})", hex::encode(cp.hash));
    Ok(())
}

// ---------------------------------------------------------------- relay

struct Relay<'a> {
    node: Node,
    sol: &'a mut Sol,
    work: Keypair,
    closed_upto: u64,
}

impl Relay<'_> {
    fn ensure_work(&mut self) -> Result<()> {
        let size = client::work_size_for(&self.sol.config()?.params);
        if self.sol.account(&self.work.pubkey())?.is_none() {
            let rent = self.sol.rpc.get_minimum_balance_for_rent_exemption(size)?;
            let create = system_instruction::create_account(&self.sol.pk(), &self.work.pubkey(), rent, size as u64, &lightclient::ID);
            let w = self.work.insecure_clone();
            self.sol.send(&[create, client::init_work(self.sol.pk(), self.work.pubkey(), 32 * 1024)], &[&w])?;
        }
        Ok(())
    }

    /// One pass: relay up to the node tip, reorg if needed, mark final, close old.
    /// Returns the number of headers relayed.
    fn step(&mut self) -> Result<u64> {
        let cfg = self.sol.config()?;
        let canon = self.sol.canon()?;
        let node_tip = chain::num(&self.node.info()?["height"])?;
        // Fork point: highest height <= our tip where the node's canonical header is ours.
        let mut start = cfg.tip_height;
        loop {
            let nh = self.node.header_at(start)?.context("node header")?;
            if canon[(start as usize) % lightclient::CANON_LEN] == nh.hash || start <= cfg.final_floor {
                break;
            }
            start -= 1;
        }
        let mut relayed = 0;
        for h in start + 1..=node_tip {
            let Some(hd) = self.node.header_at(h)? else { break };
            self.ensure_verified(&hd)?;
            relayed += 1;
            if relayed >= 16 {
                break; // yield so finality and closing keep up
            }
        }
        self.maybe_reorg()?;
        self.mark_finals()?;
        self.close_old()?;
        Ok(relayed)
    }

    fn ensure_verified(&mut self, hd: &ChainHeader) -> Result<()> {
        match self.sol.header_state(&hd.hash)? {
            Some((lightclient::STATUS_VERIFIED, _, _)) => return Ok(()),
            Some(_) => {}
            None => self.sol.send(&[client::submit_header(self.sol.pk(), hd.bytes)], &[])?,
        }
        let job = if hd.is_fallback() { None } else { Some(hd.job_id()) };
        // the mining config the block is verified against (sizes the step plan)
        let mut job_config = None;
        if let Some(j) = job {
            let Some(data) = self.sol.account(&client::job_addr(&j))? else {
                bail!("job block {} at {} needs job {} registered (posted before the checkpoint?)", hex::encode(hd.hash), hd.height, hex::encode(j));
            };
            let jc = <lightclient::JobCommit as anchor_lang::AccountDeserialize>::try_deserialize(&mut data.as_slice())?;
            job_config = Some(jc.config);
        }
        let (pk, w) = (self.sol.pk(), self.work.pubkey());
        let first = client::FIRST_CHUNK.min(hd.proof.len());
        self.sol.send(
            &[client::begin_verify(pk, w, &hd.hash, job.as_ref(), hd.proof.len() as u32), client::write_proof(pk, w, 0, hd.proof[..first].to_vec())],
            &[],
        )?;
        let mut off = first;
        while off < hd.proof.len() {
            let n = client::WRITE_CHUNK.min(hd.proof.len() - off);
            self.sol.send(&[client::write_proof(pk, w, off as u32, hd.proof[off..off + n].to_vec())], &[])?;
            off += n;
        }
        let c = self.sol.config()?;
        let p = c.params;
        let shape = client::Shape::new(hd.is_fallback(), p.fallback_m, p.fallback_n, &job_config.unwrap_or(p.fallback_config), hd.proof.len());
        for tx in client::plan(&shape, client::TX_CU_BUDGET) {
            let ixs: Vec<_> = tx.iter().map(|s: &Step| s.instruction(pk, w, &hd.hash, pk)).collect();
            self.sol.send(&ixs, &[])?;
        }
        println!("verified {} {}", hd.height, hex::encode(hd.hash));
        self.register_jobs(hd)?;
        Ok(())
    }

    fn register_jobs(&mut self, hd: &ChainHeader) -> Result<()> {
        let txs = self.node.block_txs(&hd.hash)?;
        let jobs = chain::jobs_in(&txs);
        if jobs.is_empty() {
            return Ok(());
        }
        let (_, paths) = chain::tx_root_paths(&txs);
        for (job_id, i) in jobs {
            if self.sol.account(&client::job_addr(&job_id))?.is_some() {
                continue;
            }
            self.sol.send(&[client::register_job(self.sol.pk(), &hd.hash, job_id, txs[i].clone(), paths[i].clone())], &[])?;
            println!("registered job {}", hex::encode(job_id));
        }
        Ok(())
    }

    /// If the node's tip is a verified header heavier than ours on another branch.
    fn maybe_reorg(&mut self) -> Result<()> {
        let cfg = self.sol.config()?;
        let node_tip = self.node.header_at(chain::num(&self.node.info()?["height"])?)?.context("tip")?;
        if node_tip.hash == cfg.tip_hash || !matches!(self.sol.header_state(&node_tip.hash)?, Some((lightclient::STATUS_VERIFIED, _, _))) {
            return Ok(());
        }
        let canon = self.sol.canon()?;
        let mut path = vec![];
        let mut cur = node_tip.prev();
        let mut h = node_tip.height - 1;
        loop {
            path.push(cur);
            if canon[(h as usize) % lightclient::CANON_LEN] == cur || h == 0 || path.len() > 60 {
                break;
            }
            let hd = self.node.header_by_hash(&cur)?.context("ancestor")?;
            cur = hd.prev();
            h -= 1;
        }
        self.sol.send(&[client::reorg(&node_tip.hash, &path)], &[])?;
        println!("reorg to {} (path {})", hex::encode(node_tip.hash), path.len());
        Ok(())
    }

    fn mark_finals(&mut self) -> Result<()> {
        let cfg = self.sol.config()?;
        let canon = self.sol.canon()?;
        let lo = cfg.final_height.max(cfg.tip_height.saturating_sub(lightclient::CANON_LEN as u64 - 1)) + 1;
        for h in lo..=cfg.final_floor {
            let hash = canon[(h as usize) % lightclient::CANON_LEN];
            if matches!(self.sol.header_state(&hash)?, Some((_, false, _))) {
                self.sol.send(&[client::mark_final(&hash)], &[])?;
            }
        }
        Ok(())
    }

    /// Close final headers we paid for once they are `retain_final` below the floor.
    fn close_old(&mut self) -> Result<()> {
        let cfg = self.sol.config()?;
        let upto = cfg.final_floor.saturating_sub(cfg.params.retain_final + 1);
        let from = self.closed_upto.max(cfg.checkpoint_height + 1);
        for h in from..upto {
            let Some(hd) = self.node.header_at(h)? else { break };
            if let Some((_, true, relayer)) = self.sol.header_state(&hd.hash)? {
                if relayer == self.sol.pk() && hd.hash != cfg.final_hash {
                    self.sol.send(&[client::close_header(&hd.hash, relayer)], &[])?;
                }
            }
        }
        self.closed_upto = self.closed_upto.max(upto);
        Ok(())
    }
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
    let h = send_signed(node, &user, min_fee, Body::Raw { kind: tx::kind::BRIDGE_LOCK, body })?;
    println!("lock at {}", wait_included(node, &h, &miner)?);
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
        let txs = node.block_txs(&hd.hash)?;
        headers.push(json!({
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
    for w in ev["withdrawals"].as_array().cloned().unwrap_or_default() {
        let n = chain::num(&w["nonce"])?;
        let p = node.withdrawal_proof(n)?;
        if p["final"] == true {
            withdrawals.push(p);
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
    });
    std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
    println!("wrote {} headers, {} final withdrawals to {}", tip + 1, doc["withdrawals"].as_array().unwrap().len(), out.display());
    Ok(())
}
