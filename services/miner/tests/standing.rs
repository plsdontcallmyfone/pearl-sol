//! Standing jobs (SPEC 4.5) against the mock node: the miner loads A from the segment's
//! activation blob and B from its model store, mines a verified block, and never tries to
//! deliver a segment. The attester's per-segment check recomputes C = A B.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use pearl_job::freivalds::{matmul_bt, result_hash};
use pearl_job::standing::{self, Aux, ModelStore};
use pearl_job::{data, MiningConfig};
use pearl_miner::{Miner, MinerConfig};
use pearl_node::mock::MockNode;
use pearl_node::{JobInfo, NodeApi, StandingRef};

const EASY: u32 = 0x1e040000;
const CHAIN: u32 = 4243;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("standing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-matrix model in a store at `dir` and a segment of `ticket` on it (A in a blob).
fn segment(dir: &std::path::Path, node: &MockNode, ticket: [u8; 32], c_hash_ok: bool) -> JobInfo {
    let model_id = standing::model_id(&[0x21; 32], b"test-model");
    let (m, n, k) = (8u32, 64u32, 2048u32);
    let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, Some(9));
    let bt = pearl_job::job::transpose(&b, k as usize, n as usize);
    let store = ModelStore::new(dir);
    let w = store.dir(&model_id).join("w");
    std::fs::create_dir_all(&w).unwrap();
    std::fs::write(w.join("0.i8"), pearl_job::job::as_u8(&bt)).unwrap();
    let aux = Aux { n, k, s_w: vec![1.0; n as usize], colsum: standing::colsum_bt(&bt, n as usize, k as usize), smooth: None };
    std::fs::write(w.join("0.aux"), aux.encode()).unwrap();
    let base = MiningConfig::localnet_default();
    let header = standing::matrix_job_header(&model_id, 0, k, n);
    let key = standing::matrix_job_key(&model_id, 0, k, n, &base);
    let (ha, hb) = pearl_job::commit(&key, &a, &b, k as usize, n as usize);
    let c = matmul_bt(&a, &bt, m as usize, n as usize, k as usize);
    let uri = node.put_blob(&standing::encode_activation_blob(m, k, &a).unwrap()).unwrap();
    JobInfo {
        job_id: standing::segment_id(&ticket, 0, 0),
        header_id: model_id,
        poster: [0x10; 32],
        terms_hash: header.terms_hash,
        created_height: 0,
        share_target: 0,
        config: standing::matrix_config(&base, k),
        m,
        n,
        hash_a: ha,
        hash_b: hb,
        data_uri: uri,
        status: "Open".into(),
        deliverer: Some([0x10; 32]),
        result_hash: Some(if c_hash_ok { result_hash(&key, &c) } else { [0xee; 32] }),
        result_uri: String::new(),
        evaluators: vec![],
        threshold: 1,
        deadline_height: 1000,
        kind: "segment".into(),
        standing: Some(StandingRef { ticket_id: ticket, model_id, matrix: 0 }),
    }
}

#[test]
fn miner_mines_a_segment_from_its_model_store_and_does_not_deliver_it() {
    let dir = tmp("mine");
    let node = MockNode::new(CHAIN, EASY);
    let seg = segment(&dir.join("models"), &node, [0x31; 32], true);
    let id = seg.job_id;
    node.add_job(seg);
    let logs = Arc::new(Mutex::new(Vec::<Value>::new()));
    let l2 = logs.clone();
    let cfg = MinerConfig {
        threads: 2,
        refresh: Duration::from_millis(300),
        deliver: true,
        results_dir: dir.join("results"),
        max_blocks: Some(2),
        models_dir: dir.join("models"),
        ..MinerConfig::default()
    };
    let mut miner = Miner::new(&node, None, SigningKey::from_bytes(&[0x4e; 32]), cfg, Box::new(move |v| l2.lock().unwrap().push(v)));
    miner.run(&|| false).unwrap();
    let logs = logs.lock().unwrap();
    let accepted: Vec<&Value> = logs.iter().filter(|v| v["event"] == "block_accepted" && v["job"] == hex::encode(id)).collect();
    assert!(!accepted.is_empty(), "a block was mined on the segment: {logs:?}");
    let loaded = logs.iter().find(|v| v["event"] == "job_loaded").unwrap();
    assert_eq!(loaded["kind"], "segment");
    assert_eq!(loaded["matrix"], 0);
    assert_eq!(miner.pending_deliveries(), 0, "segments are never delivered");
    assert!(node.job(&id).unwrap().is("Open"));
    assert!(!logs.iter().any(|v| v["event"] == "result_submitted"));
}

#[test]
fn miner_refuses_a_model_store_whose_weights_do_not_match_hash_b() {
    let dir = tmp("bad");
    let node = MockNode::new(CHAIN, EASY);
    let seg = segment(&dir.join("models"), &node, [0x32; 32], true);
    // corrupt the stored weights: the commitment check must refuse them
    let p = ModelStore::new(dir.join("models")).dir(&seg.header_id).join("w/0.i8");
    let mut b = std::fs::read(&p).unwrap();
    b[0] = if b[0] == 0 { 1 } else { 0 };
    std::fs::write(&p, &b).unwrap();
    let store = ModelStore::new(dir.join("models"));
    let a = pearl_node::load_segment_a(&node, &seg).unwrap();
    let bt = pearl_node::load_segment_bt(&node, &store, &seg.standing.unwrap(), 2048, 64).unwrap();
    let e = pearl_job::PreparedJob::new_bt(seg.to_job(), &a, bt).err().expect("mismatch refused");
    assert!(e.to_string().contains("hash_b"), "{e}");
}

#[test]
fn attester_segment_check_recomputes_c() {
    let dir = tmp("att");
    let node = MockNode::new(CHAIN, EASY);
    let good = segment(&dir.join("models"), &node, [0x33; 32], true);
    let bad = segment(&dir.join("models"), &node, [0x34; 32], false);
    node.add_job(good.clone());
    node.add_job(bad.clone());
    let store = ModelStore::new(dir.join("models"));
    pearl_attester::check_segment(&node, &store, &good.job_id).unwrap();
    let e = pearl_attester::check_segment(&node, &store, &bad.job_id).unwrap_err();
    assert!(e.to_string().contains("does not equal"), "{e}");
}
