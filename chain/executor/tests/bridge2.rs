//! Two-side bridge (docs/BRIDGE-V2.md): BridgeUnlockV2 / BridgeLockV2 / SetRouteV2, activation
//! gating, replay per domain, per-domain counters, the Solana cap from activation.

use agent_types::bridge2::*;
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, supply_of, BlockEnv, BlockSummary, ExecError};
use futures::executor::block_on;
use state::bridge2::{self as b2, BridgeParams, DomainCfg, DomainStats};
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const CHAIN: u32 = 91;
const ADMIN: u8 = 1;
const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const RELAYER: u8 = 200;
const BRIDGE: [u8; 20] = [0x5a; 20];

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}
fn evm(i: u8) -> Pubkey {
    let mut r = [0u8; 32];
    r[12..].copy_from_slice(&[i; 20]);
    r
}

struct Chain {
    st: MemState,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
    log: Vec<Vec<u8>>,
}

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, height: 0, nonces: Default::default(), log: Vec::new() };
        c.fund(ADMIN, 1_000_000);
        c
    }

    fn tx(&mut self, who: u8, fee: u64, body: TxBody) -> Tx {
        let n = self.nonces.entry(who).or_insert(0);
        let t = Tx::signed(&seed(who), CHAIN, *n, fee, body);
        *n += 1;
        t
    }

    /// Run a block; on failure the senders' nonces are handed back.
    fn block(&mut self, txs: &[Tx]) -> Result<BlockSummary, ExecError> {
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(MINER), job_id: None };
        match block_on(execute_block(&self.st, env, txs)) {
            Ok((ws, sum)) => {
                self.st.apply(&ws);
                self.log.extend(ws.withdrawals.iter().cloned());
                self.height += 1;
                assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
                Ok(sum)
            }
            Err(e) => {
                for t in txs {
                    let who = self.nonces.iter().find(|(k, _)| pk(**k) == t.from).map(|(k, _)| *k);
                    if let Some(w) = who {
                        *self.nonces.get_mut(&w).unwrap() -= 1;
                    }
                }
                Err(e)
            }
        }
    }

    fn one(&mut self, t: Tx) -> Result<BlockSummary, String> {
        self.block(&[t]).map_err(|e| e.to_string())
    }

    fn bal(&self, a: &Pubkey) -> u64 {
        block_on(self.st.get(&schema::account_key(a))).unwrap().map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }

    fn escrow(&self) -> u64 {
        self.bal(&escrow_address())
    }

    fn stats(&self, d: u8) -> DomainStats {
        block_on(self.st.get(&b2::domain_stats_key(d))).unwrap().map(|b| DomainStats::decode(&b).unwrap()).unwrap_or_default()
    }

    fn params(&self) -> BridgeParams {
        block_on(self.st.get(&b2::params_key())).unwrap().map(|b| BridgeParams::decode(&b).unwrap()).unwrap_or_default()
    }

    fn sigs(msg: &[u8]) -> Vec<AttesterSig> {
        ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), msg) }).collect()
    }

    fn unlock_v1(&mut self, nonce: u64, to: Pubkey, amount_spl: u64) -> Tx {
        let sigs = Self::sigs(&deposit_message(nonce, &to, amount_spl));
        self.tx(RELAYER, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: to, amount_spl, job_order: None, sigs })
    }

    fn unlock_v2(&mut self, domain: u8, nonce: u64, to: Pubkey, amount_ext: u64) -> Tx {
        let sigs = Self::sigs(&deposit_message_v2(domain, RH_TESTNET_CHAIN_ID, &BRIDGE, nonce, &to, amount_ext));
        self.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain, ext_nonce: nonce, recipient: to, amount_ext, job_order: None, sigs })
    }

    fn fund(&mut self, who: u8, amount_spl: u64) {
        let n = 1_000_000 + self.height * 7 + who as u64;
        let t = self.unlock_v1(n, pk(who), amount_spl);
        self.block(&[t]).unwrap();
    }

    fn admin(&mut self, key: u8, value: Vec<u8>) -> Result<BlockSummary, String> {
        let t = self.tx(ADMIN, 1000, TxBody::AdminConfig { key, value });
        self.one(t)
    }

    fn rh_domain(&mut self) {
        let d = DomainCfg { domain: DOMAIN_RH, ext_chain_id: RH_TESTNET_CHAIN_ID, bridge_addr: BRIDGE.to_vec(), paused: false };
        self.admin(b2::keys::BRIDGE_DOMAIN, d.encode_value()).unwrap();
    }

    /// Configure domain 2 and activate v2 at the next block.
    fn activate(&mut self) {
        self.rh_domain();
        let h = self.height + 2;
        self.admin(b2::keys::BRIDGE_V2_HEIGHT, h.to_le_bytes().to_vec()).unwrap();
        assert!(self.height + 1 >= h);
    }
}

#[test]
fn v2_kinds_refused_before_activation() {
    let mut c = Chain::new();
    c.fund(2, 10_000_000);
    // nothing configured: every v2 kind is refused, and no bridge2 record exists
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(3), 100);
    assert!(c.one(t).unwrap_err().contains("not active"));
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(1), amount: 1000 });
    assert!(c.one(t).unwrap_err().contains("not active"));
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(1), route_bps: 10, until_height: 99, cap: 9, release_key: [0; 32] });
    assert!(c.one(t).unwrap_err().contains("not active"));
    assert!(block_on(c.st.get(&b2::params_key())).unwrap().is_none());
    // a configured domain with a scheduled (future) height still refuses them
    c.rh_domain();
    let at = c.height + 10;
    c.admin(b2::keys::BRIDGE_V2_HEIGHT, at.to_le_bytes().to_vec()).unwrap();
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(3), 100);
    assert!(c.one(t).unwrap_err().contains(&format!("activate at height {at}")));
    // height rules: not in the past, not the current block; u64::MAX cancels while inactive
    let h = c.height + 1;
    assert!(c.admin(b2::keys::BRIDGE_V2_HEIGHT, h.to_le_bytes().to_vec()).unwrap_err().contains("above the current height"));
    assert!(c.admin(b2::keys::BRIDGE_V2_HEIGHT, vec![1, 2]).unwrap_err().contains("expected u64"));
    assert!(c.admin(52, vec![0]).unwrap_err().contains("unknown config key"));
    let t = c.tx(2, 1000, TxBody::AdminConfig { key: b2::keys::BRIDGE_V2_HEIGHT, value: (c.height + 50).to_le_bytes().to_vec() });
    assert!(c.one(t).unwrap_err().contains("not the admin"));
    c.admin(b2::keys::BRIDGE_V2_HEIGHT, u64::MAX.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.params().activation_height, u64::MAX);
    // reaching the height activates; it can no longer change
    let at = c.height + 3;
    c.admin(b2::keys::BRIDGE_V2_HEIGHT, at.to_le_bytes().to_vec()).unwrap();
    while c.height + 1 < at {
        let t = c.unlock_v2(DOMAIN_RH, 0, pk(3), 100);
        assert!(c.one(t).is_err());
        c.block(&[]).unwrap();
    }
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(3), 100);
    c.one(t).unwrap();
    assert_eq!(c.bal(&pk(3)), 100_000);
    let later = c.height + 100;
    assert!(c.admin(b2::keys::BRIDGE_V2_HEIGHT, later.to_le_bytes().to_vec()).unwrap_err().contains("no longer change"));
    assert!(c.admin(b2::keys::BRIDGE_V2_HEIGHT, u64::MAX.to_le_bytes().to_vec()).unwrap_err().contains("no longer change"));
}

#[test]
fn unlock_replay_is_per_domain() {
    let mut c = Chain::new();
    c.activate();
    let esc0 = c.escrow();
    // nonce 5 on Solana (kind 6) and on domain 2 (kind 40) are different deposits
    let t = c.unlock_v1(5, pk(3), 1000);
    c.one(t).unwrap();
    let t = c.unlock_v2(DOMAIN_RH, 5, pk(3), 2000);
    let s = c.one(t).unwrap();
    assert_eq!(s.deposits_v2, vec![(DOMAIN_RH, 5)]);
    assert_eq!(c.bal(&pk(3)), 3_000_000);
    assert_eq!(c.escrow(), esc0 - 3_000_000);
    // replays of either are refused
    let t = c.unlock_v2(DOMAIN_RH, 5, pk(3), 2000);
    assert!(c.one(t).unwrap_err().contains("already processed"));
    let t = c.unlock_v1(5, pk(3), 1000);
    assert!(c.one(t).unwrap_err().contains("already processed"));
    // a signature for domain 2 does not serve on another domain, and domain 1 is not a v2 domain
    let sigs = Chain::sigs(&deposit_message_v2(DOMAIN_RH, RH_TESTNET_CHAIN_ID, &BRIDGE, 6, &pk(3), 10));
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: 3, ext_nonce: 6, recipient: pk(3), amount_ext: 10, job_order: None, sigs: sigs.clone() });
    assert!(c.one(t).unwrap_err().contains("not configured"));
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: DOMAIN_SOLANA, ext_nonce: 6, recipient: pk(3), amount_ext: 10, job_order: None, sigs });
    assert!(c.one(t).unwrap_err().contains("domain 1"));
    // a v1 message signed for nonce 7 does not unlock on domain 2
    let sigs = Chain::sigs(&deposit_message(7, &pk(3), 10));
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: DOMAIN_RH, ext_nonce: 7, recipient: pk(3), amount_ext: 10, job_order: None, sigs });
    assert!(c.one(t).unwrap_err().contains("signature"));
    // one signature is below the threshold
    let msg = deposit_message_v2(DOMAIN_RH, RH_TESTNET_CHAIN_ID, &BRIDGE, 8, &pk(3), 10);
    let one = vec![AttesterSig { pubkey: pk(50), sig: sign_plain(&seed(50), &msg) }];
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: DOMAIN_RH, ext_nonce: 8, recipient: pk(3), amount_ext: 10, job_order: None, sigs: one });
    assert!(c.one(t).unwrap_err().contains("required signatures"));
    // the unlock pays no fee, like kind 6
    assert_eq!(c.bal(&pk(RELAYER)), 0);
    let st = c.stats(DOMAIN_RH);
    assert_eq!((st.unlocked, st.locked, st.deposit_count, st.withdrawal_count), (2_000_000, 0, 1, 0));
    // a changed bridge address changes the message (old signatures stop working)
    let d = DomainCfg { domain: DOMAIN_RH, ext_chain_id: RH_TESTNET_CHAIN_ID, bridge_addr: vec![0x5b; 20], paused: false };
    c.admin(b2::keys::BRIDGE_DOMAIN, d.encode_value()).unwrap();
    let t = c.unlock_v2(DOMAIN_RH, 9, pk(3), 10);
    assert!(c.one(t).unwrap_err().contains("signature"));
    // a paused domain refuses unlocks
    let d = DomainCfg { domain: DOMAIN_RH, ext_chain_id: RH_TESTNET_CHAIN_ID, bridge_addr: BRIDGE.to_vec(), paused: true };
    c.admin(b2::keys::BRIDGE_DOMAIN, d.encode_value()).unwrap();
    let t = c.unlock_v2(DOMAIN_RH, 9, pk(3), 10);
    assert!(c.one(t).unwrap_err().contains("paused"));
}

#[test]
fn lock_v2_counters_and_log_order() {
    let mut c = Chain::new();
    c.activate();
    c.fund(2, 50_000_000); // Solana-backed coins
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(3), 4_000_000); // 4e9 RH-backed coins to pk(3)
    c.one(t).unwrap();
    // pk(2) holds only Solana-backed coins, but coins are fungible: the cap is per vault
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(7), amount: 4_000_000_001 });
    assert!(c.one(t).unwrap_err().contains("multiple of 1000"));
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(7), amount: 4_000_001_000 });
    assert!(c.one(t).unwrap_err().contains("outstanding"), "more than domain 2 ever unlocked");
    let mut bad = evm(7);
    bad[0] = 1;
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: bad, amount: 1000 });
    assert!(c.one(t).unwrap_err().contains("left-padded"));
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: 3, recipient: evm(7), amount: 1000 });
    assert!(c.one(t).unwrap_err().contains("not configured"));
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_SOLANA, recipient: evm(7), amount: 1000 });
    assert!(c.one(t).unwrap_err().contains("domain 1"));
    // one block: a v1 lock, then two v2 locks; the log holds v1 first, then v2 in tx order
    let before = c.log.len();
    let a = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(7), amount: 1_000_000_000 });
    let b = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: 2_000_000 });
    let d = c.tx(3, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(8), amount: 3_000_000_000 });
    let s = c.block(&[a, b, d]).unwrap();
    assert_eq!(s.withdrawals.len(), 1);
    assert_eq!(s.withdrawals_v2.len(), 2);
    assert_eq!((s.withdrawals_v2[0].nonce, s.withdrawals_v2[1].nonce), (0, 1), "per-domain nonces from 0");
    assert_eq!(s.withdrawals_v2[0].amount_ext, 1_000_000);
    assert_eq!(s.withdrawals_v2[1].recipient, evm(8));
    let log = &c.log[before..];
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].len(), 88);
    assert_eq!(schema::Withdrawal::from_log_entry(&log[0]).unwrap().amount_spl, 2000);
    assert_eq!(WithdrawalV2::from_log_entry(&log[1]).unwrap(), s.withdrawals_v2[0]);
    assert_eq!(WithdrawalV2::from_log_entry(&log[2]).unwrap(), s.withdrawals_v2[1]);
    // records and counters
    let w1 = b2::decode_withdrawal_v2(&block_on(c.st.get(&b2::withdrawal_v2_key(DOMAIN_RH, 1))).unwrap().unwrap()).unwrap();
    assert_eq!(w1, s.withdrawals_v2[1]);
    let st = c.stats(DOMAIN_RH);
    assert_eq!((st.unlocked, st.locked, st.outstanding(), st.withdrawal_count), (4_000_000_000, 4_000_000_000, 0, 2));
    // domain 2 is drained: nothing more goes out there
    let t = c.tx(2, 1000, TxBody::BridgeLockV2 { domain: DOMAIN_RH, recipient: evm(7), amount: 1000 });
    assert!(c.one(t).unwrap_err().contains("outstanding 0"));
    // Solana keeps working with v1 entries
    let t = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: 1000 });
    let s = c.one(t).unwrap();
    assert_eq!((s.withdrawals.len(), s.withdrawals_v2.len()), (1, 0));
}

#[test]
fn solana_lock_capped_from_activation() {
    let mut c = Chain::new();
    c.activate();
    // circulating: pk(ADMIN)'s funding (Solana) + 6e9 from domain 2
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(4), 6_000_000);
    c.one(t).unwrap();
    let circulating = TOTAL_SUPPLY - c.escrow();
    let sol = circulating - 6_000_000_000;
    // pk(4) holds 6e9 that came from domain 2; Solana's vault holds only `sol`
    assert!(sol < 6_000_000_000);
    let too_much = (sol / 1000 + 1) * 1000;
    let t = c.tx(4, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: too_much });
    assert!(c.one(t).unwrap_err().contains("domain 1 outstanding"));
    // up to the Solana outstanding (the fee comes first and stays circulating) is fine
    let ok = sol / 1000 * 1000;
    let t = c.tx(4, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: ok });
    c.one(t).unwrap();
}

#[test]
fn v1_lock_unchanged_without_other_sides() {
    // activated, no other side has unlocked anything: every lock a sender can fund passes
    let mut c = Chain::new();
    c.activate();
    c.fund(2, 3_000_000);
    let all = c.bal(&pk(2)) - 1000;
    let t = c.tx(2, 1000, TxBody::BridgeLock { solana_recipient: [8; 32], amount: all / 1000 * 1000 });
    c.one(t).unwrap();
}

#[test]
fn order_unlock_v2_creates_domain_tagged_job() {
    let mut c = Chain::new();
    c.activate();
    let cfg = pearl_job::MiningConfig::localnet_default();
    let body = PostJob {
        m: 64,
        n: 64,
        k: cfg.k() as u32,
        config: cfg.to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: b"ipfs://x".to_vec(),
        worker_pay: 1_000_000_000,
        protocol_fee: 500_000_000,
        deadline_height: 50,
        max_blocks: 3,
        evaluators: vec![pk(60)],
        threshold: 1,
    };
    let amount_ext = 2_000_000;
    let msg = order_message_v2(DOMAIN_RH, RH_TESTNET_CHAIN_ID, &BRIDGE, 12, &pk(6), amount_ext, &body);
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: DOMAIN_RH, ext_nonce: 12, recipient: pk(6), amount_ext, job_order: Some(body.clone()), sigs: Chain::sigs(&msg) });
    let s = c.one(t).unwrap();
    let id = order_job_id_v2(DOMAIN_RH, 12);
    assert_eq!(s.jobs_created, vec![id]);
    let j = schema::JobRecord::decode(&block_on(c.st.get(&schema::job_key(&id))).unwrap().unwrap()).unwrap();
    assert_eq!(j.poster, pk(6));
    assert_eq!(j.header_id, pk(6));
    assert_eq!(j.deadline_height, c.height + 50, "relative deadline, like a v1 order");
    assert_eq!(c.bal(&pk(6)), 2_000_000_000 - 1_500_000_000);
    // the plain message does not authorize the order
    let msg = deposit_message_v2(DOMAIN_RH, RH_TESTNET_CHAIN_ID, &BRIDGE, 13, &pk(6), amount_ext);
    let t = c.tx(RELAYER, 0, TxBody::BridgeUnlockV2 { domain: DOMAIN_RH, ext_nonce: 13, recipient: pk(6), amount_ext, job_order: Some(body), sigs: Chain::sigs(&msg) });
    assert!(c.one(t).unwrap_err().contains("signature"));
}

#[test]
fn lien_to_domain_two() {
    let mut c = Chain::new();
    c.activate();
    c.fund(MINER, 1_000_000);
    // the miner's lien pays 100% of its income to an EVM address on domain 2
    let set = c.tx(MINER, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(9), route_bps: 10_000, until_height: c.height + 200, cap: u64::MAX, release_key: pk(70) });
    c.one(set).unwrap();
    assert_eq!(block_on(c.st.get(&b2::route_domain_key(&pk(MINER)))).unwrap(), Some(b2::encode_route_domain(DOMAIN_RH)));
    // a v1 SetRoute may not "extend" it
    let t = c.tx(MINER, 1000, TxBody::SetRoute { solana_recipient: evm(9), route_bps: 10_000, until_height: c.height + 300, cap: u64::MAX, release_key: pk(70) });
    assert!(c.one(t).unwrap_err().contains("SetRouteV2"));
    // income matures while domain 2 has no outstanding: the lien holds it
    c.fund(2, 1_000_000);
    let mut v2 = Vec::new();
    for _ in 0..4 {
        let t = c.tx(2, 5_000_000, TxBody::Transfer { to: pk(3), amount: 1 });
        let s = c.block(&[t]).unwrap();
        v2.extend(s.withdrawals_v2);
        assert!(s.withdrawals.is_empty(), "a domain 2 lien never writes v1 entries");
    }
    let r = schema::Route::decode(&block_on(c.st.get(&schema::route_key(&pk(MINER)))).unwrap().unwrap()).unwrap();
    assert!(v2.is_empty());
    assert!(r.pending >= 1000, "held while domain 2 outstanding is 0");
    // once domain 2 has unlocked enough, the next income pays the lien out as v2 entries
    let t = c.unlock_v2(DOMAIN_RH, 0, pk(5), 1_000_000_000);
    c.one(t).unwrap();
    for _ in 0..4 {
        let t = c.tx(2, 5_000_000, TxBody::Transfer { to: pk(3), amount: 1 });
        let s = c.block(&[t]).unwrap();
        v2.extend(s.withdrawals_v2);
    }
    assert!(!v2.is_empty());
    assert!(v2.iter().all(|w| w.domain == DOMAIN_RH && w.recipient == evm(9) && w.from == pk(MINER)));
    let st = c.stats(DOMAIN_RH);
    assert_eq!(st.locked, v2.iter().map(|w| w.amount_ext * 1000).sum::<u64>());
    assert_eq!(st.withdrawal_count, v2.len() as u64);
    // ClearRoute ends it; a new v1 SetRoute replaces it and drops the domain record
    let cmsg = agent_types::tx::route_clear_message(CHAIN, &pk(MINER), r.route_nonce);
    let clr = c.tx(4, 0, TxBody::ClearRoute { account: pk(MINER), route_nonce: r.route_nonce, sig: sign_plain(&seed(70), &cmsg) });
    c.fund(4, 10_000);
    let clr = Tx::signed(&seed(4), CHAIN, clr.nonce, 1000, clr.body);
    c.one(clr).unwrap();
    let t = c.tx(MINER, 1000, TxBody::SetRoute { solana_recipient: [0x4c; 32], route_bps: 100, until_height: c.height + 50, cap: 10, release_key: [0; 32] });
    c.one(t).unwrap();
    assert!(block_on(c.st.get(&b2::route_domain_key(&pk(MINER)))).unwrap().is_none());
}

#[test]
fn set_route_v2_rules() {
    let mut c = Chain::new();
    c.activate();
    c.fund(2, 1_000_000);
    let h = c.height;
    let mut bad = evm(1);
    bad[3] = 9;
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: bad, route_bps: 10, until_height: h + 9, cap: 9, release_key: [0; 32] });
    assert!(c.one(t).unwrap_err().contains("left-padded"));
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_SOLANA, recipient: evm(1), route_bps: 10, until_height: h + 9, cap: 9, release_key: [0; 32] });
    assert!(c.one(t).unwrap_err().contains("domain 1"));
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(1), route_bps: 0, until_height: h + 9, cap: 9, release_key: [0; 32] });
    assert!(c.one(t).unwrap_err().contains("route_bps"));
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(1), route_bps: 10, until_height: h + 9, cap: 9, release_key: [0; 32] });
    c.one(t).unwrap();
    // extension only, same recipient
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(2), route_bps: 10, until_height: h + 9, cap: 9, release_key: [0; 32] });
    assert!(c.one(t).unwrap_err().contains("extension"));
    let t = c.tx(2, 1000, TxBody::SetRouteV2 { domain: DOMAIN_RH, recipient: evm(1), route_bps: 20, until_height: h + 19, cap: 19, release_key: [0; 32] });
    c.one(t).unwrap();
}
