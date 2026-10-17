//! In-process mock node for tests (no sockets). It hands out templates, checks
//! submitted blocks with `pearl_job::pow::verify_block`, and applies SubmitResult /
//! CompleteJob txs from its mempool when a block is accepted. Not a consensus model:
//! one chain, no reorgs, fake roots. [`MockNode::handle_jsonrpc`] answers one JSON-RPC
//! request with the same handlers, for HTTP tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, ensure, Result};
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{json, Value};

use pearl_job::messages::result_message;
use pearl_job::pow::{block_proof_from_bytes, verify_block, BlockHeader, FallbackParams, MinedBlock};

use crate::tx::{self, Body};
use crate::{header_fields_json, key32, Account, JobInfo, NodeApi, SubmitOutcome, Template};

#[derive(Debug, Clone)]
pub struct MockBlock {
    pub header: BlockHeader,
    pub proof_len: usize,
    pub txs: usize,
}

#[derive(Debug, Default)]
pub struct MockState {
    pub chain_id: u32,
    pub target_bits: u32,
    pub fallback: FallbackParams,
    pub blocks: Vec<MockBlock>,
    pub jobs: Vec<JobInfo>,
    /// Blocks mined per job; a job gets at most `max_blocks_per_job`.
    pub job_blocks: HashMap<[u8; 32], u32>,
    pub max_blocks_per_job: u32,
    pub accounts: HashMap<[u8; 32], Account>,
    pub mempool: Vec<Vec<u8>>,
    pub rejected: Vec<String>,
    /// Advance the tip right after handing out each template (makes them stale).
    pub steal_templates: bool,
    pub templates_served: u64,
}

impl MockState {
    pub fn tip(&self) -> ([u8; 32], u64) {
        self.blocks.last().map(|b| (b.header.block_hash(), b.header.height)).unwrap_or(([0; 32], 0))
    }

    fn next_job(&self) -> Option<&JobInfo> {
        self.jobs
            .iter()
            .find(|j| j.is("Open") && self.job_blocks.get(&j.job_id).copied().unwrap_or(0) < self.max_blocks_per_job)
    }

    fn fake_block(&mut self) {
        let (prev, h) = self.tip();
        let header = BlockHeader { version: 1, height: h + 1, prev_hash: prev, nonce: 0xdead, ..Default::default() };
        self.blocks.push(MockBlock { header, proof_len: 0, txs: 0 });
    }

    /// Apply one tx; Err = dropped.
    fn apply(&mut self, wire: &[u8]) -> Result<()> {
        let (from, _) = tx::verify(wire)?;
        let (t, _) = tx::decode(wire)?;
        ensure!(t.chain_id == self.chain_id, "wrong chain_id");
        let acct = self.accounts.entry(from).or_default();
        ensure!(t.nonce == acct.nonce, "nonce {} != expected {}", t.nonce, acct.nonce);
        acct.nonce += 1;
        match t.body {
            Body::SubmitResult { job_id, result_hash, uri } => {
                let j = self.jobs.iter_mut().find(|j| j.job_id == job_id).ok_or_else(|| anyhow!("no job"))?;
                ensure!(j.is("Open"), "job not Open");
                j.status = "Delivered".into();
                j.deliverer = Some(from);
                j.result_hash = Some(result_hash);
                j.result_uri = uri;
            }
            Body::CompleteJob { job_id, sigs } => {
                let j = self.jobs.iter_mut().find(|j| j.job_id == job_id).ok_or_else(|| anyhow!("no job"))?;
                ensure!(j.is("Delivered"), "job not Delivered");
                let msg = result_message(&job_id, &j.deliverer.unwrap(), &j.result_hash.unwrap());
                let mut ok = 0;
                for (pk, sig) in &sigs {
                    if j.evaluators.contains(pk) && VerifyingKey::from_bytes(pk)?.verify(&msg, &Signature::from_bytes(sig)).is_ok() {
                        ok += 1;
                    }
                }
                ensure!(ok >= j.threshold as usize, "{ok} valid evaluator signatures < threshold {}", j.threshold);
                j.status = "Completed".into();
            }
            other => bail!("mock does not apply {:?}", other.kind()),
        }
        Ok(())
    }
}

/// Cheap to clone; all clones share one state.
#[derive(Clone, Default)]
pub struct MockNode {
    pub state: Arc<Mutex<MockState>>,
}

impl MockNode {
    pub fn new(chain_id: u32, target_bits: u32) -> Self {
        let s = MockState { chain_id, target_bits, max_blocks_per_job: u32::MAX, ..Default::default() };
        Self { state: Arc::new(Mutex::new(s)) }
    }

    pub fn add_job(&self, j: JobInfo) {
        self.state.lock().unwrap().jobs.push(j);
    }

    pub fn job(&self, id: &[u8; 32]) -> Option<JobInfo> {
        self.state.lock().unwrap().jobs.iter().find(|j| &j.job_id == id).cloned()
    }

    pub fn height(&self) -> u64 {
        self.state.lock().unwrap().tip().1
    }

    /// Answer one JSON-RPC request body; returns the response JSON.
    pub fn handle_jsonrpc(&self, body: &[u8]) -> Value {
        let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let p = req.get("params").cloned().unwrap_or(Value::Null);
        let b64 = base64::engine::general_purpose::STANDARD;
        let r: Result<Value> = (|| match req.get("method").and_then(|m| m.as_str()).unwrap_or("") {
            "getBlockTemplate" => {
                let miner = key32(&p["miner"])?;
                let t = if p["job_id"] == "fallback" { self.fallback_template(&miner)? } else { self.block_template(&miner)? };
                let mut v = json!({"header": hex::encode(t.header.to_bytes()), "header_fields": header_fields_json(&t.header),
                                   "height": t.header.height, "target_bits": t.header.target_bits});
                v["job"] = t.job.map(|j| j.to_json()).unwrap_or(Value::Null);
                v["chain_id"] = json!(t.chain_id);
                v["min_fee"] = json!(t.min_fee.to_string());
                v["fallback"] = json!({"m": t.fallback.m, "n": t.fallback.n, "config": hex::encode(t.fallback.config.to_bytes())});
                v["template_id"] = json!(t.template_id);
                Ok(v)
            }
            "submitBlock" => {
                let header = BlockHeader::from_bytes(&hex::decode(p["header"].as_str().unwrap_or(""))?)?;
                let proof = b64.decode(p["proof"].as_str().unwrap_or(""))?;
                let blk = MinedBlock { header, proof };
                match self.submit_block(&self.dummy_template(), &blk)? {
                    SubmitOutcome::Accepted { block_hash } => Ok(json!({"accepted": true, "block_hash": hex::encode(block_hash)})),
                    SubmitOutcome::Rejected { reason } => Ok(json!({"accepted": false, "reason": reason})),
                }
            }
            "getJob" => Ok(self.get_job(&key32(&p["job_id"])?)?.map(|j| j.to_json()).unwrap_or(Value::Null)),
            "getAccount" => {
                let a = self.get_account(&key32(&p["account"])?)?;
                Ok(json!({"account": crate::b58(&key32(&p["account"])?), "balance": a.balance.to_string(), "nonce": a.nonce}))
            }
            "sendTx" => Ok(json!({"tx_hash": self.send_tx(&b64.decode(p["tx"].as_str().unwrap_or(""))?)?})),
            m => bail!("method not found: {m}"),
        })();
        match r {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("{e:#}")}}),
        }
    }

    fn dummy_template(&self) -> Template {
        Template { header: BlockHeader::default(), job: None, template_id: None, chain_id: None, min_fee: 0, fallback: FallbackParams::default(), raw: Value::Null }
    }
}

impl MockNode {
    fn template(&self, payout: &[u8; 32], fallback: bool) -> Result<Template> {
        let mut s = self.state.lock().unwrap();
        let (prev, h) = s.tip();
        let job = if fallback { None } else { s.next_job().cloned() };
        let header = BlockHeader {
            version: 1,
            height: h + 1,
            prev_hash: prev,
            timestamp_ms: 1_760_000_000_000 + h * 60_000,
            target_bits: s.target_bits,
            job_id: job.as_ref().map(|j| j.job_id).unwrap_or([0; 32]),
            miner: *payout,
            tx_root: *blake3::hash(&s.mempool.concat()).as_bytes(),
            state_root: [0x5e; 32],
            withdraw_root: [0; 32],
            proof_hash: [0; 32],
            nonce: 0,
        };
        let t = Template {
            header,
            job,
            template_id: Some(format!("{}", h + 1)),
            chain_id: Some(s.chain_id),
            min_fee: 0,
            fallback: s.fallback,
            raw: Value::Null,
        };
        s.templates_served += 1;
        if s.steal_templates {
            s.fake_block();
        }
        Ok(t)
    }
}

impl NodeApi for MockNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<Template> {
        self.template(payout, false)
    }

    fn fallback_template(&self, payout: &[u8; 32]) -> Result<Template> {
        self.template(payout, true)
    }

    fn submit_block(&self, _t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        let mut s = self.state.lock().unwrap();
        let (prev, h) = s.tip();
        let reject = |s: &mut MockState, r: String| {
            s.rejected.push(r.clone());
            Ok(SubmitOutcome::Rejected { reason: r })
        };
        if b.header.prev_hash != prev || b.header.height != h + 1 {
            return reject(&mut s, "stale: prev_hash / height is not the tip".into());
        }
        if b.header.target_bits != s.target_bits {
            return reject(&mut s, "wrong target_bits".into());
        }
        let job = if b.header.is_fallback() {
            None
        } else {
            match s.jobs.iter().find(|j| j.job_id == b.header.job_id) {
                Some(j) if j.is("Open") || j.is("Delivered") => Some(j.to_job()),
                _ => return reject(&mut s, "job is not minable".into()),
            }
        };
        if let Err(e) = block_proof_from_bytes(&b.proof).and_then(|_| verify_block(&b.header, job.as_ref(), &b.proof, &s.fallback)) {
            return reject(&mut s, format!("bad work: {e:#}"));
        }
        let pool = std::mem::take(&mut s.mempool);
        let mut applied = 0;
        for w in &pool {
            if s.apply(w).is_ok() {
                applied += 1;
            }
        }
        if job.is_some() {
            *s.job_blocks.entry(b.header.job_id).or_default() += 1;
        }
        s.blocks.push(MockBlock { header: b.header, proof_len: b.proof.len(), txs: applied });
        Ok(SubmitOutcome::Accepted { block_hash: b.header.block_hash() })
    }

    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>> {
        Ok(self.job(job_id))
    }

    fn get_account(&self, account: &[u8; 32]) -> Result<Account> {
        let s = self.state.lock().unwrap();
        let mut a = s.accounts.get(account).copied().unwrap_or_default();
        // Pending txs count toward the next nonce, like a mempool-aware node.
        a.nonce += s.mempool.iter().filter(|w| w.len() > 37 && w[5..37] == account[..]).count() as u64;
        Ok(a)
    }

    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        tx::verify(wire)?;
        let mut s = self.state.lock().unwrap();
        s.mempool.push(wire.to_vec());
        Ok(hex::encode(blake3::hash(wire).as_bytes()))
    }
}
