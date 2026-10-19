//! Cadre inference gateway (SPEC 5.4).
//!
//! A buyer posts `POST /v1/completions` with an x402 payment. The gateway turns the
//! request into an inference ticket (a storefront Order whose PostJob body is the ticket
//! form, SPEC 4.5), forwards the payment to storefront-x402, runs the int7 model worker
//! (tools/cadre/worker.py) and answers with the tokens. Then it settles on the agent chain:
//! once the ticket exists (relayer unlock) it posts one standing job per registered
//! matrix (PostSegments: the request's A as a blob, C as a commitment), delivers the
//! response blob (SubmitResult), gets the attester's signature and sends CompleteJob (W
//! to this server). Miners mine the segments; their blocks are the rebate.
//!
//! The transport pieces are traits so tests drive the gateway in process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_job::freivalds::result_hash;
use pearl_job::job::{commit_a, MiningConfig};
use pearl_job::standing;
use pearl_node::tx::{self, Body, SegmentSpec, Tx};
use pearl_node::{b58, NodeApi};

// ------------------------------------------------------------------ transports

/// storefront-x402 `POST /jobs`: `(status, body, x-payment-response header)`.
pub trait Storefront: Send + Sync {
    fn post_job(&self, body: &Value, x_payment: Option<&str>) -> Result<(u16, Value, Option<String>)>;
}

/// The model worker (tools/cadre/worker.py).
pub trait Worker: Send + Sync {
    fn tokenize(&self, text: &str) -> Result<Vec<u32>>;
    /// Runs the request; writes `out_dir/a/<i>.i8`, `out_dir/c/<i>.i32`.
    fn generate(&self, prompt_ids: &[u32], max_new: u32, out_dir: &Path) -> Result<Value>;
}

/// The evaluator (services/attester `POST /result`): `(attester pubkey, signature)`.
pub trait Evaluator: Send + Sync {
    fn attest(&self, ticket: &[u8; 32], deliverer: &[u8; 32]) -> Result<([u8; 32], [u8; 64])>;
}

fn agent(timeout_s: u64) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout_s)).build()
}

pub struct HttpStorefront {
    pub url: String,
}

impl Storefront for HttpStorefront {
    fn post_job(&self, body: &Value, x_payment: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        let mut req = agent(180).post(&format!("{}/jobs", self.url.trim_end_matches('/'))).set("content-type", "application/json");
        if let Some(p) = x_payment {
            req = req.set("x-payment", p);
        }
        let r = match req.send_string(&body.to_string()) {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => bail!("storefront-x402: {e}"),
        };
        let status = r.status();
        let xpr = r.header("x-payment-response").map(String::from);
        let v: Value = r.into_json().unwrap_or(Value::Null);
        Ok((status, v, xpr))
    }
}

pub struct HttpWorker {
    pub url: String,
}

impl Worker for HttpWorker {
    fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let v: Value = agent(60).post(&format!("{}/tokenize", self.url)).send_json(json!({"text": text}))?.into_json()?;
        v["ids"].as_array().ok_or_else(|| anyhow!("worker /tokenize: {v}"))?.iter().map(|x| x.as_u64().map(|y| y as u32).ok_or_else(|| anyhow!("bad id"))).collect()
    }

    fn generate(&self, prompt_ids: &[u32], max_new: u32, out_dir: &Path) -> Result<Value> {
        let body = json!({"prompt_ids": prompt_ids, "max_new_tokens": max_new, "out_dir": out_dir.display().to_string()});
        match agent(3600).post(&format!("{}/generate", self.url)).send_json(body) {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(_, r)) => bail!("worker /generate: {}", r.into_string().unwrap_or_default()),
            Err(e) => bail!("worker /generate: {e}"),
        }
    }
}

pub struct HttpEvaluator {
    pub url: String,
}

impl Evaluator for HttpEvaluator {
    fn attest(&self, ticket: &[u8; 32], deliverer: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        let body = json!({"job": hex::encode(ticket), "deliverer": b58(deliverer)});
        let v: Value = match agent(1800).post(&format!("{}/result", self.url)).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(_, r)) => r.into_json()?,
            Err(e) => bail!("attester: {e}"),
        };
        ensure!(v["ok"] == json!(true), "attester refused: {}", v["error"]);
        let pk = pearl_job::spec::parse_key32(v["attester"].as_str().unwrap_or(""))?;
        let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap_or(""))?.try_into().map_err(|_| anyhow!("signature is not 64 bytes"))?;
        Ok((pk, sig))
    }
}

// ------------------------------------------------------------------ model + pure helpers

/// What the gateway needs of the registered model (from getModel).
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub model_id: [u8; 32],
    pub name: String,
    pub base: MiningConfig,
    /// (k, n) per matrix.
    pub matrices: Vec<(u32, u32)>,
    pub macs_per_row: u64,
}

impl ModelInfo {
    pub fn from_json(m: &Value) -> Result<Self> {
        ensure!(!m.is_null(), "model is not registered");
        let model_id = pearl_job::spec::parse_key32(m["model_id"].as_str().unwrap_or(""))?;
        let base = MiningConfig::from_bytes(&hex::decode(m["config"].as_str().unwrap_or(""))?)?;
        let matrices = m["matrices"]
            .as_array()
            .ok_or_else(|| anyhow!("getModel has no matrices"))?
            .iter()
            .map(|x| Ok((x["k"].as_u64().ok_or_else(|| anyhow!("k"))? as u32, x["n"].as_u64().ok_or_else(|| anyhow!("n"))? as u32)))
            .collect::<Result<Vec<_>>>()?;
        let macs_per_row = matrices.iter().map(|(k, n)| *k as u64 * *n as u64).sum();
        Ok(Self { model_id, name: m["name"].as_str().unwrap_or("").to_string(), base, matrices, macs_per_row })
    }

    pub fn period(&self) -> u32 {
        self.base.rows_pattern.period()
    }

    pub fn job_key(&self, i: u16) -> [u8; 32] {
        let (k, n) = self.matrices[i as usize];
        standing::matrix_job_key(&self.model_id, i, k, n, &self.base)
    }
}

/// The request blob (`cadre-request-v1`); its blob id is the ticket's `hash_a`.
pub fn request_blob(model_id: &[u8; 32], prompt_ids: &[u32], max_new: u32) -> Vec<u8> {
    // fixed field order: the same request always gives the same bytes (and price)
    format!(
        "{{\"format\":\"cadre-request-v1\",\"model_id\":\"{}\",\"prompt_ids\":{},\"max_new_tokens\":{max_new},\"sampling\":\"greedy\"}}",
        hex::encode(model_id),
        serde_json::to_string(prompt_ids).unwrap()
    )
    .into_bytes()
}

/// The storefront spec of a ticket (SPEC 4.5).
pub fn ticket_spec(model: &ModelInfo, server: &[u8; 32], prompt_len: u32, max_new: u32, request_uri: &str, request_hash: &[u8; 32], deadline_blocks: u64, blocks_per_segment: u32) -> Value {
    json!({
        "m": prompt_len, "n": max_new, "k": 1,
        "config": hex::encode(standing::ticket_config(&model.model_id)),
        "hash_a": hex::encode(request_hash), "hash_b": hex::encode(server),
        "data_uri": request_uri, "deadline_blocks": deadline_blocks,
        "max_blocks": model.matrices.len() as u32 * blocks_per_segment.max(1),
    })
}

/// One segment ready to post: the tx entry and the activation blob bytes.
pub struct PreparedSegment {
    pub spec: SegmentSpec,
    pub a_blob: Vec<u8>,
}

/// Segments from the worker's output directory: per matrix, A (m x k) and C (m x n) give
/// `hash_a = commit_a(job_key, A)` and `c_hash = blake3_keyed(C, job_key)`. The A URI is
/// the activation blob's id (content addressed, so it is known before the put).
pub fn prepare_segments(model: &ModelInfo, dir: &Path, m: u32, blocks_per_segment: u32) -> Result<Vec<PreparedSegment>> {
    let mut out = Vec::with_capacity(model.matrices.len());
    for (i, (k, n)) in model.matrices.iter().enumerate() {
        let a: Vec<i8> = std::fs::read(dir.join(format!("a/{i}.i8"))).with_context(|| format!("read A of matrix {i}"))?.into_iter().map(|x| x as i8).collect();
        ensure!(a.len() == m as usize * *k as usize, "A of matrix {i}: {} bytes, expected m x k = {}", a.len(), m as usize * *k as usize);
        let cb = std::fs::read(dir.join(format!("c/{i}.i32"))).with_context(|| format!("read C of matrix {i}"))?;
        ensure!(cb.len() == m as usize * *n as usize * 4, "C of matrix {i}: {} bytes, expected m x n x 4", cb.len());
        let c: Vec<i32> = cb.chunks_exact(4).map(|x| i32::from_le_bytes(x.try_into().unwrap())).collect();
        let key = model.job_key(i as u16);
        let a_blob = standing::encode_activation_blob(m, *k, &a)?;
        let spec = SegmentSpec {
            matrix: i as u16,
            m,
            max_blocks: blocks_per_segment.max(1),
            hash_a: commit_a(&key, &a),
            c_hash: result_hash(&key, &c),
            a_uri: pearl_job::data::blob_uri(&pearl_job::data::blob_id(&a_blob)),
        };
        out.push(PreparedSegment { spec, a_blob });
    }
    Ok(out)
}

/// The response blob (`cadre-response-v1`); its blob id is the ticket's result_hash.
pub fn response_blob(model: &ModelInfo, ticket: &[u8; 32], request_hash: &[u8; 32], gen: &Value, prompt_ids: &[u32]) -> Vec<u8> {
    let segs: Vec<Value> = (0..model.matrices.len())
        .map(|i| json!({"matrix": i, "segment_id": hex::encode(standing::segment_id(ticket, i as u16)), "m": gen["rows_padded"]}))
        .collect();
    serde_json::to_vec(&json!({
        "format": "cadre-response-v1", "model_id": hex::encode(model.model_id), "ticket_id": hex::encode(ticket),
        "request_hash": hex::encode(request_hash), "rows": gen["rows"], "prompt_ids": prompt_ids,
        "output_ids": gen["output_ids"], "text": gen["text"], "segments": segs,
    }))
    .unwrap()
}

// ------------------------------------------------------------------ gateway

#[derive(Debug, Clone)]
pub struct Config {
    pub work_dir: PathBuf,
    pub deadline_blocks: u64,
    pub blocks_per_segment: u32,
    pub max_new_tokens: u32,
    pub max_prompt_tokens: u32,
    /// How long settlement waits for the ticket to appear on the chain.
    pub ticket_wait: Duration,
    pub poll: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_dir: PathBuf::from(".cadre-gateway"),
            deadline_blocks: 400,
            blocks_per_segment: 1,
            max_new_tokens: 64,
            max_prompt_tokens: 1024,
            ticket_wait: Duration::from_secs(900),
            poll: Duration::from_millis(1000),
        }
    }
}

pub type Logger = Box<dyn Fn(Value) + Send + Sync>;

pub struct Gateway {
    pub node: Box<dyn NodeApi>,
    pub storefront: Box<dyn Storefront>,
    pub worker: Box<dyn Worker>,
    pub evaluator: Box<dyn Evaluator>,
    pub key: SigningKey,
    pub model: ModelInfo,
    pub cfg: Config,
    pub log: Logger,
    /// Per ticket: request, generation and settlement progress (also saved as JSON).
    pub requests: Mutex<BTreeMap<String, Value>>,
    /// Txs from the server key go out one at a time (nonces).
    send_lock: Mutex<()>,
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Value,
    pub headers: Vec<(String, String)>,
}

fn resp(status: u16, body: Value) -> HttpResponse {
    HttpResponse { status, body, headers: vec![] }
}

impl Gateway {
    #[allow(clippy::too_many_arguments)]
    pub fn new(node: Box<dyn NodeApi>, storefront: Box<dyn Storefront>, worker: Box<dyn Worker>, evaluator: Box<dyn Evaluator>, key: SigningKey, model: ModelInfo, cfg: Config, log: Logger) -> Self {
        Self { node, storefront, worker, evaluator, key, model, cfg, log, requests: Mutex::new(BTreeMap::new()), send_lock: Mutex::new(()) }
    }

    pub fn server(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    fn record(&self, ticket: &str, f: impl FnOnce(&mut Value)) {
        let mut g = self.requests.lock().unwrap();
        let v = g.entry(ticket.to_string()).or_insert_with(|| json!({"ticket_id": ticket}));
        f(v);
        let dir = self.cfg.work_dir.join(ticket);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("state.json"), serde_json::to_vec_pretty(v).unwrap_or_default());
    }

    pub fn handle(self: &Arc<Self>, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> HttpResponse {
        let r = match (method, path) {
            ("GET", "/health") => Ok(resp(200, json!({"ok": true, "server": b58(&self.server()), "model_id": hex::encode(self.model.model_id), "model": self.model.name}))),
            ("GET", "/v1/models") => Ok(resp(200, json!({"data": [{"id": hex::encode(self.model.model_id), "name": self.model.name,
                "matrices": self.model.matrices.len(), "macs_per_row": self.model.macs_per_row.to_string(), "recipe": standing::RECIPE_NAME}]}))),
            ("POST", "/v1/completions") => self.completions(headers, body),
            ("GET", p) if p.starts_with("/v1/requests/") => Ok(self.request_status(&p["/v1/requests/".len()..])),
            _ => Ok(resp(404, json!({"error": format!("no route {method} {path}")}))),
        };
        r.unwrap_or_else(|e| resp(400, json!({"error": format!("{e:#}")})))
    }

    fn request_status(&self, ticket: &str) -> HttpResponse {
        let local = self.requests.lock().unwrap().get(ticket).cloned();
        let chain = self.node.call("getTicket", json!({"ticket_id": ticket})).unwrap_or(Value::Null);
        if local.is_none() && chain.is_null() {
            return resp(404, json!({"error": "unknown ticket"}));
        }
        resp(200, json!({"gateway": local, "ticket": chain}))
    }

    fn completions(self: &Arc<Self>, headers: &[(String, String)], body: &[u8]) -> Result<HttpResponse> {
        let b: Value = serde_json::from_slice(body).context("body is not JSON")?;
        let ids: Vec<u32> = match (&b["prompt_ids"], b["prompt"].as_str()) {
            (Value::Array(a), _) => a.iter().map(|x| x.as_u64().map(|y| y as u32).ok_or_else(|| anyhow!("prompt_ids must be integers"))).collect::<Result<_>>()?,
            (_, Some(p)) => self.worker.tokenize(p)?,
            _ => bail!("need prompt or prompt_ids"),
        };
        ensure!(!ids.is_empty() && ids.len() as u32 <= self.cfg.max_prompt_tokens, "prompt must be 1..={} tokens", self.cfg.max_prompt_tokens);
        let max_new = b["max_tokens"].as_u64().unwrap_or(16) as u32;
        ensure!((1..=self.cfg.max_new_tokens).contains(&max_new), "max_tokens must be 1..={}", self.cfg.max_new_tokens);
        let rows = standing::ticket_rows(ids.len() as u32, max_new, self.model.period());
        ensure!(rows <= standing::MAX_TICKET_ROWS as u64, "prompt + max_tokens above {} rows", standing::MAX_TICKET_ROWS);
        let req_blob = request_blob(&self.model.model_id, &ids, max_new);
        let req_hash = pearl_job::data::blob_id(&req_blob);
        let req_uri = self.node.put_blob(&req_blob).context("put the request blob")?;
        let spec = ticket_spec(&self.model, &self.server(), ids.len() as u32, max_new, &req_uri, &req_hash, self.cfg.deadline_blocks, self.cfg.blocks_per_segment);
        let mut sf_body = json!({"spec": spec});
        if let Some(r) = b.get("recipient").filter(|r| !r.is_null()) {
            sf_body["recipient"] = r.clone();
        }
        let xp = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("x-payment")).map(|(_, v)| v.as_str());
        let (status, sv, xpr) = self.storefront.post_job(&sf_body, xp)?;
        let cadre = json!({"model_id": hex::encode(self.model.model_id), "prompt_tokens": ids.len(), "max_tokens": max_new, "rows": rows,
            "request_uri": req_uri, "request_hash": hex::encode(req_hash), "server": b58(&self.server()),
            "mined_macs_max": (rows as u128 * self.model.macs_per_row as u128).to_string()});
        if status != 201 {
            // 402 (pay first) or a storefront error, passed through with the request's terms
            let mut body = sv;
            if let Some(o) = body.as_object_mut() {
                o.insert("cadre".into(), cadre);
            }
            return Ok(resp(status, body));
        }
        let ticket = sv["job_id"].as_str().ok_or_else(|| anyhow!("storefront reply has no job_id: {sv}"))?.to_string();
        let tid: [u8; 32] = hex::decode(&ticket)?.try_into().map_err(|_| anyhow!("job_id"))?;
        self.record(&ticket, |v| {
            v["order"] = sv.clone();
            v["request"] = cadre.clone();
            v["prompt_ids"] = json!(ids);
            v["stage"] = json!("generating");
        });
        (self.log)(json!({"event": "paid", "ticket": ticket, "order": sv["id"], "prompt_tokens": ids.len(), "max_tokens": max_new}));
        let out_dir = self.cfg.work_dir.join(&ticket);
        let t0 = Instant::now();
        let gen = self.worker.generate(&ids, max_new, &out_dir)?;
        let gen_s = t0.elapsed().as_secs_f64();
        self.record(&ticket, |v| {
            v["generation"] = json!({"output_ids": gen["output_ids"], "text": gen["text"], "rows": gen["rows"], "rows_padded": gen["rows_padded"], "timing": gen["timing"], "wall_s": gen_s});
            v["stage"] = json!("settling");
        });
        (self.log)(json!({"event": "generated", "ticket": ticket, "rows": gen["rows"], "tokens": gen["output_ids"].as_array().map(|a| a.len()), "wall_s": gen_s}));
        let me = Arc::clone(self);
        let (t2, g2, ids2) = (ticket.clone(), gen.clone(), ids.clone());
        std::thread::spawn(move || {
            if let Err(e) = me.settle(&tid, &g2, &ids2, &req_hash) {
                (me.log)(json!({"event": "settle_failed", "ticket": t2, "error": format!("{e:#}")}));
                me.record(&t2, |v| {
                    v["stage"] = json!("failed");
                    v["error"] = json!(format!("{e:#}"));
                });
            }
        });
        let mut h = vec![];
        if let Some(x) = xpr {
            h.push(("x-payment-response".to_string(), x));
        }
        Ok(HttpResponse {
            status: 201,
            body: json!({"ticket_id": ticket, "order": sv, "model": self.model.name, "text": gen["text"], "output_ids": gen["output_ids"],
                "rows": gen["rows"], "timing": gen["timing"], "cadre": cadre, "status": format!("/v1/requests/{ticket}")}),
            headers: h,
        })
    }

    /// Send a tx from the server key and wait for it to be included.
    fn send_wait(&self, body: Body, what: &str) -> Result<String> {
        let _g = self.send_lock.lock().unwrap();
        let me = self.server();
        let info = self.node.call("getChainInfo", json!({}))?;
        let params = self.node.call("getParams", json!({}))?;
        let chain_id = info["chain_id"].as_u64().ok_or_else(|| anyhow!("no chain_id"))? as u32;
        let fee = pearl_node::num(&params["min_fee"]).unwrap_or(1000);
        let nonce = self.node.get_account(&me)?.nonce;
        let wire = tx::sign(&Tx { chain_id, from: me, nonce, fee, body }, &self.key)?;
        let h = self.node.send_tx(&wire).with_context(|| format!("send {what}"))?;
        let t0 = Instant::now();
        loop {
            let t = self.node.call("getTx", json!({"tx_hash": h}))?;
            if t["status"] == json!("included") {
                return Ok(h);
            }
            ensure!(t0.elapsed() < Duration::from_secs(300), "{what} tx {h} not included after 300 s");
            std::thread::sleep(self.cfg.poll);
        }
    }

    /// Post the segments, deliver the response, get it attested, complete the ticket.
    pub fn settle(&self, tid: &[u8; 32], gen: &Value, prompt_ids: &[u32], req_hash: &[u8; 32]) -> Result<()> {
        let ticket = hex::encode(tid);
        let t0 = Instant::now();
        // 1. the ticket on the chain (relayer order unlock)
        let job = loop {
            if let Some(j) = self.node.get_job(tid)? {
                break j;
            }
            ensure!(t0.elapsed() < self.cfg.ticket_wait, "ticket not on the chain after {:?}", self.cfg.ticket_wait);
            std::thread::sleep(self.cfg.poll);
        };
        ensure!(job.kind == "ticket", "job {ticket} is a {}, not a ticket", job.kind);
        self.record(&ticket, |v| v["ticket_seen_s"] = json!(t0.elapsed().as_secs_f64()));
        // 2. segments: A blobs, then PostSegments in batches
        let m = gen["rows_padded"].as_u64().ok_or_else(|| anyhow!("generation has no rows_padded"))? as u32;
        let segs = prepare_segments(&self.model, &self.cfg.work_dir.join(&ticket), m, self.cfg.blocks_per_segment)?;
        let mut put = 0;
        for s in &segs {
            let uri = self.node.put_blob(&s.a_blob)?;
            ensure!(uri == s.spec.a_uri, "node stored A under {uri}, expected {}", s.spec.a_uri);
            put += 1;
        }
        let mut txs = vec![];
        for chunk in segs.chunks(48) {
            let body = Body::PostSegments { ticket_id: *tid, segments: chunk.iter().map(|s| s.spec.clone()).collect() };
            txs.push(self.send_wait(body, "PostSegments")?);
        }
        (self.log)(json!({"event": "segments_posted", "ticket": ticket, "segments": segs.len(), "m": m, "a_blobs": put, "txs": txs}));
        self.record(&ticket, |v| {
            v["segments_posted"] = json!(segs.len());
            v["segment_txs"] = json!(txs);
            v["stage"] = json!("delivering");
        });
        // 3. response blob + SubmitResult
        let rb = response_blob(&self.model, tid, req_hash, gen, prompt_ids);
        let ruri = self.node.put_blob(&rb)?;
        let rh = pearl_job::data::blob_id(&rb);
        let h = self.send_wait(Body::SubmitResult { job_id: *tid, result_hash: rh, uri: ruri.clone() }, "SubmitResult")?;
        self.record(&ticket, |v| {
            v["response_uri"] = json!(ruri);
            v["submit_tx"] = json!(h);
            v["stage"] = json!("attesting");
        });
        // 4. evaluator signature, CompleteJob
        let (pk, sig) = self.evaluator.attest(tid, &self.server())?;
        let h = self.send_wait(Body::CompleteJob { job_id: *tid, sigs: vec![(pk, sig)] }, "CompleteJob")?;
        (self.log)(json!({"event": "completed", "ticket": ticket, "complete_tx": h, "settle_s": t0.elapsed().as_secs_f64()}));
        self.record(&ticket, |v| {
            v["complete_tx"] = json!(h);
            v["stage"] = json!("completed");
            v["settle_s"] = json!(t0.elapsed().as_secs_f64());
        });
        Ok(())
    }
}
