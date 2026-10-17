//! Attester core (no HTTP): share verification and result evaluation, each ending in
//! an ed25519 signature over the exact SPEC section 6 message bytes.
//!
//! Job source:
//! - `Node`: the job is read from the v2 agent-chain node (`getJob`, SPEC v2 4.1);
//!   the request's job fields are ignored. Default. Only `/result` applies (v2 has no
//!   share messages; block proofs are checked by the node itself).
//! - `Chain`: v1 (SVM, superseded): the job is read from the jobs program account.
//! - `Request`: the request carries a `job_spec` and it is trusted. Dev and tests only,
//!   since a client could then describe a fake job with an easy target.

use std::time::Instant;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pearl_chain::accounts::JobStatus;
use pearl_chain::ix::Programs;
use pearl_chain::{Pubkey, Rpc};
use pearl_job::freivalds::{freivalds, result_hash};
use pearl_job::messages::{result_message, share_message};
use pearl_job::spec::{parse_key32, parse_pubkey};
use pearl_node::{JobInfo, NodeApi};
use pearl_job::{data, JobSpec, PlainProof};

pub enum JobSource {
    Node(Box<dyn NodeApi>),
    Chain { rpc: Rpc, programs: Programs },
    Request,
}

pub struct Attester {
    pub key: SigningKey,
    pub source: JobSource,
    pub freivalds_rounds: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShareRequest {
    pub job: String,
    pub miner: String,
    /// PSJ1 proof bytes, base64.
    pub proof_b64: String,
    /// Only read in `Request` mode.
    #[serde(default)]
    pub job_spec: Option<JobSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShareResponse {
    pub ok: bool,
    pub job: String,
    pub miner: String,
    pub nonce: u64,
    pub t_rows: u32,
    pub t_cols: u32,
    pub attester: String,
    /// Exact signed bytes (SPEC 6 Share), hex.
    pub message_hex: String,
    /// ed25519 signature, hex (64 bytes).
    pub signature_hex: String,
    pub verify_us: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResultRequest {
    pub job: String,
    pub deliverer: String,
    /// C as i32 LE row-major m x n, base64. Either this or `result_uri`.
    #[serde(default)]
    pub result_b64: Option<String>,
    /// `file:///path/C.i32`. Defaults to the job's on-chain result_uri in Chain mode.
    #[serde(default)]
    pub result_uri: Option<String>,
    #[serde(default)]
    pub job_spec: Option<JobSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResultResponse {
    pub ok: bool,
    pub job: String,
    pub deliverer: String,
    pub result_hash: String,
    pub attester: String,
    pub message_hex: String,
    pub signature_hex: String,
    pub freivalds_rounds: usize,
    pub elapsed_ms: f64,
}

fn read_result(b64: &Option<String>, uri: &Option<String>, len: usize) -> Result<Vec<i32>> {
    let bytes = match (b64, uri) {
        (Some(b), _) => base64::engine::general_purpose::STANDARD.decode(b.trim()).context("result_b64")?,
        (None, Some(u)) => {
            let p = u.strip_prefix("file://").ok_or_else(|| anyhow!("result_uri must be file:// in M1"))?;
            std::fs::read(p).with_context(|| format!("read {p}"))?
        }
        _ => bail!("need result_b64 or result_uri"),
    };
    ensure!(bytes.len() == len * 4, "result is {} bytes, expected m*n*4 = {}", bytes.len(), len * 4);
    Ok(bytes.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect())
}

impl Attester {
    pub fn pubkey_b58(&self) -> String {
        bs58::encode(self.key.verifying_key().to_bytes()).into_string()
    }

    fn sign(&self, msg: &[u8]) -> String {
        hex::encode(self.key.sign(msg).to_bytes())
    }

    /// Job spec plus (Chain mode) the on-chain account for status checks.
    fn job_spec(&self, job: &str, given: &Option<JobSpec>) -> Result<(JobSpec, Option<pearl_chain::accounts::JobAccount>)> {
        match &self.source {
            JobSource::Request => {
                let s = given.clone().ok_or_else(|| anyhow!("job_spec required (attester runs in request mode)"))?;
                ensure!(parse_key32(&s.job)? == parse_key32(job)?, "job_spec.job does not match job");
                Ok((s, None))
            }
            JobSource::Node(_) => bail!("v1 job accounts are not available in node mode"),
            JobSource::Chain { rpc, programs } => {
                let pk = Pubkey::from_b58(job)?;
                let acct = programs.get_job(rpc, &pk)?;
                let max_nonce = programs.get_mining_config(rpc)?.max_nonce as u64;
                Ok((acct.to_spec(max_nonce)?, Some(acct)))
            }
        }
    }

    pub fn handle_share(&self, req: &ShareRequest) -> Result<ShareResponse> {
        if matches!(self.source, JobSource::Node(_)) {
            bail!("share attestation is v1 only; v2 block proofs are verified by the node");
        }
        let job_pk = parse_key32(&req.job)?;
        let miner = parse_pubkey(&req.miner)?;
        let (spec, acct) = self.job_spec(&req.job, &req.job_spec)?;
        if let Some(a) = &acct {
            ensure!(a.status == JobStatus::Open, "job is not Open ({:?})", a.status);
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(req.proof_b64.trim()).context("proof_b64")?;
        let job = spec.to_job()?;
        let t0 = Instant::now();
        let proof = PlainProof::from_bytes(&bytes)?;
        if spec.max_nonce > 0 {
            ensure!(proof.nonce < spec.max_nonce, "nonce {} >= max_nonce {}", proof.nonce, spec.max_nonce);
        }
        let v = pearl_job::verify_share(&job, &proof, &miner)?;
        let verify_us = t0.elapsed().as_secs_f64() * 1e6;
        let msg = share_message(&job_pk, &miner, v.nonce);
        Ok(ShareResponse {
            ok: true,
            job: req.job.clone(),
            miner: req.miner.clone(),
            nonce: v.nonce,
            t_rows: v.t_rows,
            t_cols: v.t_cols,
            attester: self.pubkey_b58(),
            message_hex: hex::encode(&msg),
            signature_hex: self.sign(&msg),
            verify_us,
        })
    }

    pub fn handle_result(&self, req: &ResultRequest) -> Result<ResultResponse> {
        let job_pk = parse_key32(&req.job)?;
        let deliverer = parse_key32(&req.deliverer)?;
        if let JobSource::Node(node) = &self.source {
            let info = node.get_job(&job_pk)?.ok_or_else(|| anyhow!("job {} not found on the node", hex::encode(job_pk)))?;
            return self.handle_result_node(req, &info, &job_pk, &deliverer);
        }
        let (spec, acct) = self.job_spec(&req.job, &req.job_spec)?;
        let uri = req.result_uri.clone().or_else(|| acct.as_ref().map(|a| a.result_uri.clone()).filter(|u| !u.is_empty()));
        let (rh, ms) = self.check_result(&spec, req, &uri)?;
        if let Some(a) = &acct {
            ensure!(
                matches!(a.status, JobStatus::Delivered | JobStatus::Disputed),
                "job is not Delivered ({:?})",
                a.status
            );
            ensure!(a.deliverer.0 == deliverer, "deliverer is not the job's deliverer");
            ensure!(a.result_hash == rh, "on-chain result_hash does not match the delivered C");
        }
        let msg = result_message(&job_pk, &deliverer, &rh);
        Ok(ResultResponse {
            ok: true,
            job: req.job.clone(),
            deliverer: req.deliverer.clone(),
            result_hash: hex::encode(rh),
            attester: self.pubkey_b58(),
            message_hex: hex::encode(&msg),
            signature_hex: self.sign(&msg),
            freivalds_rounds: self.freivalds_rounds,
            elapsed_ms: ms,
        })
    }

    /// Load the job's A, B (checked against its commitments), read C, Freivalds it.
    /// Returns `(result_hash, elapsed_ms)`.
    fn check_result(&self, spec: &JobSpec, req: &ResultRequest, uri: &Option<String>) -> Result<([u8; 32], f64)> {
        let t0 = Instant::now();
        let d = data::load(&spec.data_uri).context("load job data")?;
        let job = spec.to_job()?;
        let (m, n, k) = (job.m as usize, job.n as usize, job.config.k());
        ensure!(d.manifest.m == job.m && d.manifest.n == job.n && d.manifest.k as usize == k, "job data dims differ from the job");
        let (ha, hb) = pearl_job::commit(&job.job_key(), &d.a, &d.b, k, n);
        ensure!(ha == job.hash_a && hb == job.hash_b, "job data does not match the job's commitments");
        let c = read_result(&req.result_b64, uri, m * n)?;
        ensure!(
            freivalds(&d.a, &d.b, &c, m, n, k, self.freivalds_rounds, &mut rand::rng()),
            "Freivalds check failed: C != AB"
        );
        Ok((result_hash(&job.job_key(), &c), t0.elapsed().as_secs_f64() * 1e3))
    }

    /// v2: job from the node. Signs the raw `result_message`, only for a Delivered job, its deliverer, and the
    /// result_hash the chain holds; and only if this attester is one of its evaluators.
    fn handle_result_node(&self, req: &ResultRequest, info: &JobInfo, job_pk: &[u8; 32], deliverer: &[u8; 32]) -> Result<ResultResponse> {
        ensure!(info.is("Delivered"), "job is not Delivered ({})", info.status);
        ensure!(info.deliverer.as_ref() == Some(deliverer), "deliverer is not the job's deliverer");
        let me = self.key.verifying_key().to_bytes();
        ensure!(info.evaluators.is_empty() || info.evaluators.contains(&me), "this attester is not an evaluator of the job");
        let uri = req.result_uri.clone().or_else(|| Some(info.result_uri.clone()).filter(|u| !u.is_empty()));
        let (rh, ms) = self.check_result(&info.to_spec(), req, &uri)?;
        ensure!(info.result_hash == Some(rh), "on-chain result_hash does not match the delivered C");
        // Raw ed25519 over the domain-prefixed bytes, as the chain checks (docs/RPC.md).
        let msg = result_message(job_pk, deliverer, &rh);
        Ok(ResultResponse {
            ok: true,
            job: req.job.clone(),
            deliverer: req.deliverer.clone(),
            result_hash: hex::encode(rh),
            attester: self.pubkey_b58(),
            message_hex: hex::encode(&msg),
            signature_hex: self.sign(&msg),
            freivalds_rounds: self.freivalds_rounds,
            elapsed_ms: ms,
        })
    }

    /// Route one HTTP request. Returns (status, json body).
    pub fn route(&self, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let res: Result<Value> = match (method, path) {
            ("GET", "/health") => Ok(json!({"ok": true, "attester": self.pubkey_b58(),
                                             "mode": match self.source { JobSource::Node(_) => "node", JobSource::Chain { .. } => "chain", JobSource::Request => "request" }})),
            ("POST", "/share") => serde_json::from_slice::<ShareRequest>(body)
                .map_err(anyhow::Error::from)
                .and_then(|r| self.handle_share(&r))
                .and_then(|r| Ok(serde_json::to_value(r)?)),
            ("POST", "/result") => serde_json::from_slice::<ResultRequest>(body)
                .map_err(anyhow::Error::from)
                .and_then(|r| self.handle_result(&r))
                .and_then(|r| Ok(serde_json::to_value(r)?)),
            _ => return (404, json!({"ok": false, "error": format!("no route {method} {path}")})),
        };
        match res {
            Ok(v) => (200, v),
            Err(e) => (400, json!({"ok": false, "error": format!("{e:#}")})),
        }
    }
}
