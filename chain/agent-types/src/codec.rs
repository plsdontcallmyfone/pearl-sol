//! Minimal fixed little-endian reader / writer used by every encoding in this crate.

use crate::DecodeError;

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    /// u8 length prefix.
    pub fn bytes8(&mut self, v: &[u8]) -> &mut Self {
        assert!(v.len() <= u8::MAX as usize);
        self.u8(v.len() as u8).bytes(v)
    }
    /// u16 length prefix.
    pub fn bytes16(&mut self, v: &[u8]) -> &mut Self {
        assert!(v.len() <= u16::MAX as usize);
        self.u16(v.len() as u16).bytes(v)
    }
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

pub struct R<'a> {
    pub b: &'a [u8],
    pub pos: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.b.len() - self.pos < n {
            return Err(DecodeError::Truncated(self.pos));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn arr<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    pub fn bytes8(&mut self, max: usize, what: &'static str) -> Result<Vec<u8>, DecodeError> {
        let n = self.u8()? as usize;
        if n > max {
            return Err(DecodeError::TooLarge(what));
        }
        Ok(self.take(n)?.to_vec())
    }
    pub fn bytes16(&mut self, max: usize, what: &'static str) -> Result<Vec<u8>, DecodeError> {
        let n = self.u16()? as usize;
        if n > max {
            return Err(DecodeError::TooLarge(what));
        }
        Ok(self.take(n)?.to_vec())
    }
    pub fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }
    pub fn finish(&self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(DecodeError::Trailing(n)),
        }
    }
}
