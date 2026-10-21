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
    pub refs_cache: Option<(Hash, Arc<HashMap<BlobId, u64>>)>,
    /// Last template request: `(time, tip, miner, choice, mempool size, template id)`.
    /// getBlockTemplate is public and executes up to max_block_txs txs, so repeats within
    /// [`TEMPLATE_REUSE_MS`] get the same template (security pass 2026-10-21).
    pub last_template: Option<(u64, Hash, Pubkey, JobChoice, usize, String)>,
}

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
        if store.has(&id).is_some() {
            return Ok((id, false));
        }
        store.check(data.len())?;
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

    /// Last height each referenced blob is needed at, from the jobs in the tip state:
    /// open jobs until their deadline, delivered jobs until they close, closed jobs
    /// until their close height (the grace period is added by [`blobs::keep`]).
    pub async fn blob_refs(&self) -> Result<HashMap<BlobId, u64>, ChainError> {
        let mut refs: HashMap<BlobId, u64> = HashMap::new();
        let c = match self.chain.read_tip(&schema::counters_key()).await? {
            Some(b) => schema::Counters::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => return Ok(refs),
        };
        for i in c.job_count.saturating_sub(BLOB_SCAN_JOBS)..c.job_count {
            let Some(b) = self.chain.read_tip(&schema::job_index_key(i)).await? else { continue };
            let id = schema::decode_job_index(&b).map_err(|e| ChainError::Internal(e.to_string()))?;
            let Some(v) = self.chain.read_tip(&schema::job_key(&id)).await? else { continue };
            let j = schema::JobRecord::decode(&v).map_err(|e| ChainError::Internal(e.to_string()))?;
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
            for b in [blobs::parse_uri(&j.data_uri), blobs::parse_uri(&j.result_uri)].into_iter().flatten() {
                let e = refs.entry(b).or_insert(0);
                *e = (*e).max(until);
            }
        }
        // Registered models: their manifests are kept (and fetched when missing).
        for m in self.models().await? {
            if let Some(b) = blobs::parse_uri(&m.manifest_uri) {
                refs.insert(b, u64::MAX);
            }
        }
        // Profiles (SPEC 4.6): a profile's blob is kept while the profile names it.
        for b in self.profile_blobs().await.map_err(|e| ChainError::Internal(e.message))? {
            refs.insert(b, u64::MAX);
        }
        Ok(refs)
    }

    /// [`Core::blob_refs`] at the current tip, computed at most once per tip.
    pub async fn blob_refs_cached(&mut self) -> Result<Arc<HashMap<BlobId, u64>>, ChainError> {
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
        let removed = store.prune(|id, m| pinned.contains(id) || blobs::keep(m, refs.get(id).copied(), tip, grace)).await;
        if !removed.is_empty() {
            info!(count = removed.len(), "pruned blobs");
        }
        // Safety net (restarts, missed pushes, failed fetches): fetch every blob a job still
        // needs that this node lacks.
        let missing: Vec<BlobId> = refs
            .iter()
            .filter(|(id, until)| until.saturating_add(grace) >= tip && store.has(id).is_none())
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
                let meta = blobs::Meta { size: 0, stored_height: **since };
                !blobs::keep(&meta, refs.get(*id).copied(), tip, grace)
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
        let bytes = tx.encode();
        let h = self.mempool.insert_funded(tx, acc.nonce, acc.balance, min_fee).map_err(|e| e.to_string())?;
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
            self.prune_mempool().await;
            self.templates.retain(|(_, t)| self.chain.live_node(&t.header.prev_hash).is_some());
        }
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
        let t = {
            let avail = |j: &schema::JobRecord| self.job_data_available(j);
            self.chain.build_template_filtered(tip, miner, choice, &txs, now_ms, &avail).await
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
        let m = |t: &Template| t.header.prev_hash == header.prev_hash && t.header.tx_root == header.tx_root;
        if let Some(id) = template_id {
            if let Some((_, t)) = self.templates.iter().find(|(i, _)| i == id) {
                if m(t) {
                    return Some((t.txs.clone(), t.summary.coinbase, t.summary.contracts));
                }
            }
        }
        self.templates.iter().rev().find(|(_, t)| m(t)).map(|(_, t)| (t.txs.clone(), t.summary.coinbase, t.summary.contracts))
    }

    pub fn notify(&mut self, topic: &str, data: Value) {
        if self.subscribers.is_empty() {
            return;
        }
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
