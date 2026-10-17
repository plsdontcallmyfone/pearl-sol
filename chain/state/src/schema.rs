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
    /// the recipient (poster) for a storefront order (see [`order_terms_hash`]).
    pub header_id: Hash,
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
        };
        r.finish()?;
        Ok(j)
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
