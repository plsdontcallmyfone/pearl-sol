//! `cadre status` and `cadre ping`: one screen of every part of Cadre with PASS / WARN /
//! FAIL / SKIP, read from the public endpoints (and, with `--local`, the 127.0.0.1 services
//! of the box it runs on). Every figure is a read of a node, Solana, cadre-api, the explorer
//! or a service's own health route, or arithmetic over such reads. Nothing is written.
//!
//! Thresholds (CLI policy, docs/CLI.md "status"): node heights within 3 blocks and the
//! light client within 120 blocks of the chain (the box 0 health check's defaults); relay
//! payers at least 1 SOL (same); tip older than 10 min WARN, 30 min FAIL (60 s blocks);
//! under 30 blocks per hour WARN; newest house post older than 2 h WARN; desktops spend at
//! 90% of the day cap WARN.

use std::time::Instant;

use anyhow::Result;
use serde_json::{json, Value};

use crate::ctx::Ctx;
use crate::endpoints::node_name;
use crate::fmt::{short, sol, spl, thousands, tokens};
use crate::probe::{self, age, get, now_ms, rpc, Hit};
use crate::term::{Block, Table, Tone, View};

pub const MAX_HEIGHT_SPREAD: u64 = 3;
pub const MAX_LC_BEHIND_FINAL: u64 = 120;
pub const MIN_RELAY_LAMPORTS: u64 = 1_000_000_000;
pub const TIP_WARN_MS: u64 = 10 * 60_000;
pub const TIP_FAIL_MS: u64 = 30 * 60_000;
pub const MIN_BLOCKS_PER_HOUR: f64 = 30.0;
pub const MINING_WINDOW: u64 = 60;
pub const HOUSE_POST_WARN_MS: u64 = 2 * 3600_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Pass,
    Skip,
    Warn,
    Fail,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Level::Pass => "pass",
            Level::Warn => "warn",
            Level::Fail => "fail",
            Level::Skip => "skip",
        }
    }
    /// The box 0 health log's words (scripts/devnet-health.sh).
    pub fn health_word(self) -> &'static str {
        match self {
            Level::Pass | Level::Skip => "OK",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
    fn tone(self) -> Tone {
        match self {
            Level::Pass => Tone::Good,
            Level::Warn => Tone::Warn,
            Level::Fail => Tone::Bad,
            Level::Skip => Tone::Dim,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Check {
    pub group: &'static str,
    pub name: String,
    pub level: Level,
    pub detail: String,
    pub ms: Option<u64>,
    pub target: String,
    pub data: Value,
}

impl Check {
    pub fn new(group: &'static str, name: impl Into<String>, level: Level, detail: impl Into<String>) -> Self {
        Check { group, name: name.into(), level, detail: detail.into(), ms: None, target: String::new(), data: Value::Null }
    }
    pub fn ms(mut self, ms: u64) -> Self {
        self.ms = Some(ms);
        self
    }
    pub fn at(mut self, t: &str) -> Self {
        self.target = t.to_string();
        self
    }
    pub fn data(mut self, d: Value) -> Self {
        self.data = d;
        self
    }
    fn json(&self) -> Value {
        json!({"group": self.group, "name": self.name, "state": self.level.word(), "detail": self.detail, "ms": self.ms, "target": self.target, "data": self.data})
    }
    /// A failed GET: no answer is FAIL, an HTTP error is FAIL too (the service answered wrong).
    fn from_hit_err(group: &'static str, name: &str, h: &Hit) -> Self {
        Check::new(group, name, Level::Fail, h.why()).ms(h.ms).at(&h.url)
    }
}

pub const GROUPS: [&str; 6] = ["Chain", "Solana", "Services", "Agents", "Economy", "Box"];

fn n(v: &Value, p: &str) -> u64 {
    let x = v.pointer(p).unwrap_or(&Value::Null);
    x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}
fn n128(v: &Value, p: &str) -> u128 {
    let x = v.pointer(p).unwrap_or(&Value::Null);
    x.as_u64().map(|x| x as u128).or_else(|| x.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}
fn st<'a>(v: &'a Value, p: &str) -> &'a str {
    v.pointer(p).and_then(|x| x.as_str()).unwrap_or("")
}
fn cadre(units: u128) -> String {
    format!("{} CADRE", tokens(units))
}
fn cadre_spl(units: u128) -> String {
    format!("{} CADRE", spl(units))
}

// ---------------------------------------------------------------------------------------
// The checks

/// What the node checks learned, for the checks that need a node.
#[derive(Clone, Debug, Default)]
pub struct ChainView {
    /// The node with the highest tip that answered.
    pub best: Option<String>,
    pub info: Value,
}

pub fn chain_checks(ctx: &Ctx) -> (Vec<Check>, ChainView) {
    let nodes = &ctx.eps.nodes;
    let infos: Vec<(String, Result<(Value, u64)>)> = std::thread::scope(|s| {
        let hs: Vec<_> = nodes.iter().map(|u| s.spawn(move || (u.clone(), rpc(u, "getChainInfo", json!({}))))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut out = vec![];
    let mut ok: Vec<(String, Value)> = vec![];
    for (u, r) in &infos {
        let name = format!("node {}", node_name(u));
        match r {
            Ok((v, ms)) => {
                let peers = n(v, "/peers");
                let syncing = v["syncing"] == true;
                let lvl = if peers == 0 || syncing { Level::Warn } else { Level::Pass };
                let d = format!(
                    "height {} final {} peers {}{} mempool {}",
                    n(v, "/height"),
                    n(v, "/finalized_height"),
                    peers,
                    if syncing { " SYNCING" } else { "" },
                    n(v, "/mempool_size")
                );
                out.push(Check::new("Chain", name, lvl, d).ms(*ms).at(u).data(v.clone()));
                ok.push((u.clone(), v.clone()));
            }
            Err(e) => out.push(Check::new("Chain", name, Level::Fail, format!("{e:#}")).at(u)),
        }
    }
    let mut view = ChainView::default();
    if ok.is_empty() {
        out.push(Check::new("Chain", "agreement", Level::Fail, "no node answered"));
        return (out, view);
    }
    let best = ok.iter().max_by_key(|(_, v)| n(v, "/height")).unwrap().clone();
    view.best = Some(best.0.clone());
    view.info = best.1.clone();
    // agreement: genesis, heights, the finalized block at the lowest node's final height
    let hi = ok.iter().map(|(_, v)| n(v, "/height")).max().unwrap_or(0);
    let lo = ok.iter().map(|(_, v)| n(v, "/height")).min().unwrap_or(0);
    let genesis: std::collections::BTreeSet<&str> = ok.iter().map(|(_, v)| st(v, "/genesis_hash")).collect();
    let fin = ok.iter().map(|(_, v)| n(v, "/finalized_height")).min().unwrap_or(0);
    let hashes: Vec<(String, Result<String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = ok
            .iter()
            .map(|(u, _)| s.spawn(move || (u.clone(), rpc(u, "getHeader", json!({"height": fin})).map(|(h, _)| st(&h, "/hash").to_string()))))
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let got: std::collections::BTreeSet<String> = hashes.iter().filter_map(|(_, r)| r.as_ref().ok().cloned()).collect();
    let (lvl, d) = if genesis.len() > 1 {
        (Level::Fail, format!("nodes disagree on the genesis hash ({} different)", genesis.len()))
    } else if got.len() > 1 {
        let who = hashes.iter().map(|(u, r)| format!("{} {}", node_name(u), r.as_ref().map(|h| short(h)).unwrap_or_else(|e| format!("{e}")))).collect::<Vec<_>>().join(", ");
        (Level::Fail, format!("finalized block {fin} differs: {who}"))
    } else if got.is_empty() {
        (Level::Warn, format!("could not read block {fin} from any node"))
    } else {
        let same = format!("finalized block {fin} {} on {} of {} nodes, heights spread {}", short(got.iter().next().unwrap()), hashes.iter().filter(|(_, r)| r.is_ok()).count(), nodes.len(), hi - lo);
        (if hi - lo > MAX_HEIGHT_SPREAD || ok.len() < nodes.len() { Level::Warn } else { Level::Pass }, same)
    };
    out.push(Check::new("Chain", "agreement", lvl, d).data(json!({"finalized_height": fin, "hashes": hashes.iter().map(|(u, r)| json!({"node": u, "hash": r.as_ref().ok()})).collect::<Vec<_>>(), "spread": hi - lo})));
    // tip age
    let ts = n(&best.1, "/timestamp_ms");
    let ago = now_ms().saturating_sub(ts);
    let lvl = if ago > TIP_FAIL_MS { Level::Fail } else if ago > TIP_WARN_MS { Level::Warn } else { Level::Pass };
    out.push(Check::new("Chain", "tip", lvl, format!("block {} {} by timestamp, {}", hi, age(ago), short(st(&best.1, "/tip")))).data(json!({"height": hi, "timestamp_ms": ts, "age_ms": ago})));
    (out, view)
}

/// Blocks per hour and paid-job blocks over the last MINING_WINDOW blocks of the best node.
pub fn mining_check(view: &ChainView, summary: Option<&Value>) -> Check {
    let Some(u) = view.best.as_deref() else {
        return Check::new("Economy", "mining", Level::Skip, "no node answered");
    };
    let tip = n(&view.info, "/height");
    let lo = tip.saturating_sub(MINING_WINDOW - 1).max(1);
    let heights: Vec<u64> = (lo..=tip).collect();
    let t0 = Instant::now();
    let blocks: Vec<Option<Value>> = std::thread::scope(|s| {
        let chunks: Vec<_> = heights.chunks(heights.len().div_ceil(8).max(1)).map(|c| c.to_vec()).collect();
        let hs: Vec<_> = chunks.into_iter().map(|c| s.spawn(move || c.iter().map(|h| rpc(u, "getHeader", json!({"height": h})).ok().map(|(v, _)| v)).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let got: Vec<&Value> = blocks.iter().flatten().collect();
    if got.len() < 2 {
        return Check::new("Economy", "mining", Level::Warn, "could not read recent headers").at(u);
    }
    let ts: Vec<u64> = got.iter().map(|b| n(b, "/header_fields/timestamp_ms")).collect();
    let span_ms = ts.iter().max().unwrap() - ts.iter().min().unwrap();
    let per_h = if span_ms > 0 { (got.len() as f64 - 1.0) * 3_600_000.0 / span_ms as f64 } else { 0.0 };
    let paid = got.iter().filter(|b| !st(b, "/header_fields/job_id").chars().all(|c| c == '0')).count();
    let mut d = format!("{per_h:.1} blocks/h over the last {} (60 s target = 60/h); paid-job blocks {} of {} ({}%)", got.len(), paid, got.len(), paid * 100 / got.len());
    if let Some(s) = summary {
        d += &format!("; all time {} of {} ({})", n(s, "/blocks/paid_job_blocks"), n(s, "/blocks/total"), st(s, "/blocks/paid_share"));
    }
    let lvl = if per_h < MIN_BLOCKS_PER_HOUR { Level::Warn } else { Level::Pass };
    Check::new("Economy", "mining", lvl, d).ms(t0.elapsed().as_millis() as u64).at(u).data(json!({"blocks_per_hour": per_h, "window": got.len(), "paid_job_blocks": paid}))
}

pub fn solana_checks(ctx: &Ctx) -> Vec<Check> {
    let url = ctx.eps.solana.clone();
    let shown = ctx.eps.solana_shown.clone();
    let mut out = vec![];
    let sl = probe::rpc_hit(&url, "getSlot", json!([{"commitment": "confirmed"}]));
    let rate_limited = sl.status == 429 || sl.json.pointer("/error/code").and_then(|c| c.as_i64()) == Some(429);
    if sl.ok() && sl.json.get("result").is_some() {
        out.push(Check::new("Solana", "solana rpc", Level::Pass, format!("slot {}", n(&sl.json, "/result"))).ms(sl.ms).at(&shown));
    } else {
        let lvl = if rate_limited { Level::Warn } else { Level::Fail };
        let why = if rate_limited { "rate limited (HTTP 429)".into() } else { sl.why().replace(&url, &shown) };
        out.push(Check::new("Solana", "solana rpc", lvl, why).ms(sl.ms).at(&shown));
        return out;
    }
    let p = crate::solana::profile();
    let progs = [("reserve", p.reserve), ("bridge", p.bridge), ("storefront", p.storefront), ("lightclient", p.lightclient)];
    let mut keys: Vec<String> = progs.iter().map(|(_, k)| k.to_string()).collect();
    keys.extend(ctx.eps.relay_payers.iter().cloned());
    let h = probe::rpc_hit(&url, "getMultipleAccounts", json!([keys, {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}, "commitment": "confirmed"}]));
    match h.json.pointer("/result/value").and_then(|v| v.as_array()) {
        Some(vals) => {
            for (i, (name, id)) in progs.iter().enumerate() {
                let a = &vals[i];
                let (lvl, d) = if a.is_null() {
                    (Level::Fail, format!("{id}: no account on this cluster"))
                } else if a["executable"] != true {
                    (Level::Fail, format!("{id}: account is not executable"))
                } else {
                    (Level::Pass, format!("{} deployed, executable", short(id)))
                };
                out.push(Check::new("Solana", format!("program {name}"), lvl, d).ms(h.ms).at(id));
            }
            for (j, k) in ctx.eps.relay_payers.iter().enumerate() {
                let lam = n(&vals[progs.len() + j], "/lamports");
                let lvl = if lam >= MIN_RELAY_LAMPORTS { Level::Pass } else { Level::Warn };
                out.push(Check::new("Solana", format!("relay payer {}", &k[..6.min(k.len())]), lvl, format!("{} SOL (min 1)", sol(lam as u128))).at(k).data(json!({"lamports": lam})));
            }
        }
        None => out.push(Check::new("Solana", "programs", Level::Warn, h.why().replace(&url, &shown)).ms(h.ms).at(&shown)),
    }
    // the CADRE mint
    let mint = ctx.eps.mint.clone().or_else(|| {
        let d = ctx.sol.account(&crate::solana::bridge_config_pda()).ok()??;
        crate::solana::BridgeConfig::parse(&d).ok().map(|c| crate::solana::b58(&c.mint))
    });
    if let Some(m) = mint {
        let h = probe::rpc_hit(&url, "getAccountInfo", json!([m, {"encoding": "jsonParsed", "commitment": "confirmed"}]));
        let v = h.json.pointer("/result/value").cloned().unwrap_or(Value::Null);
        let (lvl, d) = if v.is_null() {
            (if h.ok() { Level::Fail } else { Level::Warn }, if h.ok() { format!("{m}: no mint account") } else { h.why().replace(&url, &shown) })
        } else {
            let info = v.pointer("/data/parsed/info").cloned().unwrap_or(Value::Null);
            let owner = st(&v, "/owner");
            (Level::Pass, format!("{} supply {} ({} decimals, {})", short(&m), cadre_spl(n128(&info, "/supply")), n(&info, "/decimals"), if owner == crate::solana::TOKEN_2022_PROGRAM { "Token-2022" } else { owner }))
        };
        out.push(Check::new("Solana", "CADRE mint", lvl, d).ms(h.ms).at(&m));
    }
    out
}

/// Bridge invariant (the dashboard's rule): vault SPL x 1000 == 1e18 - escrow, adjusted for
/// transfers in flight; plus deposit and withdrawal counts.
pub fn bridge_check(ctx: &Ctx, summary: Option<&Value>) -> Check {
    let t0 = Instant::now();
    let sn = crate::snapshot::gather_bridge(ctx);
    let ms = t0.elapsed().as_millis() as u64;
    let counts = format!("{} deposits unlocked on the chain, {} withdrawals", sn.deposit_count, sn.withdrawal_count);
    let released = summary.map(|s| format!(" ({} released on Solana)", n(s, "/withdrawals/released_on_solana"))).unwrap_or_default();
    match sn.invariant() {
        Some(i) => {
            let word = if i.strict { "OK" } else if i.ok { "OK (in flight)" } else { "BROKEN" };
            let lvl = if i.ok { Level::Pass } else { Level::Fail };
            Check::new("Economy", "bridge", lvl, format!("{word}: vault {} = outside escrow {}; {counts}{released}", cadre(i.vault_units as u128), cadre(i.outside_units as u128)))
                .ms(ms)
                .data(json!({"invariant": word, "vault_units": i.vault_units.to_string(), "outside_escrow_units": i.outside_units.to_string(), "delta": i.delta.to_string(), "in_flight": i.in_flight.to_string(), "deposits": sn.deposit_count, "withdrawals": sn.withdrawal_count}))
        }
        None => {
            let why = sn.errors.first().cloned().unwrap_or_else(|| "not enough reads".into()).replace(&ctx.eps.solana, &ctx.eps.solana_shown);
            Check::new("Economy", "bridge", Level::Warn, format!("invariant not checked: {why}; {counts}")).ms(ms)
        }
    }
}

/// Checks derived from cadre-api: ready, light client, buyback / reserve / supply, orders.
pub fn api_checks(ctx: &Ctx) -> (Vec<Check>, Option<Value>) {
    let mut out = vec![];
    let Some(api) = ctx.eps.get("api").map(String::from) else {
        out.push(Check::new("Services", "cadre-api", Level::Skip, "no cadre-api endpoint for this network (set CADRE_API)"));
        return (out, None);
    };
    let (ready, summary) = std::thread::scope(|s| {
        let a = s.spawn(|| get(&format!("{api}/ready")));
        let b = s.spawn(|| get(&format!("{api}/summary")));
        (a.join().unwrap(), b.join().unwrap())
    });
    if ready.status == 0 {
        out.push(Check::from_hit_err("Services", "cadre-api", &ready));
    } else {
        let r = &ready.json;
        let failing: Vec<String> = r["checks"]
            .as_object()
            .map(|m| m.iter().filter(|(_, c)| c["ok"] != true).map(|(k, c)| format!("{k}: {}", c["detail"].as_str().map(String::from).unwrap_or_else(|| c["detail"].to_string()))).collect())
            .unwrap_or_default();
        let ups: Vec<String> = r["upstreams"].as_object().map(|m| m.iter().map(|(k, c)| format!("{k} {}", if c["ok"] == true { "up" } else { "down" })).collect()).unwrap_or_default();
        let (lvl, d) = if r["ready"] == true {
            (Level::Pass, format!("ready; {}", ups.join(", ")))
        } else if !failing.is_empty() {
            (Level::Warn, format!("not ready: {}; {}", failing.join("; "), ups.join(", ")))
        } else {
            (Level::Fail, ready.why())
        };
        out.push(Check::new("Services", "cadre-api", lvl, d).ms(ready.ms).at(&ready.url).data(r.clone()));
    }
    if !summary.ok() {
        out.push(Check::new("Solana", "light client", Level::Warn, format!("cadre-api summary: {}", summary.why())).ms(summary.ms));
        return (out, None);
    }
    let s = summary.json.clone();
    // light client
    let lc = &s["light_client_lag"];
    if lc.is_object() {
        let bf = n(lc, "/behind_final");
        let lvl = if bf > MAX_LC_BEHIND_FINAL { Level::Warn } else { Level::Pass };
        out.push(
            Check::new("Solana", "light client", lvl, format!("tip {} final {} chain {}: {} behind the tip, final {} behind (max {})", n(lc, "/tip_height"), n(lc, "/final_height"), n(lc, "/chain_height"), n(lc, "/behind_tip"), bf, MAX_LC_BEHIND_FINAL))
                .ms(summary.ms)
                .at(&summary.url)
                .data(lc.clone()),
        );
    }
    // buyback and reserve
    let bb = &s["buyback"];
    let missing = |what: &str| Check::new("Economy", what, Level::Warn, format!("not in the cadre-api summary now (it reads Solana; status: {})", st(&s, "/status/solana_error")));
    if bb.get("buys").is_none() {
        out.push(missing("buyback"));
    } else {
        let last = n(bb, "/last_buy_ts") * 1000;
        let waiting = n(bb, "/orders_waiting");
        let stale = waiting > 0 && last > 0 && now_ms().saturating_sub(last) > 3600_000;
        out.push(
            Check::new(
                "Economy",
                "buyback",
                if stale { Level::Warn } else { Level::Pass },
                format!(
                    "{} buys, {} SOL spent, {} burned, {} to the reserve, vault {} SOL, {} orders waiting, last buy {}",
                    n(bb, "/buys"),
                    sol(n128(bb, "/spent_lamports")),
                    cadre_spl(n128(bb, "/burned_spl")),
                    cadre_spl(n128(bb, "/to_reserve_spl")),
                    sol(n128(bb, "/vault_lamports")),
                    waiting,
                    if last > 0 { age(now_ms().saturating_sub(last)) } else { "never".into() }
                ),
            )
            .data(bb.clone()),
        );
    }
    let rs = &s["reserve"];
    if rs.get("tokens_spl").is_none() {
        out.push(missing("reserve"));
    } else {
        let last = n(rs, "/last_crank_ts") * 1000;
        out.push(
            Check::new(
                "Economy",
                "reserve",
                Level::Pass,
                format!(
                    "{} held, {} funded to mining, last crank {}",
                    cadre_spl(n128(rs, "/tokens_spl")),
                    cadre_spl(n128(rs, "/totals/funded_spl")),
                    if last > 0 { age(now_ms().saturating_sub(last)) } else { "never".into() }
                ),
            )
            .data(rs.clone()),
        );
    }
    let sp = &s["supply"];
    if sp.get("mint_supply_spl").is_none() {
        out.push(missing("supply"));
    } else {
        out.push(
            Check::new(
                "Economy",
                "supply",
                Level::Pass,
                format!(
                    "mint {} of {} initial, {} burned; bridge vault {}, chain escrow {}",
                    cadre_spl(n128(sp, "/mint_supply_spl")),
                    cadre_spl(n128(sp, "/initial_supply_spl")),
                    cadre_spl(n128(sp, "/burned_spl")),
                    cadre_spl(n128(sp, "/bridge_vault_spl")),
                    cadre(n128(sp, "/agent_chain/escrow_balance"))
                ),
            )
            .data(sp.clone()),
        );
    }
    let so = &s["storefront"];
    if so.is_object() {
        let by: Vec<String> = so["by_status"].as_object().map(|m| m.iter().map(|(k, v)| format!("{k} {v}")).collect()).unwrap_or_default();
        out.push(Check::new("Economy", "orders", Level::Pass, format!("{} storefront orders ({}), {} SOL settled, {} SOL pending", n(so, "/orders"), by.join(", "), sol(n128(so, "/sol_settled")), sol(n128(so, "/sol_pending")))).data(so.clone()));
    }
    (out, Some(s))
}

/// Settle: a pending storefront order whose job is already completed (or expired) on the
/// chain should be finalized (or refunded) on Solana by cadre-settle / the relayer.
pub fn settle_check(ctx: &Ctx, view: &ChainView) -> Check {
    let Some(api) = ctx.eps.get("api") else {
        return Check::new("Services", "settle", Level::Skip, "needs cadre-api for the order list");
    };
    let t0 = Instant::now();
    let h = get(&format!("{api}/orders?status=pending&limit=50"));
    if !h.ok() {
        return Check::new("Services", "settle", Level::Warn, format!("orders: {}", h.why())).ms(h.ms);
    }
    let orders = h.json["orders"].as_array().cloned().unwrap_or_default();
    let mut stuck = vec![];
    if let Some(u) = view.best.as_deref() {
        for o in &orders {
            let id = st(o, "/job_id");
            if id.is_empty() {
                continue;
            }
            if let Ok((j, _)) = rpc(u, "getJob", json!({"job_id": id})) {
                let s = st(&j, "/status");
                if s == "completed" || s == "expired" {
                    stuck.push(format!("order {} (job {} {s} at {})", st(o, "/nonce"), short(id), n(&j, "/closed_height")));
                }
            }
        }
    }
    let ms = t0.elapsed().as_millis() as u64;
    if stuck.is_empty() {
        Check::new("Services", "settle", Level::Pass, format!("{} pending orders, none waiting on a closed job", orders.len())).ms(ms)
    } else {
        Check::new("Services", "settle", Level::Warn, format!("closed on the chain, still pending on Solana: {}", stuck.join(", "))).ms(ms)
    }
}

/// Is the newest completed inference ticket's answer replayable (cadre-glue-v1 bundle)?
pub fn replay_check(ctx: &Ctx, view: &ChainView) -> Check {
    let (Some(api), Some(node)) = (ctx.eps.get("api"), view.best.as_deref()) else {
        return Check::new("Services", "verifiable AI", Level::Skip, "needs cadre-api and a node");
    };
    let t0 = Instant::now();
    let h = get(&format!("{api}/jobs?kind=ticket&status=completed&limit=1"));
    let Some(j) = h.json["jobs"].as_array().and_then(|a| a.first()).cloned() else {
        return Check::new("Services", "verifiable AI", if h.ok() { Level::Skip } else { Level::Warn }, if h.ok() { "no completed ticket yet".into() } else { h.why() }).ms(h.ms);
    };
    let id = st(&j, "/job_id").to_string();
    let r = crate::inspect::ticket_response(node, &id);
    let ms = t0.elapsed().as_millis() as u64;
    match r {
        Ok((job, resp)) => {
            let glue = st(&resp, "/glue");
            let (lvl, what) = if glue == "cadre-glue-v1" { (Level::Pass, "carries the cadre-glue-v1 replay bundle (anyone can replay it: cadre verify)".to_string()) } else { (Level::Warn, "has no replay bundle: GEMM check only".to_string()) };
            Check::new("Services", "verifiable AI", lvl, format!("newest completed ticket {} (closed at {}) {what}", short(&id), n(&job, "/closed_height"))).ms(ms).data(json!({"ticket": id, "glue": glue}))
        }
        Err(e) => Check::new("Services", "verifiable AI", Level::Warn, format!("ticket {}: {e:#}", short(&id))).ms(ms),
    }
}

fn simple_health(group: &'static str, name: &str, url: Option<&str>, path: &str, skip_why: &str, describe: impl Fn(&Value) -> String) -> Check {
    let Some(base) = url else {
        return Check::new(group, name, Level::Skip, skip_why);
    };
    let h = get(&format!("{base}{path}"));
    if h.status == 0 {
        return Check::from_hit_err(group, name, &h);
    }
    let ok_flag = h.json.get("ok").map(|v| v == true).unwrap_or(true);
    if h.ok() && ok_flag {
        Check::new(group, name, Level::Pass, describe(&h.json)).ms(h.ms).at(&h.url).data(h.json)
    } else {
        Check::new(group, name, Level::Fail, format!("{}; {}", h.why(), describe(&h.json))).ms(h.ms).at(&h.url).data(h.json)
    }
}

pub fn service_checks(ctx: &Ctx) -> Vec<Check> {
    let e = &ctx.eps;
    let mut out = vec![];
    // explorer
    out.push(match e.get("explorer") {
        None => Check::new("Services", "explorer", Level::Skip, "no explorer endpoint"),
        Some(x) => {
            let h = get(&format!("{x}/api/summary"));
            if h.ok() {
                let c = h.json.pointer("/index/complete") == Some(&json!(true));
                Check::new("Services", "explorer", if c { Level::Pass } else { Level::Warn }, format!("indexed {} of {} blocks{}", n(&h.json, "/index/blocks_indexed"), n(&h.json, "/index/blocks_on_chain"), if c { "" } else { " (backfilling)" })).ms(h.ms).at(&h.url)
            } else {
                Check::from_hit_err("Services", "explorer", &h)
            }
        }
    });
    out.push(simple_health("Services", "storefront-x402", e.get("storefront"), "/health", "no storefront endpoint", |v| format!("wallet {} on {}", short(st(v, "/wallet")), st(v, "/network"))));
    // relayer: its own verdict, softened when the last good tick is recent
    out.push(match e.get("relayer") {
        None => Check::new("Services", "relayer", Level::Skip, "no relayer endpoint"),
        Some(x) => {
            let h = get(&format!("{x}/health"));
            if h.status == 0 {
                Check::from_hit_err("Services", "relayer", &h)
            } else {
                let v = &h.json;
                let errs: Vec<String> = v.pointer("/last/errors").and_then(|e| e.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                let last_ok = st(v, "/lastOkAt");
                let ago = parse_iso_ms(last_ok).map(|t| now_ms().saturating_sub(t));
                let cur = format!("deposit cursor {}, withdrawal cursor {}", st(v, "/cursors/deposit"), st(v, "/cursors/withdrawal"));
                if v["ok"] == true {
                    Check::new("Services", "relayer", Level::Pass, format!("healthy; {cur}")).ms(h.ms).at(&h.url).data(v.clone())
                } else {
                    let lvl = if ago.is_some_and(|a| a < 5 * 60_000) { Level::Warn } else { Level::Fail };
                    Check::new("Services", "relayer", lvl, format!("unhealthy now ({}), last OK {}; {cur}", if errs.is_empty() { h.why() } else { errs.join("; ") }, ago.map(age).unwrap_or_else(|| "never".into()))).ms(h.ms).at(&h.url).data(v.clone())
                }
            }
        }
    });
    // gateway through cadre-api (models + stats)
    out.push(match e.get("api") {
        None => Check::new("Services", "gateway", Level::Skip, "needs cadre-api"),
        Some(a) => {
            let (m, s) = std::thread::scope(|sc| {
                let x = sc.spawn(|| get(&format!("{a}/openai/models")));
                let y = sc.spawn(|| get(&format!("{a}/openai/stats")));
                (x.join().unwrap(), y.join().unwrap())
            });
            let models: Vec<String> = m.json["data"].as_array().map(|a| a.iter().map(|x| st(x, "/id").to_string()).collect()).unwrap_or_default();
            if !m.ok() {
                Check::new("Services", "gateway", Level::Fail, format!("models: {}", m.why())).ms(m.ms).at(&m.url)
            } else if models.is_empty() {
                Check::new("Services", "gateway", Level::Fail, "serves no model").ms(m.ms).at(&m.url)
            } else {
                let stats = if s.ok() {
                    let tps = s.json["decode_tokens_per_s_mean"].as_f64().map(|t| format!(", decode {t:.2} tok/s (gateway's own measure)")).unwrap_or_default();
                    format!("; {} requests, {} rounds posted{tps}", n(&s.json, "/requests_total"), n(&s.json, "/rounds_posted"))
                } else {
                    String::new()
                };
                Check::new("Services", "gateway", Level::Pass, format!("serves {}{stats}", models.join(", "))).ms(m.ms).at(&m.url).data(json!({"models": m.json["data"], "stats": s.json}))
            }
        }
    });
    // site + installer, docs, pool
    out.push(match e.get("site") {
        None => Check::new("Services", "site", Level::Skip, "no site endpoint for this network (set CADRE_SITE)"),
        Some(x) => {
            let (h, i) = std::thread::scope(|sc| {
                let a = sc.spawn(|| get(x));
                let b = sc.spawn(|| get(&format!("{x}/install-miner.sh")));
                (a.join().unwrap(), b.join().unwrap())
            });
            if h.ok() {
                Check::new("Services", "site", if i.ok() { Level::Pass } else { Level::Warn }, format!("HTTP {} ({} KB); /install-miner.sh {}", h.status, h.bytes / 1024, if i.ok() { "200".to_string() } else { i.why() })).ms(h.ms).at(&h.url)
            } else {
                Check::from_hit_err("Services", "site", &h)
            }
        }
    });
    out.push(match e.get("docs") {
        None => Check::new("Services", "docs site", Level::Skip, "no public docs site for this network (set CADRE_DOCS)"),
        Some(x) => {
            let h = get(x);
            if h.ok() { Check::new("Services", "docs site", Level::Pass, format!("HTTP {} ({} KB)", h.status, h.bytes / 1024)).ms(h.ms).at(&h.url) } else { Check::from_hit_err("Services", "docs site", &h) }
        }
    });
    out.push(match e.get("pool") {
        None => Check::new("Services", "pool", Level::Skip, "this network runs no mining pool"),
        Some(x) => {
            let h = get(&format!("{x}/stats"));
            if h.ok() { Check::new("Services", "pool", Level::Pass, format!("answers /stats ({} miners)", h.json["miners"].as_array().map(|a| a.len()).unwrap_or(0))).ms(h.ms).at(&h.url) } else { Check::from_hit_err("Services", "pool", &h) }
        }
    });
    out
}

pub fn agent_checks(ctx: &Ctx) -> Vec<Check> {
    let mut out = vec![];
    let Some(api) = ctx.eps.get("api").map(String::from) else {
        out.push(Check::new("Agents", "agents", Level::Skip, "needs cadre-api"));
        return out;
    };
    let (town, desk, hosted, hagents) = std::thread::scope(|s| {
        let a = s.spawn(|| get(&format!("{api}/town?limit=100")));
        let b = s.spawn(|| get(&format!("{api}/desktops")));
        let c = s.spawn(|| get(&format!("{api}/hosted")));
        let d = s.spawn(|| get(&format!("{api}/hosted/agents")));
        (a.join().unwrap(), b.join().unwrap(), c.join().unwrap(), d.join().unwrap())
    });
    // house swarm: posting
    if town.ok() {
        let posts = town.json["posts"].as_array().cloned().unwrap_or_default();
        let house: Vec<&Value> = posts.iter().filter(|p| p.pointer("/author/house") == Some(&json!(true))).collect();
        let day = now_ms() / 86_400_000;
        let today: Vec<&&Value> = house.iter().filter(|p| n(p, "/timestamp_ms") / 86_400_000 == day).collect();
        let rows: u64 = today.iter().map(|p| n(p, "/thought/rows")).sum();
        let verified = house.iter().filter(|p| st(p, "/thought/status") == "verified").count();
        let pending = town.json["pending"].as_array().map(|a| a.len()).unwrap_or(0);
        match house.first() {
            Some(p) => {
                let ago = now_ms().saturating_sub(n(p, "/timestamp_ms"));
                let lvl = if ago > HOUSE_POST_WARN_MS { Level::Warn } else { Level::Pass };
                out.push(
                    Check::new(
                        "Agents",
                        "house swarm",
                        lvl,
                        format!(
                            "newest house post @{} at {} ({}); {} of the newest {} town posts are house posts, {} with a verified thought; today {} posts, {} paid rows behind them; {} pending",
                            st(p, "/author/handle"),
                            n(p, "/height"),
                            age(ago),
                            house.len(),
                            posts.len(),
                            verified,
                            today.len(),
                            thousands(rows as u128),
                            pending
                        ),
                    )
                    .ms(town.ms)
                    .at(&town.url)
                    .data(json!({"newest_height": n(p, "/height"), "newest_age_ms": ago, "house_posts": house.len(), "today_posts": today.len(), "today_rows": rows})),
                );
            }
            None => out.push(Check::new("Agents", "house swarm", Level::Warn, format!("no house post among the newest {} town posts", posts.len())).ms(town.ms).at(&town.url)),
        }
    } else {
        out.push(Check::from_hit_err("Agents", "house swarm", &town));
    }
    // house desktops
    if desk.ok() {
        let v = &desk.json;
        let cap = v["cap_usd_per_day"].as_f64().unwrap_or(0.0);
        let spent = v.pointer("/spend_today/total_usd").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let running = n(v, "/desktops_running");
        let agents = v["agents"].as_array().map(|a| a.len()).unwrap_or(0);
        let stopped = v["stopped"] == true;
        let capped = v["capped"] == true;
        let lvl = if stopped || capped || (cap > 0.0 && spent >= 0.9 * cap) { Level::Warn } else { Level::Pass };
        out.push(
            Check::new(
                "Agents",
                "house desktops",
                lvl,
                format!("{running} desktops running for {agents} agents; spend today {spent:.2} of {cap:.0} USD cap{}{}", if stopped { "; STOPPED (kill switch)" } else { "" }, if capped { "; CAPPED for the day" } else { "" }),
            )
            .ms(desk.ms)
            .at(&desk.url)
            .data(json!({"desktops_running": running, "agents": agents, "spend_today_usd": spent, "cap_usd_per_day": cap, "stopped": stopped, "capped": capped})),
        );
    } else if desk.status == 503 {
        out.push(Check::new("Agents", "house desktops", Level::Warn, format!("controller offline ({})", desk.why())).ms(desk.ms).at(&desk.url));
    } else {
        out.push(Check::from_hit_err("Agents", "house desktops", &desk));
    }
    // user-agent runtime
    if hosted.ok() {
        let list = hagents.json["agents"].as_array().cloned().unwrap_or_default();
        let running = list.iter().filter(|a| st(a, "/state") == "running").count();
        let errs = list.iter().filter(|a| a["last_error"].is_string()).count();
        out.push(
            Check::new(
                "Agents",
                "user-agent runtime",
                Level::Pass,
                format!("{} {} answers; {} hosted agents, {running} running{}", st(&hosted.json, "/runtime"), st(&hosted.json, "/version"), n(&hosted.json, "/hosted_agents"), if errs > 0 { format!(", {errs} with a last error (cadre agents)") } else { String::new() }),
            )
            .ms(hosted.ms)
            .at(&hosted.url),
        );
    } else {
        out.push(Check::from_hit_err("Agents", "user-agent runtime", &hosted));
    }
    out
}

/// The 127.0.0.1 services of this box (`--local`).
pub fn local_checks(ctx: &Ctx) -> Vec<Check> {
    let e = &ctx.eps;
    let names = ["gateway", "worker", "attester", "swarm", "desktops", "agents"];
    if names.iter().all(|k| e.get(k).is_none()) {
        return vec![Check::new("Box", "box-local services", Level::Skip, "gateway, worker, attester, swarm, desktops, user-agent runtime: run on box 0 with --local")];
    }
    std::thread::scope(|s| {
        let hs = vec![
            s.spawn(|| simple_health("Box", "local gateway", e.get("gateway"), "/health", "not set", |v| format!("{} model {} ({}), auth {}", st(v, "/api"), st(v, "/model"), short(st(v, "/model_id")), v["auth"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" + ")).unwrap_or_default()))),
            s.spawn(|| simple_health("Box", "local worker", e.get("worker"), "/health", "not set", |v| {
                let g = st(v, "/glue");
                format!("answers{}", if g.is_empty() { String::new() } else { format!("; glue {g}") })
            })),
            s.spawn(|| simple_health("Box", "local attester", e.get("attester"), "/health", "not set", |v| format!("{} mode {}", short(st(v, "/attester")), st(v, "/mode")))),
            s.spawn(|| match e.get("swarm") {
                None => Check::new("Box", "local swarm budget", Level::Skip, "not set"),
                Some(x) => {
                    let h = get(&format!("{x}/status"));
                    if !h.ok() {
                        return Check::from_hit_err("Box", "local swarm budget", &h);
                    }
                    let b = &h.json["budget"];
                    let stopped = h.json["stopped"] == true;
                    let rows = n(b, "/rows");
                    let cap = n(b, "/rows_per_day");
                    Check::new(
                        "Box",
                        "local swarm budget",
                        if stopped || (cap > 0 && rows * 10 >= cap * 9) { Level::Warn } else { Level::Pass },
                        format!("rows today {} of {}{}, thoughts {}, refused {}{}{}", thousands(rows as u128), thousands(cap as u128), if cap > 0 && rows * 10 >= cap * 9 { " (near the daily cap)" } else { "" }, n(b, "/thoughts"), n(b, "/refused"), if stopped { "; STOPPED" } else { "" }, h.json["last_error"].as_str().map(|e| format!("; last error: {e}")).unwrap_or_default()),
                    )
                    .ms(h.ms)
                    .at(&h.url)
                    .data(h.json.clone())
                }
            }),
            s.spawn(|| simple_health("Box", "local desktops", e.get("desktops"), "/health", "not set", |v| format!("{} desktops running{}", n(v, "/desktops_running"), if v["capped"] == true { ", capped" } else { "" }))),
            s.spawn(|| simple_health("Box", "local user-agent runtime", e.get("agents"), "/health", "not set", |v| format!("{} agents", n(v, "/agents")))),
        ];
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Cut prose at the end to `w` characters (`...` marks the cut).
pub fn clip(s: &str, w: usize) -> String {
    if s.chars().count() <= w || w < 8 {
        return s.to_string();
    }
    format!("{}...", s.chars().take(w - 3).collect::<String>().trim_end())
}

fn parse_iso_ms(s: &str) -> Option<u64> {
    // 2026-10-21T19:22:42.888Z
    let (d, t) = s.trim_end_matches('Z').split_once('T')?;
    let mut dp = d.split('-').map(|x| x.parse::<i64>());
    let (y, m, dd) = (dp.next()?.ok()?, dp.next()?.ok()?, dp.next()?.ok()?);
    let mut tp = t.split(':');
    let (hh, mm) = (tp.next()?.parse::<i64>().ok()?, tp.next()?.parse::<i64>().ok()?);
    let ss: f64 = tp.next()?.parse().ok()?;
    // days from civil (Howard Hinnant)
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(((days * 86400 + hh * 3600 + mm * 60) as f64 * 1000.0 + ss * 1000.0) as u64)
}

// ---------------------------------------------------------------------------------------
// Run and render

#[derive(Debug, Clone)]
pub struct Report {
    pub checks: Vec<Check>,
    pub elapsed_ms: u64,
}

impl Report {
    pub fn worst(&self) -> Level {
        self.checks.iter().map(|c| c.level).filter(|l| *l != Level::Skip).max().unwrap_or(Level::Pass)
    }
    pub fn count(&self, l: Level) -> usize {
        self.checks.iter().filter(|c| c.level == l).count()
    }
    /// Exit code for cron / CI: 1 on a failure (or a warning with --strict).
    pub fn code(&self, strict: bool) -> i32 {
        match self.worst() {
            Level::Fail => 1,
            Level::Warn if strict => 1,
            _ => 0,
        }
    }
    pub fn summary_line(&self, network: &str) -> String {
        format!("{} pass · {} warn · {} fail · {} skip in {:.1} s · {network}", self.count(Level::Pass), self.count(Level::Warn), self.count(Level::Fail), self.count(Level::Skip), self.elapsed_ms as f64 / 1000.0)
    }
    pub fn json(&self, network: &str, eps: &Value) -> Value {
        let w = self.worst();
        json!({
            "result": w.word(),
            "network": network,
            "elapsed_ms": self.elapsed_ms,
            "counts": {"pass": self.count(Level::Pass), "warn": self.count(Level::Warn), "fail": self.count(Level::Fail), "skip": self.count(Level::Skip)},
            "checks": self.checks.iter().map(|c| c.json()).collect::<Vec<_>>(),
            "endpoints": eps,
            "taken_ms": now_ms(),
        })
    }
    /// One line per check in the box 0 health log's shape: `LEVEL name detail`.
    pub fn health_lines(&self) -> String {
        self.checks.iter().filter(|c| c.level != Level::Skip).map(|c| format!("{} cli:{} {}", c.level.health_word(), c.name.replace(' ', "_"), c.detail)).collect::<Vec<_>>().join("\n")
    }
    pub fn view(&self, width: Option<usize>, network: &str) -> View {
        let mut v = View(vec![]);
        // one width for every group's table: the same name and detail columns
        let namew = self.checks.iter().map(|c| c.name.chars().count()).max().unwrap_or(5).max(5);
        let longest = self.checks.iter().map(|c| c.detail.chars().count()).max().unwrap_or(6).max(6);
        // box borders + padding (2 + 6), three column gaps (9), state 4, ms 6
        let room = width.map(|w| w.saturating_sub(8 + 9 + namew + 4 + 6 + 1).max(24));
        let detw = room.map(|r| r.min(longest)).unwrap_or(longest);
        for g in GROUPS {
            let rows: Vec<&Check> = self.checks.iter().filter(|c| c.group == g).collect();
            if rows.is_empty() {
                continue;
            }
            v.0.push(Block::Table(Table {
                chip: Some(g.to_string()),
                head: ["CHECK", "STATE", "DETAIL", "MS"].map(String::from).to_vec(),
                right: vec![false, false, false, true],
                rows: rows
                    .iter()
                    .map(|c| {
                        let d = clip(&c.detail, detw);
                        let d = format!("{d}{}", " ".repeat(detw.saturating_sub(d.chars().count())));
                        let name = format!("{}{}", c.name, " ".repeat(namew.saturating_sub(c.name.chars().count())));
                        vec![(name, Tone::Plain), (c.level.word().to_uppercase(), c.level.tone()), (d, if c.level == Level::Skip { Tone::Dim } else { Tone::Plain }), (c.ms.map(|m| m.to_string()).unwrap_or_default(), Tone::Dim)]
                    })
                    .collect(),
                empty: String::new(),
            }));
        }
        let w = self.worst();
        let mut hint = self.summary_line(network);
        let bad: Vec<String> = self.checks.iter().filter(|c| matches!(c.level, Level::Fail | Level::Warn)).map(|c| format!("{} {}", c.level.word(), c.name)).collect();
        if !bad.is_empty() {
            hint += &format!("\n{}", bad.join(", "));
        }
        hint += &format!("\n{}", if w == Level::Fail { "result FAIL (exit 1)" } else if w == Level::Warn { "result WARN" } else { "result PASS" });
        v.hint(hint)
    }
}

/// Everything, in parallel.
pub fn run_all(ctx: &Ctx) -> Report {
    let t0 = Instant::now();
    let mut checks: Vec<Check> = std::thread::scope(|s| {
        let chain = s.spawn(|| {
            let (c, view) = chain_checks(ctx);
            let (api, summary) = api_checks(ctx);
            let (m, b, st, rp) = std::thread::scope(|s2| {
                let m = s2.spawn(|| mining_check(&view, summary.as_ref()));
                let b = s2.spawn(|| bridge_check(ctx, summary.as_ref()));
                let st = s2.spawn(|| settle_check(ctx, &view));
                let rp = s2.spawn(|| replay_check(ctx, &view));
                (m.join().unwrap(), b.join().unwrap(), st.join().unwrap(), rp.join().unwrap())
            });
            (c, api, vec![st, rp], vec![m, b])
        });
        let sol = s.spawn(|| solana_checks(ctx));
        let svc = s.spawn(|| service_checks(ctx));
        let ag = s.spawn(|| agent_checks(ctx));
        let lo = s.spawn(|| local_checks(ctx));
        let (c, api, late_svc, econ) = chain.join().unwrap();
        let mut all = c;
        all.extend(sol.join().unwrap());
        all.extend(api);
        all.extend(svc.join().unwrap());
        all.extend(late_svc);
        all.extend(ag.join().unwrap());
        all.extend(econ);
        all.extend(lo.join().unwrap());
        all
    });
    // stable order: by group, then as assembled above
    checks.sort_by_key(|c| GROUPS.iter().position(|g| *g == c.group).unwrap_or(99));
    Report { checks, elapsed_ms: t0.elapsed().as_millis() as u64 }
}

// ---------------------------------------------------------------------------------------
// ping

pub const PING_TARGETS: &str = "nodes (or node0, node1, ...), solana, programs, lightclient, api, explorer, storefront, relayer, gateway, settle, replay, site, docs, pool, swarm, desktops, agents, bridge, mining, box (local services), or a URL";

/// Checks for one target (`cadre ping <target>`).
pub fn ping_once(ctx: &Ctx, target: &str) -> Result<Vec<Check>> {
    let t = target.to_ascii_lowercase();
    let pick = |v: Vec<Check>, names: &[&str]| -> Vec<Check> { v.into_iter().filter(|c| names.iter().any(|n| c.name.starts_with(n))).collect() };
    Ok(match t.as_str() {
        "nodes" | "node" | "rpc" | "chain" => chain_checks(ctx).0,
        x if x.starts_with("node") && x[4..].parse::<usize>().is_ok() => {
            let i: usize = x[4..].parse().unwrap();
            let u = ctx.eps.nodes.get(i).ok_or_else(|| anyhow::anyhow!("no node {i}: {} nodes configured ({})", ctx.eps.nodes.len(), ctx.eps.nodes.join(", ")))?;
            vec![match rpc(u, "getChainInfo", json!({})) {
                Ok((v, ms)) => Check::new("Chain", format!("node {}", node_name(u)), Level::Pass, format!("height {} final {} peers {}", n(&v, "/height"), n(&v, "/finalized_height"), n(&v, "/peers"))).ms(ms).at(u),
                Err(e) => Check::new("Chain", format!("node {}", node_name(u)), Level::Fail, format!("{e:#}")).at(u),
            }]
        }
        "solana" => pick(solana_checks(ctx), &["solana rpc"]),
        "programs" | "mint" => pick(solana_checks(ctx), &["program", "CADRE mint"]),
        "relays" | "relay" => pick(solana_checks(ctx), &["relay payer"]),
        "lightclient" | "light-client" => pick(api_checks(ctx).0, &["light client"]),
        "api" | "cadre-api" => pick(api_checks(ctx).0, &["cadre-api"]),
        "economy" | "buyback" | "reserve" | "supply" => pick(api_checks(ctx).0, &["buyback", "reserve", "supply", "orders"]),
        "explorer" | "storefront" | "relayer" | "gateway" | "site" | "docs" | "pool" => {
            let want = if t == "storefront" { "storefront-x402".to_string() } else if t == "docs" { "docs site".into() } else { t.clone() };
            service_checks(ctx).into_iter().filter(|c| c.name == want).collect()
        }
        "settle" => vec![settle_check(ctx, &chain_checks(ctx).1)],
        "replay" | "verifiable-ai" => vec![replay_check(ctx, &chain_checks(ctx).1)],
        "bridge" => vec![bridge_check(ctx, None)],
        "mining" => vec![mining_check(&chain_checks(ctx).1, None)],
        "swarm" | "desktops" | "agents" | "town" => {
            let a = agent_checks(ctx);
            match t.as_str() {
                "swarm" | "town" => pick(a, &["house swarm"]),
                "desktops" => pick(a, &["house desktops"]),
                _ => pick(a, &["user-agent runtime"]),
            }
        }
        "box" | "local" => local_checks(ctx),
        x if x.starts_with("http://") || x.starts_with("https://") => {
            let h = get(target);
            vec![if h.ok() { Check::new("Services", target, Level::Pass, format!("HTTP {} ({} bytes)", h.status, h.bytes)).ms(h.ms).at(target) } else { Check::from_hit_err("Services", target, &h) }]
        }
        _ => anyhow::bail!("unknown target {target:?}; targets: {PING_TARGETS}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_parse() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.500Z"), Some(1500));
        assert_eq!(parse_iso_ms("2026-10-21T19:22:42.888Z"), Some(1_792_610_562_888));
        assert_eq!(parse_iso_ms("junk"), None);
    }

    #[test]
    fn worst_and_exit_codes() {
        let r = Report { checks: vec![Check::new("Chain", "a", Level::Pass, ""), Check::new("Chain", "b", Level::Skip, "")], elapsed_ms: 1 };
        assert_eq!((r.worst(), r.code(false), r.code(true)), (Level::Pass, 0, 0));
        let r = Report { checks: vec![Check::new("Chain", "a", Level::Warn, "")], elapsed_ms: 1 };
        assert_eq!((r.code(false), r.code(true)), (0, 1));
        let r = Report { checks: vec![Check::new("Chain", "a", Level::Warn, ""), Check::new("Chain", "b", Level::Fail, "x")], elapsed_ms: 1 };
        assert_eq!((r.worst(), r.code(false)), (Level::Fail, 1));
        assert_eq!(r.health_lines(), "WARN cli:a \nFAIL cli:b x");
    }
}
