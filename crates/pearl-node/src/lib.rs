//! Client side of the v2 agent-chain node (SPEC v2 section 6, `rpc` crate).
//!
//! - [`NodeApi`]: what the miner and attester need from a node. [`HttpNode`] speaks
//!   JSON-RPC 2.0 over HTTP POST; tests implement the trait in process.
//! - [`tx`]: SPEC section 2 transaction encoding and signing.
//! - [`JobInfo`] / [`Template`]: the node's job and block-template objects, parsed
//!   leniently (32-byte values as hex or base58, u64 as a number or a string).
//!
//! Wire shapes follow docs/RPC.md (written by the node agent); see `rpc` below.

pub mod mock;
pub mod rpc;
pub mod tx;

use anyhow::{anyhow, bail, ensure, Context, Result};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_job::job::{Job, JobHeader, MiningConfig};
use pearl_job::pow::{BlockHeader, FallbackParams, MinedBlock};
use pearl_job::spec::{parse_key32, JobSpec};

pub use rpc::HttpNode;

pub const DEFAULT_RPC: &str = "http://127.0.0.1:10630";

/// A job as the node reports it (SPEC 4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    pub job_id: [u8; 32],
    pub poster: [u8; 32],
    pub terms_hash: [u8; 32],
    pub created_height: u64,
    /// Job share target (header bytes 72..76 of the 76-byte job header).
    pub share_target: u32,
    pub config: MiningConfig,
    pub m: u32,
    pub n: u32,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub data_uri: String,
    /// "Open", "Delivered", "Completed", "Expired" (case-insensitive on input).
    pub status: String,
    pub deliverer: Option<[u8; 32]>,
    pub result_hash: Option<[u8; 32]>,
    pub result_uri: String,
    pub evaluators: Vec<[u8; 32]>,
    pub threshold: u8,
    pub deadline_height: u64,
}

impl JobInfo {
    pub fn job_header(&self) -> JobHeader {
        JobHeader {
            job_pubkey: self.job_id,
            terms_hash: self.terms_hash,
            created_slot: self.created_height,
            target_bits: self.share_target,
        }
    }

    pub fn to_job(&self) -> Job {
        Job { header: self.job_header(), config: self.config, m: self.m, n: self.n, hash_a: self.hash_a, hash_b: self.hash_b }
    }

    /// The pearl-job JobSpec (job id as hex).
    pub fn to_spec(&self) -> JobSpec {
        JobSpec {
            job: hex::encode(self.job_id),
            terms_hash: hex::encode(self.terms_hash),
            created_slot: self.created_height,
            target_bits: self.share_target,
            config: hex::encode(self.config.to_bytes()),
            m: self.m,
            n: self.n,
            hash_a: hex::encode(self.hash_a),
            hash_b: hex::encode(self.hash_b),
            data_uri: self.data_uri.clone(),
            max_nonce: 0,
        }
    }

    pub fn is(&self, status: &str) -> bool {
        self.status.eq_ignore_ascii_case(status)
    }

    pub fn from_json(v: &Value) -> Result<Self> {
        let o = v.as_object().ok_or_else(|| anyhow!("job is not an object: {v}"))?;
        let get = |k: &str| o.get(k).filter(|x| !x.is_null());
        let req = |k: &str| get(k).ok_or_else(|| anyhow!("job field {k} missing"));
        let opt_key = |k: &str| -> Result<Option<[u8; 32]>> {
            match get(k) {
                Some(x) => {
                    let s = x.as_str().ok_or_else(|| anyhow!("{k} not a string"))?;
                    if s.is_empty() {
                        return Ok(None);
                    }
                    let b = parse_key32(s)?;
                    Ok(if b == [0; 32] { None } else { Some(b) })
                }
                None => Ok(None),
            }
        };
        let config = match get("config") {
            Some(c) => MiningConfig::from_bytes(&hex::decode(as_str(c)?.trim_start_matches("0x")).context("config hex")?)?,
            None => MiningConfig::localnet_default(),
        };
        Ok(Self {
            job_id: key32(alt(o, &["job_id", "id", "job"]).ok_or_else(|| anyhow!("job_id missing"))?)?,
            poster: opt_key("poster")?.unwrap_or([0; 32]),
            terms_hash: key32(req("terms_hash")?)?,
            created_height: num(alt(o, &["created_height", "created"]).ok_or_else(|| anyhow!("created_height missing"))?)?,
            share_target: num(alt(o, &["share_target", "target_bits"]).ok_or_else(|| anyhow!("share_target missing"))?)? as u32,
            config,
            m: num(req("m")?)? as u32,
            n: num(req("n")?)? as u32,
            hash_a: key32(req("hash_a")?)?,
            hash_b: key32(req("hash_b")?)?,
            data_uri: get("data_uri").map(as_str).transpose()?.unwrap_or_default().to_string(),
            status: get("status").map(as_str).transpose()?.unwrap_or("Open").to_string(),
            deliverer: opt_key("deliverer")?,
            result_hash: opt_key("result_hash")?,
            result_uri: get("result_uri").map(as_str).transpose()?.unwrap_or_default().to_string(),
            evaluators: match get("evaluators") {
                Some(Value::Array(a)) => a.iter().map(key32).collect::<Result<_>>()?,
                _ => vec![],
            },
            threshold: get("threshold").map(num).transpose()?.unwrap_or(1) as u8,
            deadline_height: get("deadline_height").map(num).transpose()?.unwrap_or(u64::MAX),
        })
        .and_then(|j: Self| {
            // The node also reports job_key: catch any job-header rule mismatch here.
            if let Some(k) = get("job_key") {
                let want = key32(k)?;
                let got = j.to_job().job_key();
                ensure!(want == got, "node job_key {} != computed {} (job header rule differs)", hex::encode(want), hex::encode(got));
            }
            Ok(j)
        })
    }

    pub fn to_json(&self) -> Value {
        let k = |b: &[u8; 32]| hex::encode(b);
        json!({
            "job_id": k(&self.job_id), "poster": bs58::encode(self.poster).into_string(), "terms_hash": k(&self.terms_hash),
            "created_height": self.created_height, "share_target": self.share_target,
            "config": hex::encode(self.config.to_bytes()), "m": self.m, "n": self.n,
            "hash_a": k(&self.hash_a), "hash_b": k(&self.hash_b), "data_uri": self.data_uri, "status": self.status.to_lowercase(),
            "deliverer": self.deliverer.map(|d| bs58::encode(d).into_string()),
            "result_hash": self.result_hash.map(|r| hex::encode(r)), "result_uri": self.result_uri,
            "evaluators": self.evaluators.iter().map(|e| bs58::encode(e).into_string()).collect::<Vec<_>>(),
            "threshold": self.threshold, "deadline_height": self.deadline_height,
            "job_key": hex::encode(self.to_job().job_key()), "job_header": hex::encode(self.job_header().to_bytes()),
        })
    }
}

/// A block template for one payout key.
#[derive(Debug, Clone)]
pub struct Template {
    /// All fields final except `nonce` and `proof_hash` (both zero).
    pub header: BlockHeader,
    /// The job named by `header.job_id` (None for a fallback template).
    pub job: Option<JobInfo>,
    pub template_id: Option<String>,
    pub chain_id: Option<u32>,
    pub min_fee: u64,
    pub fallback: FallbackParams,
    /// The node's JSON, for fields this client does not know.
    pub raw: Value,
}

impl Template {
    /// Same work as another template (same header except timestamp): nonces carry over.
    pub fn same_work(&self, o: &Template) -> bool {
        let (mut a, mut b) = (self.header, o.header);
        a.timestamp_ms = 0;
        b.timestamp_ms = 0;
        a == b && self.template_id == o.template_id
    }

    pub fn from_json(v: &Value) -> Result<Self> {
        let o = v.as_object().ok_or_else(|| anyhow!("template is not an object: {v}"))?;
        let get = |k: &str| o.get(k).filter(|x| !x.is_null());
        let header = match get("header") {
            Some(Value::String(h)) => BlockHeader::from_bytes(&hex::decode(h.trim_start_matches("0x")).context("header hex")?)?,
            Some(Value::Object(_)) => header_from_fields(get("header").unwrap())?,
            _ => header_from_fields(v)?,
        };
        let job = match get("job") {
            Some(j) if j.is_object() => Some(JobInfo::from_json(j)?),
            _ => None,
        };
        if let Some(j) = &job {
            ensure!(j.job_id == header.job_id, "template job does not match header job_id");
        }
        let mut fallback = FallbackParams::default();
        if let Some(f) = get("fallback") {
            if let Some(m) = f.get("m") {
                fallback.m = num(m)? as u32;
            }
            if let Some(n) = f.get("n") {
                fallback.n = num(n)? as u32;
            }
            if let Some(c) = f.get("config").filter(|c| !c.is_null()) {
                fallback.config = MiningConfig::from_bytes(&hex::decode(as_str(c)?.trim_start_matches("0x"))?)?;
            }
        }
        Ok(Self {
            header,
            job,
            template_id: get("template_id").map(|t| t.as_str().map(String::from).unwrap_or_else(|| t.to_string())),
            chain_id: get("chain_id").map(num).transpose()?.map(|c| c as u32),
            min_fee: get("min_fee").map(num).transpose()?.unwrap_or(0),
            fallback,
            raw: v.clone(),
        })
    }
}

fn header_from_fields(v: &Value) -> Result<BlockHeader> {
    let o = v.as_object().ok_or_else(|| anyhow!("header is not an object"))?;
    let k = |name: &str| -> Result<[u8; 32]> { o.get(name).filter(|x| !x.is_null()).map(key32).transpose().map(|x| x.unwrap_or([0; 32])) };
    let n = |name: &str| -> Result<u64> { o.get(name).filter(|x| !x.is_null()).map(num).transpose().map(|x| x.unwrap_or(0)) };
    Ok(BlockHeader {
        version: o.get("version").map(num).transpose()?.unwrap_or(1) as u32,
        height: n("height")?,
        prev_hash: k("prev_hash")?,
        timestamp_ms: n("timestamp_ms")?,
        target_bits: n("target_bits")? as u32,
        job_id: k("job_id")?,
        miner: k("miner")?,
        tx_root: k("tx_root")?,
        state_root: k("state_root")?,
        withdraw_root: k("withdraw_root")?,
        proof_hash: [0; 32],
        nonce: 0,
    })
}

/// Template header fields as JSON (hex for 32-byte values).
pub fn header_fields_json(h: &BlockHeader) -> Value {
    let k = |b: &[u8; 32]| hex::encode(b);
    json!({"version": h.version, "height": h.height, "prev_hash": k(&h.prev_hash), "timestamp_ms": h.timestamp_ms,
           "target_bits": h.target_bits, "job_id": k(&h.job_id), "miner": k(&h.miner), "tx_root": k(&h.tx_root),
           "state_root": k(&h.state_root), "withdraw_root": k(&h.withdraw_root), "proof_hash": k(&h.proof_hash),
           "nonce": h.nonce})
}

fn alt<'a>(o: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| o.get(*k).filter(|x| !x.is_null()))
}

fn as_str(v: &Value) -> Result<&str> {
    v.as_str().ok_or_else(|| anyhow!("expected a string, got {v}"))
}

/// A 32-byte value: hex or base58 string, or a JSON byte array.
pub fn key32(v: &Value) -> Result<[u8; 32]> {
    match v {
        Value::String(s) => parse_key32(s),
        Value::Array(a) => {
            let b: Vec<u8> = a.iter().map(|x| x.as_u64().map(|y| y as u8).ok_or_else(|| anyhow!("bad byte"))).collect::<Result<_>>()?;
            b.try_into().map_err(|_| anyhow!("byte array is not 32 long"))
        }
        _ => bail!("expected a 32-byte value, got {v}"),
    }
}

/// u64 from a number or a decimal / 0x-hex string.
pub fn num(v: &Value) -> Result<u64> {
    match v {
        Value::Number(n) => n.as_u64().ok_or_else(|| anyhow!("not a u64: {n}")),
        Value::String(s) => Ok(match s.strip_prefix("0x") {
            Some(h) => u64::from_str_radix(h, 16)?,
            None => s.parse()?,
        }),
        _ => bail!("expected a number, got {v}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Account {
    pub balance: u64,
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    Accepted { block_hash: [u8; 32] },
    Rejected { reason: String },
}

/// What the miner and attester need from a node.
pub trait NodeApi: Send + Sync {
    fn block_template(&self, payout: &[u8; 32]) -> Result<Template>;
    /// A fallback-block template (`getBlockTemplate {job_id: "fallback"}`), for when
    /// the node's chosen job cannot be mined here.
    fn fallback_template(&self, payout: &[u8; 32]) -> Result<Template> {
        let t = self.block_template(payout)?;
        ensure!(t.header.is_fallback(), "node offers no fallback template");
        Ok(t)
    }
    fn submit_block(&self, template: &Template, block: &MinedBlock) -> Result<SubmitOutcome>;
    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>>;
    fn get_account(&self, account: &[u8; 32]) -> Result<Account>;
    /// Returns the tx hash (hex) the node reports.
    fn send_tx(&self, wire: &[u8]) -> Result<String>;
}

/// Read an ed25519 key file: Solana JSON array (64 bytes), or a base58 / hex string of
/// the 64-byte keypair or the 32-byte seed.
pub fn read_key_file(path: &std::path::Path) -> Result<SigningKey> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read key file {}", path.display()))?;
    let t = raw.trim();
    let bytes: Vec<u8> = if t.starts_with('[') {
        serde_json::from_str(t).with_context(|| format!("parse {}", path.display()))?
    } else if t.len() % 2 == 0 && (t.len() == 64 || t.len() == 128) && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(t)?
    } else {
        bs58::decode(t).into_vec().with_context(|| format!("{} is not JSON, hex or base58", path.display()))?
    };
    let sk = match bytes.len() {
        32 => SigningKey::from_bytes(&bytes[..].try_into().unwrap()),
        64 => {
            let sk = SigningKey::from_bytes(&bytes[..32].try_into().unwrap());
            ensure!(sk.verifying_key().to_bytes()[..] == bytes[32..], "{}: public half does not match", path.display());
            sk
        }
        n => bail!("{}: key is {n} bytes, expected 32 or 64", path.display()),
    };
    Ok(sk)
}

pub fn b58(b: &[u8; 32]) -> String {
    bs58::encode(b).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_hex_header_and_fields() {
        let h = BlockHeader { version: 1, height: 3, job_id: [0; 32], miner: [4; 32], target_bits: 0x1e040000, ..Default::default() };
        let t = Template::from_json(&json!({"header": hex::encode(h.to_bytes()), "chain_id": 7, "template_id": "x"})).unwrap();
        assert_eq!(t.header, h);
        assert_eq!(t.chain_id, Some(7));
        let t2 = Template::from_json(&header_fields_json(&h)).unwrap();
        assert_eq!(t2.header, h);
        assert!(t.job.is_none());
    }
}
