//! Agent-chain transactions as the light client needs them: the `tx_root` Merkle
//! path (chain/agent-types/src/merkle.rs) and parsers for the txs that create jobs
//! (PostJob, BridgeUnlock with a job order) and complete them (CompleteJob).
//! Layout: chain/agent-types/src/tx.rs, docs/RPC.md.

use crate::blake3;
use crate::Error;

/// `tx_hash = blake3(tx bytes)`.
pub fn tx_hash(tx: &[u8]) -> [u8; 32] {
    blake3::hash(tx)
}

/// Inclusion of `tx_hash` under `root`. `path` is a list of 33-byte entries:
/// `sibling [32] || is_left u8` (bottom-up).
pub fn verify_tx_path(tx_hash: &[u8; 32], path: &[u8], root: &[u8; 32]) -> Result<(), Error> {
    if path.len() % 33 != 0 || path.len() / 33 > 40 {
        return Err(Error::TxPath);
    }
    let mut h = blake3::hash_parts(&[&[0u8], tx_hash]);
    for e in path.chunks(33) {
        let sib = &e[..32];
        h = match e[32] {
            1 => blake3::hash_parts(&[&[1u8], sib, &h]),
            0 => blake3::hash_parts(&[&[1u8], &h, sib]),
            _ => return Err(Error::TxPath),
        };
    }
    if h != *root {
        return Err(Error::TxPath);
    }
    Ok(())
}

pub const KIND_POST_JOB: u8 = 2;
pub const KIND_COMPLETE_JOB: u8 = 4;
pub const KIND_EXPIRE_JOB: u8 = 5;
pub const KIND_BRIDGE_UNLOCK: u8 = 6;
const BODY: usize = 54;

pub struct TxView<'a> {
    pub b: &'a [u8],
}

impl<'a> TxView<'a> {
    pub fn new(b: &'a [u8], chain_id: u32) -> Result<Self, Error> {
        if b.len() < BODY + 64 || b[0] != 1 || u32::from_le_bytes(b[1..5].try_into().unwrap()) != chain_id {
            return Err(Error::Tx);
        }
        Ok(Self { b })
    }
    /// Any chain id (the caller already proved the tx is in one of our blocks).
    pub fn any_chain(b: &'a [u8]) -> Result<Self, Error> {
        if b.len() < BODY + 64 || b[0] != 1 {
            return Err(Error::Tx);
        }
        Ok(Self { b })
    }
    pub fn from(&self) -> [u8; 32] {
        self.b[5..37].try_into().unwrap()
    }
    pub fn nonce(&self) -> u64 {
        u64::from_le_bytes(self.b[37..45].try_into().unwrap())
    }
    pub fn kind(&self) -> u8 {
        self.b[53]
    }
    /// Body bytes (between kind and sig).
    pub fn body(&self) -> &'a [u8] {
        &self.b[BODY..self.b.len() - 64]
    }
}

struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Tx)?;
        let s = self.b.get(self.pos..end).ok_or(Error::Tx)?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn arr(&mut self) -> Result<[u8; 32], Error> {
        Ok(self.take(32)?.try_into().unwrap())
    }
}

/// The fields of a PostJob body the light client keeps.
pub struct PostJobFields {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub config: [u8; 52],
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub deadline: u64,
    pub max_blocks: u32,
}

fn read_post_job(r: &mut Rd) -> Result<PostJobFields, Error> {
    let m = r.u32()?;
    let n = r.u32()?;
    let k = r.u32()?;
    let config: [u8; 52] = r.take(52)?.try_into().unwrap();
    let hash_a = r.arr()?;
    let hash_b = r.arr()?;
    let ul = r.u16()? as usize;
    r.take(ul)?;
    let worker_pay = r.u64()?;
    let protocol_fee = r.u64()?;
    let deadline = r.u64()?;
    let max_blocks = r.u32()?;
    let ec = r.u8()? as usize;
    r.take(32 * ec)?;
    r.u8()?; // threshold
    Ok(PostJobFields { m, n, k, config, hash_a, hash_b, worker_pay, protocol_fee, deadline, max_blocks })
}

/// A job as the light client stores it.
pub struct JobCommit {
    pub job_id: [u8; 32],
    /// Pearl 76-byte job header: `header_id || terms_hash || 0 u64 || 0 u32`.
    pub header76: [u8; 76],
    pub config: [u8; 52],
    pub m: u32,
    pub n: u32,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
}

fn header76(id: &[u8; 32], terms: &[u8; 32]) -> [u8; 76] {
    let mut h = [0u8; 76];
    h[..32].copy_from_slice(id);
    h[32..64].copy_from_slice(terms);
    h
}

/// Job created by a PostJob tx or a BridgeUnlock with a job order. `sha256` is
/// needed for the order job id (Solana syscall on chain).
pub fn job_from_tx(tx: &TxView, sha256: fn(&[&[u8]]) -> [u8; 32]) -> Result<JobCommit, Error> {
    let mut r = Rd { b: tx.body(), pos: 0 };
    let (job_id, header_id, terms, f) = match tx.kind() {
        KIND_POST_JOB => {
            let f = read_post_job(&mut r)?;
            let poster = tx.from();
            let job_id = blake3::hash_parts(&[&poster, &tx.nonce().to_le_bytes()]);
            let terms = blake3::hash_parts(&[
                b"pearl-sol:terms:v2",
                &poster,
                &job_id,
                &f.m.to_le_bytes(),
                &f.n.to_le_bytes(),
                &f.k.to_le_bytes(),
                &f.worker_pay.to_le_bytes(),
                &f.protocol_fee.to_le_bytes(),
                &f.deadline.to_le_bytes(),
                &f.max_blocks.to_le_bytes(),
            ]);
            (job_id, job_id, terms, f)
        }
        KIND_BRIDGE_UNLOCK => {
            let solana_nonce = r.u64()?;
            let recipient = r.arr()?;
            r.u64()?;
            if r.u8()? != 1 {
                return Err(Error::Tx);
            }
            let f = read_post_job(&mut r)?;
            let job_id = sha256(&[b"pearl-sol:order-job:v1", &solana_nonce.to_le_bytes()]);
            let terms = blake3::hash_parts(&[
                b"pearl-sol:order-terms:v1",
                &recipient,
                &f.m.to_le_bytes(),
                &f.n.to_le_bytes(),
                &f.k.to_le_bytes(),
                &f.max_blocks.to_le_bytes(),
                &f.deadline.to_le_bytes(),
            ]);
            (job_id, recipient, terms, f)
        }
        _ => return Err(Error::Tx),
    };
    let cfg = crate::config::MiningConfig::from_bytes(&f.config)?;
    if cfg.k != f.k {
        return Err(Error::Tx);
    }
    Ok(JobCommit { job_id, header76: header76(&header_id, &terms), config: f.config, m: f.m, n: f.n, hash_a: f.hash_a, hash_b: f.hash_b })
}

/// CompleteJob: `(job_id, offset of the first (pubkey, sig) pair in the tx, count)`.
pub fn complete_job(tx: &TxView) -> Result<([u8; 32], usize, usize), Error> {
    if tx.kind() != KIND_COMPLETE_JOB {
        return Err(Error::Tx);
    }
    let body = tx.body();
    if body.len() < 33 {
        return Err(Error::Tx);
    }
    let job_id: [u8; 32] = body[..32].try_into().unwrap();
    let count = body[32] as usize;
    if body.len() != 33 + 96 * count {
        return Err(Error::Tx);
    }
    Ok((job_id, BODY + 33, count))
}

/// ExpireJob: the job id.
pub fn expire_job(tx: &TxView) -> Result<[u8; 32], Error> {
    let body = tx.body();
    if tx.kind() != KIND_EXPIRE_JOB || body.len() != 32 {
        return Err(Error::Tx);
    }
    Ok(body.try_into().unwrap())
}
