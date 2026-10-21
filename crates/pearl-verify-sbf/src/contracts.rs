//! Contract proofs for the Solana light client (docs/CONTRACTS.md "Proofs"): a block with
//! contract txs carries an 89-byte contracts record as a tx_root leaf (version 0x02, right
//! before the coinbase receipt). Under it, `events_root` and `receipts_root` commit to every
//! event and receipt of the block (same Merkle rule as tx_root). So an event a contract
//! emitted is proved with two paths and no state proof:
//!
//! 1. record under a FINAL header's tx_root (`verify_record`),
//! 2. event under the record's events_root (`verify_event`).
//!
//! Layouts: chain/agent-types/src/contracts.rs.

use crate::blake3;
use crate::chaintx::verify_tx_path;
use crate::header::Header;
use crate::Error;

pub const RECORD_LEN: usize = 89;
pub const RECORD_VERSION: u8 = 0x02;

/// The contracts record of one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub height: u64,
    pub receipt_count: u32,
    pub receipts_root: [u8; 32],
    pub event_count: u32,
    pub events_root: [u8; 32],
    pub gas_used: u64,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn a32(b: &[u8], at: usize) -> [u8; 32] {
    b[at..at + 32].try_into().unwrap()
}

pub fn record(b: &[u8]) -> Result<Record, Error> {
    if b.len() != RECORD_LEN || b[0] != RECORD_VERSION {
        return Err(Error::Tx);
    }
    Ok(Record {
        height: u64_at(b, 1),
        receipt_count: u32_at(b, 9),
        receipts_root: a32(b, 13),
        event_count: u32_at(b, 45),
        events_root: a32(b, 49),
        gas_used: u64_at(b, 81),
    })
}

/// The record is in the block of `header` and names its height.
pub fn verify_record(header: &Header, rec: &[u8], path: &[u8]) -> Result<Record, Error> {
    let r = record(rec)?;
    if r.height != header.height() {
        return Err(Error::Tx);
    }
    verify_tx_path(&blake3::hash(rec), path, &header.tx_root())?;
    Ok(r)
}

/// A parsed event: `contract [32] | tx_hash [32] | index u32 | count u8 | topic [32]* | data (u16 len)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventView<'a> {
    pub contract: [u8; 32],
    pub tx_hash: [u8; 32],
    pub index: u32,
    pub topics: &'a [u8],
    pub data: &'a [u8],
}

impl EventView<'_> {
    pub fn topic(&self, i: usize) -> Option<[u8; 32]> {
        self.topics.get(i * 32..i * 32 + 32).map(|t| t.try_into().unwrap())
    }
}

pub fn event(b: &[u8]) -> Result<EventView<'_>, Error> {
    if b.len() < 69 {
        return Err(Error::Tx);
    }
    let n = b[68] as usize;
    if n > 4 {
        return Err(Error::Tx);
    }
    let t_end = 69 + 32 * n;
    if b.len() < t_end + 2 {
        return Err(Error::Tx);
    }
    let dl = u16::from_le_bytes([b[t_end], b[t_end + 1]]) as usize;
    if b.len() != t_end + 2 + dl {
        return Err(Error::Tx);
    }
    Ok(EventView { contract: a32(b, 0), tx_hash: a32(b, 32), index: u32_at(b, 64), topics: &b[69..t_end], data: &b[t_end + 2..] })
}

/// An event under a header: the record by `record_path` under tx_root, the event by
/// `event_path` under the record's events_root.
pub fn verify_event<'a>(header: &Header, rec: &[u8], record_path: &[u8], ev: &'a [u8], event_path: &[u8]) -> Result<EventView<'a>, Error> {
    let r = verify_record(header, rec, record_path)?;
    let e = event(ev)?;
    verify_tx_path(&blake3::hash(ev), event_path, &r.events_root)?;
    Ok(e)
}

/// A contract tx's receipt under a header (status 0 = ok): `(tx_hash, status, gas_used)`.
pub fn verify_receipt(header: &Header, rec: &[u8], record_path: &[u8], receipt: &[u8], receipt_path: &[u8]) -> Result<([u8; 32], u8, u64), Error> {
    let r = verify_record(header, rec, record_path)?;
    // version 1 | tx_hash [32] | kind | status | gas_limit u64 | gas_used u64 | ...
    if receipt.len() < 1 + 32 + 2 + 16 || receipt[0] != 1 {
        return Err(Error::Tx);
    }
    verify_tx_path(&blake3::hash(receipt), receipt_path, &r.receipts_root)?;
    Ok((a32(receipt, 1), receipt[34], u64_at(receipt, 43)))
}
