//! pearl-miner (M1, CPU only).
//!
//! Loop: list Open jobs on the agent chain -> fetch A, B from the job's data URI ->
//! mine shares over nonces [0, max_nonce) with the miner-bound seed -> POST each to
//! the attester -> send ed25519-precompile + mining::submit_share. When a job's
//! nonces are done (and --deliver is on): compute C = AB, write it to --results-dir,
//! jobs::submit_result, ask the attester for a Result signature, send jobs::complete.
//! Closed epochs with shares are claimed with mining::claim.
//!
//! `--job-file spec.json` skips the chain: mines that job and only talks to the
//! attester (attester must run with --mode request). For testing without programs.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use clap::Parser;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_chain::accounts::JobStatus;
use pearl_chain::ix::Programs;
use pearl_chain::key::pubkey_of;
use pearl_chain::tx::build_signed;
use pearl_chain::{read_keypair, Pubkey, Rpc};
use pearl_job::freivalds::result_hash;
use pearl_job::{data, JobSpec, PlainProof, PreparedJob};

#[derive(Parser)]
#[command(name = "pearl-miner")]
struct Args {
    #[arg(long, default_value = pearl_chain::DEFAULT_RPC)]
    rpc: String,
    /// Miner keypair (Solana JSON); also pays fees. Localnet: keys/localnet/miner-keypair.json
    #[arg(long, default_value = "keys/localnet/miner-keypair.json")]
    keypair: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:10661")]
    attester: String,
    #[arg(long, default_value = pearl_chain::JOBS_PROGRAM)]
    jobs_program: String,
    #[arg(long, default_value = pearl_chain::MINING_PROGRAM)]
    mining_program: String,
    /// Mine at most this many nonces per job (capped by the chain's max_nonce).
    #[arg(long)]
    nonce_limit: Option<u64>,
    #[arg(long, default_value_t = default_threads())]
    threads: usize,
    /// Deliver C = AB after mining a job.
    #[arg(long)]
    deliver: bool,
    #[arg(long, default_value = "results")]
    results_dir: PathBuf,
    #[arg(long, default_value_t = 2000)]
    poll_ms: u64,
    /// One pass over the Open jobs, then exit.
    #[arg(long)]
    once: bool,
    /// Mine one JobSpec file without the chain (attester in request mode).
    #[arg(long)]
    job_file: Option<PathBuf>,
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8)
}

fn log(v: Value) {
    println!("{v}");
}

/// Mine all nonces in [0, limit) across threads; returns shares ordered by nonce.
fn mine_all(prep: &PreparedJob, miner: &[u8; 32], limit: u64, target: u32, threads: usize) -> Vec<PlainProof> {
    let threads = threads.max(1) as u64;
    let mut out: Vec<PlainProof> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|t| s.spawn(move || (t..limit).step_by(threads as usize).filter_map(|n| prep.mine_nonce(miner, n, target)).collect::<Vec<_>>()))
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    out.sort_by_key(|p| p.nonce);
    out
}

struct Attest {
    agent: ureq::Agent,
    url: String,
}

impl Attest {
    fn post(&self, path: &str, body: Value) -> Result<Value> {
        let r = self.agent.post(&format!("{}{path}", self.url)).send_json(body);
        let v: Value = match r {
            Ok(resp) => resp.into_json()?,
            Err(ureq::Error::Status(_, resp)) => resp.into_json()?,
            Err(e) => bail!("attester {path}: {e}"),
        };
        if v["ok"].as_bool() != Some(true) {
            bail!("attester {path} refused: {}", v["error"]);
        }
        Ok(v)
    }
}

fn sig_of(v: &Value) -> Result<([u8; 32], [u8; 64])> {
    let att = Pubkey::from_b58(v["attester"].as_str().ok_or_else(|| anyhow!("no attester"))?)?;
    let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap_or(""))?
        .try_into()
        .map_err(|_| anyhow!("signature is not 64 bytes"))?;
    Ok((att.0, sig))
}

struct Miner {
    args: Args,
    key: SigningKey,
    rpc: Rpc,
    programs: Programs,
    attest: Attest,
    done: HashSet<Pubkey>,
    /// (epoch, job) with accepted shares, to claim once the epoch closes.
    to_claim: BTreeSet<(u64, Pubkey)>,
}

impl Miner {
    fn me(&self) -> Pubkey {
        pubkey_of(&self.key)
    }

    fn mine_spec(&self, spec: &JobSpec, limit: u64) -> Result<(PreparedJob, Vec<PlainProof>)> {
        let d = data::load(&spec.data_uri).with_context(|| format!("load data {}", spec.data_uri))?;
        let prep = PreparedJob::new(spec.to_job()?, &d.a, &d.b)?;
        let t0 = Instant::now();
        let shares = mine_all(&prep, &self.me().0, limit, spec.target_bits, self.args.threads);
        log(json!({"event": "mined", "job": spec.job, "nonces": limit, "shares": shares.len(),
                   "elapsed_ms": t0.elapsed().as_secs_f64() * 1e3}));
        Ok((prep, shares))
    }

    fn attest_share(&self, spec: &JobSpec, p: &PlainProof, with_spec: bool) -> Result<Value> {
        let mut body = json!({"job": spec.job, "miner": self.me().to_b58(),
                              "proof_b64": base64::engine::general_purpose::STANDARD.encode(p.to_bytes())});
        if with_spec {
            body["job_spec"] = serde_json::to_value(spec)?;
        }
        self.attest.post("/share", body)
    }

    /// --job-file mode: mine and attest only.
    fn run_offline(&self, path: &PathBuf) -> Result<()> {
        let spec: JobSpec = serde_json::from_slice(&std::fs::read(path)?)?;
        let limit = self.args.nonce_limit.unwrap_or(if spec.max_nonce > 0 { spec.max_nonce } else { 64 });
        let (prep, shares) = self.mine_spec(&spec, limit)?;
        for p in &shares {
            let v = self.attest_share(&spec, p, true)?;
            log(json!({"event": "attested", "job": spec.job, "nonce": p.nonce, "signature": v["signature_hex"]}));
        }
        if self.args.deliver {
            let c = prep.product();
            std::fs::create_dir_all(&self.args.results_dir)?;
            let cp = std::fs::canonicalize(&self.args.results_dir)?.join(format!("{}.i32", spec.job));
            data::write_i32(&cp, &c)?;
            let v = self.attest.post("/result", json!({"job": spec.job, "deliverer": self.me().to_b58(),
                "result_uri": format!("file://{}", cp.display()), "job_spec": spec}))?;
            log(json!({"event": "result_attested", "job": spec.job, "result_hash": v["result_hash"]}));
        }
        Ok(())
    }

    fn process_job(&mut self, job: &pearl_chain::accounts::JobAccount, max_nonce: u64) -> Result<()> {
        let spec = job.to_spec(max_nonce)?;
        let limit = self.args.nonce_limit.map_or(max_nonce, |l| l.min(max_nonce));
        let (prep, shares) = self.mine_spec(&spec, limit)?;
        for p in &shares {
            let v = match self.attest_share(&spec, p, false) {
                Ok(v) => v,
                Err(e) => {
                    log(json!({"event": "attest_failed", "job": spec.job, "nonce": p.nonce, "error": format!("{e:#}")}));
                    continue;
                }
            };
            let (att, sig) = sig_of(&v)?;
            match self.programs.send_share(&self.rpc, &self.key, job.address, p.nonce, &att, &sig) {
                Ok(tx) => {
                    let epoch = self.programs.get_mining_config(&self.rpc)?.current_epoch;
                    self.to_claim.insert((epoch, job.address));
                    log(json!({"event": "share_submitted", "job": spec.job, "nonce": p.nonce, "epoch": epoch, "tx": tx}));
                }
                Err(e) => log(json!({"event": "share_failed", "job": spec.job, "nonce": p.nonce, "error": format!("{e:#}")})),
            }
        }
        if self.args.deliver {
            self.deliver(job, &prep)?;
        }
        Ok(())
    }

    fn deliver(&self, job: &pearl_chain::accounts::JobAccount, prep: &PreparedJob) -> Result<()> {
        let c = prep.product();
        let rh = result_hash(&prep.job_key, &c);
        std::fs::create_dir_all(&self.args.results_dir)?;
        let cp = std::fs::canonicalize(&self.args.results_dir)?.join(format!("{}.i32", job.address));
        data::write_i32(&cp, &c)?;
        let uri = format!("file://{}", cp.display());
        let current = self.programs.get_job(&self.rpc, &job.address)?;
        if current.status == JobStatus::Open {
            let tx = self.programs.send_submit_result(&self.rpc, &self.key, job.address, rh, &uri)?;
            log(json!({"event": "result_submitted", "job": job.address.to_b58(), "result_hash": hex::encode(rh), "tx": tx}));
        } else if current.deliverer != self.me() {
            log(json!({"event": "deliver_skipped", "job": job.address.to_b58(), "reason": format!("job is {:?}", current.status)}));
            return Ok(());
        }
        let v = self.attest.post("/result", json!({"job": job.address.to_b58(), "deliverer": self.me().to_b58(), "result_uri": uri}))?;
        let (att, sig) = sig_of(&v)?;
        let tx = self.programs.send_complete(&self.rpc, &self.key, job.address, self.me(), &rh, &att, &sig)?;
        log(json!({"event": "completed", "job": job.address.to_b58(), "tx": tx}));
        Ok(())
    }

    fn claim_closed(&mut self) -> Result<()> {
        let current = self.programs.get_mining_config(&self.rpc)?.current_epoch;
        let slot = self.rpc.get_slot()?;
        let ready: Vec<(u64, Pubkey)> = self
            .to_claim
            .iter()
            .copied()
            .filter(|(e, _)| *e < current || self.programs.get_epoch(&self.rpc, *e).map(|ep| slot >= ep.end_slot).unwrap_or(false))
            .collect();
        for (epoch, job) in ready {
            match self.send_claim(epoch, job) {
                Ok(tx) => log(json!({"event": "claimed", "epoch": epoch, "job": job.to_b58(), "tx": tx})),
                Err(e) => log(json!({"event": "claim_failed", "epoch": epoch, "job": job.to_b58(), "error": format!("{e:#}")})),
            }
            self.to_claim.remove(&(epoch, job));
        }
        Ok(())
    }

    fn send_claim(&self, epoch: u64, job: Pubkey) -> Result<String> {
        let ix = self.programs.claim_ix(self.me(), job, epoch);
        let tx = build_signed(&[ix], &[&self.key], self.rpc.latest_blockhash()?)?;
        self.rpc.send_and_confirm(&tx)
    }

    fn pass(&mut self) -> Result<()> {
        let max_nonce = self.programs.get_mining_config(&self.rpc)?.max_nonce as u64;
        let jobs = self.programs.open_jobs(&self.rpc)?;
        let slot = self.rpc.get_slot()?;
        for j in jobs {
            if self.done.contains(&j.address) || slot > j.deadline_slot {
                continue;
            }
            if let Err(e) = self.process_job(&j, max_nonce) {
                log(json!({"event": "job_failed", "job": j.address.to_b58(), "error": format!("{e:#}")}));
            }
            self.done.insert(j.address);
        }
        self.claim_closed()
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let key = read_keypair(&args.keypair)?;
    let programs = Programs { jobs: Pubkey::from_b58(&args.jobs_program)?, mining: Pubkey::from_b58(&args.mining_program)? };
    let attest = Attest { agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).build(), url: args.attester.clone() };
    let rpc = Rpc::new(&args.rpc);
    let mut miner = Miner { key, rpc, programs, attest, done: HashSet::new(), to_claim: BTreeSet::new(), args };
    log(json!({"event": "start", "miner": miner.me().to_b58(), "threads": miner.args.threads, "pid": std::process::id()}));
    if let Some(p) = miner.args.job_file.clone() {
        return miner.run_offline(&p);
    }
    loop {
        if let Err(e) = miner.pass() {
            log(json!({"event": "pass_failed", "error": format!("{e:#}")}));
        }
        if miner.args.once {
            // With --once, wait for the epochs we mined in to close so claims land.
            if !miner.to_claim.is_empty() {
                log(json!({"event": "pending_claims", "count": miner.to_claim.len()}));
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(miner.args.poll_ms));
    }
}
