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
use pearl_job::spec::{parse_pubkey, parse_u32, JobSpec};
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
        #[arg(long, default_value_t = 1024)]
        k: u32,
        #[arg(long)]
        out_dir: PathBuf,
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Build a 52-byte mining config (defaults: localnet k=1024 r=32, 4x8 tile).
    Config {
        #[arg(long, default_value_t = 1024)]
        k: u32,
        #[arg(long, default_value_t = 32)]
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
            json!({"ok": true, "m": m, "n": n, "k": k,
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
