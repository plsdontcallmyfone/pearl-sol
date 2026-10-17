//! Content-addressed blob store (M2 data availability, docs/RPC.md "Blobs").
//!
//! - A blob is any byte string up to `max_blob_bytes`; its id is `blake3(bytes)` and its
//!   URI `blob:<64 hex>`. Job data (A and B) and results (C) are stored this way and
//!   referenced from `PostJob.data_uri` / `SubmitResult.uri`.
//! - Blobs live in one runtime storage partition, one runtime blob per id (works on the
//!   tokio runtime and on the deterministic runtime in tests). Every read re-checks the
//!   hash, so a corrupted file is reported as missing instead of served.
//! - Blobs are NOT consensus state. The executor never reads them; a node only uses them
//!   to decide which jobs its block templates offer, and to serve miners, attesters and
//!   peers. Which blobs to keep is decided by the caller with [`keep`] (job references
//!   plus a grace period); [`BlobStore::prune`] removes the rest.
//! - Abuse limits: a size cap per blob, a cap on total bytes, and token buckets for RPC
//!   puts (per node) and for blobs pushed by each peer.

use std::collections::{BTreeMap, HashMap};

use commonware_runtime::{Blob as _, ReadOptions, Storage, WriteOptions};
use tracing::{debug, warn};

pub type BlobId = [u8; 32];

pub const URI_PREFIX: &str = "blob:";

/// Hard ceiling for `max_blob_bytes`: a blob travels in one p2p message (8 MiB limit)
/// with its framing, so keep a margin.
pub const MAX_BLOB_BYTES_LIMIT: usize = 7 << 20;

/// `blake3(bytes)`.
pub fn blob_id(data: &[u8]) -> BlobId {
    *blake3::hash(data).as_bytes()
}

/// `blob:<64 lowercase hex>`.
pub fn blob_uri(id: &BlobId) -> String {
    format!("{URI_PREFIX}{}", hex::encode(id))
}

/// Does `uri` use the blob scheme (well formed or not)?
pub fn is_blob_uri(uri: &[u8]) -> bool {
    uri.starts_with(URI_PREFIX.as_bytes())
}

/// The id in a well-formed `blob:<64 hex>` URI (hex in either case).
pub fn parse_uri(uri: &[u8]) -> Option<BlobId> {
    let rest = uri.strip_prefix(URI_PREFIX.as_bytes())?;
    if rest.len() != 64 || !rest.iter().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    hex::decode(rest).ok()?.try_into().ok()
}

/// A blob id from `blob:<hex>`, bare hex, or `0x` hex.
pub fn parse_id(s: &str) -> Option<BlobId> {
    let t = s.trim();
    if let Some(id) = parse_uri(t.as_bytes()) {
        return Some(id);
    }
    let t = t.trim_start_matches("0x");
    if t.len() != 64 {
        return None;
    }
    hex::decode(t).ok()?.try_into().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobConfig {
    /// Largest blob accepted (RPC or p2p). Must stay below the p2p message limit (8 MiB).
    pub max_blob_bytes: usize,
    /// Total bytes this node stores; puts beyond it are refused until pruning frees room.
    pub max_total_bytes: u64,
    /// Blocks a blob is kept after its last job reference ends (or after it was stored,
    /// if no job references it).
    pub grace_blocks: u64,
    /// putBlob token bucket (bytes per second, burst bytes), per node.
    pub put_rate_bytes_per_s: u64,
    pub put_burst_bytes: u64,
    /// Token bucket for blobs a single peer pushes to us.
    pub peer_rate_bytes_per_s: u64,
    pub peer_burst_bytes: u64,
}

impl Default for BlobConfig {
    fn default() -> Self {
        Self {
            max_blob_bytes: 4 << 20,
            max_total_bytes: 1 << 30,
            grace_blocks: 100,
            put_rate_bytes_per_s: 1 << 20,
            put_burst_bytes: 16 << 20,
            peer_rate_bytes_per_s: 1 << 20,
            peer_burst_bytes: 16 << 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub size: u64,
    /// Tip height when this node stored it.
    pub stored_height: u64,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PutError {
    #[error("empty blob")]
    Empty,
    #[error("blob is {size} bytes, max_blob_bytes is {max}")]
    TooLarge { size: usize, max: usize },
    #[error("blob store full ({total} of {max} bytes used)")]
    Full { total: u64, max: u64 },
    #[error("rate limited: try again later")]
    RateLimited,
    #[error("content does not match the blob id")]
    Mismatch,
    #[error("storage: {0}")]
    Storage(String),
}

/// Token bucket over bytes. Times are milliseconds of the node clock.
#[derive(Debug, Clone)]
pub struct Bucket {
    tokens: f64,
    last_ms: u64,
    rate: f64,
    burst: f64,
}

impl Bucket {
    pub fn new(rate_per_s: u64, burst: u64) -> Self {
        Self { tokens: burst as f64, last_ms: 0, rate: rate_per_s as f64, burst: burst as f64 }
    }

    /// Take `n` tokens if available.
    pub fn take(&mut self, n: u64, now_ms: u64) -> bool {
        if now_ms > self.last_ms {
            self.tokens = (self.tokens + (now_ms - self.last_ms) as f64 * self.rate / 1000.0).min(self.burst);
            self.last_ms = now_ms;
        }
        if self.tokens >= n as f64 {
            self.tokens -= n as f64;
            true
        } else {
            false
        }
    }
}

/// Retention rule. `last_ref` is the last height at which a job needs the blob (`None`:
/// no job references it; `Some(u64::MAX)`: a live job still needs it).
pub fn keep(meta: &Meta, last_ref: Option<u64>, tip: u64, grace: u64) -> bool {
    let by_ref = last_ref.is_some_and(|r| r.saturating_add(grace) >= tip);
    let by_age = meta.stored_height.saturating_add(grace) >= tip;
    by_ref || by_age
}

pub struct BlobStore<S: Storage> {
    storage: S,
    partition: String,
    cfg: BlobConfig,
    meta: BTreeMap<BlobId, Meta>,
    total: u64,
    rpc_bucket: Bucket,
    peer_buckets: HashMap<Vec<u8>, Bucket>,
}

fn name(id: &BlobId) -> Vec<u8> {
    hex::encode(id).into_bytes()
}

fn st(e: impl std::fmt::Display) -> PutError {
    PutError::Storage(e.to_string())
}

impl<S: Storage> BlobStore<S> {
    /// Open the store in `partition` (created on first write). Existing blobs are loaded
    /// as stored at `height` (they get a fresh grace period after a restart).
    pub async fn open(storage: S, partition: &str, cfg: BlobConfig, height: u64) -> Result<Self, PutError> {
        let mut s = Self {
            rpc_bucket: Bucket::new(cfg.put_rate_bytes_per_s, cfg.put_burst_bytes),
            storage,
            partition: partition.to_string(),
            cfg,
            meta: BTreeMap::new(),
            total: 0,
            peer_buckets: HashMap::new(),
        };
        let names = s.storage.scan(partition).await.unwrap_or_default();
        for n in names {
            let Some(id) = std::str::from_utf8(&n).ok().and_then(parse_id) else { continue };
            match s.storage.open(partition, &n).await {
                Ok((_, len)) if len > 0 => {
                    s.meta.insert(id, Meta { size: len, stored_height: height });
                    s.total += len;
                }
                Ok(_) => {
                    let _ = s.storage.remove(partition, Some(&n)).await;
                }
                Err(e) => warn!("blob {}: {e}", hex::encode(id)),
            }
        }
        debug!(count = s.meta.len(), bytes = s.total, "blob store open");
        Ok(s)
    }

    pub fn config(&self) -> &BlobConfig {
        &self.cfg
    }

    pub fn has(&self, id: &BlobId) -> Option<Meta> {
        self.meta.get(id).copied()
    }

    pub fn len(&self) -> usize {
        self.meta.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meta.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn entries(&self) -> impl Iterator<Item = (&BlobId, &Meta)> {
        self.meta.iter()
    }

    /// Size and capacity checks shared by every put path.
    pub fn check(&self, size: usize) -> Result<(), PutError> {
        if size == 0 {
            return Err(PutError::Empty);
        }
        if size > self.cfg.max_blob_bytes {
            return Err(PutError::TooLarge { size, max: self.cfg.max_blob_bytes });
        }
        if self.total + size as u64 > self.cfg.max_total_bytes {
            return Err(PutError::Full { total: self.total, max: self.cfg.max_total_bytes });
        }
        Ok(())
    }

    /// RPC put budget (per node).
    pub fn take_rpc(&mut self, bytes: usize, now_ms: u64) -> bool {
        self.rpc_bucket.take(bytes as u64, now_ms)
    }

    /// Push budget of one peer (keyed by its public key bytes).
    pub fn take_peer(&mut self, peer: &[u8], bytes: usize, now_ms: u64) -> bool {
        if self.peer_buckets.len() > 4096 {
            self.peer_buckets.clear();
        }
        let cfg = &self.cfg;
        self.peer_buckets
            .entry(peer.to_vec())
            .or_insert_with(|| Bucket::new(cfg.peer_rate_bytes_per_s, cfg.peer_burst_bytes))
            .take(bytes as u64, now_ms)
    }

    /// Store `data`. Returns its id and whether it was new.
    pub async fn put(&mut self, data: &[u8], height: u64) -> Result<(BlobId, bool), PutError> {
        let id = blob_id(data);
        if self.meta.contains_key(&id) {
            return Ok((id, false));
        }
        self.check(data.len())?;
        let (blob, _) = self.storage.open(&self.partition, &name(&id)).await.map_err(st)?;
        blob.resize(0).await.map_err(st)?;
        blob.write_at(0, data.to_vec(), WriteOptions::default()).await.map_err(st)?;
        blob.sync().await.map_err(st)?;
        self.meta.insert(id, Meta { size: data.len() as u64, stored_height: height });
        self.total += data.len() as u64;
        Ok((id, true))
    }

    /// Store bytes that claim to be blob `id` (p2p delivery): the hash must match.
    pub async fn put_expected(&mut self, id: &BlobId, data: &[u8], height: u64) -> Result<bool, PutError> {
        if blob_id(data) != *id {
            return Err(PutError::Mismatch);
        }
        self.put(data, height).await.map(|(_, new)| new)
    }

    /// Read a blob (hash re-checked; a corrupt file is dropped and reported missing).
    pub async fn get(&mut self, id: &BlobId) -> Result<Option<Vec<u8>>, PutError> {
        let Some(m) = self.meta.get(id).copied() else { return Ok(None) };
        let (blob, len) = self.storage.open(&self.partition, &name(id)).await.map_err(st)?;
        if len != m.size {
            warn!(blob = %hex::encode(id), "blob size changed on disk; dropping it");
            self.remove(id).await;
            return Ok(None);
        }
        let buf = blob.read_at(0, len as usize, ReadOptions::default()).await.map_err(st)?;
        let data = buf.coalesce().as_ref().to_vec();
        if blob_id(&data) != *id {
            warn!(blob = %hex::encode(id), "blob content does not match its id; dropping it");
            self.remove(id).await;
            return Ok(None);
        }
        Ok(Some(data))
    }

    async fn remove(&mut self, id: &BlobId) {
        if let Some(m) = self.meta.remove(id) {
            self.total -= m.size;
        }
        let _ = self.storage.remove(&self.partition, Some(&name(id))).await;
    }

    /// Remove every blob for which `keep_fn` is false. Returns the removed ids.
    pub async fn prune(&mut self, mut keep_fn: impl FnMut(&BlobId, &Meta) -> bool) -> Vec<BlobId> {
        let drop: Vec<BlobId> = self.meta.iter().filter(|(id, m)| !keep_fn(id, m)).map(|(id, _)| *id).collect();
        for id in &drop {
            self.remove(id).await;
        }
        drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_runtime::{deterministic, Runner as _, Supervisor as _};

    #[test]
    fn uris() {
        let id = blob_id(b"hello");
        let u = blob_uri(&id);
        assert!(u.starts_with("blob:") && u.len() == 69);
        assert_eq!(parse_uri(u.as_bytes()), Some(id));
        assert_eq!(parse_uri(u.to_uppercase().replace("BLOB:", "blob:").as_bytes()), Some(id));
        assert_eq!(parse_uri(b"blob:abc"), None);
        assert_eq!(parse_uri(b"file:///x"), None);
        assert!(is_blob_uri(b"blob:zz"));
        assert_eq!(parse_id(&hex::encode(id)), Some(id));
        assert_eq!(parse_id(&u), Some(id));
    }

    #[test]
    fn retention_rule() {
        let m = Meta { size: 1, stored_height: 10 };
        assert!(keep(&m, None, 110, 100));
        assert!(!keep(&m, None, 111, 100));
        assert!(keep(&m, Some(u64::MAX), 10_000, 100));
        assert!(keep(&m, Some(500), 600, 100));
        assert!(!keep(&m, Some(500), 601, 100));
    }

    #[test]
    fn bucket() {
        let mut b = Bucket::new(1000, 2000);
        assert!(b.take(2000, 0));
        assert!(!b.take(1, 0));
        assert!(b.take(1000, 1000));
        assert!(!b.take(1500, 1500));
    }

    #[test]
    fn put_get_prune_reopen() {
        deterministic::Runner::default().start(|ctx| async move {
            let cfg = BlobConfig { max_blob_bytes: 100, max_total_bytes: 250, ..Default::default() };
            let mut s = BlobStore::open(ctx.child("a"), "t-blobs", cfg.clone(), 0).await.unwrap();
            let (a, new) = s.put(&[1u8; 100], 1).await.unwrap();
            assert!(new);
            assert_eq!(s.put(&[1u8; 100], 2).await.unwrap(), (a, false));
            assert_eq!(s.put(&[0u8; 101], 1).await, Err(PutError::TooLarge { size: 101, max: 100 }));
            assert_eq!(s.put(&[], 1).await, Err(PutError::Empty));
            let (b, _) = s.put(&[2u8; 100], 1).await.unwrap();
            assert!(matches!(s.put(&[3u8; 100], 1).await, Err(PutError::Full { .. })));
            assert_eq!(s.put_expected(&a, &[9u8; 10], 1).await, Err(PutError::Mismatch));
            assert_eq!(s.get(&a).await.unwrap(), Some(vec![1u8; 100]));
            assert_eq!(s.get(&blob_id(b"x")).await.unwrap(), None);
            assert_eq!(s.total_bytes(), 200);
            let removed = s.prune(|id, _| *id == b).await;
            assert_eq!(removed, vec![a]);
            assert_eq!(s.get(&a).await.unwrap(), None);
            drop(s);
            let mut s = BlobStore::open(ctx.child("b"), "t-blobs", cfg, 7).await.unwrap();
            assert_eq!(s.len(), 1);
            assert_eq!(s.has(&b), Some(Meta { size: 100, stored_height: 7 }));
            assert_eq!(s.get(&b).await.unwrap(), Some(vec![2u8; 100]));
        });
    }
}
