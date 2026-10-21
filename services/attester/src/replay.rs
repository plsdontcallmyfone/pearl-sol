//! Full replay of an inference ticket (SPEC 4.7, docs/EVALUATORS.md "Replay"): for every round,
//! the evaluator rebuilds every segment's A from the requests' token ids and the previous
//! segments' C (cadre-glue-v1, crates/cadre-glue), requires each posted activation blob to equal
//! it bit for bit, computes C = A B from its own copy of the registered weights, requires the
//! segment's committed c_hash, and finally checks every request's output tokens, stopping rule,
//! output_sha256 and text_sha256. One pass: each segment's GEMM is computed once.
//!
//! A failure is a [`TicketFraud`] carrying a `cadre-fraud-v1` proof (crates/cadre-glue fraud.rs):
//! the server is provably at fault, so the evaluator rejects the delivery (RejectResult).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::{anyhow, ensure, Context, Result};
use cadre_glue::fraud::{FraudProof, FORMAT};
use cadre_glue::replay::{first_diff, Mismatch, RequestClaim};
use cadre_glue::tokenizer::Tokenizer;
use cadre_glue::{forward_round, gemm_bt, GemmSink, Model, GLUE_ID};
use pearl_job::freivalds::result_hash;
use pearl_job::standing::{self, ModelStore};
use pearl_node::{JobInfo, NodeApi};
use serde_json::Value;

/// The ticket's server is provably at fault; the proof says where.
#[derive(Debug, Clone)]
pub struct TicketFraud(pub FraudProof);

impl std::fmt::Display for TicketFraud {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "replay fraud ({}): {}", self.0.kind, self.0.reason)
    }
}
impl std::error::Error for TicketFraud {}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplayReport {
    pub rounds: usize,
    pub requests: usize,
    pub rows: usize,
    pub segments: usize,
    pub secs: f64,
    pub gemm_secs: f64,
}

type Loaded = (Arc<Model>, Arc<Tokenizer>);

fn cache() -> &'static Mutex<HashMap<String, Loaded>> {
    static C: OnceLock<Mutex<HashMap<String, Loaded>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The model directory in the store, complete (manifest, every weight and tensor, tokenizer),
/// fetched from the node's blobs when missing (each file checked against the manifest).
pub fn model_dir(node: &dyn NodeApi, store: &ModelStore, model_hex: &str) -> Result<PathBuf> {
    let id: [u8; 32] = hex::decode(model_hex)?.try_into().map_err(|_| anyhow!("model id is not 32 bytes"))?;
    let dir = store.dir(&id);
    let mp = dir.join("manifest.json");
    let manifest: standing::Manifest = if let Ok(b) = std::fs::read(&mp) {
        serde_json::from_slice(&b).context("parse manifest.json")?
    } else {
        let m = node.call("getModel", serde_json::json!({"model_id": model_hex}))?;
        let uri = m["manifest_uri"].as_str().ok_or_else(|| anyhow!("getModel has no manifest_uri"))?;
        let b = pearl_node::fetch_blob(node, uri)?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&mp, &b)?;
        serde_json::from_slice(&b).context("parse model manifest")?
    };
    for name in manifest.files.keys() {
        pearl_node::fetch_model_file(node, &manifest, name, &dir)?;
    }
    Ok(dir)
}

fn load(node: &dyn NodeApi, store: &ModelStore, model_hex: &str) -> Result<Loaded> {
    if let Some(x) = cache().lock().unwrap().get(model_hex) {
        return Ok(x.clone());
    }
    let dir = model_dir(node, store, model_hex)?;
    let l = (Arc::new(Model::load(&dir)?), Arc::new(Tokenizer::load(&dir)?));
    cache().lock().unwrap().insert(model_hex.to_string(), l.clone());
    Ok(l)
}

/// Does this response blob carry the replay bundle?
pub fn replayable(r: &Value) -> bool {
    r["glue"] == serde_json::json!(GLUE_ID)
}

struct Sink<'a> {
    node: &'a dyn NodeApi,
    model: &'a Model,
    ticket: [u8; 32],
    round: u16,
    fraud: Option<(String, usize, Option<Mismatch>, String, String)>,
    segments: usize,
    gemm_secs: f64,
}

#[derive(Debug)]
struct Stop;
impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stop")
    }
}
impl std::error::Error for Stop {}

impl GemmSink for Sink<'_> {
    fn gemm(&mut self, index: usize, a: &[i8], m: usize, k: usize, n: usize) -> Result<Vec<i32>> {
        let sid = standing::segment_id(&self.ticket, index as u16, self.round);
        let seg: JobInfo = self.node.get_job(&sid)?.ok_or_else(|| anyhow!("segment {} (round {}, matrix {index}) not found", hex::encode(sid), self.round))?;
        ensure!(seg.m as usize == m && seg.config.common_dim as usize == k && seg.n as usize == n, "segment {} is {}x{}x{}, the model's matrix {index} needs {m}x{k}x{n}", hex::encode(sid), seg.m, seg.config.common_dim, seg.n);
        let posted = pearl_node::load_segment_a(self.node, &seg)?;
        let key = seg.to_job().job_key();
        let bt = self.model.bt(index).ok_or_else(|| anyhow!("model has no matrix {index}"))?;
        // our weights are the registered ones (the segment's hash_b)
        ensure!(standing::commit_bt(&key, bt) == seg.hash_b, "local weights of matrix {index} do not match the registered hash_b");
        // the blob is the segment's committed A (hash_a), so a proof built on it binds to the chain
        ensure!(pearl_job::commit_a(&key, &posted) == seg.hash_a, "segment {}: activation blob does not match hash_a", hex::encode(sid));
        if let Some(d) = first_diff(index, &posted, a, k) {
            self.fraud = Some(("activation".into(), index, Some(d), hex::encode(sid), seg.data_uri.clone()));
            return Err(Stop.into());
        }
        let t = Instant::now();
        let c = gemm_bt(&posted, m, k, bt, n);
        self.gemm_secs += t.elapsed().as_secs_f64();
        if Some(result_hash(&key, &c)) != seg.result_hash {
            self.fraud = Some(("product".into(), index, None, hex::encode(sid), seg.data_uri.clone()));
            return Err(Stop.into());
        }
        self.segments += 1;
        Ok(c)
    }
}

/// Replay every round of a delivered ticket. `r` is its response blob (already matched to the
/// ticket's result_hash, model, request and rounds by the caller).
pub fn replay_ticket(node: &dyn NodeApi, store: &ModelStore, ticket: &JobInfo, model_hex: &str, r: &Value) -> Result<ReplayReport> {
    let t0 = Instant::now();
    let (model, tok) = load(node, store, model_hex)?;
    let mut rep = ReplayReport { rounds: 0, requests: 0, rows: 0, segments: 0, secs: 0.0, gemm_secs: 0.0 };
    let fraud = |kind: &str, round: u16, matrix: Option<u16>, seg: Option<String>, a_uri: Option<String>, req: RequestClaim, mx: Option<Mismatch>, reason: String| {
        anyhow::Error::new(TicketFraud(FraudProof {
            format: FORMAT.into(),
            glue: GLUE_ID.into(),
            kind: kind.into(),
            model_id: model_hex.into(),
            ticket_id: hex::encode(ticket.job_id),
            round,
            matrix,
            segment_id: seg,
            response_uri: ticket.result_uri.clone(),
            a_uri,
            request: req,
            mismatch: mx,
            reason,
        }))
    };
    for x in r["rounds"].as_array().cloned().unwrap_or_default() {
        let round = x["round"].as_u64().ok_or_else(|| anyhow!("round without a number"))? as u16;
        let m = x["m"].as_u64().unwrap_or(0) as usize;
        let mut reqs: Vec<RequestClaim> = Vec::new();
        let mut off = 0u32;
        for q in x["requests"].as_array().cloned().unwrap_or_default() {
            let c: RequestClaim = serde_json::from_value(q.clone()).context("a request in the response lacks the replay bundle (prompt_ids, output_ids, max_new_tokens)")?;
            let rows = c.rows().len() as u32;
            if q["row_offset"].as_u64() != Some(off as u64) || q["rows"].as_u64() != Some(rows as u64) {
                return Err(fraud("tokens", round, None, None, None, c, None, format!("receipt rows / row_offset ({:?} / {:?}) do not match the bundle's {rows} rows at offset {off}", q["rows"], q["row_offset"])));
            }
            off += rows;
            reqs.push(c);
        }
        ensure!(!reqs.is_empty(), "round {round} has no requests");
        let rows: Vec<Vec<u32>> = reqs.iter().map(|c| c.rows()).collect();
        let mut sink = Sink { node, model: &model, ticket: ticket.job_id, round, fraud: None, segments: 0, gemm_secs: 0.0 };
        let res = forward_round(&model, &rows, m, &mut sink);
        rep.gemm_secs += sink.gemm_secs;
        rep.segments += sink.segments;
        if let Some((kind, matrix, mx, sid, a_uri)) = sink.fraud.take() {
            // name the request that owns the first differing row
            let row = mx.as_ref().map(|d| d.row).unwrap_or(0);
            let mut acc = 0usize;
            let req = reqs.iter().find(|c| {
                acc += c.rows().len();
                row < acc
            });
            let req = req.cloned().unwrap_or_else(|| reqs[0].clone());
            let name = cadre_glue_name(&model, matrix);
            let reason = match &mx {
                Some(d) => format!("round {round} segment {sid} (matrix {matrix}, {name}): posted A differs from the cadre-glue-v1 A at row {} col {}: {} vs {} ({} elements differ); request {}", d.row, d.col, d.posted, d.expected, d.differing, req.request_id),
                None => format!("round {round} segment {sid} (matrix {matrix}, {name}): committed C is not A B"),
            };
            return Err(fraud(&kind, round, Some(matrix as u16), Some(sid), Some(a_uri), req, mx, reason));
        }
        let predicted = match res {
            Ok(p) => p,
            Err(e) if e.downcast_ref::<Stop>().is_some() => unreachable!("a stop always records its fraud"),
            Err(e) => return Err(e),
        };
        for (c, p) in reqs.iter().zip(&predicted) {
            if let Err(why) = c.check(p, &tok, &model.eos)? {
                return Err(fraud("tokens", round, None, None, None, c.clone(), None, format!("round {round}, request {}: {why}", c.request_id)));
            }
        }
        rep.rounds += 1;
        rep.requests += reqs.len();
        rep.rows += off as usize;
    }
    rep.secs = t0.elapsed().as_secs_f64();
    Ok(rep)
}

fn cadre_glue_name(model: &Model, index: usize) -> String {
    std::fs::read(model.dir.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["matrices"][index]["name"].as_str().map(|s| s.to_string()))
        .unwrap_or_default()
}
