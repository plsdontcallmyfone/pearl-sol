//! Services market (docs/MARKET.md, docs/CLI.md "Services market"): `cadre market` (summary +
//! offers), `cadre market offer <id>` / `cadre offer <id>`, `cadre market hires [...]`,
//! `cadre hire <id>`, and the builders `cadre hire --offer ...` / `cadre offer --title ...`,
//! plus the data the app's Market view draws.
//!
//! Two sources, the same shapes: cadre-api's /v1/market* routes when that cadre-api serves
//! them, else the market contract read directly from a node (queryContract for the config,
//! counts, offers and hires; getContractEvents for the timeline; getBlob for specs, requests,
//! deliveries and verdict reasons). The record and event decoders here are the Rust port of
//! apps/explorer/src/market.ts (vectors from that codec in tests/fixtures/market_vectors.json).
//! On the node path the reputation rules are the indexer's, minus what needs indexed profiles
//! and transfers: house accounts (deploy/market-devnet.json) are excluded from organic volume;
//! operator links and the funding heuristic need cadre-api and are said to be missing.
//!
//! Writes: the builders never sign unless `--send` is given together with a key (`--key FILE`
//! or CADRE_KEY); then they upload the blob and send the call through `cadre contract call`'s
//! own path. Without `--send` they print the plain-words review, the call arguments and the
//! exact `cadre contract call` command.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Result};
use base64::Engine as _;
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::Ctx;
use crate::fmt::{amount, cx_str, short_addr, short_n};
use crate::probe::get;
use crate::snapshot::{s, u};
use crate::term::{Block, Panel, Table, Tone, View};

// ---------------------------------------------------------------------------------------
// the devnet deployment (deploy/market-devnet.json, compiled in)

pub const DEVNET_MARKET: &str = "E1kpbaHn14z4Amdqb7SGjLbn576q1KPC3haAG6YQdsym";
pub const DEVNET_CODE_HASH: &str = "1647ce2418396531137c6451b9ce5f47dd6ab7197c36db9ad7c2ca95abf3080e";
pub const DEVNET_DEPLOYED_HEIGHT: u64 = 4846;
/// The house LLM judge (labelled house).
pub const DEVNET_JUDGE: &str = "F29KnYVTa3CTndnqMA1WspL44EsYF1CKNWva5syT6smx";
/// House accounts: the market desk, its judge and the market ops key. Their hires are never organic volume.
pub const HOUSE: &[(&str, &str)] = &[
    ("F29KnYVTa3CTndnqMA1WspL44EsYF1CKNWva5syT6smx", "court_clerk"),
    ("92iRhbAMCjdSMDbBwfkxF7h6U2qyd8dHr4cMg7xJzcpU", "scribe_wren"),
    ("BwGJUiA1hTxmfwgzRGVPKshgeXrVrCmbPdM6X6Vuu9X", "ledger_lark"),
    ("GkFq5knZFieWHTu3hK6xJQgdzEnUJ2rVfqbS3ruJ4EbZ", "verse_finch"),
    ("BrEnJ9yVfj6xFRRz1yU5tgBS9rniMpiev4Wunmav85kh", "tally_crow"),
    // the market ops key (admin, fee_to); its builder-path test hires never count as organic
    ("J4F39nEoXau8QLBsJZybW1jQJ4bQE1oiPMQCHEFBr3jv", "market_ops"),
];

pub const STATUS: [&str; 6] = ["open", "delivered", "completed", "rejected", "expired", "declined"];
pub const POLICY: [&str; 2] = ["buyer_names", "offer_list"];
pub const EVENTS: [&str; 10] = ["OfferListed", "OfferUpdated", "Hired", "Declined", "Delivered", "Verdict", "Completed", "Rejected", "Expired", "ConfigSet"];
const MAX_TITLE: usize = 96;
const MAX_CATEGORY: usize = 32;
const MAX_URI: usize = 256;
const MAX_EVAL: usize = 4;
const MAX_TURNAROUND: u64 = 100_800;
const MAX_DEADLINE: u64 = 201_600;
const BLOCK_S: u64 = 60;
const RULE: &str = "paid volume = completed hire prices from distinct payers; self-dealing (shared operator, operator link, or funded by the provider: heuristic, labelled) and house buyers are excluded and shown apart; counts are shown, never ranked on";

fn house_label(a: &str) -> Option<&'static str> {
    HOUSE.iter().find(|(k, _)| *k == a).map(|(_, h)| *h)
}

// ---------------------------------------------------------------------------------------
// options

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum SourceSel {
    /// cadre-api when it serves /v1/market, else the node.
    #[default]
    Auto,
    /// cadre-api only.
    Api,
    /// The market contract on the node only.
    Node,
}

#[derive(Args, Debug, Clone, Default)]
pub struct MarketOpts {
    /// Market contract address (default: the devnet market from deploy/market-devnet.json).
    #[arg(long, env = "CADRE_MARKET")]
    pub market: Option<String>,
    /// Where to read: auto (cadre-api if it serves /v1/market, else the node), api, node.
    #[arg(long, value_enum, default_value_t = SourceSel::Auto)]
    pub source: SourceSel,
}

impl MarketOpts {
    /// What the app's Market view reads: CADRE_MARKET when set, else the devnet market.
    pub fn from_env() -> MarketOpts {
        MarketOpts { market: std::env::var("CADRE_MARKET").ok().filter(|m| !m.trim().is_empty()), source: SourceSel::Auto }
    }
    fn or(&self, parent: &MarketOpts) -> MarketOpts {
        MarketOpts { market: self.market.clone().or(parent.market.clone()), source: if self.source == SourceSel::Auto { parent.source } else { self.source } }
    }
    /// The market contract as base58 (what the node's RPC takes).
    fn address(&self) -> Result<String> {
        match &self.market {
            Some(m) => Ok(bs58::encode(crate::fmt::parse_chain_addr(m)?).into_string()),
            None => Ok(DEVNET_MARKET.to_string()),
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct ListArgs {
    /// Only offers in this category.
    #[arg(long)]
    pub category: Option<String>,
    /// Only this provider's offers (address or Cx).
    #[arg(long)]
    pub provider: Option<String>,
    /// Paused offers too.
    #[arg(long)]
    pub all: bool,
    #[command(flatten)]
    pub mo: MarketOpts,
}

#[derive(Subcommand, Debug, Clone)]
pub enum MarketCmd {
    /// One offer: terms, the spec blob's fields, the provider's record, recent hires.
    Offer {
        id: u64,
        #[command(flatten)]
        mo: MarketOpts,
    },
    /// Hires, newest first, by party and status.
    Hires {
        #[arg(long)]
        buyer: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        evaluator: Option<String>,
        /// open, delivered, completed, rejected, expired or declined.
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[command(flatten)]
        mo: MarketOpts,
    },
    /// One hire: parties, money, timeline, request, delivery, verdicts with reasons.
    Hire {
        id: u64,
        #[command(flatten)]
        mo: MarketOpts,
    },
    /// An account in the market: its offers, provider record, hires as buyer / provider / evaluator.
    Agent {
        account: String,
        #[command(flatten)]
        mo: MarketOpts,
    },
}

#[derive(Args, Debug, Clone)]
pub struct OfferArgs {
    /// Show this offer (or build a new one with --title ...).
    pub id: Option<u64>,
    /// New offer: its title (at most 96 bytes).
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long, default_value = "")]
    pub category: String,
    /// Price in CADRE.
    #[arg(long)]
    pub price: Option<String>,
    /// Blocks to deliver in (60 s blocks).
    #[arg(long)]
    pub turnaround: Option<u64>,
    /// What the service does (goes into the spec blob).
    #[arg(long, default_value = "")]
    pub description: String,
    /// What the buyer gets (spec blob).
    #[arg(long, default_value = "")]
    pub deliverable: String,
    /// buyer (the buyer names evaluators) or list (only from --evaluator).
    #[arg(long, default_value = "buyer")]
    pub policy: String,
    /// An evaluator buyers may pick (repeatable; required with --policy list).
    #[arg(long = "evaluator")]
    pub evaluators: Vec<String>,
    /// List the offer paused (no hires until updated).
    #[arg(long)]
    pub paused: bool,
    /// Upload the spec blob and sign the call with --key (else: review + command only).
    #[arg(long)]
    pub send: bool,
    #[command(flatten)]
    pub mo: MarketOpts,
}

#[derive(Args, Debug, Clone)]
pub struct HireArgs {
    /// Show this hire (or build a new one with --offer ...).
    pub id: Option<u64>,
    /// New hire: the offer id.
    #[arg(long)]
    pub offer: Option<u64>,
    /// What you ask for, in plain text (goes into the request blob).
    #[arg(long)]
    pub request: Option<String>,
    /// Who decides (repeatable, 1 to 4): a person, an agent or an LLM judge.
    #[arg(long = "evaluator")]
    pub evaluators: Vec<String>,
    /// How many evaluators must accept.
    #[arg(long, default_value_t = 1)]
    pub threshold: u8,
    /// CADRE for the evaluators who decide.
    #[arg(long, default_value = "0")]
    pub judge_fee: String,
    /// Blocks until anyone may refund you (default: twice the offer's turnaround).
    #[arg(long)]
    pub deadline: Option<u64>,
    /// Upload the request blob and sign the call with --key (else: review + command only).
    #[arg(long)]
    pub send: bool,
    #[command(flatten)]
    pub mo: MarketOpts,
}

// ---------------------------------------------------------------------------------------
// codec (apps/explorer/src/market.ts)

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        R { b, o: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.b.len() >= self.o + n, "record too short");
        let s = &self.b[self.o..self.o + n];
        self.o += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? == 1)
    }
    fn addr(&mut self) -> Result<String> {
        Ok(bs58::encode(self.take(32)?).into_string())
    }
    fn hash(&mut self) -> Result<String> {
        Ok(hex::encode(self.take(32)?))
    }
    fn str(&mut self) -> Result<String> {
        let n = u32::from_le_bytes(self.take(4)?.try_into()?) as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn evals(&mut self) -> Result<Vec<String>> {
        let n = self.u8()?;
        (0..n).map(|_| self.addr()).collect()
    }
}

/// The protocol fee on a price: ceil(price x bps / 10000) (contract `fee_of`).
pub fn fee_of(price: u64, bps: u16) -> u64 {
    (price as u128 * bps as u128).div_ceil(10_000) as u64
}

/// A stored offer record (query `offer(id)`), as market.ts `decodeOffer`.
pub fn decode_offer(id: u64, b: &[u8]) -> Result<Value> {
    let mut r = R::new(b);
    let provider = r.addr()?;
    let (title, category, spec_uri, spec_hash) = (r.str()?, r.str()?, r.str()?, r.hash()?);
    let (price, turnaround, policy) = (r.u64()?, r.u64()?, r.u8()?);
    let evaluators = r.evals()?;
    let active = r.bool()?;
    let (created, updated, hires, completed) = (r.u64()?, r.u64()?, r.u64()?, r.u64()?);
    Ok(json!({"id": id, "provider": provider, "title": title, "category": category, "spec_uri": spec_uri, "spec_hash": spec_hash, "price": price.to_string(),
              "turnaround": turnaround, "policy": POLICY.get(policy as usize).copied().unwrap_or("buyer_names"), "evaluators": evaluators, "active": active,
              "created": created, "updated": updated, "hires": hires, "completed": completed}))
}

/// A stored hire record (query `hire(id)`), as market.ts `decodeHire`.
pub fn decode_hire(id: u64, b: &[u8]) -> Result<Value> {
    let mut r = R::new(b);
    let (offer, buyer, provider) = (r.u64()?, r.addr()?, r.addr()?);
    let (price, fee, judge_fee) = (r.u64()?, r.u64()?, r.u64()?);
    let (created, deadline) = (r.u64()?, r.u64()?);
    let (request_uri, request_hash) = (r.str()?, r.hash()?);
    let evaluators = r.evals()?;
    let (threshold, status, accepts, rejects, voted, acc) = (r.u8()?, r.u8()?, r.u8()?, r.u8()?, r.u8()?, r.u8()?);
    let (result_hash, result_uri, delivered, closed) = (r.hash()?, r.str()?, r.u64()?, r.u64()?);
    let bits = |m: u8| -> Vec<String> { evaluators.iter().enumerate().filter(|(i, _)| m & (1 << i) != 0).map(|(_, a)| a.clone()).collect() };
    let nz = |x: u64| if x == 0 { Value::Null } else { json!(x) };
    Ok(json!({
        "id": id, "offer": offer, "buyer": buyer, "provider": provider, "price": price.to_string(), "fee": fee.to_string(), "judge_fee": judge_fee.to_string(),
        "total": (price as u128 + fee as u128 + judge_fee as u128).to_string(), "created": created, "deadline": deadline,
        "request_uri": request_uri, "request_hash": request_hash, "evaluators": evaluators, "threshold": threshold,
        "status": STATUS.get(status as usize).copied().unwrap_or("open"), "accepts": accepts, "rejects": rejects, "voted": bits(voted), "accepted_by": bits(acc),
        "result_hash": if result_hash.bytes().all(|c| c == b'0') { Value::Null } else { json!(result_hash) },
        "result_uri": if result_uri.is_empty() { Value::Null } else { json!(result_uri) }, "delivered": nz(delivered), "closed": nz(closed),
    }))
}

pub fn decode_config(b: &[u8]) -> Result<Value> {
    let mut r = R::new(b);
    Ok(json!({"admin": r.addr()?, "fee_bps": r.u16()?, "fee_to": r.addr()?, "hires_paused": r.bool()?}))
}

pub fn decode_quote(b: &[u8]) -> Result<Value> {
    let mut r = R::new(b);
    Ok(json!({"price": r.u64()?.to_string(), "fee": r.u64()?.to_string(), "judge_fee": r.u64()?.to_string(), "total": r.u64()?.to_string()}))
}

fn topic_name(t: &str) -> Option<&'static str> {
    let t = t.to_ascii_lowercase();
    EVENTS.iter().copied().find(|n| hex::encode(agent_types::contracts::event_topic(n.as_bytes())) == t)
}

/// One contract event (getContractEvents item) in words, as market.ts `decodeEvent`;
/// None for an event this contract does not emit.
pub fn decode_event(ev: &Value) -> Option<Value> {
    let topics: Vec<&str> = ev["topics"].as_array()?.iter().filter_map(|t| t.as_str()).collect();
    let name = topic_name(topics.first()?)?;
    let id_of = |t: Option<&&str>| -> Value {
        t.and_then(|t| hex::decode(t).ok()).filter(|b| b.len() >= 8).map(|b| json!(u64::from_le_bytes(b[..8].try_into().unwrap()))).unwrap_or(Value::Null)
    };
    let addr_of = |t: Option<&&str>| -> Value { t.and_then(|t| hex::decode(t).ok()).map(|b| json!(bs58::encode(b).into_string())).unwrap_or(Value::Null) };
    let cs = name == "ConfigSet";
    let raw = hex::decode(s(ev, "data")).unwrap_or_default();
    let data = (|| -> Result<Value> {
        let mut r = R::new(&raw);
        Ok(match name {
            "OfferListed" | "OfferUpdated" => json!({"price": r.u64()?.to_string(), "turnaround": r.u64()?, "policy": POLICY.get(r.u8()? as usize).copied(), "title": r.str()?, "category": r.str()?, "active": r.bool()?}),
            "Hired" => json!({"offer": r.u64()?, "price": r.u64()?.to_string(), "fee": r.u64()?.to_string(), "judge_fee": r.u64()?.to_string(), "deadline": r.u64()?, "threshold": r.u8()?, "evaluators": r.evals()?, "request_hash": r.hash()?, "request_uri": r.str()?}),
            "Declined" => json!({"refund": r.u64()?.to_string(), "reason": r.str()?}),
            "Delivered" => {
                let (h, uri, t) = (r.hash()?, r.str()?, r.str()?);
                json!({"result_hash": h, "result_uri": uri, "thought_ref": if t.is_empty() { Value::Null } else { json!(t) }})
            }
            "Verdict" => {
                let (accept, rh, ru, res, t) = (r.bool()?, r.hash()?, r.str()?, r.hash()?, r.str()?);
                json!({"accept": accept, "reason_hash": rh, "reason_uri": if ru.is_empty() { Value::Null } else { json!(ru) }, "result_hash": res, "thought_ref": if t.is_empty() { Value::Null } else { json!(t) }})
            }
            "Completed" => json!({"buyer": r.addr()?, "price": r.u64()?.to_string(), "fee": r.u64()?.to_string(), "judge_fee": r.u64()?.to_string(), "fee_to": r.addr()?}),
            "Rejected" => json!({"refund": r.u64()?.to_string(), "judge_fee": r.u64()?.to_string()}),
            "Expired" => json!({"delivered": r.bool()?, "refund": r.u64()?.to_string()}),
            _ => json!({"fee_bps": r.u16()?, "fee_to": r.addr()?, "hires_paused": r.bool()?}),
        })
    })()
    .unwrap_or_else(|_| json!({"undecoded": s(ev, "data")}));
    Some(json!({
        "name": name, "height": u(ev, "height"), "tx_hash": ev.get("tx_hash").cloned().unwrap_or(Value::Null),
        "id": if cs { Value::Null } else { id_of(topics.get(1)) }, "who": addr_of(if cs { topics.get(1) } else { topics.get(2) }), "party": addr_of(topics.get(3)), "data": data,
    }))
}

// ---------------------------------------------------------------------------------------
// call arguments (TYPE:VALUE, the `cadre contract call` ABI)

#[derive(Debug)]
pub struct HireCall {
    pub offer: u64,
    pub price: u64,
    pub request_uri: String,
    pub request_hash: [u8; 32],
    pub deadline_blocks: u64,
    pub judge_fee: u64,
    pub evaluators: Vec<[u8; 32]>,
    pub threshold: u8,
}

impl HireCall {
    pub fn typed(&self) -> Vec<String> {
        let mut v = vec![format!("u64:{}", self.offer), format!("u64:{}", self.price), format!("str:{}", self.request_uri), format!("hash:{}", hex::encode(self.request_hash)), format!("u64:{}", self.deadline_blocks), format!("u64:{}", self.judge_fee), format!("u8:{}", self.evaluators.len())];
        v.extend(self.evaluators.iter().map(|k| format!("addr:{}", crate::fmt::cx(k))));
        v.push(format!("u8:{}", self.threshold));
        v
    }
}

#[derive(Debug)]
pub struct OfferCall {
    pub title: String,
    pub category: String,
    pub spec_uri: String,
    pub spec_hash: [u8; 32],
    pub price: u64,
    pub turnaround: u64,
    pub policy: u8,
    pub evaluators: Vec<[u8; 32]>,
    pub active: bool,
}

impl OfferCall {
    pub fn typed(&self) -> Vec<String> {
        let mut v = vec![format!("str:{}", self.title), format!("str:{}", self.category), format!("str:{}", self.spec_uri), format!("hash:{}", hex::encode(self.spec_hash)), format!("u64:{}", self.price), format!("u64:{}", self.turnaround), format!("u8:{}", self.policy), format!("u8:{}", self.evaluators.len())];
        v.extend(self.evaluators.iter().map(|k| format!("addr:{}", crate::fmt::cx(k))));
        v.push(format!("bool:{}", self.active));
        v
    }
}

fn shell_word(w: &str) -> String {
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@=,+".contains(c)) {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// Base units as a CADRE amount `cadre contract call --value` takes ("3.1").
fn cadre_arg(v: u64) -> String {
    let (w, f) = (v / 1_000_000_000, v % 1_000_000_000);
    if f == 0 {
        w.to_string()
    } else {
        format!("{w}.{}", format!("{f:09}").trim_end_matches('0'))
    }
}

/// CADRE base units in words: exact, no thousands separators lost ("2.5", "1,000").
pub fn cad(v: u128) -> String {
    amount(v, 9, 9)
}
fn cad_s(v: &Value) -> String {
    cad(num(v))
}
fn num(v: &Value) -> u128 {
    v.as_str().and_then(|x| x.parse().ok()).or_else(|| v.as_u64().map(|x| x as u128)).unwrap_or(0)
}

/// "about 30 minutes" / "about 1.5 hours" for a block count (cadre-api's `hours`).
pub fn hours(blocks: u64) -> String {
    let h = (blocks * BLOCK_S) as f64 / 3600.0;
    if h < 1.0 {
        format!("about {} minutes", ((h * 60.0).round() as u64).max(1))
    } else if h < 10.0 {
        format!("about {:.1} hours", h)
    } else {
        format!("about {} hours", h.round() as u64)
    }
}

fn pct(bps: u64) -> String {
    if bps % 100 == 0 {
        (bps / 100).to_string()
    } else {
        format!("{}", bps as f64 / 100.0)
    }
}

/// The plain-words review of a hire (cadre-api POST /v1/tx/market `hire`, same sentences).
#[allow(clippy::too_many_arguments)]
pub fn hire_review(market: &str, provider: &str, evaluators: &[String], threshold: u8, deadline: u64, price: u64, fee: u64, judge_fee: u64, fee_bps: u16) -> Vec<String> {
    let total = price as u128 + fee as u128 + judge_fee as u128;
    vec![
        format!("You pay {} CADRE now, held by the market contract {} (not by {provider}).", cad(total), short_addr(market)),
        format!(
            "Of it: {} CADRE goes to {provider} only if your evaluators accept the delivery; {} CADRE is the market fee ({}%), kept only on acceptance; {} CADRE pays the evaluators who decide.",
            cad(price as u128),
            cad(fee as u128),
            pct(fee_bps as u64),
            cad(judge_fee as u128)
        ),
        format!("Evaluators: {}; {threshold} of {} must accept.", evaluators.join(", "), evaluators.len()),
        format!("If they reject, {} CADRE comes back to you (the evaluators keep their {}).", cad(price as u128 + fee as u128), cad(judge_fee as u128)),
        format!("If nothing is delivered and decided within {deadline} blocks ({}), anyone can send a refund of all {} CADRE to you.", hours(deadline), cad(total)),
        format!("{provider} may decline before delivering; then all of it comes back at once."),
    ]
}

/// The plain-words review of listing an offer (cadre-api's `offer` review).
pub fn offer_review(title: &str, price: u64, turnaround: u64, fee_bps: u16, policy_list: Option<&[String]>, active: bool) -> Vec<String> {
    vec![
        format!("List the service \"{title}\" at {} CADRE, delivered within {turnaround} blocks ({}).", cad(price as u128), hours(turnaround)),
        format!("Buyers pay the price plus a {} CADRE market fee ({}%) into escrow; you are paid the price when their evaluators accept your delivery.", cad(fee_of(price, fee_bps) as u128), pct(fee_bps as u64)),
        match policy_list {
            Some(l) => format!("Buyers may only name these evaluators: {}.", l.join(", ")),
            None => "Buyers name their own evaluators (themselves, a person, an agent or an LLM judge).".into(),
        },
        if active { "The offer is open for hires.".into() } else { "The offer is paused (no new hires).".into() },
        "This tx moves no CADRE except the tx fee and a refundable storage deposit.".into(),
    ]
}

// ---------------------------------------------------------------------------------------
// the node path: the contract's state and events, indexed like apps/explorer MarketIndex

fn method(name: &str, fields: &[u8]) -> Vec<u8> {
    let mut v = vec![name.len() as u8];
    v.extend_from_slice(name.as_bytes());
    v.extend_from_slice(fields);
    v
}

fn err_text(e: &str) -> String {
    hex::decode(e).ok().and_then(|b| String::from_utf8(b).ok()).filter(|t| !t.is_empty()).unwrap_or_else(|| e.to_string())
}

fn query(ctx: &Ctx, market: &str, args: Vec<u8>) -> Result<Vec<u8>> {
    let r = ctx.call("queryContract", json!({"address": market, "args": hex::encode(args)}))?;
    if r.is_null() {
        bail!("no contract at {}", cx_str(market));
    }
    ensure!(r["ok"] == json!(true), "{}", err_text(s(&r, "error")));
    Ok(hex::decode(s(&r, "return"))?)
}

#[derive(Debug, Clone)]
pub struct Index {
    pub market: String,
    pub config: Value,
    pub fee_bps: u16,
    pub height: u64,
    pub offers: Vec<Value>,
    pub hires: Vec<Value>,
    /// Oldest first.
    pub events: Vec<Value>,
}

static CACHE: Mutex<Option<(Instant, String, Index)>> = Mutex::new(None);

/// Everything the contract holds (cached 5 s: the app's list and detail share one read).
pub fn read_index(ctx: &Ctx, market: &str) -> Result<Index> {
    if let Some((t, rpc, ix)) = CACHE.lock().unwrap().as_ref() {
        if t.elapsed() < Duration::from_secs(5) && ix.market == market && *rpc == ctx.rpc {
            return Ok(ix.clone());
        }
    }
    let config = decode_config(&query(ctx, market, method("config", &[]))?)?;
    let c = query(ctx, market, method("counts", &[]))?;
    let mut r = R::new(&c);
    let (n_off, n_hire) = (r.u64()?, r.u64()?);
    let height = u(&ctx.call("getChainInfo", json!({}))?, "height");
    let read = |what: &'static str, n: u64| -> Result<Vec<Value>> {
        let mut out = Vec::with_capacity(n as usize);
        let ids: Vec<u64> = (0..n).collect();
        for chunk in ids.chunks(8) {
            let got: Vec<Result<Value>> = std::thread::scope(|sc| {
                let hs: Vec<_> = chunk
                    .iter()
                    .map(|&id| {
                        sc.spawn(move || -> Result<Value> {
                            let b = query(ctx, market, method(what, &id.to_le_bytes()))?;
                            if what == "offer" { decode_offer(id, &b) } else { decode_hire(id, &b) }
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow!("reader thread panicked")))).collect()
            });
            for g in got {
                out.push(g?);
            }
        }
        Ok(out)
    };
    let offers = read("offer", n_off)?;
    let hires = read("hire", n_hire)?;
    // events, newest first from the RPC, 200 a page
    let mut raw: Vec<Value> = vec![];
    let mut off = 0u64;
    loop {
        let p = ctx.call("getContractEvents", json!({"address": market, "limit": 200, "offset": off}))?;
        let evs = p["events"].as_array().cloned().unwrap_or_default();
        let total = u(&p, "total");
        off += evs.len() as u64;
        let empty = evs.is_empty();
        raw.extend(evs);
        if empty || off >= total || off >= 20_000 {
            break;
        }
    }
    raw.reverse();
    let events: Vec<Value> = raw.iter().filter_map(decode_event).collect();
    let fee_bps = u(&config, "fee_bps") as u16;
    let ix = Index { market: market.to_string(), config, fee_bps, height, offers, hires, events };
    *CACHE.lock().unwrap() = Some((Instant::now(), ctx.rpc.clone(), ix.clone()));
    Ok(ix)
}

fn card(a: &str) -> Value {
    let h = house_label(a);
    json!({"account": a, "handle": h, "house": h.is_some()})
}

/// Why a hire's payment is not organic, or None (node path: house buyers only).
fn exclusion(h: &Value) -> Option<Value> {
    house_label(s(h, "buyer")).map(|_| json!({"kind": "house", "reason": "house buyer: house activity, not organic", "heuristic": false}))
}

/// The provider's record (market.ts `providerRecord`), from the hires the contract holds.
pub fn provider_record(hires: &[Value], provider: &str) -> Value {
    let mine: Vec<&Value> = hires.iter().filter(|h| s(h, "provider") == provider).collect();
    let done: Vec<&Value> = mine.iter().copied().filter(|h| s(h, "status") == "completed").collect();
    let (mut organic, mut house) = (0u128, 0u128);
    let mut payers: HashMap<String, u128> = HashMap::new();
    for h in &done {
        let p = num(&h["price"]);
        if exclusion(h).is_some() {
            house += p;
        } else {
            organic += p;
            *payers.entry(s(h, "buyer").to_string()).or_default() += p;
        }
    }
    let top = payers.values().copied().max().unwrap_or(0);
    let independent = done.iter().filter(|h| h["accepted_by"].as_array().is_some_and(|a| a.iter().any(|e| e.as_str() != Some(s(h, "buyer"))))).count();
    let mut turn: Vec<u64> = done.iter().filter(|h| !h["delivered"].is_null()).map(|h| u(h, "delivered").saturating_sub(u(h, "created"))).collect();
    turn.sort();
    let cnt = |st: &str| mine.iter().filter(|h| s(h, "status") == st).count();
    json!({
        "provider": provider,
        "paid_volume_organic": organic.to_string(),
        "distinct_payers": payers.len(),
        "top_payer_share_bps": if organic > 0 { json!((top * 10_000 / organic) as u64) } else { Value::Null },
        "house_volume": house.to_string(),
        "self_dealt_excluded": "0",
        "hires": mine.len(),
        "completed": done.len(),
        "rejected": cnt("rejected"),
        "delivered_then_expired": mine.iter().filter(|h| s(h, "status") == "expired" && !h["delivered"].is_null()).count(),
        "expired_undelivered": mine.iter().filter(|h| s(h, "status") == "expired" && h["delivered"].is_null()).count(),
        "declined": cnt("declined"),
        "independent_evaluation_share_bps": if done.is_empty() { Value::Null } else { json!(((independent * 10_000) as f64 / done.len() as f64).round() as u64) },
        "median_turnaround_blocks": if turn.is_empty() { Value::Null } else { json!(turn[(turn.len() - 1) / 2]) },
        "rule": RULE,
        "basis": "node: house accounts excluded; operator links and the funding heuristic need cadre-api (not applied)",
    })
}

impl Index {
    fn offer(&self, id: u64) -> Option<&Value> {
        self.offers.get(id as usize)
    }

    fn offer_view(&self, o: &Value) -> Value {
        let rec = provider_record(&self.hires, s(o, "provider"));
        let id = u(o, "id");
        let mine: Vec<&Value> = self.hires.iter().filter(|h| u(h, "offer") == id).collect();
        let mut v = o.clone();
        let price = num(&o["price"]) as u64;
        let m = v.as_object_mut().unwrap();
        m.insert("price_cadre".into(), json!(cad(price as u128)));
        m.insert("fee_bps".into(), json!(self.fee_bps));
        m.insert("fee".into(), json!(fee_of(price, self.fee_bps).to_string()));
        m.insert("turnaround_hint".into(), json!(hours(u(o, "turnaround"))));
        m.insert("provider_card".into(), card(s(o, "provider")));
        m.insert("evaluator_cards".into(), Value::Array(o["evaluators"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).map(card).collect()));
        m.insert(
            "record".into(),
            json!({"paid_volume_organic": rec["paid_volume_organic"], "distinct_payers": rec["distinct_payers"], "completed": rec["completed"], "rejected": rec["rejected"], "delivered_then_expired": rec["delivered_then_expired"], "house_volume": rec["house_volume"]}),
        );
        m.insert(
            "offer_stats".into(),
            json!({"hires": mine.len(), "completed": mine.iter().filter(|h| s(h, "status") == "completed").count(), "open": mine.iter().filter(|h| matches!(s(h, "status"), "open" | "delivered")).count()}),
        );
        v
    }

    fn hire_view(&self, h: &Value) -> Value {
        let ex = exclusion(h);
        let o = self.offer(u(h, "offer"));
        let left = u(h, "deadline").saturating_sub(self.height);
        let mut v = h.clone();
        let m = v.as_object_mut().unwrap();
        m.insert("offer_title".into(), o.map(|o| o["title"].clone()).unwrap_or(Value::Null));
        m.insert("category".into(), o.map(|o| o["category"].clone()).unwrap_or(Value::Null));
        m.insert("buyer_card".into(), card(s(h, "buyer")));
        m.insert("provider_card".into(), card(s(h, "provider")));
        m.insert("evaluator_cards".into(), Value::Array(h["evaluators"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).map(card).collect()));
        m.insert("house".into(), json!(ex.is_some()));
        m.insert("counts_as_organic".into(), json!(s(h, "status") == "completed" && ex.is_none()));
        m.insert("excluded".into(), ex.unwrap_or(Value::Null));
        m.insert("blocks_left".into(), json!(left));
        m.insert("deadline_hint".into(), json!(hours(left)));
        v
    }

    /// `/v1/market`.
    pub fn summary(&self) -> Value {
        let (mut organic, mut house) = (0u128, 0u128);
        let mut payers = HashSet::new();
        for h in self.hires.iter().filter(|h| s(h, "status") == "completed") {
            if exclusion(h).is_some() {
                house += num(&h["price"]);
            } else {
                organic += num(&h["price"]);
                payers.insert(s(h, "buyer").to_string());
            }
        }
        let by: serde_json::Map<String, Value> = STATUS.iter().map(|st| (st.to_string(), json!(self.hires.iter().filter(|h| s(h, "status") == *st).count()))).collect();
        let mut cats: Vec<String> = self.offers.iter().filter(|o| o["active"] == true).map(|o| s(o, "category").to_string()).filter(|c| !c.is_empty()).collect();
        cats.sort();
        cats.dedup();
        let devnet = self.market == DEVNET_MARKET;
        json!({
            "contract": self.market, "code_hash": if devnet { json!(DEVNET_CODE_HASH) } else { Value::Null }, "deployed_height": if devnet { json!(DEVNET_DEPLOYED_HEIGHT) } else { Value::Null },
            "height": self.height, "config": self.config,
            "fee_rule": "fee = ceil(price x fee_bps / 10000), paid by the buyer on top of the price, kept only when the hire completes (decision D6: floor 2,000 bps, admin-editable)",
            "judge": if devnet { card(DEVNET_JUDGE) } else { Value::Null },
            "offers": self.offers.len(), "active_offers": self.offers.iter().filter(|o| o["active"] == true).count(), "hires": self.hires.len(), "by_status": by,
            "volume": {"organic": organic.to_string(), "organic_distinct_payers": payers.len(), "house": house.to_string(), "self_dealt_excluded": "0"},
            "categories": cats,
            "house_accounts": HOUSE.iter().map(|(a, h)| json!({"account": a, "handle": h})).collect::<Vec<_>>(),
            "note": "House accounts are the market desk and its LLM judge; their hires are house activity and never count as organic volume.",
        })
    }

    /// `/v1/market/offers` (active only unless `all`), sorted by organic paid volume.
    pub fn offers_json(&self, category: Option<&str>, provider: Option<&str>, all: bool) -> Value {
        let mut v: Vec<Value> = self
            .offers
            .iter()
            .filter(|o| (all || o["active"] == true) && category.is_none_or(|c| s(o, "category") == c) && provider.is_none_or(|p| s(o, "provider") == p))
            .map(|o| self.offer_view(o))
            .collect();
        v.sort_by(|a, b| num(&b["record"]["paid_volume_organic"]).cmp(&num(&a["record"]["paid_volume_organic"])).then(u(&b["offer_stats"], "completed").cmp(&u(&a["offer_stats"], "completed"))).then(u(a, "id").cmp(&u(b, "id"))));
        json!({"contract": self.market, "height": self.height, "fee_bps": self.fee_bps, "count": v.len(), "offers": v, "sort_rule": "volume = organic paid volume from distinct payers (house and self-dealt excluded), then completed hires"})
    }

    /// `/v1/market/hires`.
    pub fn hires_json(&self, f: &HireFilter) -> Value {
        let mut list: Vec<&Value> = self.hires.iter().filter(|h| f.admits(h)).collect();
        list.sort_by_key(|h| std::cmp::Reverse(u(h, "id")));
        let count = list.len();
        json!({"contract": self.market, "height": self.height, "count": count, "hires": list.into_iter().take(f.limit).map(|h| self.hire_view(h)).collect::<Vec<_>>()})
    }

    fn hire_events(&self, id: u64) -> Vec<&Value> {
        self.events.iter().filter(|e| e["id"] == json!(id) && !matches!(s(e, "name"), "OfferListed" | "OfferUpdated")).collect()
    }

    /// `/v1/market/offers/:id`.
    pub fn offer_detail(&self, ctx: &Ctx, id: u64) -> Result<Value> {
        let o = self.offer(id).ok_or_else(|| anyhow!("no such offer #{id}"))?;
        let spec = blob(ctx, s(o, "spec_uri"));
        let spec_ok = spec.as_ref().map(|b| hex::encode(cadre_job::data::blob_id(&b.0)) == s(o, "spec_hash"));
        let mut recent: Vec<&Value> = self.hires.iter().filter(|h| u(h, "offer") == id).collect();
        recent.sort_by_key(|h| std::cmp::Reverse(u(h, "id")));
        let quote = decode_quote(&query(ctx, &self.market, method("quote", &[id.to_le_bytes(), 0u64.to_le_bytes()].concat()))?)?;
        Ok(json!({
            "offer": self.offer_view(o),
            "spec": spec.as_ref().map(|b| b.json().unwrap_or_else(|| json!({"text": String::from_utf8_lossy(&b.0)}))), "spec_available": spec.is_some(), "spec_hash_ok": spec_ok,
            "provider_record": provider_record(&self.hires, s(o, "provider")),
            "recent_hires": recent.into_iter().take(25).map(|h| self.hire_view(h)).collect::<Vec<_>>(),
            "quote": quote,
        }))
    }

    /// `/v1/market/hires/:id`.
    pub fn hire_detail(&self, ctx: &Ctx, id: u64) -> Result<Value> {
        let h = self.hires.get(id as usize).ok_or_else(|| anyhow!("no such hire #{id}"))?;
        let ev = self.hire_events(id);
        let req = blob(ctx, s(h, "request_uri"));
        let del = h["result_uri"].as_str().and_then(|x| blob(ctx, x));
        let delivered = ev.iter().find(|e| s(e, "name") == "Delivered");
        let text_of = |b: &Blob, k: &str| -> String { b.json().and_then(|j| j[k].as_str().map(String::from)).unwrap_or_else(|| String::from_utf8_lossy(&b.0).into_owned()) };
        let verdicts: Vec<Value> = ev
            .iter()
            .filter(|e| s(e, "name") == "Verdict")
            .map(|e| {
                let d = &e["data"];
                let vb = d["reason_uri"].as_str().and_then(|x| blob(ctx, x));
                let j = vb.as_ref().and_then(|b| b.json());
                json!({
                    "evaluator": e["who"], "evaluator_card": e["who"].as_str().map(card), "accept": d["accept"], "height": e["height"], "tx_hash": e["tx_hash"],
                    "reason": j.as_ref().map(|j| j["reason"].clone()).unwrap_or(Value::Null), "reasoning_uri": d["reason_uri"], "reasoning": j.clone().unwrap_or(Value::Null),
                    "reason_hash_ok": vb.as_ref().map(|b| hex::encode(cadre_job::data::blob_id(&b.0)) == s(d, "reason_hash")),
                    "thought": d["thought_ref"].as_str().map(|t| json!({"thought_ref": t, "receipt_found": null, "verified": null, "rule": "the gateway receipt check needs cadre-api"})),
                    "judge_kind": if j.as_ref().is_some_and(|j| j["format"] == "cadre-verdict-v1") { "llm_judge" } else { "manual" },
                    "limits": j.as_ref().map(|j| j["limits"].clone()).unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(json!({
            "hire": self.hire_view(h),
            "request": req.as_ref().map(|b| json!({"uri": h["request_uri"], "text": text_of(b, "request"), "archived": false, "hash_ok": hex::encode(cadre_job::data::blob_id(&b.0)) == s(h, "request_hash")})),
            "delivery": del.as_ref().map(|b| json!({"uri": h["result_uri"], "text": text_of(b, "text"), "archived": false, "hash_ok": hex::encode(cadre_job::data::blob_id(&b.0)) == s(h, "result_hash"),
                                                    "thought": delivered.and_then(|d| d["data"]["thought_ref"].as_str()).map(|t| json!({"thought_ref": t, "receipt_found": null}))})),
            "verdicts": verdicts,
            "timeline": ev.iter().map(|e| json!({"event": e["name"], "height": e["height"], "tx_hash": e["tx_hash"], "by": e["who"], "data": e["data"]})).collect::<Vec<_>>(),
        }))
    }

    /// `/v1/market/agents/:account`.
    pub fn agent_json(&self, a: &str) -> Value {
        let by = |pick: &dyn Fn(&Value) -> bool| -> Vec<Value> {
            let mut l: Vec<&Value> = self.hires.iter().filter(|h| pick(h)).collect();
            l.sort_by_key(|h| std::cmp::Reverse(u(h, "id")));
            l.into_iter().take(50).map(|h| self.hire_view(h)).collect()
        };
        let as_buyer = by(&|h| s(h, "buyer") == a);
        let as_provider = by(&|h| s(h, "provider") == a);
        let as_evaluator = by(&|h| h["evaluators"].as_array().is_some_and(|e| e.iter().any(|x| x.as_str() == Some(a))));
        json!({
            "account": a, "card": card(a), "house": house_label(a).is_some(),
            "offers": self.offers.iter().filter(|o| s(o, "provider") == a).map(|o| self.offer_view(o)).collect::<Vec<_>>(),
            "record": provider_record(&self.hires, a), "as_buyer": as_buyer, "as_provider": as_provider, "as_evaluator": as_evaluator,
        })
    }
}

struct Blob(Vec<u8>);
impl Blob {
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.0).ok().filter(|v: &Value| v.is_object())
    }
}

/// A market blob from the node, checked against its id (None when absent or pruned).
fn blob(ctx: &Ctx, uri: &str) -> Option<Blob> {
    let id = uri.strip_prefix("blob:").filter(|h| h.len() == 64)?.to_ascii_lowercase();
    let r = ctx.call("getBlob", json!({"blob_id": uri})).ok()?;
    let data = base64::engine::general_purpose::STANDARD.decode(s(&r, "data")).ok()?;
    (hex::encode(cadre_job::data::blob_id(&data)) == id).then_some(Blob(data))
}

#[derive(Debug, Clone, Default)]
pub struct HireFilter {
    pub buyer: Option<String>,
    pub provider: Option<String>,
    pub evaluator: Option<String>,
    pub status: Option<String>,
    pub limit: usize,
}

impl HireFilter {
    fn admits(&self, h: &Value) -> bool {
        self.buyer.as_deref().is_none_or(|b| s(h, "buyer") == b)
            && self.provider.as_deref().is_none_or(|p| s(h, "provider") == p)
            && self.evaluator.as_deref().is_none_or(|e| h["evaluators"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str() == Some(e))))
            && self.status.as_deref().is_none_or(|st| s(h, "status") == st)
    }
    fn query(&self) -> String {
        let mut q = vec![format!("limit={}", self.limit.clamp(1, 200))];
        for (k, v) in [("buyer", &self.buyer), ("provider", &self.provider), ("evaluator", &self.evaluator), ("status", &self.status)] {
            if let Some(v) = v {
                q.push(format!("{k}={v}"));
            }
        }
        q.join("&")
    }
}

// ---------------------------------------------------------------------------------------
// one reader over both sources

pub struct Reader<'a> {
    ctx: &'a Ctx,
    api: Option<String>,
    ix: Option<Index>,
    /// "cadre-api" or "node".
    pub source: &'static str,
    pub market: String,
    /// Why the node was read when cadre-api was asked first.
    pub note: Option<String>,
}

fn base58(a: &str) -> Result<String> {
    Ok(bs58::encode(crate::fmt::parse_chain_addr(a)?).into_string())
}

impl<'a> Reader<'a> {
    pub fn open(ctx: &'a Ctx, mo: &MarketOpts) -> Result<Reader<'a>> {
        let market = mo.address()?;
        let api = ctx.eps.get("api").map(String::from);
        let mut note = None;
        if mo.source != SourceSel::Node {
            match &api {
                Some(a) => {
                    let h = get(&format!("{a}/market"));
                    if h.ok() && s(&h.json, "contract") == market {
                        return Ok(Reader { ctx, api: Some(a.clone()), ix: None, source: "cadre-api", market, note: None });
                    }
                    let why = if h.ok() { format!("cadre-api serves another market ({})", short_addr(s(&h.json, "contract"))) } else { format!("cadre-api /v1/market: {}", h.why()) };
                    ensure!(mo.source != SourceSel::Api, "{why}");
                    note = Some(format!("{why}; read from the contract on the node"));
                }
                None => {
                    ensure!(mo.source != SourceSel::Api, "no cadre-api on this network (set --endpoint api=URL)");
                    note = Some("no cadre-api on this network; read from the contract on the node".into());
                }
            }
        }
        let ix = read_index(ctx, &market)?;
        Ok(Reader { ctx, api: None, ix: Some(ix), source: "node", market, note })
    }

    fn api_get(&self, path: &str) -> Result<Value> {
        let a = self.api.as_ref().unwrap();
        let h = get(&format!("{a}{path}"));
        ensure!(h.ok(), "cadre-api {path}: {}", h.why());
        Ok(h.json)
    }

    pub fn summary(&self) -> Result<Value> {
        match &self.ix {
            Some(ix) => Ok(ix.summary()),
            None => self.api_get("/market"),
        }
    }
    pub fn offers(&self, category: Option<&str>, provider: Option<&str>, all: bool) -> Result<Value> {
        let provider = provider.map(base58).transpose()?;
        match &self.ix {
            Some(ix) => Ok(ix.offers_json(category, provider.as_deref(), all)),
            None => {
                let mut q = vec![];
                if let Some(c) = category {
                    q.push(format!("category={c}"));
                }
                if let Some(p) = &provider {
                    q.push(format!("provider={p}"));
                }
                if all {
                    q.push("active=all".into());
                }
                self.api_get(&format!("/market/offers?{}", q.join("&")))
            }
        }
    }
    pub fn hires(&self, f: &HireFilter) -> Result<Value> {
        if let Some(st) = &f.status {
            ensure!(STATUS.contains(&st.as_str()), "status is one of {}", STATUS.join(", "));
        }
        let f = HireFilter { buyer: f.buyer.as_deref().map(base58).transpose()?, provider: f.provider.as_deref().map(base58).transpose()?, evaluator: f.evaluator.as_deref().map(base58).transpose()?, ..f.clone() };
        match &self.ix {
            Some(ix) => Ok(ix.hires_json(&f)),
            None => self.api_get(&format!("/market/hires?{}", f.query())),
        }
    }
    pub fn offer(&self, id: u64) -> Result<Value> {
        match &self.ix {
            Some(ix) => ix.offer_detail(self.ctx, id),
            None => self.api_get(&format!("/market/offers/{id}")),
        }
    }
    pub fn hire(&self, id: u64) -> Result<Value> {
        match &self.ix {
            Some(ix) => ix.hire_detail(self.ctx, id),
            None => self.api_get(&format!("/market/hires/{id}")),
        }
    }
    pub fn agent(&self, a: &str) -> Result<Value> {
        let a = base58(a)?;
        match &self.ix {
            Some(ix) => Ok(ix.agent_json(&a)),
            None => self.api_get(&format!("/market/agents/{a}")),
        }
    }
    fn source_line(&self) -> String {
        match (self.source, &self.note) {
            ("node", Some(n)) => format!("node (contract state + events): {n}"),
            ("node", None) => "node (contract state + events)".into(),
            _ => format!("cadre-api {}", self.api.as_deref().unwrap_or("")),
        }
    }
}

// ---------------------------------------------------------------------------------------
// words

/// `@handle (house)` or the short Cx.
fn who(c: &Value, a: &str) -> String {
    let name = match c["handle"].as_str() {
        Some(h) if !h.is_empty() => format!("@{h}"),
        _ => short_addr(a),
    };
    if c["house"] == true {
        format!("{name} (house)")
    } else {
        name
    }
}
fn who_full(c: &Value, a: &str) -> String {
    match c["handle"].as_str() {
        Some(h) if !h.is_empty() => format!("@{h}{} {}", if c["house"] == true { " (house)" } else { "" }, cx_str(a)),
        _ => cx_str(a),
    }
}
fn evals_words(v: &Value) -> Vec<String> {
    let cards = v["evaluator_cards"].as_array().cloned().unwrap_or_default();
    v["evaluators"].as_array().into_iter().flatten().enumerate().filter_map(|(i, a)| a.as_str().map(|a| who(cards.get(i).unwrap_or(&Value::Null), a))).collect()
}

fn summary_panel(r: &Reader, j: &Value) -> Panel {
    let c = &j["config"];
    let mut p = Panel::new("Services market")
        .kv("Contract", cx_str(s(j, "contract")))
        .kv("Source", r.source_line())
        .kv("Height", format!("{}", u(j, "height")));
    if !c.is_null() {
        let bps = u(c, "fee_bps");
        p = p.kv("Fee", format!("{} bps ({}%), paid by the buyer on top of the price, kept only when a hire completes", bps, pct(bps)));
        if c["hires_paused"] == true {
            p = p.kt("New hires", "PAUSED by the market admin", Tone::Warn);
        }
    }
    p = p.kv("Offers", format!("{} ({} active)", u(j, "offers"), u(j, "active_offers")));
    let by = &j["by_status"];
    let parts: Vec<String> = STATUS.iter().filter(|st| u(by, st) > 0).map(|st| format!("{} {st}", u(by, st))).collect();
    p = p.kv("Hires", if parts.is_empty() { format!("{}", u(j, "hires")) } else { format!("{}: {}", u(j, "hires"), parts.join(", ")) });
    let v = &j["volume"];
    p = p.kv(
        "Paid volume",
        format!("organic {} CADRE from {} payer(s); house {} CADRE (not organic); self-dealt {} CADRE excluded", cad_s(&v["organic"]), u(v, "organic_distinct_payers"), cad_s(&v["house"]), cad_s(&v["self_dealt_excluded"])),
    );
    if let Some(a) = j["judge"]["account"].as_str() {
        p = p.kv("House LLM judge", who_full(&j["judge"], a));
    }
    if let Some(cats) = j["categories"].as_array().filter(|c| !c.is_empty()) {
        p = p.kv("Categories", cats.iter().filter_map(|c| c.as_str()).collect::<Vec<_>>().join(", "));
    }
    p
}

pub fn offers_table(list: &[Value]) -> Table {
    Table {
        chip: Some("Offers, by organic paid volume".into()),
        head: ["ID", "TITLE", "PROVIDER", "PRICE", "FEE", "TURNAROUND", "ORGANIC PAID", "PAYERS", "CATEGORY"].map(String::from).to_vec(),
        right: vec![true, false, false, true, true, false, true, true, false],
        rows: list
            .iter()
            .map(|o| {
                let house = o["provider_card"]["house"] == true;
                vec![
                    (format!("#{}", u(o, "id")), Tone::Dim),
                    (short_n(s(o, "title"), 18), if o["active"] == true { Tone::Plain } else { Tone::Dim }),
                    (who(&o["provider_card"], s(o, "provider")), if house { Tone::Dim } else { Tone::Plain }),
                    (format!("{} CADRE", cad_s(&o["price"])), Tone::Plain),
                    (if o["fee"].is_null() { "-".into() } else { cad_s(&o["fee"]) }, Tone::Dim),
                    (format!("{} blocks", u(o, "turnaround")), Tone::Plain),
                    (cad_s(&o["record"]["paid_volume_organic"]), Tone::Plain),
                    (u(&o["record"], "distinct_payers").to_string(), Tone::Plain),
                    (s(o, "category").to_string(), Tone::Dim),
                ]
            })
            .collect(),
        empty: "no offers listed yet".into(),
    }
}

fn status_cell(st: &str) -> (String, Tone) {
    let t = match st {
        "completed" => Tone::Good,
        "open" | "delivered" => Tone::Warn,
        "rejected" | "expired" => Tone::Bad,
        _ => Tone::Dim,
    };
    (st.to_string(), t)
}

pub fn hires_table(chip: &str, list: &[Value]) -> Table {
    Table {
        chip: Some(chip.into()),
        head: ["ID", "OFFER", "BUYER", "PROVIDER", "PAID IN", "STATUS", "WHEN", "EVALUATORS"].map(String::from).to_vec(),
        right: vec![true, false, false, false, true, false, false, false],
        rows: list
            .iter()
            .map(|h| {
                let when = match s(h, "status") {
                    "open" | "delivered" => format!("deadline #{} ({} left)", u(h, "deadline"), u(h, "blocks_left")),
                    _ if !h["closed"].is_null() => format!("closed #{}", u(h, "closed")),
                    _ => format!("deadline #{}", u(h, "deadline")),
                };
                vec![
                    (format!("#{}", u(h, "id")), Tone::Dim),
                    (format!("#{} {}", u(h, "offer"), short_n(h["offer_title"].as_str().unwrap_or(""), 14)), Tone::Plain),
                    (who(&h["buyer_card"], s(h, "buyer")), Tone::Plain),
                    (who(&h["provider_card"], s(h, "provider")), Tone::Plain),
                    (format!("{} CADRE", cad_s(&h["total"])), Tone::Plain),
                    status_cell(s(h, "status")),
                    (when, Tone::Dim),
                    (format!("{} ({} of {})", evals_words(h).join(", "), u(h, "threshold"), h["evaluators"].as_array().map(|a| a.len()).unwrap_or(0)), Tone::Dim),
                ]
            })
            .collect(),
        empty: "no hires".into(),
    }
}

fn record_panel(rec: &Value) -> Panel {
    let mut p = Panel::new("Provider record")
        .kv("Organic paid volume", format!("{} CADRE from {} distinct payer(s)", cad_s(&rec["paid_volume_organic"]), u(rec, "distinct_payers")))
        .kv("Top payer share", rec["top_payer_share_bps"].as_u64().map(|b| format!("{}%", pct(b))).unwrap_or_else(|| "-".into()))
        .kv("House volume", format!("{} CADRE (shown apart, not organic)", cad_s(&rec["house_volume"])))
        .kv("Self-dealt, excluded", format!("{} CADRE", cad_s(&rec["self_dealt_excluded"])))
        .kv(
            "Hires",
            format!(
                "{}: {} completed, {} rejected, {} delivered then expired, {} expired undelivered, {} declined",
                u(rec, "hires"),
                u(rec, "completed"),
                u(rec, "rejected"),
                u(rec, "delivered_then_expired"),
                u(rec, "expired_undelivered"),
                u(rec, "declined")
            ),
        )
        .kv("Independent evaluation", rec["independent_evaluation_share_bps"].as_u64().map(|b| format!("{}% of completed hires judged by someone other than the buyer", pct(b))).unwrap_or_else(|| "-".into()))
        .kv("Median turnaround", rec["median_turnaround_blocks"].as_u64().map(|b| format!("{b} blocks ({})", hours(b))).unwrap_or_else(|| "-".into()));
    if let Some(b) = rec["basis"].as_str() {
        p = p.kt("Basis", b, Tone::Dim);
    }
    p
}

/// Text as rows; `wrap` breaks long lines at 96 columns (plain text mode, whose boxes do not
/// wrap; the app's cards wrap to their own width).
fn text_rows(mut p: Panel, t: &str, max_lines: usize, wrap: bool) -> Panel {
    let lines: Vec<String> = t.lines().flat_map(|l| if l.is_empty() || !wrap { vec![l.to_string()] } else { crate::entry::draw::wrap(l, 96) }).collect();
    for l in lines.iter().take(max_lines) {
        p = p.text(l.clone(), Tone::Plain);
    }
    if lines.len() > max_lines {
        p = p.text(format!("... {} more lines (--json has all of it)", lines.len() - max_lines), Tone::Dim);
    }
    p
}

fn ok_word(v: &Value) -> &'static str {
    match v.as_bool() {
        Some(true) => "matches the hash on chain",
        Some(false) => "DOES NOT match the hash on chain",
        None => "-",
    }
}

fn spec_panel(d: &Value) -> Panel {
    let mut p = Panel::new("Spec (the offer's blob)");
    if d["spec_available"] != true {
        return p.kt("Spec", "not available on the node (pruned or never uploaded)", Tone::Dim);
    }
    p = p.kv("Hash", ok_word(&d["spec_hash_ok"]));
    if let Some(m) = d["spec"].as_object() {
        for (k, v) in m {
            let val = match v {
                Value::String(x) => x.clone(),
                Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join("; "),
                Value::Null => continue,
                x => x.to_string(),
            };
            let val = if k == "provider" { cx_str(&val) } else { val };
            p = p.kv(k, short_n(&val, 400));
        }
    }
    p
}

fn offer_panel(o: &Value) -> Panel {
    let price = num(&o["price"]);
    let fee = num(&o["fee"]);
    let mut p = Panel::new(&format!("Offer #{}", u(o, "id")))
        .kv("Title", s(o, "title"))
        .kv("Category", s(o, "category"))
        .kv("Provider", who_full(&o["provider_card"], s(o, "provider")))
        .kv("Price", format!("{} CADRE", cad(price)))
        .kv("Market fee", format!("{} CADRE ({} bps), paid by the buyer, kept only on acceptance", cad(fee), u(o, "fee_bps")))
        .kv("Buyer pays", format!("{} CADRE plus any judge fee, into escrow", cad(price + fee)))
        .kv("Turnaround", format!("{} blocks ({})", u(o, "turnaround"), o["turnaround_hint"].as_str().map(String::from).unwrap_or_else(|| hours(u(o, "turnaround")))))
        .kv(
            "Evaluators",
            if s(o, "policy") == "offer_list" { format!("buyers pick from: {}", evals_words(o).join(", ")) } else { "the buyer names them (itself, a person, an agent or an LLM judge)".into() },
        )
        .state("State", if o["active"] == true { "active" } else { "paused" })
        .kv("Listed", format!("block {} (updated {})", u(o, "created"), u(o, "updated")));
    let st = &o["offer_stats"];
    if st.is_object() {
        p = p.kv("Hires of this offer", format!("{} ({} completed, {} open)", u(st, "hires"), u(st, "completed"), u(st, "open")));
    }
    p.kv("Spec blob", s(o, "spec_uri"))
}

fn hire_panel(h: &Value) -> Panel {
    let ex = &h["excluded"];
    let n = h["evaluators"].as_array().map(|a| a.len()).unwrap_or(0);
    let mut p = Panel::new(&format!("Hire #{}", u(h, "id")))
        .kv("Offer", format!("#{} {}", u(h, "offer"), h["offer_title"].as_str().unwrap_or("")))
        .state("Status", s(h, "status"))
        .kv("Buyer", who_full(&h["buyer_card"], s(h, "buyer")))
        .kv("Provider", who_full(&h["provider_card"], s(h, "provider")))
        .kv("Evaluators", format!("{}; {} of {n} must accept (accepts {}, rejects {})", evals_words(h).join(", "), u(h, "threshold"), u(h, "accepts"), u(h, "rejects")))
        .kv("Paid into escrow", format!("{} CADRE = price {} + market fee {} + judge fee {}", cad_s(&h["total"]), cad_s(&h["price"]), cad_s(&h["fee"]), cad_s(&h["judge_fee"])))
        .kv("Hired at", format!("block {}", u(h, "created")));
    p = match s(h, "status") {
        "open" | "delivered" => p.kv("Deadline", format!("block {} ({} blocks left, {}); after it anyone can refund the buyer", u(h, "deadline"), u(h, "blocks_left"), s(h, "deadline_hint"))),
        _ => p.kv("Deadline", format!("block {}", u(h, "deadline"))),
    };
    if !h["delivered"].is_null() {
        p = p.kv("Delivered at", format!("block {}", u(h, "delivered")));
    }
    if !h["closed"].is_null() {
        p = p.kv("Closed at", format!("block {}", u(h, "closed")));
    }
    let money = match s(h, "status") {
        "completed" => format!("{} CADRE to the provider, {} CADRE fee to the market, {} CADRE to the accepting evaluators", cad_s(&h["price"]), cad_s(&h["fee"]), cad_s(&h["judge_fee"])),
        "rejected" => format!("{} CADRE back to the buyer, {} CADRE to the rejecting evaluators", cad(num(&h["price"]) + num(&h["fee"])), cad_s(&h["judge_fee"])),
        "expired" | "declined" => format!("all {} CADRE back to the buyer", cad_s(&h["total"])),
        _ => "held by the market contract until the hire closes".into(),
    };
    p = p.kv("Money", money);
    p = if h["counts_as_organic"] == true {
        p.kt("Organic", "yes: counts toward the provider's paid volume", Tone::Good)
    } else if ex.is_object() {
        p.kt("Organic", format!("no: {}", s(ex, "reason")), Tone::Dim)
    } else if matches!(s(h, "status"), "open" | "delivered") {
        p.kt("Organic", "not yet (only completed hires count)", Tone::Dim)
    } else {
        p.kt("Organic", "no: only completed hires count", Tone::Dim)
    };
    p
}

fn event_words(e: &Value) -> String {
    let d = &e["data"];
    match s(e, "event") {
        "Hired" => format!("paid {} CADRE into escrow (price {}, fee {}, judge fee {}), deadline block {}", cad(num(&d["price"]) + num(&d["fee"]) + num(&d["judge_fee"])), cad_s(&d["price"]), cad_s(&d["fee"]), cad_s(&d["judge_fee"]), u(d, "deadline")),
        "Delivered" => format!("delivered {}{}", short_n(s(d, "result_uri"), 24), d["thought_ref"].as_str().map(|t| format!(", thought {t}")).unwrap_or_default()),
        "Verdict" => format!("{}{}", if d["accept"] == true { "accept" } else { "reject" }, d["thought_ref"].as_str().map(|t| format!(", thought {t}")).unwrap_or_default()),
        "Completed" => format!("{} CADRE to the provider, {} CADRE fee, {} CADRE judge fee", cad_s(&d["price"]), cad_s(&d["fee"]), cad_s(&d["judge_fee"])),
        "Rejected" => format!("{} CADRE back to the buyer", cad_s(&d["refund"])),
        "Declined" => format!("{} CADRE back to the buyer: {}", cad_s(&d["refund"]), s(d, "reason")),
        "Expired" => format!("{} CADRE back to the buyer ({})", cad_s(&d["refund"]), if d["delivered"] == true { "delivered, not decided" } else { "nothing delivered" }),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------------------
// commands

pub fn exec_market(ctx: &Ctx, sub: &Option<MarketCmd>, list: &ListArgs) -> Result<Out> {
    match sub {
        None => market_list(ctx, list),
        Some(MarketCmd::Offer { id, mo }) => offer_detail(ctx, &mo.or(&list.mo), *id),
        Some(MarketCmd::Hire { id, mo }) => hire_detail(ctx, &mo.or(&list.mo), *id),
        Some(MarketCmd::Hires { buyer, provider, evaluator, status, limit, mo }) => {
            let f = HireFilter { buyer: buyer.clone(), provider: provider.clone(), evaluator: evaluator.clone(), status: status.clone(), limit: *limit };
            hires_list(ctx, &mo.or(&list.mo), &f)
        }
        Some(MarketCmd::Agent { account, mo }) => agent(ctx, &mo.or(&list.mo), account),
    }
}

pub fn market_list(ctx: &Ctx, a: &ListArgs) -> Result<Out> {
    let st = ctx.ui.step("Reading the services market");
    let r = (|| -> Result<(Reader, Value, Value)> {
        let r = Reader::open(ctx, &a.mo)?;
        let sum = r.summary()?;
        let offers = r.offers(a.category.as_deref(), a.provider.as_deref(), a.all)?;
        Ok((r, sum, offers))
    })();
    let (r, sum, offers) = st.wrap(r, |(r, _, o)| format!("Read {} offer(s) from {}", u(o, "count"), r.source))?;
    let list = offers["offers"].as_array().cloned().unwrap_or_default();
    let view = View::panel(summary_panel(&r, &sum))
        .with(Block::Table(offers_table(&list)))
        .hint("`cadre offer <id>` shows an offer, `cadre hire --offer <id> --request TEXT --evaluator ADDR` builds a hire; house accounts are labelled and never count as organic volume");
    let text = format!("{} offers, {} hires ({})", u(&sum, "offers"), u(&sum, "hires"), r.source);
    Ok(Out::new(json!({"source": r.source, "note": r.note, "market": r.market, "summary": sum, "offers": list, "count": offers["count"]}), text).view(view))
}

pub fn offer_detail(ctx: &Ctx, mo: &MarketOpts, id: u64) -> Result<Out> {
    let st = ctx.ui.step(format!("Reading offer #{id}"));
    let r = Reader::open(ctx, mo).and_then(|r| r.offer(id).map(|d| (r, d)));
    let (r, d) = st.wrap(r, |(r, _)| format!("Read from {}", r.source))?;
    let o = &d["offer"];
    let q = &d["quote"];
    let mut op = offer_panel(o);
    if q.is_object() {
        op = op.kv("Quote now", format!("{} CADRE total with no judge fee (price {} + fee {})", cad_s(&q["total"]), cad_s(&q["price"]), cad_s(&q["fee"])));
    }
    let recent = d["recent_hires"].as_array().cloned().unwrap_or_default();
    let view = View::panel(op)
        .with(Block::Panel(spec_panel(&d)))
        .with(Block::Panel(record_panel(&d["provider_record"])))
        .with(Block::Table(hires_table("Recent hires of this offer", &recent)))
        .hint(format!("hire it: cadre hire --offer {id} --request \"...\" --evaluator <Cx> [--judge-fee X] [--deadline N]  (prints the review; --send with --key signs)"));
    let mut j = d.clone();
    j["source"] = json!(r.source);
    Ok(Out::new(j, format!("offer #{id} {} {} CADRE", s(o, "title"), cad_s(&o["price"]))).view(view))
}

pub fn hire_detail(ctx: &Ctx, mo: &MarketOpts, id: u64) -> Result<Out> {
    let st = ctx.ui.step(format!("Reading hire #{id}"));
    let r = Reader::open(ctx, mo).and_then(|r| r.hire(id).map(|d| (r, d)));
    let (r, d) = st.wrap(r, |(r, _)| format!("Read from {}", r.source))?;
    let view = hire_view_blocks(&d, ctx.ui.is_plain());
    let mut j = d.clone();
    j["source"] = json!(r.source);
    Ok(Out::new(j, format!("hire #{id} {}", s(&d["hire"], "status"))).view(view))
}

fn hire_view_blocks(d: &Value, wrap: bool) -> View {
    let h = &d["hire"];
    let mut v = View::panel(hire_panel(h));
    let mut rq = Panel::new("Request");
    rq = match d["request"].as_object() {
        Some(_) => text_rows(rq.kv("Blob", s(&d["request"], "uri")).kv("Hash", ok_word(&d["request"]["hash_ok"])), s(&d["request"], "text"), 20, wrap),
        None => rq.kt("Text", format!("{} not available on the node (pruned or never uploaded)", s(h, "request_uri")), Tone::Dim),
    };
    v = v.with(Block::Panel(rq));
    let del = &d["delivery"];
    if del.is_object() {
        let mut p = Panel::new("Delivery").kv("Blob", s(del, "uri")).kv("Hash", ok_word(&del["hash_ok"]));
        if let Some(t) = del["thought"]["thought_ref"].as_str() {
            let verified = match del["thought"]["verified"].as_bool() {
                Some(true) => " (gateway receipt matches this text)",
                Some(false) => " (gateway receipt does not match)",
                None => "",
            };
            p = p.kv("Thought ref", format!("{t}{verified}"));
        }
        v = v.with(Block::Panel(text_rows(p, s(del, "text"), 30, wrap)));
    } else if !h["result_uri"].is_null() {
        v = v.with(Block::Panel(Panel::new("Delivery").kt("Text", format!("{} not available on the node", s(h, "result_uri")), Tone::Dim)));
    }
    let verdicts = d["verdicts"].as_array().cloned().unwrap_or_default();
    if !verdicts.is_empty() {
        let mut p = Panel::new("Verdicts");
        for x in &verdicts {
            let ev = s(x, "evaluator");
            p = p.section(&format!("{} at block {}", who(&x["evaluator_card"], ev), u(x, "height")));
            p = p.kt("Verdict", if x["accept"] == true { "ACCEPT" } else { "REJECT" }, if x["accept"] == true { Tone::Good } else { Tone::Bad });
            p = p.kv("Judge", if s(x, "judge_kind") == "llm_judge" { "LLM judge (cadre-verdict-v1)" } else { "manual" });
            p = match x["reason"].as_str() {
                Some(r) => text_rows(p.kt("Reason", "", Tone::Dim), r, 12, wrap),
                None => p.kv("Reason", "-"),
            };
            if let Some(t) = x["thought"]["thought_ref"].as_str() {
                p = p.kv("Thought ref", t);
            }
            if let Some(u_) = x["reasoning_uri"].as_str() {
                p = p.kv("Reasoning blob", format!("{} ({})", short_n(u_, 24), ok_word(&x["reason_hash_ok"])));
            }
        }
        v = v.with(Block::Panel(p));
    }
    let tl = d["timeline"].as_array().cloned().unwrap_or_default();
    // handles of the parties for the BY column
    let mut names: HashMap<String, String> = HashMap::new();
    for (a, c) in [(s(h, "buyer"), &h["buyer_card"]), (s(h, "provider"), &h["provider_card"])] {
        names.insert(a.to_string(), who(c, a));
    }
    for (i, a) in h["evaluators"].as_array().into_iter().flatten().enumerate() {
        if let Some(a) = a.as_str() {
            names.insert(a.to_string(), who(&h["evaluator_cards"][i], a));
        }
    }
    v.with(Block::Table(Table {
        chip: Some("Timeline".into()),
        head: ["BLOCK", "EVENT", "BY", "WHAT", "TX"].map(String::from).to_vec(),
        right: vec![true, false, false, false, false],
        rows: tl.iter().map(|e| vec![(u(e, "height").to_string(), Tone::Dim), (s(e, "event").to_string(), Tone::Plain), (names.get(s(e, "by")).cloned().unwrap_or_else(|| short_addr(s(e, "by"))), Tone::Plain), (event_words(e), Tone::Plain), (crate::fmt::short(s(e, "tx_hash")), Tone::Dim)]).collect(),
        empty: "no events for this hire".into(),
    }))
}

pub fn hires_list(ctx: &Ctx, mo: &MarketOpts, f: &HireFilter) -> Result<Out> {
    let st = ctx.ui.step("Reading hires");
    let r = Reader::open(ctx, mo).and_then(|r| r.hires(f).map(|d| (r, d)));
    let (r, d) = st.wrap(r, |(r, d)| format!("Read {} hire(s) from {}", u(d, "count"), r.source))?;
    let list = d["hires"].as_array().cloned().unwrap_or_default();
    let view = View(vec![Block::Table(hires_table("Hires, newest first", &list))]).hint("`cadre hire <id>` shows one hire: timeline, request, delivery and verdicts with reasons");
    let mut j = d.clone();
    j["source"] = json!(r.source);
    Ok(Out::new(j, format!("{} hires", u(&d, "count"))).view(view))
}

pub fn agent(ctx: &Ctx, mo: &MarketOpts, account: &str) -> Result<Out> {
    let st = ctx.ui.step("Reading the account's market record");
    let r = Reader::open(ctx, mo).and_then(|r| r.agent(account).map(|d| (r, d)));
    let (r, d) = st.wrap(r, |(r, _)| format!("Read from {}", r.source))?;
    let offers = d["offers"].as_array().cloned().unwrap_or_default();
    let mut all: Vec<Value> = vec![];
    for k in ["as_buyer", "as_provider", "as_evaluator"] {
        for h in d[k].as_array().into_iter().flatten() {
            if !all.iter().any(|x| x["id"] == h["id"]) {
                all.push(h.clone());
            }
        }
    }
    all.sort_by_key(|h| std::cmp::Reverse(u(h, "id")));
    let mut rec = record_panel(&d["record"]);
    rec.chip = Some(format!("Market record of {}", who(&d["card"], s(&d, "account"))));
    let view = View::panel(rec).with(Block::Table(offers_table(&offers))).with(Block::Table(hires_table("Hires (buyer, provider or evaluator)", &all)));
    let mut j = d.clone();
    j["source"] = json!(r.source);
    Ok(Out::new(j, format!("{} offers, {} hires", offers.len(), all.len())).view(view))
}

/// The offer as the contract holds it now (builders read the node, never a cache).
fn live_offer(ctx: &Ctx, market: &str, id: u64) -> Result<Value> {
    decode_offer(id, &query(ctx, market, method("offer", &id.to_le_bytes())).map_err(|e| anyhow!("offer #{id}: {e}"))?)
}

fn name_of(a: &str) -> String {
    match house_label(a) {
        Some(h) => format!("@{h} (house)"),
        None => short_addr(a),
    }
}

fn put_blob(ctx: &Ctx, data: &[u8]) -> Result<String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let r = ctx.call("putBlob", json!({"data": b64, "push": true}))?;
    Ok(r["uri"].as_str().map(String::from).unwrap_or_else(|| cadre_job::data::blob_uri(&cadre_job::data::blob_id(data))))
}

/// Simulate the call from the connected address (no key needed), or None without one.
fn simulate(ctx: &Ctx, market: &str, args: &[u8], value: u64) -> Option<Value> {
    let me = ctx.me.as_ref().map(crate::solana::b58)?;
    ctx.call("simulateCall", json!({"from": me, "address": market, "args": hex::encode(args), "value": value})).ok()
}

/// Review, call, and either the command to run or (with --send and a key) the sent call.
#[allow(clippy::too_many_arguments)]
fn finish_build(ctx: &Ctx, market: &str, mname: &str, typed: Vec<String>, value: u64, blob_kind: &str, blob_bytes: Vec<u8>, review: Vec<String>, send: bool, extra: Value) -> Result<Out> {
    let args = crate::contract::encode_call(mname, &typed)?;
    let uri = cadre_job::data::blob_uri(&cadre_job::data::blob_id(&blob_bytes));
    let mut cmd: Vec<String> = vec!["cadre".into(), "contract".into(), "call".into(), crate::fmt::cx_str(market), mname.into()];
    cmd.extend(typed.iter().map(|t| shell_word(t)));
    if value > 0 {
        cmd.push(format!("--value {}", cadre_arg(value)));
    }
    cmd.push("--key FILE".into());
    let command = cmd.join(" ");
    let rp = text_rows(Panel::new("Review: what this does"), &review.join("\n"), 60, ctx.ui.is_plain());
    let mut cp = Panel::new("Call").kv("Contract", cx_str(market)).kv("Method", mname).kv("Value", format!("{} CADRE", cad(value as u128))).kv("Args", format!("{} bytes (hex in --json)", args.len()));
    for (i, t) in typed.iter().enumerate() {
        cp = cp.kv(&format!("arg {}", i + 1), t.clone());
    }
    let bp = Panel::new(&format!("{blob_kind} blob")).kv("URI", uri.as_str()).kv("Size", format!("{} bytes", blob_bytes.len())).kv("JSON", short_n(&String::from_utf8_lossy(&blob_bytes), 100));
    let mut j = json!({"review": review, "review_text": review.join(" "), "contract": market, "method": mname, "value": value.to_string(), "args": hex::encode(&args), "typed_args": typed,
                       "blob": {"uri": uri, "json": serde_json::from_slice::<Value>(&blob_bytes).unwrap_or(Value::Null)}, "command": command});
    if let (Some(o), Some(e)) = (j.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    if !send {
        let sim = simulate(ctx, market, &args, value);
        if let Some(sm) = &sim {
            let ok = sm["ok"] == json!(true);
            cp = cp.kt("Simulated", if ok { format!("ok from your address, {} gas", s(sm, "gas_used")) } else { format!("would fail: {}", err_text(s(sm, "error"))) }, if ok { Tone::Good } else { Tone::Bad });
            j["simulation"] = sm.clone();
        }
        let view = View::panel(rp).with(Block::Panel(cp)).with(Block::Panel(bp)).hint(format!(
            "nothing was signed or sent. To send: add --send and --key FILE (uploads the {} blob, then signs this call through `cadre contract call`), or upload the blob yourself and run:\n{command}",
            blob_kind.to_lowercase()
        ));
        return Ok(Out::new(j, review.join(" ")).view(view));
    }
    ensure!(ctx.has_key(), "--send signs with your key: pass --key FILE (or set CADRE_KEY); nothing was sent");
    let st = ctx.ui.step(format!("Uploading the {} blob", blob_kind.to_lowercase()));
    let put = st.wrap(put_blob(ctx, &blob_bytes), |u| format!("Stored {u}"))?;
    ensure!(put == uri, "the node stored the blob as {put}, expected {uri}; nothing was sent");
    let call = crate::contract::ContractCmd::Call { address: market.to_string(), method: mname.into(), args: typed, value: cadre_arg(value), gas: None, dry_run: false, decode: Some("u64".into()) };
    let out = crate::contract::exec(ctx, &call)?;
    j["sent"] = out.json.clone();
    let mut view = View::panel(rp).with(Block::Panel(bp));
    view.0.extend(out.view.0);
    if let Some(id) = out.json["decoded"][0].as_str() {
        view = view.hint(format!("{} #{id}: cadre {} {id}", if mname == "hire" { "hire" } else { "offer" }, if mname == "hire" { "hire" } else { "offer" }));
        j["id"] = json!(id);
    }
    Ok(Out::new(j, out.text).view(view))
}

pub fn hire_cmd(ctx: &Ctx, a: &HireArgs) -> Result<Out> {
    if let Some(id) = a.id {
        ensure!(a.offer.is_none() && a.request.is_none(), "`cadre hire <id>` shows a hire; to build one: cadre hire --offer <id> --request TEXT --evaluator ADDR");
        return hire_detail(ctx, &a.mo, id);
    }
    let oid = a.offer.ok_or_else(|| anyhow!("cadre hire <id> shows a hire; cadre hire --offer <id> --request TEXT --evaluator ADDR builds one"))?;
    let request = a.request.as_deref().ok_or_else(|| anyhow!("--request TEXT is required (what you ask the provider for)"))?;
    ensure!(!a.evaluators.is_empty(), "name at least one --evaluator (a person, an agent or an LLM judge such as the house judge {})", cx_str(DEVNET_JUDGE));
    let market = a.mo.address()?;
    let st = ctx.ui.step(format!("Reading offer #{oid} and the quote from the contract"));
    let r = (|| -> Result<(Value, Value, Value)> {
        let o = live_offer(ctx, &market, oid)?;
        let cfg = decode_config(&query(ctx, &market, method("config", &[]))?)?;
        let jf = crate::contract::cadre(&a.judge_fee)?;
        let q = decode_quote(&query(ctx, &market, method("quote", &[oid.to_le_bytes(), jf.to_le_bytes()].concat()))?)?;
        Ok((o, cfg, q))
    })();
    let (o, cfg, q) = st.wrap(r, |(o, _, q)| format!("Offer #{oid} \"{}\": {} CADRE total", s(o, "title"), cad_s(&q["total"])))?;
    let r = build_hire(&market, &o, &cfg, &q, a, ctx.me.as_ref().map(crate::solana::b58).as_deref(), request)?;
    let extra = json!({"offer": o, "quote": q});
    finish_build(ctx, &market, "hire", r.0.typed(), num(&q["total"]) as u64, "Request", r.1, r.2, a.send, extra)
}

/// The hire's call, its request blob and the review (checks as cadre-api's builder).
fn build_hire(market: &str, o: &Value, cfg: &Value, q: &Value, a: &HireArgs, me: Option<&str>, request: &str) -> Result<(HireCall, Vec<u8>, Vec<String>)> {
    ensure!(o["active"] == true, "offer #{} is not active", u(o, "id"));
    ensure!(cfg["hires_paused"] != true, "new hires are paused by the market admin");
    let provider = s(o, "provider");
    ensure!(me != Some(provider), "a provider cannot hire its own offer");
    let mut evs: Vec<[u8; 32]> = vec![];
    for e in &a.evaluators {
        let k = crate::fmt::parse_chain_addr(e)?;
        ensure!(!evs.contains(&k), "evaluators must be distinct");
        evs.push(k);
    }
    ensure!(evs.len() <= MAX_EVAL, "at most 4 evaluators");
    let ev58: Vec<String> = evs.iter().map(|k| bs58::encode(k).into_string()).collect();
    ensure!(!ev58.iter().any(|e| e == provider), "the provider cannot judge its own work");
    if s(o, "policy") == "offer_list" {
        let allowed: Vec<&str> = o["evaluators"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
        ensure!(ev58.iter().all(|e| allowed.contains(&e.as_str())), "this offer accepts only evaluators from its list: {}", allowed.iter().map(|x| cx_str(x)).collect::<Vec<_>>().join(", "));
    }
    ensure!(a.threshold >= 1 && a.threshold as usize <= evs.len(), "--threshold must be 1..={}", evs.len());
    let turnaround = u(o, "turnaround");
    let deadline = a.deadline.unwrap_or(turnaround * 2);
    ensure!(deadline >= turnaround && deadline <= MAX_DEADLINE, "--deadline must be {turnaround}..={MAX_DEADLINE} blocks (at least the offer's turnaround)");
    let blob = serde_json::to_vec(&json!({"format": "cadre-request-v1", "offer": u(o, "id"), "title": s(o, "title"), "buyer": me, "request": request}))?;
    let id = cadre_job::data::blob_id(&blob);
    let uri = cadre_job::data::blob_uri(&id);
    ensure!(uri.len() <= MAX_URI, "request_uri over 256 bytes");
    let call = HireCall { offer: u(o, "id"), price: num(&q["price"]) as u64, request_uri: uri, request_hash: id, deadline_blocks: deadline, judge_fee: num(&q["judge_fee"]) as u64, evaluators: evs, threshold: a.threshold };
    let review = hire_review(market, &name_of(provider), &ev58.iter().map(|e| name_of(e)).collect::<Vec<_>>(), a.threshold, deadline, call.price, num(&q["fee"]) as u64, call.judge_fee, u(cfg, "fee_bps") as u16);
    Ok((call, blob, review))
}

pub fn offer_cmd(ctx: &Ctx, a: &OfferArgs) -> Result<Out> {
    if let Some(id) = a.id {
        ensure!(a.title.is_none(), "`cadre offer <id>` shows an offer; to list one: cadre offer --title T --price X --turnaround N");
        return offer_detail(ctx, &a.mo, id);
    }
    let title = a.title.as_deref().ok_or_else(|| anyhow!("cadre offer <id> shows an offer; cadre offer --title T --price X --turnaround N [--category C] lists one"))?;
    ensure!(!title.is_empty() && title.len() <= MAX_TITLE, "title is 1..=96 bytes");
    ensure!(a.category.len() <= MAX_CATEGORY, "category over 32 bytes");
    let price = crate::contract::cadre(a.price.as_deref().ok_or_else(|| anyhow!("--price X (CADRE) is required"))?)?;
    ensure!(price > 0, "price must be above 0");
    let turnaround = a.turnaround.ok_or_else(|| anyhow!("--turnaround N (blocks) is required"))?;
    ensure!((1..=MAX_TURNAROUND).contains(&turnaround), "turnaround must be 1..=100800 blocks");
    let policy: u8 = match a.policy.as_str() {
        "buyer" | "0" | "buyer_names" => 0,
        "list" | "1" | "offer_list" => 1,
        p => bail!("--policy is buyer or list, not {p:?}"),
    };
    let mut evs: Vec<[u8; 32]> = vec![];
    for e in &a.evaluators {
        let k = crate::fmt::parse_chain_addr(e)?;
        ensure!(!evs.contains(&k), "evaluators must be distinct");
        evs.push(k);
    }
    ensure!(evs.len() <= MAX_EVAL, "at most 4 evaluators");
    ensure!(policy == 0 || !evs.is_empty(), "--policy list needs the --evaluator list");
    let me = ctx.me.map(|k| bs58::encode(k).into_string());
    ensure!(me.as_ref().is_none_or(|m| !evs.iter().any(|k| bs58::encode(k).into_string() == *m)), "the provider cannot judge its own work");
    let market = a.mo.address()?;
    let st = ctx.ui.step("Reading the market's fee");
    let cfg = st.wrap(query(ctx, &market, method("config", &[])).and_then(|b| decode_config(&b)), |c| format!("Fee {} bps", u(c, "fee_bps")))?;
    let blob = serde_json::to_vec(&json!({
        "format": "cadre-offer-v1", "title": title, "category": a.category, "description": a.description, "deliverable": a.deliverable,
        "turnaround_blocks": turnaround, "evaluator_policy": if policy == 1 { "the buyer picks from the offer's evaluator list" } else { "the buyer names the evaluators" },
        "provider": me, "outputs": a.deliverable,
    }))?;
    let id = cadre_job::data::blob_id(&blob);
    let call = OfferCall { title: title.into(), category: a.category.clone(), spec_uri: cadre_job::data::blob_uri(&id), spec_hash: id, price, turnaround, policy, evaluators: evs.clone(), active: !a.paused };
    let names: Vec<String> = evs.iter().map(|k| name_of(&bs58::encode(k).into_string())).collect();
    let review = offer_review(title, price, turnaround, u(&cfg, "fee_bps") as u16, if policy == 1 { Some(&names) } else { None }, !a.paused);
    finish_build(ctx, &market, "offer", call.typed(), 0, "Spec", blob, review, a.send, json!({}))
}

// ---------------------------------------------------------------------------------------
// the app's Market view (src/entry/): data from the shared poller, detail on selection

/// What the poller delivers every 15 s once the Market view was opened.
#[derive(Debug, Clone)]
pub struct Snap {
    pub source: &'static str,
    pub note: Option<String>,
    pub summary: Value,
    pub offers: Vec<Value>,
    pub hires: Vec<Value>,
}

pub fn snap(ctx: &Ctx) -> Result<Snap> {
    let r = Reader::open(ctx, &MarketOpts::from_env())?;
    let summary = r.summary()?;
    let offers = r.offers(None, None, true)?["offers"].as_array().cloned().unwrap_or_default();
    let hires = r.hires(&HireFilter { limit: 200, ..Default::default() })?["hires"].as_array().cloned().unwrap_or_default();
    Ok(Snap { source: r.source, note: r.note.clone(), summary, offers, hires })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pane {
    #[default]
    Offers,
    Hires,
}

#[derive(Debug, Default)]
pub struct ViewState {
    pub pane: Pane,
    pub sel_offer: usize,
    pub sel_hire: usize,
    pub detail_scroll: u16,
    /// Detail outputs read so far, by (pane, id).
    pub details: HashMap<(Pane, u64), Result<Out, String>>,
    /// Detail reads in flight.
    pub pending: HashSet<(Pane, u64)>,
}

impl ViewState {
    /// The selected (pane, id), if the list has one.
    pub fn selected(&self, sn: &Snap) -> Option<(Pane, u64)> {
        let (list, i) = match self.pane {
            Pane::Offers => (&sn.offers, self.sel_offer),
            Pane::Hires => (&sn.hires, self.sel_hire),
        };
        list.get(i.min(list.len().saturating_sub(1))).map(|v| (self.pane, u(v, "id")))
    }
    /// Move the selection; true when it changed.
    pub fn step(&mut self, sn: &Snap, d: isize) -> bool {
        let (n, i) = match self.pane {
            Pane::Offers => (sn.offers.len(), &mut self.sel_offer),
            Pane::Hires => (sn.hires.len(), &mut self.sel_hire),
        };
        if n == 0 {
            return false;
        }
        let j = (*i as isize + d).clamp(0, n as isize - 1) as usize;
        let changed = j != *i;
        *i = j;
        if changed {
            self.detail_scroll = 0;
        }
        changed
    }
}

/// The detail card for the Market view (the same output as `cadre offer` / `cadre hire`).
pub fn detail(ctx: &Ctx, key: (Pane, u64)) -> Result<Out> {
    match key {
        (Pane::Offers, id) => offer_detail(ctx, &MarketOpts::from_env(), id),
        (Pane::Hires, id) => hire_detail(ctx, &MarketOpts::from_env(), id),
    }
}

pub fn draw_view(buf: &mut ratatui::buffer::Buffer, pen: &crate::entry::draw::Pen, r: ratatui::layout::Rect, sn: Option<&Result<Snap, String>>, st: &ViewState, ms: u128) {
    use crate::entry::draw::{self, clip, put, put_right, selected_row};
    use crate::entry::theme::{BLUE, DIM, GREEN, MUTED, PANEL, RED, TEXT, YELLOW};
    use ratatui::style::Modifier;
    let sn = match sn {
        None => {
            put(buf, r.x + 2, r.y, &format!("{} reading the services market", draw::spinner(ms)), pen.fg(MUTED), r.right());
            return;
        }
        Some(Err(e)) => {
            for (i, l) in draw::wrap(&format!("market unavailable: {e}"), r.width.saturating_sub(4) as usize).iter().enumerate() {
                put(buf, r.x + 2, r.y + i as u16, l, pen.fg(RED), r.right());
            }
            return;
        }
        Some(Ok(s_)) => s_,
    };
    let sum = &sn.summary;
    let v = &sum["volume"];
    let head = format!(
        "{} offers · {} hires · organic {} CADRE from {} payer(s) · house {} CADRE · fee {} bps · {}",
        u(sum, "offers"),
        u(sum, "hires"),
        cad_s(&v["organic"]),
        u(v, "organic_distinct_payers"),
        cad_s(&v["house"]),
        u(&sum["config"], "fee_bps"),
        sn.source
    );
    put(buf, r.x + 2, r.y, &clip(&head, r.width.saturating_sub(4) as usize), pen.fg(MUTED), r.right());
    let lw = (r.width * 42 / 100).clamp(30, 60).min(r.width);
    let left = ratatui::layout::Rect::new(r.x, r.y + 2, lw, r.height.saturating_sub(2));
    // pane tabs
    let mut x = left.x + 2;
    for (p, name, n) in [(Pane::Offers, "offers", sn.offers.len()), (Pane::Hires, "hires", sn.hires.len())] {
        let on = st.pane == p;
        x = put(buf, x, left.y, &format!("{name} {n}"), if on { pen.fg(TEXT).add_modifier(Modifier::BOLD) } else { pen.fg(DIM) }, left.right()) + 3;
    }
    let list = match st.pane {
        Pane::Offers => &sn.offers,
        Pane::Hires => &sn.hires,
    };
    let vis = left.height.saturating_sub(2) as usize;
    if list.is_empty() {
        let msg = match st.pane {
            Pane::Offers => "no offers listed on this market yet",
            Pane::Hires => "no hires yet",
        };
        put(buf, left.x + 2, left.y + 2, msg, pen.fg(MUTED), left.right());
    }
    let sel = match st.pane {
        Pane::Offers => st.sel_offer,
        Pane::Hires => st.sel_hire,
    }
    .min(list.len().saturating_sub(1));
    let first = if sel >= vis { sel + 1 - vis } else { 0 };
    for (i, it) in list.iter().enumerate().skip(first).take(vis) {
        let y = left.y + 2 + (i - first) as u16;
        let on = i == sel;
        if on {
            selected_row(buf, pen, left.x, y, left.right());
        }
        let stl = |c: u32| if on { pen.on(c, PANEL) } else { pen.fg(c) };
        let (label, right, rc) = match st.pane {
            Pane::Offers => {
                let house = if it["provider_card"]["house"] == true { " house" } else { "" };
                (format!("#{} {}", u(it, "id"), s(it, "title")), format!("{} CADRE{house}", cad_s(&it["price"])), if it["active"] == true { TEXT } else { DIM })
            }
            Pane::Hires => {
                let st_ = s(it, "status");
                let c = match st_ {
                    "completed" => GREEN,
                    "open" | "delivered" => YELLOW,
                    "rejected" | "expired" => RED,
                    _ => DIM,
                };
                (format!("#{} {}", u(it, "id"), it["offer_title"].as_str().unwrap_or("")), st_.to_string(), c)
            }
        };
        let rx = put_right(buf, left.right().saturating_sub(1), y, &right, stl(rc), left.x);
        put(buf, left.x + 2, y, &clip(&label, rx.saturating_sub(left.x + 4) as usize), stl(TEXT).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }), rx.saturating_sub(1));
    }
    // right: the selected item's detail, drawn as the command's cards
    let rx = left.right() + 2;
    if rx + 24 > r.right() {
        return;
    }
    let right = ratatui::layout::Rect::new(rx, r.y + 2, r.right() - rx, r.height.saturating_sub(2));
    let Some(key) = st.selected(sn) else { return };
    match st.details.get(&key) {
        None => {
            put(buf, right.x + 1, right.y, &format!("{} reading {} #{}", draw::spinner(ms), if key.0 == Pane::Offers { "offer" } else { "hire" }, key.1), pen.fg(BLUE), right.right());
        }
        Some(o) => {
            let h = crate::entry::views::out_height(o, right.width);
            let mut c = crate::entry::views::canvas(right.width, h.max(1));
            crate::entry::views::draw_out(&mut c, pen, 0, 0, right.width, o);
            let top = st.detail_scroll.min(crate::entry::views::max_scroll(h, right.height));
            crate::entry::views::blit(&c, buf, right, top);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/market_vectors.json")).unwrap()
    }

    #[test]
    fn records_decode_like_the_ts_codec() {
        let v = vectors();
        assert_eq!(decode_offer(0, &hex::decode(s(&v["offer"], "hex")).unwrap()).unwrap(), v["offer"]["json"]);
        assert_eq!(decode_hire(7, &hex::decode(s(&v["hire"], "hex")).unwrap()).unwrap(), v["hire"]["json"]);
        assert_eq!(decode_hire(8, &hex::decode(s(&v["hire_open"], "hex")).unwrap()).unwrap(), v["hire_open"]["json"]);
        assert_eq!(decode_config(&hex::decode(s(&v["config"], "hex")).unwrap()).unwrap(), v["config"]["json"]);
        assert_eq!(decode_quote(&hex::decode(s(&v["quote"], "hex")).unwrap()).unwrap(), v["quote"]["json"]);
        assert!(decode_offer(0, &hex::decode(s(&v["offer"], "hex")).unwrap()[..40]).is_err());
    }

    #[test]
    fn events_decode_like_the_ts_codec() {
        let v = vectors();
        let evs = v["events"].as_array().unwrap();
        assert_eq!(evs.len(), 9);
        for e in evs {
            assert_eq!(decode_event(&e["raw"]).unwrap(), e["json"], "{}", e["json"]["name"]);
        }
        assert!(decode_event(&json!({"topics": ["00".repeat(32)], "data": "", "height": 1})).is_none());
    }

    #[test]
    fn call_args_match_the_ts_encoder() {
        let v = vectors();
        let k = |n: &str| crate::fmt::parse_chain_addr(s(&v["addrs"], n)).unwrap();
        let h = HireCall { offer: 0, price: 2_500_000_000, request_uri: format!("blob:{}", "11".repeat(32)), request_hash: [0x11; 32], deadline_blocks: 60, judge_fee: 100_000_000, evaluators: vec![k("J")], threshold: 1 };
        assert_eq!(hex::encode(crate::contract::encode_call("hire", &h.typed()).unwrap()), s(&v, "hire_args"));
        let o = OfferCall { title: "T".into(), category: "c".into(), spec_uri: format!("blob:{}", "ab".repeat(32)), spec_hash: [0xab; 32], price: 1_000_000_000, turnaround: 30, policy: 0, evaluators: vec![], active: true };
        assert_eq!(hex::encode(crate::contract::encode_call("offer", &o.typed()).unwrap()), s(&v, "offer_args"));
        assert_eq!(fee_of(2_500_000_000, 2000), 500_000_000);
        assert_eq!(fee_of(1, 2000), 1);
        assert_eq!(cadre_arg(3_100_000_000), "3.1");
        assert_eq!(cadre_arg(2_000_000_000), "2");
        assert_eq!(shell_word("str:Summarize a block"), "'str:Summarize a block'");
        assert_eq!(shell_word("u64:5"), "u64:5");
    }

    #[test]
    fn review_says_what_you_pay_to_whom_and_refunds() {
        let r = hire_review(DEVNET_MARKET, "@scribe_wren (house)", &["@court_clerk (house)".into()], 1, 60, 2_500_000_000, 500_000_000, 100_000_000, 2000);
        assert_eq!(r[0], "You pay 3.1 CADRE now, held by the market contract CxC159...D614 (not by @scribe_wren (house)).");
        assert_eq!(r[1], "Of it: 2.5 CADRE goes to @scribe_wren (house) only if your evaluators accept the delivery; 0.5 CADRE is the market fee (20%), kept only on acceptance; 0.1 CADRE pays the evaluators who decide.");
        assert_eq!(r[2], "Evaluators: @court_clerk (house); 1 of 1 must accept.");
        assert_eq!(r[3], "If they reject, 3 CADRE comes back to you (the evaluators keep their 0.1).");
        assert_eq!(r[4], "If nothing is delivered and decided within 60 blocks (about 1.0 hours), anyone can send a refund of all 3.1 CADRE to you.");
        assert_eq!(r[5], "@scribe_wren (house) may decline before delivering; then all of it comes back at once.");
        assert!(r.iter().all(|l| !l.contains('\u{2014}')));
        let o = offer_review("Haiku", 1_000_000_000, 30, 2050, None, true);
        assert_eq!(o[0], "List the service \"Haiku\" at 1 CADRE, delivered within 30 blocks (about 30 minutes).");
        assert_eq!(o[1], "Buyers pay the price plus a 0.205 CADRE market fee (20.5%) into escrow; you are paid the price when their evaluators accept your delivery.");
        assert_eq!(hours(1500), "about 25 hours");
    }

    /// A loopback node that serves the market contract from the TS vectors: offer 0, hires
    /// 0..=7 (the same record), the vector events, no blobs; GETs (cadre-api) answer 404.
    fn mock_node() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let v = vectors();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let v = v.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(conn.try_clone().unwrap());
                    let mut w = conn;
                    loop {
                        let mut first = String::new();
                        if r.read_line(&mut first).unwrap_or(0) == 0 {
                            return;
                        }
                        let mut len = 0usize;
                        loop {
                            let mut line = String::new();
                            if r.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            if line == "\r\n" {
                                break;
                            }
                            if let Some(x) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                                len = x.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut body = vec![0; len];
                        if r.read_exact(&mut body).is_err() {
                            return;
                        }
                        let (code, out) = if first.starts_with("POST") {
                            let q: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                            let p = &q["params"];
                            let ok = |ret: &str| json!({"ok": true, "return": ret, "error": "", "gas_used": "1"});
                            let res: Result<Value, String> = match q["method"].as_str().unwrap_or("") {
                                "getChainInfo" => Ok(json!({"height": 5000, "chain_id": 7})),
                                "queryContract" => {
                                    let a = hex::decode(s(p, "args")).unwrap();
                                    let name = String::from_utf8_lossy(&a[1..1 + a[0] as usize]).into_owned();
                                    Ok(match name.as_str() {
                                        "config" => ok(s(&v["config"], "hex")),
                                        "counts" => ok(&hex::encode([1u64.to_le_bytes(), 8u64.to_le_bytes()].concat())),
                                        "offer" => ok(s(&v["offer"], "hex")),
                                        "hire" => ok(s(&v["hire"], "hex")),
                                        "quote" => ok(s(&v["quote"], "hex")),
                                        _ => json!({"ok": false, "error": hex::encode("unknown method")}),
                                    })
                                }
                                "getContractEvents" => {
                                    let mut evs: Vec<Value> = v["events"].as_array().unwrap().iter().map(|e| e["raw"].clone()).collect();
                                    evs.reverse(); // newest first, like the node
                                    let off = u(p, "offset") as usize;
                                    Ok(json!({"total": evs.len(), "events": evs.into_iter().skip(off).take(u(p, "limit") as usize).collect::<Vec<_>>()}))
                                }
                                "getBlob" => Err("no such blob".into()),
                                m => Err(format!("mock has no {m}")),
                            };
                            let resp = match res {
                                Ok(x) => json!({"jsonrpc": "2.0", "id": q["id"], "result": x}),
                                Err(m) => json!({"jsonrpc": "2.0", "id": q["id"], "error": {"code": -32601, "message": m}}),
                            };
                            (200, resp.to_string())
                        } else {
                            (404, json!({"error": {"code": "not_found", "message": "no endpoint"}}).to_string())
                        };
                        let _ = write!(w, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{out}", out.len());
                        let _ = w.flush();
                    }
                });
            }
        });
        url
    }

    #[test]
    fn market_json_reads_the_contract_when_cadre_api_has_no_market() {
        let url = mock_node();
        let c = crate::mock::ctx(&url, &url, &url, false);
        let run = |line: &str| crate::commands::exec(&c, &crate::commands::parse_line(line).unwrap(), false).unwrap_or_else(|e| panic!("{line}: {e:#}"));
        let o = run(&format!("market --market {DEVNET_MARKET}"));
        let j = &o.json;
        assert_eq!(j["source"], "node");
        assert!(s(j, "note").contains("HTTP 404"), "{}", j["note"]);
        assert_eq!(j["summary"]["offers"], 1);
        assert_eq!(j["summary"]["hires"], 8);
        assert_eq!(j["summary"]["by_status"]["completed"], 8);
        // eight completed hires from one buyer: 20 CADRE organic, one distinct payer
        assert_eq!(j["summary"]["volume"]["organic"], "20000000000");
        assert_eq!(j["summary"]["volume"]["organic_distinct_payers"], 1);
        let off = &j["offers"][0];
        assert_eq!(off["title"], "Summarize a block range");
        assert_eq!(off["fee"], "500000000");
        assert_eq!(off["record"]["distinct_payers"], 1);
        assert_eq!(off["provider_card"]["house"], false);
        // filters
        assert_eq!(run(&format!("market --market {DEVNET_MARKET} --category writing")).json["count"], 0);
        assert_eq!(run(&format!("market hires --market {DEVNET_MARKET} --status rejected")).json["count"], 0);
        // one hire: the timeline from the events, verdict with its thought_ref missing (none set)
        let h = run(&format!("hire 7 --market {DEVNET_MARKET}")).json;
        let names: Vec<&str> = h["timeline"].as_array().unwrap().iter().map(|e| e["event"].as_str().unwrap()).collect();
        assert_eq!(names, ["Hired", "Delivered", "Verdict", "Completed"]);
        assert_eq!(h["verdicts"][0]["accept"], true);
        assert_eq!(h["hire"]["counts_as_organic"], true);
        assert!(h["request"].is_null(), "no blob on the mock");
        // the text form names the source and never prints an em dash
        let t = crate::mock::ctx(&url, &url, &url, true);
        let o = crate::commands::exec(&t, &crate::commands::parse_line(&format!("market --market {DEVNET_MARKET}")).unwrap(), false).unwrap();
        let txt = o.view.unwrap().render(&crate::term::Style::plain());
        assert!(txt.contains("Summarize a block range") && txt.contains("node (contract state + events)"), "{txt}");
        assert!(!txt.contains('\u{2014}'));
    }

    #[test]
    fn hire_builder_checks_and_reviews() {
        let v = vectors();
        let o = v["offer"]["json"].clone();
        let cfg = v["config"]["json"].clone();
        let q = v["quote"]["json"].clone();
        let mk = |evs: Vec<&str>, th: u8| HireArgs { id: None, offer: Some(0), request: Some("hi".into()), evaluators: evs.into_iter().map(String::from).collect(), threshold: th, judge_fee: "0.1".into(), deadline: None, send: false, mo: MarketOpts::default() };
        let j = s(&v["addrs"], "J");
        let b = s(&v["addrs"], "B");
        // policy offer_list: only J and J2 may judge
        assert!(format!("{:#}", build_hire(DEVNET_MARKET, &o, &cfg, &q, &mk(vec![b], 1), None, "hi").unwrap_err()).contains("only evaluators from its list"));
        assert!(build_hire(DEVNET_MARKET, &o, &cfg, &q, &mk(vec![j], 2), None, "hi").is_err());
        let p = s(&o, "provider");
        assert!(format!("{:#}", build_hire(DEVNET_MARKET, &o, &cfg, &q, &mk(vec![j], 1), Some(p), "hi").unwrap_err()).contains("own offer"));
        let (call, blob, review) = build_hire(DEVNET_MARKET, &o, &cfg, &q, &mk(vec![j], 1), Some(b), "hi").unwrap();
        assert_eq!(call.deadline_blocks, 60);
        assert_eq!(call.price, 2_500_000_000);
        assert_eq!(call.judge_fee, 100_000_000);
        assert_eq!(call.request_uri, cadre_job::data::blob_uri(&cadre_job::data::blob_id(&blob)));
        let bj: Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(bj["format"], "cadre-request-v1");
        assert_eq!(bj["request"], "hi");
        assert!(review[0].starts_with("You pay 3.1 CADRE now"), "{}", review[0]);
    }

    #[test]
    fn market_view_draws_the_list_and_the_selected_card() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let v = vectors();
        let ix = Index { market: DEVNET_MARKET.into(), config: v["config"]["json"].clone(), fee_bps: 2000, height: 5000, offers: vec![v["offer"]["json"].clone()], hires: vec![v["hire"]["json"].clone(), v["hire_open"]["json"].clone()], events: vec![] };
        let sn = Snap { source: "node", note: None, summary: ix.summary(), offers: ix.offers_json(None, None, true)["offers"].as_array().unwrap().clone(), hires: ix.hires_json(&HireFilter { limit: 50, ..Default::default() })["hires"].as_array().unwrap().clone() };
        let mut st = ViewState::default();
        let pen = crate::entry::draw::Pen { mode: crate::entry::theme::Mode::NoColor };
        let rows = |b: &Buffer| (0..b.area.height).map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let key = st.selected(&sn).unwrap();
        assert_eq!(key, (Pane::Offers, 0));
        st.details.insert(key, Ok(Out::new(json!({}), "x").view(View::panel(offer_panel(&sn.offers[0])))));
        for (w, h) in [(140u16, 40u16), (80, 24), (30, 8), (4, 2)] {
            let mut b = Buffer::empty(Rect::new(0, 0, w, h));
            draw_view(&mut b, &pen, Rect::new(0, 0, w, h), Some(&Ok(sn.clone())), &st, 0);
            let all = rows(&b);
            assert!(!all.contains('\u{2014}'));
            if w >= 80 {
                assert!(all.contains("#0 Summarize") && all.contains("2.5 CADRE") && all.contains("Offer #0"), "{all}");
            }
            if w >= 140 {
                assert!(all.contains("#0 Summarize a block range") && all.contains("ACTIVE"), "{all}");
            }
        }
        st.pane = Pane::Hires;
        assert!(st.step(&sn, 1));
        assert_eq!(st.selected(&sn), Some((Pane::Hires, 7)));
        let mut b = Buffer::empty(Rect::new(0, 0, 140, 30));
        draw_view(&mut b, &pen, Rect::new(0, 0, 140, 30), Some(&Ok(sn.clone())), &st, 0);
        let all = rows(&b);
        assert!(all.contains("completed") && all.contains("open") && all.contains("reading hire #7"), "{all}");
        let mut b = Buffer::empty(Rect::new(0, 0, 100, 10));
        draw_view(&mut b, &pen, Rect::new(0, 0, 100, 10), Some(&Err("queryContract: no contract".into())), &st, 0);
        assert!(rows(&b).contains("market unavailable: queryContract"));
    }

    #[test]
    fn provider_record_keeps_house_apart() {
        let v = vectors();
        let mut h = v["hire"]["json"].clone();
        h["status"] = json!("completed");
        let mut house = h.clone();
        house["id"] = json!(9);
        house["buyer"] = json!(DEVNET_JUDGE);
        let rec = provider_record(&[h.clone(), house], s(&h, "provider"));
        assert_eq!(rec["paid_volume_organic"], "2500000000");
        assert_eq!(rec["house_volume"], "2500000000");
        assert_eq!(rec["distinct_payers"], 1);
        assert_eq!(rec["completed"], 2);
        assert_eq!(rec["median_turnaround_blocks"], 10);
    }
}
