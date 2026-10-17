//! Fixed 256-byte block header (SPEC 3.1).
//!
//! ```text
//! off  len  field
//!   0    4  version u32
//!   4    8  height u64
//!  12   32  prev_hash
//!  44    8  timestamp_ms u64
//!  52    4  target_bits u32 (compact)
//!  56   32  job_id (zero = fallback block)
//!  88   32  miner (payout key)
//! 120   32  tx_root
//! 152   32  state_root
//! 184   32  withdraw_root
//! 216   32  proof_hash
//! 248    8  nonce u64
//! ```

use crate::codec::{R, W};
use crate::{blake3_hash, DecodeError, Hash, Pubkey, HEADER_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Header {
    pub version: u32,
    pub height: u64,
    pub prev_hash: Hash,
    pub timestamp_ms: u64,
    pub target_bits: u32,
    pub job_id: Hash,
    pub miner: Pubkey,
    pub tx_root: Hash,
    pub state_root: Hash,
    pub withdraw_root: Hash,
    pub proof_hash: Hash,
    pub nonce: u64,
}

impl Header {
    pub const SIZE: usize = 256;
    /// Byte offset of `proof_hash` (zeroed for `pow_digest`).
    pub const PROOF_HASH_OFFSET: usize = 216;
    pub const NONCE_OFFSET: usize = 248;

    pub fn encode(&self) -> [u8; 256] {
        let mut w = W::new();
        w.u32(self.version)
            .u64(self.height)
            .bytes(&self.prev_hash)
            .u64(self.timestamp_ms)
            .u32(self.target_bits)
            .bytes(&self.job_id)
            .bytes(&self.miner)
            .bytes(&self.tx_root)
            .bytes(&self.state_root)
            .bytes(&self.withdraw_root)
            .bytes(&self.proof_hash)
            .u64(self.nonce);
        w.into_inner().try_into().expect("header is 256 bytes")
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() != Self::SIZE {
            return Err(if b.len() < Self::SIZE {
                DecodeError::Truncated(b.len())
            } else {
                DecodeError::Trailing(b.len() - Self::SIZE)
            });
        }
        let mut r = R::new(b);
        let h = Self {
            version: r.u32()?,
            height: r.u64()?,
            prev_hash: r.arr()?,
            timestamp_ms: r.u64()?,
            target_bits: r.u32()?,
            job_id: r.arr()?,
            miner: r.arr()?,
            tx_root: r.arr()?,
            state_root: r.arr()?,
            withdraw_root: r.arr()?,
            proof_hash: r.arr()?,
            nonce: r.u64()?,
        };
        r.finish()?;
        if h.version != HEADER_VERSION {
            return Err(DecodeError::Version(h.version));
        }
        Ok(h)
    }

    /// `block_hash = blake3(header)`.
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }

    /// `pow_digest = blake3(header with proof_hash zeroed)`: what the proof binds to.
    pub fn pow_digest(&self) -> Hash {
        let mut b = self.encode();
        b[Self::PROOF_HASH_OFFSET..Self::PROOF_HASH_OFFSET + 32].fill(0);
        blake3_hash(&[&b])
    }

    pub fn is_fallback(&self) -> bool {
        self.job_id == [0u8; 32]
    }
}

impl commonware_codec::Write for Header {
    fn write(&self, buf: &mut impl bytes::BufMut) {
        buf.put_slice(&self.encode());
    }
}

impl commonware_codec::FixedSize for Header {
    const SIZE: usize = Header::SIZE;
}

impl commonware_codec::Read for Header {
    type Cfg = ();
    fn read_cfg(buf: &mut impl bytes::Buf, _: &()) -> Result<Self, commonware_codec::Error> {
        if buf.remaining() < Header::SIZE {
            return Err(commonware_codec::Error::EndOfBuffer);
        }
        let mut b = [0u8; 256];
        buf.copy_to_slice(&mut b);
        Header::decode(&b).map_err(|_| commonware_codec::Error::Invalid("Header", "decode"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_roundtrip() {
        let h = Header {
            version: 1,
            height: 7,
            prev_hash: [1; 32],
            timestamp_ms: 1_700_000_000_000,
            target_bits: 0x207fffff,
            job_id: [2; 32],
            miner: [3; 32],
            tx_root: [4; 32],
            state_root: [5; 32],
            withdraw_root: [6; 32],
            proof_hash: [7; 32],
            nonce: 99,
        };
        let b = h.encode();
        assert_eq!(&b[216..248], &[7u8; 32]);
        assert_eq!(u64::from_le_bytes(b[248..256].try_into().unwrap()), 99);
        assert_eq!(&b[56..88], &[2u8; 32]);
        assert_eq!(Header::decode(&b).unwrap(), h);
        let mut z = h;
        z.proof_hash = [0; 32];
        assert_eq!(h.pow_digest(), z.hash());
        assert_ne!(h.hash(), h.pow_digest());
        assert!(Header::decode(&b[..255]).is_err());
    }
}
