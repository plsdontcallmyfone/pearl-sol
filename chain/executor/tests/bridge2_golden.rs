//! Determinism check for the two-side bridge (docs/BRIDGE-V2.md): a scripted run of the ORIGINAL
//! kinds (unlocks plain and with an order, transfers, BridgeLock, a payout lien on the miner
//! with matured coinbase, a job delivered and completed, ClearRoute, SetRoute replacement,
//! AdminConfig) must produce exactly the write sets, withdrawal-log entries and coinbase
//! receipts it produced before the v2 code existed.
//!
//! This file uses only APIs that predate the v2 code, so it compiles on the base commit too;
//! GOLDEN was taken there (f3be94a) and must not change.

use agent_types::tx::{deposit_message, order_message, pubkey_from_seed, result_message, route_clear_message, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, BlockEnv};
use futures::executor::block_on;
use state::config::{keys, Config};
use state::MemState;

const CHAIN: u32 = 77;
const ATT: [u8; 2] = [50, 51];
const MINER: u8 = 9;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

struct C {
    st: MemState,
    h: u64,
    nonces: std::collections::HashMap<u8, u64>,
    acc: blake3::Hasher,
}

impl C {
    fn tx(&mut self, who: u8, fee: u64, body: TxBody) -> Tx {
        let n = self.nonces.entry(who).or_insert(0);
        let t = Tx::signed(&seed(who), CHAIN, *n, fee, body);
        *n += 1;
        t
    }
    fn block(&mut self, job: Option<Hash>, txs: &[Tx]) {
        let env = BlockEnv { chain_id: CHAIN, height: self.h + 1, miner: pk(MINER), job_id: job };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs)).unwrap();
        for (k, v) in &ws.writes {
            self.acc.update(k);
            match v {
                Some(v) => {
                    self.acc.update(&[1]);
                    self.acc.update(&(v.len() as u64).to_le_bytes());
                    self.acc.update(v);
                }
                None => {
                    self.acc.update(&[0]);
                }
            }
        }
        for e in &ws.withdrawals {
            self.acc.update(b"w");
            self.acc.update(e);
        }
        self.acc.update(&sum.coinbase.encode());
        self.st.apply(&ws);
        self.h += 1;
    }
    fn sigs(msg: &[u8]) -> Vec<AttesterSig> {
        ATT.iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), msg) }).collect()
    }
    fn unlock(&mut self, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
        let sigs = Self::sigs(&deposit_message(nonce, &to, amount_spl));
        self.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
    }
}

fn job_body(w: u64, p: u64, deadline: u64, max_blocks: u32) -> PostJob {
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
        evaluators: vec![pk(60)],
        threshold: 1,
    }
}

fn run() -> String {
    let mut cfg = Config::defaults(pk(1));
    cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
    cfg.attester_threshold = 2;
    cfg.finality_k = 3;
    cfg.emission_ppm = 100_000;
    let st = MemState::default();
    let g = genesis_writes(&cfg).unwrap();
    st.apply(&g);
    let mut c = C { st, h: 0, nonces: Default::default(), acc: blake3::Hasher::new() };
    // funding
    let a = c.unlock(1, pk(2), 50_000_000);
    let b = c.unlock(2, pk(3), 9_000_000);
    let m = c.unlock(3, pk(MINER), 1_000_000);
    let ad = c.unlock(4, pk(1), 1_000_000);
    c.block(None, &[a, b, m, ad]);
    // transfer + BridgeLock to Solana
    let t1 = c.tx(2, 1000, TxBody::Transfer { to: pk(4), amount: 123_456 });
    let t2 = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: 2_000_000 });
    c.block(None, &[t1, t2]);
    // a payout lien on the miner, then a job posted, mined, delivered, completed
    let lien = c.tx(MINER, 1000, TxBody::SetRoute { solana_recipient: [0x4c; 32], route_bps: 4000, until_height: 60, cap: u64::MAX, release_key: pk(70) });
    let post = c.tx(2, 1000, TxBody::PostJob(job_body(5_000_000_000, 1_000_000_000, 200, 5)));
    c.block(None, &[lien, post]);
    let job_id = state::schema::job_id_for_post(&pk(2), 2);
    for _ in 0..3 {
        c.block(Some(job_id), &[]);
    }
    let rh = [0x77; 32];
    let sub = c.tx(5, 1000, TxBody::SubmitResult { job_id, result_hash: rh, uri: b"ipfs://r".to_vec() });
    let fund5 = c.unlock(5, pk(5), 10_000);
    c.block(None, &[fund5]);
    c.block(None, &[sub]);
    let msg = result_message(&job_id, &pk(5), &rh);
    let done = c.tx(3, 1000, TxBody::CompleteJob { job_id, sigs: vec![AttesterSig { pubkey: pk(60), sig: sign_plain(&seed(60), &msg) }] });
    c.block(None, &[done]);
    // a storefront order unlock
    let body = job_body(1_000_000_000, 500_000_000, 50, 3);
    let amount_spl = 2_000_000;
    let osigs = C::sigs(&order_message(6, &pk(6), amount_spl, &body));
    let order = c.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: 6, recipient: pk(6), amount_spl, job_order: Some(body), sigs: osigs });
    c.block(None, &[order]);
    // matured coinbase flows through the lien for a while
    for _ in 0..6 {
        c.block(None, &[]);
    }
    // clear the lien early, then set a new one (replacement path) and an admin change
    let cmsg = route_clear_message(CHAIN, &pk(MINER), 0);
    let clr = c.tx(4, 1000, TxBody::ClearRoute { account: pk(MINER), route_nonce: 0, sig: sign_plain(&seed(70), &cmsg) });
    c.block(None, &[clr]);
    let lien2 = c.tx(MINER, 1000, TxBody::SetRoute { solana_recipient: [0x4d; 32], route_bps: 10_000, until_height: 40, cap: 3_000_000, release_key: [0; 32] });
    let adm = c.tx(1, 1000, TxBody::AdminConfig { key: keys::MIN_PROTOCOL_FEE, value: 7u64.to_le_bytes().to_vec() });
    c.block(None, &[lien2, adm]);
    for _ in 0..8 {
        c.block(None, &[]);
    }
    let l2 = c.tx(3, 1000, TxBody::BridgeLock { solana_recipient: [9; 32], amount: 1_000_000 });
    c.block(None, &[l2]);
    hex::encode(c.acc.finalize().as_bytes())
}

/// Taken on the base commit f3be94a (before the two-side bridge).
const GOLDEN: &str = "c40327eff0beb635777556caf31b64fc633d32c6333ff24ab34ef177e498ccbd";

#[test]
fn original_kinds_execute_byte_identically() {
    let got = run();
    if std::env::var("PRINT_GOLDEN").is_ok() {
        println!("GOLDEN {got}");
        return;
    }
    assert_eq!(got, GOLDEN);
}
