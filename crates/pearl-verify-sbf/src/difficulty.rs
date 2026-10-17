//! Compact targets, block work and the WTEMA retarget, ported exactly from
//! chain/consensus-pow/src/difficulty.rs and crates/pearl-job/src/job.rs.

use primitive_types::U256;

/// Bitcoin compact `nbits` to a 256-bit target (`pearl_job::job::nbits_to_target`).
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
        let shift = 8 * (exp - 3);
        if t.bits() + shift > 256 {
            U256::MAX
        } else {
            t << shift
        }
    }
}

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

/// Work of one block: `2^256 / (target + 1)`.
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

#[derive(Debug, Clone, Copy)]
pub struct Retarget {
    pub parent_bits: u32,
    pub parent_ts_ms: u64,
    /// `None` when the parent is genesis.
    pub grandparent_ts_ms: Option<u64>,
    pub target_block_time_ms: u64,
    pub half_life_ms: u64,
    pub pow_limit_bits: u32,
    pub min_difficulty_bits: u32,
}

/// Required `target_bits` for the child of the parent described by `r`.
pub fn next_bits(r: &Retarget) -> u32 {
    let Some(gp) = r.grandparent_ts_ms else {
        return r.parent_bits;
    };
    let old = nbits_to_target(r.parent_bits);
    let limit = nbits_to_target(r.pow_limit_bits);
    let t = r.parent_ts_ms as i128 - gp as i128;
    let d = t - r.target_block_time_ms as i128;
    // The chain computes old + d*old/hl in 512 bits; here the same value exactly:
    // old*|d|/hl = q*|d| + (r*|d|)/hl with q = old/hl, r = old%hl (r*|d| < 2^128),
    // and any 256-bit overflow lands on the same clamp as the 512-bit result.
    let hl = U256::from(r.half_life_ms.max(1));
    let ad = U256::from(d.unsigned_abs());
    let (q, rem) = old.div_mod(hl);
    let rd = rem * ad; // rem < 2^64, |d| < 2^128: fits
    let mut new = if d >= 0 {
        match q.checked_mul(ad).and_then(|x| x.checked_add(rd / hl)).and_then(|x| x.checked_add(old)) {
            Some(v) if v <= limit => v,
            _ => limit,
        }
    } else {
        let ceil = (rd + hl - 1) / hl;
        match q.checked_mul(ad).and_then(|x| x.checked_add(ceil)) {
            Some(qq) if qq < old => {
                let v = old - qq;
                if v > limit { limit } else { v }
            }
            _ => U256::zero(),
        }
    };
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

/// `pearl_job::job::difficulty_bound`: `target * h*w*dlen`, saturating.
pub fn difficulty_bound(nbits: u32, hw_dlen: u64) -> U256 {
    let t = nbits_to_target(nbits);
    let f = U256::from(hw_dlen);
    if t > U256::MAX / f {
        U256::MAX
    } else {
        t * f
    }
}

pub fn meets_target(hash: &[u8; 32], bound: &U256) -> bool {
    U256::from_little_endian(hash) <= *bound
}

pub fn u256_le(x: &U256) -> [u8; 32] {
    let mut o = [0u8; 32];
    x.to_little_endian(&mut o);
    o
}

pub fn u256_from_le(b: &[u8; 32]) -> U256 {
    U256::from_little_endian(b)
}
