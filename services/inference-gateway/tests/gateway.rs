//! Gateway in process: blobs and specs, rounds that a miner can mine, auth, the prepaid
//! balance and the OpenAI-compatible endpoints with fake storefront and worker.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use cadre_gateway::*;
use pearl_job::job::{Job, MiningConfig};
use pearl_job::pow::{mine_header, verify_block, BlockHeader, BlockWork, FallbackParams};
use pearl_job::{data, standing, PreparedJob};
use pearl_node::mock::MockNode;

fn tmp(tag: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gw-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn model() -> ModelInfo {
    let base = MiningConfig::localnet_default();
    let mut md = ModelInfo { model_id: standing::model_id(&[1; 32], b"m"), name: "test-model".into(), base, matrices: vec![(2048, 16), (2048, 8)], hash_b: vec![], macs_per_row: 2048 * 24 };
    let b = bts(&md);
    md.hash_b = (0..2).map(|i| standing::commit_bt(&md.job_key(i as u16), &b[i])).collect();
    md
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

/// What the worker writes with pad false: A (rows x k) and C = A B per matrix.
fn write_outputs(dir: &Path, model: &ModelInfo, rows: u32, bts: &[Vec<i8>], seed: u64) {
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("c")).unwrap();
    for (i, (k, n)) in model.matrices.iter().enumerate() {
        let (a, _) = data::random_matrices(rows as usize, 1, *k as usize, Some(seed * 100 + i as u64));
        let c = pearl_job::freivalds::matmul_bt(&a, &bts[i], rows as usize, *n as usize, *k as usize);
        std::fs::write(dir.join(format!("a/{i}.i8")), pearl_job::job::as_u8(&a)).unwrap();
        std::fs::write(dir.join(format!("c/{i}.i32")), pearl_job::freivalds::c_to_bytes(&c)).unwrap();
    }
}

#[test]
fn specs_and_blobs() {
    let md = model();
    let b = request_blob(&md.model_id, &[128000, 791], 7);
    assert_eq!(b, request_blob(&md.model_id, &[128000, 791], 7), "deterministic");
    assert_eq!(serde_json::from_slice::<Value>(&b).unwrap()["format"], "cadre-request-v1");
    let bb = balance_blob(&md.model_id, &[9; 32], 4096);
    assert_eq!(serde_json::from_slice::<Value>(&bb).unwrap()["rows"], 4096);
    let s = ticket_spec(&md, &[9; 32], 4095, 1, "blob:00", &data::blob_id(&bb), 10_000, 128);
    assert_eq!(standing::parse_ticket_config(&hex::decode(s["config"].as_str().unwrap()).unwrap()), Some(md.model_id));
    assert_eq!(standing::ticket_rows(4095, 1, 4), 4096, "a balance ticket's m + n is its row budget");
    assert_eq!((s["k"].as_u64(), s["max_blocks"].as_u64()), (Some(1), Some(128)));
}

#[test]
fn a_round_stacks_requests_and_is_mineable() {
    let md = model();
    let b = bts(&md);
    let (d1, d2) = (tmp("r1"), tmp("r2"));
    write_outputs(&d1, &md, 5, &b, 1);
    write_outputs(&d2, &md, 6, &b, 2);
    let segs = prepare_round(&md, &[(d1.clone(), 5), (d2.clone(), 6)], 12, 1).unwrap();
    assert!(prepare_round(&md, &[(d1.clone(), 5), (d2.clone(), 6)], 8, 1).is_err(), "11 rows do not fit m = 8");
    for (i, s) in segs.iter().enumerate() {
        let (k, n) = md.matrices[i];
        let (m2, _, a) = standing::decode_activation_blob(&s.a_blob).unwrap();
        assert_eq!(m2, 12);
        assert!(a[11 * k as usize..].iter().all(|&x| x == 0), "padding rows are zero");
        let header = standing::matrix_job_header(&md.model_id, i as u16, k, n);
        let key = md.job_key(i as u16);
        assert_eq!((s.spec.k, s.spec.n, s.spec.hash_b), (k, n, standing::commit_bt(&key, &b[i])), "the tx repeats the registered matrix");
        let job = Job { header, config: standing::matrix_config(&md.base, k), m: 12, n, hash_a: s.spec.hash_a, hash_b: s.spec.hash_b };
        let prep = PreparedJob::new_bt(job.clone(), &a, b[i].clone()).expect("commitments match");
        assert_eq!(pearl_job::freivalds::result_hash(&prep.job_key, &prep.product()), s.spec.c_hash, "c_hash = keyed hash of the stacked A B");
        let hdr = BlockHeader { version: 1, height: 5, target_bits: 0x207fffff, job_id: [7; 32], miner: [8; 32], ..Default::default() };
        let blk = mine_header(&hdr, &BlockWork::Job(&prep), 0..50).unwrap().expect("easy target");
        verify_block(&blk.header, Some(&job), &blk.proof, &FallbackParams::default()).unwrap();
    }
}

fn signed(sk: &SigningKey, method: &str, path: &str, body: &[u8], ts: u64) -> Vec<(String, String)> {
    let sig = sk.sign(&auth_message(method, path, ts, body));
    vec![
        ("X-Cadre-Wallet".into(), bs58::encode(sk.verifying_key().to_bytes()).into_string()),
        ("X-Cadre-Timestamp".into(), ts.to_string()),
        ("X-Cadre-Signature".into(), hex::encode(sig.to_bytes())),
    ]
}

#[test]
fn auth_signatures_keys_replay_and_skew() {
    let sk = SigningKey::from_bytes(&[5; 32]);
    let w = sk.verifying_key().to_bytes();
    let mut k = Keys::default();
    let now = now_ms();
    let h = signed(&sk, "POST", "/v1/chat/completions", b"{}", now);
    assert_eq!(k.check("POST", "/v1/chat/completions", &h, b"{}", now).unwrap(), Auth::Wallet(w));
    assert!(k.check("POST", "/v1/chat/completions", &h, b"{}", now).unwrap_err().to_string().contains("already used"));
    let h = signed(&sk, "POST", "/v1/chat/completions", b"{}", now);
    assert!(k.check("POST", "/v1/chat/completions", &h, b"{\"x\":1}", now).is_err(), "body bound");
    let h = signed(&sk, "POST", "/v1/completions", b"{}", now + 1);
    assert!(k.check("POST", "/v1/chat/completions", &h, b"{}", now).is_err(), "path bound");
    let old = signed(&sk, "POST", "/p", b"", now - 200_000);
    assert!(k.check("POST", "/p", &old, b"", now).unwrap_err().to_string().contains("120 s"));
    assert_eq!(k.check("GET", "/v1/models", &[], b"", now).unwrap(), Auth::None);
    let (id, key) = k.create(&w, now);
    let bearer = vec![("Authorization".to_string(), format!("Bearer {key}"))];
    assert_eq!(k.check("POST", "/any", &bearer, b"", now).unwrap(), Auth::Key { wallet: w, key_id: id.clone() });
    k.revoke(&w, &id).unwrap();
    assert!(k.check("POST", "/any", &bearer, b"", now).unwrap_err().to_string().contains("revoked"));
}

struct FakeSf(Arc<Mutex<Vec<(Value, Option<String>)>>>);
impl Storefront for FakeSf {
    fn post_job(&self, body: &Value, x: Option<&str>) -> Result<(u16, Value, Option<String>)> {
        let mut c = self.0.lock().unwrap();
        c.push((body.clone(), x.map(String::from)));
        let n = c.len();
        Ok(match x {
            None => (402, json!({"x402Version": 1, "error": "X-PAYMENT header is required", "accepts": [{"maxAmountRequired": "409600"}]}), None),
            Some(_) => (201, json!({"id": n.to_string(), "job_id": hex::encode([n as u8; 32]), "status": "pending",
                "payment": {"payer": bs58::encode(SigningKey::from_bytes(&[5; 32]).verifying_key().to_bytes()).into_string(), "amount": "409600"}}), Some("receipt".into())),
        })
    }
}

struct FakeWorker(ModelInfo, Vec<Vec<i8>>);
impl Worker for FakeWorker {
    fn tokenize(&self, _t: &str) -> Result<Vec<u32>> {
        Ok(vec![1, 2, 3])
    }
    fn chat_ids(&self, m: &Value, _tools: Option<&Value>) -> Result<Vec<u32>> {
        Ok(vec![7; m.as_array().unwrap().len() * 2])
    }
    fn generate(&self, ids: &[u32], max_new: u32, out: &Path, _o: &GenOpts, on_text: &mut dyn FnMut(&str)) -> Result<Value> {
        let rows = ids.len() as u32 + max_new - 1;
        write_outputs(out, &self.0, rows, &self.1, rows as u64);
        for _ in 0..max_new {
            on_text("hi ");
        }
        Ok(json!({"output_ids": vec![5; max_new as usize], "text": "hi ".repeat(max_new as usize), "rows": rows, "rows_padded": rows,
            "prompt_tokens": ids.len(), "completion_tokens": max_new, "finish_reason": "length", "timing": {"decode_tokens_per_s": 2.5}}))
    }
}

struct NoEval;
impl Evaluator for NoEval {
    fn attest(&self, _t: &[u8; 32], _d: &[u8; 32]) -> Result<([u8; 32], [u8; 64])> {
        anyhow::bail!("not in this test")
    }
}

fn gateway(tag: &str) -> (Arc<Gateway>, Arc<Mutex<Vec<(Value, Option<String>)>>>) {
    gateway_with(tag, None)
}

fn gateway_with(tag: &str, public_api_root: Option<&str>) -> (Arc<Gateway>, Arc<Mutex<Vec<(Value, Option<String>)>>>) {
    let md = model();
    let calls = Arc::new(Mutex::new(vec![]));
    let cfg = Config { work_dir: tmp(tag), ticket_wait: Duration::from_millis(200), poll: Duration::from_millis(50), balance_rows: 64, max_new_tokens: 32,
        public_api_root: public_api_root.map(String::from), ..Config::default() };
    let gw = Arc::new(Gateway::new(Box::new(MockNode::new(1, 0x207fffff)), Box::new(FakeSf(calls.clone())), Box::new(FakeWorker(md.clone(), bts(&md))),
        Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})));
    (gw, calls)
}

#[test]
fn prepaid_balance_then_signed_and_keyed_chat_completions() {
    let (gw, calls) = gateway("bal");
    let buyer = SigningKey::from_bytes(&[5; 32]);
    // models: priced from the storefront's quote of a full balance (409600 / 64 rows)
    let m = gw.handle("GET", "/v1/models", &[], b"");
    assert_eq!(m.json()["data"][0]["id"], "test-model");
    assert_eq!(m.json()["data"][0]["cadre"]["pricing"]["lamports_per_row"], "6400");
    assert_eq!(m.json()["data"][0]["cadre"]["pricing"]["default"], true);
    // top up: 402, then paid
    let r = gw.handle("POST", "/v1/balance/topup", &[], b"{}");
    assert_eq!(r.status, 402);
    let r = gw.handle("POST", "/v1/balance/topup", &[("X-PAYMENT".into(), "pay".into())], b"{}");
    assert_eq!(r.status, 201, "{}", r.json());
    let ticket = r.json()["ticket_id"].as_str().unwrap().to_string();
    let spec = &calls.lock().unwrap().last().unwrap().0["spec"].clone();
    assert_eq!((spec["m"].as_u64(), spec["n"].as_u64()), (Some(63), Some(1)));
    // a wallet-signed chat completion draws on it
    let body = br#"{"model": "test-model", "messages": [{"role": "user", "content": "hello"}], "max_tokens": 4}"#;
    let h = signed(&buyer, "POST", "/v1/chat/completions", body, now_ms());
    let r = gw.handle("POST", "/v1/chat/completions", &h, body);
    assert_eq!(r.status, 200, "{}", r.json());
    let j = r.json();
    assert_eq!(j["object"], "chat.completion");
    assert_eq!(j["choices"][0]["message"]["content"], "hi hi hi hi ");
    assert_eq!(j["usage"], json!({"prompt_tokens": 2, "completion_tokens": 4, "total_tokens": 6}));
    assert_eq!(j["cadre"]["ticket_id"], json!(ticket));
    assert_eq!(j["cadre"]["rows"], 5);
    assert_eq!(j["cadre"]["charged_lamports"], "32000", "5 rows x 6400");
    let hdr = |k: &str| r.headers.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(hdr("x-cadre-ticket"), ticket);
    assert_eq!(hdr("x-cadre-rows"), "5");
    assert!(hdr("x-cadre-explorer").ends_with(&format!("/job/{ticket}")));
    let rid = hdr("x-cadre-request");
    assert_eq!(gw.handle("GET", &format!("/v1/requests/{rid}"), &[], b"").json()["receipt"]["rows"], 5);
    // balance: 64 rows, 5 pending (padded to 8 in the next round)
    let b = gw.handle("GET", &format!("/v1/balance?wallet={}", bs58::encode(buyer.verifying_key().to_bytes()).into_string()), &[], b"");
    assert_eq!(b.json()["rows_free"], 56);
    assert_eq!(b.json()["tickets"][0]["rows_pending"], 5);
    // an API key made by a signed call works as a Bearer token
    let h = signed(&buyer, "POST", "/v1/keys", b"", now_ms());
    let k = gw.handle("POST", "/v1/keys", &h, b"");
    assert_eq!(k.status, 201);
    let key = k.json()["api_key"].as_str().unwrap().to_string();
    let bearer = vec![("Authorization".to_string(), format!("Bearer {key}"))];
    let body2 = br#"{"prompt": "x", "max_tokens": 2}"#;
    let r = gw.handle("POST", "/v1/completions", &bearer, body2);
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["object"], "text_completion");
    assert_eq!(r.json()["cadre"]["rows"], 4);
    // GET /v1/keys (wallet-signed): the key record with its last use, never the key
    let kid = k.json()["key_id"].as_str().unwrap().to_string();
    let h = signed(&buyer, "GET", "/v1/keys", b"", now_ms());
    let kl = gw.handle("GET", "/v1/keys", &h, b"");
    assert_eq!(kl.status, 200, "{}", kl.json());
    let ks = kl.json()["keys"].as_array().unwrap().clone();
    assert_eq!(ks.len(), 1);
    assert_eq!(ks[0]["key_id"], kid.as_str());
    assert!(ks[0]["created_ms"].as_u64().unwrap() > 0);
    assert!(ks[0]["last_used_ms"].as_u64().unwrap() >= ks[0]["created_ms"].as_u64().unwrap(), "used by the completion above");
    assert!(!kl.json().to_string().contains(&key), "the secret is never listed");
    assert!(ks[0].get("api_key").is_none());
    assert_eq!(gw.handle("GET", "/v1/keys", &bearer, b"").status, 400, "listing needs the wallet signature, not a key");
    assert_eq!(gw.handle("GET", "/v1/keys", &[], b"").status, 400);
    // too big for what is left: 402 insufficient_balance
    let big = br#"{"prompt": "x", "max_tokens": 32}"#;
    let mut n = 0;
    loop {
        let r = gw.handle("POST", "/v1/completions", &bearer, big);
        if r.status == 402 {
            assert_eq!(r.json()["error"]["type"], "insufficient_balance");
            break;
        }
        n += 1;
        assert!(n < 5);
    }
    // receipts list
    let rl = gw.handle("GET", &format!("/v1/receipts?wallet={}", bs58::encode(buyer.verifying_key().to_bytes()).into_string()), &[], b"");
    assert!(rl.json()["receipts"].as_array().unwrap().len() >= 2);
    // bad credentials are 401
    let mut bad = signed(&buyer, "POST", "/v1/completions", body2, now_ms());
    bad[2].1 = "00".repeat(64);
    assert_eq!(gw.handle("POST", "/v1/completions", &bad, body2).status, 401);
}

#[test]
fn per_request_x402_and_streaming() {
    let (gw, _calls) = gateway("x402");
    let body = br#"{"prompt": "hi", "max_tokens": 3}"#;
    let r = gw.handle("POST", "/v1/completions", &[], body);
    assert_eq!(r.status, 402);
    assert_eq!(r.json()["cadre"]["rows"], 8);
    let r = gw.handle("POST", "/v1/completions", &[("x-payment".into(), "p".into())], body);
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["cadre"]["rows"], 5);
    // streaming chat: SSE chunks, the last with usage and the receipt, then [DONE]
    let body = br#"{"messages": [{"role": "user", "content": "hi"}], "max_tokens": 3, "stream": true}"#;
    let r = gw.handle("POST", "/v1/chat/completions", &[("x-payment".into(), "p".into())], body);
    assert_eq!(r.status, 200);
    assert!(r.headers.iter().any(|(k, v)| k == "content-type" && v == "text/event-stream"));
    let Payload::Stream(rx) = r.body else { panic!("not a stream") };
    let events: Vec<String> = rx.iter().map(|b| String::from_utf8(b).unwrap()).collect();
    assert_eq!(events.last().unwrap(), "data: [DONE]\n\n");
    let parsed: Vec<Value> = events[..events.len() - 1].iter().map(|e| serde_json::from_str(e.trim().strip_prefix("data: ").unwrap()).unwrap()).collect();
    assert_eq!(parsed[0]["choices"][0]["delta"]["role"], "assistant");
    let text: String = parsed.iter().filter_map(|p| p["choices"][0]["delta"]["content"].as_str()).collect();
    assert_eq!(text, "hi hi hi ");
    let last = parsed.last().unwrap();
    assert_eq!(last["object"], "chat.completion.chunk");
    assert_eq!(last["usage"]["completion_tokens"], 3);
    assert!(last["cadre"]["ticket_id"].is_string());
    let st = gw.handle("GET", "/v1/stats", &[], b"");
    assert_eq!(st.json()["requests_total"], 2);
}

#[test]
fn tool_calls_from_a_bare_json_answer() {
    let tc = parse_tool_call(r#" {"name": "get_weather", "parameters": {"city": "Paris"}} "#).unwrap();
    assert_eq!(tc["function"]["name"], "get_weather");
    assert_eq!(tc["function"]["arguments"], json!(r#"{"city":"Paris"}"#));
    assert!(parse_tool_call("Paris is sunny").is_none());
    let gen = json!({"text": r#"{"name": "f", "arguments": "{}"}"#, "finish_reason": "stop", "prompt_tokens": 1, "completion_tokens": 2});
    let c = completion_json(true, true, "r", 0, "m", &gen, &json!({}));
    assert_eq!(c["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(c["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "f");
    let c = completion_json(true, false, "r", 0, "m", &gen, &json!({}));
    assert_eq!(c["choices"][0]["finish_reason"], "stop", "no tools offered: plain content");
}

/// Receipt links behind cadre-api: the gateway's /v1 is public at <api>/v1/openai, which
/// --public-url (host only, "/v1" appended) cannot express. Devnet receipts pointed at
/// 127.0.0.1:10664 before --public-api-root (2026-10-20).
#[test]
fn receipt_links_use_the_public_api_root() {
    let root = "https://api.example.test/v1/openai/";
    let (gw, _calls) = gateway_with("pubroot", Some(root));
    let body = br#"{"prompt": "hi", "max_tokens": 3}"#;
    let r = gw.handle("POST", "/v1/completions", &[("x-payment".into(), "p".into())], body);
    assert_eq!(r.status, 200, "{}", r.json());
    let rid = r.json()["cadre"]["request_id"].as_str().unwrap().to_string();
    let want = format!("https://api.example.test/v1/openai/requests/{rid}");
    assert_eq!(r.json()["cadre"]["status_url"], json!(want));
    let hdr = r.headers.iter().find(|(k, _)| k == "x-cadre-receipt").map(|(_, v)| v.clone()).unwrap();
    assert_eq!(hdr, want);
    // the stored receipt says the same, and streaming carries it in its header
    assert_eq!(gw.handle("GET", &format!("/v1/requests/{rid}"), &[], b"").json()["receipt"]["status_url"], json!(want));
    let body = br#"{"messages": [{"role": "user", "content": "hi"}], "max_tokens": 2, "stream": true}"#;
    let r = gw.handle("POST", "/v1/chat/completions", &[("x-payment".into(), "p".into())], body);
    let hdr = r.headers.iter().find(|(k, _)| k == "x-cadre-receipt").map(|(_, v)| v.clone()).unwrap();
    assert!(hdr.starts_with("https://api.example.test/v1/openai/requests/"), "{hdr}");
    // default (no root): <public_url>/v1/requests/<id>
    let (gw, _calls) = gateway("pubdefault");
    let r = gw.handle("POST", "/v1/completions", &[("x-payment".into(), "p".into())], br#"{"prompt": "hi", "max_tokens": 3}"#);
    let rid = r.json()["cadre"]["request_id"].as_str().unwrap().to_string();
    assert_eq!(r.json()["cadre"]["status_url"], json!(format!("http://127.0.0.1:10664/v1/requests/{rid}")));
}

/// MockNode plus `getParams` (the chain's max_balance_rows) for the large-ticket tests.
struct ParamsNode(MockNode, u64);
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
            return Ok(json!({"max_balance_rows": self.1}));
        }
        self.0.call(method, p)
    }
}

fn gateway_params(tag: &str, chain_max: u64, cfg_max: u64) -> (Arc<Gateway>, Arc<Mutex<Vec<(Value, Option<String>)>>>) {
    let md = model();
    let calls = Arc::new(Mutex::new(vec![]));
    let cfg = Config { work_dir: tmp(tag), ticket_wait: Duration::from_millis(200), poll: Duration::from_millis(50), balance_rows: 64, max_new_tokens: 32,
        max_balance_rows: cfg_max, ..Config::default() };
    let gw = Arc::new(Gateway::new(Box::new(ParamsNode(MockNode::new(1, 0x207fffff), chain_max)), Box::new(FakeSf(calls.clone())),
        Box::new(FakeWorker(md.clone(), bts(&md))), Box::new(NoEval), SigningKey::from_bytes(&[3; 32]), md, cfg, Box::new(|_| {})));
    (gw, calls)
}

/// ECOSYSTEM M1.5: a balance above 4096 rows needs both the gateway's and the chain's
/// max_balance_rows; it gets the slow round period unless the top-up names its own; one
/// round never stacks more rows than one A blob holds.
#[test]
fn large_balances_round_period_and_round_cap() {
    let pay = vec![("X-PAYMENT".to_string(), "pay".to_string())];
    // chain without the key (MockNode answers no getParams): 4096 is the most
    let (gw, _c) = gateway("big-old");
    let r = gw.handle("POST", "/v1/balance/topup", &pay, br#"{"rows": 8192}"#);
    assert_eq!(r.status, 400, "{}", r.json());
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("4096"));
    // chain allows 65536, gateway 32768: the smaller wins
    let (gw, calls) = gateway_params("big", 65_536, 32_768);
    assert_eq!(gw.max_balance_rows(), 32_768);
    let r = gw.handle("POST", "/v1/balance/topup", &pay, br#"{"rows": 65536}"#);
    assert_eq!(r.status, 400);
    let r = gw.handle("POST", "/v1/balance/topup", &pay, br#"{"rows": 32768}"#);
    assert_eq!(r.status, 201, "{}", r.json());
    assert_eq!(r.json()["round_secs"], 10_800, "a large balance rounds every 3 h by default");
    let spec = &calls.lock().unwrap().last().unwrap().0["spec"].clone();
    assert_eq!((spec["m"].as_u64(), spec["n"].as_u64()), (Some(32_767), Some(1)), "balance form: m = R - 1, n = 1");
    // a top-up's own period wins; bounds checked
    let r = gw.handle("POST", "/v1/balance/topup", &pay, br#"{"rows": 8192, "round_secs": 3600}"#);
    assert_eq!(r.status, 201, "{}", r.json());
    assert_eq!(r.json()["round_secs"], 3600);
    assert_eq!(gw.handle("POST", "/v1/balance/topup", &pay, br#"{"rows": 8192, "round_secs": 5}"#).status, 400);
    // small balances keep the gateway's round period
    assert_eq!(gw.round_period(4096, None), Duration::from_secs(60));
    assert_eq!(gw.round_period(4100, None), Duration::from_secs(10_800));
    assert_eq!(gw.round_period(4100, Some(600)), Duration::from_secs(600));
    // round cap: (4 MiB - 12) / k 2048 = 2047 rows, down to the row period
    let p = model().period();
    assert_eq!(gw.round_rows_cap(), 2047 / p * p);
    let served = |id: &str, rows: u32| Served {
        request_id: id.into(), wallet: String::new(), ticket_id: String::new(), dir: PathBuf::new(), rows, prompt_tokens: 0, completion_tokens: 0,
        body_sha256: String::new(), output_sha256: String::new(), text_sha256: String::new(), charged_lamports: None, at_ms: 0, round: None,
        row_offset: None, ttft_s: 0.0, decode_tokens_per_s: None,
    };
    let ids = |v: Vec<Served>| v.iter().map(|r| r.request_id.clone()).collect::<Vec<_>>();
    let take = round_take(vec![served("a", 1000), served("b", 1100), served("c", 10)], 2044, p);
    assert_eq!(ids(take), vec!["a"], "a + b = 2100 rows is over the cap; c waits behind b (oldest first)");
    let take = round_take(vec![served("a", 1000), served("b", 1000), served("c", 44)], 2044, p);
    assert_eq!(ids(take), vec!["a", "b", "c"], "2044 rows fit exactly");
    assert!(round_take(vec![served("x", 3000)], 2044, p).is_empty(), "one request larger than a round never fits (post_round refuses it)");
}

/// SPEC 4.6 thought_ref: every receipt carries sha256 of the completion text exactly as the
/// response returns it, so a post quoting the completion verbatim can be matched to it.
#[test]
fn receipts_carry_the_completion_text_hash() {
    use sha2::Digest;
    let (gw, _c) = gateway("textsha");
    let body = br#"{"messages": [{"role": "user", "content": "hi"}], "max_tokens": 3}"#;
    let r = gw.handle("POST", "/v1/chat/completions", &[("x-payment".into(), "p".into())], body);
    assert_eq!(r.status, 200, "{}", r.json());
    let content = r.json()["choices"][0]["message"]["content"].as_str().unwrap().to_string();
    let want = hex::encode(sha2::Sha256::digest(content.as_bytes()));
    let rid = r.json()["cadre"]["request_id"].as_str().unwrap().to_string();
    let rec = gw.handle("GET", &format!("/v1/requests/{rid}"), &[], b"").json()["receipt"].clone();
    assert_eq!(rec["text_sha256"], json!(want));
    assert_ne!(rec["text_sha256"], rec["output_sha256"], "output_sha256 hashes the token ids, not the text");
}
