//! Full ticket replay (SPEC 4.7) through the real evaluator, on a real model and real worker
//! output: two served requests stacked in one round (row offsets, zero padding), every segment
//! posted as the gateway posts it (A blob, hash_a, c_hash under the matrix job key).
//!
//! Needs the model and worker runs, so it runs only when these are set (otherwise it prints a
//! skip line and passes):
//!   CADRE_REPLAY_MODEL=<model dir>  CADRE_REPLAY_RUNS=<run dir>,<run dir>
//! where each run dir holds request.json + a/<i>.i8 (tools/cadre/test_replay.py writes them).
//! Checks: honest -> Accept; one segment's A altered with a correct GEMM over it -> Reject
//! naming that segment, proof at GET /proofs/<ticket>, and the proof verifies against the
//! altered blob; a c_hash that is not A B -> Reject (product).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use cadre_glue::replay::RequestClaim;
use pearl_attester::evaluator::{EvalConfig, Evaluator, Verdict};
use pearl_attester::{Attester, JobSource};
use pearl_job::freivalds::result_hash;
use pearl_job::pow::MinedBlock;
use pearl_job::standing::{self, Manifest};
use pearl_node::{Account, JobInfo, NodeApi, StandingRef, SubmitOutcome, Template};

#[derive(Clone, Default)]
struct Node {
    jobs: Arc<Mutex<HashMap<[u8; 32], JobInfo>>>,
    blobs: Arc<Mutex<HashMap<[u8; 32], Vec<u8>>>>,
    ticket: Arc<Mutex<Value>>,
    model: Arc<Mutex<Value>>,
}

impl NodeApi for Node {
    fn block_template(&self, _p: &[u8; 32]) -> Result<Template> {
        bail!("no mining here")
    }
    fn submit_block(&self, _t: &Template, _b: &MinedBlock) -> Result<SubmitOutcome> {
        bail!("no mining here")
    }
    fn get_job(&self, id: &[u8; 32]) -> Result<Option<JobInfo>> {
        Ok(self.jobs.lock().unwrap().get(id).cloned())
    }
    fn get_account(&self, _a: &[u8; 32]) -> Result<Account> {
        Ok(Account { balance: 0, nonce: 0 })
    }
    fn send_tx(&self, _w: &[u8]) -> Result<String> {
        bail!("not sending")
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        Ok(self.blobs.lock().unwrap().get(id).cloned())
    }
    fn put_blob(&self, d: &[u8]) -> Result<String> {
        let id = pearl_job::data::blob_id(d);
        self.blobs.lock().unwrap().insert(id, d.to_vec());
        Ok(pearl_job::data::blob_uri(&id))
    }
    fn call(&self, method: &str, _p: Value) -> Result<Value> {
        match method {
            "getTicket" => Ok(self.ticket.lock().unwrap().clone()),
            "getModel" => Ok(self.model.lock().unwrap().clone()),
            m => bail!("mock has no {m}"),
        }
    }
}

struct Fx {
    node: Node,
    ticket: JobInfo,
    model_id: [u8; 32],
    m: usize,
    a: Vec<Vec<i8>>,
    keys: Vec<[u8; 32]>,
    models: PathBuf,

    model: cadre_glue::Model,
    base: pearl_job::MiningConfig,
}

fn sk() -> SigningKey {
    SigningKey::from_bytes(&[9; 32])
}

fn post_segment(fx: &Fx, i: usize, a: &[i8], c_override: Option<Vec<i32>>) {
    let model = &fx.model;
    let base = fx.base;
    let (k, n) = model.shape(i).unwrap();
    let key = fx.keys[i];
    let c = c_override.unwrap_or_else(|| cadre_glue::gemm_bt(a, fx.m, k, model.bt(i).unwrap(), n));
    let blob = standing::encode_activation_blob(fx.m as u32, k as u32, a).unwrap();
    let uri = fx.node.put_blob(&blob).unwrap();
    let sid = standing::segment_id(&fx.ticket.job_id, i as u16, 0);
    fx.node.jobs.lock().unwrap().insert(
        sid,
        JobInfo {
            job_id: sid,
            header_id: fx.model_id,
            poster: [2; 32],
            terms_hash: standing::matrix_terms_hash(&fx.model_id, i as u16, k as u32, n as u32),
            created_height: 0,
            share_target: 0,
            config: standing::matrix_config(&base, k as u32),
            m: fx.m as u32,
            n: n as u32,
            hash_a: pearl_job::commit_a(&key, a),
            hash_b: standing::commit_bt(&key, model.bt(i).unwrap()),
            data_uri: uri,
            status: "Open".into(),
            deliverer: None,
            result_hash: Some(result_hash(&key, &c)),
            result_uri: String::new(),
            evaluators: vec![],
            threshold: 0,
            deadline_height: 1000,
            kind: "segment".into(),
            standing: Some(StandingRef { ticket_id: fx.ticket.job_id, model_id: fx.model_id, matrix: i as u16 }),
        },
    );
}

fn fixture() -> Option<Fx> {
    let mdir = PathBuf::from(std::env::var("CADRE_REPLAY_MODEL").ok()?);
    let runs: Vec<PathBuf> = std::env::var("CADRE_REPLAY_RUNS").ok()?.split(',').map(PathBuf::from).collect();
    let model = cadre_glue::Model::load(&mdir).unwrap();
    let man = Manifest::load(&mdir).unwrap();
    let base = man.base_config().unwrap();
    let model_id = [0x42u8; 32];
    let models = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay-models");
    let _ = std::fs::remove_dir_all(&models);
    std::fs::create_dir_all(&models).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&mdir, models.join(hex::encode(model_id))).unwrap();
    let claims: Vec<RequestClaim> = runs.iter().map(|r| serde_json::from_slice(&std::fs::read(r.join("request.json")).unwrap()).unwrap()).collect();
    let rows: Vec<usize> = claims.iter().map(|c| c.rows().len()).collect();
    let total: usize = rows.iter().sum();
    let m = total.div_ceil(model.period) * model.period;
    let keys: Vec<[u8; 32]> = (0..model.matrices).map(|i| {
        let (k, n) = model.shape(i).unwrap();
        standing::matrix_job_key(&model_id, i as u16, k as u32, n as u32, &base)
    }).collect();
    let mut a = Vec::new();
    for i in 0..model.matrices {
        let (k, _) = model.shape(i).unwrap();
        let mut v: Vec<i8> = Vec::with_capacity(m * k);
        for (r, n) in runs.iter().zip(&rows) {
            let b = std::fs::read(r.join(format!("a/{i}.i8"))).unwrap();
            v.extend(b[..n * k].iter().map(|&x| x as i8));
        }
        v.resize(m * k, 0);
        a.push(v);
    }
    // the response blob, as close_ticket writes it
    let ticket_id = [0x77u8; 32];
    let mut off = 0;
    let reqs: Vec<Value> = claims
        .iter()
        .zip(&rows)
        .map(|(c, n)| {
            let v = json!({"request_id": c.request_id, "row_offset": off, "rows": n, "prompt_tokens": c.prompt_ids.len(), "completion_tokens": c.output_ids.len(),
                "output_sha256": c.output_sha256, "text_sha256": c.text_sha256, "glue": "cadre-glue-v1", "prompt_ids": c.prompt_ids, "output_ids": c.output_ids,
                "max_new_tokens": c.max_new_tokens, "stop": c.stop});
            off += n;
            v
        })
        .collect();
    let segments: Vec<Value> = (0..model.matrices).map(|i| json!({"round": 0, "matrix": i, "segment_id": hex::encode(standing::segment_id(&ticket_id, i as u16, 0)), "m": m})).collect();
    let resp = json!({"format": "cadre-response-v1", "model_id": hex::encode(model_id), "ticket_id": hex::encode(ticket_id), "request_hash": "ab".repeat(32),
        "rows": m, "rounds": [{"round": 0, "m": m, "requests": reqs}], "segments": segments.clone(), "glue": "cadre-glue-v1"});
    let node = Node::default();
    let rb = serde_json::to_vec(&resp).unwrap();
    let ruri = node.put_blob(&rb).unwrap();
    let me = sk().verifying_key().to_bytes();
    let ticket = JobInfo {
        job_id: ticket_id,
        header_id: ticket_id,
        poster: [1; 32],
        terms_hash: [0; 32],
        created_height: 1,
        share_target: 0,
        config: base,
        m: (m - 1) as u32,
        n: 1,
        hash_a: [0xab; 32],
        hash_b: [0; 32],
        data_uri: String::new(),
        status: "Delivered".into(),
        deliverer: Some([0x55; 32]),
        result_hash: Some(pearl_job::data::blob_id(&rb)),
        result_uri: ruri,
        evaluators: vec![me],
        threshold: 1,
        deadline_height: 1000,
        kind: "ticket".into(),
        standing: None,
    };
    *node.ticket.lock().unwrap() = json!({"standing": {"model_id": hex::encode(model_id), "request_hash": "ab".repeat(32), "rounds": 1, "rows_used": m},
        "segment_list": segments});
    *node.model.lock().unwrap() = json!({"row_period": model.period, "matrix_count": model.matrices, "manifest_uri": ""});
    let fx = Fx { node, ticket, model_id, m, a, keys, models, model, base };
    for i in 0..fx.a.len() {
        post_segment(&fx, i, &fx.a[i].clone(), None);
    }
    Some(fx)
}

fn evaluator(fx: &Fx) -> Evaluator {
    let att = Attester { key: sk(), source: JobSource::Node(Box::new(fx.node.clone())), freivalds_rounds: 2, models_dir: fx.models.display().to_string().into() };
    Evaluator::new(att, EvalConfig { submit: false, ..Default::default() }).unwrap()
}

#[test]
fn ticket_full_replay_accepts_honest_and_rejects_altered_activations() {
    let Some(fx) = fixture() else {
        eprintln!("skip: set CADRE_REPLAY_MODEL and CADRE_REPLAY_RUNS (see the file header)");
        return;
    };
    let ev = evaluator(&fx);
    let t = std::time::Instant::now();
    let v = ev.check(&fx.ticket, 10);
    eprintln!("honest ticket ({} rows, {} segments): {:?} in {:.1} s", fx.m, fx.a.len(), v, t.elapsed().as_secs_f64());
    assert_eq!(v, Verdict::Accept, "the honest server's ticket replays bit for bit");

    // a correct GEMM over altered activations: matrix 61 (layers.7.down{2048:4096}), second request's rows
    let target = 61usize;
    let (k, _) = fx.model.shape(target).unwrap();
    let mut bad = fx.a[target].clone();
    let row = fx.m - 6;
    let col = 7;
    bad[row * k + col] = if bad[row * k + col] < 64 { bad[row * k + col] + 1 } else { bad[row * k + col] - 1 };
    post_segment(&fx, target, &bad, None);
    let ev = evaluator(&fx);
    let v = ev.check(&fx.ticket, 10);
    eprintln!("altered A: {v:?}");
    let sid = hex::encode(standing::segment_id(&fx.ticket.job_id, target as u16, 0));
    match &v {
        Verdict::Reject(r) => {
            assert!(r.contains(&sid) && r.contains("matrix 61") && r.contains(&format!("row {row} col {col}")), "names the segment and element: {r}");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
    let (code, proof) = ev.route("GET", &format!("/proofs/{}", hex::encode(fx.ticket.job_id)), b"");
    assert_eq!(code, 200);
    assert_eq!(proof["kind"], "activation");
    assert_eq!(proof["matrix"], json!(target));
    assert_eq!(proof["segment_id"], json!(sid));
    // anyone can verify the proof from the request bundle, the posted blob and the model
    let p: cadre_glue::fraud::FraudProof = serde_json::from_value(proof.clone()).unwrap();
    let model = &fx.model;
    // the proof's request is the one owning the row; verify against the segment's posted A
    assert_eq!(cadre_glue::fraud::verify_activation(model, &p, &bad, fx.m).unwrap(), Ok(()));
    assert!(cadre_glue::fraud::verify_activation(model, &p, &fx.a[target], fx.m).unwrap().is_err(), "the same proof fails on the honest blob");
    post_segment(&fx, target, &fx.a[target].clone(), None);

    // committed C that is not A B (the A is honest)
    let (_, n) = model.shape(3).unwrap();
    post_segment(&fx, 3, &fx.a[3].clone(), Some(vec![1; fx.m * n]));
    let ev = evaluator(&fx);
    let v = ev.check(&fx.ticket, 10);
    eprintln!("wrong C: {v:?}");
    assert!(matches!(&v, Verdict::Reject(r) if r.contains("committed C is not A B") && r.contains("matrix 3")), "{v:?}");
}
