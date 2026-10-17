//! Wire format of the open network.
//!
//! Before the handshake the listener sends a plaintext banner, `PSLNET01 || key[32]`, so
//! a dialer that only knows `host:port` learns which key to authenticate against. The
//! encrypted handshake that follows (commonware-stream) proves the listener holds that
//! key, so a forged banner only makes the dial fail. Seeds given as `key@host:port` pin
//! the key and refuse any other.
//!
//! After the handshake every encrypted frame is one message, first byte = kind:
//!
//! | kind | body |
//! |---|---|
//! | 0 Data | channel u64 LE, payload |
//! | 1 Hello | version u32 LE, listen address (optional) |
//! | 2 Ping / 3 Pong | nonce u64 LE |
//! | 4 GetAddr | (empty) |
//! | 5 Addr | count u16 LE, then count x (key[32], address, last_seen_ms u64 LE) |
//! | 6 Bye | reason u8 (1 = inbound slots full) |
//!
//! Address = tag u8 (4 or 6), ip (4 or 16 bytes), port u16 LE; an optional address is
//! tag 0 for none.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use commonware_codec::{Encode as _, ReadExt as _};
use commonware_cryptography::ed25519::PublicKey;
use commonware_runtime::IoBuf;

pub const BANNER_MAGIC: &[u8; 8] = b"PSLNET01";
pub const BANNER_LEN: usize = 40;
pub const VERSION: u32 = 1;
/// Most addresses one Addr message may carry (more is misbehaviour, as in Bitcoin).
pub const MAX_ADDRS: usize = 1000;
/// Bytes in front of a Data payload.
pub const DATA_HEADER: usize = 9;

pub const KIND_DATA: u8 = 0;
pub const KIND_HELLO: u8 = 1;
pub const KIND_PING: u8 = 2;
pub const KIND_PONG: u8 = 3;
pub const KIND_GETADDR: u8 = 4;
pub const KIND_ADDR: u8 = 5;
pub const KIND_BYE: u8 = 6;

pub const BYE_FULL: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrRecord {
    pub key: PublicKey,
    pub addr: SocketAddr,
    pub last_seen_ms: u64,
}

#[derive(Debug)]
pub enum Msg {
    Data { channel: u64, payload: IoBuf },
    Hello { version: u32, listen: Option<SocketAddr> },
    Ping(u64),
    Pong(u64),
    GetAddr,
    Addr(Vec<AddrRecord>),
    Bye(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

pub fn key_bytes(k: &PublicKey) -> Vec<u8> {
    k.encode().to_vec()
}

pub fn key_from(b: &[u8]) -> Option<PublicKey> {
    if b.len() != 32 {
        return None;
    }
    PublicKey::read(&mut &b[..]).ok()
}

pub fn banner(me: &PublicKey) -> Vec<u8> {
    let mut v = BANNER_MAGIC.to_vec();
    v.extend_from_slice(&key_bytes(me));
    v
}

pub fn parse_banner(b: &[u8]) -> Option<PublicKey> {
    if b.len() != BANNER_LEN || &b[..8] != BANNER_MAGIC {
        return None;
    }
    key_from(&b[8..])
}

pub fn data_header(channel: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(DATA_HEADER);
    v.push(KIND_DATA);
    v.extend_from_slice(&channel.to_le_bytes());
    v
}

fn put_addr(v: &mut Vec<u8>, a: &SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => {
            v.push(4);
            v.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            v.push(6);
            v.extend_from_slice(&ip.octets());
        }
    }
    v.extend_from_slice(&a.port().to_le_bytes());
}

pub fn encode(m: &Msg) -> Vec<u8> {
    let mut v = Vec::new();
    match m {
        Msg::Data { channel, payload } => {
            v = data_header(*channel);
            v.extend_from_slice(payload.as_ref());
        }
        Msg::Hello { version, listen } => {
            v.push(KIND_HELLO);
            v.extend_from_slice(&version.to_le_bytes());
            match listen {
                Some(a) => put_addr(&mut v, a),
                None => v.push(0),
            }
        }
        Msg::Ping(n) => {
            v.push(KIND_PING);
            v.extend_from_slice(&n.to_le_bytes());
        }
        Msg::Pong(n) => {
            v.push(KIND_PONG);
            v.extend_from_slice(&n.to_le_bytes());
        }
        Msg::GetAddr => v.push(KIND_GETADDR),
        Msg::Addr(list) => {
            v.push(KIND_ADDR);
            v.extend_from_slice(&(list.len() as u16).to_le_bytes());
            for r in list {
                v.extend_from_slice(&key_bytes(&r.key));
                put_addr(&mut v, &r.addr);
                v.extend_from_slice(&r.last_seen_ms.to_le_bytes());
            }
        }
        Msg::Bye(r) => {
            v.push(KIND_BYE);
            v.push(*r);
        }
    }
    v
}

struct Cur<'a> {
    b: &'a [u8],
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if self.b.len() < n {
            return Err(Malformed("truncated"));
        }
        let (h, t) = self.b.split_at(n);
        self.b = t;
        Ok(h)
    }
    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, Malformed> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Malformed> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn addr_opt(&mut self) -> Result<Option<SocketAddr>, Malformed> {
        let ip = match self.u8()? {
            0 => return Ok(None),
            4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).unwrap())),
            6 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(self.take(16)?).unwrap())),
            _ => return Err(Malformed("address tag")),
        };
        Ok(Some(SocketAddr::new(ip, self.u16()?)))
    }
    fn end(&self) -> Result<(), Malformed> {
        if self.b.is_empty() { Ok(()) } else { Err(Malformed("trailing bytes")) }
    }
}

pub fn decode(buf: IoBuf) -> Result<Msg, Malformed> {
    let b = buf.as_ref();
    let Some(&kind) = b.first() else { return Err(Malformed("empty frame")) };
    let mut c = Cur { b: &b[1..] };
    let m = match kind {
        KIND_DATA => {
            let channel = c.u64()?;
            return Ok(Msg::Data { channel, payload: buf.slice(DATA_HEADER..) });
        }
        KIND_HELLO => Msg::Hello { version: c.u32()?, listen: c.addr_opt()? },
        KIND_PING => Msg::Ping(c.u64()?),
        KIND_PONG => Msg::Pong(c.u64()?),
        KIND_GETADDR => Msg::GetAddr,
        KIND_ADDR => {
            let n = c.u16()? as usize;
            if n > MAX_ADDRS {
                return Err(Malformed("too many addresses"));
            }
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let key = key_from(c.take(32)?).ok_or(Malformed("address key"))?;
                let addr = c.addr_opt()?.ok_or(Malformed("empty address"))?;
                list.push(AddrRecord { key, addr, last_seen_ms: c.u64()? });
            }
            Msg::Addr(list)
        }
        KIND_BYE => Msg::Bye(c.u8()?),
        _ => return Err(Malformed("unknown kind")),
    };
    c.end()?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::{ed25519, Signer as _};

    #[test]
    fn roundtrip() {
        let k = ed25519::PrivateKey::from_seed(1).public_key();
        let msgs = vec![
            Msg::Hello { version: 1, listen: Some("1.2.3.4:5".parse().unwrap()) },
            Msg::Hello { version: 1, listen: None },
            Msg::Ping(7),
            Msg::Pong(8),
            Msg::GetAddr,
            Msg::Addr(vec![AddrRecord { key: k.clone(), addr: "[::1]:9".parse().unwrap(), last_seen_ms: 3 }]),
            Msg::Bye(BYE_FULL),
            Msg::Data { channel: 3, payload: IoBuf::from(vec![1, 2, 3]) },
        ];
        for m in msgs {
            let e = encode(&m);
            let d = decode(IoBuf::from(e.clone())).unwrap();
            assert_eq!(encode(&d), e);
        }
        assert_eq!(parse_banner(&banner(&k)), Some(k));
        assert!(decode(IoBuf::from(vec![9u8])).is_err());
        assert!(decode(IoBuf::from(vec![KIND_PING, 1])).is_err());
        let mut big = vec![KIND_ADDR];
        big.extend_from_slice(&1001u16.to_le_bytes());
        assert_eq!(decode(IoBuf::from(big)).unwrap_err(), Malformed("too many addresses"));
    }
}
