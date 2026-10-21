//! Cadre "Cx" address format (docs/SPEC.md 2.1): display and parsing of account keys.
//!
//! Nothing changes on chain: an address is the raw 32-byte ed25519 public key in every
//! transaction, signature and state record. `Cx` is only how people and JSON see it:
//!
//! ```text
//! Cx || 64 hex of the key, checksum casing over blake3 (EIP-55 style):
//!   h = blake3(ascii(lowercase 64-hex)); hex letter i is uppercase iff nibble i of h >= 8
//!   (nibble 2j = h[j] >> 4, nibble 2j+1 = h[j] & 15)
//! ```
//!
//! [`parse_address`] accepts the checksummed form, all-lowercase `Cx` (no checksum) and the
//! legacy base58 form; any other casing is [`CxError::Checksum`]. Shared vectors with every TS
//! implementation: `vectors/cx-address.json` at the repo root (tests/cx.rs).

use crate::Pubkey;
use thiserror::Error;

pub const CX_PREFIX: &str = "Cx";
pub const CX_LEN: usize = 66;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CxError {
    #[error("address is empty")]
    Empty,
    #[error("a Cx address is Cx + 64 hex characters (66 in all), got {0}")]
    BadLength(usize),
    #[error("a Cx address has only hex characters (0-9, a-f) after Cx")]
    BadHex,
    #[error("address checksum failed: the upper and lower case letters do not match this address, so it was probably mistyped (an all-lowercase Cx address is also accepted)")]
    Checksum,
    #[error("{0:?} is not a Cadre address (expected Cx + 64 hex, or a legacy base58 key)")]
    BadBase58(String),
    #[error("base58 address decodes to {0} bytes, not 32")]
    Not32Bytes(usize),
}

impl CxError {
    /// Stable code shared with the TS implementation and the vectors file.
    pub fn code(&self) -> &'static str {
        match self {
            CxError::Empty => "empty",
            CxError::BadLength(_) => "bad_length",
            CxError::BadHex => "bad_hex",
            CxError::Checksum => "checksum",
            CxError::BadBase58(_) => "bad_base58",
            CxError::Not32Bytes(_) => "not_32_bytes",
        }
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn lower_hex(k: &Pubkey) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (i, b) in k.iter().enumerate() {
        out[2 * i] = HEX[(b >> 4) as usize];
        out[2 * i + 1] = HEX[(b & 15) as usize];
    }
    out
}

fn checksum(lower: &[u8; 64]) -> [u8; 64] {
    let h = blake3::hash(lower);
    let h = h.as_bytes();
    let mut out = *lower;
    for (i, c) in out.iter_mut().enumerate() {
        let nib = if i % 2 == 0 { h[i / 2] >> 4 } else { h[i / 2] & 15 };
        if c.is_ascii_lowercase() && nib >= 8 {
            *c = c.to_ascii_uppercase();
        }
    }
    out
}

/// Full checksummed address: `Cx` + 64 hex (66 characters).
pub fn to_cx(k: &Pubkey) -> String {
    let c = checksum(&lower_hex(k));
    let mut s = String::with_capacity(CX_LEN);
    s.push_str(CX_PREFIX);
    s.push_str(std::str::from_utf8(&c).expect("ascii"));
    s
}

/// Short UI form: `Cx` + first 4 + `...` + last 4 checksummed hex characters.
pub fn short_cx(k: &Pubkey) -> String {
    let full = to_cx(k);
    format!("{}...{}", &full[..6], &full[CX_LEN - 4..])
}

/// Legacy base58 form of a key.
pub fn to_base58(k: &Pubkey) -> String {
    bs58::encode(k).into_string()
}

/// Parse a Cx address (checksummed or all lowercase) or a legacy base58 key.
pub fn parse_address(input: &str) -> Result<Pubkey, CxError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(CxError::Empty);
    }
    if let Some(body) = s.strip_prefix(CX_PREFIX) {
        if body.len() != 64 {
            return Err(CxError::BadLength(s.len()));
        }
        if !body.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(CxError::BadHex);
        }
        let mut lower = [0u8; 64];
        lower.copy_from_slice(body.to_ascii_lowercase().as_bytes());
        if body.as_bytes() != lower && body.as_bytes() != checksum(&lower) {
            return Err(CxError::Checksum);
        }
        let mut k = [0u8; 32];
        for i in 0..32 {
            let d = |c: u8| (c as char).to_digit(16).expect("hex") as u8;
            k[i] = d(lower[2 * i]) << 4 | d(lower[2 * i + 1]);
        }
        return Ok(k);
    }
    let v = bs58::decode(s).into_vec().map_err(|_| {
        let shown: String = s.chars().take(80).collect();
        CxError::BadBase58(shown)
    })?;
    let n = v.len();
    v.try_into().map_err(|_| CxError::Not32Bytes(n))
}

/// True when [`parse_address`] accepts the string.
pub fn is_address(s: &str) -> bool {
    parse_address(s).is_ok()
}

/// Checksummed Cx display of any accepted address string.
pub fn display_cx(s: &str) -> Result<String, CxError> {
    parse_address(s).map(|k| to_cx(&k))
}

/// Short display of any accepted address string; the input unchanged when it does not parse.
pub fn short_address(s: &str) -> String {
    parse_address(s).map(|k| short_cx(&k)).unwrap_or_else(|_| s.to_string())
}
