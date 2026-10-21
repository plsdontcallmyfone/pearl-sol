//! Test contract for the chain's VM tests (not an example to copy): cross-contract calls,
//! reentrancy attempts, gas burning, storage churn, native calls.
//!
//! call forward(target addr, value u64, gas u64, input bytes) -> callee output   (reverts if the callee fails)
//! call try_forward(target, value, gas, input) -> ok bool, data bytes           (keeps going if it fails)
//! call reenter(target addr)          calls target.forward(this contract, ...) so target calls back here
//! call burn(loops u64)               spins (gas exhaustion)
//! call fill(n u32, size u32)         writes n items of `size` bytes
//! call clear(n u32)                  removes them
//! call write_then_fail()             writes, then reverts
//! call pay(to addr, amount u64)      sends CADRE from this contract
//! call post(text str)                native town post as this contract
//! query echo(bytes) -> bytes, get(key bytes) -> value bytes

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Writer, env, native, storage};

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| match method {
        "forward" | "try_forward" => {
            let target = args.addr()?;
            let value = args.u64()?;
            let gas = args.u64()?;
            let input = args.bytes()?;
            match sdk::call(&target, value, input, gas) {
                Ok(out) if method == "forward" => Ok(out),
                Ok(out) => Ok(Writer::new().bool(true).bytes(&out).finish()),
                Err(_) if method == "forward" => Err("callee failed"),
                Err(e) => Ok(Writer::new().bool(false).bytes(&e).finish()),
            }
        }
        "reenter" => {
            let target = args.addr()?;
            let me = env().address;
            let back = Writer::method("burn").u64(1).finish();
            let input = Writer::method("forward").addr(&me).u64(0).u64(0).bytes(&back).finish();
            sdk::call(&target, 0, &input, 0).map_err(|_| "reentry refused")
        }
        "burn" => {
            let n = args.u64()?;
            let mut x: u64 = 1;
            let mut i = 0;
            while i < n {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                i += 1;
            }
            storage::set_u64(b"burn", x);
            Ok(Writer::new().u64(x).finish())
        }
        "fill" => {
            let n = args.u32()?;
            let size = args.u32()? as usize;
            let v = alloc::vec![7u8; size];
            for i in 0..n {
                storage::set(&storage::key(b"f:", &i.to_le_bytes()), &v);
            }
            Ok(Vec::new())
        }
        "clear" => {
            let n = args.u32()?;
            for i in 0..n {
                storage::remove(&storage::key(b"f:", &i.to_le_bytes()));
            }
            Ok(Vec::new())
        }
        "write_then_fail" => {
            storage::set(b"x", b"should not stay");
            sdk::emit("Never", &[], &[]);
            Err("failing on purpose")
        }
        "pay" => {
            let to = args.addr()?;
            sdk::transfer(&to, args.u64()?)?;
            Ok(Vec::new())
        }
        "post" => {
            native::post(args.str()?).map_err(|_| "post failed")?;
            Ok(Vec::new())
        }
        _ => Err("unknown method"),
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "echo" => Ok(args.bytes()?.to_vec()),
        "get" => storage::get(args.bytes()?).ok_or("missing"),
        "write" => {
            storage::set(b"q", b"query writes must fail");
            Ok(Vec::new())
        }
        _ => Err("unknown method"),
    })
}
