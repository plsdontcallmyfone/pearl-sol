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
    /// Last key of the original encoding (always written; keys above it are appended only
    /// when one differs from its default, so the record is byte-identical to an old node's
    /// until an admin sets a new key, which the executor allows only from the social upgrade
    /// height, SPEC 4.6).
    pub const LEGACY_LAST: u8 = 18;
    pub const SOCIAL_MIN_FEE: u8 = 19; // u64
    pub const SOCIAL_BYTE_FEE: u8 = 20; // u64 (per encoded tx byte)
    pub const HOUSE_OPERATORS: u8 = 21; // count u8 || pubkey* (<= 32)
    pub const POST_INLINE_MAX: u8 = 22; // u32 (1..=8192)
    pub const MAX_BALANCE_ROWS: u8 = 23; // u32 (4096..=1,048,576)
    pub const PRICE_BASE_LAMPORTS: u8 = 24; // u64 or empty (unset)
    pub const PRICE_MLAMPORTS_PER_GFLOP: u8 = 25; // u64 or empty (unset)
    pub const PRICE_OVERHEAD_LAMPORTS: u8 = 26; // u64 or empty (unset)
    /// Last key of the social extension (keys 19-26). Keys above it (the contracts
    /// extension, docs/CONTRACTS.md) are appended only when one differs from its default.
    pub const SOCIAL_LAST: u8 = 26;
    pub const GAS_PRICE: u8 = 27; // u64 (chain base units per 1,000,000 gas)
    pub const BLOCK_GAS_LIMIT: u8 = 28; // u64 (sum of the gas limits of a block's contract txs)
    pub const TX_GAS_LIMIT: u8 = 29; // u64 (largest gas limit of one tx)
    pub const CONTRACT_BASE_FEE: u8 = 30; // u64 (per contract tx)
    pub const CONTRACT_BYTE_FEE: u8 = 31; // u64 (per encoded contract tx byte)
    pub const STORAGE_BYTE_DEPOSIT: u8 = 32; // u64 (per stored byte, refundable)
    pub const MAX_CODE_SIZE: u8 = 33; // u32 (bytes, <= agent_types::contracts::MAX_CODE_SIZE)
    pub const MAX_CALL_DEPTH: u8 = 34; // u8 (1..=16)
    pub const LAST: u8 = 34;

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
            SOCIAL_MIN_FEE => "social_min_fee",
            SOCIAL_BYTE_FEE => "social_byte_fee",
            HOUSE_OPERATORS => "house_operators",
            POST_INLINE_MAX => "post_inline_max",
            MAX_BALANCE_ROWS => "max_balance_rows",
            PRICE_BASE_LAMPORTS => "price_base_lamports",
            PRICE_MLAMPORTS_PER_GFLOP => "price_mlamports_per_gflop",
            PRICE_OVERHEAD_LAMPORTS => "price_overhead_lamports",
            GAS_PRICE => "gas_price",
            BLOCK_GAS_LIMIT => "block_gas_limit",
            TX_GAS_LIMIT => "tx_gas_limit",
            CONTRACT_BASE_FEE => "contract_base_fee",
            CONTRACT_BYTE_FEE => "contract_byte_fee",
            STORAGE_BYTE_DEPOSIT => "storage_byte_deposit",
            MAX_CODE_SIZE => "max_code_size",
            MAX_CALL_DEPTH => "max_call_depth",
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
    /// Social kinds (SPEC 4.6): `fee >= max(min_fee, social_min_fee + social_byte_fee x len)`.
    pub social_min_fee: u64,
    pub social_byte_fee: u64,
    /// Operators whose linked agents may carry the `house` profile flag.
    pub house_operators: Vec<Pubkey>,
    /// Largest post body (bytes).
    pub post_inline_max: u32,
    /// Largest row budget of a balance ticket (n = 1), SPEC 4.5 / 4.6.
    pub max_balance_rows: u32,
    /// Inference price knobs read by storefront-x402 (SPEC 5.4); None = the service default.
    pub price_base_lamports: Option<u64>,
    pub price_mlamports_per_gflop: Option<u64>,
    pub price_overhead_lamports: Option<u64>,
    /// Contracts (docs/CONTRACTS.md): fee floor of a contract tx is
    /// `max(min_fee, contract_base_fee + contract_byte_fee x len + ceil(gas_limit x gas_price / 1e6))`;
    /// what is charged uses gas used instead of the limit, the rest is refunded.
    pub gas_price: u64,
    pub block_gas_limit: u64,
    pub tx_gas_limit: u64,
    pub contract_base_fee: u64,
    pub contract_byte_fee: u64,
    pub storage_byte_deposit: u64,
    pub max_code_size: u32,
    pub max_call_depth: u8,
}

/// Gas price unit: `gas_price` is chain base units per this much gas.
pub const GAS_PRICE_UNIT: u64 = 1_000_000;

/// Upper bound of `max_call_depth`.
pub const MAX_CALL_DEPTH_LIMIT: u8 = 16;

/// Default contract parameters (docs/CONTRACTS.md "Gas": measured on the devnet boxes).
pub struct ContractDefaults {
    pub gas_price: u64,
    pub block_gas_limit: u64,
    pub tx_gas_limit: u64,
    pub contract_base_fee: u64,
    pub contract_byte_fee: u64,
    pub storage_byte_deposit: u64,
    pub max_code_size: u32,
    pub max_call_depth: u8,
}

/// `block_gas_limit` 150M gas is about 1.1 s of the worst measured wasm (call_indirect at 7.36
/// ns/gas with wasmi's flat cost table) and about 0.13 s of typical code (0.77 ns/gas), so a
/// full block of contracts fits well inside a 60 s block and leaves room for shorter blocks.
pub const CONTRACT_DEFAULTS: ContractDefaults = ContractDefaults {
    gas_price: 10_000,
    block_gas_limit: 150_000_000,
    tx_gas_limit: 50_000_000,
    contract_base_fee: 1000,
    contract_byte_fee: 10,
    storage_byte_deposit: 100_000,
    max_code_size: 128 * 1024,
    max_call_depth: 8,
};

/// Most `house_operators`.
pub const MAX_HOUSE_OPERATORS: usize = 32;
/// Bounds of `max_balance_rows`.
pub const MIN_BALANCE_ROWS_LIMIT: u32 = 4096;
pub const MAX_BALANCE_ROWS_LIMIT: u32 = 1 << 20;

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
            social_min_fee: 1000,
            social_byte_fee: 0,
            house_operators: Vec::new(),
            post_inline_max: 1024,
            max_balance_rows: MIN_BALANCE_ROWS_LIMIT,
            price_base_lamports: None,
            price_mlamports_per_gflop: None,
            price_overhead_lamports: None,
            gas_price: CONTRACT_DEFAULTS.gas_price,
            block_gas_limit: CONTRACT_DEFAULTS.block_gas_limit,
            tx_gas_limit: CONTRACT_DEFAULTS.tx_gas_limit,
            contract_base_fee: CONTRACT_DEFAULTS.contract_base_fee,
            contract_byte_fee: CONTRACT_DEFAULTS.contract_byte_fee,
            storage_byte_deposit: CONTRACT_DEFAULTS.storage_byte_deposit,
            max_code_size: CONTRACT_DEFAULTS.max_code_size,
            max_call_depth: CONTRACT_DEFAULTS.max_call_depth,
        }
    }

    /// Do keys `lo..=hi` all hold their defaults?
    fn range_is_default(&self, lo: u8, hi: u8) -> bool {
        let d = Self::defaults(self.admin);
        (lo..=hi).all(|k| self.value(k) == d.value(k))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::CONFIG).u8(1);
        // Tiers: the original 19 values, then the social keys, then the contract keys; a tier
        // is written only when it (or a later one) holds a non-default value, so the record
        // stays byte-identical to an older node's until an admin sets a newer key.
        let last = if !self.range_is_default(keys::SOCIAL_LAST + 1, keys::LAST) {
            keys::LAST
        } else if !self.range_is_default(keys::LEGACY_LAST + 1, keys::SOCIAL_LAST) {
            keys::SOCIAL_LAST
        } else {
            keys::LEGACY_LAST
        };
        for k in 0..=last {
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
        for k in 0..=keys::LEGACY_LAST {
            let v = r.bytes16(usize::MAX, "config value")?;
            c.set(k, &v).map_err(|_| DecodeError::Invalid("config value"))?;
        }
        for (lo, hi) in [(keys::LEGACY_LAST + 1, keys::SOCIAL_LAST), (keys::SOCIAL_LAST + 1, keys::LAST)] {
            if r.pos < r.b.len() {
                for k in lo..=hi {
                    let v = r.bytes16(usize::MAX, "config value")?;
                    c.set(k, &v).map_err(|_| DecodeError::Invalid("config value"))?;
                }
            }
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
            SOCIAL_MIN_FEE => self.social_min_fee.to_le_bytes().to_vec(),
            SOCIAL_BYTE_FEE => self.social_byte_fee.to_le_bytes().to_vec(),
            HOUSE_OPERATORS => {
                let mut v = vec![self.house_operators.len() as u8];
                for a in &self.house_operators {
                    v.extend_from_slice(a);
                }
                v
            }
            POST_INLINE_MAX => self.post_inline_max.to_le_bytes().to_vec(),
            MAX_BALANCE_ROWS => self.max_balance_rows.to_le_bytes().to_vec(),
            PRICE_BASE_LAMPORTS => self.price_base_lamports.map(|x| x.to_le_bytes().to_vec()).unwrap_or_default(),
            PRICE_MLAMPORTS_PER_GFLOP => self.price_mlamports_per_gflop.map(|x| x.to_le_bytes().to_vec()).unwrap_or_default(),
            PRICE_OVERHEAD_LAMPORTS => self.price_overhead_lamports.map(|x| x.to_le_bytes().to_vec()).unwrap_or_default(),
            GAS_PRICE => self.gas_price.to_le_bytes().to_vec(),
            BLOCK_GAS_LIMIT => self.block_gas_limit.to_le_bytes().to_vec(),
            TX_GAS_LIMIT => self.tx_gas_limit.to_le_bytes().to_vec(),
            CONTRACT_BASE_FEE => self.contract_base_fee.to_le_bytes().to_vec(),
            CONTRACT_BYTE_FEE => self.contract_byte_fee.to_le_bytes().to_vec(),
            STORAGE_BYTE_DEPOSIT => self.storage_byte_deposit.to_le_bytes().to_vec(),
            MAX_CODE_SIZE => self.max_code_size.to_le_bytes().to_vec(),
            MAX_CALL_DEPTH => vec![self.max_call_depth],
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
        fn opt_u64(v: &[u8]) -> Result<Option<u64>, String> {
            if v.is_empty() {
                return Ok(None);
            }
            u64v(v).map(Some)
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
            SOCIAL_MIN_FEE => self.social_min_fee = u64v(v)?,
            SOCIAL_BYTE_FEE => self.social_byte_fee = u64v(v)?,
            HOUSE_OPERATORS => {
                let n = *v.first().ok_or("empty house operator list (send count 0 to clear)")? as usize;
                if v.len() != 1 + 32 * n {
                    return Err("house operator list length".into());
                }
                self.house_operators = v[1..].chunks(32).map(|c| c.try_into().unwrap()).collect();
            }
            POST_INLINE_MAX => self.post_inline_max = u32v(v)?,
            MAX_BALANCE_ROWS => self.max_balance_rows = u32v(v)?,
            PRICE_BASE_LAMPORTS => self.price_base_lamports = opt_u64(v)?,
            PRICE_MLAMPORTS_PER_GFLOP => self.price_mlamports_per_gflop = opt_u64(v)?,
            PRICE_OVERHEAD_LAMPORTS => self.price_overhead_lamports = opt_u64(v)?,
            GAS_PRICE => self.gas_price = u64v(v)?,
            BLOCK_GAS_LIMIT => self.block_gas_limit = u64v(v)?,
            TX_GAS_LIMIT => self.tx_gas_limit = u64v(v)?,
            CONTRACT_BASE_FEE => self.contract_base_fee = u64v(v)?,
            CONTRACT_BYTE_FEE => self.contract_byte_fee = u64v(v)?,
            STORAGE_BYTE_DEPOSIT => self.storage_byte_deposit = u64v(v)?,
            MAX_CODE_SIZE => self.max_code_size = u32v(v)?,
            MAX_CALL_DEPTH => {
                if v.len() != 1 {
                    return Err("expected u8".into());
                }
                self.max_call_depth = v[0];
            }
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
        if self.house_operators.len() > MAX_HOUSE_OPERATORS {
            return Err(format!("at most {MAX_HOUSE_OPERATORS} house operators"));
        }
        let mut h = self.house_operators.clone();
        h.sort();
        h.dedup();
        if h.len() != self.house_operators.len() {
            return Err("duplicate house operator".into());
        }
        if !(1..=agent_types::social::MAX_POST_BODY as u32).contains(&self.post_inline_max) {
            return Err(format!("post_inline_max must be 1..={}", agent_types::social::MAX_POST_BODY));
        }
        if !(MIN_BALANCE_ROWS_LIMIT..=MAX_BALANCE_ROWS_LIMIT).contains(&self.max_balance_rows) {
            return Err(format!("max_balance_rows must be {MIN_BALANCE_ROWS_LIMIT}..={MAX_BALANCE_ROWS_LIMIT}"));
        }
        if self.tx_gas_limit > self.block_gas_limit {
            return Err("tx_gas_limit must not exceed block_gas_limit".into());
        }
        if self.max_code_size == 0 || self.max_code_size as usize > agent_types::contracts::MAX_CODE_SIZE {
            return Err(format!("max_code_size must be 1..={}", agent_types::contracts::MAX_CODE_SIZE));
        }
        if !(1..=MAX_CALL_DEPTH_LIMIT).contains(&self.max_call_depth) {
            return Err(format!("max_call_depth must be 1..={MAX_CALL_DEPTH_LIMIT}"));
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

    /// SPEC 4.6: the config record keeps its original 19-value encoding while the new keys
    /// hold their defaults (state roots match nodes from before the upgrade), and grows only
    /// when one is set; an old record decodes with the defaults for the new keys.
    #[test]
    fn extension_keys_encode_only_when_set() {
        let mut c = Config::defaults([7; 32]);
        c.attesters = vec![[1; 32]];
        let enc = c.encode();
        let mut legacy = W::new();
        legacy.u8(tag::CONFIG).u8(1);
        for k in 0..=keys::LEGACY_LAST {
            legacy.bytes16(&c.value(k));
        }
        assert_eq!(enc, legacy.into_inner());
        assert_eq!(Config::decode(&enc).unwrap(), c);
        let mut d = c.clone();
        d.set(keys::SOCIAL_BYTE_FEE, &5u64.to_le_bytes()).unwrap();
        d.set(keys::HOUSE_OPERATORS, &[&[1u8][..], &[9; 32]].concat()).unwrap();
        d.set(keys::PRICE_MLAMPORTS_PER_GFLOP, &10_410u64.to_le_bytes()).unwrap();
        d.validate().unwrap();
        assert!(d.encode().len() > enc.len());
        let back = Config::decode(&d.encode()).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.price_mlamports_per_gflop, Some(10_410));
        assert_eq!(back.price_base_lamports, None);
        // back to defaults: the short encoding again
        d.set(keys::SOCIAL_BYTE_FEE, &0u64.to_le_bytes()).unwrap();
        d.set(keys::HOUSE_OPERATORS, &[0u8]).unwrap();
        d.set(keys::PRICE_MLAMPORTS_PER_GFLOP, &[]).unwrap();
        assert_eq!(d.encode(), enc);
        // bounds
        let mut e = c.clone();
        e.set(keys::MAX_BALANCE_ROWS, &100u32.to_le_bytes()).unwrap();
        assert!(e.validate().is_err());
        e.set(keys::MAX_BALANCE_ROWS, &65_536u32.to_le_bytes()).unwrap();
        e.validate().unwrap();
        e.set(keys::POST_INLINE_MAX, &0u32.to_le_bytes()).unwrap();
        assert!(e.validate().is_err());
    }
}
