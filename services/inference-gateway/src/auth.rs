//! Request authentication (SPEC 5.4): wallet-signed headers or a wallet-bound API key.

use std::collections::{BTreeMap, HashSet, VecDeque};

use anyhow::{anyhow, bail, ensure, Result};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const AUTH_DOMAIN: &str = "cadre-auth-v1";
/// How far a signed request's timestamp may be from the gateway clock.
pub const MAX_SKEW_MS: u64 = 120_000;
pub const KEY_PREFIX: &str = "cadre_sk_";

pub fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// The bytes a wallet signs: `"cadre-auth-v1\n" + METHOD + "\n" + PATH + "\n" + TS + "\n" + hex(sha256(body))`.
pub fn auth_message(method: &str, path: &str, ts_ms: u64, body: &[u8]) -> Vec<u8> {
    format!("{AUTH_DOMAIN}\n{}\n{path}\n{ts_ms}\n{}", method.to_uppercase(), sha256_hex(body)).into_bytes()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiKey {
    pub key_id: String,
    pub wallet: String,
    pub created_ms: u64,
    /// Last request authenticated with this key (unix ms); none until first use. Kept in
    /// the ledger, so it is as of the gateway's last ledger save.
    #[serde(default)]
    pub last_used_ms: Option<u64>,
}

/// When this process loaded its keys (unix ms). The replay guard lives in memory only, so a
/// signed request is accepted only if it was signed after the gateway started: a request
/// captured before a restart cannot be replayed into the fresh guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootMs(pub u64);

impl Default for BootMs {
    fn default() -> Self {
        BootMs(crate::now_ms())
    }
}

/// API keys (stored as sha256 of the key) and the replay guard for signatures.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Keys {
    /// sha256(api key) hex -> key record.
    pub keys: BTreeMap<String, ApiKey>,
    /// Signatures seen in the last 2 x MAX_SKEW_MS, oldest first (expiry order) ...
    #[serde(skip)]
    pub seen: VecDeque<(u64, String)>,
    /// ... and the same signatures as a set (constant-time replay lookups).
    #[serde(skip)]
    pub seen_set: HashSet<String>,
    #[serde(skip)]
    pub boot_ms: BootMs,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// How a request is authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    Wallet([u8; 32]),
    Key { wallet: [u8; 32], key_id: String },
}

impl Auth {
    pub fn wallet(&self) -> Option<[u8; 32]> {
        match self {
            Auth::None => None,
            Auth::Wallet(w) | Auth::Key { wallet: w, .. } => Some(*w),
        }
    }
}

impl Keys {
    /// Authenticate: an API key (`Authorization: Bearer cadre_sk_...`) or the wallet
    /// signature headers. No credentials at all is `Auth::None`; bad credentials are an error.
    pub fn check(&mut self, method: &str, path: &str, headers: &[(String, String)], body: &[u8], now_ms: u64) -> Result<Auth> {
        if let Some(a) = header(headers, "authorization") {
            let k = a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer ")).unwrap_or(a).trim();
            if k.starts_with(KEY_PREFIX) {
                let rec = self.keys.get_mut(&sha256_hex(k.as_bytes())).ok_or_else(|| anyhow!("unknown or revoked API key"))?;
                rec.last_used_ms = Some(now_ms);
                return Ok(Auth::Key { wallet: parse_wallet(&rec.wallet)?, key_id: rec.key_id.clone() });
            }
        }
        let (Some(w), Some(ts), Some(sig)) = (header(headers, "x-cadre-wallet"), header(headers, "x-cadre-timestamp"), header(headers, "x-cadre-signature")) else {
            if header(headers, "x-cadre-wallet").is_some() || header(headers, "x-cadre-signature").is_some() {
                bail!("wallet auth needs X-Cadre-Wallet, X-Cadre-Timestamp and X-Cadre-Signature");
            }
            return Ok(Auth::None);
        };
        let wallet = parse_wallet(w)?;
        let ts: u64 = ts.trim().parse().map_err(|_| anyhow!("X-Cadre-Timestamp must be unix milliseconds"))?;
        ensure!(ts.abs_diff(now_ms) <= MAX_SKEW_MS, "X-Cadre-Timestamp is more than 120 s from the gateway clock");
        ensure!(ts >= self.boot_ms.0, "X-Cadre-Timestamp is before the gateway started; sign the request again");
        let sig: [u8; 64] = hex::decode(sig.trim()).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| anyhow!("X-Cadre-Signature must be 64 bytes hex"))?;
        let vk = VerifyingKey::from_bytes(&wallet).map_err(|_| anyhow!("X-Cadre-Wallet is not an ed25519 key"))?;
        vk.verify_strict(&auth_message(method, path, ts, body), &Signature::from_bytes(&sig)).map_err(|_| anyhow!("bad X-Cadre-Signature"))?;
        let sh = hex::encode(sig);
        while self.seen.front().is_some_and(|(t, _)| *t + 2 * MAX_SKEW_MS < now_ms) {
            if let Some((_, old)) = self.seen.pop_front() {
                self.seen_set.remove(&old);
            }
        }
        ensure!(!self.seen_set.contains(&sh), "this signature was already used");
        self.seen_set.insert(sh.clone());
        self.seen.push_back((now_ms, sh));
        Ok(Auth::Wallet(wallet))
    }

    /// New API key for `wallet`: `(key_id, api_key)`; only the hash is kept.
    pub fn create(&mut self, wallet: &[u8; 32], now_ms: u64) -> (String, String) {
        let mut raw = [0u8; 24];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut raw);
        let key = format!("{KEY_PREFIX}{}", hex::encode(raw));
        let h = sha256_hex(key.as_bytes());
        let key_id = format!("key_{}", &h[..12]);
        self.keys.insert(h, ApiKey { key_id: key_id.clone(), wallet: bs58::encode(wallet).into_string(), created_ms: now_ms, last_used_ms: None });
        (key_id, key)
    }

    /// `wallet`'s keys, oldest first (records only: the key itself is never kept).
    pub fn list(&self, wallet: &[u8; 32]) -> Vec<ApiKey> {
        let w = bs58::encode(wallet).into_string();
        let mut v: Vec<ApiKey> = self.keys.values().filter(|k| k.wallet == w).cloned().collect();
        v.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then_with(|| a.key_id.cmp(&b.key_id)));
        v
    }

    /// Revoke one of `wallet`'s keys.
    pub fn revoke(&mut self, wallet: &[u8; 32], key_id: &str) -> Result<()> {
        let w = bs58::encode(wallet).into_string();
        let h = self.keys.iter().find(|(_, k)| k.key_id == key_id && k.wallet == w).map(|(h, _)| h.clone()).ok_or_else(|| anyhow!("no such key for this wallet"))?;
        self.keys.remove(&h);
        Ok(())
    }
}

pub fn parse_wallet(s: &str) -> Result<[u8; 32]> {
    pearl_job::spec::parse_key32(s.trim()).map_err(|_| anyhow!("{s:?} is not a base58 or hex ed25519 public key"))
}
