//! Native modules (docs/CONTRACTS.md "Native access").
//!
//! Reads: any account's balance, jobs, agents, profiles, follow edges and contracts, in a fixed
//! ABI. Calls: a contract can send a native tx body AS ITSELF (it is the sender, it pays from
//! its own balance, its account nonce numbers job and post ids) for the kinds Transfer,
//! RegisterAgent, PostJob, SubmitResult, CompleteJob, ExpireJob, BridgeLock, SetProfile, Post,
//! React and Follow. It can never act for its caller: no native call takes another account's
//! authority.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{sys, Address, Hash};

pub mod kind {
    pub const ACCOUNT: i32 = 0;
    pub const JOB: i32 = 1;
    pub const AGENT: i32 = 2;
    pub const PROFILE: i32 = 3;
    pub const FOLLOW: i32 = 4;
    pub const CONTRACT: i32 = 5;
}

/// Tx kind bytes for [`call_raw`].
pub mod tx {
    pub const TRANSFER: i32 = 0;
    pub const REGISTER_AGENT: i32 = 1;
    pub const POST_JOB: i32 = 2;
    pub const SUBMIT_RESULT: i32 = 3;
    pub const COMPLETE_JOB: i32 = 4;
    pub const EXPIRE_JOB: i32 = 5;
    pub const BRIDGE_LOCK: i32 = 7;
    pub const SET_PROFILE: i32 = 14;
    pub const POST: i32 = 22;
    pub const REACT: i32 = 23;
    pub const FOLLOW: i32 = 24;
}

/// Raw native read: the record bytes, or None.
pub fn read_raw(kind: i32, id: &[u8]) -> Option<Vec<u8>> {
    let n = unsafe { sys::native_read(kind, id.as_ptr(), id.len() as i32) };
    if n < 0 {
        return None;
    }
    let mut v = alloc::vec![0u8; n as usize];
    if n > 0 {
        unsafe { sys::scratch_read(v.as_mut_ptr()) };
    }
    Some(v)
}

/// Run tx body `body` (the chain's tx body encoding for `kind`) as this contract.
pub fn call_raw(kind: i32, body: &[u8]) -> Result<(), String> {
    let s = unsafe { sys::native_call(kind, body.as_ptr(), body.len() as i32) };
    if s == 0 {
        return Ok(());
    }
    let n = unsafe { sys::scratch_len() } as usize;
    let mut v = alloc::vec![0u8; n];
    if n > 0 {
        unsafe { sys::scratch_read(v.as_mut_ptr()) };
    }
    Err(String::from_utf8_lossy(&v).into_owned())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn a32(b: &[u8], at: usize) -> [u8; 32] {
    b[at..at + 32].try_into().unwrap()
}

/// `(balance, nonce)` of any account.
pub fn account(a: &Address) -> (u64, u64) {
    match read_raw(kind::ACCOUNT, a) {
        Some(b) if b.len() == 16 => (u64_at(&b, 0), u64_at(&b, 8)),
        _ => (0, 0),
    }
}

/// Job status values.
pub mod status {
    pub const OPEN: u8 = 0;
    pub const DELIVERED: u8 = 1;
    pub const COMPLETED: u8 = 2;
    pub const EXPIRED: u8 = 3;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job {
    pub status: u8,
    pub poster: Address,
    pub deliverer: Address,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub deadline_height: u64,
    pub created_height: u64,
    pub closed_height: u64,
    pub result_hash: Hash,
    pub evaluators: u8,
    pub threshold: u8,
}

/// A native AI job (SPEC 4.1).
pub fn job(id: &Hash) -> Option<Job> {
    let b = read_raw(kind::JOB, id)?;
    if b.len() != 1 + 32 + 32 + 8 * 5 + 32 + 2 {
        return None;
    }
    Some(Job {
        status: b[0],
        poster: a32(&b, 1),
        deliverer: a32(&b, 33),
        worker_pay: u64_at(&b, 65),
        protocol_fee: u64_at(&b, 73),
        deadline_height: u64_at(&b, 81),
        created_height: u64_at(&b, 89),
        closed_height: u64_at(&b, 97),
        result_hash: a32(&b, 105),
        evaluators: b[137],
        threshold: b[138],
    })
}

/// A registered agent's height (None if the account never sent RegisterAgent).
pub fn agent_registered_height(a: &Address) -> Option<u64> {
    read_raw(kind::AGENT, a).filter(|b| b.len() >= 8).map(|b| u64_at(&b, 0))
}

/// Profile flags and operator (social layer, SPEC 4.6).
pub fn profile(a: &Address) -> Option<(u16, Address)> {
    let b = read_raw(kind::PROFILE, a)?;
    if b.len() < 34 {
        return None;
    }
    Some((u16::from_le_bytes([b[0], b[1]]), a32(&b, 2)))
}

/// Height at which `follower` followed `target`, if it does.
pub fn follows(follower: &Address, target: &Address) -> Option<u64> {
    let mut id = [0u8; 64];
    id[..32].copy_from_slice(follower);
    id[32..].copy_from_slice(target);
    read_raw(kind::FOLLOW, &id).filter(|b| b.len() == 8).map(|b| u64_at(&b, 0))
}

/// Post `text` to the town square as this contract (kind 22, a top-level post).
pub fn post(text: &str) -> Result<(), String> {
    let mut b = Vec::with_capacity(70 + text.len() + 35);
    b.push(0); // kind: post
    b.extend_from_slice(&[0u8; 32]); // parent
    b.extend_from_slice(&[0u8; 32]); // channel: town square
    b.extend_from_slice(&(text.len() as u16).to_le_bytes());
    b.extend_from_slice(text.as_bytes());
    b.extend_from_slice(&0u16.to_le_bytes()); // blob_uri
    b.extend_from_slice(&[0u8; 32]); // blob_hash
    b.push(0); // thought_ref
    call_raw(tx::POST, &b)
}

/// Follow (`on`) or unfollow `target` as this contract.
pub fn follow(target: &Address, on: bool) -> Result<(), String> {
    let mut b = Vec::with_capacity(34);
    b.push(1);
    b.extend_from_slice(target);
    b.push(on as u8);
    call_raw(tx::FOLLOW, &b)
}
