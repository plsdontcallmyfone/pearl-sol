//! Test-only probe (never deployed): tries every way a contract could move CADRE into or out of
//! the mining pool, for contracts/tests/tests/pool.rs.
//!
//! call pay_pool(amount u64)                 native::pay_mining_pool
//!      host_pay(to addr, amount u64)        the `transfer` host function
//!      native(kind u8, body bytes)          any native tx body as this contract
//!      call(contract addr, input bytes)     a cross-contract call

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, native};

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| match method {
        "pay_pool" => {
            native::pay_mining_pool(args.u64()?).map_err(|_| "pay_mining_pool failed")?;
            Ok(Vec::new())
        }
        "host_pay" => {
            let to = args.addr()?;
            sdk::transfer(&to, args.u64()?)?;
            Ok(Vec::new())
        }
        "native" => {
            let kind = args.u8()?;
            native::call_raw(kind as i32, args.bytes()?).map_err(|_| "native call failed")?;
            Ok(Vec::new())
        }
        "call" => {
            let c = args.addr()?;
            sdk::call(&c, 0, args.bytes()?, 0).map_err(|_| "callee failed")
        }
        _ => Err("unknown method"),
    })
}
