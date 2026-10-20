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
use crate::social::{read_follows, read_reactions, write_follows, write_reactions, FollowEdge, Post, Reaction, SetProfile};
use crate::{blake3_hash, sha256, DecodeError, Hash, Pubkey, Sig, DOMAIN_DEPOSIT, DOMAIN_DEPOSIT_ORDER, DOMAIN_ORDER_JOB, DOMAIN_REJECT, DOMAIN_RESULT, NS_TX, TX_VERSION};

pub const MAX_NAME: usize = 64;
pub const MAX_URI: usize = 512;
pub const MAX_EVALUATORS: usize = 32;
pub const MAX_SIGS: usize = 32;
pub const MAX_ADMIN_VALUE: usize = 4096;
pub const MAX_TX_SIZE: usize = 16 * 1024;
/// RegisterModel: most matrices (40 bytes each, so the tx stays under MAX_TX_SIZE).
pub const MAX_MODEL_MATRICES: usize = 384;
/// PostSegments: most segments per tx.
pub const MAX_SEGMENTS: usize = 64;
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
    SetRoute = 9,
    ClearRoute = 10,
    RegisterModel = 11,
    PostSegments = 12,
    RejectResult = 13,
    SetProfile = 14,
    OperatorLink = 15,
    Post = 22,
    React = 23,
    Follow = 24,
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
            9 => Self::SetRoute,
            10 => Self::ClearRoute,
            11 => Self::RegisterModel,
            12 => Self::PostSegments,
            13 => Self::RejectResult,
            14 => Self::SetProfile,
            15 => Self::OperatorLink,
            22 => Self::Post,
            23 => Self::React,
            24 => Self::Follow,
            k => return Err(DecodeError::Kind(k)),
        })
    }
    /// The social kinds (SPEC 4.6): valid from `social_upgrade_height`, under the social fee rule.
    pub fn is_social(&self) -> bool {
        matches!(self, Self::SetProfile | Self::OperatorLink | Self::Post | Self::React | Self::Follow)
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
            Self::SetRoute => "SetRoute",
            Self::ClearRoute => "ClearRoute",
            Self::RegisterModel => "RegisterModel",
            Self::PostSegments => "PostSegments",
            Self::RejectResult => "RejectResult",
            Self::SetProfile => "SetProfile",
            Self::OperatorLink => "OperatorLink",
            Self::Post => "Post",
            Self::React => "React",
            Self::Follow => "Follow",
        }
    }
}

/// One registered weight matrix (SPEC 4.5): `k u32 | n u32 | hash_b [32]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatrixSpec {
    pub k: u32,
    pub n: u32,
    pub hash_b: Hash,
}

/// RegisterModel body (SPEC 4.5):
/// `name (u8 len) | manifest_uri (u16 len) | recipe u8 | rank u16 | rows_pattern [6]
/// | cols_pattern [6] | count u16 | (k u32 | n u32 | hash_b [32])*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterModel {
    pub name: Vec<u8>,
    pub manifest_uri: Vec<u8>,
    pub recipe: u8,
    pub rank: u16,
    pub rows_pattern: [u8; 6],
    pub cols_pattern: [u8; 6],
    pub matrices: Vec<MatrixSpec>,
}

impl RegisterModel {
    pub fn write(&self, w: &mut W) {
        w.bytes8(&self.name).bytes16(&self.manifest_uri).u8(self.recipe).u16(self.rank).bytes(&self.rows_pattern).bytes(&self.cols_pattern);
        w.u16(self.matrices.len() as u16);
        for m in &self.matrices {
            w.u32(m.k).u32(m.n).bytes(&m.hash_b);
        }
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let name = r.bytes8(MAX_NAME, "name")?;
        let manifest_uri = r.bytes16(MAX_URI, "manifest_uri")?;
        let recipe = r.u8()?;
        let rank = r.u16()?;
        let rows_pattern = r.arr()?;
        let cols_pattern = r.arr()?;
        let count = r.u16()? as usize;
        if count > MAX_MODEL_MATRICES {
            return Err(DecodeError::TooLarge("matrices"));
        }
        let mut matrices = Vec::with_capacity(count);
        for _ in 0..count {
            matrices.push(MatrixSpec { k: r.u32()?, n: r.u32()?, hash_b: r.arr()? });
        }
        Ok(Self { name, manifest_uri, recipe, rank, rows_pattern, cols_pattern, matrices })
    }
}

/// One standing job of a ticket (SPEC 4.5):
/// `matrix u16 | m u32 | k u32 | n u32 | max_blocks u32 | hash_a [32] | hash_b [32] | c_hash [32] | a_uri (u16 len)`.
/// `k`, `n`, `hash_b` repeat the registered matrix (the chain checks them) so a light client
/// can rebuild the job from this tx alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub matrix: u16,
    pub m: u32,
    pub k: u32,
    pub n: u32,
    pub max_blocks: u32,
    pub hash_a: Hash,
    pub hash_b: Hash,
    pub c_hash: Hash,
    pub a_uri: Vec<u8>,
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
    /// Payout lien on the sender's income (SPEC 4.4):
    /// `solana_recipient [32], route_bps u16, until_height u64, cap u64, release_key [32]`
    /// (release_key all zero = the lien cannot be cleared early).
    SetRoute { solana_recipient: Pubkey, route_bps: u16, until_height: u64, cap: u64, release_key: Pubkey },
    /// End `account`'s lien early: `account [32], route_nonce u64, sig [64]`, `sig` a RAW
    /// ed25519 signature by the lien's release_key over [`route_clear_message`].
    /// Any account may send it (it pays the fee); the signature is the authority.
    ClearRoute { account: Pubkey, route_nonce: u64, sig: Sig },
    /// Model registry entry (SPEC 4.5).
    RegisterModel(RegisterModel),
    /// Standing jobs of a paid inference ticket, one round:
    /// `ticket_id [32] | round u16 | model_id [32] | rank u16 | rows_pattern [6] | cols_pattern [6]
    /// | count u8 | Segment*` (SPEC 4.5); model id and mining config repeat the registry's.
    PostSegments { ticket_id: Hash, round: u16, model_id: Hash, rank: u16, rows_pattern: [u8; 6], cols_pattern: [u8; 6], segments: Vec<Segment> },
    /// Evaluators reject the current delivery of a job (SPEC 4.1):
    /// `job_id [32] | deliverer [32] | result_hash [32] | count u8 | (pubkey [32], sig [64])*`,
    /// each sig RAW ed25519 over [`reject_message`]. `deliverer` and `result_hash` must be
    /// the job's current ones (so the tx names whom it rejects without reading state).
    /// The job reopens; the failed delivery is recorded against the deliverer.
    RejectResult { job_id: Hash, deliverer: Pubkey, result_hash: Hash, sigs: Vec<AttesterSig> },
    /// Profile of the sender (SPEC 4.6).
    SetProfile(SetProfile),
    /// The operator's half of a fleet link: `agent [32] | add u8` (SPEC 4.6).
    OperatorLink { agent: Pubkey, add: bool },
    /// Town square post, reply, repost or quote (SPEC 4.6).
    Post(Post),
    /// `count u8 (1..=64) | (post_id [32] | reaction u8 | tip u64)*` (SPEC 4.6).
    React { items: Vec<Reaction> },
    /// `count u8 (1..=64) | (target [32] | on u8)*` (SPEC 4.6).
    Follow { items: Vec<FollowEdge> },
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
            Self::SetRoute { .. } => TxKind::SetRoute,
            Self::ClearRoute { .. } => TxKind::ClearRoute,
            Self::RegisterModel(_) => TxKind::RegisterModel,
            Self::PostSegments { .. } => TxKind::PostSegments,
            Self::RejectResult { .. } => TxKind::RejectResult,
            Self::SetProfile(_) => TxKind::SetProfile,
            Self::OperatorLink { .. } => TxKind::OperatorLink,
            Self::Post(_) => TxKind::Post,
            Self::React { .. } => TxKind::React,
            Self::Follow { .. } => TxKind::Follow,
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
            Self::SetRoute { solana_recipient, route_bps, until_height, cap, release_key } => {
                w.bytes(solana_recipient).u16(*route_bps).u64(*until_height).u64(*cap).bytes(release_key);
            }
            Self::ClearRoute { account, route_nonce, sig } => {
                w.bytes(account).u64(*route_nonce).bytes(sig);
            }
            Self::RegisterModel(m) => m.write(w),
            Self::PostSegments { ticket_id, round, model_id, rank, rows_pattern, cols_pattern, segments } => {
                w.bytes(ticket_id).u16(*round).bytes(model_id).u16(*rank).bytes(rows_pattern).bytes(cols_pattern).u8(segments.len() as u8);
                for g in segments {
                    w.u16(g.matrix).u32(g.m).u32(g.k).u32(g.n).u32(g.max_blocks).bytes(&g.hash_a).bytes(&g.hash_b).bytes(&g.c_hash).bytes16(&g.a_uri);
                }
            }
            Self::RejectResult { job_id, deliverer, result_hash, sigs } => {
                w.bytes(job_id).bytes(deliverer).bytes(result_hash);
                write_sigs(w, sigs);
            }
            Self::SetProfile(p) => p.write(w),
            Self::OperatorLink { agent, add } => {
                w.bytes(agent).u8(*add as u8);
            }
            Self::Post(p) => p.write(w),
            Self::React { items } => write_reactions(w, items),
            Self::Follow { items } => write_follows(w, items),
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
            TxKind::SetRoute => Self::SetRoute {
                solana_recipient: r.arr()?,
                route_bps: r.u16()?,
                until_height: r.u64()?,
                cap: r.u64()?,
                release_key: r.arr()?,
            },
            TxKind::ClearRoute => Self::ClearRoute { account: r.arr()?, route_nonce: r.u64()?, sig: r.arr()? },
            TxKind::RegisterModel => Self::RegisterModel(RegisterModel::read(r)?),
            TxKind::PostSegments => {
                let ticket_id = r.arr()?;
                let round = r.u16()?;
                let model_id = r.arr()?;
                let rank = r.u16()?;
                let rows_pattern = r.arr()?;
                let cols_pattern = r.arr()?;
                let count = r.u8()? as usize;
                if count > MAX_SEGMENTS {
                    return Err(DecodeError::TooLarge("segments"));
                }
                let mut segments = Vec::with_capacity(count);
                for _ in 0..count {
                    segments.push(Segment {
                        matrix: r.u16()?,
                        m: r.u32()?,
                        k: r.u32()?,
                        n: r.u32()?,
                        max_blocks: r.u32()?,
                        hash_a: r.arr()?,
                        hash_b: r.arr()?,
                        c_hash: r.arr()?,
                        a_uri: r.bytes16(MAX_URI, "a_uri")?,
                    });
                }
                Self::PostSegments { ticket_id, round, model_id, rank, rows_pattern, cols_pattern, segments }
            }
            TxKind::RejectResult => Self::RejectResult {
                job_id: r.arr()?,
                deliverer: r.arr()?,
                result_hash: r.arr()?,
                sigs: read_sigs(r)?,
            },
            TxKind::SetProfile => Self::SetProfile(SetProfile::read(r)?),
            TxKind::OperatorLink => {
                let agent = r.arr()?;
                let add = match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::Invalid("operator link add flag")),
                };
                Self::OperatorLink { agent, add }
            }
            TxKind::Post => Self::Post(Post::read(r)?),
            TxKind::React => Self::React { items: read_reactions(r)? },
            TxKind::Follow => Self::Follow { items: read_follows(r)? },
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

/// Message a lien's release key signs to clear it early (ClearRoute):
/// `b"pearl-sol:route-clear:v1" || chain_id u32 LE || account [32] || route_nonce u64 LE`.
pub fn route_clear_message(chain_id: u32, account: &Pubkey, route_nonce: u64) -> Vec<u8> {
    let mut w = W::new();
    w.bytes(crate::DOMAIN_ROUTE_CLEAR).u32(chain_id).bytes(account).u64(route_nonce);
    w.into_inner()
}

/// Message evaluators sign to complete a job: `b"pearl-sol:result:v1" || job_id || deliverer || result_hash`.
pub fn result_message(job_id: &Hash, deliverer: &Pubkey, result_hash: &Hash) -> Vec<u8> {
    [DOMAIN_RESULT, &job_id[..], &deliverer[..], &result_hash[..]].concat()
}

/// Message evaluators sign to reject a delivery (RejectResult):
/// `b"pearl-sol:reject:v1" || job_id || deliverer || result_hash` (same layout as
/// [`result_message`], another domain, so a signature can never serve as the other).
pub fn reject_message(job_id: &Hash, deliverer: &Pubkey, result_hash: &Hash) -> Vec<u8> {
    [DOMAIN_REJECT, &job_id[..], &deliverer[..], &result_hash[..]].concat()
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
