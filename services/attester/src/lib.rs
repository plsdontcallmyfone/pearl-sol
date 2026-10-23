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

pub mod evaluator;
pub mod judge;
pub mod replay;

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
    /// Model store for inference tickets (SPEC 5.4): registered matrices are read from it
    /// (and rebuilt from the node's blobs when missing).
    pub models_dir: std::path::PathBuf,
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
    /// `blob:<hex>` (node mode, fetched with getBlob) or `file:///path/C.i32`. Defaults to
    /// the job's on-chain result_uri.
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

fn read_result(b64: &Option<String>, uri: &Option<String>, len: usize, node: Option<&dyn NodeApi>) -> Result<Vec<i32>> {
    let bytes = match (b64, uri) {
        (Some(b), _) => base64::engine::general_purpose::STANDARD.decode(b.trim()).context("result_b64")?,
        (None, Some(u)) if u.starts_with(data::BLOB_URI_PREFIX) => {
            let node = node.ok_or_else(|| anyhow!("blob: result URIs need node mode (getBlob)"))?;
            pearl_node::fetch_blob(node, u).context("fetch result blob")?
        }
        (None, Some(u)) => {
            let p = u.strip_prefix("file://").ok_or_else(|| anyhow!("result_uri must be blob: or file://"))?;
            // bounded: never read a device, FIFO or oversized file (security pass V-2b)
            data::read_exact_file(std::path::Path::new(p), (len as u64).saturating_mul(4))?
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
        let node: Option<&dyn NodeApi> = match &self.source {
            JobSource::Node(n) => Some(n.as_ref()),
            _ => None,
        };
        let d = match node {
            Some(n) => pearl_node::load_job_data(n, &spec.data_uri),
            None => data::load(&spec.data_uri),
        }
        .context("load job data")?;
        let job = spec.to_job()?;
        let (m, n, k) = (job.m as usize, job.n as usize, job.config.k());
        ensure!(d.manifest.m == job.m && d.manifest.n == job.n && d.manifest.k as usize == k, "job data dims differ from the job");
        let (ha, hb) = pearl_job::commit(&job.job_key(), &d.a, &d.b, k, n);
        ensure!(ha == job.hash_a && hb == job.hash_b, "job data does not match the job's commitments");
        let c = read_result(&req.result_b64, uri, m * n, node)?;
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
        if info.kind == "ticket" {
            let JobSource::Node(node) = &self.source else { bail!("tickets need node mode") };
            let t0 = Instant::now();
            let rh = self.check_ticket(node.as_ref(), info, job_pk)?;
            let msg = result_message(job_pk, deliverer, &rh);
            return Ok(ResultResponse {
                ok: true,
                job: req.job.clone(),
                deliverer: req.deliverer.clone(),
                result_hash: hex::encode(rh),
                attester: self.pubkey_b58(),
                message_hex: hex::encode(&msg),
                signature_hex: self.sign(&msg),
                freivalds_rounds: 0,
                elapsed_ms: t0.elapsed().as_secs_f64() * 1e3,
            });
        }
        ensure!(info.kind != "segment", "a segment's output is committed at post time; there is nothing to attest");
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

    /// Evaluator rule for an inference ticket (SPEC 5.4): the response blob matches the
    /// ticket, every registered matrix has a segment over all the response's rows, and each
    /// segment's committed C equals A B recomputed from its activation blob and our copy of
    /// the registered weights. Returns the result_hash to sign (the response blob id).
    pub fn check_ticket(&self, node: &dyn NodeApi, info: &JobInfo, ticket_id: &[u8; 32]) -> Result<[u8; 32]> {
        let t = node.call("getTicket", json!({"ticket_id": hex::encode(ticket_id)}))?;
        let st = &t["standing"];
        let model_hex = st["model_id"].as_str().ok_or_else(|| anyhow!("ticket has no model"))?.to_string();
        let model = node.call("getModel", json!({"model_id": model_hex}))?;
        ensure!(!model.is_null(), "ticket's model is not registered");
        let period = model["row_period"].as_u64().unwrap_or(4).max(1);
        let count = model["matrix_count"].as_u64().unwrap_or(0) as usize;
        let rh = info.result_hash.ok_or_else(|| anyhow!("ticket has no result_hash"))?;
        let bytes = pearl_node::fetch_blob(node, &info.result_uri).context("fetch response blob")?;
        ensure!(pearl_job::data::blob_id(&bytes) == rh, "result_hash is not the response blob's id");
        let r: Value = serde_json::from_slice(&bytes).context("response blob is not JSON")?;
        ensure!(r["format"] == json!("cadre-response-v1"), "response format is not cadre-response-v1");
        ensure!(r["model_id"].as_str() == Some(model_hex.as_str()), "response names another model");
        ensure!(r["request_hash"].as_str() == st["request_hash"].as_str(), "response answers another request");
        ensure!(r["ticket_id"].as_str() == Some(hex::encode(ticket_id).as_str()), "response names another ticket");
        // rounds (SPEC 4.5): the response's rounds match the chain's, every round covers every
        // registered matrix with one m, and each round's requests fit in its m
        let segs = t["segment_list"].as_array().cloned().unwrap_or_default();
        let rounds = st["rounds"].as_u64().unwrap_or(0) as usize;
        ensure!(rounds > 0, "the ticket has no segment rounds");
        let rr = r["rounds"].as_array().cloned().unwrap_or_default();
        ensure!(rr.len() == rounds, "response lists {} rounds, the chain has {rounds}", rr.len());
        let mut total = 0u64;
        for (i, x) in rr.iter().enumerate() {
            let in_round: Vec<&Value> = segs.iter().filter(|g| g["round"].as_u64() == Some(i as u64)).collect();
            ensure!(in_round.len() == count, "round {i} has {} segments, the model registers {count} matrices", in_round.len());
            let m = in_round[0]["m"].as_u64().unwrap_or(0);
            ensure!(in_round.iter().all(|g| g["m"].as_u64() == Some(m)) && x["m"].as_u64() == Some(m), "round {i}: segments and response disagree on m");
            let used: u64 = x["requests"].as_array().into_iter().flatten().map(|q| q["rows"].as_u64().unwrap_or(0)).sum();
            ensure!(used <= m && m - used < period, "round {i}: requests use {used} of {m} rows (only the last period may be padding)");
            total += m;
        }
        ensure!(total == st["rows_used"].as_u64().unwrap_or(0), "rounds add up to {total} rows, the ticket used {}", st["rows_used"]);
        let store = pearl_job::standing::ModelStore::new(&self.models_dir);
        // cadre-glue-v1 responses (SPEC 4.7): full replay, which also checks every segment's
        // c_hash; a failure is a replay::TicketFraud (provable), anything else an ordinary error
        if replay::replayable(&r) {
            let rep = replay::replay_ticket(node, &store, info, &model_hex, &r)?;
            ensure!(rep.segments == segs.len(), "replay checked {} segments, the ticket has {}", rep.segments, segs.len());
            eprintln!("replay {}: {} rounds, {} requests, {} rows, {} segments in {:.1} s (GEMM {:.1} s)", hex::encode(ticket_id), rep.rounds, rep.requests, rep.rows, rep.segments, rep.secs, rep.gemm_secs);
            return Ok(rh);
        }
        let ids: Vec<[u8; 32]> = segs.iter().map(|s| parse_key32(s["segment_id"].as_str().unwrap_or(""))).collect::<Result<_>>()?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(4);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let errors = std::sync::Mutex::new(Vec::<String>::new());
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= ids.len() {
                        break;
                    }
                    if let Err(e) = check_segment(node, &store, &ids[i]) {
                        errors.lock().unwrap().push(format!("segment {}: {e:#}", hex::encode(ids[i])));
                    }
                });
            }
        });
        let errors = errors.into_inner().unwrap();
        ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(rh)
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

/// One segment: A (blob) and B (model store, checked against hash_b) give C; its keyed hash
/// must be the segment's committed c_hash.
pub fn check_segment(node: &dyn NodeApi, store: &pearl_job::standing::ModelStore, id: &[u8; 32]) -> Result<()> {
    let j = node.get_job(id)?.ok_or_else(|| anyhow!("segment not found"))?;
    let s = j.standing.ok_or_else(|| anyhow!("not a segment"))?;
    let a = pearl_node::load_segment_a(node, &j)?;
    let bt = pearl_node::load_segment_bt(node, store, &s, j.config.common_dim, j.n)?;
    let prep = pearl_job::PreparedJob::new_bt(j.to_job(), &a, bt)?;
    let c = prep.product();
    ensure!(Some(result_hash(&prep.job_key, &c)) == j.result_hash, "committed C does not equal A B");
    Ok(())
}

// ---------------------------------------------------------------- HTTP serving

/// Work in progress on the serving threads, for `/health` (`busy`).
#[derive(Default)]
pub struct ServeStats {
    /// Requests being handled now, and when the oldest of them started (ms since start).
    pub running: std::sync::atomic::AtomicUsize,
    pub queued: std::sync::atomic::AtomicUsize,
    pub done: std::sync::atomic::AtomicU64,
    oldest: std::sync::Mutex<Vec<Instant>>,
}

impl ServeStats {
    pub fn json(&self) -> Value {
        use std::sync::atomic::Ordering::SeqCst;
        let oldest = self.oldest.lock().unwrap().iter().min().map(|t| t.elapsed().as_secs());
        json!({"running": self.running.load(SeqCst), "queued": self.queued.load(SeqCst), "done": self.done.load(SeqCst), "oldest_running_s": oldest})
    }
}

/// Serve HTTP (the pearl-attester binary): `GET /health` is answered on the accepting thread at
/// once, every other request on one of `workers` threads in arrival order. Before 2026-10-23 one
/// loop did both, so a health check waited behind a ticket's glue replay (480-565 s on box 0,
/// HANDOFF 46) and the devnet health check read "timed out reading response" while the attester
/// was working normally. `health` builds the health body; `stats` is added to it as `busy`.
pub fn serve<H, W>(server: tiny_http::Server, workers: usize, health: H, work: W, stats: std::sync::Arc<ServeStats>, log: bool)
where
    H: Fn() -> Value,
    W: Fn(&str, &str, &[u8]) -> (u16, Value) + Send + Sync + 'static,
{
    use std::io::Read;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::{mpsc, Arc, Mutex};
    fn respond(req: tiny_http::Request, status: u16, v: &Value) {
        let resp = tiny_http::Response::from_string(v.to_string())
            .with_status_code(status)
            .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
        let _ = req.respond(resp);
    }
    let (tx, rx) = mpsc::channel::<(tiny_http::Request, String, String, Vec<u8>)>();
    let rx = Arc::new(Mutex::new(rx));
    let work = Arc::new(work);
    for _ in 0..workers.max(1) {
        let (rx, work, stats) = (Arc::clone(&rx), Arc::clone(&work), Arc::clone(&stats));
        std::thread::spawn(move || loop {
            let next = rx.lock().unwrap().recv();
            let Ok((req, method, path, body)) = next else { return };
            stats.queued.fetch_sub(1, SeqCst);
            stats.running.fetch_add(1, SeqCst);
            let t0 = Instant::now();
            stats.oldest.lock().unwrap().push(t0);
            let (status, v) = work(&method, &path, &body);
            stats.oldest.lock().unwrap().retain(|t| *t != t0);
            stats.running.fetch_sub(1, SeqCst);
            stats.done.fetch_add(1, SeqCst);
            if log {
                eprintln!("{method} {path} -> {status} ({} ms){}", t0.elapsed().as_millis(), v.get("error").map(|e| format!(" {e}")).unwrap_or_default());
            }
            respond(req, status, &v);
        });
    }
    for mut req in server.incoming_requests() {
        let method = req.method().as_str().to_uppercase();
        let path = req.url().split('?').next().unwrap_or("").to_string();
        if method == "GET" && path == "/health" {
            let mut v = health();
            v["busy"] = stats.json();
            respond(req, 200, &v);
            continue;
        }
        let mut body = Vec::new();
        let _ = req.as_reader().take(64 << 20).read_to_end(&mut body);
        stats.queued.fetch_add(1, SeqCst);
        if tx.send((req, method, path, body)).is_err() {
            return;
        }
    }
}
