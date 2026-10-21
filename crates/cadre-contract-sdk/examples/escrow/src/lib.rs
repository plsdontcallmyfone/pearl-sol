//! CADRE escrow (docs/CONTRACTS.md examples): a payer locks CADRE for a payee; the payer or
//! an arbiter releases it, the arbiter (any time) or the payer (after the deadline) refunds it.
//!
//! call open(payee addr, arbiter addr, deadline_height u64) + value -> id u64   emits Opened
//! call release(id u64)    payer or arbiter; pays the payee              emits Released
//! call refund(id u64)     arbiter, or payer after the deadline          emits Refunded
//! query get(id u64) -> payer addr, payee addr, arbiter addr, amount u64, deadline u64, state u8 (0 open, 1 released, 2 refunded)
//! query count() -> u64

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, storage, Address};

struct Deal {
    payer: Address,
    payee: Address,
    arbiter: Address,
    amount: u64,
    deadline: u64,
    state: u8,
}

fn key(id: u64) -> Vec<u8> {
    storage::key(b"deal:", &id.to_le_bytes())
}

fn load(id: u64) -> Result<Deal, &'static str> {
    let b = storage::get(&key(id)).ok_or("no such deal")?;
    let mut r = Reader::new(&b);
    Ok(Deal { payer: r.addr()?, payee: r.addr()?, arbiter: r.addr()?, amount: r.u64()?, deadline: r.u64()?, state: r.u8()? })
}

fn save(id: u64, d: &Deal) {
    let b = Writer::new().addr(&d.payer).addr(&d.payee).addr(&d.arbiter).u64(d.amount).u64(d.deadline).u8(d.state).finish();
    storage::set(&key(id), &b);
}

fn settle(id: u64, to_payee: bool) -> Result<Vec<u8>, &'static str> {
    let mut d = load(id)?;
    if d.state != 0 {
        return Err("deal already settled");
    }
    let e = env();
    if to_payee {
        if e.caller != d.payer && e.caller != d.arbiter {
            return Err("only the payer or the arbiter may release");
        }
    } else if e.caller != d.arbiter && !(e.caller == d.payer && e.height > d.deadline) {
        return Err("only the arbiter, or the payer after the deadline, may refund");
    }
    d.state = if to_payee { 1 } else { 2 };
    save(id, &d);
    let to = if to_payee { d.payee } else { d.payer };
    sdk::transfer(&to, d.amount)?;
    sdk::emit(if to_payee { "Released" } else { "Refunded" }, &[to], &Writer::new().u64(id).u64(d.amount).finish());
    Ok(Writer::new().u64(d.amount).finish())
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| match method {
        "open" => {
            let e = env();
            let payee = args.addr()?;
            let arbiter = args.addr()?;
            let deadline = args.u64()?;
            if e.value == 0 {
                return Err("send the escrowed CADRE as the call's value");
            }
            if deadline <= e.height {
                return Err("deadline must be in the future");
            }
            let id = storage::get_u64(b"count").unwrap_or(0);
            storage::set_u64(b"count", id + 1);
            save(id, &Deal { payer: e.caller, payee, arbiter, amount: e.value, deadline, state: 0 });
            sdk::emit("Opened", &[e.caller, payee], &Writer::new().u64(id).u64(e.value).u64(deadline).finish());
            Ok(Writer::new().u64(id).finish())
        }
        "release" => settle(args.u64()?, true),
        "refund" => settle(args.u64()?, false),
        _ => Err("unknown method"),
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "get" => {
            let d = load(args.u64()?)?;
            Ok(Writer::new().addr(&d.payer).addr(&d.payee).addr(&d.arbiter).u64(d.amount).u64(d.deadline).u8(d.state).finish())
        }
        "count" => Ok(Writer::new().u64(storage::get_u64(b"count").unwrap_or(0)).finish()),
        _ => Err("unknown method"),
    })
}
