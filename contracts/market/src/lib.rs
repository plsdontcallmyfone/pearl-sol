//! Cadre agent services market (docs/ECOSYSTEM.md Phase 3, docs/MARKET.md): offers, hires paid
//! into escrow, delivery, buyer-named evaluators (people, agents or an LLM judge) who complete or
//! reject, refunds on reject, decline or timeout, and a protocol fee with an admin-set floor
//! (decision D6: 2,000 bps by default, editable on chain by the market admin).
//!
//! instantiate(admin addr, fee_bps u16, fee_to addr)
//! call offer(title str, category str, spec_uri str, spec_hash hash, price u64, turnaround u64,
//!            policy u8, n u8, evaluators addr*n, active bool) -> id u64
//!      update_offer(id u64, <the same fields as offer>)                        provider only
//!      hire(offer u64, price u64, request_uri str, request_hash hash, deadline_blocks u64,
//!           judge_fee u64, n u8, evaluators addr*n, threshold u8) + value -> id u64
//!           value must be exactly price + fee + judge_fee, fee = ceil(price x fee_bps / 10000);
//!           `price` must equal the offer's current price (no surprise after a re-price)
//!      decline(id u64, reason str)                                            provider, while open
//!      deliver(id u64, result_hash hash, result_uri str, thought_ref str)     provider, by the deadline
//!      verdict(id u64, accept bool, reason_uri str, reason_hash hash, thought_ref str)   a named evaluator
//!      expire(id u64)                                                          anyone, after the deadline
//!      set_config(admin addr, fee_bps u16, fee_to addr, hires_paused bool)    admin only
//! query config(), offer(id u64), hire(id u64), quote(offer u64, judge_fee u64), counts()
//!
//! Money: the hire's whole value stays in this contract until the hire closes. Completed: price
//! to the provider, fee to fee_to, judge_fee split among the evaluators who voted to accept.
//! Rejected: price + fee back to the buyer, judge_fee split among the evaluators who voted to
//! reject (they did the judging). Declined or expired: everything back to the buyer. A hire is
//! accepted when `threshold` evaluators accept, and rejected as soon as so many reject that
//! `threshold` accepts can no longer be reached.
//!
//! Policy: 0 = the buyer names any evaluators (itself included); 1 = the buyer must pick them
//! from the offer's list (for example a provider that only accepts a named LLM judge). The
//! provider can never hire itself or judge its own hire. Linked accounts (same operator, funded
//! by the provider) are not visible here; the indexer excludes them from reputation.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, storage, Address, Hash};

const ZERO: Address = [0; 32];
const MAX_TITLE: usize = 96;
const MAX_CATEGORY: usize = 32;
const MAX_URI: usize = 256;
const MAX_REF: usize = 128;
const MAX_REASON: usize = 256;
const MAX_EVAL: usize = 4;
const MAX_FEE_BPS: u16 = 5_000;
const MAX_TURNAROUND: u64 = 100_800; // 70 days of 60 s blocks
const MAX_DEADLINE: u64 = 201_600; // 140 days

pub const OPEN: u8 = 0;
pub const DELIVERED: u8 = 1;
pub const COMPLETED: u8 = 2;
pub const REJECTED: u8 = 3;
pub const EXPIRED: u8 = 4;
pub const DECLINED: u8 = 5;

fn put_str(w: Writer, s: &[u8]) -> Writer {
    w.bytes(s)
}
fn take_str(r: &mut Reader, max: usize, what: &'static str) -> Result<Vec<u8>, &'static str> {
    let b = r.bytes()?;
    if b.len() > max {
        return Err(what);
    }
    if core::str::from_utf8(b).is_err() {
        return Err("text must be UTF-8");
    }
    Ok(b.to_vec())
}
fn read_evaluators(r: &mut Reader) -> Result<Vec<Address>, &'static str> {
    let n = r.u8()? as usize;
    if n > MAX_EVAL {
        return Err("at most 4 evaluators");
    }
    let mut v: Vec<Address> = Vec::with_capacity(n);
    for _ in 0..n {
        let a = r.addr()?;
        if a == ZERO {
            return Err("evaluator is the zero address");
        }
        if v.contains(&a) {
            return Err("evaluators must be distinct");
        }
        v.push(a);
    }
    Ok(v)
}
fn write_evaluators(mut w: Writer, v: &[Address]) -> Writer {
    w = w.u8(v.len() as u8);
    for a in v {
        w = w.addr(a);
    }
    w
}

struct Offer {
    provider: Address,
    title: Vec<u8>,
    category: Vec<u8>,
    spec_uri: Vec<u8>,
    spec_hash: Hash,
    price: u64,
    turnaround: u64,
    policy: u8,
    evaluators: Vec<Address>,
    active: bool,
    created: u64,
    updated: u64,
    hires: u64,
    completed: u64,
}

impl Offer {
    /// The editable fields, in call order (offer / update_offer).
    fn read_fields(r: &mut Reader, provider: Address, height: u64) -> Result<Self, &'static str> {
        let title = take_str(r, MAX_TITLE, "title over 96 bytes")?;
        let category = take_str(r, MAX_CATEGORY, "category over 32 bytes")?;
        let spec_uri = take_str(r, MAX_URI, "spec_uri over 256 bytes")?;
        let spec_hash = r.hash()?;
        let price = r.u64()?;
        let turnaround = r.u64()?;
        let policy = r.u8()?;
        let evaluators = read_evaluators(r)?;
        let active = r.bool()?;
        if title.is_empty() {
            return Err("an offer needs a title");
        }
        if price == 0 {
            return Err("price must be above 0");
        }
        if turnaround == 0 || turnaround > MAX_TURNAROUND {
            return Err("turnaround must be 1..=100800 blocks");
        }
        if policy > 1 {
            return Err("policy is 0 (buyer names evaluators) or 1 (from the offer's list)");
        }
        if policy == 1 && evaluators.is_empty() {
            return Err("policy 1 needs the offer's evaluator list");
        }
        if evaluators.contains(&provider) {
            return Err("the provider cannot judge its own work");
        }
        Ok(Offer { provider, title, category, spec_uri, spec_hash, price, turnaround, policy, evaluators, active, created: height, updated: height, hires: 0, completed: 0 })
    }
    fn write(&self) -> Vec<u8> {
        let w = Writer::new().addr(&self.provider);
        let w = put_str(w, &self.title);
        let w = put_str(w, &self.category);
        let w = put_str(w, &self.spec_uri);
        let w = w.hash(&self.spec_hash).u64(self.price).u64(self.turnaround).u8(self.policy);
        write_evaluators(w, &self.evaluators)
            .bool(self.active)
            .u64(self.created)
            .u64(self.updated)
            .u64(self.hires)
            .u64(self.completed)
            .finish()
    }
    fn read(b: &[u8]) -> Result<Self, &'static str> {
        let mut r = Reader::new(b);
        Ok(Offer {
            provider: r.addr()?,
            title: r.bytes()?.to_vec(),
            category: r.bytes()?.to_vec(),
            spec_uri: r.bytes()?.to_vec(),
            spec_hash: r.hash()?,
            price: r.u64()?,
            turnaround: r.u64()?,
            policy: r.u8()?,
            evaluators: read_evaluators(&mut r)?,
            active: r.bool()?,
            created: r.u64()?,
            updated: r.u64()?,
            hires: r.u64()?,
            completed: r.u64()?,
        })
    }
}

struct Hire {
    offer: u64,
    buyer: Address,
    provider: Address,
    price: u64,
    fee: u64,
    judge_fee: u64,
    created: u64,
    deadline: u64,
    request_uri: Vec<u8>,
    request_hash: Hash,
    evaluators: Vec<Address>,
    threshold: u8,
    status: u8,
    accepts: u8,
    rejects: u8,
    voted: u8, // bit i = evaluators[i] voted
    accepted_by: u8, // bit i = evaluators[i] voted accept
    result_hash: Hash,
    result_uri: Vec<u8>,
    delivered: u64,
    closed: u64,
}

impl Hire {
    fn write(&self) -> Vec<u8> {
        let w = Writer::new()
            .u64(self.offer)
            .addr(&self.buyer)
            .addr(&self.provider)
            .u64(self.price)
            .u64(self.fee)
            .u64(self.judge_fee)
            .u64(self.created)
            .u64(self.deadline);
        let w = put_str(w, &self.request_uri).hash(&self.request_hash);
        let w = write_evaluators(w, &self.evaluators)
            .u8(self.threshold)
            .u8(self.status)
            .u8(self.accepts)
            .u8(self.rejects)
            .u8(self.voted)
            .u8(self.accepted_by)
            .hash(&self.result_hash);
        put_str(w, &self.result_uri).u64(self.delivered).u64(self.closed).finish()
    }
    fn read(b: &[u8]) -> Result<Self, &'static str> {
        let mut r = Reader::new(b);
        Ok(Hire {
            offer: r.u64()?,
            buyer: r.addr()?,
            provider: r.addr()?,
            price: r.u64()?,
            fee: r.u64()?,
            judge_fee: r.u64()?,
            created: r.u64()?,
            deadline: r.u64()?,
            request_uri: r.bytes()?.to_vec(),
            request_hash: r.hash()?,
            evaluators: read_evaluators(&mut r)?,
            threshold: r.u8()?,
            status: r.u8()?,
            accepts: r.u8()?,
            rejects: r.u8()?,
            voted: r.u8()?,
            accepted_by: r.u8()?,
            result_hash: r.hash()?,
            result_uri: r.bytes()?.to_vec(),
            delivered: r.u64()?,
            closed: r.u64()?,
        })
    }
    fn total(&self) -> u64 {
        self.price + self.fee + self.judge_fee
    }
}

struct Cfg {
    admin: Address,
    fee_bps: u16,
    fee_to: Address,
    hires_paused: bool,
}
fn cfg() -> Cfg {
    let b = storage::get(b"cfg").unwrap_or_default();
    let mut r = Reader::new(&b);
    Cfg { admin: r.addr().unwrap_or(ZERO), fee_bps: r.u16().unwrap_or(0), fee_to: r.addr().unwrap_or(ZERO), hires_paused: r.bool().unwrap_or(false) }
}
fn save_cfg(c: &Cfg) -> Result<(), &'static str> {
    if c.fee_bps > MAX_FEE_BPS {
        return Err("fee above 5,000 bps");
    }
    if c.fee_to == ZERO {
        return Err("fee_to must be set");
    }
    if c.admin == ZERO {
        return Err("admin must be set");
    }
    storage::set(b"cfg", &Writer::new().addr(&c.admin).u16(c.fee_bps).addr(&c.fee_to).bool(c.hires_paused).finish());
    Ok(())
}

/// The protocol fee on a price: ceil(price x bps / 10000).
pub fn fee_of(price: u64, bps: u16) -> u64 {
    let v = (price as u128 * bps as u128).div_ceil(10_000);
    v as u64
}

fn okey(id: u64) -> Vec<u8> {
    storage::key(b"o:", &id.to_le_bytes())
}
fn hkey(id: u64) -> Vec<u8> {
    storage::key(b"h:", &id.to_le_bytes())
}
fn load_offer(id: u64) -> Result<Offer, &'static str> {
    Offer::read(&storage::get(&okey(id)).ok_or("no such offer")?)
}
fn load_hire(id: u64) -> Result<Hire, &'static str> {
    Hire::read(&storage::get(&hkey(id)).ok_or("no such hire")?)
}
fn idt(id: u64) -> Hash {
    let mut t = [0u8; 32];
    t[..8].copy_from_slice(&id.to_le_bytes());
    t
}

/// Split `amount` evenly over the evaluators whose bit is set in `mask`; the remainder goes to
/// the first of them. Nothing is sent when no bit is set.
fn split(amount: u64, evaluators: &[Address], mask: u8) -> Result<(), &'static str> {
    if amount == 0 {
        return Ok(());
    }
    let who: Vec<&Address> = evaluators.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, a)| a).collect();
    if who.is_empty() {
        return Err("no evaluator to pay");
    }
    let each = amount / who.len() as u64;
    let rest = amount - each * who.len() as u64;
    for (i, a) in who.iter().enumerate() {
        let v = each + if i == 0 { rest } else { 0 };
        if v > 0 {
            sdk::transfer(a, v)?;
        }
    }
    Ok(())
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let c = Cfg { admin: args.addr()?, fee_bps: args.u16()?, fee_to: args.addr()?, hires_paused: false };
        save_cfg(&c)?;
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, args| {
        let e = env();
        let me = e.caller;
        if method != "hire" && e.value != 0 {
            return Err("only hire takes value");
        }
        match method {
            "offer" => {
                let o = Offer::read_fields(args, me, e.height)?;
                let id = storage::get_u64(b"no").unwrap_or(0);
                storage::set_u64(b"no", id + 1);
                storage::set(&okey(id), &o.write());
                let d = put_str(Writer::new().u64(o.price).u64(o.turnaround).u8(o.policy), &o.title);
                sdk::emit("OfferListed", &[idt(id), me], &put_str(d, &o.category).bool(o.active).finish());
                Ok(Writer::new().u64(id).finish())
            }
            "update_offer" => {
                let id = args.u64()?;
                let old = load_offer(id)?;
                if old.provider != me {
                    return Err("only the offer's provider");
                }
                let mut o = Offer::read_fields(args, me, e.height)?;
                o.created = old.created;
                o.hires = old.hires;
                o.completed = old.completed;
                storage::set(&okey(id), &o.write());
                let d = put_str(Writer::new().u64(o.price).u64(o.turnaround).u8(o.policy), &o.title);
                sdk::emit("OfferUpdated", &[idt(id), me], &put_str(d, &o.category).bool(o.active).finish());
                Ok(Vec::new())
            }
            "hire" => {
                let c = cfg();
                if c.hires_paused {
                    return Err("new hires are paused by the market admin");
                }
                let oid = args.u64()?;
                let price = args.u64()?;
                let request_uri = take_str(args, MAX_URI, "request_uri over 256 bytes")?;
                let request_hash = args.hash()?;
                let deadline_blocks = args.u64()?;
                let judge_fee = args.u64()?;
                let evaluators = read_evaluators(args)?;
                let threshold = args.u8()?;
                let mut o = load_offer(oid)?;
                if !o.active {
                    return Err("the offer is not active");
                }
                if me == o.provider {
                    return Err("a provider cannot hire its own offer");
                }
                if price != o.price {
                    return Err("price does not match the offer's current price");
                }
                if deadline_blocks < o.turnaround || deadline_blocks > MAX_DEADLINE {
                    return Err("deadline_blocks must be at least the offer's turnaround and at most 201600");
                }
                if evaluators.is_empty() {
                    return Err("name at least one evaluator");
                }
                if threshold == 0 || threshold as usize > evaluators.len() {
                    return Err("threshold must be 1..=evaluators");
                }
                if evaluators.contains(&o.provider) {
                    return Err("the provider cannot judge its own work");
                }
                if o.policy == 1 && evaluators.iter().any(|a| !o.evaluators.contains(a)) {
                    return Err("this offer accepts only evaluators from its list");
                }
                let fee = fee_of(price, c.fee_bps);
                let total = price.checked_add(fee).and_then(|v| v.checked_add(judge_fee)).ok_or("amount overflow")?;
                if e.value != total {
                    return Err("value must be exactly price + fee + judge_fee (see quote)");
                }
                let id = storage::get_u64(b"nh").unwrap_or(0);
                storage::set_u64(b"nh", id + 1);
                let h = Hire {
                    offer: oid,
                    buyer: me,
                    provider: o.provider,
                    price,
                    fee,
                    judge_fee,
                    created: e.height,
                    deadline: e.height + deadline_blocks,
                    request_uri,
                    request_hash,
                    evaluators,
                    threshold,
                    status: OPEN,
                    accepts: 0,
                    rejects: 0,
                    voted: 0,
                    accepted_by: 0,
                    result_hash: ZERO,
                    result_uri: Vec::new(),
                    delivered: 0,
                    closed: 0,
                };
                storage::set(&hkey(id), &h.write());
                o.hires += 1;
                storage::set(&okey(oid), &o.write());
                let d = Writer::new().u64(oid).u64(price).u64(fee).u64(judge_fee).u64(h.deadline).u8(threshold);
                let d = write_evaluators(d, &h.evaluators).hash(&request_hash);
                sdk::emit("Hired", &[idt(id), me, o.provider], &put_str(d, &h.request_uri).finish());
                Ok(Writer::new().u64(id).finish())
            }
            "decline" => {
                let id = args.u64()?;
                let reason = take_str(args, MAX_REASON, "reason over 256 bytes")?;
                let mut h = load_hire(id)?;
                if me != h.provider {
                    return Err("only the hire's provider");
                }
                if h.status != OPEN {
                    return Err("only an open hire (not yet delivered) can be declined");
                }
                h.status = DECLINED;
                h.closed = e.height;
                storage::set(&hkey(id), &h.write());
                sdk::transfer(&h.buyer, h.total())?;
                sdk::emit("Declined", &[idt(id), me, h.buyer], &put_str(Writer::new().u64(h.total()), &reason).finish());
                Ok(Vec::new())
            }
            "deliver" => {
                let id = args.u64()?;
                let result_hash = args.hash()?;
                let result_uri = take_str(args, MAX_URI, "result_uri over 256 bytes")?;
                let thought_ref = take_str(args, MAX_REF, "thought_ref over 128 bytes")?;
                let mut h = load_hire(id)?;
                if me != h.provider {
                    return Err("only the hire's provider delivers");
                }
                if h.status != OPEN {
                    return Err("the hire is not open for delivery");
                }
                if e.height > h.deadline {
                    return Err("the hire's deadline passed");
                }
                if result_hash == ZERO {
                    return Err("result_hash is zero");
                }
                h.status = DELIVERED;
                h.result_hash = result_hash;
                h.result_uri = result_uri;
                h.delivered = e.height;
                storage::set(&hkey(id), &h.write());
                let d = put_str(Writer::new().hash(&result_hash), &h.result_uri);
                sdk::emit("Delivered", &[idt(id), me, h.buyer], &put_str(d, &thought_ref).finish());
                Ok(Vec::new())
            }
            "verdict" => {
                let id = args.u64()?;
                let accept = args.bool()?;
                let reason_uri = take_str(args, MAX_URI, "reason_uri over 256 bytes")?;
                let reason_hash = args.hash()?;
                let thought_ref = take_str(args, MAX_REF, "thought_ref over 128 bytes")?;
                let mut h = load_hire(id)?;
                let Some(i) = h.evaluators.iter().position(|a| *a == me) else { return Err("only a named evaluator of this hire") };
                if h.status != DELIVERED {
                    return Err("the hire has no delivery to judge");
                }
                if e.height > h.deadline {
                    return Err("the hire's deadline passed");
                }
                if h.voted & (1 << i) != 0 {
                    return Err("this evaluator already voted");
                }
                h.voted |= 1 << i;
                if accept {
                    h.accepts += 1;
                    h.accepted_by |= 1 << i;
                } else {
                    h.rejects += 1;
                }
                let n = h.evaluators.len() as u8;
                let d = put_str(Writer::new().bool(accept).hash(&reason_hash), &reason_uri);
                sdk::emit("Verdict", &[idt(id), me, h.provider], &put_str(d.hash(&h.result_hash), &thought_ref).finish());
                if h.accepts >= h.threshold {
                    h.status = COMPLETED;
                    h.closed = e.height;
                    storage::set(&hkey(id), &h.write());
                    let mut o = load_offer(h.offer)?;
                    o.completed += 1;
                    storage::set(&okey(h.offer), &o.write());
                    let c = cfg();
                    sdk::transfer(&h.provider, h.price)?;
                    if h.fee > 0 {
                        sdk::transfer(&c.fee_to, h.fee)?;
                    }
                    split(h.judge_fee, &h.evaluators, h.accepted_by)?;
                    let d = Writer::new().addr(&h.buyer).u64(h.price).u64(h.fee).u64(h.judge_fee).addr(&c.fee_to);
                    sdk::emit("Completed", &[idt(id), h.provider, h.buyer], &d.finish());
                } else if h.rejects > n - h.threshold {
                    h.status = REJECTED;
                    h.closed = e.height;
                    storage::set(&hkey(id), &h.write());
                    let refund = h.price + h.fee;
                    sdk::transfer(&h.buyer, refund)?;
                    let rejected_by = h.voted & !h.accepted_by;
                    split(h.judge_fee, &h.evaluators, rejected_by)?;
                    sdk::emit("Rejected", &[idt(id), h.provider, h.buyer], &Writer::new().u64(refund).u64(h.judge_fee).finish());
                } else {
                    storage::set(&hkey(id), &h.write());
                }
                Ok(Writer::new().u8(h.status).finish())
            }
            "expire" => {
                let id = args.u64()?;
                let mut h = load_hire(id)?;
                if h.status != OPEN && h.status != DELIVERED {
                    return Err("the hire is already closed");
                }
                if e.height <= h.deadline {
                    return Err("the hire's deadline has not passed");
                }
                let was_delivered = h.status == DELIVERED;
                h.status = EXPIRED;
                h.closed = e.height;
                storage::set(&hkey(id), &h.write());
                sdk::transfer(&h.buyer, h.total())?;
                sdk::emit("Expired", &[idt(id), h.provider, h.buyer], &Writer::new().bool(was_delivered).u64(h.total()).finish());
                Ok(Vec::new())
            }
            "set_config" => {
                let c = cfg();
                if me != c.admin {
                    return Err("only the market admin");
                }
                let n = Cfg { admin: args.addr()?, fee_bps: args.u16()?, fee_to: args.addr()?, hires_paused: args.bool()? };
                save_cfg(&n)?;
                sdk::emit("ConfigSet", &[n.admin], &Writer::new().u16(n.fee_bps).addr(&n.fee_to).bool(n.hires_paused).finish());
                Ok(Vec::new())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, args| match method {
        "config" => {
            let c = cfg();
            Ok(Writer::new().addr(&c.admin).u16(c.fee_bps).addr(&c.fee_to).bool(c.hires_paused).finish())
        }
        "counts" => Ok(Writer::new().u64(storage::get_u64(b"no").unwrap_or(0)).u64(storage::get_u64(b"nh").unwrap_or(0)).finish()),
        "offer" => Ok(storage::get(&okey(args.u64()?)).ok_or("no such offer")?),
        "hire" => Ok(storage::get(&hkey(args.u64()?)).ok_or("no such hire")?),
        "quote" => {
            let o = load_offer(args.u64()?)?;
            let judge_fee = args.u64()?;
            let fee = fee_of(o.price, cfg().fee_bps);
            Ok(Writer::new().u64(o.price).u64(fee).u64(judge_fee).u64(o.price + fee + judge_fee).finish())
        }
        _ => Err("unknown method"),
    })
}
