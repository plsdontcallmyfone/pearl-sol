//! Counter: the smallest useful contract (docs/CONTRACTS.md examples).
//!
//! instantiate(start u64)        owner = the sender
//! call increment(by u64) -> u64  anyone; emits Incremented(new value)
//! call reset()                   owner only
//! query get() -> u64, owner() -> addr

#![no_std]
extern crate alloc;

use cadre_contract_sdk::{self as sdk, abi::Writer, env, storage};

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let start = if args.remaining() >= 8 { args.u64()? } else { 0 };
        storage::set_u64(b"n", start);
        storage::set(b"owner", &env().caller);
        Ok(Writer::new().u64(start).finish())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| match method {
        "increment" => {
            let by = args.u64()?;
            let n = storage::get_u64(b"n").unwrap_or(0).checked_add(by).ok_or("overflow")?;
            storage::set_u64(b"n", n);
            sdk::emit("Incremented", &[], &n.to_le_bytes());
            Ok(Writer::new().u64(n).finish())
        }
        "reset" => {
            if storage::get_addr(b"owner") != Some(env().caller) {
                return Err("only the owner may reset");
            }
            storage::set_u64(b"n", 0);
            sdk::emit("Reset", &[], &[]);
            Ok(Writer::new().u64(0).finish())
        }
        _ => Err("unknown method"),
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, _| match method {
        "get" => Ok(Writer::new().u64(storage::get_u64(b"n").unwrap_or(0)).finish()),
        "owner" => Ok(Writer::new().addr(&storage::get_addr(b"owner").unwrap_or([0; 32])).finish()),
        _ => Err("unknown method"),
    })
}
