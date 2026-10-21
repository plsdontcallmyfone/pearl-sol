//! pearl-pool: a mining pool for the agent chain (SPEC v2 sections 3 and 7, M2).
//!
//! - The pool asks the node for a block template with its OWN payout key in the
//!   header's `miner` field and hands that header out as work. Each registered miner
//!   gets its own nonce range (`id << nonce_range_bits`), so no two miners do the same
//!   work and a share's nonce says whose it is.
//! - A share is a block proof (PSJ1, seed mode 3, bound to the header's `pow_digest`)
//!   that meets the pool's share target, which is easier than the block target. The
//!   pool checks it with `pearl_job::pow::verify_block_proof` at the share target; a
//!   share whose jackpot hash also meets the block target is a block and goes to the
//!   node with `submitBlock`. Pearl pools check shares the same way (plain proofs,
//!   research/NOTES-proof.md).
//! - Rejected: unknown work, stale work (its parent is no longer the tip), nonce outside
//!   the miner's range, duplicate (work, nonce), bad proof bytes, low work.
//! - Rewards: PPLNS over the last N shares (weighted by share work) as they stood when
//!   the block was found. The split is fixed at that moment: `fee = reward x fee_bps /
//!   10000`, each miner gets `floor((reward - fee) x w_i / W)`, the rounding remainder
//!   stays with the pool. Credits are immature until the block is `maturity` deep (the
//!   chain credits coinbase after finality_k); a block no longer canonical then is
//!   orphaned and its credits are dropped.
//! - Payouts: Transfer txs from the pool key, consecutive nonces, up to `max_batch` per
//!   round. Each payout is signed and saved to disk before it is sent, then re-sent
//!   (same bytes, same nonce) until the pool account's nonce passes it, so a crash can
//!   neither pay twice nor lose a payout. The tx fee comes out of the miner's payout.
//! - State is one JSON file, rewritten atomically (tmp + rename) after every change.

pub mod client;
pub mod http;
pub mod node;
pub mod testing;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use primitive_types::U256;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pearl_job::job::{difficulty_bound, meets_target, nbits_to_target, Job};
use pearl_job::pow::{block_proof_from_bytes, fallback_job, proof_hash, verify_block_proof, BlockHeader, MinedBlock};
use pearl_node::tx::{self, Body, Tx};
use pearl_node::{b58, header_fields_json, SubmitOutcome, Template};

pub use node::PoolNode;

pub const DEFAULT_PORT: u16 = 10663;
/// Share target = block target x this. Retuned 16 -> 11 on 2026-10-17 for the 4 x 8
/// fallback default (one tile, so one jackpot attempt, per nonce). At 64 x 64 a nonce had
/// 128 tiles but counts as at most one share, which capped factor 16 at about 11 shares
/// per block found (live run: 13.6 pool shares per chain block incl. stale); at 4 x 8
/// every share is its own nonce, so factor 16 gave the full 16 (live: 14.6-16.9) and 11
/// brings it back (live: 13.4; tests/pool.rs `default_share_factor_on_4x8_fallback`).
pub const DEFAULT_SHARE_FACTOR: u64 = 11;
/// The easiest target the chain allows (devnet genesis).
pub const POW_LIMIT_BITS: u32 = 0x207fffff;
const STATE_VERSION: u32 = 1;

// ---------------------------------------------------------------- targets

/// Bitcoin compact encoding of a 256-bit target (inverse of `nbits_to_target`, rounding
/// the target down to 3 significant bytes).
pub fn target_to_nbits(t: U256) -> u32 {
    if t.is_zero() {
        return 0;
    }
    let mut size = (t.bits() + 7) / 8;
    let mut compact: u32 = if size <= 3 { (t.low_u64() << (8 * (3 - size))) as u32 } else { (t >> (8 * (size - 3))).low_u32() };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    compact | ((size as u32) << 24)
}

/// Work of one share or block: `2^256 / (target + 1)`, saturated to u128.
pub fn work_of(nbits: u32) -> u128 {
    let t = nbits_to_target(nbits);
    if t == U256::MAX {
        return 1;
    }
    let w = U256::MAX / (t + U256::one());
    if w > U256::from(u128::MAX) {
        u128::MAX
    } else {
        w.low_u128().max(1)
    }
}

/// The share target for a block target: a fixed `share_bits` or `block x factor`, never
/// harder than the block target and never easier than the pow limit.
pub fn share_bits_for(block_bits: u32, fixed: Option<u32>, factor: u64) -> u32 {
    let bt = nbits_to_target(block_bits);
    let limit = nbits_to_target(POW_LIMIT_BITS);
    let cand = match fixed {
        Some(b) => b,
        None => {
            let f = U256::from(factor.max(1));
            let t = if bt > limit / f { limit } else { (bt * f).min(limit) };
            target_to_nbits(t)
        }
    };
    if nbits_to_target(cand) < bt {
        block_bits
    } else {
        cand
    }
}

// ---------------------------------------------------------------- config

#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Pool fee on each block reward, basis points.
    pub fee_bps: u32,
    /// PPLNS window: the last N shares when a block is found.
    pub pplns_n: usize,
    /// Blocks until a found block's reward is paid out. None = node finality_k + 1.
    pub maturity: Option<u64>,
    /// Fixed share target (compact). None = block target x `share_factor`.
    pub share_bits: Option<u32>,
    /// Default [`DEFAULT_SHARE_FACTOR`].
    pub share_factor: u64,
    /// Smallest payout (after the tx fee).
    pub min_payout: u64,
    /// Payout tx fee. None = the template's min_fee.
    pub payout_fee: Option<u64>,
    /// Payout txs per round (the node accepts nonces up to 64 ahead).
    pub max_batch: usize,
    /// Miner `id` gets nonces `[id << bits, (id + 1) << bits)`.
    pub nonce_range_bits: u32,
    /// Overrides the template's chain_id for payout txs.
    pub chain_id: Option<u32>,
    /// Re-fetch the template at least this often (new mempool txs) even on the same tip.
    pub template_refresh: Duration,
    /// Re-send an unconfirmed payout after this long.
    pub resend_after: Duration,
    pub state_path: PathBuf,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            fee_bps: 100,
            pplns_n: 1000,
            maturity: None,
            share_bits: None,
            share_factor: DEFAULT_SHARE_FACTOR,
            min_payout: 1_000_000,
            payout_fee: None,
            max_batch: 32,
            nonce_range_bits: 40,
            chain_id: None,
            template_refresh: Duration::from_secs(5),
            resend_after: Duration::from_secs(20),
            state_path: PathBuf::from(".state/pool.json"),
        }
    }
}

// ---------------------------------------------------------------- persisted state

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MinerRec {
    pub id: u32,
    pub payout: String,
    pub nonce_start: u64,
    pub nonce_end: u64,
    pub shares: u64,
    pub rejected: u64,
    pub blocks: u64,
    /// Credited from blocks not yet mature.
    pub immature: u64,
    /// Matured and not yet paid.
    pub balance: u64,
    /// In payout txs not yet confirmed.
    pub in_flight: u64,
    /// Confirmed payouts (amount received, excluding tx fees).
    pub paid: u64,
    /// Tx fees taken from this miner's payouts.
    pub tx_fees: u64,
    pub registered_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareRec {
    pub seq: u64,
    pub miner: u32,
    pub weight: u128,
    pub height: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BlockStatus {
    Immature,
    Matured,
    Orphaned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FoundBlock {
    pub hash: String,
    pub height: u64,
    pub finder: u32,
    pub reward: u64,
    pub fee: u64,
    pub remainder: u64,
    /// miner id -> amount.
    pub credits: BTreeMap<u32, u64>,
    pub window_shares: usize,
    pub status: BlockStatus,
    pub found_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PayoutStatus {
    Pending,
    Confirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Payout {
    pub nonce: u64,
    pub miner: u32,
    pub to: String,
    pub amount: u64,
    pub fee: u64,
    pub tx: String,
    pub tx_hash: Option<String>,
    pub status: PayoutStatus,
    pub created_ms: u64,
    pub sends: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Totals {
    pub blocks_found: u64,
    pub blocks_matured: u64,
    pub blocks_orphaned: u64,
    pub shares: u64,
    pub rejected: u64,
    /// Sum of matured block rewards (what the chain credited the pool key).
    pub rewards_matured: u64,
    /// Pool fee + rounding remainder on matured blocks.
    pub pool_earned: u64,
    /// Amount + tx fee of confirmed payouts (what left the pool account).
    pub paid_out: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolState {
    pub version: u32,
    pub pool_key: String,
    pub miners: BTreeMap<String, MinerRec>,
    pub next_miner_id: u32,
    pub window: VecDeque<ShareRec>,
    pub share_seq: u64,
    /// work id -> (work height, nonces seen). Kept for works on the current tip.
    pub seen: BTreeMap<String, (u64, BTreeSet<u64>)>,
    pub blocks: Vec<FoundBlock>,
    /// Next payout tx nonce (at least the pool account's chain nonce).
    pub next_nonce: u64,
    pub payouts: Vec<Payout>,
    pub totals: Totals,
}

impl PoolState {
    fn miner_by_id(&mut self, id: u32) -> Option<&mut MinerRec> {
        self.miners.values_mut().find(|m| m.id == id)
    }
}

/// PPLNS split of one block reward over a share window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub fee: u64,
    pub remainder: u64,
    pub credits: BTreeMap<u32, u64>,
}

/// `fee = reward x fee_bps / 10000` (floor); `net = reward - fee`; miner i gets
/// `floor(net x w_i / W)` where `w_i` sums its shares' weights in the window; the
/// rounding remainder goes to the pool. Always `sum(credits) + fee + remainder == reward`.
pub fn pplns_split<'a>(reward: u64, fee_bps: u32, window: impl IntoIterator<Item = &'a ShareRec>) -> Split {
    let fee = ((reward as u128 * fee_bps.min(10_000) as u128) / 10_000) as u64;
    let net = reward - fee;
    let mut w: BTreeMap<u32, u128> = BTreeMap::new();
    for s in window {
        *w.entry(s.miner).or_default() += s.weight;
    }
    let total: U256 = w.values().fold(U256::zero(), |a, x| a + U256::from(*x));
    let mut credits = BTreeMap::new();
    let mut given = 0u64;
    if !total.is_zero() {
        for (m, wi) in &w {
            let c = (U256::from(net) * U256::from(*wi) / total).low_u64();
            if c > 0 {
                credits.insert(*m, c);
                given += c;
            }
        }
    }
    Split { fee, remainder: net - given, credits }
}

// ---------------------------------------------------------------- work

#[derive(Debug, Clone)]
pub struct Work {
    pub id: String,
    pub template: Template,
    pub share_bits: u32,
    pub reward: u64,
}

impl Work {
    fn new(template: Template, share_bits: u32, reward: u64) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(&template.header.to_bytes());
        h.update(template.template_id.as_deref().unwrap_or("").as_bytes());
        let id = hex::encode(&h.finalize().as_bytes()[..16]);
        Self { id, template, share_bits, reward }
    }
}

/// Outcome of one share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareOutcome {
    Accepted { block: Option<BlockResult> },
    Rejected { reason: String, stale: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResult {
    pub accepted: bool,
    pub hash: String,
    pub height: u64,
    pub reason: Option<String>,
}

impl ShareOutcome {
    pub fn to_json(&self) -> Value {
        match self {
            ShareOutcome::Accepted { block } => json!({"accepted": true, "block": block.as_ref().map(|b| json!({
                "accepted": b.accepted, "hash": b.hash, "height": b.height, "reason": b.reason}))}),
            ShareOutcome::Rejected { reason, stale } => json!({"accepted": false, "reason": reason, "stale": stale}),
        }
    }
}

pub type Logger = Box<dyn Fn(Value) + Send + Sync>;

struct Inner {
    st: PoolState,
    works: HashMap<String, Arc<Work>>,
    /// Current default work and fallback work ids.
    current: Option<String>,
    current_fallback: Option<String>,
    tip: ([u8; 32], u64),
    last_template: Option<Instant>,
    min_fee: u64,
    chain_id: Option<u32>,
    /// Last send time of each pending payout (by nonce); not persisted.
    sent_at: HashMap<u64, Instant>,
    /// Registrations with no accepted share yet (security pass V-10): kept in memory
    /// only, at most MAX_PENDING_MINERS, dropped after PENDING_TTL without a request.
    /// A miner moves into the persisted state with its first accepted share.
    pending: HashMap<String, (MinerRec, Instant)>,
    /// Ids of dropped pending registrations (never used for an accepted share), reused first.
    free_ids: Vec<u32>,
}

/// Unproven registrations held at once; more answer an error until some expire.
pub const MAX_PENDING_MINERS: usize = 4096;
/// A pending registration that asks for no work and sends no share for this long is dropped.
pub const PENDING_TTL: Duration = Duration::from_secs(30 * 60);

pub struct Pool {
    node: Arc<dyn PoolNode>,
    key: SigningKey,
    pub cfg: PoolConfig,
    maturity: u64,
    inner: Mutex<Inner>,
    submit_lock: Mutex<()>,
    log: Logger,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn parse_payout(s: &str) -> Result<[u8; 32]> {
    pearl_job::spec::parse_key32(s).with_context(|| format!("bad payout key {s}"))
}

impl Pool {
    /// Load state from `cfg.state_path` (or start empty) and fetch the first template.
    pub fn open(node: Arc<dyn PoolNode>, key: SigningKey, cfg: PoolConfig, log: Logger) -> Result<Self> {
        let pool_key = b58(&key.verifying_key().to_bytes());
        let st = match std::fs::read(&cfg.state_path) {
            Ok(b) => {
                let st: PoolState = serde_json::from_slice(&b).with_context(|| format!("parse {}", cfg.state_path.display()))?;
                if st.pool_key != pool_key {
                    bail!("{} belongs to pool key {}, not {pool_key}", cfg.state_path.display(), st.pool_key);
                }
                st
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                PoolState { version: STATE_VERSION, pool_key: pool_key.clone(), next_miner_id: 1, ..Default::default() }
            }
            Err(e) => return Err(e).with_context(|| format!("read {}", cfg.state_path.display())),
        };
        let maturity = match cfg.maturity {
            Some(m) => m,
            None => node.finality_k().context("node finality_k (or pass --maturity)")? + 1,
        };
        let pool = Self {
            node,
            key,
            cfg,
            maturity,
            inner: Mutex::new(Inner {
                st,
                works: HashMap::new(),
                current: None,
                current_fallback: None,
                tip: ([0xff; 32], u64::MAX),
                last_template: None,
                min_fee: 0,
                chain_id: None,
                pending: HashMap::new(),
                free_ids: Vec::new(),
                sent_at: HashMap::new(),
            }),
            submit_lock: Mutex::new(()),
            log,
        };
        pool.log(json!({"event": "open", "pool": pool_key, "maturity": maturity, "fee_bps": pool.cfg.fee_bps,
                        "pplns_n": pool.cfg.pplns_n, "state": pool.cfg.state_path.display().to_string(),
                        "miners": pool.state().miners.len(), "blocks": pool.state().blocks.len()}));
        if let Err(e) = pool.refresh() {
            pool.log(json!({"event": "refresh_failed", "error": format!("{e:#}")}));
        }
        Ok(pool)
    }

    fn log(&self, v: Value) {
        (self.log)(v)
    }

    pub fn pool_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn maturity(&self) -> u64 {
        self.maturity
    }

    /// A copy of the persisted state.
    pub fn state(&self) -> PoolState {
        self.inner.lock().unwrap().st.clone()
    }

    fn persist(&self, st: &PoolState) -> Result<()> {
        let p = &self.cfg.state_path;
        if let Some(d) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(st)?)?;
        std::fs::rename(&tmp, p)?;
        Ok(())
    }

    // ------------------------------------------------------------ miners

    /// Register a payout key (idempotent: the same key keeps its id and nonce range).
    pub fn register(&self, payout: &[u8; 32]) -> Result<MinerRec> {
        let mut g = self.inner.lock().unwrap();
        let k = b58(payout);
        if let Some(m) = g.st.miners.get(&k) {
            return Ok(m.clone());
        }
        let now = Instant::now();
        if let Some((m, seen)) = g.pending.get_mut(&k) {
            *seen = now;
            return Ok(m.clone());
        }
        // Unproven registrations stay out of the state file (no write per registration)
        // and are capped, so mass registration cannot slow writes or eat the nonce space.
        let expired: Vec<String> = g.pending.iter().filter(|(_, (_, t))| now.duration_since(*t) >= PENDING_TTL).map(|(k, _)| k.clone()).collect();
        for e in expired {
            if let Some((m, _)) = g.pending.remove(&e) {
                g.free_ids.push(m.id);
            }
        }
        ensure!(g.pending.len() < MAX_PENDING_MINERS, "too many pending registrations; retry later");
        let bits = self.cfg.nonce_range_bits.min(63);
        let (id, fresh) = match g.free_ids.pop() {
            Some(id) => (id, false),
            None => (g.st.next_miner_id.max(1), true),
        };
        if (id as u128 + 1) << bits > u64::MAX as u128 + 1 {
            bail!("nonce space exhausted: no range left for miner {id}");
        }
        let start = (id as u64) << bits;
        let end = start.wrapping_add(1u64 << bits).wrapping_sub(1);
        let rec = MinerRec {
            id,
            payout: k.clone(),
            nonce_start: start,
            nonce_end: end,
            shares: 0,
            rejected: 0,
            blocks: 0,
            immature: 0,
            balance: 0,
            in_flight: 0,
            paid: 0,
            tx_fees: 0,
            registered_ms: now_ms(),
        };
        g.pending.insert(k.clone(), (rec.clone(), now));
        if fresh {
            g.st.next_miner_id = id + 1;
        }
        drop(g);
        self.log(json!({"event": "register", "miner": k, "id": id, "nonce_start": start, "nonce_end": end}));
        Ok(rec)
    }

    pub fn miner(&self, payout: &[u8; 32]) -> Option<MinerRec> {
        let g = self.inner.lock().unwrap();
        let k = b58(payout);
        g.st.miners.get(&k).cloned().or_else(|| g.pending.get(&k).map(|(m, _)| m.clone()))
    }

    /// Unproven registrations held in memory (not in the state file).
    pub fn pending_miners(&self) -> usize {
        self.inner.lock().unwrap().pending.len()
    }

    // ------------------------------------------------------------ templates

    fn make_work(&self, t: Template) -> Result<Work> {
        if t.header.miner != self.pool_key() {
            bail!("node template pays {} not the pool key", b58(&t.header.miner));
        }
        let share_bits = share_bits_for(t.header.target_bits, self.cfg.share_bits, self.cfg.share_factor);
        let reward = self.node.template_reward(&t);
        Ok(Work::new(t, share_bits, reward))
    }

    fn install(&self, g: &mut Inner, w: Work, fallback: bool) -> Arc<Work> {
        g.min_fee = w.template.min_fee;
        if w.template.chain_id.is_some() {
            g.chain_id = w.template.chain_id;
        }
        let id = w.id.clone();
        let w = g.works.entry(id.clone()).or_insert_with(|| Arc::new(w)).clone();
        if fallback {
            g.current_fallback = Some(id);
        } else {
            g.current = Some(id);
        }
        w
    }

    /// Follow the node: on a new tip (or after `template_refresh`) fetch a new template;
    /// works on an old parent become stale and are dropped.
    pub fn refresh(&self) -> Result<()> {
        let tip = self.node.tip()?;
        let (changed, due) = {
            let g = self.inner.lock().unwrap();
            (g.tip != tip, g.last_template.map_or(true, |t| t.elapsed() >= self.cfg.template_refresh) || g.current.is_none())
        };
        if !changed && !due {
            return Ok(());
        }
        let t = self.node.block_template(&self.pool_key())?;
        let w = self.make_work(t)?;
        let mut g = self.inner.lock().unwrap();
        // The template's parent is the tip we follow (the node may have moved since `tip`).
        let (prev, height) = (w.template.header.prev_hash, w.template.header.height);
        let changed = g.tip != (prev, height.saturating_sub(1));
        if changed {
            g.tip = (prev, height.saturating_sub(1));
            g.current_fallback = None;
            g.works.retain(|_, x| x.template.header.prev_hash == prev);
            // Dedup sets live as long as work at that height can still be submitted (also
            // across a restart, when the same template comes back under the same work id).
            let before = g.st.seen.len();
            g.st.seen.retain(|_, (h, _)| *h >= height);
            if g.st.seen.len() != before {
                self.persist(&g.st)?;
            }
        }
        let (height, bits, share, job) = (w.template.header.height, w.template.header.target_bits, w.share_bits, w.template.job.as_ref().map(|j| hex::encode(j.job_id)));
        let new = !g.works.contains_key(&w.id);
        let id = self.install(&mut g, w, false).id.clone();
        g.last_template = Some(Instant::now());
        drop(g);
        if new {
            self.log(json!({"event": "work", "work_id": id, "height": height, "target_bits": bits, "share_bits": share, "job": job, "tip_changed": changed}));
        }
        Ok(())
    }

    /// Work for a miner (registers it if needed). `fallback` asks for a fallback block
    /// (for a miner that cannot load the current job's data).
    pub fn work(&self, payout: &[u8; 32], fallback: bool) -> Result<Value> {
        let m = self.register(payout)?;
        self.refresh()?;
        let w = {
            let g = self.inner.lock().unwrap();
            let id = if fallback { g.current_fallback.clone() } else { g.current.clone() };
            id.and_then(|i| g.works.get(&i).cloned())
        };
        let w = match (w, fallback) {
            (Some(w), _) => w,
            (None, true) => {
                let t = self.node.fallback_template(&self.pool_key())?;
                let w = self.make_work(t)?;
                let mut g = self.inner.lock().unwrap();
                if w.template.header.prev_hash != g.tip.0 {
                    bail!("fallback template is not on the current tip; retry");
                }
                self.install(&mut g, w, true)
            }
            (None, false) => bail!("no work yet (node unreachable?)"),
        };
        let t = &w.template;
        Ok(json!({
            "work_id": w.id, "header": hex::encode(t.header.to_bytes()), "header_fields": header_fields_json(&t.header),
            "template_id": t.template_id, "height": t.header.height, "target_bits": t.header.target_bits,
            "share_bits": w.share_bits, "chain_id": t.chain_id, "min_fee": t.min_fee.to_string(),
            "job": t.job.as_ref().map(|j| j.to_json()),
            "fallback": {"m": t.fallback.m, "n": t.fallback.n, "config": hex::encode(t.fallback.config.to_bytes())},
            "nonce_start": m.nonce_start, "nonce_end": m.nonce_end, "miner_id": m.id,
            "pool": b58(&self.pool_key()),
        }))
    }

    // ------------------------------------------------------------ shares

    fn reject(&self, payout: &str, reason: String, stale: bool) -> Result<ShareOutcome> {
        let mut g = self.inner.lock().unwrap();
        g.st.totals.rejected += 1;
        if let Some(m) = g.st.miners.get_mut(payout) {
            m.rejected += 1;
        } else if let Some((m, _)) = g.pending.get_mut(payout) {
            m.rejected += 1;
        }
        // Rejected-share counters are saved with the next accepted share or block, not on
        // every rejection: unauthenticated junk shares must not each rewrite the state file.
        drop(g);
        self.log(json!({"event": "share_rejected", "miner": payout, "reason": reason, "stale": stale}));
        Ok(ShareOutcome::Rejected { reason, stale })
    }

    /// Check one share and, if it meets the block target, submit it as a block.
    pub fn submit_share(&self, payout: &[u8; 32], work_id: &str, nonce: u64, proof: &[u8]) -> Result<ShareOutcome> {
        let pk = b58(payout);
        // 1. Cheap checks under the lock.
        let (work, miner) = {
            let g = self.inner.lock().unwrap();
            let Some(m) = g.st.miners.get(&pk).cloned().or_else(|| g.pending.get(&pk).map(|(m, _)| m.clone())) else {
                drop(g);
                return self.reject(&pk, "unregistered miner".into(), false);
            };
            let Some(w) = g.works.get(work_id).cloned() else {
                drop(g);
                return self.reject(&pk, "unknown or stale work".into(), true);
            };
            if w.template.header.prev_hash != g.tip.0 {
                drop(g);
                return self.reject(&pk, "stale work: a new block arrived".into(), true);
            }
            if nonce < m.nonce_start || nonce > m.nonce_end {
                drop(g);
                return self.reject(&pk, format!("nonce {nonce} outside this miner's range"), false);
            }
            if g.st.seen.get(work_id).is_some_and(|(_, s)| s.contains(&nonce)) {
                drop(g);
                return self.reject(&pk, "duplicate share".into(), false);
            }
            (w, m)
        };
        // 2. Verify without the lock.
        let mut header: BlockHeader = work.template.header;
        header.nonce = nonce;
        header.proof_hash = [0; 32];
        let pd = header.pow_digest();
        let p = match block_proof_from_bytes(proof) {
            Ok(p) => p,
            Err(e) => return self.reject(&pk, format!("invalid proof: {e:#}"), false),
        };
        let job: Job = match &work.template.job {
            Some(j) if !header.is_fallback() => j.to_job(),
            _ => fallback_job(&pd, &work.template.fallback)?.job,
        };
        let vs = match verify_block_proof(&job, &p, &pd, work.share_bits, nonce) {
            Ok(v) => v,
            Err(e) => return self.reject(&pk, format!("low work or invalid: {e:#}"), false),
        };
        let is_block = meets_target(&vs.jackpot_hash, &difficulty_bound(header.target_bits, &job.config));
        // 3. Record under the lock (re-check stale and duplicate: another thread may have raced).
        let snapshot: Option<Vec<ShareRec>> = {
            let mut g = self.inner.lock().unwrap();
            if work.template.header.prev_hash != g.tip.0 {
                drop(g);
                return self.reject(&pk, "stale work: a new block arrived".into(), true);
            }
            // First accepted share of a pending registration: it joins the persisted state.
            if !g.st.miners.contains_key(&pk) {
                match g.pending.remove(&pk) {
                    Some((m, _)) if m.id == miner.id => {
                        g.st.miners.insert(pk.clone(), m);
                    }
                    other => {
                        if let Some(o) = other {
                            g.pending.insert(pk.clone(), o);
                        }
                        drop(g);
                        return self.reject(&pk, "registration expired; register again".into(), false);
                    }
                }
            }
            let height = header.height;
            let seen = g.st.seen.entry(work_id.to_string()).or_insert_with(|| (height, BTreeSet::new()));
            if !seen.1.insert(nonce) {
                drop(g);
                return self.reject(&pk, "duplicate share".into(), false);
            }
            g.st.share_seq += 1;
            let rec = ShareRec { seq: g.st.share_seq, miner: miner.id, weight: work_of(work.share_bits), height };
            g.st.window.push_back(rec);
            while g.st.window.len() > self.cfg.pplns_n.max(1) {
                g.st.window.pop_front();
            }
            g.st.totals.shares += 1;
            if let Some(m) = g.st.miners.get_mut(&pk) {
                m.shares += 1;
            }
            self.persist(&g.st)?;
            is_block.then(|| g.st.window.iter().cloned().collect())
        };
        let Some(window) = snapshot else {
            return Ok(ShareOutcome::Accepted { block: None });
        };
        // 4. A block: submit, then fix its PPLNS split.
        header.proof_hash = proof_hash(proof);
        let mined = MinedBlock { header, proof: proof.to_vec() };
        // One block submission at a time, and only on the current tip: two shares on the
        // same work that both meet the block target would otherwise make sibling blocks
        // (the node accepts the second as a side branch; it would be orphaned).
        let _submitting = self.submit_lock.lock().unwrap();
        if self.inner.lock().unwrap().tip.0 != header.prev_hash {
            self.log(json!({"event": "block_share_superseded", "height": header.height, "miner": pk}));
            return Ok(ShareOutcome::Accepted { block: None });
        }
        let out = self.node.submit_block(&work.template, &mined)?;
        let hash = match &out {
            SubmitOutcome::Accepted { block_hash } => *block_hash,
            SubmitOutcome::Rejected { reason } => {
                self.log(json!({"event": "block_rejected", "height": header.height, "reason": reason, "miner": pk}));
                return Ok(ShareOutcome::Accepted {
                    block: Some(BlockResult { accepted: false, hash: hex::encode(header.block_hash()), height: header.height, reason: Some(reason.clone()) }),
                });
            }
        };
        let split = pplns_split(work.reward, self.cfg.fee_bps, &window);
        {
            let mut g = self.inner.lock().unwrap();
            // Our block is the new tip: work on its parent is stale from now on.
            g.tip = (hash, header.height);
            g.works.retain(|_, x| x.template.header.prev_hash == hash);
            g.current = None;
            g.current_fallback = None;
            for (id, c) in &split.credits {
                if let Some(m) = g.st.miner_by_id(*id) {
                    m.immature += c;
                }
            }
            if let Some(m) = g.st.miners.get_mut(&pk) {
                m.blocks += 1;
            }
            g.st.totals.blocks_found += 1;
            g.st.blocks.push(FoundBlock {
                hash: hex::encode(hash),
                height: header.height,
                finder: miner.id,
                reward: work.reward,
                fee: split.fee,
                remainder: split.remainder,
                credits: split.credits.clone(),
                window_shares: window.len(),
                status: BlockStatus::Immature,
                found_ms: now_ms(),
            });
            self.persist(&g.st)?;
        }
        self.log(json!({"event": "block_found", "height": header.height, "hash": hex::encode(hash), "finder": pk,
                        "reward": work.reward.to_string(), "fee": split.fee.to_string(), "remainder": split.remainder.to_string(),
                        "window": window.len(), "miners_credited": split.credits.len()}));
        drop(_submitting);
        if let Err(e) = self.refresh() {
            self.log(json!({"event": "refresh_failed", "error": format!("{e:#}")}));
        }
        Ok(ShareOutcome::Accepted { block: Some(BlockResult { accepted: true, hash: hex::encode(hash), height: header.height, reason: None }) })
    }

    // ------------------------------------------------------------ maturity + payouts

    /// Move found blocks `maturity` deep to matured (credits become payable) or orphaned.
    pub fn mature(&self) -> Result<usize> {
        let (_, tip_h) = self.node.tip()?;
        let due: Vec<(usize, u64, String)> = {
            let g = self.inner.lock().unwrap();
            g.st.blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.status == BlockStatus::Immature && tip_h >= b.height + self.maturity)
                .map(|(i, b)| (i, b.height, b.hash.clone()))
                .collect()
        };
        if due.is_empty() {
            return Ok(0);
        }
        let mut verdicts = Vec::new();
        for (i, h, hash) in due {
            let canon = self.node.canonical_hash(h)?.map(hex::encode);
            verdicts.push((i, canon.as_deref() == Some(hash.as_str())));
        }
        let mut g = self.inner.lock().unwrap();
        let n = verdicts.len();
        for (i, ok) in verdicts {
            let b = g.st.blocks[i].clone();
            for (id, c) in &b.credits {
                if let Some(m) = g.st.miner_by_id(*id) {
                    m.immature -= c;
                    if ok {
                        m.balance += c;
                    }
                }
            }
            if ok {
                g.st.blocks[i].status = BlockStatus::Matured;
                g.st.totals.blocks_matured += 1;
                g.st.totals.rewards_matured += b.reward;
                g.st.totals.pool_earned += b.fee + b.remainder;
            } else {
                g.st.blocks[i].status = BlockStatus::Orphaned;
                g.st.totals.blocks_orphaned += 1;
            }
            self.log(json!({"event": if ok { "block_matured" } else { "block_orphaned" }, "height": b.height, "hash": b.hash, "reward": b.reward.to_string()}));
        }
        self.persist(&g.st)?;
        Ok(n)
    }

    /// Confirm sent payouts, re-send stale ones, and start new ones for miners whose
    /// matured balance reaches `min_payout`.
    pub fn payouts(&self) -> Result<Vec<Payout>> {
        let me = self.pool_key();
        let acct = self.node.get_account(&me)?;
        let mut created = Vec::new();
        let mut to_send: Vec<(u64, Vec<u8>)> = Vec::new();
        {
            let mut g = self.inner.lock().unwrap();
            // Confirm: the pool key signs one tx per nonce, so a passed nonce is ours.
            let mut changed = false;
            for i in 0..g.st.payouts.len() {
                let p = g.st.payouts[i].clone();
                if p.status == PayoutStatus::Pending && p.nonce < acct.nonce {
                    g.st.payouts[i].status = PayoutStatus::Confirmed;
                    g.st.totals.paid_out += p.amount + p.fee;
                    if let Some(m) = g.st.miner_by_id(p.miner) {
                        m.in_flight -= p.amount + p.fee;
                        m.paid += p.amount;
                        m.tx_fees += p.fee;
                    }
                    g.sent_at.remove(&p.nonce);
                    changed = true;
                    self.log(json!({"event": "payout_confirmed", "to": p.to, "amount": p.amount.to_string(), "nonce": p.nonce}));
                }
            }
            if g.st.next_nonce < acct.nonce {
                g.st.next_nonce = acct.nonce;
                changed = true;
            }
            // New payouts, funded only by what the account holds beyond pending ones.
            let pending: Vec<Payout> = g.st.payouts.iter().filter(|p| p.status == PayoutStatus::Pending).cloned().collect();
            let committed: u64 = pending.iter().map(|p| p.amount + p.fee).sum();
            let mut available = acct.balance.saturating_sub(committed);
            let fee = self.cfg.payout_fee.unwrap_or(g.min_fee);
            let chain_id = self.cfg.chain_id.or(g.chain_id);
            let room = self.cfg.max_batch.saturating_sub(pending.len()).min(64usize.saturating_sub(pending.len()));
            if room > 0 {
                if let Some(chain_id) = chain_id {
                    let mut due: Vec<(u64, u32, String)> = g
                        .st
                        .miners
                        .values()
                        .filter(|m| m.balance > fee && m.balance - fee >= self.cfg.min_payout.max(1))
                        .map(|m| (m.balance, m.id, m.payout.clone()))
                        .collect();
                    due.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                    for (bal, id, to) in due.into_iter().take(room) {
                        if bal > available {
                            continue;
                        }
                        let nonce = g.st.next_nonce;
                        let amount = bal - fee;
                        let tx = Tx { chain_id, from: me, nonce, fee, body: Body::Transfer { to: parse_payout(&to)?, amount } };
                        let wire = tx::sign(&tx, &self.key)?;
                        let p = Payout {
                            nonce,
                            miner: id,
                            to: to.clone(),
                            amount,
                            fee,
                            tx: base64::engine::general_purpose::STANDARD.encode(&wire),
                            tx_hash: None,
                            status: PayoutStatus::Pending,
                            created_ms: now_ms(),
                            sends: 0,
                        };
                        let m = g.st.miner_by_id(id).unwrap();
                        m.balance -= bal;
                        m.in_flight += bal;
                        g.st.next_nonce += 1;
                        available -= bal;
                        g.st.payouts.push(p.clone());
                        created.push(p);
                        changed = true;
                    }
                }
            }
            // Signed payouts hit the disk before any is sent.
            if changed {
                self.persist(&g.st)?;
            }
            let resend = self.cfg.resend_after;
            for p in g.st.payouts.iter().filter(|p| p.status == PayoutStatus::Pending) {
                if g.sent_at.get(&p.nonce).map_or(true, |t| t.elapsed() >= resend) {
                    to_send.push((p.nonce, base64::engine::general_purpose::STANDARD.decode(&p.tx)?));
                }
            }
            to_send.sort();
        }
        for (nonce, wire) in to_send {
            let r = self.node.send_tx(&wire);
            let mut g = self.inner.lock().unwrap();
            g.sent_at.insert(nonce, Instant::now());
            let Some(i) = g.st.payouts.iter().position(|p| p.nonce == nonce && p.status == PayoutStatus::Pending) else { continue };
            g.st.payouts[i].sends += 1;
            let p = g.st.payouts[i].clone();
            match r {
                Ok(h) => {
                    if g.st.payouts[i].tx_hash.is_none() {
                        g.st.payouts[i].tx_hash = Some(h.clone());
                    }
                    self.log(json!({"event": "payout_sent", "to": p.to, "amount": p.amount.to_string(), "fee": p.fee, "nonce": nonce, "tx": h, "sends": p.sends}));
                }
                // A re-send of an included or pending tx is refused: harmless.
                Err(e) => self.log(json!({"event": "payout_send_failed", "to": p.to, "nonce": nonce, "sends": p.sends, "error": format!("{e:#}")})),
            }
            self.persist(&g.st)?;
        }
        Ok(created)
    }

    /// One background round: follow the tip, mature blocks, payouts.
    pub fn tick(&self) -> Result<()> {
        self.refresh()?;
        self.mature()?;
        self.payouts()?;
        Ok(())
    }

    /// Pool-wide numbers for `/stats`.
    pub fn stats(&self) -> Value {
        let g = self.inner.lock().unwrap();
        let st = &g.st;
        let cur = g.current.as_ref().and_then(|i| g.works.get(i));
        let sum = |f: fn(&MinerRec) -> u64| st.miners.values().map(f).sum::<u64>();
        json!({
            "pool": st.pool_key, "fee_bps": self.cfg.fee_bps, "pplns_n": self.cfg.pplns_n, "maturity": self.maturity,
            "min_payout": self.cfg.min_payout.to_string(), "height": g.tip.1, "tip": hex::encode(g.tip.0),
            "work_id": cur.map(|w| w.id.clone()), "target_bits": cur.map(|w| w.template.header.target_bits),
            "share_bits": cur.map(|w| w.share_bits), "reward": cur.map(|w| w.reward.to_string()),
            "miners": st.miners.len(), "window": st.window.len(), "totals": st.totals,
            "immature": sum(|m| m.immature).to_string(), "balances": sum(|m| m.balance).to_string(),
            "in_flight": sum(|m| m.in_flight).to_string(), "paid": sum(|m| m.paid).to_string(),
            "pending_payouts": st.payouts.iter().filter(|p| p.status == PayoutStatus::Pending).count(),
            "blocks": st.blocks.iter().rev().take(20).collect::<Vec<_>>(),
        })
    }

    pub fn miner_json(&self, payout: &[u8; 32]) -> Value {
        let g = self.inner.lock().unwrap();
        let k = b58(payout);
        if let Some((m, _)) = g.pending.get(&k) {
            return json!({"miner": m, "window_shares": 0, "payouts": [], "pending": true});
        }
        match g.st.miners.get(&k) {
            None => Value::Null,
            Some(m) => {
                let window_shares = g.st.window.iter().filter(|s| s.miner == m.id).count();
                let payouts: Vec<&Payout> = g.st.payouts.iter().filter(|p| p.miner == m.id).rev().take(20).collect();
                json!({"miner": m, "window_shares": window_shares, "payouts": payouts})
            }
        }
    }

    pub fn node(&self) -> &Arc<dyn PoolNode> {
        &self.node
    }
}

/// Decode a share's proof field (base64, or hex in `proof_hex`).
pub fn decode_proof(v: &Value) -> Result<Vec<u8>> {
    if let Some(s) = v.get("proof").and_then(|p| p.as_str()) {
        return Ok(base64::engine::general_purpose::STANDARD.decode(s)?);
    }
    if let Some(s) = v.get("proof_hex").and_then(|p| p.as_str()) {
        return Ok(hex::decode(s)?);
    }
    Err(anyhow!("share has no proof"))
}
