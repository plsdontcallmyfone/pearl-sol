//! Gateway pieces in process: the ticket spec and request blob, segment commitments that a
//! miner can mine against the registered matrix, and the x402 pass-through of
//! POST /v1/completions with fake storefront and worker.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::{Job, MiningConfig};
use pearl_job::pow::{mine_header, verify_block, BlockHeader, BlockWork, FallbackParams};
use pearl_job::{data, standing, PreparedJob};
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn model() -> ModelInfo {
    let base = MiningConfig::localnet_default();
    ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "m".into(), base, matrices: vec![(2048, 16), (2048, 8)], macs_per_row: 2048 * 24 }
}

/// What the worker writes: A (m x k) and C = A B for every matrix, with B from `bts`.
fn write_outputs(dir: &Path, model: &ModelInfo, m: u32, bts: &[Vec<i8>]) {
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("c")).unwrap();
    for (i, (k, n)) in model.matrices.iter().enumerate() {
        let (a, _) = data::random_matrices(m as usize, 1, *k as usize, Some(i as u64 + 40));
        let c = pearl_job::freivalds::matmul_bt(&a, &bts[i], m as usize, *n as usize, *k as usize);
        std::fs::write(dir.join(format!("a/{i}.i8")), pearl_job::job::as_u8(&a)).unwrap();
        std::fs::write(dir.join(format!("c/{i}.i32")), pearl_job::freivalds::c_to_bytes(&c)).unwrap();
    }
}

fn bts(model: &ModelInfo) -> Vec<Vec<i8>> {
    model
        .matrices
        .iter()
        .enumerate()
        .map(|(i, (k, n))| {
            let (_, b) = data::random_matrices(1, *n as usize, *k as usize, Some(i as u64));
            pearl_job::job::transpose(&b, *k as usize, *n as usize)
        })
        .collect()
}

#[test]
fn ticket_spec_and_request_blob() {
    let md = model();
    let ids = [128000u32, 791, 6864];
    let b = request_blob(&md.model_id, &ids, 7);
    assert_eq!(b, request_blob(&md.model_id, &ids, 7), "deterministic");
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["format"], "cadre-request-v1");
    assert_eq!(v["prompt_ids"], json!(ids));
    let server = [9u8; 32];
    let h = data::blob_id(&b);
    let s = ticket_spec(&md, &server, 3, 7, "blob:00", &h, 400, 2);
    let cfg = hex::decode(s["config"].as_str().unwrap()).unwrap();
    assert_eq!(standing::parse_ticket_config(&cfg), Some(md.model_id));
    assert_eq!((s["m"].as_u64(), s["n"].as_u64(), s["k"].as_u64()), (Some(3), Some(7), Some(1)));
    assert_eq!(s["hash_b"], json!(hex::encode(server)));
    assert_eq!(s["hash_a"], json!(hex::encode(h)));
    assert_eq!(s["max_blocks"], json!(4));
}

#[test]
fn prepared_segments_are_mineable_against_the_registered_matrices() {
    let md = model();
    let dir = tmp("segs");
    let b = bts(&md);
    write_outputs(&dir, &md, 8, &b);
    let segs = prepare_segments(&md, &dir, 8, 1).unwrap();
    assert_eq!(segs.len(), 2);
    for (i, s) in segs.iter().enumerate() {
        let (k, n) = md.matrices[i];
        let (m2, k2, a) = standing::decode_activation_blob(&s.a_blob).unwrap();
        assert_eq!((m2, k2), (8, k));
        assert_eq!(s.spec.a_uri, data::blob_uri(&data::blob_id(&s.a_blob)));
        // the job a miner sees: registered header + config, hash_b over the stored B^T
        let header = standing::matrix_job_header(&md.model_id, i as u16, k, n);
        let cfg = standing::matrix_config(&md.base, k);
        let key = md.job_key(i as u16);
        let job = Job { header, config: cfg, m: 8, n, hash_a: s.spec.hash_a, hash_b: standing::commit_bt(&key, &b[i]) };
        let prep = PreparedJob::new_bt(job.clone(), &a, b[i].clone()).expect("commitments match");
        assert_eq!(pearl_job::freivalds::result_hash(&prep.job_key, &prep.product()), s.spec.c_hash, "c_hash = keyed hash of A B");
        // a block proof on it verifies
        let hdr = BlockHeader { version: 1, height: 5, target_bits: 0x207fffff, job_id: [7; 32], miner: [8; 32], ..Default::default() };
        let blk = mine_header(&hdr, &BlockWork::Job(&prep), 0..50).unwrap().expect("easy target");
        verify_block(&blk.header, Some(&job), &blk.proof, &FallbackParams::default()).unwrap();
    }
    // a wrong-size output is refused
    std::fs::write(dir.join("c/1.i32"), [0u8; 12]).unwrap();
    assert!(prepare_segments(&md, &dir, 8, 1).is_err());
}

struct FakeSf(Arc<Mutex<Vec<(Value, Option<String>)>>>);
impl Storefront for FakeSf {
    fn post_job(&self, body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        self.0.lock().unwrap().push((body.clone(), x.map(String::from)));
        Ok(match x {
            None => (402, json!({"x402Version": 1, "error": "X-PAYMENT header is required", "accepts": [{"maxAmountRequired": "123"}]}), None),
            Some(_) => (201, json!({"id": "7", "job_id": hex::encode([0x77u8; 32]), "status": "pending"}), Some("receipt".into())),
        })
    }
}

struct FakeWorker(ModelInfo, Vec<Vec<i8>>);
impl Worker for FakeWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path) -> Result<Value> {
        let rows = ids.len() as u32 + max_new - 1;
        let padded = rows.div_ceil(4) * 4;
        write_outputs(out, &self.0, padded, &self.1);
        Ok(json!({"output_ids": vec![5; max_new as usize], "text": "hello", "rows": rows, "rows_padded": padded, "timing": {}}))
    }
}

struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

#[test]
fn completions_pass_x402_through_and_answer_with_tokens() {
    let md = model();
    let dir = tmp("http");
    let calls = Arc::new(Mutex::new(vec![]));
    let node = MockNode::new(1, 0x207fffff);
    let cfg = Config { work_dir: dir.clone(), ticket_wait: Duration::from_millis(200), poll: Duration::from_millis(50), ..Config::default() };
    let gw = Arc::new(Gateway::new(Box::new(node.clone()), Box::new(FakeSf(calls.clone())), Box::new(FakeWorker(md.clone(), bts(&md))), Box::new(NoEval),
        SigningKey::from_bytes(&[3; 32]), md.clone(), cfg, Box::new(|_| {})));
    let body = json!({"prompt": "hi", "max_tokens": 5}).to_string();
    let r = gw.handle("POST", "/v1/completions", &[], body.as_bytes());
    assert_eq!(r.status, 402);
    assert_eq!(r.body["accepts"][0]["maxAmountRequired"], "123");
    assert_eq!(r.body["cadre"]["prompt_tokens"], 3);
    assert_eq!(r.body["cadre"]["rows"], 8);
    // the request blob is on the node, and the spec names it
    let sent = calls.lock().unwrap()[0].0.clone();
    let uri = sent["spec"]["data_uri"].as_str().unwrap().to_string();
    assert!(pearl_node::fetch_blob(&node, &uri).is_ok());
    assert_eq!(sent["spec"]["hash_b"], json!(hex::encode(gw.server())));
    // paid: same spec again (deterministic), then generation
    let r = gw.handle("POST", "/v1/completions", &[("X-Payment".into(), "pay".into())], body.as_bytes());
    assert_eq!(r.status, 201, "{}", r.body);
    assert_eq!(r.body["text"], "hello");
    assert_eq!(r.body["ticket_id"], json!(hex::encode([0x77u8; 32])));
    assert_eq!(r.headers, vec![("x-payment-response".to_string(), "receipt".to_string())]);
    let c = calls.lock().unwrap();
    assert_eq!(c[0].0, c[1].0, "the paid request carries the same spec as the quote");
    assert_eq!(c[1].1.as_deref(), Some("pay"));
    drop(c);
    let st = gw.handle("GET", &format!("/v1/requests/{}", hex::encode([0x77u8; 32])), &[], b"");
    assert_eq!(st.status, 200);
    assert_eq!(st.body["gateway"]["generation"]["text"], "hello");
    // refusals
    assert_eq!(gw.handle("POST", "/v1/completions", &[], br#"{"prompt": "hi", "max_tokens": 0}"#).status, 400);
    assert_eq!(gw.handle("POST", "/v1/completions", &[], b"{}").status, 400);
    assert_eq!(gw.handle("GET", "/nope", &[], b"").status, 404);
}
