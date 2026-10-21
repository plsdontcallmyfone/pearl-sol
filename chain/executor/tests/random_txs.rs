//! Random tx sequences through the executor (security pass 2026-10-21).
//!
//! Properties, over thousands of generated sequences mixing every money-moving kind with
//! hostile amounts (0, u64::MAX, more than the balance), duplicate deposits, wrong nonces,
//! job blocks and payout liens:
//! 1. no panic in execution;
//! 2. supply conservation: balances + live job pay + immature coinbase + lien pending
//!    equals TOTAL_SUPPLY after every block;
//! 3. determinism: the same sequence on a second state gives the same records.

use agent_types::tx::{deposit_message, pubkey_from_seed, route_clear_message, sign_plain};
use agent_types::*;
use executor::{genesis_writes, supply_of, BlockEnv, BlockExecutor};
use futures::executor::block_on;
use proptest::prelude::*;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const CHAIN: u32 = 99;
const ATT: u8 = 50;
const USERS: u8 = 4;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

#[derive(Debug, Clone)]
enum Op {
    Unlock { to: u8, nonce: u8, amount_spl: u64 },
    Transfer { from: u8, to: u8, amount: u64, fee: u64 },
    Lock { from: u8, amount: u64 },
    PostJob { from: u8, w: u64, p: u64, deadline: u8, max_blocks: u8 },
    Expire { from: u8, job_of: u8, job_nonce: u8 },
    SetRoute { from: u8, bps: u16, until: u8, cap: u64 },
    ClearRoute { from: u8, account: u8, route_nonce: u8 },
    BadNonce { from: u8 },
    /// End the block; `job` mines the given (poster, nonce) job if it exists.
    Seal { job: Option<(u8, u8)> },
}

fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0u64), Just(u64::MAX), 1u64..10_000, 1_000u64..100_000_000_000, any::<u64>()]
}

fn op() -> impl Strategy<Value = Op> {
    let u = 0..USERS;
    prop_oneof![
        (u.clone(), 0u8..6, prop_oneof![Just(0u64), 1u64..10_000_000, any::<u64>()]).prop_map(|(to, nonce, amount_spl)| Op::Unlock { to, nonce, amount_spl }),
        (u.clone(), u.clone(), amount(), prop_oneof![Just(1000u64), amount()]).prop_map(|(from, to, amount, fee)| Op::Transfer { from, to, amount, fee }),
        (u.clone(), amount()).prop_map(|(from, amount)| Op::Lock { from, amount }),
        (u.clone(), amount(), amount(), 0u8..20, 0u8..5).prop_map(|(from, w, p, deadline, max_blocks)| Op::PostJob { from, w, p, deadline, max_blocks }),
        (u.clone(), u.clone(), 0u8..8).prop_map(|(from, job_of, job_nonce)| Op::Expire { from, job_of, job_nonce }),
        (u.clone(), 0u16..12_000, 0u8..30, amount()).prop_map(|(from, bps, until, cap)| Op::SetRoute { from, bps, until, cap }),
        (u.clone(), u.clone(), 0u8..3).prop_map(|(from, account, route_nonce)| Op::ClearRoute { from, account, route_nonce }),
        u.clone().prop_map(|from| Op::BadNonce { from }),
        prop::option::of((u.clone(), 0u8..8)).prop_map(|job| Op::Seal { job }),
    ]
}

struct Sim {
    st: MemState,
    height: u64,
    pending: Vec<Tx>,
}

impl Sim {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(1));
        cfg.attesters = vec![pk(ATT)];
        cfg.attester_threshold = 1;
        cfg.finality_k = 3;
        cfg.emission_ppm = 100_000;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        Self { st, height: 0, pending: Vec::new() }
    }

    fn nonce(&self, who: u8) -> u64 {
        let p = self.pending.iter().filter(|t| t.from == pk(who + 10)).count() as u64;
        block_on(self.st.get(&schema::account_key(&pk(who + 10)))).unwrap().map(|b| Account::decode(&b).unwrap().nonce).unwrap_or(0) + p
    }

    fn signed(&self, who: u8, fee: u64, body: TxBody) -> Tx {
        Tx::signed(&seed(who + 10), CHAIN, self.nonce(who), fee, body)
    }

    fn push(&mut self, op: &Op) {
        let h = self.height + 1;
        let t = match op {
            Op::Unlock { to, nonce, amount_spl } => {
                let to = pk(to + 10);
                let msg = deposit_message(*nonce as u64, &to, *amount_spl);
                let sigs = vec![AttesterSig { pubkey: pk(ATT), sig: sign_plain(&seed(ATT), &msg) }];
                self.signed(0, 0, TxBody::BridgeUnlock { solana_nonce: *nonce as u64, recipient: to, amount_spl: *amount_spl, job_order: None, sigs })
            }
            Op::Transfer { from, to, amount, fee } => self.signed(*from, *fee, TxBody::Transfer { to: pk(to + 10), amount: *amount }),
            Op::Lock { from, amount } => self.signed(*from, 1000, TxBody::BridgeLock { solana_recipient: [3; 32], amount: *amount }),
            Op::PostJob { from, w, p, deadline, max_blocks } => {
                let cfg = pearl_job::MiningConfig::localnet_default();
                let body = PostJob {
                    m: 64,
                    n: 64,
                    k: cfg.k() as u32,
                    config: cfg.to_bytes(),
                    hash_a: [1; 32],
                    hash_b: [2; 32],
                    data_uri: b"ipfs://x".to_vec(),
                    worker_pay: *w,
                    protocol_fee: *p,
                    deadline_height: h + *deadline as u64,
                    max_blocks: *max_blocks as u32,
                    evaluators: vec![pk(60)],
                    threshold: 1,
                };
                self.signed(*from, 1000, TxBody::PostJob(body))
            }
            Op::Expire { from, job_of, job_nonce } => {
                let id = schema::job_id_for_post(&pk(job_of + 10), *job_nonce as u64);
                self.signed(*from, 1000, TxBody::ExpireJob { job_id: id })
            }
            Op::SetRoute { from, bps, until, cap } => {
                self.signed(*from, 1000, TxBody::SetRoute { solana_recipient: [4; 32], route_bps: *bps, until_height: h + *until as u64, cap: *cap, release_key: pk(70) })
            }
            Op::ClearRoute { from, account, route_nonce } => {
                let a = pk(account + 10);
                let sig = sign_plain(&seed(70), &route_clear_message(CHAIN, &a, *route_nonce as u64));
                self.signed(*from, 1000, TxBody::ClearRoute { account: a, route_nonce: *route_nonce as u64, sig })
            }
            Op::BadNonce { from } => Tx::signed(&seed(from + 10), CHAIN, self.nonce(*from) + 5, 1000, TxBody::Transfer { to: pk(1), amount: 1 }),
            Op::Seal { job } => {
                self.seal(job.map(|(p, n)| schema::job_id_for_post(&pk(p + 10), n as u64)));
                return;
            }
        };
        self.pending.push(t);
    }

    /// Build a block the way a miner does (failing txs skipped), then check the invariant.
    fn seal(&mut self, job: Option<Hash>) {
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(9), job_id: job };
        let mut ex = match block_on(BlockExecutor::begin(&self.st, env)) {
            Ok(ex) => ex,
            // the named job is unknown or not mineable: a fallback block instead
            Err(_) => block_on(BlockExecutor::begin(&self.st, BlockEnv { job_id: None, ..env })).unwrap(),
        };
        for t in std::mem::take(&mut self.pending) {
            let _ = block_on(ex.apply_tx(&t));
        }
        let (ws, _) = block_on(ex.finish(true)).unwrap();
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
    }
}

fn run(ops: &[Op]) -> Vec<Vec<u8>> {
    let mut s = Sim::new();
    for o in ops {
        s.push(o);
    }
    s.seal(None);
    for _ in 0..4 {
        s.seal(None); // let coinbase and liens mature
    }
    s.st.values()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn random_sequences_conserve_supply_and_replay(ops in prop::collection::vec(op(), 1..80)) {
        let a = run(&ops);
        let b = run(&ops);
        prop_assert_eq!(a, b, "two runs of one sequence diverged");
    }
}
