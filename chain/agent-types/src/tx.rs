//! Transactions (SPEC 2).
//!
//! ```text
//! Tx  = version u8 (=1) || chain_id u32 || from [32] || nonce u64 || fee u64
//!       || kind u8 || body || sig [64]
//! sig = ed25519 over varint(len NS) || NS || all preceding bytes, NS = b"pearl-sol:tx:v1"
//! ```
//! All integers little-endian. Variable-length fields carry a u8 or u16 length prefix
//! as documented per kind in docs/RPC.md.

use commonware_codec::{Encode, ReadExt};
use commonware_cryptography::{ed25519, Signer, Verifier};

use crate::codec::{R, W};
use crate::{blake3_hash, sha256, DecodeError, Hash, Pubkey, Sig, DOMAIN_DEPOSIT, DOMAIN_DEPOSIT_ORDER, DOMAIN_ORDER_JOB, DOMAIN_RESULT, NS_TX, TX_VERSION};

pub const MAX_NAME: usize = 64;
pub const MAX_URI: usize = 512;
pub const MAX_EVALUATORS: usize = 32;
pub const MAX_SIGS: usize = 32;
pub const MAX_ADMIN_VALUE: usize = 4096;
pub const MAX_TX_SIZE: usize = 16 * 1024;
/// Fixed prefix: version 1 + chain_id 4 + from 32 + nonce 8 + fee 8 + kind 1.
pub const PREFIX_LEN: usize = 54;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TxKind {
    Transfer = 0,
    RegisterAgent = 1,
    PostJob = 2,
    SubmitResult = 3,
    CompleteJob = 4,
    ExpireJob = 5,
    BridgeUnlock = 6,
    BridgeLock = 7,
    AdminConfig = 8,
}

impl TxKind {
    pub fn from_u8(k: u8) -> Result<Self, DecodeError> {
        Ok(match k {
            0 => Self::Transfer,
            1 => Self::RegisterAgent,
            2 => Self::PostJob,
            3 => Self::SubmitResult,
            4 => Self::CompleteJob,
            5 => Self::ExpireJob,
            6 => Self::BridgeUnlock,
            7 => Self::BridgeLock,
            8 => Self::AdminConfig,
            k => return Err(DecodeError::Kind(k)),
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Transfer => "Transfer",
            Self::RegisterAgent => "RegisterAgent",
            Self::PostJob => "PostJob",
            Self::SubmitResult => "SubmitResult",
            Self::CompleteJob => "CompleteJob",
            Self::ExpireJob => "ExpireJob",
            Self::BridgeUnlock => "BridgeUnlock",
            Self::BridgeLock => "BridgeLock",
            Self::AdminConfig => "AdminConfig",
        }
    }
}

/// (pubkey, signature) pair used by evaluator and attester sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttesterSig {
    pub pubkey: Pubkey,
    pub sig: Sig,
}

/// PostJob body (SPEC 4.1).
///
/// ```text
/// m u32 | n u32 | k u32 | config [52] | hash_a [32] | hash_b [32] | data_uri (u16 len, bytes)
/// | worker_pay u64 | protocol_fee u64 | deadline_height u64 | max_blocks u32
/// | evaluators (count u8, pubkey [32]*) | threshold u8
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostJob {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub config: [u8; 52],
    pub hash_a: Hash,
    pub hash_b: Hash,
    pub data_uri: Vec<u8>,
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub deadline_height: u64,
    pub max_blocks: u32,
    pub evaluators: Vec<Pubkey>,
    pub threshold: u8,
}

impl PostJob {
    pub fn write(&self, w: &mut W) {
        w.u32(self.m)
            .u32(self.n)
            .u32(self.k)
            .bytes(&self.config)
            .bytes(&self.hash_a)
            .bytes(&self.hash_b)
            .bytes16(&self.data_uri)
            .u64(self.worker_pay)
            .u64(self.protocol_fee)
            .u64(self.deadline_height)
            .u32(self.max_blocks)
            .u8(self.evaluators.len() as u8);
        for e in &self.evaluators {
            w.bytes(e);
        }
        w.u8(self.threshold);
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let m = r.u32()?;
        let n = r.u32()?;
        let k = r.u32()?;
        let config = r.arr()?;
        let hash_a = r.arr()?;
        let hash_b = r.arr()?;
        let data_uri = r.bytes16(MAX_URI, "data_uri")?;
        let worker_pay = r.u64()?;
        let protocol_fee = r.u64()?;
        let deadline_height = r.u64()?;
        let max_blocks = r.u32()?;
        let count = r.u8()? as usize;
        if count > MAX_EVALUATORS {
            return Err(DecodeError::TooLarge("evaluators"));
        }
        let mut evaluators = Vec::with_capacity(count);
        for _ in 0..count {
            evaluators.push(r.arr()?);
        }
        let threshold = r.u8()?;
        Ok(Self {
            m,
            n,
            k,
            config,
            hash_a,
            hash_b,
            data_uri,
            worker_pay,
            protocol_fee,
            deadline_height,
            max_blocks,
            evaluators,
            threshold,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        self.write(&mut w);
        w.into_inner()
    }

    /// sha256 of the body bytes (`order.body_hash` on Solana); part of the order unlock message.
    pub fn body_hash(&self) -> Hash {
        sha256(&[&self.encode()])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxBody {
    /// `to [32], amount u64`
    Transfer { to: Pubkey, amount: u64 },
    /// `name_len u8, name, uri_len u16, uri`
    RegisterAgent { name: Vec<u8>, uri: Vec<u8> },
    PostJob(PostJob),
    /// `job_id [32], result_hash [32], uri_len u16, uri`
    SubmitResult { job_id: Hash, result_hash: Hash, uri: Vec<u8> },
    /// `job_id [32], count u8, (pubkey [32], sig [64])*`
    CompleteJob { job_id: Hash, sigs: Vec<AttesterSig> },
    /// `job_id [32]`
    ExpireJob { job_id: Hash },
    /// `solana_nonce u64, recipient [32], amount_spl u64, job_order u8 (0|1),
    ///  [PostJob body if 1], count u8, (pubkey [32], sig [64])*`.
    /// For an order, the body's `deadline_height` is RELATIVE (blocks after the unlock).
    BridgeUnlock {
        solana_nonce: u64,
        recipient: Pubkey,
        amount_spl: u64,
        job_order: Option<PostJob>,
        sigs: Vec<AttesterSig>,
    },
    /// `solana_recipient [32], amount u64` (amount a multiple of 1000)
    BridgeLock { solana_recipient: Pubkey, amount: u64 },
    /// `key u8, value_len u16, value`
    AdminConfig { key: u8, value: Vec<u8> },
}

fn write_sigs(w: &mut W, sigs: &[AttesterSig]) {
    w.u8(sigs.len() as u8);
    for s in sigs {
        w.bytes(&s.pubkey).bytes(&s.sig);
    }
}

fn read_sigs(r: &mut R<'_>) -> Result<Vec<AttesterSig>, DecodeError> {
    let n = r.u8()? as usize;
    if n > MAX_SIGS {
        return Err(DecodeError::TooLarge("sigs"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(AttesterSig { pubkey: r.arr()?, sig: r.arr()? });
    }
    Ok(v)
}

impl TxBody {
    pub fn kind(&self) -> TxKind {
        match self {
            Self::Transfer { .. } => TxKind::Transfer,
            Self::RegisterAgent { .. } => TxKind::RegisterAgent,
            Self::PostJob(_) => TxKind::PostJob,
            Self::SubmitResult { .. } => TxKind::SubmitResult,
            Self::CompleteJob { .. } => TxKind::CompleteJob,
            Self::ExpireJob { .. } => TxKind::ExpireJob,
            Self::BridgeUnlock { .. } => TxKind::BridgeUnlock,
            Self::BridgeLock { .. } => TxKind::BridgeLock,
            Self::AdminConfig { .. } => TxKind::AdminConfig,
        }
    }

    pub fn write(&self, w: &mut W) {
        match self {
            Self::Transfer { to, amount } => {
                w.bytes(to).u64(*amount);
            }
            Self::RegisterAgent { name, uri } => {
                w.bytes8(name).bytes16(uri);
            }
            Self::PostJob(p) => p.write(w),
            Self::SubmitResult { job_id, result_hash, uri } => {
                w.bytes(job_id).bytes(result_hash).bytes16(uri);
            }
            Self::CompleteJob { job_id, sigs } => {
                w.bytes(job_id);
                write_sigs(w, sigs);
            }
            Self::ExpireJob { job_id } => {
                w.bytes(job_id);
            }
            Self::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs } => {
                w.u64(*solana_nonce).bytes(recipient).u64(*amount_spl);
                match job_order {
                    None => {
                        w.u8(0);
                    }
                    Some(p) => {
                        w.u8(1);
                        p.write(w);
                    }
                }
                write_sigs(w, sigs);
            }
            Self::BridgeLock { solana_recipient, amount } => {
                w.bytes(solana_recipient).u64(*amount);
            }
            Self::AdminConfig { key, value } => {
                w.u8(*key).bytes16(value);
            }
        }
    }

    pub fn read(kind: TxKind, r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(match kind {
            TxKind::Transfer => Self::Transfer { to: r.arr()?, amount: r.u64()? },
            TxKind::RegisterAgent => {
                let name = r.bytes8(MAX_NAME, "name")?;
                let uri = r.bytes16(MAX_URI, "uri")?;
                Self::RegisterAgent { name, uri }
            }
            TxKind::PostJob => Self::PostJob(PostJob::read(r)?),
            TxKind::SubmitResult => Self::SubmitResult {
                job_id: r.arr()?,
                result_hash: r.arr()?,
                uri: r.bytes16(MAX_URI, "uri")?,
            },
            TxKind::CompleteJob => Self::CompleteJob { job_id: r.arr()?, sigs: read_sigs(r)? },
            TxKind::ExpireJob => Self::ExpireJob { job_id: r.arr()? },
            TxKind::BridgeUnlock => {
                let solana_nonce = r.u64()?;
                let recipient = r.arr()?;
                let amount_spl = r.u64()?;
                let job_order = match r.u8()? {
                    0 => None,
                    1 => Some(PostJob::read(r)?),
                    _ => return Err(DecodeError::Invalid("job_order flag")),
                };
                let sigs = read_sigs(r)?;
                Self::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs }
            }
            TxKind::BridgeLock => Self::BridgeLock { solana_recipient: r.arr()?, amount: r.u64()? },
            TxKind::AdminConfig => {
                let key = r.u8()?;
                let value = r.bytes16(MAX_ADMIN_VALUE, "admin value")?;
                Self::AdminConfig { key, value }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tx {
    pub chain_id: u32,
    pub from: Pubkey,
    pub nonce: u64,
    pub fee: u64,
    pub body: TxBody,
    pub sig: Sig,
}

impl Tx {
    /// Bytes covered by the signature: everything before `sig`.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(TX_VERSION)
            .u32(self.chain_id)
            .bytes(&self.from)
            .u64(self.nonce)
            .u64(self.fee)
            .u8(self.body.kind() as u8);
        self.body.write(&mut w);
        w.into_inner()
    }

    /// The exact message ed25519 signs: `varint(len NS) || NS || signing_bytes`.
    pub fn framed_signing_message(&self) -> Vec<u8> {
        frame(NS_TX, &self.signing_bytes())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = self.signing_bytes();
        b.extend_from_slice(&self.sig);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() > MAX_TX_SIZE {
            return Err(DecodeError::TooLarge("tx"));
        }
        let mut r = R::new(b);
        let version = r.u8()?;
        if version != TX_VERSION {
            return Err(DecodeError::Version(version as u32));
        }
        let chain_id = r.u32()?;
        let from = r.arr()?;
        let nonce = r.u64()?;
        let fee = r.u64()?;
        let kind = TxKind::from_u8(r.u8()?)?;
        let body = TxBody::read(kind, &mut r)?;
        let sig = r.arr()?;
        r.finish()?;
        Ok(Self { chain_id, from, nonce, fee, body, sig })
    }

    /// `tx_hash = blake3(encoded tx including sig)`.
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }

    /// Build and sign with a 32-byte ed25519 secret seed (same as a Solana keypair's first half).
    pub fn signed(seed: &[u8; 32], chain_id: u32, nonce: u64, fee: u64, body: TxBody) -> Self {
        let sk = private_key(seed);
        let from = pubkey_of(&sk);
        let mut tx = Self { chain_id, from, nonce, fee, body, sig: [0; 64] };
        tx.sig = sign_raw(&sk, NS_TX, &tx.signing_bytes());
        tx
    }

    pub fn verify_sig(&self) -> bool {
        verify(&self.from, NS_TX, &self.signing_bytes(), &self.sig)
    }
}

/// Commonware `union_unique` framing: `varint(len ns) || ns || msg`.
pub fn frame(ns: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ns.len() + msg.len() + 2);
    let mut n = ns.len();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(ns);
    out.extend_from_slice(msg);
    out
}

pub fn private_key(seed: &[u8; 32]) -> ed25519::PrivateKey {
    ed25519::PrivateKey::read(&mut &seed[..]).expect("any 32 bytes is an ed25519 seed")
}

pub fn pubkey_of(sk: &ed25519::PrivateKey) -> Pubkey {
    sk.public_key().encode().as_ref().try_into().unwrap()
}

pub fn pubkey_from_seed(seed: &[u8; 32]) -> Pubkey {
    pubkey_of(&private_key(seed))
}

pub fn sign_raw(sk: &ed25519::PrivateKey, ns: &[u8], msg: &[u8]) -> Sig {
    sk.sign(ns, msg).encode().as_ref().try_into().unwrap()
}

/// Sign `msg` under namespace `ns` with a 32-byte seed.
pub fn sign_with_seed(seed: &[u8; 32], ns: &[u8], msg: &[u8]) -> Sig {
    sign_raw(&private_key(seed), ns, msg)
}

/// Verify a Commonware-framed ed25519 signature. Invalid keys or signatures return false.
pub fn verify(pubkey: &Pubkey, ns: &[u8], msg: &[u8], sig: &Sig) -> bool {
    let Ok(pk) = ed25519::PublicKey::read(&mut &pubkey[..]) else {
        return false;
    };
    let Ok(s) = ed25519::Signature::read(&mut &sig[..]) else {
        return false;
    };
    pk.verify(ns, msg, &s)
}

/// Raw ed25519 signature (no framing) with a 32-byte seed.
pub fn sign_plain(seed: &[u8; 32], msg: &[u8]) -> Sig {
    use ed25519_dalek::Signer as _;
    ed25519_dalek::SigningKey::from_bytes(seed).sign(msg).to_bytes()
}

/// Raw ed25519 strict verification (same rule as Solana's precompile for honest keys).
pub fn verify_plain(pubkey: &Pubkey, msg: &[u8], sig: &Sig) -> bool {
    let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(pubkey) else {
        return false;
    };
    vk.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(sig)).is_ok()
}

/// Attested message for a plain deposit: `b"pearl-sol:deposit:v1" || nonce || recipient || amount_spl`.
pub fn deposit_message(solana_nonce: u64, recipient: &Pubkey, amount_spl: u64) -> Vec<u8> {
    let mut w = W::new();
    w.bytes(DOMAIN_DEPOSIT).u64(solana_nonce).bytes(recipient).u64(amount_spl);
    w.into_inner()
}

/// Attested message for a storefront order:
/// `b"pearl-sol:deposit-order:v1" || nonce || recipient || amount_spl || sha256(PostJob body)`.
pub fn order_message(solana_nonce: u64, recipient: &Pubkey, amount_spl: u64, job: &PostJob) -> Vec<u8> {
    let mut w = W::new();
    w.bytes(DOMAIN_DEPOSIT_ORDER).u64(solana_nonce).bytes(recipient).u64(amount_spl).bytes(&job.body_hash());
    w.into_inner()
}

/// Order job id: `sha256(b"pearl-sol:order-job:v1" || nonce u64 LE)`.
pub fn order_job_id(solana_nonce: u64) -> Hash {
    sha256(&[DOMAIN_ORDER_JOB, &solana_nonce.to_le_bytes()])
}

/// Message evaluators sign to complete a job: `b"pearl-sol:result:v1" || job_id || deliverer || result_hash`.
pub fn result_message(job_id: &Hash, deliverer: &Pubkey, result_hash: &Hash) -> Vec<u8> {
    [DOMAIN_RESULT, &job_id[..], &deliverer[..], &result_hash[..]].concat()
}

impl commonware_codec::Write for Tx {
    fn write(&self, buf: &mut impl bytes::BufMut) {
        let b = self.encode();
        (b.len() as u32).write(buf);
        buf.put_slice(&b);
    }
}

impl commonware_codec::EncodeSize for Tx {
    fn encode_size(&self) -> usize {
        4 + self.encode().len()
    }
}

impl commonware_codec::Read for Tx {
    type Cfg = ();
    fn read_cfg(buf: &mut impl bytes::Buf, _: &()) -> Result<Self, commonware_codec::Error> {
        let len = u32::read(buf)? as usize;
        if len > MAX_TX_SIZE || buf.remaining() < len {
            return Err(commonware_codec::Error::Invalid("Tx", "length"));
        }
        let mut b = vec![0u8; len];
        buf.copy_to_slice(&mut b);
        Tx::decode(&b).map_err(|_| commonware_codec::Error::Invalid("Tx", "decode"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_matches_commonware() {
        let f = frame(NS_TX, b"abc");
        assert_eq!(f, commonware_utils_union(NS_TX, b"abc"));
    }

    fn commonware_utils_union(ns: &[u8], msg: &[u8]) -> Vec<u8> {
        // NS_TX is 15 bytes, so the varint is one byte.
        let mut v = vec![ns.len() as u8];
        v.extend_from_slice(ns);
        v.extend_from_slice(msg);
        v
    }

    #[test]
    fn sign_and_verify() {
        let seed = [9u8; 32];
        let tx = Tx::signed(&seed, 7, 0, 10, TxBody::Transfer { to: [1; 32], amount: 5 });
        assert!(tx.verify_sig());
        let dec = Tx::decode(&tx.encode()).unwrap();
        assert_eq!(dec, tx);
        let mut bad = tx.clone();
        bad.fee = 11;
        assert!(!bad.verify_sig());
    }
}
