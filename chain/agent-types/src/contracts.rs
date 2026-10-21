//! Contract tx bodies, receipts, events and the per-block contracts record (docs/CONTRACTS.md,
//! SPEC 4.7).
//!
//! Kinds 30-34: DeployCode, Instantiate, Call, Migrate, SetContractAdmin. They are valid from
//! [`contracts_upgrade_height`] (devnet) or genesis (every other chain).
//!
//! Every block with at least one contract tx carries a contracts record (version 0x02, 89
//! bytes) committed as a `tx_root` leaf right before the coinbase receipt:
//!
//! ```text
//! off len field
//!   0   1 version u8 = 0x02 (a tx starts with 0x01, a coinbase receipt with 0x00)
//!   1   8 height u64
//!   9   4 receipt_count u32   (contract txs in the block)
//!  13  32 receipts_root [32]  (merkle.rs tree over blake3(receipt) in block order)
//!  45   4 event_count u32
//!  49  32 events_root [32]    (merkle.rs tree over blake3(event) in block order)
//!  81   8 gas_used u64        (sum over the block's contract txs)
//! = 89 bytes
//! ```
//! So the Solana light client proves a contract event (or a call's receipt) with two
//! inclusion paths: the record under a final header's `tx_root`, then the event under the
//! record's `events_root`. The header itself is unchanged.

use crate::codec::{R, W};
use crate::{blake3_hash, merkle, DecodeError, Hash, Pubkey};

/// Codec cap of a DeployCode's code; the executor applies the smaller config `max_code_size`.
pub const MAX_CODE_SIZE: usize = 256 * 1024;
/// A DeployCode tx may be this large (code + header + signature); other kinds keep MAX_TX_SIZE.
pub const MAX_DEPLOY_TX_SIZE: usize = MAX_CODE_SIZE + 1024;
/// Call / Instantiate / Migrate arguments.
pub const MAX_ARGS: usize = 8192;
/// Instantiate label (shown by explorers).
pub const MAX_LABEL: usize = 64;
/// Return data kept in a receipt.
pub const MAX_RETURN: usize = 4096;
/// Event limits.
pub const MAX_TOPICS: usize = 4;
pub const MAX_EVENT_DATA: usize = 4096;
/// Error message kept in a failed receipt.
pub const MAX_ERROR: usize = 200;

pub const RECORD_VERSION: u8 = 0x02;
pub const RECORD_LEN: usize = 89;
pub const RECEIPT_VERSION: u8 = 1;

/// Cadre public devnet chain id (deploy/genesis.json).
pub const DEVNET_CHAIN_ID: u32 = 1_128_350_802;
/// Coordinated activation height of the contract kinds and config keys 27-34 on the public
/// devnet; every other chain has them from genesis.
pub const DEVNET_CONTRACTS_HEIGHT: u64 = 6000;

/// First height at which the contract kinds and config keys 27-34 are valid on `chain_id`.
pub fn contracts_upgrade_height(chain_id: u32) -> u64 {
    match chain_id {
        DEVNET_CHAIN_ID => DEVNET_CONTRACTS_HEIGHT,
        _ => 0,
    }
}

/// `code_hash = blake3(code)`.
pub fn code_hash(code: &[u8]) -> Hash {
    blake3_hash(&[code])
}

/// Contract address: `blake3("pearl-sol:contract:v1" || creator || nonce u64 LE)` (the
/// Instantiate tx's sender and nonce). No ed25519 key controls it.
pub fn contract_address(creator: &Pubkey, nonce: u64) -> Pubkey {
    blake3_hash(&[b"pearl-sol:contract:v1", creator, &nonce.to_le_bytes()])
}

/// Topic of a named event: `blake3("pearl-sol:event:v1" || name)` (the SDK's `event!`).
pub fn event_topic(name: &[u8]) -> Hash {
    blake3_hash(&[b"pearl-sol:event:v1", name])
}

/// DeployCode (kind 30): `gas_limit u64 | code (u32 len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployCode {
    pub gas_limit: u64,
    pub code: Vec<u8>,
}

/// Instantiate (kind 31): `code_hash [32] | admin [32] | value u64 | gas_limit u64
/// | label (u8 len) | args (u16 len)`. `admin` zero = the contract can never be migrated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instantiate {
    pub code_hash: Hash,
    pub admin: Pubkey,
    pub value: u64,
    pub gas_limit: u64,
    pub label: Vec<u8>,
    pub args: Vec<u8>,
}

/// Call (kind 32): `contract [32] | value u64 | gas_limit u64 | args (u16 len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub contract: Pubkey,
    pub value: u64,
    pub gas_limit: u64,
    pub args: Vec<u8>,
}

/// Migrate (kind 33, admin only): `contract [32] | code_hash [32] | gas_limit u64 | args (u16 len)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrate {
    pub contract: Pubkey,
    pub code_hash: Hash,
    pub gas_limit: u64,
    pub args: Vec<u8>,
}

impl DeployCode {
    pub fn write(&self, w: &mut W) {
        w.u64(self.gas_limit).u32(self.code.len() as u32).bytes(&self.code);
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let gas_limit = r.u64()?;
        let n = r.u32()? as usize;
        if n > MAX_CODE_SIZE {
            return Err(DecodeError::TooLarge("code"));
        }
        Ok(Self { gas_limit, code: r.take(n)?.to_vec() })
    }
}

impl Instantiate {
    pub fn write(&self, w: &mut W) {
        w.bytes(&self.code_hash).bytes(&self.admin).u64(self.value).u64(self.gas_limit).bytes8(&self.label).bytes16(&self.args);
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            code_hash: r.arr()?,
            admin: r.arr()?,
            value: r.u64()?,
            gas_limit: r.u64()?,
            label: r.bytes8(MAX_LABEL, "label")?,
            args: r.bytes16(MAX_ARGS, "args")?,
        })
    }
}

impl Call {
    pub fn write(&self, w: &mut W) {
        w.bytes(&self.contract).u64(self.value).u64(self.gas_limit).bytes16(&self.args);
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self { contract: r.arr()?, value: r.u64()?, gas_limit: r.u64()?, args: r.bytes16(MAX_ARGS, "args")? })
    }
}

impl Migrate {
    pub fn write(&self, w: &mut W) {
        w.bytes(&self.contract).bytes(&self.code_hash).u64(self.gas_limit).bytes16(&self.args);
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self { contract: r.arr()?, code_hash: r.arr()?, gas_limit: r.u64()?, args: r.bytes16(MAX_ARGS, "args")? })
    }
}

/// One event a contract emitted (`emit` host call), in block order.
///
/// `contract [32] | tx_hash [32] | index u32 (in its tx) | count u8 (0..=4) | topic [32]* | data (u16 len)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub contract: Pubkey,
    pub tx_hash: Hash,
    pub index: u32,
    pub topics: Vec<Hash>,
    pub data: Vec<u8>,
}

impl Event {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.bytes(&self.contract).bytes(&self.tx_hash).u32(self.index).u8(self.topics.len() as u8);
        for t in &self.topics {
            w.bytes(t);
        }
        w.bytes16(&self.data);
        w.into_inner()
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let contract = r.arr()?;
        let tx_hash = r.arr()?;
        let index = r.u32()?;
        let n = r.u8()? as usize;
        if n > MAX_TOPICS {
            return Err(DecodeError::TooLarge("topics"));
        }
        let mut topics = Vec::with_capacity(n);
        for _ in 0..n {
            topics.push(r.arr()?);
        }
        let data = r.bytes16(MAX_EVENT_DATA, "event data")?;
        Ok(Self { contract, tx_hash, index, topics, data })
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let e = Self::read(&mut r)?;
        r.finish()?;
        Ok(e)
    }
    /// The value the events tree takes as this leaf: `blake3(event)`.
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }
}

/// Outcome of a contract tx.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    /// The contract called `revert` (or a callee's failure was passed up).
    Reverted = 1,
    OutOfGas = 2,
    /// Wasm trap (unreachable, out-of-bounds memory, stack overflow, ...) or a host-call misuse.
    Trapped = 3,
    /// Nothing ran: unknown contract or code, missing entry point, not the admin, value
    /// or storage deposit not covered.
    Refused = 4,
}

impl Status {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Ok,
            1 => Self::Reverted,
            2 => Self::OutOfGas,
            3 => Self::Trapped,
            4 => Self::Refused,
            _ => return Err(DecodeError::Invalid("receipt status")),
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Reverted => "reverted",
            Self::OutOfGas => "out_of_gas",
            Self::Trapped => "trapped",
            Self::Refused => "refused",
        }
    }
}

/// Receipt of one contract tx (a receipts_root leaf is `blake3(receipt)`).
///
/// ```text
/// version u8 (=1) | tx_hash [32] | kind u8 | status u8 | gas_limit u64 | gas_used u64
/// | fee_charged u64 | contract [32] | code_hash [32] | event_count u16
/// | return (u16 len, <= 4096) | error (u8 len, <= 200)
/// ```
/// `contract` is the called or created contract (zero for DeployCode); `code_hash` the code
/// that ran (or was deployed). On failure every state change of the tx except the fee and
/// the nonce is undone, and `event_count` is 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub tx_hash: Hash,
    pub kind: u8,
    pub status: Status,
    pub gas_limit: u64,
    pub gas_used: u64,
    pub fee_charged: u64,
    pub contract: Pubkey,
    pub code_hash: Hash,
    pub event_count: u16,
    pub ret: Vec<u8>,
    pub error: Vec<u8>,
}

impl Receipt {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(RECEIPT_VERSION)
            .bytes(&self.tx_hash)
            .u8(self.kind)
            .u8(self.status as u8)
            .u64(self.gas_limit)
            .u64(self.gas_used)
            .u64(self.fee_charged)
            .bytes(&self.contract)
            .bytes(&self.code_hash)
            .u16(self.event_count)
            .bytes16(&self.ret)
            .bytes8(&self.error);
        w.into_inner()
    }
    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let v = r.u8()?;
        if v != RECEIPT_VERSION {
            return Err(DecodeError::Version(v as u32));
        }
        Ok(Self {
            tx_hash: r.arr()?,
            kind: r.u8()?,
            status: Status::from_u8(r.u8()?)?,
            gas_limit: r.u64()?,
            gas_used: r.u64()?,
            fee_charged: r.u64()?,
            contract: r.arr()?,
            code_hash: r.arr()?,
            event_count: r.u16()?,
            ret: r.bytes16(MAX_RETURN, "return")?,
            error: r.bytes8(MAX_ERROR, "error")?,
        })
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let x = Self::read(&mut r)?;
        r.finish()?;
        Ok(x)
    }
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }
}

/// The per-block contracts record (module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContractsRecord {
    pub height: u64,
    pub receipt_count: u32,
    pub receipts_root: Hash,
    pub event_count: u32,
    pub events_root: Hash,
    pub gas_used: u64,
}

impl ContractsRecord {
    pub fn build(height: u64, receipts: &[Receipt], events: &[Event]) -> Self {
        let rh: Vec<Hash> = receipts.iter().map(Receipt::hash).collect();
        let eh: Vec<Hash> = events.iter().map(Event::hash).collect();
        Self {
            height,
            receipt_count: receipts.len() as u32,
            receipts_root: merkle::root(&rh),
            event_count: events.len() as u32,
            events_root: merkle::root(&eh),
            gas_used: receipts.iter().map(|r| r.gas_used).sum(),
        }
    }
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut w = W::new();
        w.u8(RECORD_VERSION)
            .u64(self.height)
            .u32(self.receipt_count)
            .bytes(&self.receipts_root)
            .u32(self.event_count)
            .bytes(&self.events_root)
            .u64(self.gas_used);
        w.into_inner().try_into().expect("89 bytes")
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        let v = r.u8()?;
        if v != RECORD_VERSION {
            return Err(DecodeError::Version(v as u32));
        }
        let x = Self {
            height: r.u64()?,
            receipt_count: r.u32()?,
            receipts_root: r.arr()?,
            event_count: r.u32()?,
            events_root: r.arr()?,
            gas_used: r.u64()?,
        };
        r.finish()?;
        Ok(x)
    }
    /// The value the tx_root tree takes as this leaf: `blake3(record)`.
    pub fn hash(&self) -> Hash {
        blake3_hash(&[&self.encode()])
    }
}

/// Merkle path (33-byte entries `sibling [32] || sibling_is_left u8`, as the Solana verifier
/// takes them) of item `index` among `hashes`.
pub fn path_bytes(hashes: &[Hash], index: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for (sib, left) in merkle::proof(hashes, index) {
        out.extend_from_slice(&sib);
        out.push(left as u8);
    }
    out
}

/// Check a 33-byte-entry path for `leaf_value` (a blake3 of an item) against `root`.
pub fn verify_path(leaf_value: &Hash, path: &[u8], root: &Hash) -> bool {
    if path.len() % 33 != 0 {
        return false;
    }
    let p: Vec<(Hash, bool)> = path.chunks(33).map(|c| (c[..32].try_into().unwrap(), c[32] == 1)).collect();
    merkle::verify(leaf_value, &p, root)
}

/// Light-client style check of an event: `record` is under `tx_root` by `record_path`, and
/// `event` is under the record's `events_root` by `event_path`.
pub fn verify_event(tx_root: &Hash, record: &[u8], record_path: &[u8], event: &[u8], event_path: &[u8]) -> bool {
    let Ok(rec) = ContractsRecord::decode(record) else { return false };
    if !verify_path(&rec.hash(), record_path, tx_root) {
        return false;
    }
    if Event::decode(event).is_err() {
        return false;
    }
    verify_path(&blake3_hash(&[event]), event_path, &rec.events_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_and_roundtrip() {
        let ev = Event { contract: [1; 32], tx_hash: [2; 32], index: 0, topics: vec![event_topic(b"Transfer")], data: vec![9; 10] };
        let rc = Receipt {
            tx_hash: [2; 32],
            kind: 32,
            status: Status::Ok,
            gas_limit: 100,
            gas_used: 50,
            fee_charged: 1050,
            contract: [1; 32],
            code_hash: [3; 32],
            event_count: 1,
            ret: vec![1, 2],
            error: vec![],
        };
        assert_eq!(Event::decode(&ev.encode()).unwrap(), ev);
        assert_eq!(Receipt::decode(&rc.encode()).unwrap(), rc);
        let rec = ContractsRecord::build(7, std::slice::from_ref(&rc), std::slice::from_ref(&ev));
        let b = rec.encode();
        assert_eq!(b.len(), RECORD_LEN);
        assert_eq!(b[0], RECORD_VERSION);
        assert_eq!(ContractsRecord::decode(&b).unwrap(), rec);
        assert_eq!(rec.gas_used, 50);
        // one event: its path is empty and the events root is its leaf
        assert!(verify_path(&ev.hash(), &[], &rec.events_root));
    }

    #[test]
    fn addresses_are_distinct() {
        assert_ne!(contract_address(&[1; 32], 0), contract_address(&[1; 32], 1));
        assert_ne!(contract_address(&[1; 32], 0), contract_address(&[2; 32], 0));
        assert_eq!(contracts_upgrade_height(7), 0);
        assert_eq!(contracts_upgrade_height(DEVNET_CHAIN_ID), DEVNET_CONTRACTS_HEIGHT);
    }
}
