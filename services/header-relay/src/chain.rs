//! Agent-chain node side (docs/RPC.md): headers with proofs, block txs, tx_root paths,
//! params, withdrawals.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use pearl_node::HttpNode;
use pearl_verify_sbf as pv;
use serde_json::{json, Value};

pub fn b64d(s: &str) -> Result<Vec<u8>> {
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}
pub fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

#[derive(Clone, Debug)]
pub struct ChainHeader {
    pub bytes: [u8; 256],
    pub hash: [u8; 32],
    pub height: u64,
    pub proof: Vec<u8>,
    pub cum_work: Option<String>,
}

impl ChainHeader {
    pub fn prev(&self) -> [u8; 32] {
        self.bytes[12..44].try_into().unwrap()
    }
    pub fn timestamp_ms(&self) -> u64 {
        u64::from_le_bytes(self.bytes[44..52].try_into().unwrap())
    }
    pub fn is_fallback(&self) -> bool {
        self.bytes[56..88].iter().all(|&b| b == 0)
    }
    pub fn job_id(&self) -> [u8; 32] {
        self.bytes[56..88].try_into().unwrap()
    }
}

pub struct Node {
    pub rpc: HttpNode,
}

impl Node {
    pub fn new(url: &str) -> Self {
        Self { rpc: HttpNode::new(url) }
    }

    pub fn info(&self) -> Result<Value> {
        self.rpc.call("getChainInfo", json!({}))
    }

    pub fn params(&self) -> Result<Value> {
        self.rpc.call("getParams", json!({}))
    }

    fn parse_header(v: &Value) -> Result<ChainHeader> {
        let hex_h = v["header"].as_str().ok_or_else(|| anyhow!("no header"))?;
        let bytes: [u8; 256] = hex::decode(hex_h)?.try_into().map_err(|_| anyhow!("header not 256 bytes"))?;
        let proof = match v.get("proof").and_then(|p| p.as_str()) {
            Some(p) => b64d(p)?,
            None => vec![],
        };
        let hash = pv::blake3::hash(&bytes);
        let height = u64::from_le_bytes(bytes[4..12].try_into().unwrap());
        Ok(ChainHeader { bytes, hash, height, proof, cum_work: v.get("cumulative_work").and_then(|c| c.as_str()).map(String::from) })
    }

    /// Canonical header at `height` with its proof, or None past the tip.
    pub fn header_at(&self, height: u64) -> Result<Option<ChainHeader>> {
        let v = self.rpc.call("getHeader", json!({"height": height}))?;
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(Self::parse_header(&v)?))
    }

    pub fn header_by_hash(&self, hash: &[u8; 32]) -> Result<Option<ChainHeader>> {
        let v = self.rpc.call("getHeader", json!({"hash": hex::encode(hash)}))?;
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(Self::parse_header(&v)?))
    }

    /// Block txs (wire bytes) in block order.
    pub fn block_txs(&self, hash: &[u8; 32]) -> Result<Vec<Vec<u8>>> {
        Ok(self.block_body(hash)?.0)
    }

    /// Block txs and the coinbase receipt record (97 bytes; the last tx_root leaf, R1).
    /// `None` for the genesis block and for a pre-R1 node.
    pub fn block_body(&self, hash: &[u8; 32]) -> Result<(Vec<Vec<u8>>, Option<Vec<u8>>)> {
        let v = self.rpc.call("getBlock", json!({"hash": hex::encode(hash)}))?;
        let txs = v["txs"].as_array().cloned().unwrap_or_default();
        let txs = txs.iter().map(|t| b64d(t["tx"].as_str().context("tx b64")?)).collect::<Result<Vec<_>>>()?;
        let cb = match v["coinbase"]["record"].as_str() {
            Some(r) => Some(hex::decode(r)?),
            None => None,
        };
        Ok((txs, cb))
    }

    pub fn withdrawal_proof(&self, nonce: u64) -> Result<Value> {
        self.rpc.call("getWithdrawalProof", json!({"nonce": nonce}))
    }

    /// Historical proof against the withdraw_root of the node's final header at `at_height`.
    pub fn withdrawal_proof_at(&self, nonce: u64, at_height: u64) -> Result<Value> {
        self.rpc.call("getWithdrawalProof", json!({"nonce": nonce, "at_height": at_height}))
    }
}

/// chain/agent-types/src/merkle.rs: root and per-leaf paths (33-byte entries) over the
/// txs, then the coinbase receipt as the last leaf when given (R1: every block has one).
/// `paths[i]` is tx `i`'s path; `paths[txs.len()]` the receipt's.
pub fn tx_root_paths(txs: &[Vec<u8>], coinbase: Option<&[u8]>) -> ([u8; 32], Vec<Vec<u8>>) {
    let mut leaves: Vec<&[u8]> = txs.iter().map(|t| t.as_slice()).collect();
    if let Some(c) = coinbase {
        leaves.push(c);
    }
    if leaves.is_empty() {
        return ([0; 32], vec![]);
    }
    let leaf = |h: &[u8; 32]| pv::blake3::hash_parts(&[&[0u8], h]);
    let node = |l: &[u8; 32], r: &[u8; 32]| pv::blake3::hash_parts(&[&[1u8], l, r]);
    let mut level: Vec<[u8; 32]> = leaves.iter().map(|t| leaf(&pv::blake3::hash(t))).collect();
    let mut paths = vec![Vec::new(); leaves.len()];
    let mut idx: Vec<usize> = (0..leaves.len()).collect();
    while level.len() > 1 {
        for (t, i) in idx.iter_mut().enumerate() {
            let sib = *i ^ 1;
            if sib < level.len() {
                paths[t].extend_from_slice(&level[sib]);
                paths[t].push((sib < *i) as u8);
            }
            *i /= 2;
        }
        level = level.chunks(2).map(|c| if c.len() == 2 { node(&c[0], &c[1]) } else { c[0] }).collect();
    }
    (level[0], paths)
}

/// Jobs created by txs of a block: `(job_id, tx index)`.
pub fn jobs_in(txs: &[Vec<u8>]) -> Vec<([u8; 32], usize)> {
    let mut out = vec![];
    for (i, t) in txs.iter().enumerate() {
        let Ok(v) = pv::chaintx::TxView::any_chain(t) else { continue };
        if v.kind() == pv::chaintx::KIND_POST_SEGMENTS {
            // standing jobs (SPEC 4.5): one job per segment, all from this tx
            let mut js = vec![];
            if pv::chaintx::for_each_segment(&v, |j| js.push(j.job_id)).is_ok() {
                out.extend(js.into_iter().map(|j| (j, i)));
            }
            continue;
        }
        if v.kind() != pv::chaintx::KIND_POST_JOB && v.kind() != pv::chaintx::KIND_BRIDGE_UNLOCK {
            continue;
        }
        if let Ok(j) = pv::chaintx::job_from_tx(&v, sha256) {
            out.push((j.job_id, i));
        }
    }
    out
}

pub fn num(v: &Value) -> Result<u64> {
    pearl_node::num(v)
}

/// lightclient::Params from node getParams + getChainInfo.
pub fn lc_params(node: &Node, relay_reward: u64, retain_final: u64) -> Result<lightclient::Params> {
    let p = node.params()?;
    let info = node.info()?;
    let cfg: [u8; 52] = hex::decode(p["fallback_config"].as_str().context("fallback_config")?)?.try_into().map_err(|_| anyhow!("config len"))?;
    Ok(lightclient::Params {
        chain_id: num(&info["chain_id"])? as u32,
        finality_k: num(&p["finality_k"])? as u32,
        target_block_time_ms: num(&p["target_block_time_ms"])?,
        half_life_ms: num(&p["wtema_half_life_ms"])?,
        pow_limit_bits: num(&p["pow_limit_bits"])? as u32,
        min_difficulty_bits: num(&p["min_difficulty_bits"])? as u32,
        max_future_ms: num(&p["max_future_ms"])?,
        fallback_m: num(&p["fallback_m"])? as u32,
        fallback_n: num(&p["fallback_n"])? as u32,
        fallback_config: cfg,
        relay_reward,
        retain_final,
    })
}

/// U256 hex string (node encoding) to little-endian bytes.
pub fn u256_hex_le(s: &str) -> Result<[u8; 32]> {
    let s = s.trim_start_matches("0x");
    let v = pv::U256::from_str_radix(s, 16).map_err(|e| anyhow!("u256 hex: {e:?}"))?;
    Ok(pv::difficulty::u256_le(&v))
}

/// The agent-chain side the relay pipeline needs (Node over RPC; a vector of mined
/// headers in tests/catchup.rs).
pub trait Src {
    /// (height, hash) of the canonical tip.
    fn tip(&self) -> Result<(u64, [u8; 32])>;
    fn header_at(&self, height: u64) -> Result<Option<ChainHeader>>;
    /// Block txs and the coinbase receipt (see `Node::block_body`).
    fn block_body(&self, hash: &[u8; 32]) -> Result<(Vec<Vec<u8>>, Option<Vec<u8>>)>;
}

impl Src for Node {
    fn tip(&self) -> Result<(u64, [u8; 32])> {
        let info = self.info()?;
        let h = num(&info["height"])?;
        let tip: [u8; 32] = hex::decode(info["tip"].as_str().context("tip")?)?.try_into().map_err(|_| anyhow!("tip hash"))?;
        Ok((h, tip))
    }
    fn header_at(&self, height: u64) -> Result<Option<ChainHeader>> {
        Node::header_at(self, height)
    }
    fn block_body(&self, hash: &[u8; 32]) -> Result<(Vec<Vec<u8>>, Option<Vec<u8>>)> {
        Node::block_body(self, hash)
    }
}
