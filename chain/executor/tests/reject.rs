//! RejectResult (SPEC 4.1): evaluators reject a delivery, the job reopens, the failure
//! is recorded against the deliverer, and a rejected deliverer cannot redeliver.

use agent_types::tx::{deposit_message, pubkey_from_seed, reject_message, result_message, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, supply_of, BlockEnv};
use futures::executor::block_on;
use state::config::Config;
use state::reject::{self, Reject, RejectStats};
use state::schema::{self, Account, JobRecord, JobStatus};
use state::{MemState, StateRead};

const CHAIN: u32 = 99;
const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;

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

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(1));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        Self { st, height: 0, nonces: Default::default() }
    }

    /// Run one tx in its own block; on failure the sender's nonce is rolled back.
    fn run(&mut self, who: u8, body: TxBody) -> Result<executor::BlockSummary, String> {
        let n = *self.nonces.get(&who).unwrap_or(&0);
        let fee = if matches!(body, TxBody::BridgeUnlock { .. }) { 0 } else { 1000 };
        let t = Tx::signed(&seed(who), CHAIN, n, fee, body);
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(MINER), job_id: None };
        let (ws, sum) = block_on(execute_block(&self.st, env, &[t])).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        self.nonces.insert(who, n + 1);
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }

    fn fund(&mut self, who: u8, amount_spl: u64) {
        let nonce = 1000 + self.height * 7 + who as u64;
        let msg = deposit_message(nonce, &pk(who), amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        self.run(200, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: pk(who), amount_spl, job_order: None, sigs }).unwrap();
    }

    fn get(&self, k: &schema::Key) -> Option<Vec<u8>> {
        block_on(self.st.get(k)).unwrap()
    }
    fn job(&self, id: &Hash) -> JobRecord {
        JobRecord::decode(&self.get(&schema::job_key(id)).unwrap()).unwrap()
    }
    fn bal(&self, a: &Pubkey) -> u64 {
        self.get(&schema::account_key(a)).map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }
    fn rejects(&self, id: &Hash) -> Vec<Reject> {
        let n = self.get(&reject::reject_count_key(id)).map(|b| reject::decode_count(&b).unwrap()).unwrap_or(0);
        (0..n).map(|i| Reject::decode(&self.get(&reject::reject_key(id, i)).unwrap()).unwrap()).collect()
    }
    fn stats(&self, a: &Pubkey) -> RejectStats {
        self.get(&reject::reject_stats_key(a)).map(|b| RejectStats::decode(&b).unwrap()).unwrap_or_default()
    }
}

fn job_body(evals: &[u8], threshold: u8, w: u64, p: u64, deadline: u64) -> PostJob {
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
        max_blocks: 10,
        evaluators: evals.iter().map(|i| pk(*i)).collect(),
        threshold,
    }
}

fn rsig(i: u8, msg: &[u8]) -> AttesterSig {
    AttesterSig { pubkey: pk(i), sig: sign_plain(&seed(i), msg) }
}

const W: u64 = 5_000_000_000;
const P: u64 = 1_000_000_000;

fn setup(evals: &[u8], threshold: u8) -> (Chain, Hash) {
    let mut c = Chain::new();
    for who in [2u8, 3, 4, 70] {
        c.fund(who, 100_000_000);
    }
    let s = c.run(2, TxBody::PostJob(job_body(evals, threshold, W, P, 200))).unwrap();
    (c, s.jobs_created[0])
}

#[test]
fn reject_reopens_and_records_against_the_deliverer() {
    let (mut c, id) = setup(&[60, 61], 1);
    let bad_rh = [0x66; 32];
    c.run(3, TxBody::SubmitResult { job_id: id, result_hash: bad_rh, uri: b"ipfs://garbage".to_vec() }).unwrap();
    assert_eq!(c.job(&id).status, JobStatus::Delivered);

    let msg = reject_message(&id, &pk(3), &bad_rh);
    // a CompleteJob signature (result domain) does not serve as a reject
    let wrong_domain = rsig(60, &result_message(&id, &pk(3), &bad_rh));
    let e = c
        .run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: bad_rh, sigs: vec![wrong_domain] })
        .unwrap_err();
    assert!(e.contains("bad signature"), "{e}");
    // an outsider's signature fails
    let e = c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: bad_rh, sigs: vec![rsig(77, &msg)] }).unwrap_err();
    assert!(e.contains("not in the set"), "{e}");
    // the tx must name the current delivery
    let e = c
        .run(70, TxBody::RejectResult { job_id: id, deliverer: pk(4), result_hash: bad_rh, sigs: vec![rsig(60, &reject_message(&id, &pk(4), &bad_rh))] })
        .unwrap_err();
    assert!(e.contains("current delivery"), "{e}");

    // anyone may send it; the evaluator's signature is the authority
    let escrowed_before = c.job(&id).worker_pay;
    c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: bad_rh, sigs: vec![rsig(60, &msg)] }).unwrap();
    let j = c.job(&id);
    assert_eq!(j.status, JobStatus::Open);
    assert_eq!(j.deliverer, [0; 32]);
    assert_eq!(j.result_hash, [0; 32]);
    assert!(j.result_uri.is_empty());
    assert_eq!(j.worker_pay, escrowed_before, "W stays escrowed");
    let r = c.rejects(&id);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0], Reject { deliverer: pk(3), result_hash: bad_rh, height: c.height, protocol_fee: P, signers: vec![pk(60)] });
    let s = c.stats(&pk(3));
    assert_eq!((s.rejected, s.rejected_fee, s.first_height, s.last_height), (1, P, c.height, c.height));

    // a second reject of the now-open job fails
    let e = c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: bad_rh, sigs: vec![rsig(60, &msg)] }).unwrap_err();
    assert!(e.contains("not delivered"), "{e}");

    // the rejected deliverer cannot deliver again, even another result
    let e = c.run(3, TxBody::SubmitResult { job_id: id, result_hash: [0x77; 32], uri: vec![] }).unwrap_err();
    assert!(e.contains("rejected"), "{e}");

    // someone else delivers, the evaluator accepts, W goes to the new deliverer
    let good = [0x55; 32];
    c.run(4, TxBody::SubmitResult { job_id: id, result_hash: good, uri: b"ipfs://good".to_vec() }).unwrap();
    let before = c.bal(&pk(4));
    c.run(70, TxBody::CompleteJob { job_id: id, sigs: vec![rsig(61, &result_message(&id, &pk(4), &good))] }).unwrap();
    assert_eq!(c.job(&id).status, JobStatus::Completed);
    assert_eq!(c.bal(&pk(4)), before + W);
    assert_eq!(c.stats(&pk(4)), RejectStats::default());
}

#[test]
fn reject_needs_the_threshold_and_distinct_signers() {
    let (mut c, id) = setup(&[60, 61, 62], 2);
    let rh = [0x66; 32];
    c.run(3, TxBody::SubmitResult { job_id: id, result_hash: rh, uri: vec![] }).unwrap();
    let msg = reject_message(&id, &pk(3), &rh);
    let e = c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: rh, sigs: vec![rsig(60, &msg)] }).unwrap_err();
    assert!(e.contains("1 of 2"), "{e}");
    let e = c
        .run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: rh, sigs: vec![rsig(60, &msg), rsig(60, &msg)] })
        .unwrap_err();
    assert!(e.contains("duplicate"), "{e}");
    c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: rh, sigs: vec![rsig(60, &msg), rsig(62, &msg)] }).unwrap();
    assert_eq!(c.rejects(&id)[0].signers, vec![pk(60), pk(62)]);
}

#[test]
fn rejected_then_expired_refunds_the_poster_and_keeps_the_record() {
    let mut c = Chain::new();
    for who in [2u8, 3, 70] {
        c.fund(who, 100_000_000);
    }
    let deadline = c.height + 6;
    let id = c.run(2, TxBody::PostJob(job_body(&[60], 1, W, P, deadline))).unwrap().jobs_created[0];
    let rh = [0x66; 32];
    c.run(3, TxBody::SubmitResult { job_id: id, result_hash: rh, uri: vec![] }).unwrap();
    c.run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: rh, sigs: vec![rsig(60, &reject_message(&id, &pk(3), &rh))] })
        .unwrap();
    while c.height <= deadline {
        c.run(70, TxBody::Transfer { to: pk(71), amount: 1 }).unwrap();
    }
    let before = c.bal(&pk(2));
    c.run(70, TxBody::ExpireJob { job_id: id }).unwrap();
    let j = c.job(&id);
    assert_eq!(j.status, JobStatus::Expired);
    assert_eq!(j.deliverer, [0; 32], "a rejected delivery is not a delivered-then-expired one");
    assert_eq!(c.bal(&pk(2)), before + W);
    assert_eq!(c.rejects(&id).len(), 1);
    assert_eq!(c.stats(&pk(3)).rejected, 1);
}

#[test]
fn open_and_completed_jobs_cannot_be_rejected() {
    let (mut c, id) = setup(&[60], 1);
    let rh = [0x55; 32];
    let msg = reject_message(&id, &[0; 32], &[0; 32]);
    let e = c.run(70, TxBody::RejectResult { job_id: id, deliverer: [0; 32], result_hash: [0; 32], sigs: vec![rsig(60, &msg)] }).unwrap_err();
    assert!(e.contains("not delivered"), "{e}");
    c.run(3, TxBody::SubmitResult { job_id: id, result_hash: rh, uri: vec![] }).unwrap();
    c.run(70, TxBody::CompleteJob { job_id: id, sigs: vec![rsig(60, &result_message(&id, &pk(3), &rh))] }).unwrap();
    let e = c
        .run(70, TxBody::RejectResult { job_id: id, deliverer: pk(3), result_hash: rh, sigs: vec![rsig(60, &reject_message(&id, &pk(3), &rh))] })
        .unwrap_err();
    assert!(e.contains("not delivered"), "{e}");
    let e = c.run(70, TxBody::RejectResult { job_id: [9; 32], deliverer: pk(3), result_hash: rh, sigs: vec![] }).unwrap_err();
    assert!(e.contains("unknown job"), "{e}");
}
