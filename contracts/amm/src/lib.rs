//! Cadre AMM (docs/TRADING.md): one constant-product pool per instance, native CADRE against
//! one token contract (the SDK's example token or any contract with the same `transfer`,
//! `transfer_from` and `approve` methods). Every swap is a Cadre `Call` tx with a receipt and a
//! `Swap` event, so an agent's trading record is read straight from the chain.
//!
//! instantiate(token addr, fee_bps u16, admin addr)     admin zero = the fee can never change
//! call add_liquidity(max_token u64, min_lp u64, max_height u64) + value CADRE -> lp u64
//!      first deposit sets the price: lp = isqrt(c * t) - 1000 (1000 locked for good);
//!      later: t = ceil(c * T / C) pulled with transfer_from (approve this pool first)
//! call remove_liquidity(lp u64, min_cadre u64, min_token u64, max_height u64) -> cadre u64, token u64
//! call swap_cadre_for_token(min_out u64, to addr, max_height u64) + value CADRE -> out u64
//! call swap_token_for_cadre(amount_in u64, min_out u64, to addr, max_height u64) -> out u64
//!      (approve this pool for amount_in on the token first)
//! call info() -> token addr, reserve_cadre u64, reserve_token u64, lp_supply u64, fee_bps u16
//!      (also a call method so other contracts, like the mandate, can read reserves)
//! call set_fee(fee_bps u16)                            admin only, at most 100 bps
//! query info(), lp_of(addr) -> u64, quote(side u8, amount_in u64) -> out u64
//!
//! Reserves are tracked in storage, not read from balances, so a donation never moves the price.
//! `max_height` (0 = none) bounds when a swap may land: a tx left in the mempool past it fails.
//! Events: Swap(trader, to) data side u8 | in u64 | out u64 | reserve_cadre u64 | reserve_token u64 | origin addr,
//!         Mint(provider) / Burn(provider) data cadre u64 | token u64 | lp u64.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Writer, env, storage, Address};

const MIN_LIQ: u64 = 1_000;
const MAX_FEE_BPS: u16 = 100;
const ZERO: Address = [0; 32];

fn token() -> Address {
    storage::get_addr(b"token").unwrap_or(ZERO)
}
fn rc() -> u64 {
    storage::get_u64(b"rc").unwrap_or(0)
}
fn rt() -> u64 {
    storage::get_u64(b"rt").unwrap_or(0)
}
fn supply() -> u64 {
    storage::get_u64(b"lp").unwrap_or(0)
}
fn fee() -> u16 {
    storage::get(b"fee").and_then(|v| v.get(..2).map(|b| u16::from_le_bytes([b[0], b[1]]))).unwrap_or(30)
}
fn lp_key(a: &Address) -> Vec<u8> {
    storage::key(b"l:", a)
}
fn lp_of(a: &Address) -> u64 {
    storage::get_u64(&lp_key(a)).unwrap_or(0)
}
fn set_lp(a: &Address, v: u64) {
    if v == 0 {
        storage::remove(&lp_key(a));
    } else {
        storage::set_u64(&lp_key(a), v);
    }
}

fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Output for `x` in against reserves (r_in, r_out), fee on the input.
pub fn out_for(r_in: u64, r_out: u64, x: u64, fee_bps: u16) -> u64 {
    let xf = x as u128 * (10_000 - fee_bps as u128);
    let den = r_in as u128 * 10_000 + xf;
    if den == 0 {
        return 0;
    }
    (xf * r_out as u128 / den) as u64
}

fn check_height(max_height: u64) -> Result<(), &'static str> {
    if max_height != 0 && env().height > max_height {
        return Err("deadline passed");
    }
    Ok(())
}

fn pull_token(from: &Address, amount: u64) -> Result<(), &'static str> {
    let me = env().address;
    let input = Writer::method("transfer_from").addr(from).addr(&me).u64(amount).finish();
    sdk::call(&token(), 0, &input, 0).map(|_| ()).map_err(|_| "token transfer_from failed (approve the pool first)")
}

fn push_token(to: &Address, amount: u64) -> Result<(), &'static str> {
    let input = Writer::method("transfer").addr(to).u64(amount).finish();
    sdk::call(&token(), 0, &input, 0).map(|_| ()).map_err(|_| "token transfer failed")
}

fn set_reserves(c: u64, t: u64) {
    storage::set_u64(b"rc", c);
    storage::set_u64(b"rt", t);
}

fn info_bytes() -> Vec<u8> {
    Writer::new().addr(&token()).u64(rc()).u64(rt()).u64(supply()).u16(fee()).finish()
}

fn swap_event(side: u8, trader: &Address, to: &Address, x: u64, out: u64) {
    let e = env();
    let data = Writer::new().u8(side).u64(x).u64(out).u64(rc()).u64(rt()).addr(&e.origin).finish();
    sdk::emit("Swap", &[*trader, *to], &data);
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let t = args.addr()?;
        let f = args.u16()?;
        let admin = args.addr()?;
        if t == ZERO {
            return Err("token address is zero");
        }
        if f > MAX_FEE_BPS {
            return Err("fee above 100 bps");
        }
        storage::set(b"token", &t);
        storage::set(b"fee", &f.to_le_bytes());
        storage::set(b"admin", &admin);
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        let me = e.caller;
        match method {
            "info" => Ok(info_bytes()),
            "add_liquidity" => {
                let max_token = args.u64()?;
                let min_lp = args.u64()?;
                check_height(args.u64()?)?;
                let c = e.value;
                if c == 0 {
                    return Err("send CADRE as value");
                }
                let (cr, tr, s) = (rc(), rt(), supply());
                let (t, lp) = if s == 0 {
                    let t = max_token;
                    let root = isqrt(c as u128 * t as u128) as u64;
                    if root <= MIN_LIQ {
                        return Err("first deposit too small");
                    }
                    set_lp(&ZERO, MIN_LIQ);
                    storage::set_u64(b"lp", MIN_LIQ);
                    (t, root - MIN_LIQ)
                } else {
                    let t = ((c as u128 * tr as u128 + cr as u128 - 1) / cr as u128) as u64;
                    if t > max_token {
                        return Err("needs more token than max_token at this price");
                    }
                    (t, (c as u128 * s as u128 / cr as u128) as u64)
                };
                if lp == 0 || lp < min_lp {
                    return Err("lp below min_lp");
                }
                pull_token(&me, t)?;
                set_reserves(cr + c, tr.checked_add(t).ok_or("overflow")?);
                storage::set_u64(b"lp", supply() + lp);
                set_lp(&me, lp_of(&me) + lp);
                sdk::emit("Mint", &[me], &Writer::new().u64(c).u64(t).u64(lp).finish());
                Ok(Writer::new().u64(lp).finish())
            }
            "remove_liquidity" => {
                let lp = args.u64()?;
                let min_c = args.u64()?;
                let min_t = args.u64()?;
                check_height(args.u64()?)?;
                let have = lp_of(&me);
                if lp == 0 || lp > have {
                    return Err("not enough lp");
                }
                let (cr, tr, s) = (rc(), rt(), supply());
                let c = (lp as u128 * cr as u128 / s as u128) as u64;
                let t = (lp as u128 * tr as u128 / s as u128) as u64;
                if c < min_c || t < min_t {
                    return Err("below min amounts");
                }
                set_lp(&me, have - lp);
                storage::set_u64(b"lp", s - lp);
                set_reserves(cr - c, tr - t);
                sdk::transfer(&me, c)?;
                push_token(&me, t)?;
                sdk::emit("Burn", &[me], &Writer::new().u64(c).u64(t).u64(lp).finish());
                Ok(Writer::new().u64(c).u64(t).finish())
            }
            "swap_cadre_for_token" => {
                let min_out = args.u64()?;
                let to = args.addr()?;
                check_height(args.u64()?)?;
                let x = e.value;
                if x == 0 {
                    return Err("send CADRE as value");
                }
                let (cr, tr) = (rc(), rt());
                if cr == 0 || tr == 0 {
                    return Err("no liquidity");
                }
                let out = out_for(cr, tr, x, fee());
                if out == 0 || out < min_out {
                    return Err("slippage: out below min_out");
                }
                set_reserves(cr + x, tr - out);
                push_token(&to, out)?;
                swap_event(0, &me, &to, x, out);
                Ok(Writer::new().u64(out).finish())
            }
            "swap_token_for_cadre" => {
                let x = args.u64()?;
                let min_out = args.u64()?;
                let to = args.addr()?;
                check_height(args.u64()?)?;
                if x == 0 {
                    return Err("amount_in is zero");
                }
                let (cr, tr) = (rc(), rt());
                if cr == 0 || tr == 0 {
                    return Err("no liquidity");
                }
                let out = out_for(tr, cr, x, fee());
                if out == 0 || out < min_out {
                    return Err("slippage: out below min_out");
                }
                pull_token(&me, x)?;
                set_reserves(cr - out, tr.checked_add(x).ok_or("overflow")?);
                sdk::transfer(&to, out)?;
                swap_event(1, &me, &to, x, out);
                Ok(Writer::new().u64(out).finish())
            }
            "set_fee" => {
                let f = args.u16()?;
                let admin = storage::get_addr(b"admin").unwrap_or(ZERO);
                if admin == ZERO || admin != me {
                    return Err("only the admin");
                }
                if f > MAX_FEE_BPS {
                    return Err("fee above 100 bps");
                }
                storage::set(b"fee", &f.to_le_bytes());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "info" => Ok(info_bytes()),
        "lp_of" => Ok(Writer::new().u64(lp_of(&args.addr()?)).finish()),
        "quote" => {
            let side = args.u8()?;
            let x = args.u64()?;
            let out = if side == 0 { out_for(rc(), rt(), x, fee()) } else { out_for(rt(), rc(), x, fee()) };
            Ok(Writer::new().u64(out).finish())
        }
        _ => Err("unknown method"),
    })
}
