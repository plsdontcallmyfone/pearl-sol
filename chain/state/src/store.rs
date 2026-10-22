//! QMDB storage for the chain state.
//!
//! - `current` (unordered, variable values) holds every record in [`crate::schema`];
//!   its canonical root is the header's `state_root`.
//! - `keyless` (variable values) is the append-only withdrawal log; its root is the
//!   header's `withdraw_root`.
//!
//! Non-final blocks live as speculative [`Snapshot`]s: an `Arc` to a merkleized batch of
//! each database, chained from the parent block's snapshot (one chain per branch, so
//! shallow reorgs are free). When a block becomes final, [`Store::finalize`] applies its
//! batches (which include every unapplied ancestor) and commits both databases; sibling
//! branches become stale and the caller drops them.
//!
//! Keep-alive: a Commonware merkleized batch holds only a `Weak` reference to its parent
//! batch, and node / value lookups walk that chain. If an UNCOMMITTED ancestor batch is
//! dropped while a descendant lives, the descendant silently loses that ancestor's nodes
//! and the next build or read fails ("left child missing" panic in merkle/batch.rs, or
//! "journal error: item out of range" on a value read). The chain keeps every non-final
//! block's snapshot in `live`, so it never hit this, but a caller that holds only the
//! tip snapshot (a test, a fast-sync or replay loop) did. Each [`Snapshot`] therefore
//! keeps strong references to its uncommitted ancestors (`keep`, at most the blocks
//! above the committed height, so about K entries), pruned as finality advances.
//!
//! Read cache (node-local, 2026-10-22, devnet box 0 CPU): a read of a key that no
//! uncommitted block on the snapshot's branch wrote resolves to the committed database,
//! which is one pread of the value journal per call. Every [`Snapshot`] (and every
//! ancestor in `keep`) records the keys its block wrote, so [`Store::get`] can tell such a
//! read apart and answer it from [`ReadCache`], a bounded map of committed values (absent
//! keys included). [`Store::finalize`] drops the cached entries of every key the newly
//! committed blocks wrote, and the whole cache on any error, so a cached value is always
//! the committed one. Values over [`CACHE_MAX_VALUE`] are not cached.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use commonware_codec::{Encode, RangeCfg};
use commonware_cryptography::{blake3::Digest as B3, sha256::Digest as ShaDigest, Sha256};
use commonware_parallel::Sequential;
use commonware_runtime::buffer::paged::CacheRef;
use commonware_runtime::Spawner;
use commonware_storage::journal::contiguous::variable::Config as JournalConfig;
use commonware_storage::merkle::full::Config as MerkleConfig;
use commonware_storage::merkle::Location;
use commonware_storage::mmr;
use commonware_storage::qmdb::any::unordered::variable::Update;
use commonware_storage::qmdb::any::value::VariableEncoding;
use commonware_storage::qmdb::{current, keyless};
use commonware_storage::translator::EightCap;

use crate::schema::Key;
use crate::{StateError, StateRead};

pub type Family = mmr::Family;
pub type StateDb<E> = current::unordered::variable::Db<Family, E, B3, Vec<u8>, Sha256, EightCap, 32, Sequential>;
pub type LogDb<E> = keyless::variable::Db<Family, E, Vec<u8>, Sha256, Sequential>;
pub type StateBatch = current::batch::MerkleizedBatch<Family, ShaDigest, Update<B3, Vec<u8>>, 32, Sequential>;
pub type LogBatch = keyless::batch::MerkleizedBatch<Family, ShaDigest, VariableEncoding<Vec<u8>>, Sequential>;

/// Largest record value accepted by the codec.
pub const MAX_VALUE: usize = 1 << 20;

/// Post-state of one block.
#[derive(Clone)]
pub struct Snapshot {
    pub state: Arc<StateBatch>,
    pub log: Arc<LogBatch>,
    /// Number of withdrawal-log operations (appends + commits) in this snapshot.
    pub log_size: u64,
    pub state_root: [u8; 32],
    pub withdraw_root: [u8; 32],
    /// Block height of this snapshot (the committed height for a committed snapshot).
    pub height: u64,
    /// Strong references to the uncommitted ancestors' batches (see the module doc).
    keep: Arc<Vec<Ancestor>>,
    /// Keys this block wrote (empty for a committed snapshot opened from disk).
    written: Arc<HashSet<Key>>,
}

impl Snapshot {
    /// Keys this snapshot's own block wrote.
    pub fn written(&self) -> &HashSet<Key> {
        &self.written
    }

    /// True when `key` was written by this block or by an ancestor it keeps (an
    /// uncommitted block on its branch): such a read must go through the batch chain.
    fn overlays(&self, key: &Key) -> bool {
        self.written.contains(key) || self.keep.iter().any(|a| a.written.contains(key))
    }
}

/// An ancestor's batches, kept alive by its descendants until it is committed.
struct Ancestor {
    height: u64,
    _state: Arc<StateBatch>,
    _log: Arc<LogBatch>,
    written: Arc<HashSet<Key>>,
}

/// Largest value the read cache keeps.
pub const CACHE_MAX_VALUE: usize = 64 << 10;
/// Default byte budget of the read cache (two generations of half each).
pub const DEFAULT_CACHE_BYTES: usize = 64 << 20;

/// Bounded cache of COMMITTED values (module doc, read cache): two generations, new
/// entries go to `hot`; when `hot` passes half the budget it becomes `cold` (the old `cold`
/// is dropped), and a `cold` hit moves back to `hot`. Counters for getNodeStats.
pub struct ReadCache {
    inner: Mutex<CacheInner>,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub bypass: AtomicU64,
}

struct CacheInner {
    hot: HashMap<Key, Option<Vec<u8>>>,
    cold: HashMap<Key, Option<Vec<u8>>>,
    hot_bytes: usize,
    cold_bytes: usize,
    budget: usize,
}

fn entry_bytes(v: &Option<Vec<u8>>) -> usize {
    64 + v.as_ref().map_or(0, |b| b.len())
}

impl ReadCache {
    pub fn new(budget: usize) -> Self {
        Self {
            inner: Mutex::new(CacheInner { hot: HashMap::new(), cold: HashMap::new(), hot_bytes: 0, cold_bytes: 0, budget }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            bypass: AtomicU64::new(0),
        }
    }

    fn get(&self, key: &Key) -> Option<Option<Vec<u8>>> {
        let mut c = self.inner.lock().unwrap();
        if let Some(v) = c.hot.get(key) {
            return Some(v.clone());
        }
        let v = c.cold.remove(key)?;
        c.cold_bytes -= entry_bytes(&v);
        c.insert(*key, v.clone());
        Some(v)
    }

    fn put(&self, key: Key, v: Option<Vec<u8>>) {
        if v.as_ref().is_some_and(|b| b.len() > CACHE_MAX_VALUE) {
            return;
        }
        self.inner.lock().unwrap().insert(key, v);
    }

    fn invalidate<'a>(&self, keys: impl Iterator<Item = &'a Key>) {
        let mut c = self.inner.lock().unwrap();
        for k in keys {
            if let Some(v) = c.hot.remove(k) {
                c.hot_bytes -= entry_bytes(&v);
            }
            if let Some(v) = c.cold.remove(k) {
                c.cold_bytes -= entry_bytes(&v);
            }
        }
    }

    pub fn clear(&self) {
        let mut c = self.inner.lock().unwrap();
        c.hot.clear();
        c.cold.clear();
        c.hot_bytes = 0;
        c.cold_bytes = 0;
    }

    /// `(entries, bytes, budget)`.
    pub fn size(&self) -> (usize, usize, usize) {
        let c = self.inner.lock().unwrap();
        (c.hot.len() + c.cold.len(), c.hot_bytes + c.cold_bytes, c.budget)
    }
}

impl CacheInner {
    fn insert(&mut self, key: Key, v: Option<Vec<u8>>) {
        if self.budget == 0 {
            return;
        }
        let b = entry_bytes(&v);
        if let Some(old) = self.hot.insert(key, v) {
            self.hot_bytes -= entry_bytes(&old);
        }
        self.hot_bytes += b;
        if self.hot_bytes > self.budget / 2 {
            self.cold = std::mem::take(&mut self.hot);
            self.cold_bytes = std::mem::replace(&mut self.hot_bytes, 0);
        }
    }
}

/// Output of executing a block, ready to merkleize.
#[derive(Debug, Clone, Default)]
pub struct WriteSet {
    pub writes: BTreeMap<Key, Option<Vec<u8>>>,
    /// Withdrawal log entries in order.
    pub withdrawals: Vec<Vec<u8>>,
}

pub struct Store<E: commonware_storage::Context + Spawner> {
    state: Option<StateDb<E>>,
    log: Option<LogDb<E>>,
    /// Snapshot of the committed (final) state.
    committed: Snapshot,
    /// Committed values by key (module doc, read cache).
    cache: ReadCache,
}

fn d32(d: &ShaDigest) -> [u8; 32] {
    d.encode().as_ref().try_into().unwrap()
}

const PAGE_SIZE: NonZeroU16 = NonZeroU16::new(4096).unwrap();
const PAGE_CACHE: NonZeroUsize = NonZeroUsize::new(256).unwrap();

impl<E: commonware_storage::Context + Spawner> Store<E> {
    /// Open (or create) both databases under partitions prefixed by `prefix`.
    pub async fn open(context: E, prefix: &str) -> Result<Self, StateError> {
        let page_cache = CacheRef::from_pooler(&context, PAGE_SIZE, PAGE_CACHE);
        let merkle = |name: &str| MerkleConfig {
            journal_partition: format!("{prefix}-{name}-mmr-journal"),
            metadata_partition: format!("{prefix}-{name}-mmr-metadata"),
            items_per_blob: NonZeroU64::new(4096).unwrap(),
            write_buffer: NonZeroUsize::new(1 << 16).unwrap(),
            replay_buffer: NonZeroUsize::new(1 << 16).unwrap(),
            strategy: Sequential,
            page_cache: page_cache.clone(),
        };
        fn journal<C>(prefix: &str, name: &str, codec_config: C, page_cache: &CacheRef) -> JournalConfig<C> {
            JournalConfig {
                partition: format!("{prefix}-{name}-log"),
                items_per_section: NonZeroU64::new(4096).unwrap(),
                compression: None,
                codec_config,
                page_cache: page_cache.clone(),
                write_buffer: NonZeroUsize::new(1 << 16).unwrap(),
                replay_buffer: NonZeroUsize::new(1 << 16).unwrap(),
            }
        }
        let state_cfg = current::VariableConfig {
            merkle_config: merkle("state"),
            journal_config: journal(prefix, "state", ((), (RangeCfg::from(0..=MAX_VALUE), ())), &page_cache),
            grafted_metadata_partition: format!("{prefix}-state-grafted"),
            translator: EightCap,
            init_cache_size: Some(NonZeroUsize::new(1024).unwrap()),
            init_buffer: NonZeroUsize::new(1 << 20).unwrap(),
            init_concurrency: (),
        };
        let state = StateDb::init(context.child("state"), state_cfg).await.map_err(StateError::db)?;
        let log_cfg = keyless::variable::Config {
            merkle: merkle("withdrawals"),
            log: journal(prefix, "withdrawals", (RangeCfg::from(0..=4096usize), ()), &page_cache),
        };
        let log = LogDb::init(context.child("withdrawals"), log_cfg).await.map_err(StateError::db)?;
        let meta = state.get_metadata().await.map_err(StateError::db)?;
        let height = meta.and_then(|v| v.get(..8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))).unwrap_or(0);
        let committed = Snapshot {
            state: state.to_batch(),
            log: log.to_batch(),
            log_size: *log.bounds().end,
            state_root: d32(&state.root()),
            withdraw_root: d32(&log.root()),
            height,
            keep: Arc::new(Vec::new()),
            written: Arc::new(HashSet::new()),
        };
        Ok(Self { state: Some(state), log: Some(log), committed, cache: ReadCache::new(DEFAULT_CACHE_BYTES) })
    }

    /// Set the read cache's byte budget (0 turns it off). Drops what it holds.
    pub fn set_cache_bytes(&mut self, budget: usize) {
        self.cache = ReadCache::new(budget);
    }

    /// The read cache (stats).
    pub fn cache(&self) -> &ReadCache {
        &self.cache
    }

    fn sdb(&self) -> &StateDb<E> {
        self.state.as_ref().expect("state db present")
    }

    fn ldb(&self) -> &LogDb<E> {
        self.log.as_ref().expect("log db present")
    }

    /// Snapshot of the committed (final) state.
    pub fn committed(&self) -> &Snapshot {
        &self.committed
    }

    /// Height recorded with the last commit (`None` for a fresh database).
    pub async fn committed_height(&self) -> Result<Option<u64>, StateError> {
        let m = self.sdb().get_metadata().await.map_err(StateError::db)?;
        Ok(m.and_then(|v| v.get(..8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))))
    }

    /// Read `key` as of `snap`. A key no uncommitted block on the snapshot's branch wrote
    /// is read from the committed database through the read cache (module doc).
    pub async fn get(&self, snap: &Snapshot, key: &Key) -> Result<Option<Vec<u8>>, StateError> {
        let is_committed = Arc::ptr_eq(&snap.state, &self.committed.state);
        // a snapshot at or below the committed height that is not the committed one is a
        // stale branch: read it as before, through its own batch chain
        if !is_committed && (snap.height <= self.committed.height || snap.overlays(key)) {
            self.cache.bypass.fetch_add(1, Ordering::Relaxed);
            return snap.state.get(&B3(*key), self.sdb()).await.map_err(StateError::db);
        }
        if let Some(v) = self.cache.get(key) {
            self.cache.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(v);
        }
        self.cache.misses.fetch_add(1, Ordering::Relaxed);
        let v = self.sdb().get(&B3(*key)).await.map_err(StateError::db)?;
        self.cache.put(*key, v.clone());
        Ok(v)
    }

    /// Merkleize `ws` on top of `parent`, giving the child block's snapshot.
    pub async fn build(&self, parent: &Snapshot, ws: &WriteSet, height: u64) -> Result<Snapshot, StateError> {
        let mut b = parent.state.new_batch::<Sha256>();
        for (k, v) in &ws.writes {
            b = b.write(B3(*k), v.clone());
        }
        let meta = Some(height.to_le_bytes().to_vec());
        let state = b.merkleize(self.sdb(), meta).await.map_err(StateError::db)?;
        let (log, log_size) = if ws.withdrawals.is_empty() {
            (parent.log.clone(), parent.log_size)
        } else {
            let mut lb = parent.log.new_batch::<Sha256>();
            for w in &ws.withdrawals {
                lb = lb.append(w.clone());
            }
            let size = parent.log_size + ws.withdrawals.len() as u64 + 1;
            let floor = self.ldb().inactivity_floor_loc();
            (lb.merkleize(self.ldb(), None, floor).await, size)
        };
        // Keep the parent and its uncommitted ancestors alive (module doc, keep-alive).
        let floor = self.committed.height;
        let parent_committed = Arc::ptr_eq(&parent.state, &self.committed.state);
        let mut keep: Vec<Ancestor> = Vec::with_capacity(parent.keep.len() + 1);
        for a in parent.keep.iter().filter(|a| a.height > floor) {
            keep.push(Ancestor { height: a.height, _state: a._state.clone(), _log: a._log.clone(), written: a.written.clone() });
        }
        if !parent_committed && parent.height > floor {
            keep.push(Ancestor { height: parent.height, _state: parent.state.clone(), _log: parent.log.clone(), written: parent.written.clone() });
        }
        Ok(Snapshot {
            state_root: d32(&state.root()),
            withdraw_root: d32(&log.root()),
            state,
            log,
            log_size,
            height,
            keep: Arc::new(keep),
            written: Arc::new(ws.writes.keys().copied().collect()),
        })
    }

    /// Make `snap` (a descendant of the committed snapshot) the committed state.
    pub async fn finalize(&mut self, snap: &Snapshot) -> Result<(), StateError> {
        // every key the newly committed blocks wrote is in `snap` or an ancestor it keeps
        // (a superset: kept ancestors already committed are dropped again, harmlessly)
        self.cache.invalidate(snap.written.iter().chain(snap.keep.iter().flat_map(|a| a.written.iter())));
        let r = self.finalize_inner(snap).await;
        if r.is_err() {
            self.cache.clear();
        }
        r
    }

    async fn finalize_inner(&mut self, snap: &Snapshot) -> Result<(), StateError> {
        let state = self.state.take().expect("state db present");
        let (state, _) = state.apply_batch(snap.state.clone()).await.map_err(StateError::db)?;
        let state = state.commit().await.map_err(StateError::db)?;
        self.state = Some(state);
        if !Arc::ptr_eq(&snap.log, &self.committed.log) && snap.log_size != self.committed.log_size {
            let log = self.log.take().expect("log db present");
            let (log, _) = log.apply_batch(snap.log.clone()).await.map_err(StateError::db)?;
            let log = log.commit().await.map_err(StateError::db)?;
            self.log = Some(log);
        }
        let mut c = snap.clone();
        c.keep = Arc::new(Vec::new());
        c.written = Arc::new(HashSet::new());
        self.committed = c;
        Ok(())
    }

    /// Committed withdrawal-log entry at `loc` (`None` for commit markers).
    pub async fn log_get(&self, loc: u64) -> Result<Option<Vec<u8>>, StateError> {
        self.ldb().get(Location::new(loc)).await.map_err(StateError::db)
    }

    /// Committed log size (operations, including commit markers).
    pub fn log_size(&self) -> u64 {
        *self.ldb().bounds().end
    }

    /// Inclusion proof for the committed log entry at `loc`, against the committed
    /// `withdraw_root`. Returns the codec-encoded `merkle::Proof` and the root.
    pub async fn withdrawal_proof(&self, loc: u64) -> Result<(Vec<u8>, [u8; 32]), StateError> {
        let (proof, _ops) = self
            .ldb()
            .proof(Location::new(loc), NonZeroU64::new(1).unwrap())
            .await
            .map_err(StateError::db)?;
        Ok((proof.encode().to_vec(), d32(&self.ldb().root())))
    }

    /// Historical inclusion proof for the committed log entry at `loc`, against the
    /// withdraw_root the log had when it held `op_count` operations (a commit boundary:
    /// the `log_size` of some final block's snapshot). Codec-encoded `merkle::Proof`.
    /// Verifies against that block's header `withdraw_root`.
    pub async fn withdrawal_proof_at(&self, loc: u64, op_count: u64) -> Result<Vec<u8>, StateError> {
        let (proof, _ops) = self
            .ldb()
            .historical_proof(Location::new(op_count), Location::new(loc), NonZeroU64::new(1).unwrap())
            .await
            .map_err(StateError::db)?;
        Ok(proof.encode().to_vec())
    }

    /// Check a [`Store::state_proof`] (codec-encoded) that `key` currently holds `value`
    /// under `root` (a header's `state_root`). Used for contract storage proofs
    /// (docs/CONTRACTS.md "Proofs").
    pub fn verify_state_proof(key: &Key, value: &[u8], proof: &[u8], root: &[u8; 32]) -> bool {
        use commonware_codec::{DecodeExt, Read as _};
        let mut buf = proof;
        let Ok(p) = current::unordered::variable::KeyValueProof::<Family, ShaDigest, 32>::read_cfg(&mut buf, &4096usize) else { return false };
        if !buf.is_empty() {
            return false;
        }
        let Ok(r) = ShaDigest::decode(&root[..]) else { return false };
        StateDb::<E>::verify_key_value_proof(B3(*key), value.to_vec(), &p, &r)
    }

    /// Codec-encoded current-value proof for `key` against the committed `state_root`.
    pub async fn state_proof(&self, key: &Key) -> Result<Vec<u8>, StateError> {
        let p = self.sdb().key_value_proof(B3(*key)).await.map_err(StateError::db)?;
        Ok(p.encode().to_vec())
    }
}

/// Read view of one snapshot.
pub struct View<'a, E: commonware_storage::Context + Spawner> {
    pub store: &'a Store<E>,
    pub snap: &'a Snapshot,
}

impl<E: commonware_storage::Context + Spawner> StateRead for View<'_, E> {
    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>, StateError> {
        self.store.get(self.snap, key).await
    }
}
