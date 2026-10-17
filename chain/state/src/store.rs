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

use std::collections::BTreeMap;
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
use std::sync::Arc;

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
}

/// An ancestor's batches, kept alive by its descendants until it is committed.
struct Ancestor {
    height: u64,
    _state: Arc<StateBatch>,
    _log: Arc<LogBatch>,
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
        };
        Ok(Self { state: Some(state), log: Some(log), committed })
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

    /// Read `key` as of `snap`.
    pub async fn get(&self, snap: &Snapshot, key: &Key) -> Result<Option<Vec<u8>>, StateError> {
        snap.state.get(&B3(*key), self.sdb()).await.map_err(StateError::db)
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
            keep.push(Ancestor { height: a.height, _state: a._state.clone(), _log: a._log.clone() });
        }
        if !parent_committed && parent.height > floor {
            keep.push(Ancestor { height: parent.height, _state: parent.state.clone(), _log: parent.log.clone() });
        }
        Ok(Snapshot {
            state_root: d32(&state.root()),
            withdraw_root: d32(&log.root()),
            state,
            log,
            log_size,
            height,
            keep: Arc::new(keep),
        })
    }

    /// Make `snap` (a descendant of the committed snapshot) the committed state.
    pub async fn finalize(&mut self, snap: &Snapshot) -> Result<(), StateError> {
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
