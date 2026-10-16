//! Periodic index patterns (rows of A / columns of B that make one hashed tile).
//!
//! Ported from Pearl `zk-pow/src/api/proof_utils.rs` (`impl PeriodicPattern`), ISC.

use anyhow::{bail, ensure, Result};

/// Generalised arithmetic progression of up to 3 dimensions:
/// `a*stride[0] + b*stride[1] + c*stride[2]` for `a < length[0]`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicPattern {
    pub shape: [(u32, u32); 3],
}

impl PeriodicPattern {
    pub const NUM_DIMS: usize = 3;

    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        ensure!(data.len() == 2 * Self::NUM_DIMS, "pattern: expected 6 bytes, got {}", data.len());
        let mut shape = [(0u32, 0u32); Self::NUM_DIMS];
        let mut min_stride = 1u32;
        let mut is_done = false;
        for (i, chunk) in data.chunks(2).enumerate() {
            let factor = 1 + chunk[0] as u32;
            let length = 1 + chunk[1] as u32;
            if length == 1 || is_done {
                ensure!(factor == 1 && length == 1, "pattern: non-canonical representation");
                is_done = true;
            } else if factor <= 1 && min_stride != 1 {
                bail!("pattern: a single stride must not be broken");
            }
            ensure!(min_stride <= (1 << 24) / (factor * length), "pattern: period must be <= 2^24");
            let stride = factor * min_stride;
            shape[i] = (stride, length);
            min_stride = stride * length;
        }
        let result = Self { shape };
        ensure!(result.to_bytes()[..] == data[..], "pattern: round-trip mismatch");
        Ok(result)
    }

    pub fn to_bytes(&self) -> [u8; 6] {
        let mut data = [0u8; 6];
        let mut min_stride = 1u32;
        for (i, &(stride, length)) in self.shape.iter().enumerate() {
            let factor = stride / min_stride;
            data[2 * i] = (factor - 1) as u8;
            data[2 * i + 1] = (length - 1) as u8;
            min_stride = stride * length;
        }
        data
    }

    pub fn to_list(&self) -> Vec<u32> {
        let mut res = vec![0u32];
        for &(stride, length) in &self.shape {
            let mut next = Vec::with_capacity(res.len() * length as usize);
            for i in 0..length {
                for &r in &res {
                    next.push(r + i * stride);
                }
            }
            res = next;
        }
        res
    }

    pub fn max(&self) -> u32 {
        self.to_list().into_iter().max().unwrap_or(0)
    }

    pub fn from_list(pattern: &[u32]) -> Result<Self> {
        ensure!(!pattern.is_empty(), "pattern cannot be empty");
        ensure!(pattern.windows(2).all(|w| w[0] < w[1]), "pattern must be sorted, no duplicates");
        ensure!(pattern[0] == 0, "pattern must start at 0");
        let mut p = pattern.to_vec();
        let mut shape_vec = Vec::new();
        while p.len() > 1 {
            let mut found = false;
            for period in 1..p.len() {
                if p.len() % period == 0 {
                    let s = p[period];
                    if (0..p.len() - period).all(|i| p[i] + s == p[i + period]) {
                        shape_vec.push((s, (p.len() / period) as u32));
                        p.truncate(period);
                        found = true;
                        break;
                    }
                }
            }
            ensure!(found, "pattern is not periodic");
        }
        shape_vec.reverse();
        let period = shape_vec.last().map_or(1, |&(s, l)| s * l);
        while shape_vec.len() < Self::NUM_DIMS {
            shape_vec.push((period, 1));
        }
        ensure!(shape_vec.len() == Self::NUM_DIMS, "pattern has more than 3 dimensions");
        let result = Self { shape: [shape_vec[0], shape_vec[1], shape_vec[2]] };
        ensure!(result.is_valid(), "constructed pattern is not valid");
        Ok(result)
    }

    /// True when `offset` is a legal tile start for this pattern.
    pub fn offset_is_valid(&self, mut offset: u32) -> bool {
        for &(stride, length) in self.shape.iter().rev() {
            offset %= stride * length;
            if offset >= stride {
                return false;
            }
        }
        true
    }

    pub fn is_valid(&self) -> bool {
        matches!(Self::from_bytes(&self.to_bytes()), Ok(r) if r.shape == self.shape)
    }

    pub fn period(&self) -> u32 {
        let &(stride, length) = self.shape.last().unwrap();
        stride * length
    }

    pub fn size(&self) -> u32 {
        self.shape.iter().map(|&(_, l)| l).product()
    }

    pub fn indices_with_offset(&self, offset: u32) -> Vec<u32> {
        self.to_list().into_iter().map(|i| i + offset).collect()
    }

    /// All index groups (tiles) of this pattern over a dimension of `total` entries.
    pub fn partition(&self, total: usize) -> Result<Vec<Vec<usize>>> {
        let period = self.period() as usize;
        ensure!(total % period == 0, "dimension {total} must be a multiple of pattern period {period}");
        let base: Vec<usize> = self.to_list().iter().map(|&i| i as usize).collect();
        Ok((0..total)
            .filter(|&i| self.offset_is_valid(i as u32))
            .map(|off| base.iter().map(|&d| off + d).collect())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_roundtrip() {
        let p = PeriodicPattern::from_list(&[0, 1, 2, 3]).unwrap();
        assert_eq!(p.size(), 4);
        assert_eq!(p.period(), 4);
        assert_eq!(PeriodicPattern::from_bytes(&p.to_bytes()).unwrap(), p);
        assert!(p.offset_is_valid(8));
        assert!(!p.offset_is_valid(2));
        let q = PeriodicPattern::from_list(&[0, 8, 64, 72]).unwrap();
        assert_eq!(q.to_list(), vec![0, 8, 64, 72]);
        assert_eq!(q.partition(128).unwrap().len(), 32);
    }
}
