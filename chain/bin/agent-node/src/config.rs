//! Genesis and node config files (JSON).

use std::path::Path;

use agent_types::Pubkey;
use anyhow::{bail, Context, Result};
use consensus_pow::Genesis;
use serde::{Deserialize, Serialize};
use state::config::Config;

/// genesis.json: identical on every node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisFile {
    pub chain_id: u32,
    pub timestamp_ms: u64,
    /// Compact genesis target (hex string like "0x207fffff" or a number).
    pub target_bits: serde_json::Value,
    pub admin: String,
    #[serde(default)]
    pub attesters: Vec<String>,
    #[serde(default = "one")]
    pub attester_threshold: u8,
    /// Overrides by AdminConfig parameter name (see getParams), values as JSON.
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

fn one() -> u8 {
    1
}

pub fn parse_key(s: &str) -> Result<Pubkey> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(hex::decode(t)?.try_into().unwrap());
    }
    let v = bs58::decode(t).into_vec().with_context(|| format!("bad key {s:?}"))?;
    v.try_into().map_err(|_| anyhow::anyhow!("key {s:?} is not 32 bytes"))
}

fn num(v: &serde_json::Value) -> Result<u64> {
    Ok(match v {
        serde_json::Value::Number(n) => n.as_u64().context("not a u64")?,
        serde_json::Value::String(s) => match s.strip_prefix("0x") {
            Some(h) => u64::from_str_radix(h, 16)?,
            None => s.parse()?,
        },
        serde_json::Value::Bool(b) => *b as u64,
        _ => bail!("expected a number, got {v}"),
    })
}

impl GenesisFile {
    pub fn load(p: &Path) -> Result<Self> {
        serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?)
            .with_context(|| format!("parse {}", p.display()))
    }

    pub fn to_genesis(&self) -> Result<Genesis> {
        use state::config::keys;
        let mut c = Config::defaults(parse_key(&self.admin)?);
        c.attesters = self.attesters.iter().map(|a| parse_key(a)).collect::<Result<_>>()?;
        c.attester_threshold = self.attester_threshold;
        for (name, v) in &self.params {
            let k = (0..=keys::LAST).find(|k| keys::name(*k) == name).with_context(|| format!("unknown param {name}"))?;
            let bytes = match k {
                keys::ADMIN => parse_key(v.as_str().context("admin is a key string")?)?.to_vec(),
                keys::FALLBACK_CONFIG => hex::decode(v.as_str().context("hex")?)?,
                keys::ATTESTERS => bail!("set attesters at the top level"),
                keys::ATTESTER_THRESHOLD | keys::BRIDGE_PAUSED => vec![num(v)? as u8],
                keys::POW_LIMIT_BITS
                | keys::FINALITY_K
                | keys::MAX_BLOCK_TXS
                | keys::FALLBACK_M
                | keys::FALLBACK_N
                | keys::MAX_JOB_BLOCKS
                | keys::MIN_DIFFICULTY_BITS => (num(v)? as u32).to_le_bytes().to_vec(),
                _ => num(v)?.to_le_bytes().to_vec(),
            };
            c.set(k, &bytes).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        }
        c.validate().map_err(|e| anyhow::anyhow!("genesis params: {e}"))?;
        Ok(Genesis { chain_id: self.chain_id, timestamp_ms: self.timestamp_ms, target_bits: num(&self.target_bits)? as u32, config: c })
    }
}

/// node.json: per node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFile {
    pub data_dir: String,
    pub genesis: String,
    /// 32-byte ed25519 seed (hex) of the node's p2p identity.
    pub p2p_key: String,
    pub p2p_listen: String,
    pub rpc_listen: String,
    pub ws_listen: String,
    /// Static peers: `{ "key": base58/hex pubkey, "addr": "127.0.0.1:10640" }` (may include self).
    pub peers: Vec<PeerEntry>,
    #[serde(default = "default_verifier")]
    pub verifier: String,
    #[serde(default = "default_tip_ms")]
    pub tip_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    pub key: String,
    pub addr: String,
}

fn default_verifier() -> String {
    "pearl".into()
}
fn default_tip_ms() -> u64 {
    2000
}

impl NodeFile {
    pub fn load(p: &Path) -> Result<Self> {
        serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?)
            .with_context(|| format!("parse {}", p.display()))
    }
}
