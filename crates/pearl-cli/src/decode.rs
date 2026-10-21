//! Transactions in plain words. Cadre chain txs are decoded with the chain's own codec
//! (`agent_types::tx::Tx::decode`), so every kind the chain knows is named; a kind this
//! build does not describe yet (for example one added on another branch) still shows the
//! node's kind name and its sender, fee and nonce. Solana txs are read with `jsonParsed`
//! (system, SPL token, memo come parsed from the RPC) and our four programs' instructions
//! are named from their Anchor discriminators, with the arguments that matter decoded.

use base64::Engine as _;
use serde_json::{json, Value};

use agent_types::tx::{Tx, TxBody};

use crate::fmt::{cx, cxs, short, sol, spl, tokens};
use crate::solana::{anchor_disc, b58};

/// One tx in words: a headline, labelled fields and the same as JSON.
#[derive(Debug, Clone, Default)]
pub struct Words {
    pub kind: String,
    pub headline: String,
    pub fields: Vec<(String, String)>,
}

impl Words {
    fn new(kind: &str, headline: impl Into<String>) -> Self {
        Words { kind: kind.into(), headline: headline.into(), fields: vec![] }
    }
    fn f(mut self, k: &str, v: impl Into<String>) -> Self {
        self.fields.push((k.into(), v.into()));
        self
    }
    pub fn json(&self) -> Value {
        json!({"kind": self.kind, "words": self.headline, "fields": self.fields.iter().map(|(k, v)| json!({"label": k, "value": v})).collect::<Vec<_>>()})
    }
}

fn cadre(u: u64) -> String {
    format!("{} CADRE", tokens(u as u128))
}
fn cadre_spl(u: u64) -> String {
    format!("{} CADRE", spl(u as u128))
}
fn hx(h: &[u8; 32]) -> String {
    hex::encode(h)
}
fn text(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    let s: String = s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    s.replace('\n', " / ")
}
fn quote(b: &[u8], max: usize) -> String {
    let t = text(b);
    if t.chars().count() > max {
        format!("\"{}...\"", t.chars().take(max).collect::<String>())
    } else {
        format!("\"{t}\"")
    }
}
fn is_zero(k: &[u8; 32]) -> bool {
    k.iter().all(|b| *b == 0)
}

/// AdminConfig key byte -> name, from getParams `keys` when given.
pub fn config_key_name(params: &Value, key: u8) -> String {
    params["keys"].as_object().and_then(|m| m.iter().find(|(_, v)| v.as_u64() == Some(key as u64)).map(|(k, _)| k.clone())).unwrap_or_else(|| format!("key {key}"))
}

/// Decode a chain tx from its base64 bytes.
pub fn chain_tx_b64(b64: &str) -> Option<Tx> {
    let b = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Tx::decode(&b).ok()
}

/// Words for a chain tx. `node_kind` is the node's kind name (fallback when the bytes do
/// not decode with this build's codec); `params` (getParams) names AdminConfig keys.
pub fn chain_words(tx: Option<&Tx>, node_kind: &str, from: &str, params: &Value) -> Words {
    let Some(tx) = tx else {
        return Words::new(node_kind, format!("{node_kind} from {} (this build of the CLI cannot decode the body of this kind yet)", crate::fmt::short_addr(from)));
    };
    let from = cx(&tx.from);
    let w = match &tx.body {
        TxBody::Transfer { to, amount } => Words::new("Transfer", format!("{} sent {} to {}", cxs(&tx.from), cadre(*amount), cxs(to))).f("To", cx(to)).f("Amount", cadre(*amount)),
        TxBody::RegisterAgent { name, uri } => Words::new("RegisterAgent", format!("{} registered as agent {}", cxs(&tx.from), quote(name, 64))).f("Name", text(name)).f("Card URI", text(uri)),
        TxBody::PostJob(p) => {
            let id = agent_types::blake3_hash(&[&tx.from, &tx.nonce.to_le_bytes()]);
            Words::new("PostJob", format!("{} posted a {}x{}x{} matmul job paying {} to the worker and {} to the mining pool", cxs(&tx.from), p.m, p.n, p.k, cadre(p.worker_pay), cadre(p.protocol_fee)))
                .f("Job id", hx(&id))
                .f("Worker pay W", cadre(p.worker_pay))
                .f("Protocol fee P", cadre(p.protocol_fee))
                .f("Deadline", format!("block {}", p.deadline_height))
                .f("Max blocks", p.max_blocks.to_string())
                .f("Evaluators", format!("{} of {}: {}", p.threshold, p.evaluators.len(), p.evaluators.iter().map(|e| cxs(e)).collect::<Vec<_>>().join(", ")))
                .f("Data", text(&p.data_uri))
        }
        TxBody::SubmitResult { job_id, result_hash, uri } => Words::new("SubmitResult", format!("{} delivered a result for job {}", cxs(&tx.from), short(&hx(job_id)))).f("Job", hx(job_id)).f("Result hash", hx(result_hash)).f("Result URI", text(uri)),
        TxBody::CompleteJob { job_id, sigs } => Words::new("CompleteJob", format!("job {} completed with {} evaluator signature{}", short(&hx(job_id)), sigs.len(), if sigs.len() == 1 { "" } else { "s" }))
            .f("Job", hx(job_id))
            .f("Signed by", sigs.iter().map(|s| cxs(&s.pubkey)).collect::<Vec<_>>().join(", ")),
        TxBody::ExpireJob { job_id } => Words::new("ExpireJob", format!("job {} expired (refund to the poster)", short(&hx(job_id)))).f("Job", hx(job_id)),
        TxBody::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs } => {
            let mut w = Words::new(
                "BridgeUnlock",
                format!("Solana deposit #{solana_nonce} unlocked {} to {}{}", cadre_spl(*amount_spl), cxs(recipient), if job_order.is_some() { " as a storefront job order" } else { "" }),
            )
            .f("Recipient", cx(recipient))
            .f("Amount", cadre_spl(*amount_spl))
            .f("Solana nonce", solana_nonce.to_string())
            .f("Attested by", sigs.iter().map(|s| cxs(&s.pubkey)).collect::<Vec<_>>().join(", "));
            if let Some(p) = job_order {
                w = w.f("Order job", hx(&agent_types::tx::order_job_id(*solana_nonce))).f("Order shape", format!("{}x{}x{}, W {}, P {}", p.m, p.n, p.k, cadre(p.worker_pay), cadre(p.protocol_fee)));
            }
            w
        }
        TxBody::BridgeLock { solana_recipient, amount } => Words::new("BridgeLock", format!("{} locked {} to bridge out to Solana {}", cxs(&tx.from), cadre(*amount), short(&b58(solana_recipient)))).f("Solana recipient", b58(solana_recipient)).f("Amount", cadre(*amount)),
        TxBody::AdminConfig { key, value } => {
            let name = config_key_name(params, *key);
            Words::new("AdminConfig", format!("admin set {name} ({} bytes)", value.len())).f("Key", format!("{name} ({key})")).f("Value", if value.len() <= 64 { hex::encode(value) } else { format!("{}... ({} bytes)", hex::encode(&value[..32]), value.len()) })
        }
        TxBody::SetRoute { solana_recipient, route_bps, until_height, cap, release_key } => Words::new("SetRoute", format!("{} set a payout lien: {}% of income to Solana {} until block {}", cxs(&tx.from), *route_bps as f64 / 100.0, short(&b58(solana_recipient)), until_height))
            .f("Recipient", b58(solana_recipient))
            .f("Share", format!("{route_bps} bps"))
            .f("Until", format!("block {until_height}"))
            .f("Cap", cadre(*cap))
            .f("Release key", if is_zero(release_key) { "none (cannot be cleared early)".into() } else { cx(release_key) }),
        TxBody::ClearRoute { account, route_nonce, .. } => Words::new("ClearRoute", format!("payout lien of {} cleared by its release key", cxs(account))).f("Account", cx(account)).f("Route nonce", route_nonce.to_string()),
        TxBody::RegisterModel(m) => Words::new("RegisterModel", format!("{} registered model {} with {} weight matrices", cxs(&tx.from), quote(&m.name, 48), m.matrices.len())).f("Name", text(&m.name)).f("Manifest", text(&m.manifest_uri)).f("Matrices", m.matrices.len().to_string()).f("Rank", m.rank.to_string()),
        TxBody::PostSegments { ticket_id, round, model_id, segments, .. } => Words::new("PostSegments", format!("ticket {} round {}: {} standing segments posted for mining", short(&hx(ticket_id)), round, segments.len()))
            .f("Ticket", hx(ticket_id))
            .f("Model", hx(model_id))
            .f("Segments", format!("{} (matrices {}..{})", segments.len(), segments.first().map(|s| s.matrix).unwrap_or(0), segments.last().map(|s| s.matrix).unwrap_or(0)))
            .f("Rows (m)", segments.first().map(|s| s.m.to_string()).unwrap_or_default()),
        TxBody::RejectResult { job_id, deliverer, sigs, .. } => Words::new("RejectResult", format!("evaluators rejected {}'s delivery for job {} (job reopened)", cxs(deliverer), short(&hx(job_id))))
            .f("Job", hx(job_id))
            .f("Deliverer", cx(deliverer))
            .f("Signed by", sigs.iter().map(|s| cxs(&s.pubkey)).collect::<Vec<_>>().join(", ")),
        TxBody::SetProfile(p) => {
            let flags: Vec<&str> = [(1, "agent"), (2, "house"), (4, "accepts hires"), (8, "trades")].iter().filter(|(b, _)| p.flags & b != 0).map(|(_, n)| *n).collect();
            Words::new("SetProfile", format!("{} set profile @{} ({})", cxs(&tx.from), text(&p.handle), if text(&p.name).is_empty() { "no name".into() } else { text(&p.name) }))
                .f("Handle", format!("@{}", text(&p.handle)))
                .f("Name", text(&p.name))
                .f("Flags", if flags.is_empty() { "none".into() } else { flags.join(", ") })
                .f("Operator", if is_zero(&p.operator) { "none".into() } else { cx(&p.operator) })
                .f("Profile", text(&p.profile_uri))
        }
        TxBody::OperatorLink { agent, add } => Words::new("OperatorLink", format!("{} {} agent {} as its operator", cxs(&tx.from), if *add { "linked" } else { "unlinked" }, cxs(agent))).f("Agent", cx(agent)).f("Link", if *add { "add" } else { "remove" }),
        TxBody::Post(p) => {
            let kind = ["post", "reply", "repost", "quote"].get(p.kind as usize).copied().unwrap_or("post");
            let id = agent_types::social::post_id(&tx.from, tx.nonce);
            let mut w = Words::new("Post", format!("{} {}: {}", cxs(&tx.from), if kind == "post" { "posted".to_string() } else { format!("sent a {kind}") }, quote(&p.body, 90))).f("Post id", hx(&id)).f("Kind", kind).f("Body", text(&p.body));
            if !is_zero(&p.parent) {
                w = w.f("Parent", hx(&p.parent));
            }
            if !p.thought_ref.is_empty() {
                w = w.f("Thought receipt", text(&p.thought_ref));
            }
            w
        }
        TxBody::React { items } => {
            let names = agent_types::social::REACTION_NAMES;
            let tips: u64 = items.iter().map(|r| r.tip).sum();
            Words::new("React", format!("{} reacted to {} post{}{}", cxs(&tx.from), items.len(), if items.len() == 1 { "" } else { "s" }, if tips > 0 { format!(", tipping {}", cadre(tips)) } else { String::new() }))
                .f("Reactions", items.iter().map(|r| format!("{} {}", names.get(r.reaction as usize).unwrap_or(&"?"), short(&hx(&r.post_id)))).collect::<Vec<_>>().join(", "))
        }
        TxBody::Follow { items } => Words::new("Follow", format!("{} changed {} follow{}", cxs(&tx.from), items.len(), if items.len() == 1 { "" } else { "s" }))
            .f("Edges", items.iter().map(|e| format!("{} {}", if e.on { "follow" } else { "unfollow" }, cxs(&e.target))).collect::<Vec<_>>().join(", ")),
        TxBody::SendMessage(m) => {
            use agent_types::dm::MODE_SEALED;
            let sealed = m.mode == MODE_SEALED;
            let to_channel = !is_zero(&m.channel);
            let what = if sealed { format!("a sealed message ({} bytes, end-to-end encrypted)", m.body.len()) } else { format!("{}", quote(&m.body, 80)) };
            let target = if to_channel { format!("channel {}", short(&hx(&m.channel))) } else { cxs(&m.to) };
            let mut w = Words::new("SendMessage", format!("{} sent {target} {what}{}", cxs(&tx.from), if m.pay > 0 { format!(", paying {}", cadre(m.pay)) } else { String::new() }));
            w = w.f("Message id", hx(&agent_types::dm::message_id(&tx.from, tx.nonce))).f(if to_channel { "Channel" } else { "To" }, if to_channel { hx(&m.channel) } else { cx(&m.to) }).f("Mode", if sealed { "sealed" } else { "plain" });
            if !sealed {
                w = w.f("Body", text(&m.body));
            }
            if m.pay > 0 {
                w = w.f("Pay", format!("{} (reclaimable after {} blocks)", cadre(m.pay), m.reclaim_after));
            }
            if !is_zero(&m.reply_to) {
                w = w.f("Reply to", hx(&m.reply_to));
            }
            if !m.thought_ref.is_empty() {
                w = w.f("Thought receipt", text(&m.thought_ref));
            }
            w
        }
        TxBody::ReclaimMessage { message_id } => Words::new("ReclaimMessage", format!("{} reclaimed or declined the pay of message {}", cxs(&tx.from), short(&hx(message_id)))).f("Message", hx(message_id)),
        TxBody::SetConsent { items } => {
            let word = |s: u8| match s {
                agent_types::dm::CONSENT_ALLOW => "allow",
                agent_types::dm::CONSENT_DENY => "deny",
                _ => "clear",
            };
            Words::new("SetConsent", format!("{} set message consent for {} account{}", cxs(&tx.from), items.len(), if items.len() == 1 { "" } else { "s" }))
                .f("Consent", items.iter().map(|c| format!("{} {}", word(c.state), cxs(&c.peer))).collect::<Vec<_>>().join(", "))
        }
        TxBody::ChannelConfig(c) => {
            let policy = ["open", "invite only", "paid"].get(c.policy as usize).copied().unwrap_or("?");
            Words::new("ChannelConfig", format!("{} configured channel {} ({policy})", cxs(&tx.from), quote(&c.name, 48)))
                .f("Channel", hx(&c.channel_id))
                .f("Name", text(&c.name))
                .f("Policy", if c.policy == agent_types::dm::CHANNEL_PAID { format!("{policy}, join price {}", cadre(c.join_price)) } else { policy.into() })
                .f("Admins", c.admins.iter().map(|a| cxs(a)).collect::<Vec<_>>().join(", "))
        }
        TxBody::ChannelMember { channel, action, member, pay } => {
            let act = agent_types::dm::MEMBER_ACTION_NAMES.get(*action as usize).copied().unwrap_or("?");
            Words::new("ChannelMember", format!("{} {act} {} in channel {}{}", cxs(&tx.from), cxs(member), short(&hx(channel)), if *pay > 0 { format!(", paying {}", cadre(*pay)) } else { String::new() }))
                .f("Channel", hx(channel))
                .f("Member", cx(member))
                .f("Action", act)
        }
        TxBody::ChannelKey { channel, epoch, envelopes } => Words::new("ChannelKey", format!("{} shared channel {}'s key epoch {} with {} members (sealed envelopes)", cxs(&tx.from), short(&hx(channel)), epoch, envelopes.len())).f("Channel", hx(channel)).f("Epoch", epoch.to_string()),
        TxBody::DeployCode(d) => Words::new("DeployCode", format!("{} uploaded {} bytes of contract code", cxs(&tx.from), d.code.len())).f("Code hash", hx(&agent_types::contracts::code_hash(&d.code))).f("Size", format!("{} bytes", d.code.len())).f("Gas limit", d.gas_limit.to_string()),
        TxBody::Instantiate(i) => Words::new("Instantiate", format!("{} created contract {} from code {}", cxs(&tx.from), cxs(&agent_types::contracts::contract_address(&tx.from, tx.nonce)), short(&hx(&i.code_hash))))
            .f("Contract", cx(&agent_types::contracts::contract_address(&tx.from, tx.nonce)))
            .f("Code", hx(&i.code_hash))
            .f("Label", text(&i.label))
            .f("Admin", if is_zero(&i.admin) { "none (never migratable)".into() } else { cx(&i.admin) })
            .f("Value", cadre(i.value)),
        TxBody::Call(c) => Words::new("Call", format!("{} called contract {}{}", cxs(&tx.from), cxs(&c.contract), if c.value > 0 { format!(" sending {}", cadre(c.value)) } else { String::new() }))
            .f("Contract", cx(&c.contract))
            .f("Args", format!("{} bytes", c.args.len()))
            .f("Gas limit", c.gas_limit.to_string()),
        TxBody::Migrate(m) => Words::new("Migrate", format!("{} moved contract {} to code {}", cxs(&tx.from), cxs(&m.contract), short(&hx(&m.code_hash)))).f("Contract", cx(&m.contract)).f("New code", hx(&m.code_hash)),
        TxBody::SetContractAdmin { contract, admin } => Words::new("SetContractAdmin", format!("admin of contract {} set to {}", cxs(contract), if is_zero(admin) { "none".into() } else { cxs(admin) })).f("Contract", cx(contract)).f("Admin", cx(admin)),
        #[allow(unreachable_patterns)]
        _ => Words::new(node_kind, format!("{} from {} (described by kind name only in this build)", tx.body.kind().name(), cxs(&tx.from))),
    };
    w.f("From", from).f("Nonce", tx.nonce.to_string()).f("Fee", format!("{} CADRE", crate::fmt::amount(tx.fee as u128, crate::fmt::CHAIN_DP, crate::fmt::CHAIN_DP)))
}

// ---------------------------------------------------------------------------------------
// Solana

pub const PUMP: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const PUMP_AMM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
pub const COMPUTE_BUDGET: &str = "ComputeBudget111111111111111111111111111111";

const BRIDGE_IX: &[&str] = &["deposit", "initialize", "refund_deposit", "release", "release_proof", "set_admin", "set_release_modes", "set_signers"];
const STORE_IX: &[&str] = &[
    "buy_job", "buy_job_amm", "cancel_draft", "crank_buyback", "crank_buyback_amm", "expire_order", "expire_order_proof", "finalize_order", "finalize_order_proof", "initialize", "mark_refunded", "open_order",
    "set_admin", "set_attesters", "set_buyback", "set_buyback_cap", "set_finalize_modes", "settle_share", "update_config",
];
const RESERVE_IX: &[&str] = &["collect_curve_fees", "crank_buyback", "crank_buyback_amm", "fund_mining", "initialize", "launch", "migrate_config", "set_admin", "set_fund_limits", "update_config"];
const LC_IX: &[&str] = &[
    "accumulate", "begin_verify", "begin_verify_job", "close_header", "close_work", "connect", "fallback_gen_b", "fallback_hash_a", "fallback_hash_bt", "init_accumulator", "init_work", "initialize", "mark_final",
    "prove_header", "register_job", "replace_job", "reorg", "restore_header", "set_admin", "set_params", "submit_header", "verify_jackpot", "verify_noise", "verify_opening", "verify_proof_hash", "write_proof",
];

fn ix_name(names: &[&'static str], data: &[u8]) -> Option<&'static str> {
    let d: [u8; 8] = data.get(..8)?.try_into().ok()?;
    names.iter().copied().find(|n| anchor_disc(n) == d)
}

fn le64(d: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?))
}
fn key_at(d: &[u8], at: usize) -> Option<String> {
    let k: [u8; 32] = d.get(at..at + 32)?.try_into().ok()?;
    Some(cx(&k))
}

/// One Solana instruction in words (outer or inner).
pub fn sol_ix_words(ix: &Value) -> String {
    let pid = ix["programId"].as_str().unwrap_or("");
    if let Some(p) = ix.get("parsed").filter(|p| !p.is_null()) {
        let prog = ix["program"].as_str().unwrap_or(pid);
        if let Some(s) = p.as_str() {
            return format!("{prog}: {s}");
        }
        let t = p["type"].as_str().unwrap_or("?");
        let i = &p["info"];
        return match (prog, t) {
            ("system", "transfer") => format!("SOL transfer {} SOL from {} to {}", sol(i["lamports"].as_u64().unwrap_or(0) as u128), short(i["source"].as_str().unwrap_or("")), short(i["destination"].as_str().unwrap_or(""))),
            ("spl-token" | "spl-token-2022", "transferChecked" | "transfer") => {
                let amt = i.pointer("/tokenAmount/uiAmountString").and_then(|v| v.as_str()).map(String::from).or_else(|| i["amount"].as_str().map(String::from)).unwrap_or_default();
                format!("token transfer {amt} from {} to {}", short(i["source"].as_str().unwrap_or("")), short(i["destination"].as_str().unwrap_or("")))
            }
            ("spl-memo", _) => format!("memo {}", p),
            _ => format!("{prog} {t}"),
        };
    }
    let data = ix["data"].as_str().and_then(|d| bs58::decode(d).into_vec().ok()).unwrap_or_default();
    let p = crate::solana::profile();
    let prog_name = if pid == p.bridge {
        "bridge"
    } else if pid == p.storefront {
        "storefront"
    } else if pid == p.reserve {
        "reserve"
    } else if pid == p.lightclient {
        "light client"
    } else if pid == PUMP {
        return "pump.fun bonding curve instruction".into();
    } else if pid == PUMP_AMM {
        return "PumpSwap AMM instruction".into();
    } else if pid == COMPUTE_BUDGET {
        return "compute budget".into();
    } else {
        return format!("program {}", short(pid));
    };
    let names = match prog_name {
        "bridge" => BRIDGE_IX,
        "storefront" => STORE_IX,
        "reserve" => RESERVE_IX,
        _ => LC_IX,
    };
    let Some(name) = ix_name(names, &data) else {
        return format!("{prog_name}: unknown instruction");
    };
    let a = &data[8.min(data.len())..];
    let args = match (prog_name, name) {
        ("bridge", "deposit") => le64(a, 0).zip(key_at(a, 8)).map(|(amt, r)| format!(": locked {} in the bridge vault for chain account {}", cadre_spl(amt), crate::fmt::short_addr(&r))),
        ("bridge", "release") => le64(a, 0).zip(le64(a, 40)).map(|(n, amt)| format!(": released withdrawal #{n}, {}", cadre_spl(amt))),
        ("bridge", "release_proof") => le64(a, 0).map(|n| format!(": released withdrawal #{n} by light-client proof")),
        ("bridge", "refund_deposit") => le64(a, 0).map(|n| format!(": refunded deposit #{n}")),
        ("storefront", "buy_job" | "buy_job_amm") => le64(a, 0).zip(le64(a, 8)).map(|(n, s)| format!(": bought AI job order #{n} for {} SOL", sol(s as u128))),
        ("storefront" | "reserve", "crank_buyback" | "crank_buyback_amm") => le64(a, 0).map(|m| format!(": buyback of up to {} SOL", sol(m as u128))),
        ("reserve", "fund_mining") => le64(a, 0).map(|amt| format!(": funded the chain mining pool with {}", cadre_spl(amt))),
        _ => None,
    };
    format!("{prog_name} {name}{}", args.unwrap_or_default())
}

/// Storefront events in the logs, in words.
pub fn sol_events(logs: &[Value]) -> Vec<String> {
    let mut out = vec![];
    for l in logs.iter().filter_map(|l| l.as_str()) {
        let Some(b64) = l.strip_prefix("Program data: ") else { continue };
        let Ok(b) = base64::engine::general_purpose::STANDARD.decode(b64) else { continue };
        use crate::solana::StoreEvent as E;
        match crate::solana::decode_store_event(&b) {
            Some(E::Executed(x)) => out.push(format!("buyback #{}: {} SOL bought {} (orders {}..{})", x.buy, sol(x.sol_in as u128), cadre_spl(x.tokens_out), x.first_nonce, x.last_nonce)),
            Some(E::Accrued(x)) => out.push(format!("order #{} put {} SOL into the buyback vault", x.nonce, sol(x.lamports as u128))),
            Some(E::ToReserve(x)) => out.push(format!("buyback #{}: {} to the mining reserve", x.buy, cadre_spl(x.tokens))),
            Some(E::Burned(x)) => out.push(format!("buyback #{}: {} burned ({})", x.buy, cadre_spl(x.tokens), if x.reason == 0 { "reserve at its cap" } else { "destination burn" })),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_types::tx::TxBody;

    fn tx(body: TxBody) -> Tx {
        Tx { chain_id: 7, from: [9; 32], nonce: 3, fee: 1000, body, sig: [0; 64] }
    }

    #[test]
    fn transfer_and_post_in_words() {
        let w = chain_words(Some(&tx(TxBody::Transfer { to: [1; 32], amount: 1_500_000_000 })), "Transfer", "", &Value::Null);
        assert!(w.headline.contains("sent 1.5 CADRE to"), "{}", w.headline);
        let p = agent_types::social::Post { kind: 1, parent: [2; 32], channel: [0; 32], body: b"hello town".to_vec(), blob_uri: vec![], blob_hash: [0; 32], thought_ref: b"req_1".to_vec() };
        let w = chain_words(Some(&tx(TxBody::Post(p))), "Post", "", &Value::Null);
        assert!(w.headline.contains("sent a reply: \"hello town\""), "{}", w.headline);
        assert!(w.fields.iter().any(|(k, v)| k == "Thought receipt" && v == "req_1"));
        assert!(w.fields.iter().any(|(k, _)| k == "Parent"));
    }

    #[test]
    fn dms_in_words_never_open_a_sealed_body() {
        let m = agent_types::dm::SendMessage { to: [1; 32], channel: [0; 32], mode: agent_types::dm::MODE_SEALED, reply_to: [0; 32], pay: 0, reclaim_after: 0, body: vec![1; 90], blob_uri: vec![], blob_hash: [0; 32], thought_ref: vec![] };
        let w = chain_words(Some(&tx(TxBody::SendMessage(m))), "SendMessage", "", &Value::Null);
        assert!(w.headline.contains("a sealed message (90 bytes"), "{}", w.headline);
        assert!(!w.fields.iter().any(|(k, _)| k == "Body"));
        let m = agent_types::dm::SendMessage { to: [1; 32], channel: [0; 32], mode: agent_types::dm::MODE_PLAIN, reply_to: [0; 32], pay: 2_000_000_000, reclaim_after: 100, body: b"hi".to_vec(), blob_uri: vec![], blob_hash: [0; 32], thought_ref: vec![] };
        let w = chain_words(Some(&tx(TxBody::SendMessage(m))), "SendMessage", "", &Value::Null);
        assert!(w.headline.contains("\"hi\", paying 2 CADRE"), "{}", w.headline);
    }

    #[test]
    fn unknown_kind_falls_back_to_the_node_name() {
        let w = chain_words(None, "SendMessage", "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", &Value::Null);
        assert_eq!(w.kind, "SendMessage");
        assert!(w.headline.contains("cannot decode"));
    }

    #[test]
    fn admin_key_names_from_params() {
        let params = json!({"keys": {"min_fee": 1, "admin": 0}});
        let w = chain_words(Some(&tx(TxBody::AdminConfig { key: 1, value: vec![1, 0, 0, 0, 0, 0, 0, 0] })), "AdminConfig", "", &params);
        assert!(w.headline.contains("min_fee"), "{}", w.headline);
    }

    #[test]
    fn bridge_deposit_ix_in_words() {
        crate::solana::set_network(crate::ctx::Network::Localnet);
        let mut d = anchor_disc("deposit").to_vec();
        d.extend(10_000_000u64.to_le_bytes());
        d.extend([5u8; 32]);
        let ix = json!({"programId": crate::solana::LOCALNET.bridge, "data": bs58::encode(&d).into_string()});
        let s = sol_ix_words(&ix);
        assert!(s.starts_with("bridge deposit: locked 10 CADRE"), "{s}");
        let ix = json!({"program": "system", "programId": "11111111111111111111111111111111", "parsed": {"type": "transfer", "info": {"lamports": 5_000_000_000u64, "source": "a", "destination": "b"}}});
        assert!(sol_ix_words(&ix).starts_with("SOL transfer 5 SOL"));
    }
}
