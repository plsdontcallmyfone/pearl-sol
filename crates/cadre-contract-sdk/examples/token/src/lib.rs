//! Fungible token (docs/CONTRACTS.md examples): balances, allowances, an optional minter.
//!
//! instantiate(name str, symbol str, decimals u8, supply u64, mintable bool)   supply to the sender
//! call transfer(to addr, amount u64)                     emits Transfer(from, to)
//! call approve(spender addr, amount u64)                 emits Approval(owner, spender)
//! call transfer_from(from addr, to addr, amount u64)     spends the allowance
//! call mint(to addr, amount u64)                         minter only, if mintable
//! call burn(amount u64)
//! query balance_of(addr) -> u64, allowance(owner addr, spender addr) -> u64,
//!       info() -> name str, symbol str, decimals u8, total_supply u64, minter addr

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Writer, env, storage, Address};

fn bal_key(a: &Address) -> Vec<u8> {
    storage::key(b"b:", a)
}
fn allow_key(o: &Address, s: &Address) -> Vec<u8> {
    let mut k = storage::key(b"a:", o);
    k.extend_from_slice(s);
    k
}
fn bal(a: &Address) -> u64 {
    storage::get_u64(&bal_key(a)).unwrap_or(0)
}
fn set_bal(a: &Address, v: u64) {
    if v == 0 {
        storage::remove(&bal_key(a));
    } else {
        storage::set_u64(&bal_key(a), v);
    }
}
fn supply() -> u64 {
    storage::get_u64(b"supply").unwrap_or(0)
}

fn move_tokens(from: &Address, to: &Address, amount: u64) -> Result<(), &'static str> {
    let fb = bal(from);
    if fb < amount {
        return Err("insufficient token balance");
    }
    if from != to {
        set_bal(from, fb - amount);
        set_bal(to, bal(to).checked_add(amount).ok_or("overflow")?);
    }
    sdk::emit("Transfer", &[*from, *to], &amount.to_le_bytes());
    Ok(())
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let name = args.str()?;
        let symbol = args.str()?;
        let decimals = args.u8()?;
        let initial = args.u64()?;
        let mintable = args.bool()?;
        if name.is_empty() || symbol.is_empty() || symbol.len() > 16 || decimals > 18 {
            return Err("bad token metadata");
        }
        let me = env().caller;
        storage::set(b"name", name.as_bytes());
        storage::set(b"symbol", symbol.as_bytes());
        storage::set(b"decimals", &[decimals]);
        storage::set(b"minter", &if mintable { me } else { [0; 32] });
        storage::set_u64(b"supply", initial);
        set_bal(&me, initial);
        sdk::emit("Transfer", &[[0; 32], me], &initial.to_le_bytes());
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let me = env().caller;
        match method {
            "transfer" => {
                let to = args.addr()?;
                move_tokens(&me, &to, args.u64()?)?;
            }
            "approve" => {
                let spender = args.addr()?;
                let amount = args.u64()?;
                storage::set_u64(&allow_key(&me, &spender), amount);
                sdk::emit("Approval", &[me, spender], &amount.to_le_bytes());
            }
            "transfer_from" => {
                let from = args.addr()?;
                let to = args.addr()?;
                let amount = args.u64()?;
                let k = allow_key(&from, &me);
                let a = storage::get_u64(&k).unwrap_or(0);
                if a < amount {
                    return Err("allowance too small");
                }
                storage::set_u64(&k, a - amount);
                move_tokens(&from, &to, amount)?;
            }
            "mint" => {
                let minter = storage::get_addr(b"minter").unwrap_or([0; 32]);
                if minter == [0; 32] || minter != me {
                    return Err("only the minter may mint");
                }
                let to = args.addr()?;
                let amount = args.u64()?;
                storage::set_u64(b"supply", supply().checked_add(amount).ok_or("overflow")?);
                set_bal(&to, bal(&to).checked_add(amount).ok_or("overflow")?);
                sdk::emit("Transfer", &[[0; 32], to], &amount.to_le_bytes());
            }
            "burn" => {
                let amount = args.u64()?;
                let b = bal(&me);
                if b < amount {
                    return Err("insufficient token balance");
                }
                set_bal(&me, b - amount);
                storage::set_u64(b"supply", supply() - amount);
                sdk::emit("Transfer", &[me, [0; 32]], &amount.to_le_bytes());
            }
            _ => return Err("unknown method"),
        }
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "balance_of" => Ok(Writer::new().u64(bal(&args.addr()?)).finish()),
        "allowance" => {
            let o = args.addr()?;
            let s = args.addr()?;
            Ok(Writer::new().u64(storage::get_u64(&allow_key(&o, &s)).unwrap_or(0)).finish())
        }
        "info" => {
            let name = storage::get(b"name").unwrap_or_default();
            let symbol = storage::get(b"symbol").unwrap_or_default();
            let decimals = storage::get(b"decimals").and_then(|d| d.first().copied()).unwrap_or(0);
            Ok(Writer::new()
                .bytes(&name)
                .bytes(&symbol)
                .u8(decimals)
                .u64(supply())
                .addr(&storage::get_addr(b"minter").unwrap_or([0; 32]))
                .finish())
        }
        _ => Err("unknown method"),
    })
}
