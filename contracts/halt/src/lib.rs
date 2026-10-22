//! Cadre protocol halt record (docs/ECOSYSTEM.md 7.8, SPEC 4.8): one signed, on-chain switch that
//! the hosted agent runtime (`cadre-agents`), the self-hosted runner (`cadre-agent`) and the house
//! traders read before they act. Deploy it with admin zero (immutable code): nobody can migrate it
//! into something else; only the halt authority stored here can change the record.
//!
//! instantiate(authority addr)
//! call set(scope u8, reason str)            authority only; scope is a bit mask, 0 = resume
//!          1 = ALL (every agent action), 2 = TRADE (no trades), 4 = LAUNCH (no token launches)
//!      set_authority(authority addr)        authority only; never zero
//! query status() -> scope u8, seq u64, height u64, authority addr, reason str
//!
//! Every change emits `Halt` (topic 1 = the authority; data scope u8, seq u64, reason str), so the
//! history is on chain. What it cannot do, said plainly: it cannot stop a process. A hosted runtime
//! enforces it; a self-hosted runner obeys it only as long as whoever runs it does not modify it.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, storage, Address};

const ZERO: Address = [0; 32];
const MAX_REASON: usize = 280;
const SCOPES: u8 = 1 | 2 | 4;

struct Rec {
    scope: u8,
    seq: u64,
    height: u64,
    authority: Address,
    reason: Vec<u8>,
}

fn load() -> Rec {
    let b = storage::get(b"rec").unwrap_or_default();
    let mut r = Reader::new(&b);
    Rec {
        scope: r.u8().unwrap_or(0),
        seq: r.u64().unwrap_or(0),
        height: r.u64().unwrap_or(0),
        authority: r.addr().unwrap_or(ZERO),
        reason: r.bytes().map(|x| x.to_vec()).unwrap_or_default(),
    }
}

fn save(x: &Rec) {
    storage::set(b"rec", &write(x));
}

fn write(x: &Rec) -> Vec<u8> {
    Writer::new().u8(x.scope).u64(x.seq).u64(x.height).addr(&x.authority).bytes(&x.reason).finish()
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let authority = args.addr()?;
        if authority == ZERO {
            return Err("authority must be set");
        }
        save(&Rec { scope: 0, seq: 0, height: env().height, authority, reason: Vec::new() });
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        if e.value != 0 {
            return Err("this contract takes no value");
        }
        let mut x = load();
        if e.caller != x.authority {
            return Err("only the halt authority");
        }
        match method {
            "set" => {
                let scope = args.u8()?;
                if scope & !SCOPES != 0 {
                    return Err("unknown scope bits (1 all, 2 trade, 4 launch)");
                }
                let reason = args.bytes()?;
                if reason.len() > MAX_REASON {
                    return Err("reason above 280 bytes");
                }
                if core::str::from_utf8(reason).is_err() {
                    return Err("reason must be UTF-8");
                }
                x.scope = scope;
                x.seq += 1;
                x.height = e.height;
                x.reason = reason.to_vec();
                save(&x);
                sdk::emit("Halt", &[x.authority], &Writer::new().u8(scope).u64(x.seq).bytes(&x.reason).finish());
                Ok(Writer::new().u64(x.seq).finish())
            }
            "set_authority" => {
                let a = args.addr()?;
                if a == ZERO {
                    return Err("authority must be set");
                }
                let old = x.authority;
                x.authority = a;
                x.seq += 1;
                x.height = e.height;
                save(&x);
                sdk::emit("HaltAuthority", &[old, a], &Writer::new().u64(x.seq).finish());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, _args| match method {
        "status" => Ok(write(&load())),
        _ => Err("unknown method"),
    })
}
