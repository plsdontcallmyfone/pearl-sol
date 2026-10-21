//! The chain manager: block tree, validation, execution, fork choice, finality.
//!
//! - Every executed block whose ancestry includes the finalized block `F` lives in
//!   `live` with its speculative state [`Snapshot`] (one QMDB batch chain per branch).
//! - The tip is the live block with the most cumulative work (`2^256 / (target+1)` per
//!   block); ties keep the first seen.
//! - `F` trails the tip by `finality_k` (K) blocks on the canonical chain. When it
//!   advances, its snapshot is applied and committed to disk and every branch that does
//!   not descend from it is dropped. A block whose parent is below `F` and not `F`
//!   itself is refused: reorgs deeper than K never happen.
//! - Bodies and proofs of accepted blocks are stored in a prunable archive (index =
//!   height, key = hash); on restart the node reopens QMDB at the last commit, finds `F`
//!   among the stored blocks and re-executes everything above it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
use std::sync::Arc;

use agent_types::block::{decode_header_with_proof, encode_header_with_proof};
use agent_types::{Block, Hash, Header, Pubkey, Tx, HEADER_VERSION};
use commonware_codec::RangeCfg;
use commonware_cryptography::blake3::Digest as B3;
use commonware_runtime::buffer::paged::CacheRef;
use commonware_runtime::Spawner;
use commonware_storage::archive::{prunable, Archive as _, Identifier, MultiArchive as _};
use commonware_storage::translator::EightCap;
use executor::{BlockEnv, BlockExecutor, BlockSummary};
use primitive_types::U256;
use state::config::Config;
use state::schema::{self, JobRecord, Withdrawal};
use state::{Snapshot, StateRead, Store, View};
use tracing::{debug, info, warn};

use crate::difficulty::{next_bits, nbits_to_target, work, Retarget};
use crate::verifier::PowVerifier;

pub type BlockArchive<E> = prunable::Archive<EightCap, E, B3, Vec<u8>>;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ChainError {
    /// The block or header is invalid (a peer sending it is misbehaving).
    #[error("invalid: {0}")]
    Invalid(String),
    /// It would reorg below the finalized block.
    #[error("refused: fork below the finalized block (reorg deeper than K)")]
    TooDeep,
    #[error("internal: {0}")]
    Internal(String),
}

/// The one context-dependent "invalid" reason: retried later, never cached or banned for.
pub const TIME_TOO_NEW: &str = "timestamp too far in the future";

/// Is this rejection message the future-timestamp one?
pub fn is_time_too_new(msg: &str) -> bool {
    msg.contains(TIME_TOO_NEW)
}

fn invalid<T>(m: impl Into<String>) -> Result<T, ChainError> {
    Err(ChainError::Invalid(m.into()))
}

fn internal(e: impl std::fmt::Display) -> ChainError {
    ChainError::Internal(e.to_string())
}

/// Genesis parameters (identical on every node; the genesis hash is derived from them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genesis {
    pub chain_id: u32,
    pub timestamp_ms: u64,
    pub target_bits: u32,
    pub config: Config,
}

pub struct LiveNode {
    pub header: Header,
    pub hash: Hash,
    pub cum_work: U256,
    pub snapshot: Snapshot,
    pub summary: BlockSummary,
    pub txs: Arc<Vec<Tx>>,
    /// Post-state parameters (they govern this block's children).
    pub config: Config,
    /// Withdrawal-log size before this block (locations of its withdrawals follow it).
    pub log_base: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct CanonEntry {
    pub hash: Hash,
    pub timestamp_ms: u64,
    pub target_bits: u32,
    pub cum_work: U256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainEvent {
    NewTip { hash: Hash, height: u64, reorg_depth: u64, disconnected: Vec<Hash>, connected: Vec<Hash> },
    Finalized { hash: Hash, height: u64 },
    WithdrawalFinal(Withdrawal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accept {
    /// Executed and added to the tree (maybe with orphans that it connected).
    Added { tip: bool },
    Known,
    /// Stored until its parent arrives.
    Orphan { missing: Hash },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderAccept {
    Known,
    /// Header (and proof) checked as far as possible without the parent state.
    Stored { need_parent: Option<Hash> },
}

/// Which work a template is built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobChoice {
    /// The mineable job with the largest remaining cap, else a fallback block.
    Auto,
    Fallback,
    Job(Hash),
}

#[derive(Debug, Clone)]
pub struct Template {
    /// Final except `nonce` and `proof_hash` (zero); the miner may change `timestamp_ms`.
    pub header: Header,
    pub txs: Vec<Tx>,
    pub job: Option<JobRecord>,
    pub summary: BlockSummary,
    pub fallback: pearl_job::pow::FallbackParams,
    pub config: Config,
}

const MTP_SPAN: usize = 11;
const MAX_ORPHANS: usize = 4096;
/// Headers waiting for their bodies (headers-first sync walks back from a peer's tip to the
/// node's chain, so a fresh node holds one per block of history). Only the 256-byte header is
/// kept (the proof comes again with the body and is verified then), so this is about 80 MB at
/// the cap.
const MAX_HEADERS_ONLY: usize = 1 << 18;
/// Most bodies headers-first sync asks for at once, lowest height first and at most this far
/// above the tip. Bodies whose parent has not connected wait in the orphan pool, so this stays
/// well under [`MAX_ORPHANS`] (2026-10-21: asking for every body at once overflowed the pool on
/// a chain longer than it, and a fresh devnet node stopped at height 1).
pub const BODY_WINDOW: usize = 512;
/// Byte budget of the orphan pool (security pass 2026-10-21). An orphan's work cannot be
/// checked before its parent connects, so any peer can send them for free; without a budget a
/// peer could park MAX_ORPHANS blocks of up to MAX_BLOCK_SIZE (16 MiB) at heights finality
/// never prunes and leave the pool full for honest bodies. Devnet blocks are about 25 KB and
/// sync keeps at most BODY_WINDOW bodies in flight, so honest use stays far below it. Over its
/// count or bytes the pool drops its oldest orphan, and that orphan's header goes back to the
/// body queue so its body is asked for again.
pub const MAX_ORPHAN_BYTES: usize = 256 << 20;
/// Newest jobs an auto-choice block template looks at (node policy, not consensus). It must
/// exceed the open jobs of the busiest moment: one 8B round alone is 640 segment jobs, and at
/// 512 the oldest segments of a round were never offered (scripts/serve8b run, 2026-10-21).
pub const TEMPLATE_JOB_SCAN: u64 = 4096;

pub struct Chain<E: commonware_storage::Context + Spawner, V: PowVerifier> {
    store: Store<E>,
    archive: Option<BlockArchive<E>>,
    verifier: V,
    genesis: Genesis,
    genesis_hash: Hash,
    live: HashMap<Hash, LiveNode>,
    finalized: Hash,
    tip: Hash,
    canon: Vec<CanonEntry>,
    orphans: HashMap<Hash, Block>,
    orphans_by_parent: HashMap<Hash, Vec<Hash>>,
    headers_only: HashMap<Hash, Header>,
    /// Header-only entries whose body has not been asked for yet, by height.
    body_queue: BTreeSet<(u64, Hash)>,
    /// Header-only entries whose body was asked for ([`Chain::next_bodies`]).
    body_asked: HashSet<Hash>,
    /// Insertion order and bytes of `orphans`, insertion order of `headers_only` (budgets:
    /// over a cap the oldest entry is dropped instead of refusing new ones, so junk cannot
    /// leave either buffer full for honest entries).
    orphan_order: VecDeque<Hash>,
    orphan_bytes: usize,
    header_order: VecDeque<Hash>,
    invalid: HashSet<Hash>,
    /// Blocks refused for forking below F (their descendants are refused too).
    too_deep: HashSet<Hash>,
    tx_index: HashMap<Hash, (Hash, u32)>,
    withdraw_loc: BTreeMap<u64, u64>,
    /// Withdrawal-log size (operations, a commit boundary) after each FINAL block that
    /// appended withdrawals, by height. The log size at any final height H is the value
    /// of the last key <= H; historical proofs (getWithdrawalProof at_height) use it.
    log_size_at: BTreeMap<u64, u64>,
    /// Per-block rewards (`fees u64 || subsidy u64 || matured u64`), every connected
    /// block on any branch, keyed by hash (node-local index, not consensus state).
    rewards: Option<BlockArchive<E>>,
    /// Account history over FINAL canonical blocks: `(height, tx index)` ascending.
    /// Extended when blocks finalize (they never reorg); the non-final part of a
    /// query is read from the live canonical chain, so reorgs need no undo.
    history: HashMap<Pubkey, Vec<(u64, u32)>>,
    /// Last ClearRoute naming each account over FINAL canonical blocks: `(height, tx
    /// index, route_nonce)`. Same rule as `history`: extended at finalization, the live
    /// part is read from the canonical branch at query time.
    clears: HashMap<Pubkey, (u64, u32, u64)>,
}

/// Fees, subsidy and matured coinbase of one block (getBlock).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rewards {
    pub fees: u64,
    pub subsidy: u64,
    pub matured: u64,
}

impl Rewards {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(24);
        v.extend_from_slice(&self.fees.to_le_bytes());
        v.extend_from_slice(&self.subsidy.to_le_bytes());
        v.extend_from_slice(&self.matured.to_le_bytes());
        v
    }
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != 24 {
            return None;
        }
        let u = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        Some(Self { fees: u(0), subsidy: u(8), matured: u(16) })
    }
}

/// The block and position of a route tx (getRoute, SPEC 4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteTxEntry {
    pub block_hash: Hash,
    pub height: u64,
    pub index: u32,
    pub route_nonce: u64,
}

fn clear_of(t: &Tx) -> Option<(Pubkey, u64)> {
    match &t.body {
        agent_types::TxBody::ClearRoute { account, route_nonce, .. } => Some((*account, *route_nonce)),
        _ => None,
    }
}

/// One account-history entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    pub block_hash: Hash,
    pub height: u64,
    pub index: u32,
}

/// Accounts a tx touches, as far as the tx bytes say: the sender, a Transfer's
/// recipient, a BridgeUnlock's recipient, a RejectResult's deliverer, a Follow's targets,
/// an OperatorLink's agent, a DM's recipient, a ChannelMember's named member (job payouts,
/// coinbase and tip recipients are not listed).
pub fn tx_accounts(t: &Tx) -> Vec<Pubkey> {
    let mut v = vec![t.from];
    match &t.body {
        agent_types::TxBody::Transfer { to, .. } => v.push(*to),
        agent_types::TxBody::BridgeUnlock { recipient, .. } => v.push(*recipient),
        agent_types::TxBody::ClearRoute { account, .. } => v.push(*account),
        agent_types::TxBody::RejectResult { deliverer, .. } => v.push(*deliverer),
        agent_types::TxBody::Follow { items } => v.extend(items.iter().map(|x| x.target)),
        agent_types::TxBody::OperatorLink { agent, .. } => v.push(*agent),
        agent_types::TxBody::SendMessage(m) if m.to != [0; 32] => v.push(m.to),
        agent_types::TxBody::ChannelMember { member, .. } if *member != [0; 32] => v.push(*member),
        _ => {}
    }
    v.dedup();
    v
}

fn genesis_header(g: &Genesis, state_root: Hash, withdraw_root: Hash) -> Header {
    Header {
        version: HEADER_VERSION,
        height: 0,
        prev_hash: [0; 32],
        timestamp_ms: g.timestamp_ms,
        target_bits: g.target_bits,
        job_id: [0; 32],
        miner: [0; 32],
        // Genesis has no txs; its tx_root commits to the genesis spec instead.
        tx_root: genesis_spec_digest(g),
        state_root,
        withdraw_root,
        proof_hash: agent_types::block::proof_hash(&[]),
        nonce: 0,
    }
}

/// `blake3("pearl-sol:genesis:v1" || chain_id u32 || timestamp_ms u64 || target_bits u32 || config)`.
pub fn genesis_spec_digest(g: &Genesis) -> Hash {
    agent_types::blake3_hash(&[
        b"pearl-sol:genesis:v1",
        &g.chain_id.to_le_bytes(),
        &g.timestamp_ms.to_le_bytes(),
        &g.target_bits.to_le_bytes(),
        &g.config.encode(),
    ])
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Chain<E, V> {
    /// Open (or create) the chain under `prefix`.
    pub async fn open(context: E, prefix: &str, genesis: Genesis, verifier: V) -> Result<Self, ChainError> {
        if nbits_to_target(genesis.target_bits).is_zero() {
            return invalid("genesis target_bits");
        }
        let store = Store::open(context.child("state"), prefix).await.map_err(internal)?;
        let archive_cfg = prunable::Config {
            translator: EightCap,
            metadata_partition: format!("{prefix}-blocks-meta"),
            key_partition: format!("{prefix}-blocks-key"),
            key_page_cache: CacheRef::from_pooler(&context, NonZeroU16::new(4096).unwrap(), NonZeroUsize::new(64).unwrap()),
            value_partition: format!("{prefix}-blocks-value"),
            compression: None,
            codec_config: (RangeCfg::from(0..=agent_types::block::MAX_BLOCK_SIZE), ()),
            items_per_section: NonZeroU64::new(4096).unwrap(),
            key_write_buffer: NonZeroUsize::new(1 << 16).unwrap(),
            value_write_buffer: NonZeroUsize::new(1 << 20).unwrap(),
            replay_buffer: NonZeroUsize::new(1 << 20).unwrap(),
        };
        let archive = BlockArchive::init(context.child("blocks"), archive_cfg).await.map_err(internal)?;
        let rewards_cfg = prunable::Config {
            translator: EightCap,
            metadata_partition: format!("{prefix}-rewards-meta"),
            key_partition: format!("{prefix}-rewards-key"),
            key_page_cache: CacheRef::from_pooler(&context, NonZeroU16::new(4096).unwrap(), NonZeroUsize::new(16).unwrap()),
            value_partition: format!("{prefix}-rewards-value"),
            compression: None,
            codec_config: (RangeCfg::from(0..=64usize), ()),
            items_per_section: NonZeroU64::new(4096).unwrap(),
            key_write_buffer: NonZeroUsize::new(1 << 12).unwrap(),
            value_write_buffer: NonZeroUsize::new(1 << 12).unwrap(),
            replay_buffer: NonZeroUsize::new(1 << 16).unwrap(),
        };
        let rewards = BlockArchive::init(context.child("rewards"), rewards_cfg).await.map_err(internal)?;
        let committed_height = store.committed_height().await.map_err(internal)?;

        // The genesis hash is a pure function of the genesis spec.
        let gws = executor::genesis_writes(&genesis.config).map_err(ChainError::Invalid)?;
        let empty = store.committed().clone();

        let mut chain = Self {
            store,
            archive: Some(archive),
            verifier,
            genesis: genesis.clone(),
            genesis_hash: [0; 32],
            live: HashMap::new(),
            finalized: [0; 32],
            tip: [0; 32],
            canon: Vec::new(),
            orphans: HashMap::new(),
            orphans_by_parent: HashMap::new(),
            headers_only: HashMap::new(),
            body_queue: BTreeSet::new(),
            body_asked: HashSet::new(),
            orphan_order: VecDeque::new(),
            orphan_bytes: 0,
            header_order: VecDeque::new(),
            invalid: HashSet::new(),
            too_deep: HashSet::new(),
            tx_index: HashMap::new(),
            withdraw_loc: BTreeMap::new(),
            log_size_at: BTreeMap::new(),
            rewards: Some(rewards),
            history: HashMap::new(),
            clears: HashMap::new(),
        };

        match committed_height {
            None => {
                let snap = chain.store.build(&empty, &gws, 0).await.map_err(internal)?;
                let header = genesis_header(&genesis, snap.state_root, snap.withdraw_root);
                let hash = header.hash();
                chain.store.finalize(&snap).await.map_err(internal)?;
                let block = Block { header, proof: Vec::new(), txs: Vec::new(), coinbase: None };
                chain.archive_put(&block).await?;
                chain.genesis_hash = hash;
                chain.install_root(block, snap, U256::zero()).await?;
                info!(genesis = %hex(&hash), "created genesis");
            }
            Some(hc) => chain.restore(hc).await?,
        }
        Ok(chain)
    }

    async fn install_root(&mut self, block: Block, snap: Snapshot, cum_work: U256) -> Result<(), ChainError> {
        let hash = block.hash();
        let config = self.read_config(&snap).await?;
        self.canon.truncate(block.header.height as usize);
        self.canon.push(CanonEntry {
            hash,
            timestamp_ms: block.header.timestamp_ms,
            target_bits: block.header.target_bits,
            cum_work,
        });
        self.live.insert(
            hash,
            LiveNode {
                header: block.header,
                hash,
                cum_work,
                log_base: snap.log_size,
                snapshot: snap,
                summary: BlockSummary::default(),
                txs: Arc::new(block.txs),
                config,
            },
        );
        self.finalized = hash;
        self.tip = hash;
        Ok(())
    }

    async fn restore(&mut self, hc: u64) -> Result<(), ChainError> {
        // Load every stored block.
        let archive = self.archive.as_ref().unwrap();
        let ranges: Vec<(u64, u64)> = archive.ranges().collect();
        let mut by_hash: HashMap<Hash, Block> = HashMap::new();
        let mut heights: BTreeMap<u64, Vec<Hash>> = BTreeMap::new();
        for (lo, hi) in ranges {
            for h in lo..=hi {
                if let Some(vals) = archive.get_all(h).await.map_err(internal)? {
                    for v in vals {
                        let b = Block::decode(&v).map_err(internal)?;
                        heights.entry(b.header.height).or_default().push(b.hash());
                        by_hash.insert(b.hash(), b);
                    }
                }
            }
        }
        let committed = self.store.committed().clone();
        let f_hash = heights
            .get(&hc)
            .and_then(|hs| {
                hs.iter().copied().find(|h| {
                    let hd = &by_hash[h].header;
                    hd.state_root == committed.state_root && hd.withdraw_root == committed.withdraw_root
                })
            })
            .ok_or_else(|| internal(format!("no stored block at committed height {hc} matches the committed roots")))?;
        // Canonical chain down to genesis.
        let mut path = vec![f_hash];
        while by_hash[path.last().unwrap()].header.height > 0 {
            let p = by_hash[path.last().unwrap()].header.prev_hash;
            if !by_hash.contains_key(&p) {
                return Err(internal("stored chain is missing an ancestor of the finalized block"));
            }
            path.push(p);
        }
        path.reverse();
        let g = &by_hash[&path[0]].header;
        let expect = genesis_header(&self.genesis, g.state_root, g.withdraw_root);
        if expect != *g {
            return Err(internal("stored genesis does not match this genesis spec"));
        }
        self.genesis_hash = path[0];
        let mut cw = U256::zero();
        for (i, h) in path.iter().enumerate() {
            let b = &by_hash[h];
            if i > 0 {
                cw = cw.saturating_add(work(b.header.target_bits));
            }
            self.canon.push(CanonEntry { hash: *h, timestamp_ms: b.header.timestamp_ms, target_bits: b.header.target_bits, cum_work: cw });
            for (j, t) in b.txs.iter().enumerate() {
                self.tx_index.insert(t.hash(), (*h, j as u32));
                for a in tx_accounts(t) {
                    self.history.entry(a).or_default().push((b.header.height, j as u32));
                }
                if let Some((a, rn)) = clear_of(t) {
                    self.clears.insert(a, (b.header.height, j as u32, rn));
                }
            }
        }
        let fb = by_hash[&f_hash].clone();
        self.canon.pop();
        self.install_root(fb, committed, cw).await?;
        // Withdrawal locations in the committed log.
        for loc in 0..self.store.log_size() {
            if let Some(e) = self.store.log_get(loc).await.map_err(internal)? {
                if let Ok(w) = Withdrawal::from_log_entry(&e) {
                    self.withdraw_loc.insert(w.nonce, loc);
                    // A block's entries are followed by its commit op: size = last loc + 2.
                    self.log_size_at.insert(w.height, loc + 2);
                }
            }
        }
        // Re-execute everything above F.
        let now = u64::MAX / 2;
        let mut replayed = 0;
        for (_, hs) in heights.range(hc + 1..) {
            for h in hs {
                let b = by_hash[h].clone();
                match self.insert_block(b, now).await {
                    Ok(_) => replayed += 1,
                    Err(e) => debug!(block = %hex(h), "stored block not replayed: {e}"),
                }
            }
        }
        info!(finalized = hc, replayed, tip = self.tip_height(), "restored chain");
        Ok(())
    }

    async fn archive_put(&mut self, b: &Block) -> Result<(), ChainError> {
        let a = self.archive.take().unwrap();
        let key = B3(b.hash());
        let present = a.has_at(b.header.height, &key).await.map_err(internal)?;
        let a = if present { a } else { a.put_multi_sync(b.header.height, key, b.encode()).await.map_err(internal)? };
        self.archive = Some(a);
        Ok(())
    }

    async fn rewards_put(&mut self, hash: &Hash, height: u64, r: &Rewards) -> Result<(), ChainError> {
        let a = self.rewards.take().unwrap();
        let key = B3(*hash);
        let present = a.has_at(height, &key).await.map_err(internal)?;
        let a = if present { a } else { a.put_multi_sync(height, key, r.encode()).await.map_err(internal)? };
        self.rewards = Some(a);
        Ok(())
    }

    /// Rewards of any block this node connected (`None` for blocks stored before the
    /// rewards index existed). Genesis has none.
    pub async fn block_rewards(&self, h: &Hash) -> Result<Option<Rewards>, ChainError> {
        if *h == self.genesis_hash {
            return Ok(Some(Rewards::default()));
        }
        if let Some(n) = self.live.get(h) {
            if n.header.height > 0 && *h != self.finalized {
                return Ok(Some(Rewards { fees: n.summary.fees, subsidy: n.summary.subsidy, matured: n.summary.matured }));
            }
        }
        let a = self.rewards.as_ref().unwrap();
        Ok(a.get(Identifier::Key(&B3(*h))).await.map_err(internal)?.and_then(|v| Rewards::decode(&v)))
    }

    /// Txs touching `account` on the canonical chain, newest first, strictly before the
    /// cursor `before = (height, index)` when given. Returns up to `limit` entries and
    /// whether more exist.
    pub fn account_txs(&self, account: &Pubkey, before: Option<(u64, u32)>, limit: usize) -> (Vec<HistoryEntry>, bool) {
        let mut out = Vec::new();
        let ok = |pos: (u64, u32)| before.is_none_or(|b| pos < b);
        let f = self.finalized().header.height;
        // Live (non-final) canonical part, read from the current branch.
        for height in (f + 1..=self.tip_height()).rev() {
            let Some(c) = self.canon.get(height as usize) else { continue };
            let Some(n) = self.live.get(&c.hash) else { continue };
            for (i, t) in n.txs.iter().enumerate().rev() {
                if ok((height, i as u32)) && tx_accounts(t).contains(account) {
                    if out.len() == limit {
                        return (out, true);
                    }
                    out.push(HistoryEntry { block_hash: c.hash, height, index: i as u32 });
                }
            }
        }
        if let Some(hist) = self.history.get(account) {
            for &(height, index) in hist.iter().rev() {
                if !ok((height, index)) {
                    continue;
                }
                if out.len() == limit {
                    return (out, true);
                }
                let Some(c) = self.canon.get(height as usize) else { continue };
                out.push(HistoryEntry { block_hash: c.hash, height, index });
            }
        }
        (out, false)
    }

    /// The last ClearRoute naming `account` on the canonical chain (reorg safe: the
    /// non-final part is read from the current branch).
    pub fn last_clear(&self, account: &Pubkey) -> Option<RouteTxEntry> {
        let f = self.finalized().header.height;
        for height in (f + 1..=self.tip_height()).rev() {
            let Some(c) = self.canon.get(height as usize) else { continue };
            let Some(n) = self.live.get(&c.hash) else { continue };
            for (i, t) in n.txs.iter().enumerate().rev() {
                if let Some((a, rn)) = clear_of(t) {
                    if a == *account {
                        return Some(RouteTxEntry { block_hash: c.hash, height, index: i as u32, route_nonce: rn });
                    }
                }
            }
        }
        let &(height, index, route_nonce) = self.clears.get(account)?;
        let c = self.canon.get(height as usize)?;
        Some(RouteTxEntry { block_hash: c.hash, height, index, route_nonce })
    }

    /// The SetRoute that wrote `account`'s current route record: the last SetRoute sent
    /// by `account` in the canonical block at `set_height` (the record's `set_height`).
    pub async fn route_set_tx(&self, account: &Pubkey, set_height: u64, route_nonce: u64) -> Result<Option<RouteTxEntry>, ChainError> {
        let Some(hash) = self.canonical_hash(set_height) else { return Ok(None) };
        let txs: Arc<Vec<Tx>> = match self.live.get(&hash) {
            Some(n) => n.txs.clone(),
            None => match self.get_block(&hash).await? {
                Some(b) => Arc::new(b.txs),
                None => return Ok(None),
            },
        };
        let idx = txs.iter().rposition(|t| t.from == *account && matches!(t.body, agent_types::TxBody::SetRoute { .. }));
        Ok(idx.map(|i| RouteTxEntry { block_hash: hash, height: set_height, index: i as u32, route_nonce }))
    }

    async fn read_config(&self, snap: &Snapshot) -> Result<Config, ChainError> {
        let v = self.store.get(snap, &schema::config_key()).await.map_err(internal)?.ok_or_else(|| internal("no config"))?;
        Config::decode(&v).map_err(internal)
    }

    // ------------------------------------------------------------------ queries

    pub fn genesis_hash(&self) -> Hash {
        self.genesis_hash
    }
    pub fn chain_id(&self) -> u32 {
        self.genesis.chain_id
    }
    pub fn tip(&self) -> &LiveNode {
        &self.live[&self.tip]
    }
    pub fn tip_height(&self) -> u64 {
        self.tip().header.height
    }
    pub fn finalized(&self) -> &LiveNode {
        &self.live[&self.finalized]
    }
    pub fn live_node(&self, h: &Hash) -> Option<&LiveNode> {
        self.live.get(h)
    }
    pub fn live_count(&self) -> usize {
        self.live.len()
    }
    pub fn store(&self) -> &Store<E> {
        &self.store
    }
    pub fn canonical_hash(&self, height: u64) -> Option<Hash> {
        self.canon.get(height as usize).map(|c| c.hash)
    }
    pub fn canon_entry(&self, height: u64) -> Option<&CanonEntry> {
        self.canon.get(height as usize)
    }
    pub fn is_canonical(&self, h: &Hash, height: u64) -> bool {
        self.canonical_hash(height) == Some(*h)
    }
    /// Confirmations of a canonical block (1 = the tip itself), 0 if not canonical.
    pub fn confirmations(&self, h: &Hash, height: u64) -> u64 {
        if self.is_canonical(h, height) { self.tip_height() - height + 1 } else { 0 }
    }
    pub fn finality_k(&self) -> u64 {
        self.tip().config.finality_k as u64
    }
    pub fn is_final(&self, h: &Hash, height: u64) -> bool {
        self.is_canonical(h, height) && height <= self.finalized().header.height
    }
    pub fn tx_location(&self, tx: &Hash) -> Option<(Hash, u32)> {
        self.tx_index.get(tx).copied()
    }
    pub fn withdrawal_location(&self, nonce: u64) -> Option<u64> {
        self.withdraw_loc.get(&nonce).copied()
    }
    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }
    pub fn is_known(&self, h: &Hash) -> bool {
        self.live.contains_key(h)
            || self.orphans.contains_key(h)
            || self.headers_only.contains_key(h)
            || self.canon.iter().rev().take(4096).any(|c| c.hash == *h)
    }
    /// Header-only entries waiting for a body.
    pub fn wanted_bodies(&self) -> Vec<Hash> {
        self.headers_only.keys().copied().collect()
    }

    /// Bodies to ask peers for now (headers-first sync): header-only entries not asked for yet,
    /// lowest height first, at most `window` above the tip and at most `window` outstanding.
    /// Each hash is handed out once; a body that could not be kept is handed out again.
    pub fn next_bodies(&mut self, window: usize) -> Vec<Hash> {
        let limit = self.tip_height().saturating_add(window as u64);
        let mut out = Vec::new();
        while self.body_asked.len() < window {
            let Some(&(height, hash)) = self.body_queue.first() else { break };
            if height > limit {
                break;
            }
            self.body_queue.pop_first();
            if self.headers_only.contains_key(&hash) {
                self.body_asked.insert(hash);
                out.push(hash);
            }
        }
        out
    }

    /// Bodies asked for and not yet received.
    pub fn bodies_outstanding(&self) -> usize {
        self.body_asked.len()
    }

    /// A header-only entry got its body (connected or orphaned) or was dropped.
    fn forget_header(&mut self, hash: &Hash) {
        if let Some(hd) = self.headers_only.remove(hash) {
            self.body_queue.remove(&(hd.height, *hash));
        }
        self.body_asked.remove(hash);
    }

    /// Read `key` in the tip's state.
    pub async fn read_tip(&self, key: &schema::Key) -> Result<Option<Vec<u8>>, ChainError> {
        self.store.get(&self.tip().snapshot, key).await.map_err(internal)
    }

    /// Read `key` in a live block's post-state.
    pub async fn read_at(&self, h: &Hash, key: &schema::Key) -> Result<Option<Vec<u8>>, ChainError> {
        let n = self.live.get(h).ok_or_else(|| internal("not a live block"))?;
        self.store.get(&n.snapshot, key).await.map_err(internal)
    }

    /// Stored block by hash (any branch the node accepted).
    pub async fn get_block(&self, h: &Hash) -> Result<Option<Block>, ChainError> {
        if let Some(b) = self.orphans.get(h) {
            return Ok(Some(b.clone()));
        }
        let a = self.archive.as_ref().unwrap();
        match a.get(Identifier::Key(&B3(*h))).await.map_err(internal)? {
            Some(v) => Ok(Some(Block::decode(&v).map_err(internal)?)),
            None => Ok(None),
        }
    }

    /// Header + proof for sync (live, final, orphan or header-only).
    pub async fn header_with_proof(&self, h: &Hash) -> Result<Option<Vec<u8>>, ChainError> {
        // header-only entries keep no proof: a peer asking gets it from a node with the block
        Ok(self.get_block(h).await?.map(|b| encode_header_with_proof(&b.header, &b.proof)))
    }

    // ------------------------------------------------------------------ ancestry

    /// Timestamps of `parent` and up to `n - 1` of its ancestors, newest first.
    fn ancestor_timestamps(&self, parent: &Hash, n: usize) -> Vec<u64> {
        let mut out = Vec::with_capacity(n);
        let mut cur = *parent;
        while out.len() < n {
            match self.live.get(&cur) {
                Some(node) => {
                    out.push(node.header.timestamp_ms);
                    if cur == self.finalized || node.header.height == 0 {
                        // continue on the final canonical chain
                        let mut h = node.header.height;
                        while out.len() < n && h > 0 {
                            h -= 1;
                            out.push(self.canon[h as usize].timestamp_ms);
                        }
                        break;
                    }
                    cur = node.header.prev_hash;
                }
                None => break,
            }
        }
        out
    }

    fn median_time_past(&self, parent: &Hash) -> u64 {
        let mut ts = self.ancestor_timestamps(parent, MTP_SPAN);
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    /// Required target for a child of `parent` (a live block).
    pub fn required_bits(&self, parent: &Hash) -> u32 {
        let p = &self.live[parent];
        let ts = self.ancestor_timestamps(parent, 2);
        let c = &p.config;
        next_bits(&Retarget {
            parent_bits: p.header.target_bits,
            parent_ts_ms: p.header.timestamp_ms,
            grandparent_ts_ms: if p.header.height == 0 { None } else { ts.get(1).copied() },
            target_block_time_ms: c.target_block_time_ms,
            half_life_ms: c.wtema_half_life_ms,
            pow_limit_bits: c.pow_limit_bits,
            min_difficulty_bits: c.min_difficulty_bits,
        })
    }

    /// Earliest valid timestamp for a child of `parent`.
    pub fn min_timestamp(&self, parent: &Hash) -> u64 {
        self.median_time_past(parent) + 1
    }

    fn is_descendant_of(&self, h: &Hash, anc: &Hash) -> bool {
        let Some(target) = self.live.get(anc).map(|n| n.header.height) else { return false };
        let mut cur = *h;
        loop {
            match self.live.get(&cur) {
                Some(n) if n.header.height > target => cur = n.header.prev_hash,
                Some(n) => return n.hash == *anc,
                None => return false,
            }
        }
    }

    // ------------------------------------------------------------------ headers-first

    /// Check a header + proof before its body is fetched. PoW is verified now for
    /// fallback blocks and for jobs the tip state knows; other job blocks are verified
    /// when their parent state exists (before execution).
    pub async fn insert_header(&mut self, header: Header, proof: Vec<u8>) -> Result<HeaderAccept, ChainError> {
        let hash = header.hash();
        if self.invalid.contains(&hash) {
            return invalid("known invalid");
        }
        if self.is_known(&hash) {
            return Ok(HeaderAccept::Known);
        }
        let f = self.finalized().header.height;
        if header.height <= f {
            self.too_deep.insert(hash);
            return Err(ChainError::TooDeep);
        }
        if agent_types::block::proof_hash(&proof) != header.proof_hash {
            return invalid("proof_hash mismatch");
        }
        let cfg = self.tip().config.clone();
        let t = nbits_to_target(header.target_bits);
        if t.is_zero() || t > nbits_to_target(cfg.pow_limit_bits) {
            return invalid("target_bits above pow limit");
        }
        if header.is_fallback() {
            self.check_pow(&header, None, &proof, &cfg)?;
        } else {
            // The job of a job block lives in its parent's state. With the parent live, check
            // the work now (a header naming a job its parent does not have is invalid); only
            // headers above unknown ancestry wait for their body (security pass 2026-10-21:
            // reading the tip state let anyone hang free headers off any live block).
            let job = match self.live.get(&header.prev_hash) {
                Some(p) => {
                    let snap = p.snapshot.clone();
                    let j = self.store.get(&snap, &schema::job_key(&header.job_id)).await.map_err(internal)?;
                    if j.is_none() {
                        self.invalid.insert(hash);
                        return invalid("job block names a job its parent does not have");
                    }
                    j
                }
                None => self.read_tip(&schema::job_key(&header.job_id)).await?,
            };
            if let Some(j) = job {
                let j = JobRecord::decode(&j).map_err(internal)?;
                let pj = j.pearl_job().map_err(ChainError::Invalid)?;
                self.check_pow(&header, Some(&pj), &proof, &cfg)?;
            }
        }
        let parent_known = self.live.contains_key(&header.prev_hash)
            || self.orphans.contains_key(&header.prev_hash)
            || self.headers_only.contains_key(&header.prev_hash);
        if self.too_deep.contains(&header.prev_hash) || (!parent_known && header.height - 1 <= f) {
            self.too_deep.insert(hash);
            return Err(ChainError::TooDeep);
        }
        while self.headers_only.len() >= MAX_HEADERS_ONLY {
            let Some(old) = self.header_order.pop_front() else { break };
            if self.headers_only.contains_key(&old) {
                debug!(header = %hex(&old), "header buffer full: dropped the oldest");
                self.forget_header(&old);
            }
        }
        self.header_order.push_back(hash);
        self.body_queue.insert((header.height, hash));
        self.headers_only.insert(hash, header);
        Ok(HeaderAccept::Stored { need_parent: if parent_known { None } else { Some(header.prev_hash) } })
    }

    fn check_pow(&mut self, h: &Header, job: Option<&pearl_job::Job>, proof: &[u8], cfg: &Config) -> Result<(), ChainError> {
        match self.verifier.verify(h, job, proof, &cfg.fallback_params()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.invalid.insert(h.hash());
                invalid(format!("bad proof of work: {e}"))
            }
        }
    }

    // ------------------------------------------------------------------ orphan budget

    fn orphan_size(b: &Block) -> usize {
        Header::SIZE + b.proof.len() + b.txs.iter().map(|t| t.encode().len() + 4).sum::<usize>() + 128
    }

    /// Bytes held by the orphan pool.
    pub fn orphan_bytes(&self) -> usize {
        self.orphan_bytes
    }

    /// Header-only entries held.
    pub fn headers_only_count(&self) -> usize {
        self.headers_only.len()
    }

    fn store_orphan(&mut self, hash: Hash, parent: Hash, block: Block) {
        let size = Self::orphan_size(&block);
        while !self.orphans.is_empty() && (self.orphans.len() >= MAX_ORPHANS || self.orphan_bytes + size > MAX_ORPHAN_BYTES) {
            let Some(old) = self.orphan_order.pop_front() else { break };
            if let Some(b) = self.take_orphan(&old) {
                debug!(orphan = %hex(&old), height = b.header.height, "orphan pool over budget: dropped the oldest");
                // its body is asked for again when the window reaches it
                if !self.headers_only.contains_key(&old) && self.headers_only.len() < MAX_HEADERS_ONLY {
                    self.header_order.push_back(old);
                    self.body_queue.insert((b.header.height, old));
                    self.headers_only.insert(old, b.header);
                }
            }
        }
        self.orphan_bytes += size;
        self.orphan_order.push_back(hash);
        self.orphans_by_parent.entry(parent).or_default().push(hash);
        self.orphans.insert(hash, block);
    }

    fn take_orphan(&mut self, h: &Hash) -> Option<Block> {
        let b = self.orphans.remove(h)?;
        self.orphan_bytes -= Self::orphan_size(&b);
        if let Some(v) = self.orphans_by_parent.get_mut(&b.header.prev_hash) {
            v.retain(|x| x != h);
            if v.is_empty() {
                self.orphans_by_parent.remove(&b.header.prev_hash);
            }
        }
        Some(b)
    }

    // ------------------------------------------------------------------ blocks

    /// Validate, execute and connect a block (plus any orphans it unblocks).
    pub async fn insert_block(&mut self, block: Block, now_ms: u64) -> Result<(Accept, Vec<ChainEvent>), ChainError> {
        let hash = block.hash();
        if self.invalid.contains(&hash) {
            return invalid("known invalid");
        }
        if self.live.contains_key(&hash) || self.canon.get(block.header.height as usize).map(|c| c.hash) == Some(hash) {
            return Ok((Accept::Known, Vec::new()));
        }
        if block.header.version != HEADER_VERSION {
            return invalid("header version");
        }
        // A body that does not match its header says nothing about the header.
        if let Err(e) = block.check_commitments() {
            return invalid(e);
        }
        if self.too_deep.contains(&block.header.prev_hash) {
            self.too_deep.insert(hash);
            return Err(ChainError::TooDeep);
        }
        let parent = block.header.prev_hash;
        if !self.live.contains_key(&parent) {
            let f = self.finalized().header.height;
            if block.header.height == 0 || block.header.height - 1 <= f {
                self.too_deep.insert(hash);
                return Err(ChainError::TooDeep);
            }
            self.forget_header(&hash);
            self.store_orphan(hash, parent, block);
            return Ok((Accept::Orphan { missing: parent }, Vec::new()));
        }
        let mut events = Vec::new();
        let tip = self.connect(block, now_ms, &mut events).await?;
        // Orphans waiting on this block (and their descendants).
        let mut queue: VecDeque<Hash> = VecDeque::from([hash]);
        let mut any_tip = tip;
        while let Some(p) = queue.pop_front() {
            for child in self.orphans_by_parent.remove(&p).unwrap_or_default() {
                if let Some(b) = self.take_orphan(&child) {
                    match self.connect(b, now_ms, &mut events).await {
                        Ok(t) => {
                            any_tip |= t;
                            queue.push_back(child);
                        }
                        Err(e) => warn!(block = %hex(&child), "orphan rejected: {e}"),
                    }
                }
            }
        }
        Ok((Accept::Added { tip: any_tip }, events))
    }

    /// Full validation and execution of a block whose parent is live.
    async fn connect(&mut self, block: Block, now_ms: u64, events: &mut Vec<ChainEvent>) -> Result<bool, ChainError> {
        let hash = block.hash();
        let h = block.header;
        let res = self.validate_and_execute(&block, now_ms).await;
        let (snap, summary, config) = match res {
            Ok(x) => x,
            Err(e) => {
                // A block "too far in the future" is not invalid for good: it becomes valid
                // as the clock advances. Caching it would leave a node whose clock runs a
                // little behind on its own fork for good (a miner could aim a timestamp at
                // that edge to split the network). Security pass 2026-10-21.
                if matches!(&e, ChainError::Invalid(m) if !is_time_too_new(m)) {
                    self.invalid.insert(hash);
                }
                return Err(e);
            }
        };
        let parent = &self.live[&h.prev_hash];
        let cum_work = parent.cum_work.saturating_add(work(h.target_bits));
        let log_base = parent.snapshot.log_size;
        self.archive_put(&block).await?;
        self.rewards_put(&hash, h.height, &Rewards { fees: summary.fees, subsidy: summary.subsidy, matured: summary.matured }).await?;
        self.forget_header(&hash);
        self.live.insert(
            hash,
            LiveNode { header: h, hash, cum_work, snapshot: snap, summary, txs: Arc::new(block.txs), config, log_base },
        );
        debug!(height = h.height, block = %hex(&hash), "connected");
        if cum_work > self.tip().cum_work {
            self.set_tip(hash, events).await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn validate_and_execute(&mut self, block: &Block, now_ms: u64) -> Result<(Snapshot, BlockSummary, Config), ChainError> {
        let h = &block.header;
        let parent = self.live.get(&h.prev_hash).expect("parent is live");
        let pcfg = parent.config.clone();
        if h.height != parent.header.height + 1 {
            return invalid("height is not parent + 1");
        }
        if h.timestamp_ms < self.min_timestamp(&h.prev_hash) {
            return invalid("timestamp not after median time past");
        }
        if h.timestamp_ms > now_ms.saturating_add(pcfg.max_future_ms) {
            return invalid(TIME_TOO_NEW);
        }
        let want = self.required_bits(&h.prev_hash);
        if h.target_bits != want {
            return invalid(format!("target_bits {:#x}, required {:#x}", h.target_bits, want));
        }
        if block.txs.len() > pcfg.max_block_txs as usize {
            return invalid("too many txs");
        }
        let parent_snap = self.live[&h.prev_hash].snapshot.clone();
        // Proof of work against the parent state's job.
        let job = if h.is_fallback() {
            None
        } else {
            let v = self.store.get(&parent_snap, &schema::job_key(&h.job_id)).await.map_err(internal)?;
            let Some(v) = v else { return invalid("job block names an unknown job") };
            Some(JobRecord::decode(&v).map_err(internal)?)
        };
        let pj = match &job {
            Some(j) => Some(j.pearl_job().map_err(ChainError::Invalid)?),
            None => None,
        };
        self.check_pow(h, pj.as_ref(), &block.proof, &pcfg)?;
        // Execute.
        let env = BlockEnv {
            chain_id: self.genesis.chain_id,
            height: h.height,
            miner: h.miner,
            job_id: if h.is_fallback() { None } else { Some(h.job_id) },
        };
        let view = View { store: &self.store, snap: &parent_snap };
        let (ws, summary) = executor::execute_block(&view, env, &block.txs).await.map_err(|e| match e {
            executor::ExecError::State(s) => internal(s),
            other => ChainError::Invalid(other.to_string()),
        })?;
        let snap = self.store.build(&parent_snap, &ws, h.height).await.map_err(internal)?;
        if snap.state_root != h.state_root {
            return invalid("state_root mismatch");
        }
        if snap.withdraw_root != h.withdraw_root {
            return invalid("withdraw_root mismatch");
        }
        // The body's receipt is committed by tx_root (check_commitments); it must be
        // the one execution produced.
        if block.coinbase != Some(summary.coinbase) {
            return invalid("coinbase receipt mismatch");
        }
        let config = self.read_config(&snap).await?;
        Ok((snap, summary, config))
    }

    async fn set_tip(&mut self, new: Hash, events: &mut Vec<ChainEvent>) -> Result<(), ChainError> {
        let old = self.tip;
        // Fork point.
        let (mut a, mut b) = (old, new);
        let mut disconnected = Vec::new();
        let mut connected = Vec::new();
        while self.live[&a].header.height > self.live[&b].header.height {
            disconnected.push(a);
            a = self.live[&a].header.prev_hash;
        }
        while self.live[&b].header.height > self.live[&a].header.height {
            connected.push(b);
            b = self.live[&b].header.prev_hash;
        }
        while a != b {
            disconnected.push(a);
            connected.push(b);
            a = self.live[&a].header.prev_hash;
            b = self.live[&b].header.prev_hash;
        }
        connected.reverse();
        let fork_height = self.live[&a].header.height;
        let reorg_depth = self.live[&old].header.height - fork_height;
        // Canonical index and tx index.
        for d in &disconnected {
            for t in self.live[d].txs.iter() {
                self.tx_index.remove(&t.hash());
            }
        }
        self.canon.truncate(fork_height as usize + 1);
        for c in &connected {
            let n = &self.live[c];
            self.canon.push(CanonEntry {
                hash: *c,
                timestamp_ms: n.header.timestamp_ms,
                target_bits: n.header.target_bits,
                cum_work: n.cum_work,
            });
            for (i, t) in n.txs.iter().enumerate() {
                self.tx_index.insert(t.hash(), (*c, i as u32));
            }
        }
        self.tip = new;
        if reorg_depth > 0 {
            info!(depth = reorg_depth, new_tip = %hex(&new), height = self.tip_height(), "reorg");
        }
        events.push(ChainEvent::NewTip { hash: new, height: self.tip_height(), reorg_depth, disconnected, connected });
        self.advance_finality(events).await
    }

    async fn advance_finality(&mut self, events: &mut Vec<ChainEvent>) -> Result<(), ChainError> {
        let k = self.tip().config.finality_k as u64;
        let tip_h = self.tip_height();
        let f_h = self.finalized().header.height;
        if tip_h < k || tip_h - k <= f_h {
            return Ok(());
        }
        let target_h = tip_h - k;
        let new_f = self.canon[target_h as usize].hash;
        // Apply every newly final block in order (the snapshot of new_f includes them;
        // one commit is enough, but events are per block).
        let snap = self.live[&new_f].snapshot.clone();
        self.store.finalize(&snap).await.map_err(internal)?;
        for height in f_h + 1..=target_h {
            let hh = self.canon[height as usize].hash;
            let n = &self.live[&hh];
            for (i, w) in n.summary.withdrawals.iter().enumerate() {
                self.withdraw_loc.insert(w.nonce, n.log_base + i as u64);
                events.push(ChainEvent::WithdrawalFinal(*w));
            }
            if !n.summary.withdrawals.is_empty() {
                self.log_size_at.insert(height, n.snapshot.log_size);
            }
            for (i, t) in n.txs.iter().enumerate() {
                for a in tx_accounts(t) {
                    self.history.entry(a).or_default().push((height, i as u32));
                }
                if let Some((a, rn)) = clear_of(t) {
                    self.clears.insert(a, (height, i as u32, rn));
                }
            }
            events.push(ChainEvent::Finalized { hash: hh, height });
        }
        self.finalized = new_f;
        // Drop everything that does not descend from the new F, and the old canonical
        // blocks below it (they are final and served from the archive).
        let keep: HashSet<Hash> =
            self.live.keys().copied().filter(|h| *h == new_f || self.is_descendant_of(h, &new_f)).collect();
        self.live.retain(|h, _| keep.contains(h));
        self.orphans.retain(|_, b| b.header.height > target_h);
        let orphans = &self.orphans;
        self.orphans_by_parent.retain(|_, v| {
            v.retain(|h| orphans.contains_key(h));
            !v.is_empty()
        });
        self.headers_only.retain(|_, hd| hd.height > target_h);
        self.body_queue.retain(|(h, _)| *h > target_h);
        let headers = &self.headers_only;
        self.body_asked.retain(|h| headers.contains_key(h));
        self.orphan_bytes = self.orphans.values().map(Self::orphan_size).sum();
        let o = &self.orphans;
        self.orphan_order.retain(|h| o.contains_key(h));
        let hd = &self.headers_only;
        self.header_order.retain(|h| hd.contains_key(h));
        if self.invalid.len() > 100_000 {
            self.invalid.clear();
        }
        if self.too_deep.len() > 100_000 {
            self.too_deep.clear();
        }
        Ok(())
    }

    // ------------------------------------------------------------------ templates

    /// Open jobs a block on the tip could mine, newest `scan` jobs only.
    pub async fn mineable_jobs(&self, scan: u64) -> Result<Vec<JobRecord>, ChainError> {
        self.mineable_jobs_at(&self.tip, scan).await
    }

    pub async fn mineable_jobs_at(&self, parent: &Hash, scan: u64) -> Result<Vec<JobRecord>, ChainError> {
        let next_h = self.live.get(parent).ok_or_else(|| internal("not live"))?.header.height + 1;
        let c = match self.read_at(parent, &schema::counters_key()).await? {
            Some(v) => schema::Counters::decode(&v).map_err(internal)?,
            None => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for i in c.job_count.saturating_sub(scan)..c.job_count {
            let Some(id) = self.read_at(parent, &schema::job_index_key(i)).await? else { continue };
            let id = schema::decode_job_index(&id).map_err(internal)?;
            if let Some(v) = self.read_at(parent, &schema::job_key(&id)).await? {
                let j = JobRecord::decode(&v).map_err(internal)?;
                if executor::check_mineable(&j, next_h).is_ok() && j.pearl_job().is_ok() {
                    out.push(j);
                }
            }
        }
        Ok(out)
    }

    /// Build a block template on the tip. `candidates` are mempool txs in priority order;
    /// failing ones are skipped.
    pub async fn build_template(
        &self,
        miner: Pubkey,
        choice: JobChoice,
        candidates: &[Tx],
        now_ms: u64,
    ) -> Result<Template, ChainError> {
        self.build_template_at(self.tip, miner, choice, candidates, now_ms).await
    }

    /// Template on any live block (tests use this to build competing branches).
    pub async fn build_template_at(
        &self,
        parent: Hash,
        miner: Pubkey,
        choice: JobChoice,
        candidates: &[Tx],
        now_ms: u64,
    ) -> Result<Template, ChainError> {
        self.build_template_filtered(parent, miner, choice, candidates, now_ms, &|_| true).await
    }

    /// Template on `parent`, offering only jobs for which `available` holds (the node
    /// uses it to skip jobs whose data it does not hold; consensus does not care). A
    /// named job that is not available is an error.
    pub async fn build_template_filtered(
        &self,
        parent: Hash,
        miner: Pubkey,
        choice: JobChoice,
        candidates: &[Tx],
        now_ms: u64,
        available: &(dyn Fn(&JobRecord) -> bool + Sync),
    ) -> Result<Template, ChainError> {
        let tip = self.live.get(&parent).ok_or_else(|| ChainError::Invalid("parent is not a live block".into()))?;
        let height = tip.header.height + 1;
        let job = match choice {
            JobChoice::Fallback => None,
            JobChoice::Job(id) => {
                let v = self.read_at(&parent, &schema::job_key(&id)).await?.ok_or_else(|| ChainError::Invalid("unknown job".into()))?;
                let j = JobRecord::decode(&v).map_err(internal)?;
                executor::check_mineable(&j, height).map_err(ChainError::Invalid)?;
                if !available(&j) {
                    return Err(ChainError::Invalid(format!(
                        "job data {} is not available on this node yet",
                        String::from_utf8_lossy(&j.data_uri)
                    )));
                }
                Some(j)
            }
            JobChoice::Auto => {
                let mut jobs = self.mineable_jobs_at(&parent, TEMPLATE_JOB_SCAN).await?;
                jobs.retain(|j| available(j));
                jobs.sort_by(|a, b| b.cap_remaining.cmp(&a.cap_remaining).then(a.index.cmp(&b.index)));
                jobs.into_iter().next()
            }
        };
        let env = BlockEnv {
            chain_id: self.genesis.chain_id,
            height,
            miner,
            job_id: job.as_ref().map(|j| j.job_id),
        };
        let view = View { store: &self.store, snap: &tip.snapshot };
        let mut ex = BlockExecutor::begin(&view, env).await.map_err(|e| ChainError::Invalid(e.to_string()))?;
        let max = tip.config.max_block_txs as usize;
        let mut txs = Vec::new();
        for t in candidates {
            if txs.len() >= max {
                break;
            }
            if ex.apply_tx(t).await.is_ok() {
                txs.push(t.clone());
            }
        }
        let (ws, summary) = ex.finish(true).await.map_err(|e| ChainError::Invalid(e.to_string()))?;
        let snap = self.store.build(&tip.snapshot, &ws, height).await.map_err(internal)?;
        let header = Header {
            version: HEADER_VERSION,
            height,
            prev_hash: parent,
            timestamp_ms: now_ms.max(self.min_timestamp(&parent)),
            target_bits: self.required_bits(&parent),
            job_id: job.as_ref().map(|j| j.job_id).unwrap_or([0; 32]),
            miner,
            tx_root: agent_types::block::tx_root_with(&txs, Some(&summary.coinbase)),
            state_root: snap.state_root,
            withdraw_root: snap.withdraw_root,
            proof_hash: [0; 32],
            nonce: 0,
        };
        Ok(Template { header, txs, job, summary, fallback: tip.config.fallback_params(), config: tip.config.clone() })
    }

    /// Decode a sync payload (header + proof).
    pub fn decode_header_payload(b: &[u8]) -> Result<(Header, Vec<u8>), ChainError> {
        decode_header_with_proof(b).map_err(|e| ChainError::Invalid(e.to_string()))
    }

    /// Historical withdrawal proof against the withdraw_root of the FINAL canonical
    /// block at `at_height` (which must be at or above the withdrawal's block):
    /// `(log entry, location, proof bytes, that header's withdraw_root, its block hash)`.
    /// `Ok(None)` if the withdrawal is not final yet.
    pub async fn withdrawal_proof_at(&self, nonce: u64, at_height: u64) -> Result<Option<(Vec<u8>, u64, Vec<u8>, Hash, Hash)>, ChainError> {
        let Some(loc) = self.withdraw_loc.get(&nonce).copied() else { return Ok(None) };
        if at_height > self.finalized().header.height {
            return invalid(format!("at_height {at_height} is not final (final height {})", self.finalized().header.height));
        }
        let size = self.log_size_at.range(..=at_height).next_back().map(|(_, s)| *s).unwrap_or(0);
        if loc >= size {
            return invalid(format!("withdrawal {nonce} is not in the log at height {at_height} (it is in a later block)"));
        }
        let hash = self.canonical_hash(at_height).ok_or_else(|| internal("no canonical block at a final height"))?;
        let header = if hash == self.finalized { self.finalized().header } else {
            self.get_block(&hash).await?.ok_or_else(|| internal("final block missing from the archive"))?.header
        };
        let entry = self.store.log_get(loc).await.map_err(internal)?.ok_or_else(|| internal("log entry missing"))?;
        let proof = self.store.withdrawal_proof_at(loc, size).await.map_err(internal)?;
        Ok(Some((entry, loc, proof, header.withdraw_root, hash)))
    }

    /// Committed withdrawal proof: `(log entry, location, proof bytes, root)`.
    pub async fn withdrawal_proof(&self, nonce: u64) -> Result<Option<(Vec<u8>, u64, Vec<u8>, Hash)>, ChainError> {
        let Some(loc) = self.withdraw_loc.get(&nonce).copied() else { return Ok(None) };
        let entry = self.store.log_get(loc).await.map_err(internal)?.ok_or_else(|| internal("log entry missing"))?;
        let (proof, root) = self.store.withdrawal_proof(loc).await.map_err(internal)?;
        Ok(Some((entry, loc, proof, root)))
    }
}

/// Read helper over a live snapshot (for executor-style reads outside the chain).
pub struct TipRead<'a, E: commonware_storage::Context + Spawner> {
    pub view: View<'a, E>,
}

impl<E: commonware_storage::Context + Spawner> StateRead for TipRead<'_, E> {
    async fn get(&self, key: &schema::Key) -> Result<Option<Vec<u8>>, state::StateError> {
        self.view.get(key).await
    }
}

pub fn hex(h: &Hash) -> String {
    h.iter().take(6).map(|b| format!("{b:02x}")).collect()
}
