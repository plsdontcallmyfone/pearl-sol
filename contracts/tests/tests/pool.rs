//! Paying the mining pool from a contract (docs/CONTRACTS.md 6, docs/MARKET.md 3): the services
//! market's fee lands in the pool when fee_to is the pool, a deployed v1 market migrates to it
//! with its offers and open hires intact, no contract path takes CADRE out of the pool, the supply
//! invariant holds after every block, and a pool credit on a dropped branch is undone by a reorg.
#![allow(dead_code)]

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const MARKET: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_market.wasm");
const PROBE: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_poolprobe.wasm");
/// The market code deployed on Cadre devnet at height 4845 (getCode, code hash 1647ce24...), before
/// the fee could go to the pool: the Migrate test starts from it.
const MARKET_V1: &[u8] = include_bytes!("../fixtures/cadre_market_v1.wasm");

const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const ADMIN: u8 = 1;
const PROVIDER: u8 = 2;
const BUYER: u8 = 3;
const JUDGE: u8 = 4;
const JUDGE2: u8 = 5;
const JUDGE3: u8 = 6;
const FEE_TO: u8 = 8;
const STRANGER: u8 = 10;
const CADRE: u64 = 1_000_000_000;
const CHAIN_ID: u32 = 7;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

struct In(Vec<u8>);
impl In {
    fn m(name: &str) -> Self {
        let mut v = vec![name.len() as u8];
        v.extend_from_slice(name.as_bytes());
        Self(v)
    }
    fn raw() -> Self {
        Self(Vec::new())
    }
    fn u8(mut self, x: u8) -> Self {
        self.0.push(x);
        self
    }
    fn u16(mut self, x: u16) -> Self {
        self.0.extend_from_slice(&x.to_le_bytes());
        self
    }
    fn u64(mut self, x: u64) -> Self {
        self.0.extend_from_slice(&x.to_le_bytes());
        self
    }
    fn bool(self, b: bool) -> Self {
        self.u8(b as u8)
    }
    fn addr(mut self, a: &[u8; 32]) -> Self {
        self.0.extend_from_slice(a);
        self
    }
    fn bytes(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(&(b.len() as u32).to_le_bytes());
        self.0.extend_from_slice(b);
        self
    }
    fn str(self, s: &str) -> Self {
        self.bytes(s.as_bytes())
    }
    fn evals(mut self, v: &[Pubkey]) -> Self {
        self = self.u8(v.len() as u8);
        for a in v {
            self = self.addr(a);
        }
        self
    }
    fn b(self) -> Vec<u8> {
        self.0
    }
}

struct Chain {
    st: MemState,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, height: 0, nonces: Default::default() };
        for who in [ADMIN, PROVIDER, BUYER, JUDGE, JUDGE2, JUDGE3, STRANGER] {
            c.fund(pk(who), 10_000 * CADRE);
        }
        c
    }
    fn block(&mut self, txs: &[Tx]) -> Result<BlockSummary, String> {
        let env = BlockEnv { chain_id: CHAIN_ID, height: self.height + 1, miner: pk(MINER), job_id: None };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }
    fn skip(&mut self, n: u64) {
        for _ in 0..n {
            self.block(&[]).unwrap();
        }
    }
    fn fund(&mut self, who: Pubkey, amount: u64) {
        let nonce = 10_000 + self.height;
        let msg = deposit_message(nonce, &who, amount);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = *self.nonces.get(&200).unwrap_or(&0);
        let t = Tx::signed(&seed(200), CHAIN_ID, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl: amount, job_order: None, sigs });
        self.block(&[t]).unwrap();
        self.nonces.insert(200, n + 1);
    }
    fn cfg(&self) -> Config {
        Config::decode(&self.get(&schema::config_key()).unwrap()).unwrap()
    }
    fn get(&self, k: &schema::Key) -> Option<Vec<u8>> {
        block_on(self.st.get(k)).unwrap()
    }
    fn bal(&self, a: &Pubkey) -> u64 {
        self.get(&schema::account_key(a)).map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }
    fn nonce(&self, who: u8) -> u64 {
        *self.nonces.get(&who).unwrap_or(&0)
    }
    fn send(&mut self, who: u8, body: TxBody) -> BlockSummary {
        let t0 = Tx::signed(&seed(who), CHAIN_ID, self.nonce(who), 0, body.clone());
        let fee = contract_fee_floor(&self.cfg(), t0.encode().len(), executor::gas_limit_of(&body));
        let t = Tx::signed(&seed(who), CHAIN_ID, self.nonce(who), fee, body);
        let s = self.block(&[t]).unwrap();
        *self.nonces.entry(who).or_insert(0) += 1;
        s
    }
    fn deploy(&mut self, who: u8, code: &[u8]) -> Hash {
        let gas = ::contracts::deploy_cost(code.len());
        let s = self.send(who, TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code: code.to_vec() }));
        assert_eq!(s.receipts[0].status, Status::Ok);
        eprintln!("gas: market deploy {} bytes {} gas, fee {}", code.len(), s.receipts[0].gas_used, s.receipts[0].fee_charged);
        ac::code_hash(code)
    }
    fn instantiate(&mut self, who: u8, code_hash: Hash, args: Vec<u8>) -> Pubkey {
        let addr = contract_address(&pk(who), self.nonce(who));
        let s = self.send(who, TxBody::Instantiate(ac::Instantiate { code_hash, admin: [0; 32], value: 0, gas_limit: 50_000_000, label: b"market".to_vec(), args }));
        let r = &s.receipts[0];
        assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
        addr
    }
    fn call(&mut self, who: u8, contract: Pubkey, value: u64, args: Vec<u8>) -> (ac::Receipt, BlockSummary) {
        let s = self.send(who, TxBody::Call(ac::Call { contract, value, gas_limit: 50_000_000, args }));
        (s.receipts[0].clone(), s)
    }
    fn ok(&mut self, who: u8, contract: Pubkey, value: u64, args: Vec<u8>) -> (ac::Receipt, BlockSummary) {
        let (r, s) = self.call(who, contract, value, args);
        assert_eq!(r.status, Status::Ok, "expected ok: {}", String::from_utf8_lossy(&r.error));
        (r, s)
    }
    fn fails(&mut self, who: u8, contract: Pubkey, value: u64, args: Vec<u8>, want: &str) {
        let before = self.bal(&contract);
        let payer_before = self.bal(&pk(who));
        let (r, s) = self.call(who, contract, value, args);
        assert_ne!(r.status, Status::Ok, "expected a failure ({want})");
        let e = String::from_utf8_lossy(&r.error).to_string();
        assert!(e.contains(want), "error {e:?} should contain {want:?}");
        assert!(s.events.is_empty(), "failed call kept no events");
        assert_eq!(self.bal(&contract), before, "failed call moved no CADRE into the contract");
        assert_eq!(payer_before - self.bal(&pk(who)), r.fee_charged, "a failed call costs only its fee");
    }
    fn q(&self, contract: &Pubkey, args: Vec<u8>) -> Vec<u8> {
        let r = block_on(query_contract(&self.st, CHAIN_ID, self.height + 1, contract, args, 50_000_000)).unwrap();
        assert!(r.ok, "{}", String::from_utf8_lossy(&r.error));
        r.ret
    }
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// The fields of a hire record a test looks at (layout in contracts/market `Hire::write`).
#[derive(Debug)]
struct HireView {
    buyer: Pubkey,
    provider: Pubkey,
    price: u64,
    fee: u64,
    judge_fee: u64,
    deadline: u64,
    status: u8,
    accepts: u8,
    rejects: u8,
    result_hash: Hash,
}
fn hire_view(b: &[u8]) -> HireView {
    let a32 = |o: usize| -> [u8; 32] { b[o..o + 32].try_into().unwrap() };
    let mut o = 8;
    let buyer = a32(o);
    o += 32;
    let provider = a32(o);
    o += 32;
    let price = u64_at(b, o);
    let fee = u64_at(b, o + 8);
    let judge_fee = u64_at(b, o + 16);
    let deadline = u64_at(b, o + 32);
    o += 40;
    let ulen = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    o += 4 + ulen + 32;
    let n = b[o] as usize;
    o += 1 + 32 * n;
    let status = b[o + 1];
    let accepts = b[o + 2];
    let rejects = b[o + 3];
    o += 6;
    HireView { buyer, provider, price, fee, judge_fee, deadline, status, accepts, rejects, result_hash: a32(o) }
}

const OPEN: u8 = 0;
const DELIVERED: u8 = 1;
const COMPLETED: u8 = 2;
const REJECTED: u8 = 3;
const EXPIRED: u8 = 4;
const DECLINED: u8 = 5;

struct M {
    addr: Pubkey,
}

fn setup(c: &mut Chain, bps: u16) -> M {
    let code = c.deploy(ADMIN, MARKET);
    let addr = c.instantiate(ADMIN, code, In::raw().addr(&pk(ADMIN)).u16(bps).addr(&pk(FEE_TO)).b());
    M { addr }
}

fn offer_args(m: &str, price: u64, turnaround: u64, policy: u8, evals: &[Pubkey]) -> In {
    In::m(m)
        .str("Summarise a document")
        .str("writing")
        .str("blob:0000000000000000000000000000000000000000000000000000000000000001")
        .addr(&[7; 32])
        .u64(price)
        .u64(turnaround)
        .u8(policy)
        .evals(evals)
        .bool(true)
}

fn list(c: &mut Chain, m: &M, price: u64, turnaround: u64) -> u64 {
    let (r, s) = c.ok(PROVIDER, m.addr, 0, offer_args("offer", price, turnaround, 0, &[]).b());
    eprintln!("gas: market offer {} (fee {})", r.gas_used, r.fee_charged);
    assert!(s.events.iter().any(|e| e.topics[0] == ac::event_topic(b"OfferListed") && e.topics[2] == pk(PROVIDER)));
    u64_at(&r.ret, 0)
}

fn hire_args(offer: u64, price: u64, deadline: u64, judge_fee: u64, evals: &[Pubkey], threshold: u8) -> Vec<u8> {
    In::m("hire")
        .u64(offer)
        .u64(price)
        .str("blob:00000000000000000000000000000000000000000000000000000000000000aa")
        .addr(&[0xaa; 32])
        .u64(deadline)
        .u64(judge_fee)
        .evals(evals)
        .u8(threshold)
        .b()
}

fn quote(c: &Chain, m: &M, offer: u64, judge_fee: u64) -> (u64, u64, u64) {
    let q = c.q(&m.addr, In::m("quote").u64(offer).u64(judge_fee).b());
    (u64_at(&q, 0), u64_at(&q, 8), u64_at(&q, 24))
}

fn hire(c: &mut Chain, m: &M, offer: u64, judge_fee: u64, evals: &[Pubkey], threshold: u8) -> (u64, u64) {
    let (price, _, total) = quote(c, m, offer, judge_fee);
    let (r, s) = c.ok(BUYER, m.addr, total, hire_args(offer, price, 20, judge_fee, evals, threshold));
    eprintln!("gas: market hire {} (fee {})", r.gas_used, r.fee_charged);
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Hired")).unwrap();
    assert_eq!((ev.topics[2], ev.topics[3]), (pk(BUYER), pk(PROVIDER)));
    (u64_at(&r.ret, 0), total)
}

fn deliver(c: &mut Chain, m: &M, id: u64) {
    let (r, _) = c.ok(PROVIDER, m.addr, 0, In::m("deliver").u64(id).addr(&[0xbb; 32]).str("blob:00000000000000000000000000000000000000000000000000000000000000bb").str("req-123").b());
    eprintln!("gas: market deliver {} (fee {})", r.gas_used, r.fee_charged);
}

fn verdict(who: u8, id: u64, accept: bool) -> Vec<u8> {
    In::m("verdict").u64(id).bool(accept).str("blob:00000000000000000000000000000000000000000000000000000000000000cc").addr(&[0xcc; 32]).str("judge-req-9").b()
}

fn view(c: &Chain, m: &M, id: u64) -> HireView {
    hire_view(&c.q(&m.addr, In::m("hire").u64(id).b()))
}


fn pool() -> Pubkey {
    pool_address()
}

fn setup_to(c: &mut Chain, code: &[u8], bps: u16, fee_to: &Pubkey, admin: Pubkey) -> M {
    let h = c.deploy(ADMIN, code);
    let addr = contract_address(&pk(ADMIN), c.nonce(ADMIN));
    let s = c.send(ADMIN, TxBody::Instantiate(ac::Instantiate { code_hash: h, admin, value: 0, gas_limit: 50_000_000, label: b"market".to_vec(), args: In::raw().addr(&pk(ADMIN)).u16(bps).addr(fee_to).b() }));
    assert_eq!(s.receipts[0].status, Status::Ok, "{}", String::from_utf8_lossy(&s.receipts[0].error));
    M { addr }
}

fn complete_one(c: &mut Chain, m: &M, price: u64) -> (u64, u64) {
    let offer = list(c, m, price, 10);
    let (_, fee, _) = quote(c, m, offer, 0);
    let (id, _) = hire(c, m, offer, 0, &[pk(JUDGE)], 1);
    deliver(c, m, id);
    (id, fee)
}

#[test]
fn sdk_pool_address_matches_the_chain() {
    // the SDK computes blake3("pearl-sol:mining-pool:v1") in the contract; the probe pays it and
    // the chain's own pool account is the one credited
    let mut c = Chain::new();
    let h = c.deploy(ADMIN, PROBE);
    let probe = c.instantiate(ADMIN, h, Vec::new());
    c.fund(probe, 5 * CADRE);
    let (p0, b0) = (c.bal(&pool()), c.bal(&probe));
    let (r, _) = c.ok(STRANGER, probe, 0, In::m("pay_pool").u64(2 * CADRE).b());
    eprintln!("gas: pay_mining_pool {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(c.bal(&pool()) - p0, 2 * CADRE);
    assert_eq!(b0 - c.bal(&probe), 2 * CADRE, "only the contract's own balance paid");
}

#[test]
fn market_fee_lands_in_the_mining_pool() {
    let mut c = Chain::new();
    let m = setup_to(&mut c, MARKET, 2_000, &pool(), [0; 32]);
    let p0 = c.bal(&pool());
    let price = 50 * CADRE;
    let (id, fee) = complete_one(&mut c, &m, price);
    assert_eq!(fee, 10 * CADRE);
    assert_eq!(c.bal(&pool()), p0, "nothing reaches the pool before the hire completes");
    let prov = c.bal(&pk(PROVIDER));
    let (r, s) = c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    eprintln!("gas: market verdict accept + payout to the pool {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(view(&c, &m, id).status, COMPLETED);
    assert_eq!(c.bal(&pool()) - p0, fee, "the protocol fee is in the mining pool");
    assert_eq!(c.bal(&pk(PROVIDER)) - prov, price);
    assert_eq!(c.bal(&m.addr), 0);
    let done = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Completed")).unwrap();
    assert_eq!(&done.data[done.data.len() - 32..], &pool(), "Completed names the pool as fee_to");
    // a rejected hire refunds price + fee: the pool gets nothing
    let offer = list(&mut c, &m, price, 10);
    let (id2, total) = hire(&mut c, &m, offer, 0, &[pk(JUDGE)], 1);
    deliver(&mut c, &m, id2);
    let buyer = c.bal(&pk(BUYER));
    c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id2, false));
    assert_eq!(c.bal(&pk(BUYER)) - buyer, total);
    assert_eq!(c.bal(&pool()) - p0, fee, "a reject leaves the pool as it was");
}

#[test]
fn fee_to_can_be_switched_to_the_pool_and_back_but_not_to_the_escrow() {
    let mut c = Chain::new();
    let m = setup_to(&mut c, MARKET, 2_000, &pk(FEE_TO), [0; 32]);
    let price = 10 * CADRE;
    let (id, fee) = complete_one(&mut c, &m, price);
    let (p0, f0) = (c.bal(&pool()), c.bal(&pk(FEE_TO)));
    c.ok(ADMIN, m.addr, 0, In::m("set_config").addr(&pk(ADMIN)).u16(2_000).addr(&pool()).bool(false).b());
    let cfgq = c.q(&m.addr, In::m("config").b());
    assert_eq!(&cfgq[34..66], &pool());
    // the open hire completes under the new config: its fee goes to the pool
    c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!((c.bal(&pool()) - p0, c.bal(&pk(FEE_TO)) - f0), (fee, 0));
    c.fails(STRANGER, m.addr, 0, In::m("set_config").addr(&pk(STRANGER)).u16(2_000).addr(&pk(STRANGER)).bool(false).b(), "only the market admin");
    c.fails(ADMIN, m.addr, 0, In::m("set_config").addr(&pk(ADMIN)).u16(2_000).addr(&escrow_address()).bool(false).b(), "bridge escrow");
    c.ok(ADMIN, m.addr, 0, In::m("set_config").addr(&pk(ADMIN)).u16(2_000).addr(&pk(FEE_TO)).bool(false).b());
    let (id, fee) = complete_one(&mut c, &m, price);
    let p1 = c.bal(&pool());
    c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!((c.bal(&pool()) - p1, c.bal(&pk(FEE_TO)) - f0), (0, fee));
}

#[test]
fn deployed_v1_market_migrates_to_pool_fees_keeping_its_hires() {
    let mut c = Chain::new();
    assert_eq!(hex(&ac::code_hash(MARKET_V1)), "1647ce2418396531137c6451b9ce5f47dd6ab7197c36db9ad7c2ca95abf3080e", "fixture is the devnet code");
    // admin = the market admin, as on devnet (it may Migrate)
    let m = setup_to(&mut c, MARKET_V1, 2_000, &pk(FEE_TO), pk(ADMIN));
    let price = 20 * CADRE;
    let (id, fee) = complete_one(&mut c, &m, price);
    // v1 cannot pay the pool: with fee_to = pool a completing verdict reverts (why v2 exists)
    c.ok(ADMIN, m.addr, 0, In::m("set_config").addr(&pk(ADMIN)).u16(2_000).addr(&pool()).bool(false).b());
    c.fails(JUDGE, m.addr, 0, verdict(JUDGE, id, true), "reserved account");
    assert_eq!(view(&c, &m, id).status, DELIVERED);
    // Migrate to v2 (no migrate entry: code swap only, storage and balance kept)
    let v2 = c.deploy(ADMIN, MARKET);
    let s = c.send(ADMIN, TxBody::Migrate(ac::Migrate { contract: m.addr, code_hash: v2, gas_limit: 5_000_000, args: Vec::new() }));
    assert_eq!(s.receipts[0].status, Status::Ok, "{}", String::from_utf8_lossy(&s.receipts[0].error));
    eprintln!("gas: market migrate {} (fee {})", s.receipts[0].gas_used, s.receipts[0].fee_charged);
    let h = view(&c, &m, id);
    assert_eq!((h.status, h.price, h.fee), (DELIVERED, price, fee), "the open hire survived the migration");
    let p0 = c.bal(&pool());
    c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!(c.bal(&pool()) - p0, fee);
    assert_eq!(c.bal(&m.addr), 0);
    // only the admin may migrate
    let s = c.send(STRANGER, TxBody::Migrate(ac::Migrate { contract: m.addr, code_hash: v2, gas_limit: 5_000_000, args: Vec::new() }));
    assert_ne!(s.receipts[0].status, Status::Ok);
}

#[test]
fn no_contract_path_takes_cadre_out_of_the_pool() {
    let mut c = Chain::new();
    let h = c.deploy(ADMIN, PROBE);
    let probe = c.instantiate(ADMIN, h, Vec::new());
    c.fund(probe, 5 * CADRE);
    c.ok(STRANGER, probe, 0, In::m("pay_pool").u64(3 * CADRE).b());
    let p0 = c.bal(&pool());
    assert_eq!(p0, 3 * CADRE);
    let transfer = |to: &Pubkey, amt: u64| -> Vec<u8> {
        let mut b = to.to_vec();
        b.extend_from_slice(&amt.to_le_bytes());
        b
    };
    // 1. the transfer host function still refuses reserved accounts (both ways it could be used)
    c.fails(STRANGER, probe, 0, In::m("host_pay").addr(&pool()).u64(1).b(), "reserved account");
    c.fails(STRANGER, probe, 0, In::m("host_pay").addr(&escrow_address()).u64(1).b(), "reserved account");
    // 2. a native Transfer moves only the contract's own balance: asking for more than it holds
    //    (the pool's balance included) fails and the pool is untouched
    let own = c.bal(&probe);
    c.fails(STRANGER, probe, 0, In::m("native").u8(0).bytes(&transfer(&pk(STRANGER), own + 1)).b(), "native call failed");
    assert_eq!(c.bal(&pool()), p0);
    // 3. a native Transfer to the escrow is refused (BridgeLock is the only way in)
    c.fails(STRANGER, probe, 0, In::m("native").u8(0).bytes(&transfer(&escrow_address(), 1)).b(), "native call failed");
    // 4. the pool is not a contract: calling it fails
    c.fails(STRANGER, probe, 0, In::m("call").addr(&pool()).bytes(&In::m("anything").b()).b(), "callee failed");
    // 5. no signed tx can come from the pool (reserved address cannot send)
    let t = Tx::signed(&seed(77), CHAIN_ID, 0, 1_000, TxBody::Transfer { to: pk(STRANGER), amount: 1 });
    let mut forged = t.clone();
    forged.from = pool();
    let _ = c.block(&[forged]);
    assert_eq!(c.bal(&pool()), p0, "the pool only went up");
    // 6. kinds a contract may not run natively (AdminConfig, BridgeUnlock, ...) are refused
    c.fails(STRANGER, probe, 0, In::m("native").u8(6).bytes(&[0u8; 8]).b(), "native call failed");
    assert_eq!(c.bal(&pool()), p0);
}

#[test]
fn pool_credit_on_a_dropped_branch_is_undone() {
    // a reorg replaces the tip's write set: build the hire, fork the state, complete on branch A,
    // mine an empty block on branch B, then let B carry the verdict one block later
    let mut a = Chain::new();
    let m = setup_to(&mut a, MARKET, 2_000, &pool(), [0; 32]);
    let (id, fee) = complete_one(&mut a, &m, 30 * CADRE);
    let fork = |x: &Chain| Chain { st: MemState { map: std::sync::Mutex::new(x.st.map.lock().unwrap().clone()) }, height: x.height, nonces: x.nonces.clone() };
    let mut b = fork(&a);
    let p0 = a.bal(&pool());
    a.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!(a.bal(&pool()) - p0, fee);
    b.block(&[]).unwrap();
    assert_eq!(b.bal(&pool()), p0, "branch B never saw the verdict");
    assert_eq!(view(&b, &m, id).status, DELIVERED);
    b.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!(b.bal(&pool()) - p0, fee, "the same verdict on the winning branch pays once");
    assert_eq!(supply_of(&b.st.values()).unwrap(), TOTAL_SUPPLY as u128);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
