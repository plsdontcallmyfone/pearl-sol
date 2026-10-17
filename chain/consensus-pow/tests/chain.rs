//! Chain manager tests in Commonware's deterministic runtime (in-memory storage).
//! Fork-choice tests use MockVerifier (proof = "mock" || pow_digest); `real_pearl_*`
//! tests mine and verify real pearl-job block proofs at an easy target.

use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Accept, Chain, ChainError, ChainEvent, Genesis, JobChoice, MockVerifier, PearlVerifier, PowVerifier, Template};
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
    Block { header: h, proof, txs: t.txs.clone() }
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

#[test]
fn linear_chain_finality_and_restart() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0;
        // fund alice through the bridge, then 10 blocks
        now += 1000;
        let b = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000)], now).await;
        add(&mut c, b, now).await;
        let t0 = c.tip().hash;
        let tip = extend(&mut c, t0, 9, 9, &mut now).await;
        assert_eq!(c.tip_height(), 10);
        assert_eq!(c.finalized().header.height, 7);
        assert_eq!(balance(&c, &pk(2)).await, 1_000_000);
        assert_eq!(c.store().committed_height().await.unwrap(), Some(7));
        assert!(c.live_count() <= 4);
        drop(c);
        let c2 = C::open(ctx.child("b"), "n", genesis(3), MockVerifier).await.unwrap();
        assert_eq!(c2.genesis_hash(), g);
        assert_eq!(c2.tip().hash, tip);
        assert_eq!(c2.finalized().header.height, 7);
        assert_eq!(balance(&c2, &pk(2)).await, 1_000_000);
        // the genesis hash depends on the spec: a different spec refuses this data
        assert!(C::open(ctx.child("c"), "n", genesis(4), MockVerifier).await.is_err());
    });
}

#[test]
fn fork_race_heaviest_wins_ties_keep_first() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(10), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let now = T0 + 1000;
        let a = mine_on(&c, g, 3, &[], now).await;
        let b = mine_on(&c, g, 4, &[], now).await;
        assert_ne!(a.hash(), b.hash());
        add(&mut c, a.clone(), now).await;
        let (acc, ev) = add(&mut c, b.clone(), now).await;
        assert_eq!(acc, Accept::Added { tip: false }, "equal work: first seen stays tip");
        assert!(ev.is_empty());
        assert_eq!(c.tip().hash, a.hash());
        // b's branch gets one more block and wins
        let b2 = mine_on(&c, b.hash(), 4, &[], now + 1000).await;
        let (acc, ev) = add(&mut c, b2.clone(), now + 1000).await;
        assert_eq!(acc, Accept::Added { tip: true });
        assert_eq!(c.tip().hash, b2.hash());
        match &ev[0] {
            ChainEvent::NewTip { reorg_depth, disconnected, connected, .. } => {
                assert_eq!(*reorg_depth, 1);
                assert_eq!(disconnected, &vec![a.hash()]);
                assert_eq!(connected, &vec![b.hash(), b2.hash()]);
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(c.canonical_hash(1), Some(b.hash()));
    });
}

#[test]
fn reorg_within_k_and_double_spend() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(5), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let fund = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000)], now).await;
        let fork = fund.hash();
        add(&mut c, fund, now).await;
        // Branch A: alice pays bob. Branch B: the same nonce pays carol.
        let to_bob = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(3), amount: 600_000 });
        let to_carol = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::Transfer { to: pk(4), amount: 600_000 });
        now += 1000;
        let a1 = mine_on(&c, fork, 5, &[to_bob.clone()], now).await;
        add(&mut c, a1.clone(), now).await;
        let a_tip = extend(&mut c, a1.hash(), 1, 5, &mut now).await;
        assert_eq!(balance(&c, &pk(3)).await, 600_000);
        assert_eq!(c.tx_location(&to_bob.hash()).unwrap().0, a1.hash());
        // B: 3 blocks from the fork (reorg depth 2 < K = 5)
        let mut bnow = T0 + 2000;
        let b1 = mine_on(&c, fork, 6, &[to_carol.clone()], bnow).await;
        add(&mut c, b1.clone(), bnow).await;
        let b_tip = extend(&mut c, b1.hash(), 2, 6, &mut bnow).await;
        assert_eq!(c.tip().hash, b_tip);
        assert_ne!(c.tip().hash, a_tip);
        assert_eq!(balance(&c, &pk(3)).await, 0, "bob's payment was reorged out");
        assert_eq!(balance(&c, &pk(4)).await, 600_000);
        assert_eq!(balance(&c, &pk(2)).await, 1_000_000 - 600_000 - 10);
        assert!(c.tx_location(&to_bob.hash()).is_none());
        assert_eq!(c.tx_location(&to_carol.hash()).unwrap().0, b1.hash());
        // The orphaned tx cannot be replayed on B: its nonce is spent.
        let bad = mine_on(&c, b_tip, 6, &[], bnow + 1000).await;
        let mut forged = bad.clone();
        forged.txs = vec![to_bob.clone()];
        forged.header.tx_root = block::tx_root(&forged.txs);
        let proof = MockVerifier::proof_for(&forged.header);
        forged.header.proof_hash = block::proof_hash(&proof);
        forged.proof = proof;
        let e = c.insert_block(forged, bnow + 1000).await.unwrap_err();
        assert!(e.to_string().contains("nonce"), "{e}");
        let t = c.build_template(pk(6), JobChoice::Fallback, &[to_bob], bnow + 1000).await.unwrap();
        assert!(t.txs.is_empty(), "template skips the double spend");
    });
}

#[test]
fn refuses_reorg_deeper_than_k() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0;
        let h2 = extend(&mut c, g, 2, 3, &mut now).await;
        // Build a long private branch from h2 in a second chain instance that saw only h2.
        let mut other = C::open(ctx.child("b"), "o", genesis(3), MockVerifier).await.unwrap();
        let mut onow = T0;
        let mut blocks = Vec::new();
        for h in 1..=2 {
            let b = c.get_block(&c.canonical_hash(h).unwrap()).await.unwrap().unwrap();
            add(&mut other, b, now).await;
        }
        onow += 2000;
        let mut p = h2;
        for _ in 0..10 {
            onow += 500; // faster blocks: harder target, so this branch is heavier too
            let b = mine_on(&other, p, 4, &[], onow).await;
            p = b.hash();
            blocks.push(b.clone());
            add(&mut other, b, onow).await;
        }
        // Main chain moves 5 blocks past h2: F is now at height 4.
        let _ = extend(&mut c, h2, 5, 3, &mut now).await;
        assert_eq!(c.finalized().header.height, 4);
        // The private branch forks at height 2 < F: refused, even though it is longer.
        let e = c.insert_block(blocks[0].clone(), onow).await.unwrap_err();
        assert_eq!(e, ChainError::TooDeep);
        for b in &blocks[1..] {
            let r = c.insert_block(b.clone(), onow).await;
            assert!(matches!(r, Err(ChainError::TooDeep)), "{r:?}");
        }
        assert_eq!(c.tip_height(), 7);
        let (hd, pf) = (blocks[3].header, blocks[3].proof.clone());
        assert_eq!(c.insert_header(hd, pf).await.unwrap_err(), ChainError::TooDeep);
    });
}

#[test]
fn bad_pow_and_bad_roots_rejected() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let now = T0 + 1000;
        let good = mine_on(&c, g, 3, &[], now).await;
        // proof for another header
        let mut b = good.clone();
        b.header.miner = pk(4);
        let e = c.insert_block(b, now).await.unwrap_err();
        assert!(e.to_string().contains("proof"), "{e}");
        // proof bytes that do not match proof_hash
        let mut b = good.clone();
        b.proof[5] ^= 1;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("proof_hash"));
        // wrong state root (resealed so PoW is valid)
        let mut b = good.clone();
        b.header.state_root[0] ^= 1;
        let p = MockVerifier::proof_for(&b.header);
        b.header.proof_hash = block::proof_hash(&p);
        b.proof = p;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("state_root"));
        // wrong target bits
        let mut b = good.clone();
        b.header.target_bits = 0x1f7fffff;
        let p = MockVerifier::proof_for(&b.header);
        b.header.proof_hash = block::proof_hash(&p);
        b.proof = p;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("target_bits"));
        // timestamp too far ahead
        let mut b = good.clone();
        b.header.timestamp_ms = now + 10 * 60_000;
        let p = MockVerifier::proof_for(&b.header);
        b.header.proof_hash = block::proof_hash(&p);
        b.proof = p;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("future"));
        // bad tx signature
        let mut t = unlock(200, 0, 1, pk(2), 5);
        t.sig[3] ^= 1;
        let tpl = c.build_template(pk(3), JobChoice::Fallback, &[], now).await.unwrap();
        let mut b = seal_mock(&tpl);
        b.txs = vec![t];
        b.header.tx_root = block::tx_root(&b.txs);
        let p = MockVerifier::proof_for(&b.header);
        b.header.proof_hash = block::proof_hash(&p);
        b.proof = p;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("signature"));
        add(&mut c, good, now).await;
        assert_eq!(c.tip_height(), 1);
    });
}

#[test]
fn orphans_connect_when_parent_arrives() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut src = C::open(ctx.child("a"), "s", genesis(10), MockVerifier).await.unwrap();
        let mut dst = C::open(ctx.child("b"), "d", genesis(10), MockVerifier).await.unwrap();
        let mut now = T0;
        let g0 = src.genesis_hash();
        let tip = extend(&mut src, g0, 4, 3, &mut now).await;
        let mut blocks = Vec::new();
        for h in 1..=4 {
            blocks.push(src.get_block(&src.canonical_hash(h).unwrap()).await.unwrap().unwrap());
        }
        for b in blocks.iter().rev().take(3) {
            let (a, _) = add(&mut dst, b.clone(), now).await;
            assert!(matches!(a, Accept::Orphan { .. }));
        }
        let (a, ev) = add(&mut dst, blocks[0].clone(), now).await;
        assert_eq!(a, Accept::Added { tip: true });
        assert!(!ev.is_empty());
        assert_eq!(dst.tip().hash, tip);
        assert_eq!(dst.orphan_count(), 0);
    });
}

// ---------------------------------------------------------------- real proofs

fn seal_real(t: &Template, a: &[i8], b: &[i8]) -> Block {
    let ph = consensus_pow::verifier::to_pearl_header(&t.header);
    let mined = match &t.job {
        None => pearl_job::pow::mine_header(&ph, &pearl_job::BlockWork::Fallback(t.fallback), 0..64).unwrap(),
        Some(j) => {
            let prep = pearl_job::PreparedJob::new(j.pearl_job().unwrap(), a, b).unwrap();
            pearl_job::pow::mine_header(&ph, &pearl_job::BlockWork::Job(&prep), 0..64).unwrap()
        }
    }
    .expect("easy target wins within 64 nonces");
    let header = Header::decode(&mined.header.to_bytes()).unwrap();
    Block { header, proof: mined.proof, txs: t.txs.clone() }
}

#[test]
fn real_pearl_fallback_and_job_blocks() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), PearlVerifier).await.unwrap();
        let mut now = T0 + 1000;
        // fallback block with a real proof, funding the poster
        let t = c.build_template(pk(9), JobChoice::Fallback, &[unlock(200, 0, 1, pk(2), 10_000_000)], now).await.unwrap();
        let b = seal_real(&t, &[], &[]);
        assert!(b.proof.len() > 1000);
        add(&mut c, b.clone(), now).await;
        // a tampered real proof is rejected
        let t2 = c.build_template(pk(9), JobChoice::Fallback, &[], now + 1000).await.unwrap();
        let mut bad = seal_real(&t2, &[], &[]);
        let last = bad.proof.len() - 1;
        bad.proof[last] ^= 0x55;
        bad.header.proof_hash = block::proof_hash(&bad.proof);
        assert!(c.insert_block(bad, now + 1000).await.unwrap_err().to_string().contains("proof of work"));
        // post a job with real matrices and commitments
        let cfg = pearl_job::MiningConfig::localnet_default();
        let (m, n, k) = (64u32, 64u32, cfg.k());
        let a: Vec<i8> = (0..m as usize * k).map(|i| (((i * 7) % 129) as i16 - 64) as i8).collect();
        let bm: Vec<i8> = (0..k * n as usize).map(|i| (((i * 11) % 129) as i16 - 64) as i8).collect();
        let job_id = schema::job_id_for_post(&pk(2), 0);
        let (w, p, deadline, max_blocks) = (1_000_000u64, 5_000_000u64, 100u64, 10u32);
        let terms = schema::terms_hash(&pk(2), &job_id, m, n, k as u32, w, p, deadline, max_blocks);
        let jh = pearl_job::JobHeader { job_pubkey: job_id, terms_hash: terms, created_slot: 0, target_bits: 0 };
        let jk = pearl_job::job_key(&jh, &cfg);
        let (ha, hb) = pearl_job::commit(&jk, &a, &bm, k, n as usize);
        let post = Tx::signed(
            &seed(2),
            CHAIN,
            0,
            10,
            TxBody::PostJob(PostJob {
                m,
                n,
                k: k as u32,
                config: cfg.to_bytes(),
                hash_a: ha,
                hash_b: hb,
                data_uri: b"mem://test".to_vec(),
                worker_pay: w,
                protocol_fee: p,
                deadline_height: deadline,
                max_blocks,
                evaluators: vec![pk(60)],
                threshold: 1,
            }),
        );
        now += 1000;
        let t = c.build_template(pk(9), JobChoice::Fallback, &[post], now).await.unwrap();
        add(&mut c, seal_real(&t, &[], &[]), now).await;
        // Auto picks the job; mine it with real proofs on its own matrices.
        now += 1000;
        let t = c.build_template(pk(9), JobChoice::Auto, &[], now).await.unwrap();
        assert_eq!(t.header.job_id, job_id);
        assert!(t.summary.subsidy > 0);
        let jb = seal_real(&t, &a, &bm);
        let (acc, _) = add(&mut c, jb, now).await;
        assert_eq!(acc, Accept::Added { tip: true });
        // a proof mined on other matrices fails against the job's commitments
        now += 1000;
        let t = c.build_template(pk(9), JobChoice::Job(job_id), &[], now).await.unwrap();
        let mut fb_header = t.clone();
        fb_header.job = None;
        fb_header.header.job_id = [0; 32];
        let wrong = seal_real(&fb_header, &[], &[]);
        let mut forged = wrong.clone();
        forged.header.job_id = job_id;
        assert!(c.insert_block(forged, now).await.is_err());
        let ok = seal_real(&t, &a, &bm);
        add(&mut c, ok, now).await;
        assert_eq!(c.tip_height(), 4);
        let j = c.read_tip(&schema::job_key(&job_id)).await.unwrap().unwrap();
        let j = schema::JobRecord::decode(&j).unwrap();
        assert_eq!(j.blocks_mined, 2);
    });
}
