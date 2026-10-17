//! Mining configuration (52 bytes) and periodic tile patterns, ported from
//! crates/pearl-job (`job::MiningConfig`, `pattern::PeriodicPattern`, `job::check_shape`).

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    /// (stride, length) per dimension.
    pub shape: [(u32, u32); 3],
}

impl Pattern {
    pub fn from_bytes(d: &[u8]) -> Result<Self, Error> {
        if d.len() != 6 {
            return Err(Error::Config);
        }
        let mut shape = [(0u32, 0u32); 3];
        let mut min_stride = 1u32;
        let mut done = false;
        for i in 0..3 {
            let factor = 1 + d[2 * i] as u32;
            let length = 1 + d[2 * i + 1] as u32;
            if length == 1 || done {
                if !(factor == 1 && length == 1) {
                    return Err(Error::Config);
                }
                done = true;
            } else if factor <= 1 && min_stride != 1 {
                return Err(Error::Config);
            }
            if min_stride > (1 << 24) / (factor * length) {
                return Err(Error::Config);
            }
            let stride = factor * min_stride;
            shape[i] = (stride, length);
            min_stride = stride * length;
        }
        Ok(Self { shape })
    }

    pub fn size(&self) -> u32 {
        self.shape[0].1 * self.shape[1].1 * self.shape[2].1
    }

    pub fn period(&self) -> u32 {
        let (s, l) = self.shape[2];
        s * l
    }

    pub fn max(&self) -> u32 {
        self.shape.iter().map(|&(s, l)| s * (l - 1)).sum()
    }

    pub fn offset_is_valid(&self, mut offset: u32) -> bool {
        for &(stride, length) in self.shape.iter().rev() {
            offset %= stride * length;
            if offset >= stride {
                return false;
            }
        }
        true
    }

    /// Index `i` of the pattern (dimension 0 fastest), in increasing order.
    pub fn index(&self, i: u32) -> u32 {
        let (s0, l0) = self.shape[0];
        let (s1, l1) = self.shape[1];
        let (s2, _) = self.shape[2];
        let a = i % l0;
        let b = (i / l0) % l1;
        let c = i / (l0 * l1);
        a * s0 + b * s1 + c * s2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiningConfig {
    pub k: u32,
    pub rank: u32,
    pub rows: Pattern,
    pub cols: Pattern,
}

impl MiningConfig {
    pub const SIZE: usize = 52;

    pub fn from_bytes(b: &[u8]) -> Result<Self, Error> {
        if b.len() != 52 {
            return Err(Error::Config);
        }
        let mma = u16::from_le_bytes([b[6], b[7]]);
        if mma != 0 || b[20..52].iter().any(|&x| x != 0) {
            return Err(Error::Config);
        }
        Ok(Self {
            k: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            rank: u16::from_le_bytes([b[4], b[5]]) as u32,
            rows: Pattern::from_bytes(&b[8..14])?,
            cols: Pattern::from_bytes(&b[14..20])?,
        })
    }

    pub fn h(&self) -> u32 {
        self.rows.size()
    }
    pub fn w(&self) -> u32 {
        self.cols.size()
    }
    pub fn dlen(&self) -> u32 {
        self.k - self.k % self.rank
    }

    /// `job::check_shape`.
    pub fn check_shape(&self, m: u32, n: u32) -> Result<(), Error> {
        let (k, r, h, w) = (self.k as u64, self.rank as u64, self.h() as u64, self.w() as u64);
        let dlen = self.dlen() as u64;
        let ok = r.is_power_of_two()
            && (32..=1024).contains(&r)
            && r % 16 == 0
            && k <= 1 << 16
            && k % 64 == 0
            && k <= 4 * r * r
            && k >= 16 * r
            && k >= 1024
            && h % 2 == 0
            && w % 2 == 0
            && h * w >= 32
            && h * w <= 256
            && dlen % 64 == 0
            && m <= 1 << 24
            && n <= 1 << 24
            && (h + w) * dlen <= 1 << 22
            && m > 0
            && m % self.rows.period() == 0
            && n > 0
            && n % self.cols.period() == 0;
        if ok {
            Ok(())
        } else {
            Err(Error::Shape)
        }
    }
}
