//! Chain speed upgrade in the executor (docs/SPEED.md 3.1-3.3): the one-time config migration
//! at the speed height, emission per day before and after, uncle rewards (share by depth,
//! nephew, cap and pool debits, maturity, payout lien split, double inclusion), with the supply
//! invariant checked after every block.

use agent_types::speed::{TEST_SPEED_CHAIN_BASE, UncleRecord};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{execute_block_with_uncles, genesis_writes, supply_of, BlockEnv, BlockSummary, UncleInput};
use futures::executor::block_on;
use state::config::{keys, Config};
use state::schema::{self, Account, JobRecord};
use state::{MemState, StateRead};

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

const ADMIN: u8 = 1;
const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const UNCLE_MINER: u8 = 11;

struct Chain {
    st: MemState,
    chain_id: u32,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

impl Chain {
    fn new(chain_id: u32, k: u32) -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = k;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        Self { st, chain_id, height: 0, nonces: Default::default() }
    }
    fn tx(&mut self, who: u8, fee: u64, body: TxBody) -> Tx {
        let n = self.nonces.entry(who).or_insert(0);
        let t = Tx::signed(&seed(who), self.chain_id, *n, fee, body);
        *n += 1;
        t
    }
    fn block_u(&mut self, job: Option<Hash>, txs: &[Tx], uncles: Vec<UncleInput>) -> Result<BlockSummary, executor::ExecError> {
        let env = BlockEnv { chain_id: self.chain_id, height: self.height + 1, miner: pk(MINER), job_id: job };
        let (ws, sum) = block_on(execute_block_with_uncles(&self.st, env, txs, uncles))?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }
    fn block(&mut self, txs: &[Tx]) -> BlockSummary {
        self.block_u(None, txs, Vec::new()).unwrap()
    }
    fn get(&self, k: &schema::Key) -> Option<Vec<u8>> {
        block_on(self.st.get(k)).unwrap()
    }
    fn cfg(&self) -> Config {
        Config::decode(&self.get(&schema::config_key()).unwrap()).unwrap()
    }
    fn bal(&self, a: &Pubkey) -> u64 {
        self.get(&schema::account_key(a)).map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }
    fn job(&self, id: &Hash) -> JobRecord {
        JobRecord::decode(&self.get(&schema::job_key(id)).unwrap()).unwrap()
    }
    fn unlock(&mut self, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
        let msg = deposit_message(nonce, &to, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        self.tx(200, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
    }
    /// Fund the pool and post a job with protocol fee `p` (cap = 0.8 p); returns the job id.
    fn setup_job(&mut self, pool_spl: u64, p: u64) -> Hash {
        let t = self.unlock(1, pk(2), 1_000_000_000);
        self.block(&[t]);
        let t = self.unlock(2, pool_address(), pool_spl);
        self.block(&[t]);
        let cfg = pearl_job::MiningConfig::localnet_default();
        let body = PostJob {
            m: 64,
            n: 64,
            k: cfg.k() as u32,
            config: cfg.to_bytes(),
            hash_a: [1; 32],
            hash_b: [2; 32],
            data_uri: b"ipfs://x".to_vec(),
            worker_pay: 1_000_000,
            protocol_fee: p,
            deadline_height: 10_000,
            max_blocks: 1000,
            evaluators: vec![pk(60)],
            threshold: 1,
        };
        let t = self.tx(2, 1000, TxBody::PostJob(body));
        self.block(&[t]).jobs_created[0]
    }
}

fn uncle(job: Hash, height: u64, tag: u8) -> UncleInput {
    UncleInput { hash: [tag; 32], height, miner: pk(UNCLE_MINER), job_id: job }
}

#[test]
fn migration_runs_once_at_the_speed_height_on_migrating_chains_only() {
    let s = 5u64;
    let mut c = Chain::new(TEST_SPEED_CHAIN_BASE + s as u32, 30);
    let mut plain = Chain::new(99, 30);
    for h in 1..=8u64 {
        c.block(&[]);
        plain.block(&[]);
        let cfg = c.cfg();
        if h < s {
            assert_eq!((cfg.target_block_time_ms, cfg.finality_k, cfg.emission_ppb), (60_000, 30, 0), "before S at {h}");
        } else {
            assert_eq!((cfg.target_block_time_ms, cfg.finality_k, cfg.emission_ppb), (30_000, 60, 3_500), "from S at {h}");
        }
        let p = plain.cfg();
        assert_eq!((p.target_block_time_ms, p.finality_k, p.emission_ppb), (60_000, 30, 0), "a non-migrating chain never changes");
    }
}

#[test]
fn block_s_matures_at_s_plus_new_k() {
    let s = 3u64;
    let mut c = Chain::new(TEST_SPEED_CHAIN_BASE + s as u32, 2);
    c.block(&[]);
    c.block(&[]);
    let sum = c.block(&[]); // block S
    assert_eq!(sum.coinbase.mature_height, s + 4, "K doubled from 2 to 4 inside block S");
}

#[test]
fn emission_per_day_is_unchanged() {
    let pool0: u64 = 100_000_000 * 1_000_000_000; // 100M CADRE in base units
    let mut c60 = Config::defaults([1; 32]);
    let mut c30 = c60.clone();
    c30.emission_ppb = c60.emission_ppm * 1000 / 2;
    assert_eq!(c30.emission_ppb, 3_500);
    let day = |c: &Config, blocks: u32| {
        let mut pool = pool0;
        for _ in 0..blocks {
            pool -= c.emission_of(pool);
        }
        pool0 - pool
    };
    let d60 = day(&c60, 1440);
    let d30 = day(&c30, 2880);
    // 1 - (1 - 7e-6)^1440 = 1.00294% and 1 - (1 - 3.5e-6)^2880 = 1.00294%
    let f60 = d60 as f64 / pool0 as f64;
    let f30 = d30 as f64 / pool0 as f64;
    assert!((0.010029..0.010030).contains(&f60), "{f60}");
    assert!((0.010029..0.010030).contains(&f30), "{f30}");
    assert!((d60 as i128 - d30 as i128).unsigned_abs() * 100_000 < d60 as u128, "within 0.001%: {d60} vs {d30}");
    c60.emission_ppm = 7;
    assert_eq!(c60.emission_of(1_000_000), 7);
}

#[test]
fn uncle_rewards_by_depth_with_nephew_cap_pool_and_maturity() {
    let k = 3u32;
    let mut c = Chain::new(99, k);
    let job = c.setup_job(1_000_000, 100_000_000_000);
    let cap0 = c.job(&job).cap_remaining;
    let mined0 = c.job(&job).blocks_mined;
    let pool0 = c.bal(&pool_address());
    let cfg = c.cfg();
    let base = cfg.emission_of(pool0).min(cap0);
    let h = c.height + 1;
    let s = c.block_u(None, &[], vec![uncle(job, h - 1, 0xa1)]).unwrap();
    let r1 = base / 2;
    let n1 = r1 / 32;
    assert_eq!(
        s.uncles,
        vec![UncleRecord { height: h, uncle_hash: [0xa1; 32], uncle_height: h - 1, miner: pk(UNCLE_MINER), job_id: job, reward: r1, nephew: n1, mature_height: h + k as u64 }]
    );
    assert_eq!(c.bal(&pool_address()), pool0 - r1 - n1);
    let j = c.job(&job);
    assert_eq!(j.cap_remaining, cap0 - r1 - n1);
    assert_eq!(j.blocks_mined, mined0, "uncles do not count toward max_blocks");
    assert_eq!(schema::decode_uncle_value(&c.get(&schema::uncle_key(&[0xa1; 32])).unwrap()), Some(h));
    // depth 2: a quarter
    let pool1 = c.bal(&pool_address());
    let base2 = cfg.emission_of(pool1).min(c.job(&job).cap_remaining);
    let h2 = c.height + 1;
    let s2 = c.block_u(None, &[], vec![uncle(job, h2 - 2, 0xa2)]).unwrap();
    assert_eq!((s2.uncles[0].reward, s2.uncles[0].nephew), (base2 / 4, base2 / 4 / 32));
    // maturity: the uncle miner and this block's miner get them at h + K
    let um0 = c.bal(&pk(UNCLE_MINER));
    while c.height < h + k as u64 - 1 {
        c.block(&[]);
    }
    assert_eq!(c.bal(&pk(UNCLE_MINER)), um0, "not before maturity");
    let m0 = c.bal(&pk(MINER));
    c.block(&[]); // height h + K
    assert_eq!(c.bal(&pk(UNCLE_MINER)), um0 + r1);
    assert!(c.bal(&pk(MINER)) >= m0 + n1, "nephew credited with the block's own coinbase");
}

#[test]
fn uncle_errors_depth_duplicates_unknown_job() {
    let mut c = Chain::new(99, 3);
    let job = c.setup_job(1_000_000, 100_000_000_000);
    let h = c.height + 1;
    let e = c.block_u(None, &[], vec![uncle(job, h - 3, 1)]).unwrap_err().to_string();
    assert!(e.contains("depth 3"), "{e}");
    let e = c.block_u(None, &[], vec![uncle(job, h, 1)]).unwrap_err().to_string();
    assert!(e.contains("depth 0"), "{e}");
    let e = c.block_u(None, &[], vec![uncle([0x77; 32], h - 1, 1)]).unwrap_err().to_string();
    assert!(e.contains("unknown job"), "{e}");
    let e = c.block_u(None, &[], vec![uncle(job, h - 1, 1), uncle(job, h - 1, 2), uncle(job, h - 1, 3)]).unwrap_err().to_string();
    assert!(e.contains("too many uncles"), "{e}");
    c.block_u(None, &[], vec![uncle(job, h - 1, 5)]).unwrap();
    let h = c.height + 1;
    let e = c.block_u(None, &[], vec![uncle(job, h - 2, 5)]).unwrap_err().to_string();
    assert!(e.contains("already included"), "{e}");
}

#[test]
fn uncle_reward_is_bounded_by_the_job_cap() {
    let mut c = Chain::new(99, 3);
    // tiny protocol fee: cap 0.8 x 1000 = 800, far below the pool emission
    let job = c.setup_job(1_000_000_000, 1000);
    let cap = c.job(&job).cap_remaining;
    assert_eq!(cap, 800);
    let h = c.height + 1;
    let s = c.block_u(None, &[], vec![uncle(job, h - 1, 9)]).unwrap();
    assert_eq!((s.uncles[0].reward, s.uncles[0].nephew), (400, 12));
    assert_eq!(c.job(&job).cap_remaining, 800 - 412);
}

#[test]
fn uncle_reward_is_split_by_a_payout_lien() {
    let k = 3u32;
    let mut c = Chain::new(99, k);
    let job = c.setup_job(1_000_000, 100_000_000_000);
    let t = c.unlock(3, pk(UNCLE_MINER), 1000);
    c.block(&[t]);
    let lender = [0x4c; 32];
    let t = c.tx(UNCLE_MINER, 1000, TxBody::SetRoute { solana_recipient: lender, route_bps: 5000, until_height: 10_000, cap: u64::MAX, release_key: [0; 32] });
    c.block(&[t]);
    for _ in 0..k {
        c.block(&[]);
    }
    let r0 = block_on(c.st.get(&schema::route_key(&pk(UNCLE_MINER)))).unwrap().map(|b| schema::Route::decode(&b).unwrap()).unwrap();
    let h = c.height + 1;
    let s = c.block_u(None, &[], vec![uncle(job, h - 1, 0x31)]).unwrap();
    let reward = s.uncles[0].reward;
    assert!(reward > 0);
    for _ in 0..k {
        c.block(&[]);
    }
    let r1 = block_on(c.st.get(&schema::route_key(&pk(UNCLE_MINER)))).unwrap().map(|b| schema::Route::decode(&b).unwrap()).unwrap();
    assert_eq!(r1.routed - r0.routed, reward / 2, "half of the matured uncle reward went to the lien");
}

#[test]
fn admin_key_45_activates_at_the_speed_height() {
    let s = 4u64;
    let mut c = Chain::new(TEST_SPEED_CHAIN_BASE + s as u32, 30);
    let t = c.unlock(1, pk(ADMIN), 1_000_000);
    c.block(&[t]);
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::EMISSION_PPB, value: 2_000u64.to_le_bytes().to_vec() });
    let env = BlockEnv { chain_id: c.chain_id, height: c.height + 1, miner: pk(MINER), job_id: None };
    let e = block_on(executor::execute_block(&c.st, env, &[t])).unwrap_err().to_string();
    assert!(e.contains("activates at height 4"), "{e}");
    *c.nonces.get_mut(&ADMIN).unwrap() -= 1;
    while c.height < s {
        c.block(&[]);
    }
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::EMISSION_PPB, value: 2_000u64.to_le_bytes().to_vec() });
    c.block(&[t]);
    assert_eq!(c.cfg().emission_ppb, 2_000);
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: 46, value: vec![0] });
    let env = BlockEnv { chain_id: c.chain_id, height: c.height + 1, miner: pk(MINER), job_id: None };
    assert!(block_on(executor::execute_block(&c.st, env, &[t])).is_err(), "46 is not a config key");
}
