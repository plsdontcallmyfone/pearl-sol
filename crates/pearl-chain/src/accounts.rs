//! Borsh decoders for the agent-chain Anchor accounts the services read.
//! Source of truth: agent/programs/jobs/src/lib.rs (`Job`) and
//! agent/programs/mining/src/lib.rs (`MiningConfig`, `Epoch`).

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

use pearl_job::job::JobHeader;
use pearl_job::JobSpec;

use crate::key::Pubkey;

pub fn account_discriminator(name: &str) -> [u8; 8] {
    let h: [u8; 32] = Sha256::digest(format!("account:{name}").as_bytes()).into();
    h[..8].try_into().unwrap()
}

pub fn ix_discriminator(name: &str) -> [u8; 8] {
    let h: [u8; 32] = Sha256::digest(format!("global:{name}").as_bytes()).into();
    h[..8].try_into().unwrap()
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.p + n <= self.b.len(), "account data truncated at {}", self.p);
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn pk(&mut self) -> Result<Pubkey> {
        Ok(Pubkey(self.arr()?))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8(self.take(n)?.to_vec())?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Open,
    Delivered,
    Completed,
    Disputed,
    Expired,
    Refunded,
}

/// Byte offset of `Job.status` (after discriminator, poster, job_id): for memcmp.
pub const JOB_STATUS_OFFSET: usize = 8 + 32 + 8;

#[derive(Debug, Clone)]
pub struct JobAccount {
    pub address: Pubkey,
    pub poster: Pubkey,
    pub job_id: u64,
    pub status: JobStatus,
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub config: [u8; 52],
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub data_uri: String,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub created_slot: u64,
    pub posted_slot: u64,
    pub deadline_slot: u64,
    pub share_target: u32,
    pub terms_hash: [u8; 32],
    pub header: [u8; 76],
    pub deliverer: Pubkey,
    pub result_hash: [u8; 32],
    pub result_uri: String,
    pub delivered_slot: u64,
}

impl JobAccount {
    pub fn decode(address: Pubkey, data: &[u8]) -> Result<Self> {
        let mut r = R { b: data, p: 0 };
        ensure!(r.arr::<8>()? == account_discriminator("Job"), "not a jobs::Job account");
        let poster = r.pk()?;
        let job_id = r.u64()?;
        let status = match r.u8()? {
            0 => JobStatus::Open,
            1 => JobStatus::Delivered,
            2 => JobStatus::Completed,
            3 => JobStatus::Disputed,
            4 => JobStatus::Expired,
            5 => JobStatus::Refunded,
            s => bail!("unknown job status {s}"),
        };
        Ok(Self {
            address,
            poster,
            job_id,
            status,
            hash_a: r.arr()?,
            hash_b: r.arr()?,
            config: r.arr()?,
            m: r.u32()?,
            n: r.u32()?,
            k: r.u32()?,
            data_uri: r.string()?,
            worker_pay: r.u64()?,
            protocol_fee: r.u64()?,
            created_slot: r.u64()?,
            posted_slot: r.u64()?,
            deadline_slot: r.u64()?,
            share_target: r.u32()?,
            terms_hash: r.arr()?,
            header: r.arr()?,
            deliverer: r.pk()?,
            result_hash: r.arr()?,
            result_uri: r.string()?,
            delivered_slot: r.u64()?,
        })
    }

    /// The JobSpec the proof layer uses. Rebuilds the header from the account fields
    /// and checks it equals the stored header.
    pub fn to_spec(&self, max_nonce: u64) -> Result<JobSpec> {
        let h = JobHeader {
            job_pubkey: self.address.0,
            terms_hash: self.terms_hash,
            created_slot: self.created_slot,
            target_bits: self.share_target,
        };
        ensure!(h.to_bytes() == self.header, "job {} stored header does not match its fields", self.address);
        let cfg = pearl_job::MiningConfig::from_bytes(&self.config).context("job config")?;
        ensure!(cfg.common_dim == self.k, "job {} config k={} but k field={}", self.address, cfg.common_dim, self.k);
        Ok(JobSpec {
            job: self.address.to_b58(),
            terms_hash: hex::encode(self.terms_hash),
            created_slot: self.created_slot,
            target_bits: self.share_target,
            config: hex::encode(self.config),
            m: self.m,
            n: self.n,
            hash_a: hex::encode(self.hash_a),
            hash_b: hex::encode(self.hash_b),
            data_uri: self.data_uri.clone(),
            max_nonce,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MiningConfigAccount {
    pub admin: Pubkey,
    pub attesters: Vec<Pubkey>,
    pub threshold: u8,
    pub epoch_slots: u64,
    pub emission_bps: u16,
    pub alpha_bps: u16,
    pub max_nonce: u32,
    pub current_epoch: u64,
    pub outstanding: u64,
}

impl MiningConfigAccount {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut r = R { b: data, p: 0 };
        ensure!(r.arr::<8>()? == account_discriminator("MiningConfig"), "not a mining::MiningConfig account");
        let admin = r.pk()?;
        let n = r.u32()? as usize;
        ensure!(n <= 64, "attester list too long");
        let attesters = (0..n).map(|_| r.pk()).collect::<Result<_>>()?;
        Ok(Self {
            admin,
            attesters,
            threshold: r.u8()?,
            epoch_slots: r.u64()?,
            emission_bps: r.u16()?,
            alpha_bps: r.u16()?,
            max_nonce: r.u32()?,
            current_epoch: r.u64()?,
            outstanding: r.u64()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EpochAccount {
    pub index: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub budget: u64,
}

impl EpochAccount {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut r = R { b: data, p: 0 };
        ensure!(r.arr::<8>()? == account_discriminator("Epoch"), "not a mining::Epoch account");
        Ok(Self { index: r.u64()?, start_slot: r.u64()?, end_slot: r.u64()?, budget: r.u64()? })
    }
}
