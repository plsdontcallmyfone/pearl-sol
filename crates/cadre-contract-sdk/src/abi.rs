//! Call encoding (docs/CONTRACTS.md "ABI"): little-endian fields after a method name.
//!
//! ```text
//! input  = method (u8 len, UTF-8) | field*
//! field  = u8 | u16 | u32 | u64 (LE) | bool (u8 0/1) | addr [32] | hash [32]
//!        | bytes (u32 len) | str (u32 len, UTF-8)
//! ```
//! The CLI (`pearl contract call ADDR METHOD u64:5 addr:<base58> str:hello`), the wallet and
//! cadre-api use the same layout; return data is fields with no method.

use alloc::vec::Vec;

pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

const SHORT: &str = "input too short";

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if self.b.len() - self.pos < n {
            return Err(SHORT);
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn method(&mut self) -> Result<&'a str, &'static str> {
        let n = self.u8()? as usize;
        core::str::from_utf8(self.take(n)?).map_err(|_| "method is not UTF-8")
    }
    pub fn u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, &'static str> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, &'static str> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bool(&mut self) -> Result<bool, &'static str> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("bad bool"),
        }
    }
    pub fn addr(&mut self) -> Result<[u8; 32], &'static str> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    pub fn hash(&mut self) -> Result<[u8; 32], &'static str> {
        self.addr()
    }
    pub fn bytes(&mut self) -> Result<&'a [u8], &'static str> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> Result<&'a str, &'static str> {
        core::str::from_utf8(self.bytes()?).map_err(|_| "string is not UTF-8")
    }
    pub fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }
    /// Fail on trailing bytes.
    pub fn end(&self) -> Result<(), &'static str> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err("trailing input")
        }
    }
}

#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    /// Start a call input for `method`.
    pub fn method(name: &str) -> Self {
        let mut w = Self::new();
        w.0.push(name.len() as u8);
        w.0.extend_from_slice(name.as_bytes());
        w
    }
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bool(self, v: bool) -> Self {
        self.u8(v as u8)
    }
    pub fn addr(mut self, v: &[u8; 32]) -> Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn hash(self, v: &[u8; 32]) -> Self {
        self.addr(v)
    }
    pub fn bytes(mut self, v: &[u8]) -> Self {
        self.0.extend_from_slice(&(v.len() as u32).to_le_bytes());
        self.0.extend_from_slice(v);
        self
    }
    pub fn str(self, v: &str) -> Self {
        self.bytes(v.as_bytes())
    }
    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}
