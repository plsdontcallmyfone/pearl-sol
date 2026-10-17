//! JSON description of a job (what the jobs account holds), shared by the CLI,
//! the miner and the attester. Strings: pubkeys base58, hashes and config hex.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::job::{Job, JobHeader, MiningConfig};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    /// Job account pubkey, base58.
    pub job: String,
    /// blake3 of the job terms, hex (32 bytes).
    pub terms_hash: String,
    pub created_slot: u64,
    /// Compact share target (nbits).
    pub target_bits: u32,
    /// 52-byte mining config, hex.
    pub config: String,
    pub m: u32,
    pub n: u32,
    /// hex; may be empty before commitments are computed.
    #[serde(default)]
    pub hash_a: String,
    #[serde(default)]
    pub hash_b: String,
    #[serde(default)]
    pub data_uri: String,
    /// Exclusive upper bound on nonces (SPEC 3.4 `max_nonce`); 0 = unknown.
    #[serde(default)]
    pub max_nonce: u64,
}

pub fn parse_pubkey(s: &str) -> Result<[u8; 32]> {
    let v = bs58::decode(s).into_vec().with_context(|| format!("bad base58 pubkey {s:?}"))?;
    ensure!(v.len() == 32, "pubkey {s:?} decodes to {} bytes", v.len());
    Ok(v.try_into().unwrap())
}

pub fn parse_hash(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim_start_matches("0x")).with_context(|| format!("bad hex {s:?}"))?;
    ensure!(v.len() == 32, "hash {s:?} is {} bytes, expected 32", v.len());
    Ok(v.try_into().unwrap())
}

/// A 32-byte value as 64 hex characters (optional 0x) or base58.
pub fn parse_key32(s: &str) -> Result<[u8; 32]> {
    let t = s.trim_start_matches("0x");
    if t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        parse_hash(t)
    } else {
        parse_pubkey(s)
    }
}

/// Decimal or 0x-prefixed hex.
pub fn parse_u32(s: &str) -> Result<u32> {
    Ok(match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16)?,
        None => s.parse()?,
    })
}

impl JobSpec {
    pub fn header(&self) -> Result<JobHeader> {
        Ok(JobHeader {
            job_pubkey: parse_key32(&self.job)?,
            terms_hash: parse_hash(&self.terms_hash)?,
            created_slot: self.created_slot,
            target_bits: self.target_bits,
        })
    }

    pub fn mining_config(&self) -> Result<MiningConfig> {
        MiningConfig::from_bytes(&hex::decode(&self.config).context("config hex")?)
    }

    pub fn job_key(&self) -> Result<[u8; 32]> {
        Ok(crate::job::job_key(&self.header()?, &self.mining_config()?))
    }

    /// Full job; needs `hash_a` and `hash_b`.
    pub fn to_job(&self) -> Result<Job> {
        Ok(Job {
            header: self.header()?,
            config: self.mining_config()?,
            m: self.m,
            n: self.n,
            hash_a: parse_hash(&self.hash_a).context("hash_a")?,
            hash_b: parse_hash(&self.hash_b).context("hash_b")?,
        })
    }
}
