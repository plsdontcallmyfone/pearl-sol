//! The services market contract (contracts/market, docs/MARKET.md) in the real executor: offers,
//! hires paid into escrow, delivery, buyer-named evaluators completing or rejecting, refunds on
//! reject, decline and timeout, the D6 fee floor (2,000 bps default, admin-editable), and the
//! self-dealing refusals the contract can see (a provider hiring or judging itself). Every
//! block checks the supply invariant; every closed hire leaves the contract holding nothing for it.

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const MARKET: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_market.wasm");

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

#[test]
fn hire_deliver_accept_pays_provider_fee_and_judge() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let price = 50 * CADRE;
    let offer = list(&mut c, &m, price, 10);
    // D6: 2,000 bps on top of the price, rounded up
    let (p, fee, total) = quote(&c, &m, offer, 3 * CADRE);
    assert_eq!((p, fee, total), (price, 10 * CADRE, 63 * CADRE));
    let (prov0, fee0, judge0) = (c.bal(&pk(PROVIDER)), c.bal(&pk(FEE_TO)), c.bal(&pk(JUDGE)));
    let (id, total) = hire(&mut c, &m, offer, 3 * CADRE, &[pk(JUDGE)], 1);
    assert_eq!(c.bal(&m.addr), total, "the whole hire sits in escrow");
    let h = view(&c, &m, id);
    assert_eq!((h.buyer, h.provider, h.status, h.price, h.fee, h.judge_fee), (pk(BUYER), pk(PROVIDER), OPEN, price, fee, 3 * CADRE));
    deliver(&mut c, &m, id);
    assert_eq!(view(&c, &m, id).status, DELIVERED);
    assert_eq!(view(&c, &m, id).result_hash, [0xbb; 32]);
    let p_before = c.bal(&pk(PROVIDER));
    let (r, s) = c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    eprintln!("gas: market verdict accept + payout {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(view(&c, &m, id).status, COMPLETED);
    let names: Vec<_> = s.events.iter().map(|e| e.topics[0]).collect();
    assert!(names.contains(&ac::event_topic(b"Verdict")) && names.contains(&ac::event_topic(b"Completed")));
    assert_eq!(c.bal(&pk(PROVIDER)) - p_before, price);
    assert!(c.bal(&pk(PROVIDER)) > prov0);
    assert_eq!(c.bal(&pk(FEE_TO)) - fee0, fee);
    assert_eq!(c.bal(&pk(JUDGE)) + r.fee_charged - judge0, 3 * CADRE);
    assert_eq!(c.bal(&m.addr), 0, "nothing left in escrow");
    // a second verdict on a closed hire is refused
    c.fails(JUDGE, m.addr, 0, verdict(JUDGE, id, false), "no delivery");
}

#[test]
fn fee_floor_is_exact_and_admin_editable() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let price = 7; // rounding: ceil(7 x 0.2) = 2
    let offer = list(&mut c, &m, price, 5);
    let (_, fee, total) = quote(&c, &m, offer, 0);
    assert_eq!((fee, total), (2, 9));
    c.fails(BUYER, m.addr, total - 1, hire_args(offer, price, 10, 0, &[pk(BUYER)], 1), "exactly price + fee");
    c.fails(BUYER, m.addr, total + 1, hire_args(offer, price, 10, 0, &[pk(BUYER)], 1), "exactly price + fee");
    c.fails(BUYER, m.addr, total, hire_args(offer, price + 1, 10, 0, &[pk(BUYER)], 1), "current price");
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 4, 0, &[pk(BUYER)], 1), "at least the offer's turnaround");
    // only the admin edits the fee, and never above 5,000 bps
    let cfg = |bps: u16, paused: bool| In::m("set_config").addr(&pk(ADMIN)).u16(bps).addr(&pk(FEE_TO)).bool(paused).b();
    c.fails(STRANGER, m.addr, 0, cfg(0, false), "only the market admin");
    c.fails(ADMIN, m.addr, 0, cfg(5_001, false), "above 5,000");
    c.ok(ADMIN, m.addr, 0, cfg(3_000, false));
    let (_, fee, total) = quote(&c, &m, offer, 0);
    assert_eq!((fee, total), (3, 10));
    c.ok(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(BUYER)], 1));
    // pausing stops new hires only
    c.ok(ADMIN, m.addr, 0, cfg(3_000, true));
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(BUYER)], 1), "paused");
    // value is refused on every method but hire
    c.fails(PROVIDER, m.addr, 1, offer_args("offer", 5, 5, 0, &[]).b(), "only hire takes value");
}

#[test]
fn reject_refunds_buyer_and_pays_the_judging() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let offer = list(&mut c, &m, 40 * CADRE, 10);
    let b0 = c.bal(&pk(BUYER));
    let (id, total) = hire(&mut c, &m, offer, 2 * CADRE, &[pk(JUDGE)], 1);
    let hire_tx_fee = b0 - total - c.bal(&pk(BUYER));
    deliver(&mut c, &m, id);
    let (p0, j0, f0, bb) = (c.bal(&pk(PROVIDER)), c.bal(&pk(JUDGE)), c.bal(&pk(FEE_TO)), c.bal(&pk(BUYER)));
    let (r, s) = c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, false));
    assert!(s.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Rejected")));
    let h = view(&c, &m, id);
    assert_eq!((h.status, h.rejects), (REJECTED, 1));
    assert_eq!(c.bal(&pk(BUYER)) - bb, 48 * CADRE, "price + fee back to the buyer");
    assert_eq!(c.bal(&pk(PROVIDER)), p0, "the provider is not paid");
    assert_eq!(c.bal(&pk(FEE_TO)), f0, "no fee on a rejected hire");
    assert_eq!(c.bal(&pk(JUDGE)) + r.fee_charged - j0, 2 * CADRE, "the judge is paid for judging");
    assert_eq!(c.bal(&m.addr), 0);
    // the buyer lost exactly the judge fee and its own tx fee
    assert_eq!(b0 - c.bal(&pk(BUYER)), 2 * CADRE + hire_tx_fee);
    // the provider cannot deliver again on a closed hire
    c.fails(PROVIDER, m.addr, 0, In::m("deliver").u64(id).addr(&[1; 32]).str("x").str("").b(), "not open");
}

#[test]
fn timeout_refunds_everything_open_or_delivered() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let offer = list(&mut c, &m, 10 * CADRE, 5);
    // open, never delivered
    let b0 = c.bal(&pk(BUYER));
    let (id, total) = hire(&mut c, &m, offer, CADRE, &[pk(JUDGE)], 1);
    let fee_hire = b0 - total - c.bal(&pk(BUYER));
    c.fails(STRANGER, m.addr, 0, In::m("expire").u64(id).b(), "not passed");
    let dl = view(&c, &m, id).deadline;
    c.skip(dl - c.height - 1);
    // a tx runs at height + 1: this one runs at the deadline itself
    c.fails(STRANGER, m.addr, 0, In::m("expire").u64(id).b(), "not passed");
    // past the deadline nobody can deliver any more
    c.fails(PROVIDER, m.addr, 0, In::m("deliver").u64(id).addr(&[1; 32]).str("x").str("").b(), "deadline passed");
    let (_, s) = c.ok(STRANGER, m.addr, 0, In::m("expire").u64(id).b());
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Expired")).unwrap();
    assert_eq!(ev.data[0], 0, "expired before any delivery");
    assert_eq!(view(&c, &m, id).status, EXPIRED);
    assert_eq!(b0 - c.bal(&pk(BUYER)), fee_hire, "everything back but the buyer's own tx fee");
    c.fails(STRANGER, m.addr, 0, In::m("expire").u64(id).b(), "already closed");
    // delivered but never judged: delivered-then-expired, still a full refund
    let (id2, _) = hire(&mut c, &m, offer, CADRE, &[pk(JUDGE)], 1);
    deliver(&mut c, &m, id2);
    let dl = view(&c, &m, id2).deadline;
    c.skip(dl - c.height + 1);
    c.fails(JUDGE, m.addr, 0, verdict(JUDGE, id2, true), "deadline passed");
    let bb = c.bal(&pk(BUYER));
    let (_, s) = c.ok(STRANGER, m.addr, 0, In::m("expire").u64(id2).b());
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Expired")).unwrap();
    assert_eq!(ev.data[0], 1, "expired after a delivery");
    assert_eq!(c.bal(&pk(BUYER)) - bb, 10 * CADRE + 2 * CADRE + CADRE);
    assert_eq!(c.bal(&m.addr), 0);
}

#[test]
fn decline_refunds_and_only_while_open() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let offer = list(&mut c, &m, 10 * CADRE, 5);
    let (id, total) = hire(&mut c, &m, offer, 0, &[pk(BUYER)], 1);
    c.fails(BUYER, m.addr, 0, In::m("decline").u64(id).str("no").b(), "only the hire's provider");
    let bb = c.bal(&pk(BUYER));
    let (_, s) = c.ok(PROVIDER, m.addr, 0, In::m("decline").u64(id).str("too busy this week").b());
    assert!(s.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Declined")));
    assert_eq!(c.bal(&pk(BUYER)) - bb, total);
    assert_eq!(view(&c, &m, id).status, DECLINED);
    // a delivered hire can no longer be declined
    let (id2, _) = hire(&mut c, &m, offer, 0, &[pk(BUYER)], 1);
    deliver(&mut c, &m, id2);
    c.fails(PROVIDER, m.addr, 0, In::m("decline").u64(id2).str("no").b(), "only an open hire");
    // the buyer as its own evaluator accepts
    c.ok(BUYER, m.addr, 0, verdict(BUYER, id2, true));
    assert_eq!(view(&c, &m, id2).status, COMPLETED);
}

#[test]
fn self_dealing_and_evaluator_rules() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let offer = list(&mut c, &m, 10 * CADRE, 5);
    let (price, _, total) = quote(&c, &m, offer, 0);
    // a provider cannot hire itself or be named to judge its own work
    c.fails(PROVIDER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(JUDGE)], 1), "cannot hire its own offer");
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(PROVIDER)], 1), "cannot judge its own work");
    c.fails(PROVIDER, m.addr, 0, offer_args("offer", 5, 5, 1, &[pk(PROVIDER)]).b(), "cannot judge its own work");
    // evaluator list rules
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[], 1), "at least one evaluator");
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(JUDGE), pk(JUDGE)], 1), "distinct");
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(JUDGE)], 2), "threshold");
    c.fails(BUYER, m.addr, total, hire_args(offer, price, 10, 0, &[pk(1), pk(4), pk(5), pk(6), pk(10)], 1), "at most 4");
    // policy 1: only the offer's judges
    let (r, _) = c.ok(PROVIDER, m.addr, 0, offer_args("offer", 10 * CADRE, 5, 1, &[pk(JUDGE)]).b());
    let strict = u64_at(&r.ret, 0);
    c.fails(BUYER, m.addr, total, hire_args(strict, price, 10, 0, &[pk(BUYER)], 1), "only evaluators from its list");
    c.ok(BUYER, m.addr, total, hire_args(strict, price, 10, 0, &[pk(JUDGE)], 1));
    // only the provider updates or delivers; only named evaluators vote, after a delivery
    let mut upd = In::m("update_offer").u64(offer).b();
    upd.extend_from_slice(&offer_args("x", 5 * CADRE, 5, 0, &[]).b()[2..]);
    c.fails(STRANGER, m.addr, 0, upd.clone(), "only the offer's provider");
    c.ok(PROVIDER, m.addr, 0, upd);
    let (price2, _, total2) = quote(&c, &m, offer, 0);
    assert_eq!(price2, 5 * CADRE, "re-priced");
    let (r, _) = c.ok(BUYER, m.addr, total2, hire_args(offer, price2, 10, 0, &[pk(JUDGE)], 1));
    let id = u64_at(&r.ret, 0);
    c.fails(JUDGE, m.addr, 0, verdict(JUDGE, id, true), "no delivery");
    c.fails(STRANGER, m.addr, 0, In::m("deliver").u64(id).addr(&[1; 32]).str("x").str("").b(), "only the hire's provider");
    deliver(&mut c, &m, id);
    c.fails(STRANGER, m.addr, 0, verdict(STRANGER, id, true), "only a named evaluator");
    c.fails(BUYER, m.addr, 0, verdict(BUYER, id, true), "only a named evaluator");
}

#[test]
fn two_of_three_judges_split_the_fee() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let offer = list(&mut c, &m, 30 * CADRE, 5);
    let jf = 10; // 10 base units over 2 accepting judges: 5 each
    let (id, _) = hire(&mut c, &m, offer, jf, &[pk(JUDGE), pk(JUDGE2), pk(JUDGE3)], 2);
    deliver(&mut c, &m, id);
    let (j1, j2, j3) = (c.bal(&pk(JUDGE)), c.bal(&pk(JUDGE2)), c.bal(&pk(JUDGE3)));
    let (r1, _) = c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id, true));
    assert_eq!(view(&c, &m, id).status, DELIVERED, "one of two accepts is not enough");
    let (r2, _) = c.ok(JUDGE2, m.addr, 0, verdict(JUDGE2, id, false));
    assert_eq!(view(&c, &m, id).status, DELIVERED, "1 accept 1 reject: the third decides");
    let (r3, _) = c.ok(JUDGE3, m.addr, 0, verdict(JUDGE3, id, true));
    let h = view(&c, &m, id);
    assert_eq!((h.status, h.accepts, h.rejects), (COMPLETED, 2, 1));
    assert_eq!(c.bal(&pk(JUDGE)) + r1.fee_charged - j1, 5);
    assert_eq!(j2 - c.bal(&pk(JUDGE2)), r2.fee_charged, "the dissenting judge is not paid");
    assert_eq!(c.bal(&pk(JUDGE3)) + r3.fee_charged - j3, 5);
    assert_eq!(c.bal(&m.addr), 0);
    // 2-of-3 rejects as soon as two reject
    let (id2, _) = hire(&mut c, &m, offer, 3, &[pk(JUDGE), pk(JUDGE2), pk(JUDGE3)], 2);
    deliver(&mut c, &m, id2);
    c.ok(JUDGE, m.addr, 0, verdict(JUDGE, id2, false));
    assert_eq!(view(&c, &m, id2).status, DELIVERED);
    c.fails(JUDGE, m.addr, 0, verdict(JUDGE, id2, true), "already voted");
    let j2b = c.bal(&pk(JUDGE2));
    let (r, _) = c.ok(JUDGE2, m.addr, 0, verdict(JUDGE2, id2, false));
    assert_eq!(view(&c, &m, id2).status, REJECTED);
    // 3 base units over 2 rejecting judges: 2 to the first, 1 to the second
    assert_eq!(c.bal(&pk(JUDGE2)) + r.fee_charged - j2b, 1);
    c.fails(JUDGE3, m.addr, 0, verdict(JUDGE3, id2, true), "no delivery");
    assert_eq!(c.bal(&m.addr), 0);
}

#[test]
fn queries_and_counts() {
    let mut c = Chain::new();
    let m = setup(&mut c, 2_000);
    let cfg = c.q(&m.addr, In::m("config").b());
    assert_eq!(&cfg[0..32], &pk(ADMIN));
    assert_eq!(u16::from_le_bytes([cfg[32], cfg[33]]), 2_000);
    assert_eq!(&cfg[34..66], &pk(FEE_TO));
    let offer = list(&mut c, &m, CADRE, 3);
    hire(&mut c, &m, offer, 0, &[pk(JUDGE)], 1);
    let n = c.q(&m.addr, In::m("counts").b());
    assert_eq!((u64_at(&n, 0), u64_at(&n, 8)), (1, 1));
    let o = c.q(&m.addr, In::m("offer").u64(offer).b());
    assert_eq!(&o[0..32], &pk(PROVIDER));
    // instantiate refuses a missing fee_to or a fee above 5,000 bps
    let code = ac::code_hash(MARKET);
    let addr = contract_address(&pk(ADMIN), c.nonce(ADMIN));
    let s = c.send(ADMIN, TxBody::Instantiate(ac::Instantiate { code_hash: code, admin: [0; 32], value: 0, gas_limit: 50_000_000, label: b"m".to_vec(), args: In::raw().addr(&pk(ADMIN)).u16(2_000).addr(&[0; 32]).b() }));
    assert_ne!(s.receipts[0].status, Status::Ok);
    let _ = addr;
}
