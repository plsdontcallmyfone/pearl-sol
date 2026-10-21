//! Read commands that look one thing up across both chains and the services: `tx` (a Cadre
//! tx hash or a Solana signature, in words), `account`, `block`, `agent` (profile, operator,
//! balances on both chains, posts and thoughts with receipts, desktops, hosting, jobs),
//! `agents`, `verify` (the replay verdict of an inference ticket), `watch` (live tail).
//! Read-only: nothing here signs or sends anything.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::{Ctx, Network};
use crate::decode::{self, Words};
use crate::fmt::{short, short_n, sol, spl, tokens};
use crate::probe::{self, age, get, now_ms};
use crate::snapshot::{s, u};
use crate::solana::{self, b58};
use crate::term::{Block, Panel, Table, Tone, View};

fn cadre(units: u128) -> String {
    format!("{} CADRE", tokens(units))
}
fn units(v: &Value) -> u128 {
    v.as_str().and_then(|x| x.parse().ok()).or_else(|| v.as_u64().map(|x| x as u128)).unwrap_or(0)
}
fn cell(x: impl Into<String>) -> (String, Tone) {
    (x.into(), Tone::Plain)
}
fn dim(x: impl Into<String>) -> (String, Tone) {
    (x.into(), Tone::Dim)
}

/// Explorer link for a path (`/tx/<hash>`), when the network has an explorer.
pub fn explorer_link(ctx: &Ctx, path: &str) -> Option<String> {
    ctx.eps.get("explorer").map(|e| format!("{e}{path}"))
}

fn solana_link(ctx: &Ctx, sig: &str) -> Option<String> {
    match ctx.opts.network {
        Network::Devnet => Some(format!("https://explorer.solana.com/tx/{sig}?cluster=devnet")),
        _ => None,
    }
}

fn add_words(mut p: Panel, w: &Words) -> Panel {
    p = p.text(w.headline.clone(), Tone::Plain);
    for (k, v) in &w.fields {
        p = p.kv(k, v.as_str());
    }
    p
}

/// What kind of id the user typed.
#[derive(Debug, PartialEq, Eq)]
pub enum TxId {
    Chain(String),
    Solana(String),
}

pub fn classify_tx(id: &str) -> Result<TxId> {
    let t = id.trim().trim_start_matches("0x");
    if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(TxId::Chain(t.to_ascii_lowercase()));
    }
    match bs58::decode(t).into_vec() {
        Ok(b) if b.len() == 64 => Ok(TxId::Solana(t.to_string())),
        _ => bail!("{id:?} is neither a Cadre tx hash (64 hex) nor a Solana signature (base58, 64 bytes)"),
    }
}

// ---------------------------------------------------------------------------------------
// tx

pub fn tx(ctx: &Ctx, id: &str) -> Result<Out> {
    match classify_tx(id)? {
        TxId::Chain(h) => chain_tx(ctx, &h),
        TxId::Solana(sig) => solana_tx(ctx, &sig),
    }
}

fn params(ctx: &Ctx) -> Value {
    ctx.call("getParams", json!({})).unwrap_or(Value::Null)
}

fn chain_tx(ctx: &Ctx, hash: &str) -> Result<Out> {
    let mut v = ctx.call("getTx", json!({"tx_hash": hash}))?;
    ensure!(!v.is_null(), "no tx {hash} on {} (not in a block or the mempool of this node)", ctx.rpc);
    let tx = decode::chain_tx_b64(s(&v, "tx"));
    let mut w = decode::chain_words(tx.as_ref(), s(&v, "kind"), s(&v, "from"), &params(ctx));
    if let Some(h) = ctx.call("getProfile", json!({"account": s(&v, "from")})).ok().and_then(|p| p["handle"].as_str().map(String::from)) {
        w.headline = w.headline.replacen(&crate::fmt::short_addr(s(&v, "from")), &format!("@{h}"), 1);
    }
    let info = ctx.call("getChainInfo", json!({})).unwrap_or(Value::Null);
    let included = v.get("height").is_some_and(|h| !h.is_null());
    let fin = included && u(&info, "finalized_height") >= u(&v, "height");
    let state = if fin { "final" } else if included { "included" } else { "pending" };
    let mut p = Panel::new(&format!("Transaction: {}", w.kind));
    p = add_words(p, &w);
    p = p.section("Status").state("State", state).kv("Hash", hash);
    if included {
        p = p.kv("Block", format!("{} (index {}, {} confirmations)", u(&v, "height"), u(&v, "index"), u(&v, "confirmations")));
    }
    if let Some(r) = v.get("receipt").filter(|r| r.is_object()) {
        p = p.kv("Contract result", format!("{} gas used {} of {}, fee {}", s(r, "status"), u(r, "gas_used"), u(r, "gas_limit"), cadre(units(&r["fee_charged"]))));
    }
    let mut links = json!({});
    if let Some(l) = explorer_link(ctx, &format!("/tx/{hash}")) {
        p = p.kv("Explorer", l.as_str());
        links["explorer"] = json!(l);
    }
    if included {
        if let Some(l) = explorer_link(ctx, &format!("/block/{}", u(&v, "height"))) {
            links["block"] = json!(l);
        }
    }
    let t = format!("{} {} {}{}", w.kind, short(hash), state, if included { format!(" at {}", u(&v, "height")) } else { String::new() });
    if let Some(o) = v.as_object_mut() {
        o.insert("chain".into(), json!("cadre"));
        o.insert("state".into(), json!(state));
        o.insert("final".into(), json!(fin));
        o.insert("decoded".into(), w.json());
        o.insert("links".into(), links);
    }
    Ok(Out::new(v, t).view(View::panel(p)))
}

fn solana_tx(ctx: &Ctx, sig: &str) -> Result<Out> {
    let v = ctx.sol.call("getTransaction", json!([sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0, "commitment": "confirmed"}]))?;
    ensure!(!v.is_null(), "no Solana tx {sig} on {} (unknown, not confirmed yet, or older than the RPC keeps)", ctx.sol_rpc);
    let msg = &v["transaction"]["message"];
    let ixs = msg["instructions"].as_array().cloned().unwrap_or_default();
    let words: Vec<String> = ixs.iter().map(decode::sol_ix_words).collect();
    let inner_ix: Vec<Value> = v["meta"]["innerInstructions"].as_array().into_iter().flatten().flat_map(|x| x["instructions"].as_array().cloned().unwrap_or_default()).collect();
    let inner = inner_ix.len();
    let mut inner_words: Vec<String> = vec![];
    for w in inner_ix.iter().map(decode::sol_ix_words) {
        if !inner_words.contains(&w) && !w.starts_with("program ") {
            inner_words.push(w);
        }
    }
    let logs = v["meta"]["logMessages"].as_array().cloned().unwrap_or_default();
    let events = decode::sol_events(&logs);
    let err = &v["meta"]["err"];
    let ok = err.is_null();
    let signer = msg["accountKeys"].as_array().and_then(|a| a.iter().find(|k| k["signer"] == true)).and_then(|k| k["pubkey"].as_str()).unwrap_or("");
    let headline = words.iter().find(|w| !w.starts_with("compute budget")).cloned().unwrap_or_else(|| "no instructions".into());
    let mut p = Panel::new("Solana transaction").text(headline.clone(), Tone::Plain);
    p = p.section("Instructions");
    for (i, w) in words.iter().enumerate() {
        p = p.kv(&format!("{}", i + 1), w.as_str());
    }
    if inner > 0 {
        p = p.kv("Inner", format!("{inner} inner instructions"));
        for w in inner_words.iter().take(8) {
            p = p.kv("", w.as_str());
        }
    }
    for e in &events {
        p = p.kv("Event", e.as_str());
    }
    p = p.section("Status").kt("State", if ok { "OK" } else { "FAILED" }, if ok { Tone::Good } else { Tone::Bad });
    if !ok {
        p = p.kv("Error", err.to_string());
    }
    p = p.kv("Slot", u(&v, "slot").to_string()).kv("Fee", format!("{} lamports", u(&v["meta"], "fee"))).kv("Signer", signer);
    if let Some(t) = v["blockTime"].as_u64() {
        p = p.kv("Time", age(now_ms().saturating_sub(t * 1000)));
    }
    let mut links = json!({});
    if let Some(l) = solana_link(ctx, sig) {
        p = p.kv("Solana explorer", l.as_str());
        links["solana"] = json!(l);
    }
    if let Some(l) = explorer_link(ctx, &format!("/tx/{sig}")) {
        links["explorer"] = json!(l);
    }
    let j = json!({"chain": "solana", "signature": sig, "success": ok, "error": err, "slot": v["slot"], "block_time": v["blockTime"], "fee": v["meta"]["fee"], "signer": signer,
                   "instructions": words, "inner_instructions": inner, "inner_words": inner_words, "events": events, "links": links});
    Ok(Out::new(j, format!("solana {} {}: {headline}", short(sig), if ok { "ok" } else { "failed" })).view(View::panel(p)))
}

// ---------------------------------------------------------------------------------------
// account

/// `@handle` or a handle without `@` resolves through getHandle; an address stays.
pub fn resolve_account(ctx: &Ctx, a: &str) -> Result<(String, Value)> {
    let t = a.trim();
    if let Some(h) = t.strip_prefix('@') {
        let p = ctx.call("getHandle", json!({"handle": h}))?;
        ensure!(!p.is_null(), "no profile with handle @{h}");
        return Ok((s(&p, "account").to_string(), p));
    }
    if t.starts_with(agent_types::cx::CX_PREFIX) || pearl_job::spec::parse_key32(t).is_ok() {
        // Cx (a wrong checksum is an error here, not a handle lookup), base58 or hex
        let acct = b58(&crate::fmt::parse_chain_addr(t)?);
        let p = ctx.call("getProfile", json!({"account": acct})).unwrap_or(Value::Null);
        return Ok((acct, p));
    }
    let p = ctx.call("getHandle", json!({"handle": t}))?;
    ensure!(!p.is_null(), "{t:?} is not an address and no profile has that handle");
    Ok((s(&p, "account").to_string(), p))
}

/// SOL and CADRE SPL of a Solana address (the bridge Config names the mint).
pub fn solana_balances(ctx: &Ctx, a: &str) -> Value {
    let r = (|| -> Result<Value> {
        let k = pearl_job::spec::parse_key32(a)?;
        let lam = ctx.sol.balance(&k)?;
        let cfg = ctx.sol.account(&solana::bridge_config_pda())?.map(|d| solana::BridgeConfig::parse(&d)).transpose()?;
        let spl_bal = match cfg {
            Some(c) => ctx.sol.token_balance(&solana::ata(&k, &c.mint, &c.token_program))?.unwrap_or(0),
            None => 0,
        };
        Ok(json!({"sol_lamports": lam.to_string(), "cadre_spl": spl_bal.to_string()}))
    })();
    r.unwrap_or_else(|e| json!({"error": format!("{e:#}")}))
}

fn sol_line(b: &Value) -> String {
    if let Some(e) = b["error"].as_str() {
        return format!("unavailable: {e}");
    }
    format!("{} SOL · {} CADRE (SPL)", sol(units(&b["sol_lamports"])), spl(units(&b["cadre_spl"])))
}

fn tx_rows(ctx: &Ctx, txs: &[Value], params: &Value) -> Vec<Vec<(String, Tone)>> {
    let _ = ctx;
    txs.iter()
        .map(|x| {
            let t = decode::chain_tx_b64(s(x, "tx"));
            let w = decode::chain_words(t.as_ref(), s(x, "kind"), s(x, "from"), params);
            vec![cell(u(x, "height").to_string()), cell(w.kind.clone()), cell(short_n(&w.headline, 70)), dim(short_n(s(x, "tx_hash"), 8))]
        })
        .collect()
}

pub fn account(ctx: &Ctx, a: &str) -> Result<Out> {
    let (acct, profile) = resolve_account(ctx, a)?;
    let ac = ctx.call("getAccount", json!({"account": acct}))?;
    let agent = ctx.call("getAgent", json!({"account": acct})).unwrap_or(Value::Null);
    let txs = ctx.call("getAccountTxs", json!({"account": acct, "limit": 10})).map(|v| v["txs"].as_array().cloned().unwrap_or_default()).unwrap_or_default();
    let solb = solana_balances(ctx, &acct);
    let pr = params(ctx);
    let mut p = Panel::new("Account").kv("Address", crate::fmt::cx_str(&acct)).section("Cadre chain").kv("Balance", cadre(units(&ac["balance"]))).kv("Nonce", u(&ac, "nonce").to_string());
    p = p.kv("Payout lien", crate::commands::route_text(&ac["route"]));
    p = p.kv("Agent", if agent.is_null() { "not registered".to_string() } else { format!("{} ({})", s(&agent, "name"), s(&agent, "uri")) });
    if profile.is_object() && profile["has_profile"] == true {
        p = p.section("Profile").kv("Handle", format!("@{}", s(&profile, "handle"))).kv("Name", s(&profile, "name"));
        p = p.kv("Kind", profile_kind(&profile));
    }
    p = p.section("Solana").kv("Address", acct.as_str()).kv("Balances", sol_line(&solb));
    let mut v = View::panel(p);
    v.0.push(Block::Table(Table { chip: Some("Recent txs".into()), head: ["HEIGHT", "KIND", "WHAT", "TX"].map(String::from).to_vec(), right: vec![true, false, false, false], rows: tx_rows(ctx, &txs, &pr), empty: "no transactions".into() }));
    if let Some(l) = explorer_link(ctx, &format!("/agent/{acct}")) {
        v = v.hint(format!("explorer {l}"));
    }
    let t = format!("{} balance {} nonce {}", crate::fmt::short_addr(&acct), cadre(units(&ac["balance"])), u(&ac, "nonce"));
    Ok(Out::new(json!({"account": acct, "chain": ac, "agent": agent, "profile": profile, "solana": solb, "txs": txs}), t).view(v))
}

fn profile_kind(p: &Value) -> String {
    let mut k = vec![];
    if p["is_agent"] == true {
        k.push("agent");
    }
    if p["house"] == true {
        k.push("house");
    }
    if p["accepts_hires"] == true {
        k.push("accepts hires");
    }
    if p["trades"] == true {
        k.push("trades");
    }
    if k.is_empty() {
        "person".into()
    } else {
        k.join(", ")
    }
}

// ---------------------------------------------------------------------------------------
// block

pub fn block(ctx: &Ctx, id: &str) -> Result<Out> {
    let t = id.trim().trim_start_matches("0x");
    let q = if let Ok(h) = t.parse::<u64>() { json!({"height": h}) } else { json!({"hash": t}) };
    let b = ctx.call("getBlock", q)?;
    ensure!(b.is_object(), "no block {id}");
    let hf = &b["header_fields"];
    let job = s(hf, "job_id");
    let fallback = job.chars().all(|c| c == '0');
    let txs = b["txs"].as_array().cloned().unwrap_or_default();
    let pr = params(ctx);
    let mut p = Panel::new(&format!("Block {}", u(hf, "height")))
        .kv("Hash", s(&b, "hash"))
        .kv("Miner", crate::fmt::cx_str(s(hf, "miner")))
        .kv("Work", if fallback { "fallback block (no paid job; earns its tx fees only)".to_string() } else { format!("paid job {job}") })
        .kv("Reward", format!("{} (fees {}, subsidy {})", cadre(units(&b["reward"])), cadre(units(&b["fees"])), cadre(units(&b["subsidy"]))))
        .kv("Time", age(now_ms().saturating_sub(u(hf, "timestamp_ms"))))
        .kv("Txs", txs.len().to_string())
        .state("State", if b["final"] == true { "final" } else if b["canonical"] == true { "included" } else { "pending" })
        .kv("Confirmations", u(&b, "confirmations").to_string());
    if let Some(c) = b.get("contracts").filter(|c| c.is_object()) {
        p = p.kv("Contracts", format!("{} receipts, {} events, gas {}", u(c, "receipt_count"), u(c, "event_count"), u(c, "gas_used")));
    }
    let mut v = View::panel(p);
    let rows = txs
        .iter()
        .map(|x| {
            let t = decode::chain_tx_b64(s(x, "tx"));
            let w = decode::chain_words(t.as_ref(), s(x, "kind"), s(x, "from"), &pr);
            vec![cell(w.kind.clone()), cell(short_n(&w.headline, 76)), dim(short_n(s(x, "tx_hash"), 8))]
        })
        .collect();
    v.0.push(Block::Table(Table { chip: Some("Transactions".into()), head: ["KIND", "WHAT", "TX"].map(String::from).to_vec(), right: vec![false, false, false], rows, empty: "no transactions".into() }));
    if let Some(l) = explorer_link(ctx, &format!("/block/{}", u(hf, "height"))) {
        v = v.hint(format!("explorer {l}"));
    }
    let mut j = b.clone();
    if let Some(o) = j.as_object_mut() {
        o.insert(
            "decoded".into(),
            json!(txs
                .iter()
                .map(|x| {
                    let t = decode::chain_tx_b64(s(x, "tx"));
                    decode::chain_words(t.as_ref(), s(x, "kind"), s(x, "from"), &pr).json()
                })
                .collect::<Vec<_>>()),
        );
    }
    Ok(Out::new(j, format!("block {} {} txs {}", u(hf, "height"), txs.len(), if fallback { "fallback" } else { "paid job" })).view(v))
}

// ---------------------------------------------------------------------------------------
// agent

pub fn agent(ctx: &Ctx, who: Option<&str>, explorer: &Option<String>) -> Result<Out> {
    let (a, prof) = match who {
        Some(w) => resolve_account(ctx, w)?,
        None => {
            let me = ctx.me.as_ref().map(b58).ok_or_else(|| anyhow!("no account: pass an address or @handle, or --key / --address"))?;
            let p = ctx.call("getProfile", json!({"account": me})).unwrap_or(Value::Null);
            (me, p)
        }
    };
    let agent = ctx.call("getAgent", json!({"account": a}))?;
    let route = ctx.call("getRoute", json!({"account": a}))?;
    let ac = ctx.call("getAccount", json!({"account": a})).unwrap_or(Value::Null);
    let base = crate::agent::explorer_url(ctx, explorer);
    let api = ctx.eps.get("api").map(String::from);
    let handle = s(&prof, "handle").to_string();
    // everything off-chain, in parallel, best effort
    let (record, solb, posts, desk, hosted, ua) = std::thread::scope(|sc| {
        let r = sc.spawn(|| crate::agent::fetch_profile(&base, &a));
        let sb = sc.spawn(|| solana_balances(ctx, &a));
        let ps = sc.spawn(|| api.as_ref().map(|x| get(&format!("{x}/profiles/{a}/posts?limit=5"))));
        let dk = sc.spawn(|| api.as_ref().filter(|_| !handle.is_empty()).map(|x| get(&format!("{x}/desktops/{handle}"))));
        let ho = sc.spawn(|| api.as_ref().map(|x| get(&format!("{x}/hosted/agents/{a}"))));
        let us = sc.spawn(|| api.as_ref().map(|x| get(&format!("{x}/useragents/{a}"))));
        (r.join().unwrap(), sb.join().unwrap(), ps.join().unwrap(), dk.join().unwrap(), ho.join().unwrap(), us.join().unwrap())
    });
    let (profile, perr) = match record {
        Ok(p) => (p, None),
        Err(e) => (Value::Null, Some(format!("{e:#}"))),
    };
    let mut view = View::panel(crate::commands::agent_panel(&a, &agent, &route, true));
    // social profile and operator
    let mut sp = Panel::new("Town profile");
    if prof.is_object() && prof["has_profile"] == true {
        sp = sp.kv("Handle", format!("@{}", s(&prof, "handle"))).kv("Name", s(&prof, "name")).kv("Kind", profile_kind(&prof));
        let op = s(&prof, "operator");
        sp = sp.kv("Operator", if op.is_empty() { "none".to_string() } else { format!("{} ({})", crate::fmt::cx_str(op), if prof["operator_linked"] == true { "linked both ways" } else { "named by the agent only, no OperatorLink" }) });
        let st = &prof["stats"];
        sp = sp.kv("Town", format!("{} posts, {} replies, {} followers, {} following", u(st, "posts"), u(st, "replies"), u(st, "followers"), u(st, "following")));
    } else {
        sp = sp.kt("Handle", "no town profile", Tone::Dim);
    }
    sp = sp.section("Balances").kv("Cadre chain", cadre(units(&ac["balance"]))).kv("Solana", sol_line(&solb));
    if let Some(h) = hosted.as_ref().filter(|h| h.ok()) {
        let x = &h.json;
        sp = sp.section("Hosted runtime").state("State", s(x, "state")).kv("Budget today", format!("{} of {} rows, {} of {} SOL", u(&x["budget_today"], "rows"), u(&x["budget_today"], "rows_cap"), sol(units(&x["budget_today"]["sol_lamports"])), sol(units(&x["budget_today"]["sol_lamports_cap"]))));
        if let Some(e) = x["last_error"].as_str() {
            sp = sp.kt("Last error", short_n(e, 90), Tone::Warn);
        }
    }
    if let Some(u2) = ua.as_ref().filter(|h| h.ok() && prof["house"] != true) {
        let checks = u2.json["checks"].as_array().map(|c| c.iter().filter(|x| x["ok"] == true).count()).unwrap_or(0);
        let total = u2.json["checks"].as_array().map(|c| c.len()).unwrap_or(0);
        sp = sp.kv("User agent", format!("{} ({checks} of {total} checks)", if u2.json["verified"] == true { "verified" } else { "not verified" }));
    }
    if let Some(d) = desk.as_ref().filter(|d| d.ok()) {
        let x = &d.json;
        sp = sp.section("Desktops");
        for (role, dsk) in x["desktops"].as_object().into_iter().flatten() {
            sp = sp.kv(role, format!("{}: {}", s(dsk, "state"), short_n(s(dsk, "current"), 80)));
        }
    }
    view.0.push(Block::Panel(sp));
    // posts and thoughts
    let post_list = posts.as_ref().filter(|h| h.ok()).and_then(|h| h.json["posts"].as_array().cloned()).unwrap_or_default();
    if posts.is_some() {
        let rows = post_list
            .iter()
            .map(|p| {
                let th = &p["thought"];
                let receipt = if th.is_object() { format!("{} {}", s(th, "status"), short_n(s(th, "request_id"), 16)) } else { "no thought".into() };
                vec![cell(u(p, "height").to_string()), cell(s(p, "kind")), cell(short_n(&s(p, "body").replace('\n', " / "), 60)), (receipt, if s(th, "status") == "verified" { Tone::Good } else { Tone::Dim })]
            })
            .collect();
        view.0.push(Block::Table(Table { chip: Some("Recent posts".into()), head: ["HEIGHT", "KIND", "BODY", "THOUGHT RECEIPT"].map(String::from).to_vec(), right: vec![true, false, false, false], rows, empty: "no posts".into() }));
    }
    // the explorer record (jobs, money behind work)
    if profile.is_object() {
        let (blocks, _) = crate::agent::profile_view(&profile);
        view.0.extend(blocks);
    } else if let Some(e) = &perr {
        view = view.hint(format!("record unavailable: {e} (set --explorer or PEARL_EXPLORER)"));
    }
    let t = format!("{}{}: {}", crate::fmt::short_addr(&a), if handle.is_empty() { String::new() } else { format!(" @{handle}") }, if agent.is_null() { "not registered".into() } else { format!("{} since height {}", s(&agent, "name"), u(&agent, "registered_height")) });
    Ok(Out::new(
        json!({"account": a, "agent": agent, "route": route, "explorer": base, "profile": profile, "profile_error": perr,
               "town": prof, "balances": {"chain": ac["balance"], "solana": solb}, "posts": post_list,
               "desktops": desk.filter(|d| d.ok()).map(|d| d.json), "hosted": hosted.filter(|h| h.ok()).map(|h| h.json), "user_agent": ua.filter(|h| h.ok()).map(|h| h.json)}),
        t,
    )
    .view(view))
}

// ---------------------------------------------------------------------------------------
// agents

pub fn agents(ctx: &Ctx, limit: usize) -> Result<Out> {
    let api = ctx.eps.get("api").ok_or_else(|| anyhow!("needs cadre-api (set CADRE_API)"))?.to_string();
    let (profiles, town, desk, hosted) = std::thread::scope(|sc| {
        let p = sc.spawn(|| probe::get_json(&format!("{api}/profiles?sort=recent&limit=200")));
        let t = sc.spawn(|| get(&format!("{api}/town?limit=100")));
        let d = sc.spawn(|| get(&format!("{api}/desktops")));
        let h = sc.spawn(|| get(&format!("{api}/hosted/agents")));
        (p.join().unwrap(), t.join().unwrap(), d.join().unwrap(), h.join().unwrap())
    });
    let profiles = profiles?;
    let tip = ctx.call("getChainInfo", json!({})).map(|i| u(&i, "height")).unwrap_or(0);
    let list: Vec<Value> = profiles["profiles"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["is_agent"] == true).take(limit).collect();
    // newest post of each agent that has posted (its own posts page, 8 at a time)
    let mut last: HashMap<String, (u64, u64)> = HashMap::new();
    for p in town.json["posts"].as_array().into_iter().flatten() {
        let a = s(&p["author"], "account").to_string();
        last.entry(a).or_insert((u(p, "height"), u(p, "timestamp_ms")));
    }
    let want: Vec<String> = list.iter().filter(|p| u(&p["stats"], "posts") + u(&p["stats"], "replies") > 0).map(|p| s(p, "account").to_string()).filter(|a| !last.contains_key(a)).collect();
    let found: Vec<(String, u64, u64)> = std::thread::scope(|sc| {
        let hs: Vec<_> = want
            .chunks(want.len().div_ceil(8).max(1))
            .map(|c| {
                let api = &api;
                sc.spawn(move || {
                    c.iter()
                        .filter_map(|a| {
                            let h = get(&format!("{api}/profiles/{a}/posts?limit=1"));
                            h.json["posts"].as_array().and_then(|x| x.first()).map(|p| (a.clone(), u(p, "height"), u(p, "timestamp_ms")))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    for (a, h, t) in found {
        last.insert(a, (h, t));
    }
    let mut desks: HashMap<String, String> = HashMap::new();
    for a in desk.json["agents"].as_array().into_iter().flatten() {
        let states: Vec<String> = a["desktops"].as_object().into_iter().flatten().map(|(r, d)| format!("{r} {}", s(d, "state"))).collect();
        desks.insert(s(a, "account").to_string(), states.join(", "));
    }
    let mut hosted_m: HashMap<String, (String, bool)> = HashMap::new();
    for a in hosted.json["agents"].as_array().into_iter().flatten() {
        hosted_m.insert(s(a, "agent").to_string(), (s(a, "state").to_string(), a["last_error"].is_string()));
    }
    let mut out = vec![];
    for p in &list {
        let a = s(p, "account");
        let kind = if p["house"] == true { "house" } else { "user" };
        let (lh, lt) = last.get(a).copied().unwrap_or((0, 0));
        let d = desks.get(a).cloned();
        let h = hosted_m.get(a).cloned();
        let recent = lt > 0 && now_ms().saturating_sub(lt) < 24 * 3600_000;
        let health = if h.as_ref().is_some_and(|x| x.1) {
            "error"
        } else if h.as_ref().is_some_and(|x| x.0 == "running") || d.as_ref().is_some_and(|x| x.contains("running")) || recent {
            "ok"
        } else {
            "idle"
        };
        out.push(json!({"account": a, "handle": p["handle"], "name": p["name"], "kind": kind, "operator": p["operator"], "operator_linked": p["operator_linked"],
                        "last_post_height": if lh > 0 { json!(lh) } else { Value::Null }, "last_post_ms": if lt > 0 { json!(lt) } else { Value::Null },
                        "desktops": d, "hosted": h.as_ref().map(|x| x.0.clone()), "health": health, "posts": p["stats"]["posts"]}));
    }
    let rows = out
        .iter()
        .map(|x| {
            let lp = if x["last_post_height"].is_null() { "never".to_string() } else { format!("{} ({} blocks ago)", u(x, "last_post_height"), tip.saturating_sub(u(x, "last_post_height"))) };
            let run = [x["desktops"].as_str().map(|d| format!("desktops: {d}")), x["hosted"].as_str().map(|h| format!("hosted {h}"))].into_iter().flatten().collect::<Vec<_>>().join("; ");
            let hs = s(x, "health").to_string();
            let tone = match hs.as_str() {
                "ok" => Tone::Good,
                "error" => Tone::Bad,
                _ => Tone::Warn,
            };
            vec![cell(format!("@{}", s(x, "handle"))), cell(s(x, "kind")), dim(crate::fmt::short_addr(s(x, "account"))), cell(lp), cell(if run.is_empty() { "-".into() } else { run }), (hs.to_uppercase(), tone)]
        })
        .collect();
    let house = out.iter().filter(|x| x["kind"] == "house").count();
    let v = View(vec![Block::Table(Table { chip: Some("Agents".into()), head: ["HANDLE", "KIND", "ACCOUNT", "LAST POST", "RUNNING", "HEALTH"].map(String::from).to_vec(), right: vec![false; 6], rows, empty: "no agent profiles".into() })])
        .hint(format!("{} agents ({house} house, {} user) of {} profiles · health: ok = hosted or desktop running or posted in 24 h, idle = none of those, error = the runtime reports a last error · cadre agent @handle", out.len(), out.len() - house, u(&profiles, "total")));
    Ok(Out::new(json!({"total_profiles": profiles["total"], "agents": out, "tip_height": tip}), format!("{} agents", out.len())).view(v))
}

// ---------------------------------------------------------------------------------------
// tickets: response blob and the replay verdict

/// A ticket's job (node getJob) and its response blob (JSON), when delivered.
pub fn ticket_response(node_url: &str, id: &str) -> Result<(Value, Value)> {
    let (job, _) = probe::rpc(node_url, "getJob", json!({"job_id": id}))?;
    ensure!(!job.is_null(), "no job {id}");
    let uri = s(&job, "result_uri");
    ensure!(!uri.is_empty(), "not delivered yet (no response blob)");
    let bid = uri.trim_start_matches("blob:");
    let (b, _) = probe::rpc(node_url, "getBlob", json!({"blob_id": bid}))?;
    let data = base64::engine::general_purpose::STANDARD.decode(s(&b, "data")).context("blob data")?;
    let r: Value = serde_json::from_slice(&data).context("response blob is not JSON")?;
    Ok((job, r))
}

/// The verdict on a ticket from what the chain and the services already say (no replay).
pub fn verdict(ctx: &Ctx, id: &str) -> Result<Value> {
    let job = ctx.call("getJob", json!({"job_id": id}))?;
    ensure!(!job.is_null(), "no job or ticket {id}");
    let status = s(&job, "status").to_string();
    let rejects = ctx.call("getJobRejects", json!({"job_id": id})).unwrap_or(Value::Null);
    let rej_n = rejects["rejects"].as_array().map(|a| a.len()).or_else(|| rejects.as_array().map(|a| a.len())).unwrap_or(u(&job, "reject_count") as usize);
    let resp = if s(&job, "result_uri").is_empty() { None } else { ticket_response(&ctx.rpc, id).ok().map(|x| x.1) };
    let glue = resp.as_ref().map(|r| s(r, "glue").to_string()).unwrap_or_default();
    let replayable = glue == "cadre-glue-v1";
    let proof = ctx.eps.get("attester").map(|a| get(&format!("{a}/proofs/{id}"))).filter(|h| h.ok()).map(|h| h.json);
    let evals: Vec<String> = job["evaluators"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(short).collect()).unwrap_or_default();
    let (word, why) = if proof.is_some() {
        ("fraud", "the attester holds a cadre-fraud-v1 proof against this ticket's answer".to_string())
    } else if rej_n > 0 && status != "completed" {
        ("rejected", format!("evaluators rejected the delivery {rej_n} time(s); the ticket reopened"))
    } else {
        match status.as_str() {
            "completed" if replayable => ("accepted", format!("completed by evaluator signature ({}); the answer carries the cadre-glue-v1 replay bundle, so anyone can re-run it (cadre verify {} --run)", evals.join(", "), short(id))),
            "completed" => ("accepted", format!("completed by evaluator signature ({}) on the GEMM check; no replay bundle, so the answer text itself is not proven", evals.join(", "))),
            "delivered" => ("delivered", "delivered; waiting for the evaluators".to_string()),
            "expired" => ("expired", "expired before completion".to_string()),
            _ => ("open", format!("not delivered yet ({} of {} rows used)", u(&job["standing"], "rows_used"), u(&job["standing"], "rows"))),
        }
    };
    Ok(json!({"ticket": id, "verdict": word, "why": why, "status": status, "rejects": rej_n, "glue": if glue.is_empty() { Value::Null } else { json!(glue) }, "replayable": replayable,
              "fraud_proof": proof, "evaluators": job["evaluators"], "closed_height": job["closed_height"], "mode": "fetched"}))
}

pub fn verdict_panel(v: &Value) -> Panel {
    let w = s(v, "verdict");
    let tone = match w {
        "accepted" => Tone::Good,
        "fraud" | "rejected" | "expired" => Tone::Bad,
        _ => Tone::Warn,
    };
    let mut p = Panel::new("Replay verdict").kt("Verdict", w.to_uppercase(), tone).kv("Why", s(v, "why"));
    p = p.kv("Replay bundle", if v["replayable"] == true { "cadre-glue-v1".to_string() } else { "none".into() });
    if v["mode"] == "replayed" {
        p = p.kv("Replayed here", s(v, "replay"));
    }
    p
}

/// `cadre verify <ticket> [--run --models DIR]`.
pub fn verify(ctx: &Ctx, id: &str, run: bool, models: &Option<std::path::PathBuf>) -> Result<Out> {
    let id = id.trim().to_ascii_lowercase();
    let mut v = verdict(ctx, &id)?;
    if run {
        let dir = models.clone().unwrap_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".cadre/models"));
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let key32 = pearl_job::spec::parse_key32(&id)?;
        let info = pearl_node::NodeApi::get_job(&ctx.node, &key32)?.ok_or_else(|| anyhow!("no job {id}"))?;
        // check_ticket only reads (node, blobs, model store); the key is never used to sign here
        let att = pearl_attester::Attester { key: ed25519_dalek::SigningKey::from_bytes(&[0x5a; 32]), source: pearl_attester::JobSource::Request, freivalds_rounds: 2, models_dir: dir.clone() };
        let step = ctx.ui.step(format!("Replaying ticket {} (model files go to {})", short(&id), dir.display()));
        let t0 = std::time::Instant::now();
        let r = att.check_ticket(&ctx.node, &info, &key32);
        let secs = t0.elapsed().as_secs_f64();
        match r {
            Ok(rh) => {
                step.ok(format!("Replayed in {secs:.1} s: honest"));
                v["mode"] = json!("replayed");
                v["replay"] = json!(format!("honest: every round rebuilt and every segment's C checked in {secs:.1} s; result_hash {}", short(&hex::encode(rh))));
                let w = if s(&v, "status") == "completed" { "accepted" } else { "honest" };
                v["verdict"] = json!(w);
                v["replay_ok"] = json!(true);
            }
            Err(e) => {
                let fraud = e.downcast_ref::<pearl_attester::replay::TicketFraud>().map(|f| serde_json::to_value(&f.0).unwrap_or(Value::Null));
                v["mode"] = json!("replayed");
                v["replay_ok"] = json!(false);
                if let Some(f) = fraud {
                    step.fail(&anyhow!("fraud found"));
                    v["verdict"] = json!("fraud");
                    v["replay"] = json!(format!("FRAUD: {e:#}"));
                    v["fraud_proof"] = f;
                } else {
                    step.fail(&e);
                    v["replay"] = json!(format!("could not replay: {e:#}"));
                }
            }
        }
    }
    let t = format!("ticket {}: {} ({})", short(&id), s(&v, "verdict"), s(&v, "why"));
    let code = if s(&v, "verdict") == "fraud" || (run && v["replay_ok"] == false) { 1 } else { 0 };
    let mut o = Out::new(v.clone(), t).view(View::panel(verdict_panel(&v)));
    o.code = code;
    Ok(o)
}

// ---------------------------------------------------------------------------------------
// watch

pub fn watch(ctx: &Ctx, count: Option<u64>, from: Option<u64>, pending: bool) -> Result<Out> {
    let json_mode = ctx.opts.json;
    let st = ctx.ui.style();
    let pr = params(ctx);
    let mut handles: HashMap<String, String> = HashMap::new();
    let mut handle = |a: &str| -> String {
        if let Some(h) = handles.get(a) {
            return h.clone();
        }
        let h = ctx.call("getProfile", json!({"account": a})).ok().and_then(|p| p["handle"].as_str().map(|h| format!("@{h}"))).unwrap_or_else(|| crate::fmt::short_addr(a));
        handles.insert(a.to_string(), h.clone());
        h
    };
    let info = ctx.call("getChainInfo", json!({}))?;
    let mut next = from.unwrap_or(u(&info, "height") + 1);
    let mut seen_pending: std::collections::HashSet<String> = Default::default();
    let (mut blocks, mut txs) = (0u64, 0u64);
    if !json_mode {
        println!("{}", st.paint(Tone::Dim, &format!("watching {} from block {next} (Ctrl-C stops)", ctx.rpc)));
    }
    let out = std::io::stdout();
    loop {
        let tip = u(&ctx.call("getChainInfo", json!({}))?, "height");
        while next <= tip {
            let b = ctx.call("getBlock", json!({"height": next}))?;
            if !b.is_object() {
                break;
            }
            let hf = &b["header_fields"];
            let job = s(hf, "job_id");
            let fallback = job.chars().all(|c| c == '0');
            let list = b["txs"].as_array().cloned().unwrap_or_default();
            let mut lock = out.lock();
            if json_mode {
                let _ = writeln!(lock, "{}", json!({"event": "block", "height": next, "hash": b["hash"], "miner": hf["miner"], "job_id": if fallback { Value::Null } else { json!(job) }, "txs": list.len(), "reward": b["reward"], "timestamp_ms": hf["timestamp_ms"]}));
            } else {
                let _ = writeln!(
                    lock,
                    "{} {}  {:<12} {:<18} {:>3} txs  reward {}",
                    st.paint(Tone::Good, "■"),
                    st.bold(&format!("block {next}")),
                    crate::fmt::short_addr(s(hf, "miner")),
                    if fallback { "fallback".to_string() } else { format!("job {}", short(job)) },
                    list.len(),
                    cadre(units(&b["reward"]))
                );
            }
            drop(lock);
            for x in &list {
                let t = decode::chain_tx_b64(s(x, "tx"));
                let mut w = decode::chain_words(t.as_ref(), s(x, "kind"), s(x, "from"), &pr);
                let from = s(x, "from").to_string();
                if !from.is_empty() && matches!(w.kind.as_str(), "Post" | "React" | "Follow" | "SetProfile" | "OperatorLink") {
                    let h = handle(&from);
                    w.headline = w.headline.replacen(&crate::fmt::short_addr(&from), &h, 1);
                }
                let mut lock = out.lock();
                if json_mode {
                    let _ = writeln!(lock, "{}", json!({"event": "tx", "height": next, "tx_hash": x["tx_hash"], "kind": w.kind, "words": w.headline, "from": from}));
                } else {
                    let _ = writeln!(lock, "    {} {:<13} {}", st.paint(Tone::Dim, &short_n(s(x, "tx_hash"), 8)), w.kind, w.headline);
                }
                txs += 1;
            }
            blocks += 1;
            next += 1;
            if count.is_some_and(|c| blocks >= c) {
                return Ok(Out::new(json!({"blocks": blocks, "txs": txs, "last_height": next - 1}), format!("watched {blocks} blocks, {txs} txs")));
            }
        }
        if pending {
            if let Ok(m) = ctx.call("getMempool", json!({})) {
                for h in m["txs"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
                    if seen_pending.insert(h.to_string()) {
                        let t = ctx.call("getTx", json!({"tx_hash": h})).unwrap_or(Value::Null);
                        let d = decode::chain_tx_b64(s(&t, "tx"));
                        let w = decode::chain_words(d.as_ref(), s(&t, "kind"), s(&t, "from"), &pr);
                        let mut lock = out.lock();
                        if json_mode {
                            let _ = writeln!(lock, "{}", json!({"event": "pending", "tx_hash": h, "kind": w.kind, "words": w.headline}));
                        } else {
                            let _ = writeln!(lock, "  {} {} {:<13} {}", st.paint(Tone::Warn, "pending"), st.paint(Tone::Dim, &short_n(h, 8)), w.kind, w.headline);
                        }
                    }
                }
            }
        }
        let _ = out.lock().flush();
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ids() {
        assert_eq!(classify_tx(&"ab".repeat(32)).unwrap(), TxId::Chain("ab".repeat(32)));
        assert_eq!(classify_tx(&format!("0x{}", "AB".repeat(32))).unwrap(), TxId::Chain("ab".repeat(32)));
        let sig = bs58::encode([7u8; 64]).into_string();
        assert_eq!(classify_tx(&sig).unwrap(), TxId::Solana(sig.clone()));
        assert!(classify_tx("hello").is_err());
    }
}
