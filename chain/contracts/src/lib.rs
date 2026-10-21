//! Cadre contract VM host (docs/CONTRACTS.md).
//!
//! Contracts are WebAssembly modules run by the wasmi interpreter with fuel metering
//! (1 gas = 1 fuel). The chain state is async (QMDB) while wasm runs synchronously, so every
//! host call that touches state SUSPENDS the wasm call: the host function returns a
//! [`Request`] as a wasmi host error, wasmi keeps the paused call (`ResumableCallHostTrap`),
//! the executor serves the request against its overlay (awaiting state reads, running a
//! nested contract frame or a native tx body) and resumes the call with the answer. Pure
//! memory calls (input, env, return data, hashing, events) are served inline.
//!
//! Determinism: no floats (rejected at deploy), no SIMD (not compiled in), no threads, no
//! start function, imports only from the fixed `cadre` table below, memory capped, wasm
//! stack and call depth capped by fixed wasmi limits, every trap mapped to a fixed message.
//! Nothing here reads a clock, a random source or the host environment.

pub mod gas;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_types::{Hash, Pubkey};
use wasmi::{
    Caller, CompilationMode, Config, EnforcedLimits, Engine, Error, Extern, ExternType, Func, Instance, Linker, Memory, Module, ResumableCall,
    ResumableCallHostTrap, Store, StoreLimits, StoreLimitsBuilder, Val,
};

/// Import module name every host function lives in.
pub const IMPORT_MODULE: &str = "cadre";
/// Linear memory cap per contract frame (64 KiB pages).
pub const MAX_MEMORY_PAGES: u64 = 64;
pub const MAX_MEMORY_BYTES: usize = (MAX_MEMORY_PAGES as usize) * 65536;
/// Fixed wasmi stack limits (set explicitly so a wasmi default change cannot fork the chain).
pub const MAX_RECURSION_DEPTH: usize = 1000;
pub const MAX_STACK_HEIGHT: usize = 1_000_000;
/// Events one tx may emit, across all its frames.
pub const MAX_EVENTS_PER_TX: usize = 64;
/// Debug log bytes kept per frame (simulation only).
pub const MAX_LOG: usize = 4096;
/// Entry points (exports of type `() -> ()`).
pub const ENTRY_POINTS: [&str; 4] = ["instantiate", "call", "query", "migrate"];

/// Size of the env block `env_read` writes:
/// `chain_id u32 | height u64 | self [32] | caller [32] | origin [32] | value u64 | miner [32]`.
pub const ENV_LEN: usize = 148;

/// What a frame runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A tx (Instantiate, Call, Migrate) or a nested call from one: may write.
    Execute,
    /// RPC query: storage writes, transfers, value calls, native calls and events abort.
    Query,
}

/// Block and call context of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    pub chain_id: u32,
    pub height: u64,
    /// The contract running.
    pub address: Pubkey,
    /// Immediate caller: the tx sender for a top frame, the calling contract for a nested one.
    pub caller: Pubkey,
    /// The tx sender (zero for a query).
    pub origin: Pubkey,
    /// CADRE moved to the contract with this call (base units).
    pub value: u64,
    pub miner: Pubkey,
}

impl Env {
    pub fn encode(&self) -> [u8; ENV_LEN] {
        let mut b = [0u8; ENV_LEN];
        b[0..4].copy_from_slice(&self.chain_id.to_le_bytes());
        b[4..12].copy_from_slice(&self.height.to_le_bytes());
        b[12..44].copy_from_slice(&self.address);
        b[44..76].copy_from_slice(&self.caller);
        b[76..108].copy_from_slice(&self.origin);
        b[108..116].copy_from_slice(&self.value.to_le_bytes());
        b[116..148].copy_from_slice(&self.miner);
        b
    }
}

/// A host call the executor must serve (the frame is paused until [`Frame::resume`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Reply `Value(Some/None)`: the value goes to scratch, wasm gets its length or -1.
    StorageRead { key: Vec<u8> },
    /// Reply `Unit`.
    StorageWrite { key: Vec<u8>, value: Vec<u8> },
    /// Reply `Unit`.
    StorageRemove { key: Vec<u8> },
    /// Reply `I64(balance)`.
    Balance { account: Pubkey },
    /// Reply `I32(0 ok | 1 insufficient balance | 2 refused)`.
    Transfer { to: Pubkey, amount: u64 },
    /// Run `call` on `contract` with `input`, moving `value` from this contract to it, with at
    /// most `gas` (0 = all remaining). Reply `Call { ok, data, gas_used }`.
    Call { contract: Pubkey, value: u64, input: Vec<u8>, gas: u64 },
    /// Reply `Value`: the record in the native ABI (docs/CONTRACTS.md "Native access").
    NativeRead { kind: u32, id: Vec<u8> },
    /// Run tx body `body` of tx kind `kind` as the contract. Reply `Status(0 ok | 1 failed)`,
    /// with the failure message in scratch.
    NativeCall { kind: u32, body: Vec<u8> },
}

/// The executor's answer to a [`Request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Unit,
    Value(Option<Vec<u8>>),
    I64(i64),
    Status(i32, Vec<u8>),
    Call { ok: bool, data: Vec<u8>, gas_used: u64 },
}

/// Why a frame stopped without finishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abort {
    /// `revert(msg)`.
    Revert(Vec<u8>),
    OutOfGas,
    /// A wasm trap or a misused host call; the message is fixed per cause.
    Trap(String),
}

impl Abort {
    pub fn message(&self) -> Vec<u8> {
        match self {
            Abort::Revert(m) => m.clone(),
            Abort::OutOfGas => b"out of gas".to_vec(),
            Abort::Trap(t) => t.as_bytes().to_vec(),
        }
    }
}

/// Result of starting or resuming a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Done { ret: Vec<u8> },
    Request(Request),
    Abort(Abort),
}

/// Host error used both for requests (resumed) and aborts (terminal).
#[derive(Debug)]
enum HostSignal {
    Req(Request),
    Abort(Abort),
}

impl std::fmt::Display for HostSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cadre host signal")
    }
}

impl wasmi::errors::HostError for HostSignal {}

fn req(r: Request) -> Error {
    Error::host(HostSignal::Req(r))
}
fn abort(a: Abort) -> Error {
    Error::host(HostSignal::Abort(a))
}
fn misuse(msg: &str) -> Error {
    abort(Abort::Trap(format!("host call: {msg}")))
}

/// An event a frame emitted (topics, data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

/// Per-frame host state (wasmi Store data).
pub struct HostState {
    env: Env,
    mode: Mode,
    input: Vec<u8>,
    scratch: Vec<u8>,
    ret: Vec<u8>,
    events: Vec<RawEvent>,
    /// Events already emitted by earlier frames of this tx (for the per-tx cap).
    events_before: usize,
    log: Vec<u8>,
    limits: StoreLimits,
}

/// The engine and the compiled-module cache shared by all frames.
#[derive(Clone)]
pub struct Vm {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    cache: Arc<Mutex<HashMap<Hash, Module>>>,
}

/// Most compiled modules kept (the cache is cleared when full; it only saves time).
const CACHE_MAX: usize = 256;

/// Deploy-time facts about valid code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeInfo {
    /// Entry points the module exports, in [`ENTRY_POINTS`] order.
    pub entries: Vec<String>,
    /// Imported host functions.
    pub imports: Vec<String>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

/// The fixed engine configuration (consensus rule).
pub fn engine_config() -> Config {
    let mut c = Config::default();
    c.consume_fuel(true)
        .floats(false)
        .wasm_multi_memory(false)
        .wasm_memory64(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false)
        .wasm_mutable_global(true)
        .wasm_sign_extension(true)
        .wasm_saturating_float_to_int(false)
        .wasm_multi_value(true)
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_tail_call(true)
        .wasm_extended_const(true)
        .ignore_custom_sections(true)
        .compilation_mode(CompilationMode::Eager)
        .enforced_limits(EnforcedLimits::strict())
        .set_max_recursion_depth(MAX_RECURSION_DEPTH)
        .set_max_stack_height(MAX_STACK_HEIGHT);
    c
}

/// The host import table: name, params, results (I32 = 'i', I64 = 'l').
pub const IMPORTS: &[(&str, &str, &str)] = &[
    ("input_len", "", "i"),
    ("input_read", "i", ""),
    ("env_read", "i", ""),
    ("set_return", "ii", ""),
    ("revert", "ii", ""),
    ("scratch_len", "", "i"),
    ("scratch_read", "i", ""),
    ("storage_read", "ii", "i"),
    ("storage_write", "iiii", ""),
    ("storage_remove", "ii", ""),
    ("emit", "iiii", ""),
    ("balance", "i", "l"),
    ("transfer", "il", "i"),
    ("call", "iliil", "i"),
    ("native_read", "iii", "i"),
    ("native_call", "iii", "i"),
    ("blake3", "iii", ""),
    ("sha256", "iii", ""),
    ("ed25519_verify", "iiii", "i"),
    ("gas_left", "", "l"),
    ("log", "ii", ""),
];

fn charge(c: &mut Caller<'_, HostState>, amount: u64) -> Result<(), Error> {
    let left = c.get_fuel().map_err(|_| misuse("fuel"))?;
    if left < amount {
        c.set_fuel(0).map_err(|_| misuse("fuel"))?;
        return Err(abort(Abort::OutOfGas));
    }
    c.set_fuel(left - amount).map_err(|_| misuse("fuel"))
}

fn memory(c: &Caller<'_, HostState>) -> Result<Memory, Error> {
    match c.get_export("memory") {
        Some(Extern::Memory(m)) => Ok(m),
        _ => Err(misuse("no exported memory")),
    }
}

fn read_mem(c: &Caller<'_, HostState>, ptr: i32, len: i32, max: usize) -> Result<Vec<u8>, Error> {
    if len < 0 || len as usize > max {
        return Err(misuse("length out of range"));
    }
    let m = memory(c)?;
    let mut buf = vec![0u8; len as usize];
    m.read(c, ptr as u32 as usize, &mut buf).map_err(|_| misuse("memory read out of bounds"))?;
    Ok(buf)
}

fn write_mem(c: &mut Caller<'_, HostState>, ptr: i32, data: &[u8]) -> Result<(), Error> {
    let m = memory(c)?;
    m.write(c, ptr as u32 as usize, data).map_err(|_| misuse("memory write out of bounds"))
}

fn read32(c: &Caller<'_, HostState>, ptr: i32) -> Result<[u8; 32], Error> {
    Ok(read_mem(c, ptr, 32, 32)?.try_into().unwrap())
}

fn writable(c: &Caller<'_, HostState>) -> Result<(), Error> {
    if c.data().mode == Mode::Query {
        return Err(misuse("state change in a query"));
    }
    Ok(())
}

/// A length argument as gas units (negative lengths are refused later by `read_mem`).
fn k(len: i32) -> u64 {
    if len < 0 { 0 } else { len as u64 }
}

fn build_linker(engine: &Engine) -> Linker<HostState> {
    use agent_types::contracts::{MAX_ARGS, MAX_EVENT_DATA, MAX_RETURN, MAX_TOPICS};
    let mut l = Linker::<HostState>::new(engine);
    let m = IMPORT_MODULE;
    l.func_wrap(m, "input_len", |mut c: Caller<'_, HostState>| -> Result<i32, Error> {
        charge(&mut c, gas::HOST_BASE)?;
        Ok(c.data().input.len() as i32)
    })
    .unwrap();
    l.func_wrap(m, "input_read", |mut c: Caller<'_, HostState>, ptr: i32| -> Result<(), Error> {
        let n = c.data().input.len() as u64;
        charge(&mut c, gas::HOST_BASE + n * gas::COPY_PER_BYTE)?;
        let data = std::mem::take(&mut c.data_mut().input);
        let r = write_mem(&mut c, ptr, &data);
        c.data_mut().input = data;
        r
    })
    .unwrap();
    l.func_wrap(m, "env_read", |mut c: Caller<'_, HostState>, ptr: i32| -> Result<(), Error> {
        charge(&mut c, gas::HOST_BASE + ENV_LEN as u64 * gas::COPY_PER_BYTE)?;
        let e = c.data().env.encode();
        write_mem(&mut c, ptr, &e)
    })
    .unwrap();
    l.func_wrap(m, "set_return", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<(), Error> {
        charge(&mut c, gas::HOST_BASE + k(len) * gas::COPY_PER_BYTE)?;
        let data = read_mem(&c, ptr, len, MAX_RETURN)?;
        c.data_mut().ret = data;
        Ok(())
    })
    .unwrap();
    l.func_wrap(m, "revert", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<(), Error> {
        charge(&mut c, gas::HOST_BASE)?;
        let msg = read_mem(&c, ptr, len.min(agent_types::contracts::MAX_ERROR as i32), agent_types::contracts::MAX_ERROR)?;
        Err(abort(Abort::Revert(msg)))
    })
    .unwrap();
    l.func_wrap(m, "scratch_len", |mut c: Caller<'_, HostState>| -> Result<i32, Error> {
        charge(&mut c, gas::HOST_BASE)?;
        Ok(c.data().scratch.len() as i32)
    })
    .unwrap();
    l.func_wrap(m, "scratch_read", |mut c: Caller<'_, HostState>, ptr: i32| -> Result<(), Error> {
        let n = c.data().scratch.len() as u64;
        charge(&mut c, gas::HOST_BASE + n * gas::COPY_PER_BYTE)?;
        let data = std::mem::take(&mut c.data_mut().scratch);
        let r = write_mem(&mut c, ptr, &data);
        c.data_mut().scratch = data;
        r
    })
    .unwrap();
    l.func_wrap(m, "storage_read", |mut c: Caller<'_, HostState>, kp: i32, kl: i32| -> Result<i32, Error> {
        charge(&mut c, gas::STORAGE_READ + k(kl) * gas::COPY_PER_BYTE)?;
        let key = read_mem(&c, kp, kl, state_limits::MAX_KEY)?;
        Err(req(Request::StorageRead { key }))
    })
    .unwrap();
    l.func_wrap(m, "storage_write", |mut c: Caller<'_, HostState>, kp: i32, kl: i32, vp: i32, vl: i32| -> Result<(), Error> {
        writable(&c)?;
        charge(&mut c, gas::STORAGE_WRITE + (k(kl) + k(vl)) * gas::STORAGE_WRITE_PER_BYTE)?;
        let key = read_mem(&c, kp, kl, state_limits::MAX_KEY)?;
        let value = read_mem(&c, vp, vl, state_limits::MAX_VALUE)?;
        Err(req(Request::StorageWrite { key, value }))
    })
    .unwrap();
    l.func_wrap(m, "storage_remove", |mut c: Caller<'_, HostState>, kp: i32, kl: i32| -> Result<(), Error> {
        writable(&c)?;
        charge(&mut c, gas::STORAGE_WRITE + k(kl) * gas::STORAGE_WRITE_PER_BYTE)?;
        let key = read_mem(&c, kp, kl, state_limits::MAX_KEY)?;
        Err(req(Request::StorageRemove { key }))
    })
    .unwrap();
    l.func_wrap(m, "emit", |mut c: Caller<'_, HostState>, tp: i32, tn: i32, dp: i32, dl: i32| -> Result<(), Error> {
        writable(&c)?;
        if tn < 0 || tn as usize > MAX_TOPICS {
            return Err(misuse("at most 4 topics"));
        }
        charge(&mut c, gas::EMIT + (tn as u64 * 32 + k(dl)) * gas::EMIT_PER_BYTE)?;
        let t = read_mem(&c, tp, tn * 32, MAX_TOPICS * 32)?;
        let data = read_mem(&c, dp, dl, MAX_EVENT_DATA)?;
        let s = c.data_mut();
        if s.events_before + s.events.len() >= MAX_EVENTS_PER_TX {
            return Err(misuse("too many events in one tx"));
        }
        s.events.push(RawEvent { topics: t.chunks(32).map(|x| x.try_into().unwrap()).collect(), data });
        Ok(())
    })
    .unwrap();
    l.func_wrap(m, "balance", |mut c: Caller<'_, HostState>, ap: i32| -> Result<i64, Error> {
        charge(&mut c, gas::BALANCE)?;
        let account = read32(&c, ap)?;
        Err(req(Request::Balance { account }))
    })
    .unwrap();
    l.func_wrap(m, "transfer", |mut c: Caller<'_, HostState>, ap: i32, amount: i64| -> Result<i32, Error> {
        writable(&c)?;
        charge(&mut c, gas::TRANSFER)?;
        let to = read32(&c, ap)?;
        Err(req(Request::Transfer { to, amount: amount as u64 }))
    })
    .unwrap();
    l.func_wrap(m, "call", |mut c: Caller<'_, HostState>, ap: i32, value: i64, ip: i32, il: i32, g: i64| -> Result<i32, Error> {
        if value != 0 {
            writable(&c)?;
        }
        charge(&mut c, gas::CALL + k(il) * gas::COPY_PER_BYTE)?;
        let contract = read32(&c, ap)?;
        let input = read_mem(&c, ip, il, MAX_ARGS)?;
        Err(req(Request::Call { contract, value: value as u64, input, gas: g as u64 }))
    })
    .unwrap();
    l.func_wrap(m, "native_read", |mut c: Caller<'_, HostState>, kind: i32, ip: i32, il: i32| -> Result<i32, Error> {
        charge(&mut c, gas::NATIVE_READ + k(il) * gas::COPY_PER_BYTE)?;
        let id = read_mem(&c, ip, il, 64)?;
        Err(req(Request::NativeRead { kind: kind as u32, id }))
    })
    .unwrap();
    l.func_wrap(m, "native_call", |mut c: Caller<'_, HostState>, kind: i32, bp: i32, bl: i32| -> Result<i32, Error> {
        writable(&c)?;
        charge(&mut c, gas::NATIVE_CALL + k(bl) * gas::NATIVE_CALL_PER_BYTE)?;
        let body = read_mem(&c, bp, bl, agent_types::tx::MAX_TX_SIZE)?;
        Err(req(Request::NativeCall { kind: kind as u32, body }))
    })
    .unwrap();
    l.func_wrap(m, "blake3", |mut c: Caller<'_, HostState>, p: i32, n: i32, out: i32| -> Result<(), Error> {
        charge(&mut c, gas::HASH + k(n) * gas::HASH_PER_BYTE)?;
        let data = read_mem(&c, p, n, MAX_MEMORY_BYTES)?;
        let h = agent_types::blake3_hash(&[&data]);
        write_mem(&mut c, out, &h)
    })
    .unwrap();
    l.func_wrap(m, "sha256", |mut c: Caller<'_, HostState>, p: i32, n: i32, out: i32| -> Result<(), Error> {
        charge(&mut c, gas::HASH + k(n) * gas::HASH_PER_BYTE)?;
        let data = read_mem(&c, p, n, MAX_MEMORY_BYTES)?;
        let h = agent_types::sha256(&[&data]);
        write_mem(&mut c, out, &h)
    })
    .unwrap();
    l.func_wrap(m, "ed25519_verify", |mut c: Caller<'_, HostState>, pk: i32, mp: i32, ml: i32, sp: i32| -> Result<i32, Error> {
        charge(&mut c, gas::ED25519_VERIFY + k(ml) * gas::HASH_PER_BYTE)?;
        let key = read32(&c, pk)?;
        let msg = read_mem(&c, mp, ml, MAX_MEMORY_BYTES)?;
        let sig: [u8; 64] = read_mem(&c, sp, 64, 64)?.try_into().unwrap();
        Ok(agent_types::tx::verify_plain(&key, &msg, &sig) as i32)
    })
    .unwrap();
    l.func_wrap(m, "gas_left", |mut c: Caller<'_, HostState>| -> Result<i64, Error> {
        charge(&mut c, gas::HOST_BASE)?;
        Ok(c.get_fuel().map_err(|_| misuse("fuel"))? as i64)
    })
    .unwrap();
    l.func_wrap(m, "log", |mut c: Caller<'_, HostState>, p: i32, n: i32| -> Result<(), Error> {
        charge(&mut c, gas::LOG + k(n) * gas::COPY_PER_BYTE)?;
        let data = read_mem(&c, p, n, MAX_LOG)?;
        let s = c.data_mut();
        if s.log.len() + data.len() < MAX_LOG {
            s.log.extend_from_slice(&data);
            s.log.push(b'\n');
        }
        Ok(())
    })
    .unwrap();
    l
}

/// Storage limits the host checks (same as state::contracts).
pub mod state_limits {
    pub const MAX_KEY: usize = 128;
    pub const MAX_VALUE: usize = 64 * 1024;
}

fn sig_of(s: &str) -> Vec<wasmi::ValType> {
    s.chars()
        .map(|c| match c {
            'i' => wasmi::ValType::I32,
            _ => wasmi::ValType::I64,
        })
        .collect()
}

/// Walk the module's top-level sections (id, size) without parsing them.
fn sections(code: &[u8]) -> Result<Vec<u8>, String> {
    if code.len() < 8 || &code[0..4] != b"\0asm" || code[4..8] != [1, 0, 0, 0] {
        return Err("not a WebAssembly 1.0 module".into());
    }
    let mut pos = 8;
    let mut ids = Vec::new();
    while pos < code.len() {
        let id = code[pos];
        pos += 1;
        let mut size: u64 = 0;
        let mut shift = 0;
        loop {
            let b = *code.get(pos).ok_or("truncated section size")?;
            pos += 1;
            size |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err("bad section size".into());
            }
        }
        pos = pos.checked_add(size as usize).filter(|p| *p <= code.len()).ok_or("section past the end")?;
        ids.push(id);
    }
    Ok(ids)
}

impl Vm {
    pub fn new() -> Self {
        let engine = Engine::new(&engine_config());
        let linker = Arc::new(build_linker(&engine));
        Self { engine, linker, cache: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Deploy-time checks (consensus rule). `max_size` is the config's `max_code_size`.
    pub fn validate(&self, code: &[u8], max_size: usize) -> Result<(Module, CodeInfo), String> {
        if code.len() > max_size {
            return Err(format!("code is {} bytes, above max_code_size {max_size}", code.len()));
        }
        let ids = sections(code)?;
        if ids.contains(&8) {
            return Err("start functions are not allowed".into());
        }
        let module = Module::new(&self.engine, code).map_err(|e| format!("invalid module: {}", short(&e)))?;
        let mut imports = Vec::new();
        for imp in module.imports() {
            if imp.module() != IMPORT_MODULE {
                return Err(format!("import from module {:?}: only {IMPORT_MODULE:?} is allowed", imp.module()));
            }
            let Some((_, p, r)) = IMPORTS.iter().find(|(n, _, _)| *n == imp.name()) else {
                return Err(format!("unknown host function {:?}", imp.name()));
            };
            match imp.ty() {
                ExternType::Func(ft) => {
                    if ft.params() != sig_of(p).as_slice() || ft.results() != sig_of(r).as_slice() {
                        return Err(format!("host function {:?} has the wrong signature", imp.name()));
                    }
                }
                _ => return Err(format!("import {:?}: only functions may be imported", imp.name())),
            }
            imports.push(imp.name().to_string());
        }
        let mut entries = Vec::new();
        let mut has_memory = false;
        for ex in module.exports() {
            match ex.ty() {
                ExternType::Memory(mt) if ex.name() == "memory" => {
                    if mt.minimum() > MAX_MEMORY_PAGES {
                        return Err(format!("memory starts at {} pages, above {MAX_MEMORY_PAGES}", mt.minimum()));
                    }
                    has_memory = true;
                }
                ExternType::Func(ft) if ENTRY_POINTS.contains(&ex.name()) => {
                    if !ft.params().is_empty() || !ft.results().is_empty() {
                        return Err(format!("entry point {:?} must take and return nothing", ex.name()));
                    }
                    entries.push(ex.name().to_string());
                }
                _ => {}
            }
        }
        if !has_memory {
            return Err("the module must export its memory as \"memory\"".into());
        }
        if !entries.iter().any(|e| e == "call" || e == "query" || e == "instantiate") {
            return Err("the module exports no entry point (instantiate, call, query)".into());
        }
        entries.sort_by_key(|e| ENTRY_POINTS.iter().position(|x| x == e));
        Ok((module, CodeInfo { entries, imports }))
    }

    /// The compiled module for `code` (cached by its hash; compile failures are errors, which
    /// cannot happen for code that passed `validate` at deploy).
    pub fn module(&self, code_hash: &Hash, code: &[u8]) -> Result<Module, String> {
        if let Some(m) = self.cache.lock().unwrap().get(code_hash) {
            return Ok(m.clone());
        }
        let m = Module::new(&self.engine, code).map_err(|e| format!("invalid module: {}", short(&e)))?;
        let mut c = self.cache.lock().unwrap();
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        c.insert(*code_hash, m.clone());
        Ok(m)
    }

    /// Does `module` export entry point `name`?
    pub fn has_entry(module: &Module, name: &str) -> bool {
        module.exports().any(|e| e.name() == name && matches!(e.ty(), ExternType::Func(_)))
    }

    /// Instantiate a frame with `gas` (load cost not yet charged: the caller charges
    /// [`load_cost`] first).
    pub fn frame(&self, module: &Module, env: Env, mode: Mode, input: Vec<u8>, gas: u64, events_before: usize) -> Result<Frame, Abort> {
        let state = HostState {
            env,
            mode,
            input,
            scratch: Vec::new(),
            ret: Vec::new(),
            events: Vec::new(),
            events_before,
            log: Vec::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MAX_MEMORY_BYTES)
                .memories(1)
                .tables(4)
                .table_elements(10_000)
                .instances(1)
                .trap_on_grow_failure(false)
                .build(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_fuel(gas).map_err(|_| Abort::Trap("fuel".into()))?;
        let instance = match self.linker.instantiate_and_start(&mut store, module) {
            Ok(i) => i,
            Err(e) => return Err(map_error(&e, &store)),
        };
        Ok(Frame { store, instance, pending: None })
    }
}

/// Gas a frame's load costs (charged before it runs).
pub fn load_cost(code_len: usize) -> u64 {
    code_len as u64 * gas::LOAD_PER_CODE_BYTE
}

/// Gas DeployCode's validation costs.
pub fn deploy_cost(code_len: usize) -> u64 {
    code_len as u64 * gas::DEPLOY_PER_CODE_BYTE
}

/// A fixed, version-stable description of a wasmi error (receipts are consensus data, so
/// error text must not depend on anything but the cause).
fn short(e: &Error) -> String {
    if let Some(t) = e.as_trap_code() {
        return format!("{t:?}");
    }
    let s = e.to_string();
    s.chars().take(120).collect()
}

fn map_error(e: &Error, store: &Store<HostState>) -> Abort {
    if let Some(h) = e.downcast_ref::<HostSignal>() {
        return match h {
            HostSignal::Abort(a) => a.clone(),
            HostSignal::Req(_) => Abort::Trap("host call: request outside a resumable call".into()),
        };
    }
    if let Some(t) = e.as_trap_code() {
        if t == wasmi::TrapCode::OutOfFuel {
            return Abort::OutOfGas;
        }
        return Abort::Trap(format!("trap: {}", trap_name(t)));
    }
    let _ = store;
    Abort::Trap("trap: instantiation failed".into())
}

fn trap_name(t: wasmi::TrapCode) -> &'static str {
    use wasmi::TrapCode::*;
    match t {
        UnreachableCodeReached => "unreachable",
        MemoryOutOfBounds => "memory out of bounds",
        TableOutOfBounds => "table out of bounds",
        IndirectCallToNull => "indirect call to null",
        IntegerDivisionByZero => "integer division by zero",
        IntegerOverflow => "integer overflow",
        BadConversionToInteger => "bad conversion to integer",
        StackOverflow => "stack overflow",
        BadSignature => "bad signature",
        OutOfFuel => "out of gas",
        GrowthOperationLimited => "growth limited",
    }
}

/// One running contract call.
pub struct Frame {
    store: Store<HostState>,
    instance: Instance,
    pending: Option<ResumableCallHostTrap>,
}

impl Frame {
    pub fn gas_left(&self) -> u64 {
        self.store.get_fuel().unwrap_or(0)
    }

    /// Take `amount` gas from the frame (false = not enough; the frame keeps 0).
    pub fn charge(&mut self, amount: u64) -> bool {
        let left = self.gas_left();
        if left < amount {
            let _ = self.store.set_fuel(0);
            return false;
        }
        let _ = self.store.set_fuel(left - amount);
        true
    }

    pub fn events(&mut self) -> Vec<RawEvent> {
        std::mem::take(&mut self.store.data_mut().events)
    }

    pub fn log(&self) -> &[u8] {
        &self.store.data().log
    }

    /// Events a finished nested call added to this tx (for the per-tx event cap).
    pub fn add_events_before(&mut self, n: usize) {
        self.store.data_mut().events_before += n;
    }

    pub fn events_len(&self) -> usize {
        self.store.data().events.len()
    }

    /// Start entry point `name` (must exist: checked with [`Vm::has_entry`]).
    pub fn start(&mut self, name: &str) -> Step {
        let Some(f): Option<Func> = self.instance.get_func(&self.store, name) else {
            return Step::Abort(Abort::Trap(format!("no entry point {name}")));
        };
        let r = f.call_resumable(&mut self.store, &[], &mut []);
        self.step(r)
    }

    /// Answer the pending request and continue.
    pub fn resume(&mut self, reply: Reply) -> Step {
        let Some(inv) = self.pending.take() else { return Step::Abort(Abort::Trap("host call: nothing to resume".into())) };
        let vals: Vec<Val> = match reply {
            Reply::Unit => vec![],
            Reply::Value(v) => {
                let n = match &v {
                    Some(b) => b.len() as i32,
                    None => -1,
                };
                if let Some(b) = &v {
                    if !self.charge(b.len() as u64 * gas::STORAGE_READ_PER_BYTE) {
                        return Step::Abort(Abort::OutOfGas);
                    }
                }
                self.store.data_mut().scratch = v.unwrap_or_default();
                vec![Val::I32(n)]
            }
            Reply::I64(x) => vec![Val::I64(x)],
            Reply::Status(s, msg) => {
                self.store.data_mut().scratch = msg;
                vec![Val::I32(s)]
            }
            Reply::Call { ok, data, gas_used } => {
                if !self.charge(gas_used) {
                    return Step::Abort(Abort::OutOfGas);
                }
                self.store.data_mut().scratch = data;
                vec![Val::I32(if ok { 0 } else { 1 })]
            }
        };
        let r = inv.resume(&mut self.store, &vals, &mut []);
        self.step(r)
    }

    fn step(&mut self, r: Result<ResumableCall, Error>) -> Step {
        match r {
            Ok(ResumableCall::Finished) => Step::Done { ret: std::mem::take(&mut self.store.data_mut().ret) },
            Ok(ResumableCall::OutOfFuel(_)) => {
                let _ = self.store.set_fuel(0);
                Step::Abort(Abort::OutOfGas)
            }
            Ok(ResumableCall::HostTrap(inv)) => {
                let sig = inv.host_error().downcast_ref::<HostSignal>();
                match sig {
                    Some(HostSignal::Req(r)) => {
                        let r = r.clone();
                        self.pending = Some(inv);
                        Step::Request(r)
                    }
                    Some(HostSignal::Abort(a)) => Step::Abort(a.clone()),
                    None => Step::Abort(Abort::Trap("trap: host".into())),
                }
            }
            Err(e) => {
                let a = map_error(&e, &self.store);
                if a == Abort::OutOfGas {
                    let _ = self.store.set_fuel(0);
                }
                Step::Abort(a)
            }
        }
    }
}
