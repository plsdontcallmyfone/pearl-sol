//! Security pass V-8: anonymous balance reads (`GET /v1/prepaid/{w}`,
//! `GET /v1/balance?wallet=`) rescan the chain for a wallet at most once per
//! DISCOVER_TTL, so they cannot be used to fan out node calls.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

/// MockNode that counts getAccountTxs calls.
struct CountNode(MockNode, Arc<AtomicUsize>);
impl pearl_node::NodeApi for CountNode {
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
        if method == "getAccountTxs" {
            self.1.fetch_add(1, Ordering::SeqCst);
            return Ok(json!({"txs": []}));
        }
        self.0.call(method, p)
    }
}

struct NoSf;
impl Storefront for NoSf {
    fn post_job(&self, _b: &Value, _x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        anyhow::bail!("not in this test")
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
    fn generate(&self, _i: &[u32], _n: u32, _o: &Path, _g: &GenOpts, _t: &mut dyn FnMut(&str)) -> Result<Value> {
        anyhow::bail!("not in this test")
    }
}
struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

#[test]
fn anonymous_balance_reads_rescan_at_most_once_per_ttl() {
    let base = MiningConfig::localnet_default();
    let md = ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "test-model".into(), base, matrices: vec![(2048, 16)], hash_b: vec![[0; 32]], macs_per_row: 2048 * 16 };
    let n = Arc::new(AtomicUsize::new(0));
    let cfg = Config { work_dir: tmp("discover"), min_free_disk_bytes: 0, ..Config::default() };
    let gw = Arc::new(Gateway::new(Box::new(CountNode(MockNode::new(1, 0x207fffff), n.clone())), Box::new(NoSf), Box::new(NoWorker), Box::new(NoEval),
        SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})));
    let w = bs58::encode([9u8; 32]).into_string();
    for _ in 0..20 {
        assert_eq!(gw.handle("GET", &format!("/v1/prepaid/{w}"), &[], b"").status, 200);
        assert_eq!(gw.handle("GET", &format!("/v1/balance?wallet={w}"), &[], b"").status, 200);
    }
    assert_eq!(n.load(Ordering::SeqCst), 1, "40 anonymous reads of one wallet, one chain scan");
    // another wallet gets its own scan
    let w2 = bs58::encode([8u8; 32]).into_string();
    gw.handle("GET", &format!("/v1/prepaid/{w2}"), &[], b"");
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert!(DISCOVER_TTL <= Duration::from_secs(60));
}
