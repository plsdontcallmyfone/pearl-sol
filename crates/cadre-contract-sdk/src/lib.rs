//! Cadre contract SDK (docs/CONTRACTS.md, "Build a contract" in the docs site).
//!
//! A contract is a `cdylib` built for `wasm32-unknown-unknown` that exports any of the entry
//! points `instantiate`, `call`, `query`, `migrate` (no arguments, no results) and its
//! `memory`. Inside an entry point, [`dispatch`] reads the input (method name + fields in the
//! [`abi`] encoding), runs your handler and returns its output or reverts with its error.
//!
//! ```ignore
//! #![no_std]
//! use cadre_contract_sdk::{self as sdk, abi::Writer, storage};
//!
//! #[no_mangle]
//! pub extern "C" fn call() {
//!     sdk::dispatch(|method, args| match method {
//!         "increment" => {
//!             let n = storage::get_u64(b"n").unwrap_or(0) + args.u64()?;
//!             storage::set_u64(b"n", n);
//!             Ok(Writer::new().u64(n).finish())
//!         }
//!         _ => Err("unknown method"),
//!     })
//! }
//! ```
//!
//! Rules the chain enforces (docs/CONTRACTS.md): no floating point, no start function, imports
//! only from `cadre`, 4 MiB memory, every host call and instruction costs gas, a failed call
//! undoes all its state changes, and a contract already on the call stack cannot be called
//! again (no reentrancy).

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub mod abi;
pub mod native;

pub type Address = [u8; 32];
pub type Hash = [u8; 32];

/// The host functions (module `cadre`). Use the safe wrappers below instead.
pub mod sys {
    #[link(wasm_import_module = "cadre")]
    extern "C" {
        pub fn input_len() -> i32;
        pub fn input_read(ptr: *mut u8);
        pub fn env_read(ptr: *mut u8);
        pub fn set_return(ptr: *const u8, len: i32);
        pub fn revert(ptr: *const u8, len: i32) -> !;
        pub fn scratch_len() -> i32;
        pub fn scratch_read(ptr: *mut u8);
        pub fn storage_read(kptr: *const u8, klen: i32) -> i32;
        pub fn storage_write(kptr: *const u8, klen: i32, vptr: *const u8, vlen: i32);
        pub fn storage_remove(kptr: *const u8, klen: i32);
        pub fn emit(tptr: *const u8, tcount: i32, dptr: *const u8, dlen: i32);
        pub fn balance(aptr: *const u8) -> i64;
        pub fn transfer(aptr: *const u8, amount: i64) -> i32;
        pub fn call(aptr: *const u8, value: i64, iptr: *const u8, ilen: i32, gas: i64) -> i32;
        pub fn native_read(kind: i32, iptr: *const u8, ilen: i32) -> i32;
        pub fn native_call(kind: i32, bptr: *const u8, blen: i32) -> i32;
        pub fn blake3(ptr: *const u8, len: i32, out: *mut u8);
        pub fn sha256(ptr: *const u8, len: i32, out: *mut u8);
        pub fn ed25519_verify(pk: *const u8, mptr: *const u8, mlen: i32, sig: *const u8) -> i32;
        pub fn gas_left() -> i64;
        pub fn log(ptr: *const u8, len: i32);
    }
}

#[cfg(all(feature = "allocator", target_arch = "wasm32"))]
mod bump {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;

    /// Bump allocator: a call's memory lives only as long as the call, so nothing is freed.
    pub struct Bump {
        next: UnsafeCell<usize>,
    }
    unsafe impl Sync for Bump {}

    impl Bump {
        pub const fn new() -> Self {
            Self { next: UnsafeCell::new(0) }
        }
    }

    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let next = &mut *self.next.get();
            if *next == 0 {
                *next = core::arch::wasm32::memory_size(0) * 65536;
            }
            let start = (*next + l.align() - 1) & !(l.align() - 1);
            let end = match start.checked_add(l.size()) {
                Some(e) => e,
                None => return core::ptr::null_mut(),
            };
            let have = core::arch::wasm32::memory_size(0) * 65536;
            if end > have {
                let pages = (end - have).div_ceil(65536);
                if core::arch::wasm32::memory_grow(0, pages) == usize::MAX {
                    return core::ptr::null_mut();
                }
            }
            *next = end;
            start as *mut u8
        }
        unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
    }

    #[global_allocator]
    static ALLOC: Bump = Bump::new();
}

#[cfg(all(feature = "panic-handler", target_arch = "wasm32"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    revert("panic")
}

/// Call context (`env_read`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Env {
    pub chain_id: u32,
    pub height: u64,
    /// This contract.
    pub address: Address,
    /// Immediate caller (the tx sender, or the calling contract).
    pub caller: Address,
    /// The tx sender.
    pub origin: Address,
    /// CADRE (base units, 9 decimals) sent with this call; already in this contract's balance.
    pub value: u64,
    pub miner: Address,
}

fn arr32(b: &[u8]) -> [u8; 32] {
    let mut a = [0u8; 32];
    a.copy_from_slice(&b[..32]);
    a
}

pub fn env() -> Env {
    let mut b = [0u8; 148];
    unsafe { sys::env_read(b.as_mut_ptr()) };
    Env {
        chain_id: u32::from_le_bytes(b[0..4].try_into().unwrap()),
        height: u64::from_le_bytes(b[4..12].try_into().unwrap()),
        address: arr32(&b[12..44]),
        caller: arr32(&b[44..76]),
        origin: arr32(&b[76..108]),
        value: u64::from_le_bytes(b[108..116].try_into().unwrap()),
        miner: arr32(&b[116..148]),
    }
}

/// The call's raw input.
pub fn input() -> Vec<u8> {
    let n = unsafe { sys::input_len() } as usize;
    let mut v = vec![0u8; n];
    if n > 0 {
        unsafe { sys::input_read(v.as_mut_ptr()) };
    }
    v
}

fn scratch() -> Vec<u8> {
    let n = unsafe { sys::scratch_len() } as usize;
    let mut v = vec![0u8; n];
    if n > 0 {
        unsafe { sys::scratch_read(v.as_mut_ptr()) };
    }
    v
}

/// Set the call's return data (at most 4096 bytes).
pub fn ret(data: &[u8]) {
    unsafe { sys::set_return(data.as_ptr(), data.len() as i32) }
}

/// Stop the call, undo its state changes, and fail with `msg` (shown in the receipt).
pub fn revert(msg: &str) -> ! {
    unsafe { sys::revert(msg.as_ptr(), msg.len() as i32) }
}

/// Revert unless `cond`.
pub fn require(cond: bool, msg: &str) {
    if !cond {
        revert(msg)
    }
}

/// Read the input as `method (u8 len) | fields`, run `f`, then return its output or revert
/// with its error. Use it as the whole body of an entry point.
pub fn dispatch<F>(f: F)
where
    F: FnOnce(&str, &mut abi::Reader) -> Result<Vec<u8>, &'static str>,
{
    let data = input();
    let mut r = abi::Reader::new(&data);
    let method = match r.method() {
        Ok(m) => m,
        Err(e) => revert(e),
    };
    match f(method, &mut r) {
        Ok(out) => ret(&out),
        Err(e) => revert(e),
    }
}

/// Like [`dispatch`] for `instantiate` and `migrate`, whose input is the fields alone (no
/// method name).
pub fn init<F>(f: F)
where
    F: FnOnce(&mut abi::Reader) -> Result<Vec<u8>, &'static str>,
{
    let data = input();
    let mut r = abi::Reader::new(&data);
    match f(&mut r) {
        Ok(out) => ret(&out),
        Err(e) => revert(e),
    }
}

/// Contract storage (keys up to 128 bytes, values up to 64 KiB). Growth is paid for by the
/// tx sender as a refundable deposit (storage_byte_deposit per byte).
pub mod storage {
    use super::*;

    pub fn get(key: &[u8]) -> Option<Vec<u8>> {
        let n = unsafe { sys::storage_read(key.as_ptr(), key.len() as i32) };
        if n < 0 {
            return None;
        }
        Some(scratch())
    }
    pub fn set(key: &[u8], value: &[u8]) {
        unsafe { sys::storage_write(key.as_ptr(), key.len() as i32, value.as_ptr(), value.len() as i32) }
    }
    pub fn remove(key: &[u8]) {
        unsafe { sys::storage_remove(key.as_ptr(), key.len() as i32) }
    }
    pub fn get_u64(key: &[u8]) -> Option<u64> {
        get(key).and_then(|v| v.as_slice().try_into().ok()).map(u64::from_le_bytes)
    }
    pub fn set_u64(key: &[u8], v: u64) {
        set(key, &v.to_le_bytes())
    }
    pub fn get_addr(key: &[u8]) -> Option<Address> {
        get(key).filter(|v| v.len() == 32).map(|v| arr32(&v))
    }
    /// `prefix || part` as a key (e.g. `key(b"bal:", &addr)`).
    pub fn key(prefix: &[u8], part: &[u8]) -> Vec<u8> {
        let mut k = Vec::with_capacity(prefix.len() + part.len());
        k.extend_from_slice(prefix);
        k.extend_from_slice(part);
        k
    }
}

/// `blake3(data)`.
pub fn blake3(data: &[u8]) -> Hash {
    let mut out = [0u8; 32];
    unsafe { sys::blake3(data.as_ptr(), data.len() as i32, out.as_mut_ptr()) };
    out
}

pub fn sha256(data: &[u8]) -> Hash {
    let mut out = [0u8; 32];
    unsafe { sys::sha256(data.as_ptr(), data.len() as i32, out.as_mut_ptr()) };
    out
}

/// Raw ed25519 check (the same keys as Cadre and Solana accounts).
pub fn ed25519_verify(pubkey: &Address, msg: &[u8], sig: &[u8; 64]) -> bool {
    unsafe { sys::ed25519_verify(pubkey.as_ptr(), msg.as_ptr(), msg.len() as i32, sig.as_ptr()) == 1 }
}

/// Topic of a named event: `blake3("pearl-sol:event:v1" || name)`.
pub fn event_topic(name: &str) -> Hash {
    let mut b = Vec::with_capacity(18 + name.len());
    b.extend_from_slice(b"pearl-sol:event:v1");
    b.extend_from_slice(name.as_bytes());
    blake3(&b)
}

/// Emit event `name` (topic 0) with up to 3 more topics and `data` (at most 4096 bytes).
/// Events are indexed per contract, shown by explorers and provable on Solana.
pub fn emit(name: &str, topics: &[Hash], data: &[u8]) {
    let mut t = Vec::with_capacity(32 * (1 + topics.len()));
    t.extend_from_slice(&event_topic(name));
    for x in topics.iter().take(3) {
        t.extend_from_slice(x);
    }
    unsafe { sys::emit(t.as_ptr(), (t.len() / 32) as i32, data.as_ptr(), data.len() as i32) }
}

/// CADRE balance (base units) of any account or contract.
pub fn balance(of: &Address) -> u64 {
    unsafe { sys::balance(of.as_ptr()) as u64 }
}

/// Send CADRE from this contract's balance.
pub fn transfer(to: &Address, amount: u64) -> Result<(), &'static str> {
    match unsafe { sys::transfer(to.as_ptr(), amount as i64) } {
        0 => Ok(()),
        1 => Err("insufficient balance"),
        _ => Err("cannot transfer to a reserved account"),
    }
}

/// Call `contract`'s `call` entry with `input` (see [`abi::Writer::method`]), sending
/// `value` from this contract, with at most `gas` (0 = all remaining). On failure the
/// callee's changes are undone and the error text comes back.
pub fn call(contract: &Address, value: u64, input: &[u8], gas: u64) -> Result<Vec<u8>, Vec<u8>> {
    let s = unsafe { sys::call(contract.as_ptr(), value as i64, input.as_ptr(), input.len() as i32, gas as i64) };
    let data = scratch();
    if s == 0 {
        Ok(data)
    } else {
        Err(data)
    }
}

pub fn gas_left() -> u64 {
    unsafe { sys::gas_left() as u64 }
}

/// Debug line, kept only when the call is simulated (`simulateCall` / `pearl contract call --dry-run`).
pub fn log(msg: &str) {
    unsafe { sys::log(msg.as_ptr(), msg.len() as i32) }
}

/// Bytes to `String` without panicking on bad UTF-8.
pub fn utf8(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
