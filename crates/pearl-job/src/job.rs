//! Job header, mining configuration, commitments, seed derivation and difficulty.
//!
//! Config layout, seed chain, salting and the difficulty bound are ported from Pearl
//! `zk-pow/src/api/{proof,proof_utils,seed,sanity_checks}.rs`, ISC. The 76-byte job
//! header and the miner-bound seed are pearl-sol changes (SPEC 3.3, 3.4).

use anyhow::{ensure, Result};
use primitive_types::U256;

use crate::merkle::{blake3_digest, pad_to_chunk_boundary, Digest};
use crate::pattern::PeriodicPattern;

pub type Hash256 = [u8; 32];

/// SPEC 3.3: `job_pubkey(32) || terms_hash(32) || created_slot u64 LE || target_bits u32 LE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobHeader {
    pub job_pubkey: [u8; 32],
    pub terms_hash: [u8; 32],
    pub created_slot: u64,
    /// Bitcoin-compact target; this is the job's share target.
    pub target_bits: u32,
}

impl JobHeader {
    pub const SIZE: usize = 76;

    pub fn to_bytes(&self) -> [u8; 76] {
        let mut b = [0u8; 76];
        b[0..32].copy_from_slice(&self.job_pubkey);
        b[32..64].copy_from_slice(&self.terms_hash);
        b[64..72].copy_from_slice(&self.created_slot.to_le_bytes());
        b[72..76].copy_from_slice(&self.target_bits.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        ensure!(b.len() == 76, "header must be 76 bytes, got {}", b.len());
        Ok(Self {
            job_pubkey: b[0..32].try_into().unwrap(),
            terms_hash: b[32..64].try_into().unwrap(),
            created_slot: u64::from_le_bytes(b[64..72].try_into().unwrap()),
            target_bits: u32::from_le_bytes(b[72..76].try_into().unwrap()),
        })
    }
}

/// Pearl `MiningConfiguration`, dense only (MoE trailer must be zero). 52 bytes:
/// `k u32 | rank u16 | mma_type u16 (=0) | rows_pattern 6 | cols_pattern 6 | 32 zero bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiningConfig {
    pub common_dim: u32,
    pub rank: u16,
    pub rows_pattern: PeriodicPattern,
    pub cols_pattern: PeriodicPattern,
}

impl MiningConfig {
    pub const SIZE: usize = 52;

    /// Localnet default: k=1024, r=32, 4x8 contiguous tile (h*w=32, the smallest
    /// Pearl allows). Smallest plain proof, best fit for the M2 SBF verifier.
    pub fn localnet_default() -> Self {
        Self {
            common_dim: 1024,
            rank: 32,
            rows_pattern: PeriodicPattern::from_list(&[0, 1, 2, 3]).unwrap(),
            cols_pattern: PeriodicPattern::from_list(&[0, 1, 2, 3, 4, 5, 6, 7]).unwrap(),
        }
    }

    pub fn to_bytes(&self) -> [u8; 52] {
        let mut b = [0u8; 52];
        b[0..4].copy_from_slice(&self.common_dim.to_le_bytes());
        b[4..6].copy_from_slice(&self.rank.to_le_bytes());
        // b[6..8] = mma_type 0 (Int7xInt7ToInt32)
        b[8..14].copy_from_slice(&self.rows_pattern.to_bytes());
        b[14..20].copy_from_slice(&self.cols_pattern.to_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        ensure!(b.len() == 52, "config must be 52 bytes, got {}", b.len());
        let mma = u16::from_le_bytes(b[6..8].try_into().unwrap());
        ensure!(mma == 0, "unsupported mma_type {mma}");
        ensure!(b[20..52].iter().all(|&x| x == 0), "MoE / reserved trailer must be zero");
        let c = Self {
            common_dim: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            rank: u16::from_le_bytes(b[4..6].try_into().unwrap()),
            rows_pattern: PeriodicPattern::from_bytes(&b[8..14])?,
            cols_pattern: PeriodicPattern::from_bytes(&b[14..20])?,
        };
        ensure!(c.to_bytes()[..] == b[..], "config round-trip mismatch");
        Ok(c)
    }

    pub fn k(&self) -> usize {
        self.common_dim as usize
    }
    pub fn r(&self) -> usize {
        self.rank as usize
    }
    pub fn h(&self) -> usize {
        self.rows_pattern.size() as usize
    }
    pub fn w(&self) -> usize {
        self.cols_pattern.size() as usize
    }
    pub fn dot_product_length(&self) -> usize {
        self.k() - self.k() % self.r()
    }
}

/// Everything a verifier needs about a job, all of it from the job account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub header: JobHeader,
    pub config: MiningConfig,
    pub m: u32,
    pub n: u32,
    pub hash_a: Hash256,
    pub hash_b: Hash256,
}

impl Job {
    pub fn job_key(&self) -> Hash256 {
        job_key(&self.header, &self.config)
    }

    /// Pearl `public_params_sanity_check` (dense subset) plus divisibility of m, n
    /// by the pattern periods, which the miner's tile partition needs.
    pub fn sanity_check(&self) -> Result<()> {
        check_shape(&self.config, self.m as usize, self.n as usize)
    }
}

pub fn check_shape(c: &MiningConfig, m: usize, n: usize) -> Result<()> {
    let (k, r, h, w) = (c.k(), c.r(), c.h(), c.w());
    ensure!(r.is_power_of_two() && (32..=1024).contains(&r), "rank must be 2^5..2^10, got {r}");
    ensure!(r % 16 == 0, "r must be divisible by 16");
    ensure!(k <= 1 << 16, "k must be <= 2^16, got {k}");
    ensure!(k % 64 == 0, "k must be a multiple of 64, got {k}");
    ensure!(k <= 4 * r * r, "k must be <= 4r^2");
    ensure!(k >= 16 * r, "k must be >= 16r");
    ensure!(k >= 1024, "k must be >= 1024, got {k}");
    ensure!(h % 2 == 0 && w % 2 == 0, "tile h, w must be even (h={h} w={w})");
    ensure!(h * w >= 32 && h * w <= 256, "h*w must be in [32, 256], got {}", h * w);
    ensure!(c.dot_product_length() % 64 == 0, "dot product length must be a multiple of 64");
    ensure!(m <= 1 << 24 && n <= 1 << 24, "m, n must be <= 2^24");
    ensure!((h + w) * c.dot_product_length() <= 1 << 22, "worker input over 4 MiB");
    ensure!(m > 0 && m % c.rows_pattern.period() as usize == 0, "m={m} must be a multiple of the row pattern period");
    ensure!(n > 0 && n % c.cols_pattern.period() as usize == 0, "n={n} must be a multiple of the col pattern period");
    Ok(())
}

pub fn job_key(header: &JobHeader, config: &MiningConfig) -> Hash256 {
    let mut d = [0u8; 128];
    d[..76].copy_from_slice(&header.to_bytes());
    d[76..].copy_from_slice(&config.to_bytes());
    blake3_digest(&d, None)
}

/// `hash_a = blake3_keyed(pad1024(A row-major), job_key)`.
pub fn commit_a(job_key: &Hash256, a_row_major: &[i8]) -> Hash256 {
    blake3_digest(&pad_to_chunk_boundary(as_u8(a_row_major)), Some(job_key))
}

/// `hash_b = blake3_keyed(pad1024(B^T row-major), job_key)`; takes B (k x n) row-major.
pub fn commit_b(job_key: &Hash256, b_row_major: &[i8], k: usize, n: usize) -> Hash256 {
    let bt = transpose(b_row_major, k, n);
    blake3_digest(&pad_to_chunk_boundary(as_u8(&bt)), Some(job_key))
}

pub fn as_u8(v: &[i8]) -> &[u8] {
    // SAFETY: i8 and u8 have identical size and alignment.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len()) }
}

/// Transpose a row-major `rows x cols` matrix.
pub fn transpose<T: Copy + Default>(x: &[T], rows: usize, cols: usize) -> Vec<T> {
    let mut out = vec![T::default(); rows * cols];
    for i in 0..rows {
        for j in 0..cols {
            out[j * rows + i] = x[i * cols + j];
        }
    }
    out
}

/// `blake3("pearl/cert-v3/noise-seed/A")` (Pearl `seed.rs`).
pub const SEED_SALT_A: Hash256 = [
    0x82, 0x49, 0x40, 0x6c, 0xa0, 0xed, 0x15, 0x16, 0x96, 0x16, 0xf6, 0x92, 0xfc, 0xf0, 0x76, 0xf8, 0x92, 0xdb, 0xdb,
    0x2a, 0x70, 0x23, 0xb8, 0x52, 0xf0, 0xd4, 0x77, 0x19, 0xc3, 0x90, 0x01, 0x7b,
];
/// `blake3("pearl/cert-v3/noise-seed/B")`.
pub const SEED_SALT_B: Hash256 = [
    0x11, 0x30, 0x06, 0x32, 0xec, 0x63, 0x01, 0xca, 0x2b, 0xe2, 0xaf, 0x71, 0x8b, 0x3f, 0x4d, 0x4f, 0x1a, 0xe9, 0xc6,
    0x39, 0x88, 0xe8, 0xcc, 0x04, 0x48, 0x44, 0x30, 0x1d, 0x71, 0xb8, 0x9a, 0xa9,
];

fn bind_root(root: &Hash256, dim: u32, salt: &Hash256) -> Hash256 {
    let mut msg = [0u8; 64];
    msg[..32].copy_from_slice(root);
    msg[32..36].copy_from_slice(&dim.to_le_bytes());
    blake3_digest(&msg, Some(salt))
}

/// How the noise seeds are derived from the commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedDerivation {
    /// Pearl pre-V3: `b = blake3(job_key || hash_b)`, `a = blake3(b || hash_a)`.
    Legacy,
    /// Pearl V3: roots salted with m / n first, then the Legacy chain.
    Salted,
    /// pearl-sol (SPEC 3.4, NOTES-proof Q4): roots salted as in V3 (`hash_a'`,
    /// `hash_b'`), then `b = blake3(job_key || miner || nonce_le || hash_b')`,
    /// `a = blake3(b || hash_a')`.
    MinerBound { miner: [u8; 32], nonce: u64 },
}

/// Returns `(b_noise_seed, a_noise_seed)`; `a_noise_seed` also keys the jackpot hash.
pub fn derive_seeds(
    mode: SeedDerivation,
    job_key: &Hash256,
    hash_a: &Hash256,
    hash_b: &Hash256,
    m: u32,
    n: u32,
) -> (Hash256, Hash256) {
    let (ha, hb) = match mode {
        SeedDerivation::Legacy => (*hash_a, *hash_b),
        _ => (bind_root(hash_a, m, &SEED_SALT_A), bind_root(hash_b, n, &SEED_SALT_B)),
    };
    let b_seed = match mode {
        SeedDerivation::MinerBound { miner, nonce } => {
            let mut d = [0u8; 32 + 32 + 8 + 32];
            d[0..32].copy_from_slice(job_key);
            d[32..64].copy_from_slice(&miner);
            d[64..72].copy_from_slice(&nonce.to_le_bytes());
            d[72..104].copy_from_slice(&hb);
            blake3_digest(&d, None)
        }
        _ => blake3_digest(&[&job_key[..], &hb[..]].concat(), None),
    };
    let a_seed = blake3_digest(&[&b_seed[..], &ha[..]].concat(), None);
    (b_seed, a_seed)
}

/// Bitcoin compact `nbits` to a 256-bit target (Pearl `nbits_to_difficulty`).
pub fn nbits_to_target(nbits: u32) -> U256 {
    let exp = (nbits >> 24) as usize;
    let mant = nbits & 0x00ff_ffff;
    if mant == 0 || exp == 0 || mant & 0x0080_0000 != 0 {
        return U256::zero();
    }
    let t = U256::from(mant);
    if exp <= 3 {
        t >> (8 * (3 - exp))
    } else if 8 * (exp - 3) >= 256 {
        U256::MAX
    } else {
        // Saturate instead of silently dropping high bits.
        let shift = 8 * (exp - 3);
        if t.bits() + shift > 256 {
            U256::MAX
        } else {
            t << shift
        }
    }
}

/// Pearl `extract_difficulty_bound`: `target * h*w*dot_product_length`, saturating.
/// No rank penalty (that is a Pearl-mainnet consensus rule; our r can be 32).
pub fn difficulty_bound(nbits: u32, c: &MiningConfig) -> U256 {
    let t = nbits_to_target(nbits);
    let f = U256::from(c.h() * c.w() * c.dot_product_length());
    if t > U256::MAX / f {
        U256::MAX
    } else {
        t * f
    }
}

pub fn meets_target(hash: &Digest, bound: &U256) -> bool {
    U256::from_little_endian(hash) <= *bound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salts_match_context_strings() {
        assert_eq!(SEED_SALT_A, blake3_digest(b"pearl/cert-v3/noise-seed/A", None));
        assert_eq!(SEED_SALT_B, blake3_digest(b"pearl/cert-v3/noise-seed/B", None));
    }

    /// Pearl's pinned vectors (`seed.rs::commitment_hash_pinned_vectors`); m=192, n=320.
    #[test]
    fn pearl_pinned_seed_vectors() {
        let jk = [0x11u8; 32];
        let (b, a) = derive_seeds(SeedDerivation::Legacy, &jk, &[0xAA; 32], &[0xBB; 32], 192, 320);
        assert_eq!(hex::encode(b), "add6f7ea5feebf89c8a77e2ebfa0d82442e7dbb0046dbd48971861d12fcb0177");
        assert_eq!(hex::encode(a), "483b07b6f73105030b9482255f37723f3fed69ae916724ee8291848b8c28794b");
        let (b, a) = derive_seeds(SeedDerivation::Salted, &jk, &[0xAA; 32], &[0xBB; 32], 192, 320);
        assert_eq!(hex::encode(b), "60ed9b73c5a9599b200b6cd563e7f0d5d9a67d2402d85fd4ef966c580080d0e5");
        assert_eq!(hex::encode(a), "301784168005ec833ab0aa60006f7fe7faaa95307d8c1fc6819b2ffdd717eccf");
    }

    #[test]
    fn nbits() {
        assert_eq!(
            nbits_to_target(0x1d00ffff),
            U256::from_str_radix("00000000ffff0000000000000000000000000000000000000000000000000000", 16).unwrap()
        );
        assert_eq!(nbits_to_target(0x03123456), U256::from(0x123456));
        assert_eq!(nbits_to_target(0x1d800000), U256::zero());
    }

    #[test]
    fn config_roundtrip() {
        let c = MiningConfig::localnet_default();
        assert_eq!(MiningConfig::from_bytes(&c.to_bytes()).unwrap(), c);
        check_shape(&c, 64, 64).unwrap();
    }
}
