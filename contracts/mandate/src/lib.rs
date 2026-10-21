//! Cadre-native mandate (docs/TRADING.md, decision D7 on the Cadre chain): user-owned trading
//! accounts kept by one contract. The user deposits CADRE; the named agent key can only swap
//! that CADRE against allowed tokens on pools whose code is the audited AMM (code hash checked
//! on chain), inside limits stored here; the user withdraws CADRE or tokens at any time, also
//! while paused, revoked, expired or under the admin kill switch. Fees: a performance fee only
//! on realized CADRE profit above a high-water mark (`settle`, permissionless), with
//! `protocol_bps` of every fee sent to `protocol_to`.
//!
//! instantiate(admin addr, protocol_bps u16, protocol_to addr, amm_code hash)
//! call open(agent addr, fee_to addr, perf_fee_bps u16, settle_blocks u64, limits) + value -> id u64
//! call deposit(id u64) + value
//! call withdraw(id u64, amount u64)                      user only, never blocked
//! call withdraw_token(id u64, token addr, amount u64)    user only, never blocked
//! call set_limits(id u64, limits) / set_agent(id u64, agent addr) / revoke(id u64)   user only
//! call swap(id u64, pool addr, side u8, amount_in u64, min_out u64, max_height u64) -> out u64   agent only
//!      side 0: CADRE -> token, side 1: token -> CADRE
//! call settle(id u64)                                    anyone, once per settle_blocks
//! call set_config(protocol_bps u16, protocol_to addr, amm_code hash, swaps_paused bool, admin addr)   admin only
//! query get(id u64) -> the mandate record (layout in `Mandate::write`), position(id u64, token addr) -> u64,
//!       config() -> admin, protocol_bps u16, protocol_to, amm_code, swaps_paused bool, count() -> u64
//!
//! limits = max_in u64 | window_cap u64 | window_blocks u64 | max_trades u16 | max_slippage_bps u16
//!          | min_pool_cadre u64 | max_trade_share_bps u16 | expiry_height u64 | paused bool
//!          | any_token bool | n u8 | tokens addr*n (n <= 4)
//! PnL base: equity (CADRE held for the mandate) + withdrawn + fees - deposited; token
//! positions are never marked, so a fee is earned only on profit realized back into CADRE.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, native, storage, Address, Hash};

const ZERO: Address = [0; 32];
const MAX_TOKENS: usize = 4;
const MAX_PROTOCOL_BPS: u16 = 5_000;
const MAX_PERF_BPS: u16 = 5_000;

#[derive(Clone)]
struct Limits {
    max_in: u64,
    window_cap: u64,
    window_blocks: u64,
    max_trades: u16,
    max_slippage_bps: u16,
    min_pool_cadre: u64,
    max_trade_share_bps: u16,
    expiry_height: u64,
    paused: bool,
    any_token: bool,
    tokens: Vec<Address>,
}

impl Limits {
    fn read(r: &mut Reader) -> Result<Self, &'static str> {
        let mut l = Limits {
            max_in: r.u64()?,
            window_cap: r.u64()?,
            window_blocks: r.u64()?,
            max_trades: r.u16()?,
            max_slippage_bps: r.u16()?,
            min_pool_cadre: r.u64()?,
            max_trade_share_bps: r.u16()?,
            expiry_height: r.u64()?,
            paused: r.bool()?,
            any_token: r.bool()?,
            tokens: Vec::new(),
        };
        let n = r.u8()? as usize;
        if n > MAX_TOKENS {
            return Err("at most 4 tokens");
        }
        for _ in 0..n {
            l.tokens.push(r.addr()?);
        }
        if !l.any_token && l.tokens.is_empty() {
            return Err("no tokens allowed");
        }
        if l.max_slippage_bps > 10_000 || l.max_trade_share_bps > 10_000 || l.window_blocks == 0 {
            return Err("bad limits");
        }
        Ok(l)
    }
    fn write(&self, w: Writer) -> Writer {
        let mut w = w
            .u64(self.max_in)
            .u64(self.window_cap)
            .u64(self.window_blocks)
            .u16(self.max_trades)
            .u16(self.max_slippage_bps)
            .u64(self.min_pool_cadre)
            .u16(self.max_trade_share_bps)
            .u64(self.expiry_height)
            .bool(self.paused)
            .bool(self.any_token)
            .u8(self.tokens.len() as u8);
        for t in &self.tokens {
            w = w.addr(t);
        }
        w
    }
    fn allows(&self, t: &Address) -> bool {
        self.any_token || self.tokens.contains(t)
    }
}

struct Mandate {
    user: Address,
    agent: Address,
    fee_to: Address,
    perf_bps: u16,
    settle_blocks: u64,
    revoked: bool,
    limits: Limits,
    equity: u64,
    deposited: u64,
    withdrawn: u64,
    fees_agent: u64,
    fees_protocol: u64,
    hwm: i64,
    last_settle: u64,
    window_start: u64,
    window_used: u64,
    window_trades: u16,
    trades: u64,
    volume: u64,
}

impl Mandate {
    fn write(&self) -> Vec<u8> {
        let w = Writer::new().addr(&self.user).addr(&self.agent).addr(&self.fee_to).u16(self.perf_bps).u64(self.settle_blocks).bool(self.revoked);
        self.limits
            .write(w)
            .u64(self.equity)
            .u64(self.deposited)
            .u64(self.withdrawn)
            .u64(self.fees_agent)
            .u64(self.fees_protocol)
            .u64(self.hwm as u64)
            .u64(self.last_settle)
            .u64(self.window_start)
            .u64(self.window_used)
            .u16(self.window_trades)
            .u64(self.trades)
            .u64(self.volume)
            .finish()
    }
    fn read(b: &[u8]) -> Result<Self, &'static str> {
        let mut r = Reader::new(b);
        Ok(Mandate {
            user: r.addr()?,
            agent: r.addr()?,
            fee_to: r.addr()?,
            perf_bps: r.u16()?,
            settle_blocks: r.u64()?,
            revoked: r.bool()?,
            limits: Limits::read(&mut r)?,
            equity: r.u64()?,
            deposited: r.u64()?,
            withdrawn: r.u64()?,
            fees_agent: r.u64()?,
            fees_protocol: r.u64()?,
            hwm: r.u64()? as i64,
            last_settle: r.u64()?,
            window_start: r.u64()?,
            window_used: r.u64()?,
            window_trades: r.u16()?,
            trades: r.u64()?,
            volume: r.u64()?,
        })
    }
    fn pnl(&self) -> i64 {
        let v = self.equity as i128 + self.withdrawn as i128 + self.fees_agent as i128 + self.fees_protocol as i128 - self.deposited as i128;
        v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
}

fn mkey(id: u64) -> Vec<u8> {
    storage::key(b"m:", &id.to_le_bytes())
}
fn load(id: u64) -> Result<Mandate, &'static str> {
    Mandate::read(&storage::get(&mkey(id)).ok_or("no such mandate")?)
}
fn save(id: u64, m: &Mandate) {
    storage::set(&mkey(id), &m.write());
}
fn pkey(id: u64, t: &Address) -> Vec<u8> {
    let mut k = storage::key(b"p:", &id.to_le_bytes());
    k.extend_from_slice(t);
    k
}
fn pos(id: u64, t: &Address) -> u64 {
    storage::get_u64(&pkey(id, t)).unwrap_or(0)
}
fn set_pos(id: u64, t: &Address, v: u64) {
    if v == 0 {
        storage::remove(&pkey(id, t));
    } else {
        storage::set_u64(&pkey(id, t), v);
    }
}

struct Cfg {
    admin: Address,
    protocol_bps: u16,
    protocol_to: Address,
    amm_code: Hash,
    swaps_paused: bool,
}
fn cfg() -> Cfg {
    let b = storage::get(b"cfg").unwrap_or_default();
    let mut r = Reader::new(&b);
    Cfg {
        admin: r.addr().unwrap_or(ZERO),
        protocol_bps: r.u16().unwrap_or(0),
        protocol_to: r.addr().unwrap_or(ZERO),
        amm_code: r.hash().unwrap_or(ZERO),
        swaps_paused: r.bool().unwrap_or(false),
    }
}
fn save_cfg(c: &Cfg) -> Result<(), &'static str> {
    if c.protocol_bps > MAX_PROTOCOL_BPS {
        return Err("protocol share above 50%");
    }
    storage::set(b"cfg", &Writer::new().addr(&c.admin).u16(c.protocol_bps).addr(&c.protocol_to).hash(&c.amm_code).bool(c.swaps_paused).finish());
    Ok(())
}

fn cp(r_in: u64, r_out: u64, x: u64) -> u64 {
    let den = r_in as u128 + x as u128;
    if den == 0 {
        0
    } else {
        (r_out as u128 * x as u128 / den) as u64
    }
}

/// The pool's code hash must be the configured AMM code (native read kind 5 = contract).
fn check_pool(pool: &Address, code: &Hash) -> Result<(), &'static str> {
    let rec = native::read_raw(5, pool).ok_or("pool is not a contract")?;
    if rec.len() < 32 || &rec[..32] != code {
        return Err("pool code is not the allowed AMM");
    }
    Ok(())
}

fn user_only(m: &Mandate) -> Result<(), &'static str> {
    if env().caller != m.user {
        return Err("only the mandate's user");
    }
    Ok(())
}

fn ev(name: &str, id: u64, who: &Address, data: Vec<u8>) {
    let mut t = [0u8; 32];
    t[..8].copy_from_slice(&id.to_le_bytes());
    sdk::emit(name, &[t, *who], &data);
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let c = Cfg { admin: args.addr()?, protocol_bps: args.u16()?, protocol_to: args.addr()?, amm_code: args.hash()?, swaps_paused: false };
        save_cfg(&c)?;
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        let me = e.caller;
        match method {
            "open" => {
                let agent = args.addr()?;
                let fee_to = args.addr()?;
                let perf_bps = args.u16()?;
                let settle_blocks = args.u64()?;
                let limits = Limits::read(args)?;
                if perf_bps > MAX_PERF_BPS {
                    return Err("performance fee above 50%");
                }
                let id = storage::get_u64(b"n").unwrap_or(0);
                storage::set_u64(b"n", id + 1);
                let m = Mandate {
                    user: me,
                    agent,
                    fee_to,
                    perf_bps,
                    settle_blocks,
                    revoked: false,
                    limits,
                    equity: e.value,
                    deposited: e.value,
                    withdrawn: 0,
                    fees_agent: 0,
                    fees_protocol: 0,
                    hwm: 0,
                    last_settle: 0,
                    window_start: e.height,
                    window_used: 0,
                    window_trades: 0,
                    trades: 0,
                    volume: 0,
                };
                save(id, &m);
                ev("Opened", id, &me, Writer::new().addr(&agent).addr(&fee_to).u16(perf_bps).u64(e.value).finish());
                Ok(Writer::new().u64(id).finish())
            }
            "deposit" => {
                let id = args.u64()?;
                let mut m = load(id)?;
                user_only(&m)?;
                if e.value == 0 {
                    return Err("send CADRE as value");
                }
                m.equity += e.value;
                m.deposited += e.value;
                save(id, &m);
                ev("Deposited", id, &me, Writer::new().u64(e.value).u64(m.equity).finish());
                Ok(Vec::new())
            }
            "withdraw" => {
                let id = args.u64()?;
                let amount = args.u64()?;
                let mut m = load(id)?;
                user_only(&m)?;
                if amount == 0 || amount > m.equity {
                    return Err("not enough CADRE in the mandate");
                }
                m.equity -= amount;
                m.withdrawn += amount;
                save(id, &m);
                sdk::transfer(&m.user, amount)?;
                ev("Withdrawn", id, &me, Writer::new().addr(&ZERO).u64(amount).u64(m.equity).finish());
                Ok(Vec::new())
            }
            "withdraw_token" => {
                let id = args.u64()?;
                let t = args.addr()?;
                let amount = args.u64()?;
                let m = load(id)?;
                user_only(&m)?;
                let p = pos(id, &t);
                if amount == 0 || amount > p {
                    return Err("not enough of that token in the mandate");
                }
                set_pos(id, &t, p - amount);
                sdk::call(&t, 0, &Writer::method("transfer").addr(&m.user).u64(amount).finish(), 0).map_err(|_| "token transfer failed")?;
                ev("Withdrawn", id, &me, Writer::new().addr(&t).u64(amount).u64(m.equity).finish());
                Ok(Vec::new())
            }
            "set_limits" => {
                let id = args.u64()?;
                let mut m = load(id)?;
                user_only(&m)?;
                m.limits = Limits::read(args)?;
                save(id, &m);
                ev("LimitsSet", id, &me, m.limits.write(Writer::new()).finish());
                Ok(Vec::new())
            }
            "set_agent" => {
                let id = args.u64()?;
                let agent = args.addr()?;
                let mut m = load(id)?;
                user_only(&m)?;
                m.agent = agent;
                m.revoked = false;
                save(id, &m);
                ev("AgentSet", id, &me, Writer::new().addr(&agent).finish());
                Ok(Vec::new())
            }
            "revoke" => {
                let id = args.u64()?;
                let mut m = load(id)?;
                user_only(&m)?;
                m.revoked = true;
                m.agent = ZERO;
                save(id, &m);
                ev("Revoked", id, &me, Vec::new());
                Ok(Vec::new())
            }
            "swap" => {
                let id = args.u64()?;
                let pool = args.addr()?;
                let side = args.u8()?;
                let x = args.u64()?;
                let min_out = args.u64()?;
                let max_height = args.u64()?;
                let c = cfg();
                let mut m = load(id)?;
                if c.swaps_paused {
                    return Err("swaps are paused by the kill switch");
                }
                if m.revoked || m.agent == ZERO {
                    return Err("the agent was revoked");
                }
                if me != m.agent {
                    return Err("only the mandate's agent");
                }
                if m.limits.paused {
                    return Err("the user paused this mandate");
                }
                if m.limits.expiry_height != 0 && e.height >= m.limits.expiry_height {
                    return Err("the mandate expired");
                }
                if side > 1 || x == 0 || min_out == 0 {
                    return Err("bad side or zero amount");
                }
                check_pool(&pool, &c.amm_code)?;
                let info = sdk::call(&pool, 0, &Writer::method("info").finish(), 0).map_err(|_| "pool info failed")?;
                let mut r = Reader::new(&info);
                let token = r.addr()?;
                let rc = r.u64()?;
                let rt = r.u64()?;
                if !m.limits.allows(&token) {
                    return Err("token not allowed by the limits");
                }
                let (quote, value) = if side == 0 { (cp(rc, rt, x), x) } else { let q = cp(rt, rc, x); (q, q) };
                if rc < m.limits.min_pool_cadre {
                    return Err("pool CADRE reserve below the limit");
                }
                if value as u128 * 10_000 > rc as u128 * m.limits.max_trade_share_bps as u128 || value > m.limits.max_in {
                    return Err("trade larger than the limits allow");
                }
                let floor = (quote as u128 * (10_000 - m.limits.max_slippage_bps as u128) / 10_000) as u64;
                if min_out < floor.max(1) {
                    return Err("min_out looser than max_slippage_bps from the pool's quote");
                }
                if e.height >= m.window_start.saturating_add(m.limits.window_blocks) {
                    m.window_start = e.height;
                    m.window_used = 0;
                    m.window_trades = 0;
                }
                m.window_used = m.window_used.saturating_add(value);
                m.window_trades = m.window_trades.saturating_add(1);
                if m.window_used > m.limits.window_cap {
                    return Err("window cap reached");
                }
                if m.window_trades > m.limits.max_trades {
                    return Err("too many trades in this window");
                }
                let me_addr = e.address;
                let bal0 = sdk::balance(&me_addr);
                let out;
                if side == 0 {
                    if x > m.equity {
                        return Err("not enough CADRE in the mandate");
                    }
                    let ret = sdk::call(&pool, x, &Writer::method("swap_cadre_for_token").u64(min_out).addr(&me_addr).u64(max_height).finish(), 0)
                        .map_err(|_| "pool swap failed")?;
                    out = Reader::new(&ret).u64()?;
                    let bal1 = sdk::balance(&me_addr);
                    if bal0 < bal1 || bal0 - bal1 != x || out < min_out {
                        return Err("swap accounting check failed");
                    }
                    m.equity -= x;
                    set_pos(id, &token, pos(id, &token).checked_add(out).ok_or("overflow")?);
                } else {
                    let p = pos(id, &token);
                    if x > p {
                        return Err("not enough of that token in the mandate");
                    }
                    sdk::call(&token, 0, &Writer::method("approve").addr(&pool).u64(x).finish(), 0).map_err(|_| "approve failed")?;
                    let ret = sdk::call(&pool, 0, &Writer::method("swap_token_for_cadre").u64(x).u64(min_out).addr(&me_addr).u64(max_height).finish(), 0)
                        .map_err(|_| "pool swap failed")?;
                    out = Reader::new(&ret).u64()?;
                    let bal1 = sdk::balance(&me_addr);
                    if bal1 < bal0 || bal1 - bal0 != out || out < min_out {
                        return Err("swap accounting check failed");
                    }
                    // leave no allowance behind
                    sdk::call(&token, 0, &Writer::method("approve").addr(&pool).u64(0).finish(), 0).map_err(|_| "approve failed")?;
                    set_pos(id, &token, p - x);
                    m.equity += out;
                }
                m.trades += 1;
                m.volume = m.volume.saturating_add(value);
                save(id, &m);
                ev(
                    "MandateSwap",
                    id,
                    &me,
                    Writer::new().addr(&pool).addr(&token).u8(side).u64(x).u64(out).u64(quote).u64(rc).u64(rt).u64(m.equity).finish(),
                );
                Ok(Writer::new().u64(out).finish())
            }
            "settle" => {
                let id = args.u64()?;
                let c = cfg();
                let mut m = load(id)?;
                if m.last_settle != 0 && e.height < m.last_settle.saturating_add(m.settle_blocks) {
                    return Err("settle interval not reached");
                }
                let pnl = m.pnl();
                let (fee, proto) = if pnl > m.hwm {
                    let gain = (pnl - m.hwm) as u128;
                    let fee = (gain * m.perf_bps as u128 / 10_000).min(m.equity as u128) as u64;
                    (fee, (fee as u128 * c.protocol_bps as u128 / 10_000) as u64)
                } else {
                    (0, 0)
                };
                m.equity -= fee;
                m.fees_agent += fee - proto;
                m.fees_protocol += proto;
                if pnl > m.hwm {
                    m.hwm = pnl;
                }
                m.last_settle = e.height.max(1);
                save(id, &m);
                if proto > 0 {
                    sdk::transfer(&c.protocol_to, proto)?;
                }
                if fee > proto {
                    sdk::transfer(&m.fee_to, fee - proto)?;
                }
                ev("Settled", id, &me, Writer::new().u64(pnl as u64).u64(m.hwm as u64).u64(fee - proto).u64(proto).u64(m.equity).finish());
                Ok(Writer::new().u64(fee - proto).u64(proto).finish())
            }
            "set_config" => {
                let c = cfg();
                if me != c.admin || c.admin == ZERO {
                    return Err("only the admin");
                }
                let n = Cfg { protocol_bps: args.u16()?, protocol_to: args.addr()?, amm_code: args.hash()?, swaps_paused: args.bool()?, admin: args.addr()? };
                save_cfg(&n)?;
                sdk::emit("ConfigSet", &[n.admin], &Writer::new().u16(n.protocol_bps).addr(&n.protocol_to).hash(&n.amm_code).bool(n.swaps_paused).finish());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "get" => Ok(load(args.u64()?)?.write()),
        "position" => {
            let id = args.u64()?;
            let t = args.addr()?;
            Ok(Writer::new().u64(pos(id, &t)).finish())
        }
        "config" => {
            let c = cfg();
            Ok(Writer::new().addr(&c.admin).u16(c.protocol_bps).addr(&c.protocol_to).hash(&c.amm_code).bool(c.swaps_paused).finish())
        }
        "count" => Ok(Writer::new().u64(storage::get_u64(b"n").unwrap_or(0)).finish()),
        _ => Err("unknown method"),
    })
}
