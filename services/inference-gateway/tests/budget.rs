//! Ticket block budget (SPEC 5.4, 2026-10-21): a balance ticket's `max_blocks` scales with the
//! model's matrix count (one segment per matrix per round), is cut to what an order and the
//! chain accept, and the gateway never posts more rounds than the budget holds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::standing;
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gwb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A model of `count` matrices of k 2048 x n 8 (the e2e's scaled Llama-3.1-8B shape).
fn model(count: usize) -> ModelInfo {
    let base = MiningConfig::localnet_default();
    ModelInfo {
        model_id: standing::model_id(&[1; 32], b"m"),
        name: "shape-model".into(),
        base,
        matrices: vec![(2048, 8); count],
        hash_b: vec![[0; 32]; count],
        macs_per_row: 2048 * 8 * count as u64,
    }
}

struct FakeSf;
impl Storefront for FakeSf {
    fn post_job(&self, body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        let mb = body["spec"]["max_blocks"].as_u64().unwrap_or(0);
        Ok(match x {
            None => (402, json!({"x402Version": 1, "accepts": [{"maxAmountRequired": "409600"}]}), None),
            Some(_) => (201, json!({"id": "1", "job_id": hex::encode([mb as u8; 32]), "status": "pending",
                "payment": {"payer": bs58::encode(SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()).into_string(), "amount": "409600"},
                "spec_max_blocks": mb}), None),
        })
    }
}

struct FakeWorker(ModelInfo);
impl Worker for FakeWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn chat_ids(&self, m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        Ok(vec![7; m.as_array().unwrap().len() * 2])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path, _o: &GenOpts, _on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        let rows = ids.len() as u32 + max_new - 1;
        std::fs::create_dir_all(out.join("a"))?;
        std::fs::create_dir_all(out.join("c"))?;
        for (i, (k, n)) in self.0.matrices.iter().enumerate() {
            std::fs::write(out.join(format!("a/{i}.i8")), vec![0u8; rows as usize * *k as usize])?;
            std::fs::write(out.join(format!("c/{i}.i32")), vec![0u8; rows as usize * *n as usize * 4])?;
        }
        Ok(json!({"output_ids": vec![5; max_new as usize], "text": "ok", "rows": rows, "rows_padded": rows,
            "prompt_tokens": ids.len(), "completion_tokens": max_new, "finish_reason": "length", "timing": {}}))
    }
}

struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

/// MockNode with the chain parameters the budget reads.
struct ParamsNode(MockNode, Value);
impl pearl_node::NodeApi for ParamsNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<pearl_node::Template> {
        self.0.block_template(payout)
    }
    fn submit_block(&self, t: &pearl_node::Template, b: &pearl_job::pow::MinedBlock) -> Result<pearl_node::SubmitOutcome> {
        self.0.submit_block(t, b)
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<pearl_node::JobInfo>> {
        self.0.get_job(id)
    }
    fn get_account(&self, a: &[u8; 32]) -> Result<pearl_node::Account> {
        self.0.get_account(a)
    }
    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        self.0.send_tx(wire)
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.0.get_blob(id)
    }
    fn put_blob(&self, d: &[u8]) -> Result<String> {
        self.0.put_blob(d)
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        if method == "getParams" {
            return Ok(self.1.clone());
        }
        self.0.call(method, p)
    }
}

fn gateway(tag: &str, matrices: usize, max_ticket_blocks: u32, max_balance_rows: u64) -> Arc<Gateway> {
    let md = model(matrices);
    let params = json!({"max_balance_rows": 65_536, "max_job_blocks": 100_000, "target_block_time_ms": 60_000, "min_fee": 1000});
    let cfg = Config { work_dir: tmp(tag), ticket_wait: Duration::from_millis(200), poll: Duration::from_millis(50), balance_rows: 64, max_new_tokens: 32,
        max_ticket_blocks, max_balance_rows, ..Config::default() };
    Arc::new(Gateway::new(Box::new(ParamsNode(MockNode::new(1, 0x207fffff), params)), Box::new(FakeSf), Box::new(FakeWorker(md.clone())),
        Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})))
}

#[test]
fn budget_scales_with_the_matrix_count_and_is_cut_to_the_caps() {
    // Llama-3.1-8B shape (640 matrices) at the storefront's initial cap: 15 rounds, not 10,000 / 640
    // segments mid-round (the old fixed 10,000 left a 16th round refused by the chain)
    let gw = gateway("8b", 640, 10_000, 8192);
    assert_eq!(gw.round_blocks(), 640);
    assert_eq!(gw.ticket_budget(4096, None).unwrap(), (9_600, 15));
    // the chain's max_job_blocks (100,000) as the cap: 156 rounds of 60 s
    let gw = gateway("8b-100k", 640, 100_000, 8192);
    assert_eq!(gw.ticket_budget(4096, None).unwrap(), (156 * 640, 156));
    // a large balance at the 3 h round period: rounds over its life (10,000 blocks x 60 s =
    // 600,000 s / 10,800 s = 56, + 1, + 8192 / 2044 rounds its rows can fill early = 61)
    assert_eq!(gw.round_period(8192, None), Duration::from_secs(10_800));
    assert_eq!(gw.ticket_budget(8192, None).unwrap(), (61 * 640, 61));
    // an own period of a day: 7 + 1 + 4 = 12 rounds
    assert_eq!(gw.ticket_budget(8192, Some(86_400)).unwrap(), (12 * 640, 12));
    // Llama-3.2-1B shape (144): 69 rounds at the 10,000 cap
    let gw = gateway("1b", 144, 10_000, 4096);
    assert_eq!(gw.ticket_budget(4096, None).unwrap(), (69 * 144, 69));
    // rows bound the rounds too: 64 rows at row period 4 can make at most 16 rounds
    let p = gw.model.period() as u64;
    assert_eq!(gw.ticket_budget(64, None).unwrap().1 as u64, 64 / p);
    // a cap below one round is refused (raise the cap or lower blocks per segment)
    let gw = gateway("tiny", 640, 500, 4096);
    let e = gw.ticket_budget(4096, None).unwrap_err().to_string();
    assert!(e.contains("640 blocks"), "{e}");
    let r = gw.handle("POST", "/v1/balance/topup", &[("X-PAYMENT".into(), "pay".into())], br#"{"rows": 4096}"#);
    assert_eq!(r.status, 400, "{}", r.json());
}

fn signed(sk: &SigningKey, method: &str, path: &str, body: &[u8]) -> Vec<(String, String)> {
    let ts = now_ms();
    let msg = format!("cadre-auth-v1\n{method}\n{path}\n{ts}\n{}", hex::encode(<sha2::Sha256 as sha2::Digest>::digest(body)));
    let sig = sk.sign(msg.as_bytes());
    vec![
        ("X-Cadre-Wallet".into(), bs58::encode(sk.verifying_key().to_bytes()).into_string()),
        ("X-Cadre-Timestamp".into(), ts.to_string()),
        ("X-Cadre-Signature".into(), hex::encode(sig.to_bytes())),
    ]
}

#[test]
fn the_last_round_takes_only_what_it_can_stack_and_then_the_ticket_is_spent() {
    let gw = gateway("last", 640, 10_000, 8192);
    let buyer = SigningKey::from_bytes(&[5; 32]);
    let r = gw.handle("POST", "/v1/balance/topup", &[("X-PAYMENT".into(), "pay".into())], br#"{"rows": 4096}"#);
    assert_eq!(r.status, 201, "{}", r.json());
    assert_eq!((r.json()["max_rounds"].as_u64(), r.json()["max_blocks"].as_u64()), (Some(15), Some(9_600)));
    let ticket = r.json()["ticket_id"].as_str().unwrap().to_string();
    let wallet = bs58::encode(buyer.verifying_key().to_bytes()).into_string();
    let p = gw.model.period() as u64;
    let cap = gw.round_rows_cap() as u64;
    // 14 of 15 rounds posted (100 rows each): the 15th may stack at most one round of rows
    gw.ledger.lock().unwrap().tickets.get_mut(&ticket).unwrap().rounds =
        (0..14).map(|i| RoundRec { round: i, m: 100, requests: vec![], txs: vec![] }).collect();
    let bal = gw.handle("GET", &format!("/v1/balance?wallet={wallet}"), &[], b"");
    assert_eq!(bal.json()["rows_free"].as_u64(), Some(cap.min(4096 - 1400)), "{}", bal.json());
    let l = gw.ledger.lock().unwrap();
    let t = &l.tickets[&ticket];
    assert_eq!(t.rounds_left(), Some(1));
    assert_eq!(l.free_in_budget(t, p, cap), cap.min(l.free(t, p)));
    drop(l);
    // a request still fits the last round
    let body = br#"{"prompt": "x", "max_tokens": 4}"#;
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", body), body);
    assert_eq!(r.status, 200, "{}", r.json());
    // the 15th round posted: no free rows, a new request is refused with 402 (top up again)
    gw.ledger.lock().unwrap().tickets.get_mut(&ticket).unwrap().rounds.push(RoundRec { round: 14, m: 8, requests: vec![], txs: vec![] });
    gw.ledger.lock().unwrap().tickets.get_mut(&ticket).unwrap().pending.clear();
    let bal = gw.handle("GET", &format!("/v1/balance?wallet={wallet}"), &[], b"");
    assert_eq!(bal.json()["rows_free"], 0, "{}", bal.json());
    let r = gw.handle("POST", "/v1/completions", &signed(&buyer, "POST", "/v1/completions", body), body);
    assert_eq!(r.status, 402, "{}", r.json());
    assert_eq!(r.json()["error"]["type"], "insufficient_balance");
}
