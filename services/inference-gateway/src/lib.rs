//! Cadre inference gateway (SPEC 5.4, API v1).
//!
//! Payment is a PREPAID BALANCE: a buyer tops up once over x402 (one storefront Order = one
//! balance ticket of R rows, SPEC 4.5), then sends wallet-signed (or API-key) requests that
//! draw rows from it with nothing on the chain in the request path. Every request's A and C
//! (one row per token through every registered matrix) are kept; each period the gateway
//! posts the served rows of a ticket as one segment round (PostSegments), and when the
//! balance is used up, near its deadline or closed, it delivers the per-request receipts
//! (response blob), gets the attester's signature and completes the ticket (W pro rata to
//! rows used, the rest refunded on the chain). A per-request x402 payment (one ticket per
//! request) still works when a request carries no credentials.
//!
//! OpenAI-compatible: `/v1/chat/completions` (SSE streaming), `/v1/completions`,
//! `/v1/models` with prices; a receipt (headers + `cadre` object) on every response.

pub mod auth;
pub mod server;
pub mod transport;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pearl_job::freivalds::result_hash;
use pearl_job::job::{commit_a, MiningConfig};
use pearl_job::standing;
use pearl_node::tx::{self, Body, SegmentSpec, Tx};
use pearl_node::{b58, NodeApi};

pub use auth::{auth_message, auth_message_v2, Auth, Keys};
pub use transport::{Evaluator, GenOpts, HttpEvaluator, HttpStorefront, HttpWorker, Storefront, Worker};

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ------------------------------------------------------------------ model + pure helpers

/// What the gateway needs of the registered model (from getModel).
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub model_id: [u8; 32],
    pub name: String,
    pub base: MiningConfig,
    /// (k, n) per matrix.
    pub matrices: Vec<(u32, u32)>,
    /// Registered hash_b per matrix.
    pub hash_b: Vec<[u8; 32]>,
    pub macs_per_row: u64,
}

impl ModelInfo {
    pub fn from_json(m: &Value) -> Result<Self> {
        ensure!(!m.is_null(), "model is not registered");
        let model_id = pearl_job::spec::parse_key32(m["model_id"].as_str().unwrap_or(""))?;
        let base = MiningConfig::from_bytes(&hex::decode(m["config"].as_str().unwrap_or(""))?)?;
        let matrices = m["matrices"]
            .as_array()
            .ok_or_else(|| anyhow!("getModel has no matrices"))?
            .iter()
            .map(|x| Ok((x["k"].as_u64().ok_or_else(|| anyhow!("k"))? as u32, x["n"].as_u64().ok_or_else(|| anyhow!("n"))? as u32)))
            .collect::<Result<Vec<_>>>()?;
        let hash_b = m["matrices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| pearl_job::spec::parse_key32(x["hash_b"].as_str().unwrap_or("")))
            .collect::<Result<Vec<_>>>()?;
        let macs_per_row = matrices.iter().map(|(k, n)| *k as u64 * *n as u64).sum();
        Ok(Self { model_id, name: m["name"].as_str().unwrap_or("").to_string(), base, matrices, hash_b, macs_per_row })
    }

    /// The registry fields every PostSegments repeats.
    pub fn head(&self) -> tx::SegmentsHead {
        tx::SegmentsHead { model_id: self.model_id, rank: self.base.rank, rows_pattern: self.base.rows_pattern.to_bytes(), cols_pattern: self.base.cols_pattern.to_bytes() }
    }

    pub fn period(&self) -> u32 {
        self.base.rows_pattern.period()
    }

    pub fn job_key(&self, i: u16) -> [u8; 32] {
        let (k, n) = self.matrices[i as usize];
        standing::matrix_job_key(&self.model_id, i, k, n, &self.base)
    }
}

/// The request blob (`cadre-request-v1`) of a one-request ticket; its blob id is `hash_a`.
pub fn request_blob(model_id: &[u8; 32], prompt_ids: &[u32], max_new: u32) -> Vec<u8> {
    // fixed field order: the same request always gives the same bytes (and price)
    format!(
        "{{\"format\":\"cadre-request-v1\",\"model_id\":\"{}\",\"prompt_ids\":{},\"max_new_tokens\":{max_new},\"sampling\":\"greedy\"}}",
        hex::encode(model_id),
        serde_json::to_string(prompt_ids).unwrap()
    )
    .into_bytes()
}

/// The descriptor blob of a balance ticket (`cadre-balance-v1`); its blob id is `hash_a`.
pub fn balance_blob(model_id: &[u8; 32], server: &[u8; 32], rows: u64) -> Vec<u8> {
    format!("{{\"format\":\"cadre-balance-v1\",\"model_id\":\"{}\",\"server\":\"{}\",\"rows\":{rows}}}", hex::encode(model_id), b58(server)).into_bytes()
}

/// The storefront spec of a ticket (SPEC 4.5): `m + n` rows (padded) of budget.
#[allow(clippy::too_many_arguments)]
pub fn ticket_spec(model: &ModelInfo, server: &[u8; 32], m: u32, n: u32, data_uri: &str, hash_a: &[u8; 32], deadline_blocks: u64, max_blocks: u32) -> Value {
    json!({
        "m": m, "n": n, "k": 1,
        "config": hex::encode(standing::ticket_config(&model.model_id)),
        "hash_a": hex::encode(hash_a), "hash_b": hex::encode(server),
        "data_uri": data_uri, "deadline_blocks": deadline_blocks, "max_blocks": max_blocks,
    })
}

/// One segment ready to post: the tx entry and the activation blob bytes.
pub struct PreparedSegment {
    pub spec: SegmentSpec,
    pub a_blob: Vec<u8>,
}

/// A round's segments: per matrix, the requests' A rows (in `dirs` order) stacked and padded
/// with zero rows to `m`; C likewise. `hash_a = commit_a(job_key, A)`, `c_hash =
/// blake3_keyed(C, job_key)`; the A URI is the activation blob's (content-addressed) id.
pub fn prepare_round(model: &ModelInfo, dirs: &[(PathBuf, u32)], m: u32, blocks_per_segment: u32) -> Result<Vec<PreparedSegment>> {
    (0..model.matrices.len()).map(|i| prepare_matrix(model, dirs, m, i, blocks_per_segment)).collect()
}

/// One matrix of [`prepare_round`].
pub fn prepare_matrix(model: &ModelInfo, dirs: &[(PathBuf, u32)], m: u32, i: usize, blocks_per_segment: u32) -> Result<PreparedSegment> {
    let total: u32 = dirs.iter().map(|d| d.1).sum();
    ensure!(total <= m && m % model.period() == 0, "round of {total} rows does not fit m = {m}");
    let (k, n) = *model.matrices.get(i).ok_or_else(|| anyhow!("model has no matrix {i}"))?;
    let (k, n) = (k as usize, n as usize);
    let mut a: Vec<i8> = Vec::with_capacity(m as usize * k);
    let mut c: Vec<i32> = Vec::with_capacity(m as usize * n);
    for (dir, rows) in dirs {
        let ab = std::fs::read(dir.join(format!("a/{i}.i8"))).with_context(|| format!("read A of matrix {i} in {}", dir.display()))?;
        ensure!(ab.len() >= *rows as usize * k, "A of matrix {i} in {}: {} bytes, need {} rows", dir.display(), ab.len(), rows);
        a.extend(ab[..*rows as usize * k].iter().map(|&x| x as i8));
        let cb = std::fs::read(dir.join(format!("c/{i}.i32"))).with_context(|| format!("read C of matrix {i}"))?;
        ensure!(cb.len() >= *rows as usize * n * 4, "C of matrix {i} in {}: too short", dir.display());
        c.extend(cb[..*rows as usize * n * 4].chunks_exact(4).map(|x| i32::from_le_bytes(x.try_into().unwrap())));
    }
    a.resize(m as usize * k, 0);
    c.resize(m as usize * n, 0);
    let key = model.job_key(i as u16);
    let a_blob = standing::encode_activation_blob(m, k as u32, &a)?;
    let spec = SegmentSpec {
        matrix: i as u16,
        m,
        k: k as u32,
        n: n as u32,
        max_blocks: blocks_per_segment.max(1),
        hash_a: commit_a(&key, &a),
        hash_b: model.hash_b.get(i).copied().unwrap_or([0; 32]),
        c_hash: result_hash(&key, &c),
        a_uri: pearl_job::data::blob_uri(&pearl_job::data::blob_id(&a_blob)),
    };
    Ok(PreparedSegment { spec, a_blob })
}

/// The oldest pending requests (in order) whose rows, padded to the row period, fit `cap`;
/// the rest wait for the next round.
pub fn round_take(pending: Vec<Served>, cap: u32, period: u32) -> Vec<Served> {
    let mut out = Vec::new();
    let mut sum: u64 = 0;
    for r in pending {
        if round_up(sum + r.rows as u64, period as u64) > cap as u64 {
            break;
        }
        sum += r.rows as u64;
        out.push(r);
    }
    out
}

fn round_up(x: u64, p: u64) -> u64 {
    x.div_ceil(p.max(1)) * p.max(1)
}

// ------------------------------------------------------------------ ledger

/// One served request: its receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Served {
    pub request_id: String,
    pub wallet: String,
    pub ticket_id: String,
    pub dir: PathBuf,
    /// Rows through every matrix: prompt + completion - 1.
    pub rows: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub body_sha256: String,
    pub output_sha256: String,
    /// sha256 (hex) of the completion text exactly as the response carries it (message content /
    /// text), so a post quoting the completion verbatim can be matched to its paid request
    /// (SPEC 4.6 thought_ref). Empty for requests served before this field existed.
    #[serde(default)]
    pub text_sha256: String,
    pub charged_lamports: Option<u64>,
    pub at_ms: u64,
    pub round: Option<u16>,
    pub row_offset: Option<u32>,
    pub ttft_s: f64,
    pub decode_tokens_per_s: Option<f64>,
    /// What an evaluator needs to replay the request (SPEC 4.7): the token ids the worker fed and
    /// produced, the stopping rule and the glue it ran. Published in the response blob; the A
    /// blobs already carry the prompt (layer 0 activations are the quantized embeddings), so
    /// this adds no new exposure. Empty for requests served before this field existed.
    #[serde(default)]
    pub prompt_ids: Vec<u32>,
    #[serde(default)]
    pub output_ids: Vec<u32>,
    #[serde(default)]
    pub max_new_tokens: u32,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub glue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoundRec {
    pub round: u16,
    pub m: u32,
    pub requests: Vec<String>,
    pub txs: Vec<String>,
}

/// One ticket this gateway serves: a prepaid balance (or a one-request ticket).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TicketState {
    pub ticket_id: String,
    /// storefront order nonce (None: posted on the chain by the wallet itself).
    pub order: Option<String>,
    pub wallet: String,
    pub rows: u64,
    /// x402 price / rows (None when unknown, e.g. a chain-native ticket).
    pub lamports_per_row: Option<u64>,
    /// rows reserved by requests in flight
    pub reserved: u64,
    /// served requests not posted in a round yet
    pub pending: Vec<String>,
    pub rounds: Vec<RoundRec>,
    /// "open" | "closing" | "delivered" | "completed" | "failed"
    pub state: String,
    pub single: bool,
    pub created_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub deadline_height: Option<u64>,
    /// Round period of this ticket (seconds); None = the gateway's `round_every`, or
    /// `big_round_every` for a ticket above 4096 rows (SPEC 5.4, ECOSYSTEM M1.5).
    #[serde(default)]
    pub round_secs: Option<u64>,
    /// Rounds this ticket's block budget holds (`max_blocks / (matrices x blocks_per_segment)`,
    /// SPEC 5.4 "Ticket block budget"). None: not known yet (read from the chain at settle).
    #[serde(default)]
    pub max_rounds: Option<u32>,
    /// The round being posted (recorded before its first PostSegments is signed, cleared when the
    /// round is in `rounds`): a round the chain has started is resumed with exactly these
    /// requests, never with whatever is pending by then (devnet box 0 2026-10-22: a resumed
    /// round restacked with later requests named m 1712 against the chain's 1280, every
    /// PostSegments was refused at the tip and sat in the mempool holding the server's nonce).
    #[serde(default)]
    pub posting: Option<PostingRound>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PostingRound {
    pub round: u16,
    pub m: u32,
    pub requests: Vec<String>,
}

impl TicketState {
    /// Rounds still allowed (None: unbounded as far as this ledger knows).
    pub fn rounds_left(&self) -> Option<u32> {
        self.max_rounds.map(|m| m.saturating_sub(self.rounds.len() as u32))
    }
    pub fn posted_rows(&self) -> u64 {
        self.rounds.iter().map(|r| r.m as u64).sum()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Ledger {
    pub tickets: BTreeMap<String, TicketState>,
    pub requests: BTreeMap<String, Served>,
    pub keys: Keys,
}

impl Ledger {
    /// Free rows of a ticket: the budget minus posted rounds and what pending and in-flight
    /// requests will need once padded into the next round.
    pub fn free(&self, t: &TicketState, period: u64) -> u64 {
        let pending: u64 = t.pending.iter().filter_map(|r| self.requests.get(r)).map(|r| r.rows as u64).sum();
        t.rows.saturating_sub(t.posted_rows() + round_up(pending + t.reserved, period))
    }

    /// Free rows a new request may draw, with the ticket's round budget: none once every
    /// round it may post is posted, and in its last round only what that round can still
    /// stack (`round_cap` rows), so a served request is never left without a round.
    pub fn free_in_budget(&self, t: &TicketState, period: u64, round_cap: u64) -> u64 {
        let base = self.free(t, period);
        match t.rounds_left() {
            Some(0) => 0,
            Some(1) => {
                let pending: u64 = t.pending.iter().filter_map(|r| self.requests.get(r)).map(|r| r.rows as u64).sum();
                base.min(round_cap.saturating_sub(round_up(pending + t.reserved, period)))
            }
            _ => base,
        }
    }
}

// ------------------------------------------------------------------ gateway

/// A node's default blob size limit (chain/blobs `max_blob_bytes`, 4 MiB): one round's A blob
/// per matrix must fit it.
pub const BLOB_LIMIT: u64 = 4 << 20;
/// Bounds of a ticket's own round period (`round_secs` in a top-up).
pub const MIN_ROUND_SECS: u64 = 60;
pub const MAX_ROUND_SECS: u64 = 86_400;

#[derive(Debug, Clone)]
pub struct Config {
    pub work_dir: PathBuf,
    pub deadline_blocks: u64,
    /// Deadline of balance tickets (blocks after the unlock).
    pub balance_deadline_blocks: u64,
    pub blocks_per_segment: u32,
    pub max_new_tokens: u32,
    pub max_prompt_tokens: u32,
    pub balance_rows: u64,
    /// Largest balance a top-up may ask for (the chain's `max_balance_rows` bounds it too).
    pub max_balance_rows: u64,
    pub round_every: Duration,
    /// Round period of tickets above 4096 rows (large house balances, M1.5).
    pub big_round_every: Duration,
    /// Most rows one round stacks: A (m x k int8) of every segment must fit one blob.
    pub max_round_rows: u32,
    pub min_free_rows: u64,
    pub close_margin_blocks: u64,
    /// Largest `max_blocks` a ticket asks for: the storefront's on-chain `max_blocks_cap`
    /// (10,000 at initialize) bounds an order's; the chain's `max_job_blocks` bounds it too.
    pub max_ticket_blocks: u32,
    pub ticket_wait: Duration,
    pub poll: Duration,
    /// Explorer base URL for receipts (`<explorer>/job/<ticket>`).
    pub explorer_url: String,
    /// This gateway's public base URL (status links: `<public_url>/v1/requests/<id>`).
    pub public_url: String,
    /// The public URL of this gateway's `/v1` root when a proxy mounts it elsewhere (cadre-api
    /// serves it at `<api>/v1/openai`). Status links are then `<public_api_root>/requests/<id>`.
    pub public_api_root: Option<String>,
    /// Inference requests at the worker at once (in its batch or its queue); one more answers
    /// 503 with Retry-After before any rows are reserved or any x402 payment is taken. Keep it at
    /// or below the worker's max_batch + max_queue (SPEC 5.4 "Worker concurrency"). 0 = no cap.
    pub max_inference: usize,
    /// Request scratch data (each served request's A/C files, `work_dir/requests/<id>`, about
    /// 2.7 MB per row on Llama-3.2-1B) is deleted once its round is posted: A is then on the node
    /// as blobs, C is committed in the segment. `max_request_data_bytes` caps what may wait for a
    /// round: a paid request that would pass it, or leave the disk under `min_free_disk_bytes`,
    /// answers 503 `insufficient_storage` before any rows are reserved or paid; above half the
    /// cap the oldest waiting rounds are posted early (SPEC 5.4 "Request data retention").
    pub max_request_data_bytes: u64,
    /// Waiting request data at which rounds are posted early (fullest first), whatever the cap:
    /// early posting starts at `min(max_request_data_bytes / 2, data_pressure_bytes)`, so a cap
    /// raised as a stopgap does not also raise the point where the backlog starts draining
    /// (box 0, 2026-10-23: a 64 GB cap would otherwise wait until 32 GB). 0 = cap / 2 only.
    pub data_pressure_bytes: u64,
    pub min_free_disk_bytes: u64,
    /// Keep request scratch data after its round is posted (debugging only).
    pub keep_request_data: bool,
    /// Server-key txs in flight at once: the node's mempool takes nonces up to 64 ahead of the
    /// account's confirmed nonce (chain/mempool `max_ahead`), so a round of 144 segments goes out
    /// in a sliding window of this many instead of batches waited on one by one.
    pub send_window: usize,
    /// A server tx at the account's next nonce that this many new blocks skip is refused by the
    /// chain (templates drop a tx that fails at the tip and the mempool keeps it): the send stops
    /// with an error instead of waiting out its timeout, and the next send replaces it.
    pub stall_blocks: u64,
    /// Highest fee a replacement may pay, as a multiple of the chain's min_fee.
    pub max_fee_multiple: u64,
    /// A ticket whose settlement failed waits this long before the next try (doubling per
    /// consecutive failure, at most 16 times), so one ticket never holds every settle pass.
    pub settle_backoff: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_dir: PathBuf::from(".cadre-gateway"),
            deadline_blocks: 400,
            balance_deadline_blocks: 10_000,
            blocks_per_segment: 1,
            max_new_tokens: 256,
            max_prompt_tokens: 2048,
            balance_rows: standing::MAX_TICKET_ROWS as u64,
            max_balance_rows: standing::MAX_TICKET_ROWS as u64,
            round_every: Duration::from_secs(60),
            big_round_every: Duration::from_secs(10_800),
            max_round_rows: 0,
            min_free_rows: 64,
            close_margin_blocks: 50,
            max_ticket_blocks: 10_000,
            ticket_wait: Duration::from_secs(900),
            poll: Duration::from_millis(1000),
            explorer_url: "http://127.0.0.1:10670".into(),
            public_url: "http://127.0.0.1:10664".into(),
            public_api_root: None,
            max_inference: 32,
            max_request_data_bytes: 16 << 30,
            data_pressure_bytes: 8 << 30,
            min_free_disk_bytes: 8 << 30,
            keep_request_data: false,
            send_window: 64,
            stall_blocks: 5,
            max_fee_multiple: 64,
            settle_backoff: Duration::from_secs(30),
        }
    }
}

pub type Logger = Box<dyn Fn(Value) + Send + Sync>;

pub struct Gateway {
    pub node: Box<dyn NodeApi>,
    pub storefront: Box<dyn Storefront>,
    pub worker: Box<dyn Worker>,
    pub evaluator: Box<dyn Evaluator>,
    pub key: SigningKey,
    pub model: ModelInfo,
    pub cfg: Config,
    pub log: Logger,
    pub ledger: Mutex<Ledger>,
    /// Txs from the server key go out one at a time (nonces).
    send_lock: Mutex<()>,
    /// One round-posting pass at a time.
    settle_lock: Mutex<()>,
    /// One closing pass at a time. Closing a ticket waits for the evaluator, whose full glue
    /// replay takes minutes (box 0, 2026-10-23: 480-565 s for 1,663-1,732 rows), so closes run
    /// on their own thread and never hold up round posting (SPEC 5.4 "Request data retention").
    close_lock: Mutex<()>,
    /// The ticket being closed now and since when (round posting skips it).
    closing_now: Mutex<Option<(String, Instant)>>,
    /// Tickets whose last close failed: (try again at, consecutive failures, last error).
    close_retry: Mutex<BTreeMap<String, (Instant, u32, String)>>,
    /// Cached `lamports_per_row` of a full balance (price, at).
    price: Mutex<Option<(u64, Instant)>>,
    /// Measured: (ttft_s, decode tokens/s) of recent requests.
    pub recent: Mutex<VecDeque<(f64, Option<f64>, u32)>>,
    /// When `discover` last ran for a wallet on an unauthenticated read (security pass V-8).
    discovered: Mutex<std::collections::HashMap<[u8; 32], Instant>>,
    /// Inference requests now at the worker (Config::max_inference).
    pub inferring: Arc<AtomicUsize>,
    /// Inference requests refused with 503 since start (gateway cap or worker queue full).
    pub refused_busy: AtomicUsize,
    /// Requests being generated now: id -> estimated scratch bytes (not in the ledger yet).
    in_flight: Mutex<BTreeMap<String, u64>>,
    /// Requests refused for disk space since start.
    pub refused_storage: AtomicUsize,
    /// Request dirs deleted since start.
    pub data_deleted: AtomicUsize,
    /// Request data bytes written (measured on disk when a request is booked) and deleted, since
    /// start: their difference over time is the backlog's growth rate.
    pub data_written_bytes: AtomicU64,
    pub data_freed_bytes: AtomicU64,
    /// Served requests whose round can no longer be posted (the ticket's deadline passed or the
    /// chain closed it): dropped from the ticket and their data deleted, since start.
    pub pending_dropped: AtomicUsize,
    /// The node's blob headroom (bytes RPC puts may still add) and when it was read; None =
    /// unknown (an older node without the field, or the read failed).
    node_blob: Mutex<Option<(Option<u64>, Instant)>>,
    /// Paid requests refused because the node's blob store is nearly full, since start.
    pub refused_node_storage: AtomicUsize,
    /// Server txs that replaced another of the server's pooled txs at the same nonce (a stuck or
    /// stale one), orphans cancelled with a 0 self-transfer, and sends stopped as stalled.
    pub tx_replaced: AtomicUsize,
    pub tx_cancelled: AtomicUsize,
    pub tx_stalls: AtomicUsize,
    /// Tickets whose last settlement failed: (try again at, consecutive failures, last error).
    settle_retry: Mutex<BTreeMap<String, (Instant, u32, String)>>,
}

/// One of the server's own txs in the node's mempool (getMempool + getTx).
#[derive(Debug, Clone, PartialEq)]
pub struct PooledTx {
    pub hash: String,
    pub fee: u64,
    /// Kind byte and body bytes, to tell the same tx at a higher fee from another one.
    pub kind: u8,
    pub body: Vec<u8>,
}

/// The fee a tx must pay to replace a pooled one (chain/mempool `replacement_floor`: 10% more,
/// at least 1).
pub fn replacement_fee(old: u64) -> u64 {
    old.saturating_add((old / 10).max(1))
}

/// Is `msg` a node refusing a tx because another one holds its nonce in the mempool?
pub fn is_nonce_pooled(msg: &str) -> bool {
    msg.contains("is pooled with a fee") || msg.contains("replacement fee")
}

/// How long a getBlobStats read of the node's blob headroom is reused.
pub const NODE_BLOB_TTL: Duration = Duration::from_secs(10);

/// Is `msg` a node refusing a putBlob for space ("blob store full" or "blob store nearly full")?
pub fn is_blob_store_full(msg: &str) -> bool {
    msg.contains("blob store full") || msg.contains("blob store nearly full")
}

/// Free bytes on the filesystem holding `p` (None when unknown).
/// Bytes of the regular files under `dir` (a request's scratch: `a/*.i8`, `c/*.i32`).
pub fn dir_bytes(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn disk_free_bytes(p: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        None
    }
}

/// One admitted inference request; dropping it frees the slot (also from the stream thread).
struct InferSlot(Arc<AtomicUsize>);
impl Drop for InferSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A service the gateway depends on (storefront-x402, the chain node's blob store) did not
/// answer: 503 with Retry-After, never the catch-all 400. A stopped storefront made every top-up
/// quote a 400 "invalid_request_error" (2026-10-23 04:39-04:59 UTC), which callers read as their
/// own mistake; the house judge logged "top-up quote: status code 400" and gave up on hire 73.
fn upstream_down(what: &str, e: anyhow::Error) -> HttpResponse {
    let mut r = err(503, "upstream_unavailable", format!("{what}: {e:#}"));
    r.headers.push(("retry-after".into(), "30".into()));
    r
}

fn busy(msg: impl std::fmt::Display) -> HttpResponse {
    let mut r = err(503, "overloaded", msg);
    r.headers.push(("retry-after".into(), "2".into()));
    r
}

/// Unauthenticated balance reads rescan the chain for a wallet at most this often.
pub const DISCOVER_TTL: Duration = Duration::from_secs(15);
/// Wallets remembered for the TTL above; past this, expired entries are dropped first.
const DISCOVER_CACHE_MAX: usize = 10_000;

pub enum Payload {
    Json(Value),
    /// SSE body, chunk by chunk.
    Stream(mpsc::Receiver<Vec<u8>>),
}

pub struct HttpResponse {
    pub status: u16,
    pub body: Payload,
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    pub fn json(&self) -> &Value {
        match &self.body {
            Payload::Json(v) => v,
            Payload::Stream(_) => &Value::Null,
        }
    }
}

fn resp(status: u16, body: Value) -> HttpResponse {
    HttpResponse { status, body: Payload::Json(body), headers: vec![] }
}

fn err(status: u16, kind: &str, msg: impl std::fmt::Display) -> HttpResponse {
    resp(status, json!({"error": {"message": msg.to_string(), "type": kind, "code": status}}))
}

fn rand_id(prefix: &str) -> String {
    let mut raw = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut raw);
    format!("{prefix}{}", hex::encode(raw))
}

/// A parsed inference request (chat or text).
struct InferReq {
    chat: bool,
    ids: Vec<u32>,
    max_new: u32,
    stop: Vec<String>,
    stream: bool,
    tools: bool,
}

impl Gateway {
    #[allow(clippy::too_many_arguments)]
    pub fn new(node: Box<dyn NodeApi>, storefront: Box<dyn Storefront>, worker: Box<dyn Worker>, evaluator: Box<dyn Evaluator>, key: SigningKey, model: ModelInfo, cfg: Config, log: Logger) -> Self {
        let mut ledger: Ledger = std::fs::read(cfg.work_dir.join("ledger.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        ledger.keys.audience = Some(b58(&key.verifying_key().to_bytes()));
        ledger.keys.require_audience = std::env::var(auth::REQUIRE_AUDIENCE_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        Self {
            node,
            storefront,
            worker,
            evaluator,
            key,
            model,
            cfg,
            log,
            ledger: Mutex::new(ledger),
            send_lock: Mutex::new(()),
            settle_lock: Mutex::new(()),
            close_lock: Mutex::new(()),
            closing_now: Mutex::new(None),
            close_retry: Mutex::new(BTreeMap::new()),
            price: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
            discovered: Mutex::new(std::collections::HashMap::new()),
            inferring: Arc::new(AtomicUsize::new(0)),
            refused_busy: AtomicUsize::new(0),
            in_flight: Mutex::new(BTreeMap::new()),
            refused_storage: AtomicUsize::new(0),
            data_deleted: AtomicUsize::new(0),
            data_written_bytes: AtomicU64::new(0),
            data_freed_bytes: AtomicU64::new(0),
            pending_dropped: AtomicUsize::new(0),
            node_blob: Mutex::new(None),
            refused_node_storage: AtomicUsize::new(0),
            tx_replaced: AtomicUsize::new(0),
            tx_cancelled: AtomicUsize::new(0),
            tx_stalls: AtomicUsize::new(0),
            settle_retry: Mutex::new(BTreeMap::new()),
        }
    }

    /// `discover` for an unauthenticated caller: at most once per wallet per
    /// DISCOVER_TTL, so anonymous balance polls cannot fan out into node calls
    /// (getAccountTxs + up to 100 getTicket each). Returns whether it ran.
    fn discover_throttled(&self, w: &[u8; 32]) -> bool {
        {
            let mut d = self.discovered.lock().unwrap();
            let now = Instant::now();
            if d.get(w).is_some_and(|t| now.duration_since(*t) < DISCOVER_TTL) {
                return false;
            }
            if d.len() >= DISCOVER_CACHE_MAX {
                d.retain(|_, t| now.duration_since(*t) < DISCOVER_TTL);
                if d.len() >= DISCOVER_CACHE_MAX {
                    return false;
                }
            }
            d.insert(*w, now);
        }
        self.discover(w);
        true
    }

    pub fn server(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    fn save(&self, l: &Ledger) {
        let _ = std::fs::create_dir_all(&self.cfg.work_dir);
        let p = self.cfg.work_dir.join("ledger.json");
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(l).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    fn with_ledger<T>(&self, f: impl FnOnce(&mut Ledger) -> T) -> T {
        let mut g = self.ledger.lock().unwrap();
        let r = f(&mut g);
        self.save(&g);
        r
    }

    // ------------------------------------------------------------ HTTP

    pub fn handle(self: &Arc<Self>, method: &str, path_q: &str, headers: &[(String, String)], body: &[u8]) -> HttpResponse {
        let (path, query) = path_q.split_once('?').unwrap_or((path_q, ""));
        let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}="))).map(String::from);
        let auth = match self.ledger.lock().unwrap().keys.check(method, path, headers, body, now_ms()) {
            Ok(a) => a,
            Err(e) => return err(401, "authentication_error", e),
        };
        let r: Result<HttpResponse> = (|| match (method, path) {
            ("GET", "/health") => Ok(resp(200, json!({"ok": true, "server": b58(&self.server()), "model_id": hex::encode(self.model.model_id), "model": self.model.name, "api": "cadre-gateway v1", "auth": [auth::AUTH_DOMAIN_V2, auth::AUTH_DOMAIN]}))),
            ("GET", "/v1/models") => Ok(resp(200, self.models_json())),
            ("GET", "/v1/stats") => Ok(resp(200, self.stats_json())),
            ("POST", "/v1/keys") => {
                let w = auth_wallet_signed(&auth)?;
                let (key_id, api_key) = self.with_ledger(|l| l.keys.create(&w, now_ms()));
                Ok(resp(201, json!({"key_id": key_id, "api_key": api_key, "wallet": b58(&w), "note": "shown once; send it as Authorization: Bearer"})))
            }
            ("GET", "/v1/keys") => {
                let w = auth_wallet_signed(&auth)?;
                let list: Vec<Value> = self.ledger.lock().unwrap().keys.list(&w).into_iter().map(|k| json!({"key_id": k.key_id, "created_ms": k.created_ms, "last_used_ms": k.last_used_ms})).collect();
                Ok(resp(200, json!({"wallet": b58(&w), "keys": list})))
            }
            ("DELETE", p) if p.starts_with("/v1/keys/") => {
                let w = auth_wallet_signed(&auth)?;
                self.with_ledger(|l| l.keys.revoke(&w, &p["/v1/keys/".len()..]))?;
                Ok(resp(200, json!({"revoked": &p["/v1/keys/".len()..]})))
            }
            ("POST", "/v1/balance/topup") => self.topup(headers, body),
            ("GET", "/v1/balance") => {
                // an authenticated caller always rescans; anonymous reads are throttled (V-8)
                match (auth.wallet(), q("wallet")) {
                    (Some(w), _) => {
                        self.discover(&w);
                        Ok(resp(200, self.balance_json(&w)))
                    }
                    (None, Some(w)) => {
                        let w = auth::parse_wallet(&w)?;
                        self.discover_throttled(&w);
                        Ok(resp(200, self.balance_json(&w)))
                    }
                    _ => bail!("authenticate, or pass ?wallet=ADDR"),
                }
            }
            ("GET", p) if p.starts_with("/v1/prepaid/") => {
                let w = auth::parse_wallet(&p["/v1/prepaid/".len()..])?;
                self.discover_throttled(&w);
                Ok(resp(200, self.balance_json(&w)))
            }
            ("POST", "/v1/balance/close") => {
                let w = auth.wallet().ok_or_else(|| anyhow!("authenticate to close your balance"))?;
                let n = self.with_ledger(|l| {
                    let mut n = 0;
                    for t in l.tickets.values_mut().filter(|t| t.wallet == b58(&w) && t.state == "open") {
                        t.state = "closing".into();
                        n += 1;
                    }
                    n
                });
                let me = Arc::clone(self);
                std::thread::spawn(move || me.settle_pass());
                Ok(resp(202, json!({"closing": n})))
            }
            ("POST", "/v1/chat/completions") => self.infer(true, &auth, headers, body),
            ("POST", "/v1/completions") => self.infer(false, &auth, headers, body),
            ("GET", "/v1/receipts") => {
                let w = auth::parse_wallet(&q("wallet").ok_or_else(|| anyhow!("pass ?wallet=ADDR"))?)?;
                let l = self.ledger.lock().unwrap();
                let list: Vec<Value> = l.requests.values().filter(|r| r.wallet == b58(&w)).rev().take(200).map(|r| self.receipt_json(&l, r)).collect();
                Ok(resp(200, json!({"wallet": b58(&w), "receipts": list})))
            }
            ("GET", p) if p.starts_with("/v1/requests/") || p.starts_with("/v1/receipts/") => Ok(self.request_status(p.rsplit('/').next().unwrap_or(""))),
            _ => Ok(err(404, "not_found", format!("no route {method} {path}"))),
        })();
        r.unwrap_or_else(|e| err(400, "invalid_request_error", format!("{e:#}")))
    }

    fn lamports_per_row(&self) -> Option<u64> {
        if let Some((p, at)) = *self.price.lock().unwrap() {
            if at.elapsed() < Duration::from_secs(60) {
                return Some(p);
            }
        }
        let rows = self.cfg.balance_rows;
        let blob = balance_blob(&self.model.model_id, &self.server(), rows);
        let spec = ticket_spec(&self.model, &self.server(), rows as u32 - 1, 1, &pearl_job::data::blob_uri(&pearl_job::data::blob_id(&blob)),
            &pearl_job::data::blob_id(&blob), self.cfg.balance_deadline_blocks, self.ticket_budget(rows, None).ok()?.0);
        let (st, v, _) = self.storefront.post_job(&json!({"spec": spec}), None).ok()?;
        if st != 402 {
            return None;
        }
        let price: u64 = v["accepts"][0]["maxAmountRequired"].as_str()?.parse().ok()?;
        let per = price.div_ceil(rows);
        *self.price.lock().unwrap() = Some((per, Instant::now()));
        Some(per)
    }

    /// Largest balance a top-up may ask for: the gateway's own cap and the chain's
    /// `max_balance_rows` (getParams; 4096 on a chain without the key), never below 4096.
    pub fn max_balance_rows(&self) -> u64 {
        let chain = self.node.call("getParams", json!({})).ok().and_then(|v| v["max_balance_rows"].as_u64()).unwrap_or(standing::MAX_TICKET_ROWS as u64);
        self.cfg.max_balance_rows.min(chain).max(standing::MAX_TICKET_ROWS as u64)
    }

    /// Most rows one round may stack: every segment's A blob (`"PSA1" || m || k || m x k
    /// int8`, 12 bytes of header) must fit one blob of `BLOB_LIMIT` bytes at the model's
    /// widest k, rounded down to the row period. `max_round_rows` (if set) lowers it.
    pub fn round_rows_cap(&self) -> u32 {
        let p = self.model.period().max(1);
        let kmax = self.model.matrices.iter().map(|(k, _)| *k).max().unwrap_or(1).max(1) as u64;
        let by_blob = ((BLOB_LIMIT - 12) / kmax) as u32 / p * p;
        if self.cfg.max_round_rows > 0 { by_blob.min(self.cfg.max_round_rows / p * p) } else { by_blob }
    }

    /// Round period of a ticket of `rows` rows with an optional own period.
    pub fn round_period(&self, rows: u64, own: Option<u64>) -> Duration {
        match own {
            Some(s) => Duration::from_secs(s),
            None if rows > standing::MAX_TICKET_ROWS as u64 => self.cfg.big_round_every,
            None => self.cfg.round_every,
        }
    }

    /// Blocks one round of segments allocates: one segment per registered matrix,
    /// `blocks_per_segment` blocks each (640 on Llama-3.1-8B, 144 on Llama-3.2-1B).
    pub fn round_blocks(&self) -> u32 {
        self.model.matrices.len() as u32 * self.cfg.blocks_per_segment.max(1)
    }

    /// Ticket block budget (SPEC 5.4 "Ticket block budget", 2026-10-21): `(max_blocks,
    /// max_rounds)` for a balance of `rows` rows with an optional own round period. It scales
    /// with the model's matrix count: `max_blocks = max_rounds x round_blocks`, where
    /// `max_rounds` is every round the ticket can post over its life (one per round period
    /// over the deadline, plus the rounds its rows can fill early, never more than rows / row
    /// period), cut to the largest budget an order and the chain accept
    /// (`min(max_ticket_blocks, max_job_blocks) / round_blocks`). The settlement loop never
    /// posts more than `max_rounds` rounds (`Ledger::free_in_budget`), so a ticket can not
    /// run out of blocks mid-round.
    pub fn ticket_budget(&self, rows: u64, own_round_secs: Option<u64>) -> Result<(u32, u32)> {
        let per = self.round_blocks() as u64;
        let params = self.node.call("getParams", json!({})).unwrap_or(Value::Null);
        let chain_cap = params["max_job_blocks"].as_u64().unwrap_or(100_000);
        let cap = (self.cfg.max_ticket_blocks as u64).min(chain_cap);
        ensure!(per <= cap, "one round of this model allocates {per} blocks, above the ticket cap {cap} (--max-ticket-blocks, the storefront's max_blocks_cap, the chain's max_job_blocks)");
        let block_ms = params["target_block_time_ms"].as_u64().unwrap_or(60_000).max(1);
        let life_s = self.cfg.balance_deadline_blocks.saturating_mul(block_ms) / 1000;
        let period_s = self.round_period(rows, own_round_secs).as_secs().max(1);
        let p = self.model.period().max(1) as u64;
        let by_rows = (rows / p).max(1);
        let by_time = life_s.div_ceil(period_s) + 1 + rows / (self.round_rows_cap().max(1) as u64);
        let rounds = by_rows.min(by_time).min(cap / per).max(1);
        Ok(((rounds * per) as u32, rounds as u32))
    }

    pub fn models_json(&self) -> Value {
        let per = self.lamports_per_row();
        json!({"object": "list", "data": [{
            "id": self.model.name, "object": "model", "created": 0, "owned_by": "cadre",
            "context_length": standing::MAX_TICKET_ROWS,
            "cadre": {
                "model_id": hex::encode(self.model.model_id), "recipe": standing::RECIPE_NAME,
                "quantization": "int7 (W7A7: per-channel weights with SmoothQuant folded, per-token asymmetric activations)",
                "matrices": self.model.matrices.len(), "macs_per_row": self.model.macs_per_row.to_string(),
                "decoding": "greedy (temperature and top_p are accepted and ignored)",
                "pricing": {"lamports_per_row": per.map(|p| p.to_string()), "prompt_lamports_per_token": per.map(|p| p.to_string()),
                    "completion_lamports_per_token": per.map(|p| p.to_string()),
                    "basis": "x402 price of a full balance (storefront knobs over 2 x macs_per_row x rows) / rows; one row = one token",
                    "default": true, "usd": null, "usd_note": "no price source in this stack"},
                "server": b58(&self.server()),
            },
        }]})
    }

    pub fn stats_json(&self) -> Value {
        // before any lock below: request_data_bytes takes the ledger lock itself
        let nb = self.node_blob.lock().unwrap().and_then(|(h, _)| h);
        let data = json!({"bytes_waiting": self.request_data_bytes(), "max_bytes": self.cfg.max_request_data_bytes, "disk_free_bytes": disk_free_bytes(&self.cfg.work_dir),
            "min_free_disk_bytes": self.cfg.min_free_disk_bytes, "refused_storage": self.refused_storage.load(Ordering::SeqCst), "deleted": self.data_deleted.load(Ordering::SeqCst),
            "written_bytes": self.data_written_bytes.load(Ordering::SeqCst), "freed_bytes": self.data_freed_bytes.load(Ordering::SeqCst),
            "pressure_bytes": self.pressure_bytes(), "pending_dropped": self.pending_dropped.load(Ordering::SeqCst),
            "node_blob_headroom_bytes": nb, "refused_node_storage": self.refused_node_storage.load(Ordering::SeqCst)});
        let r = self.recent.lock().unwrap();
        let n = r.len();
        let mean = |f: &dyn Fn(&(f64, Option<f64>, u32)) -> Option<f64>| {
            let v: Vec<f64> = r.iter().filter_map(f).collect();
            if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
        };
        let l = self.ledger.lock().unwrap();
        json!({
            "requests_total": l.requests.len(), "recent": n,
            "ttft_s_mean": mean(&|x| Some(x.0)), "decode_tokens_per_s_mean": mean(&|x| x.1),
            "tickets": l.tickets.len(), "rounds_posted": l.tickets.values().map(|t| t.rounds.len()).sum::<usize>(),
            "inference": {"at_worker": self.inferring.load(Ordering::SeqCst), "max": self.cfg.max_inference, "refused_busy": self.refused_busy.load(Ordering::SeqCst)},
            "request_data": data,
            "settle": {"tx_replaced": self.tx_replaced.load(Ordering::SeqCst), "tx_cancelled": self.tx_cancelled.load(Ordering::SeqCst),
                "tx_stalls": self.tx_stalls.load(Ordering::SeqCst), "rounds_in_progress": l.tickets.values().filter(|t| t.posting.is_some()).count(),
                "backoff": self.settle_backlog_json(), "closing": self.closing_json(), "close_backoff": self.close_backlog_json()},
            "note": "measured on this gateway's worker; nothing estimated",
        })
    }

    fn balance_json(&self, w: &[u8; 32]) -> Value {
        let cap = self.round_rows_cap() as u64;
        let l = self.ledger.lock().unwrap();
        let p = self.model.period() as u64;
        let ws = b58(w);
        let ts: Vec<Value> = l
            .tickets
            .values()
            .filter(|t| t.wallet == ws)
            .map(|t| {
                let pending: u64 = t.pending.iter().filter_map(|r| l.requests.get(r)).map(|r| r.rows as u64).sum();
                json!({"ticket_id": t.ticket_id, "order": t.order, "status": t.state, "rows": t.rows, "rows_reserved": t.reserved,
                    "rows_posted": t.posted_rows(), "rows_pending": pending, "rows_free": if t.state == "open" { l.free_in_budget(t, p, cap) } else { 0 },
                    "max_rounds": t.max_rounds,
                    "rounds": t.rounds.len(), "lamports_per_row": t.lamports_per_row.map(|x| x.to_string()), "single_request": t.single,
                    "deadline_height": t.deadline_height, "explorer_url": self.explorer(&t.ticket_id)})
            })
            .collect();
        let free: u64 = l.tickets.values().filter(|t| t.wallet == ws && t.state == "open").map(|t| l.free_in_budget(t, p, cap)).sum();
        json!({"wallet": ws, "rows_free": free, "tickets": ts})
    }

    fn explorer(&self, ticket: &str) -> String {
        format!("{}/job/{ticket}", self.cfg.explorer_url.trim_end_matches('/'))
    }

    /// Public status link of one served request (receipt `status_url`, `X-Cadre-Receipt`).
    pub fn status_url(&self, request_id: &str) -> String {
        match &self.cfg.public_api_root {
            Some(root) => format!("{}/requests/{request_id}", root.trim_end_matches('/')),
            None => format!("{}/v1/requests/{request_id}", self.cfg.public_url.trim_end_matches('/')),
        }
    }

    fn receipt_json(&self, l: &Ledger, r: &Served) -> Value {
        let t = l.tickets.get(&r.ticket_id);
        json!({
            "request_id": r.request_id, "ticket_id": r.ticket_id, "order": t.and_then(|t| t.order.clone()), "wallet": r.wallet,
            "rows": r.rows, "prompt_tokens": r.prompt_tokens, "completion_tokens": r.completion_tokens,
            "charged_lamports": r.charged_lamports.map(|x| x.to_string()), "lamports_per_row": t.and_then(|t| t.lamports_per_row).map(|x| x.to_string()),
            "round": r.round, "row_offset": r.row_offset, "body_sha256": r.body_sha256, "output_sha256": r.output_sha256, "text_sha256": r.text_sha256,
            "ticket_status": t.map(|t| t.state.clone()), "status_url": self.status_url(&r.request_id),
            "explorer_url": self.explorer(&r.ticket_id), "decoding": "greedy", "at_ms": r.at_ms,
            "measured": {"ttft_s": r.ttft_s, "decode_tokens_per_s": r.decode_tokens_per_s},
        })
    }

    fn request_status(&self, id: &str) -> HttpResponse {
        let (rec, ticket) = {
            let l = self.ledger.lock().unwrap();
            match l.requests.get(id) {
                Some(r) => (self.receipt_json(&l, r), r.ticket_id.clone()),
                None => match l.tickets.get(id) {
                    Some(t) => (serde_json::to_value(t).unwrap_or(Value::Null), t.ticket_id.clone()),
                    None => return err(404, "not_found", "unknown request or ticket"),
                },
            }
        };
        let chain = self.node.call("getTicket", json!({"ticket_id": ticket})).unwrap_or(Value::Null);
        resp(200, json!({"receipt": rec, "ticket": chain}))
    }

    /// Chain-native balances: open tickets the wallet posted itself that name this gateway.
    fn discover(&self, w: &[u8; 32]) {
        let Ok(v) = self.node.call("getAccountTxs", json!({"account": b58(w), "limit": 100})) else { return };
        for t in v["txs"].as_array().into_iter().flatten() {
            if t["kind"] != json!("PostJob") {
                continue;
            }
            let Some(n) = t["nonce"].as_u64() else { continue };
            let id = hex::encode(pearl_job::merkle::blake3_digest(&[&w[..], &n.to_le_bytes()].concat(), None));
            if self.ledger.lock().unwrap().tickets.contains_key(&id) {
                continue;
            }
            let Ok(Some(j)) = self.node.call("getTicket", json!({"ticket_id": id})).map(|x| (!x.is_null()).then_some(x)) else { continue };
            let st = &j["standing"];
            if j["status"] != json!("open") || st["server"].as_str() != Some(b58(&self.server()).as_str()) || st["model_id"].as_str() != Some(hex::encode(self.model.model_id).as_str()) {
                continue;
            }
            let rows = st["rows"].as_u64().unwrap_or(0);
            self.with_ledger(|l| {
                l.tickets.entry(id.clone()).or_insert(TicketState {
                    ticket_id: id.clone(), order: None, wallet: b58(w), rows, lamports_per_row: None, reserved: 0, pending: vec![], rounds: vec![],
                    state: "open".into(), single: false, created_ms: now_ms(), error: None, deadline_height: j["deadline_height"].as_u64(), round_secs: None,
                    max_rounds: j["max_blocks"].as_u64().map(|b| (b / self.round_blocks().max(1) as u64) as u32),
                    posting: None,
                });
            });
        }
    }

    fn topup(&self, headers: &[(String, String)], body: &[u8]) -> Result<HttpResponse> {
        let b: Value = if body.is_empty() { json!({}) } else { serde_json::from_slice(body).context("body is not JSON")? };
        let rows = b["rows"].as_u64().unwrap_or(self.cfg.balance_rows);
        let p = self.model.period() as u64;
        let max = self.max_balance_rows();
        ensure!(rows >= p && rows % p == 0 && rows <= max, "rows must be a multiple of {p} in [{p}, {max}]");
        let round_secs = match b.get("round_secs").filter(|v| !v.is_null()) {
            None => None,
            Some(v) => {
                let r = v.as_u64().ok_or_else(|| anyhow!("round_secs must be an integer"))?;
                ensure!((MIN_ROUND_SECS..=MAX_ROUND_SECS).contains(&r), "round_secs must be in [{MIN_ROUND_SECS}, {MAX_ROUND_SECS}]");
                Some(r)
            }
        };
        let blob = balance_blob(&self.model.model_id, &self.server(), rows);
        let uri = match self.node.put_blob(&blob) {
            Ok(u) => u,
            Err(e) => return Ok(upstream_down("put the balance descriptor", e)),
        };
        let (max_blocks, max_rounds) = self.ticket_budget(rows, round_secs)?;
        let spec = ticket_spec(&self.model, &self.server(), rows as u32 - 1, 1, &uri, &pearl_job::data::blob_id(&blob), self.cfg.balance_deadline_blocks, max_blocks);
        let mut sf = json!({"spec": spec});
        if let Some(r) = b.get("recipient").filter(|r| !r.is_null()) {
            sf["recipient"] = r.clone();
        }
        let xp = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("x-payment")).map(|(_, v)| v.as_str());
        let (st, v, xpr) = match self.storefront.post_job(&sf, xp) {
            Ok(x) => x,
            Err(e) => return Ok(upstream_down("the storefront did not answer the top-up", e)),
        };
        if st != 201 {
            let mut v = v;
            if let Some(o) = v.as_object_mut() {
                o.insert("cadre".into(), json!({"rows": rows, "model": self.model.name, "server": b58(&self.server())}));
            }
            return Ok(resp(st, v));
        }
        let ticket = v["job_id"].as_str().ok_or_else(|| anyhow!("storefront reply has no job_id"))?.to_string();
        let wallet = v["payment"]["payer"].as_str().map(String::from).or_else(|| b["recipient"].as_str().map(String::from)).ok_or_else(|| anyhow!("no payer in the storefront reply"))?;
        let paid: Option<u64> = v["payment"]["amount"].as_str().and_then(|x| x.parse().ok());
        let t = TicketState {
            ticket_id: ticket.clone(), order: v["id"].as_str().map(String::from), wallet: wallet.clone(), rows,
            lamports_per_row: paid.map(|p| p.div_ceil(rows)), reserved: 0, pending: vec![], rounds: vec![], state: "open".into(),
            single: false, created_ms: now_ms(), error: None, deadline_height: None, round_secs, max_rounds: Some(max_rounds),
            posting: None,
        };
        self.with_ledger(|l| l.tickets.insert(ticket.clone(), t));
        (self.log)(json!({"event": "topup", "ticket": ticket, "wallet": wallet, "rows": rows, "order": v["id"]}));
        let period_s = self.round_period(rows, round_secs).as_secs();
        let mut r = resp(201, json!({"ticket_id": ticket, "order": v["id"], "rows": rows, "wallet": wallet, "lamports_paid": paid.map(|p| p.to_string()), "explorer_url": self.explorer(&ticket),
            "round_secs": period_s, "max_round_rows": self.round_rows_cap(), "max_rounds": max_rounds, "max_blocks": max_blocks}));
        if let Some(x) = xpr {
            r.headers.push(("x-payment-response".into(), x));
        }
        Ok(r)
    }

    fn parse_infer(&self, chat: bool, b: &Value) -> Result<InferReq> {
        let (ids, tools) = if chat {
            let msgs = b.get("messages").filter(|m| m.is_array()).ok_or_else(|| anyhow!("messages is required"))?;
            let tools = b.get("tools").filter(|t| t.is_array() && !t.as_array().unwrap().is_empty());
            (self.worker.chat_ids(msgs, tools)?, tools.is_some())
        } else {
            match (&b["prompt_ids"], &b["prompt"]) {
                (Value::Array(a), _) => (a.iter().map(|x| x.as_u64().map(|y| y as u32).ok_or_else(|| anyhow!("prompt_ids must be integers"))).collect::<Result<_>>()?, false),
                (_, Value::String(p)) => (self.worker.tokenize(p)?, false),
                _ => bail!("prompt is required"),
            }
        };
        ensure!(!ids.is_empty() && ids.len() as u32 <= self.cfg.max_prompt_tokens, "prompt must be 1..={} tokens", self.cfg.max_prompt_tokens);
        let max_new = b["max_completion_tokens"].as_u64().or(b["max_tokens"].as_u64()).unwrap_or(64) as u32;
        ensure!((1..=self.cfg.max_new_tokens).contains(&max_new), "max_tokens must be 1..={}", self.cfg.max_new_tokens);
        let stop = match &b["stop"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            _ => vec![],
        };
        Ok(InferReq { chat, ids, max_new, stop, stream: b["stream"] == json!(true), tools })
    }

    /// Take an inference slot (None when `max_inference` are at the worker already).
    fn admit_inference(&self) -> Option<InferSlot> {
        let n = self.inferring.fetch_add(1, Ordering::SeqCst);
        if self.cfg.max_inference > 0 && n >= self.cfg.max_inference {
            self.inferring.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(InferSlot(self.inferring.clone()))
    }

    /// `/v1/chat/completions` and `/v1/completions`.
    fn infer(self: &Arc<Self>, chat: bool, auth: &Auth, headers: &[(String, String)], body: &[u8]) -> Result<HttpResponse> {
        let t_req = Instant::now();
        let b: Value = serde_json::from_slice(body).context("body is not JSON")?;
        let r = self.parse_infer(chat, &b)?;
        // backpressure first: a request over the cap reserves nothing and pays nothing
        let slot = match self.admit_inference() {
            Some(s) => s,
            None => {
                self.refused_busy.fetch_add(1, Ordering::SeqCst);
                return Ok(busy(format!("{} inference requests at the worker (max {}); retry shortly", self.inferring.load(Ordering::SeqCst), self.cfg.max_inference)));
            }
        };
        let need_bytes = (r.ids.len() as u64 + r.max_new as u64).saturating_sub(1) * self.bytes_per_row();
        if let Some(why) = self.storage_refusal(need_bytes) {
            self.refused_storage.fetch_add(1, Ordering::SeqCst);
            let mut x = err(503, "insufficient_storage", why);
            x.headers.push(("retry-after".into(), "30".into()));
            return Ok(x);
        }
        let need_rows = (r.ids.len() as u64 + r.max_new as u64).saturating_sub(1);
        if let Some(why) = self.node_storage_refusal(need_rows) {
            self.refused_node_storage.fetch_add(1, Ordering::SeqCst);
            let mut x = err(503, "insufficient_storage", why);
            x.headers.push(("retry-after".into(), "60".into()));
            return Ok(x);
        }
        let need = (r.ids.len() as u64 + r.max_new as u64).saturating_sub(1);
        let p = self.model.period() as u64;
        // a request is posted whole in one round, whose A blobs must fit one blob each
        ensure!(round_up(need, p) <= self.round_rows_cap() as u64, "prompt + max_tokens is {need} rows, above one settlement round ({} rows)", self.round_rows_cap());
        let cap = self.round_rows_cap() as u64;
        let ticket = match auth.wallet() {
            Some(w) => {
                // draw on the wallet's balance: reserve the rows now
                let reserve = |l: &mut Ledger| -> Option<String> {
                    let ws = b58(&w);
                    let id = l.tickets.values().filter(|t| t.wallet == ws && t.state == "open" && !t.single && l.free_in_budget(t, p, cap) >= need).map(|t| t.ticket_id.clone()).next()?;
                    l.tickets.get_mut(&id).unwrap().reserved += need;
                    Some(id)
                };
                match self.with_ledger(reserve) {
                    Some(t) => t,
                    None => {
                        self.discover(&w);
                        match self.with_ledger(reserve) {
                            Some(t) => t,
                            None => return Ok(err(402, "insufficient_balance", format!("no prepaid balance with {need} free rows for {}: POST /v1/balance/topup", b58(&w)))),
                        }
                    }
                }
            }
            None => {
                // per-request x402: one ticket for this request
                let req_blob = request_blob(&self.model.model_id, &r.ids, r.max_new);
                let hash = pearl_job::data::blob_id(&req_blob);
                let uri = match self.node.put_blob(&req_blob) {
                    Ok(u) => u,
                    Err(e) => return Ok(upstream_down("put the request blob", e)),
                };
                let spec = ticket_spec(&self.model, &self.server(), r.ids.len() as u32, r.max_new, &uri, &hash, self.cfg.deadline_blocks,
                    self.model.matrices.len() as u32 * self.cfg.blocks_per_segment.max(1));
                let xp = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("x-payment")).map(|(_, v)| v.as_str());
                let mut sfb = json!({"spec": spec});
                if let Some(rc) = b.get("recipient").filter(|x| !x.is_null()) {
                    sfb["recipient"] = rc.clone();
                }
                let (st, v, _) = match self.storefront.post_job(&sfb, xp) {
                    Ok(x) => x,
                    Err(e) => return Ok(upstream_down("the storefront did not answer the x402 request", e)),
                };
                if st != 201 {
                    let mut v = v;
                    if let Some(o) = v.as_object_mut() {
                        o.insert("cadre".into(), json!({"prompt_tokens": r.ids.len(), "max_tokens": r.max_new, "rows": round_up(r.ids.len() as u64 + r.max_new as u64, p),
                            "note": "pay this one request over x402, or top up a prepaid balance (POST /v1/balance/topup) and authenticate"}));
                    }
                    return Ok(resp(st, v));
                }
                let id = v["job_id"].as_str().ok_or_else(|| anyhow!("storefront reply has no job_id"))?.to_string();
                let rows = round_up(r.ids.len() as u64 + r.max_new as u64, p);
                let paid: Option<u64> = v["payment"]["amount"].as_str().and_then(|x| x.parse().ok());
                let wallet = v["payment"]["payer"].as_str().unwrap_or("").to_string();
                let t = TicketState {
                    ticket_id: id.clone(), order: v["id"].as_str().map(String::from), wallet, rows, lamports_per_row: paid.map(|x| x.div_ceil(rows)),
                    reserved: need, pending: vec![], rounds: vec![], state: "open".into(), single: true, created_ms: now_ms(), error: None, deadline_height: None,
                    round_secs: None, max_rounds: Some(1),
                    posting: None,
                };
                self.with_ledger(|l| l.tickets.insert(id.clone(), t));
                id
            }
        };
        let request_id = rand_id("req_");
        let dir = self.cfg.work_dir.join("requests").join(&request_id);
        self.in_flight.lock().unwrap().insert(request_id.clone(), need_bytes);
        let model_name = self.model.name.clone();
        let created = now_ms() / 1000;
        let wallet = auth.wallet().map(|w| b58(&w)).unwrap_or_else(|| self.ledger.lock().unwrap().tickets[&ticket].wallet.clone());
        let body_sha = auth::sha256_hex(body);
        let opts = GenOpts { stop: r.stop.clone(), pad: false };
        if !r.stream {
            let mut first: Option<f64> = None;
            let gen = self.worker.generate(&r.ids, r.max_new, &dir, &opts, &mut |_t| {
                first.get_or_insert(t_req.elapsed().as_secs_f64());
            });
            drop(slot);
            let gen = match gen {
                Ok(g) => g,
                Err(e) => {
                    self.with_ledger(|l| l.tickets.get_mut(&ticket).map(|t| t.reserved = t.reserved.saturating_sub(need)));
                    self.in_flight.lock().unwrap().remove(&request_id);
                    self.remove_request_data(&dir);
                    if format!("{e:#}").contains(transport::WORKER_BUSY) {
                        self.refused_busy.fetch_add(1, Ordering::SeqCst);
                        return Ok(busy(format!("{e:#}")));
                    }
                    return Err(e);
                }
            };
            let ttft = first.unwrap_or_else(|| t_req.elapsed().as_secs_f64());
            let (rec, receipt) = self.record(&request_id, &ticket, &wallet, &dir, &r, &gen, need, &body_sha, ttft);
            let out = completion_json(r.chat, r.tools, &request_id, created, &model_name, &gen, &receipt);
            let mut h = receipt_headers(&rec, &receipt);
            h.push(("content-type".into(), "application/json".into()));
            return Ok(HttpResponse { status: 200, body: Payload::Json(out), headers: h });
        }
        // streaming: SSE chunks from the worker's token stream
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let me = Arc::clone(self);
        let id_chunk = if r.chat { format!("chatcmpl-{request_id}") } else { format!("cmpl-{request_id}") };
        let rid = request_id.clone();
        std::thread::spawn(move || {
            let _slot = slot;
            let request_id = rid;
            let sse = |v: Value| format!("data: {v}\n\n").into_bytes();
            let chunk = |delta: Value, finish: Value| {
                if r.chat {
                    json!({"id": id_chunk, "object": "chat.completion.chunk", "created": created, "model": model_name, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
                } else {
                    json!({"id": id_chunk, "object": "text_completion", "created": created, "model": model_name, "choices": [{"index": 0, "text": delta["content"], "finish_reason": finish}]})
                }
            };
            if r.chat {
                let _ = tx.send(sse(chunk(json!({"role": "assistant", "content": ""}), Value::Null)));
            }
            let mut first: Option<f64> = None;
            let gen = me.worker.generate(&r.ids, r.max_new, &dir, &opts, &mut |t| {
                first.get_or_insert(t_req.elapsed().as_secs_f64());
                let _ = tx.send(sse(chunk(json!({"content": t}), Value::Null)));
            });
            match gen {
                Ok(gen) => {
                    let ttft = first.unwrap_or_else(|| t_req.elapsed().as_secs_f64());
                    let (_rec, receipt) = me.record(&request_id, &ticket, &wallet, &dir, &r, &gen, need, &body_sha, ttft);
                    let mut last = chunk(json!({}), json!(finish_reason(&gen, r.tools)));
                    last["usage"] = usage(&gen);
                    last["cadre"] = receipt;
                    let _ = tx.send(sse(last));
                }
                Err(e) => {
                    me.with_ledger(|l| l.tickets.get_mut(&ticket).map(|t| t.reserved = t.reserved.saturating_sub(need)));
                    me.in_flight.lock().unwrap().remove(&request_id);
                    me.remove_request_data(&dir);
                    let _ = tx.send(sse(json!({"error": {"message": format!("{e:#}"), "type": "server_error"}})));
                }
            }
            let _ = tx.send(b"data: [DONE]\n\n".to_vec());
        });
        Ok(HttpResponse {
            status: 200,
            body: Payload::Stream(rx),
            headers: vec![("content-type".into(), "text/event-stream".into()), ("cache-control".into(), "no-cache".into()), ("x-cadre-request".into(), request_id.clone()),
                ("x-cadre-receipt".into(), self.status_url(&request_id))],
        })
    }

    /// Book a served request on its ticket: release the reservation, add the receipt.
    #[allow(clippy::too_many_arguments)]
    fn record(&self, id: &str, ticket: &str, wallet: &str, dir: &Path, r: &InferReq, gen: &Value, reserved: u64, body_sha: &str, ttft: f64) -> (Served, Value) {
        let rows = gen["rows"].as_u64().unwrap_or(0) as u32;
        let completion = gen["completion_tokens"].as_u64().unwrap_or_else(|| gen["output_ids"].as_array().map(|a| a.len() as u64).unwrap_or(0)) as u32;
        let decode = gen["timing"]["decode_tokens_per_s"].as_f64();
        let out_sha = auth::sha256_hex(serde_json::to_string(&gen["output_ids"]).unwrap_or_default().as_bytes());
        let text_sha = auth::sha256_hex(gen["text"].as_str().unwrap_or("").as_bytes());
        let (rec, receipt) = self.with_ledger(|l| {
            let lpr = l.tickets.get(ticket).and_then(|t| t.lamports_per_row);
            let s = Served {
                request_id: id.into(), wallet: wallet.into(), ticket_id: ticket.into(), dir: dir.to_path_buf(), rows, prompt_tokens: r.ids.len() as u32,
                completion_tokens: completion, body_sha256: body_sha.into(), output_sha256: out_sha, text_sha256: text_sha, charged_lamports: lpr.map(|x| x * rows as u64),
                at_ms: now_ms(), round: None, row_offset: None, ttft_s: ttft, decode_tokens_per_s: decode,
                prompt_ids: r.ids.clone(),
                output_ids: gen["output_ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect()).unwrap_or_default(),
                max_new_tokens: r.max_new,
                stop: r.stop.clone(),
                glue: gen["glue"].as_str().unwrap_or("").to_string(),
            };
            if let Some(t) = l.tickets.get_mut(ticket) {
                t.reserved = t.reserved.saturating_sub(reserved);
                t.pending.push(id.into());
            }
            l.requests.insert(id.into(), s.clone());
            let rj = self.receipt_json(l, &s);
            (s, rj)
        });
        self.in_flight.lock().unwrap().remove(id);
        self.data_written_bytes.fetch_add(dir_bytes(dir), Ordering::SeqCst);
        {
            let mut q = self.recent.lock().unwrap();
            q.push_back((ttft, decode, completion));
            while q.len() > 100 {
                q.pop_front();
            }
        }
        (self.log)(json!({"event": "served", "request": id, "ticket": ticket, "rows": rows, "ttft_s": ttft, "decode_tokens_per_s": decode}));
        (rec, receipt)
    }

    // ------------------------------------------------------------ settlement

    /// Send a tx from the server key and wait for it to be included.
    fn send_wait(&self, body: Body, what: &str) -> Result<String> {
        Ok(self.send_many(vec![body], what)?.remove(0))
    }

    /// The server's own txs in the node's mempool by nonce (empty when the node has no
    /// getMempool; such a node also refuses nothing for a pooled nonce).
    pub fn own_pooled(&self) -> BTreeMap<u64, PooledTx> {
        let me = b58(&self.server());
        let mut out = BTreeMap::new();
        let Ok(mp) = self.node.call("getMempool", json!({})) else { return out };
        for h in mp["txs"].as_array().into_iter().flatten().filter_map(|h| h.as_str()) {
            let Ok(t) = self.node.call("getTx", json!({"tx_hash": h})) else { continue };
            if t["from"].as_str() != Some(me.as_str()) || t["status"] != json!("pending") {
                continue;
            }
            use base64::Engine;
            let Some(wire) = t["tx"].as_str().and_then(|x| base64::engine::general_purpose::STANDARD.decode(x).ok()) else { continue };
            if wire.len() < 54 + 64 {
                continue;
            }
            let nonce = u64::from_le_bytes(wire[37..45].try_into().unwrap());
            let fee = u64::from_le_bytes(wire[45..53].try_into().unwrap());
            out.insert(nonce, PooledTx { hash: h.to_string(), fee, kind: wire[53], body: wire[54..wire.len() - 64].to_vec() });
        }
        out
    }

    /// Sign `body` at `nonce` and send it. A pooled tx of ours at that nonce with the same body
    /// is reused; another one (stuck: refused at the tip, or stale from an earlier attempt) is
    /// replaced with the mempool's replacement fee.
    #[allow(clippy::too_many_arguments)]
    fn send_at(&self, chain_id: u32, min_fee: u64, nonce: u64, body: &Body, pooled: &mut BTreeMap<u64, PooledTx>, what: &str) -> Result<String> {
        let me = self.server();
        let enc = body.encode()?;
        let cap = min_fee.max(1).saturating_mul(self.cfg.max_fee_multiple.max(1));
        for attempt in 0..2 {
            let old = pooled.get(&nonce).cloned();
            if let Some(o) = &old {
                if o.kind == body.kind() && o.body == enc {
                    return Ok(o.hash.clone()); // the same tx, sent before (maybe at a higher fee)
                }
            }
            let fee = old.as_ref().map_or(min_fee, |o| replacement_fee(o.fee).max(min_fee));
            ensure!(fee <= cap, "{what}: replacing the server's pooled tx at nonce {nonce} needs fee {fee}, above the cap {cap} ({}x min_fee)", self.cfg.max_fee_multiple);
            let wire = tx::sign(&Tx { chain_id, from: me, nonce, fee, body: body.clone() }, &self.key)?;
            match self.node.send_tx(&wire) {
                Ok(h) => {
                    if let Some(o) = old.filter(|_| what != "cancel") {
                        self.tx_replaced.fetch_add(1, Ordering::SeqCst);
                        (self.log)(json!({"event": "tx_replaced", "what": what, "nonce": nonce, "old": o.hash, "old_fee": o.fee, "new": h, "fee": fee}));
                    }
                    pooled.insert(nonce, PooledTx { hash: h.clone(), fee, kind: body.kind(), body: enc.clone() });
                    return Ok(h);
                }
                // the same tx from an earlier attempt is still in the pool
                Err(e) if format!("{e:#}").contains("already") => return Ok(hex::encode(blake3::hash(&wire).as_bytes())),
                // the pool changed since it was read (or was never read): read it again, once
                Err(e) if attempt == 0 && is_nonce_pooled(&format!("{e:#}")) => {
                    let fresh = self.own_pooled();
                    match fresh.get(&nonce) {
                        Some(p) => {
                            pooled.insert(nonce, p.clone());
                        }
                        None => return Err(e).with_context(|| format!("send {what} at nonce {nonce}: the node says the nonce is pooled but getMempool does not list it")),
                    }
                }
                Err(e) => return Err(e).with_context(|| format!("send {what} at nonce {nonce}")),
            }
        }
        bail!("send {what} at nonce {nonce}: still refused after replacing the pooled tx")
    }

    /// Send txs from the server key at consecutive nonces from the account's confirmed nonce and
    /// wait until every one is included. Returns their hashes. Robust against the server's own
    /// leftovers in the mempool (SPEC 5.4 "Server nonce"): a pooled tx at one of these nonces
    /// is reused when it is the same tx and replaced otherwise; leftovers past the last nonce are
    /// cancelled (0 self-transfer) so they cannot collide with later txs; at most `send_window`
    /// are in flight; a tx at the next nonce that `stall_blocks` new blocks skip stops the send
    /// with an error (the chain refuses it) instead of holding the settle pipeline.
    fn send_many(&self, bodies: Vec<Body>, what: &str) -> Result<Vec<String>> {
        if bodies.is_empty() {
            return Ok(vec![]);
        }
        let _g = self.send_lock.lock().unwrap();
        let me = self.server();
        let info = self.node.call("getChainInfo", json!({}))?;
        let params = self.node.call("getParams", json!({}))?;
        let chain_id = info["chain_id"].as_u64().ok_or_else(|| anyhow!("no chain_id"))? as u32;
        let fee = pearl_node::num(&params["min_fee"]).unwrap_or(1000);
        let height = |v: &Value| v["height"].as_u64().unwrap_or(0);
        let base = self.node.get_account(&me)?.nonce;
        let mut pooled = self.own_pooled();
        let window = self.cfg.send_window.clamp(1, 64) as u64;
        let mut hs: Vec<String> = Vec::with_capacity(bodies.len());
        let mut done = 0usize; // included, in order
        let mut confirmed = base;
        let mut progress = (Instant::now(), height(&info), confirmed);
        let mut foreign = 0u32;
        loop {
            // fill the window
            while hs.len() < bodies.len() && base + (hs.len() as u64) < confirmed + window {
                let n = base + hs.len() as u64;
                let h = self.send_at(chain_id, fee, n, &bodies[hs.len()], &mut pooled, what).with_context(|| format!("{what} {}", hs.len()))?;
                hs.push(h);
            }
            if hs.len() == bodies.len() {
                // leftovers of ours past the last nonce: cancel them
                let last = base + bodies.len() as u64 - 1;
                let orphans: Vec<u64> = pooled.range(last + 1..).map(|(n, _)| *n).filter(|n| *n < confirmed + window).collect();
                for n in orphans {
                    let cancel = Body::Transfer { to: me, amount: 0 };
                    if pooled.get(&n).is_some_and(|p| p.kind == cancel.kind() && p.body == cancel.encode().unwrap_or_default()) {
                        continue;
                    }
                    match self.send_at(chain_id, fee, n, &cancel, &mut pooled, "cancel") {
                        Ok(h) => {
                            self.tx_cancelled.fetch_add(1, Ordering::SeqCst);
                            (self.log)(json!({"event": "tx_cancelled", "nonce": n, "tx": h}));
                        }
                        Err(e) => (self.log)(json!({"event": "tx_cancel_error", "nonce": n, "error": format!("{e:#}")})),
                    }
                }
            }
            // collect inclusions in nonce order
            while done < hs.len() {
                let t = self.node.call("getTx", json!({"tx_hash": hs[done]}))?;
                if t["status"] == json!("included") {
                    done += 1;
                    foreign = 0;
                    continue;
                }
                if confirmed > base + done as u64 {
                    // the chain used this nonce for another tx of ours (an earlier attempt's)
                    foreign += 1;
                    ensure!(foreign < 3, "{what} tx {} at nonce {}: the chain included another tx of this key at that nonce", hs[done], base + done as u64);
                }
                break;
            }
            if done == hs.len() && hs.len() == bodies.len() {
                return Ok(hs);
            }
            std::thread::sleep(self.cfg.poll);
            confirmed = self.node.get_account(&me)?.nonce;
            let tip = self.node.call("getChainInfo", json!({})).map(|v| height(&v)).unwrap_or(progress.1);
            if confirmed > progress.2 {
                progress = (Instant::now(), tip, confirmed);
            } else {
                let skipped = tip.saturating_sub(progress.1);
                let stuck = hs.get(done).cloned().unwrap_or_default();
                if self.cfg.stall_blocks > 0 && skipped >= self.cfg.stall_blocks {
                    self.tx_stalls.fetch_add(1, Ordering::SeqCst);
                    bail!("{what} tx {stuck} at nonce {}: {skipped} blocks skipped it (the chain refuses it at the tip); the next send replaces it", base + done as u64);
                }
                if progress.0.elapsed() >= Duration::from_secs(600) {
                    self.tx_stalls.fetch_add(1, Ordering::SeqCst);
                    bail!("{what} tx {stuck} not included after 600 s");
                }
            }
        }
    }

    /// Background loop: one settlement pass every `round_every`.
    /// The settlement loops: round posting here, ticket closes on a thread of their own.
    pub fn run_rounds(self: Arc<Self>) {
        let me = Arc::clone(&self);
        std::thread::spawn(move || loop {
            me.close_pass();
            std::thread::sleep(me.loop_sleep());
        });
        loop {
            self.post_pass();
            std::thread::sleep(self.loop_sleep());
        }
    }

    fn loop_sleep(&self) -> Duration {
        self.cfg.round_every.min(Duration::from_secs(5)).max(Duration::from_millis(200))
    }

    /// One posting pass then one closing pass (the HTTP close handler and the tests; the
    /// service runs the two passes on separate threads, [`Gateway::run_rounds`]).
    pub fn settle_pass(&self) {
        self.post_pass();
        self.close_pass();
    }

    /// For every ticket: post a round of its served rows when due (SPEC 5.4 "Settle order");
    /// a ticket whose served rows can no longer be posted drops them. Never waits for a close.
    pub fn post_pass(&self) {
        let Ok(_g) = self.settle_lock.try_lock() else { return };
        for id in self.settle_order() {
            if self.closing_now.lock().unwrap().as_ref().is_some_and(|(c, _)| *c == id) {
                continue; // being closed on the close thread
            }
            if let Some((at, _, _)) = self.settle_retry.lock().unwrap().get(&id) {
                if Instant::now() < *at {
                    continue; // failed recently: the other tickets go first
                }
            }
            let r = self.settle_ticket_post(&id);
            self.note_result(&self.settle_retry, &id, &r);
            if let Err(e) = r {
                let msg = format!("{e:#}");
                if is_blob_store_full(&msg) {
                    // the node refused the round's A blobs: refuse new paid work at once instead of
                    // waiting for the next getBlobStats read (backpressure, SPEC 5.4)
                    *self.node_blob.lock().unwrap() = Some((Some(0), Instant::now()));
                }
                (self.log)(json!({"event": "settle_error", "ticket": id, "error": msg}));
            }
        }
        self.sweep_request_data();
    }

    /// For every ticket due to close (used up, near its deadline, closing, a one-request ticket,
    /// or delivered but not completed): deliver and complete it. The evaluator's replay can take
    /// minutes; round posting goes on meanwhile.
    pub fn close_pass(&self) {
        let Ok(_g) = self.close_lock.try_lock() else { return };
        let Ok(tip) = self.node.call("getChainInfo", json!({})).map(|v| v["height"].as_u64().unwrap_or(0)) else { return };
        let ids: Vec<String> = {
            let l = self.ledger.lock().unwrap();
            l.tickets.iter().filter(|(_, t)| self.close_due(&l, t, tip)).map(|(k, _)| k.clone()).collect()
        };
        for id in ids {
            if let Some((at, _, _)) = self.close_retry.lock().unwrap().get(&id) {
                if Instant::now() < *at {
                    continue;
                }
            }
            *self.closing_now.lock().unwrap() = Some((id.clone(), Instant::now()));
            let r = self.settle_ticket_close(&id);
            *self.closing_now.lock().unwrap() = None;
            self.note_result(&self.close_retry, &id, &r);
            if let Err(e) = r {
                (self.log)(json!({"event": "close_error", "ticket": id, "error": format!("{e:#}")}));
            }
        }
    }

    fn note_result(&self, map: &Mutex<BTreeMap<String, (Instant, u32, String)>>, id: &str, r: &Result<()>) {
        let mut sr = map.lock().unwrap();
        match r {
            Ok(()) => {
                sr.remove(id);
            }
            Err(e) => {
                let fails = sr.get(id).map_or(0, |x| x.1) + 1;
                let wait = self.cfg.settle_backoff.saturating_mul(1 << (fails - 1).min(4));
                sr.insert(id.to_string(), (Instant::now() + wait, fails, format!("{e:#}")));
            }
        }
    }

    /// Whether a ticket is due to close at chain height `tip`: delivered (complete it), or
    /// nothing waits to be posted, it has a round, and it is closing, single, used up, or
    /// within `close_margin_blocks` of its deadline.
    fn close_due(&self, l: &Ledger, t: &TicketState, tip: u64) -> bool {
        if t.state == "delivered" {
            return true;
        }
        if !(t.state == "open" || t.state == "closing") || !t.pending.is_empty() || t.reserved != 0 || t.rounds.is_empty() || t.posting.is_some() {
            return false;
        }
        let free = l.free_in_budget(t, self.model.period() as u64, self.round_rows_cap() as u64);
        let near = t.deadline_height.is_some_and(|d| d.saturating_sub(tip) <= self.cfg.close_margin_blocks);
        t.single || t.state == "closing" || free < self.cfg.min_free_rows || near
    }

    /// The order a settle pass visits tickets (SPEC 5.4 "Settle order"): a round the chain has
    /// started (recorded as posting) first, so it completes; then, under data pressure, the most
    /// waiting rows first (a round costs one tx per matrix whatever its rows, so the fullest
    /// rounds free the most scratch per tx); otherwise the oldest waiting request first.
    pub fn settle_order(&self) -> Vec<String> {
        let pressure = self.data_pressure();
        let l = self.ledger.lock().unwrap();
        let mut v: Vec<(bool, u64, u64, String)> = l
            .tickets
            .iter()
            .filter(|(_, t)| t.state == "open" || t.state == "closing" || t.state == "delivered")
            .map(|(k, t)| {
                let rows: u64 = t.pending.iter().filter_map(|r| l.requests.get(r)).map(|r| r.rows as u64).sum();
                let oldest = t.pending.iter().filter_map(|r| l.requests.get(r)).map(|r| r.at_ms).min().unwrap_or(u64::MAX);
                (t.posting.is_none(), if pressure { u64::MAX - rows } else { oldest }, if pressure { oldest } else { u64::MAX - rows }, k.clone())
            })
            .collect();
        v.sort();
        v.into_iter().map(|x| x.3).collect()
    }

    /// Tickets now waiting after a failed settlement: (ticket, consecutive failures, seconds
    /// until the next try, last error).
    /// The ticket the close thread is completing now (the evaluator's replay can take minutes).
    pub fn closing_json(&self) -> Value {
        match &*self.closing_now.lock().unwrap() {
            Some((id, at)) => json!({"ticket": id, "for_s": at.elapsed().as_secs()}),
            None => Value::Null,
        }
    }

    pub fn close_backlog_json(&self) -> Value {
        let now = Instant::now();
        let sr = self.close_retry.lock().unwrap();
        Value::Array(sr.iter().map(|(k, (at, n, e))| json!({"ticket": k, "failures": n, "retry_in_s": at.saturating_duration_since(now).as_secs(), "error": e})).collect())
    }

    pub fn settle_backlog_json(&self) -> Value {
        let now = Instant::now();
        let sr = self.settle_retry.lock().unwrap();
        Value::Array(sr.iter().map(|(k, (at, n, e))| json!({"ticket": k, "failures": n, "retry_in_s": at.saturating_duration_since(now).as_secs(), "error": e})).collect())
    }

    // ------------------------------------------------------------ request data retention

    /// Scratch bytes one row of this model needs: A (k bytes) and C (4 n bytes) per matrix.
    pub fn bytes_per_row(&self) -> u64 {
        self.model.matrices.iter().map(|(k, n)| *k as u64 + 4 * *n as u64).sum()
    }

    /// Scratch bytes waiting for a round (served, round not posted, ticket still open) plus
    /// the estimate of every request being generated.
    pub fn request_data_bytes(&self) -> u64 {
        let bpr = self.bytes_per_row();
        let waiting: u64 = {
            let l = self.ledger.lock().unwrap();
            l.tickets.values().filter(|t| t.state == "open" || t.state == "closing")
                .flat_map(|t| t.pending.iter()).filter_map(|r| l.requests.get(r)).map(|r| r.rows as u64 * bpr).sum()
        };
        waiting + self.in_flight.lock().unwrap().values().sum::<u64>()
    }

    /// Why a new request of `need` scratch bytes must wait (None: it may run).
    pub fn storage_refusal(&self, need: u64) -> Option<String> {
        let held = self.request_data_bytes();
        if self.cfg.max_request_data_bytes > 0 && held + need > self.cfg.max_request_data_bytes {
            return Some(format!("request data waiting for settlement is {} MB (+{} MB for this request) above the gateway's cap of {} MB; rounds are being posted, retry shortly",
                held >> 20, need >> 20, self.cfg.max_request_data_bytes >> 20));
        }
        if let Some(free) = disk_free_bytes(&self.cfg.work_dir) {
            let unwritten: u64 = self.in_flight.lock().unwrap().values().sum();
            if free.saturating_sub(unwritten + need) < self.cfg.min_free_disk_bytes {
                return Some(format!("the gateway's disk has {} MB free, under its reserve of {} MB once this request ({} MB) and those running are written; retry later",
                    free >> 20, self.cfg.min_free_disk_bytes >> 20, need >> 20));
            }
        }
        None
    }

    /// A bytes per row (one k-byte row per registered matrix): what a posted round adds to the
    /// node's blob store at most (matrices sharing an input share one blob, so less in practice).
    pub fn a_bytes_per_row(&self) -> u64 {
        self.model.matrices.iter().map(|(k, _)| *k as u64).sum()
    }

    /// Bytes the node's blob store accepts from RPC puts before it refuses them (getBlobStats
    /// `rpc_headroom_bytes`; an older node without it: `max_total_bytes - total_bytes`). Read at
    /// most every [`NODE_BLOB_TTL`]; None when unknown.
    pub fn node_blob_headroom(&self) -> Option<u64> {
        if let Some((h, at)) = *self.node_blob.lock().unwrap() {
            if at.elapsed() < NODE_BLOB_TTL {
                return h;
            }
        }
        let h = self.node.call("getBlobStats", json!({})).ok().and_then(|v| {
            v["rpc_headroom_bytes"].as_u64().or_else(|| Some(v["max_total_bytes"].as_u64()?.saturating_sub(v["total_bytes"].as_u64()?)))
        });
        *self.node_blob.lock().unwrap() = Some((h, Instant::now()));
        h
    }

    /// Node blob backpressure (SPEC 5.4 "Request data retention"): a paid request whose A rows,
    /// with every row already waiting for a round, would not fit in the node's blob headroom
    /// waits (503 insufficient_storage) instead of piling up scratch that cannot be posted.
    pub fn node_storage_refusal(&self, need_rows: u64) -> Option<String> {
        let head = self.node_blob_headroom()?;
        let bpr = self.bytes_per_row().max(1);
        let waiting_rows = self.request_data_bytes() / bpr;
        let need = (waiting_rows + need_rows) * self.a_bytes_per_row();
        if need > head {
            return Some(format!(
                "the chain node's blob store is nearly full: {} MB of room for activation blobs, {} MB needed by the rounds waiting and this request; retry after the node prunes",
                head >> 20, need >> 20
            ));
        }
        None
    }

    /// Over half the data cap: post rounds early (oldest pending first) instead of waiting for
    /// a ticket's round period (3 h for large balances).
    fn data_pressure(&self) -> bool {
        let at = self.pressure_bytes();
        at > 0 && self.request_data_bytes() >= at
    }

    /// Waiting bytes at which rounds post early: half the cap, or `data_pressure_bytes` if lower
    /// (0 when neither is set).
    pub fn pressure_bytes(&self) -> u64 {
        let half = self.cfg.max_request_data_bytes / 2;
        match (half, self.cfg.data_pressure_bytes) {
            (0, p) => p,
            (h, 0) => h,
            (h, p) => h.min(p),
        }
    }

    fn remove_request_data(&self, dir: &Path) {
        if self.cfg.keep_request_data {
            return;
        }
        let bytes = dir_bytes(dir);
        match std::fs::remove_dir_all(dir) {
            Ok(()) => {
                self.data_deleted.fetch_add(1, Ordering::SeqCst);
                self.data_freed_bytes.fetch_add(bytes, Ordering::SeqCst);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => (self.log)(json!({"event": "request_data_delete_error", "dir": dir.display().to_string(), "error": e.to_string()})),
        }
    }

    /// Delete every request dir no round will read again: posted, its ticket no longer open
    /// (completed, failed), or unknown to the ledger and not being generated (a crash between
    /// generation and booking; 10 minutes of grace).
    pub fn sweep_request_data(&self) -> usize {
        let root = self.cfg.work_dir.join("requests");
        let Ok(rd) = std::fs::read_dir(&root) else { return 0 };
        let mut gone = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let keep = {
                let l = self.ledger.lock().unwrap();
                match l.requests.get(&name) {
                    Some(r) => r.round.is_none() && l.tickets.get(&r.ticket_id).is_some_and(|t| t.state == "open" || t.state == "closing"),
                    None => {
                        self.in_flight.lock().unwrap().contains_key(&name)
                            || e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map_or(true, |age| age < Duration::from_secs(600))
                    }
                }
            };
            if !keep && !self.cfg.keep_request_data {
                self.remove_request_data(&e.path());
                gone += 1;
            }
        }
        gone
    }

    /// Post and close one ticket as one settlement step (both phases; tests and tools).
    pub fn settle_ticket(&self, id: &str) -> Result<()> {
        self.settle_ticket_post(id)?;
        let tip = self.node.call("getChainInfo", json!({}))?["height"].as_u64().unwrap_or(0);
        let due = {
            let l = self.ledger.lock().unwrap();
            l.tickets.get(id).is_some_and(|t| self.close_due(&l, t, tip))
        };
        if due {
            self.settle_ticket_close(id)?;
        }
        Ok(())
    }

    /// The posting half of settlement: read the ticket on the chain, record its deadline, drop
    /// served rows that can no longer be posted, and post a round when one is due.
    pub fn settle_ticket_post(&self, id: &str) -> Result<()> {
        let tid: [u8; 32] = hex::decode(id)?.try_into().map_err(|_| anyhow!("ticket id"))?;
        let t = self.ledger.lock().unwrap().tickets.get(id).cloned().ok_or_else(|| anyhow!("unknown ticket"))?;
        if t.state == "delivered" {
            return Ok(()); // the close thread completes it
        }
        let chain = self.node.call("getTicket", json!({"ticket_id": id}))?;
        if chain.is_null() {
            ensure!(now_ms().saturating_sub(t.created_ms) < self.cfg.ticket_wait.as_millis() as u64, "ticket not on the chain after {:?}", self.cfg.ticket_wait);
            return Ok(()); // the relayer has not unlocked the order yet
        }
        let st = &chain["standing"];
        let deadline = chain["deadline_height"].as_u64().unwrap_or(u64::MAX);
        let tip = self.node.call("getChainInfo", json!({}))?["height"].as_u64().unwrap_or(0);
        // the round budget of a ticket this ledger did not size (older ledger, chain-native ticket)
        let per = self.round_blocks().max(1) as u64;
        let chain_rounds_cap = chain["max_blocks"].as_u64().map(|b| (b / per) as u32);
        self.with_ledger(|l| {
            l.tickets.get_mut(id).map(|x| {
                x.deadline_height = Some(deadline);
                if x.max_rounds.is_none() {
                    x.max_rounds = chain_rounds_cap;
                }
            })
        });
        let t = self.ledger.lock().unwrap().tickets.get(id).cloned().ok_or_else(|| anyhow!("unknown ticket"))?;
        if !t.pending.is_empty() {
            // the chain takes segments only while the ticket is Open and before its deadline
            let open = chain.get("status").map_or(true, |s| s == "open");
            // a PostSegments lands at tip + 1 at the earliest, and the chain refuses it above the deadline
            if !open || tip >= deadline {
                let why = if !open { format!("the chain has closed the ticket (status {})", chain["status"]) } else { format!("the ticket's deadline {deadline} passed (tip {tip})") };
                self.drop_pending(id, &why);
                return Ok(());
            }
        }
        let p = self.model.period() as u64;
        let cap = self.round_rows_cap() as u64;
        let pending_rows: u64 = {
            let l = self.ledger.lock().unwrap();
            t.pending.iter().filter_map(|r| l.requests.get(r)).map(|r| r.rows as u64).sum()
        };
        let chain_rounds = st["rounds"].as_u64().unwrap_or(0) as u16;
        let resume = (chain_rounds as usize) > t.rounds.len();
        let due = !t.pending.is_empty()
            && (resume
                || t.posting.is_some()
                || t.single
                || t.state == "closing"
                || self.oldest_pending_age(&t) >= self.round_period(t.rows, t.round_secs)
                || pending_rows >= self.round_rows_cap() as u64
                || self.data_pressure()
                || self.ledger.lock().unwrap().free_in_budget(&t, p, cap) < self.cfg.min_free_rows);
        if due {
            // a round the chain has started but this ledger has not recorded (an earlier attempt
            // stopped half way) is resumed: same round, only the matrices still missing
            let round = if resume { chain_rounds - 1 } else { chain_rounds };
            let chain_m = resume.then(|| st["round_rows"].as_u64().unwrap_or(0) as u32);
            self.post_round(&tid, id, &t, round, chain_m)?;
        }
        Ok(())
    }

    /// The closing half: deliver and complete a ticket the ledger says is due
    /// ([`Gateway::close_due`]), re-checked against the chain's deadline.
    pub fn settle_ticket_close(&self, id: &str) -> Result<()> {
        let tid: [u8; 32] = hex::decode(id)?.try_into().map_err(|_| anyhow!("ticket id"))?;
        let chain = self.node.call("getTicket", json!({"ticket_id": id}))?;
        ensure!(!chain.is_null(), "ticket {id} is not on the chain");
        // check and mark in one ledger step: an "open" ticket becomes "closing", so no request is
        // reserved on it while its result is built and delivered (such a request would be served
        // but never posted: the result names only the rounds already posted)
        let t = self.with_ledger(|l| {
            let t = l.tickets.get_mut(id).ok_or_else(|| anyhow!("unknown ticket"))?;
            if t.state != "delivered" && (!t.pending.is_empty() || t.reserved != 0 || t.posting.is_some()) {
                return Ok(None); // a request was served since the close was chosen: post its round first
            }
            if t.state == "open" {
                t.state = "closing".into();
            }
            Ok::<_, anyhow::Error>(Some(t.clone()))
        })?;
        let Some(t) = t else { return Ok(()) };
        self.close_ticket(&tid, id, &t, &chain)
    }

    /// Served requests of a ticket whose round can no longer be posted: dropped from the
    /// ticket (they stay in the ledger as receipts without a round) and their data deleted.
    fn drop_pending(&self, id: &str, why: &str) {
        let (dirs, rows) = self.with_ledger(|l| {
            let Some(t) = l.tickets.get_mut(id) else { return (vec![], 0u64) };
            let ids: Vec<String> = std::mem::take(&mut t.pending);
            t.posting = None;
            if t.rounds.is_empty() {
                t.state = "failed".into();
                t.error = Some(format!("served rows never posted: {why}"));
            }
            let rows = ids.iter().filter_map(|r| l.requests.get(r)).map(|r| r.rows as u64).sum();
            (ids.iter().filter_map(|r| l.requests.get(r)).map(|r| (r.request_id.clone(), r.dir.clone())).collect::<Vec<_>>(), rows)
        });
        self.pending_dropped.fetch_add(dirs.len(), Ordering::SeqCst);
        (self.log)(json!({"event": "pending_dropped", "ticket": id, "requests": dirs.iter().map(|d| d.0.clone()).collect::<Vec<_>>(), "rows": rows, "why": why}));
        for (_, d) in &dirs {
            self.remove_request_data(d);
        }
    }

    fn oldest_pending_age(&self, t: &TicketState) -> Duration {
        let l = self.ledger.lock().unwrap();
        let oldest = t.pending.iter().filter_map(|r| l.requests.get(r)).map(|r| r.at_ms).min().unwrap_or(now_ms());
        Duration::from_millis(now_ms().saturating_sub(oldest))
    }

    /// The requests of round `round`: the recorded posting set, or for a round the chain has
    /// started (`chain_m` = its m) without one, the oldest-first prefix of the pending requests
    /// whose A matches a segment already on the chain; otherwise the oldest pending that fit.
    fn round_requests(&self, tid: &[u8; 32], t: &TicketState, round: u16, chain_m: Option<u32>) -> Result<Vec<Served>> {
        let period = self.model.period() as u64;
        let (all, recorded): (Vec<Served>, Option<Vec<Served>>) = {
            let l = self.ledger.lock().unwrap();
            let all = t.pending.iter().filter_map(|r| l.requests.get(r).cloned()).collect();
            let rec = t.posting.as_ref().filter(|p| p.round == round).map(|p| p.requests.iter().filter_map(|r| l.requests.get(r).cloned()).collect::<Vec<_>>());
            (all, rec)
        };
        if let Some(rec) = recorded.filter(|r| !r.is_empty()) {
            let m = round_up(rec.iter().map(|r| r.rows as u64).sum(), period) as u32;
            ensure!(chain_m.map_or(true, |cm| cm == m), "round {round}: the recorded requests make m {m}, the chain's round has m {}", chain_m.unwrap_or(0));
            return Ok(rec);
        }
        let Some(cm) = chain_m else {
            return Ok(round_take(all, self.round_rows_cap(), self.model.period()));
        };
        // a round started on the chain before this ledger recorded its requests: it was the
        // oldest pending that fit then, so a prefix of today's pending list; find the one whose A
        // is the A of a matrix already posted
        let mut posted = None;
        for i in 0..self.model.matrices.len() {
            if let Some(j) = self.node.get_job(&standing::segment_id(tid, i as u16, round))? {
                posted = Some((i, j));
                break;
            }
        }
        let (i, job) = posted.ok_or_else(|| anyhow!("round {round} is on the chain (m {cm}) but none of its segments is"))?;
        let mut sum = 0u64;
        for n in 1..=all.len() {
            sum += all[n - 1].rows as u64;
            let m = round_up(sum, period);
            if m > cm as u64 {
                break;
            }
            if m < cm as u64 {
                continue;
            }
            let dirs: Vec<(PathBuf, u32)> = all[..n].iter().map(|r| (r.dir.clone(), r.rows)).collect();
            let seg = prepare_matrix(&self.model, &dirs, cm, i, self.cfg.blocks_per_segment)?;
            if seg.spec.a_uri == job.data_uri && seg.spec.hash_a == job.hash_a {
                (self.log)(json!({"event": "round_recovered", "ticket": hex::encode(tid), "round": round, "m": cm, "requests": n, "pending": all.len()}));
                return Ok(all[..n].to_vec());
            }
        }
        bail!("round {round} is on the chain with m {cm}, and no oldest-first prefix of the {} pending requests rebuilds the A of its matrix {i}; it needs a manual repair", all.len())
    }

    fn post_round(&self, tid: &[u8; 32], id: &str, t: &TicketState, round: u16, chain_m: Option<u32>) -> Result<()> {
        let reqs = self.round_requests(tid, t, round, chain_m)?;
        ensure!(!reqs.is_empty(), "a pending request is larger than one round ({} rows)", self.round_rows_cap());
        let rows: u32 = reqs.iter().map(|r| r.rows).sum();
        let m = round_up(rows as u64, self.model.period() as u64) as u32;
        let ids: Vec<String> = reqs.iter().map(|r| r.request_id.clone()).collect();
        // record the round before any of it reaches the chain, so a resume posts the same rows
        self.with_ledger(|l| {
            if let Some(tt) = l.tickets.get_mut(id) {
                tt.posting = Some(PostingRound { round, m, requests: ids.clone() });
            }
        });
        let dirs: Vec<(PathBuf, u32)> = reqs.iter().map(|r| (r.dir.clone(), r.rows)).collect();
        let segs = prepare_round(&self.model, &dirs, m, self.cfg.blocks_per_segment)?;
        for s in &segs {
            let uri = self.node.put_blob(&s.a_blob)?;
            ensure!(uri == s.spec.a_uri, "node stored A under {uri}, expected {}", s.spec.a_uri);
        }
        // one segment per tx: a Solana light client registers each segment job from its posting
        // tx, and a Solana tx (1232 bytes) holds one such chain tx with its tx_root path
        let head = self.model.head();
        let mut bodies = Vec::with_capacity(segs.len());
        for s in &segs {
            if self.node.get_job(&standing::segment_id(tid, s.spec.matrix, round))?.is_some() {
                continue; // posted by an earlier attempt
            }
            bodies.push(Body::PostSegments { ticket_id: *tid, round, head, segments: vec![s.spec.clone()] });
        }
        let txs = self.send_many(bodies, "PostSegments")?;
        self.with_ledger(|l| {
            let mut off = 0u32;
            for r in &reqs {
                if let Some(x) = l.requests.get_mut(&r.request_id) {
                    x.round = Some(round);
                    x.row_offset = Some(off);
                }
                off += r.rows;
            }
            if let Some(tt) = l.tickets.get_mut(id) {
                tt.pending.retain(|x| !ids.contains(x));
                tt.rounds.push(RoundRec { round, m, requests: ids.clone(), txs: txs.clone() });
                tt.posting = None;
            }
        });
        (self.log)(json!({"event": "round_posted", "ticket": id, "round": round, "m": m, "requests": ids.len(), "segments": segs.len(), "txs": txs}));
        // the round is on the chain and in the ledger: A is on the node, C is committed
        for r in &reqs {
            self.remove_request_data(&r.dir);
        }
        Ok(())
    }

    fn close_ticket(&self, tid: &[u8; 32], id: &str, t: &TicketState, chain: &Value) -> Result<()> {
        let (rounds, single_req) = {
            let l = self.ledger.lock().unwrap();
            let rounds: Vec<Value> = t
                .rounds
                .iter()
                .map(|r| {
                    let reqs: Vec<Value> = r.requests.iter().filter_map(|q| l.requests.get(q)).map(|q| {
                        let mut v = json!({"request_id": q.request_id, "body_sha256": q.body_sha256,
                            "row_offset": q.row_offset, "rows": q.rows, "prompt_tokens": q.prompt_tokens, "completion_tokens": q.completion_tokens, "output_sha256": q.output_sha256, "text_sha256": q.text_sha256});
                        if !q.glue.is_empty() {
                            // the replay bundle (SPEC 4.7): an evaluator rebuilds every A from these
                            v["glue"] = json!(q.glue);
                            v["prompt_ids"] = json!(q.prompt_ids);
                            v["output_ids"] = json!(q.output_ids);
                            v["max_new_tokens"] = json!(q.max_new_tokens);
                            v["stop"] = json!(q.stop);
                        }
                        v
                    }).collect();
                    json!({"round": r.round, "m": r.m, "requests": reqs})
                })
                .collect();
            (rounds, t.rounds.first().and_then(|r| r.requests.first()).and_then(|q| l.requests.get(q)).cloned())
        };
        let segments: Vec<Value> = t
            .rounds
            .iter()
            .flat_map(|r| (0..self.model.matrices.len()).map(move |i| json!({"round": r.round, "matrix": i, "segment_id": hex::encode(standing::segment_id(tid, i as u16, r.round)), "m": r.m})))
            .collect();
        let mut resp_v = json!({
            "format": "cadre-response-v1", "model_id": hex::encode(self.model.model_id), "ticket_id": id,
            "request_hash": chain["standing"]["request_hash"], "rows": t.posted_rows(), "rounds": rounds, "segments": segments,
        });
        // replayable (SPEC 4.7) when every request of every round ran the deterministic glue
        let glue = {
            let l = self.ledger.lock().unwrap();
            let all: Vec<String> = t.rounds.iter().flat_map(|r| r.requests.iter()).map(|q| l.requests.get(q).map(|x| x.glue.clone()).unwrap_or_default()).collect();
            if !all.is_empty() && all.iter().all(|g| g == "cadre-glue-v1") { Some("cadre-glue-v1") } else { None }
        };
        if let Some(g) = glue {
            resp_v["glue"] = json!(g);
        }
        if t.single {
            if let Some(q) = single_req {
                resp_v["prompt_tokens"] = json!(q.prompt_tokens);
                resp_v["output_sha256"] = json!(q.output_sha256);
            }
        }
        let rb = serde_json::to_vec(&resp_v)?;
        if chain["status"] == json!("open") {
            let ruri = self.node.put_blob(&rb)?;
            self.send_wait(Body::SubmitResult { job_id: *tid, result_hash: pearl_job::data::blob_id(&rb), uri: ruri }, "SubmitResult")?;
            self.with_ledger(|l| l.tickets.get_mut(id).map(|x| x.state = "delivered".into()));
        }
        let (pk, sig) = self.evaluator.attest(tid, &self.server())?;
        let h = self.send_wait(Body::CompleteJob { job_id: *tid, sigs: vec![(pk, sig)] }, "CompleteJob")?;
        self.with_ledger(|l| l.tickets.get_mut(id).map(|x| x.state = "completed".into()));
        (self.log)(json!({"event": "completed", "ticket": id, "complete_tx": h, "rounds": t.rounds.len(), "rows_used": t.posted_rows(), "rows": t.rows}));
        Ok(())
    }
}

fn auth_wallet_signed(a: &Auth) -> Result<[u8; 32]> {
    match a {
        Auth::Wallet(w) => Ok(*w),
        _ => bail!("this call needs the wallet signature headers (not an API key)"),
    }
}

fn usage(gen: &Value) -> Value {
    let p = gen["prompt_tokens"].as_u64().unwrap_or(0);
    let c = gen["completion_tokens"].as_u64().unwrap_or(0);
    json!({"prompt_tokens": p, "completion_tokens": c, "total_tokens": p + c})
}

/// A tool call the model emitted as a bare JSON object `{"name", "parameters"|"arguments"}`.
pub fn parse_tool_call(text: &str) -> Option<Value> {
    let t = text.trim();
    let v: Value = serde_json::from_str(t).ok()?;
    let name = v["name"].as_str()?;
    let args = v.get("parameters").or(v.get("arguments")).cloned().unwrap_or(json!({}));
    Some(json!({"id": format!("call_{}", &auth::sha256_hex(t.as_bytes())[..16]), "type": "function",
        "function": {"name": name, "arguments": if args.is_string() { args } else { json!(args.to_string()) }}}))
}

fn finish_reason(gen: &Value, tools: bool) -> &'static str {
    if tools && parse_tool_call(gen["text"].as_str().unwrap_or("")).is_some() {
        return "tool_calls";
    }
    if gen["finish_reason"] == json!("stop") { "stop" } else { "length" }
}

pub fn completion_json(chat: bool, tools: bool, id: &str, created: u64, model: &str, gen: &Value, receipt: &Value) -> Value {
    let text = gen["text"].as_str().unwrap_or("").to_string();
    let fr = finish_reason(gen, tools);
    if chat {
        let mut msg = json!({"role": "assistant", "content": text});
        if fr == "tool_calls" {
            msg = json!({"role": "assistant", "content": Value::Null, "tool_calls": [parse_tool_call(&text).unwrap()]});
        }
        json!({"id": format!("chatcmpl-{id}"), "object": "chat.completion", "created": created, "model": model,
            "choices": [{"index": 0, "message": msg, "finish_reason": fr}], "usage": usage(gen), "cadre": receipt})
    } else {
        json!({"id": format!("cmpl-{id}"), "object": "text_completion", "created": created, "model": model,
            "choices": [{"index": 0, "text": text, "finish_reason": fr}], "usage": usage(gen), "cadre": receipt})
    }
}

fn receipt_headers(r: &Served, receipt: &Value) -> Vec<(String, String)> {
    let s = |k: &str| receipt[k].as_str().unwrap_or("").to_string();
    vec![
        ("x-cadre-request".into(), r.request_id.clone()),
        ("x-cadre-ticket".into(), r.ticket_id.clone()),
        ("x-cadre-order".into(), s("order")),
        ("x-cadre-rows".into(), r.rows.to_string()),
        ("x-cadre-charged-lamports".into(), r.charged_lamports.map(|x| x.to_string()).unwrap_or_default()),
        ("x-cadre-receipt".into(), s("status_url")),
        ("x-cadre-explorer".into(), s("explorer_url")),
    ]
}
