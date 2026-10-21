//! Contracts over a multi-node network (docs/CONTRACTS.md): three real node actors on the
//! deterministic runtime deploy, instantiate and call contracts sent to different nodes and
//! mined by different nodes; every node agrees (tips, contract records, query results,
//! receipts, contracts record in getBlock); a reorg undoes a call on the losing branch; a
//! final contract storage item is proved against the final header's state_root.


#![allow(dead_code)]

use std::time::{Duration, UNIX_EPOCH};

use agent_node::node::{run, ActorConfig};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use base64::Engine as _;
use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::simulated::{Config as NetConfig, Link, Network, Oracle};
use commonware_p2p::Manager as _;
use commonware_runtime::{deterministic, Clock, Runner as _, Spawner as _, Supervisor as _};
use commonware_utils::ordered::Set;
use consensus_pow::{Chain, Genesis, MockVerifier};
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use rpc::{ApiRequest, Core};
use serde_json::{json, Value};
use state::config::Config;

const CHAIN: u32 = 11;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

fn genesis(k: u32) -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.finality_k = k;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    c.min_fee = 10;
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: 0x207fffff, config: c }
}

#[derive(Clone)]
struct Api(mpsc::Sender<ApiRequest>);

impl Api {
    async fn call(&self, method: &str, params: Value) -> Value {
        let (tx, rx) = oneshot::channel();
        self.0.clone().send(ApiRequest::Call { method: method.into(), params, reply: tx }).await.unwrap();
        rx.await.unwrap().unwrap_or_else(|e| panic!("{method}: {}", e.message))
    }

    async fn tip(&self) -> (String, u64) {
        let v = self.call("getChainInfo", json!({})).await;
        (v["tip"].as_str().unwrap().to_string(), v["height"].as_u64().unwrap())
    }

    /// Mine one fallback block on this node's tip.
    async fn mine(&self, miner: u8) -> Value {
        let t = self.call("getBlockTemplate", json!({"miner": bs58::encode(pk(miner)).into_string(), "job_id": "fallback"})).await;
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self
            .call(
                "submitBlock",
                json!({"header": hex::encode(h.encode()), "proof": base64::engine::general_purpose::STANDARD.encode(&proof),
                       "template_id": t["template_id"]}),
            )
            .await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        r
    }

    async fn try_call(&self, method: &str, params: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        self.0.clone().send(ApiRequest::Call { method: method.into(), params, reply: tx }).await.unwrap();
        rx.await.unwrap().map_err(|e| e.message)
    }

    /// Mine one block with a template choice ("fallback", "auto" = omit, or a job id).
    async fn mine_with(&self, miner: u8, job: Option<&str>) -> (Value, Value) {
        let mut q = json!({"miner": bs58::encode(pk(miner)).into_string()});
        if let Some(j) = job {
            q["job_id"] = json!(j);
        }
        let t = self.call("getBlockTemplate", q).await;
        let mut h = Header::decode(&hex::decode(t["header"].as_str().unwrap()).unwrap()).unwrap();
        let proof = MockVerifier::proof_for(&h);
        h.proof_hash = block::proof_hash(&proof);
        let r = self
            .call(
                "submitBlock",
                json!({"header": hex::encode(h.encode()), "proof": base64::engine::general_purpose::STANDARD.encode(&proof),
                       "template_id": t["template_id"]}),
            )
            .await;
        assert_eq!(r["accepted"], json!(true), "{r}");
        (t, r)
    }

    async fn send(&self, tx: &Tx) -> Result<Value, String> {
        self.try_call("sendTx", json!({"tx": base64::engine::general_purpose::STANDARD.encode(tx.encode())})).await
    }

    async fn balance(&self, who: &Pubkey) -> u64 {
        let v = self.call("getAccount", json!({"account": bs58::encode(who).into_string()})).await;
        v["balance"].as_str().unwrap().parse().unwrap()
    }
}

/// Blob grace period in the test nodes (blocks).
const BLOB_GRACE: u64 = 20;

const LINK: Link = Link { latency: Duration::from_millis(20), jitter: Duration::from_millis(5), success_rate: commonware_utils::probability!(1.0) };

struct Net {
    oracle: Oracle<ed25519::PublicKey, deterministic::Context>,
    keys: Vec<ed25519::PublicKey>,
    apis: Vec<Api>,
}

impl Net {
    async fn link(&self, a: usize, b: usize) {
        let _ = self.oracle.add_link(self.keys[a].clone(), self.keys[b].clone(), LINK).await;
        let _ = self.oracle.add_link(self.keys[b].clone(), self.keys[a].clone(), LINK).await;
    }
    async fn unlink(&self, a: usize, b: usize) {
        let _ = self.oracle.remove_link(self.keys[a].clone(), self.keys[b].clone()).await;
        let _ = self.oracle.remove_link(self.keys[b].clone(), self.keys[a].clone()).await;
    }
}

async fn setup(ctx: &deterministic::Context, n: usize, k: u32, linked: bool) -> Net {
    let (network, oracle) = Network::new(
        ctx.child("network"),
        NetConfig { max_size: 8 << 20, max_peers_per_set: std::num::NonZeroUsize::new(n).unwrap(), disconnect_on_block: false, tracked_peer_sets: std::num::NonZeroUsize::new(3).unwrap() },
    );
    network.start();
    let signers: Vec<ed25519::PrivateKey> = (0..n).map(|i| ed25519::PrivateKey::from_seed(100 + i as u64)).collect();
    let keys: Vec<ed25519::PublicKey> = signers.iter().map(|s| s.public_key()).collect();
    let mut manager = oracle.manager();
    let _ = manager.track(0, Set::try_from(keys.clone()).unwrap());
    let mut apis = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let ctl = oracle.control(key.clone());
        let q = net::quota();
        let ch = net::Channels {
            blocks: ctl.clone().register(net::CH_BLOCKS, q).await.unwrap(),
            txs: ctl.clone().register(net::CH_TXS, q).await.unwrap(),
            tips: ctl.clone().register(net::CH_TIPS, q).await.unwrap(),
            headers: ctl.clone().register(net::CH_HEADERS, q).await.unwrap(),
            bodies: ctl.clone().register(net::CH_BODIES, q).await.unwrap(),
            blobs: ctl.clone().register(net::CH_BLOBS, q).await.unwrap(),
            blob_fetch: ctl.clone().register(net::CH_BLOB_FETCH, q).await.unwrap(),
        };
        let prefix = format!("node{i}");
        let chain = Chain::open(ctx.child("chain").with_attribute("node", i), &prefix, genesis(k), MockVerifier).await.unwrap();
        let bcfg = blobs::BlobConfig { grace_blocks: BLOB_GRACE, ..Default::default() };
        let store = blobs::BlobStore::open(ctx.child("blobs").with_attribute("node", i), &format!("{prefix}-blobs"), bcfg, 0).await.unwrap();
        let core = Core::new(chain).with_blobs(store);
        let (tx, rx) = mpsc::channel(256);
        let me = key.clone();
        let provider = oracle.manager();
        let blocker = ctl.clone();
        ctx.child("node").with_attribute("node", i).spawn(move |c| async move {
            run(c, core, me, provider, blocker, ch, rx, ActorConfig { tip_interval: Duration::from_millis(500), ..Default::default() }).await;
        });
        apis.push(Api(tx));
    }
    let net = Net { oracle, keys, apis };
    if linked {
        for a in 0..n {
            for b in a + 1..n {
                net.link(a, b).await;
            }
        }
    }
    net
}

fn runner(seed: u64) -> deterministic::Runner {
    deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(seed)
            .with_start_time(UNIX_EPOCH + Duration::from_millis(T0 + 1000))
            .with_timeout(Some(Duration::from_secs(3600))),
    )
}

async fn wait_tips_equal(ctx: &deterministic::Context, apis: &[Api], secs: u64) -> (String, u64) {
    for _ in 0..secs * 10 {
        let mut tips = Vec::new();
        for a in apis {
            tips.push(a.tip().await);
        }
        if tips.iter().all(|t| *t == tips[0]) {
            return tips[0].clone();
        }
        ctx.sleep(Duration::from_millis(100)).await;
    }
    let mut tips = Vec::new();
    for a in apis {
        tips.push(a.tip().await);
    }
    panic!("tips did not converge: {tips:?}");
}



fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}
fn b58(k: &Pubkey) -> String {
    bs58::encode(k).into_string()
}
fn unlock(nonce: u64, sol_nonce: u64, who: u8, amount_spl: u64) -> Tx {
    let msg = deposit_message(sol_nonce, &pk(who), amount_spl);
    let sig = AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) };
    Tx::signed(&seed(200), CHAIN, nonce, 0, TxBody::BridgeUnlock { solana_nonce: sol_nonce, recipient: pk(who), amount_spl, job_order: None, sigs: vec![sig] })
}

use agent_types::contracts as ac;

const COUNTER: &[u8] = include_bytes!("../../../contracts/testdata/counter.wasm");
const TOKEN: &[u8] = include_bytes!("../../../contracts/testdata/token.wasm");

fn ctx_fee(tx_body: &TxBody, who: u8, nonce: u64) -> Tx {
    let cfg = genesis(4).config;
    let probe = Tx::signed(&seed(who), CHAIN, nonce, 0, tx_body.clone());
    let fee = executor::contract_fee_floor(&cfg, probe.encode().len(), executor::gas_limit_of(tx_body));
    Tx::signed(&seed(who), CHAIN, nonce, fee, tx_body.clone())
}

fn method(name: &str, fields: &[u8]) -> Vec<u8> {
    let mut v = vec![name.len() as u8];
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(fields);
    v
}

async fn same_everywhere(apis: &[Api], method_name: &str, params: Value) -> Value {
    let first = apis[0].call(method_name, params.clone()).await;
    for a in &apis[1..] {
        assert_eq!(a.call(method_name, params.clone()).await, first, "{method_name} differs between nodes");
    }
    first
}

#[test]
fn contracts_agree_across_nodes_and_reorg_cleanly() {
    runner(41).start(|ctx| async move {
        let net = setup(&ctx, 3, 4, true).await;
        let a = net.apis.clone();
        // fund two senders
        a[0].send(&unlock(0, 1, 2, 1_000_000_000)).await.unwrap();
        a[0].send(&unlock(1, 2, 3, 1_000_000_000)).await.unwrap();
        a[0].mine(9).await;
        wait_tips_equal(&ctx, &a, 30).await;
        // deploy through node 1, mined by node 1
        let deploy = |code: &[u8], nonce| ctx_fee(&TxBody::DeployCode(ac::DeployCode { gas_limit: ::contracts::deploy_cost(code.len()), code: code.to_vec() }), 2, nonce);
        a[1].send(&deploy(COUNTER, 0)).await.unwrap();
        a[1].send(&deploy(TOKEN, 1)).await.unwrap();
        a[1].mine(9).await;
        wait_tips_equal(&ctx, &a, 30).await;
        let ch = ac::code_hash(COUNTER);
        let code = same_everywhere(&a, "getCode", json!({"code_hash": hex::encode(ch)})).await;
        assert_eq!(code["size"], json!(COUNTER.len()));
        // instantiate through node 2, mined by node 0 (the tx gossips)
        let inst = ctx_fee(&TxBody::Instantiate(ac::Instantiate { code_hash: ch, admin: [0; 32], value: 0, gas_limit: 20_000_000, label: b"counter".to_vec(), args: 5u64.to_le_bytes().to_vec() }), 3, 0);
        a[2].send(&inst).await.unwrap();
        let addr = ac::contract_address(&pk(3), 0);
        for _ in 0..50 {
            if a[0].call("getMempool", json!({})).await["size"].as_u64().unwrap() > 0 {
                break;
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        a[0].mine(9).await;
        wait_tips_equal(&ctx, &a, 30).await;
        let c = same_everywhere(&a, "getContract", json!({"address": b58(&addr)})).await;
        assert_eq!(c["label"], json!("counter"));
        let r = same_everywhere(&a, "getContractReceipt", json!({"tx_hash": hex::encode(inst.hash())})).await;
        assert_eq!(r["status"], json!("ok"), "{r}");
        // three calls from different nodes in one block
        let call = |who: u8, nonce: u64, by: u64| ctx_fee(&TxBody::Call(ac::Call { contract: addr, value: 0, gas_limit: 20_000_000, args: method("increment", &by.to_le_bytes()) }), who, nonce);
        let c1 = call(2, 2, 1);
        let c2 = call(3, 1, 10);
        a[0].send(&c1).await.unwrap();
        a[1].send(&c2).await.unwrap();
        for _ in 0..50 {
            if a[2].call("getMempool", json!({})).await["size"].as_u64().unwrap() >= 2 {
                break;
            }
            ctx.sleep(Duration::from_millis(100)).await;
        }
        let (_, sub) = a[2].mine_with(9, Some("fallback")).await;
        wait_tips_equal(&ctx, &a, 30).await;
        let q = method("get", &[]);
        let v = same_everywhere(&a, "queryContract", json!({"address": b58(&addr), "args": hex::encode(&q)})).await;
        assert_eq!(v["return"], json!(hex::encode(16u64.to_le_bytes())));
        let blk = same_everywhere(&a, "getBlock", json!({"hash": sub["block_hash"]})).await;
        assert_eq!(blk["contracts"]["receipt_count"], json!(2), "{}", blk["contracts"]);
        assert_eq!(blk["contracts"]["event_count"], json!(2));
        let ev = same_everywhere(&a, "getContractEvents", json!({"address": b58(&addr)})).await;
        assert_eq!(ev["total"], json!(2));

        // reorg: node 2 alone mines a call on a short branch, the others mine a longer one
        net.unlink(0, 2).await;
        net.unlink(1, 2).await;
        let c3 = call(2, 3, 1000);
        a[2].send(&c3).await.unwrap();
        a[2].mine(8).await;
        assert_eq!(a[2].call("queryContract", json!({"address": b58(&addr), "args": hex::encode(&q)})).await["return"], json!(hex::encode(1016u64.to_le_bytes())));
        a[0].mine(9).await;
        a[0].mine(9).await;
        wait_tips_equal(&ctx, &a[..2], 30).await;
        net.link(0, 2).await;
        net.link(1, 2).await;
        a[0].mine(9).await;
        wait_tips_equal(&ctx, &a, 60).await;
        // node 2 dropped its branch: the call is undone there (it may land again from the mempool)
        let v = a[2].call("queryContract", json!({"address": b58(&addr), "args": hex::encode(&q)})).await;
        let got = u64::from_le_bytes(hex::decode(v["return"].as_str().unwrap()).unwrap().try_into().unwrap());
        assert!(got == 16 || got == 1016, "{got}");
        same_everywhere(&a, "queryContract", json!({"address": b58(&addr), "args": hex::encode(&q)})).await;

        // finality, then a storage proof against the final header
        for _ in 0..6 {
            a[1].mine(9).await;
        }
        wait_tips_equal(&ctx, &a, 30).await;
        let pr = a[0].call("getContractStateProof", json!({"address": b58(&addr), "key": hex::encode(b"n")})).await;
        assert_eq!(pr["state_root"], pr["header_state_root"]);
        let key: [u8; 32] = hex::decode(pr["state_key"].as_str().unwrap()).unwrap().try_into().unwrap();
        let value = hex::decode(pr["value"].as_str().unwrap()).unwrap();
        let proof = hex::decode(pr["proof"].as_str().unwrap()).unwrap();
        let root: [u8; 32] = hex::decode(pr["state_root"].as_str().unwrap()).unwrap().try_into().unwrap();
        assert!(state::Store::<deterministic::Context>::verify_state_proof(&key, &value, &proof, &root));
        let (k2, v2) = state::contracts::decode_store(&value).unwrap();
        assert_eq!(k2, b"n");
        assert!(u64::from_le_bytes(v2.try_into().unwrap()) >= 16);
    });
}
