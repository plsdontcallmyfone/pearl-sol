//! One read of everything the dashboard shows. Every figure on screen comes from a
//! field here, and every field is an RPC read (agent-chain node, Solana, pool) or
//! arithmetic over such reads, named in `derived`. Nothing is estimated or priced:
//! there is no price source, so none is shown.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use pearl_node::num;

use crate::ctx::Ctx;
use crate::solana::{self, b58, BridgeConfig, StorefrontConfig, CHAIN_AGENT, CHAIN_TOKEN};

/// Recent blocks read for /Mining and the pending (immature) balance.
pub const BLOCK_WINDOW: u64 = 24;
/// Bridge nonces scanned for in-flight transfers (older ones are taken as settled).
pub const BRIDGE_WINDOW: u64 = 64;

#[derive(Debug, Clone, Default)]
pub struct SolSide {
    pub error: Option<String>,
    pub bridge: Option<BridgeConfig>,
    pub store: Option<StorefrontConfig>,
    pub vault_spl: Option<u64>,
    pub my_spl: Option<u64>,
    pub my_sol: Option<u64>,
    /// Withdrawal nonce -> released on Solana (Processed PDA exists).
    pub released: Vec<(u64, bool)>,
    /// SPL of Solana deposits in the window not yet unlocked on the chain nor refunded.
    pub inflight_dep_spl: Option<u64>,
    pub inflight_dep_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invariant {
    pub vault_units: i128,
    pub outside_units: i128,
    /// vault x 1000 - (1e18 - escrow).
    pub delta: i128,
    /// What in-flight transfers explain, chain units: deposits locked on Solana but not
    /// yet unlocked on the chain, plus withdrawals locked on the chain but not yet
    /// released from the vault (both still sit in the vault).
    pub in_flight: i128,
    /// delta == in_flight.
    pub ok: bool,
    /// delta == 0 (nothing in flight).
    pub strict: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub taken_ms: u64,
    pub me: Option<String>,
    pub sol_me: Option<String>,
    pub errors: Vec<String>,
    pub info: Value,
    pub params: Value,
    pub escrow: Value,
    pub account: Value,
    pub agent: Value,
    pub route: Value,
    /// listJobs page, posting order.
    pub jobs: Vec<Value>,
    pub jobs_total: u64,
    /// Canonical blocks, newest first.
    pub blocks: Vec<Value>,
    pub txs: Vec<Value>,
    pub withdrawals: Vec<Value>,
    pub withdrawal_count: u64,
    pub deposit_count: u64,
    pub sol: SolSide,
    pub pool: Option<Value>,
    pub pool_miner: Option<Value>,
    /// "not connected" | "reachable" | "unreachable: ..."
    pub credit: String,
    pub miner_pid: Option<u32>,
}

/// u64 from a JSON field (number or decimal string), 0 if absent.
pub fn u(v: &Value, k: &str) -> u64 {
    v.get(k).filter(|x| !x.is_null()).and_then(|x| num(x).ok()).unwrap_or(0)
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

impl Snapshot {
    pub fn chain_ok(&self) -> bool {
        self.info.is_object()
    }

    pub fn height(&self) -> u64 {
        u(&self.info, "height")
    }

    pub fn finality_k(&self) -> u64 {
        u(&self.info, "finality_k")
    }

    pub fn balance(&self) -> Option<u64> {
        self.account.is_object().then(|| u(&self.account, "balance"))
    }

    /// Rewards of this account's blocks that are not yet K deep (credited after K).
    pub fn pending_rewards(&self) -> u64 {
        let Some(me) = &self.me else { return 0 };
        self.blocks
            .iter()
            .filter(|b| b["header_fields"]["miner"].as_str() == Some(me.as_str()) && b["final"] == false)
            .map(|b| u(b, "reward"))
            .sum()
    }

    /// Rewards of this account's blocks in the window (final or not).
    pub fn window_rewards(&self) -> (u64, usize) {
        let Some(me) = &self.me else { return (0, 0) };
        let mine: Vec<&Value> = self.blocks.iter().filter(|b| b["header_fields"]["miner"].as_str() == Some(me.as_str())).collect();
        (mine.iter().map(|b| u(b, "reward")).sum(), mine.len())
    }

    /// Job block subsidy now: `pool x emission_ppm / 1e6` (capped per job by its
    /// cap_remaining). None until both reads are in.
    pub fn job_subsidy(&self) -> Option<u64> {
        if !self.escrow.is_object() || !self.params.is_object() {
            return None;
        }
        Some((u(&self.escrow, "pool_balance") as u128 * u(&self.params, "emission_ppm") as u128 / 1_000_000) as u64)
    }

    pub fn inflight_wd_spl(&self) -> u64 {
        let rel: std::collections::HashMap<u64, bool> = self.sol.released.iter().copied().collect();
        self.withdrawals.iter().filter(|w| rel.get(&u(w, "nonce")) == Some(&false)).map(|w| u(w, "amount_spl")).sum()
    }

    /// `vault SPL x 1000 == 1e18 - escrow`, adjusted for transfers in flight.
    pub fn invariant(&self) -> Option<Invariant> {
        let vault = self.sol.vault_spl? as i128 * 1000;
        if !self.escrow.is_object() {
            return None;
        }
        let outside = u(&self.escrow, "outside_escrow") as i128;
        let dep = self.sol.inflight_dep_spl? as i128 * 1000;
        let wd = self.inflight_wd_spl() as i128 * 1000;
        let delta = vault - outside;
        Some(Invariant { vault_units: vault, outside_units: outside, delta, in_flight: dep + wd, ok: delta == dep + wd, strict: delta == 0 })
    }

    pub fn to_json(&self) -> Value {
        let inv = self.invariant();
        let (earned, mined) = self.window_rewards();
        json!({
            "taken_ms": self.taken_ms,
            "address": self.me, "solana_address": self.sol_me,
            "errors": self.errors,
            "chain": self.info, "params": self.params, "escrow": self.escrow,
            "account": self.account, "agent": self.agent, "route": self.route,
            "jobs": {"total": self.jobs_total, "jobs": self.jobs},
            "blocks": self.blocks.iter().map(|b| json!({
                "height": b["header_fields"]["height"], "hash": b["hash"], "miner": b["header_fields"]["miner"],
                "job_id": b["header_fields"]["job_id"], "fees": b["fees"], "subsidy": b["subsidy"], "reward": b["reward"],
                "txs": b["txs"].as_array().map(|a| a.len()), "final": b["final"]})).collect::<Vec<_>>(),
            "activity": self.txs,
            "bridge": {
                "withdrawal_count": self.withdrawal_count, "deposit_count": self.deposit_count,
                "withdrawals": self.withdrawals.iter().map(|w| {
                    let n = u(w, "nonce");
                    let mut w = w.clone();
                    w["released"] = json!(self.sol.released.iter().find(|(k, _)| *k == n).map(|(_, r)| *r));
                    w
                }).collect::<Vec<_>>(),
            },
            "solana": {
                "error": self.sol.error,
                "bridge_config": self.sol.bridge.as_ref().map(|c| json!({
                    "mint": b58(&c.mint), "vault": b58(&c.vault), "token_program": b58(&c.token_program),
                    "deposit_nonce": c.deposit_nonce, "threshold": c.threshold, "signers": c.signers.len(),
                    "release_modes": c.release_modes})),
                "storefront_config": self.sol.store.as_ref().map(|c| json!({
                    "paused": c.paused, "protocol_fee_bps": c.protocol_fee_bps, "min_sol_in": c.min_sol_in.to_string(),
                    "max_sol_in": c.max_sol_in.to_string(), "max_dim": c.max_dim, "max_blocks_cap": c.max_blocks_cap,
                    "max_deadline_blocks": c.max_deadline_blocks, "orders": c.orders, "total_sol": c.total_sol.to_string(),
                    "total_tokens_spl": c.total_tokens.to_string()})),
                "vault_spl": self.sol.vault_spl.map(|x| x.to_string()),
                "spl_balance": self.sol.my_spl.map(|x| x.to_string()),
                "sol_balance": self.sol.my_sol.map(|x| x.to_string()),
                "inflight_deposits_spl": self.sol.inflight_dep_spl.map(|x| x.to_string()),
                "inflight_deposit_count": self.sol.inflight_dep_count,
            },
            "pool": self.pool, "pool_miner": self.pool_miner,
            "agent_credit": self.credit,
            "miner_pid": self.miner_pid,
            "derived": {
                "pending_rewards": self.pending_rewards().to_string(),
                "window_rewards": earned.to_string(), "window_blocks_mined": mined, "block_window": self.blocks.len(),
                "job_subsidy": self.job_subsidy().map(|x| x.to_string()),
                "inflight_withdrawals_spl": self.inflight_wd_spl().to_string(),
                "invariant": inv.map(|i| json!({
                    "vault_units": i.vault_units.to_string(), "outside_escrow": i.outside_units.to_string(),
                    "delta": i.delta.to_string(), "in_flight": i.in_flight.to_string(), "ok": i.ok, "strict": i.strict})),
            },
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Read everything. Failures are collected in `errors`; the rest still renders.
pub fn gather(ctx: &Ctx) -> Snapshot {
    let mut sn = Snapshot {
        taken_ms: now_ms(),
        me: ctx.me.as_ref().map(b58),
        sol_me: ctx.sol_me.as_ref().map(b58),
        miner_pid: ctx.miner_pid(),
        ..Default::default()
    };
    let mut err = |sn: &mut Snapshot, what: &str, e: anyhow::Error| sn.errors.push(format!("{what}: {e}"));

    match ctx.call("getChainInfo", json!({})) {
        Ok(v) => sn.info = v,
        Err(e) => err(&mut sn, "agent chain", e),
    }
    if sn.chain_ok() {
        gather_chain(ctx, &mut sn, &mut err);
    }
    gather_solana(ctx, &mut sn);

    // pool (optional service)
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_millis(800)).build();
    let get = |path: &str| -> Option<Value> {
        let base = ctx.pool.as_deref()?;
        agent.get(&format!("{}{}", base.trim_end_matches('/'), path)).call().ok()?.into_json().ok()
    };
    sn.pool = get("/stats");
    if sn.pool.is_some() {
        if let Some(me) = &sn.me {
            sn.pool_miner = get(&format!("/miner?payout={me}"));
        }
    }
    sn.credit = match &ctx.opts.credit_url {
        None => "not connected".into(),
        Some(url) => match agent.get(url).call() {
            Ok(_) => "reachable".into(),
            Err(e) => format!("unreachable: {e}"),
        },
    };
    sn
}

fn gather_chain(ctx: &Ctx, sn: &mut Snapshot, err: &mut impl FnMut(&mut Snapshot, &str, anyhow::Error)) {
    for (m, slot) in [("getParams", 0), ("getEscrow", 1)] {
        match ctx.call(m, json!({})) {
            Ok(v) if slot == 0 => sn.params = v,
            Ok(v) => sn.escrow = v,
            Err(e) => err(sn, m, e),
        }
    }
    if let Some(me) = sn.me.clone() {
        let a = json!({"account": me});
        match ctx.call("getAccount", a.clone()) {
            Ok(v) => {
                sn.route = v.get("route").cloned().unwrap_or(Value::Null);
                sn.account = v;
            }
            Err(e) => err(sn, "getAccount", e),
        }
        match ctx.call("getAgent", a.clone()) {
            Ok(v) => sn.agent = v,
            Err(e) => err(sn, "getAgent", e),
        }
        match ctx.call("getRoute", a) {
            Ok(v) => sn.route = v,
            Err(e) => err(sn, "getRoute", e),
        }
        match ctx.call("getAccountTxs", json!({"account": me, "limit": 50})) {
            Ok(v) => sn.txs = v["txs"].as_array().cloned().unwrap_or_default(),
            Err(e) => err(sn, "getAccountTxs", e),
        }
    }
    // newest 50 jobs
    match ctx.call("listJobs", json!({"limit": 1})) {
        Ok(v) => {
            sn.jobs_total = u(&v, "total");
            let off = sn.jobs_total.saturating_sub(50);
            match ctx.call("listJobs", json!({"offset": off, "limit": 50})) {
                Ok(v) => sn.jobs = v["jobs"].as_array().cloned().unwrap_or_default(),
                Err(e) => err(sn, "listJobs", e),
            }
        }
        Err(e) => err(sn, "listJobs", e),
    }
    let tip = sn.height();
    let win = BLOCK_WINDOW.max(sn.finality_k() + 1).min(64);
    for h in (tip.saturating_sub(win - 1)..=tip).rev() {
        match ctx.call("getBlock", json!({"height": h})) {
            Ok(v) if v.is_object() => sn.blocks.push(v),
            Ok(_) => {}
            Err(e) => {
                err(sn, "getBlock", e);
                break;
            }
        }
    }
    match ctx.call("getBridgeEvents", json!({"limit": 1})) {
        Ok(v) => {
            sn.withdrawal_count = u(&v, "withdrawal_count");
            sn.deposit_count = u(&v, "deposit_count");
            let from = sn.withdrawal_count.saturating_sub(BRIDGE_WINDOW);
            match ctx.call("getBridgeEvents", json!({"from_nonce": from, "limit": BRIDGE_WINDOW})) {
                Ok(v) => sn.withdrawals = v["withdrawals"].as_array().cloned().unwrap_or_default(),
                Err(e) => err(sn, "getBridgeEvents", e),
            }
        }
        Err(e) => err(sn, "getBridgeEvents", e),
    }
}

fn gather_solana(ctx: &Ctx, sn: &mut Snapshot) {
    let r = (|| -> anyhow::Result<()> {
        let rpc = &ctx.sol;
        if let Some(me) = &ctx.sol_me {
            sn.sol.my_sol = Some(rpc.balance(me)?);
        }
        if let Some(d) = rpc.account(&solana::storefront_config_pda())? {
            sn.sol.store = Some(StorefrontConfig::parse(&d)?);
        }
        let Some(d) = rpc.account(&solana::bridge_config_pda())? else {
            anyhow::bail!("bridge not initialised on this cluster");
        };
        let cfg = BridgeConfig::parse(&d)?;
        sn.sol.vault_spl = rpc.token_balance(&cfg.vault)?;
        if let Some(me) = &ctx.sol_me {
            sn.sol.my_spl = Some(rpc.token_balance(&solana::ata(me, &cfg.mint, &cfg.token_program))?.unwrap_or(0));
        }
        // withdrawals released?
        let nonces: Vec<u64> = sn.withdrawals.iter().map(|w| u(w, "nonce")).collect();
        let pdas: Vec<_> = nonces.iter().map(|n| solana::processed_pda(CHAIN_AGENT, *n)).collect();
        let got = rpc.accounts(&pdas)?;
        sn.sol.released = nonces.into_iter().zip(got.iter().map(|a| a.is_some())).collect();
        // deposits in flight: in the window, not processed on the chain, not refunded
        if sn.chain_ok() {
            let lo = cfg.deposit_nonce.saturating_sub(BRIDGE_WINDOW);
            let range: Vec<u64> = (lo..cfg.deposit_nonce).collect();
            let deps = rpc.accounts(&range.iter().map(|n| solana::deposit_pda(*n)).collect::<Vec<_>>())?;
            let refunds = rpc.accounts(&range.iter().map(|n| solana::processed_pda(CHAIN_TOKEN, *n)).collect::<Vec<_>>())?;
            let (mut amt, mut cnt) = (0u64, 0u64);
            for (i, n) in range.iter().enumerate() {
                if refunds[i].is_some() {
                    continue;
                }
                let Some(d) = &deps[i] else { continue };
                let p = ctx.call("getDeposit", json!({"solana_nonce": n}))?;
                if p["processed"] != true {
                    amt += solana::deposit_amount(d)?;
                    cnt += 1;
                }
            }
            sn.sol.inflight_dep_spl = Some(amt);
            sn.sol.inflight_dep_count = cnt;
        }
        sn.sol.bridge = Some(cfg);
        Ok(())
    })();
    if let Err(e) = r {
        sn.sol.error = Some(e.to_string());
        sn.errors.push(format!("solana: {e}"));
    }
}

#[cfg(test)]
pub mod sample {
    //! A fixed snapshot for render and formatting tests (values shaped like a demo run).
    use super::*;

    pub const ME: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";

    pub fn snapshot() -> Snapshot {
        let other = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
        let blocks = (0..8u64)
            .map(|i| {
                let h = 260 - i;
                let mine = i % 3 == 0;
                json!({"hash": format!("{:064x}", h), "header_fields": {"height": h, "miner": if mine { ME } else { other },
                       "job_id": if i == 2 { format!("{:064x}", 0xabcdu64) } else { "0".repeat(64) }},
                       "fees": "2000", "subsidy": if i == 2 { "45000000" } else { "0" },
                       "reward": if i == 2 { "45002000" } else if mine { "1000000000" } else { "2000" },
                       "txs": [], "final": i >= 6})
            })
            .collect();
        Snapshot {
            taken_ms: 1,
            me: Some(ME.into()),
            sol_me: Some(ME.into()),
            info: json!({"chain_id": 1347616769u32, "height": 260, "finalized_height": 254, "finality_k": 6, "target_bits": 0x1f00ffffu32,
                         "next_target_bits": 0x1f00ffffu32, "peers": 2, "mempool_size": 0}),
            params: json!({"emission_ppm": 10000, "alpha_ppm": 800000, "min_fee": "1000", "min_protocol_fee_effective": "2"}),
            escrow: json!({"escrow_balance": "981163606850158000", "pool": "P", "pool_balance": "9000000000000000",
                           "total_supply": "1000000000000000000", "outside_escrow": "18836393149842000"}),
            account: json!({"balance": "40740000000", "nonce": 3}),
            agent: json!({"account": ME, "name": "demo-agent", "uri": "https://example.invalid/agent", "registered_height": 40}),
            route: json!({"solana_recipient": other, "route_bps": 5000, "until_height": 10279, "active": true,
                          "routed": "1000000500", "withdrawn": "1000000000", "pending": "500", "remaining_cap": "999998999999500"}),
            jobs: vec![
                json!({"job_id": "aa".repeat(32), "status": "completed", "m": 64, "n": 64, "k": 2048, "worker_pay": "5000000000",
                       "protocol_fee": "1000000000", "blocks_mined": 1, "max_blocks": 10, "deadline_height": 900}),
                json!({"job_id": "bb".repeat(32), "status": "open", "m": 128, "n": 64, "k": 2048, "worker_pay": "7500000000",
                       "protocol_fee": "1500000000", "blocks_mined": 0, "max_blocks": 4, "deadline_height": 1200}),
            ],
            jobs_total: 2,
            blocks,
            txs: vec![json!({"tx_hash": "cc".repeat(32), "kind": "Transfer", "role": "sender", "fee": "1000", "height": 250, "final": true})],
            withdrawals: vec![json!({"nonce": 0, "amount_spl": "1000000", "final": true}), json!({"nonce": 1, "amount_spl": "250000", "final": false})],
            withdrawal_count: 2,
            deposit_count: 5,
            sol: SolSide {
                error: None,
                bridge: None,
                store: Some(StorefrontConfig {
                    paused: false,
                    protocol_fee_bps: 2000,
                    max_slippage_bps: 300,
                    min_sol_in: 10_000_000,
                    max_sol_in: 10_000_000_000,
                    max_dim: 4096,
                    max_blocks_cap: 1000,
                    max_deadline_blocks: 100_000,
                    orders: 2,
                    total_sol: 200_000_000,
                    total_tokens: 6_000_000_000_000,
                }),
                // vault x 1000 - outside = 250000 x 1000: withdrawal 1 in flight (not released)
                vault_spl: Some(18_836_393_399_842),
                my_spl: Some(8_690_000_000_000),
                my_sol: Some(2_500_000_000),
                released: vec![(0, true), (1, false)],
                inflight_dep_spl: Some(0),
                inflight_dep_count: 0,
            },
            pool: None,
            pool_miner: None,
            credit: "not connected".into(),
            miner_pid: None,
            errors: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_figures() {
        let s = sample::snapshot();
        // blocks i = 0, 3 (not final) mined by ME: 2 x 1 token; i = 6 is final.
        assert_eq!(s.pending_rewards(), 2_000_000_000);
        assert_eq!(s.window_rewards(), (3_000_000_000, 3));
        assert_eq!(s.job_subsidy(), Some(90_000_000_000_000)); // 9e15 x 10000 ppm
        assert_eq!(s.inflight_wd_spl(), 250_000);
        let i = s.invariant().unwrap();
        // withdrawal 1 (250000 SPL) is locked on the chain, not released: the vault is
        // exactly that much above 1e18 - escrow.
        assert_eq!(i.delta, 250_000_000);
        assert_eq!(i.in_flight, 250_000_000);
        assert!(i.ok && !i.strict);
        let mut s2 = s.clone();
        s2.sol.vault_spl = Some(18_836_393_149_842 - 1);
        assert!(!s2.invariant().unwrap().ok);
        s2.sol.released = vec![(0, true), (1, true)];
        s2.sol.vault_spl = Some(18_836_393_149_842);
        assert!(s2.invariant().unwrap().strict);
        let j = s.to_json();
        assert_eq!(j["derived"]["invariant"]["ok"], true);
        assert_eq!(j["bridge"]["withdrawals"][1]["released"], false);
    }
}
