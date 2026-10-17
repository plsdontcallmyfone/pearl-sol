//! Compact targets, work and Pearl's WTEMA retarget (node/blockchain/difficulty.go).

use primitive_types::{U256, U512};

pub use pearl_job::job::nbits_to_target;

/// Bitcoin `BigToCompact`.
pub fn target_to_bits(t: U256) -> u32 {
    if t.is_zero() {
        return 0;
    }
    let mut size = t.bits().div_ceil(8) as u32;
    let mut compact: u32 = if size <= 3 {
        (t.low_u64() << (8 * (3 - size))) as u32
    } else {
        (t >> (8 * (size - 3) as usize)).low_u32()
    };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    compact | (size << 24)
}

/// Work of one block: `2^256 / (target + 1)` (Bitcoin `GetBlockProof`).
pub fn work(bits: u32) -> U256 {
    let t = nbits_to_target(bits);
    if t.is_zero() {
        return U256::zero();
    }
    if t == U256::MAX {
        return U256::one();
    }
    (!t / (t + 1)) + 1
}

/// Retarget inputs taken from the parent block.
#[derive(Debug, Clone, Copy)]
pub struct Retarget {
    pub parent_bits: u32,
    pub parent_ts_ms: u64,
    /// `None` when the parent is genesis.
    pub grandparent_ts_ms: Option<u64>,
    pub target_block_time_ms: u64,
    pub half_life_ms: u64,
    pub pow_limit_bits: u32,
    /// Hardest allowed target (compact), 0 = no bound.
    pub min_difficulty_bits: u32,
}

/// Required `target_bits` for the child of the parent described by `r`:
/// `new = old + (t - T) * old / half_life`, `t` = the parent's own solve time,
/// clamped to `[1, pow_limit]` (and to the hardest bound when set). The first block
/// after genesis uses the genesis target. Negative adjustments use floor division
/// (Go `big.Int.Div`, Euclidean).
pub fn next_bits(r: &Retarget) -> u32 {
    let Some(gp) = r.grandparent_ts_ms else {
        return r.parent_bits;
    };
    let old = nbits_to_target(r.parent_bits);
    let limit = nbits_to_target(r.pow_limit_bits);
    let t = r.parent_ts_ms as i128 - gp as i128;
    let d = t - r.target_block_time_ms as i128;
    let hl = U512::from(r.half_life_ms.max(1));
    let old5 = U512::from(old);
    let mag = U512::from(d.unsigned_abs()) * old5;
    let new5 = if d >= 0 {
        old5 + mag / hl
    } else {
        let q = (mag + hl - 1) / hl;
        if q >= old5 { U512::zero() } else { old5 - q }
    };
    let mut new = if new5 > U512::from(limit) { limit } else { U256::try_from(new5).unwrap_or(limit) };
    if new.is_zero() {
        new = U256::one();
    }
    if r.min_difficulty_bits != 0 {
        let hardest = nbits_to_target(r.min_difficulty_bits);
        if new < hardest {
            new = hardest;
        }
    }
    target_to_bits(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_roundtrip() {
        for bits in [0x1d00ffffu32, 0x207fffff, 0x1e040000, 0x1d020000, 0x03123456] {
            assert_eq!(target_to_bits(nbits_to_target(bits)), bits, "{bits:x}");
        }
    }

    #[test]
    fn work_monotone() {
        assert!(work(0x1d00ffff) > work(0x1e040000));
        assert_eq!(work(0x1d00ffff), U256::from(0x100010001u64));
    }

    fn r(t: u64) -> Retarget {
        Retarget {
            parent_bits: 0x1e040000,
            parent_ts_ms: 1_000_000 + t,
            grandparent_ts_ms: Some(1_000_000),
            target_block_time_ms: 60_000,
            half_life_ms: 600_000,
            pow_limit_bits: 0x207fffff,
            min_difficulty_bits: 0,
        }
    }

    #[test]
    fn wtema_direction() {
        let base = nbits_to_target(0x1e040000);
        assert_eq!(next_bits(&r(60_000)), 0x1e040000);
        assert!(nbits_to_target(next_bits(&r(120_000))) > base, "slow block eases");
        assert!(nbits_to_target(next_bits(&r(1_000))) < base, "fast block hardens");
        // +60 s over a 600 s half life = +10%
        let up = nbits_to_target(next_bits(&r(120_000)));
        assert_eq!(up, nbits_to_target(target_to_bits(base + base / 10)));
        // clamped at the limit
        let mut x = r(1_000_000_000);
        x.pow_limit_bits = 0x1e080000;
        assert_eq!(next_bits(&x), 0x1e080000);
        // genesis child keeps the genesis target
        x.grandparent_ts_ms = None;
        assert_eq!(next_bits(&x), 0x1e040000);
    }
}
