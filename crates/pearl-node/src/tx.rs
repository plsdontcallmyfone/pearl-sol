//! SPEC v2 section 2 transaction encoding (fixed little-endian binary).
//!
//! ```text
//! Tx  = version u8 (=1) || chain_id u32 || from [32] || nonce u64 || fee u64
//!       || kind u8 || body || sig [64]
//! sig = ed25519 over varint(len NS) || NS || all preceding bytes, NS = b"pearl-sol:tx:v1"
//! ```
//! The framing is Commonware's `sign(namespace, msg)` (`union_unique`), so a wallet can
//! produce the signature with a plain ed25519 `signMessage` over [`signing_bytes`].

use anyhow::{bail, ensure, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

pub const TX_VERSION: u8 = 1;
pub const TX_NAMESPACE: &[u8] = b"pearl-sol:tx:v1";

pub mod kind {
    pub const TRANSFER: u8 = 0;
    pub const REGISTER_AGENT: u8 = 1;
    pub const POST_JOB: u8 = 2;
    pub const SUBMIT_RESULT: u8 = 3;
    pub const COMPLETE_JOB: u8 = 4;
    pub const EXPIRE_JOB: u8 = 5;
    pub const BRIDGE_UNLOCK: u8 = 6;
    pub const BRIDGE_LOCK: u8 = 7;
    pub const ADMIN_CONFIG: u8 = 8;
    pub const SET_ROUTE: u8 = 9;
    pub const CLEAR_ROUTE: u8 = 10;
    pub const REGISTER_MODEL: u8 = 11;
    pub const POST_SEGMENTS: u8 = 12;
}

/// One standing job of a ticket (SPEC 4.5, PostSegments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSpec {
    pub matrix: u16,
    pub m: u32,
    pub max_blocks: u32,
    pub hash_a: [u8; 32],
    pub c_hash: [u8; 32],
    pub a_uri: String,
}

/// Unsigned LEB128, as Commonware's `varint` for lengths.
pub fn varint(mut x: u64, out: &mut Vec<u8>) {
    loop {
        let b = (x & 0x7f) as u8;
        x >>= 7;
        if x == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// The body kinds the miner and attester build. Others are opaque bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Transfer { to: [u8; 32], amount: u64 },
    SubmitResult { job_id: [u8; 32], result_hash: [u8; 32], uri: String },
    CompleteJob { job_id: [u8; 32], sigs: Vec<([u8; 32], [u8; 64])> },
    ExpireJob { job_id: [u8; 32] },
    /// `ticket_id [32] | round u16 | count u8 | (matrix u16 | m u32 | max_blocks u32 | hash_a | c_hash | a_uri (u16 len))*`
    PostSegments { ticket_id: [u8; 32], round: u16, segments: Vec<SegmentSpec> },
    /// Any kind, body bytes as given.
    Raw { kind: u8, body: Vec<u8> },
}

impl Body {
    pub fn kind(&self) -> u8 {
        match self {
            Body::Transfer { .. } => kind::TRANSFER,
            Body::SubmitResult { .. } => kind::SUBMIT_RESULT,
            Body::CompleteJob { .. } => kind::COMPLETE_JOB,
            Body::ExpireJob { .. } => kind::EXPIRE_JOB,
            Body::PostSegments { .. } => kind::POST_SEGMENTS,
            Body::Raw { kind, .. } => *kind,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut v = Vec::new();
        match self {
            Body::Transfer { to, amount } => {
                v.extend_from_slice(to);
                v.extend_from_slice(&amount.to_le_bytes());
            }
            Body::SubmitResult { job_id, result_hash, uri } => {
                ensure!(uri.len() <= u16::MAX as usize, "uri longer than 65535 bytes");
                v.extend_from_slice(job_id);
                v.extend_from_slice(result_hash);
                v.extend_from_slice(&(uri.len() as u16).to_le_bytes());
                v.extend_from_slice(uri.as_bytes());
            }
            Body::CompleteJob { job_id, sigs } => {
                ensure!(sigs.len() <= u8::MAX as usize, "more than 255 evaluator signatures");
                v.extend_from_slice(job_id);
                v.push(sigs.len() as u8);
                for (pk, sig) in sigs {
                    v.extend_from_slice(pk);
                    v.extend_from_slice(sig);
                }
            }
            Body::ExpireJob { job_id } => v.extend_from_slice(job_id),
            Body::PostSegments { ticket_id, round, segments } => {
                ensure!(!segments.is_empty() && segments.len() <= pearl_job::standing::MAX_SEGMENTS_PER_TX, "1..=64 segments per tx");
                v.extend_from_slice(ticket_id);
                v.extend_from_slice(&round.to_le_bytes());
                v.push(segments.len() as u8);
                for g in segments {
                    ensure!(g.a_uri.len() <= 512, "a_uri too long");
                    v.extend_from_slice(&g.matrix.to_le_bytes());
                    v.extend_from_slice(&g.m.to_le_bytes());
                    v.extend_from_slice(&g.max_blocks.to_le_bytes());
                    v.extend_from_slice(&g.hash_a);
                    v.extend_from_slice(&g.c_hash);
                    v.extend_from_slice(&(g.a_uri.len() as u16).to_le_bytes());
                    v.extend_from_slice(g.a_uri.as_bytes());
                }
            }
            Body::Raw { body, .. } => v.extend_from_slice(body),
        }
        Ok(v)
    }
}

/// An unsigned transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tx {
    pub chain_id: u32,
    pub from: [u8; 32],
    pub nonce: u64,
    pub fee: u64,
    pub body: Body,
}

/// Bytes before the signature.
pub fn unsigned_bytes(tx: &Tx) -> Result<Vec<u8>> {
    let mut v = Vec::with_capacity(54 + 128);
    v.push(TX_VERSION);
    v.extend_from_slice(&tx.chain_id.to_le_bytes());
    v.extend_from_slice(&tx.from);
    v.extend_from_slice(&tx.nonce.to_le_bytes());
    v.extend_from_slice(&tx.fee.to_le_bytes());
    v.push(tx.body.kind());
    v.extend_from_slice(&tx.body.encode()?);
    Ok(v)
}

/// What the ed25519 signature covers: `varint(len NS) || NS || unsigned bytes`.
pub fn signing_bytes(unsigned: &[u8]) -> Vec<u8> {
    pearl_job::messages::frame(TX_NAMESPACE, unsigned)
}

/// Full wire bytes, signed by `key` (which must be `tx.from`).
pub fn sign(tx: &Tx, key: &SigningKey) -> Result<Vec<u8>> {
    ensure!(key.verifying_key().to_bytes() == tx.from, "signing key is not tx.from");
    let mut u = unsigned_bytes(tx)?;
    let sig = key.sign(&signing_bytes(&u));
    u.extend_from_slice(&sig.to_bytes());
    Ok(u)
}

/// Check a wire tx's signature against its `from` field. Returns `(from, kind)`.
pub fn verify(wire: &[u8]) -> Result<([u8; 32], u8)> {
    ensure!(wire.len() >= 54 + 64, "tx too short");
    if wire[0] != TX_VERSION {
        bail!("tx version {}", wire[0]);
    }
    let (u, s) = wire.split_at(wire.len() - 64);
    let from: [u8; 32] = u[5..37].try_into().unwrap();
    let vk = VerifyingKey::from_bytes(&from)?;
    vk.verify(&signing_bytes(u), &Signature::from_bytes(s.try_into().unwrap()))?;
    Ok((from, u[53]))
}

/// Parse wire bytes (no signature check; see [`verify`]). Known kinds decode into
/// their [`Body`] variant, others into `Body::Raw`.
pub fn decode(wire: &[u8]) -> Result<(Tx, [u8; 64])> {
    ensure!(wire.len() >= 54 + 64, "tx too short");
    ensure!(wire[0] == TX_VERSION, "tx version {}", wire[0]);
    let (u, s) = wire.split_at(wire.len() - 64);
    let chain_id = u32::from_le_bytes(u[1..5].try_into().unwrap());
    let from: [u8; 32] = u[5..37].try_into().unwrap();
    let nonce = u64::from_le_bytes(u[37..45].try_into().unwrap());
    let fee = u64::from_le_bytes(u[45..53].try_into().unwrap());
    let k = u[53];
    let b = &u[54..];
    let a32 = |x: &[u8]| -> [u8; 32] { x.try_into().unwrap() };
    let body = match k {
        kind::TRANSFER => {
            ensure!(b.len() == 40, "Transfer body is {} bytes", b.len());
            Body::Transfer { to: a32(&b[..32]), amount: u64::from_le_bytes(b[32..40].try_into().unwrap()) }
        }
        kind::SUBMIT_RESULT => {
            ensure!(b.len() >= 66, "SubmitResult body too short");
            let ul = u16::from_le_bytes(b[64..66].try_into().unwrap()) as usize;
            ensure!(b.len() == 66 + ul, "SubmitResult uri length mismatch");
            Body::SubmitResult { job_id: a32(&b[..32]), result_hash: a32(&b[32..64]), uri: String::from_utf8(b[66..].to_vec())? }
        }
        kind::COMPLETE_JOB => {
            ensure!(b.len() >= 33, "CompleteJob body too short");
            let c = b[32] as usize;
            ensure!(b.len() == 33 + 96 * c, "CompleteJob signature count mismatch");
            let sigs = (0..c)
                .map(|i| {
                    let o = 33 + 96 * i;
                    (a32(&b[o..o + 32]), <[u8; 64]>::try_from(&b[o + 32..o + 96]).unwrap())
                })
                .collect();
            Body::CompleteJob { job_id: a32(&b[..32]), sigs }
        }
        kind::EXPIRE_JOB => {
            ensure!(b.len() == 32, "ExpireJob body is {} bytes", b.len());
            Body::ExpireJob { job_id: a32(b) }
        }
        _ => Body::Raw { kind: k, body: b.to_vec() },
    };
    Ok((Tx { chain_id, from, nonce, fee, body }, s.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let from = key.verifying_key().to_bytes();
        let tx = Tx {
            chain_id: 0x01020304,
            from,
            nonce: 7,
            fee: 9,
            body: Body::SubmitResult { job_id: [1; 32], result_hash: [2; 32], uri: "file:///c".into() },
        };
        let w = sign(&tx, &key).unwrap();
        assert_eq!(w[0], 1);
        assert_eq!(&w[1..5], &[4, 3, 2, 1]);
        assert_eq!(&w[5..37], &from);
        assert_eq!(&w[37..45], &7u64.to_le_bytes());
        assert_eq!(&w[45..53], &9u64.to_le_bytes());
        assert_eq!(w[53], 3);
        assert_eq!(&w[54..86], &[1; 32]);
        assert_eq!(&w[86..118], &[2; 32]);
        assert_eq!(&w[118..120], &9u16.to_le_bytes());
        assert_eq!(&w[120..129], b"file:///c");
        assert_eq!(w.len(), 129 + 64);
        assert_eq!(verify(&w).unwrap(), (from, 3));
        assert_eq!(decode(&w).unwrap().0, tx);
        let sb = signing_bytes(&w[..129]);
        assert_eq!(sb[0], 15);
        assert_eq!(&sb[1..16], b"pearl-sol:tx:v1");
        let mut bad = w.clone();
        bad[40] ^= 1;
        assert!(verify(&bad).is_err());

        let c = Body::CompleteJob { job_id: [5; 32], sigs: vec![([6; 32], [7; 64]), ([8; 32], [9; 64])] }.encode().unwrap();
        assert_eq!(c.len(), 32 + 1 + 2 * 96);
        assert_eq!(c[32], 2);
        assert_eq!(&c[33..65], &[6; 32]);
        let mut v = Vec::new();
        varint(300, &mut v);
        assert_eq!(v, [0xac, 0x02]);
    }
}
