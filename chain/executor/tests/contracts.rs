//! Contracts in the executor (docs/CONTRACTS.md): deploy / instantiate / call / query, the gas
//! fee rule and refunds, failed calls landing with their state undone, storage deposits,
//! cross-contract calls, no reentrancy, call depth, native access (jobs, town posts), the
//! block gas limit, migration and admin rules, activation height, the contracts record, and a
//! determinism digest compared across machines.

use agent_types::contracts::{self as ac, contract_address, Status, DEVNET_CHAIN_ID, DEVNET_CONTRACTS_HEIGHT};
use agent_types::tx::{pubkey_from_seed, result_message, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::{keys, Config, GAS_PRICE_UNIT};
use state::contracts::{self as sc, Contract, StoredReceipt};
use state::schema::{self, Account};
use state::{MemState, StateRead};

const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const ADMIN: u8 = 1;
const EVAL: u8 = 60;
const CADRE: u64 = 1_000_000_000;

const COUNTER: &[u8] = include_bytes!("../../contracts/testdata/counter.wasm");
const ESCROW: &[u8] = include_bytes!("../../contracts/testdata/escrow.wasm");
const TOKEN: &[u8] = include_bytes!("../../contracts/testdata/token.wasm");
const BOUNTY: &[u8] = include_bytes!("../../contracts/testdata/bounty.wasm");
const TESTKIT: &[u8] = include_bytes!("../../contracts/testdata/testkit.wasm");

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

/// ABI input: method (u8 len) then fields.
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
    fn u32(mut self, x: u32) -> Self {
        self.0.extend_from_slice(&x.to_le_bytes());
        self
    }
    fn u64(mut self, x: u64) -> Self {
        self.0.extend_from_slice(&x.to_le_bytes());
        self
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
    chain_id: u32,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

impl Chain {
    fn new(chain_id: u32) -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, chain_id, height: 0, nonces: Default::default() };
        for who in [2u8, 3, 4, 5, 6, 7, ADMIN] {
            c.fund(pk(who), 1_000_000_000); // 1000 CADRE each
        }
        c
    }

    fn block(&mut self, txs: &[Tx]) -> Result<BlockSummary, String> {
        let env = BlockEnv { chain_id: self.chain_id, height: self.height + 1, miner: pk(MINER), job_id: None };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        // the record is present exactly when the block has contract txs
        let has = txs.iter().any(|t| t.body.kind().is_contract());
        assert_eq!(sum.contracts.is_some(), has);
        if let Some(r) = &sum.contracts {
            assert_eq!(*r, ac::ContractsRecord::build(self.height, &sum.receipts, &sum.events));
        }
        Ok(sum)
    }

    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let nonce = 10_000 + self.height;
        let msg = agent_types::tx::deposit_message(nonce, &who, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = *self.nonces.get(&200).unwrap_or(&0);
        let t = Tx::signed(&seed(200), self.chain_id, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl, job_order: None, sigs });
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

    fn tx(&self, who: u8, fee: u64, body: TxBody) -> Tx {
        Tx::signed(&seed(who), self.chain_id, self.nonce(who), fee, body)
    }
    /// The contract fee floor of `body` from `who`.
    fn floor(&self, who: u8, body: &TxBody) -> u64 {
        let t = self.tx(who, 0, body.clone());
        contract_fee_floor(&self.cfg(), t.encode().len(), executor::gas_limit_of(body))
    }
    fn send(&mut self, who: u8, body: TxBody) -> Result<BlockSummary, String> {
        let fee = self.floor(who, &body);
        let t = self.tx(who, fee, body);
        let r = self.block(&[t]);
        if r.is_ok() {
            *self.nonces.entry(who).or_insert(0) += 1;
        }
        r
    }
    fn receipt(&mut self, who: u8, body: TxBody) -> ac::Receipt {
        let s = self.send(who, body).unwrap();
        assert_eq!(s.receipts.len(), 1);
        s.receipts[0].clone()
    }

    fn deploy(&mut self, who: u8, code: &[u8]) -> Hash {
        let gas = ::contracts::deploy_cost(code.len());
        let r = self.receipt(who, TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code: code.to_vec() }));
        assert_eq!(r.status, Status::Ok);
        ac::code_hash(code)
    }
    fn instantiate(&mut self, who: u8, code_hash: Hash, admin: Pubkey, value: u64, args: Vec<u8>) -> (Pubkey, ac::Receipt) {
        let addr = contract_address(&pk(who), self.nonce(who));
        let r = self.receipt(who, TxBody::Instantiate(ac::Instantiate { code_hash, admin, value, gas_limit: 50_000_000, label: b"test".to_vec(), args }));
        (addr, r)
    }
    fn call(&mut self, who: u8, contract: Pubkey, value: u64, args: Vec<u8>) -> ac::Receipt {
        self.call_gas(who, contract, value, args, 100_000_000)
    }
    fn call_gas(&mut self, who: u8, contract: Pubkey, value: u64, args: Vec<u8>, gas_limit: u64) -> ac::Receipt {
        self.receipt(who, TxBody::Call(ac::Call { contract, value, gas_limit, args }))
    }
    fn query(&self, contract: &Pubkey, args: Vec<u8>) -> executor::QueryResult {
        block_on(query_contract(&self.st, self.chain_id, self.height + 1, contract, args, 50_000_000)).unwrap()
    }
    fn query_u64(&self, contract: &Pubkey, args: Vec<u8>) -> u64 {
        let r = self.query(contract, args);
        assert!(r.ok, "{}", String::from_utf8_lossy(&r.error));
        u64::from_le_bytes(r.ret[..8].try_into().unwrap())
    }
    fn contract(&self, a: &Pubkey) -> Contract {
        Contract::decode(&self.get(&sc::contract_key(a)).unwrap()).unwrap()
    }

    /// Order-independent digest of the whole state (for cross-machine comparison).
    fn digest(&self) -> Hash {
        let m = self.st.map.lock().unwrap();
        let mut h = blake3::Hasher::new();
        for (k, v) in m.iter() {
            h.update(k);
            h.update(&(v.len() as u64).to_le_bytes());
            h.update(v);
        }
        *h.finalize().as_bytes()
    }
}

#[test]
fn counter_end_to_end_with_fees_and_receipts() {
    let mut c = Chain::new(7);
    let cfg = c.cfg();
    let b0 = c.bal(&pk(2));
    let ch = c.deploy(2, COUNTER);
    let deposit = COUNTER.len() as u64 * cfg.storage_byte_deposit;
    assert!(c.bal(&pk(2)) < b0 - deposit, "code deposit and fee paid");
    // deploying the same code again is refused (invalid tx)
    let gas = ::contracts::deploy_cost(COUNTER.len());
    assert!(c.send(2, TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code: COUNTER.to_vec() })).is_err());
    let (addr, r) = c.instantiate(3, ch, pk(3), 0, In::raw().u64(41).b());
    assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(c.query_u64(&addr, In::m("get").b()), 41);
    let before = c.bal(&pk(4));
    let body = TxBody::Call(ac::Call { contract: addr, value: 0, gas_limit: 100_000_000, args: In::m("increment").u64(1).b() });
    let fee = c.floor(4, &body);
    let s = c.send(4, body).unwrap();
    let r = &s.receipts[0];
    assert_eq!(r.status, Status::Ok);
    assert_eq!(r.ret, 42u64.to_le_bytes());
    assert!(r.gas_used > 0 && r.gas_used < r.gas_limit);
    // charged = fee minus the unused gas
    let refund = (r.gas_limit - r.gas_used) as u128 * cfg.gas_price as u128 / GAS_PRICE_UNIT as u128;
    assert_eq!(r.fee_charged, fee - refund as u64);
    assert_eq!(before - c.bal(&pk(4)), r.fee_charged);
    assert_eq!(s.fees, r.fee_charged, "block fees count what was charged");
    // the event, and its proof path under the contracts record
    assert_eq!(s.events.len(), 1);
    assert_eq!(s.events[0].topics[0], ac::event_topic(b"Incremented"));
    let rec = s.contracts.unwrap();
    assert!(ac::verify_path(&s.events[0].hash(), &[], &rec.events_root));
    // the stored receipt
    let sr = StoredReceipt::decode(&c.get(&sc::receipt_key(&r.tx_hash)).unwrap()).unwrap();
    assert_eq!(sr.receipt, *r);
    assert_eq!(sr.events, s.events);
    assert_eq!(c.query_u64(&addr, In::m("get").b()), 42);
    let k = c.contract(&addr);
    assert_eq!((k.calls, k.events), (2, 1));
    // only the owner may reset
    assert_eq!(c.call(4, addr, 0, In::m("reset").b()).status, Status::Reverted);
    assert_eq!(c.call(3, addr, 0, In::m("reset").b()).status, Status::Ok);
    assert_eq!(c.query_u64(&addr, In::m("get").b()), 0);
}

#[test]
fn failures_land_and_undo_state() {
    let mut c = Chain::new(7);
    let ch = c.deploy(2, TESTKIT);
    let (kit, _) = c.instantiate(2, ch, [0; 32], 0, vec![]);
    // revert: no storage, no events, fee + nonce taken
    let n = c.nonce(3);
    let b = c.bal(&pk(3));
    let r = c.call(3, kit, 0, In::m("write_then_fail").b());
    assert_eq!(r.status, Status::Reverted);
    assert_eq!(r.error, b"failing on purpose");
    assert_eq!(r.event_count, 0);
    assert_eq!(c.nonce(3), n + 1);
    assert_eq!(b - c.bal(&pk(3)), r.fee_charged);
    assert!(!c.query(&kit, In::m("get").bytes(b"x").b()).ok, "the write was undone");
    // out of gas: the whole gas limit is charged
    let r = c.call_gas(3, kit, 0, In::m("burn").u64(10_000_000).b(), 2_000_000);
    assert_eq!(r.status, Status::OutOfGas);
    assert_eq!(r.gas_used, 2_000_000);
    // a value call that fails gives the value back
    let b = c.bal(&pk(3));
    let r = c.call(3, kit, 5 * CADRE, In::m("write_then_fail").b());
    assert_eq!(r.status, Status::Reverted);
    assert_eq!(b - c.bal(&pk(3)), r.fee_charged);
    assert_eq!(c.bal(&kit), 0);
    // calling nothing is refused but still lands
    let r = c.call(3, pk(77), 0, In::m("x").b());
    assert_eq!(r.status, Status::Refused);
    assert_eq!(r.gas_used, 0);
}

#[test]
fn cross_contract_calls_and_no_reentrancy() {
    let mut c = Chain::new(7);
    let kit = c.deploy(2, TESTKIT);
    let cnt = c.deploy(2, COUNTER);
    let (a, _) = c.instantiate(2, kit, [0; 32], 0, vec![]);
    let (b, _) = c.instantiate(2, kit, [0; 32], 0, vec![]);
    let (counter, _) = c.instantiate(2, cnt, [0; 32], 0, In::raw().u64(0).b());
    // a -> counter.increment
    let inner = In::m("increment").u64(5).b();
    let r = c.call(3, a, 0, In::m("forward").addr(&counter).u64(0).u64(0).bytes(&inner).b());
    assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(r.ret, 5u64.to_le_bytes());
    assert_eq!(c.query_u64(&counter, In::m("get").b()), 5);
    // a -> b -> a is refused (a is on the stack), and the whole tx reverts
    let r = c.call(3, a, 0, In::m("reenter").addr(&b).b());
    assert_eq!(r.status, Status::Reverted);
    assert_eq!(r.error, b"reentry refused");
    // a -> a directly is refused too
    let back = In::m("burn").u64(1).b();
    let r = c.call(3, a, 0, In::m("try_forward").addr(&a).u64(0).u64(0).bytes(&back).b());
    assert_eq!(r.status, Status::Ok);
    assert_eq!(r.ret[0], 0, "inner call failed");
    assert!(String::from_utf8_lossy(&r.ret[5..]).contains("reentrancy refused"));
    // a failing callee's writes are undone while the caller continues
    let fail = In::m("write_then_fail").b();
    let r = c.call(3, a, 0, In::m("try_forward").addr(&b).u64(0).u64(0).bytes(&fail).b());
    assert_eq!(r.status, Status::Ok);
    assert!(!c.query(&b, In::m("get").bytes(b"x").b()).ok);
    // call depth: a chain of forwards deeper than max_call_depth fails
    let depth = c.cfg().max_call_depth as usize;
    let kits: Vec<Pubkey> = (0..depth + 1).map(|_| c.instantiate(2, kit, [0; 32], 0, vec![]).0).collect();
    let mut input = In::m("burn").u64(1).b();
    for k in kits[1..].iter().rev() {
        input = In::m("forward").addr(k).u64(0).u64(0).bytes(&input).b();
    }
    let r = c.call(3, kits[0], 0, input.clone());
    assert_eq!(r.status, Status::Reverted, "depth {} must fail", depth + 1);
    // value moves with nested calls: fund a, a pays b, b pays out
    assert_eq!(c.call(3, a, 3 * CADRE, In::m("burn").u64(1).b()).status, Status::Ok);
    let pay = In::m("pay").addr(&pk(6)).u64(CADRE).b();
    let b6 = c.bal(&pk(6));
    let r = c.call(3, a, 0, In::m("forward").addr(&b).u64(2 * CADRE).u64(0).bytes(&pay).b());
    assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(c.bal(&a), CADRE);
    assert_eq!(c.bal(&b), CADRE);
    assert_eq!(c.bal(&pk(6)) - b6, CADRE);
}

#[test]
fn storage_deposits_charge_growth_and_refund_shrinkage() {
    let mut c = Chain::new(7);
    let kit = c.deploy(2, TESTKIT);
    let (a, _) = c.instantiate(2, kit, [0; 32], 0, vec![]);
    let price = c.cfg().storage_byte_deposit;
    let b = c.bal(&pk(3));
    let r = c.call(3, a, 0, In::m("fill").u32(10).u32(100).b());
    assert_eq!(r.status, Status::Ok);
    let k = c.contract(&a);
    assert_eq!(k.storage_items, 10);
    // key "f:" + 4 bytes = 6, value 100, overhead 64
    assert_eq!(k.storage_bytes, 10 * (6 + 100 + sc::ITEM_OVERHEAD));
    assert_eq!(k.deposit, k.storage_bytes * price);
    assert_eq!(b - c.bal(&pk(3)), r.fee_charged + k.deposit);
    // someone else frees half and gets half the deposit back
    let b4 = c.bal(&pk(4));
    let r = c.call(4, a, 0, In::m("clear").u32(5).b());
    let k2 = c.contract(&a);
    assert_eq!(k2.storage_items, 5);
    assert_eq!(k2.deposit, k.deposit / 2);
    assert_eq!(c.bal(&pk(4)) + r.fee_charged - b4, k.deposit / 2);
    // a sender who cannot cover the deposit: refused, nothing written
    let poor = 8u8;
    c.fund(pk(poor), 5_000); // 5 CADRE
    let r = c.call(poor, a, 0, In::m("fill").u32(50).u32(1000).b());
    assert_eq!(r.status, Status::Refused, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(c.contract(&a).storage_items, 5);
}

#[test]
fn escrow_token_and_bounty_flows() {
    let mut c = Chain::new(7);
    // escrow
    let ech = c.deploy(2, ESCROW);
    let (esc, _) = c.instantiate(2, ech, [0; 32], 0, vec![]);
    let h = c.height;
    let r = c.call(3, esc, 10 * CADRE, In::m("open").addr(&pk(4)).addr(&pk(5)).u64(h + 100).b());
    assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(c.bal(&esc), 10 * CADRE);
    assert_eq!(c.call(4, esc, 0, In::m("release").u64(0).b()).status, Status::Reverted, "payee cannot release");
    assert_eq!(c.call(3, esc, 0, In::m("refund").u64(0).b()).status, Status::Reverted, "payer cannot refund before the deadline");
    let b4 = c.bal(&pk(4));
    assert_eq!(c.call(5, esc, 0, In::m("release").u64(0).b()).status, Status::Ok);
    assert_eq!(c.bal(&pk(4)) - b4, 10 * CADRE);
    assert_eq!(c.call(5, esc, 0, In::m("release").u64(0).b()).status, Status::Reverted, "no double release");

    // token
    let tch = c.deploy(2, TOKEN);
    let (tok, r) = c.instantiate(3, tch, [0; 32], 0, In::raw().str("Lamp Oil").str("OIL").u8(6).u64(1_000_000).u8(1).b());
    assert_eq!(r.status, Status::Ok, "{}", String::from_utf8_lossy(&r.error));
    assert_eq!(c.query_u64(&tok, In::m("balance_of").addr(&pk(3)).b()), 1_000_000);
    assert_eq!(c.call(3, tok, 0, In::m("transfer").addr(&pk(4)).u64(250).b()).status, Status::Ok);
    assert_eq!(c.call(4, tok, 0, In::m("transfer").addr(&pk(5)).u64(251).b()).status, Status::Reverted);
    assert_eq!(c.call(4, tok, 0, In::m("approve").addr(&pk(5)).u64(100).b()).status, Status::Ok);
    assert_eq!(c.call(5, tok, 0, In::m("transfer_from").addr(&pk(4)).addr(&pk(6)).u64(60).b()).status, Status::Ok);
    assert_eq!(c.call(5, tok, 0, In::m("transfer_from").addr(&pk(4)).addr(&pk(6)).u64(60).b()).status, Status::Reverted);
    assert_eq!(c.query_u64(&tok, In::m("balance_of").addr(&pk(6)).b()), 60);
    assert_eq!(c.query_u64(&tok, In::m("allowance").addr(&pk(4)).addr(&pk(5)).b()), 40);
    assert_eq!(c.call(4, tok, 0, In::m("mint").addr(&pk(4)).u64(1).b()).status, Status::Reverted, "only the minter");
    assert_eq!(c.call(3, tok, 0, In::m("mint").addr(&pk(4)).u64(1).b()).status, Status::Ok);

    // bounty: post with a town announcement, two submissions, award
    let bch = c.deploy(2, BOUNTY);
    let (bty, _) = c.instantiate(2, bch, [0; 32], 0, vec![]);
    let h = c.height;
    let s = c
        .send(3, TxBody::Call(ac::Call { contract: bty, value: 20 * CADRE, gas_limit: 100_000_000,
            args: In::m("post").str("Summarize block 100").str("").u64(h + 50).addr(&[0; 32]).u8(1).b() }))
        .unwrap();
    assert_eq!(s.receipts[0].status, Status::Ok, "{}", String::from_utf8_lossy(&s.receipts[0].error));
    // the contract posted to the town as itself (its nonce numbers the post)
    let post_id = agent_types::social::post_id(&bty, 0);
    assert!(c.get(&state::social::post_key(&post_id)).is_some(), "town post by the contract");
    assert_eq!(c.call(4, bty, 0, In::m("submit").u64(0).str("blob:aa").bytes(&[]).b()).status, Status::Reverted, "hash is 32 bytes");
    let sub = |who: u8, c: &mut Chain| c.call(who, bty, 0, { let mut v = In::m("submit").u64(0).str("ipfs://x").b(); v.extend_from_slice(&[who; 32]); v });
    assert_eq!(sub(4, &mut c).status, Status::Ok);
    assert_eq!(sub(5, &mut c).status, Status::Ok);
    assert_eq!(c.call(4, bty, 0, In::m("award").u64(0).u32(1).b()).status, Status::Reverted, "only the judge");
    let b5 = c.bal(&pk(5));
    assert_eq!(c.call(3, bty, 0, In::m("award").u64(0).u32(1).b()).status, Status::Ok);
    assert_eq!(c.bal(&pk(5)) - b5, 20 * CADRE);

    // a bounty linked to a native job pays the job's deliverer once it is completed
    let h = c.height;
    assert_eq!(c.call(3, bty, 7 * CADRE, In::m("post").str("Run my matmul").str("").u64(h + 50).addr(&[0; 32]).u8(0).b()).status, Status::Ok);
    let job = sample_job(h + 40);
    let job_nonce = c.nonce(6);
    let fee = 1000;
    let t = c.tx(6, fee, TxBody::PostJob(job));
    c.block(&[t]).unwrap();
    *c.nonces.entry(6).or_insert(0) += 1;
    let job_id = schema::job_id_for_post(&pk(6), job_nonce);
    let mut link = In::m("link_job").u64(1).b();
    link.extend_from_slice(&job_id);
    assert_eq!(c.call(3, bty, 0, link).status, Status::Ok);
    assert_eq!(c.call(4, bty, 0, In::m("claim_job").u64(1).b()).status, Status::Reverted, "not completed yet");
    let rh = [0x44; 32];
    let t = c.tx(7, 1000, TxBody::SubmitResult { job_id, result_hash: rh, uri: vec![] });
    c.block(&[t]).unwrap();
    *c.nonces.entry(7).or_insert(0) += 1;
    let msg = result_message(&job_id, &pk(7), &rh);
    let sig = AttesterSig { pubkey: pk(EVAL), sig: sign_plain(&seed(EVAL), &msg) };
    let t = c.tx(6, 1000, TxBody::CompleteJob { job_id, sigs: vec![sig] });
    c.block(&[t]).unwrap();
    *c.nonces.entry(6).or_insert(0) += 1;
    let b7 = c.bal(&pk(7));
    assert_eq!(c.call(4, bty, 0, In::m("claim_job").u64(1).b()).status, Status::Ok);
    assert_eq!(c.bal(&pk(7)) - b7, 7 * CADRE, "the deliverer got the bounty");
}

fn sample_job(deadline: u64) -> PostJob {
    let cfg = pearl_job::MiningConfig::localnet_default();
    PostJob {
        m: 4,
        n: 8,
        k: cfg.common_dim,
        config: cfg.to_bytes(),
        hash_a: [1; 32],
        hash_b: [2; 32],
        data_uri: Vec::new(),
        worker_pay: 3 * CADRE,
        protocol_fee: 1_000_000,
        deadline_height: deadline,
        max_blocks: 10,
        evaluators: vec![pk(EVAL)],
        threshold: 1,
    }
}

#[test]
fn block_gas_limit_and_fee_floor() {
    let mut c = Chain::new(7);
    let kit = c.deploy(2, TESTKIT);
    let (a, _) = c.instantiate(2, kit, [0; 32], 0, vec![]);
    // shrink the block gas limit to 3x a tx's limit
    let lim = 100_000_000u64;
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::TX_GAS_LIMIT, value: lim.to_le_bytes().to_vec() });
    c.block(&[t]).unwrap();
    *c.nonces.entry(ADMIN).or_insert(0) += 1;
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::BLOCK_GAS_LIMIT, value: (3 * lim).to_le_bytes().to_vec() });
    c.block(&[t]).unwrap();
    *c.nonces.entry(ADMIN).or_insert(0) += 1;
    // four spinning calls of 100M gas each: the fourth does not fit
    let mk = |c: &Chain, who: u8| {
        let body = TxBody::Call(ac::Call { contract: a, value: 0, gas_limit: lim, args: In::m("burn").u64(1_000_000_000).b() });
        let fee = c.floor(who, &body);
        c.tx(who, fee, body)
    };
    let txs: Vec<Tx> = (3..=6).map(|w| mk(&c, w)).collect();
    let err = c.block(&txs).unwrap_err();
    assert!(err.contains("remaining gas"), "{err}");
    let s = c.block(&txs[..3]).unwrap();
    assert_eq!(s.gas_used, 3 * lim);
    assert!(s.receipts.iter().all(|r| r.status == Status::OutOfGas));
    for w in 3..=5 {
        *c.nonces.entry(w).or_insert(0) += 1;
    }
    // a gas limit above tx_gas_limit, and an underpaid fee, are invalid
    let body = TxBody::Call(ac::Call { contract: a, value: 0, gas_limit: lim + 1, args: vec![] });
    let t = c.tx(6, 1_000_000_000, body);
    assert!(c.block(&[t]).is_err());
    let body = TxBody::Call(ac::Call { contract: a, value: 0, gas_limit: lim, args: vec![] });
    let fee = c.floor(6, &body);
    let t = c.tx(6, fee - 1, body);
    assert!(c.block(&[t]).unwrap_err().contains("below"));
}

#[test]
fn migrate_and_admin_rules() {
    let mut c = Chain::new(7);
    let cnt = c.deploy(2, COUNTER);
    let kit = c.deploy(2, TESTKIT);
    let (upg, _) = c.instantiate(3, cnt, pk(3), 0, In::raw().u64(9).b());
    let (frozen, _) = c.instantiate(3, cnt, [0; 32], 0, In::raw().u64(9).b());
    let mig = |a: Pubkey, h: Hash| TxBody::Migrate(ac::Migrate { contract: a, code_hash: h, gas_limit: 10_000_000, args: vec![] });
    assert_eq!(c.receipt(4, mig(upg, kit)).status, Status::Refused, "not the admin");
    assert_eq!(c.receipt(3, mig(frozen, kit)).status, Status::Refused, "immutable");
    assert_eq!(c.receipt(3, mig(upg, kit)).status, Status::Ok);
    assert_eq!(c.contract(&upg).code_hash, kit);
    // storage survives a migration: the testkit reads the counter's "n"
    let r = c.query(&upg, In::m("get").bytes(b"n").b());
    assert_eq!(r.ret, 9u64.to_le_bytes());
    // hand the admin to nobody: frozen for good
    let r = c.receipt(3, TxBody::SetContractAdmin { contract: upg, admin: [0; 32] });
    assert_eq!(r.status, Status::Ok);
    assert_eq!(c.receipt(3, mig(upg, cnt)).status, Status::Refused);
}

#[test]
fn activation_height_on_devnet() {
    let mut c = Chain::new(DEVNET_CHAIN_ID);
    assert!(c.height < DEVNET_CONTRACTS_HEIGHT);
    let gas = ::contracts::deploy_cost(COUNTER.len());
    let err = c.send(2, TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code: COUNTER.to_vec() })).unwrap_err();
    assert!(err.contains("activate at height"), "{err}");
    let err = c.send(ADMIN, TxBody::AdminConfig { key: keys::GAS_PRICE, value: 5u64.to_le_bytes().to_vec() }).unwrap_err();
    assert!(err.contains("activates at height"), "{err}");
    // jump to the height (state carries no height, so only the env changes)
    c.height = DEVNET_CONTRACTS_HEIGHT - 1;
    c.deploy(2, COUNTER);
    // the config record keeps its short encoding until a contract key is set
    let before = c.get(&schema::config_key()).unwrap().len();
    let t = c.tx(ADMIN, 1000, TxBody::AdminConfig { key: keys::GAS_PRICE, value: 5u64.to_le_bytes().to_vec() });
    c.block(&[t]).unwrap();
    assert!(c.get(&schema::config_key()).unwrap().len() > before);
    assert_eq!(c.cfg().gas_price, 5);
}

/// One fixed scenario; its state digest must be the same on every machine. Set
/// `CADRE_DIGEST=1` to print it (compare the Mac and the test box).
#[test]
fn determinism_digest() {
    let run = || {
        let mut c = Chain::new(7);
        let kit = c.deploy(2, TESTKIT);
        let tok = c.deploy(2, TOKEN);
        let (a, _) = c.instantiate(2, kit, [0; 32], 0, vec![]);
        let (t, _) = c.instantiate(3, tok, [0; 32], 0, In::raw().str("T").str("T").u8(0).u64(1000).u8(0).b());
        c.call(3, t, 0, In::m("transfer").addr(&pk(4)).u64(7).b());
        c.call(4, a, 0, In::m("fill").u32(20).u32(33).b());
        c.call_gas(5, a, 0, In::m("burn").u64(1_000_000).b(), 3_000_000);
        c.call(5, a, 0, In::m("burn").u64(10_000).b());
        let inner = In::m("transfer").addr(&pk(6)).u64(1).b();
        c.call(4, a, 0, In::m("try_forward").addr(&t).u64(0).u64(0).bytes(&inner).b());
        c.digest()
    };
    let d1 = run();
    let d2 = run();
    assert_eq!(d1, d2);
    if std::env::var("CADRE_DIGEST").is_ok() {
        println!("CADRE_DIGEST {}", hex::encode(d1));
    }
}

/// Gas of the example operations under the default config (docs/CONTRACTS.md "Measured";
/// `cargo test -p executor --test contracts gas_table -- --nocapture`).
#[test]
fn gas_table() {
    let mut c = Chain::new(7);
    let cfg = c.cfg();
    let mut rows: Vec<(String, u64, u64)> = Vec::new();
    let mut note = |name: &str, r: &ac::Receipt| {
        assert_eq!(r.status, Status::Ok, "{name}: {}", String::from_utf8_lossy(&r.error));
        rows.push((name.to_string(), r.gas_used, r.fee_charged));
    };
    for (n, code) in [("counter", COUNTER), ("escrow", ESCROW), ("token", TOKEN), ("bounty", BOUNTY)] {
        let gas = ::contracts::deploy_cost(code.len());
        let r = c.receipt(2, TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code: code.to_vec() }));
        note(&format!("deploy {n} ({} bytes)", code.len()), &r);
    }
    let (ctr, r) = c.instantiate(3, ac::code_hash(COUNTER), [0; 32], 0, In::raw().u64(0).b());
    note("instantiate counter", &r);
    note("counter increment", &c.call(3, ctr, 0, In::m("increment").u64(1).b()));
    let (tok, r) = c.instantiate(3, ac::code_hash(TOKEN), [0; 32], 0, In::raw().str("Oil").str("OIL").u8(6).u64(1_000_000).u8(0).b());
    note("instantiate token", &r);
    note("token transfer (new holder)", &c.call(3, tok, 0, In::m("transfer").addr(&pk(4)).u64(5).b()));
    note("token transfer (existing holder)", &c.call(3, tok, 0, In::m("transfer").addr(&pk(4)).u64(5).b()));
    let (esc, _) = c.instantiate(3, ac::code_hash(ESCROW), [0; 32], 0, vec![]);
    let h = c.height;
    note("escrow open", &c.call(3, esc, CADRE, In::m("open").addr(&pk(4)).addr(&pk(5)).u64(h + 10).b()));
    note("escrow release", &c.call(3, esc, 0, In::m("release").u64(0).b()));
    let (bty, _) = c.instantiate(3, ac::code_hash(BOUNTY), [0; 32], 0, vec![]);
    let h = c.height;
    note("bounty post + town post", &c.call(3, bty, CADRE, In::m("post").str("Label 100 images").str("").u64(h + 10).addr(&[0; 32]).u8(1).b()));
    println!("gas_price {} per {} gas, min_fee {}", cfg.gas_price, GAS_PRICE_UNIT, cfg.min_fee);
    for (n, g, f) in &rows {
        println!("GAS {n:40} {g:>12} gas  fee {f:>10} base units");
    }
}
