//! Orphan-chain repair (2026-10-22, devnet box 0 stall at 5762).
//!
//! A gossiped block whose parent is unknown is stored as an orphan and its parent is asked for
//! once (`Core::add_block` -> FetchHeader). Its children arrive as orphans too and each asks for
//! ITS parent, which is already a known orphan, so nothing asks again for the missing ancestor
//! at the root of the orphan chain; tip announcements of the orphan tip are ignored because
//! `is_known` counts orphans. If that single request is lost (or answered while the header
//! could not be kept), the node stays on its own branch until a restart. These helpers let the
//! node re-ask for the root's missing ancestor whenever a peer announces a tip it holds only
//! as an orphan or header (node policy, not consensus).

use agent_types::Hash;
use commonware_runtime::Spawner;

use super::{Chain, BODY_WINDOW};
use crate::verifier::PowVerifier;

/// What a node should ask for to connect a known but unconnected block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repair {
    /// The header (and then the body) of this missing ancestor.
    Header(Hash),
    /// The body of this ancestor, whose header is held but whose body never arrived.
    Body(Hash),
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Chain<E, V> {
    /// For a block held as an orphan or header only (not live): walk its ancestry through the
    /// orphan pool and the header-only entries to the first block that is neither live nor
    /// held, and say what to fetch. `None` if `h` is live, unknown, or its ancestry connects.
    pub fn repair_for(&self, h: &Hash) -> Option<Repair> {
        if self.live.contains_key(h) || !(self.orphans.contains_key(h) || self.headers_only.contains_key(h)) {
            return None;
        }
        let mut cur = *h;
        let mut last_header_only: Option<Hash> = None;
        for _ in 0..(BODY_WINDOW * 4) {
            if self.live.contains_key(&cur) {
                // the ancestry reaches a live block: the lowest header-only entry above it needs its body
                return last_header_only.map(Repair::Body);
            }
            if let Some(b) = self.orphans.get(&cur) {
                cur = b.header.prev_hash;
                continue;
            }
            if let Some(hd) = self.headers_only.get(&cur) {
                last_header_only = Some(cur);
                cur = hd.prev_hash;
                continue;
            }
            // neither live nor held: the missing ancestor (skip ones refused for good)
            if self.invalid.contains(&cur) || self.too_deep.contains(&cur) || cur == [0; 32] {
                return None;
            }
            return Some(Repair::Header(cur));
        }
        None
    }

    /// Hand a header-only entry's body out again (its earlier fetch may have been lost).
    pub fn reask_body(&mut self, h: &Hash) {
        self.body_asked.remove(h);
        if let Some(hd) = self.headers_only.get(h) {
            self.body_queue.insert((hd.height, *h));
        }
    }
}
