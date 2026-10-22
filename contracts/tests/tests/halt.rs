//! The protocol halt record (contracts/halt, docs/ECOSYSTEM.md 7.8): only the halt authority sets
//! it, scopes are a fixed bit mask, every change is an event with a sequence number, the record
//! reads back in the layout cadre-api and the agent runners decode, and the code is immutable.
#![allow(dead_code)]

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const HALT: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_halt.wasm");

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

const AUTH: u8 = ADMIN;

fn status(c: &Chain, h: &Pubkey) -> (u8, u64, u64, Pubkey, String) {
    let b = c.q(h, In::m("status").b());
    let n = u32::from_le_bytes(b[49..53].try_into().unwrap()) as usize;
    assert_eq!(b.len(), 53 + n, "status layout: scope u8, seq u64, height u64, authority [32], reason (u32 len + bytes)");
    (b[0], u64_at(&b, 1), u64_at(&b, 9), b[17..49].try_into().unwrap(), String::from_utf8(b[53..].to_vec()).unwrap())
}

fn set(scope: u8, reason: &str) -> Vec<u8> {
    In::m("set").u8(scope).str(reason).b()
}

fn deploy_halt(c: &mut Chain) -> Pubkey {
    let code = c.deploy(ADMIN, HALT);
    c.instantiate(ADMIN, code, In::raw().addr(&pk(AUTH)).b())
}

#[test]
fn authority_sets_and_clears_the_halt() {
    let mut c = Chain::new();
    let h = deploy_halt(&mut c);
    let (scope, seq, _, auth, reason) = status(&c, &h);
    assert_eq!((scope, seq, auth, reason.as_str()), (0, 0, pk(AUTH), ""));
    let (r, s) = c.ok(AUTH, h, 0, set(1, "incident 1: agents paused while a bug is fixed"));
    eprintln!("gas: halt set {} (fee {})", r.gas_used, r.fee_charged);
    let ev = s.events.iter().find(|e| e.topics[0] == ac::event_topic(b"Halt")).expect("Halt event");
    assert_eq!(ev.topics[1], pk(AUTH));
    let (scope, seq, height, _, reason) = status(&c, &h);
    assert_eq!((scope, seq, height), (1, 1, c.height));
    assert!(reason.starts_with("incident 1"));
    c.ok(AUTH, h, 0, set(2 | 4, "trading and launches only"));
    assert_eq!(status(&c, &h).0, 6);
    c.ok(AUTH, h, 0, set(0, "resumed"));
    let (scope, seq, _, _, reason) = status(&c, &h);
    assert_eq!((scope, seq, reason.as_str()), (0, 3, "resumed"));
}

#[test]
fn only_the_authority_and_only_known_scopes() {
    let mut c = Chain::new();
    let h = deploy_halt(&mut c);
    c.fails(STRANGER, h, 0, set(1, "not mine"), "only the halt authority");
    c.fails(AUTH, h, 0, set(8, "x"), "unknown scope bits");
    c.fails(AUTH, h, 0, set(1, &"x".repeat(281)), "reason above 280 bytes");
    c.fails(AUTH, h, 5, set(1, "with value"), "takes no value");
    assert_eq!(status(&c, &h).1, 0, "nothing changed");
    // hand the authority on; the old one loses it
    c.fails(AUTH, h, 0, In::m("set_authority").addr(&[0; 32]).b(), "authority must be set");
    c.ok(AUTH, h, 0, In::m("set_authority").addr(&pk(STRANGER)).b());
    c.fails(AUTH, h, 0, set(1, "old authority"), "only the halt authority");
    c.ok(STRANGER, h, 0, set(1, "new authority"));
    let (scope, seq, _, auth, _) = status(&c, &h);
    assert_eq!((scope, seq, auth), (1, 2, pk(STRANGER)));
}

#[test]
fn deployed_immutable_nobody_can_migrate_it() {
    let mut c = Chain::new();
    let h = deploy_halt(&mut c);
    let code = ac::code_hash(HALT);
    let s = c.send(ADMIN, TxBody::Migrate(ac::Migrate { contract: h, code_hash: code, gas_limit: 1_000_000, args: Vec::new() }));
    assert_ne!(s.receipts[0].status, Status::Ok);
    assert!(String::from_utf8_lossy(&s.receipts[0].error).contains("immutable"));
}
