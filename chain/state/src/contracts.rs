//! Contract records (docs/CONTRACTS.md, SPEC 4.9). Schema tags 40-48.
//!
//! - `CodeInfo ["code", code_hash]` (tag 40) and the code itself `["code-bytes", code_hash]`
//!   (tag 41, `tag || code`), indexed by `["code-index", i]` (tag 42).
//! - `ContractCounters ["contract-counters"]` (tag 43): codes, contracts.
//! - `Contract ["contract", address]` (tag 44), indexed by `["contract-index", i]` (tag 45).
//! - contract storage `["cstore", contract || key]` (tag 46): `tag || key_len u8 || key || value`
//!   (every stored value starts with the tag, so a full-state scan never mistakes a
//!   contract's bytes for another record).
//! - `["creceipt", tx_hash]` (tag 47): `tag || height u64 || receipt || count u16 || event*`.
//! - per-contract event index `["cevent", contract || n u64]` (tag 48): `tag || height u64 || tx_hash || index u32`.
//!
//! Supply: storage deposits (CodeInfo.deposit, Contract.deposit) count in the supply
//! invariant; a contract's CADRE balance is an ordinary Account record.

use agent_types::codec::{R, W};
use agent_types::contracts::{Event, Receipt};
use agent_types::{DecodeError, Hash, Pubkey};

use crate::schema::{key, Key};

pub mod tag {
    pub const CODE: u8 = 40;
    pub const CODE_BYTES: u8 = 41;
    pub const CODE_INDEX: u8 = 42;
    pub const COUNTERS: u8 = 43;
    pub const CONTRACT: u8 = 44;
    pub const CONTRACT_INDEX: u8 = 45;
    pub const STORE: u8 = 46;
    pub const RECEIPT: u8 = 47;
    pub const EVENT_INDEX: u8 = 48;
}

/// Bytes a stored item is charged for on top of its key and value (the record overhead).
pub const ITEM_OVERHEAD: u64 = 64;
/// Longest storage key a contract may use.
pub const MAX_KEY: usize = 128;
/// Longest storage value.
pub const MAX_VALUE: usize = 64 * 1024;

pub fn code_key(h: &Hash) -> Key {
    key(tag::CODE, h)
}
pub fn code_bytes_key(h: &Hash) -> Key {
    key(tag::CODE_BYTES, h)
}
pub fn code_index_key(i: u64) -> Key {
    key(tag::CODE_INDEX, &i.to_le_bytes())
}
pub fn counters_key() -> Key {
    key(tag::COUNTERS, &[])
}
pub fn contract_key(a: &Pubkey) -> Key {
    key(tag::CONTRACT, a)
}
pub fn contract_index_key(i: u64) -> Key {
    key(tag::CONTRACT_INDEX, &i.to_le_bytes())
}
pub fn store_key(contract: &Pubkey, k: &[u8]) -> Key {
    key(tag::STORE, &[&contract[..], k].concat())
}
pub fn receipt_key(tx_hash: &Hash) -> Key {
    key(tag::RECEIPT, tx_hash)
}
pub fn event_index_key(contract: &Pubkey, n: u64) -> Key {
    key(tag::EVENT_INDEX, &[&contract[..], &n.to_le_bytes()].concat())
}

fn expect(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

/// Deployed code (the bytes live under `code_bytes_key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeInfo {
    pub code_hash: Hash,
    pub deployer: Pubkey,
    pub height: u64,
    pub size: u32,
    /// Storage deposit for the code bytes, locked for good (code is never deleted).
    pub deposit: u64,
    pub instances: u64,
    pub index: u64,
}

impl CodeInfo {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::CODE)
            .bytes(&self.code_hash)
            .bytes(&self.deployer)
            .u64(self.height)
            .u32(self.size)
            .u64(self.deposit)
            .u64(self.instances)
            .u64(self.index);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::CODE)?;
        let c = Self {
            code_hash: r.arr()?,
            deployer: r.arr()?,
            height: r.u64()?,
            size: r.u32()?,
            deposit: r.u64()?,
            instances: r.u64()?,
            index: r.u64()?,
        };
        r.finish()?;
        Ok(c)
    }
}

pub fn encode_code_bytes(code: &[u8]) -> Vec<u8> {
    [&[tag::CODE_BYTES][..], code].concat()
}
pub fn decode_code_bytes(b: &[u8]) -> Result<&[u8], DecodeError> {
    match b.split_first() {
        Some((&tag::CODE_BYTES, rest)) => Ok(rest),
        _ => Err(DecodeError::Invalid("code bytes tag")),
    }
}

/// Index entries (`code-index`, `contract-index`): `tag || id [32]`.
pub fn encode_index(t: u8, id: &[u8; 32]) -> Vec<u8> {
    [&[t][..], id].concat()
}
pub fn decode_index(t: u8, b: &[u8]) -> Result<[u8; 32], DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, t)?;
    let id = r.arr()?;
    r.finish()?;
    Ok(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContractCounters {
    pub codes: u64,
    pub contracts: u64,
}

impl ContractCounters {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::COUNTERS).u64(self.codes).u64(self.contracts);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::COUNTERS)?;
        let c = Self { codes: r.u64()?, contracts: r.u64()? };
        r.finish()?;
        Ok(c)
    }
}

/// An instantiated contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub address: Pubkey,
    pub code_hash: Hash,
    /// Zero = immutable (no Migrate, no admin change).
    pub admin: Pubkey,
    pub creator: Pubkey,
    pub label: Vec<u8>,
    pub created_height: u64,
    pub migrated_height: u64,
    /// Storage charged for: sum of key + value + ITEM_OVERHEAD over its items.
    pub storage_bytes: u64,
    pub storage_items: u64,
    /// Deposit held for `storage_bytes` (refunded pro rata as storage shrinks).
    pub deposit: u64,
    pub index: u64,
    pub calls: u64,
    pub events: u64,
}

impl Contract {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::CONTRACT)
            .bytes(&self.address)
            .bytes(&self.code_hash)
            .bytes(&self.admin)
            .bytes(&self.creator)
            .bytes8(&self.label)
            .u64(self.created_height)
            .u64(self.migrated_height)
            .u64(self.storage_bytes)
            .u64(self.storage_items)
            .u64(self.deposit)
            .u64(self.index)
            .u64(self.calls)
            .u64(self.events);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::CONTRACT)?;
        let c = Self {
            address: r.arr()?,
            code_hash: r.arr()?,
            admin: r.arr()?,
            creator: r.arr()?,
            label: r.bytes8(agent_types::contracts::MAX_LABEL, "label")?,
            created_height: r.u64()?,
            migrated_height: r.u64()?,
            storage_bytes: r.u64()?,
            storage_items: r.u64()?,
            deposit: r.u64()?,
            index: r.u64()?,
            calls: r.u64()?,
            events: r.u64()?,
        };
        r.finish()?;
        Ok(c)
    }
}

/// Stored contract value: `tag || key_len u8 || key || value`.
pub fn encode_store(k: &[u8], v: &[u8]) -> Vec<u8> {
    let mut w = W::new();
    w.u8(tag::STORE).bytes8(k).bytes(v);
    w.into_inner()
}
/// `(key, value)` of a stored contract value.
pub fn decode_store(b: &[u8]) -> Result<(Vec<u8>, Vec<u8>), DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, tag::STORE)?;
    let k = r.bytes8(MAX_KEY, "store key")?;
    let v = r.take(r.remaining())?.to_vec();
    Ok((k, v))
}
/// Bytes an item is charged for.
pub fn item_bytes(k: &[u8], v: &[u8]) -> u64 {
    k.len() as u64 + v.len() as u64 + ITEM_OVERHEAD
}

/// A stored receipt with its events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReceipt {
    pub height: u64,
    pub receipt: Receipt,
    pub events: Vec<Event>,
}

impl StoredReceipt {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::RECEIPT).u64(self.height).bytes(&self.receipt.encode()).u16(self.events.len() as u16);
        for e in &self.events {
            w.bytes(&e.encode());
        }
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::RECEIPT)?;
        let height = r.u64()?;
        let receipt = Receipt::read(&mut r)?;
        let n = r.u16()? as usize;
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            events.push(Event::read(&mut r)?);
        }
        r.finish()?;
        Ok(Self { height, receipt, events })
    }
}

/// Per-contract event index entry: `(height, tx_hash, index in the tx)`.
pub fn encode_event_ref(height: u64, tx_hash: &Hash, index: u32) -> Vec<u8> {
    let mut w = W::new();
    w.u8(tag::EVENT_INDEX).u64(height).bytes(tx_hash).u32(index);
    w.into_inner()
}
pub fn decode_event_ref(b: &[u8]) -> Result<(u64, Hash, u32), DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, tag::EVENT_INDEX)?;
    let x = (r.u64()?, r.arr()?, r.u32()?);
    r.finish()?;
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let c = Contract {
            address: [1; 32],
            code_hash: [2; 32],
            admin: [0; 32],
            creator: [3; 32],
            label: b"counter".to_vec(),
            created_height: 5,
            migrated_height: 0,
            storage_bytes: 100,
            storage_items: 1,
            deposit: 10,
            index: 0,
            calls: 2,
            events: 1,
        };
        assert_eq!(Contract::decode(&c.encode()).unwrap(), c);
        let ci = CodeInfo { code_hash: [9; 32], deployer: [1; 32], height: 3, size: 1000, deposit: 7, instances: 1, index: 0 };
        assert_eq!(CodeInfo::decode(&ci.encode()).unwrap(), ci);
        let (k, v) = decode_store(&encode_store(b"key", &[1, 2, 3])).unwrap();
        assert_eq!((k.as_slice(), v.as_slice()), (&b"key"[..], &[1u8, 2, 3][..]));
        assert_eq!(decode_store(&encode_store(b"", &[])).unwrap(), (vec![], vec![]));
        // distinct from every core tag
        for t in [tag::CODE, tag::CODE_BYTES, tag::CONTRACT, tag::STORE, tag::RECEIPT] {
            assert!(t > crate::schema::tag::HANDLE);
        }
    }
}
