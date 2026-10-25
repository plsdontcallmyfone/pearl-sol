//! Node API state: the chain, the mempool, block templates and subscribers.
//!
//! The node actor owns one [`Core`] and calls it for RPC requests, gossip and sync
//! deliveries. Side effects for the network are queued in [`Core::outbox`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use agent_types::{Block, Hash, Pubkey, Tx, TxBody};
use blobs::{BlobId, BlobStore, PutError};
use commonware_runtime::Spawner;
use consensus_pow::{Accept, Chain, ChainError, ChainEvent, JobChoice, PowVerifier, Template};
use mempool::Mempool;
use serde_json::{json, Value};
use state::schema::{self, Account};
use tracing::{debug, info, warn};

/// Something the actor must send to peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Block(Vec<u8>),
    Tx(Vec<u8>),
    /// Ask the sync layer for this header (headers-first backfill).
    FetchHeader(Hash),
    FetchBody(Hash),
    /// Push a blob (raw bytes) to every peer (one hop, only for blobs put over RPC).
    Blob(Vec<u8>),
    /// Fetch a blob by id through the blob resolver.
    FetchBlob(BlobId),
    /// Stop fetching a blob nobody needs any more.
    CancelBlob(BlobId),
    /// A weak block message (docs/SPEED.md 2.2), sent on the tips channel.
    Weak(Vec<u8>),
}

/// Live peer list for getPeers (open net); returns the JSON result.
pub type PeerInfoFn = Arc<dyn Fn() -> Value + Send + Sync>;

/// How often (in tip blocks) the blob store is pruned.
pub const BLOB_PRUNE_EVERY: u64 = 10;
/// Jobs scanned (newest first) for blob references when pruning.
pub const BLOB_SCAN_JOBS: u64 = 100_000;
/// At most this many blobs wanted at once (queued or in flight; at most
/// `blobfetch::BLOB_FETCH_WINDOW` of them in flight). The prune pass wants the rest later.
pub const MAX_WANTED_BLOBS: usize = 1024;
/// Unsolicited blob pushes stop once the store is `1 / PUSH_SHARE_DIVISOR` full.
pub const PUSH_SHARE_DIVISOR: u64 = 2;

/// WebSocket subscriber: topics and a channel of JSON notification strings.
pub struct Subscriber {
    pub topics: Vec<String>,
    /// Bounded (server::WS_QUEUE): a subscriber that stops reading is dropped when its
    /// queue fills, instead of growing the node's memory without bound.
    pub tx: futures::channel::mpsc::Sender<String>,
}

pub struct Core<E: commonware_storage::Context + Spawner, V: PowVerifier> {
    pub chain: Chain<E, V>,
    pub mempool: Mempool,
    pub templates: VecDeque<(String, Template)>,
    pub outbox: Vec<Outbound>,
    pub subscribers: Vec<Subscriber>,
    pub peers: usize,
    pub syncing: bool,
    /// Counters for throughput reporting.
    pub blocks_accepted: u64,
    pub txs_accepted: u64,
    /// Data availability: job data and results by blob id (None: no blob support).
    pub blobs: Option<BlobStore<E>>,
    /// Blobs wanted from peers (queued or in flight), with the tip height when first wanted.
    pub blob_wanted: HashMap<BlobId, u64>,
    /// Which wanted blobs are asked for now (bounded; see `blobfetch`).
    pub blob_fetch: crate::blobfetch::BlobFetches,
    pub last_blob_prune: u64,
    /// getPeers in open mode.
    pub peer_info: Option<PeerInfoFn>,
    /// getPeers in static mode: keys (base58) of the peers heard from.
    pub static_peers: Vec<String>,
    /// Chunk blob ids listed by each registered model's manifest we hold (SPEC 4.5),
    /// keyed by manifest blob id: pinned, never pruned.
    pub model_pins: HashMap<BlobId, Vec<BlobId>>,
    /// [`Core::blob_refs`] of the tip it was computed on: getBlob of a missing id reads it
    /// instead of rescanning up to BLOB_SCAN_JOBS jobs per call (security pass 2026-10-21).
    pub refs_cache: Option<(Hash, Arc<HashMap<BlobId, BlobRef>>)>,
    /// Last template request: `(time, tip, miner, choice, mempool size, template id)`.
    /// getBlockTemplate is public and executes up to max_block_txs txs, so repeats within
    /// [`TEMPLATE_REUSE_MS`] get the same template (security pass 2026-10-21).
    pub last_template: Option<(u64, Hash, Pubkey, JobChoice, usize, String)>,
    /// Weak blocks and pre-confirmations (docs/SPEED.md 2).
    pub preconf: crate::preconf::Preconf,
    /// Node-local index of the tip's jobs (jobindex.rs).
    pub jobs: crate::jobindex::JobIndex,
    /// Pooled txs that failed against the tip: `(consecutive tips, last tip, reason)`
    /// (mempool eviction, [`EVICT_AFTER_TIPS`]).
    pub tx_strikes: HashMap<Hash, (u32, Hash, String)>,
    /// Txs this node dropped from its pool, newest last: `(hash, reason, tip height)`.
    pub dropped: VecDeque<(Hash, String, u64)>,
    /// Txs evicted since start.
    pub evicted: u64,
    /// RPC calls by method since start: `(calls, total microseconds)` (getNodeStats).
    pub rpc_stats: HashMap<String, (u64, u64)>,
    /// Node start (ms) for getNodeStats.
    pub started_ms: u64,
}

/// A pooled tx that fails against this many consecutive tips (each a different block) is
/// dropped from the pool (2026-10-22: a PostSegments refused at every tip held its sender's
/// nonce for 9 hours, HANDOFF 39). Failures caused only by an earlier failing tx of the same
/// sender do not count.
pub const EVICT_AFTER_TIPS: u32 = 3;
/// Dropped txs remembered for getTxStatus.
pub const DROPPED_MEMORY: usize = 4096;

/// A getBlockTemplate for the same tip, miner, choice and mempool size within this many
/// milliseconds reuses the last template.
pub const TEMPLATE_REUSE_MS: u64 = 1000;

/// Blob ids a `cadre-model-v1` manifest lists (every file's chunks).
pub fn manifest_blobs(data: &[u8]) -> Vec<BlobId> {
    let Ok(v) = serde_json::from_slice::<Value>(data) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(files) = v.get("files").and_then(|f| f.as_object()) {
        for f in files.values() {
            for b in f.get("blobs").and_then(|b| b.as_array()).into_iter().flatten() {
                if let Some(id) = b.as_str().and_then(blobs::parse_id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

pub const MAX_TEMPLATES: usize = 64;

/// What a referenced blob is for (getBlobStats `classes`). When several references name one
/// blob the first class in this order wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlobClass {
    /// A registered model's manifest (its chunks are pinned through the manifest).
    Model,
    /// A segment's activation matrix A (`PostSegments.a_uri`).
    SegmentA,
    /// A profile blob (SPEC 4.6).
    Social,
    /// An inference ticket's request or response blob.
    Ticket,
    /// A plain job's data or result.
    Job,
}

impl BlobClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::SegmentA => "segment_a",
            Self::Social => "social",
            Self::Ticket => "ticket",
            Self::Job => "job",
        }
    }
}

/// One blob reference: the last height something needs it at (the grace period is added by
/// [`blobs::keep`]; `u64::MAX` = needed for as long as the chain says so) and its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRef {
    pub until: u64,
    pub class: BlobClass,
    /// `until` without the segment replay window (SPEC 4.5): a blob with `until` past the tip
    /// but `mine_until` behind it is held for replay only and counts against
    /// `replay_max_bytes`. Equal to `until` for everything but segment A.
    pub mine_until: u64,
}

fn add_ref(refs: &mut HashMap<BlobId, BlobRef>, id: BlobId, until: u64, class: BlobClass) {
    add_ref2(refs, id, until, until, class);
}

fn add_ref2(refs: &mut HashMap<BlobId, BlobRef>, id: BlobId, until: u64, mine_until: u64, class: BlobClass) {
    let e = refs.entry(id).or_insert(BlobRef { until, class, mine_until });
    e.until = e.until.max(until);
    e.mine_until = e.mine_until.max(mine_until);
    e.class = e.class.min(class);
}

/// Replay-only blobs past the replay budget (SPEC 4.5 "Segment data retention"): among the
/// stored blobs that only the replay window keeps (reference live, mining hold and age grace
/// ended, not pinned), the ones with the latest replay end are kept up to `budget` bytes and
/// the rest returned, to be pruned. Also returns the replay-only bytes kept.
pub fn replay_over_budget<'a>(
    entries: impl Iterator<Item = (&'a BlobId, &'a blobs::Meta)>,
    refs: &HashMap<BlobId, BlobRef>,
    pinned: &std::collections::HashSet<BlobId>,
    tip: u64,
    grace: u64,
    budget: u64,
) -> (std::collections::HashSet<BlobId>, u64) {
    let mut cand: Vec<(u64, BlobId, u64)> = Vec::new();
    for (id, m) in entries {
        let Some(r) = refs.get(id) else { continue };
        if pinned.contains(id) || r.until.saturating_add(grace) < tip || r.mine_until.saturating_add(grace) >= tip {
            continue;
        }
        if blobs::held_by_age(m, Some(r.until), tip, grace) {
            continue;
        }
        cand.push((r.until, *id, m.size));
    }
    cand.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let (mut held, mut drop) = (0u64, std::collections::HashSet::new());
    for (_, id, size) in cand {
        if held.saturating_add(size) <= budget {
            held += size;
        } else {
            drop.insert(id);
        }
    }
    (drop, held)
}

/// Segment A retention (SPEC 4.5 "Segment data retention"; node policy over consensus state).
/// `seg` is the segment at the tip, `seg_f` at the finalized block F (None: posted above F),
/// `ticket` / `ticket_f` its ticket at the tip / at F. Returns the last height A is needed at:
/// - mining: while the segment is mineable at the tip or at F (so a block on it is K deep
///   before its A goes), its job index is within the newest `template_scan` jobs (older ones
///   are never offered by a template) and the tip is within `mine_blocks` of its posting
///   (0 = no limit): until `min(posted + mine_blocks, deadline)`;
/// - replay (when `keep_replay`): while the ticket is live at F, until its deadline plus
///   `replay_blocks`; once it is closed at F (Completed or Expired, final), until its close
///   height plus `replay_blocks`.
#[allow(clippy::too_many_arguments)]
pub fn segment_a_until(
    seg: &schema::JobRecord,
    seg_f: Option<&schema::JobRecord>,
    ticket: Option<&schema::JobRecord>,
    ticket_f: Option<&schema::JobRecord>,
    tip: u64,
    f_height: u64,
    job_count: u64,
    template_scan: u64,
    mine_blocks: u64,
    keep_replay: bool,
    replay_blocks: u64,
) -> u64 {
    let mineable_tip = executor::check_mineable(seg, tip + 1).is_ok();
    let mineable_f = seg_f.map_or(true, |s| executor::check_mineable(s, f_height + 1).is_ok());
    let offered = seg.index.saturating_add(template_scan) >= job_count;
    let mining = if (mineable_tip || mineable_f) && offered {
        if mine_blocks > 0 { seg.created_height.saturating_add(mine_blocks).min(seg.deadline_height) } else { seg.deadline_height }
    } else {
        0
    };
    let replay = if !keep_replay {
        0
    } else {
        match (ticket_f, ticket) {
            (Some(t), _) if !t.status.is_live() => t.closed_height.saturating_add(replay_blocks),
            (Some(t), _) => t.deadline_height.saturating_add(replay_blocks),
            (None, Some(t)) => t.deadline_height.saturating_add(replay_blocks),
            // the ticket is unknown (a corrupt chain): the segment's own deadline bounds it
            (None, None) => seg.deadline_height.saturating_add(replay_blocks),
        }
    };
    mining.max(replay)
}

/// [`segment_a_until`] split into its mining and replay holds: `(mining, replay)`.
#[allow(clippy::too_many_arguments)]
pub fn segment_a_holds(
    seg: &schema::JobRecord,
    seg_f: Option<&schema::JobRecord>,
    ticket: Option<&schema::JobRecord>,
    ticket_f: Option<&schema::JobRecord>,
    tip: u64,
    f_height: u64,
    job_count: u64,
    template_scan: u64,
    mine_blocks: u64,
    replay_blocks: u64,
) -> (u64, u64) {
    let mining = segment_a_until(seg, seg_f, ticket, ticket_f, tip, f_height, job_count, template_scan, mine_blocks, false, replay_blocks);
    let both = segment_a_until(seg, seg_f, ticket, ticket_f, tip, f_height, job_count, template_scan, mine_blocks, true, replay_blocks);
    (mining, both)
}

#[derive(Debug, Clone)]
pub struct SubmitResult {
    pub accepted: bool,
    pub hash: Hash,
    pub height: u64,
    pub tip: bool,
    pub reason: Option<String>,
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    pub fn new(chain: Chain<E, V>) -> Self {
        let chain_id = chain.chain_id();
        Self {
            chain,
            mempool: Mempool::new(chain_id, 50_000),
            templates: VecDeque::new(),
            outbox: Vec::new(),
            subscribers: Vec::new(),
            peers: 0,
            syncing: false,
            blocks_accepted: 0,
            txs_accepted: 0,
            blobs: None,
            blob_wanted: HashMap::new(),
            blob_fetch: Default::default(),
            last_blob_prune: 0,
            peer_info: None,
            static_peers: Vec::new(),
            model_pins: HashMap::new(),
            refs_cache: None,
            last_template: None,
            preconf: Default::default(),
            jobs: Default::default(),
            tx_strikes: HashMap::new(),
            dropped: VecDeque::new(),
            evicted: 0,
            rpc_stats: HashMap::new(),
            started_ms: 0,
        }
    }

    pub fn with_blobs(mut self, store: BlobStore<E>) -> Self {
        self.blobs = Some(store);
        self
    }

    /// Is the data a job's blocks need held here? Non-blob URIs are not checked.
    pub fn job_data_available(&self, j: &schema::JobRecord) -> bool {
        match blobs::parse_uri(&j.data_uri) {
            Some(id) => self.blobs.as_ref().is_some_and(|b| b.has(&id).is_some()),
            None => true,
        }
    }

    /// Start fetching a blob we do not hold (bounded).
    pub fn want_blob(&mut self, id: BlobId) {
        let Some(store) = &self.blobs else { return };
        if store.has(&id).is_some() || self.blob_wanted.contains_key(&id) || self.blob_wanted.len() >= MAX_WANTED_BLOBS {
            return;
        }
        // A full store cannot keep what it fetches (the put fails), and the prune pass would
        // want it again every few blocks: a node with the default 1 GiB quota on a devnet
        // holding about 5 GB of referenced blobs re-fetched them from its peers forever.
        let c = store.config();
        if store.total_bytes().saturating_add(c.max_blob_bytes as u64) > c.max_total_bytes {
            return;
        }
        self.blob_wanted.insert(id, self.chain.tip_height());
        self.blob_fetch.push(id);
    }

    /// No longer want `id`: cancel its fetch if one is in flight. Returns true if it was wanted.
    pub fn unwant_blob(&mut self, id: BlobId) -> bool {
        let was = self.blob_wanted.remove(&id).is_some();
        if self.blob_fetch.remove(&id) {
            self.outbox.push(Outbound::CancelBlob(id));
        }
        was
    }

    /// putBlob: rate limit, store, push to peers.
    pub async fn put_blob(&mut self, data: &[u8], now_ms: u64) -> Result<(BlobId, bool), PutError> {
        self.put_blob_opt(data, now_ms, true).await
    }

    /// putBlob with `push = false`: stored here only; peers fetch it on demand (model uploads).
    pub async fn put_blob_opt(&mut self, data: &[u8], now_ms: u64, push: bool) -> Result<(BlobId, bool), PutError> {
        let height = self.chain.tip_height();
        let Some(store) = self.blobs.as_mut() else { return Err(PutError::Storage("this node has no blob store".into())) };
        let id = blobs::blob_id(data);
        if store.touch(&id, height) {
            return Ok((id, false));
        }
        // RPC writers stop short of the quota (backpressure, SPEC 4.5 "Segment data retention")
        store.check_rpc(data.len())?;
        if !store.take_rpc(data.len(), now_ms) {
            return Err(PutError::RateLimited);
        }
        let r = store.put(data, height).await?;
        // a blob this node was fetching itself (a registered model's manifest that nobody had yet):
        // stop the resolver asking peers for it
        self.unwant_blob(id);
        if push {
            self.outbox.push(Outbound::Blob(data.to_vec()));
        }
        Ok(r)
    }

    /// A blob pushed by `peer` (not relayed further). Returns false if the peer
    /// misbehaved (oversized).
    pub async fn receive_blob(&mut self, peer: &[u8], data: &[u8], now_ms: u64) -> bool {
        let height = self.chain.tip_height();
        let Some(store) = self.blobs.as_mut() else { return true };
        if data.len() > store.config().max_blob_bytes {
            return false;
        }
        let id = blobs::blob_id(data);
        if store.has(&id).is_some() {
            return true;
        }
        if !store.take_peer(peer, data.len(), now_ms) {
            debug!("blob push over the peer budget: dropped");
            return true;
        }
        // Unsolicited pushes (not a blob this node is fetching for a job) may fill at most
        // half the store, so peers cannot crowd out the data jobs reference (security pass
        // 2026-10-21); the referenced half is reached through fetches.
        if !self.blob_wanted.contains_key(&id) && store.total_bytes().saturating_add(data.len() as u64) > store.config().max_total_bytes / PUSH_SHARE_DIVISOR {
            debug!("blob push over the unsolicited share of the store: dropped");
            return true;
        }
        match store.put(data, height).await {
            Ok(_) => {
                self.unwant_blob(id);
            }
            Err(e) => debug!("pushed blob not stored: {e}"),
        }
        true
    }

    /// A blob we fetched arrived: verdict (false = the peer sent bad data).
    pub async fn deliver_blob(&mut self, id: BlobId, data: &[u8]) -> bool {
        let height = self.chain.tip_height();
        let Some(store) = self.blobs.as_mut() else { return true };
        if data.len() > store.config().max_blob_bytes || blobs::blob_id(data) != id {
            return false;
        }
        self.blob_wanted.remove(&id);
        self.blob_fetch.remove(&id);
        if let Err(e) = store.put(data, height).await {
            // usually the store is full (max_total_bytes): an operator needs to see this, the
            // fetch is dropped and only a later want retries it
            warn!(blob = %hex::encode(id), "fetched blob not stored: {e}");
        }
        true
    }

    pub async fn serve_blob(&mut self, id: &BlobId) -> Option<Vec<u8>> {
        self.blobs.as_mut()?.get(id).await.ok().flatten()
    }

    /// Blob ids referenced by txs of a block (job data and results).
    fn block_blob_refs(txs: &[Tx]) -> Vec<BlobId> {
        let mut v = Vec::new();
        for t in txs {
            match &t.body {
                TxBody::PostJob(p) => v.extend(blobs::parse_uri(&p.data_uri)),
                TxBody::BridgeUnlock { job_order: Some(p), .. } => v.extend(blobs::parse_uri(&p.data_uri)),
                TxBody::SubmitResult { uri, .. } => v.extend(blobs::parse_uri(uri)),
                _ => {}
            }
        }
        v
    }

    /// The blob parameters (SPEC 4.5 "Segment data retention") in the tip state.
    pub async fn blob_params(&self) -> Result<state::blobparams::BlobParams, ChainError> {
        Ok(match self.chain.read_tip(&state::blobparams::blob_params_key()).await? {
            Some(b) => state::blobparams::BlobParams::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => Default::default(),
        })
    }

    async fn job_at(&self, at: Option<&Hash>, id: &Hash) -> Result<Option<schema::JobRecord>, ChainError> {
        let v = match at {
            Some(h) => self.chain.read_at(h, &schema::job_key(id)).await?,
            None => self.chain.read_tip(&schema::job_key(id)).await?,
        };
        v.map(|v| schema::JobRecord::decode(&v).map_err(|e| ChainError::Internal(e.to_string()))).transpose()
    }

    /// Last height each referenced blob is needed at, from the jobs in the tip state:
    /// open jobs until their deadline, delivered jobs until they close, closed jobs
    /// until their close height (the grace period is added by [`blobs::keep`]); segment A
    /// blobs by [`segment_a_until`] (reads the finalized state too).
    pub async fn blob_refs(&self) -> Result<HashMap<BlobId, BlobRef>, ChainError> {
        let mut refs: HashMap<BlobId, BlobRef> = HashMap::new();
        let c = match self.chain.read_tip(&schema::counters_key()).await? {
            Some(b) => schema::Counters::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => return Ok(refs),
        };
        let tip = self.chain.tip_height();
        let (f_hash, f_height) = (self.chain.finalized().hash, self.chain.finalized().header.height);
        let (mine_blocks, keep_replay) = match &self.blobs {
            Some(b) => (b.config().segment_mine_blocks, b.config().keep_replay),
            None => (blobs::DEFAULT_SEGMENT_MINE_BLOCKS, true),
        };
        let replay_blocks = self.blob_params().await?.segment_replay_blocks as u64;
        // tickets at the tip and at F, read once per ticket
        let mut tickets: HashMap<Hash, (Option<schema::JobRecord>, Option<schema::JobRecord>)> = HashMap::new();
        for i in c.job_count.saturating_sub(BLOB_SCAN_JOBS)..c.job_count {
            let Some(b) = self.chain.read_tip(&schema::job_index_key(i)).await? else { continue };
            let id = schema::decode_job_index(&b).map_err(|e| ChainError::Internal(e.to_string()))?;
            let Some(v) = self.chain.read_tip(&schema::job_key(&id)).await? else { continue };
            let j = schema::JobRecord::decode(&v).map_err(|e| ChainError::Internal(e.to_string()))?;
            if let Some((ticket_id, _, _)) = j.segment() {
                let Some(a) = blobs::parse_uri(&j.data_uri) else { continue };
                let seg_f = self.job_at(Some(&f_hash), &id).await?;
                if !tickets.contains_key(&ticket_id) {
                    let t = self.job_at(None, &ticket_id).await?;
                    let tf = self.job_at(Some(&f_hash), &ticket_id).await?;
                    tickets.insert(ticket_id, (t, tf));
                }
                let (t, tf) = &tickets[&ticket_id];
                let (mining, both) = segment_a_holds(
                    &j, seg_f.as_ref(), t.as_ref(), tf.as_ref(), tip, f_height, c.job_count,
                    consensus_pow::chain::TEMPLATE_JOB_SCAN, mine_blocks, replay_blocks,
                );
                add_ref2(&mut refs, a, if keep_replay { both } else { mining }, mining, BlobClass::SegmentA);
                continue;
            }
            let mut until = match j.status {
                schema::JobStatus::Open => j.deadline_height,
                schema::JobStatus::Delivered => u64::MAX,
                _ => j.closed_height,
            };
            if j.is_ticket() {
                // a ticket's request and response blobs (the per-request receipts) stay as long as
                // its segments are mineable (SPEC 4.5), not just a grace period after completion
                until = until.max(j.deadline_height);
            }
            let class = if j.is_ticket() { BlobClass::Ticket } else { BlobClass::Job };
            for b in [blobs::parse_uri(&j.data_uri), blobs::parse_uri(&j.result_uri)].into_iter().flatten() {
                add_ref(&mut refs, b, until, class);
            }
        }
        // Registered models: their manifests are kept (and fetched when missing).
        for m in self.models().await? {
            if let Some(b) = blobs::parse_uri(&m.manifest_uri) {
                add_ref(&mut refs, b, u64::MAX, BlobClass::Model);
            }
        }
        // Profiles (SPEC 4.6): a profile's blob is kept while the profile names it.
        for b in self.profile_blobs().await.map_err(|e| ChainError::Internal(e.message))? {
            add_ref(&mut refs, b, u64::MAX, BlobClass::Social);
        }
        Ok(refs)
    }

    /// getBlobStats: totals, limits, the retention parameters and a breakdown of the stored
    /// blobs by class (model, segment_a, social, ticket, job, other = referenced by nothing:
    /// posts, DMs, uploads waiting for their tx) and by fate at the next prune pass (kept or
    /// prunable). Bytes are payload bytes (what `max_total_bytes` counts).
    pub async fn blob_stats(&mut self) -> Result<Value, ChainError> {
        let refs = self.blob_refs_cached().await?;
        let pinned = self.model_pinned().await;
        let replay_blocks = self.blob_params().await?.segment_replay_blocks;
        let tip = self.chain.tip_height();
        let Some(b) = self.blobs.as_ref() else { return Err(ChainError::Internal("this node has no blob store".into())) };
        let c = b.config();
        let grace = c.grace_blocks;
        #[derive(Default, Clone, Copy)]
        struct Tally {
            count: u64,
            bytes: u64,
            kept: u64,
            kept_bytes: u64,
            prunable: u64,
            prunable_bytes: u64,
        }
        let (over, replay_held) = replay_over_budget(b.entries(), &refs, &pinned, tip, grace, c.replay_budget());
        let mut age_held = 0u64;
        let mut by: std::collections::BTreeMap<&'static str, Tally> = std::collections::BTreeMap::new();
        for k in ["model", "segment_a", "social", "ticket", "job", "other"] {
            by.insert(k, Tally::default());
        }
        let (mut ref_bytes, mut prune_bytes) = (0u64, 0u64);
        for (id, m) in b.entries() {
            let r = refs.get(id);
            let class = if pinned.contains(id) { "model" } else { r.map_or("other", |r| r.class.name()) };
            let keep = pinned.contains(id) || (blobs::keep(m, r.map(|r| r.until), tip, grace) && !over.contains(id));
            // kept only by the age grace (uploads waiting for their tx, fresh fetches)
            if !pinned.contains(id) && !r.is_some_and(|r| r.until.saturating_add(grace) >= tip) && blobs::held_by_age(m, r.map(|r| r.until), tip, grace) {
                age_held += m.size;
            }
            let t = by.get_mut(class).unwrap();
            t.count += 1;
            t.bytes += m.size;
            if keep {
                t.kept += 1;
                t.kept_bytes += m.size;
            } else {
                t.prunable += 1;
                t.prunable_bytes += m.size;
                prune_bytes += m.size;
            }
            if r.is_some() || pinned.contains(id) {
                ref_bytes += m.size;
            }
        }
        let classes: serde_json::Map<String, Value> = by
            .into_iter()
            .map(|(k, t)| {
                (k.to_string(), json!({"count": t.count, "bytes": t.bytes, "kept": t.kept, "kept_bytes": t.kept_bytes,
                                       "prunable": t.prunable, "prunable_bytes": t.prunable_bytes}))
            })
            .collect();
        Ok(json!({
            "count": b.len(), "total_bytes": b.total_bytes(), "fetching": self.blob_wanted.len(), "fetch_in_flight": self.blob_fetch.in_flight(),
            "max_blob_bytes": c.max_blob_bytes, "max_total_bytes": c.max_total_bytes, "grace_blocks": c.grace_blocks,
            "put_rate_bytes_per_s": c.put_rate_bytes_per_s, "last_prune_height": self.last_blob_prune,
            "rpc_reserve_bytes": c.rpc_reserve(), "rpc_put_limit_bytes": c.rpc_put_limit(), "rpc_headroom_bytes": b.rpc_headroom(),
            "puts_open": b.rpc_headroom() >= c.max_blob_bytes as u64,
            "referenced_bytes": ref_bytes, "unreferenced_bytes": b.total_bytes() - ref_bytes, "prunable_bytes": prune_bytes,
            "age_held_bytes": age_held, "replay_held_bytes": replay_held,
            "replay_over_budget_bytes": over.iter().filter_map(|id| b.has(id)).map(|m| m.size).sum::<u64>(),
            "retention": {"segment_mine_blocks": c.segment_mine_blocks, "keep_replay": c.keep_replay, "replay_max_bytes": c.replay_budget(),
                          "segment_replay_blocks": replay_blocks, "template_job_scan": consensus_pow::chain::TEMPLATE_JOB_SCAN},
            "classes": classes,
        }))
    }

    /// [`Core::blob_refs`] at the current tip, computed at most once per tip.
    pub async fn blob_refs_cached(&mut self) -> Result<Arc<HashMap<BlobId, BlobRef>>, ChainError> {
        let tip = self.chain.tip().hash;
        if let Some((h, r)) = &self.refs_cache {
            if *h == tip {
                return Ok(r.clone());
            }
        }
        let r = Arc::new(self.blob_refs().await?);
        self.refs_cache = Some((tip, r.clone()));
        Ok(r)
    }

    /// getModel / listModels: does this node hold the model's manifest, and is it fetching it?
    /// A registered model whose manifest no node holds (its upload never ran) shows
    /// `manifest_present: false, manifest_fetching: true` on every node.
    pub fn add_manifest_state(&self, v: &mut Value, m: &schema::Model) {
        let id = blobs::parse_uri(&m.manifest_uri);
        let present = id.is_some_and(|id| self.blobs.as_ref().is_some_and(|b| b.has(&id).is_some()));
        let fetching = id.is_some_and(|id| self.blob_wanted.contains_key(&id));
        if let Some(o) = v.as_object_mut() {
            o.insert("manifest_present".into(), json!(present));
            o.insert("manifest_fetching".into(), json!(fetching));
        }
    }

    /// Every registered model (tip state), in registration order.
    pub async fn models(&self) -> Result<Vec<schema::Model>, ChainError> {
        let n = match self.chain.read_tip(&schema::model_count_key()).await? {
            Some(b) => schema::decode_model_count(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => 0,
        };
        let mut out = Vec::new();
        for i in 0..n {
            let Some(b) = self.chain.read_tip(&schema::model_index_key(i)).await? else { continue };
            let id = schema::decode_model_index(&b).map_err(|e| ChainError::Internal(e.to_string()))?;
            if let Some(v) = self.chain.read_tip(&schema::model_key(&id)).await? {
                out.push(schema::Model::decode(&v).map_err(|e| ChainError::Internal(e.to_string()))?);
            }
        }
        Ok(out)
    }

    /// Model chunk blobs this node pins: those listed by the manifests of registered
    /// models it holds (parsed once per manifest). Pinned blobs are never pruned and
    /// are fetched on demand (getBlob), not proactively.
    pub async fn model_pinned(&mut self) -> HashSet<BlobId> {
        let models = self.models().await.unwrap_or_default();
        let mut out = HashSet::new();
        for m in models {
            let Some(mid) = blobs::parse_uri(&m.manifest_uri) else { continue };
            out.insert(mid);
            if !self.model_pins.contains_key(&mid) {
                if let Some(data) = self.serve_blob(&mid).await {
                    self.model_pins.insert(mid, manifest_blobs(&data));
                }
            }
            if let Some(v) = self.model_pins.get(&mid) {
                out.extend(v.iter().copied());
            }
        }
        out
    }

    /// Drop blobs no job needs any more (after the grace period) and stop fetches for
    /// blobs nobody references. Returns the removed ids.
    pub async fn prune_blobs(&mut self) -> Vec<BlobId> {
        if self.blobs.is_none() {
            return Vec::new();
        }
        let tip = self.chain.tip_height();
        self.last_blob_prune = tip;
        let refs = match self.blob_refs_cached().await {
            Ok(r) => r,
            Err(e) => {
                debug!("blob prune skipped: {e}");
                return Vec::new();
            }
        };
        let pinned = self.model_pinned().await;
        let store = self.blobs.as_mut().unwrap();
        let grace = store.config().grace_blocks;
        let (over, replay_held) = replay_over_budget(store.entries(), &refs, &pinned, tip, grace, store.config().replay_budget());
        if !over.is_empty() {
            info!(count = over.len(), replay_held, budget = store.config().replay_budget(), "replay-only segment A over the replay budget: pruning the earliest-ending");
        }
        // room for one more blob of any size: else a fetched replay A would be dropped again at
        // the next pass after its age grace (a fetch loop with the peers)
        let replay_room = replay_held.saturating_add(store.config().max_blob_bytes as u64) <= store.config().replay_budget();
        let removed = store.prune(|id, m| pinned.contains(id) || (blobs::keep(m, refs.get(id).map(|r| r.until), tip, grace) && !over.contains(id))).await;
        if !removed.is_empty() {
            info!(count = removed.len(), "pruned blobs");
        }
        // Safety net (restarts, missed pushes, failed fetches): fetch every blob a job still
        // needs that this node lacks.
        let missing: Vec<BlobId> = refs
            .iter()
            // replay-only A is fetched only while the replay budget has room (else the next
            // pass would drop it again: a fetch loop)
            .filter(|(id, r)| r.until.saturating_add(grace) >= tip && store.has(id).is_none() && (replay_room || r.mine_until.saturating_add(grace) >= tip))
            .map(|(id, _)| *id)
            .collect();
        for id in missing {
            self.want_blob(id);
        }
        let stale: Vec<BlobId> = self
            .blob_wanted
            .iter()
            .filter(|(id, since)| {
                if pinned.contains(*id) {
                    return false;
                }
                let meta = blobs::Meta { size: 0, stored_height: **since, reloaded: false };
                !blobs::keep(&meta, refs.get(*id).map(|r| r.until), tip, grace)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.unwant_blob(id);
        }
        removed
    }

    pub async fn account(&self, a: &Pubkey) -> Result<Account, ChainError> {
        Ok(match self.chain.read_tip(&schema::account_key(a)).await? {
            Some(b) => Account::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => Account::default(),
        })
    }

    /// Admit a tx from RPC or gossip. `relay` queues it for peers.
    pub async fn add_tx(&mut self, tx: Tx, relay: bool) -> Result<Hash, String> {
        let acc = self.account(&tx.from).await.map_err(|e| e.to_string())?;
        let min_fee = self.chain.tip().config.min_fee;
        // Fail fast on job bodies the executor would refuse (min protocol fee, URIs), so
        // they do not sit in the mempool forever.
        let job = match &tx.body {
            TxBody::PostJob(p) => Some(p),
            TxBody::BridgeUnlock { job_order: Some(p), .. } => Some(p),
            _ => None,
        };
        if let Some(p) = job {
            executor::precheck_post_job(p, &self.chain.tip().config)?;
        }
        if let TxBody::RegisterModel(m) = &tx.body {
            let next = self.chain.tip_height() + 1;
            let max = agent_types::tx::max_model_matrices(tx.chain_id, next);
            if m.matrices.len() > max {
                return Err(format!("a model registers 1..={max} matrices at height {next}"));
            }
        }
        // social kinds (SPEC 4.6): activation height and the social fee floor
        executor::precheck_social(&tx, &self.chain.tip().config, self.chain.tip_height() + 1)?;
        // DM kinds (SPEC 4.10): activation, fee floor, then the executor's own rules on the tip
        executor::precheck_dm(&tx, &self.chain.tip().config, self.chain.tip_height() + 1)?;
        self.admit_dm(&tx, acc.nonce).await?;
        // contract kinds (docs/CONTRACTS.md): activation, gas limits, fee floor, code validation
        executor::precheck_contract(&tx, &self.chain.tip().config, self.chain.tip_height() + 1)?;
        // BridgeUnlock pays no fee: only attester-signed, unprocessed deposits are pooled
        if let TxBody::BridgeUnlock { solana_nonce, .. } = &tx.body {
            executor::precheck_bridge_unlock(&tx, &self.chain.tip().config)?;
            if self.chain.read_tip(&schema::deposit_key(*solana_nonce)).await.map_err(|e| e.to_string())?.is_some() {
                return Err(format!("deposit {solana_nonce} already processed"));
            }
        }
        if tx.from == self.chain.tip().config.admin {
            executor::precheck_admin_config(&tx, acc.balance)?;
        }
        let pooled = self.mempool.pooled_at(&tx.from, tx.nonce);
        crate::preconf::check_replacement(&self.preconf, &self.chain.tip().hash, &tx, pooled)?;
        // a pooled tx that fails at the tip may be replaced at any fee (no 10% bump)
        let failing = pooled.is_some_and(|p| self.tx_strikes.contains_key(&p));
        let bytes = tx.encode();
        let h = self.mempool.insert_funded_opts(tx, acc.nonce, acc.balance, min_fee, failing).map_err(|e| e.to_string())?;
        self.txs_accepted += 1;
        if relay {
            self.outbox.push(Outbound::Tx(bytes));
        }
        self.notify("txs", json!({"tx_hash": hex::encode(h)}));
        Ok(h)
    }

    /// Accept a block from a miner (`relay` = gossip it) or a peer.
    pub async fn add_block(&mut self, block: Block, now_ms: u64, relay: bool) -> SubmitResult {
        let hash = block.hash();
        let height = block.header.height;
        let bytes = if relay { Some(block.encode()) } else { None };
        match self.chain.insert_block(block, now_ms).await {
            Ok((Accept::Added { tip }, events)) => {
                self.blocks_accepted += 1;
                if let Some(b) = bytes {
                    self.outbox.push(Outbound::Block(b));
                }
                self.on_events(events).await;
                SubmitResult { accepted: true, hash, height, tip, reason: None }
            }
            Ok((Accept::Known, _)) => SubmitResult { accepted: true, hash, height, tip: false, reason: Some("already known".into()) },
            Ok((Accept::Orphan { missing }, _)) => {
                tracing::debug!(block = %hex::encode(&hash[..6]), height, missing = %hex::encode(&missing[..6]), "orphan block: fetching its parent's header");
                self.outbox.push(Outbound::FetchHeader(missing));
                SubmitResult { accepted: true, hash, height, tip: false, reason: Some("orphan: parent unknown, fetching".into()) }
            }
            Err(e) => SubmitResult { accepted: false, hash, height, tip: false, reason: Some(e.to_string()) },
        }
    }

    /// Mempool and subscriber upkeep after chain events.
    pub async fn on_events(&mut self, events: Vec<ChainEvent>) {
        let mut tip_moved = false;
        for ev in events {
            match ev {
                ChainEvent::NewTip { hash, height, reorg_depth, disconnected, connected } => {
                    tip_moved = true;
                    for d in &disconnected {
                        if let Some(n) = self.chain.live_node(d) {
                            for t in n.txs.iter().cloned().collect::<Vec<_>>() {
                                self.mempool.reinsert(t);
                            }
                        }
                    }
                    let mut refs = Vec::new();
                    for c in &connected {
                        if let Some(n) = self.chain.live_node(c) {
                            for t in n.txs.iter() {
                                self.mempool.remove(&t.hash());
                            }
                            refs.extend(Self::block_blob_refs(&n.txs));
                        } else if let Ok(Some(b)) = self.chain.get_block(c).await {
                            // connected and already finalized in the same insert (catch-up)
                            for t in &b.txs {
                                self.mempool.remove(&t.hash());
                            }
                            refs.extend(Self::block_blob_refs(&b.txs));
                        }
                    }
                    for id in refs {
                        self.want_blob(id);
                    }
                    let (job_id, miner, tx_count) = match self.chain.live_node(&hash) {
                        Some(n) => (n.header.job_id, n.header.miner, n.txs.len()),
                        None => ([0; 32], [0; 32], 0),
                    };
                    if reorg_depth > 0 {
                        info!(height, depth = reorg_depth, "tip moved to another branch");
                    } else {
                        debug!(height, "new tip");
                    }
                    self.notify(
                        "heads",
                        json!({"hash": hex::encode(hash), "height": height, "job_id": hex::encode(job_id),
                               "miner": bs58::encode(miner).into_string(), "tx_count": tx_count, "reorg_depth": reorg_depth}),
                    );
                }
                ChainEvent::Finalized { hash, height } => {
                    self.notify("finalized", json!({"hash": hex::encode(hash), "height": height}));
                }
                ChainEvent::WithdrawalFinal(w) => {
                    let v = crate::dispatch::withdrawal_json(&self.chain, &w);
                    self.notify("withdrawals", v);
                }
            }
        }
        if tip_moved && self.chain.tip_height() >= self.last_blob_prune + BLOB_PRUNE_EVERY {
            self.prune_blobs().await;
        }
        if tip_moved {
            self.prune_weak();
            self.prune_mempool().await;
            self.templates.retain(|(_, t)| self.chain.live_node(&t.header.prev_hash).is_some());
            if self.jobs.at.is_some() {
                if let Err(e) = self.sync_jobs().await {
                    warn!("job index update failed, rebuilding on next use: {e}");
                    self.jobs.at = None;
                }
            }
            self.check_pool().await;
        }
    }

    /// Execute the pool's candidate txs against the tip (as a template would) and drop any
    /// that failed at [`EVICT_AFTER_TIPS`] consecutive tips. The sender can then send another
    /// tx at that nonce (or replace a failing one at any fee before that, see `add_tx`).
    pub async fn check_pool(&mut self) {
        if self.mempool.is_empty() {
            self.tx_strikes.clear();
            return;
        }
        let tip = self.chain.tip().hash;
        let txs = self.candidate_txs().await;
        let n = self.chain.tip();
        let env = executor::BlockEnv { chain_id: self.chain.chain_id(), height: n.header.height + 1, miner: [0; 32], job_id: None };
        let view = state::View { store: self.chain.store(), snap: &n.snapshot };
        let mut ex = match executor::BlockExecutor::begin(&view, env).await {
            Ok(x) => x,
            Err(e) => {
                debug!("pool check skipped: {e}");
                return;
            }
        };
        let mut failed: Vec<(Hash, String)> = Vec::new();
        let mut ok: Vec<Hash> = Vec::new();
        let mut blocked: HashSet<Pubkey> = HashSet::new();
        for t in &txs {
            if blocked.contains(&t.from) {
                continue; // fails because an earlier tx of its sender did
            }
            match ex.apply_tx(t).await {
                Ok(_) => ok.push(t.hash()),
                Err(e) => {
                    blocked.insert(t.from);
                    failed.push((t.hash(), e.to_string()));
                }
            }
        }
        drop(ex);
        for h in ok {
            self.tx_strikes.remove(&h);
        }
        let height = self.chain.tip_height();
        for (h, reason) in failed {
            let e = self.tx_strikes.entry(h).or_insert((0, [0; 32], String::new()));
            if e.1 != tip {
                e.0 += 1;
                e.1 = tip;
            }
            e.2 = reason;
            if e.0 >= EVICT_AFTER_TIPS {
                let reason = format!("failed at {} consecutive tips: {}", e.0, e.2);
                self.tx_strikes.remove(&h);
                if let Some(t) = self.mempool.remove(&h) {
                    info!(tx = %hex::encode(&h[..6]), from = %bs58::encode(t.from).into_string(), nonce = t.nonce, %reason, "dropped a pooled tx");
                    self.evicted += 1;
                    self.dropped.push_back((h, reason, height));
                    while self.dropped.len() > DROPPED_MEMORY {
                        self.dropped.pop_front();
                    }
                }
            }
        }
        let pool = &self.mempool;
        self.tx_strikes.retain(|h, _| pool.contains(h));
    }

    /// Why this node dropped `h` from its pool, if it did recently.
    pub fn dropped_reason(&self, h: &Hash) -> Option<(&str, u64)> {
        self.dropped.iter().rev().find(|(x, _, _)| x == h).map(|(_, r, ht)| (r.as_str(), *ht))
    }

    async fn prune_mempool(&mut self) {
        let mut nonces = HashMap::new();
        for s in self.mempool.senders() {
            if let Ok(a) = self.account(&s).await {
                nonces.insert(s, a.nonce);
            }
        }
        self.mempool.prune(&nonces);
    }

    /// Mempool txs in block order for the tip.
    pub async fn candidate_txs(&self) -> Vec<Tx> {
        let mut nonces = HashMap::new();
        for s in self.mempool.senders() {
            if let Ok(a) = self.account(&s).await {
                nonces.insert(s, a.nonce);
            }
        }
        let max = self.chain.tip().config.max_block_txs as usize;
        self.mempool.select(&nonces, max)
    }

    pub async fn template(&mut self, miner: Pubkey, choice: JobChoice, now_ms: u64) -> Result<(String, Template), ChainError> {
        let tip = self.chain.tip().hash;
        if let Some((t0, h, m, c, n, id)) = &self.last_template {
            if *h == tip && *m == miner && *c == choice && *n == self.mempool.len() && now_ms >= *t0 && now_ms - *t0 < TEMPLATE_REUSE_MS {
                if let Some((_, t)) = self.templates.iter().find(|(i, _)| i == id) {
                    return Ok((id.clone(), t.clone()));
                }
            }
        }
        let txs = self.candidate_txs().await;
        // auto choice: mineable jobs from the job index instead of a 4096-job state scan
        let jobs = if choice == JobChoice::Auto {
            match self.sync_jobs().await {
                Ok(()) => Some(self.jobs.template_candidates(self.chain.tip_height() + 1, consensus_pow::chain::TEMPLATE_JOB_SCAN)),
                Err(e) => {
                    warn!("job index unavailable for the template ({e}); scanning state");
                    self.jobs.at = None;
                    None
                }
            }
        } else {
            None
        };
        let t = {
            let avail = |j: &schema::JobRecord| self.job_data_available(j);
            self.chain.build_template_with(tip, miner, choice, &txs, now_ms, &avail, jobs).await
        };
        if let (Err(_), JobChoice::Job(id)) = (&t, choice) {
            // a named job whose data we lack: start fetching it so a retry can succeed
            if let Ok(Some(v)) = self.chain.read_tip(&schema::job_key(&id)).await {
                if let Some(b) = schema::JobRecord::decode(&v).ok().and_then(|j| blobs::parse_uri(&j.data_uri)) {
                    self.want_blob(b);
                }
            }
        }
        let t = t?;
        let id = template_id(&t);
        if !self.templates.iter().any(|(i, _)| *i == id) {
            self.templates.push_back((id.clone(), t.clone()));
            while self.templates.len() > MAX_TEMPLATES {
                self.templates.pop_front();
            }
        }
        self.last_template = Some((now_ms, tip, miner, choice, self.mempool.len(), id.clone()));
        Ok((id, t))
    }

    /// Txs and coinbase receipt for a mined header: from the named template or any
    /// template with the same parent and tx_root.
    pub fn template_txs(&self, header: &agent_types::Header, template_id: Option<&str>) -> Option<(Vec<Tx>, agent_types::Coinbase, Option<agent_types::contracts::ContractsRecord>)> {
        self.template_body(header, template_id).map(|(t, c, k, _)| (t, c, k))
    }

    /// [`Core::template_txs`] plus the template's uncles (docs/SPEED.md 3.3).
    pub fn template_body(&self, header: &agent_types::Header, template_id: Option<&str>) -> Option<(Vec<Tx>, agent_types::Coinbase, Option<agent_types::contracts::ContractsRecord>, Vec<agent_types::speed::Uncle>)> {
        let m = |t: &Template| t.header.prev_hash == header.prev_hash && t.header.tx_root == header.tx_root;
        if let Some(id) = template_id {
            if let Some((_, t)) = self.templates.iter().find(|(i, _)| i == id) {
                if m(t) {
                    return Some((t.txs.clone(), t.summary.coinbase, t.summary.contracts, t.uncles.clone()));
                }
            }
        }
        self.templates.iter().rev().find(|(_, t)| m(t)).map(|(_, t)| (t.txs.clone(), t.summary.coinbase, t.summary.contracts, t.uncles.clone()))
    }

    pub fn notify(&mut self, topic: &str, mut data: Value) {
        if self.subscribers.is_empty() {
            return;
        }
        crate::cxjson::add_cx_fields(&mut data);
        let msg = json!({"jsonrpc": "2.0", "method": "notify", "params": {"topic": topic, "data": data}}).to_string();
        self.subscribers.retain_mut(|s| {
            if !s.topics.iter().any(|t| t == topic) {
                return !s.tx.is_closed();
            }
            s.tx.try_send(msg.clone()).is_ok()
        });
    }

    pub fn take_outbox(&mut self) -> Vec<Outbound> {
        std::mem::take(&mut self.outbox)
    }

    /// Ask peers for the next bodies headers-first sync can use (lowest height first, a window
    /// above the tip; consensus_pow::BODY_WINDOW). The node calls this after every event.
    /// Hand out the fetches due now: block bodies in height order, and blobs at most
    /// `BLOB_FETCH_WINDOW` at a time (stale blob fetches cancelled and queued again).
    pub fn schedule_fetches(&mut self) {
        for h in self.chain.next_bodies(consensus_pow::BODY_WINDOW) {
            self.outbox.push(Outbound::FetchBody(h));
        }
        let (start, cancel) = self.blob_fetch.pump(self.chain.tip_height(), crate::blobfetch::BLOB_FETCH_WINDOW);
        for id in cancel {
            self.outbox.push(Outbound::CancelBlob(id));
        }
        for id in start {
            self.outbox.push(Outbound::FetchBlob(id));
        }
    }
}

/// `blake3(header with nonce, proof_hash and timestamp zeroed)`, hex.
pub fn template_id(t: &Template) -> String {
    let mut h = t.header;
    h.nonce = 0;
    h.proof_hash = [0; 32];
    h.timestamp_ms = 0;
    hex::encode(h.hash())
}

#[cfg(test)]
mod replay_budget_tests {
    use super::*;

    fn meta(size: u64) -> blobs::Meta {
        blobs::Meta { size, stored_height: 0, reloaded: true }
    }

    /// Replay-only blobs past the budget go, the latest replay end first is kept; blobs the
    /// mining rule holds, pinned ones, ended references and fresh uploads never count.
    #[test]
    fn keeps_latest_replay_end_within_budget() {
        let (tip, grace) = (1_000, 10);
        let ids: Vec<BlobId> = (0u8..6).map(|i| [i; 32]).collect();
        let mut store = std::collections::BTreeMap::new();
        for id in &ids {
            store.insert(*id, meta(100));
        }
        store.insert(ids[5], blobs::Meta { size: 100, stored_height: 995, reloaded: false });
        let mut refs = HashMap::new();
        add_ref2(&mut refs, ids[0], 5_000, 10, BlobClass::SegmentA); // replay only, latest end
        add_ref2(&mut refs, ids[1], 4_000, 10, BlobClass::SegmentA); // replay only
        add_ref2(&mut refs, ids[2], 3_000, 10, BlobClass::SegmentA); // replay only, earliest end
        add_ref2(&mut refs, ids[3], 6_000, 2_000, BlobClass::SegmentA); // mining still holds it
        add_ref(&mut refs, ids[4], 500, BlobClass::Job); // reference ended: not replay's business
        add_ref2(&mut refs, ids[5], 2_000, 10, BlobClass::SegmentA); // fresh upload (age grace)
        let pinned = std::collections::HashSet::new();
        let (drop, held) = replay_over_budget(store.iter(), &refs, &pinned, tip, grace, 250);
        assert_eq!(held, 200);
        assert_eq!(drop, [ids[2]].into_iter().collect());
        let pinned: std::collections::HashSet<BlobId> = [ids[0]].into_iter().collect();
        let (drop, held) = replay_over_budget(store.iter(), &refs, &pinned, tip, grace, 100);
        assert_eq!(held, 100);
        assert_eq!(drop, [ids[2]].into_iter().collect(), "pinned blobs never count");
        let (drop, _) = replay_over_budget(store.iter(), &refs, &std::collections::HashSet::new(), tip, grace, 0);
        assert_eq!(drop.len(), 3, "budget 0 drops every replay-only blob");
    }
}
