//! Contract kinds (docs/CONTRACTS.md, SPEC 4.9): DeployCode, Instantiate, Call, Migrate,
//! SetContractAdmin.
//!
//! Rules shared by all five:
//! - valid from `contracts_upgrade_height(chain_id)`;
//! - fee floor `max(min_fee, contract_base_fee + contract_byte_fee x len + ceil(gas_limit x
//!   gas_price / 1e6))`, checked before the fee is taken (an underpaying tx is invalid);
//! - `gas_limit <= tx_gas_limit` and `gas_limit <= block_gas_limit - gas used so far in the
//!   block` (else invalid), so a block never runs more than `block_gas_limit` gas;
//! - once the fee and nonce are taken, an Instantiate / Call / Migrate / SetContractAdmin
//!   ALWAYS lands: if it fails (revert, out of gas, trap, refused) every state change except
//!   the fee and the nonce is undone and the receipt says why. The unused gas
//!   `floor((gas_limit - gas_used) x gas_price / 1e6)` goes back to the sender.
//! - DeployCode is invalid (never lands) when its code fails validation, so it is checked at
//!   admission too.
//!
//! The call driver runs frames on an explicit stack (no async recursion): each wasm frame
//! suspends on a state request, the driver serves it from the overlay and resumes it. Each
//! nested call runs in its own overlay savepoint. A contract already on the stack cannot be
//! called again (no reentrancy), and the stack is at most `max_call_depth` frames.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use agent_types::codec::{R, W};
use agent_types::contracts::{self as ac, Event, Receipt, Status};
use agent_types::{is_reserved, Hash, Pubkey, Tx, TxBody, TxKind};
use ::contracts::{Abort, Env, Frame, Mode, RawEvent, Reply, Request, Step, Vm};
use state::config::{Config, GAS_PRICE_UNIT};
use state::contracts::{self as sc, CodeInfo, Contract, ContractCounters, StoredReceipt};
use state::{schema, StateError, StateRead};

use crate::{bad, BlockEnv, BlockExecutor, TxError, TxOutcome};

static VM: OnceLock<Vm> = OnceLock::new();

/// The process-wide VM (engine + compiled-module cache).
pub fn vm() -> &'static Vm {
    VM.get_or_init(Vm::new)
}

fn corrupt(e: impl std::fmt::Display) -> TxError {
    TxError::State(StateError::Corrupt(e.to_string()))
}

/// Gas limit a contract tx asks for (0 for SetContractAdmin and non-contract kinds).
pub fn gas_limit_of(body: &TxBody) -> u64 {
    match body {
        TxBody::DeployCode(d) => d.gas_limit,
        TxBody::Instantiate(i) => i.gas_limit,
        TxBody::Call(c) => c.gas_limit,
        TxBody::Migrate(m) => m.gas_limit,
        _ => 0,
    }
}

/// Fee floor of a contract tx of `len` encoded bytes asking for `gas_limit`.
pub fn contract_fee_floor(cfg: &Config, len: usize, gas_limit: u64) -> u64 {
    let gas_fee = (gas_limit as u128 * cfg.gas_price as u128).div_ceil(GAS_PRICE_UNIT as u128).min(u64::MAX as u128) as u64;
    cfg.contract_base_fee.saturating_add(cfg.contract_byte_fee.saturating_mul(len as u64)).saturating_add(gas_fee).max(cfg.min_fee)
}

/// Unused gas paid back.
fn gas_refund(cfg: &Config, gas_limit: u64, gas_used: u64) -> u64 {
    ((gas_limit.saturating_sub(gas_used) as u128 * cfg.gas_price as u128) / GAS_PRICE_UNIT as u128) as u64
}

/// The stateless contract checks sendTx runs before the mempool (the executor repeats them):
/// activation height, gas limits, the fee floor, and for DeployCode the full code validation.
pub fn precheck_contract(tx: &Tx, cfg: &Config, next_height: u64) -> Result<(), String> {
    precheck(tx, cfg, next_height, true)
}

fn precheck(tx: &Tx, cfg: &Config, height: u64, validate_code: bool) -> Result<(), String> {
    let kind = tx.body.kind();
    if !kind.is_contract() {
        return Ok(());
    }
    let act = ac::contracts_upgrade_height(tx.chain_id);
    if height < act {
        return Err(format!("contract txs activate at height {act} (block {height})"));
    }
    let gas_limit = gas_limit_of(&tx.body);
    if gas_limit > cfg.tx_gas_limit {
        return Err(format!("gas_limit {gas_limit} above tx_gas_limit {}", cfg.tx_gas_limit));
    }
    let len = tx.encode().len();
    let floor = contract_fee_floor(cfg, len, gas_limit);
    if tx.fee < floor {
        return Err(format!(
            "contract fee {} below {floor} (contract_base_fee {} + contract_byte_fee {} x {len} bytes + gas_limit {gas_limit} x gas_price {} / 1e6, at least min_fee)",
            tx.fee, cfg.contract_base_fee, cfg.contract_byte_fee, cfg.gas_price
        ));
    }
    if let TxBody::DeployCode(d) = &tx.body {
        let cost = ::contracts::deploy_cost(d.code.len());
        if cost > gas_limit {
            return Err(format!("DeployCode of {} bytes needs gas_limit >= {cost}", d.code.len()));
        }
        if validate_code {
            vm().validate(&d.code, cfg.max_code_size as usize)?;
        }
    }
    Ok(())
}

/// What a contract tx did.
struct Outcome {
    status: Status,
    gas_used: u64,
    ret: Vec<u8>,
    error: Vec<u8>,
    contract: Pubkey,
    code_hash: Hash,
    events: Vec<(Pubkey, RawEvent)>,
    native: TxOutcome,
}

impl Outcome {
    fn refused(contract: Pubkey, code_hash: Hash, msg: &str) -> Self {
        Self {
            status: Status::Refused,
            gas_used: 0,
            ret: Vec::new(),
            error: msg.as_bytes().to_vec(),
            contract,
            code_hash,
            events: Vec::new(),
            native: TxOutcome::default(),
        }
    }
}

/// One frame on the driver's stack.
struct Ctx {
    frame: Frame,
    address: Pubkey,
    /// Gas given to this frame, its load cost included.
    given: u64,
    mode: Mode,
    /// Storage growth per contract: (bytes, items).
    deltas: BTreeMap<Pubkey, (i64, i64)>,
    events: Vec<(Pubkey, RawEvent)>,
    native: TxOutcome,
}

/// Result of driving a top frame to its end.
struct Driven {
    result: Result<Vec<u8>, Abort>,
    gas_used: u64,
    deltas: BTreeMap<Pubkey, (i64, i64)>,
    events: Vec<(Pubkey, RawEvent)>,
    native: TxOutcome,
    log: Vec<u8>,
}

fn merge_native(into: &mut TxOutcome, o: TxOutcome) {
    if let Some(j) = o.job_id {
        into.extra_jobs.push(j);
    }
    into.extra_jobs.extend(o.extra_jobs);
    into.withdrawals.extend(o.withdrawals);
    into.routed.extend(o.routed);
}

fn merge_deltas(into: &mut BTreeMap<Pubkey, (i64, i64)>, from: BTreeMap<Pubkey, (i64, i64)>) {
    for (k, (b, i)) in from {
        let e = into.entry(k).or_insert((0, 0));
        e.0 += b;
        e.1 += i;
    }
}

/// Native tx kinds a contract may run as itself (docs/CONTRACTS.md "Native access").
pub const NATIVE_CALL_KINDS: [TxKind; 11] = [
    TxKind::Transfer,
    TxKind::RegisterAgent,
    TxKind::PostJob,
    TxKind::SubmitResult,
    TxKind::CompleteJob,
    TxKind::ExpireJob,
    TxKind::BridgeLock,
    TxKind::SetProfile,
    TxKind::Post,
    TxKind::React,
    TxKind::Follow,
];

/// Native read kinds (docs/CONTRACTS.md "Native access").
pub mod native {
    pub const ACCOUNT: u32 = 0;
    pub const JOB: u32 = 1;
    pub const AGENT: u32 = 2;
    pub const PROFILE: u32 = 3;
    pub const FOLLOW: u32 = 4;
    pub const CONTRACT: u32 = 5;
}

/// Result of an RPC query or a call simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    pub ok: bool,
    pub ret: Vec<u8>,
    pub error: Vec<u8>,
    pub gas_used: u64,
    pub log: Vec<u8>,
    pub events: Vec<Event>,
}

/// Run a contract's `query` entry (Mode::Query: no state change can happen) against `base`
/// at `height`, with at most `gas`. Nested calls run in query mode too.
pub async fn query_contract<S: StateRead>(base: &S, chain_id: u32, height: u64, address: &Pubkey, input: Vec<u8>, gas: u64) -> Result<QueryResult, String> {
    let env = BlockEnv { chain_id, height, miner: [0; 32], job_id: None };
    let mut ex = BlockExecutor::begin(base, env).await.map_err(|e| e.to_string())?;
    ex.run_entry(&[0; 32], address, "query", input, 0, gas, Mode::Query).await.map_err(|e| e.to_string())
}

/// Simulate a Call by `from` (no signature, no fee, nothing kept): gas used, return data,
/// events and debug log, for gas estimation.
#[allow(clippy::too_many_arguments)]
pub async fn simulate_call<S: StateRead>(base: &S, chain_id: u32, height: u64, from: &Pubkey, address: &Pubkey, input: Vec<u8>, value: u64, gas: u64) -> Result<QueryResult, String> {
    let env = BlockEnv { chain_id, height, miner: [0; 32], job_id: None };
    let mut ex = BlockExecutor::begin(base, env).await.map_err(|e| e.to_string())?;
    ex.run_entry(from, address, "call", input, value, gas, Mode::Execute).await.map_err(|e| e.to_string())
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Activation, gas and fee rules of a contract tx (before the fee is taken).
    pub(crate) fn check_contract(&self, tx: &Tx) -> Result<(), TxError> {
        precheck(tx, &self.cfg, self.env.height, false).map_err(TxError::Invalid)?;
        let gas_limit = gas_limit_of(&tx.body);
        let left = self.cfg.block_gas_limit.saturating_sub(self.summary.gas_used);
        if gas_limit > left {
            return bad(format!("gas_limit {gas_limit} above the block's remaining gas {left}"));
        }
        Ok(())
    }

    pub(crate) async fn contract(&self, a: &Pubkey) -> Result<Option<Contract>, TxError> {
        match self.ov.get(&sc::contract_key(a)).await? {
            Some(b) => Contract::decode(&b).map(Some).map_err(corrupt),
            None => Ok(None),
        }
    }

    fn put_contract(&mut self, c: &Contract) {
        self.ov.put(sc::contract_key(&c.address), Some(c.encode()));
    }

    async fn code_info(&self, h: &Hash) -> Result<Option<CodeInfo>, TxError> {
        match self.ov.get(&sc::code_key(h)).await? {
            Some(b) => CodeInfo::decode(&b).map(Some).map_err(corrupt),
            None => Ok(None),
        }
    }

    async fn code_bytes(&self, h: &Hash) -> Result<Vec<u8>, TxError> {
        let b = self.ov.get(&sc::code_bytes_key(h)).await?.ok_or_else(|| corrupt("code info without code bytes"))?;
        Ok(sc::decode_code_bytes(&b).map_err(corrupt)?.to_vec())
    }

    async fn counters_c(&self) -> Result<ContractCounters, TxError> {
        match self.ov.get(&sc::counters_key()).await? {
            Some(b) => ContractCounters::decode(&b).map_err(corrupt),
            None => Ok(ContractCounters::default()),
        }
    }

    /// Execute a contract tx whose fee and nonce were taken.
    pub(crate) async fn exec_contract(&mut self, tx: &Tx, out: &mut TxOutcome) -> Result<(), TxError> {
        let gas_limit = gas_limit_of(&tx.body);
        let o = match &tx.body {
            TxBody::DeployCode(d) => self.deploy_code(&tx.from, d).await?,
            TxBody::Instantiate(i) => self.instantiate(tx, i).await?,
            TxBody::Call(c) => self.call(tx, c).await?,
            TxBody::Migrate(m) => self.migrate(tx, m).await?,
            TxBody::SetContractAdmin { contract, admin } => self.set_admin(&tx.from, contract, admin).await?,
            _ => return bad("not a contract kind"),
        };
        let tx_hash = tx.hash();
        let h = self.env.height;
        let ok = o.status == Status::Ok;
        let events: Vec<Event> = if ok {
            o.events.into_iter().enumerate().map(|(i, (c, e))| Event { contract: c, tx_hash, index: i as u32, topics: e.topics, data: e.data }).collect()
        } else {
            Vec::new()
        };
        // per-contract event index (only for a successful tx)
        for e in &events {
            let Some(mut c) = self.contract(&e.contract).await? else { return Err(corrupt("event from a missing contract")) };
            self.ov.put(sc::event_index_key(&e.contract, c.events), Some(sc::encode_event_ref(h, &tx_hash, e.index)));
            c.events += 1;
            self.put_contract(&c);
        }
        let refund = gas_refund(&self.cfg, gas_limit, o.gas_used);
        if refund > 0 {
            self.ov.credit(&tx.from, refund).await?;
        }
        let mut error = o.error;
        error.truncate(ac::MAX_ERROR);
        let mut ret = o.ret;
        ret.truncate(ac::MAX_RETURN);
        let receipt = Receipt {
            tx_hash,
            kind: tx.body.kind() as u8,
            status: o.status,
            gas_limit,
            gas_used: o.gas_used,
            fee_charged: tx.fee - refund,
            contract: o.contract,
            code_hash: o.code_hash,
            event_count: events.len() as u16,
            ret,
            error,
        };
        self.ov.put(sc::receipt_key(&tx_hash), Some(StoredReceipt { height: h, receipt: receipt.clone(), events: events.clone() }.encode()));
        if ok {
            merge_native(out, o.native);
        }
        out.fee_refund = refund;
        out.receipt = Some(receipt);
        out.events = events;
        Ok(())
    }

    async fn deploy_code(&mut self, from: &Pubkey, d: &ac::DeployCode) -> Result<Outcome, TxError> {
        let (_, _info) = vm().validate(&d.code, self.cfg.max_code_size as usize).map_err(TxError::Invalid)?;
        let cost = ::contracts::deploy_cost(d.code.len());
        if cost > d.gas_limit {
            return bad(format!("DeployCode needs gas_limit >= {cost}"));
        }
        let h = ac::code_hash(&d.code);
        if self.code_info(&h).await?.is_some() {
            return bad("code already deployed (instantiate it by its code hash)");
        }
        let deposit = (d.code.len() as u64).saturating_mul(self.cfg.storage_byte_deposit);
        self.ov.debit(from, deposit).await.map_err(|_| TxError::Invalid(format!("cannot pay the code storage deposit {deposit}")))?;
        let mut c = self.counters_c().await?;
        let info = CodeInfo { code_hash: h, deployer: *from, height: self.env.height, size: d.code.len() as u32, deposit, instances: 0, index: c.codes };
        self.ov.put(sc::code_key(&h), Some(info.encode()));
        self.ov.put(sc::code_bytes_key(&h), Some(sc::encode_code_bytes(&d.code)));
        self.ov.put(sc::code_index_key(c.codes), Some(sc::encode_index(sc::tag::CODE_INDEX, &h)));
        c.codes += 1;
        self.ov.put(sc::counters_key(), Some(c.encode()));
        let mut o = Outcome::refused([0; 32], h, "");
        o.status = Status::Ok;
        o.gas_used = cost;
        o.error.clear();
        Ok(o)
    }

    async fn instantiate(&mut self, tx: &Tx, i: &ac::Instantiate) -> Result<Outcome, TxError> {
        let addr = ac::contract_address(&tx.from, tx.nonce);
        let Some(mut code) = self.code_info(&i.code_hash).await? else { return Ok(Outcome::refused(addr, i.code_hash, "unknown code hash")) };
        if self.contract(&addr).await?.is_some() {
            return Ok(Outcome::refused(addr, i.code_hash, "contract address already in use"));
        }
        self.ov.savepoint();
        let mut c = self.counters_c().await?;
        let rec = Contract {
            address: addr,
            code_hash: i.code_hash,
            admin: i.admin,
            creator: tx.from,
            label: i.label.clone(),
            created_height: self.env.height,
            migrated_height: 0,
            storage_bytes: 0,
            storage_items: 0,
            deposit: 0,
            index: c.contracts,
            calls: 0,
            events: 0,
        };
        self.put_contract(&rec);
        self.ov.put(sc::contract_index_key(c.contracts), Some(sc::encode_index(sc::tag::CONTRACT_INDEX, &addr)));
        c.contracts += 1;
        self.ov.put(sc::counters_key(), Some(c.encode()));
        code.instances += 1;
        self.ov.put(sc::code_key(&i.code_hash), Some(code.encode()));
        if i.value > 0 && self.ov.debit(&tx.from, i.value).await.is_err() {
            self.ov.rollback_savepoint();
            return Ok(Outcome::refused(addr, i.code_hash, "value not covered by the sender's balance"));
        }
        self.ov.credit(&addr, i.value).await?;
        let bytes = self.code_bytes(&i.code_hash).await?;
        let module = vm().module(&i.code_hash, &bytes).map_err(TxError::Invalid)?;
        if !Vm::has_entry(&module, "instantiate") {
            self.ov.release_savepoint();
            let mut o = Outcome::refused(addr, i.code_hash, "");
            o.status = Status::Ok;
            o.error.clear();
            return Ok(o);
        }
        let d = self.drive_top(&tx.from, &tx.from, &addr, &bytes, &i.code_hash, "instantiate", i.args.clone(), i.value, i.gas_limit, Mode::Execute).await?;
        self.finish_top(&tx.from, addr, i.code_hash, d).await
    }

    async fn call(&mut self, tx: &Tx, c: &ac::Call) -> Result<Outcome, TxError> {
        let Some(rec) = self.contract(&c.contract).await? else { return Ok(Outcome::refused(c.contract, [0; 32], "no contract at this address")) };
        let bytes = self.code_bytes(&rec.code_hash).await?;
        let module = vm().module(&rec.code_hash, &bytes).map_err(TxError::Invalid)?;
        if !Vm::has_entry(&module, "call") {
            return Ok(Outcome::refused(c.contract, rec.code_hash, "the contract has no call entry point"));
        }
        self.ov.savepoint();
        if c.value > 0 {
            if self.ov.debit(&tx.from, c.value).await.is_err() {
                self.ov.rollback_savepoint();
                return Ok(Outcome::refused(c.contract, rec.code_hash, "value not covered by the sender's balance"));
            }
            self.ov.credit(&c.contract, c.value).await?;
        }
        let d = self.drive_top(&tx.from, &tx.from, &c.contract, &bytes, &rec.code_hash, "call", c.args.clone(), c.value, c.gas_limit, Mode::Execute).await?;
        self.finish_top(&tx.from, c.contract, rec.code_hash, d).await
    }

    async fn migrate(&mut self, tx: &Tx, m: &ac::Migrate) -> Result<Outcome, TxError> {
        let Some(mut rec) = self.contract(&m.contract).await? else { return Ok(Outcome::refused(m.contract, m.code_hash, "no contract at this address")) };
        if rec.admin == [0; 32] {
            return Ok(Outcome::refused(m.contract, m.code_hash, "the contract is immutable (no admin)"));
        }
        if rec.admin != tx.from {
            return Ok(Outcome::refused(m.contract, m.code_hash, "only the contract's admin may migrate it"));
        }
        let Some(mut code) = self.code_info(&m.code_hash).await? else { return Ok(Outcome::refused(m.contract, m.code_hash, "unknown code hash")) };
        self.ov.savepoint();
        rec.code_hash = m.code_hash;
        rec.migrated_height = self.env.height;
        self.put_contract(&rec);
        code.instances += 1;
        self.ov.put(sc::code_key(&m.code_hash), Some(code.encode()));
        let bytes = self.code_bytes(&m.code_hash).await?;
        let module = vm().module(&m.code_hash, &bytes).map_err(TxError::Invalid)?;
        if !Vm::has_entry(&module, "migrate") {
            self.ov.release_savepoint();
            let mut o = Outcome::refused(m.contract, m.code_hash, "");
            o.status = Status::Ok;
            o.error.clear();
            return Ok(o);
        }
        let d = self.drive_top(&tx.from, &tx.from, &m.contract, &bytes, &m.code_hash, "migrate", m.args.clone(), 0, m.gas_limit, Mode::Execute).await?;
        self.finish_top(&tx.from, m.contract, m.code_hash, d).await
    }

    async fn set_admin(&mut self, from: &Pubkey, contract: &Pubkey, admin: &Pubkey) -> Result<Outcome, TxError> {
        let Some(mut rec) = self.contract(contract).await? else { return Ok(Outcome::refused(*contract, [0; 32], "no contract at this address")) };
        if rec.admin == [0; 32] || rec.admin != *from {
            return Ok(Outcome::refused(*contract, rec.code_hash, "only the contract's admin may change it"));
        }
        rec.admin = *admin;
        self.put_contract(&rec);
        let mut o = Outcome::refused(*contract, rec.code_hash, "");
        o.status = Status::Ok;
        o.error.clear();
        Ok(o)
    }

    /// Close the tx-level savepoint of a driven top frame: on success settle storage
    /// deposits (the sender pays for growth, gets back a pro-rata share of the deposit for
    /// shrinkage) and keep the writes; on failure undo everything.
    async fn finish_top(&mut self, origin: &Pubkey, contract: Pubkey, code_hash: Hash, d: Driven) -> Result<Outcome, TxError> {
        let mut o = Outcome {
            status: Status::Ok,
            gas_used: d.gas_used,
            ret: Vec::new(),
            error: Vec::new(),
            contract,
            code_hash,
            events: Vec::new(),
            native: TxOutcome::default(),
        };
        match d.result {
            Ok(ret) => match self.settle_deposits(origin, &d.deltas).await? {
                Ok(()) => {
                    if let Some(mut rec) = self.contract(&contract).await? {
                        rec.calls += 1;
                        self.put_contract(&rec);
                    }
                    self.ov.release_savepoint();
                    o.ret = ret;
                    o.events = d.events;
                    o.native = d.native;
                }
                Err(msg) => {
                    self.ov.rollback_savepoint();
                    o.status = Status::Refused;
                    o.error = msg.into_bytes();
                }
            },
            Err(a) => {
                self.ov.rollback_savepoint();
                o.status = match a {
                    Abort::Revert(_) => Status::Reverted,
                    Abort::OutOfGas => Status::OutOfGas,
                    Abort::Trap(_) => Status::Trapped,
                };
                o.error = a.message();
            }
        }
        Ok(o)
    }

    async fn settle_deposits(&mut self, origin: &Pubkey, deltas: &BTreeMap<Pubkey, (i64, i64)>) -> Result<Result<(), String>, TxError> {
        let price = self.cfg.storage_byte_deposit;
        for (addr, (db, di)) in deltas {
            let Some(mut rec) = self.contract(addr).await? else { return Err(corrupt("storage change on a missing contract")) };
            let before = rec.storage_bytes;
            rec.storage_bytes = (before as i128 + *db as i128).max(0) as u64;
            rec.storage_items = (rec.storage_items as i128 + *di as i128).max(0) as u64;
            if *db > 0 {
                let cost = (*db as u64).saturating_mul(price);
                if self.ov.debit(origin, cost).await.is_err() {
                    return Ok(Err(format!("storage deposit {cost} not covered by the sender's balance")));
                }
                rec.deposit += cost;
            } else if *db < 0 && before > 0 {
                let freed = (-*db) as u64;
                let refund = ((rec.deposit as u128 * freed.min(before) as u128) / before as u128) as u64;
                rec.deposit -= refund;
                self.ov.credit(origin, refund).await?;
            }
            self.put_contract(&rec);
        }
        Ok(Ok(()))
    }

    /// Entry for queries and simulations: run `entry` of `address` with no fee and no
    /// receipt, inside a savepoint that is always undone.
    pub(crate) async fn run_entry(&mut self, origin: &Pubkey, address: &Pubkey, entry: &str, input: Vec<u8>, value: u64, gas: u64, mode: Mode) -> Result<QueryResult, TxError> {
        let Some(rec) = self.contract(address).await? else { return bad("no contract at this address") };
        let bytes = self.code_bytes(&rec.code_hash).await?;
        let module = vm().module(&rec.code_hash, &bytes).map_err(TxError::Invalid)?;
        if !Vm::has_entry(&module, entry) {
            return bad(format!("the contract has no {entry} entry point"));
        }
        self.ov.savepoint();
        if value > 0 {
            self.ov.debit(origin, value).await?;
            self.ov.credit(address, value).await?;
        }
        let d = self.drive_top(origin, origin, address, &bytes, &rec.code_hash, entry, input, value, gas, mode).await?;
        self.ov.rollback_savepoint();
        let tx_hash = [0u8; 32];
        Ok(match d.result {
            Ok(ret) => QueryResult {
                ok: true,
                ret,
                error: Vec::new(),
                gas_used: d.gas_used,
                log: d.log,
                events: d.events.into_iter().enumerate().map(|(i, (c, e))| Event { contract: c, tx_hash, index: i as u32, topics: e.topics, data: e.data }).collect(),
            },
            Err(a) => QueryResult { ok: false, ret: Vec::new(), error: a.message(), gas_used: d.gas_used, log: d.log, events: Vec::new() },
        })
    }

    /// Run a top frame (entry `entry` of `address`) to its end, serving every request.
    #[allow(clippy::too_many_arguments)]
    async fn drive_top(
        &mut self,
        origin: &Pubkey,
        caller: &Pubkey,
        address: &Pubkey,
        code: &[u8],
        code_hash: &Hash,
        entry: &str,
        input: Vec<u8>,
        value: u64,
        gas: u64,
        mode: Mode,
    ) -> Result<Driven, TxError> {
        let load = ::contracts::load_cost(code.len());
        let fail = |a: Abort, used: u64| Driven { result: Err(a), gas_used: used, deltas: BTreeMap::new(), events: Vec::new(), native: TxOutcome::default(), log: Vec::new() };
        if gas < load {
            return Ok(fail(Abort::OutOfGas, gas));
        }
        let module = vm().module(code_hash, code).map_err(TxError::Invalid)?;
        let env = Env { chain_id: self.env.chain_id, height: self.env.height, address: *address, caller: *caller, origin: *origin, value, miner: self.env.miner };
        let frame = match vm().frame(&module, env, mode, input, gas - load, 0) {
            Ok(f) => f,
            Err(a) => return Ok(fail(a, gas)),
        };
        let mut stack = vec![Ctx { frame, address: *address, given: gas, mode, deltas: BTreeMap::new(), events: Vec::new(), native: TxOutcome::default() }];
        let mut step = stack[0].frame.start(entry);
        loop {
            {
                let cur = stack.last_mut().unwrap();
                let addr = cur.address;
                for e in cur.frame.events() {
                    cur.events.push((addr, e));
                }
            }
            match step {
                Step::Request(Request::Call { contract, value, input, gas }) => {
                    match self.open_child(&stack, origin, contract, value, input, gas).await? {
                        Ok(child) => {
                            stack.push(child);
                            step = stack.last_mut().unwrap().frame.start("call");
                        }
                        Err((msg, used)) => {
                            step = stack.last_mut().unwrap().frame.resume(Reply::Call { ok: false, data: msg.into_bytes(), gas_used: used });
                        }
                    }
                }
                Step::Request(r) => {
                    let cur = stack.last_mut().unwrap();
                    let reply = self.serve(cur, r).await?;
                    step = cur.frame.resume(reply);
                }
                Step::Done { ret } => {
                    if stack.len() == 1 {
                        let top = stack.pop().unwrap();
                        let used = top.given - top.frame.gas_left();
                        let log = top.frame.log().to_vec();
                        return Ok(Driven { result: Ok(ret), gas_used: used, deltas: top.deltas, events: top.events, native: top.native, log });
                    }
                    let child = stack.pop().unwrap();
                    if let Some(mut rec) = self.contract(&child.address).await? {
                        rec.calls += 1;
                        self.put_contract(&rec);
                    }
                    self.ov.release_savepoint();
                    let used = child.given - child.frame.gas_left();
                    let parent = stack.last_mut().unwrap();
                    let n = child.events.len();
                    parent.events.extend(child.events);
                    parent.frame.add_events_before(n);
                    merge_deltas(&mut parent.deltas, child.deltas);
                    merge_native(&mut parent.native, child.native);
                    step = parent.frame.resume(Reply::Call { ok: true, data: ret, gas_used: used });
                }
                Step::Abort(a) => {
                    if stack.len() == 1 {
                        let top = stack.pop().unwrap();
                        let used = if a == Abort::OutOfGas { top.given } else { top.given - top.frame.gas_left() };
                        let log = top.frame.log().to_vec();
                        return Ok(Driven { result: Err(a), gas_used: used, deltas: BTreeMap::new(), events: Vec::new(), native: TxOutcome::default(), log });
                    }
                    let child = stack.pop().unwrap();
                    self.ov.rollback_savepoint();
                    let used = if a == Abort::OutOfGas { child.given } else { child.given - child.frame.gas_left() };
                    let parent = stack.last_mut().unwrap();
                    step = parent.frame.resume(Reply::Call { ok: false, data: a.message(), gas_used: used });
                }
            }
        }
    }

    /// Set up a nested call frame (opens its savepoint), or say why the call fails at once:
    /// `Err((message, gas the caller is charged))`.
    async fn open_child(&mut self, stack: &[Ctx], origin: &Pubkey, contract: Pubkey, value: u64, input: Vec<u8>, gas: u64) -> Result<Result<Ctx, (String, u64)>, TxError> {
        let parent = stack.last().unwrap();
        let mode = parent.mode;
        if stack.len() >= self.cfg.max_call_depth as usize {
            return Ok(Err((format!("call depth limit {}", self.cfg.max_call_depth), 0)));
        }
        if stack.iter().any(|c| c.address == contract) {
            return Ok(Err(("reentrancy refused: the contract is already on the call stack".into(), 0)));
        }
        let Some(rec) = self.contract(&contract).await? else { return Ok(Err(("no contract at this address".into(), 0))) };
        let left = parent.frame.gas_left();
        let give = if gas == 0 || gas > left { left } else { gas };
        let code = self.code_bytes(&rec.code_hash).await?;
        let load = ::contracts::load_cost(code.len());
        if give < load {
            return Ok(Err(("out of gas".into(), give)));
        }
        let module = vm().module(&rec.code_hash, &code).map_err(TxError::Invalid)?;
        if !Vm::has_entry(&module, "call") {
            return Ok(Err(("the callee has no call entry point".into(), load)));
        }
        self.ov.savepoint();
        if value > 0 {
            if self.ov.debit(&parent.address, value).await.is_err() {
                self.ov.rollback_savepoint();
                return Ok(Err(("value not covered by the calling contract's balance".into(), load)));
            }
            self.ov.credit(&contract, value).await?;
        }
        let before: usize = stack.iter().map(|c| c.events.len()).sum();
        let env = Env { chain_id: self.env.chain_id, height: self.env.height, address: contract, caller: parent.address, origin: *origin, value, miner: self.env.miner };
        match vm().frame(&module, env, mode, input, give - load, before) {
            Ok(frame) => Ok(Ok(Ctx { frame, address: contract, given: give, mode, deltas: BTreeMap::new(), events: Vec::new(), native: TxOutcome::default() })),
            Err(a) => {
                self.ov.rollback_savepoint();
                Ok(Err((String::from_utf8_lossy(&a.message()).into_owned(), give)))
            }
        }
    }

    /// Serve one host request of frame `cur`.
    async fn serve(&mut self, cur: &mut Ctx, r: Request) -> Result<Reply, TxError> {
        let me = cur.address;
        Ok(match r {
            Request::StorageRead { key } => {
                let v = self.ov.get(&sc::store_key(&me, &key)).await?;
                Reply::Value(match v {
                    Some(b) => Some(sc::decode_store(&b).map_err(corrupt)?.1),
                    None => None,
                })
            }
            Request::StorageWrite { key, value } => {
                let k = sc::store_key(&me, &key);
                let old = match self.ov.get(&k).await? {
                    Some(b) => Some(sc::decode_store(&b).map_err(corrupt)?),
                    None => None,
                };
                let d = cur.deltas.entry(me).or_insert((0, 0));
                d.0 += sc::item_bytes(&key, &value) as i64;
                match &old {
                    Some((ok, ov)) => d.0 -= sc::item_bytes(ok, ov) as i64,
                    None => d.1 += 1,
                }
                self.ov.put(k, Some(sc::encode_store(&key, &value)));
                Reply::Unit
            }
            Request::StorageRemove { key } => {
                let k = sc::store_key(&me, &key);
                if let Some(b) = self.ov.get(&k).await? {
                    let (ok, ov) = sc::decode_store(&b).map_err(corrupt)?;
                    let d = cur.deltas.entry(me).or_insert((0, 0));
                    d.0 -= sc::item_bytes(&ok, &ov) as i64;
                    d.1 -= 1;
                    self.ov.put(k, None);
                }
                Reply::Unit
            }
            Request::Balance { account } => Reply::I64(self.ov.account(&account).await?.balance as i64),
            Request::Transfer { to, amount } => {
                if is_reserved(&to) || to == [0; 32] {
                    Reply::Status(2, b"cannot transfer to a reserved account".to_vec())
                } else if amount == 0 {
                    Reply::Status(0, Vec::new())
                } else if self.ov.account(&me).await?.balance < amount {
                    Reply::Status(1, b"insufficient balance".to_vec())
                } else {
                    self.ov.debit(&me, amount).await?;
                    self.ov.credit(&to, amount).await?;
                    Reply::Status(0, Vec::new())
                }
            }
            Request::NativeRead { kind, id } => Reply::Value(self.native_read(kind, &id).await?),
            Request::NativeCall { kind, body } => {
                let Some(k) = u8::try_from(kind).ok().and_then(|k| TxKind::from_u8(k).ok()).filter(|k| NATIVE_CALL_KINDS.contains(k)) else {
                    return Ok(Reply::Status(1, format!("native call kind {kind} is not allowed").into_bytes()));
                };
                let mut rd = R::new(&body);
                let tb = match TxBody::read(k, &mut rd).and_then(|b| rd.finish().map(|_| b)) {
                    Ok(b) => b,
                    Err(e) => return Ok(Reply::Status(1, format!("bad {} body: {e}", k.name()).into_bytes())),
                };
                if k.is_social() && self.env.height < agent_types::social::social_upgrade_height(self.env.chain_id) {
                    return Ok(Reply::Status(1, b"social kinds are not active yet".to_vec()));
                }
                self.ov.savepoint();
                let nonce = self.ov.account(&me).await?.nonce;
                let mut o = TxOutcome::default();
                match self.exec_body(&me, nonce, &tb, &mut o).await {
                    Ok(()) => {
                        let mut acc = self.ov.account(&me).await?;
                        acc.nonce += 1;
                        self.ov.put_account(&me, &acc);
                        self.ov.release_savepoint();
                        merge_native(&mut cur.native, o);
                        Reply::Status(0, Vec::new())
                    }
                    Err(TxError::Invalid(m)) => {
                        self.ov.rollback_savepoint();
                        Reply::Status(1, m.into_bytes())
                    }
                    Err(e) => return Err(e),
                }
            }
            Request::Call { .. } => unreachable!("calls are handled by the driver"),
        })
    }

    /// Native records in the contract ABI (docs/CONTRACTS.md "Native access").
    async fn native_read(&self, kind: u32, id: &[u8]) -> Result<Option<Vec<u8>>, TxError> {
        let id32 = |id: &[u8]| -> Option<[u8; 32]> { id.try_into().ok() };
        let mut w = W::new();
        match kind {
            native::ACCOUNT => {
                let Some(a) = id32(id) else { return Ok(None) };
                let acc = self.ov.account(&a).await?;
                w.u64(acc.balance).u64(acc.nonce);
            }
            native::JOB => {
                let Some(j) = id32(id) else { return Ok(None) };
                let Some(j) = self.ov.job(&j).await? else { return Ok(None) };
                w.u8(j.status as u8)
                    .bytes(&j.poster)
                    .bytes(&j.deliverer)
                    .u64(j.worker_pay)
                    .u64(j.protocol_fee)
                    .u64(j.deadline_height)
                    .u64(j.created_height)
                    .u64(j.closed_height)
                    .bytes(&j.result_hash)
                    .u8(j.evaluators.len() as u8)
                    .u8(j.threshold);
            }
            native::AGENT => {
                let Some(a) = id32(id) else { return Ok(None) };
                let Some(b) = self.ov.get(&schema::agent_key(&a)).await? else { return Ok(None) };
                let ag = schema::Agent::decode(&b).map_err(corrupt)?;
                w.u64(ag.registered_height).bytes8(&ag.name).bytes16(&ag.uri);
            }
            native::PROFILE => {
                let Some(a) = id32(id) else { return Ok(None) };
                let Some(b) = self.ov.get(&state::social::profile_key(&a)).await? else { return Ok(None) };
                let p = state::social::Profile::decode(&b).map_err(corrupt)?;
                w.u16(p.flags).bytes(&p.operator).u64(p.created_height).bytes8(&p.handle).bytes8(&p.name);
            }
            native::FOLLOW => {
                if id.len() != 64 {
                    return Ok(None);
                }
                let (f, t): ([u8; 32], [u8; 32]) = (id[..32].try_into().unwrap(), id[32..].try_into().unwrap());
                let Some(b) = self.ov.get(&state::social::follow_key(&f, &t)).await? else { return Ok(None) };
                w.u64(state::social::decode_u64(schema::tag::FOLLOW, &b).map_err(corrupt)?);
            }
            native::CONTRACT => {
                let Some(a) = id32(id) else { return Ok(None) };
                let Some(c) = self.contract(&a).await? else { return Ok(None) };
                w.bytes(&c.code_hash).bytes(&c.admin).bytes(&c.creator).u64(c.created_height);
            }
            _ => return Ok(None),
        }
        Ok(Some(w.into_inner()))
    }
}
