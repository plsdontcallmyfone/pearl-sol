//! Attester handlers in request mode (no chain, no socket). Signature bytes are
//! checked against SPEC section 6 layouts written out by hand here.

use base64::Engine as _;
use ed25519_dalek::{Signature, SigningKey, Verifier};
use serde_json::{json, Value};

use pearl_attester::{Attester, JobSource};
use pearl_job::freivalds::{c_to_bytes, matmul, result_hash};
use pearl_job::{data, JobHeader, JobSpec, MiningConfig, PreparedJob};

const EASY: u32 = 0x1e040000;

fn b58(b: &[u8; 32]) -> String {
    bs58::encode(b).into_string()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

struct Fx {
    att: Attester,
    spec: JobSpec,
    prep: PreparedJob,
    job_pk: [u8; 32],
    miner: [u8; 32],
    a: Vec<i8>,
    b: Vec<i8>,
}

fn fixture(tag: &str) -> Fx {
    let (m, n, k) = (64u32, 64u32, 1024u32);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("attester-{tag}"));
    let (a, b) = data::random_matrices(m as usize, n as usize, k as usize, Some(11));
    let mp = data::save(&dir, m, n, k, &a, &b).unwrap();
    let job_pk = [0x21u8; 32];
    let config = MiningConfig::localnet_default();
    let header = JobHeader { job_pubkey: job_pk, terms_hash: [0x33; 32], created_slot: 77, target_bits: EASY };
    let key = pearl_job::job_key(&header, &config);
    let (ha, hb) = pearl_job::commit(&key, &a, &b, k as usize, n as usize);
    let spec = JobSpec {
        job: b58(&job_pk),
        terms_hash: hex::encode([0x33u8; 32]),
        created_slot: 77,
        target_bits: EASY,
        config: hex::encode(config.to_bytes()),
        m,
        n,
        hash_a: hex::encode(ha),
        hash_b: hex::encode(hb),
        data_uri: format!("file://{}", mp.display()),
        max_nonce: 256,
    };
    let prep = PreparedJob::new(spec.to_job().unwrap(), &a, &b).unwrap();
    let att = Attester { key: SigningKey::from_bytes(&[5u8; 32]), source: JobSource::Request, freivalds_rounds: 2 };
    Fx { att, spec, prep, job_pk, miner: [0x44; 32], a, b }
}

fn post(att: &Attester, path: &str, body: Value) -> (u16, Value) {
    att.route("POST", path, body.to_string().as_bytes())
}

fn check_sig(att: &Attester, v: &Value, expect_msg: &[u8]) {
    let msg = hex::decode(v["message_hex"].as_str().unwrap()).unwrap();
    assert_eq!(msg, expect_msg, "signed bytes differ from SPEC section 6");
    let sig: [u8; 64] = hex::decode(v["signature_hex"].as_str().unwrap()).unwrap().try_into().unwrap();
    att.key.verifying_key().verify(expect_msg, &Signature::from_bytes(&sig)).expect("signature verifies");
    assert_eq!(v["attester"].as_str().unwrap(), b58(&att.key.verifying_key().to_bytes()));
}

#[test]
fn share_signature_matches_spec_bytes() {
    let fx = fixture("share");
    let p = fx.prep.mine_range(&fx.miner, 0..64, EASY).expect("share");
    let (st, v) = post(&fx.att, "/share", json!({
        "job": fx.spec.job, "miner": b58(&fx.miner), "proof_b64": b64(&p.to_bytes()), "job_spec": fx.spec,
    }));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["nonce"].as_u64().unwrap(), p.nonce);
    // SPEC 6: b"pearl-sol:share:v1" || job [32] || miner [32] || nonce u64 LE
    let mut expect = b"pearl-sol:share:v1".to_vec();
    expect.extend_from_slice(&fx.job_pk);
    expect.extend_from_slice(&fx.miner);
    expect.extend_from_slice(&p.nonce.to_le_bytes());
    assert_eq!(expect.len(), 90);
    check_sig(&fx.att, &v, &expect);
}

#[test]
fn share_rejections() {
    let fx = fixture("share-bad");
    let p = fx.prep.mine_range(&fx.miner, 0..64, EASY).expect("share");
    let body = |miner: &[u8; 32], proof: Vec<u8>| {
        json!({"job": fx.spec.job, "miner": b58(miner), "proof_b64": b64(&proof), "job_spec": fx.spec})
    };
    // someone else claims the miner's proof
    let (st, v) = post(&fx.att, "/share", body(&[0x55; 32], p.to_bytes()));
    assert_eq!(st, 400);
    assert!(v.get("signature_hex").is_none());
    // tampered leaf
    let mut t = p.clone();
    t.a.leaf_data[1][9] ^= 4;
    assert_eq!(post(&fx.att, "/share", body(&fx.miner, t.to_bytes())).0, 400);
    // nonce over max_nonce
    let mut t = p.clone();
    t.nonce = 10_000;
    let (st, v) = post(&fx.att, "/share", body(&fx.miner, t.to_bytes()));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("max_nonce"));
    // job_spec for a different job
    let (st, _) = post(&fx.att, "/share", json!({"job": b58(&[1; 32]), "miner": b58(&fx.miner),
        "proof_b64": b64(&p.to_bytes()), "job_spec": fx.spec}));
    assert_eq!(st, 400);
    // garbage
    assert_eq!(fx.att.route("POST", "/share", b"{not json").0, 400);
    assert_eq!(fx.att.route("GET", "/nope", b"").0, 404);
    assert_eq!(fx.att.route("GET", "/health", b"").0, 200);
}

#[test]
fn result_signature_matches_spec_bytes() {
    let fx = fixture("result");
    let (m, n, k) = (64, 64, 1024);
    let c = matmul(&fx.a, &fx.b, m, n, k);
    let deliverer = [0x66u8; 32];
    let (st, v) = post(&fx.att, "/result", json!({
        "job": fx.spec.job, "deliverer": b58(&deliverer), "result_b64": b64(&c_to_bytes(&c)), "job_spec": fx.spec,
    }));
    assert_eq!(st, 200, "{v}");
    let rh = result_hash(&fx.prep.job_key, &c);
    assert_eq!(v["result_hash"].as_str().unwrap(), hex::encode(rh));
    // SPEC 6: b"pearl-sol:result:v1" || job [32] || deliverer [32] || result_hash [32]
    let mut expect = b"pearl-sol:result:v1".to_vec();
    expect.extend_from_slice(&fx.job_pk);
    expect.extend_from_slice(&deliverer);
    expect.extend_from_slice(&rh);
    assert_eq!(expect.len(), 115);
    check_sig(&fx.att, &v, &expect);

    // also via result_uri
    let cpath = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("attester-result/C.i32");
    data::write_i32(&cpath, &c).unwrap();
    let (st, v2) = post(&fx.att, "/result", json!({
        "job": fx.spec.job, "deliverer": b58(&deliverer), "result_uri": format!("file://{}", cpath.display()), "job_spec": fx.spec,
    }));
    assert_eq!(st, 200, "{v2}");
    assert_eq!(v2["message_hex"], v["message_hex"]);
}

#[test]
fn result_rejections() {
    let fx = fixture("result-bad");
    let (m, n, k) = (64, 64, 1024);
    let mut c = matmul(&fx.a, &fx.b, m, n, k);
    c[100] += 1;
    let (st, v) = post(&fx.att, "/result", json!({
        "job": fx.spec.job, "deliverer": b58(&[0x66; 32]), "result_b64": b64(&c_to_bytes(&c)), "job_spec": fx.spec,
    }));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("Freivalds"), "{v}");
    // wrong size
    let (st, _) = post(&fx.att, "/result", json!({
        "job": fx.spec.job, "deliverer": b58(&[0x66; 32]), "result_b64": b64(&[0u8; 12]), "job_spec": fx.spec,
    }));
    assert_eq!(st, 400);
    // data swapped under the job: commitments no longer match
    let mut spec = fx.spec.clone();
    spec.hash_a = hex::encode([0u8; 32]);
    let good = matmul(&fx.a, &fx.b, m, n, k);
    let (st, v) = post(&fx.att, "/result", json!({
        "job": spec.job, "deliverer": b58(&[0x66; 32]), "result_b64": b64(&c_to_bytes(&good)), "job_spec": spec,
    }));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("commitments"), "{v}");
}

// ------------------------------------------------------------------ node mode (v2)

use pearl_node::mock::MockNode;
use pearl_node::JobInfo;

/// The fixture job registered on a mock node as Delivered by `deliverer` with C's hash.
fn node_fixture(tag: &str, evaluator: [u8; 32]) -> (Fx, MockNode, Vec<i32>, std::path::PathBuf) {
    let fx = fixture(tag);
    let c = matmul(&fx.a, &fx.b, 64, 64, 1024);
    let cpath = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("attester-{tag}/C.i32"));
    data::write_i32(&cpath, &c).unwrap();
    let job = fx.spec.to_job().unwrap();
    let node = MockNode::new(1, EASY);
    node.add_job(JobInfo {
        job_id: fx.job_pk,
        poster: [1; 32],
        terms_hash: job.header.terms_hash,
        created_height: job.header.created_slot,
        share_target: job.header.target_bits,
        config: job.config,
        m: 64,
        n: 64,
        hash_a: job.hash_a,
        hash_b: job.hash_b,
        data_uri: fx.spec.data_uri.clone(),
        status: "Delivered".into(),
        deliverer: Some([0x66; 32]),
        result_hash: Some(result_hash(&fx.prep.job_key, &c)),
        result_uri: format!("file://{}", cpath.display()),
        evaluators: vec![evaluator],
        threshold: 1,
        deadline_height: 100,
    });
    (fx, node, c, cpath)
}

fn node_attester(node: &MockNode) -> Attester {
    Attester { key: SigningKey::from_bytes(&[5u8; 32]), source: JobSource::Node(Box::new(node.clone())), freivalds_rounds: 2 }
}

#[test]
fn node_mode_signs_delivered_result() {
    let me = SigningKey::from_bytes(&[5u8; 32]).verifying_key().to_bytes();
    let (fx, node, c, _) = node_fixture("node-ok", me);
    let att = node_attester(&node);
    // Job id as hex, no result_uri: taken from the job.
    let (st, v) = post(&att, "/result", json!({"job": hex::encode(fx.job_pk), "deliverer": b58(&[0x66; 32])}));
    assert_eq!(st, 200, "{v}");
    let mut expect = b"pearl-sol:result:v1".to_vec();
    expect.extend_from_slice(&fx.job_pk);
    expect.extend_from_slice(&[0x66; 32]);
    expect.extend_from_slice(&result_hash(&fx.prep.job_key, &c));
    check_sig(&att, &v, &expect);
    // Shares are v1 only.
    let (st, v) = post(&att, "/share", json!({"job": fx.spec.job, "miner": b58(&[1; 32]), "proof_b64": ""}));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("v1"));
    assert_eq!(att.route("GET", "/health", b"").1["mode"], "node");
}

#[test]
fn node_mode_rejections() {
    let me = SigningKey::from_bytes(&[5u8; 32]).verifying_key().to_bytes();
    let (fx, node, c, _) = node_fixture("node-bad", me);
    let att = node_attester(&node);
    let req = |d: &[u8; 32]| json!({"job": hex::encode(fx.job_pk), "deliverer": b58(d)});
    // not the deliverer
    let (st, v) = post(&att, "/result", req(&[0x67; 32]));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("deliverer"), "{v}");
    // a C that differs from the delivered one (via result_b64)
    let mut bad = c.clone();
    bad[3] += 1;
    let (st, v) = post(&att, "/result", json!({"job": hex::encode(fx.job_pk), "deliverer": b58(&[0x66; 32]), "result_b64": b64(&c_to_bytes(&bad))}));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("Freivalds"), "{v}");
    // on-chain result_hash differs
    node.state.lock().unwrap().jobs[0].result_hash = Some([9; 32]);
    let (st, v) = post(&att, "/result", req(&[0x66; 32]));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("result_hash"), "{v}");
    // not Delivered
    node.state.lock().unwrap().jobs[0].status = "Open".into();
    let (st, v) = post(&att, "/result", req(&[0x66; 32]));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("not Delivered"), "{v}");
    // unknown job
    let (st, _) = post(&att, "/result", json!({"job": hex::encode([0xee; 32]), "deliverer": b58(&[0x66; 32])}));
    assert_eq!(st, 400);
    // not an evaluator
    let (fx2, node2, _, _) = node_fixture("node-noeval", [0x42; 32]);
    let (st, v) = post(&node_attester(&node2), "/result", json!({"job": hex::encode(fx2.job_pk), "deliverer": b58(&[0x66; 32])}));
    assert_eq!(st, 400);
    assert!(v["error"].as_str().unwrap().contains("evaluator"), "{v}");
}
