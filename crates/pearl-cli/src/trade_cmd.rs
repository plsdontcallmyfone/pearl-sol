//! Trading on Solana from the CLI (docs/CLI.md "Trade", docs/TRADING.md): CADRE (or any pump
//! coin by --mint) on its pump curve or canonical PumpSwap pool, the same builders the site's
//! /buy page uses (cadre-api POST /v1/tx/pump-buy and /v1/tx/pump-sell).
//!
//!   cadre quote buy --sol 0.01 [--mint M]        what 0.01 SOL buys (fee-free pool quote)
//!   cadre quote sell --tokens 1000 [--mint M]    what 1,000 tokens sell for
//!   cadre positions [ADDR]                       balances, own-wallet and mandate positions
//!   cadre buy --sol 0.01 --key FILE [--yes]      review in plain words, y/N, sign, send
//!   cadre sell --tokens 1000 --key FILE [--yes]
//!
//! Signing happens only with the key file named by --key (or --sol-key), and only after the
//! review: the builder's numbers (most SOL spent, least tokens received, pool, slippage) are
//! printed and the command asks "Sign and send? [y/N]". Without a terminal (a pipe, --json,
//! the app's palette) it needs --yes. The CLI checks the unsigned tx names this key as its fee
//! payer and only signer before signing; cadre-api never sees the key.

use std::io::{BufRead, IsTerminal, Write};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use clap::Subcommand;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

use crate::commands::Out;
use crate::ctx::Ctx;
use crate::entry::trade::{mine_lines, spl};
use crate::fmt::{parse_amount, short_n, sol};
use crate::solana::b58;
use crate::term::{Block, Panel, Tone, View};

#[derive(Subcommand, Debug, Clone)]
pub enum QuoteCmd {
    /// What a buy of `--sol` SOL returns (fee-free constant-product quote from the pool).
    Buy {
        #[arg(long)]
        sol: String,
        /// A pump coin mint (default CADRE).
        #[arg(long)]
        mint: Option<String>,
    },
    /// What a sell of `--tokens` tokens returns.
    Sell {
        #[arg(long)]
        tokens: String,
        #[arg(long)]
        mint: Option<String>,
    },
}

fn api(ctx: &Ctx) -> Result<String> {
    Ok(ctx.eps.get("api").ok_or_else(|| anyhow!("needs cadre-api (set CADRE_API or --endpoint api=...)"))?.trim_end_matches('/').to_string())
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).user_agent(concat!("cadre-cli/", env!("CARGO_PKG_VERSION"))).build()
}

fn read_json(r: std::result::Result<ureq::Response, ureq::Error>, what: &str) -> Result<Value> {
    match r {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let m = v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).map(String::from).unwrap_or_else(|| short_n(&body, 200));
            bail!("{what}: HTTP {code}: {m}")
        }
        Err(e) => bail!("{what}: {e}"),
    }
}

fn get(url: &str, what: &str) -> Result<Value> {
    read_json(http().get(url).call(), what)
}

fn post(url: &str, body: Value, what: &str) -> Result<Value> {
    read_json(http().post(url).send_json(body), what)
}

fn shortvec(b: &[u8]) -> Result<(usize, usize)> {
    let (mut n, mut shift, mut i) = (0usize, 0, 0);
    loop {
        let x = *b.get(i).ok_or_else(|| anyhow!("truncated transaction"))?;
        n |= ((x & 0x7f) as usize) << shift;
        i += 1;
        if x & 0x80 == 0 {
            return Ok((n, i));
        }
        shift += 7;
        ensure!(shift < 21, "bad shortvec");
    }
}

/// Sign a builder's unsigned legacy tx (signature slots zeroed) with `key`, after checking
/// that the message names `key` as its fee payer and its only required signer.
pub fn sign_unsigned(tx_b64: &str, key: &SigningKey) -> Result<Vec<u8>> {
    let mut tx = base64::engine::general_purpose::STANDARD.decode(tx_b64.trim()).context("transaction is not base64")?;
    let (nsig, off) = shortvec(&tx)?;
    ensure!(nsig == 1, "the transaction wants {nsig} signatures; this command signs only single-signer txs");
    let msg_at = off + 64 * nsig;
    ensure!(tx.len() > msg_at + 3, "truncated transaction");
    let msg = tx[msg_at..].to_vec();
    ensure!(msg[0] & 0x80 == 0, "a versioned transaction; the builders return legacy txs");
    ensure!(msg[0] == 1, "the message requires {} signers, expected 1", msg[0]);
    let (nkeys, koff) = shortvec(&msg[3..])?;
    ensure!(nkeys >= 1 && msg.len() >= 3 + koff + 32, "no account keys in the message");
    let payer = &msg[3 + koff..3 + koff + 32];
    let me = key.verifying_key().to_bytes();
    ensure!(payer == me, "the transaction's fee payer is {}, not this key ({})", bs58::encode(payer).into_string(), bs58::encode(me).into_string());
    let sig = key.sign(&msg).to_bytes();
    tx[off..off + 64].copy_from_slice(&sig);
    Ok(tx)
}

/// The review lines of a built buy or sell (from the builder's `summary`).
pub fn review(side: &str, b: &Value) -> Vec<(String, String)> {
    let s = &b["summary"];
    let st = |k: &str| s[k].as_str().map(String::from).or_else(|| s[k].as_u64().map(|x| x.to_string())).unwrap_or_default();
    let n = |k: &str| st(k).parse::<u128>().unwrap_or(0);
    let venue = st("venue");
    let mut l = vec![];
    if side == "buy" {
        l.push(("Action".into(), format!("buy tokens on {} (Solana)", if venue.is_empty() { "pump" } else { &venue })));
        l.push(("You spend at most".into(), format!("{} SOL (pump's fee included), plus the tx fee", sol(n("sol_in_lamports")))));
        l.push(("You receive at least".into(), format!("{} tokens (quote {} before fees)", spl(n("min_tokens_out_spl")), spl(n("quote_tokens_out_spl")))));
    } else {
        l.push(("Action".into(), format!("sell tokens on {} (Solana)", if venue.is_empty() { "pump" } else { &venue })));
        l.push(("You sell exactly".into(), format!("{} tokens (you hold {})", spl(n("amount_spl")), spl(n("balance_spl")))));
        l.push(("You receive at least".into(), format!("{} SOL (quote {} before fees)", sol(n("min_sol_out_lamports")), sol(n("quote_sol_out_lamports")))));
    }
    l.push(("Slippage".into(), format!("{} bps on top of a {} bps fee allowance", st("slippage_bps"), st("fee_allowance_bps"))));
    l.push(("Mint".into(), st("mint")));
    if !st("pool").is_empty() {
        l.push(("Pool".into(), st("pool")));
    } else if !st("bonding_curve").is_empty() {
        l.push(("Bonding curve".into(), st("bonding_curve")));
    }
    l.push(("Program".into(), format!("{} {}", st("program"), st("instruction"))));
    l.push(("Fee payer and signer".into(), b["fee_payer"].as_str().unwrap_or("").to_string()));
    l
}

fn ask_yes(ctx: &Ctx, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    ensure!(!ctx.opts.json && ctx.ui.is_plain(), "nothing was signed: pass --yes to sign without the prompt (--json and the app have no prompt)");
    ensure!(std::io::stdin().is_terminal(), "nothing was signed: stdin is not a terminal, pass --yes to sign without the prompt");
    print!("Sign and send? [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    ensure!(matches!(line.trim(), "y" | "Y" | "yes" | "YES"), "cancelled: nothing was signed");
    Ok(())
}

fn solana_link(ctx: &Ctx, sig: &str) -> Option<String> {
    match ctx.opts.network {
        crate::ctx::Network::Devnet => Some(format!("https://explorer.solana.com/tx/{sig}?cluster=devnet")),
        _ => None,
    }
}

fn mint_q(mint: &Option<String>) -> String {
    mint.as_ref().map(|m| format!("&mint={m}")).unwrap_or_default()
}

/// `cadre quote buy|sell`.
pub fn quote(ctx: &Ctx, q: &QuoteCmd) -> Result<Out> {
    let api = api(ctx)?;
    let (url, side) = match q {
        QuoteCmd::Buy { sol, mint } => (format!("{api}/pump/quote?side=buy&sol_in={}{}", parse_amount(sol, 9)?, mint_q(mint)), "buy"),
        QuoteCmd::Sell { tokens, mint } => (format!("{api}/pump/quote?side=sell&tokens_in={}{}", parse_amount(tokens, 6)?, mint_q(mint)), "sell"),
    };
    let j = get(&url, "cadre-api /pump/quote")?;
    let num = |k: &str| j[k].as_str().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
    let mut p = Panel::new(&format!("Quote: {side}")).kv("Venue", format!("{} (Solana)", j["venue"].as_str().unwrap_or("?")));
    if let Some(m) = j["mint"].as_str() {
        p = p.kv("Mint", m);
    }
    if side == "buy" {
        p = p.kv("SOL in", format!("{} SOL", sol(num("sol_in_lamports")))).kt("Tokens out", format!("{} before fees", spl(num("tokens_out_spl"))), Tone::Good);
    } else {
        p = p.kv("Tokens in", spl(num("tokens_in_spl"))).kt("SOL out", format!("{} SOL before fees", sol(num("sol_out_lamports"))), Tone::Good);
    }
    let r = &j["reserves"];
    let rn = |k: &str| r[k].as_str().and_then(|x| x.parse::<u128>().ok());
    if let (Some(b), Some(q)) = (rn("base_spl"), rn("quote_lamports")) {
        p = p.kv("Pool reserves", format!("{} tokens / {} SOL", spl(b), sol(q)));
        if let Some(real) = rn("real_quote_lamports") {
            p = p.kv("Real SOL in the pool", format!("{} SOL", sol(real)));
        }
    } else if let (Some(t), Some(q)) = (rn("virtual_token"), rn("virtual_sol")) {
        p = p.kv("Curve reserves (virtual)", format!("{} tokens / {} SOL", spl(t), sol(q)));
    }
    let text = format!("{side} quote from {}", j["venue"].as_str().unwrap_or("?"));
    Ok(Out::new(j.clone(), text).view(View::panel(p).hint(j["note"].as_str().unwrap_or("fee-free constant-product quote"))))
}

/// `cadre positions [ADDR]`.
pub fn positions(ctx: &Ctx, address: Option<&str>) -> Result<Out> {
    let api = api(ctx)?;
    let a = match address {
        Some(a) => a.to_string(),
        None => ctx.sol_me.as_ref().or(ctx.me.as_ref()).map(b58).ok_or_else(|| anyhow!("no account: pass an address, --key FILE or --address ADDR"))?,
    };
    let mine = match get(&format!("{api}/trading/agents/{a}"), "cadre-api /trading/agents") {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.to_string().contains("HTTP 404") => Ok(None),
        Err(e) => Err(format!("{e:#}")),
    };
    let bal = get(&format!("{api}/accounts/{a}/balances"), "cadre-api /accounts/balances").map_err(|e| format!("{e:#}"));
    let lines = mine_lines(Some(&a), &Some(mine.clone()), &Some(bal.clone()));
    let mut p = Panel::new("Positions");
    for (k, v, c) in &lines {
        let tone = if *c == crate::entry::theme::RED { Tone::Warn } else if *c == crate::entry::theme::GREEN { Tone::Good } else if *c == crate::entry::theme::DIM { Tone::Dim } else { Tone::Plain };
        p = p.kt(k, v.as_str(), tone);
    }
    let j = json!({"account": a, "trading": mine.as_ref().ok().cloned().flatten(), "trading_error": mine.err(), "balances": bal.as_ref().ok(), "balances_error": bal.as_ref().err()});
    Ok(Out::new(j, format!("positions of {}", short_n(&a, 12))).view(View::panel(p).hint("realized PnL only, from chain data; a position at the pool mid is a mark, not profit")))
}

/// `cadre buy` / `cadre sell`: build, review, ask, sign with --key, send, confirm.
pub fn trade(ctx: &Ctx, side: &str, amount: &str, slippage_bps: u16, mint: &Option<String>, yes: bool) -> Result<Out> {
    let api = api(ctx)?;
    let key = ctx.sol_key()?;
    let me = bs58::encode(key.verifying_key().to_bytes()).into_string();
    ensure!(slippage_bps <= 2_000, "--slippage-bps is at most 2000");
    let mut body = if side == "buy" {
        json!({"buyer": me, "sol_in": parse_amount(amount, 9)?.to_string(), "slippage_bps": slippage_bps})
    } else {
        json!({"seller": me, "amount_spl": parse_amount(amount, 6)?.to_string(), "slippage_bps": slippage_bps})
    };
    if let Some(m) = mint {
        body["mint"] = json!(m);
    }
    let st = ctx.ui.step(format!("Building the {side} on cadre-api"));
    let built = st.wrap(post(&format!("{api}/tx/pump-{side}"), body, &format!("cadre-api /tx/pump-{side}")), |_| format!("{side} built (unsigned)"))?;
    let lines = review(side, &built);
    if ctx.ui.is_plain() && !ctx.opts.json {
        let mut p = Panel::new(&format!("Review: {side}"));
        for (k, v) in &lines {
            p = p.kv(k, v.as_str());
        }
        if let Some(n) = built["note"].as_str() {
            p = p.text(n, Tone::Dim);
        }
        println!("{}", View::panel(p).render(&ctx.ui.style()));
    }
    ask_yes(ctx, yes)?;
    let signed = sign_unsigned(built["transaction"].as_str().ok_or_else(|| anyhow!("the builder returned no transaction"))?, key)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&signed);
    let st = ctx.ui.step("Sending to Solana");
    let sent = st.wrap(post(&format!("{api}/tx/solana"), json!({"tx": b64}), "cadre-api /tx/solana"), |v| format!("sent {}", short_n(v["signature"].as_str().unwrap_or(""), 16)))?;
    let sig = sent["signature"].as_str().unwrap_or("").to_string();
    let st = ctx.ui.step("Waiting for confirmation");
    let conf = st.wrap(ctx.sol.confirm(&sig, Duration::from_secs(90)), |slot| format!("confirmed in slot {slot}"));
    let mut p = Panel::new(&format!("{} {}", if side == "buy" { "Bought" } else { "Sold" }, if conf.is_ok() { "" } else { "(not confirmed)" }).trim().to_string().as_str()).kv("Signature", sig.as_str());
    if let Some(l) = solana_link(ctx, &sig) {
        p = p.kv("Explorer", l);
    }
    for (k, v) in lines.iter().take(3).skip(1) {
        p = p.kv(k, v.as_str());
    }
    let (status, slot) = match &conf {
        Ok(s) => ("confirmed".to_string(), Some(*s)),
        Err(e) => (format!("{e:#}"), None),
    };
    p = p.kt("Status", status.as_str(), if conf.is_ok() { Tone::Good } else { Tone::Warn });
    let j = json!({"side": side, "signature": sig, "slot": slot, "status": status, "summary": built["summary"], "explorer": solana_link(ctx, &sig)});
    let code = if conf.is_ok() { 0 } else { 1 };
    let out = Out::new(j, format!("{side} {sig}")).view(View(vec![Block::Panel(p)]).hint("the fill is a Solana tx signed by your key; `cadre positions` shows the result"));
    Ok(Out { code, ..out })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A legacy tx as the builders return it: 1 empty signature slot, then the message.
    fn unsigned(payer: [u8; 32]) -> String {
        let mut msg = vec![1u8, 0, 1, 2];
        msg.extend(payer);
        msg.extend([7u8; 32]);
        msg.extend([9u8; 32]); // blockhash
        msg.push(0); // no instructions
        let mut tx = vec![1u8];
        tx.extend([0u8; 64]);
        tx.extend(&msg);
        base64::engine::general_purpose::STANDARD.encode(tx)
    }

    #[test]
    fn signs_only_its_own_single_signer_tx() {
        let k = SigningKey::from_bytes(&[3u8; 32]);
        let me = k.verifying_key().to_bytes();
        let tx = sign_unsigned(&unsigned(me), &k).unwrap();
        let sig = ed25519_dalek::Signature::from_slice(&tx[1..65]).unwrap();
        k.verifying_key().verify_strict(&tx[65..], &sig).unwrap();
        // another fee payer: refused
        let e = sign_unsigned(&unsigned([5u8; 32]), &k).unwrap_err();
        assert!(e.to_string().contains("fee payer"), "{e}");
        // two signatures wanted: refused
        let mut raw = base64::engine::general_purpose::STANDARD.decode(unsigned(me)).unwrap();
        raw[0] = 2;
        assert!(sign_unsigned(&base64::engine::general_purpose::STANDARD.encode(raw), &k).is_err());
    }

    #[test]
    fn review_says_what_moves() {
        let b = json!({"fee_payer": "Me1", "summary": {"venue": "pumpswap", "mint": "M", "sol_in_lamports": "10000000", "quote_tokens_out_spl": "380000000000", "min_tokens_out_spl": "371000000000", "slippage_bps": 100, "fee_allowance_bps": 125, "pool": "P", "program": "pAMM", "instruction": "buy_exact_quote_in_v2"}});
        let l = review("buy", &b);
        let t: Vec<String> = l.iter().map(|(a, b)| format!("{a}: {b}")).collect();
        assert!(t.contains(&"You spend at most: 0.01 SOL (pump's fee included), plus the tx fee".to_string()), "{t:?}");
        assert!(t.iter().any(|x| x.starts_with("You receive at least: 371")), "{t:?}");
        assert!(t.contains(&"Pool: P".to_string()));
        let s = json!({"fee_payer": "Me1", "summary": {"venue": "pumpswap", "amount_spl": "1000000000", "balance_spl": "5000000000", "quote_sol_out_lamports": "30000", "min_sol_out_lamports": "29000", "slippage_bps": 100, "fee_allowance_bps": 125}});
        let t: Vec<String> = review("sell", &s).iter().map(|(a, b)| format!("{a}: {b}")).collect();
        assert!(t.iter().any(|x| x.starts_with("You sell exactly: 1,000") || x.starts_with("You sell exactly: 1000")), "{t:?}");
    }

    #[test]
    fn shortvec_reads_multi_byte() {
        assert_eq!(shortvec(&[0x05]).unwrap(), (5, 1));
        assert_eq!(shortvec(&[0x80, 0x01]).unwrap(), (128, 2));
        assert!(shortvec(&[0x80]).is_err());
    }
}
