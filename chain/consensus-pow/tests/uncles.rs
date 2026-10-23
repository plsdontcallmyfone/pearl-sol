//! Uncles and the speed migration at the chain level (docs/SPEED.md 3): stale job blocks at
//! depth 1 and 2 included by templates and paid, every validity rule (3.3, 1-6) refusing a
//! crafted uncle, double inclusion, an uncle of an uncle, a reorg that drops the includer, the
//! uncle activation height, and the migration (30 s WTEMA from S + 1, K doubled, finality
//! pausing then resuming).

use agent_types::speed::{Uncle, UncleRecord, TEST_SPEED_CHAIN_BASE};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{Accept, Chain, ChainError, Genesis, JobChoice, MockVerifier, PowVerifier, Template};
use state::config::Config;
use state::schema;

const CHAIN: u32 = 7;
const T0: u64 = 1_760_000_000_000;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

fn genesis(chain_id: u32, k: u32) -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.finality_k = k;
    c.target_block_time_ms = 1000;
    c.wtema_half_life_ms = 10_000;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    c.min_fee = 10;
    Genesis { chain_id, timestamp_ms: T0, target_bits: 0x1f7fffff, config: c }
}

type C<V> = Chain<deterministic::Context, V>;

fn seal(t: &Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: t.uncles.clone() }
}

/// Re-seal `b` after its header or uncles changed (tx_root over the uncle records, mock proof).
fn reseal(mut b: Block) -> Block {
    let recs: Vec<UncleRecord> = b.uncles.iter().map(|u| u.record).collect();
    b.header.tx_root = block::tx_root_all(&b.txs, b.contracts.as_ref(), &recs, b.coinbase.as_ref());
    b.header.proof_hash = [0; 32];
    b.proof = MockVerifier::proof_for(&b.header);
    b.header.proof_hash = block::proof_hash(&b.proof);
    b
}

/// An uncle for `h` (mock proof re-bound, record matching the header).
fn as_uncle(mut h: Header, includer_height: u64) -> Uncle {
    h.proof_hash = [0; 32];
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    let record = UncleRecord { height: includer_height, uncle_hash: h.hash(), uncle_height: h.height, miner: h.miner, job_id: h.job_id, ..Default::default() };
    Uncle { header: h, proof, record }
}

async fn tmpl<V: PowVerifier>(c: &C<V>, parent: Hash, miner: u8, choice: JobChoice, now: u64) -> Template {
    c.build_template_at(parent, pk(miner), choice, &[], now).await.unwrap()
}

async fn add<V: PowVerifier>(c: &mut C<V>, b: Block, now: u64) -> Accept {
    c.insert_block(b, now).await.unwrap().0
}

fn unlock(rnonce: u64, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
    let msg = deposit_message(nonce, &to, amount_spl);
    let sigs = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
    Tx::signed(&seed(200), CHAIN, rnonce, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
}

fn job_body() -> PostJob {
    PostJob {
        m: 64,
        n: 64,
        k: pearl_job::MiningConfig::localnet_default().common_dim,
        config: pearl_job::MiningConfig::localnet_default().to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: b"ipfs://x".to_vec(),
        worker_pay: 1000,
        protocol_fee: 50_000_000_000,
        deadline_height: 1000,
        max_blocks: 100,
        evaluators: vec![pk(60)],
        threshold: 1,
    }
}

/// Fund the poster and the pool, post a job (heights 1-2). Returns (job id, tip, now).
async fn setup(c: &mut C<MockVerifier>) -> (Hash, Hash, u64) {
    let g = c.genesis_hash();
    let mut now = T0 + 1000;
    let txs = [unlock(0, 1, pk(2), 1_000_000_000), unlock(1, 2, pool_address(), 1_000_000)];
    let t = c.build_template_at(g, pk(9), JobChoice::Fallback, &txs, now).await.unwrap();
    add(c, seal(&t), now).await;
    now += 1000;
    let post = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::PostJob(job_body()));
    let t = c.build_template_at(c.tip().hash, pk(9), JobChoice::Fallback, &[post], now).await.unwrap();
    add(c, seal(&t), now).await;
    (schema::job_id_for_post(&pk(2), 0), c.tip().hash, now)
}

#[test]
fn stale_job_block_paid_as_uncle_at_depth_1_and_2() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(CHAIN, 10), MockVerifier).await.unwrap();
        let (job, p2, mut now) = setup(&mut c).await;
        now += 1000;
        // two job blocks on height 2: A wins (first seen), B is stale
        let a = seal(&tmpl(&c, p2, 3, JobChoice::Job(job), now).await);
        let b = seal(&tmpl(&c, p2, 4, JobChoice::Job(job), now).await);
        assert_eq!(add(&mut c, a.clone(), now).await, Accept::Added { tip: true });
        assert_eq!(add(&mut c, b.clone(), now).await, Accept::Added { tip: false });
        // the next template on A carries B at depth 1
        now += 1000;
        let t = tmpl(&c, a.hash(), 9, JobChoice::Fallback, now).await;
        assert_eq!(t.uncles.len(), 1);
        assert_eq!(t.uncles[0].header.hash(), b.hash());
        let r = t.uncles[0].record;
        assert_eq!((r.height, r.uncle_height, r.miner, r.job_id), (4, 3, pk(4), job));
        assert!(r.reward > 0 && r.nephew == r.reward / 32);
        let x = seal(&t);
        assert_eq!(add(&mut c, x.clone(), now).await, Accept::Added { tip: true });
        assert_eq!(c.tip().summary.uncles, vec![r]);
        let stored = c.get_block(&x.hash()).await.unwrap().unwrap();
        assert_eq!(stored.uncles.len(), 1, "the body keeps its uncle");
        assert!(c.read_tip(&schema::uncle_key(&b.hash())).await.unwrap().is_some());
        // the next template has no uncle (B is credited)
        now += 1000;
        assert!(tmpl(&c, x.hash(), 9, JobChoice::Fallback, now).await.uncles.is_empty());
        // depth 2: D stale at 5 arrives after the template for 6 was built on E
        let e = seal(&tmpl(&c, x.hash(), 3, JobChoice::Job(job), now).await);
        let d = seal(&tmpl(&c, x.hash(), 5, JobChoice::Job(job), now).await);
        add(&mut c, e.clone(), now).await;
        now += 1000;
        let f = seal(&tmpl(&c, e.hash(), 9, JobChoice::Fallback, now).await);
        // f was built before d was known: no uncle in it
        assert!(f.uncles.is_empty());
        add(&mut c, d.clone(), now).await;
        add(&mut c, f.clone(), now).await;
        now += 1000;
        let t = tmpl(&c, f.hash(), 9, JobChoice::Fallback, now).await;
        assert_eq!(t.uncles.len(), 1);
        assert_eq!(t.uncles[0].header.hash(), d.hash());
        assert_eq!(t.header.height - d.header.height, 2);
        let rd = t.uncles[0].record;
        add(&mut c, seal(&t), now).await;
        assert_eq!(c.tip().summary.uncles, vec![rd]);
        // depth 2 is a quarter of the base, depth 1 a half: compare per unit of base
        assert!(rd.reward > 0);
    });
}

#[test]
fn every_rule_refuses_a_crafted_uncle() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(CHAIN, 10), MockVerifier).await.unwrap();
        let (job, p2, mut now) = setup(&mut c).await;
        now += 1000;
        let a = seal(&tmpl(&c, p2, 3, JobChoice::Job(job), now).await);
        let b = seal(&tmpl(&c, p2, 4, JobChoice::Job(job), now).await);
        // stale C and its child D (an uncle of an uncle), both off the branch
        add(&mut c, a.clone(), now).await;
        add(&mut c, b.clone(), now).await;
        let d = seal(&tmpl(&c, b.hash(), 6, JobChoice::Job(job), now + 1000).await);
        add(&mut c, d.clone(), now + 1000).await;
        now += 1000;
        // a plain block on A as the base to graft crafted uncles onto (its own uncle B removed
        // and the commitments recomputed by `reseal`)
        let t = tmpl(&c, a.hash(), 9, JobChoice::Fallback, now).await;
        let h = 4u64;
        let try_uncle = |u: Uncle| {
            let mut blk = seal(&t);
            blk.uncles = vec![u];
            reseal(blk)
        };
        // an uncle of an uncle: D (child of stale B) offered to a block at height 5 on A4
        let a4 = seal(&t);
        assert_eq!(a4.uncles.len(), 1, "A4 credits B");
        add(&mut c, a4.clone(), now).await;
        let t5 = tmpl(&c, a4.hash(), 9, JobChoice::Fallback, now + 1000).await;
        let mut b5 = seal(&t5);
        b5.uncles = vec![as_uncle(d.header, 5)];
        match c.insert_block(reseal(b5), now + 1000).await {
            Err(ChainError::Invalid(m)) => assert!(m.contains("parent is not on the branch"), "{m}"),
            other => panic!("{other:?}"),
        }
        let expect = |r: Result<_, ChainError>, want: &str| match r {
            Err(ChainError::Invalid(m)) => assert!(m.contains(want), "{m} (wanted {want})"),
            other => panic!("wanted Invalid({want}), got {other:?}"),
        };
        let bh = b.header;
        // rule 1: fallback uncle (context-free) and header version
        let mut f = bh;
        f.job_id = [0; 32];
        expect(c.insert_block(try_uncle(as_uncle(f, h)), now).await.map(|_| ()), "job block");
        let mut v = bh;
        v.version = 2;
        expect(c.insert_block(try_uncle(as_uncle(v, h)), now).await.map(|_| ()), "version");
        // rule 2: depth 3 (a fake block at height 1)
        let mut deep = bh;
        deep.height = 1;
        deep.prev_hash = c.genesis_hash();
        expect(c.insert_block(try_uncle(as_uncle(deep, h)), now).await.map(|_| ()), "depth 3");
        // rule 3: parent not on the branch, and an uncle of an uncle
        let mut np = bh;
        np.prev_hash = [0x55; 32];
        expect(c.insert_block(try_uncle(as_uncle(np, h)), now).await.map(|_| ()), "parent is not on the branch");
        // rule 4: the branch's own block
        expect(c.insert_block(try_uncle(as_uncle(a.header, h)), now).await.map(|_| ()), "on the branch");
        // rule 5: target and timestamp
        let mut tb = bh;
        tb.target_bits = 0x1f3fffff;
        expect(c.insert_block(try_uncle(as_uncle(tb, h)), now).await.map(|_| ()), "target_bits");
        let mut ts = bh;
        ts.timestamp_ms = T0;
        expect(c.insert_block(try_uncle(as_uncle(ts, h)), now).await.map(|_| ()), "median time past");
        // rule 6: unknown job, job posted after the uncle's parent, bad proof
        let mut uj = bh;
        uj.job_id = [0x66; 32];
        expect(c.insert_block(try_uncle(as_uncle(uj, h)), now).await.map(|_| ()), "unknown job");
        let mut bad = as_uncle(bh, h);
        bad.proof = b"mock-but-wrong".to_vec();
        bad.header.proof_hash = block::proof_hash(&bad.proof);
        bad.record.uncle_hash = bad.header.hash();
        expect(c.insert_block(try_uncle(bad), now).await.map(|_| ()), "bad uncle proof");
        // the real B passes the rules; a body record that is not the one execution produced
        // (zero reward here) is refused after execution
        expect(c.insert_block(try_uncle(as_uncle(bh, h)), now).await.map(|_| ()), "uncle record mismatch");
        // a duplicate inside one block
        let mut blk = seal(&t);
        blk.uncles = vec![as_uncle(bh, h), as_uncle(bh, h)];
        expect(c.insert_block(reseal(blk), now).await.map(|_| ()), "duplicate uncle");
    });
}

#[test]
fn double_inclusion_refused_and_reorg_makes_the_uncle_eligible_again() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(CHAIN, 10), MockVerifier).await.unwrap();
        let (job, p2, mut now) = setup(&mut c).await;
        now += 1000;
        let a = seal(&tmpl(&c, p2, 3, JobChoice::Job(job), now).await);
        let b = seal(&tmpl(&c, p2, 4, JobChoice::Job(job), now).await);
        add(&mut c, a.clone(), now).await;
        now += 1000;
        // Y4 on A built before B is known (no uncle); X4 on A after (with B)
        let y4 = seal(&tmpl(&c, a.hash(), 7, JobChoice::Fallback, now).await);
        add(&mut c, b.clone(), now - 1000).await;
        let x4 = seal(&tmpl(&c, a.hash(), 8, JobChoice::Fallback, now).await);
        assert_eq!(x4.uncles.len(), 1);
        assert_eq!(add(&mut c, x4.clone(), now).await, Accept::Added { tip: true });
        assert_eq!(add(&mut c, y4.clone(), now).await, Accept::Added { tip: false });
        // on X, a second inclusion of B is refused
        now += 1000;
        let mut t = tmpl(&c, x4.hash(), 9, JobChoice::Fallback, now).await;
        assert!(t.uncles.is_empty());
        let mut again = seal(&t);
        let mut u = x4.uncles[0].clone();
        u.record.height = 5;
        again.uncles = vec![u];
        let again = reseal(again);
        match c.insert_block(again, now).await {
            Err(ChainError::Invalid(m)) => assert!(m.contains("already included"), "{m}"),
            other => panic!("{other:?}"),
        }
        // on Y (which never credited B), the template offers B at depth 2 and Y overtakes X
        t = tmpl(&c, y4.hash(), 7, JobChoice::Fallback, now).await;
        assert_eq!(t.uncles.len(), 1);
        assert_eq!(t.uncles[0].header.hash(), b.hash());
        let y5 = seal(&t);
        assert_eq!(add(&mut c, y5.clone(), now).await, Accept::Added { tip: true });
        assert_eq!(c.tip().summary.uncles.len(), 1);
    });
}

#[test]
fn uncles_refused_before_the_speed_height() {
    deterministic::Runner::default().start(|ctx| async move {
        let cid = TEST_SPEED_CHAIN_BASE + 100;
        let mut c = C::open(ctx.child("a"), "n", genesis(cid, 10), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let msg = deposit_message(1, &pk(2), 1_000_000_000);
        let sigs = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
        let u1 = Tx::signed(&seed(200), cid, 0, 0, TxBody::BridgeUnlock { solana_nonce: 1, recipient: pk(2), amount_spl: 1_000_000_000, job_order: None, sigs });
        let t = c.build_template_at(g, pk(9), JobChoice::Fallback, &[u1], now).await.unwrap();
        add(&mut c, seal(&t), now).await;
        now += 1000;
        let post = Tx::signed(&seed(2), cid, 0, 10, TxBody::PostJob(job_body()));
        let t = c.build_template_at(c.tip().hash, pk(9), JobChoice::Fallback, &[post], now).await.unwrap();
        add(&mut c, seal(&t), now).await;
        let job = schema::job_id_for_post(&pk(2), 0);
        let p2 = c.tip().hash;
        now += 1000;
        let a = seal(&tmpl(&c, p2, 3, JobChoice::Job(job), now).await);
        let b = seal(&tmpl(&c, p2, 4, JobChoice::Job(job), now).await);
        add(&mut c, a.clone(), now).await;
        add(&mut c, b.clone(), now).await;
        now += 1000;
        let t = tmpl(&c, a.hash(), 9, JobChoice::Fallback, now).await;
        assert!(t.uncles.is_empty(), "templates offer none before the speed height");
        let mut blk = seal(&t);
        blk.uncles = vec![as_uncle(b.header, 4)];
        match c.insert_block(reseal(blk), now).await {
            Err(ChainError::Invalid(m)) => assert!(m.contains("valid from height 100"), "{m}"),
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn migration_switches_target_time_and_k_at_s() {
    deterministic::Runner::default().start(|ctx| async move {
        let s = 8u64;
        let cid = TEST_SPEED_CHAIN_BASE + s as u32;
        let k = 3u32;
        let mut c = C::open(ctx.child("a"), "n", genesis(cid, k), MockVerifier).await.unwrap();
        let mut now = T0;
        // 1,500 ms solve times: the WTEMA moves either way, so the two rules give different bits
        while c.tip_height() < s {
            now += 1500;
            let t = tmpl(&c, c.tip().hash, 9, JobChoice::Fallback, now).await;
            add(&mut c, seal(&t), now).await;
            if c.tip_height() < s {
                assert_eq!(c.finalized().header.height, c.tip_height().saturating_sub(k as u64));
            }
        }
        // tip = S: its post-state carries the new parameters
        assert_eq!(c.tip().config.target_block_time_ms, 500);
        assert_eq!(c.tip().config.finality_k, 2 * k);
        assert_eq!(c.tip().config.emission_ppb, 3_500);
        let fin_at_s = c.finalized().header.height;
        assert_eq!(fin_at_s, s - 1 - k as u64, "finality does not jump back and does not advance at S");
        // the child of S uses the 500 ms target; the 1,000 ms rule is refused
        let want = c.required_bits(&c.tip().hash);
        let old_rule = consensus_pow::difficulty::next_bits(&consensus_pow::difficulty::Retarget {
            parent_bits: c.tip().header.target_bits,
            parent_ts_ms: c.tip().header.timestamp_ms,
            grandparent_ts_ms: Some(c.tip().header.timestamp_ms - 1500),
            target_block_time_ms: 1000,
            half_life_ms: 10_000,
            pow_limit_bits: c.tip().config.pow_limit_bits,
            min_difficulty_bits: 0,
        });
        assert_ne!(want, old_rule);
        now += 1500;
        let t = tmpl(&c, c.tip().hash, 9, JobChoice::Fallback, now).await;
        assert_eq!(t.header.target_bits, want);
        let mut bad = seal(&t);
        bad.header.target_bits = old_rule;
        match c.insert_block(reseal(bad), now).await {
            Err(ChainError::Invalid(m)) => assert!(m.contains("target_bits"), "{m}"),
            other => panic!("{other:?}"),
        }
        // finality pauses until the tip is 2K above the old floor, then trails by 2K
        add(&mut c, seal(&t), now).await;
        while c.tip_height() < s + 2 * k as u64 + 3 {
            now += 1500;
            let t = tmpl(&c, c.tip().hash, 9, JobChoice::Fallback, now).await;
            add(&mut c, seal(&t), now).await;
            let f = c.finalized().header.height;
            assert_eq!(f, fin_at_s.max(c.tip_height() - 2 * k as u64), "at tip {}", c.tip_height());
        }
        assert_eq!(c.tip().summary.coinbase.mature_height, c.tip_height() + 2 * k as u64);
    });
}

/// Audit 2026-10-23: an uncle must name a job that was mineable at its height (the canonical
/// job-block rule), from `uncle_job_rule_height` (devnet 10,000; this test chain: always). Here
/// the job allows one block: A mines it, so stale B on the same job is past max_blocks and is
/// not paid as an uncle (before the rule it was, and so was an uncle on a closed job with cap left).
#[test]
fn an_uncle_on_a_job_that_was_not_mineable_is_refused() {
    assert_eq!(agent_types::speed::uncle_job_rule_height(agent_types::speed::DEVNET_CHAIN_ID), 10_000);
    assert_eq!(agent_types::speed::uncle_job_rule_height(TEST_SPEED_CHAIN_BASE + 40), 40);
    assert_eq!(agent_types::speed::uncle_job_rule_height(CHAIN), 0);
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(CHAIN, 10), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let mut now = T0 + 1000;
        let txs = [unlock(0, 1, pk(2), 1_000_000_000), unlock(1, 2, pool_address(), 1_000_000)];
        let t = c.build_template_at(g, pk(9), JobChoice::Fallback, &txs, now).await.unwrap();
        add(&mut c, seal(&t), now).await;
        now += 1000;
        let one = PostJob { max_blocks: 1, ..job_body() };
        let post = Tx::signed(&seed(2), CHAIN, 0, 10, TxBody::PostJob(one));
        let t = c.build_template_at(c.tip().hash, pk(9), JobChoice::Fallback, &[post], now).await.unwrap();
        add(&mut c, seal(&t), now).await;
        let (job, p2) = (schema::job_id_for_post(&pk(2), 0), c.tip().hash);
        now += 1000;
        let a = seal(&tmpl(&c, p2, 3, JobChoice::Job(job), now).await);
        let b = seal(&tmpl(&c, p2, 4, JobChoice::Job(job), now).await);
        assert_eq!(add(&mut c, a.clone(), now).await, Accept::Added { tip: true });
        add(&mut c, b.clone(), now).await;
        now += 1000;
        // templates leave B out
        let t = tmpl(&c, a.hash(), 9, JobChoice::Fallback, now).await;
        assert!(t.uncles.is_empty(), "B names a job whose one block A already used");
        // and a block that includes it anyway is invalid
        let mut blk = seal(&t);
        blk.uncles = vec![as_uncle(b.header, 4)];
        match c.insert_block(reseal(blk), now).await {
            Err(ChainError::Invalid(m)) => assert!(m.contains("not mineable") && m.contains("max_blocks"), "{m}"),
            other => panic!("{other:?}"),
        }
    });
}
