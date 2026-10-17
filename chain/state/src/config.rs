//! Chain parameters. All of them live in state and are editable by the admin key
//! through `AdminConfig { key, value }` (key ids below; values little-endian).

use agent_types::codec::{R, W};
use agent_types::{DecodeError, Pubkey};

use crate::schema::tag;

pub mod keys {
    pub const ADMIN: u8 = 0; // [32]
    pub const MIN_FEE: u8 = 1; // u64
    pub const TARGET_BLOCK_TIME_MS: u8 = 2; // u64
    pub const WTEMA_HALF_LIFE_MS: u8 = 3; // u64
    pub const POW_LIMIT_BITS: u8 = 4; // u32 (easiest allowed target, compact)
    pub const FINALITY_K: u8 = 5; // u32
    pub const EMISSION_PPM: u8 = 6; // u64 (per block, of the pool)
    pub const ALPHA_PPM: u8 = 7; // u64 (< 1e6)
    pub const ATTESTERS: u8 = 8; // count u8 || pubkey*
    pub const ATTESTER_THRESHOLD: u8 = 9; // u8
    pub const MAX_BLOCK_TXS: u8 = 10; // u32
    pub const MAX_FUTURE_MS: u8 = 11; // u64
    pub const FALLBACK_M: u8 = 12; // u32
    pub const FALLBACK_N: u8 = 13; // u32
    pub const FALLBACK_CONFIG: u8 = 14; // [52]
    pub const MIN_PROTOCOL_FEE: u8 = 15; // u64
    pub const BRIDGE_PAUSED: u8 = 16; // u8 (0|1)
    pub const MAX_JOB_BLOCKS: u8 = 17; // u32 (upper bound on a job's max_blocks)
    pub const MIN_DIFFICULTY_BITS: u8 = 18; // u32 (hardest allowed target, compact; 0 = none)
    pub const LAST: u8 = 18;

    pub fn name(k: u8) -> &'static str {
        match k {
            ADMIN => "admin",
            MIN_FEE => "min_fee",
            TARGET_BLOCK_TIME_MS => "target_block_time_ms",
            WTEMA_HALF_LIFE_MS => "wtema_half_life_ms",
            POW_LIMIT_BITS => "pow_limit_bits",
            FINALITY_K => "finality_k",
            EMISSION_PPM => "emission_ppm",
            ALPHA_PPM => "alpha_ppm",
            ATTESTERS => "attesters",
            ATTESTER_THRESHOLD => "attester_threshold",
            MAX_BLOCK_TXS => "max_block_txs",
            MAX_FUTURE_MS => "max_future_ms",
            FALLBACK_M => "fallback_m",
            FALLBACK_N => "fallback_n",
            FALLBACK_CONFIG => "fallback_config",
            MIN_PROTOCOL_FEE => "min_protocol_fee",
            BRIDGE_PAUSED => "bridge_paused",
            MAX_JOB_BLOCKS => "max_job_blocks",
            MIN_DIFFICULTY_BITS => "min_difficulty_bits",
            _ => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub admin: Pubkey,
    pub min_fee: u64,
    pub target_block_time_ms: u64,
    pub wtema_half_life_ms: u64,
    pub pow_limit_bits: u32,
    pub finality_k: u32,
    pub emission_ppm: u64,
    pub alpha_ppm: u64,
    pub attesters: Vec<Pubkey>,
    pub attester_threshold: u8,
    pub max_block_txs: u32,
    pub max_future_ms: u64,
    pub fallback_m: u32,
    pub fallback_n: u32,
    pub fallback_config: [u8; 52],
    pub min_protocol_fee: u64,
    pub bridge_paused: bool,
    pub max_job_blocks: u32,
    pub min_difficulty_bits: u32,
}

impl Config {
    /// SPEC defaults (60 s blocks, K = 30, alpha 0.8, 7 ppm per block).
    pub fn defaults(admin: Pubkey) -> Self {
        Self {
            admin,
            min_fee: 1000,
            target_block_time_ms: 60_000,
            wtema_half_life_ms: 3_600_000,
            pow_limit_bits: 0x207fffff,
            finality_k: 30,
            emission_ppm: 7,
            alpha_ppm: 800_000,
            attesters: Vec::new(),
            attester_threshold: 1,
            max_block_txs: 2000,
            max_future_ms: 120_000,
            fallback_m: 4,
            fallback_n: 8,
            fallback_config: pearl_job::MiningConfig::localnet_default().to_bytes(),
            min_protocol_fee: 0,
            bridge_paused: false,
            max_job_blocks: 100_000,
            min_difficulty_bits: 0,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::CONFIG).u8(1);
        for k in 0..=keys::LAST {
            w.bytes16(&self.value(k));
        }
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        if r.u8()? != tag::CONFIG || r.u8()? != 1 {
            return Err(DecodeError::Invalid("config tag"));
        }
        let mut c = Self::defaults([0; 32]);
        for k in 0..=keys::LAST {
            let v = r.bytes16(usize::MAX, "config value")?;
            c.set(k, &v).map_err(|_| DecodeError::Invalid("config value"))?;
        }
        r.finish()?;
        Ok(c)
    }

    /// Encoded value of parameter `k`.
    pub fn value(&self, k: u8) -> Vec<u8> {
        use keys::*;
        match k {
            ADMIN => self.admin.to_vec(),
            MIN_FEE => self.min_fee.to_le_bytes().to_vec(),
            TARGET_BLOCK_TIME_MS => self.target_block_time_ms.to_le_bytes().to_vec(),
            WTEMA_HALF_LIFE_MS => self.wtema_half_life_ms.to_le_bytes().to_vec(),
            POW_LIMIT_BITS => self.pow_limit_bits.to_le_bytes().to_vec(),
            FINALITY_K => self.finality_k.to_le_bytes().to_vec(),
            EMISSION_PPM => self.emission_ppm.to_le_bytes().to_vec(),
            ALPHA_PPM => self.alpha_ppm.to_le_bytes().to_vec(),
            ATTESTERS => {
                let mut v = vec![self.attesters.len() as u8];
                for a in &self.attesters {
                    v.extend_from_slice(a);
                }
                v
            }
            ATTESTER_THRESHOLD => vec![self.attester_threshold],
            MAX_BLOCK_TXS => self.max_block_txs.to_le_bytes().to_vec(),
            MAX_FUTURE_MS => self.max_future_ms.to_le_bytes().to_vec(),
            FALLBACK_M => self.fallback_m.to_le_bytes().to_vec(),
            FALLBACK_N => self.fallback_n.to_le_bytes().to_vec(),
            FALLBACK_CONFIG => self.fallback_config.to_vec(),
            MIN_PROTOCOL_FEE => self.min_protocol_fee.to_le_bytes().to_vec(),
            BRIDGE_PAUSED => vec![self.bridge_paused as u8],
            MAX_JOB_BLOCKS => self.max_job_blocks.to_le_bytes().to_vec(),
            MIN_DIFFICULTY_BITS => self.min_difficulty_bits.to_le_bytes().to_vec(),
            _ => Vec::new(),
        }
    }

    /// Set parameter `k` from its encoded value, checking it. Cross-field rules are in
    /// [`Config::validate`].
    pub fn set(&mut self, k: u8, v: &[u8]) -> Result<(), String> {
        use keys::*;
        fn u64v(v: &[u8]) -> Result<u64, String> {
            Ok(u64::from_le_bytes(v.try_into().map_err(|_| "expected u64".to_string())?))
        }
        fn u32v(v: &[u8]) -> Result<u32, String> {
            Ok(u32::from_le_bytes(v.try_into().map_err(|_| "expected u32".to_string())?))
        }
        match k {
            ADMIN => self.admin = v.try_into().map_err(|_| "expected 32-byte pubkey")?,
            MIN_FEE => self.min_fee = u64v(v)?,
            TARGET_BLOCK_TIME_MS => self.target_block_time_ms = u64v(v)?,
            WTEMA_HALF_LIFE_MS => self.wtema_half_life_ms = u64v(v)?,
            POW_LIMIT_BITS => self.pow_limit_bits = u32v(v)?,
            FINALITY_K => self.finality_k = u32v(v)?,
            EMISSION_PPM => self.emission_ppm = u64v(v)?,
            ALPHA_PPM => self.alpha_ppm = u64v(v)?,
            ATTESTERS => {
                let n = *v.first().ok_or("empty attester list")? as usize;
                if v.len() != 1 + 32 * n {
                    return Err("attester list length".into());
                }
                self.attesters = v[1..].chunks(32).map(|c| c.try_into().unwrap()).collect();
            }
            ATTESTER_THRESHOLD => {
                if v.len() != 1 {
                    return Err("expected u8".into());
                }
                self.attester_threshold = v[0];
            }
            MAX_BLOCK_TXS => self.max_block_txs = u32v(v)?,
            MAX_FUTURE_MS => self.max_future_ms = u64v(v)?,
            FALLBACK_M => self.fallback_m = u32v(v)?,
            FALLBACK_N => self.fallback_n = u32v(v)?,
            FALLBACK_CONFIG => self.fallback_config = v.try_into().map_err(|_| "expected 52 bytes")?,
            MIN_PROTOCOL_FEE => self.min_protocol_fee = u64v(v)?,
            BRIDGE_PAUSED => {
                if v.len() != 1 || v[0] > 1 {
                    return Err("expected 0 or 1".into());
                }
                self.bridge_paused = v[0] == 1;
            }
            MAX_JOB_BLOCKS => self.max_job_blocks = u32v(v)?,
            MIN_DIFFICULTY_BITS => self.min_difficulty_bits = u32v(v)?,
            _ => return Err(format!("unknown config key {k}")),
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.target_block_time_ms == 0 || self.wtema_half_life_ms == 0 {
            return Err("block time and half life must be positive".into());
        }
        if !(1..=10_000).contains(&self.finality_k) {
            return Err("finality_k must be in 1..=10000".into());
        }
        if self.alpha_ppm >= 1_000_000 {
            return Err("alpha must be below 1 (alpha_ppm < 1e6)".into());
        }
        if self.emission_ppm > 1_000_000 {
            return Err("emission_ppm above 1e6".into());
        }
        if self.attester_threshold as usize > self.attesters.len() || (!self.attesters.is_empty() && self.attester_threshold == 0)
        {
            return Err("attester threshold must be 1..=attesters".into());
        }
        let mut s = self.attesters.clone();
        s.sort();
        s.dedup();
        if s.len() != self.attesters.len() {
            return Err("duplicate attester".into());
        }
        if pearl_job::job::nbits_to_target(self.pow_limit_bits).is_zero() {
            return Err("pow_limit_bits is not a valid compact target".into());
        }
        if self.max_block_txs == 0 {
            return Err("max_block_txs must be positive".into());
        }
        let fc = pearl_job::MiningConfig::from_bytes(&self.fallback_config).map_err(|e| e.to_string())?;
        pearl_job::job::check_shape(&fc, self.fallback_m as usize, self.fallback_n as usize).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn fallback_params(&self) -> pearl_job::pow::FallbackParams {
        pearl_job::pow::FallbackParams {
            m: self.fallback_m,
            n: self.fallback_n,
            config: pearl_job::MiningConfig::from_bytes(&self.fallback_config).expect("validated"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults_valid() {
        let mut c = Config::defaults([7; 32]);
        c.attesters = vec![[1; 32], [2; 32]];
        c.attester_threshold = 2;
        c.validate().unwrap();
        assert_eq!(Config::decode(&c.encode()).unwrap(), c);
        for k in 0..=keys::LAST {
            let mut d = c.clone();
            d.set(k, &c.value(k)).unwrap();
            assert_eq!(d, c, "key {k}");
        }
    }
}
