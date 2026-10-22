//! Job index (rpc/src/jobindex.rs), listJobs paging, template choice from the index, and the
//! mempool eviction rule (core.rs EVICT_AFTER_TIPS), on `rpc::Core` in the deterministic runtime.

use std::collections::BTreeMap;

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Chain, Genesis, JobChoice, MockVerifier};
use rpc::Core;
use serde_json::{json, Value};
use state::config::Config;
use state::schema;

const CHAIN: u32 = 11;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

fn genesis() -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.finality_k = 3;
    c.target_block_time_ms = 1000;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    c.min_fee = 10;
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: 0x207fffff, config: c }
}

type K = Core<deterministic::Context, MockVerifier>;

fn unlock(rnonce: u64, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(nonce, &to, amount_spl);
    let sigs = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
    Tx::signed(&seed(7), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
}

fn job_body(deadline: u64, max_blocks: u32) -> PostJob {
    PostJob {
        m: 64,
        n: 64,
        k: pearl_job::MiningConfig::localnet_default().common_dim,
        config: pearl_job::MiningConfig::localnet_default().to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: b"mem://test".to_vec(),
        worker_pay: 1000,
        protocol_fee: 5_000_000,
        deadline_height: deadline,
        max_blocks,
        evaluators: vec![pk(60)],
        threshold: 1,
    }
}

fn seal(t: &consensus_pow::Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: Vec::new() }
}

/// Mine one block on the tip carrying the pool (fallback or the auto job).
async fn mine(k: &mut K, choice: JobChoice, now: u64) -> Block {
    let (_, t) = k.template(pk(9), choice, now).await.unwrap();
    let b = seal(&t);
    let r = k.add_block(b.clone(), now, false).await;
    assert!(r.accepted && r.tip, "{:?}", r.reason);
    b
}

/// A block on any live parent with the given txs (competing branches).
async fn mine_at(k: &mut K, parent: Hash, txs: &[Tx], miner: u8, now: u64) -> Block {
    let t = k.chain.build_template_at(parent, pk(miner), JobChoice::Fallback, txs, now).await.unwrap();
    assert_eq!(t.txs.len(), txs.len());
    let b = seal(&t);
    let r = k.add_block(b.clone(), now, false).await;
    assert!(r.accepted, "{:?}", r.reason);
    b
}

/// Every job at the tip by a full state scan: index -> (id, status, mineable).
async fn scan(k: &K) -> BTreeMap<u64, (String, String, bool)> {
    let c = schema::Counters::decode(&k.chain.read_tip(&schema::counters_key()).await.unwrap().unwrap()).unwrap();
    let next = k.chain.tip_height() + 1;
    let mut out = BTreeMap::new();
    for i in 0..c.job_count {
        let id = schema::decode_job_index(&k.chain.read_tip(&schema::job_index_key(i)).await.unwrap().unwrap()).unwrap();
        let j = schema::JobRecord::decode(&k.chain.read_tip(&schema::job_key(&id)).await.unwrap().unwrap()).unwrap();
        out.insert(i, (hex::encode(id), j.status.name().to_string(), executor::check_mineable(&j, next).is_ok()));
    }
    out
}

/// Every page of listJobs with `params`: the first page in offset mode, then `after` cursors.
async fn list_all(k: &mut K, params: Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut after: Option<u64> = None;
    loop {
        let mut p = params.clone();
        p["limit"] = json!(3);
        if let Some(a) = after {
            p["after"] = json!(a);
        }
        let r = k.dispatch("listJobs", &p, T0).await.unwrap();
        out.extend(r["jobs"].as_array().unwrap().iter().cloned());
        match r["next_cursor"].as_u64() {
            Some(c) => after = Some(c),
            None => break,
        }
    }
    out
}

async fn check(k: &mut K, what: &str) {
    let truth = scan(k).await;
    // unfiltered, every job once, in order
    let all = list_all(k, json!({})).await;
    let got: Vec<(u64, String, String)> = all.iter().map(|j| (j["index"].as_u64().unwrap(), j["job_id"].as_str().unwrap().into(), j["status"].as_str().unwrap().into())).collect();
    let want: Vec<(u64, String, String)> = truth.iter().map(|(i, (id, st, _))| (*i, id.clone(), st.clone())).collect();
    assert_eq!(got, want, "{what}: all jobs");
    for st in ["open", "expired", "completed", "mineable", "live"] {
        let got: Vec<u64> = list_all(k, json!({"status": st})).await.iter().map(|j| j["index"].as_u64().unwrap()).collect();
        let want: Vec<u64> = truth
            .iter()
            .filter(|(_, (_, s, m))| match st {
                "mineable" => *m,
                "live" => s == "open" || s == "delivered",
                _ => s == st,
            })
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(got, want, "{what}: status {st}");
        let r = k.dispatch("listJobs", &json!({"status": st, "after": 0, "limit": 1}), T0).await.unwrap();
        assert_eq!(r["total"].as_u64().unwrap() as usize, want.len(), "{what}: total {st}");
    }
    // descending cursor
    let r = k.dispatch("listJobs", &json!({"before": truth.len() as u64, "limit": 2}), T0).await.unwrap();
    let got: Vec<u64> = r["jobs"].as_array().unwrap().iter().map(|j| j["index"].as_u64().unwrap()).collect();
    let want: Vec<u64> = truth.keys().rev().take(2).copied().collect();
    assert_eq!(got, want, "{what}: before");
    // the index never re-scans after it is built
    assert_eq!(k.jobs.rebuilds, 1, "{what}: rebuilds");
}

#[test]
fn job_index_matches_a_full_scan_across_mining_expiry_and_reorgs() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        let mut now = T0;
        now += 1000;
        k.add_tx(unlock(0, 1, pk(2), 10_000_000), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, now).await;
        // 5 jobs: two expire soon, one is mined out by a single block
        let mut n = 0u64;
        for (deadline, max_blocks) in [(5, 10), (6, 10), (100, 1), (100, 10), (100, 10)] {
            k.add_tx(Tx::signed(&seed(2), CHAIN, n, 10, TxBody::PostJob(job_body(deadline, max_blocks))), false).await.unwrap();
            n += 1;
        }
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await;
        check(&mut k, "after posting").await;
        // auto blocks mine jobs from the index (blocks_mined changes, one job is used up)
        for _ in 0..3 {
            now += 1000;
            let b = mine(&mut k, JobChoice::Auto, now).await;
            assert!(!b.header.is_fallback(), "auto offers a job");
        }
        check(&mut k, "after mining").await;
        // past both short deadlines: ExpireJob closes them
        while k.chain.tip_height() < 7 {
            now += 1000;
            mine(&mut k, JobChoice::Fallback, now).await;
        }
        let first = schema::job_id_for_post(&pk(2), 0);
        let second = schema::job_id_for_post(&pk(2), 1);
        k.add_tx(Tx::signed(&seed(2), CHAIN, n, 10, TxBody::ExpireJob { job_id: first }), false).await.unwrap();
        k.add_tx(Tx::signed(&seed(2), CHAIN, n + 1, 10, TxBody::ExpireJob { job_id: second }), false).await.unwrap();
        n += 2;
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await;
        check(&mut k, "after expiry").await;
        // a fork from 2 blocks back: branch A (the tip) posts 2 jobs, branch B posts 1
        // different job and a third expiry, then outgrows A
        let tip_h = k.chain.tip_height();
        let base = k.chain.canonical_hash(tip_h - 1).unwrap();
        let a1 = vec![
            Tx::signed(&seed(2), CHAIN, n, 10, TxBody::PostJob(job_body(200, 10))),
            Tx::signed(&seed(2), CHAIN, n + 1, 10, TxBody::PostJob(job_body(201, 10))),
        ];
        // n is the account nonce at the tip; at `base` it is n - 2 (the two expiries are in the tip)
        let nb = n - 2;
        now += 1000;
        let tip_hash = k.chain.tip().hash;
        let ba = mine_at(&mut k, tip_hash, &a1, 9, now).await;
        assert_eq!(k.chain.tip().hash, ba.hash());
        check(&mut k, "branch A").await;
        let b1 = vec![Tx::signed(&seed(2), CHAIN, nb, 10, TxBody::PostJob(job_body(300, 10)))];
        now += 1000;
        let bb1 = mine_at(&mut k, base, &b1, 8, now).await;
        now += 1000;
        let bb2 = mine_at(&mut k, bb1.hash(), &[], 8, now).await;
        now += 1000;
        let bb3 = mine_at(&mut k, bb2.hash(), &[], 8, now).await;
        assert_eq!(k.chain.tip().hash, bb3.hash(), "branch B is the tip");
        check(&mut k, "after the reorg to B").await;
        // and back: A grows longer again
        let mut p = ba.hash();
        for _ in 0..3 {
            now += 1000;
            p = mine_at(&mut k, p, &[], 9, now).await.hash();
        }
        assert_eq!(k.chain.tip().hash, p, "branch A again");
        check(&mut k, "after the reorg back to A").await;
        // getNodeStats reports the index and the per-method counters
        let s = k.dispatch("getNodeStats", &json!({}), now).await.unwrap();
        assert_eq!(s["job_index"]["built"], json!(true));
        assert!(s["rpc"]["listJobs"]["calls"].as_u64().unwrap() > 10);
        assert!(s["state_cache"]["hits"].as_u64().unwrap() > 0);
    });
}

#[test]
fn listjobs_kind_filter_and_bad_kind() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        k.add_tx(unlock(0, 1, pk(2), 10_000_000), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, T0 + 1000).await;
        k.add_tx(Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::PostJob(job_body(100, 10))), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, T0 + 2000).await;
        let r = k.dispatch("listJobs", &json!({"kind": "job"}), T0).await.unwrap();
        assert_eq!(r["total"], json!(1));
        let r = k.dispatch("listJobs", &json!({"kind": "segment"}), T0).await.unwrap();
        assert_eq!(r["total"], json!(0));
        assert!(k.dispatch("listJobs", &json!({"kind": "nope"}), T0).await.is_err());
        // offset mode keeps its old shape and adds next_cursor
        let r = k.dispatch("listJobs", &json!({"status": "open", "offset": 0, "limit": 200}), T0).await.unwrap();
        assert_eq!(r["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(r["next_cursor"], Value::Null);
    });
}

/// 2026-10-22 (HANDOFF 39): a pooled tx that fails at every tip held its sender's nonce forever.
#[test]
fn failing_pooled_tx_is_dropped_after_three_tips_and_is_replaceable() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        let mut now = T0 + 1000;
        k.add_tx(unlock(0, 1, pk(2), 1_000), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, now).await;
        let bal = k.account(&pk(2)).await.unwrap().balance;
        // admissible (pays its fee) but can never execute: moves more than the balance
        let bad = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: bal * 2 });
        let behind = Tx::signed(&seed(2), CHAIN, 1, 10, TxBody::Transfer { to: pk(8), amount: 1 });
        let hb = k.add_tx(bad.clone(), false).await.unwrap();
        let hn = k.add_tx(behind.clone(), false).await.unwrap();
        for tips in 1..=2u32 {
            now += 1000;
            let b = mine(&mut k, JobChoice::Fallback, now).await;
            assert!(b.txs.is_empty());
            assert_eq!(k.tx_strikes.get(&hb).map(|s| s.0), Some(tips), "strikes after {tips} tips");
            assert!(!k.tx_strikes.contains_key(&hn), "a tx stuck only behind its sender's failing tx is not struck");
            assert!(k.mempool.contains(&hb));
        }
        // the same tip seen again (a template rebuild) does not count twice
        k.check_pool().await;
        assert_eq!(k.tx_strikes.get(&hb).map(|s| s.0), Some(2));
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await;
        assert!(!k.mempool.contains(&hb), "dropped after 3 tips");
        assert!(k.mempool.contains(&hn), "later nonces stay for the sender to fill the gap");
        let st = k.dispatch("getTxStatus", &json!({"tx_hash": hex::encode(hb)}), now).await.unwrap();
        assert_eq!(st["status"], json!("dropped"), "{st}");
        assert!(st["reason"].as_str().unwrap().contains("3 consecutive tips"), "{st}");
        // the sender sends at that nonce again at the SAME fee: accepted, and both mine
        let fix = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: 1 });
        k.add_tx(fix, false).await.unwrap();
        now += 1000;
        let b = mine(&mut k, JobChoice::Fallback, now).await;
        assert_eq!(b.txs.len(), 2);
        assert!(k.mempool.is_empty());
        let s = k.dispatch("getNodeStats", &json!({}), now).await.unwrap();
        assert_eq!(s["mempool"]["evicted"], json!(1));
    });
}

#[test]
fn failing_pooled_tx_can_be_replaced_at_the_same_fee_before_it_is_dropped() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        let mut now = T0 + 1000;
        k.add_tx(unlock(0, 1, pk(2), 1_000), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, now).await;
        let bal = k.account(&pk(2)).await.unwrap().balance;
        let bad = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: bal * 2 });
        k.add_tx(bad.clone(), false).await.unwrap();
        let good = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: 1 });
        // not yet seen failing: the usual 10% replacement rule
        assert!(k.add_tx(good.clone(), false).await.unwrap_err().contains("pooled with a fee"));
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await;
        // failing at the tip now: replaced at the same fee
        let h = k.add_tx(good, false).await.unwrap();
        assert!(!k.mempool.contains(&bad.hash()) && k.mempool.contains(&h));
        now += 1000;
        let b = mine(&mut k, JobChoice::Fallback, now).await;
        assert_eq!(b.txs.len(), 1);
    });
}

/// A tx that fails once because another sender's tx must land first is not dropped.
#[test]
fn a_tx_that_succeeds_again_loses_its_strikes() {
    deterministic::Runner::default().start(|ctx| async move {
        let chain = Chain::open(ctx.child("c"), "n", genesis(), MockVerifier).await.unwrap();
        let mut k: K = Core::new(chain);
        let mut now = T0 + 1000;
        k.add_tx(unlock(0, 1, pk(2), 1_000), false).await.unwrap();
        k.add_tx(unlock(1, 2, pk(3), 1), false).await.unwrap();
        mine(&mut k, JobChoice::Fallback, now).await;
        let b2 = k.account(&pk(2)).await.unwrap().balance;
        let b3 = k.account(&pk(3)).await.unwrap().balance;
        // pk 3 spends more than it has until pk 2's transfer to it lands
        let spend = Tx::signed(&seed(3), CHAIN, 0, 10, TxBody::Transfer { to: pk(8), amount: b3 + b2 / 2 });
        let h3 = k.add_tx(spend, false).await.unwrap();
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await;
        assert_eq!(k.tx_strikes.get(&h3).map(|s| s.0), Some(1));
        k.add_tx(Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(3), amount: b2 / 2 + 100 }), false).await.unwrap();
        now += 1000;
        mine(&mut k, JobChoice::Fallback, now).await; // pk 2's transfer lands (pk 3's may too)
        if k.mempool.contains(&h3) {
            assert!(!k.tx_strikes.contains_key(&h3), "succeeds at the new tip: strikes cleared");
            now += 1000;
            mine(&mut k, JobChoice::Fallback, now).await;
        }
        assert!(!k.mempool.contains(&h3));
        assert_eq!(k.evicted, 0);
    });
}
