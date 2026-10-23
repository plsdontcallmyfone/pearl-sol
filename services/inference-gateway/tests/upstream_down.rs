//! A stopped storefront-x402 (or a node that refuses blobs) is the gateway's upstream being
//! down, not a bad request: POST /v1/balance/topup answers 503 upstream_unavailable with
//! Retry-After, not the catch-all 400 "invalid_request_error". On devnet 2026-10-23 04:39-04:59
//! UTC the storefront was stopped for an operator tool and every top-up quote came back 400,
//! which the house judge logged as "top-up quote: status code 400".

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::MiningConfig;
use pearl_job::standing;
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// What HttpStorefront returns when nothing listens on the storefront's port.
struct DownSf;
impl Storefront for DownSf {
    fn post_job(&self, _b: &Value, _x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        anyhow::bail!("storefront-x402: http://127.0.0.1:10662/jobs: Connection Failed: Connect error: Connection refused (os error 111)")
    }
}
/// The storefront answering: its unpaid 402 quote.
struct QuoteSf;
impl Storefront for QuoteSf {
    fn post_job(&self, _b: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        assert!(x.is_none());
        Ok((402, json!({"x402Version": 1, "error": "X-PAYMENT header is required", "accepts": [{"maxAmountRequired": "409600"}]}), None))
    }
}
/// A node whose blob store refuses writes.
struct NoBlobNode(MockNode);
impl pearl_node::NodeApi for NoBlobNode {
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
    fn put_blob(&self, _d: &[u8]) -> Result<String> {
        anyhow::bail!("putBlob: connection refused")
    }
    fn call(&self, method: &str, p: Value) -> Result<Value> {
        self.0.call(method, p)
    }
}
struct NoWorker;
impl Worker for NoWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        anyhow::bail!("not in this test")
    }
    fn chat_ids(&self, _m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        anyhow::bail!("not in this test")
    }
    fn generate(&self, _ids: &[u32], _max_new: u32, _out: &Path, _o: &GenOpts, _on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        anyhow::bail!("not in this test")
    }
}
struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

fn gw(tag: &str, node: Box<dyn pearl_node::NodeApi>, sf: Box<dyn Storefront>) -> Arc<Gateway> {
    let base = MiningConfig::localnet_default();
    let md = ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "test-model".into(), base, matrices: vec![(2048, 16)], hash_b: vec![[0; 32]], macs_per_row: 2048 * 16 };
    let cfg = Config { work_dir: tmp(tag), min_free_disk_bytes: 0, ..Config::default() };
    Arc::new(Gateway::new(node, sf, Box::new(NoWorker), Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})))
}

fn body(r: &HttpResponse) -> Value {
    match &r.body {
        Payload::Json(v) => v.clone(),
        Payload::Stream(_) => panic!("a stream"),
    }
}

fn header<'a>(r: &'a HttpResponse, k: &str) -> Option<&'a str> {
    r.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
}

#[test]
fn topup_with_the_storefront_down_is_503_not_400() {
    let g = gw("updown-sf", Box::new(MockNode::new(1, 0x207fffff)), Box::new(DownSf));
    let r = g.handle("POST", "/v1/balance/topup", &[], br#"{"rows": 2048}"#);
    let v = body(&r);
    assert_eq!(r.status, 503, "{v}");
    assert_eq!(v["error"]["type"], "upstream_unavailable");
    assert!(v["error"]["message"].as_str().unwrap().contains("Connection refused"));
    assert_eq!(header(&r, "retry-after"), Some("30"));
}

#[test]
fn topup_with_the_blob_store_down_is_503() {
    let g = gw("updown-blob", Box::new(NoBlobNode(MockNode::new(1, 0x207fffff))), Box::new(QuoteSf));
    let r = g.handle("POST", "/v1/balance/topup", &[], br#"{"rows": 2048}"#);
    assert_eq!(r.status, 503, "{}", body(&r));
}

#[test]
fn bad_rows_stay_400_and_a_live_storefront_quotes_402() {
    let g = gw("updown-ok", Box::new(MockNode::new(1, 0x207fffff)), Box::new(QuoteSf));
    assert_eq!(g.handle("POST", "/v1/balance/topup", &[], br#"{"rows": 2047}"#).status, 400);
    assert_eq!(g.handle("POST", "/v1/balance/topup", &[], br#"{"rows": 1000000}"#).status, 400);
    assert_eq!(g.handle("POST", "/v1/balance/topup", &[], br#"{"rows": 2048}"#).status, 402);
}
