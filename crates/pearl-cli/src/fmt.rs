//! Number, address and layout formatting. Display only: JSON output always carries
//! the exact base-unit strings next to these.

use anyhow::{bail, ensure, Result};

/// Chain base units per token (9 dp, docs/RPC.md).
pub const CHAIN_DP: u32 = 9;
/// Solana SPL base units per token (6 dp).
pub const SPL_DP: u32 = 6;

/// `1234567` -> `1,234,567`.
pub fn thousands(n: u128) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Base units with `dp` decimals, shown with at most `frac` fraction digits
/// (truncated, trailing zeros dropped): `amount(1_500_000_000, 9, 4)` = `1.5`.
pub fn amount(units: u128, dp: u32, frac: u32) -> String {
    let scale = 10u128.pow(dp);
    let whole = units / scale;
    let rem = units % scale;
    let shown = rem / 10u128.pow(dp.saturating_sub(frac));
    let mut f = format!("{:0width$}", shown, width = frac.min(dp) as usize);
    while f.ends_with('0') {
        f.pop();
    }
    if f.is_empty() {
        thousands(whole)
    } else {
        format!("{}.{}", thousands(whole), f)
    }
}

/// Chain units as tokens, 4 fraction digits.
pub fn tokens(units: u128) -> String {
    amount(units, CHAIN_DP, 4)
}

/// SPL units as tokens, 4 fraction digits.
pub fn spl(units: u128) -> String {
    amount(units, SPL_DP, 4)
}

/// Lamports as SOL, 4 fraction digits.
pub fn sol(lamports: u128) -> String {
    amount(lamports, 9, 4)
}

/// Compact form for tight cells: `18.83M`, `1.2B`, `950.1K`, else `amount`.
pub fn compact(units: u128, dp: u32) -> String {
    let whole = units / 10u128.pow(dp);
    let (div, suffix) = match whole {
        w if w >= 1_000_000_000 => (1_000_000_000u128, "B"),
        w if w >= 1_000_000 => (1_000_000, "M"),
        w if w >= 10_000 => (1_000, "K"),
        _ => return amount(units, dp, 4),
    };
    amount(units / div, dp, 2) + suffix
}

/// Decimal token string -> base units with `dp` decimals, exact (no float):
/// `parse_amount("1.5", 9)` = 1_500_000_000. Refuses more fraction digits than `dp`.
pub fn parse_amount(s: &str, dp: u32) -> Result<u64> {
    let s = s.trim().replace(['_', ','], "");
    ensure!(!s.is_empty(), "empty amount");
    let (w, f) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s.as_str(), ""),
    };
    ensure!(w.chars().all(|c| c.is_ascii_digit()) && f.chars().all(|c| c.is_ascii_digit()), "amount {s:?} is not a decimal number");
    ensure!(!(w.is_empty() && f.is_empty()), "amount {s:?} is not a decimal number");
    if f.len() > dp as usize {
        bail!("amount {s:?} has more than {dp} decimals");
    }
    let whole: u128 = if w.is_empty() { 0 } else { w.parse()? };
    let frac: u128 = if f.is_empty() { 0 } else { f.parse::<u128>()? * 10u128.pow(dp - f.len() as u32) };
    let v = whole.checked_mul(10u128.pow(dp)).and_then(|x| x.checked_add(frac)).filter(|x| *x <= u64::MAX as u128);
    v.map(|x| x as u64).ok_or_else(|| anyhow::anyhow!("amount {s:?} is too large"))
}

/// `7xKXabc...xyz9` -> `7xKX...xyz9` (4 + 4).
pub fn short(s: &str) -> String {
    short_n(s, 4)
}

pub fn short_n(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= 2 * n + 3 {
        return s.to_string();
    }
    format!("{}...{}", c[..n].iter().collect::<String>(), c[c.len() - n..].iter().collect::<String>())
}

/// Dotted leader: `label ......value`, exactly `width` columns when it fits. The
/// value is kept whole; the label is cut first.
pub fn leader(label: &str, value: &str, width: usize) -> String {
    let vl = value.chars().count();
    let ll = label.chars().count();
    if vl + 2 >= width {
        return value.chars().take(width).collect();
    }
    let room = width - vl;
    let label: String = if ll + 2 > room { label.chars().take(room.saturating_sub(2)).collect() } else { label.to_string() };
    let dots = room - label.chars().count() - 1;
    format!("{label} {}{value}", ".".repeat(dots))
}

/// Split a shell line into words; single or double quotes group words.
pub fn split_words(line: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => {
                cur.push(c);
                started = true;
            }
        }
    }
    ensure!(quote.is_none(), "unclosed quote");
    if started {
        out.push(cur);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Cadre-chain addresses (docs/SPEC.md 2.1): shown as Cx, typed as Cx or legacy base58.
// Solana addresses (Solana recipients, SPL, program ids, Solscan links) stay base58.

/// A Cadre-chain address typed by a person: checksummed or all-lowercase Cx, legacy base58,
/// or 64 hex. A Cx string with the wrong casing fails with the checksum error.
pub fn parse_chain_addr(s: &str) -> Result<[u8; 32]> {
    let t = s.trim();
    if t.starts_with(agent_types::cx::CX_PREFIX) {
        return agent_types::cx::parse_address(t).map_err(|e| anyhow::anyhow!("{t:?}: {e}"));
    }
    let h = t.trim_start_matches("0x");
    if h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(hex::decode(h)?.try_into().unwrap());
    }
    agent_types::cx::parse_address(t).map_err(|e| anyhow::anyhow!("{t:?} is not a Cadre address (Cx..., or legacy base58): {e}"))
}

/// Full checksummed Cx of a key.
pub fn cx(k: &[u8; 32]) -> String {
    agent_types::cx::to_cx(k)
}

/// Short Cx of a key: `Cxa380...7d06`.
pub fn cxs(k: &[u8; 32]) -> String {
    agent_types::cx::short_cx(k)
}

/// Full Cx of an address string (base58 from the RPC, or Cx); anything else unchanged.
pub fn cx_str(s: &str) -> String {
    agent_types::cx::display_cx(s).unwrap_or_else(|_| s.to_string())
}

/// Short Cx of an address string; anything that is not an address goes through `short`.
pub fn short_addr(s: &str) -> String {
    match agent_types::cx::parse_address(s) {
        Ok(k) => cxs(&k),
        Err(_) => short(s),
    }
}

/// Field names that hold a Cadre-chain address in the CLI's JSON (same list as the node's
/// chain/rpc/src/cxjson.rs); objects under `solana` and Solana tx views are skipped.
const ADDRESS_FIELDS: &[&str] = &[
    "account", "address", "admin", "admins", "agent", "a", "b", "attesters", "author", "contract", "creator",
    "deliverer", "deployer", "escrow", "evaluators", "follower", "from", "house_operators", "member", "miner",
    "operator", "peer", "pool", "poster", "recipient", "registrant", "release_key", "signers", "target", "to",
];

fn cx_value(v: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    let one = |s: &str| -> Option<String> {
        let k: [u8; 32] = bs58::decode(s).into_vec().ok()?.try_into().ok()?;
        Some(cx(&k))
    };
    match v {
        Value::String(s) => one(s).map(Value::String),
        Value::Array(a) if !a.is_empty() && a.iter().all(|x| x.is_string()) => {
            a.iter().map(|x| one(x.as_str()?).map(Value::String)).collect::<Option<Vec<_>>>().map(Value::Array)
        }
        _ => None,
    }
}

/// `--json`: add `k_cx` next to every base58 Cadre address field `k` (base58 kept for the
/// transition, docs/CLI.md "Addresses").
pub fn add_cx_fields(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            if m.get("chain").and_then(|c| c.as_str()) == Some("solana") {
                return;
            }
            for (k, x) in m.iter_mut() {
                if k != "solana" {
                    add_cx_fields(x);
                }
            }
            let adds: Vec<(String, Value)> = m
                .iter()
                .filter(|(k, _)| ADDRESS_FIELDS.contains(&k.as_str()) && !m.contains_key(&format!("{k}_cx")))
                .filter_map(|(k, x)| cx_value(x).map(|c| (format!("{k}_cx"), c)))
                .collect();
            m.extend(adds);
        }
        Value::Array(a) => a.iter_mut().for_each(add_cx_fields),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(100), "100");
        assert_eq!(tokens(1_500_000_000), "1.5");
        assert_eq!(tokens(40_740_000_000), "40.74");
        assert_eq!(tokens(1), "0");
        assert_eq!(tokens(123_456_789_012_345), "123,456.789");
        assert_eq!(spl(18_836_393_149_842), "18,836,393.1498");
        assert_eq!(sol(10_000_000), "0.01");
        assert_eq!(compact(18_836_393_149_842, 6), "18.83M");
        assert_eq!(compact(1_000_000_000_000_000_000, 9), "1B");
        assert_eq!(compact(12_345_000_000_000, 9), "12.34K");
        assert_eq!(compact(1_234_000_000, 9), "1.234");
    }

    #[test]
    fn parse() {
        assert_eq!(parse_amount("1.5", 9).unwrap(), 1_500_000_000);
        assert_eq!(parse_amount("0.000000001", 9).unwrap(), 1);
        assert_eq!(parse_amount("1,000", 6).unwrap(), 1_000_000_000);
        assert_eq!(parse_amount(".5", 6).unwrap(), 500_000);
        assert_eq!(parse_amount("7", 0).unwrap(), 7);
        assert!(parse_amount("1.0000001", 6).is_err());
        assert!(parse_amount("-1", 9).is_err());
        assert!(parse_amount("abc", 9).is_err());
        assert!(parse_amount(".", 9).is_err());
        assert!(parse_amount("99999999999", 9).is_err());
    }

    #[test]
    fn layout() {
        assert_eq!(short("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"), "7xKX...gAsU");
        assert_eq!(short("abc"), "abc");
        let l = leader("supply locked", "23.4%", 24);
        assert_eq!(l, "supply locked .....23.4%");
        assert_eq!(l.chars().count(), 24);
        assert_eq!(leader("a very long label indeed", "OK", 10), "a very .OK");
        assert_eq!(leader("x", "12345678901", 8), "12345678");
    }

    #[test]
    fn cx_addresses() {
        let b58 = "C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7";
        let full = "Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06";
        let k = parse_chain_addr(b58).unwrap();
        assert_eq!(parse_chain_addr(full).unwrap(), k);
        assert_eq!(parse_chain_addr(&full.to_ascii_lowercase().replacen("cx", "Cx", 1)).unwrap(), k);
        assert_eq!(parse_chain_addr("a38030920a564589bdfe976bdb50f37cdeaecbbb34b3934e65b3f1a717bb7d06").unwrap(), k);
        let bad = full.replacen("bDFe", "bdFe", 1);
        assert!(format!("{:#}", parse_chain_addr(&bad).unwrap_err()).contains("checksum failed"));
        assert!(parse_chain_addr("nope").is_err());
        assert_eq!(cx(&k), full);
        assert_eq!(cxs(&k), "Cxa380...7d06");
        assert_eq!(short_addr(b58), "Cxa380...7d06");
        assert_eq!(short_addr(full), "Cxa380...7d06");
        assert_eq!(short_addr("ab".repeat(32).as_str()), "abab...abab");
        assert_eq!(cx_str(b58), full);
        let mut j = serde_json::json!({"account": b58, "solana": {"address": b58}, "vault": b58, "txs": [{"from": b58}], "evaluators": [b58]});
        add_cx_fields(&mut j);
        assert_eq!(j["account"], b58);
        assert_eq!(j["account_cx"], full);
        assert!(j["solana"].get("address_cx").is_none());
        assert!(j.get("vault_cx").is_none());
        assert_eq!(j["txs"][0]["from_cx"], full);
        assert_eq!(j["evaluators_cx"][0], full);
        let mut s = serde_json::json!({"chain": "solana", "signer": b58, "account": b58});
        add_cx_fields(&mut s);
        assert!(s.get("account_cx").is_none());
    }

    #[test]
    fn words() {
        assert_eq!(split_words("send  abc 1.5").unwrap(), ["send", "abc", "1.5"]);
        assert_eq!(split_words("register 'my agent' \"\"").unwrap(), ["register", "my agent", ""]);
        assert!(split_words("x 'open").is_err());
        assert!(split_words("   ").unwrap().is_empty());
    }
}
