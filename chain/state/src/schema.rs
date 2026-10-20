//! State schema: keys and record encodings.
//!
//! Every key is `blake3("pearl-sol:state:v1" || tag u8 || id)`; every value starts with
//! its tag byte so a full scan (tests, invariant checks) can decode any record.

use agent_types::codec::{R, W};
use agent_types::{blake3_hash, DecodeError, Hash, Pubkey};

pub type Key = [u8; 32];

pub mod tag {
    pub const ACCOUNT: u8 = 1;
    pub const JOB: u8 = 2;
    pub const AGENT: u8 = 3;
    pub const CONFIG: u8 = 4;
    pub const DEPOSIT: u8 = 5;
    pub const IMMATURE: u8 = 6;
    pub const COUNTERS: u8 = 7;
    pub const JOB_INDEX: u8 = 8;
    pub const WITHDRAWAL: u8 = 9;
    pub const ROUTE: u8 = 10;
    pub const MODEL: u8 = 11;
    pub const MODEL_INDEX: u8 = 12;
    pub const MODEL_COUNT: u8 = 13;
    /// RejectResult records (state/src/reject.rs).
    pub const REJECT: u8 = 14;
    pub const REJECT_COUNT: u8 = 15;
    pub const REJECT_STATS: u8 = 16;
    /// Social records (state/src/social.rs, SPEC 4.6).
    pub const PROFILE: u8 = 17;
    pub const PROFILE_INDEX: u8 = 18;
    pub const PROFILE_COUNT: u8 = 19;
    pub const OPERATOR: u8 = 20;
    pub const POST: u8 = 21;
    pub const SOCIAL: u8 = 22;
    pub const REACT: u8 = 23;
    pub const FOLLOW: u8 = 24;
    pub const HANDLE: u8 = 25;
}

pub fn key(tag: u8, id: &[u8]) -> Key {
    blake3_hash(&[b"pearl-sol:state:v1", &[tag], id])
}

pub fn account_key(a: &Pubkey) -> Key {
    key(tag::ACCOUNT, a)
}
pub fn job_key(job_id: &Hash) -> Key {
    key(tag::JOB, job_id)
}
pub fn agent_key(a: &Pubkey) -> Key {
    key(tag::AGENT, a)
}
pub fn config_key() -> Key {
    key(tag::CONFIG, &[])
}
pub fn deposit_key(solana_nonce: u64) -> Key {
    key(tag::DEPOSIT, &solana_nonce.to_le_bytes())
}
/// Coinbase rewards that mature (are credited) when block `height` executes.
pub fn immature_key(mature_height: u64) -> Key {
    key(tag::IMMATURE, &mature_height.to_le_bytes())
}
pub fn counters_key() -> Key {
    key(tag::COUNTERS, &[])
}
pub fn job_index_key(i: u64) -> Key {
    key(tag::JOB_INDEX, &i.to_le_bytes())
}
pub fn withdrawal_key(nonce: u64) -> Key {
    key(tag::WITHDRAWAL, &nonce.to_le_bytes())
}
/// The payout lien (SetRoute) of an account; kept after it ends (route_nonce history).
pub fn route_key(a: &Pubkey) -> Key {
    key(tag::ROUTE, a)
}

/// A registered model (SPEC 4.5).
pub fn model_key(model_id: &Hash) -> Key {
    key(tag::MODEL, model_id)
}
pub fn model_index_key(i: u64) -> Key {
    key(tag::MODEL_INDEX, &i.to_le_bytes())
}
pub fn model_count_key() -> Key {
    key(tag::MODEL_COUNT, &[])
}

fn expect_tag(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Account {
    pub balance: u64,
    pub nonce: u64,
}

impl Account {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::ACCOUNT).u64(self.balance).u64(self.nonce);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::ACCOUNT)?;
        let a = Self { balance: r.u64()?, nonce: r.u64()? };
        r.finish()?;
        Ok(a)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JobStatus {
    Open = 0,
    Delivered = 1,
    Completed = 2,
    Expired = 3,
}

impl JobStatus {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Open,
            1 => Self::Delivered,
            2 => Self::Completed,
            3 => Self::Expired,
            _ => return Err(DecodeError::Invalid("job status")),
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Delivered => "delivered",
            Self::Completed => "completed",
            Self::Expired => "expired",
        }
    }
    /// Holds W in escrow.
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Open | Self::Delivered)
    }
}

/// Standing-job extension of a JobRecord (SPEC 4.5). Plain jobs carry none (and encode
/// exactly as before).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobExt {
    /// A paid inference ticket (never mineable).
    Ticket(TicketExt),
    /// A standing job of a ticket: B is registered matrix `matrix` of `model_id`.
    Segment { ticket_id: Hash, model_id: Hash, matrix: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TicketExt {
    pub model_id: Hash,
    /// The server (all zero until claimed by the first PostSegments).
    pub server: Pubkey,
    /// `round_up(prompt + max_new, row period)`.
    pub rows: u64,
    /// `alpha x P` at creation: the whole subsidy the ticket's segments may earn.
    pub cap_total: u64,
    pub segments: u32,
    /// Sum of the segments' max_blocks (at most the ticket's max_blocks).
    pub blocks_allocated: u32,
    pub blocks_mined: u32,
    pub subsidy_paid: u64,
    /// Segment rounds started (SPEC 4.5); the current round is `rounds - 1`.
    pub rounds: u16,
    /// m of the current round (every segment of a round has it).
    pub round_rows: u32,
    /// Sum of every round's m: the rows served, at most `rows`.
    pub rows_used: u64,
}

impl JobExt {
    fn write(&self, w: &mut W) {
        match self {
            JobExt::Ticket(t) => {
                w.u8(1)
                    .bytes(&t.model_id)
                    .bytes(&t.server)
                    .u64(t.rows)
                    .u64(t.cap_total)
                    .u32(t.segments)
                    .u32(t.blocks_allocated)
                    .u32(t.blocks_mined)
                    .u64(t.subsidy_paid)
                    .u16(t.rounds)
                    .u32(t.round_rows)
                    .u64(t.rows_used);
            }
            JobExt::Segment { ticket_id, model_id, matrix } => {
                w.u8(2).bytes(ticket_id).bytes(model_id).u16(*matrix);
            }
        }
    }

    fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(match r.u8()? {
            1 => JobExt::Ticket(TicketExt {
                model_id: r.arr()?,
                server: r.arr()?,
                rows: r.u64()?,
                cap_total: r.u64()?,
                segments: r.u32()?,
                blocks_allocated: r.u32()?,
                blocks_mined: r.u32()?,
                subsidy_paid: r.u64()?,
                rounds: r.u16()?,
                round_rows: r.u32()?,
                rows_used: r.u64()?,
            }),
            2 => JobExt::Segment { ticket_id: r.arr()?, model_id: r.arr()?, matrix: r.u16()? },
            _ => return Err(DecodeError::Invalid("job extension")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRecord {
    pub job_id: Hash,
    pub poster: Pubkey,
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub config: [u8; 52],
    pub hash_a: Hash,
    pub hash_b: Hash,
    pub data_uri: Vec<u8>,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub deadline_height: u64,
    pub max_blocks: u32,
    pub evaluators: Vec<Pubkey>,
    pub threshold: u8,
    pub created_height: u64,
    pub terms_hash: Hash,
    pub status: JobStatus,
    pub blocks_mined: u32,
    pub cap_remaining: u64,
    pub subsidy_paid: u64,
    pub deliverer: Pubkey,
    pub result_hash: Hash,
    pub result_uri: Vec<u8>,
    pub closed_height: u64,
    pub index: u64,
    /// First field of the Pearl job header (`job_pubkey`): the job id for a PostJob,
    /// the recipient (poster) for a storefront order (see [`order_terms_hash`]),
    /// the model id for a standing job (segment).
    pub header_id: Hash,
    /// Ticket or segment (SPEC 4.5); `None` for a plain job.
    pub ext: Option<JobExt>,
}

impl JobRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::JOB)
            .bytes(&self.job_id)
            .bytes(&self.poster)
            .u32(self.m)
            .u32(self.n)
            .u32(self.k)
            .bytes(&self.config)
            .bytes(&self.hash_a)
            .bytes(&self.hash_b)
            .bytes16(&self.data_uri)
            .u64(self.worker_pay)
            .u64(self.protocol_fee)
            .u64(self.deadline_height)
            .u32(self.max_blocks)
            .u8(self.evaluators.len() as u8);
        for e in &self.evaluators {
            w.bytes(e);
        }
        w.u8(self.threshold)
            .u64(self.created_height)
            .bytes(&self.terms_hash)
            .u8(self.status as u8)
            .u32(self.blocks_mined)
            .u64(self.cap_remaining)
            .u64(self.subsidy_paid)
            .bytes(&self.deliverer)
            .bytes(&self.result_hash)
            .bytes16(&self.result_uri)
            .u64(self.closed_height)
            .u64(self.index)
            .bytes(&self.header_id);
        if let Some(e) = &self.ext {
            e.write(&mut w);
        }
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::JOB)?;
        let job_id = r.arr()?;
        let poster = r.arr()?;
        let m = r.u32()?;
        let n = r.u32()?;
        let k = r.u32()?;
        let config = r.arr()?;
        let hash_a = r.arr()?;
        let hash_b = r.arr()?;
        let data_uri = r.bytes16(usize::MAX, "data_uri")?;
        let worker_pay = r.u64()?;
        let protocol_fee = r.u64()?;
        let deadline_height = r.u64()?;
        let max_blocks = r.u32()?;
        let ne = r.u8()? as usize;
        let mut evaluators = Vec::with_capacity(ne);
        for _ in 0..ne {
            evaluators.push(r.arr()?);
        }
        let j = Self {
            job_id,
            poster,
            m,
            n,
            k,
            config,
            hash_a,
            hash_b,
            data_uri,
            worker_pay,
            protocol_fee,
            deadline_height,
            max_blocks,
            evaluators,
            threshold: r.u8()?,
            created_height: r.u64()?,
            terms_hash: r.arr()?,
            status: JobStatus::from_u8(r.u8()?)?,
            blocks_mined: r.u32()?,
            cap_remaining: r.u64()?,
            subsidy_paid: r.u64()?,
            deliverer: r.arr()?,
            result_hash: r.arr()?,
            result_uri: r.bytes16(usize::MAX, "result_uri")?,
            closed_height: r.u64()?,
            index: r.u64()?,
            header_id: r.arr()?,
            ext: None,
        };
        let j = if r.remaining() > 0 { JobRecord { ext: Some(JobExt::read(&mut r)?), ..j } } else { j };
        r.finish()?;
        Ok(j)
    }

    pub fn ticket(&self) -> Option<&TicketExt> {
        match &self.ext {
            Some(JobExt::Ticket(t)) => Some(t),
            _ => None,
        }
    }

    pub fn is_ticket(&self) -> bool {
        self.ticket().is_some()
    }

    /// `(ticket_id, model_id, matrix)` of a segment.
    pub fn segment(&self) -> Option<(Hash, Hash, u16)> {
        match self.ext {
            Some(JobExt::Segment { ticket_id, model_id, matrix }) => Some((ticket_id, model_id, matrix)),
            _ => None,
        }
    }

    /// "job", "ticket" or "segment".
    pub fn kind_name(&self) -> &'static str {
        match self.ext {
            None => "job",
            Some(JobExt::Ticket(_)) => "ticket",
            Some(JobExt::Segment { .. }) => "segment",
        }
    }

    /// Pearl 76-byte job header for v2 chain jobs:
    /// `header_id || terms_hash || created_height = 0 u64 || share_target = 0 u32`,
    /// with `header_id = job_id` for a PostJob and `= poster` for a storefront order.
    /// The two trailing fields are zero so the poster can compute `job_key` (and so
    /// `hash_a`, `hash_b`) before the job lands in a block; the pow-bound seed already
    /// binds each proof to its parent, so there is no pre-mining to prevent.
    pub fn pearl_header(&self) -> pearl_job::JobHeader {
        pearl_job::JobHeader { job_pubkey: self.header_id, terms_hash: self.terms_hash, created_slot: 0, target_bits: 0 }
    }

    pub fn mining_config(&self) -> Result<pearl_job::MiningConfig, String> {
        pearl_job::MiningConfig::from_bytes(&self.config).map_err(|e| e.to_string())
    }

    /// The `pearl_job::Job` a verifier needs for blocks mined on this job.
    pub fn pearl_job(&self) -> Result<pearl_job::Job, String> {
        Ok(pearl_job::Job {
            header: self.pearl_header(),
            config: self.mining_config()?,
            m: self.m,
            n: self.n,
            hash_a: self.hash_a,
            hash_b: self.hash_b,
        })
    }

    pub fn job_key(&self) -> Result<Hash, String> {
        Ok(pearl_job::job_key(&self.pearl_header(), &self.mining_config()?))
    }
}

/// `terms_hash = blake3("pearl-sol:terms:v2" || poster || job_id || m u32 || n u32 || k u32
///  || worker_pay u64 || protocol_fee u64 || deadline_height u64 || max_blocks u32)`.
#[allow(clippy::too_many_arguments)]
pub fn terms_hash(
    poster: &Pubkey,
    job_id: &Hash,
    m: u32,
    n: u32,
    k: u32,
    worker_pay: u64,
    protocol_fee: u64,
    deadline_height: u64,
    max_blocks: u32,
) -> Hash {
    let mut w = W::new();
    w.bytes(b"pearl-sol:terms:v2")
        .bytes(poster)
        .bytes(job_id)
        .u32(m)
        .u32(n)
        .u32(k)
        .u64(worker_pay)
        .u64(protocol_fee)
        .u64(deadline_height)
        .u32(max_blocks);
    blake3_hash(&[&w.into_inner()])
}

/// Storefront orders: `terms_hash = blake3("pearl-sol:order-terms:v1" || poster || m u32 || n u32
///  || k u32 || max_blocks u32 || deadline_blocks u64)`, `deadline_blocks` RELATIVE as in the
/// order's PostJob body, and the Pearl header's first field is the poster (the unlock
/// recipient). The buyer commits `hash_a`, `hash_b` on Solana before the fill sets W, P
/// and before the bridge nonce (so the job id) and the unlock height are known, so the
/// order header uses only fields fixed when the order is opened.
pub fn order_terms_hash(poster: &Pubkey, m: u32, n: u32, k: u32, max_blocks: u32, deadline_blocks: u64) -> Hash {
    let mut w = W::new();
    w.bytes(b"pearl-sol:order-terms:v1").bytes(poster).u32(m).u32(n).u32(k).u32(max_blocks).u64(deadline_blocks);
    blake3_hash(&[&w.into_inner()])
}

/// `job_id = blake3(poster || poster_nonce u64 LE)` for a PostJob tx.
pub fn job_id_for_post(poster: &Pubkey, nonce: u64) -> Hash {
    blake3_hash(&[poster, &nonce.to_le_bytes()])
}

/// `job_id = sha256("pearl-sol:order-job:v1" || solana_nonce u64 LE)` for a storefront order.
pub fn job_id_for_order(solana_nonce: u64) -> Hash {
    agent_types::tx::order_job_id(solana_nonce)
}

/// A registered model (SPEC 4.5, schema tag 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub model_id: Hash,
    pub registrant: Pubkey,
    pub name: Vec<u8>,
    pub manifest_uri: Vec<u8>,
    pub recipe: u8,
    /// The model's mining config with `common_dim` of the first matrix (each matrix uses its own k).
    pub config: [u8; 52],
    pub matrices: Vec<agent_types::MatrixSpec>,
    /// `sum k x n` over the matrices: MACs per token row.
    pub macs_per_row: u64,
    pub registered_height: u64,
    pub index: u64,
    pub tickets: u64,
    pub segments: u64,
    pub blocks_mined: u64,
    pub subsidy_paid: u64,
}

impl Model {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::MODEL)
            .bytes(&self.model_id)
            .bytes(&self.registrant)
            .bytes8(&self.name)
            .bytes16(&self.manifest_uri)
            .u8(self.recipe)
            .bytes(&self.config)
            .u16(self.matrices.len() as u16);
        for m in &self.matrices {
            w.u32(m.k).u32(m.n).bytes(&m.hash_b);
        }
        w.u64(self.macs_per_row)
            .u64(self.registered_height)
            .u64(self.index)
            .u64(self.tickets)
            .u64(self.segments)
            .u64(self.blocks_mined)
            .u64(self.subsidy_paid);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::MODEL)?;
        let model_id = r.arr()?;
        let registrant = r.arr()?;
        let name = r.bytes8(255, "name")?;
        let manifest_uri = r.bytes16(usize::MAX, "manifest_uri")?;
        let recipe = r.u8()?;
        let config = r.arr()?;
        let n = r.u16()? as usize;
        let mut matrices = Vec::with_capacity(n);
        for _ in 0..n {
            matrices.push(agent_types::MatrixSpec { k: r.u32()?, n: r.u32()?, hash_b: r.arr()? });
        }
        let m = Self {
            model_id,
            registrant,
            name,
            manifest_uri,
            recipe,
            config,
            matrices,
            macs_per_row: r.u64()?,
            registered_height: r.u64()?,
            index: r.u64()?,
            tickets: r.u64()?,
            segments: r.u64()?,
            blocks_mined: r.u64()?,
            subsidy_paid: r.u64()?,
        };
        r.finish()?;
        Ok(m)
    }

    pub fn base_config(&self) -> Result<pearl_job::MiningConfig, String> {
        pearl_job::MiningConfig::from_bytes(&self.config).map_err(|e| e.to_string())
    }

    /// Mining config of matrix `i` (the model's rank and patterns, the matrix's k).
    pub fn matrix_config(&self, i: usize) -> Result<pearl_job::MiningConfig, String> {
        let m = self.matrices.get(i).ok_or("no such matrix")?;
        Ok(pearl_job::standing::matrix_config(&self.base_config()?, m.k))
    }
}

pub fn encode_model_index(model_id: &Hash) -> Vec<u8> {
    let mut v = vec![tag::MODEL_INDEX];
    v.extend_from_slice(model_id);
    v
}

pub fn decode_model_index(b: &[u8]) -> Result<Hash, DecodeError> {
    let mut r = R::new(b);
    expect_tag(&mut r, tag::MODEL_INDEX)?;
    let h = r.arr()?;
    r.finish()?;
    Ok(h)
}

pub fn encode_model_count(n: u64) -> Vec<u8> {
    let mut w = W::new();
    w.u8(tag::MODEL_COUNT).u64(n);
    w.into_inner()
}

pub fn decode_model_count(b: &[u8]) -> Result<u64, DecodeError> {
    let mut r = R::new(b);
    expect_tag(&mut r, tag::MODEL_COUNT)?;
    let n = r.u64()?;
    r.finish()?;
    Ok(n)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub name: Vec<u8>,
    pub uri: Vec<u8>,
    pub registered_height: u64,
    pub updated_height: u64,
}

impl Agent {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::AGENT).bytes8(&self.name).bytes16(&self.uri).u64(self.registered_height).u64(self.updated_height);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::AGENT)?;
        let a = Self {
            name: r.bytes8(255, "name")?,
            uri: r.bytes16(usize::MAX, "uri")?,
            registered_height: r.u64()?,
            updated_height: r.u64()?,
        };
        r.finish()?;
        Ok(a)
    }
}

/// Processed deposit (replay set entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deposit {
    pub height: u64,
    pub recipient: Pubkey,
    pub amount_spl: u64,
}

impl Deposit {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::DEPOSIT).u64(self.height).bytes(&self.recipient).u64(self.amount_spl);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::DEPOSIT)?;
        let d = Self { height: r.u64()?, recipient: r.arr()?, amount_spl: r.u64()? };
        r.finish()?;
        Ok(d)
    }
}

/// Coinbase rewards maturing at one height: `(miner, amount, earned_height)*`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Immature {
    pub entries: Vec<(Pubkey, u64, u64)>,
}

impl Immature {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::IMMATURE).u32(self.entries.len() as u32);
        for (m, a, h) in &self.entries {
            w.bytes(m).u64(*a).u64(*h);
        }
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::IMMATURE)?;
        let n = r.u32()? as usize;
        let mut entries = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            entries.push((r.arr()?, r.u64()?, r.u64()?));
        }
        r.finish()?;
        Ok(Self { entries })
    }
    pub fn total(&self) -> u64 {
        self.entries.iter().map(|e| e.1).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    pub job_count: u64,
    pub withdrawal_count: u64,
    pub deposit_count: u64,
}

impl Counters {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::COUNTERS).u64(self.job_count).u64(self.withdrawal_count).u64(self.deposit_count);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::COUNTERS)?;
        let c = Self { job_count: r.u64()?, withdrawal_count: r.u64()?, deposit_count: r.u64()? };
        r.finish()?;
        Ok(c)
    }
}

pub fn encode_job_index(job_id: &Hash) -> Vec<u8> {
    let mut v = vec![tag::JOB_INDEX];
    v.extend_from_slice(job_id);
    v
}

pub fn decode_job_index(b: &[u8]) -> Result<Hash, DecodeError> {
    let mut r = R::new(b);
    expect_tag(&mut r, tag::JOB_INDEX)?;
    let h = r.arr()?;
    r.finish()?;
    Ok(h)
}

/// A BridgeLock: one entry of the withdrawal log (keyless) and a state record.
///
/// Log entry bytes (88): `nonce u64 || from [32] || solana_recipient [32] || amount_spl u64 || height u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Withdrawal {
    pub nonce: u64,
    pub from: Pubkey,
    pub solana_recipient: Pubkey,
    pub amount_spl: u64,
    pub height: u64,
}

impl Withdrawal {
    pub const LOG_LEN: usize = 88;

    pub fn log_entry(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u64(self.nonce).bytes(&self.from).bytes(&self.solana_recipient).u64(self.amount_spl).u64(self.height);
        w.into_inner()
    }

    pub fn from_log_entry(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let w = Self { nonce: r.u64()?, from: r.arr()?, solana_recipient: r.arr()?, amount_spl: r.u64()?, height: r.u64()? };
        r.finish()?;
        Ok(w)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![tag::WITHDRAWAL];
        v.extend_from_slice(&self.log_entry());
        v
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.first() != Some(&tag::WITHDRAWAL) {
            return Err(DecodeError::Invalid("record tag"));
        }
        Self::from_log_entry(&b[1..])
    }
}

/// Payout lien of one account (SPEC 4.4, Agent Credit R2).
///
/// ```text
/// tag u8 = 10 | solana_recipient [32] | route_bps u16 | until_height u64 | cap u64
/// | release_key [32] | route_nonce u64 | set_height u64 | routed u64 | pending u64
/// | withdrawn u64 | cleared u8
/// ```
/// `routed`: income taken by the lien so far (counts toward `cap`); `pending`: taken but
/// not yet in a withdrawal entry (below 1000 base units, or the bridge is paused);
/// `withdrawn`: sent out in withdrawal entries (chain base units). The record stays after
/// the lien ends so `route_nonce` keeps increasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Route {
    pub solana_recipient: Pubkey,
    pub route_bps: u16,
    pub until_height: u64,
    pub cap: u64,
    pub release_key: Pubkey,
    pub route_nonce: u64,
    pub set_height: u64,
    pub routed: u64,
    pub pending: u64,
    pub withdrawn: u64,
    pub cleared: bool,
}

impl Route {
    /// The lien takes a share of income credited at `height`.
    pub fn is_active(&self, height: u64) -> bool {
        !self.cleared && height <= self.until_height && self.routed < self.cap
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::ROUTE)
            .bytes(&self.solana_recipient)
            .u16(self.route_bps)
            .u64(self.until_height)
            .u64(self.cap)
            .bytes(&self.release_key)
            .u64(self.route_nonce)
            .u64(self.set_height)
            .u64(self.routed)
            .u64(self.pending)
            .u64(self.withdrawn)
            .u8(self.cleared as u8);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::ROUTE)?;
        let x = Self {
            solana_recipient: r.arr()?,
            route_bps: r.u16()?,
            until_height: r.u64()?,
            cap: r.u64()?,
            release_key: r.arr()?,
            route_nonce: r.u64()?,
            set_height: r.u64()?,
            routed: r.u64()?,
            pending: r.u64()?,
            withdrawn: r.u64()?,
            cleared: match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(DecodeError::Invalid("cleared flag")),
            },
        };
        r.finish()?;
        Ok(x)
    }
}
