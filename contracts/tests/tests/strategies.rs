//! Strategies with post-mortems (contracts/strategies, docs/STRATEGIES.md) in the real executor:
//! a version, an entry in a later block, an exit in a later block, a post-mortem once, stats per
//! version, and the learning rule (a revision cites post-mortems that name the claimed fixable
//! bucket, never VARIANCE, each post-mortem motivates one version). Supply invariant every block.
#![allow(dead_code)]

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const STRAT: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_strategies.wasm");

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
    fn u32(mut self, x: u32) -> Self {
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
        eprintln!("gas: strategies deploy {} bytes {} gas, fee {}", code.len(), s.receipts[0].gas_used, s.receipts[0].fee_charged);
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

impl Chain {
    /// Two calls by the same sender in ONE block (consecutive nonces).
    fn two_in_one_block(&mut self, who: u8, contract: Pubkey, a: Vec<u8>, b: Vec<u8>) -> Vec<ac::Receipt> {
        let mut txs = Vec::new();
        for (i, args) in [a, b].into_iter().enumerate() {
            let body = TxBody::Call(ac::Call { contract, value: 0, gas_limit: 50_000_000, args });
            let n = self.nonce(who) + i as u64;
            let t0 = Tx::signed(&seed(who), CHAIN_ID, n, 0, body.clone());
            let fee = contract_fee_floor(&self.cfg(), t0.encode().len(), executor::gas_limit_of(&body));
            txs.push(Tx::signed(&seed(who), CHAIN_ID, n, fee, body));
        }
        let s = self.block(&txs).unwrap();
        *self.nonces.entry(who).or_insert(0) += 2;
        s.receipts
    }
}

const AGENT: u8 = PROVIDER;
const B_INFORMATION: u8 = 1;
const B_THESIS: u8 = 2;
const B_EXECUTION: u8 = 4;
const B_SIZING: u8 = 8;
const B_RISK: u8 = 16;
const B_LEVERAGE: u8 = 32;
const B_VARIANCE: u8 = 64;

fn h(x: u8) -> [u8; 32] {
    [x; 32]
}

fn deploy(c: &mut Chain, start: u64) -> Pubkey {
    let code = c.deploy(ADMIN, STRAT);
    c.instantiate(ADMIN, code, In::raw().u64(start).b())
}

fn publish(hash: u8, parent: u64, claimed: u8, motives: &[u64]) -> Vec<u8> {
    let mut i = In::m("publish").addr(&h(hash)).u64(parent).str("blob:spec").str("req_author").u8(claimed).u8(motives.len() as u8);
    for m in motives {
        i = i.u64(*m);
    }
    i.b()
}

fn enter(version: u64) -> Vec<u8> {
    // 0.05 SOL at a quoted price, stop 800 bps, target 1500 bps, max risk 0.0045 SOL
    In::m("enter").u64(version).addr(&h(7)).u64(50_000_000).u64(123_456).u64(400_000_000).u32(800).u32(1500).u64(4_500_000).u64(1_000).str("req_decision").b()
}

fn exit(entry: u64, reason: u8, proceeds: u64) -> Vec<u8> {
    let (fill_out, fill_slot) = if reason == 5 { (0, 0) } else { (399_000_000, 1_002) };
    In::m("exit").u64(entry).u64(fill_slot).u64(fill_out).u64(125_000).u64(1_300).u8(reason).u64(proceeds).addr(&h(9)).b()
}

fn pm(entry: u64, buckets: u8) -> Vec<u8> {
    In::m("postmortem").u64(entry).addr(&h(11)).u8(buckets).addr(&h(12)).str("req_pm").b()
}

fn ret_u64(r: &ac::Receipt) -> u64 {
    u64_at(&r.ret, 0)
}

struct V {
    author: Pubkey,
    line: u64,
    parent: u64,
    claimed: u8,
    motives: Vec<u64>,
    height: u64,
    superseded_by: u64,
    open_entry: u64,
    entries: u64,
    trades: u64,
    wins: u64,
    losses: u64,
    flats: u64,
    no_fills: u64,
    pnl: i64,
    lucky: u64,
    bucket: [u64; 7],
}

fn version(c: &Chain, s: &Pubkey, id: u64) -> V {
    let b = c.q(s, In::m("version").u64(id).b());
    let mut o = 0;
    let addr = |o: &mut usize| -> Pubkey {
        let a: Pubkey = b[*o..*o + 32].try_into().unwrap();
        *o += 32;
        a
    };
    let u = |o: &mut usize| -> u64 {
        let x = u64_at(&b, *o);
        *o += 8;
        x
    };
    let author = addr(&mut o);
    let line = u(&mut o);
    let parent = u(&mut o);
    o += 32;
    for _ in 0..2 {
        let n = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        o += 4 + n;
    }
    let claimed = b[o];
    let n = b[o + 1] as usize;
    o += 2;
    let motives = (0..n).map(|_| u(&mut o)).collect();
    let height = u(&mut o);
    let superseded_by = u(&mut o);
    let open_entry = u(&mut o);
    let entries = u(&mut o);
    let trades = u(&mut o);
    let wins = u(&mut o);
    let losses = u(&mut o);
    let flats = u(&mut o);
    let no_fills = u(&mut o);
    let pnl = u(&mut o) as i64;
    let lucky = u(&mut o);
    let mut bucket = [0u64; 7];
    for x in bucket.iter_mut() {
        *x = u(&mut o);
    }
    assert_eq!(o, b.len(), "version layout fully read");
    V { author, line, parent, claimed, motives, height, superseded_by, open_entry, entries, trades, wins, losses, flats, no_fills, pnl, lucky, bucket }
}

/// One full trade on version `v`: enter, skip a block, exit with `proceeds`, post-mortem `buckets`.
fn trade(c: &mut Chain, s: Pubkey, v: u64, proceeds: u64, buckets: u8) -> u64 {
    let (r, _) = c.ok(AGENT, s, 0, enter(v));
    let id = ret_u64(&r);
    c.ok(AGENT, s, 0, exit(id, if proceeds < 50_000_000 { 1 } else { 2 }, proceeds));
    c.ok(AGENT, s, 0, pm(id, buckets));
    id
}

#[test]
fn version_entry_exit_postmortem_and_stats() {
    let mut c = Chain::new();
    let s = deploy(&mut c, 0);
    let (r, sum) = c.ok(AGENT, s, 0, publish(1, 0, 0, &[]));
    eprintln!("gas: publish {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(ret_u64(&r), 1);
    assert!(sum.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Version") && e.topics[1] == pk(AGENT) && e.topics[2] == h(1)));
    let v = version(&c, &s, 1);
    assert_eq!((v.author, v.line, v.parent, v.claimed, v.height), (pk(AGENT), 1, 0, 0, c.height));

    // a post-mortem needs an exit; an exit needs a later block than the entry
    let (r, _) = c.ok(AGENT, s, 0, enter(1));
    eprintln!("gas: enter {} (fee {})", r.gas_used, r.fee_charged);
    let e = ret_u64(&r);
    assert_eq!(version(&c, &s, 1).open_entry, e);
    c.fails(AGENT, s, 0, enter(1), "the line already has an open trade");
    c.fails(AGENT, s, 0, pm(e, B_RISK), "no exit yet");
    c.fails(STRANGER, s, 0, exit(e, 1, 45_000_000), "only the entry's author");
    let (r, _) = c.ok(AGENT, s, 0, exit(e, 1, 45_000_000));
    eprintln!("gas: exit {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(u64_at(&r.ret, 0) as i64, -5_000_000, "pnl = proceeds - size, computed by the contract");
    c.fails(AGENT, s, 0, exit(e, 1, 45_000_000), "already exited");
    c.fails(AGENT, s, 0, pm(e, B_LEVERAGE), "LEVERAGE does not apply");
    c.fails(AGENT, s, 0, pm(e, B_VARIANCE | B_RISK), "VARIANCE only on a loss, and only when no fault");
    let (r, _) = c.ok(AGENT, s, 0, pm(e, B_RISK | B_SIZING));
    eprintln!("gas: postmortem {} (fee {})", r.gas_used, r.fee_charged);
    assert_eq!(r.ret, vec![2], "outcome loss");
    c.fails(AGENT, s, 0, pm(e, B_RISK), "already explained");

    // a lucky win (a fault bit on a win), a clean win, a VARIANCE loss
    trade(&mut c, s, 1, 60_000_000, B_INFORMATION);
    trade(&mut c, s, 1, 58_000_000, 0);
    trade(&mut c, s, 1, 46_000_000, B_VARIANCE);
    let v = version(&c, &s, 1);
    assert_eq!((v.trades, v.wins, v.losses, v.flats, v.no_fills, v.lucky, v.entries), (4, 2, 2, 0, 0, 1, 4));
    assert_eq!(v.pnl, -5_000_000 + 10_000_000 + 8_000_000 - 4_000_000);
    assert_eq!(v.bucket, [1, 0, 0, 1, 1, 0, 1]);
    assert_eq!(v.open_entry, 0);

    // the trade query: entry, exit flag + exit, pm flag + pm
    let t = c.q(&s, In::m("trade").u64(e).b());
    assert_eq!(&t[0..32], &pk(AGENT));
    assert_eq!(u64_at(&t, 32), 1, "version");
    // VARIANCE is refused on a win
    let (r, _) = c.ok(AGENT, s, 0, enter(1));
    let w = ret_u64(&r);
    c.ok(AGENT, s, 0, exit(w, 2, 51_000_000));
    c.fails(AGENT, s, 0, pm(w, B_VARIANCE), "VARIANCE only on a loss");
    c.ok(AGENT, s, 0, pm(w, 0));
}

#[test]
fn entries_and_exits_land_in_later_blocks_only() {
    let mut c = Chain::new();
    let s = deploy(&mut c, 0);
    // version and entry in one block: the entry is refused
    let r = c.two_in_one_block(AGENT, s, publish(1, 0, 0, &[]), enter(1));
    assert_eq!(r[0].status, Status::Ok);
    assert_ne!(r[1].status, Status::Ok);
    assert!(String::from_utf8_lossy(&r[1].error).contains("later block than its version"));
    // entry and exit in one block: the exit is refused
    let r = c.two_in_one_block(AGENT, s, enter(1), exit(1, 1, 40_000_000));
    assert_eq!(r[0].status, Status::Ok, "{}", String::from_utf8_lossy(&r[0].error));
    assert_ne!(r[1].status, Status::Ok);
    assert!(String::from_utf8_lossy(&r[1].error).contains("later block than its entry"));
    // a no-fill exit spends nothing
    c.fails(AGENT, s, 0, exit(1, 5, 1), "no fill: nothing was bought");
    c.ok(AGENT, s, 0, exit(1, 5, 50_000_000));
    let (r, _) = c.ok(AGENT, s, 0, pm(1, B_EXECUTION));
    assert_eq!(r.ret, vec![4], "outcome no fill");
    let v = version(&c, &s, 1);
    assert_eq!((v.no_fills, v.pnl, v.bucket[2]), (1, 0, 1));
    // only the author trades a version; bad fields refused
    c.fails(STRANGER, s, 0, enter(1), "only the version's author");
    c.fails(AGENT, s, 0, In::m("enter").u64(1).addr(&h(7)).u64(0).u64(1).u64(1).u32(800).u32(1500).u64(1).u64(1).str("r").b(), "must be positive");
    c.fails(AGENT, s, 0, In::m("enter").u64(1).addr(&h(7)).u64(1).u64(1).u64(1).u32(10_000).u32(1500).u64(1).u64(1).str("r").b(), "stop_bps");
    c.fails(AGENT, s, 0, In::m("enter").u64(1).addr(&h(7)).u64(1).u64(1).u64(1).u32(800).u32(1500).u64(1).u64(1).str("").b(), "decision_ref");
    c.fails(AGENT, s, 5, enter(1), "takes no value");
}

#[test]
fn learning_rule_a_revision_cites_a_fixable_bucket() {
    let mut c = Chain::new();
    let s = deploy(&mut c, 0);
    c.ok(AGENT, s, 0, publish(1, 0, 0, &[]));
    c.fails(AGENT, s, 0, publish(2, 0, 1, &[]), "a new line claims no fix");
    let risk = trade(&mut c, s, 1, 40_000_000, B_RISK);
    let var = trade(&mut c, s, 1, 47_000_000, B_VARIANCE);
    let info = trade(&mut c, s, 1, 44_000_000, B_INFORMATION | B_SIZING);
    // no motive, VARIANCE, LEVERAGE, unknown: refused
    c.fails(AGENT, s, 0, publish(2, 1, 5, &[]), "must cite the post-mortem");
    c.fails(AGENT, s, 0, publish(2, 1, 7, &[var]), "fixable bucket 1..5");
    c.fails(AGENT, s, 0, publish(2, 1, 6, &[risk]), "fixable bucket 1..5");
    c.fails(AGENT, s, 0, publish(2, 1, 0, &[risk]), "fixable bucket 1..5");
    // a motive that does not name the claimed bucket; a VARIANCE-only post-mortem never motivates
    c.fails(AGENT, s, 0, publish(2, 1, 5, &[var]), "does not name the claimed bucket");
    c.fails(AGENT, s, 0, publish(2, 1, 1, &[risk]), "does not name the claimed bucket");
    c.fails(AGENT, s, 0, publish(2, 1, 5, &[risk, risk]), "cited twice");
    c.fails(AGENT, s, 0, publish(2, 1, 5, &[99]), "no post-mortem");
    c.fails(STRANGER, s, 0, publish(2, 1, 5, &[risk]), "only the parent's author");
    // the fix: claimed RISK_MANAGEMENT (5), motivated by the RISK post-mortem
    let (r, sum) = c.ok(AGENT, s, 0, publish(2, 1, 5, &[risk]));
    let v2 = ret_u64(&r);
    assert!(sum.events.iter().any(|e| e.topics[0] == ac::event_topic(b"Version")));
    let p = version(&c, &s, 1);
    let v = version(&c, &s, v2);
    assert_eq!((p.superseded_by, v.parent, v.line, v.claimed, v.motives.clone()), (v2, 1, 1, 5, vec![risk]));
    // the parent no longer trades or forks; the motive cannot be reused
    c.fails(AGENT, s, 0, enter(1), "superseded");
    c.fails(AGENT, s, 0, publish(3, 1, 1, &[info]), "not the head of its line");
    let (r, _) = c.ok(AGENT, s, 0, enter(v2));
    let e2 = ret_u64(&r);
    c.fails(AGENT, s, 0, publish(3, v2, 5, &[risk]), "open trade");
    c.ok(AGENT, s, 0, exit(e2, 2, 55_000_000));
    c.ok(AGENT, s, 0, pm(e2, 0));
    c.fails(AGENT, s, 0, publish(3, v2, 5, &[risk]), "already motivated another version");
    // a post-mortem of the parent's trades may still motivate the next fix in the same line
    let (r, _) = c.ok(AGENT, s, 0, publish(3, v2, 1, &[info]));
    assert_eq!(version(&c, &s, ret_u64(&r)).line, 1);
    // another agent's line cannot be cited
    c.ok(STRANGER, s, 0, publish(4, 0, 0, &[]));
    let other = version(&c, &s, 4);
    assert_eq!(other.line, 4);
}

#[test]
fn start_height_gates_every_call_and_the_code_is_immutable() {
    let mut c = Chain::new();
    let start = c.height + 6;
    let s = deploy(&mut c, start);
    c.fails(AGENT, s, 0, publish(1, 0, 0, &[]), "not active before start_height");
    c.skip(6);
    c.ok(AGENT, s, 0, publish(1, 0, 0, &[]));
    let b = c.q(&s, In::m("counts").b());
    assert_eq!((u64_at(&b, 0), u64_at(&b, 8), u64_at(&b, 16)), (1, 0, start));
    let code = ac::code_hash(STRAT);
    let r = c.send(ADMIN, TxBody::Migrate(ac::Migrate { contract: s, code_hash: code, gas_limit: 1_000_000, args: Vec::new() }));
    assert_ne!(r.receipts[0].status, Status::Ok);
    assert!(String::from_utf8_lossy(&r.receipts[0].error).contains("immutable"));
}
