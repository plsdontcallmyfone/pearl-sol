//! Header fetch tracking (C-U2): every header the node asks the resolver for is counted
//! here, capped, and expires.
//!
//! The resolver keeps a fetch until it is delivered or cancelled and asks a peer again
//! every 250 ms. Header fetches start from what peers say (a tip announcement, the parent
//! of a served header, the parent of a gossiped block), and a hash nobody holds is never
//! delivered. Untracked, such fetches and their request traffic only grow. The rules:
//!
//! - one entry per hash; asking again while it is tracked changes nothing (no refresh, so
//!   an entry always expires on time);
//! - an entry older than [`HEADER_FETCH_TIMEOUT_MS`] is cancelled; if the header is still
//!   needed, the next tip announcement or the repair path asks again;
//! - a peer holds at most [`MAX_HEADER_FETCHES_PER_PEER`] entries started by its own gossip;
//!   more are refused until some resolve or expire;
//! - at most [`MAX_HEADER_FETCHES`] entries in all; over it the oldest is cancelled.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash as StdHash;

use agent_types::Hash;

/// Header fetches outstanding at once. Honest backfill walks one parent at a time per
/// branch, so real use is a handful; the cap only bounds what a flood can hold.
pub const MAX_HEADER_FETCHES: usize = 1024;
/// Outstanding header fetches one peer's gossip (tips, orphan blocks) may start. An honest
/// peer announces one tip per interval and its fetches resolve in a round trip.
pub const MAX_HEADER_FETCHES_PER_PEER: usize = 16;
/// A header fetch not delivered after this long is cancelled: ten resolver request rounds
/// (2 s each), by which every connected peer has been asked more than once.
pub const HEADER_FETCH_TIMEOUT_MS: u64 = 20_000;

#[derive(Debug)]
struct Entry<P> {
    seq: u64,
    at_ms: u64,
    peer: Option<P>,
}

/// What [`HeaderFetches::want`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Want {
    /// Start the fetch.
    pub fetch: bool,
    /// Cancel this older fetch (the tracker was full).
    pub evicted: Option<Hash>,
}

#[derive(Debug)]
pub struct HeaderFetches<P> {
    asked: HashMap<Hash, Entry<P>>,
    /// Insertion order, oldest first; an entry is current while `asked` holds its seq.
    order: VecDeque<(u64, Hash)>,
    per_peer: HashMap<P, usize>,
    seq: u64,
}

impl<P> Default for HeaderFetches<P> {
    fn default() -> Self {
        Self { asked: HashMap::new(), order: VecDeque::new(), per_peer: HashMap::new(), seq: 0 }
    }
}

impl<P: Clone + Eq + StdHash> HeaderFetches<P> {
    /// The node wants header `h`. `peer` is the peer whose gossip named it (a tip, an orphan
    /// block's parent); `None` when the chain itself did (the parent of a served header).
    pub fn want(&mut self, h: Hash, peer: Option<&P>, now_ms: u64) -> Want {
        if self.asked.contains_key(&h) {
            return Want::default();
        }
        if let Some(p) = peer {
            if self.per_peer.get(p).copied().unwrap_or(0) >= MAX_HEADER_FETCHES_PER_PEER {
                return Want::default();
            }
        }
        let mut evicted = None;
        while self.asked.len() >= MAX_HEADER_FETCHES {
            let Some((seq, old)) = self.order.pop_front() else { break };
            if self.asked.get(&old).is_some_and(|e| e.seq == seq) {
                self.forget(&old);
                evicted = Some(old);
            }
        }
        if let Some(p) = peer {
            *self.per_peer.entry(p.clone()).or_insert(0) += 1;
        }
        self.seq += 1;
        self.asked.insert(h, Entry { seq: self.seq, at_ms: now_ms, peer: peer.cloned() });
        self.order.push_back((self.seq, h));
        if self.order.len() > 4 * MAX_HEADER_FETCHES {
            let asked = &self.asked;
            self.order.retain(|(seq, h)| asked.get(h).is_some_and(|e| e.seq == *seq));
        }
        Want { fetch: true, evicted }
    }

    /// Header `h` was delivered and accepted (or is not needed): stop tracking it.
    pub fn done(&mut self, h: &Hash) {
        self.forget(h);
    }

    /// Entries older than the timeout, removed: the caller cancels their fetches.
    pub fn expire(&mut self, now_ms: u64) -> Vec<Hash> {
        let mut out = Vec::new();
        while let Some(&(seq, h)) = self.order.front() {
            match self.asked.get(&h) {
                Some(e) if e.seq == seq => {
                    if now_ms.saturating_sub(e.at_ms) < HEADER_FETCH_TIMEOUT_MS {
                        break;
                    }
                    self.forget(&h);
                    out.push(h);
                }
                _ => {} // resolved or replaced since
            }
            self.order.pop_front();
        }
        out
    }

    fn forget(&mut self, h: &Hash) {
        let Some(e) = self.asked.remove(h) else { return };
        if let Some(p) = e.peer {
            if let Some(n) = self.per_peer.get_mut(&p) {
                *n -= 1;
                if *n == 0 {
                    self.per_peer.remove(&p);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.asked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    pub fn contains(&self, h: &Hash) -> bool {
        self.asked.contains_key(h)
    }

    /// Entries started by `peer`'s gossip.
    pub fn held_by(&self, peer: &P) -> usize {
        self.per_peer.get(peer).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u32) -> Hash {
        let mut b = [0u8; 32];
        b[..4].copy_from_slice(&i.to_be_bytes());
        b
    }

    /// C-U2: a peer announcing unknown tips as fast as it can holds a fixed number of
    /// fetches, and an honest peer's announcement is still fetched.
    #[test]
    fn tip_flood_from_one_peer_is_held_to_its_share() {
        let mut f = HeaderFetches::<u8>::default();
        let mut started = 0;
        for i in 0..100_000 {
            let w = f.want(id(i), Some(&66), 1_000 + i as u64 / 100);
            started += w.fetch as usize;
            assert_eq!(w.evicted, None);
        }
        assert_eq!(started, MAX_HEADER_FETCHES_PER_PEER);
        assert_eq!(f.len(), MAX_HEADER_FETCHES_PER_PEER);
        assert_eq!(f.held_by(&66), MAX_HEADER_FETCHES_PER_PEER);
        // the honest peer's tip and the parent the chain asks for next are fetched
        assert!(f.want(id(500_000), Some(&1), 2_000).fetch);
        assert!(f.want(id(500_001), None, 2_000).fetch);
        // delivered: the entries are gone, the flood's stay until they expire
        f.done(&id(500_000));
        f.done(&id(500_001));
        assert_eq!(f.len(), MAX_HEADER_FETCHES_PER_PEER);
    }

    /// C-U2: fetches the chain starts (parents of served headers) are capped too: over the
    /// cap the oldest is cancelled, so the newest wants always get a fetch.
    #[test]
    fn flood_of_parent_fetches_stays_under_the_cap() {
        let mut f = HeaderFetches::<u8>::default();
        let mut cancelled = Vec::new();
        for i in 0..10_000u32 {
            let w = f.want(id(i), None, 1_000);
            assert!(w.fetch);
            cancelled.extend(w.evicted);
            assert!(f.len() <= MAX_HEADER_FETCHES);
            assert!(f.order.len() <= 4 * MAX_HEADER_FETCHES);
        }
        assert_eq!(f.len(), MAX_HEADER_FETCHES);
        assert_eq!(cancelled, (0..10_000 - MAX_HEADER_FETCHES as u32).map(id).collect::<Vec<_>>(), "oldest first");
        assert!(f.contains(&id(9_999)) && !f.contains(&id(0)));
        // many peers at their share each: the total still stops at the cap
        let mut f = HeaderFetches::<u32>::default();
        for p in 0..1_000u32 {
            for i in 0..MAX_HEADER_FETCHES_PER_PEER as u32 {
                f.want(id(p * 100 + i), Some(&p), 1_000);
            }
        }
        assert_eq!(f.len(), MAX_HEADER_FETCHES);
        assert_eq!(f.per_peer.values().sum::<usize>(), MAX_HEADER_FETCHES);
    }

    /// C-U2: an entry nobody delivers expires on time, asking again does not keep it alive,
    /// and the peer's share is free again after it.
    #[test]
    fn entries_expire_and_cannot_be_pinned() {
        let mut f = HeaderFetches::<u8>::default();
        let t0 = 50_000;
        for i in 0..MAX_HEADER_FETCHES_PER_PEER as u32 {
            assert!(f.want(id(i), Some(&66), t0).fetch);
        }
        assert!(f.want(id(100), None, t0 + 5_000).fetch);
        // the peer names the same hashes again and again: no new fetch, no refresh
        for t in (0..HEADER_FETCH_TIMEOUT_MS).step_by(250) {
            for i in 0..MAX_HEADER_FETCHES_PER_PEER as u32 {
                assert!(!f.want(id(i), Some(&66), t0 + t).fetch);
            }
            assert!(f.expire(t0 + t).is_empty());
        }
        assert!(!f.want(id(999), Some(&66), t0 + HEADER_FETCH_TIMEOUT_MS - 1).fetch, "share full until expiry");
        let gone = f.expire(t0 + HEADER_FETCH_TIMEOUT_MS);
        assert_eq!(gone, (0..MAX_HEADER_FETCHES_PER_PEER as u32).map(id).collect::<Vec<_>>());
        assert_eq!(f.held_by(&66), 0);
        assert_eq!(f.len(), 1, "the younger entry stays");
        assert!(f.want(id(999), Some(&66), t0 + HEADER_FETCH_TIMEOUT_MS).fetch);
        assert_eq!(f.expire(t0 + 5_000 + HEADER_FETCH_TIMEOUT_MS), vec![id(100)]);
        // a hash delivered and wanted again later is tracked from its new time
        assert!(f.want(id(7), None, 200_000).fetch);
        assert_eq!(f.expire(200_000), vec![id(999)]);
        f.done(&id(7));
        assert!(f.want(id(7), None, 215_000).fetch);
        assert!(f.expire(200_000 + HEADER_FETCH_TIMEOUT_MS).is_empty());
        assert_eq!(f.expire(215_000 + HEADER_FETCH_TIMEOUT_MS), vec![id(7)]);
        assert!(f.is_empty() && f.order.is_empty());
    }
}
