use agent_types::tx::{deposit_message, order_job_id, order_message, pubkey_from_seed, result_message, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, supply_of, BlockEnv, BlockExecutor};
use futures::executor::block_on;
use state::config::{keys, Config};
use state::schema::{self, Account, JobRecord, JobStatus};
use state::{MemState, StateRead};

const CHAIN: u32 = 99;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

struct Chain {
    st: MemState,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

const ADMIN: u8 = 1;
const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        cfg.emission_ppm = 100_000; // 10% of the pool per job block, so caps bind in tests
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        Self { st, height: 0, nonces: Default::default() }
    }

    fn tx(&mut self, who: u8, fee: u64, body: TxBody) -> Tx {
        let n = self.nonces.entry(who).or_insert(0);
        let t = Tx::signed(&seed(who), CHAIN, *n, fee, body);
        *n += 1;
        t
    }

    fn block(&mut self, job: Option<Hash>, txs: &[Tx]) -> Result<executor::BlockSummary, executor::ExecError> {
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(MINER), job_id: job };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs))?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }

    fn bal(&self, a: &Pubkey) -> u64 {
        block_on(self.st.get(&schema::account_key(a))).unwrap().map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }

    fn job(&self, id: &Hash) -> JobRecord {
        JobRecord::decode(&block_on(self.st.get(&schema::job_key(id))).unwrap().unwrap()).unwrap()
    }

    fn unlock(&mut self, relayer: u8, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
        let msg = deposit_message(nonce, &to, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        self.tx(relayer, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
    }

    fn fund(&mut self, who: u8, amount_spl: u64) {
        let n = 1000 + self.height * 7 + who as u64;
        let t = self.unlock(200, n, pk(who), amount_spl);
        self.block(None, &[t]).unwrap();
    }
}

fn job_body(evals: &[u8], w: u64, p: u64, deadline: u64, max_blocks: u32) -> PostJob {
    let cfg = pearl_job::MiningConfig::localnet_default();
    PostJob {
        m: 64,
        n: 64,
        k: cfg.k() as u32,
        config: cfg.to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: b"ipfs://x".to_vec(),
        worker_pay: w,
        protocol_fee: p,
        deadline_height: deadline,
        max_blocks,
        evaluators: evals.iter().map(|i| pk(*i)).collect(),
        threshold: 1,
    }
}

#[test]
fn transfer_fees_mature_after_k() {
    let mut c = Chain::new();
    c.fund(2, 1_000_000); // 1e9 base units
    assert_eq!(c.bal(&pk(2)), 1_000_000_000);
    let t = c.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: 500 });
    let s = c.block(None, &[t]).unwrap();
    assert_eq!(s.fees, 1000);
    assert_eq!(c.bal(&pk(3)), 500);
    assert_eq!(c.bal(&pk(MINER)), 0);
    let fee_height = c.height;
    while c.height < fee_height + 3 {
        c.block(None, &[]).unwrap();
    }
    assert_eq!(c.bal(&pk(MINER)), 1000);
}

#[test]
fn bad_sig_nonce_fee_reject_block() {
    let mut c = Chain::new();
    c.fund(2, 1_000_000);
    let mut t = c.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: 1 });
    t.sig[0] ^= 1;
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("signature"));
    // replayed nonce: same tx twice
    c.nonces.insert(2, 0);
    let t = c.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: 1 });
    assert!(c.block(None, &[t.clone(), t.clone()]).unwrap_err().to_string().contains("nonce"));
    c.block(None, &[t.clone()]).unwrap();
    assert!(c.block(None, &[t]).is_err());
    let t = c.tx(2, 10, TxBody::Transfer { to: pk(3), amount: 1 });
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("min_fee"));
    // wrong chain id
    let t = Tx::signed(&seed(2), CHAIN + 1, 1, 1000, TxBody::Transfer { to: pk(3), amount: 1 });
    assert!(c.block(None, &[t]).is_err());
}

#[test]
fn bridge_unlock_threshold_and_replay() {
    let mut c = Chain::new();
    let t = c.unlock(200, 7, pk(4), 5);
    c.block(None, &[t]).unwrap();
    assert_eq!(c.bal(&pk(4)), 5000);
    let t = c.unlock(200, 7, pk(4), 5);
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("already processed"));
    // one signature is below the threshold of 2
    let msg = deposit_message(8, &pk(4), 5);
    let sigs = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
    let t = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 8, recipient: pk(4), amount_spl: 5, job_order: None, sigs });
    assert!(c.block(None, &[t]).is_err());
    // a non-attester signature fails
    let sigs = [50u8, 77].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
    let t = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 8, recipient: pk(4), amount_spl: 5, job_order: None, sigs });
    assert!(c.block(None, &[t]).is_err());
}

#[test]
fn bridge_lock_records_withdrawal() {
    let mut c = Chain::new();
    c.fund(2, 1_000_000);
    let t = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: 1234 });
    assert!(c.block(None, &[t]).is_err());
    c.nonces.insert(2, 0);
    let esc = c.bal(&escrow_address());
    let t = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: 2_000_000 });
    let s = c.block(None, &[t]).unwrap();
    assert_eq!(s.withdrawals.len(), 1);
    assert_eq!(s.withdrawals[0].amount_spl, 2000);
    assert_eq!(s.withdrawals[0].nonce, 0);
    assert_eq!(c.bal(&escrow_address()), esc + 2_000_000);
}

#[test]
fn job_lifecycle_and_alpha_cap() {
    let mut c = Chain::new();
    c.fund(2, 100_000_000); // poster: 1e11
    // seed the pool so subsidy is limited by the job cap, not the pool
    let t = c.unlock(200, 1, pool_address(), 1_000_000_000);
    c.block(None, &[t]).unwrap();
    let w = 5_000_000_000u64;
    let p = 1_000_000_000u64;
    let body = job_body(&[60], w, p, 100, 1000);
    let post_nonce = c.nonces.get(&2).copied().unwrap_or(0);
    let t = c.tx(2, 1000, TxBody::PostJob(body));
    let s = c.block(None, &[t]).unwrap();
    let id = s.jobs_created[0];
    assert_eq!(id, schema::job_id_for_post(&pk(2), post_nonce));
    let j = c.job(&id);
    assert_eq!(j.cap_remaining, p * 8 / 10);
    // mine job blocks until the cap is used up
    let mut total = 0;
    for _ in 0..200 {
        match c.block(Some(id), &[]) {
            Ok(s) => total += s.subsidy,
            Err(_) => break,
        }
    }
    assert_eq!(total, p * 8 / 10, "subsidy over the job lifetime is exactly alpha x P");
    assert!(c.block(Some(id), &[]).unwrap_err().to_string().contains("cap"));
    // deliver and complete
    let rh = [0x33; 32];
    let t = c.tx(3, 0, TxBody::SubmitResult { job_id: id, result_hash: rh, uri: vec![] });
    assert!(c.block(None, &[t]).is_err(), "deliverer without balance cannot pay min_fee");
    c.nonces.insert(3, 0);
    c.fund(3, 1000);
    let t = c.tx(3, 1000, TxBody::SubmitResult { job_id: id, result_hash: rh, uri: b"ipfs://r".to_vec() });
    c.block(None, &[t]).unwrap();
    let msg = result_message(&id, &pk(3), &rh);
    let bad = AttesterSig { pubkey: pk(61), sig: sign_plain(&seed(61), &msg) };
    let t = c.tx(3, 1000, TxBody::CompleteJob { job_id: id, sigs: vec![bad] });
    assert!(c.block(None, &[t]).is_err());
    c.nonces.insert(3, 1);
    let good = AttesterSig { pubkey: pk(60), sig: sign_plain(&seed(60), &msg) };
    let before = c.bal(&pk(3));
    let t = c.tx(3, 1000, TxBody::CompleteJob { job_id: id, sigs: vec![good] });
    c.block(None, &[t]).unwrap();
    assert_eq!(c.bal(&pk(3)), before + w - 1000);
    assert_eq!(c.job(&id).status, JobStatus::Completed);
    assert!(c.block(Some(id), &[]).is_err(), "completed jobs are not mineable");
}

#[test]
fn order_unlock_posts_job_with_relative_deadline() {
    let mut c = Chain::new();
    let w = 4_000_000u64;
    let p = 1_000_000u64;
    let body = job_body(&[60], w, p, 50, 5);
    let amount_spl = (w + p) / SPL_SCALE;
    let msg = order_message(77, &pk(5), amount_spl, &body);
    let sigs = ATT[1..].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
    let t = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 77, recipient: pk(5), amount_spl, job_order: Some(body.clone()), sigs });
    let s = c.block(None, &[t]).unwrap();
    let id = order_job_id(77);
    assert_eq!(s.jobs_created, vec![id]);
    let j = c.job(&id);
    assert_eq!(j.poster, pk(5));
    assert_eq!(j.deadline_height, c.height + 50);
    assert_eq!(c.bal(&pk(5)), 0);
    // order header: recipient || order terms (no W, P, job id or unlock height), so the
    // buyer can commit hash_a / hash_b before the order is filled
    assert_eq!(j.header_id, pk(5));
    assert_eq!(j.terms_hash, schema::order_terms_hash(&pk(5), j.m, j.n, j.k, j.max_blocks, 50));
    assert_eq!(j.pearl_header().job_pubkey, pk(5));
    // a plain-deposit signature does not authorise an order unlock
    let dmsg = deposit_message(78, &pk(5), amount_spl);
    let sigs = ATT[1..].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &dmsg) }).collect();
    let t = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 78, recipient: pk(5), amount_spl, job_order: Some(body), sigs });
    assert!(c.block(None, &[t]).is_err());
    c.nonces.insert(200, 1);
    // expire: W goes back to the recipient
    c.fund(6, 1000);
    while c.height <= j.deadline_height {
        c.block(None, &[]).unwrap();
    }
    let t = c.tx(6, 1000, TxBody::ExpireJob { job_id: id });
    c.block(None, &[t]).unwrap();
    assert_eq!(c.bal(&pk(5)), w);
    assert_eq!(c.job(&id).status, JobStatus::Expired);
}

#[test]
fn admin_config_only_admin() {
    let mut c = Chain::new();
    c.fund(2, 1000);
    c.fund(ADMIN, 1000);
    let t = c.tx(2, 1000, TxBody::AdminConfig { key: keys::MIN_FEE, value: 5u64.to_le_bytes().to_vec() });
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("admin"));
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::ALPHA_PPM, value: 1_000_000u64.to_le_bytes().to_vec() });
    assert!(c.block(None, &[t]).is_err(), "alpha must stay below 1");
    c.nonces.insert(ADMIN, 0);
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::MIN_FEE, value: 5u64.to_le_bytes().to_vec() });
    c.block(None, &[t]).unwrap();
    c.nonces.insert(2, 0);
    let t = c.tx(2, 5, TxBody::Transfer { to: pk(3), amount: 1 });
    c.block(None, &[t]).unwrap();
}

#[test]
fn template_builder_skips_failing_txs() {
    let mut c = Chain::new();
    c.fund(2, 1000);
    let good = c.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: 10 });
    let bad = c.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: u64::MAX });
    let env = BlockEnv { chain_id: CHAIN, height: c.height + 1, miner: pk(MINER), job_id: None };
    let (ws, s) = block_on(async {
        let mut ex = BlockExecutor::begin(&c.st, env).await.unwrap();
        ex.apply_tx(&good).await.unwrap();
        assert!(ex.apply_tx(&bad).await.is_err());
        ex.finish(true).await.unwrap()
    });
    assert_eq!(s.tx_count, 1);
    c.st.apply(&ws);
    assert_eq!(supply_of(&c.st.values()).unwrap(), TOTAL_SUPPLY as u128);
}

/// Pseudo-random mix of every tx kind; the invariant is checked after each block.
#[test]
fn randomized_supply_invariant() {
    let mut c = Chain::new();
    for i in 2..8u8 {
        c.fund(i, 50_000_000);
    }
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let mut jobs: Vec<Hash> = Vec::new();
    for _ in 0..60 {
        let mut txs = Vec::new();
        for _ in 0..4 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let who = 2 + (x % 6) as u8;
            let to = 2 + ((x >> 8) % 6) as u8;
            let body = match (x >> 16) % 5 {
                0 => TxBody::Transfer { to: pk(to), amount: (x >> 24) % 1_000_000 },
                1 => TxBody::BridgeLock { solana_recipient: [1; 32], amount: ((x >> 24) % 1000) * 1000 + 1000 },
                2 => TxBody::PostJob(job_body(&[to], (x >> 30) % 10_000_000, 1_000_000, c.height + 20, 3)),
                3 if !jobs.is_empty() => TxBody::ExpireJob { job_id: jobs[(x as usize >> 3) % jobs.len()] },
                _ => TxBody::Transfer { to: pool_address(), amount: 1000 },
            };
            txs.push(c.tx(who, 1000, body));
        }
        // Build the block tx by tx (skipping failures) so nonces stay consistent.
        let env = BlockEnv { chain_id: CHAIN, height: c.height + 1, miner: pk(MINER), job_id: jobs.first().copied() };
        let res = block_on(async {
            let mut ex = match BlockExecutor::begin(&c.st, env).await {
                Ok(ex) => ex,
                Err(_) => BlockExecutor::begin(&c.st, BlockEnv { job_id: None, ..env }).await.unwrap(),
            };
            let mut ok = Vec::new();
            for t in &txs {
                if ex.apply_tx(t).await.is_ok() {
                    ok.push(t.clone());
                }
            }
            (ok, ex.finish(true).await.unwrap())
        });
        let (ok, (ws, s)) = res;
        jobs.extend(s.jobs_created.iter().copied());
        // nonces of failed txs were not consumed: resync
        for t in &txs {
            if !ok.contains(t) {
                let who = (2..8u8).find(|i| pk(*i) == t.from).unwrap();
                let n = c.nonces.get_mut(&who).unwrap();
                *n = (*n).min(t.nonce);
            }
        }
        c.st.apply(&ws);
        c.height += 1;
        assert_eq!(supply_of(&c.st.values()).unwrap(), TOTAL_SUPPLY as u128);
    }
}

#[test]
fn min_protocol_fee_rejects_post_job_and_order_unlock() {
    let mut c = Chain::new();
    c.fund(2, 100_000);
    c.fund(ADMIN, 1000);
    // alpha = 0.8: P = 1 rounds the cap to 0, so it is refused even with min_protocol_fee 0
    let t = c.tx(2, 1000, TxBody::PostJob(job_body(&[60], 1000, 1, 100, 5)));
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("alpha x P rounds to 0"));
    c.nonces.insert(2, 0);
    let t = c.tx(2, 1000, TxBody::PostJob(job_body(&[60], 1000, 2, 100, 5)));
    c.block(None, &[t]).unwrap();
    // the admin raises the floor; P below it is refused, P at it is accepted
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::MIN_PROTOCOL_FEE, value: 1_000_000u64.to_le_bytes().to_vec() });
    c.block(None, &[t]).unwrap();
    let t = c.tx(2, 1000, TxBody::PostJob(job_body(&[60], 1000, 999_999, 100, 5)));
    let e = c.block(None, &[t]).unwrap_err().to_string();
    assert!(e.contains("below min_protocol_fee 1000000"), "{e}");
    c.nonces.insert(2, 1);
    let t = c.tx(2, 1000, TxBody::PostJob(job_body(&[60], 1000, 1_000_000, 100, 5)));
    c.block(None, &[t]).unwrap();
    // an order unlock whose P is below the floor fails as a whole: no unlock, no job
    let body = job_body(&[60], 4_000_000, 999_000, 50, 5);
    let amount_spl = (4_000_000 + 999_000) / SPL_SCALE;
    let msg = order_message(90, &pk(5), amount_spl, &body);
    let sigs = ATT[1..].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
    let t = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 90, recipient: pk(5), amount_spl, job_order: Some(body.clone()), sigs });
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("below min_protocol_fee"));
    assert_eq!(c.bal(&pk(5)), 0);
    // stateless precheck (sendTx) agrees
    let cfg = block_on(async { executor::Overlay::new(&c.st).config().await.unwrap() });
    assert_eq!(executor::effective_min_protocol_fee(&cfg), 1_000_000);
    assert!(executor::precheck_post_job(&body, &cfg).is_err());
    assert!(executor::precheck_post_job(&job_body(&[60], 1, 1_000_000, 50, 5), &cfg).is_ok());
}

#[test]
fn blob_uris_must_be_well_formed() {
    let mut c = Chain::new();
    c.fund(2, 100_000);
    let mut b = job_body(&[60], 1000, 1_000_000, 100, 5);
    b.data_uri = b"blob:1234".to_vec();
    let t = c.tx(2, 1000, TxBody::PostJob(b.clone()));
    assert!(c.block(None, &[t]).unwrap_err().to_string().contains("malformed blob URI"));
    c.nonces.insert(2, 0);
    let id = blobs::blob_id(b"job data");
    b.data_uri = blobs::blob_uri(&id).into_bytes();
    let t = c.tx(2, 1000, TxBody::PostJob(b));
    let s = c.block(None, &[t]).unwrap();
    let job = s.jobs_created[0];
    assert_eq!(blobs::parse_uri(&c.job(&job).data_uri), Some(id));
    // results: malformed blob uri refused, well-formed accepted
    let t = c.tx(2, 1000, TxBody::SubmitResult { job_id: job, result_hash: [3; 32], uri: b"blob:xyz".to_vec() });
    assert!(c.block(None, &[t]).is_err());
    c.nonces.insert(2, 1);
    let r = blobs::blob_uri(&blobs::blob_id(b"C")).into_bytes();
    let t = c.tx(2, 1000, TxBody::SubmitResult { job_id: job, result_hash: [3; 32], uri: r.clone() });
    c.block(None, &[t]).unwrap();
    assert_eq!(c.job(&job).result_uri, r);
}
