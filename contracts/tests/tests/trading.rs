//! The trading contracts (docs/TRADING.md) in the real executor: the AMM (liquidity, swaps both
//! ways, slippage and deadlines, donations, fee admin) and the Cadre mandate (the user owns the
//! account, the agent can only swap within limits, revoke / rotate / pause / expiry / kill switch,
//! withdraw never blocked, fees only on realized CADRE profit above the high-water mark).
//! Every block checks the supply invariant.

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const AMM: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_amm.wasm");
const MANDATE: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_mandate.wasm");
const TOKEN: &[u8] = include_bytes!("../../../chain/contracts/testdata/token.wasm");
const COUNTER: &[u8] = include_bytes!("../../../chain/contracts/testdata/counter.wasm");

const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const ADMIN: u8 = 1;
const LP: u8 = 2;
const TRADER: u8 = 3;
const USER: u8 = 4;
const AGENT: u8 = 5;
const PROTO: u8 = 6;
const AGENT2: u8 = 7;
const FEE_TO: u8 = 8;
const CADRE: u64 = 1_000_000_000;

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
    fn b(self) -> Vec<u8> {
        self.0
    }
}

struct Chain {
    st: MemState,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

const CHAIN_ID: u32 = 7;

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, height: 0, nonces: Default::default() };
        for who in [ADMIN, LP, TRADER, USER, AGENT, PROTO, AGENT2, FEE_TO] {
            c.fund(pk(who), 100_000 * CADRE);
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
        ac::code_hash(code)
    }
    fn instantiate(&mut self, who: u8, code_hash: Hash, args: Vec<u8>) -> Pubkey {
        let addr = contract_address(&pk(who), self.nonce(who));
        let s = self.send(who, TxBody::Instantiate(ac::Instantiate { code_hash, admin: [0; 32], value: 0, gas_limit: 50_000_000, label: b"t".to_vec(), args }));
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
        let (r, s) = self.call(who, contract, value, args);
        assert_ne!(r.status, Status::Ok, "expected a failure ({want})");
        let e = String::from_utf8_lossy(&r.error).to_string();
        assert!(e.contains(want), "error {e:?} should contain {want:?}");
        assert!(s.events.is_empty(), "failed call kept no events");
        assert_eq!(self.bal(&contract), before, "failed call moved no CADRE into the contract");
    }
    fn q(&self, contract: &Pubkey, args: Vec<u8>) -> Vec<u8> {
        let r = block_on(query_contract(&self.st, CHAIN_ID, self.height + 1, contract, args, 50_000_000)).unwrap();
        assert!(r.ok, "{}", String::from_utf8_lossy(&r.error));
        r.ret
    }
    fn q64(&self, contract: &Pubkey, args: Vec<u8>) -> u64 {
        u64::from_le_bytes(self.q(contract, args)[..8].try_into().unwrap())
    }
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn out_for(r_in: u64, r_out: u64, x: u64, fee_bps: u64) -> u64 {
    let xf = x as u128 * (10_000 - fee_bps) as u128;
    (xf * r_out as u128 / (r_in as u128 * 10_000 + xf)) as u64
}
fn cp(r_in: u64, r_out: u64, x: u64) -> u64 {
    (r_out as u128 * x as u128 / (r_in as u128 + x as u128)) as u64
}

struct Market {
    token: Pubkey,
    amm: Pubkey,
    amm_code: Hash,
}

/// An agent token (the SDK example token) and a CADRE pool for it, seeded by LP.
fn market(c: &mut Chain) -> Market {
    let tc = c.deploy(LP, TOKEN);
    let token = c.instantiate(LP, tc, In::raw().str("Agent Coin").str("AGNT").u8(9).u64(1_000_000_000 * CADRE).bool(false).b());
    let amm_code = c.deploy(LP, AMM);
    let amm = c.instantiate(LP, amm_code, In::raw().addr(&token).u16(30).addr(&[0; 32]).b());
    c.ok(LP, token, 0, In::m("approve").addr(&amm).u64(u64::MAX).b());
    // 10,000 CADRE against 1,000,000 tokens: 100 tokens per CADRE
    let (r, s) = c.ok(LP, amm, 10_000 * CADRE, In::m("add_liquidity").u64(1_000_000 * CADRE).u64(1).u64(0).b());
    assert!(u64_at(&r.ret, 0) > 0);
    assert!(s.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Mint")));
    Market { token, amm, amm_code }
}
fn reserves(c: &Chain, amm: &Pubkey) -> (u64, u64, u64) {
    let i = c.q(amm, In::m("info").b());
    (u64_at(&i, 32), u64_at(&i, 40), u64_at(&i, 48))
}
fn tok_bal(c: &Chain, token: &Pubkey, who: &Pubkey) -> u64 {
    c.q64(token, In::m("balance_of").addr(who).b())
}

#[test]
fn amm_swaps_both_ways_with_fees_events_slippage_and_deadline() {
    let mut c = Chain::new();
    let m = market(&mut c);
    let (rc, rt, _) = reserves(&c, &m.amm);
    assert_eq!((rc, rt), (10_000 * CADRE, 1_000_000 * CADRE));
    // buy tokens with 10 CADRE
    let x = 10 * CADRE;
    let want = out_for(rc, rt, x, 30);
    assert_eq!(c.q64(&m.amm, In::m("quote").u8(0).u64(x).b()), want);
    let b0 = c.bal(&pk(TRADER));
    let (r, s) = c.ok(TRADER, m.amm, x, In::m("swap_cadre_for_token").u64(want).addr(&pk(TRADER)).u64(0).b());
    assert_eq!(u64_at(&r.ret, 0), want);
    eprintln!("gas: amm swap_cadre_for_token {} (fee charged {} base units)", r.gas_used, r.fee_charged);
    assert_eq!(tok_bal(&c, &m.token, &pk(TRADER)), want);
    // the trader also pays the storage deposit for its new token balance item: (key 34 + value 8 + 64) bytes
    let deposit = (34 + 8 + 64) * c.cfg().storage_byte_deposit;
    assert_eq!(b0 - c.bal(&pk(TRADER)), x + r.fee_charged + deposit);
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Swap")).unwrap();
    assert_eq!(ev.topics[1], pk(TRADER));
    assert_eq!(ev.data[0], 0);
    assert_eq!(u64_at(&ev.data, 1), x);
    assert_eq!(u64_at(&ev.data, 9), want);
    assert_eq!(&ev.data[33..65], &pk(TRADER), "origin in the event");
    let k0 = rc as u128 * rt as u128;
    let (rc1, rt1, _) = reserves(&c, &m.amm);
    assert!(rc1 as u128 * rt1 as u128 >= k0, "k never falls");
    // sell half back
    let t = want / 2;
    c.ok(TRADER, m.token, 0, In::m("approve").addr(&m.amm).u64(t).b());
    let back = out_for(rt1, rc1, t, 30);
    let b1 = c.bal(&pk(TRADER));
    let (r, _) = c.ok(TRADER, m.amm, 0, In::m("swap_token_for_cadre").u64(t).u64(back).addr(&pk(TRADER)).u64(0).b());
    assert_eq!(u64_at(&r.ret, 0), back);
    assert_eq!(c.bal(&pk(TRADER)) + r.fee_charged - b1, back);
    // slippage: asking for 1 more than the pool gives reverts and moves nothing
    let (rc2, rt2, _) = reserves(&c, &m.amm);
    let w = out_for(rc2, rt2, x, 30);
    c.fails(TRADER, m.amm, x, In::m("swap_cadre_for_token").u64(w + 1).addr(&pk(TRADER)).u64(0).b(), "slippage");
    // deadline: max_height in the past
    c.fails(TRADER, m.amm, x, In::m("swap_cadre_for_token").u64(1).addr(&pk(TRADER)).u64(c.height).b(), "deadline");
    // a sell without an allowance fails
    c.fails(TRADER, m.amm, 0, In::m("swap_token_for_cadre").u64(1_000).u64(1).addr(&pk(TRADER)).u64(0).b(), "transfer_from");
    // donations do not move the price (reserves are tracked, not read from balances)
    let q0 = c.q64(&m.amm, In::m("quote").u8(0).u64(x).b());
    c.send(TRADER, TxBody::Transfer { to: m.amm, amount: 500 * CADRE });
    assert_eq!(c.q64(&m.amm, In::m("quote").u8(0).u64(x).b()), q0);
    // immutable fee (admin zero)
    c.fails(LP, m.amm, 0, In::m("set_fee").u16(10).b(), "only the admin");
    // remove all of LP's liquidity: proportional out, 1000 lp stays locked
    let lp = c.q64(&m.amm, In::m("lp_of").addr(&pk(LP)).b());
    let (rc3, rt3, s3) = reserves(&c, &m.amm);
    let (r, _) = c.ok(LP, m.amm, 0, In::m("remove_liquidity").u64(lp).u64(1).u64(1).u64(0).b());
    assert_eq!(u64_at(&r.ret, 0), (lp as u128 * rc3 as u128 / s3 as u128) as u64);
    assert_eq!(u64_at(&r.ret, 8), (lp as u128 * rt3 as u128 / s3 as u128) as u64);
    let (_, _, s4) = reserves(&c, &m.amm);
    assert_eq!(s4, 1_000);
}

fn limits(token: &Pubkey, f: impl FnOnce(&mut Lim)) -> In {
    let mut l = Lim { max_in: 100 * CADRE, window_cap: 300 * CADRE, window_blocks: 1_440, max_trades: 20, slip: 500, min_pool: 1_000 * CADRE, share: 1_000, expiry: 0, paused: false };
    f(&mut l);
    In::raw()
        .u64(l.max_in)
        .u64(l.window_cap)
        .u64(l.window_blocks)
        .u16(l.max_trades)
        .u16(l.slip)
        .u64(l.min_pool)
        .u16(l.share)
        .u64(l.expiry)
        .bool(l.paused)
        .bool(false)
        .u8(1)
        .addr(token)
}
struct Lim {
    max_in: u64,
    window_cap: u64,
    window_blocks: u64,
    max_trades: u16,
    slip: u16,
    min_pool: u64,
    share: u16,
    expiry: u64,
    paused: bool,
}
fn with(prefix: In, rest: In) -> Vec<u8> {
    let mut v = prefix.b();
    v.extend(rest.b());
    v
}

struct Desk {
    m: Market,
    mandate: Pubkey,
    id: u64,
}

fn desk(c: &mut Chain) -> Desk {
    let m = market(c);
    let mc = c.deploy(ADMIN, MANDATE);
    let mandate = c.instantiate(ADMIN, mc, In::raw().addr(&pk(ADMIN)).u16(1_000).addr(&pk(PROTO)).addr(&m.amm_code).b());
    let (r, _) = c.ok(USER, mandate, 500 * CADRE, with(In::m("open").addr(&pk(AGENT)).addr(&pk(FEE_TO)).u16(2_000).u64(60), limits(&m.token, |_| {})));
    let id = u64_at(&r.ret, 0);
    Desk { m, mandate, id }
}
fn swap_args(d: &Desk, side: u8, x: u64, min_out: u64) -> Vec<u8> {
    In::m("swap").u64(d.id).addr(&d.m.amm).u8(side).u64(x).u64(min_out).u64(0).b()
}
fn buy_min(c: &Chain, d: &Desk, x: u64, bps: u64) -> u64 {
    let (rc, rt, _) = reserves(c, &d.m.amm);
    (cp(rc, rt, x) as u128 * (10_000 - bps) as u128 / 10_000) as u64
}
fn sell_min(c: &Chain, d: &Desk, x: u64, bps: u64) -> u64 {
    let (rc, rt, _) = reserves(c, &d.m.amm);
    (cp(rt, rc, x) as u128 * (10_000 - bps) as u128 / 10_000) as u64
}
fn record(c: &Chain, d: &Desk) -> Vec<u8> {
    c.q(&d.mandate, In::m("get").u64(d.id).b())
}
fn equity(c: &Chain, d: &Desk) -> u64 {
    // user, agent, fee_to (96) | perf u16 | settle u64 | revoked u8 = 107; limits with 1 token = 8*3+2+2+8+2+8+1+1+1+32 = 81
    u64_at(&record(c, d), 107 + 81)
}
fn position(c: &Chain, d: &Desk) -> u64 {
    c.q64(&d.mandate, In::m("position").u64(d.id).addr(&d.m.token).b())
}

#[test]
fn mandate_agent_swaps_inside_limits_and_user_keeps_custody() {
    let mut c = Chain::new();
    let d = desk(&mut c);
    assert_eq!(equity(&c, &d), 500 * CADRE);
    // agent buys 50 CADRE of the token
    let x = 50 * CADRE;
    let min = buy_min(&c, &d, x, 100);
    let (r, s) = c.ok(AGENT, d.mandate, 0, swap_args(&d, 0, x, min));
    let got = u64_at(&r.ret, 0);
    eprintln!("gas: mandate swap buy {} (fee charged {} base units)", r.gas_used, r.fee_charged);
    assert!(got >= min);
    assert_eq!(position(&c, &d), got);
    assert_eq!(equity(&c, &d), 450 * CADRE);
    assert_eq!(tok_bal(&c, &d.m.token, &d.mandate), got, "the contract holds the tokens");
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"MandateSwap")).expect("MandateSwap event");
    assert_eq!(ev.topics[2], pk(AGENT));
    assert!(s.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Swap")), "the pool's Swap event in the same receipt");
    // agent sells half
    let t = got / 2;
    let min = sell_min(&c, &d, t, 100);
    let (r, _) = c.ok(AGENT, d.mandate, 0, swap_args(&d, 1, t, min));
    let back = u64_at(&r.ret, 0);
    eprintln!("gas: mandate swap sell {} (fee charged {} base units)", r.gas_used, r.fee_charged);
    assert!(back >= min);
    assert_eq!(position(&c, &d), got - t);
    assert_eq!(equity(&c, &d), 450 * CADRE + back);
    assert_eq!(c.q64(&d.m.token, In::m("allowance").addr(&d.mandate).addr(&d.m.amm).b()), 0, "no allowance left behind");

    // exploit set: the agent cannot take funds out or change anything
    c.fails(AGENT, d.mandate, 0, In::m("withdraw").u64(d.id).u64(CADRE).b(), "only the mandate's user");
    c.fails(AGENT, d.mandate, 0, In::m("withdraw_token").u64(d.id).addr(&d.m.token).u64(1).b(), "only the mandate's user");
    c.fails(AGENT, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.max_in = u64::MAX)), "only the mandate's user");
    c.fails(AGENT, d.mandate, 0, In::m("set_agent").u64(d.id).addr(&pk(AGENT)).b(), "only the mandate's user");
    c.fails(AGENT, d.mandate, 0, In::m("revoke").u64(d.id).b(), "only the mandate's user");
    c.fails(TRADER, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "only the mandate's agent");
    // a pool with other code (a counter) or a second AMM-like contract the agent wrote is refused
    let cc = c.deploy(AGENT, COUNTER);
    let fake = c.instantiate(AGENT, cc, In::raw().u64(0).b());
    c.fails(AGENT, d.mandate, 0, In::m("swap").u64(d.id).addr(&fake).u8(0).u64(CADRE).u64(1).u64(0).b(), "pool code is not the allowed AMM");
    c.fails(AGENT, d.mandate, 0, In::m("swap").u64(d.id).addr(&pk(AGENT)).u8(0).u64(CADRE).u64(1).u64(0).b(), "pool is not a contract");
    // a pool of the right code for a token that is not allowed
    let tc = ac::code_hash(TOKEN);
    let other = c.instantiate(TRADER, tc, In::raw().str("Other").str("OTH").u8(9).u64(1_000_000 * CADRE).bool(false).b());
    let pool2 = c.instantiate(TRADER, d.m.amm_code, In::raw().addr(&other).u16(30).addr(&[0; 32]).b());
    c.ok(TRADER, other, 0, In::m("approve").addr(&pool2).u64(u64::MAX).b());
    c.ok(TRADER, pool2, 5_000 * CADRE, In::m("add_liquidity").u64(500_000 * CADRE).u64(1).u64(0).b());
    c.fails(AGENT, d.mandate, 0, In::m("swap").u64(d.id).addr(&pool2).u8(0).u64(CADRE).u64(1).u64(0).b(), "token not allowed");
    // limits
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, 101 * CADRE, buy_min(&c, &d, 101 * CADRE, 100)), "larger than the limits");
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, 1), "min_out looser");
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 600)), "min_out looser");
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 1, got, sell_min(&c, &d, got, 100)), "not enough of that token");
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.window_cap = 120 * CADRE)));
    // used so far: 50 buy + the sell's quote value; 100 more passes the 120 cap
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, 100 * CADRE, buy_min(&c, &d, 100 * CADRE, 100)), "window cap");
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.max_trades = 2)));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "too many trades");
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.min_pool = 1_000_000 * CADRE)));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "pool CADRE reserve below");
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.share = 1)));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, 2 * CADRE, buy_min(&c, &d, 2 * CADRE, 100)), "larger than the limits");
    // pause, expiry
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.paused = true)));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "paused");
    c.ok(USER, d.mandate, 0, In::m("withdraw").u64(d.id).u64(CADRE).b());
    let h = c.height;
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.expiry = h + 1)));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "expired");
    c.ok(USER, d.mandate, 0, In::m("withdraw_token").u64(d.id).addr(&d.m.token).u64(1_000).b());
    assert_eq!(tok_bal(&c, &d.m.token, &pk(USER)), 1_000);
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |_| {})));
    // the window rolls after window_blocks
    c.ok(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)));
    // kill switch: swaps stop, withdrawals and deposits keep working; only the admin can flip it
    let set_cfg = |paused: bool| In::m("set_config").u16(1_000).addr(&pk(PROTO)).addr(&d.m.amm_code).bool(paused).addr(&pk(ADMIN)).b();
    c.fails(USER, d.mandate, 0, set_cfg(true), "only the admin");
    c.ok(ADMIN, d.mandate, 0, set_cfg(true));
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "kill switch");
    c.ok(USER, d.mandate, 0, In::m("withdraw").u64(d.id).u64(CADRE).b());
    c.ok(USER, d.mandate, CADRE, In::m("deposit").u64(d.id).b());
    c.ok(ADMIN, d.mandate, 0, set_cfg(false));
    // rotation and revoke
    c.ok(USER, d.mandate, 0, In::m("set_agent").u64(d.id).addr(&pk(AGENT2)).b());
    c.fails(AGENT, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "only the mandate's agent");
    c.ok(AGENT2, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)));
    c.ok(USER, d.mandate, 0, In::m("revoke").u64(d.id).b());
    c.fails(AGENT2, d.mandate, 0, swap_args(&d, 0, CADRE, buy_min(&c, &d, CADRE, 100)), "revoked");
    // the user takes everything out after revoke, under the kill switch
    c.ok(ADMIN, d.mandate, 0, set_cfg(true));
    let p = position(&c, &d);
    c.ok(USER, d.mandate, 0, In::m("withdraw_token").u64(d.id).addr(&d.m.token).u64(p).b());
    let e = equity(&c, &d);
    let ub = c.bal(&pk(USER));
    let (r, _) = c.ok(USER, d.mandate, 0, In::m("withdraw").u64(d.id).u64(e).b());
    assert_eq!(c.bal(&pk(USER)) + r.fee_charged - ub, e);
    assert_eq!(equity(&c, &d), 0);
    assert_eq!(position(&c, &d), 0);
    c.fails(USER, d.mandate, 0, In::m("withdraw").u64(d.id).u64(1).b(), "not enough CADRE");
}

#[test]
fn mandate_fees_only_on_realized_profit_with_protocol_share() {
    let mut c = Chain::new();
    let d = desk(&mut c);
    // settle at zero: nothing charged
    let (r, _) = c.ok(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b());
    assert_eq!((u64_at(&r.ret, 0), u64_at(&r.ret, 8)), (0, 0));
    c.fails(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b(), "settle interval");
    // buy 100, an outside trader pumps the pool, settle before selling: unrealized, nothing charged
    let x = 100 * CADRE;
    c.ok(AGENT, d.mandate, 0, swap_args(&d, 0, x, buy_min(&c, &d, x, 100)));
    c.ok(TRADER, d.m.amm, 2_000 * CADRE, In::m("swap_cadre_for_token").u64(1).addr(&pk(TRADER)).u64(0).b());
    c.skip(60);
    let (r, _) = c.ok(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b());
    assert_eq!((u64_at(&r.ret, 0), u64_at(&r.ret, 8)), (0, 0), "token marks are never charged");
    // sell everything: realized profit
    c.ok(USER, d.mandate, 0, with(In::m("set_limits").u64(d.id), limits(&d.m.token, |l| l.max_in = 1_000 * CADRE)));
    let p = position(&c, &d);
    c.ok(AGENT, d.mandate, 0, swap_args(&d, 1, p, sell_min(&c, &d, p, 100)));
    let eq = equity(&c, &d);
    assert!(eq > 500 * CADRE, "profit realized: {eq}");
    let pnl = eq - 500 * CADRE;
    let fee = pnl * 2_000 / 10_000;
    let proto = fee * 1_000 / 10_000;
    let (p0, f0) = (c.bal(&pk(PROTO)), c.bal(&pk(FEE_TO)));
    c.skip(60);
    let (r, _) = c.ok(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b());
    assert_eq!(u64_at(&r.ret, 0), fee - proto);
    assert_eq!(u64_at(&r.ret, 8), proto);
    assert_eq!(c.bal(&pk(PROTO)) - p0, proto, "1,000 bps of the fee to protocol_to");
    assert_eq!(c.bal(&pk(FEE_TO)) - f0, fee - proto);
    assert_eq!(equity(&c, &d), eq - fee);
    // at the mark: nothing more; deposits and withdrawals are not profit
    c.skip(60);
    let (r, _) = c.ok(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b());
    assert_eq!((u64_at(&r.ret, 0), u64_at(&r.ret, 8)), (0, 0));
    c.ok(USER, d.mandate, 50 * CADRE, In::m("deposit").u64(d.id).b());
    c.ok(USER, d.mandate, 0, In::m("withdraw").u64(d.id).u64(20 * CADRE).b());
    c.skip(60);
    let (r, _) = c.ok(TRADER, d.mandate, 0, In::m("settle").u64(d.id).b());
    assert_eq!((u64_at(&r.ret, 0), u64_at(&r.ret, 8)), (0, 0), "flows are not profit");
}
