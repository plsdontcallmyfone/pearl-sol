//! Cadre swarms (docs/SWARMS.md): a public registry of groups of sovereign agents that share a
//! goal. A swarm is a grouping and a goal on the record, not a control layer: its creator is
//! attribution only and has no method here after `create` (no removal, no pause, no funds), and
//! nobody can make an agent a member without that agent's own signature. Deploy with admin zero
//! (immutable code). The contract keeps no CADRE: `create` and `fund` pass every unit straight to
//! the members' own accounts in the same call.
//!
//! instantiate(start_height u64)                 every call below is refused before start_height
//! call create(name str, goal_hash hash, goal_uri str, lead addr, salt u64, n u8,
//!             n x (agent addr, role str, amount u64, accept_sig [64])) -> id u64
//!          caller = the creator. Every member signed its acceptance of the definition
//!          (accept message below), the lead is one of them, the creator is not, and the call's
//!          value equals the sum of `amount`, each sent to that member at once.
//!      create_signed(the same fields, creator addr, creator_sig [64]) -> id u64
//!          anyone submits (for example the lead, once it holds CADRE); the creator signed the
//!          creator message (a short text, so a Solana wallet's signMessage shows it).
//!      propose(swarm u64, agent addr, role str) -> proposal u64      the lead invites an agent
//!      cancel(proposal u64)                                          the lead withdraws it
//!      accept(proposal u64)                                          the invited agent joins
//!      accept_signed(proposal u64, sig [64])                         anyone submits its join signature
//!      decline(proposal u64)                                         the invited agent says no
//!      leave(swarm u64)                                              any member leaves; a leaving lead leaves no lead
//!      hand_lead(swarm u64, to addr)                                 the lead names a successor (zero clears)
//!      take_lead(swarm u64)                                          the named successor takes the lead
//!      fund(swarm u64, n u8, n x (agent addr, amount u64))           anyone pays members directly (value = sum)
//! query counts() -> swarms u64, proposals u64, start_height u64
//!       swarm(id) -> creator, lead, successor, definition, goal_hash, height u64, slots u32,
//!                    active u32, proposals u32, name bytes, goal_uri bytes
//!       members(id, offset u32, limit u8) -> total u32, n u8, n x (agent, role bytes, joined u64, left u64, via u64)
//!       member(id, agent) -> found u8 [, active u8, role bytes, joined u64, left u64]
//!       by_member(agent, offset u32, limit u8) / by_creator(creator, offset u32, limit u8) -> total u32, n u8, n x id u64
//!       proposal(id) -> swarm u64, agent, proposer, state u8, height u64, closed u64, role bytes
//!       proposals(swarm, offset u32, limit u8) -> total u32, n u8, n x id u64
//!       by_definition(hash) -> id u64 (0 = none)
//!       definition(creator, name, goal_hash, goal_uri, lead, salt, n, n x (agent, role)) -> hash, creator message bytes
//!
//! Signed messages (ed25519 by the same keys as Cadre and Solana accounts):
//!   definition = blake3("cadre-swarm-def-v1" || this contract || creator || len32 name || name ||
//!                goal_hash || len32 goal_uri || goal_uri || lead || n u8 || n x (agent || len32 role || role) || salt u64)
//!   accept     = "cadre-swarm-accept-v1" || this contract || definition            (each member)
//!   join       = "cadre-swarm-join-v1" || this contract || proposal u64            (an invited agent)
//!   creator    = "Cadre swarm v1\ncontract: <hex>\ndefinition: <hex>\n" + CREATOR_NOTE  (UTF-8 text)
//! (len32 = u32 little endian length; integers little endian.)

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, storage, Address, Hash};

/// Members named at creation, and active members at any time.
pub const MAX_MEMBERS: u32 = 32;
/// Member slots a swarm may ever use (joins after leaving take a new slot).
pub const MAX_SLOTS: u32 = 64;
const MAX_NAME: usize = 64;
const MAX_ROLE: usize = 32;
const MAX_URI: usize = 160;
const MAX_PAGE: u8 = 32;
const ZERO: Address = [0u8; 32];
pub const CREATOR_NOTE: &str = "This creates a public grouping of sovereign agents. It gives you no control over them and moves no funds.";

const P_OPEN: u8 = 0;
const P_ACCEPTED: u8 = 1;
const P_CANCELLED: u8 = 2;
const P_DECLINED: u8 = 3;

struct Swarm {
    creator: Address,
    lead: Address,
    successor: Address,
    def: Hash,
    goal_hash: Hash,
    height: u64,
    slots: u32,
    active: u32,
    nprop: u32,
    name: Vec<u8>,
    goal_uri: Vec<u8>,
}

struct Slot {
    agent: Address,
    role: Vec<u8>,
    joined: u64,
    left: u64,
    via: u64,
}

struct Proposal {
    swarm: u64,
    agent: Address,
    proposer: Address,
    state: u8,
    height: u64,
    closed: u64,
    role: Vec<u8>,
}

fn k64(prefix: &[u8], id: u64) -> Vec<u8> {
    storage::key(prefix, &id.to_le_bytes())
}
fn k_slot(id: u64, i: u32) -> Vec<u8> {
    let mut k = k64(b"sl", id);
    k.extend_from_slice(&i.to_le_bytes());
    k
}
fn k_mi(id: u64, a: &Address) -> Vec<u8> {
    let mut k = k64(b"mi", id);
    k.extend_from_slice(a);
    k
}
fn k_po(id: u64, a: &Address) -> Vec<u8> {
    let mut k = k64(b"po", id);
    k.extend_from_slice(a);
    k
}
fn k_sp(id: u64, i: u32) -> Vec<u8> {
    let mut k = k64(b"sp", id);
    k.extend_from_slice(&i.to_le_bytes());
    k
}
fn k_list(prefix: &[u8], a: &Address, i: u32) -> Vec<u8> {
    let mut k = storage::key(prefix, a);
    k.extend_from_slice(&i.to_le_bytes());
    k
}
fn k_count(prefix: &[u8], a: &Address) -> Vec<u8> {
    storage::key(prefix, a)
}

fn ws(s: &Swarm) -> Vec<u8> {
    Writer::new()
        .addr(&s.creator)
        .addr(&s.lead)
        .addr(&s.successor)
        .hash(&s.def)
        .hash(&s.goal_hash)
        .u64(s.height)
        .u32(s.slots)
        .u32(s.active)
        .u32(s.nprop)
        .bytes(&s.name)
        .bytes(&s.goal_uri)
        .finish()
}
fn rs(b: &[u8]) -> Result<Swarm, &'static str> {
    let mut r = Reader::new(b);
    Ok(Swarm {
        creator: r.addr()?,
        lead: r.addr()?,
        successor: r.addr()?,
        def: r.hash()?,
        goal_hash: r.hash()?,
        height: r.u64()?,
        slots: r.u32()?,
        active: r.u32()?,
        nprop: r.u32()?,
        name: r.bytes()?.to_vec(),
        goal_uri: r.bytes()?.to_vec(),
    })
}
fn wslot(s: &Slot) -> Vec<u8> {
    Writer::new().addr(&s.agent).bytes(&s.role).u64(s.joined).u64(s.left).u64(s.via).finish()
}
fn rslot(b: &[u8]) -> Result<Slot, &'static str> {
    let mut r = Reader::new(b);
    Ok(Slot { agent: r.addr()?, role: r.bytes()?.to_vec(), joined: r.u64()?, left: r.u64()?, via: r.u64()? })
}
fn wp(p: &Proposal) -> Vec<u8> {
    Writer::new().u64(p.swarm).addr(&p.agent).addr(&p.proposer).u8(p.state).u64(p.height).u64(p.closed).bytes(&p.role).finish()
}
fn rp(b: &[u8]) -> Result<Proposal, &'static str> {
    let mut r = Reader::new(b);
    Ok(Proposal { swarm: r.u64()?, agent: r.addr()?, proposer: r.addr()?, state: r.u8()?, height: r.u64()?, closed: r.u64()?, role: r.bytes()?.to_vec() })
}

fn swarm(id: u64) -> Result<Swarm, &'static str> {
    storage::get(&k64(b"s", id)).ok_or("no such swarm").and_then(|b| rs(&b))
}
fn put_swarm(id: u64, s: &Swarm) {
    storage::set(&k64(b"s", id), &ws(s));
}
fn slot(id: u64, i: u32) -> Result<Slot, &'static str> {
    storage::get(&k_slot(id, i)).ok_or("no such member slot").and_then(|b| rslot(&b))
}
fn proposal(pid: u64) -> Result<Proposal, &'static str> {
    storage::get(&k64(b"p", pid)).ok_or("no such proposal").and_then(|b| rp(&b))
}
/// The member's latest slot index, when it is an active member.
fn active_slot(id: u64, a: &Address) -> Option<u32> {
    let i = storage::get(&k_mi(id, a)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
    let s = slot(id, i).ok()?;
    if s.left == 0 { Some(i) } else { None }
}

fn text(b: &[u8], max: usize, what: &'static str) -> Result<Vec<u8>, &'static str> {
    if b.len() > max {
        return Err(what);
    }
    core::str::from_utf8(b).map_err(|_| "text must be UTF-8")?;
    Ok(b.to_vec())
}

fn sig64(r: &mut Reader) -> Result<[u8; 64], &'static str> {
    let b = r.take(64)?;
    let mut s = [0u8; 64];
    s.copy_from_slice(b);
    Ok(s)
}

fn u64h(x: u64) -> Hash {
    let mut h = [0u8; 32];
    h[..8].copy_from_slice(&x.to_le_bytes());
    h
}

fn push_len(v: &mut Vec<u8>, b: &[u8]) {
    v.extend_from_slice(&(b.len() as u32).to_le_bytes());
    v.extend_from_slice(b);
}

/// The definition hash every member accepts (see the header).
pub fn definition(this: &Address, creator: &Address, name: &[u8], goal_hash: &Hash, goal_uri: &[u8], lead: &Address, members: &[(Address, Vec<u8>)], salt: u64) -> Hash {
    let mut v = Vec::with_capacity(256 + members.len() * 72);
    v.extend_from_slice(b"cadre-swarm-def-v1");
    v.extend_from_slice(this);
    v.extend_from_slice(creator);
    push_len(&mut v, name);
    v.extend_from_slice(goal_hash);
    push_len(&mut v, goal_uri);
    v.extend_from_slice(lead);
    v.push(members.len() as u8);
    for (a, role) in members {
        v.extend_from_slice(a);
        push_len(&mut v, role);
    }
    v.extend_from_slice(&salt.to_le_bytes());
    sdk::blake3(&v)
}

fn hex(out: &mut Vec<u8>, b: &[u8]) {
    const H: &[u8; 16] = b"0123456789abcdef";
    for x in b {
        out.push(H[(x >> 4) as usize]);
        out.push(H[(x & 15) as usize]);
    }
}

pub fn creator_message(this: &Address, def: &Hash) -> Vec<u8> {
    let mut m = Vec::with_capacity(256);
    m.extend_from_slice(b"Cadre swarm v1\ncontract: ");
    hex(&mut m, this);
    m.extend_from_slice(b"\ndefinition: ");
    hex(&mut m, def);
    m.push(b'\n');
    m.extend_from_slice(CREATOR_NOTE.as_bytes());
    m
}

fn accept_message(this: &Address, def: &Hash) -> Vec<u8> {
    let mut m = Vec::with_capacity(85);
    m.extend_from_slice(b"cadre-swarm-accept-v1");
    m.extend_from_slice(this);
    m.extend_from_slice(def);
    m
}

fn join_message(this: &Address, pid: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(59);
    m.extend_from_slice(b"cadre-swarm-join-v1");
    m.extend_from_slice(this);
    m.extend_from_slice(&pid.to_le_bytes());
    m
}

fn list_push(prefix_list: &[u8], prefix_count: &[u8], a: &Address, id: u64) {
    let n = storage::get(&k_count(prefix_count, a)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
    storage::set(&k_list(prefix_list, a, n), &id.to_le_bytes());
    storage::set(&k_count(prefix_count, a), &(n + 1).to_le_bytes());
}

/// Add `agent` as a member in a new slot (the caller checked it is not active).
fn add_member(id: u64, s: &mut Swarm, agent: &Address, role: Vec<u8>, height: u64, via: u64) -> Result<(), &'static str> {
    if s.active >= MAX_MEMBERS {
        return Err("the swarm already has 32 active members");
    }
    if s.slots >= MAX_SLOTS {
        return Err("the swarm has used all 64 member slots");
    }
    let first_time = storage::get(&k_mi(id, agent)).is_none();
    let i = s.slots;
    storage::set(&k_slot(id, i), &wslot(&Slot { agent: *agent, role, joined: height, left: 0, via }));
    storage::set(&k_mi(id, agent), &i.to_le_bytes());
    if first_time {
        list_push(b"bm", b"bn", agent, id);
    }
    s.slots += 1;
    s.active += 1;
    Ok(())
}

struct Def {
    name: Vec<u8>,
    goal_hash: Hash,
    goal_uri: Vec<u8>,
    lead: Address,
    salt: u64,
    members: Vec<(Address, Vec<u8>, u64, [u8; 64])>,
}

fn read_def(a: &mut Reader) -> Result<Def, &'static str> {
    let name = text(a.bytes()?, MAX_NAME, "name above 64 bytes")?;
    if name.is_empty() {
        return Err("a swarm needs a name");
    }
    let goal_hash = a.hash()?;
    let goal_uri = text(a.bytes()?, MAX_URI, "goal_uri above 160 bytes")?;
    let lead = a.addr()?;
    let salt = a.u64()?;
    let n = a.u8()? as usize;
    if n == 0 || n as u32 > MAX_MEMBERS {
        return Err("a swarm is created with 1 to 32 members");
    }
    let mut members = Vec::with_capacity(n);
    for _ in 0..n {
        let agent = a.addr()?;
        let role = text(a.bytes()?, MAX_ROLE, "role above 32 bytes")?;
        let amount = a.u64()?;
        let sig = sig64(a)?;
        members.push((agent, role, amount, sig));
    }
    Ok(Def { name, goal_hash, goal_uri, lead, salt, members })
}

fn create(creator: Address, d: Def, value: u64, height: u64, this: &Address) -> Result<u64, &'static str> {
    if creator == ZERO {
        return Err("creator is the zero key");
    }
    let mut sum: u64 = 0;
    for (i, (agent, _, amount, _)) in d.members.iter().enumerate() {
        if *agent == ZERO {
            return Err("a member is the zero key");
        }
        if *agent == creator {
            return Err("the creator is attribution, not a member: name agents only");
        }
        if d.members[..i].iter().any(|m| m.0 == *agent) {
            return Err("a member is named twice");
        }
        sum = sum.checked_add(*amount).ok_or("amounts overflow")?;
    }
    if !d.members.iter().any(|m| m.0 == d.lead) {
        return Err("the lead must be one of the members");
    }
    if sum != value {
        return Err("the call's value must equal the sum of the member amounts");
    }
    let pairs: Vec<(Address, Vec<u8>)> = d.members.iter().map(|m| (m.0, m.1.clone())).collect();
    let def = definition(this, &creator, &d.name, &d.goal_hash, &d.goal_uri, &d.lead, &pairs, d.salt);
    if storage::get(&storage::key(b"dh", &def)).is_some() {
        return Err("this definition is already registered");
    }
    let msg = accept_message(this, &def);
    for (agent, _, _, sig) in &d.members {
        if !sdk::ed25519_verify(agent, &msg, sig) {
            return Err("a member's acceptance signature does not verify: agents join only by their own signature");
        }
    }
    let id = storage::get_u64(b"ns").unwrap_or(0) + 1;
    let mut s = Swarm { creator, lead: d.lead, successor: ZERO, def, goal_hash: d.goal_hash, height, slots: 0, active: 0, nprop: 0, name: d.name, goal_uri: d.goal_uri };
    for (agent, role, amount, _) in d.members {
        add_member(id, &mut s, &agent, role, height, 0)?;
        if amount > 0 {
            sdk::transfer(&agent, amount)?;
        }
    }
    put_swarm(id, &s);
    storage::set(&storage::key(b"dh", &def), &id.to_le_bytes());
    storage::set_u64(b"ns", id);
    list_push(b"bc", b"bk", &creator, id);
    sdk::emit("SwarmCreated", &[creator, s.lead, u64h(id)], &Writer::new().u64(id).hash(&def).u32(s.active).u64(value).finish());
    Ok(id)
}

fn join(id: u64, pid: u64, mut p: Proposal, height: u64) -> Result<(), &'static str> {
    if p.state != P_OPEN {
        return Err("the proposal is not open");
    }
    let mut s = swarm(id)?;
    if active_slot(id, &p.agent).is_some() {
        return Err("already an active member");
    }
    add_member(id, &mut s, &p.agent, p.role.clone(), height, pid)?;
    p.state = P_ACCEPTED;
    p.closed = height;
    storage::set(&k64(b"p", pid), &wp(&p));
    storage::remove(&k_po(id, &p.agent));
    put_swarm(id, &s);
    sdk::emit("Joined", &[p.agent, u64h(id), u64h(pid)], &Writer::new().u64(id).u64(pid).bytes(&p.role).finish());
    Ok(())
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let start = args.u64()?;
        args.end()?;
        storage::set_u64(b"start", start);
        storage::set_u64(b"ns", 0);
        storage::set_u64(b"np", 0);
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, a| {
        let e = env();
        if e.height < storage::get_u64(b"start").unwrap_or(0) {
            return Err("not active before start_height");
        }
        let me = e.caller;
        let payable = matches!(method, "create" | "create_signed" | "fund");
        if !payable && e.value != 0 {
            return Err("this method takes no value");
        }
        match method {
            "create" => {
                let d = read_def(a)?;
                a.end()?;
                let id = create(me, d, e.value, e.height, &e.address)?;
                Ok(Writer::new().u64(id).finish())
            }
            "create_signed" => {
                let d = read_def(a)?;
                let creator = a.addr()?;
                let sig = sig64(a)?;
                a.end()?;
                let pairs: Vec<(Address, Vec<u8>)> = d.members.iter().map(|m| (m.0, m.1.clone())).collect();
                let def = definition(&e.address, &creator, &d.name, &d.goal_hash, &d.goal_uri, &d.lead, &pairs, d.salt);
                if !sdk::ed25519_verify(&creator, &creator_message(&e.address, &def), &sig) {
                    return Err("the creator's signature does not verify");
                }
                let id = create(creator, d, e.value, e.height, &e.address)?;
                Ok(Writer::new().u64(id).finish())
            }
            "propose" => {
                let id = a.u64()?;
                let agent = a.addr()?;
                let role = text(a.bytes()?, MAX_ROLE, "role above 32 bytes")?;
                a.end()?;
                let mut s = swarm(id)?;
                if s.lead == ZERO || s.lead != me {
                    return Err("only the swarm's lead proposes new members");
                }
                if agent == ZERO || agent == s.creator {
                    return Err("propose an agent (not the zero key, not the creator)");
                }
                if active_slot(id, &agent).is_some() {
                    return Err("already an active member");
                }
                if storage::get(&k_po(id, &agent)).is_some() {
                    return Err("this agent already has an open proposal");
                }
                let pid = storage::get_u64(b"np").unwrap_or(0) + 1;
                storage::set(&k64(b"p", pid), &wp(&Proposal { swarm: id, agent, proposer: me, state: P_OPEN, height: e.height, closed: 0, role: role.clone() }));
                storage::set_u64(b"np", pid);
                storage::set(&k_po(id, &agent), &pid.to_le_bytes());
                storage::set(&k_sp(id, s.nprop), &pid.to_le_bytes());
                s.nprop += 1;
                put_swarm(id, &s);
                sdk::emit("Proposed", &[agent, u64h(id), u64h(pid)], &Writer::new().u64(id).u64(pid).addr(&me).bytes(&role).finish());
                Ok(Writer::new().u64(pid).finish())
            }
            "cancel" | "decline" => {
                let pid = a.u64()?;
                a.end()?;
                let mut p = proposal(pid)?;
                if p.state != P_OPEN {
                    return Err("the proposal is not open");
                }
                if method == "cancel" {
                    if swarm(p.swarm)?.lead != me {
                        return Err("only the swarm's lead cancels a proposal");
                    }
                    p.state = P_CANCELLED;
                } else {
                    if p.agent != me {
                        return Err("only the invited agent declines");
                    }
                    p.state = P_DECLINED;
                }
                p.closed = e.height;
                storage::set(&k64(b"p", pid), &wp(&p));
                storage::remove(&k_po(p.swarm, &p.agent));
                sdk::emit("ProposalClosed", &[p.agent, u64h(p.swarm), u64h(pid)], &Writer::new().u64(pid).u8(p.state).finish());
                Ok(Vec::new())
            }
            "accept" => {
                let pid = a.u64()?;
                a.end()?;
                let p = proposal(pid)?;
                if p.agent != me {
                    return Err("only the invited agent accepts (or use accept_signed with its signature)");
                }
                join(p.swarm, pid, p, e.height)?;
                Ok(Vec::new())
            }
            "accept_signed" => {
                let pid = a.u64()?;
                let sig = sig64(a)?;
                a.end()?;
                let p = proposal(pid)?;
                if !sdk::ed25519_verify(&p.agent, &join_message(&e.address, pid), &sig) {
                    return Err("the invited agent's join signature does not verify");
                }
                join(p.swarm, pid, p, e.height)?;
                Ok(Vec::new())
            }
            "leave" => {
                let id = a.u64()?;
                a.end()?;
                let mut s = swarm(id)?;
                let i = active_slot(id, &me).ok_or("not an active member")?;
                let mut sl = slot(id, i)?;
                sl.left = e.height;
                storage::set(&k_slot(id, i), &wslot(&sl));
                s.active -= 1;
                if s.lead == me {
                    s.lead = ZERO;
                    s.successor = ZERO;
                }
                if s.successor == me {
                    s.successor = ZERO;
                }
                put_swarm(id, &s);
                sdk::emit("Left", &[me, u64h(id)], &Writer::new().u64(id).finish());
                Ok(Vec::new())
            }
            "hand_lead" => {
                let id = a.u64()?;
                let to = a.addr()?;
                a.end()?;
                let mut s = swarm(id)?;
                if s.lead == ZERO || s.lead != me {
                    return Err("only the swarm's lead names a successor");
                }
                if to != ZERO && (to == me || active_slot(id, &to).is_none()) {
                    return Err("the successor must be another active member (zero clears)");
                }
                s.successor = to;
                put_swarm(id, &s);
                sdk::emit("Successor", &[to, u64h(id)], &Writer::new().u64(id).addr(&to).finish());
                Ok(Vec::new())
            }
            "take_lead" => {
                let id = a.u64()?;
                a.end()?;
                let mut s = swarm(id)?;
                if s.successor == ZERO || s.successor != me {
                    return Err("only the named successor takes the lead");
                }
                if active_slot(id, &me).is_none() {
                    return Err("not an active member");
                }
                s.lead = me;
                s.successor = ZERO;
                put_swarm(id, &s);
                sdk::emit("LeadChanged", &[me, u64h(id)], &Writer::new().u64(id).addr(&me).finish());
                Ok(Vec::new())
            }
            "fund" => {
                let id = a.u64()?;
                swarm(id)?;
                let n = a.u8()? as usize;
                if n == 0 || n as u32 > MAX_MEMBERS {
                    return Err("fund 1 to 32 members");
                }
                let mut pays = Vec::with_capacity(n);
                let mut sum: u64 = 0;
                for _ in 0..n {
                    let agent = a.addr()?;
                    let amount = a.u64()?;
                    if active_slot(id, &agent).is_none() {
                        return Err("fund pays active members only");
                    }
                    if amount == 0 {
                        return Err("each amount is above 0");
                    }
                    sum = sum.checked_add(amount).ok_or("amounts overflow")?;
                    pays.push((agent, amount));
                }
                a.end()?;
                if sum != e.value {
                    return Err("the call's value must equal the sum of the amounts");
                }
                for (agent, amount) in &pays {
                    sdk::transfer(agent, *amount)?;
                }
                sdk::emit("Funded", &[me, u64h(id)], &Writer::new().u64(id).u8(n as u8).u64(sum).finish());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

fn page(prefix_list: &[u8], prefix_count: &[u8], who: &Address, offset: u32, limit: u8) -> Vec<u8> {
    let total = storage::get(&k_count(prefix_count, who)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
    let lim = limit.min(64) as u32;
    let mut ids = Vec::new();
    let mut i = offset;
    while i < total && (ids.len() as u32) < lim {
        if let Some(b) = storage::get(&k_list(prefix_list, who, i)) {
            ids.push(u64::from_le_bytes(b[..8].try_into().unwrap()));
        }
        i += 1;
    }
    let mut w = Writer::new().u32(total).u8(ids.len() as u8);
    for x in ids {
        w = w.u64(x);
    }
    w.finish()
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, a| match method {
        "counts" => Ok(Writer::new().u64(storage::get_u64(b"ns").unwrap_or(0)).u64(storage::get_u64(b"np").unwrap_or(0)).u64(storage::get_u64(b"start").unwrap_or(0)).finish()),
        "swarm" => {
            let id = a.u64()?;
            storage::get(&k64(b"s", id)).ok_or("no such swarm")
        }
        "members" => {
            let id = a.u64()?;
            let offset = a.u32()?;
            let limit = a.u8()?.min(MAX_PAGE) as u32;
            let s = swarm(id)?;
            let mut w = Writer::new().u32(s.slots);
            let end = s.slots.min(offset.saturating_add(limit));
            let n = end.saturating_sub(offset);
            w = w.u8(n as u8);
            for i in offset..end {
                let sl = slot(id, i)?;
                w = w.addr(&sl.agent).bytes(&sl.role).u64(sl.joined).u64(sl.left).u64(sl.via);
            }
            Ok(w.finish())
        }
        "member" => {
            let id = a.u64()?;
            let agent = a.addr()?;
            match storage::get(&k_mi(id, &agent)) {
                None => Ok(Writer::new().u8(0).finish()),
                Some(b) => {
                    let sl = slot(id, u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
                    Ok(Writer::new().u8(1).u8((sl.left == 0) as u8).bytes(&sl.role).u64(sl.joined).u64(sl.left).finish())
                }
            }
        }
        "by_member" | "by_creator" => {
            let who = a.addr()?;
            let offset = a.u32()?;
            let limit = a.u8()?;
            Ok(if method == "by_member" { page(b"bm", b"bn", &who, offset, limit) } else { page(b"bc", b"bk", &who, offset, limit) })
        }
        "proposal" => {
            let pid = a.u64()?;
            storage::get(&k64(b"p", pid)).ok_or("no such proposal")
        }
        "proposals" => {
            let id = a.u64()?;
            let offset = a.u32()?;
            let limit = a.u8()?.min(64) as u32;
            let s = swarm(id)?;
            let end = s.nprop.min(offset.saturating_add(limit));
            let mut w = Writer::new().u32(s.nprop).u8(end.saturating_sub(offset) as u8);
            for i in offset..end {
                let b = storage::get(&k_sp(id, i)).ok_or("missing proposal index")?;
                w = w.u64(u64::from_le_bytes(b[..8].try_into().unwrap()));
            }
            Ok(w.finish())
        }
        "by_definition" => {
            let h = a.hash()?;
            Ok(Writer::new().u64(storage::get(&storage::key(b"dh", &h)).map(|b| u64::from_le_bytes(b[..8].try_into().unwrap())).unwrap_or(0)).finish())
        }
        "definition" => {
            let creator = a.addr()?;
            let name = text(a.bytes()?, MAX_NAME, "name above 64 bytes")?;
            let goal_hash = a.hash()?;
            let goal_uri = text(a.bytes()?, MAX_URI, "goal_uri above 160 bytes")?;
            let lead = a.addr()?;
            let salt = a.u64()?;
            let n = a.u8()? as usize;
            let mut pairs = Vec::with_capacity(n);
            for _ in 0..n {
                let ag = a.addr()?;
                pairs.push((ag, text(a.bytes()?, MAX_ROLE, "role above 32 bytes")?));
            }
            let this = env().address;
            let def = definition(&this, &creator, &name, &goal_hash, &goal_uri, &lead, &pairs, salt);
            Ok(Writer::new().hash(&def).bytes(&creator_message(&this, &def)).finish())
        }
        _ => Err("unknown method"),
    })
}
