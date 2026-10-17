//! `pearl-job` CLI. Every subcommand prints exactly one JSON object on stdout.
//! Success: `{"ok": true, ...}`, exit 0. Failure: `{"ok": false, "error": "..."}`, exit 1.
//! Interface documented in crates/pearl-job/CLI.md; keep it stable.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use pearl_job::data;
use pearl_job::freivalds::{freivalds, matmul, result_hash};
use pearl_job::job::MiningConfig;
use pearl_job::pattern::PeriodicPattern;
use pearl_job::pow::{self, BlockHeader, BlockWork, FallbackParams};
use pearl_job::spec::{parse_key32, parse_pubkey, parse_u32, JobSpec};
use pearl_job::{commit, PlainProof, PreparedJob};

#[derive(Parser)]
#[command(name = "pearl-job", version, about = "Job-bound Pearl plain proofs (JSON output)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Random int7 A (m x k) and B (k x n) into a data directory.
    Gen {
        #[arg(long)]
        m: u32,
        #[arg(long)]
        n: u32,
        #[arg(long, default_value_t = 2048)]
        k: u32,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Build a 52-byte mining config (defaults: localnet k=2048 r=128, 4x8 tile; r >= 128, rank penalty).
    Config {
        #[arg(long, default_value_t = 2048)]
        k: u32,
        #[arg(long, default_value_t = 128)]
        rank: u16,
        /// Comma-separated row pattern, e.g. 0,1,2,3
        #[arg(long, default_value = "0,1,2,3")]
        rows: String,
        #[arg(long, default_value = "0,1,2,3,4,5,6,7")]
        cols: String,
    },
    /// 76-byte job header and job_key.
    Header(JobArgs),
    /// hash_a / hash_b of the job's matrices.
    Commit {
        #[command(flatten)]
        job: JobArgs,
        #[arg(long)]
        data: String,
    },
    /// Mine shares for a miner over a nonce range.
    Mine {
        #[command(flatten)]
        job: JobArgs,
        #[arg(long)]
        data: String,
        /// Miner pubkey, base58.
        #[arg(long)]
        miner: String,
        #[arg(long, default_value_t = 0)]
        nonce_start: u64,
        #[arg(long, default_value_t = 1)]
        nonce_end: u64,
        /// Override the job's target_bits for the share check.
        #[arg(long)]
        share_target: Option<String>,
        /// Collect every winning nonce, not just the first.
        #[arg(long)]
        all: bool,
        /// Write proofs here: a file for one proof, a directory (proof-<nonce>.bin) with --all.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify a plain share proof natively.
    Verify {
        #[command(flatten)]
        job: JobArgs,
        #[arg(long)]
        miner: String,
        /// Proof file (binary PSJ1).
        #[arg(long, conflicts_with = "proof_b64")]
        proof: Option<PathBuf>,
        #[arg(long)]
        proof_b64: Option<String>,
        #[arg(long)]
        share_target: Option<String>,
    },
    /// Compute C = AB (i32 LE row-major) and its result_hash.
    Matmul {
        #[command(flatten)]
        job: JobArgs,
        #[arg(long)]
        data: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Freivalds check of a claimed C = AB, plus result_hash.
    Freivalds {
        #[command(flatten)]
        job: JobArgs,
        #[arg(long)]
        data: String,
        /// C file, i32 LE row-major m x n.
        #[arg(long)]
        c: PathBuf,
        #[arg(long, default_value_t = 2)]
        rounds: usize,
        /// Fixed RNG seed (tests only; omit to use the OS RNG).
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Build a 256-byte v2 block header; prints it with pow_digest and block_hash.
    BlockHeader(HeaderArgs),
    /// Mine a v2 block: iterate the header nonce on a job (job args + --data) or a fallback block.
    BlockMine {
        #[command(flatten)]
        header: HeaderArgs,
        #[command(flatten)]
        job: JobArgs,
        /// Job data; omit for a fallback block (header job_id zero).
        #[arg(long)]
        data: Option<String>,
        #[command(flatten)]
        fallback: FallbackArgs,
        #[arg(long, default_value_t = 0)]
        nonce_start: u64,
        #[arg(long, default_value_t = 1)]
        nonce_end: u64,
        /// Write the proof here instead of printing proof_b64.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify a v2 block's work (proof_hash, pow-bound seed, header target).
    BlockVerify {
        #[command(flatten)]
        header: HeaderArgs,
        /// Job args (with --hash-a, --hash-b, --m, --n); omit for fallback blocks.
        #[command(flatten)]
        job: JobArgs,
        #[command(flatten)]
        fallback: FallbackArgs,
        #[arg(long, conflicts_with = "proof_b64")]
        proof: Option<PathBuf>,
        #[arg(long)]
        proof_b64: Option<String>,
    },
}

/// Fallback block matrix shape (chain parameter).
#[derive(Args, Clone)]
struct FallbackArgs {
    #[arg(long, default_value_t = 4)]
    fallback_m: u32,
    #[arg(long, default_value_t = 8)]
    fallback_n: u32,
}

impl FallbackArgs {
    fn params(&self) -> FallbackParams {
        FallbackParams { m: self.fallback_m, n: self.fallback_n, ..FallbackParams::default() }
    }
}

/// A block header: `--header <hex256>` as the base, then per-field overrides.
/// 32-byte fields take hex or base58.
#[derive(Args, Clone, Default)]
struct HeaderArgs {
    #[arg(long)]
    header: Option<String>,
    #[arg(long)]
    version: Option<u32>,
    #[arg(long)]
    height: Option<u64>,
    #[arg(long)]
    prev_hash: Option<String>,
    #[arg(long)]
    timestamp_ms: Option<u64>,
    /// Chain target (compact), decimal or 0x hex.
    #[arg(long)]
    block_target: Option<String>,
    /// Zero (or omitted) = fallback block.
    #[arg(long)]
    job_id: Option<String>,
    /// Payout key.
    #[arg(long)]
    payout: Option<String>,
    #[arg(long)]
    tx_root: Option<String>,
    #[arg(long)]
    state_root: Option<String>,
    #[arg(long)]
    withdraw_root: Option<String>,
    #[arg(long)]
    proof_hash: Option<String>,
    #[arg(long)]
    header_nonce: Option<u64>,
}

impl HeaderArgs {
    fn build(&self) -> Result<BlockHeader> {
        let mut h = match &self.header {
            Some(x) => BlockHeader::from_bytes(&hex::decode(x.trim_start_matches("0x")).context("header hex")?)?,
            None => BlockHeader { version: 1, ..Default::default() },
        };
        let k = |o: &Option<String>, f: &mut [u8; 32]| -> Result<()> {
            if let Some(v) = o {
                *f = parse_key32(v)?;
            }
            Ok(())
        };
        if let Some(v) = self.version {
            h.version = v;
        }
        if let Some(v) = self.height {
            h.height = v;
        }
        if let Some(v) = self.timestamp_ms {
            h.timestamp_ms = v;
        }
        if let Some(v) = &self.block_target {
            h.target_bits = parse_u32(v)?;
        }
        if let Some(v) = self.header_nonce {
            h.nonce = v;
        }
        k(&self.prev_hash, &mut h.prev_hash)?;
        k(&self.job_id, &mut h.job_id)?;
        k(&self.payout, &mut h.miner)?;
        k(&self.tx_root, &mut h.tx_root)?;
        k(&self.state_root, &mut h.state_root)?;
        k(&self.withdraw_root, &mut h.withdraw_root)?;
        k(&self.proof_hash, &mut h.proof_hash)?;
        Ok(h)
    }
}

fn header_json(h: &BlockHeader) -> Value {
    json!({"header": hex::encode(h.to_bytes()), "pow_digest": hex::encode(h.pow_digest()),
           "block_hash": hex::encode(h.block_hash()), "height": h.height, "nonce": h.nonce,
           "target_bits": h.target_bits, "fallback": h.is_fallback()})
}

/// A job, either from `--job-file` (JobSpec JSON) or from flags. Flags override the file.
#[derive(Args, Clone, Default)]
struct JobArgs {
    #[arg(long)]
    job_file: Option<PathBuf>,
    /// Job account pubkey, base58.
    #[arg(long)]
    job: Option<String>,
    #[arg(long)]
    terms_hash: Option<String>,
    #[arg(long)]
    slot: Option<u64>,
    /// Compact target, decimal or 0x hex.
    #[arg(long)]
    target_bits: Option<String>,
    /// 52-byte config hex (default: localnet config).
    #[arg(long)]
    config: Option<String>,
    #[arg(long)]
    m: Option<u32>,
    #[arg(long)]
    n: Option<u32>,
    #[arg(long)]
    hash_a: Option<String>,
    #[arg(long)]
    hash_b: Option<String>,
}

impl JobArgs {
    fn given(&self) -> bool {
        self.job_file.is_some() || self.job.is_some()
    }

    fn spec(&self) -> Result<JobSpec> {
        let mut s = match &self.job_file {
            Some(p) => serde_json::from_slice::<JobSpec>(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)
                .context("parse job file")?,
            None => JobSpec {
                job: String::new(),
                terms_hash: String::new(),
                created_slot: 0,
                target_bits: 0,
                config: hex::encode(MiningConfig::localnet_default().to_bytes()),
                m: 0,
                n: 0,
                hash_a: String::new(),
                hash_b: String::new(),
                data_uri: String::new(),
                max_nonce: 0,
            },
        };
        if let Some(v) = &self.job {
            s.job = v.clone();
        }
        if let Some(v) = &self.terms_hash {
            s.terms_hash = v.clone();
        }
        if let Some(v) = self.slot {
            s.created_slot = v;
        }
        if let Some(v) = &self.target_bits {
            s.target_bits = parse_u32(v)?;
        }
        if let Some(v) = &self.config {
            s.config = v.trim_start_matches("0x").to_string();
        }
        if let Some(v) = self.m {
            s.m = v;
        }
        if let Some(v) = self.n {
            s.n = v;
        }
        if let Some(v) = &self.hash_a {
            s.hash_a = v.clone();
        }
        if let Some(v) = &self.hash_b {
            s.hash_b = v.clone();
        }
        if s.job.is_empty() || s.terms_hash.is_empty() {
            bail!("job needs --job and --terms-hash (or --job-file)");
        }
        Ok(s)
    }

    /// Spec with m, n from the data manifest and hash_a / hash_b computed if missing.
    fn spec_with_data(&self, d: &data::JobData) -> Result<JobSpec> {
        let mut s = self.spec()?;
        let cfg = s.mining_config()?;
        if s.m == 0 {
            s.m = d.manifest.m;
        }
        if s.n == 0 {
            s.n = d.manifest.n;
        }
        if s.m != d.manifest.m || s.n != d.manifest.n || cfg.common_dim != d.manifest.k {
            bail!(
                "job dims m={} n={} k={} do not match data m={} n={} k={}",
                s.m, s.n, cfg.common_dim, d.manifest.m, d.manifest.n, d.manifest.k
            );
        }
        let (ha, hb) = commit(&s.job_key()?, &d.a, &d.b, cfg.k(), s.n as usize);
        for (given, got, name) in [(&mut s.hash_a, ha, "hash_a"), (&mut s.hash_b, hb, "hash_b")] {
            if given.is_empty() {
                *given = hex::encode(got);
            } else if pearl_job::spec::parse_hash(given)? != got {
                bail!("data does not match the job's {name}");
            }
        }
        Ok(s)
    }
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn share_json(p: &PlainProof, bytes: &[u8], path: Option<&PathBuf>) -> Value {
    let mut v = json!({
        "nonce": p.nonce, "t_rows": p.t_rows, "t_cols": p.t_cols, "proof_len": bytes.len(),
    });
    match path {
        Some(p) => v["proof_path"] = json!(p.display().to_string()),
        None => v["proof_b64"] = json!(b64(bytes)),
    }
    v
}

fn run(cli: Cli) -> Result<Value> {
    Ok(match cli.cmd {
        Cmd::Gen { m, n, k, out_dir, seed } => {
            let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, seed);
            let mp = data::save(&out_dir, m, n, k, &a, &b)?;
            let abs = std::fs::canonicalize(&mp)?;
            // the same data as one blob for the node's blob store (putBlob, data_uri blob:<id>)
            let blob = data::encode_job_blob(m, n, k, &a, &b)?;
            let blob_path = out_dir.join("job.blob");
            std::fs::write(&blob_path, &blob)?;
            let blob_id = data::blob_id(&blob);
            json!({"ok": true, "m": m, "n": n, "k": k,
                   "blob_file": std::fs::canonicalize(&blob_path)?.display().to_string(),
                   "blob_id": hex::encode(blob_id), "blob_uri": data::blob_uri(&blob_id), "blob_size": blob.len(),
                   "manifest": abs.display().to_string(),
                   "a": std::fs::canonicalize(out_dir.join("A.i8"))?.display().to_string(),
                   "b": std::fs::canonicalize(out_dir.join("B.i8"))?.display().to_string(),
                   "data_uri": format!("file://{}", abs.display())})
        }
        Cmd::Config { k, rank, rows, cols } => {
            let parse = |s: &str| -> Result<Vec<u32>> { s.split(',').map(|x| Ok(x.trim().parse()?)).collect() };
            let c = MiningConfig {
                common_dim: k,
                rank,
                rows_pattern: PeriodicPattern::from_list(&parse(&rows)?)?,
                cols_pattern: PeriodicPattern::from_list(&parse(&cols)?)?,
            };
            pearl_job::job::check_shape(&c, c.rows_pattern.period() as usize * 2, c.cols_pattern.period() as usize * 2)?;
            json!({"ok": true, "config": hex::encode(c.to_bytes()), "k": k, "rank": rank,
                   "h": c.h(), "w": c.w(), "row_period": c.rows_pattern.period(), "col_period": c.cols_pattern.period()})
        }
        Cmd::Header(ja) => {
            let s = ja.spec()?;
            let h = s.header()?;
            json!({"ok": true, "header": hex::encode(h.to_bytes()), "config": s.config, "job_key": hex::encode(s.job_key()?)})
        }
        Cmd::Commit { job, data: uri } => {
            let d = data::load(&uri)?;
            let mut ja = job.clone();
            ja.hash_a = None;
            ja.hash_b = None;
            let s = ja.spec_with_data(&d)?;
            s.to_job()?.sanity_check()?;
            json!({"ok": true, "hash_a": s.hash_a, "hash_b": s.hash_b, "job_key": hex::encode(s.job_key()?),
                   "header": hex::encode(s.header()?.to_bytes()), "config": s.config, "m": s.m, "n": s.n,
                   "k": d.manifest.k})
        }
        Cmd::Mine { job, data: uri, miner, nonce_start, nonce_end, share_target, all, out } => {
            let d = data::load(&uri)?;
            let s = job.spec_with_data(&d)?;
            let target = match share_target {
                Some(t) => parse_u32(&t)?,
                None => s.target_bits,
            };
            let miner = parse_pubkey(&miner)?;
            let prep = PreparedJob::new(s.to_job()?, &d.a, &d.b)?;
            let t0 = Instant::now();
            let mut shares = Vec::new();
            if all {
                if let Some(dir) = &out {
                    std::fs::create_dir_all(dir)?;
                }
            }
            let mut tried = 0u64;
            for nonce in nonce_start..nonce_end {
                tried += 1;
                if let Some(p) = prep.mine_nonce(&miner, nonce, target) {
                    let bytes = p.to_bytes();
                    let path = out.as_ref().map(|o| if all { o.join(format!("proof-{nonce}.bin")) } else { o.clone() });
                    if let Some(path) = &path {
                        std::fs::write(path, &bytes)?;
                    }
                    shares.push(share_json(&p, &bytes, path.as_ref()));
                    if !all {
                        break;
                    }
                }
            }
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            json!({"ok": true, "found": !shares.is_empty(), "shares": shares, "nonces_tried": tried,
                   "elapsed_ms": ms, "ms_per_nonce": if tried > 0 { ms / tried as f64 } else { 0.0 },
                   "share_target": target, "hash_a": s.hash_a, "hash_b": s.hash_b})
        }
        Cmd::Verify { job, miner, proof, proof_b64, share_target } => {
            let s = job.spec()?;
            let bytes = match (proof, proof_b64) {
                (Some(p), _) => std::fs::read(&p).with_context(|| format!("read {}", p.display()))?,
                (None, Some(b)) => base64::engine::general_purpose::STANDARD.decode(b.trim())?,
                _ => bail!("need --proof or --proof-b64"),
            };
            let target = match share_target {
                Some(t) => parse_u32(&t)?,
                None => s.target_bits,
            };
            let job = s.to_job()?;
            let miner = parse_pubkey(&miner)?;
            let t0 = Instant::now();
            let res = PlainProof::from_bytes(&bytes)
                .and_then(|p| pearl_job::verify_share_with_target(&job, &p, &miner, target));
            let us = t0.elapsed().as_secs_f64() * 1e6;
            match res {
                Ok(v) => json!({"ok": true, "valid": true, "nonce": v.nonce, "t_rows": v.t_rows, "t_cols": v.t_cols,
                                "jackpot_hash": hex::encode(v.jackpot_hash), "proof_len": bytes.len(), "verify_us": us}),
                Err(e) => json!({"ok": true, "valid": false, "reason": format!("{e:#}"), "proof_len": bytes.len(),
                                 "verify_us": us}),
            }
        }
        Cmd::Matmul { job, data: uri, out } => {
            let d = data::load(&uri)?;
            let (m, n, k) = (d.manifest.m as usize, d.manifest.n as usize, d.manifest.k as usize);
            let s = job.spec_with_data(&d)?;
            let c = matmul(&d.a, &d.b, m, n, k);
            data::write_i32(&out, &c)?;
            json!({"ok": true, "c": out.display().to_string(), "m": m, "n": n,
                   "result_hash": hex::encode(result_hash(&s.job_key()?, &c))})
        }
        Cmd::Freivalds { job, data: uri, c, rounds, seed } => {
            let d = data::load(&uri)?;
            let (m, n, k) = (d.manifest.m as usize, d.manifest.n as usize, d.manifest.k as usize);
            let s = job.spec_with_data(&d)?;
            let cm = data::read_i32(&c, m * n)?;
            let t0 = Instant::now();
            let ok = match seed {
                Some(sd) => freivalds(&d.a, &d.b, &cm, m, n, k, rounds, &mut <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(sd)),
                None => freivalds(&d.a, &d.b, &cm, m, n, k, rounds, &mut rand::rng()),
            };
            json!({"ok": true, "valid": ok, "rounds": rounds, "elapsed_ms": t0.elapsed().as_secs_f64() * 1e3,
                   "result_hash": hex::encode(result_hash(&s.job_key()?, &cm)),
                   "hash_a": s.hash_a, "hash_b": s.hash_b})
        }
        Cmd::BlockHeader(ha) => {
            let mut v = header_json(&ha.build()?);
            v["ok"] = json!(true);
            v
        }
        Cmd::BlockMine { header, job, data: uri, fallback, nonce_start, nonce_end, out } => {
            let mut t = header.build()?;
            let prep = match &uri {
                Some(u) => {
                    let d = data::load(u)?;
                    let s = job.spec_with_data(&d)?;
                    let prep = PreparedJob::new(s.to_job()?, &d.a, &d.b)?;
                    if t.is_fallback() {
                        t.job_id = prep.job.header.job_pubkey;
                    }
                    Some(prep)
                }
                None => {
                    if job.given() {
                        bail!("job blocks need --data");
                    }
                    if !t.is_fallback() {
                        bail!("header has a job_id but no --data was given");
                    }
                    None
                }
            };
            let work = match &prep {
                Some(p) => BlockWork::Job(p),
                None => BlockWork::Fallback(fallback.params()),
            };
            let t0 = Instant::now();
            let mut tried = 0u64;
            let mut found = None;
            for nonce in nonce_start..nonce_end {
                tried += 1;
                if let Some(b) = pow::mine_header_nonce(&t, &work, nonce)? {
                    found = Some(b);
                    break;
                }
            }
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            let mut v = json!({"ok": true, "found": found.is_some(), "nonces_tried": tried, "elapsed_ms": ms,
                               "ms_per_nonce": if tried > 0 { ms / tried as f64 } else { 0.0 }});
            if let Some(b) = found {
                let hj = header_json(&b.header);
                for (key, val) in hj.as_object().unwrap() {
                    v[key] = val.clone();
                }
                v["proof_len"] = json!(b.proof.len());
                match &out {
                    Some(p) => {
                        std::fs::write(p, &b.proof)?;
                        v["proof_path"] = json!(p.display().to_string());
                    }
                    None => v["proof_b64"] = json!(b64(&b.proof)),
                }
            }
            v
        }
        Cmd::BlockVerify { header, job, fallback, proof, proof_b64 } => {
            let h = header.build()?;
            let bytes = match (proof, proof_b64) {
                (Some(p), _) => std::fs::read(&p).with_context(|| format!("read {}", p.display()))?,
                (None, Some(b)) => base64::engine::general_purpose::STANDARD.decode(b.trim())?,
                _ => bail!("need --proof or --proof-b64"),
            };
            let job = if job.given() && !h.is_fallback() { Some(job.spec()?.to_job()?) } else { None };
            let t0 = Instant::now();
            let res = pow::verify_block(&h, job.as_ref(), &bytes, &fallback.params());
            let us = t0.elapsed().as_secs_f64() * 1e6;
            match res {
                Ok(v) => json!({"ok": true, "valid": true, "nonce": v.nonce, "t_rows": v.t_rows, "t_cols": v.t_cols,
                                "jackpot_hash": hex::encode(v.jackpot_hash), "block_hash": hex::encode(h.block_hash()),
                                "proof_len": bytes.len(), "verify_us": us}),
                Err(e) => json!({"ok": true, "valid": false, "reason": format!("{e:#}"), "proof_len": bytes.len(),
                                 "verify_us": us}),
            }
        }
    })
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(v) => println!("{v}"),
        Err(e) => {
            println!("{}", json!({"ok": false, "error": format!("{e:#}")}));
            std::process::exit(1);
        }
    }
}
