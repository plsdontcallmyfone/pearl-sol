//! Cadre tool receipts (docs/TOOLBELT.md 5): hosted agents commit the Merkle root of each batch of
//! their off-chain tool receipts here, signed by the agent's own key, so the receipts are on the
//! Cadre chain at one small tx per batch instead of one per call. Deploy with admin zero (immutable).
//!
//! instantiate()
//! call commit(root hash, first_seq u64, n u32, price_lamports u64, blob hash)
//!      the caller is the agent; first_seq must equal the caller's next_seq (no gaps, no replays);
//!      n is 1..256; takes no value. Emits `ToolBatch` (topic 1 = the agent, topic 2 = the root;
//!      data first_seq u64, n u32, price_lamports u64, blob hash).
//! query get(agent addr) -> next_seq u64, batches u64, price_total u64, last_root hash,
//!      last_blob hash, last_height u64
//!
//! What a commitment proves, said plainly: that this agent's key committed this root (and so these
//! receipts, which anyone can recompute from the blob) at this height, and what the agent says it
//! paid. It does not prove what any web page said or that the work is good.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Writer, env, storage, Address};

const MAX_N: u32 = 256;

struct Rec {
    next_seq: u64,
    batches: u64,
    price_total: u64,
    last_root: [u8; 32],
    last_blob: [u8; 32],
    last_height: u64,
}

fn key(a: &Address) -> Vec<u8> {
    let mut k = Vec::with_capacity(33);
    k.push(b'a');
    k.extend_from_slice(a);
    k
}

fn load(a: &Address) -> Rec {
    let b = storage::get(&key(a)).unwrap_or_default();
    let mut r = cadre_contract_sdk::abi::Reader::new(&b);
    Rec {
        next_seq: r.u64().unwrap_or(0),
        batches: r.u64().unwrap_or(0),
        price_total: r.u64().unwrap_or(0),
        last_root: r.hash().unwrap_or([0; 32]),
        last_blob: r.hash().unwrap_or([0; 32]),
        last_height: r.u64().unwrap_or(0),
    }
}

fn write(x: &Rec) -> Vec<u8> {
    Writer::new().u64(x.next_seq).u64(x.batches).u64(x.price_total).hash(&x.last_root).hash(&x.last_blob).u64(x.last_height).finish()
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|_args| Ok(Vec::new()))
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        if e.value != 0 {
            return Err("this contract takes no value");
        }
        match method {
            "commit" => {
                let root = args.hash()?;
                let first = args.u64()?;
                let n = args.u32()?;
                let price = args.u64()?;
                let blob = args.hash()?;
                args.end()?;
                if n == 0 || n > MAX_N {
                    return Err("n is 1..256");
                }
                let mut x = load(&e.caller);
                if first != x.next_seq {
                    return Err("first_seq must equal the agent's next_seq");
                }
                x.next_seq = first.checked_add(n as u64).ok_or("seq overflow")?;
                x.batches += 1;
                x.price_total = x.price_total.saturating_add(price);
                x.last_root = root;
                x.last_blob = blob;
                x.last_height = e.height;
                storage::set(&key(&e.caller), &write(&x));
                sdk::emit("ToolBatch", &[e.caller, root], &Writer::new().u64(first).u32(n).u64(price).hash(&blob).finish());
                Ok(Writer::new().u64(x.next_seq).finish())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "get" => {
            let a = args.addr()?;
            Ok(write(&load(&a)))
        }
        _ => Err("unknown method"),
    })
}
