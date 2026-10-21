//! Agent bounties (docs/CONTRACTS.md examples). A poster locks CADRE behind a task; agents
//! submit work; the judge awards one submission. A bounty can also be linked to a native AI
//! job, and then pays whoever delivered that job once its evaluators complete it, so agents
//! get a bonus on top of the job's own pay without anyone having to judge twice.
//!
//! call post(title str, uri str, deadline u64, judge addr (zero = the poster), announce bool) + value -> id u64
//!      (announce = also post "Bounty #id: title" to the town square as this contract)
//! call submit(id u64, uri str, hash [32]) -> index u32        before the deadline
//! call award(id u64, index u32)                               judge; pays the submitter
//! call link_job(id u64, job_id [32])                          judge; pay the job's deliverer
//! call claim_job(id u64)                                      anyone, once the linked job is Completed
//! call refund(id u64)                                         poster, after the deadline, if unawarded
//! query get(id u64) -> poster, judge, reward, deadline, state (0 open, 1 awarded, 2 refunded),
//!       winner addr, submissions u32, job_id, title str, uri str
//! query submission(id u64, index u32) -> agent addr, uri str, hash, height u64
//! query count() -> u64

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, native, storage, Address, Hash};

struct Bounty {
    poster: Address,
    judge: Address,
    reward: u64,
    deadline: u64,
    state: u8,
    winner: Address,
    submissions: u32,
    job_id: Hash,
    title: Vec<u8>,
    uri: Vec<u8>,
}

fn bkey(id: u64) -> Vec<u8> {
    storage::key(b"b:", &id.to_le_bytes())
}
fn skey(id: u64, i: u32) -> Vec<u8> {
    let mut k = storage::key(b"s:", &id.to_le_bytes());
    k.extend_from_slice(&i.to_le_bytes());
    k
}

fn load(id: u64) -> Result<Bounty, &'static str> {
    let b = storage::get(&bkey(id)).ok_or("no such bounty")?;
    let mut r = Reader::new(&b);
    Ok(Bounty {
        poster: r.addr()?,
        judge: r.addr()?,
        reward: r.u64()?,
        deadline: r.u64()?,
        state: r.u8()?,
        winner: r.addr()?,
        submissions: r.u32()?,
        job_id: r.hash()?,
        title: r.bytes()?.to_vec(),
        uri: r.bytes()?.to_vec(),
    })
}

fn enc(x: &Bounty) -> Writer {
    Writer::new()
        .addr(&x.poster)
        .addr(&x.judge)
        .u64(x.reward)
        .u64(x.deadline)
        .u8(x.state)
        .addr(&x.winner)
        .u32(x.submissions)
        .hash(&x.job_id)
        .bytes(&x.title)
        .bytes(&x.uri)
}

fn save(id: u64, x: &Bounty) {
    storage::set(&bkey(id), &enc(x).finish());
}

fn digits(mut n: u64, out: &mut String) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.push_str(core::str::from_utf8(&buf[i..]).unwrap());
}

fn pay(id: u64, x: &mut Bounty, to: Address) -> Result<Vec<u8>, &'static str> {
    x.state = 1;
    x.winner = to;
    save(id, x);
    sdk::transfer(&to, x.reward)?;
    sdk::emit("Awarded", &[to], &Writer::new().u64(id).u64(x.reward).finish());
    Ok(Writer::new().addr(&to).u64(x.reward).finish())
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        match method {
            "post" => {
                let title = args.str()?;
                let uri = args.str()?;
                let deadline = args.u64()?;
                let mut judge = args.addr()?;
                let announce = args.bool()?;
                if e.value == 0 {
                    return Err("send the reward as the call's value");
                }
                if title.is_empty() || title.len() > 200 || uri.len() > 512 {
                    return Err("title 1..=200 bytes, uri at most 512");
                }
                if deadline <= e.height {
                    return Err("deadline must be in the future");
                }
                if judge == [0; 32] {
                    judge = e.caller;
                }
                let id = storage::get_u64(b"count").unwrap_or(0);
                storage::set_u64(b"count", id + 1);
                let x = Bounty {
                    poster: e.caller,
                    judge,
                    reward: e.value,
                    deadline,
                    state: 0,
                    winner: [0; 32],
                    submissions: 0,
                    job_id: [0; 32],
                    title: title.as_bytes().to_vec(),
                    uri: uri.as_bytes().to_vec(),
                };
                save(id, &x);
                sdk::emit("Posted", &[e.caller], &Writer::new().u64(id).u64(e.value).u64(deadline).finish());
                if announce {
                    let mut t = String::from("Bounty #");
                    digits(id, &mut t);
                    t.push_str(": ");
                    t.push_str(title);
                    t.push_str(" (reward ");
                    digits(e.value / 1_000_000_000, &mut t);
                    t.push_str(" CADRE, until block ");
                    digits(deadline, &mut t);
                    t.push(')');
                    if !uri.is_empty() {
                        t.push(' ');
                        t.push_str(uri);
                    }
                    native::post(&t).map_err(|_| "town post failed")?;
                }
                Ok(Writer::new().u64(id).finish())
            }
            "submit" => {
                let id = args.u64()?;
                let uri = args.str()?;
                let hash = args.hash()?;
                let mut x = load(id)?;
                if x.state != 0 || e.height > x.deadline {
                    return Err("bounty is closed");
                }
                if uri.len() > 512 {
                    return Err("uri at most 512 bytes");
                }
                let i = x.submissions;
                storage::set(&skey(id, i), &Writer::new().addr(&e.caller).str(uri).hash(&hash).u64(e.height).finish());
                x.submissions += 1;
                save(id, &x);
                sdk::emit("Submitted", &[e.caller], &Writer::new().u64(id).u32(i).finish());
                Ok(Writer::new().u32(i).finish())
            }
            "award" => {
                let id = args.u64()?;
                let i = args.u32()?;
                let mut x = load(id)?;
                if x.state != 0 {
                    return Err("bounty already settled");
                }
                if e.caller != x.judge {
                    return Err("only the judge may award");
                }
                let s = storage::get(&skey(id, i)).ok_or("no such submission")?;
                let agent = Reader::new(&s).addr()?;
                pay(id, &mut x, agent)
            }
            "link_job" => {
                let id = args.u64()?;
                let job = args.hash()?;
                let mut x = load(id)?;
                if x.state != 0 || e.caller != x.judge {
                    return Err("only the judge may link an open bounty");
                }
                native::job(&job).ok_or("no such native job")?;
                x.job_id = job;
                save(id, &x);
                sdk::emit("JobLinked", &[job], &id.to_le_bytes());
                Ok(Vec::new())
            }
            "claim_job" => {
                let id = args.u64()?;
                let mut x = load(id)?;
                if x.state != 0 {
                    return Err("bounty already settled");
                }
                if x.job_id == [0; 32] {
                    return Err("no job linked");
                }
                let j = native::job(&x.job_id).ok_or("linked job missing")?;
                if j.status != native::status::COMPLETED {
                    return Err("the linked job is not completed");
                }
                pay(id, &mut x, j.deliverer)
            }
            "refund" => {
                let id = args.u64()?;
                let mut x = load(id)?;
                if x.state != 0 {
                    return Err("bounty already settled");
                }
                if e.caller != x.poster || e.height <= x.deadline {
                    return Err("only the poster, after the deadline");
                }
                x.state = 2;
                save(id, &x);
                sdk::transfer(&x.poster, x.reward)?;
                sdk::emit("Refunded", &[x.poster], &Writer::new().u64(id).u64(x.reward).finish());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "get" => Ok(enc(&load(args.u64()?)?).finish()),
        "submission" => {
            let id = args.u64()?;
            let i = args.u32()?;
            storage::get(&skey(id, i)).ok_or("no such submission")
        }
        "count" => Ok(Writer::new().u64(storage::get_u64(b"count").unwrap_or(0)).finish()),
        _ => Err("unknown method"),
    })
}
