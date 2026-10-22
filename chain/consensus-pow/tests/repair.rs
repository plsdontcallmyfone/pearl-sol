#![allow(dead_code)]
//! Chain manager tests in Commonware's deterministic runtime (in-memory storage).
//! Fork-choice tests use MockVerifier (proof = "mock" || pow_digest); `real_pearl_*`
//! tests mine and verify real pearl-job block proofs at an easy target.

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Accept, Chain, ChainEvent, Genesis, JobChoice, MockVerifier, PowVerifier, Template};
use state::config::Config;
use state::schema::{self, Account};

const CHAIN: u32 = 7;
const EASY: u32 = 0x207fffff;
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
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: EASY, config: c }
}

fn seal_mock(t: &Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: Vec::new() }
}

type C<V> = Chain<deterministic::Context, V>;

async fn mine_on<V: PowVerifier>(c: &C<V>, parent: Hash, miner: u8, txs: &[Tx], now: u64) -> Block {
    let t = c.build_template_at(parent, pk(miner), JobChoice::Fallback, txs, now).await.unwrap();
    assert_eq!(t.txs.len(), txs.len(), "all txs fit");
    seal_mock(&t)
}

async fn add<V: PowVerifier>(c: &mut C<V>, b: Block, now: u64) -> (Accept, Vec<ChainEvent>) {
    c.insert_block(b, now).await.unwrap()
}

async fn balance<V: PowVerifier>(c: &C<V>, who: &Pubkey) -> u64 {
    c.read_tip(&schema::account_key(who)).await.unwrap().map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
}

fn unlock(relayer: u8, rnonce: u64, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(nonce, &to, amount_spl);
    let sigs = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
    Tx::signed(&seed(relayer), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
}

/// Extend `parent` by `n` empty fallback blocks; returns the last hash.
async fn extend<V: PowVerifier>(c: &mut C<V>, mut parent: Hash, n: usize, miner: u8, now: &mut u64) -> Hash {
    for _ in 0..n {
        *now += 1000;
        let b = mine_on(c, parent, miner, &[], *now).await;
        parent = b.hash();
        add(c, b, *now).await;
    }
    parent
}


/// The devnet stall at 5762 (2026-10-22): a node holding the canonical tip only as orphans
/// (their root's parent never fetched) must learn what to ask for again, and connect once given.
#[test]
fn orphan_chain_repair_names_the_missing_ancestor() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut src = C::open(ctx.child("a"), "s", genesis(10), MockVerifier).await.unwrap();
        let mut dst = C::open(ctx.child("b"), "d", genesis(10), MockVerifier).await.unwrap();
        let mut now = T0;
        let g0 = src.genesis_hash();
        // a shared prefix of 2 blocks
        let base = extend(&mut src, g0, 2, 3, &mut now).await;
        for h in 1..=2 {
            let b = src.get_block(&src.canonical_hash(h).unwrap()).await.unwrap().unwrap();
            add(&mut dst, b, now).await;
        }
        assert_eq!(dst.tip().hash, base);
        // dst mines its own block 3; src builds 3', 4', 5' (heavier)
        now += 1000;
        let own = mine_on(&dst, base, 4, &[], now).await;
        add(&mut dst, own.clone(), now).await;
        let tip = extend(&mut src, base, 3, 3, &mut now).await;
        let fork: Vec<Block> = {
            let mut v = Vec::new();
            for h in 3..=5 { v.push(src.get_block(&src.canonical_hash(h).unwrap()).await.unwrap().unwrap()); }
            v
        };
        assert!(dst.repair_for(&tip).is_none(), "unknown tip: the tip announcement path fetches it");
        // 4' and 5' arrive by gossip, 3' is lost
        for b in &fork[1..] {
            let (a, _) = add(&mut dst, b.clone(), now).await;
            assert!(matches!(a, Accept::Orphan { .. }));
        }
        assert!(dst.is_known(&tip), "the orphan tip is known: tip announcements alone never fetch again");
        assert_eq!(dst.repair_for(&tip), Some(consensus_pow::Repair::Header(fork[0].hash())));
        // its header arrives but not the body: ask for the body again
        dst.insert_header(fork[0].header, fork[0].proof.clone()).await.unwrap();
        assert_eq!(dst.repair_for(&tip), Some(consensus_pow::Repair::Body(fork[0].hash())));
        let first = dst.next_bodies(512);
        assert_eq!(first, vec![fork[0].hash()]);
        assert!(dst.next_bodies(512).is_empty(), "handed out once");
        dst.reask_body(&fork[0].hash());
        assert_eq!(dst.next_bodies(512), vec![fork[0].hash()], "a lost body fetch is handed out again");
        // the body arrives: the orphans connect and the heavier branch wins
        let (a, _) = add(&mut dst, fork[0].clone(), now).await;
        assert_eq!(a, Accept::Added { tip: true });
        assert_eq!(dst.tip().hash, tip);
        assert_eq!(dst.repair_for(&tip), None);
        assert_eq!(dst.orphan_count(), 0);
        assert!(!dst.is_canonical(&own.hash(), 3));
    });
}
