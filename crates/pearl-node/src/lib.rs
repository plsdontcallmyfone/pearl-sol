//! Client side of the v2 agent-chain node (SPEC v2 section 6, `rpc` crate).
//!
//! - [`NodeApi`]: what the miner and attester need from a node. [`HttpNode`] speaks
//!   JSON-RPC 2.0 over HTTP POST; tests implement the trait in process.
//! - [`tx`]: SPEC section 2 transaction encoding and signing.
//! - [`JobInfo`] / [`Template`]: the node's job and block-template objects, parsed
//!   leniently (32-byte values as hex or base58, u64 as a number or a string).
//!
//! Wire shapes follow docs/RPC.md (written by the node agent); see `rpc` below.

pub mod failover;
pub mod mock;
pub mod rpc;
pub mod tx;

use anyhow::{anyhow, bail, ensure, Context, Result};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_job::job::{Job, JobHeader, MiningConfig};
use pearl_job::pow::{BlockHeader, FallbackParams, MinedBlock};
use pearl_job::spec::{parse_key32, JobSpec};

pub use failover::FailoverNode;
pub use rpc::{is_transport, HttpNode, TransportError};

pub const DEFAULT_RPC: &str = "http://127.0.0.1:10630";

/// A job as the node reports it (SPEC 4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    pub job_id: [u8; 32],
    /// First field of the Pearl job header: the job id for a PostJob, the poster for a
    /// storefront order (node `header_id`, docs/RPC.md).
    pub header_id: [u8; 32],
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
    /// "job", "ticket" or "segment" (SPEC 4.5; older nodes report none: "job").
    pub kind: String,
    /// For a segment: the registered matrix its B is.
    pub standing: Option<StandingRef>,
}

/// A segment's registered matrix (SPEC 4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandingRef {
    pub ticket_id: [u8; 32],
    pub model_id: [u8; 32],
    pub matrix: u16,
}

impl JobInfo {
    pub fn is_segment(&self) -> bool {
        self.standing.is_some()
    }

    pub fn job_header(&self) -> JobHeader {
        JobHeader {
            job_pubkey: self.header_id,
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
            job: hex::encode(self.header_id),
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
        // an inference ticket's config is the ticket form (SPEC 4.5), never mined: no mining config
        let ticket = get("kind").and_then(|k| k.as_str()) == Some("ticket");
        let config = match get("config") {
            Some(_) if ticket => MiningConfig::localnet_default(),
            Some(c) => MiningConfig::from_bytes(&hex::decode(as_str(c)?.trim_start_matches("0x")).context("config hex")?)?,
            None => MiningConfig::localnet_default(),
        };
        let job_id = key32(alt(o, &["job_id", "id", "job"]).ok_or_else(|| anyhow!("job_id missing"))?)?;
        let header_id = match (get("header_id"), get("job_header")) {
            (Some(h), _) => key32(h)?,
            (None, Some(h)) => {
                let b = hex::decode(as_str(h)?).context("job_header hex")?;
                ensure!(b.len() == 76, "job_header is {} bytes, not 76", b.len());
                b[..32].try_into().unwrap()
            }
            (None, None) => job_id,
        };
        Ok(Self {
            job_id,
            header_id,
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
            kind: get("kind").map(as_str).transpose()?.unwrap_or("job").to_string(),
            standing: match (get("kind").and_then(|k| k.as_str()), get("standing")) {
                (Some("segment"), Some(s)) => Some(StandingRef {
                    ticket_id: key32(s.get("ticket_id").ok_or_else(|| anyhow!("standing.ticket_id missing"))?)?,
                    model_id: key32(s.get("model_id").ok_or_else(|| anyhow!("standing.model_id missing"))?)?,
                    matrix: num(s.get("matrix").ok_or_else(|| anyhow!("standing.matrix missing"))?)? as u16,
                }),
                _ => None,
            },
        })
        .and_then(|j: Self| {
            // The node also reports job_key: catch any job-header rule mismatch here.
            if let Some(k) = get("job_key").filter(|_| !ticket) {
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
            "job_id": k(&self.job_id), "header_id": k(&self.header_id), "poster": bs58::encode(self.poster).into_string(), "terms_hash": k(&self.terms_hash),
            "created_height": self.created_height, "share_target": self.share_target,
            "config": hex::encode(self.config.to_bytes()), "m": self.m, "n": self.n,
            "hash_a": k(&self.hash_a), "hash_b": k(&self.hash_b), "data_uri": self.data_uri, "status": self.status.to_lowercase(),
            "deliverer": self.deliverer.map(|d| bs58::encode(d).into_string()),
            "result_hash": self.result_hash.map(|r| hex::encode(r)), "result_uri": self.result_uri,
            "evaluators": self.evaluators.iter().map(|e| bs58::encode(e).into_string()).collect::<Vec<_>>(),
            "threshold": self.threshold, "deadline_height": self.deadline_height,
            "job_key": hex::encode(self.to_job().job_key()), "job_header": hex::encode(self.job_header().to_bytes()),
            "kind": self.kind,
            "standing": self.standing.map(|s| json!({"ticket_id": k(&s.ticket_id), "model_id": k(&s.model_id), "matrix": s.matrix})),
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
    /// Weak target the node offers for this template (docs/SPEED.md 2.3), None when weak
    /// blocks are off or the node predates them.
    pub fn weak_target_bits(&self) -> Option<u32> {
        self.raw.get("weak_target_bits").and_then(|v| v.as_u64()).map(|b| b as u32)
    }

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
    /// `submitWeakBlock` (docs/SPEED.md 2.3): a proof that meets only the weak target.
    fn submit_weak_block(&self, _template: &Template, _block: &MinedBlock) -> Result<SubmitOutcome> {
        Ok(SubmitOutcome::Rejected { reason: "this node client does not send weak blocks".into() })
    }
    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>>;
    fn get_account(&self, account: &[u8; 32]) -> Result<Account>;
    /// Returns the tx hash (hex) the node reports.
    fn send_tx(&self, wire: &[u8]) -> Result<String>;
    /// getBlob: the blob's bytes, or None if this node does not hold it (yet: a node
    /// starts fetching a blob that a job references, so a later retry may succeed).
    fn get_blob(&self, _id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        bail!("this node API has no blob store")
    }
    /// putBlob: store bytes on the node; returns the `blob:<hex>` URI.
    fn put_blob(&self, _data: &[u8]) -> Result<String> {
        bail!("this node API has no blob store")
    }
    /// Any JSON-RPC method (getModel, getTicket, putBlob with options, ...).
    fn call(&self, method: &str, _params: Value) -> Result<Value> {
        bail!("this node API cannot call {method}")
    }
}

/// B^T (n x k) of a segment's registered matrix: from the model store, or rebuilt from the
/// node's blobs (manifest from getModel, then every chunk of the weight file) and cached in
/// the store. The caller checks it against hash_b (PreparedJob::new_bt does).
pub fn load_segment_bt(node: &dyn NodeApi, store: &pearl_job::standing::ModelStore, s: &StandingRef, k: u32, n: u32) -> Result<Vec<i8>> {
    if let Ok(bt) = store.load_bt(&s.model_id, s.matrix, k, n) {
        return Ok(bt);
    }
    let m = node.call("getModel", json!({"model_id": hex::encode(s.model_id)}))?;
    ensure!(!m.is_null(), "model {} is not registered", hex::encode(s.model_id));
    let uri = m["manifest_uri"].as_str().ok_or_else(|| anyhow!("getModel has no manifest_uri"))?;
    let manifest: pearl_job::standing::Manifest = serde_json::from_slice(&fetch_blob(node, uri)?).context("parse model manifest")?;
    let mx = manifest.matrices.iter().find(|x| x.index == s.matrix).ok_or_else(|| anyhow!("manifest has no matrix {}", s.matrix))?;
    let dir = store.dir(&s.model_id);
    for f in [&mx.weights, &mx.aux] {
        fetch_model_file(node, &manifest, f, &dir)?;
    }
    store.load_bt(&s.model_id, s.matrix, k, n)
}

/// How long [`fetch_model_file`] waits for chunks its node is fetching from peers.
pub const MODEL_FETCH_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Rebuild one file of a model from its chunks (blake3 of the whole file checked).
pub fn fetch_model_file(node: &dyn NodeApi, manifest: &pearl_job::standing::Manifest, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let f = manifest.files.get(name).ok_or_else(|| anyhow!("manifest lists no file {name}"))?;
    let path = dir.join(name);
    if let Ok(b) = std::fs::read(&path) {
        if b.len() as u64 == f.size && hex::encode(blake3::hash(&b).as_bytes()) == f.blake3 {
            return Ok(path);
        }
    }
    // Ask for every chunk before failing on a missing one: each getBlob of a pinned chunk the node
    // lacks starts its p2p fetch, so all of a file's chunks are fetched at once instead of one per
    // attempt (devnet 2026-10-20: one chunk per template made a fresh node take hours per model).
    // Then wait a little for the missing ones.
    let mut parts: Vec<Option<Vec<u8>>> = vec![None; f.blobs.len()];
    let t0 = std::time::Instant::now();
    loop {
        for (i, c) in f.blobs.iter().enumerate() {
            if parts[i].is_none() {
                let id = pearl_job::data::parse_blob_uri(c).ok_or_else(|| anyhow!("malformed blob URI {c:?}"))?;
                if let Some(b) = node.get_blob(&id)? {
                    ensure!(pearl_job::data::blob_id(&b) == id, "blob {c}: content does not match its id");
                    parts[i] = Some(b);
                }
            }
        }
        let missing = parts.iter().filter(|p| p.is_none()).count();
        if missing == 0 {
            break;
        }
        ensure!(t0.elapsed() < MODEL_FETCH_WAIT, "{name}: {missing} of {} chunks are not on this node yet (fetch started)", f.blobs.len());
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let mut data = Vec::with_capacity(f.size as usize);
    for p in parts.into_iter().flatten() {
        data.extend_from_slice(&p);
    }
    ensure!(data.len() as u64 == f.size, "{name}: chunks give {} bytes, manifest says {}", data.len(), f.size);
    ensure!(hex::encode(blake3::hash(&data).as_bytes()) == f.blake3, "{name}: content does not match the manifest's blake3");
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &data)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// A segment's A (m x k) from its `data_uri` activation blob.
pub fn load_segment_a(node: &dyn NodeApi, job: &JobInfo) -> Result<Vec<i8>> {
    let (m, k, a) = pearl_job::standing::decode_activation_blob(&fetch_blob(node, &job.data_uri)?)?;
    ensure!(m == job.m && k == job.config.common_dim, "activation blob is {m} x {k}, segment is {} x {}", job.m, job.config.common_dim);
    Ok(a)
}

/// Fetch a `blob:` URI's bytes from the node and check them against the id.
pub fn fetch_blob(node: &dyn NodeApi, uri: &str) -> Result<Vec<u8>> {
    let id = pearl_job::data::parse_blob_uri(uri).ok_or_else(|| anyhow!("malformed blob URI {uri:?}"))?;
    let bytes = node.get_blob(&id)?.ok_or_else(|| anyhow!("blob {uri} is not on this node yet (getBlob returned null)"))?;
    ensure!(pearl_job::data::blob_id(&bytes) == id, "blob {uri}: content does not match its id");
    Ok(bytes)
}

/// Job data from a `data_uri`: `blob:<hex>` through the node's getBlob (a PSD1 job blob),
/// anything else (file://, a path) from the local filesystem.
pub fn load_job_data(node: &dyn NodeApi, uri: &str) -> Result<pearl_job::data::JobData> {
    if uri.starts_with(pearl_job::data::BLOB_URI_PREFIX) {
        return pearl_job::data::decode_job_blob(&fetch_blob(node, uri)?);
    }
    pearl_job::data::load(uri)
}

/// Result C (m*n i32) from a `result_uri`: `blob:<hex>` via getBlob, or `file://` / a path.
pub fn load_result(node: &dyn NodeApi, uri: &str, len: usize) -> Result<Vec<i32>> {
    if uri.starts_with(pearl_job::data::BLOB_URI_PREFIX) {
        return pearl_job::data::decode_result_blob(&fetch_blob(node, uri)?, len);
    }
    let p = uri.strip_prefix("file://").unwrap_or(uri);
    ensure!(!p.contains("://"), "unsupported result URI scheme in {uri:?} (blob: or file://)");
    pearl_job::data::read_i32(std::path::Path::new(p), len)
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

    /// A node that holds a chunk only from the second getBlob of it on (the first one starts its
    /// p2p fetch), like a devnet node that pins a model it has not fetched yet.
    struct FetchingNode {
        blobs: std::collections::HashMap<[u8; 32], Vec<u8>>,
        asked: std::sync::Mutex<std::collections::HashMap<[u8; 32], u32>>,
    }

    impl NodeApi for FetchingNode {
        fn block_template(&self, _: &[u8; 32]) -> Result<Template> {
            unimplemented!()
        }
        fn submit_block(&self, _: &Template, _: &MinedBlock) -> Result<SubmitOutcome> {
            unimplemented!()
        }
        fn get_job(&self, _: &[u8; 32]) -> Result<Option<JobInfo>> {
            unimplemented!()
        }
        fn get_account(&self, _: &[u8; 32]) -> Result<Account> {
            unimplemented!()
        }
        fn send_tx(&self, _: &[u8]) -> Result<String> {
            unimplemented!()
        }
        fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
            let mut a = self.asked.lock().unwrap();
            let n = a.entry(*id).or_insert(0);
            *n += 1;
            Ok(if *n >= 2 { self.blobs.get(id).cloned() } else { None })
        }
    }

    #[test]
    fn model_file_asks_for_every_chunk_at_once() {
        let data: Vec<u8> = (0..(pearl_job::standing::MODEL_CHUNK_BYTES * 3 + 77) as u32).map(|i| (i * 7 % 251) as u8).collect();
        let chunks: Vec<Vec<u8>> = pearl_job::standing::chunks(&data).map(|c| c.to_vec()).collect();
        assert_eq!(chunks.len(), 4);
        let uris: Vec<String> = chunks.iter().map(|c| pearl_job::data::blob_uri(&pearl_job::data::blob_id(c))).collect();
        let manifest: pearl_job::standing::Manifest = serde_json::from_value(json!({
            "format": "cadre-model-v1", "name": "t", "recipe": {}, "arch": {}, "rank": 128, "rows_pattern": [], "cols_pattern": [], "matrices": [],
            "files": {"w/0.i8": {"size": data.len(), "blake3": hex::encode(blake3::hash(&data).as_bytes()), "blobs": uris}},
        }))
        .unwrap();
        let node = FetchingNode {
            blobs: chunks.iter().map(|c| (pearl_job::data::blob_id(c), c.clone())).collect(),
            asked: Default::default(),
        };
        let dir = std::env::temp_dir().join(format!("pearl-node-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = fetch_model_file(&node, &manifest, "w/0.i8", &dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        // every chunk was asked for in the first pass (each fetch started at once), then once more
        let asked = node.asked.lock().unwrap();
        assert_eq!(asked.len(), 4);
        assert!(asked.values().all(|n| *n == 2), "{asked:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
