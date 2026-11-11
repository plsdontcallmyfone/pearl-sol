//! Header batches for sync (2026-11-11). Node-local networking, not consensus.
//!
//! Headers-first backfill used to walk one header per round trip from a peer's tip down to
//! the node's own chain: a fresh devnet node needed 67 minutes for about 17,000 headers
//! (docs/measurements/2026-11-10-readiness-drills.md, B1). A batch is the same walk in bulk:
//! the header a node asked for and its ancestors, newest first, each with its proof.
//!
//! A batch is bound to the hash that was asked for: entry 0 must hash to it and every next
//! entry must hash to the `prev_hash` of the one before, so a peer cannot answer with headers
//! of its own choosing. Each header then goes through [`Chain::insert_header`], the same
//! checks in the same order (tip first) as a walk of single headers: proof hash, target
//! limit, proof of work where it can be checked, and the anchoring rules (C-U1).
//!
//! Bounds: at most [`MAX_HEADER_BATCH`] entries and [`MAX_HEADER_BATCH_BYTES`] bytes (one
//! entry may exceed the byte bound alone, as it does when served singly).

use agent_types::block::{decode_header_with_proof, encode_header_with_proof};
use agent_types::{Hash, Header};
use commonware_runtime::Spawner;

use super::{invalid, Chain, ChainError, HeaderAccept, BODY_WINDOW};
use crate::verifier::PowVerifier;

/// Most headers in one batch.
pub const MAX_HEADER_BATCH: usize = 512;
/// Most bytes in one batch of more than one header. The reply shares the headers channel
/// with single headers and waits on the same 2 s request timeout, and a devnet header with
/// its proof is about 25 KB, so a batch there is about 40 headers.
pub const MAX_HEADER_BATCH_BYTES: usize = 1 << 20;
/// First bytes of a batch. A single header payload starts with the header version.
pub const HEADER_BATCH_MAGIC: [u8; 4] = *b"CHB1";
const BATCH_PREFIX: usize = 6;

/// What [`Chain::insert_header_batch`] did with a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeaderBatchAccept {
    /// Headers newly held.
    pub stored: usize,
    /// Headers the node already had.
    pub known: usize,
    /// Highest and lowest height among the newly held ones (0 if none).
    pub top_height: u64,
    pub low_height: u64,
    /// The header to ask for next and its height: the missing ancestor below the batch, if
    /// any.
    pub need_parent: Option<(Hash, u64)>,
}

/// `magic || count u16 LE || count x (len u32 LE || header || proof)`.
pub fn encode_header_batch(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut v = Vec::with_capacity(BATCH_PREFIX + entries.iter().map(|e| e.len() + 4).sum::<usize>());
    v.extend_from_slice(&HEADER_BATCH_MAGIC);
    v.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in entries {
        v.extend_from_slice(&(e.len() as u32).to_le_bytes());
        v.extend_from_slice(e);
    }
    v
}

/// Does this sync payload claim to be a batch?
pub fn is_header_batch(b: &[u8]) -> bool {
    b.len() >= HEADER_BATCH_MAGIC.len() && b[..HEADER_BATCH_MAGIC.len()] == HEADER_BATCH_MAGIC
}

/// Decode a batch, newest header first. The count and byte bounds are checked before any
/// entry is read; nothing here touches the chain.
pub fn decode_header_batch(b: &[u8]) -> Result<Vec<(Header, Vec<u8>)>, ChainError> {
    if b.len() < BATCH_PREFIX || !is_header_batch(b) {
        return invalid("header batch: bad prefix");
    }
    let count = u16::from_le_bytes([b[4], b[5]]) as usize;
    if count == 0 || count > MAX_HEADER_BATCH {
        return invalid(format!("header batch: {count} entries"));
    }
    if count > 1 && b.len() > MAX_HEADER_BATCH_BYTES {
        return invalid(format!("header batch: {} bytes", b.len()));
    }
    let mut out = Vec::with_capacity(count);
    let mut at = BATCH_PREFIX;
    for _ in 0..count {
        if b.len() - at < 4 {
            return invalid("header batch: truncated");
        }
        let len = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        if b.len() - at < len {
            return invalid("header batch: truncated");
        }
        out.push(decode_header_with_proof(&b[at..at + len]).map_err(|e| ChainError::Invalid(format!("header batch: {e}")))?);
        at += len;
    }
    if at != b.len() {
        return invalid("header batch: trailing bytes");
    }
    Ok(out)
}

/// Is `batch` one chain that starts at `anchor` and walks down parent by parent?
pub fn check_header_links(anchor: &Hash, batch: &[(Header, Vec<u8>)]) -> Result<(), ChainError> {
    if batch.is_empty() || batch.len() > MAX_HEADER_BATCH {
        return invalid(format!("header batch: {} entries", batch.len()));
    }
    let mut expect = *anchor;
    let mut above: Option<u64> = None;
    for (i, (h, _)) in batch.iter().enumerate() {
        if h.hash() != expect {
            return invalid(if i == 0 { "header batch: does not start at the header asked for".to_string() } else { format!("header batch: entry {i} is not the parent of entry {}", i - 1) });
        }
        if above.is_some_and(|a| h.height.checked_add(1) != Some(a)) {
            return invalid(format!("header batch: entry {i} height is not one below entry {}", i - 1));
        }
        above = Some(h.height);
        expect = h.prev_hash;
    }
    Ok(())
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Chain<E, V> {
    /// Serve a batch: the header of `anchor` and its ancestors down to height 1, newest first,
    /// at most `max` entries and `max_bytes` bytes (the first entry always fits). `None` if
    /// the node does not hold `anchor` with its proof.
    pub async fn header_batch(&self, anchor: &Hash, max: usize, max_bytes: usize) -> Result<Option<Vec<u8>>, ChainError> {
        let mut entries: Vec<Vec<u8>> = Vec::new();
        let mut bytes = BATCH_PREFIX;
        let mut cur = *anchor;
        while entries.len() < max.min(MAX_HEADER_BATCH) {
            let Some(b) = self.get_block(&cur).await? else { break };
            if b.header.height == 0 {
                break; // genesis: every node of this chain has it
            }
            let e = encode_header_with_proof(&b.header, &b.proof);
            if !entries.is_empty() && bytes + 4 + e.len() > max_bytes {
                break;
            }
            bytes += 4 + e.len();
            cur = b.header.prev_hash;
            entries.push(e);
        }
        Ok(if entries.is_empty() { None } else { Some(encode_header_batch(&entries)) })
    }

    /// Take a batch delivered for `anchor`. Nothing is stored unless the whole batch is one
    /// chain down from `anchor` ([`check_header_links`]). Then every header is checked and
    /// held by [`Chain::insert_header`], tip first, exactly as single deliveries would be; the
    /// walk stops where it meets the node's own chain or goes below finality. An invalid
    /// header fails the batch (the headers above it were valid and stay held).
    pub async fn insert_header_batch(&mut self, anchor: &Hash, batch: Vec<(Header, Vec<u8>)>) -> Result<HeaderBatchAccept, ChainError> {
        check_header_links(anchor, &batch)?;
        let mut acc = HeaderBatchAccept::default();
        let mut lowest: Option<Hash> = None;
        for (header, proof) in batch {
            let hash = header.hash();
            if self.live.contains_key(&hash) || self.is_canonical(&hash, header.height) {
                break; // the node's own chain from here down
            }
            match self.insert_header(header, proof).await {
                Ok(HeaderAccept::Stored { .. }) => {
                    if acc.stored == 0 {
                        acc.top_height = header.height;
                    }
                    acc.stored += 1;
                    acc.low_height = header.height;
                }
                Ok(HeaderAccept::Known) => acc.known += 1,
                Err(ChainError::TooDeep) => break,
                Err(e) => return Err(e),
            }
            lowest = Some(hash);
            if self.live.contains_key(&header.prev_hash) {
                break;
            }
        }
        // the missing ancestor below what is held now (None once the walk connects)
        if let Some(l) = lowest {
            acc.need_parent = self.missing_below(&l);
        }
        Ok(acc)
    }

    /// Walk down from held entry `from` through held entries (header-only or orphan) to the
    /// first block that is neither held nor live: its hash and height. `None` if the ancestry
    /// reaches a live block or a refused one. Bounded like [`Chain::repair_for`].
    fn missing_below(&self, from: &Hash) -> Option<(Hash, u64)> {
        let mut cur = *from;
        let mut height = None;
        for _ in 0..(BODY_WINDOW * 4) {
            if self.live.contains_key(&cur) {
                return None;
            }
            let held = self.orphans.get(&cur).map(|b| &b.header).or_else(|| self.headers_only.get(&cur));
            let Some(hd) = held else {
                if self.invalid.contains(&cur) || self.too_deep.contains(&cur) || cur == [0; 32] {
                    return None;
                }
                return height.map(|h| (cur, h));
            };
            height = Some(hd.height.saturating_sub(1));
            cur = hd.prev_hash;
        }
        None
    }

    /// Highest header-only entry whose ancestry reaches a live block: how far the bodies have
    /// to go. `None` when no body is wanted.
    pub fn anchored_header_top(&self) -> Option<u64> {
        let queued = self.body_queue.last().map(|(h, _)| *h);
        let asked = self.body_asked.keys().filter_map(|h| self.headers_only.get(h).map(|hd| hd.height)).max();
        queued.max(asked)
    }
}
