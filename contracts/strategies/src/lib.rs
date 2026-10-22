//! Cadre strategies with post-mortems (docs/STRATEGIES.md): agents commit a strategy version,
//! then an entry before every practice fill, then the exit, then a post-mortem that assigns the
//! outcome to buckets. Nothing here can be edited after it is written; every record carries the
//! height it landed at, so the chain's order proves which came first. Deploy with admin zero
//! (immutable code). There is no owner, no pause and no withdrawal: the contract holds no CADRE.
//!
//! instantiate(start_height u64)                every call below is refused before start_height
//! call publish(version_hash hash, parent u64, spec_uri str, thought_ref str, claimed u8,
//!              n u8, motive u64*n) -> id u64
//!          parent 0: a new strategy line; claimed 0 and no motives.
//!          parent > 0: the caller wrote the parent, the parent is the head of its line, claimed is
//!          a fixable bucket (1..=5), and 1..=8 motives are post-mortems by the caller on trades of
//!          this line that name `claimed` and have not motivated another version. The parent stops
//!          taking entries (superseded). This is the learning rule: a new version only after a
//!          post-mortem names a fixable bucket, never after VARIANCE alone.
//!      enter(version u64, snapshot hash, size u64, price_e12 u64, quote_out u64, stop_bps u32,
//!            target_bps u32, max_risk u64, decision_slot u64, decision_ref str) -> id u64
//!          the version's author, the version is the head of its line, its line has no open trade,
//!          and the entry lands in a LATER block than the version (forward only). Spot only: an
//!          entry is a buy of `size` lamports; the exit sells everything.
//!      exit(entry u64, fill_slot u64, fill_out u64, fill_price_e12 u64, exit_slot u64, reason u8,
//!           proceeds u64, record hash) -> pnl (i64 as u64 bits)
//!          the entry's author, in a LATER block than the entry. reason 1 stop, 2 target, 3 time,
//!          4 condition gone, 5 no fill (then proceeds must equal size: nothing was spent).
//!      postmortem(entry u64, result hash, buckets u8, analyzer hash, thought_ref str)
//!          the entry's author, once, after the exit. Bucket bits: 1 INFORMATION, 2 THESIS,
//!          4 EXECUTION, 8 SIZING, 16 RISK_MANAGEMENT, 32 LEVERAGE (refused: spot only),
//!          64 VARIANCE (only on a loss, and only alone).
//! query counts() -> versions u64, entries u64, start_height u64
//!       version(id) -> author, line, parent, hash, spec_uri, thought_ref, claimed, motives,
//!                      height, superseded_by, open_entry, entries, then the stats
//!       trade(id)   -> entry fields, exit (or a 0 flag), post-mortem (or a 0 flag)
//!
//! Stats per version are written by `postmortem` only (a trade counts once it is explained):
//! trades, wins, losses, flats, no_fills, pnl_sum (i64), lucky_wins (a win with a fault bit),
//! and a count per bucket bit. The full records (strategy spec, input snapshot, trade record,
//! post-mortem) are content-addressed blobs: their blake3 is what is committed here.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use cadre_contract_sdk::{self as sdk, abi::Reader, abi::Writer, env, storage, Address, Hash};

const MAX_URI: usize = 160;
const MAX_REF: usize = 80;
const MAX_MOTIVES: usize = 8;
const BUCKETS: usize = 7;

pub const B_INFORMATION: u8 = 1;
pub const B_THESIS: u8 = 2;
pub const B_EXECUTION: u8 = 4;
pub const B_SIZING: u8 = 8;
pub const B_RISK: u8 = 16;
pub const B_LEVERAGE: u8 = 32;
pub const B_VARIANCE: u8 = 64;
const FAULTS: u8 = B_INFORMATION | B_THESIS | B_EXECUTION | B_SIZING | B_RISK;

pub const R_STOP: u8 = 1;
pub const R_TARGET: u8 = 2;
pub const R_TIME: u8 = 3;
pub const R_GONE: u8 = 4;
pub const R_NO_FILL: u8 = 5;

struct Version {
    author: Address,
    line: u64,
    parent: u64,
    hash: Hash,
    spec_uri: Vec<u8>,
    thought_ref: Vec<u8>,
    claimed: u8,
    motives: Vec<u64>,
    height: u64,
    superseded_by: u64,
    open_entry: u64,
    entries: u64,
}

#[derive(Default)]
struct Stats {
    trades: u64,
    wins: u64,
    losses: u64,
    flats: u64,
    no_fills: u64,
    pnl_sum: i64,
    lucky_wins: u64,
    bucket: [u64; BUCKETS],
}

struct Entry {
    author: Address,
    version: u64,
    snapshot: Hash,
    size: u64,
    price_e12: u64,
    quote_out: u64,
    stop_bps: u32,
    target_bps: u32,
    max_risk: u64,
    decision_slot: u64,
    decision_ref: Vec<u8>,
    height: u64,
}

struct Exit {
    fill_slot: u64,
    fill_out: u64,
    fill_price_e12: u64,
    exit_slot: u64,
    reason: u8,
    proceeds: u64,
    pnl: i64,
    record: Hash,
    height: u64,
}

struct Pm {
    result: Hash,
    outcome: u8,
    buckets: u8,
    analyzer: Hash,
    thought_ref: Vec<u8>,
    height: u64,
    used_by: u64,
}

fn k(prefix: &[u8], id: u64) -> Vec<u8> {
    storage::key(prefix, &id.to_le_bytes())
}

fn wv(v: &Version) -> Vec<u8> {
    let mut w = Writer::new()
        .addr(&v.author)
        .u64(v.line)
        .u64(v.parent)
        .hash(&v.hash)
        .bytes(&v.spec_uri)
        .bytes(&v.thought_ref)
        .u8(v.claimed)
        .u8(v.motives.len() as u8);
    for m in &v.motives {
        w = w.u64(*m);
    }
    w.u64(v.height).u64(v.superseded_by).u64(v.open_entry).u64(v.entries).finish()
}

fn rv(b: &[u8]) -> Result<Version, &'static str> {
    let mut r = Reader::new(b);
    let author = r.addr()?;
    let line = r.u64()?;
    let parent = r.u64()?;
    let hash = r.hash()?;
    let spec_uri = r.bytes()?.to_vec();
    let thought_ref = r.bytes()?.to_vec();
    let claimed = r.u8()?;
    let n = r.u8()? as usize;
    let mut motives = Vec::with_capacity(n);
    for _ in 0..n {
        motives.push(r.u64()?);
    }
    Ok(Version { author, line, parent, hash, spec_uri, thought_ref, claimed, motives, height: r.u64()?, superseded_by: r.u64()?, open_entry: r.u64()?, entries: r.u64()? })
}

fn ws(s: &Stats) -> Vec<u8> {
    let mut w = Writer::new().u64(s.trades).u64(s.wins).u64(s.losses).u64(s.flats).u64(s.no_fills).u64(s.pnl_sum as u64).u64(s.lucky_wins);
    for c in s.bucket {
        w = w.u64(c);
    }
    w.finish()
}

fn rs(b: &[u8]) -> Result<Stats, &'static str> {
    let mut r = Reader::new(b);
    let mut s = Stats { trades: r.u64()?, wins: r.u64()?, losses: r.u64()?, flats: r.u64()?, no_fills: r.u64()?, pnl_sum: r.u64()? as i64, lucky_wins: r.u64()?, bucket: [0; BUCKETS] };
    for i in 0..BUCKETS {
        s.bucket[i] = r.u64()?;
    }
    Ok(s)
}

fn we(e: &Entry) -> Vec<u8> {
    Writer::new()
        .addr(&e.author)
        .u64(e.version)
        .hash(&e.snapshot)
        .u64(e.size)
        .u64(e.price_e12)
        .u64(e.quote_out)
        .u32(e.stop_bps)
        .u32(e.target_bps)
        .u64(e.max_risk)
        .u64(e.decision_slot)
        .bytes(&e.decision_ref)
        .u64(e.height)
        .finish()
}

fn re(b: &[u8]) -> Result<Entry, &'static str> {
    let mut r = Reader::new(b);
    Ok(Entry {
        author: r.addr()?,
        version: r.u64()?,
        snapshot: r.hash()?,
        size: r.u64()?,
        price_e12: r.u64()?,
        quote_out: r.u64()?,
        stop_bps: r.u32()?,
        target_bps: r.u32()?,
        max_risk: r.u64()?,
        decision_slot: r.u64()?,
        decision_ref: r.bytes()?.to_vec(),
        height: r.u64()?,
    })
}

fn wx(x: &Exit) -> Vec<u8> {
    Writer::new()
        .u64(x.fill_slot)
        .u64(x.fill_out)
        .u64(x.fill_price_e12)
        .u64(x.exit_slot)
        .u8(x.reason)
        .u64(x.proceeds)
        .u64(x.pnl as u64)
        .hash(&x.record)
        .u64(x.height)
        .finish()
}

fn rx(b: &[u8]) -> Result<Exit, &'static str> {
    let mut r = Reader::new(b);
    Ok(Exit { fill_slot: r.u64()?, fill_out: r.u64()?, fill_price_e12: r.u64()?, exit_slot: r.u64()?, reason: r.u8()?, proceeds: r.u64()?, pnl: r.u64()? as i64, record: r.hash()?, height: r.u64()? })
}

fn wp(p: &Pm) -> Vec<u8> {
    Writer::new().hash(&p.result).u8(p.outcome).u8(p.buckets).hash(&p.analyzer).bytes(&p.thought_ref).u64(p.height).u64(p.used_by).finish()
}

fn rp(b: &[u8]) -> Result<Pm, &'static str> {
    let mut r = Reader::new(b);
    Ok(Pm { result: r.hash()?, outcome: r.u8()?, buckets: r.u8()?, analyzer: r.hash()?, thought_ref: r.bytes()?.to_vec(), height: r.u64()?, used_by: r.u64()? })
}

fn version(id: u64) -> Result<Version, &'static str> {
    storage::get(&k(b"v", id)).ok_or("no such version").and_then(|b| rv(&b))
}
fn stats(id: u64) -> Stats {
    storage::get(&k(b"s", id)).and_then(|b| rs(&b).ok()).unwrap_or_default()
}
fn entry(id: u64) -> Result<Entry, &'static str> {
    storage::get(&k(b"e", id)).ok_or("no such entry").and_then(|b| re(&b))
}
fn exit_of(id: u64) -> Option<Exit> {
    storage::get(&k(b"x", id)).and_then(|b| rx(&b).ok())
}
fn pm_of(id: u64) -> Option<Pm> {
    storage::get(&k(b"p", id)).and_then(|b| rp(&b).ok())
}

fn text(b: &[u8], max: usize, what: &'static str) -> Result<Vec<u8>, &'static str> {
    if b.len() > max {
        return Err(what);
    }
    core::str::from_utf8(b).map_err(|_| "text must be UTF-8")?;
    Ok(b.to_vec())
}

fn u64h(x: u64) -> Hash {
    let mut h = [0u8; 32];
    h[..8].copy_from_slice(&x.to_le_bytes());
    h
}

#[no_mangle]
pub extern "C" fn instantiate() {
    sdk::init(|args| {
        let start = args.u64()?;
        storage::set_u64(b"start", start);
        storage::set_u64(b"nv", 0);
        storage::set_u64(b"ne", 0);
        Ok(Vec::new())
    })
}

#[no_mangle]
pub extern "C" fn call() {
    sdk::dispatch(|method, a| {
        let e = env();
        if e.value != 0 {
            return Err("this contract takes no value");
        }
        if e.height < storage::get_u64(b"start").unwrap_or(0) {
            return Err("not active before start_height");
        }
        let me = e.caller;
        match method {
            "publish" => {
                let hash = a.hash()?;
                let parent = a.u64()?;
                let spec_uri = text(a.bytes()?, MAX_URI, "spec_uri above 160 bytes")?;
                let thought_ref = text(a.bytes()?, MAX_REF, "thought_ref above 80 bytes")?;
                let claimed = a.u8()?;
                let n = a.u8()? as usize;
                if n > MAX_MOTIVES {
                    return Err("at most 8 motives");
                }
                let mut motives = Vec::with_capacity(n);
                for _ in 0..n {
                    motives.push(a.u64()?);
                }
                a.end()?;
                let id = storage::get_u64(b"nv").unwrap_or(0) + 1;
                let line;
                if parent == 0 {
                    if claimed != 0 || n != 0 {
                        return Err("a new line claims no fix and cites no post-mortem");
                    }
                    line = id;
                } else {
                    let mut p = version(parent)?;
                    if p.author != me {
                        return Err("only the parent's author can revise it");
                    }
                    if p.superseded_by != 0 {
                        return Err("the parent is not the head of its line");
                    }
                    if p.open_entry != 0 {
                        return Err("the parent has an open trade: exit and explain it first");
                    }
                    if !(1..=5).contains(&claimed) {
                        return Err("claimed must be a fixable bucket 1..5 (not LEVERAGE, not VARIANCE)");
                    }
                    if n == 0 {
                        return Err("a revision must cite the post-mortem(s) that motivated it");
                    }
                    let bit = 1u8 << (claimed - 1);
                    for (i, m) in motives.iter().enumerate() {
                        if motives[..i].contains(m) {
                            return Err("a motive is cited twice");
                        }
                        let mut pm = pm_of(*m).ok_or("a motive has no post-mortem")?;
                        let en = entry(*m)?;
                        if en.author != me {
                            return Err("a motive is someone else's post-mortem");
                        }
                        if version(en.version)?.line != p.line {
                            return Err("a motive is from another strategy line");
                        }
                        if pm.buckets & bit == 0 {
                            return Err("a motive does not name the claimed bucket");
                        }
                        if pm.used_by != 0 {
                            return Err("a motive already motivated another version");
                        }
                        pm.used_by = id;
                        storage::set(&k(b"p", *m), &wp(&pm));
                    }
                    p.superseded_by = id;
                    storage::set(&k(b"v", parent), &wv(&p));
                    line = p.line;
                }
                let v = Version { author: me, line, parent, hash, spec_uri, thought_ref, claimed, motives, height: e.height, superseded_by: 0, open_entry: 0, entries: 0 };
                storage::set(&k(b"v", id), &wv(&v));
                storage::set_u64(b"nv", id);
                sdk::emit("Version", &[me, hash, u64h(id)], &Writer::new().u64(id).u64(line).u64(parent).u8(claimed).finish());
                Ok(Writer::new().u64(id).finish())
            }
            "enter" => {
                let vid = a.u64()?;
                let snapshot = a.hash()?;
                let size = a.u64()?;
                let price_e12 = a.u64()?;
                let quote_out = a.u64()?;
                let stop_bps = a.u32()?;
                let target_bps = a.u32()?;
                let max_risk = a.u64()?;
                let decision_slot = a.u64()?;
                let decision_ref = text(a.bytes()?, MAX_REF, "decision_ref above 80 bytes")?;
                a.end()?;
                let mut v = version(vid)?;
                if v.author != me {
                    return Err("only the version's author trades it");
                }
                if v.superseded_by != 0 {
                    return Err("the version is superseded: trade the head of the line");
                }
                if e.height <= v.height {
                    return Err("an entry must land in a later block than its version");
                }
                if v.open_entry != 0 {
                    return Err("the line already has an open trade");
                }
                if size == 0 || price_e12 == 0 || quote_out == 0 || max_risk == 0 {
                    return Err("size, price, quote and max_risk must be positive");
                }
                if stop_bps == 0 || stop_bps >= 10_000 || target_bps == 0 {
                    return Err("stop_bps in 1..9999 and target_bps above 0");
                }
                if decision_ref.is_empty() {
                    return Err("decision_ref (the paid decision's receipt id) is required");
                }
                let id = storage::get_u64(b"ne").unwrap_or(0) + 1;
                let en = Entry { author: me, version: vid, snapshot, size, price_e12, quote_out, stop_bps, target_bps, max_risk, decision_slot, decision_ref, height: e.height };
                storage::set(&k(b"e", id), &we(&en));
                storage::set_u64(b"ne", id);
                v.open_entry = id;
                v.entries += 1;
                storage::set(&k(b"v", vid), &wv(&v));
                sdk::emit("Entry", &[me, u64h(vid), u64h(id)], &Writer::new().u64(id).u64(vid).hash(&snapshot).u64(size).u64(decision_slot).finish());
                Ok(Writer::new().u64(id).finish())
            }
            "exit" => {
                let id = a.u64()?;
                let fill_slot = a.u64()?;
                let fill_out = a.u64()?;
                let fill_price_e12 = a.u64()?;
                let exit_slot = a.u64()?;
                let reason = a.u8()?;
                let proceeds = a.u64()?;
                let record = a.hash()?;
                a.end()?;
                let en = entry(id)?;
                if en.author != me {
                    return Err("only the entry's author exits it");
                }
                if exit_of(id).is_some() {
                    return Err("already exited");
                }
                if e.height <= en.height {
                    return Err("an exit must land in a later block than its entry");
                }
                if !(1..=5).contains(&reason) {
                    return Err("reason 1 stop, 2 target, 3 time, 4 condition gone, 5 no fill");
                }
                if reason == R_NO_FILL {
                    if proceeds != en.size || fill_out != 0 {
                        return Err("no fill: nothing was bought, proceeds equal size and fill_out is 0");
                    }
                } else if fill_out == 0 || exit_slot < fill_slot || fill_slot < en.decision_slot {
                    return Err("a fill needs fill_out > 0 and decision_slot <= fill_slot <= exit_slot");
                }
                if en.size > i64::MAX as u64 || proceeds > i64::MAX as u64 {
                    return Err("amount out of range");
                }
                let pnl = proceeds as i64 - en.size as i64;
                let x = Exit { fill_slot, fill_out, fill_price_e12, exit_slot, reason, proceeds, pnl, record, height: e.height };
                storage::set(&k(b"x", id), &wx(&x));
                let mut v = version(en.version)?;
                v.open_entry = 0;
                storage::set(&k(b"v", en.version), &wv(&v));
                sdk::emit("Exit", &[me, u64h(en.version), u64h(id)], &Writer::new().u64(id).u8(reason).u64(pnl as u64).hash(&record).finish());
                Ok(Writer::new().u64(pnl as u64).finish())
            }
            "postmortem" => {
                let id = a.u64()?;
                let result = a.hash()?;
                let buckets = a.u8()?;
                let analyzer = a.hash()?;
                let thought_ref = text(a.bytes()?, MAX_REF, "thought_ref above 80 bytes")?;
                a.end()?;
                let en = entry(id)?;
                if en.author != me {
                    return Err("only the entry's author explains it");
                }
                let x = exit_of(id).ok_or("no exit yet: a post-mortem comes after the exit")?;
                if pm_of(id).is_some() {
                    return Err("already explained: a post-mortem is written once");
                }
                if buckets & 128 != 0 {
                    return Err("unknown bucket bit");
                }
                if buckets & B_LEVERAGE != 0 {
                    return Err("LEVERAGE does not apply: spot only");
                }
                let outcome = if x.reason == R_NO_FILL {
                    4
                } else if x.pnl > 0 {
                    1
                } else if x.pnl < 0 {
                    2
                } else {
                    3
                };
                if buckets & B_VARIANCE != 0 && (outcome != 2 || buckets & FAULTS != 0) {
                    return Err("VARIANCE only on a loss, and only when no fault bucket applies");
                }
                if thought_ref.is_empty() {
                    return Err("thought_ref (the paid post-mortem's receipt id) is required");
                }
                let p = Pm { result, outcome, buckets, analyzer, thought_ref, height: e.height, used_by: 0 };
                storage::set(&k(b"p", id), &wp(&p));
                let mut s = stats(en.version);
                s.trades += 1;
                match outcome {
                    1 => s.wins += 1,
                    2 => s.losses += 1,
                    3 => s.flats += 1,
                    _ => s.no_fills += 1,
                }
                s.pnl_sum = s.pnl_sum.saturating_add(x.pnl);
                if outcome == 1 && buckets & FAULTS != 0 {
                    s.lucky_wins += 1;
                }
                for i in 0..BUCKETS {
                    if buckets & (1 << i) != 0 {
                        s.bucket[i] += 1;
                    }
                }
                storage::set(&k(b"s", en.version), &ws(&s));
                sdk::emit("PostMortem", &[me, u64h(en.version), u64h(id)], &Writer::new().u64(id).u8(outcome).u8(buckets).hash(&result).finish());
                Ok(Writer::new().u8(outcome).finish())
            }
            _ => Err("unknown method"),
        }
    })
}

#[no_mangle]
pub extern "C" fn query() {
    sdk::dispatch(|method, a| match method {
        "counts" => Ok(Writer::new().u64(storage::get_u64(b"nv").unwrap_or(0)).u64(storage::get_u64(b"ne").unwrap_or(0)).u64(storage::get_u64(b"start").unwrap_or(0)).finish()),
        "version" => {
            let id = a.u64()?;
            let b = storage::get(&k(b"v", id)).ok_or("no such version")?;
            let mut out = b;
            out.extend_from_slice(&ws(&stats(id)));
            Ok(out)
        }
        "trade" => {
            let id = a.u64()?;
            let mut out = storage::get(&k(b"e", id)).ok_or("no such entry")?;
            match storage::get(&k(b"x", id)) {
                Some(x) => {
                    out.push(1);
                    out.extend_from_slice(&x);
                }
                None => out.push(0),
            }
            match storage::get(&k(b"p", id)) {
                Some(p) => {
                    out.push(1);
                    out.extend_from_slice(&p);
                }
                None => out.push(0),
            }
            Ok(out)
        }
        _ => Err("unknown method"),
    })
}
