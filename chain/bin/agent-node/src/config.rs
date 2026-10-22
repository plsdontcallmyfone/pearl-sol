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
    /// Static mode only.
    #[serde(default)]
    pub peers: Vec<PeerEntry>,
    /// "static" (Commonware discovery over `peers`, the M1 default) or "open".
    #[serde(default = "default_net")]
    pub net: String,
    /// Open mode: seeds as `host:port` or `key@host:port` (key base58 or hex pins it).
    #[serde(default)]
    pub seeds: Vec<String>,
    /// Open mode: address to advertise (default: p2p_listen when it is dialable).
    #[serde(default)]
    pub p2p_external: Option<String>,
    /// Open mode limits and timers (all optional).
    #[serde(default)]
    pub open: OpenFile,
    #[serde(default = "default_verifier")]
    pub verifier: String,
    #[serde(default = "default_tip_ms")]
    pub tip_interval_ms: u64,
    /// Blob store (data availability) limits, all optional (docs/NODE.md).
    #[serde(default)]
    pub blobs: BlobsFile,
}

/// Optional overrides of `blobs::BlobConfig`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlobsFile {
    /// false: no blob store (putBlob/getBlob fail, every job counts as available).
    pub enabled: Option<bool>,
    pub max_blob_bytes: Option<usize>,
    pub max_total_bytes: Option<u64>,
    pub grace_blocks: Option<u64>,
    pub put_rate_bytes_per_s: Option<u64>,
    pub put_burst_bytes: Option<u64>,
    pub peer_rate_bytes_per_s: Option<u64>,
    pub peer_burst_bytes: Option<u64>,
    /// RPC puts stop this far below max_total_bytes (0 or unset: 5%, at least two max blobs).
    pub rpc_reserve_bytes: Option<u64>,
    /// Blocks after posting that a segment's A blob is kept for mining (0: until its deadline).
    pub segment_mine_blocks: Option<u64>,
    /// false: a mining-only node, keeps segment A for mining only (no replay window).
    pub keep_replay: Option<bool>,
}

impl BlobsFile {
    pub fn config(&self) -> Result<Option<blobs::BlobConfig>> {
        if self.enabled == Some(false) {
            return Ok(None);
        }
        let mut c = blobs::BlobConfig::default();
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = self.$f { c.$f = v; } )* };
        }
        set!(max_blob_bytes, max_total_bytes, grace_blocks, put_rate_bytes_per_s, put_burst_bytes, peer_rate_bytes_per_s, peer_burst_bytes, rpc_reserve_bytes, segment_mine_blocks, keep_replay);
        if c.max_blob_bytes == 0 || c.max_blob_bytes > blobs::MAX_BLOB_BYTES_LIMIT {
            bail!("blobs.max_blob_bytes must be 1..={} (below the p2p message limit)", blobs::MAX_BLOB_BYTES_LIMIT);
        }
        Ok(Some(c))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEntry {
    pub key: String,
    pub addr: String,
}

/// Optional overrides of `net::open::Config` (see that type for meanings).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenFile {
    /// Allow loopback / private addresses (default: true when p2p_listen is local).
    pub allow_private: Option<bool>,
    pub max_outbound: Option<usize>,
    pub max_inbound: Option<usize>,
    pub max_inbound_per_ip: Option<usize>,
    pub max_inbound_per_subnet: Option<usize>,
    pub ban_threshold: Option<u32>,
    pub ban_duration_s: Option<u64>,
    pub dial_interval_ms: Option<u64>,
    pub gossip_interval_ms: Option<u64>,
    pub ping_interval_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
}

fn default_net() -> String {
    "static".into()
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

/// Parse a seed: `host:port` or `key@host:port` (DNS names resolve once, first address).
pub fn parse_seed(s: &str) -> Result<net::open::Seed> {
    use std::net::ToSocketAddrs as _;
    let s = s.trim();
    let (key, hostport) = match s.split_once('@') {
        Some((k, h)) => {
            let k = parse_key(k)?;
            let pk: commonware_cryptography::ed25519::PublicKey =
                commonware_codec::ReadExt::read(&mut &k[..]).map_err(|e| anyhow::anyhow!("seed key: {e}"))?;
            (Some(pk), h)
        }
        None => (None, s),
    };
    let addr = hostport
        .to_socket_addrs()
        .with_context(|| format!("seed {s:?}: expected host:port"))?
        .next()
        .with_context(|| format!("seed {s:?} resolves to nothing"))?;
    Ok(net::open::Seed { addr, key })
}

impl NodeFile {
    /// Build the open-network config (`namespace` = net::open_namespace(genesis hash)).
    pub fn open_config(&self, signer: commonware_cryptography::ed25519::PrivateKey, namespace: &[u8]) -> Result<net::open::Config> {
        use std::time::Duration;
        let listen: std::net::SocketAddr = self.p2p_listen.parse().context("p2p_listen")?;
        let o = &self.open;
        let allow = o.allow_private.unwrap_or_else(|| net::open::limits::is_local(listen.ip()));
        let mut c = if allow {
            net::open::Config::local(signer, namespace, listen)
        } else {
            net::open::Config::recommended(signer, namespace, listen)
        };
        c.allow_private = allow;
        c.external = match &self.p2p_external {
            Some(a) => Some(a.parse().context("p2p_external")?),
            None => None,
        };
        c.seeds = self.seeds.iter().map(|s| parse_seed(s)).collect::<Result<_>>()?;
        if let Some(v) = o.max_outbound {
            c.max_outbound = v;
        }
        if let Some(v) = o.max_inbound {
            c.max_inbound = v;
        }
        if let Some(v) = o.max_inbound_per_ip {
            c.max_inbound_per_ip = v;
        }
        if let Some(v) = o.max_inbound_per_subnet {
            c.max_inbound_per_subnet = v;
        }
        if let Some(v) = o.ban_threshold {
            c.ban_threshold = v;
        }
        if let Some(v) = o.ban_duration_s {
            c.ban_duration = Duration::from_secs(v);
        }
        if let Some(v) = o.dial_interval_ms {
            c.dial_interval = Duration::from_millis(v);
        }
        if let Some(v) = o.gossip_interval_ms {
            c.gossip_interval = Duration::from_millis(v);
        }
        if let Some(v) = o.ping_interval_ms {
            c.ping_interval = Duration::from_millis(v);
        }
        if let Some(v) = o.idle_timeout_ms {
            c.idle_timeout = Duration::from_millis(v);
        }
        Ok(c)
    }
}

/// Address book persistence for open mode: `[{"addr": "ip:port", "key": hex}]`.
/// Missing or unreadable files load as empty.
pub fn load_addrs(path: &Path) -> Vec<(std::net::SocketAddr, commonware_cryptography::ed25519::PublicKey)> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Vec<PeerEntry>>(&text) else { return Vec::new() };
    v.into_iter()
        .filter_map(|e| {
            let a = e.addr.parse().ok()?;
            let k = parse_key(&e.key).ok()?;
            let pk = commonware_codec::ReadExt::read(&mut &k[..]).ok()?;
            Some((a, pk))
        })
        .collect()
}

pub fn save_addrs(path: &Path, addrs: &[(std::net::SocketAddr, Option<commonware_cryptography::ed25519::PublicKey>, bool)]) {
    use commonware_codec::Encode as _;
    let v: Vec<PeerEntry> = addrs
        .iter()
        .filter_map(|(a, k, _)| k.as_ref().map(|k| PeerEntry { key: hex::encode(k.encode()), addr: a.to_string() }))
        .collect();
    let tmp = path.with_extension("json.tmp");
    if let Ok(t) = serde_json::to_string_pretty(&v) {
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::Signer as _;

    #[test]
    fn seeds_and_open_config() {
        let s = parse_seed("127.0.0.1:10640").unwrap();
        assert_eq!(s.addr, "127.0.0.1:10640".parse().unwrap());
        assert!(s.key.is_none());
        let k = bs58::encode(agent_types::tx::pubkey_from_seed(&[7u8; 32])).into_string();
        let s = parse_seed(&format!("{k}@127.0.0.1:10641")).unwrap();
        assert!(s.key.is_some());
        assert!(parse_seed("nonsense").is_err());
        let nf: NodeFile = serde_json::from_value(serde_json::json!({
            "data_dir": "/tmp/x", "genesis": "/tmp/g", "p2p_key": "00", "p2p_listen": "127.0.0.1:10641",
            "rpc_listen": "127.0.0.1:10632", "ws_listen": "127.0.0.1:10633",
            "net": "open", "seeds": ["127.0.0.1:10640"], "open": {"max_inbound": 3}
        }))
        .unwrap();
        let c = nf.open_config(commonware_cryptography::ed25519::PrivateKey::from_seed(1), b"ns").unwrap();
        assert!(c.allow_private);
        assert_eq!(c.max_inbound, 3);
        assert_eq!(c.seeds.len(), 1);
        assert!(nf.peers.is_empty());
        let dir = std::env::temp_dir().join(format!("pearl-sol-addrs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("p2p-addrs.json");
        let k = commonware_cryptography::ed25519::PrivateKey::from_seed(5).public_key();
        save_addrs(&f, &[("127.0.0.1:10642".parse().unwrap(), Some(k.clone()), true), ("127.0.0.1:1".parse().unwrap(), None, false)]);
        assert_eq!(load_addrs(&f), vec![("127.0.0.1:10642".parse().unwrap(), k)]);
        assert!(load_addrs(&dir.join("missing.json")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
