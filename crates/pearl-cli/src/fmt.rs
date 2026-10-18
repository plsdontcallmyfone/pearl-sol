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
    fn words() {
        assert_eq!(split_words("send  abc 1.5").unwrap(), ["send", "abc", "1.5"]);
        assert_eq!(split_words("register 'my agent' \"\"").unwrap(), ["register", "my agent", ""]);
        assert!(split_words("x 'open").is_err());
        assert!(split_words("   ").unwrap().is_empty());
    }
}
