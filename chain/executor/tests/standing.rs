//! Model registry, inference tickets and their standing jobs (SPEC 4.5).

use agent_types::tx::{order_job_id, order_message, pubkey_from_seed, result_message, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, supply_of, BlockEnv};
use futures::executor::block_on;
use pearl_job::standing;
use state::config::Config;
use state::schema::{self, Account, JobExt, JobRecord, JobStatus, Model};
use state::{MemState, StateRead};

const CHAIN: u32 = 99;
const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const EVAL: u8 = 60;
const REGISTRANT: u8 = 2;
const BUYER: u8 = 3;
const SERVER: u8 = 4;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}
fn blob(i: u8) -> Vec<u8> {
    format!("blob:{}", hex::encode([i; 32])).into_bytes()
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
        cfg.emission_ppm = 100_000; // 10% of the pool per job block, so caps bind
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        Self { st, height: 0, nonces: Default::default() }
    }

    fn block(&mut self, job: Option<Hash>, txs: &[Tx]) -> Result<executor::BlockSummary, executor::ExecError> {
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(MINER), job_id: job };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs))?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }

    /// One tx in its own block; the sender's nonce only advances when it lands.
    fn send(&mut self, who: u8, body: TxBody) -> Result<executor::BlockSummary, String> {
        let n = *self.nonces.get(&who).unwrap_or(&0);
        let t = Tx::signed(&seed(who), CHAIN, n, 1000, body);
        let r = self.block(None, &[t]).map_err(|e| e.to_string());
        if r.is_ok() {
            self.nonces.insert(who, n + 1);
        }
        r
    }

    fn unlock(&mut self, nonce: u64, to: Pubkey, amount_spl: u64, job: Option<PostJob>) -> Result<executor::BlockSummary, String> {
        let msg = match &job {
            None => agent_types::tx::deposit_message(nonce, &to, amount_spl),
            Some(p) => order_message(nonce, &to, amount_spl, p),
        };
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = *self.nonces.get(&200).unwrap_or(&0);
        let t = Tx::signed(&seed(200), CHAIN, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: job, sigs });
        let r = self.block(None, &[t]).map_err(|e| e.to_string());
        if r.is_ok() {
            self.nonces.insert(200, n + 1);
        }
        r
    }

    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let n = 1000 + self.height;
        self.unlock(n, who, amount_spl, None).unwrap();
    }

    fn bal(&self, a: &Pubkey) -> u64 {
        block_on(self.st.get(&schema::account_key(a))).unwrap().map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }

    fn job(&self, id: &Hash) -> JobRecord {
        JobRecord::decode(&block_on(self.st.get(&schema::job_key(id))).unwrap().unwrap()).unwrap()
    }

    fn model(&self, id: &Hash) -> Model {
        Model::decode(&block_on(self.st.get(&schema::model_key(id))).unwrap().unwrap()).unwrap()
    }
}

/// Two matrices shaped like a Llama layer's qkv and down projection.
fn register_body(name: &[u8]) -> RegisterModel {
    let c = pearl_job::MiningConfig::localnet_default();
    RegisterModel {
        name: name.to_vec(),
        manifest_uri: blob(0x44),
        recipe: 1,
        rank: c.rank,
        rows_pattern: c.rows_pattern.to_bytes(),
        cols_pattern: c.cols_pattern.to_bytes(),
        matrices: vec![MatrixSpec { k: 2048, n: 3072, hash_b: [0x61; 32] }, MatrixSpec { k: 8192, n: 2048, hash_b: [0x62; 32] }],
    }
}

fn ticket_body(model_id: &Hash, prompt: u32, max_new: u32, server: Pubkey, w: u64, p: u64, deadline: u64, max_blocks: u32) -> PostJob {
    PostJob {
        m: prompt,
        n: max_new,
        k: 1,
        config: standing::ticket_config(model_id),
        hash_a: [0x5e; 32],
        hash_b: server,
        data_uri: blob(0x51),
        worker_pay: w,
        protocol_fee: p,
        deadline_height: deadline,
        max_blocks,
        evaluators: vec![pk(EVAL)],
        threshold: 1,
    }
}

fn seg(matrix: u16, m: u32, max_blocks: u32) -> Segment {
    Segment { matrix, m, max_blocks, hash_a: [matrix as u8 + 1; 32], c_hash: [matrix as u8 + 0x80; 32], a_uri: blob(0x90 + matrix as u8) }
}

fn setup() -> (Chain, Hash) {
    let mut c = Chain::new();
    c.fund(pk(REGISTRANT), 1_000_000);
    c.fund(pk(BUYER), 100_000_000);
    c.fund(pk(SERVER), 1_000_000);
    c.fund(pool_address(), 1_000_000_000);
    c.send(REGISTRANT, TxBody::RegisterModel(register_body(b"llama-test"))).unwrap();
    (c, standing::model_id(&pk(REGISTRANT), b"llama-test"))
}

#[test]
fn register_model_checks_shapes_and_uniqueness() {
    let (mut c, id) = setup();
    let m = c.model(&id);
    assert_eq!(m.registrant, pk(REGISTRANT));
    assert_eq!(m.macs_per_row, 2048 * 3072 + 8192 * 2048);
    assert_eq!(m.matrices.len(), 2);
    assert_eq!(m.index, 0);
    let err = c.send(REGISTRANT, TxBody::RegisterModel(register_body(b"llama-test"))).unwrap_err();
    assert!(err.contains("already registered"), "{err}");
    // another registrant may use the same name: a different model id
    c.fund(pk(7), 1000);
    c.send(7, TxBody::RegisterModel(register_body(b"llama-test"))).unwrap();
    assert_eq!(c.model(&standing::model_id(&pk(7), b"llama-test")).index, 1);

    let mut bad = register_body(b"small-k");
    bad.matrices[0].k = 1024; // k < 16r: not a job
    assert!(c.send(REGISTRANT, TxBody::RegisterModel(bad)).unwrap_err().contains("cannot be a job"));
    let mut bad = register_body(b"big");
    bad.matrices[0] = MatrixSpec { k: 8192, n: 16384, hash_b: [1; 32] };
    assert!(c.send(REGISTRANT, TxBody::RegisterModel(bad)).unwrap_err().contains("2^26"));
    let mut bad = register_body(b"odd-n");
    bad.matrices[0].n = 3071;
    assert!(c.send(REGISTRANT, TxBody::RegisterModel(bad)).is_err());
    let mut bad = register_body(b"uri");
    bad.manifest_uri = b"ipfs://x".to_vec();
    assert!(c.send(REGISTRANT, TxBody::RegisterModel(bad)).unwrap_err().contains("blob"));
    let mut bad = register_body(b"recipe");
    bad.recipe = 2;
    assert!(c.send(REGISTRANT, TxBody::RegisterModel(bad)).unwrap_err().contains("recipe"));
}

#[test]
fn ticket_segments_mine_within_alpha_p() {
    let (mut c, model) = setup();
    let (w, p) = (5_000_000_000u64, 1_000_003_000u64);
    let deadline = c.height + 100;
    // refusals first
    assert!(c.send(BUYER, TxBody::PostJob(ticket_body(&[9; 32], 5, 7, pk(SERVER), w, p, deadline, 4))).unwrap_err().contains("unregistered"));
    let mut b = ticket_body(&model, 5, 7, pk(SERVER), w, p, deadline, 4);
    b.k = 2;
    assert!(c.send(BUYER, TxBody::PostJob(b)).unwrap_err().contains("k = 1"));
    assert!(c.send(BUYER, TxBody::PostJob(ticket_body(&model, 4000, 200, pk(SERVER), w, p, deadline, 4))).unwrap_err().contains("rows"));
    let mut b = ticket_body(&model, 5, 7, pk(SERVER), w, p, deadline, 4);
    b.data_uri = b"file:///x".to_vec();
    assert!(c.send(BUYER, TxBody::PostJob(b)).unwrap_err().contains("request blob"));

    let pool0 = c.bal(&pool_address());
    let n = *c.nonces.get(&BUYER).unwrap_or(&0);
    c.send(BUYER, TxBody::PostJob(ticket_body(&model, 5, 7, pk(SERVER), w, p, deadline, 4))).unwrap();
    let tid = schema::job_id_for_post(&pk(BUYER), n);
    let t = c.job(&tid);
    let te = *t.ticket().unwrap();
    assert_eq!((te.rows, te.server, te.model_id), (12, pk(SERVER), model));
    assert_eq!(te.cap_total, p * 8 / 10);
    assert_eq!(t.cap_remaining, te.cap_total);
    assert_eq!(c.bal(&pool_address()), pool0 + p);
    assert_eq!(c.model(&model).tickets, 1);
    assert!(c.block(Some(tid), &[]).unwrap_err().to_string().contains("not mineable"));

    // segment rules
    assert!(c.send(BUYER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(0, 12, 1)] }).unwrap_err().contains("server"));
    for (g, why) in [(seg(2, 12, 1), "no matrix"), (seg(0, 10, 1), "multiple"), (seg(0, 16, 1), "budget"), (seg(0, 12, 5), "blocks"), (seg(0, 12, 0), "max_blocks")] {
        let e = c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![g] }).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
    }
    let mut g = seg(0, 12, 1);
    g.a_uri = b"ipfs://a".to_vec();
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![g] }).unwrap_err().contains("blob"));
    // SubmitResult by anyone but the server is refused
    c.fund(pk(8), 1000);
    assert!(c.send(8, TxBody::SubmitResult { job_id: tid, result_hash: [1; 32], uri: blob(0x52) }).unwrap_err().contains("server"));

    c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(0, 12, 1), seg(1, 12, 3)] }).unwrap();
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(1, 12, 1)] }).is_err(), "one segment per matrix per round");
    let s0 = standing::segment_id(&tid, 0, 0);
    let s1 = standing::segment_id(&tid, 1, 0);
    let (j0, j1) = (c.job(&s0), c.job(&s1));
    let macs = 2048u64 * 3072 + 8192 * 2048;
    assert_eq!(j0.cap_remaining, (te.cap_total as u128 * 12 * 2048 * 3072 / (12 * macs as u128)) as u64);
    assert_eq!(j1.cap_remaining, (te.cap_total as u128 * 12 * 8192 * 2048 / (12 * macs as u128)) as u64);
    assert!(j0.cap_remaining + j1.cap_remaining <= te.cap_total);
    assert_eq!(c.job(&tid).cap_remaining, te.cap_total - j0.cap_remaining - j1.cap_remaining);
    // a segment is a standing job on the registered matrix
    assert_eq!((j0.k, j0.n, j0.m, j0.hash_b, j0.header_id), (2048, 3072, 12, [0x61; 32], model));
    assert_eq!(j0.terms_hash, standing::matrix_terms_hash(&model, 0, 2048, 3072));
    assert_eq!(j0.job_key().unwrap(), standing::matrix_job_key(&model, 0, 2048, 3072, &pearl_job::MiningConfig::localnet_default()));
    assert_eq!(j0.result_hash, [0x80; 32]);
    assert_eq!(j0.segment(), Some((tid, model, 0)));
    assert_eq!(j0.worker_pay + j0.protocol_fee, 0);
    assert!(c.send(SERVER, TxBody::SubmitResult { job_id: s0, result_hash: [1; 32], uri: blob(1) }).unwrap_err().contains("committed"));

    // mine: every block is capped by its segment, the ticket rolls the totals up
    let mut total = 0;
    for id in [s0, s1] {
        let s = c.block(Some(id), &[]).unwrap().subsidy;
        assert!(s > 0);
        total += s;
    }
    // the pool pays 10% per block here, so one block uses up each segment's cap
    assert!(c.block(Some(s1), &[]).unwrap_err().to_string().contains("cap"));
    assert!(c.block(Some(s0), &[]).unwrap_err().to_string().contains("max_blocks"));
    assert_eq!(total, j0.cap_remaining + j1.cap_remaining);
    let te2 = *c.job(&tid).ticket().unwrap();
    assert_eq!(te2.blocks_mined, 2);
    assert_eq!(te2.subsidy_paid, total);
    assert!(total <= p * 8 / 10, "the ticket's whole subsidy stays under alpha x P");
    assert_eq!(c.model(&model).blocks_mined, 2);
    assert_eq!(c.model(&model).subsidy_paid, total);
    assert_eq!(c.model(&model).segments, 2);

    // the server delivers, the evaluator signs, W goes to the server
    let rh = [0x77; 32];
    c.send(SERVER, TxBody::SubmitResult { job_id: tid, result_hash: rh, uri: blob(0x52) }).unwrap();
    let msg = result_message(&tid, &pk(SERVER), &rh);
    let before = c.bal(&pk(SERVER));
    c.send(SERVER, TxBody::CompleteJob { job_id: tid, sigs: vec![AttesterSig { pubkey: pk(EVAL), sig: sign_plain(&seed(EVAL), &msg) }] }).unwrap();
    assert_eq!(c.bal(&pk(SERVER)), before + w - 1000, "every row was used: all of W");
    assert_eq!(c.job(&tid).status, JobStatus::Completed);
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 1, segments: vec![seg(1, 4, 1)] }).unwrap_err().contains("closed"));
}

#[test]
fn order_ticket_server_claimed_by_first_poster_and_partial_rows_earn_less() {
    let (mut c, model) = setup();
    let (w, p) = (4_000_000u64, 1_000_000u64);
    let body = ticket_body(&model, 6, 10, [0; 32], w, p, 50, 2);
    c.unlock(77, pk(BUYER), (w + p) / SPL_SCALE, Some(body)).unwrap();
    let tid = order_job_id(77);
    let t = c.job(&tid);
    assert_eq!(t.deadline_height, c.height + 50);
    assert_eq!(t.poster, pk(BUYER));
    assert_eq!(t.ticket().unwrap().server, [0; 32]);
    // the first PostSegments claims the server; the request stopped early (8 of 16 rows)
    c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(0, 8, 1), seg(1, 8, 1)] }).unwrap();
    assert_eq!(c.job(&tid).ticket().unwrap().server, pk(SERVER));
    let caps = c.job(&standing::segment_id(&tid, 0, 0)).cap_remaining + c.job(&standing::segment_id(&tid, 1, 0)).cap_remaining;
    let cap_total = p * 8 / 10;
    assert!(caps <= cap_total / 2 && caps + 2 >= cap_total / 2, "half the rows, half the cap: {caps}");
    // past the deadline the segments stop being mineable
    while c.height <= t.deadline_height {
        c.block(None, &[]).unwrap();
    }
    assert!(c.block(Some(standing::segment_id(&tid, 0, 0)), &[]).unwrap_err().to_string().contains("expired"));
    // ExpireJob on the ticket refunds W to the buyer; on a segment it just closes it
    let before = c.bal(&pk(BUYER));
    c.send(SERVER, TxBody::ExpireJob { job_id: tid }).unwrap();
    assert_eq!(c.bal(&pk(BUYER)), before + w);
    c.send(SERVER, TxBody::ExpireJob { job_id: standing::segment_id(&tid, 0, 0) }).unwrap();
    assert_eq!(c.job(&standing::segment_id(&tid, 0, 0)).status, JobStatus::Expired);
}

#[test]
fn job_records_with_extensions_round_trip() {
    let (mut c, model) = setup();
    let n = *c.nonces.get(&BUYER).unwrap_or(&0);
    c.send(BUYER, TxBody::PostJob(ticket_body(&model, 4, 4, pk(SERVER), 1_000_000, 1_000_000, c.height + 20, 1))).unwrap();
    let tid = schema::job_id_for_post(&pk(BUYER), n);
    let t = c.job(&tid);
    assert!(matches!(t.ext, Some(JobExt::Ticket(_))));
    assert_eq!(JobRecord::decode(&t.encode()).unwrap(), t);
    let plain = JobRecord { ext: None, ..t.clone() };
    let enc = plain.encode();
    assert!(enc.len() < t.encode().len(), "plain jobs carry no extension bytes");
    assert_eq!(JobRecord::decode(&enc).unwrap(), plain);
    let m = c.model(&model);
    assert_eq!(Model::decode(&m.encode()).unwrap(), m);
}

#[test]
fn balance_ticket_rounds_and_pay_for_rows_used() {
    let (mut c, model) = setup();
    let (w, p) = (8_000_000u64, 4_000_000u64);
    let n = *c.nonces.get(&BUYER).unwrap_or(&0);
    // a prepaid balance: R = round_up(m + n) = 64 rows
    c.send(BUYER, TxBody::PostJob(ticket_body(&model, 63, 1, pk(SERVER), w, p, c.height + 100, 10))).unwrap();
    let tid = schema::job_id_for_post(&pk(BUYER), n);
    assert_eq!(c.job(&tid).ticket().unwrap().rows, 64);
    // round 0: 8 rows over both matrices, in two txs (the second continues the round)
    c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(0, 8, 1)] }).unwrap();
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(1, 12, 1)] }).unwrap_err().contains("has m 8"));
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 2, segments: vec![seg(1, 8, 1)] }).unwrap_err().contains("round 2"));
    c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 0, segments: vec![seg(1, 8, 1)] }).unwrap();
    // round 1: 16 more rows; a second segment for matrix 0 in a new round
    c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 1, segments: vec![seg(0, 16, 1), seg(1, 16, 1)] }).unwrap();
    let te = *c.job(&tid).ticket().unwrap();
    assert_eq!((te.rounds, te.round_rows, te.rows_used, te.segments), (2, 16, 24, 4));
    assert!(c.send(SERVER, TxBody::PostSegments { ticket_id: tid, round: 2, segments: vec![seg(0, 44, 1)] }).unwrap_err().contains("budget"));
    let r1 = standing::segment_id(&tid, 0, 1);
    assert_ne!(r1, standing::segment_id(&tid, 0, 0));
    assert_eq!(c.job(&r1).m, 16);
    // caps over all rounds stay under alpha x P x rows_used / R
    let caps: u64 = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().map(|&(m, r)| c.job(&standing::segment_id(&tid, m, r)).cap_remaining).sum();
    assert!(caps <= te.cap_total * 24 / 64, "{caps}");
    c.block(Some(r1), &[]).unwrap();
    // complete: the server gets W x 24 / 64, the buyer W x 40 / 64 + P x 40 / 64 from the pool
    let rh = [0x77; 32];
    c.send(SERVER, TxBody::SubmitResult { job_id: tid, result_hash: rh, uri: blob(0x52) }).unwrap();
    let msg = result_message(&tid, &pk(SERVER), &rh);
    let (srv0, buy0, pool0) = (c.bal(&pk(SERVER)), c.bal(&pk(BUYER)), c.bal(&pool_address()));
    let t = Tx::signed(&seed(SERVER), CHAIN, *c.nonces.get(&SERVER).unwrap(), 1000,
        TxBody::CompleteJob { job_id: tid, sigs: vec![AttesterSig { pubkey: pk(EVAL), sig: sign_plain(&seed(EVAL), &msg) }] });
    let s = c.block(None, &[t]).unwrap();
    c.nonces.insert(SERVER, c.nonces[&SERVER] + 1);
    assert_eq!(c.bal(&pk(SERVER)), srv0 + w * 24 / 64 - 1000);
    assert_eq!(c.bal(&pk(BUYER)), buy0 + w * 40 / 64 + p * 40 / 64);
    assert_eq!(c.bal(&pool_address()) + p * 40 / 64, pool0 + s.subsidy, "P share back from the pool");
}
