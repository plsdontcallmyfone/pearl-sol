//! Reference evaluator (docs/EVALUATORS.md): the open, runnable form of the attester's
//! check. Anyone can run it with their own key; a buyer names that key in a job's
//! evaluator list (PostJob, storefront `spec.evaluators`, `pearl job post --evaluator`).
//!
//! Loop: list Delivered jobs on the node, keep those naming this key as an evaluator,
//! check each delivery once, and:
//! - ACCEPT: sign `pearl-sol:result:v1 || job_id || deliverer || result_hash`; if this key
//!   alone meets the job's threshold (or peers' published signatures complete it), send
//!   CompleteJob.
//! - REJECT: only on a definitive failure of the deliverer (the delivered C is the wrong
//!   size, does not hash to the job's result_hash, or fails Freivalds against the job's own
//!   committed A and B; or the delivery's data stayed unavailable for `unavailable_blocks`
//!   blocks after it was first seen). Sign `pearl-sol:reject:v1 || ...` and, when the
//!   threshold is met, send RejectResult (SPEC 4.1): the job reopens and the failure is on
//!   the deliverer's record.
//! - HOLD: anything that is not the deliverer's provable fault (node or blob fetch errors,
//!   the poster's own data missing or not matching its commitments). Retried next round.
//!
//! Inference tickets are accepted when the full ticket check passes (SPEC 5.4). A ticket whose
//! response carries the cadre-glue-v1 replay bundle is replayed in full (SPEC 4.7, replay.rs):
//! every segment's A rebuilt and compared bit for bit, every C and c_hash, the tokens, the stop
//! rule and the text hash; a replay mismatch is the server's provable fault, so the ticket is
//! REJECTED and the cadre-fraud-v1 proof published at `GET /proofs/<ticket_id hex>`. Any other
//! failure (missing data, fetch errors, old responses without the bundle failing the GEMM-only
//! check) holds. Segments have no delivery.
//!
//! Every verdict is published at `GET /verdicts` and `GET /signatures/<job_id hex>` so
//! other evaluators of a threshold job can collect the signatures (`--peer`).

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::{Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pearl_job::freivalds::{freivalds, result_hash};
use pearl_job::messages::result_message;
use pearl_node::{tx as ntx, JobInfo, NodeApi};

use crate::{Attester, JobSource};

/// RejectResult tx kind (SPEC 2).
pub const KIND_REJECT_RESULT: u8 = 13;
/// RejectResult evaluator domain (SPEC 4.1).
pub const DOMAIN_REJECT: &[u8] = b"pearl-sol:reject:v1";

/// `b"pearl-sol:reject:v1" || job_id || deliverer || result_hash` (raw ed25519).
pub fn reject_message(job_id: &[u8; 32], deliverer: &[u8; 32], result_hash: &[u8; 32]) -> Vec<u8> {
    [DOMAIN_REJECT, &job_id[..], &deliverer[..], &result_hash[..]].concat()
}

/// RejectResult body: `job_id [32] | deliverer [32] | result_hash [32] | count u8 | (pubkey, sig)*`.
pub fn reject_body(job_id: &[u8; 32], deliverer: &[u8; 32], result_hash: &[u8; 32], sigs: &[([u8; 32], [u8; 64])]) -> Vec<u8> {
    let mut v = Vec::with_capacity(97 + 96 * sigs.len());
    v.extend_from_slice(job_id);
    v.extend_from_slice(deliverer);
    v.extend_from_slice(result_hash);
    v.push(sigs.len() as u8);
    for (pk, s) in sigs {
        v.extend_from_slice(pk);
        v.extend_from_slice(s);
    }
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    Reject(String),
    Hold(String),
}

/// One signed verdict on one delivery, as published to peers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signed {
    pub job_id: String,
    pub deliverer: String,
    pub result_hash: String,
    pub decision: Decision,
    pub reason: String,
    pub evaluator: String,
    /// The exact signed bytes (result or reject message), hex.
    pub message_hex: String,
    pub signature_hex: String,
    /// Tx hash when this evaluator sent the CompleteJob / RejectResult itself.
    pub tx_hash: Option<String>,
}

pub struct EvalConfig {
    /// Send CompleteJob / RejectResult when the threshold is met (needs a fee balance).
    pub submit: bool,
    /// Blocks after first seeing a delivery whose result data cannot be fetched before it
    /// is rejected as unavailable (0 = never reject for unavailability).
    pub unavailable_blocks: u64,
    /// Other evaluators' base URLs (their `GET /signatures/<job>`).
    pub peers: Vec<String>,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self { submit: true, unavailable_blocks: 20, peers: vec![] }
    }
}

pub struct Evaluator {
    pub att: Attester,
    pub cfg: EvalConfig,
    /// key = job_id || deliverer || result_hash (one verdict per delivery)
    verdicts: Mutex<HashMap<[u8; 96], Signed>>,
    /// first height a delivery's result could not be fetched
    unavailable_since: Mutex<HashMap<[u8; 96], u64>>,
    /// cadre-fraud-v1 proofs behind ticket rejections, by ticket id hex (GET /proofs/<id>)
    proofs: Mutex<HashMap<String, cadre_glue::fraud::FraudProof>>,
}

fn dkey(j: &[u8; 32], d: &[u8; 32], r: &[u8; 32]) -> [u8; 96] {
    let mut k = [0u8; 96];
    k[..32].copy_from_slice(j);
    k[32..64].copy_from_slice(d);
    k[64..].copy_from_slice(r);
    k
}

fn b58(b: &[u8]) -> String {
    bs58::encode(b).into_string()
}

impl Evaluator {
    pub fn new(att: Attester, cfg: EvalConfig) -> Result<Self> {
        anyhow::ensure!(matches!(att.source, JobSource::Node(_)), "the evaluator needs node mode");
        Ok(Self { att, cfg, verdicts: Mutex::new(HashMap::new()), unavailable_since: Mutex::new(HashMap::new()), proofs: Mutex::new(HashMap::new()) })
    }

    fn node(&self) -> &dyn NodeApi {
        match &self.att.source {
            JobSource::Node(n) => n.as_ref(),
            _ => unreachable!("checked in new"),
        }
    }

    pub fn pubkey(&self) -> [u8; 32] {
        self.att.key.verifying_key().to_bytes()
    }

    /// Check one Delivered job (plain or ticket). Pure apart from node reads.
    pub fn check(&self, job: &JobInfo, height: u64) -> Verdict {
        let (Some(deliverer), Some(rh)) = (job.deliverer, job.result_hash) else {
            return Verdict::Hold("job has no delivery".into());
        };
        if job.kind == "ticket" {
            return match self.att.check_ticket(self.node(), job, &job.job_id) {
                Ok(h) if h == rh => Verdict::Accept,
                Ok(_) => Verdict::Hold("ticket check signed another result_hash".into()),
                // a cadre-glue-v1 replay mismatch is the server's provable fault (SPEC 4.7): reject,
                // and publish the fraud proof at GET /proofs/<ticket>
                Err(e) if e.downcast_ref::<crate::replay::TicketFraud>().is_some() => {
                    let f = e.downcast_ref::<crate::replay::TicketFraud>().unwrap().0.clone();
                    let reason = format!("replay: {}", f.reason);
                    self.proofs.lock().unwrap().insert(hex::encode(job.job_id), f);
                    Verdict::Reject(reason)
                }
                Err(e) => Verdict::Hold(format!("ticket check did not pass (held, not the server's provable fault): {e:#}")),
            };
        }
        if job.kind == "segment" {
            return Verdict::Hold("a segment has no delivery".into());
        }
        let k = dkey(&job.job_id, &deliverer, &rh);
        let node = self.node();
        // The poster's data: if it is missing or does not match the job, that is not the
        // deliverer's fault, so hold.
        let d = match pearl_node::load_job_data(node, &job.data_uri) {
            Ok(d) => d,
            Err(e) => return Verdict::Hold(format!("job data not available: {e:#}")),
        };
        let pj = job.to_job();
        let (m, n, kk) = (pj.m as usize, pj.n as usize, pj.config.k());
        if d.manifest.m != pj.m || d.manifest.n != pj.n || d.manifest.k as usize != kk {
            return Verdict::Hold("job data dims differ from the job (poster's fault)".into());
        }
        let key = pj.job_key();
        let (ha, hb) = pearl_job::commit(&key, &d.a, &d.b, kk, n);
        if ha != pj.hash_a || hb != pj.hash_b {
            return Verdict::Hold("job data does not match the job's commitments (poster's fault)".into());
        }
        // The deliverer's C.
        let c = match fetch_result(node, &job.result_uri, m * n) {
            Ok(Ok(c)) => c,
            Ok(Err(bad)) => return Verdict::Reject(bad),
            Err(e) => {
                let since = *self.unavailable_since.lock().unwrap().entry(k).or_insert(height);
                if self.cfg.unavailable_blocks > 0 && height >= since + self.cfg.unavailable_blocks {
                    return Verdict::Reject(format!("result data unavailable for {} blocks: {e:#}", height - since));
                }
                return Verdict::Hold(format!("result data not available yet: {e:#}"));
            }
        };
        if result_hash(&key, &c) != rh {
            return Verdict::Reject("delivered C does not hash to the job's result_hash".into());
        }
        if !freivalds(&d.a, &d.b, &c, m, n, kk, self.att.freivalds_rounds.max(1), &mut rand::rng()) {
            return Verdict::Reject("Freivalds check failed: C != A B".into());
        }
        Verdict::Accept
    }

    fn sign(&self, job: &JobInfo, decision: Decision, reason: String) -> Signed {
        let (d, rh) = (job.deliverer.unwrap(), job.result_hash.unwrap());
        let msg = match decision {
            Decision::Accept => result_message(&job.job_id, &d, &rh),
            Decision::Reject => reject_message(&job.job_id, &d, &rh),
        };
        Signed {
            job_id: hex::encode(job.job_id),
            deliverer: b58(&d),
            result_hash: hex::encode(rh),
            decision,
            reason,
            evaluator: b58(&self.pubkey()),
            message_hex: hex::encode(&msg),
            signature_hex: hex::encode(self.att.key.sign(&msg).to_bytes()),
            tx_hash: None,
        }
    }

    /// Signatures for the same delivery and decision from this key and the peers, checked.
    fn collect(&self, job: &JobInfo, mine: &Signed, peer_sigs: &[Signed]) -> Vec<([u8; 32], [u8; 64])> {
        let msg = hex::decode(&mine.message_hex).unwrap_or_default();
        let mut out: Vec<([u8; 32], [u8; 64])> = Vec::new();
        for s in std::iter::once(mine).chain(peer_sigs.iter()) {
            if s.decision != mine.decision || s.message_hex != mine.message_hex {
                continue;
            }
            let Ok(pk) = bs58::decode(&s.evaluator).into_vec() else { continue };
            let Ok(pk) = <[u8; 32]>::try_from(pk.as_slice()) else { continue };
            if !job.evaluators.contains(&pk) || out.iter().any(|(p, _)| *p == pk) {
                continue;
            }
            let Ok(sig) = hex::decode(&s.signature_hex) else { continue };
            let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else { continue };
            let ok = VerifyingKey::from_bytes(&pk)
                .ok()
                .map(|vk| vk.verify(&msg, &ed25519_dalek::Signature::from_bytes(&sig)).is_ok())
                .unwrap_or(false);
            if ok {
                out.push((pk, sig));
            }
        }
        out
    }

    fn peer_sigs(&self, job_id: &[u8; 32]) -> Vec<Signed> {
        let mut v = Vec::new();
        for p in &self.cfg.peers {
            let url = format!("{}/signatures/{}", p.trim_end_matches('/'), hex::encode(job_id));
            match ureq::get(&url).timeout(std::time::Duration::from_secs(5)).call() {
                Ok(r) => {
                    if let Ok(list) = r.into_json::<Vec<Signed>>() {
                        v.extend(list);
                    }
                }
                Err(e) => eprintln!("peer {p}: {e}"),
            }
        }
        v
    }

    fn submit(&self, job: &JobInfo, decision: Decision, sigs: &[([u8; 32], [u8; 64])]) -> Result<String> {
        let node = self.node();
        let info = node.call("getChainInfo", json!({}))?;
        let chain_id = info["chain_id"].as_u64().ok_or_else(|| anyhow!("getChainInfo has no chain_id"))? as u32;
        let params = node.call("getParams", json!({}))?;
        let fee = pearl_node::num(&params["min_fee"]).unwrap_or(1000);
        let me = self.pubkey();
        let acc = node.get_account(&me)?;
        anyhow::ensure!(acc.balance >= fee, "evaluator {} cannot pay the fee {fee} (balance {})", b58(&me), acc.balance);
        let body = match decision {
            Decision::Accept => ntx::Body::CompleteJob { job_id: job.job_id, sigs: sigs.to_vec() },
            Decision::Reject => ntx::Body::Raw {
                kind: KIND_REJECT_RESULT,
                body: reject_body(&job.job_id, &job.deliverer.unwrap(), &job.result_hash.unwrap(), sigs),
            },
        };
        let wire = ntx::sign(&ntx::Tx { chain_id, from: me, nonce: acc.nonce, fee, body }, &self.att.key)?;
        node.send_tx(&wire).context("sendTx")
    }

    /// One pass over the node's Delivered jobs. Returns the verdicts made or completed
    /// in this pass.
    pub fn round(&self) -> Result<Vec<Signed>> {
        let node = self.node();
        let height = node.call("getChainInfo", json!({}))?["height"].as_u64().unwrap_or(0);
        let me = self.pubkey();
        let list = node.call("listJobs", json!({"status": "delivered", "limit": 200}))?;
        let mut done = Vec::new();
        for v in list["jobs"].as_array().cloned().unwrap_or_default() {
            let job = match JobInfo::from_json(&v) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("skip job: {e:#}");
                    continue;
                }
            };
            if !job.evaluators.contains(&me) {
                continue;
            }
            let (Some(d), Some(rh)) = (job.deliverer, job.result_hash) else { continue };
            let k = dkey(&job.job_id, &d, &rh);
            let existing = self.verdicts.lock().unwrap().get(&k).cloned();
            let mut signed = match existing {
                Some(s) if s.tx_hash.is_some() => continue,
                Some(s) => s,
                None => match self.check(&job, height) {
                    Verdict::Accept => self.sign(&job, Decision::Accept, "check passed".into()),
                    Verdict::Reject(r) => self.sign(&job, Decision::Reject, r),
                    Verdict::Hold(r) => {
                        eprintln!("hold {}: {r}", hex::encode(job.job_id));
                        continue;
                    }
                },
            };
            if self.cfg.submit {
                let peers = if job.threshold > 1 { self.peer_sigs(&job.job_id) } else { vec![] };
                let sigs = self.collect(&job, &signed, &peers);
                if sigs.len() >= job.threshold.max(1) as usize {
                    match self.submit(&job, signed.decision, &sigs[..job.threshold.max(1) as usize]) {
                        Ok(h) => signed.tx_hash = Some(h),
                        Err(e) => eprintln!("submit {}: {e:#}", hex::encode(job.job_id)),
                    }
                }
            }
            self.verdicts.lock().unwrap().insert(k, signed.clone());
            done.push(signed);
        }
        Ok(done)
    }

    pub fn verdicts(&self) -> Vec<Signed> {
        self.verdicts.lock().unwrap().values().cloned().collect()
    }

    /// HTTP: `/health`, `/verdicts`, `/signatures/<job_id hex>`, plus the attester's
    /// `/result` (sign on request, unchanged).
    pub fn route(&self, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        match (method, path) {
            ("GET", "/health") => (
                200,
                json!({"ok": true, "evaluator": b58(&self.pubkey()), "mode": "evaluator", "submit": self.cfg.submit,
                       "unavailable_blocks": self.cfg.unavailable_blocks, "peers": self.cfg.peers,
                       "spec": "docs/EVALUATORS.md", "verdicts": self.verdicts.lock().unwrap().len()}),
            ),
            ("GET", "/verdicts") => (200, serde_json::to_value(self.verdicts()).unwrap_or(Value::Null)),
            ("GET", p) if p.starts_with("/proofs/") => {
                let id = p.trim_start_matches("/proofs/").to_ascii_lowercase();
                match self.proofs.lock().unwrap().get(&id) {
                    Some(f) => (200, serde_json::to_value(f).unwrap_or(Value::Null)),
                    None => (404, json!({"ok": false, "error": "no fraud proof for this job"})),
                }
            }
            ("GET", p) if p.starts_with("/signatures/") => {
                let id = p.trim_start_matches("/signatures/").to_ascii_lowercase();
                let v: Vec<Signed> = self.verdicts().into_iter().filter(|s| s.job_id == id).collect();
                (200, serde_json::to_value(v).unwrap_or(Value::Null))
            }
            _ => self.att.route(method, path, body),
        }
    }
}

/// `Ok(Ok(C))`, `Ok(Err(reason))` when the bytes exist but are wrong (the deliverer's
/// fault), `Err` when they cannot be fetched (yet).
fn fetch_result(node: &dyn NodeApi, uri: &str, len: usize) -> Result<std::result::Result<Vec<i32>, String>> {
    let bytes = if uri.starts_with(pearl_job::data::BLOB_URI_PREFIX) {
        pearl_node::fetch_blob(node, uri)?
    } else if let Some(p) = uri.strip_prefix("file://") {
        std::fs::read(p).with_context(|| format!("read {p}"))?
    } else if uri.is_empty() {
        return Ok(Err("delivery has no result uri".into()));
    } else {
        return Ok(Err(format!("result uri {uri:?} is not blob: or file://")));
    };
    if bytes.len() != len * 4 {
        return Ok(Err(format!("delivered C is {} bytes, expected m*n*4 = {}", bytes.len(), len * 4)));
    }
    Ok(Ok(bytes.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect()))
}
