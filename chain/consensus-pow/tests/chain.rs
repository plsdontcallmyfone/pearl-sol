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
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts }
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
        forged.header.tx_root = forged.tx_root();
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
        let e = c.insert_block(b.clone(), now).await.unwrap_err();
        assert!(e.to_string().contains("coinbase receipt does not match"), "{e}");
        b.coinbase.as_mut().unwrap().miner = pk(4);
        b.header.tx_root = b.tx_root();
        let e = c.insert_block(b, now).await.unwrap_err();
        assert!(e.to_string().contains("proof"), "{e}");
        // R1: a receipt claiming more than execution produced (resealed: valid PoW, tx_root)
        let mut b = good.clone();
        b.coinbase.as_mut().unwrap().fees += 1;
        b.header.tx_root = b.tx_root();
        let p = MockVerifier::proof_for(&b.header);
        b.header.proof_hash = block::proof_hash(&p);
        b.proof = p;
        let e = c.insert_block(b, now).await.unwrap_err();
        assert!(e.to_string().contains("coinbase receipt mismatch"), "{e}");
        // a body without its receipt, or a receipt the header does not commit to
        let mut b = good.clone();
        b.coinbase = None;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("missing coinbase"));
        let mut b = good.clone();
        b.coinbase.as_mut().unwrap().subsidy = 5;
        assert!(c.insert_block(b, now).await.unwrap_err().to_string().contains("tx_root"));
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
        b.header.tx_root = b.tx_root();
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
        None => pearl_job::pow::mine_header(&ph, &pearl_job::BlockWork::Fallback(t.fallback), 0..4096).unwrap(),
        Some(j) => {
            let prep = pearl_job::PreparedJob::new(j.pearl_job().unwrap(), a, b).unwrap();
            pearl_job::pow::mine_header(&ph, &pearl_job::BlockWork::Job(&prep), 0..4096).unwrap()
        }
    }
    .expect("easy target wins within 4096 nonces");
    let header = Header::decode(&mined.header.to_bytes()).unwrap();
    Block { header, proof: mined.proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts }
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

// ---------------------------------------------------------------- M2: rewards, history, DA, fallback size

fn transfer(from: u8, nonce: u64, to: Pubkey, amount: u64) -> Tx {
    Tx::signed(&seed(from), CHAIN, nonce, 10, TxBody::Transfer { to, amount })
}

fn hist<V: PowVerifier>(c: &C<V>, who: &Pubkey, before: Option<(u64, u32)>, limit: usize) -> (Vec<(u64, u32)>, bool) {
    let (v, more) = c.account_txs(who, before, limit);
    (v.iter().map(|e| (e.height, e.index)).collect(), more)
}

#[test]
fn rewards_and_history_across_reorg_and_restart() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(5), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let fund = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000)], now).await;
        let fork = fund.hash();
        add(&mut c, fund, now).await;
        // A: alice pays bob twice (heights 2 and 3)
        now += 1000;
        let a1 = mine_on(&c, fork, 5, &[transfer(2, 0, pk(3), 1000)], now).await;
        add(&mut c, a1.clone(), now).await;
        now += 1000;
        let a2 = mine_on(&c, a1.hash(), 5, &[transfer(2, 1, pk(3), 1000)], now).await;
        add(&mut c, a2.clone(), now).await;
        assert_eq!(hist(&c, &pk(2), None, 10), (vec![(3, 0), (2, 0), (1, 0)], false));
        assert_eq!(hist(&c, &pk(3), None, 10).0, vec![(3, 0), (2, 0)]);
        // B: alice pays carol, two txs in one block, and B gets heavier (reorg depth 2)
        let mut bnow = T0 + 2000;
        let b1 = mine_on(&c, fork, 6, &[transfer(2, 0, pk(4), 1000), transfer(2, 1, pk(4), 2000)], bnow).await;
        add(&mut c, b1.clone(), bnow).await;
        let b_tip = extend(&mut c, b1.hash(), 2, 6, &mut bnow).await;
        assert_eq!(c.tip().hash, b_tip);
        assert!(hist(&c, &pk(3), None, 10).0.is_empty(), "bob's reorged-out payments leave his history");
        assert_eq!(hist(&c, &pk(4), None, 10).0, vec![(2, 1), (2, 0)]);
        // pagination: newest first, cursor = last (height, index), strictly before it
        assert_eq!(hist(&c, &pk(2), None, 2), (vec![(2, 1), (2, 0)], true));
        assert_eq!(hist(&c, &pk(2), Some((2, 0)), 2), (vec![(1, 0)], false));
        assert_eq!(hist(&c, &pk(2), Some((2, 1)), 1), (vec![(2, 0)], true));
        // per-block rewards: fees of both branches are kept by hash, subsidy 0 on fallback
        let r = c.block_rewards(&b1.hash()).await.unwrap().unwrap();
        assert_eq!((r.fees, r.subsidy), (20, 0));
        let r = c.block_rewards(&a2.hash()).await.unwrap().unwrap();
        assert_eq!((r.fees, r.subsidy), (10, 0), "side-branch block still reports its rewards");
        assert_eq!(c.block_rewards(&g).await.unwrap().unwrap().fees, 0);
        // finalize everything up to b1 and beyond; the final index gives the same answers
        let tip = extend(&mut c, b_tip, 6, 6, &mut bnow).await;
        assert!(c.finalized().header.height >= 4);
        assert_eq!(hist(&c, &pk(2), None, 10), (vec![(2, 1), (2, 0), (1, 0)], false));
        assert_eq!(hist(&c, &pk(2), Some((2, 1)), 10).0, vec![(2, 0), (1, 0)]);
        assert!(hist(&c, &pk(3), None, 10).0.is_empty());
        // matured coinbase shows up in a later block's rewards
        let mut matured = 0;
        for h in 1..=c.tip_height() {
            let hh = c.canonical_hash(h).unwrap();
            matured += c.block_rewards(&hh).await.unwrap().unwrap().matured;
        }
        assert!(matured >= 20, "the fees of b1 matured after K ({matured})");
        let fin = c.canonical_hash(2).unwrap();
        assert_eq!(c.block_rewards(&fin).await.unwrap().unwrap().fees, 20, "final block rewards read from the index");
        drop(c);
        // restart: history rebuilt from the stored canonical chain
        let c2 = C::open(ctx.child("b"), "n", genesis(5), MockVerifier).await.unwrap();
        assert_eq!(c2.tip().hash, tip);
        assert_eq!(hist(&c2, &pk(2), None, 10), (vec![(2, 1), (2, 0), (1, 0)], false));
        assert_eq!(hist(&c2, &pk(4), None, 1), (vec![(2, 1)], true));
        assert!(hist(&c2, &pk(3), None, 10).0.is_empty());
        assert_eq!(c2.block_rewards(&b1.hash()).await.unwrap().unwrap().fees, 20);
    });
}

fn clear_tx(sender: u8, nonce: u64, account: Pubkey, route_nonce: u64) -> Tx {
    let msg = tx::route_clear_message(CHAIN, &account, route_nonce);
    Tx::signed(&seed(sender), CHAIN, nonce, 10, TxBody::ClearRoute { account, route_nonce, sig: sign_plain(&seed(70), &msg) })
}

/// R7 + R8: ClearRoute txs naming an account are in its history (sent by anyone), and
/// `last_clear` / `route_set_tx` locate the clear and the current SetRoute on the
/// canonical chain: a reorg drops a clear, finalization keeps it, a restart rebuilds it.
#[test]
fn route_txs_across_reorg_finality_and_restart() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(5), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let fund = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000), unlock(200, 1, 2, pk(4), 1000)], now).await;
        add(&mut c, fund.clone(), now).await;
        now += 1000;
        let set = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::SetRoute { solana_recipient: [0x4c; 32], route_bps: 5000, until_height: 1000, cap: u64::MAX, release_key: pk(70) });
        let sb = mine_on(&c, fund.hash(), 9, &[transfer(4, 0, pk(5), 1), set], now).await;
        let fork = sb.hash();
        add(&mut c, sb, now).await;
        let e = c.route_set_tx(&pk(2), 2, 0).await.unwrap().unwrap();
        assert_eq!((e.block_hash, e.height, e.index), (fork, 2, 1));
        assert!(c.last_clear(&pk(2)).is_none());
        // A: a third party (pk 4) sends the clear at height 3
        now += 1000;
        let a1 = mine_on(&c, fork, 5, &[clear_tx(4, 1, pk(2), 0)], now).await;
        add(&mut c, a1.clone(), now).await;
        let lc = c.last_clear(&pk(2)).unwrap();
        assert_eq!((lc.block_hash, lc.height, lc.index, lc.route_nonce), (a1.hash(), 3, 0, 0));
        assert_eq!(hist(&c, &pk(2), None, 10).0, vec![(3, 0), (2, 1), (1, 0)]);
        assert!(c.last_clear(&pk(4)).is_none(), "the sender is not the lien account");
        // B: heavier branch without the clear (reorg depth 1)
        let mut bnow = now;
        let b_tip = extend(&mut c, fork, 2, 6, &mut bnow).await;
        assert_eq!(c.tip().hash, b_tip);
        assert!(c.last_clear(&pk(2)).is_none(), "the reorged-out clear is gone");
        assert_eq!(hist(&c, &pk(2), None, 10).0, vec![(2, 1), (1, 0)]);
        // on B the clear lands at height 5, then finalizes
        bnow += 1000;
        let b3 = mine_on(&c, b_tip, 6, &[clear_tx(4, 1, pk(2), 0)], bnow).await;
        add(&mut c, b3.clone(), bnow).await;
        let tip = extend(&mut c, b3.hash(), 7, 6, &mut bnow).await;
        assert!(c.finalized().header.height >= 5);
        let lc = c.last_clear(&pk(2)).unwrap();
        assert_eq!((lc.block_hash, lc.height, lc.index), (b3.hash(), 5, 0), "read from the final index");
        assert_eq!(hist(&c, &pk(2), None, 1).0, vec![(5, 0)]);
        let e = c.route_set_tx(&pk(2), 2, 0).await.unwrap().unwrap();
        assert_eq!((e.block_hash, e.index), (fork, 1), "final SetRoute block read from the archive");
        drop(c);
        let c2 = C::open(ctx.child("b"), "n", genesis(5), MockVerifier).await.unwrap();
        assert_eq!(c2.tip().hash, tip);
        let lc = c2.last_clear(&pk(2)).unwrap();
        assert_eq!((lc.block_hash, lc.height, lc.index, lc.route_nonce), (b3.hash(), 5, 0, 0));
        assert_eq!(c2.route_set_tx(&pk(2), 2, 0).await.unwrap().unwrap().index, 1);
    });
}

fn mock_job_body(data_uri: &[u8], p: u64) -> PostJob {
    PostJob {
        m: 64,
        n: 64,
        k: pearl_job::MiningConfig::localnet_default().common_dim,
        config: pearl_job::MiningConfig::localnet_default().to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: data_uri.to_vec(),
        worker_pay: 1000,
        protocol_fee: p,
        deadline_height: 100,
        max_blocks: 10,
        evaluators: vec![pk(60)],
        threshold: 1,
    }
}

#[test]
fn template_skips_jobs_whose_data_is_unavailable() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(5), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let b = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1_000_000)], now).await;
        add(&mut c, b, now).await;
        now += 1000;
        let post = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::PostJob(mock_job_body(format!("blob:{}", "ab".repeat(32)).as_bytes(), 5_000_000)));
        let job_id = schema::job_id_for_post(&pk(2), 0);
        let b = mine_on(&c, c.tip().hash, 9, &[post], now).await;
        add(&mut c, b, now).await;
        now += 1000;
        let tip = c.tip().hash;
        let none = |_: &schema::JobRecord| false;
        let all = |_: &schema::JobRecord| true;
        // Auto with the data missing: a fallback template
        let t = c.build_template_filtered(tip, pk(9), JobChoice::Auto, &[], now, &none).await.unwrap();
        assert!(t.job.is_none() && t.header.job_id == [0; 32]);
        // naming the job: refused with a reason
        let e = c.build_template_filtered(tip, pk(9), JobChoice::Job(job_id), &[], now, &none).await.unwrap_err();
        assert!(e.to_string().contains("not available"), "{e}");
        // data present: the job is offered and its block is valid for every node
        let t = c.build_template_filtered(tip, pk(9), JobChoice::Auto, &[], now, &all).await.unwrap();
        assert_eq!(t.header.job_id, job_id);
        assert!(t.summary.subsidy > 0);
        let jb = seal_mock(&t);
        let h = jb.hash();
        add(&mut c, jb, now).await;
        let r = c.block_rewards(&h).await.unwrap().unwrap();
        assert_eq!((r.fees, r.subsidy), (0, t.summary.subsidy));
    });
}

#[test]
fn fallback_size_is_a_chain_parameter() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), PearlVerifier).await.unwrap();
        let mut now = T0 + 1000;
        // default 4 x 8 at k = 2048, r = 128 (Pearl rank-penalty floor)
        let t = c.build_template(pk(9), JobChoice::Fallback, &[unlock(200, 0, 1, pk(1), 1000)], now).await.unwrap();
        assert_eq!((t.fallback.m, t.fallback.n, t.fallback.config.k(), t.fallback.config.r()), (4, 8, 2048, 128));
        let b = seal_real(&t, &[], &[]);
        add(&mut c, b, now).await;
        // the admin changes the shape to 8 x 16
        let set = |nonce: u64, key: u8, v: u32| Tx::signed(&seed(1), CHAIN, nonce, 10, TxBody::AdminConfig { key, value: v.to_le_bytes().to_vec() });
        now += 1000;
        let t = c
            .build_template(pk(9), JobChoice::Fallback, &[set(0, state::config::keys::FALLBACK_M, 8), set(1, state::config::keys::FALLBACK_N, 16)], now)
            .await
            .unwrap();
        assert_eq!(t.txs.len(), 2);
        assert_eq!((t.fallback.m, t.fallback.n), (4, 8), "this block is still verified with the parent's params");
        add(&mut c, seal_real(&t, &[], &[]), now).await;
        now += 1000;
        let t = c.build_template(pk(9), JobChoice::Fallback, &[], now).await.unwrap();
        assert_eq!((t.fallback.m, t.fallback.n), (8, 16));
        // a proof built with the old shape no longer verifies
        let mut old = t.clone();
        old.fallback = pearl_job::FallbackParams::default();
        let bad = seal_real(&old, &[], &[]);
        assert!(c.insert_block(bad, now).await.unwrap_err().to_string().contains("proof of work"));
        add(&mut c, seal_real(&t, &[], &[]), now).await;
        assert_eq!(c.tip_height(), 3);
        // a shape the config cannot tile is refused by AdminConfig
        now += 1000;
        let t = c.build_template(pk(9), JobChoice::Fallback, &[set(2, state::config::keys::FALLBACK_M, 6)], now).await.unwrap();
        assert!(t.txs.is_empty(), "m = 6 is not a multiple of the 4-row tile");
        // a fallback config below the rank-penalty floor (the pre-port r = 32) is refused
        let old_cfg = pearl_job::MiningConfig { common_dim: 1024, rank: 32, ..pearl_job::MiningConfig::localnet_default() };
        let tx = Tx::signed(&seed(1), CHAIN, 2, 10, TxBody::AdminConfig { key: state::config::keys::FALLBACK_CONFIG, value: old_cfg.to_bytes().to_vec() });
        let t = c.build_template(pk(9), JobChoice::Fallback, &[tx], now).await.unwrap();
        assert!(t.txs.is_empty(), "r = 32 is below the rank-penalty floor");
    });
}

/// Historical withdrawal proofs (getWithdrawalProof at_height) are the same after a
/// restart: the per-height log sizes are rebuilt from the committed log.
#[test]
fn historical_withdrawal_proofs_survive_restart() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(3), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let b = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000)], now).await;
        add(&mut c, b, now).await;
        let mut tip = c.tip().hash;
        for n in 0..3u64 {
            now += 1000;
            let lock = Tx::signed(&seed(2), CHAIN, n, 10, TxBody::BridgeLock { solana_recipient: [9; 32], amount: 100_000 });
            let b = mine_on(&c, tip, 9, &[lock], now).await;
            add(&mut c, b, now).await;
            let t = c.tip().hash;
            tip = extend(&mut c, t, 1, 9, &mut now).await;
        }
        tip = extend(&mut c, tip, 4, 9, &mut now).await;
        let f = c.finalized().header.height;
        let mut before = vec![];
        for nonce in 0..3u64 {
            for at in 1..=f {
                before.push((nonce, at, c.withdrawal_proof_at(nonce, at).await.map_err(|e| e.to_string())));
            }
        }
        let ok = before.iter().filter(|(_, _, r)| matches!(r, Ok(Some(_)))).count();
        let refused = before.iter().filter(|(_, _, r)| r.is_err()).count();
        assert!(ok >= 3 && refused >= 3, "{ok} ok, {refused} refused below the withdrawal block");
        // distinct roots across heights (one per withdrawal block)
        let roots: std::collections::HashSet<_> = before.iter().filter_map(|(_, _, r)| r.as_ref().ok().and_then(|o| o.as_ref()).map(|x| x.3)).collect();
        assert_eq!(roots.len(), 3);
        drop(c);
        let c2 = C::open(ctx.child("b"), "n", genesis(3), MockVerifier).await.unwrap();
        assert_eq!(c2.tip().hash, tip);
        for (nonce, at, r) in before {
            assert_eq!(c2.withdrawal_proof_at(nonce, at).await.map_err(|e| e.to_string()), r, "nonce {nonce} at {at}");
        }
    });
}

/// Finality jumps several blocks in one insert (AdminConfig lowers K from 6 to 1), then
/// blocks keep being built on top, with withdrawals before and after; historical proofs
/// at every final height; restart and extend.
#[test]
fn finality_jump_then_build_on_top() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(6), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let b = mine_on(&c, g, 9, &[unlock(200, 0, 1, pk(2), 1000), unlock(200, 1, 2, pk(1), 1000)], now).await;
        add(&mut c, b, now).await;
        let mut n2 = 0u64;
        let mut lock = |c2: &mut u64| { let t = Tx::signed(&seed(2), CHAIN, *c2, 10, TxBody::BridgeLock { solana_recipient: [9; 32], amount: 1000 }); *c2 += 1; t };
        for _ in 0..4 {
            now += 1000;
            let tip = c.tip().hash;
            let txs = vec![lock(&mut n2), lock(&mut n2)];
            let b = mine_on(&c, tip, 9, &txs, now).await;
            add(&mut c, b, now).await;
        }
        let f0 = c.finalized().header.height;
        now += 1000;
        let tip = c.tip().hash;
        let admin = Tx::signed(&seed(1), CHAIN, 0, 10, TxBody::AdminConfig { key: state::config::keys::FINALITY_K, value: 1u32.to_le_bytes().to_vec() });
        let b = mine_on(&c, tip, 9, &[admin, lock(&mut n2)], now).await;
        add(&mut c, b, now).await;
        // the next block sees K = 1 in its parent config
        let tip = c.tip().hash;
        let tip = extend(&mut c, tip, 1, 9, &mut now).await;
        let f1 = c.finalized().header.height;
        assert!(f1 >= f0 + 2, "finality jumped");
        let mut tip = tip;
        for _ in 0..4 {
            now += 1000;
            let txs = vec![lock(&mut n2)];
            let b = mine_on(&c, tip, 9, &txs, now).await;
            tip = b.hash();
            add(&mut c, b, now).await;
        }
        let f = c.finalized().header.height;
        let mut ok = 0;
        for nonce in 0..n2 {
            for at in 1..=f {
                match c.withdrawal_proof_at(nonce, at).await {
                    Ok(Some(_)) => ok += 1,
                    Ok(None) => {}
                    Err(e) => { let s = e.to_string(); assert!(s.contains("later block") || s.contains("not final"), "nonce {nonce} at {at}: {s}"); }
                }
            }
        }
        assert!(ok > 30, "{ok} historical proofs");
        drop(c);
        let mut c = C::open(ctx.child("b"), "n", genesis(6), MockVerifier).await.unwrap();
        let tip = c.tip().hash;
        extend(&mut c, tip, 3, 9, &mut now).await;
    });
}
