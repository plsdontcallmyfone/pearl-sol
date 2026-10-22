//! Swarms (contracts/swarms, docs/SWARMS.md) in the real executor: a swarm is created only with
//! every member's own acceptance signature, the creator has no method after `create` (no removal,
//! no pause, no funds), members leave on their own, the lead proposes and the invited agent alone
//! accepts, funding goes straight to members. Supply invariant every block.

use agent_types::contracts::{self as ac, contract_address, Status};
use agent_types::tx::{deposit_message, pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{contract_fee_floor, execute_block, genesis_writes, query_contract, supply_of, BlockEnv, BlockSummary};
use futures::executor::block_on;
use state::config::Config;
use state::schema::{self, Account};
use state::{MemState, StateRead};

const SWARMS: &[u8] = include_bytes!("../../target/wasm32-unknown-unknown/release/cadre_swarms.wasm");

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
        eprintln!("gas: swarms deploy {} bytes {} gas, fee {}", code.len(), s.receipts[0].gas_used, s.receipts[0].fee_charged);
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

fn ret_u64(r: &ac::Receipt) -> u64 {
    u64_at(&r.ret, 0)
}

const CREATOR: u8 = BUYER;
const M0: u8 = 20; // members are seeds 20..
fn m(i: u8) -> u8 {
    M0 + i
}

fn deploy(c: &mut Chain, start: u64) -> Pubkey {
    let code = c.deploy(ADMIN, SWARMS);
    c.instantiate(ADMIN, code, In::raw().u64(start).b())
}

struct Def {
    creator: Pubkey,
    name: String,
    goal: [u8; 32],
    uri: String,
    lead: Pubkey,
    salt: u64,
    members: Vec<(u8, String, u64)>,
}

impl Def {
    fn new(n: u8) -> Self {
        let members = (0..n).map(|i| (m(i), if i == 0 { "lead".to_string() } else { format!("worker-{i}") }, 0u64)).collect();
        Def { creator: pk(CREATOR), name: "research crew".into(), goal: [7; 32], uri: "blob:goal".into(), lead: pk(m(0)), salt: 42, members }
    }
    fn def_query(&self) -> Vec<u8> {
        let mut i = In::m("definition").addr(&self.creator).str(&self.name).addr(&self.goal).str(&self.uri).addr(&self.lead).u64(self.salt).u8(self.members.len() as u8);
        for (who, role, _) in &self.members {
            i = i.addr(&pk(*who)).str(role);
        }
        i.b()
    }
}

fn def_hash(c: &Chain, s: &Pubkey, d: &Def) -> ([u8; 32], Vec<u8>) {
    let r = c.q(s, d.def_query());
    let h: [u8; 32] = r[..32].try_into().unwrap();
    let n = u32::from_le_bytes(r[32..36].try_into().unwrap()) as usize;
    (h, r[36..36 + n].to_vec())
}

fn accept_msg(s: &Pubkey, def: &[u8; 32]) -> Vec<u8> {
    let mut v = b"cadre-swarm-accept-v1".to_vec();
    v.extend_from_slice(s);
    v.extend_from_slice(def);
    v
}

fn join_msg(s: &Pubkey, pid: u64) -> Vec<u8> {
    let mut v = b"cadre-swarm-join-v1".to_vec();
    v.extend_from_slice(s);
    v.extend_from_slice(&pid.to_le_bytes());
    v
}

/// The create args; `forge` replaces the signature of member index `forge` with one by STRANGER.
fn create_args(c: &Chain, s: &Pubkey, d: &Def, method: &str, forge: Option<usize>) -> Vec<u8> {
    let (h, _) = def_hash(c, s, d);
    let msg = accept_msg(s, &h);
    let mut i = In::m(method).str(&d.name).addr(&d.goal).str(&d.uri).addr(&d.lead).u64(d.salt).u8(d.members.len() as u8);
    for (k, (who, role, amount)) in d.members.iter().enumerate() {
        let signer = if forge == Some(k) { STRANGER } else { *who };
        let sig = sign_plain(&seed(signer), &msg);
        i = i.addr(&pk(*who)).str(role).u64(*amount);
        i.0.extend_from_slice(&sig);
    }
    i.b()
}

fn create_signed_args(c: &Chain, s: &Pubkey, d: &Def, creator_seed: u8) -> Vec<u8> {
    let (_, cmsg) = def_hash(c, s, d);
    let mut b = create_args(c, s, d, "create_signed", None);
    b.extend_from_slice(&d.creator);
    b.extend_from_slice(&sign_plain(&seed(creator_seed), &cmsg));
    b
}

struct S {
    creator: Pubkey,
    lead: Pubkey,
    successor: Pubkey,
    height: u64,
    slots: u32,
    active: u32,
    nprop: u32,
    name: String,
}

fn swarm(c: &Chain, s: &Pubkey, id: u64) -> S {
    let b = c.q(s, In::m("swarm").u64(id).b());
    let a = |o: usize| -> Pubkey { b[o..o + 32].try_into().unwrap() };
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let nl = u32at(5 * 32 + 8 + 12) as usize;
    let name = String::from_utf8(b[5 * 32 + 8 + 16..5 * 32 + 8 + 16 + nl].to_vec()).unwrap();
    S { creator: a(0), lead: a(32), successor: a(64), height: u64_at(&b, 160), slots: u32at(168), active: u32at(172), nprop: u32at(176), name }
}

/// (agent, role, joined, left) of every slot.
fn members(c: &Chain, s: &Pubkey, id: u64) -> Vec<(Pubkey, String, u64, u64)> {
    let b = c.q(s, In::m("members").u64(id).u32(0).u8(32).b());
    let n = b[4] as usize;
    let mut o = 5;
    let mut out = Vec::new();
    for _ in 0..n {
        let a: Pubkey = b[o..o + 32].try_into().unwrap();
        let rl = u32::from_le_bytes(b[o + 32..o + 36].try_into().unwrap()) as usize;
        let role = String::from_utf8(b[o + 36..o + 36 + rl].to_vec()).unwrap();
        o += 36 + rl;
        out.push((a, role, u64_at(&b, o), u64_at(&b, o + 8)));
        o += 24;
    }
    assert_eq!(o, b.len());
    out
}

fn ids(c: &Chain, s: &Pubkey, method: &str, who: &Pubkey) -> (u32, Vec<u64>) {
    let b = c.q(s, In::m(method).addr(who).u32(0).u8(64).b());
    let total = u32::from_le_bytes(b[0..4].try_into().unwrap());
    let n = b[4] as usize;
    (total, (0..n).map(|i| u64_at(&b, 5 + 8 * i)).collect())
}

fn setup(n: u8) -> (Chain, Pubkey) {
    let mut c = Chain::new();
    for i in 0..n {
        c.fund(pk(m(i)), 10 * CADRE);
    }
    let s = deploy(&mut c, 0);
    (c, s)
}

#[test]
fn create_needs_every_members_own_signature() {
    let (mut c, s) = setup(4);
    let mut d = Def::new(3);
    d.members[1].2 = 2 * CADRE;
    d.members[2].2 = CADRE;
    let before: Vec<u64> = (0..3).map(|i| c.bal(&pk(m(i)))).collect();
    // a forged acceptance (signed by someone else) is refused: membership cannot be faked
    let forged = create_args(&c, &s, &d, "create", Some(1));
    c.fails(CREATOR, s, 3 * CADRE, forged, "acceptance signature does not verify");
    // value must match the amounts
    let args = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, CADRE, args.clone(), "value must equal");
    let (r, sum) = c.ok(CREATOR, s, 3 * CADRE, args.clone());
    eprintln!("gas: swarms create 3 members {} gas, fee {}", r.gas_used, r.fee_charged);
    assert_eq!(ret_u64(&r), 1);
    assert_eq!(sum.events.len(), 1, "one SwarmCreated event");
    assert_eq!(c.bal(&pk(m(1))), before[1] + 2 * CADRE, "member 1 paid directly");
    assert_eq!(c.bal(&pk(m(2))), before[2] + CADRE, "member 2 paid directly");
    assert_eq!(c.bal(&s), 0, "the contract keeps no CADRE");
    let sw = swarm(&c, &s, 1);
    assert_eq!((sw.creator, sw.lead, sw.successor, sw.slots, sw.active, sw.nprop), (pk(CREATOR), pk(m(0)), [0; 32], 3, 3, 0));
    assert_eq!(sw.name, "research crew");
    assert_eq!(sw.height, c.height);
    let ms = members(&c, &s, 1);
    assert_eq!(ms.iter().map(|x| (x.0, x.1.as_str())).collect::<Vec<_>>(), vec![(pk(m(0)), "lead"), (pk(m(1)), "worker-1"), (pk(m(2)), "worker-2")]);
    assert_eq!(ids(&c, &s, "by_member", &pk(m(1))), (1, vec![1]));
    assert_eq!(ids(&c, &s, "by_creator", &pk(CREATOR)), (1, vec![1]));
    assert_eq!(ids(&c, &s, "by_member", &pk(CREATOR)), (0, vec![]), "the creator is not a member");
    let (h, _) = def_hash(&c, &s, &d);
    assert_eq!(u64_at(&c.q(&s, In::m("by_definition").addr(&h).b()), 0), 1);
    // the same definition cannot be registered twice (replay)
    c.fails(CREATOR, s, 3 * CADRE, args, "already registered");
    // an acceptance is bound to the exact definition: change a role and the old signatures fail
    let (h_old, _) = def_hash(&c, &s, &d);
    let mut d2 = Def::new(3);
    d2.salt = 43;
    d2.members[2].1 = "boss".into();
    let (h_new, _) = def_hash(&c, &s, &d2);
    assert_ne!(h_old, h_new);
    let mut stale = In::m("create").str(&d2.name).addr(&d2.goal).str(&d2.uri).addr(&d2.lead).u64(d2.salt).u8(3);
    for (who, role, amount) in &d2.members {
        stale = stale.addr(&pk(*who)).str(role).u64(*amount);
        stale.0.extend_from_slice(&sign_plain(&seed(*who), &accept_msg(&s, &h_old)));
    }
    c.fails(CREATOR, s, 0, stale.b(), "acceptance signature does not verify");
}

#[test]
fn create_rules() {
    let (mut c, s) = setup(3);
    let mut d = Def::new(2);
    d.lead = pk(STRANGER);
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "lead must be one of the members");
    let mut d = Def::new(2);
    d.members[1].0 = CREATOR;
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "attribution, not a member");
    let mut d = Def::new(2);
    d.members[1].0 = m(0);
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "named twice");
    let d = Def::new(0);
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "1 to 32 members");
    let d = Def::new(33);
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "1 to 32 members");
}

#[test]
fn create_signed_by_anyone_with_the_creators_signature() {
    let (mut c, s) = setup(3);
    let d = Def::new(3);
    let bad = create_signed_args(&c, &s, &d, STRANGER);
    c.fails(m(0), s, 0, bad, "creator's signature does not verify");
    let good = create_signed_args(&c, &s, &d, CREATOR);
    let (r, _) = c.ok(m(0), s, 0, good);
    eprintln!("gas: swarms create_signed 3 members {} gas, fee {}", r.gas_used, r.fee_charged);
    let sw = swarm(&c, &s, ret_u64(&r));
    assert_eq!(sw.creator, pk(CREATOR), "the creator is the signer, not the submitter");
    assert_eq!(ids(&c, &s, "by_creator", &pk(CREATOR)).1, vec![1]);
    assert_eq!(ids(&c, &s, "by_creator", &pk(m(0))).0, 0);
}

#[test]
fn the_creator_has_no_control() {
    let (mut c, s) = setup(4);
    let d = Def::new(3);
    let a = create_args(&c, &s, &d, "create", None);
    c.ok(CREATOR, s, 0, a);
    for meth in ["remove", "kick", "pause", "withdraw", "set_lead", "update"] {
        c.fails(CREATOR, s, 0, In::m(meth).u64(1).addr(&pk(m(1))).b(), "unknown method");
    }
    c.fails(CREATOR, s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("x").b(), "only the swarm's lead");
    c.fails(CREATOR, s, 0, In::m("hand_lead").u64(1).addr(&pk(m(1))).b(), "only the swarm's lead");
    c.fails(CREATOR, s, 0, In::m("leave").u64(1).b(), "not an active member");
    c.fails(CREATOR, s, 0, In::m("take_lead").u64(1).b(), "only the named successor");
    assert_eq!(swarm(&c, &s, 1).active, 3);
}

#[test]
fn proposals_need_the_invited_agents_own_acceptance() {
    let (mut c, s) = setup(6);
    let d = Def::new(2);
    let a = create_args(&c, &s, &d, "create", None);
    c.ok(CREATOR, s, 0, a);
    c.fails(m(1), s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("analyst").b(), "only the swarm's lead");
    let (r, _) = c.ok(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("analyst").b());
    eprintln!("gas: swarms propose {} gas, fee {}", r.gas_used, r.fee_charged);
    let p1 = ret_u64(&r);
    c.fails(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("analyst").b(), "already has an open proposal");
    c.fails(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(1))).str("x").b(), "already an active member");
    // the lead cannot accept for the agent; a stranger cannot either
    c.fails(m(0), s, 0, In::m("accept").u64(p1).b(), "only the invited agent accepts");
    c.fails(STRANGER, s, 0, In::m("accept").u64(p1).b(), "only the invited agent accepts");
    let mut forged = In::m("accept_signed").u64(p1);
    forged.0.extend_from_slice(&sign_plain(&seed(m(0)), &join_msg(&s, p1)));
    c.fails(STRANGER, s, 0, forged.b(), "join signature does not verify");
    let (r, _) = c.ok(m(3), s, 0, In::m("accept").u64(p1).b());
    eprintln!("gas: swarms accept {} gas, fee {}", r.gas_used, r.fee_charged);
    assert_eq!(swarm(&c, &s, 1).active, 3);
    c.fails(m(3), s, 0, In::m("accept").u64(p1).b(), "not open");
    // accept_signed: anyone submits the invited agent's own join signature
    let p2 = ret_u64(&c.ok(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(4))).str("scout").b()).0);
    let mut js = In::m("accept_signed").u64(p2);
    js.0.extend_from_slice(&sign_plain(&seed(m(4)), &join_msg(&s, p2)));
    let (r, _) = c.ok(STRANGER, s, 0, js.b());
    eprintln!("gas: swarms accept_signed {} gas, fee {}", r.gas_used, r.fee_charged);
    assert_eq!(swarm(&c, &s, 1).active, 4);
    assert_eq!(ids(&c, &s, "by_member", &pk(m(4))).1, vec![1]);
    // decline and cancel
    let p3 = ret_u64(&c.ok(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(5))).str("x").b()).0);
    c.fails(m(0), s, 0, In::m("decline").u64(p3).b(), "only the invited agent declines");
    c.ok(m(5), s, 0, In::m("decline").u64(p3).b());
    c.fails(m(5), s, 0, In::m("accept").u64(p3).b(), "not open");
    let p4 = ret_u64(&c.ok(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(5))).str("y").b()).0);
    c.fails(m(5), s, 0, In::m("cancel").u64(p4).b(), "only the swarm's lead cancels");
    c.ok(m(0), s, 0, In::m("cancel").u64(p4).b());
    c.fails(m(5), s, 0, In::m("accept").u64(p4).b(), "not open");
    let b = c.q(&s, In::m("proposals").u64(1).u32(0).u8(10).b());
    assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 4);
    let pb = c.q(&s, In::m("proposal").u64(p3).b());
    assert_eq!(pb[8 + 64], 3, "p3 declined");
    assert_eq!(swarm(&c, &s, 1).nprop, 4);
}

#[test]
fn members_leave_on_their_own_and_the_lead_hands_over() {
    let (mut c, s) = setup(4);
    let d = Def::new(3);
    let a = create_args(&c, &s, &d, "create", None);
    c.ok(CREATOR, s, 0, a);
    let (r, _) = c.ok(m(2), s, 0, In::m("leave").u64(1).b());
    eprintln!("gas: swarms leave {} gas, fee {}", r.gas_used, r.fee_charged);
    c.fails(m(2), s, 0, In::m("leave").u64(1).b(), "not an active member");
    let ms = members(&c, &s, 1);
    assert!(ms[2].3 > 0, "left height recorded");
    assert_eq!(swarm(&c, &s, 1).active, 2);
    assert_eq!(ids(&c, &s, "by_member", &pk(m(2))).1, vec![1], "history keeps the swarm");
    // a member that left can come back only by a new proposal it accepts (a new slot)
    let p = ret_u64(&c.ok(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(2))).str("back").b()).0);
    c.ok(m(2), s, 0, In::m("accept").u64(p).b());
    let sw = swarm(&c, &s, 1);
    assert_eq!((sw.slots, sw.active), (4, 3));
    assert_eq!(ids(&c, &s, "by_member", &pk(m(2))).0, 1, "no duplicate index entry");
    // hand the lead over: the successor must take it
    c.fails(m(0), s, 0, In::m("hand_lead").u64(1).addr(&pk(STRANGER)).b(), "another active member");
    c.ok(m(0), s, 0, In::m("hand_lead").u64(1).addr(&pk(m(1))).b());
    assert_eq!(swarm(&c, &s, 1).successor, pk(m(1)));
    c.fails(m(2), s, 0, In::m("take_lead").u64(1).b(), "only the named successor");
    c.ok(m(1), s, 0, In::m("take_lead").u64(1).b());
    let sw = swarm(&c, &s, 1);
    assert_eq!((sw.lead, sw.successor), (pk(m(1)), [0; 32]));
    c.fails(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("x").b(), "only the swarm's lead");
    // the lead leaves: no lead, nobody can propose
    c.ok(m(1), s, 0, In::m("leave").u64(1).b());
    assert_eq!(swarm(&c, &s, 1).lead, [0; 32]);
    c.fails(m(0), s, 0, In::m("propose").u64(1).addr(&pk(m(3))).str("x").b(), "only the swarm's lead");
}

#[test]
fn fund_pays_active_members_directly() {
    let (mut c, s) = setup(3);
    let d = Def::new(3);
    let a = create_args(&c, &s, &d, "create", None);
    c.ok(CREATOR, s, 0, a);
    c.ok(m(2), s, 0, In::m("leave").u64(1).b());
    c.fails(STRANGER, s, CADRE, In::m("fund").u64(1).u8(1).addr(&pk(m(2))).u64(CADRE).b(), "active members only");
    c.fails(STRANGER, s, CADRE, In::m("fund").u64(1).u8(1).addr(&pk(m(1))).u64(2 * CADRE).b(), "value must equal");
    let b0 = c.bal(&pk(m(0)));
    let b1 = c.bal(&pk(m(1)));
    let (r, _) = c.ok(STRANGER, s, 3 * CADRE, In::m("fund").u64(1).u8(2).addr(&pk(m(0))).u64(CADRE).addr(&pk(m(1))).u64(2 * CADRE).b());
    eprintln!("gas: swarms fund 2 members {} gas, fee {}", r.gas_used, r.fee_charged);
    assert_eq!(c.bal(&pk(m(0))), b0 + CADRE);
    assert_eq!(c.bal(&pk(m(1))), b1 + 2 * CADRE);
    assert_eq!(c.bal(&s), 0);
    c.fails(STRANGER, s, CADRE, In::m("leave").u64(1).b(), "takes no value");
}

#[test]
fn start_height_gate() {
    let mut c = Chain::new();
    c.fund(pk(m(0)), 10 * CADRE);
    let start = c.height + 5;
    let s = deploy(&mut c, start);
    let d = Def::new(1);
    let a = create_args(&c, &s, &d, "create", None);
    c.fails(CREATOR, s, 0, a, "not active before start_height");
    c.skip(5);
    let a = create_args(&c, &s, &d, "create", None);
    c.ok(CREATOR, s, 0, a);
}

#[test]
fn gas_table_large_swarms() {
    for n in [12u8, 32] {
        let (mut c, s) = setup(0);
        let mut d = Def::new(n);
        for x in d.members.iter_mut() {
            x.2 = CADRE / 10;
        }
        let a = create_args(&c, &s, &d, "create", None);
        let len = a.len();
        let before = c.bal(&pk(CREATOR));
        let (r, _) = c.ok(CREATOR, s, n as u64 * CADRE / 10, a);
        let deposit = before - c.bal(&pk(CREATOR)) - n as u64 * CADRE / 10 - r.fee_charged;
        eprintln!("gas: swarms create {n} members (each paid 0.1 CADRE) {} gas, fee {}, storage deposit {deposit}, args {len} bytes", r.gas_used, r.fee_charged);
        assert_eq!(swarm(&c, &s, 1).active, n as u32);
    }
}
