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
//!
//! Batch asks (a header and its ancestors in one answer, 2026-11-11) are tracked by
//! [`BatchAsks`] under their own, smaller bounds, and serving them by [`ServeBudget`].

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
    /// The peer whose gossip started the header walk this fetch continues (not counted
    /// against it here; batch asks of the walk are, in [`BatchAsks`]).
    walk: Option<P>,
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
        self.asked.insert(h, Entry { seq: self.seq, at_ms: now_ms, peer: peer.cloned(), walk: None });
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

    /// The tracked fetch of `h` continues a header walk started by `walk`'s gossip.
    pub fn set_walk(&mut self, h: &Hash, walk: Option<&P>) {
        if let Some(e) = self.asked.get_mut(h) {
            e.walk = walk.cloned();
        }
    }

    /// The peer whose gossip started the fetch of `h` or the walk it continues, if tracked.
    pub fn walk_of(&self, h: &Hash) -> Option<&P> {
        self.asked.get(h).and_then(|e| e.peer.as_ref().or(e.walk.as_ref()))
    }
}

/// Batch asks outstanding at once. A header walk has one outstanding at a time, so real use
/// is one or two; with the tracker full a walk goes on with single headers.
pub const MAX_BATCH_ASKS: usize = 32;
/// Outstanding batch asks of the walks one peer's gossip started.
pub const MAX_BATCH_ASKS_PER_PEER: usize = 2;
/// A batch ask not answered after this long is cancelled and the single header is asked for
/// instead: four resolver request rounds (2 s each). A peer that serves batches answers in
/// one; a peer from before batches says "do not have it" at once.
pub const BATCH_ASK_TIMEOUT_MS: u64 = 8_000;
/// After a batch ask of a peer's walks went unanswered, those walks use single headers and
/// try a batch again (next to the single header) no sooner than this. Among peers that never
/// serve batches that is one small extra request per interval and no waiting.
pub const BATCH_PROBE_INTERVAL_MS: u64 = 30_000;
/// Peers whose probe pause is remembered.
pub const MAX_BATCH_PAUSES: usize = 256;
/// A walk asks for batches only while the header it needs is more than this many blocks above
/// the node's tip. A batch always carries as many ancestors as fit, so a node a few blocks
/// behind (a restart, a short partition) would download far more than it lacks; it walks with
/// single headers as before.
pub const BATCH_MIN_GAP: u64 = 16;

/// What [`BatchAsks::want`] decided for the next header of a walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchWant {
    /// Ask for the batch that starts at the header.
    pub batch: bool,
    /// Ask for the single header (the path every peer serves).
    pub single: bool,
}

#[derive(Debug)]
struct BatchEntry<P> {
    at_ms: u64,
    peer: Option<P>,
}

/// Batch asks of the header walks: which are outstanding, and which walks get them.
///
/// A walk step that follows an accepted batch asks for the next batch only: a peer just served
/// this chain in batches. Any other step (the first below a tip, a step after a single header)
/// asks for the single header, and a batch ask rides along as a probe when the walk's peer has
/// none outstanding and is not paused. A batch ask nobody answers within
/// [`BATCH_ASK_TIMEOUT_MS`] is cancelled, the single header is asked for instead, and the
/// peer whose gossip started the walk is paused for [`BATCH_PROBE_INTERVAL_MS`]. So among peers
/// that never serve batches a node syncs exactly as before, one peer that does is enough to
/// switch, and a peer that starts walks nobody can serve pauses only its own.
///
/// Bounds: [`MAX_BATCH_ASKS`] entries, [`MAX_BATCH_ASKS_PER_PEER`] per peer whose gossip
/// started the walk (walks no peer started share one such allowance); an ask over a bound
/// falls back to the single header, which [`HeaderFetches`] bounds.
#[derive(Debug)]
pub struct BatchAsks<P> {
    enabled: bool,
    asked: HashMap<Hash, BatchEntry<P>>,
    per_peer: HashMap<Option<P>, usize>,
    /// No probe for this peer's walks before the time (ms).
    paused: HashMap<Option<P>, u64>,
}

impl<P> BatchAsks<P> {
    /// `enabled` false: every walk step is a single header and nothing is tracked (the
    /// behaviour before batches).
    pub fn new(enabled: bool) -> Self {
        Self { enabled, asked: HashMap::new(), per_peer: HashMap::new(), paused: HashMap::new() }
    }
}

impl<P: Clone + Eq + StdHash> BatchAsks<P> {
    /// A walk needs header `h` next. `peer` is the peer whose gossip started the walk, when
    /// known; `after_batch` says the step follows an accepted batch.
    pub fn want(&mut self, h: Hash, peer: Option<&P>, after_batch: bool, now_ms: u64) -> BatchWant {
        if !self.enabled {
            return BatchWant { batch: false, single: true };
        }
        if self.asked.contains_key(&h) {
            return BatchWant::default();
        }
        let origin = peer.cloned();
        let held = self.per_peer.get(&origin).copied().unwrap_or(0);
        let room = self.asked.len() < MAX_BATCH_ASKS && held < MAX_BATCH_ASKS_PER_PEER;
        let batch = if after_batch { room } else { room && held == 0 && self.paused.get(&origin).is_none_or(|until| now_ms >= *until) };
        if batch {
            *self.per_peer.entry(origin.clone()).or_insert(0) += 1;
            self.asked.insert(h, BatchEntry { at_ms: now_ms, peer: origin });
        }
        BatchWant { batch, single: !(after_batch && batch) }
    }

    /// The batch asked for at `h` was delivered and accepted. Returns whether it was tracked
    /// and the peer whose gossip started the walk.
    pub fn delivered(&mut self, h: &Hash) -> Option<Option<P>> {
        let e = self.forget(h)?;
        self.paused.remove(&e.peer);
        Some(e.peer)
    }

    /// Batch asks older than the timeout, removed, oldest first, each with the peer whose
    /// gossip started its walk: the caller cancels them and asks for the single headers. Those
    /// peers are paused.
    pub fn expire(&mut self, now_ms: u64) -> Vec<(Hash, Option<P>)> {
        let mut out: Vec<(u64, Hash)> = self.asked.iter().filter(|(_, e)| now_ms.saturating_sub(e.at_ms) >= BATCH_ASK_TIMEOUT_MS).map(|(h, e)| (e.at_ms, *h)).collect();
        if out.is_empty() {
            return Vec::new();
        }
        out.sort_unstable();
        self.paused.retain(|_, until| *until > now_ms);
        let mut gone = Vec::with_capacity(out.len());
        for (_, h) in out {
            if let Some(e) = self.forget(&h) {
                if self.paused.len() < MAX_BATCH_PAUSES {
                    self.paused.insert(e.peer.clone(), now_ms + BATCH_PROBE_INTERVAL_MS);
                }
                gone.push((h, e.peer));
            }
        }
        gone
    }

    fn forget(&mut self, h: &Hash) -> Option<BatchEntry<P>> {
        let e = self.asked.remove(h)?;
        if let Some(n) = self.per_peer.get_mut(&e.peer) {
            *n -= 1;
            if *n == 0 {
                self.per_peer.remove(&e.peer);
            }
        }
        Some(e)
    }

    pub fn contains(&self, h: &Hash) -> bool {
        self.asked.contains_key(h)
    }

    pub fn len(&self) -> usize {
        self.asked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    /// Outstanding batch asks of the walks `peer`'s gossip started (`None`: no peer's).
    pub fn held_by(&self, peer: Option<&P>) -> usize {
        self.per_peer.get(&peer.cloned()).copied().unwrap_or(0)
    }

    /// Is `peer` paused at `now_ms` (its walks get no batch probe)?
    pub fn is_paused(&self, peer: Option<&P>, now_ms: u64) -> bool {
        self.paused.get(&peer.cloned()).is_some_and(|until| now_ms < *until)
    }
}

/// Batches served at once before the budget runs dry.
pub const BATCH_SERVE_BURST: u32 = 8;
/// One more batch may be served every this many milliseconds.
pub const BATCH_SERVE_REFILL_MS: u64 = 250;

/// Budget for serving header batches, over all peers (a serve request does not say who asks).
/// One batch reads up to 512 blocks from the archive for a 32-byte request, so the node serves
/// [`BATCH_SERVE_BURST`] at once and one every [`BATCH_SERVE_REFILL_MS`] after that. A request
/// over the budget gets "do not have it" and the asker falls back to single headers. A syncing
/// node asks for one batch at a time, so honest use is a few per second.
#[derive(Debug)]
pub struct ServeBudget {
    tokens: u32,
    at_ms: u64,
}

impl Default for ServeBudget {
    fn default() -> Self {
        Self { tokens: BATCH_SERVE_BURST, at_ms: 0 }
    }
}

impl ServeBudget {
    fn refill(&mut self, now_ms: u64) {
        let add = now_ms.saturating_sub(self.at_ms) / BATCH_SERVE_REFILL_MS;
        if add > 0 {
            self.tokens = (self.tokens as u64 + add).min(BATCH_SERVE_BURST as u64) as u32;
            self.at_ms = if self.tokens == BATCH_SERVE_BURST { now_ms } else { self.at_ms + add * BATCH_SERVE_REFILL_MS };
        }
    }

    pub fn available(&mut self, now_ms: u64) -> bool {
        self.refill(now_ms);
        self.tokens > 0
    }

    /// Spend one serve; false if the budget is empty.
    pub fn take(&mut self, now_ms: u64) -> bool {
        if !self.available(now_ms) {
            return false;
        }
        self.tokens -= 1;
        true
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

    /// Among peers that never answer a batch ask a walk asks for the single header at every
    /// step, and one batch ask per probe interval rides along.
    #[test]
    fn unanswered_batch_asks_leave_the_single_path_running() {
        let mut b = BatchAsks::<u8>::new(true);
        let t0 = 10_000;
        // the first step asks for both
        assert_eq!(b.want(id(1), Some(&1), false, t0), BatchWant { batch: true, single: true });
        // the single headers arrive: every next step asks for one, and no second probe while
        // the first is out
        for i in 2..50 {
            assert_eq!(b.want(id(i), Some(&1), false, t0 + i as u64), BatchWant { batch: false, single: true });
        }
        assert_eq!(b.len(), 1);
        assert!(b.expire(t0 + BATCH_ASK_TIMEOUT_MS - 1).is_empty());
        assert_eq!(b.expire(t0 + BATCH_ASK_TIMEOUT_MS), vec![(id(1), Some(1))]);
        assert!(b.is_empty());
        // no new batch ask until the probe interval has passed
        let t1 = t0 + BATCH_ASK_TIMEOUT_MS;
        assert!(b.is_paused(Some(&1), t1));
        let mut batches = 0;
        for i in 0..1_000u32 {
            let w = b.want(id(100 + i), Some(&1), false, t1 + i as u64 * 20);
            assert!(w.single);
            batches += w.batch as usize;
            b.expire(t1 + i as u64 * 20);
        }
        assert_eq!(batches, 0, "20 s of walking, inside the probe interval");
        assert_eq!(b.want(id(5_000), Some(&1), false, t1 + BATCH_PROBE_INTERVAL_MS), BatchWant { batch: true, single: true });
        // switched off: single headers, nothing tracked
        let mut off = BatchAsks::<u8>::new(false);
        assert_eq!(off.want(id(1), None, false, t0), BatchWant { batch: false, single: true });
        assert_eq!(off.want(id(2), None, true, t0), BatchWant { batch: false, single: true });
        assert!(off.is_empty());
    }

    /// A step after an accepted batch is one batch ask. The tracker holds at most
    /// MAX_BATCH_ASKS, at most MAX_BATCH_ASKS_PER_PEER per peer, and an ask over a bound is a
    /// single header. A peer whose walks nobody serves pauses its own probes, not another's.
    #[test]
    fn batch_asks_are_capped_per_peer_and_in_all_and_expire() {
        let mut b = BatchAsks::<u8>::new(true);
        let t0 = 10_000;
        assert_eq!(b.want(id(1), Some(&7), false, t0), BatchWant { batch: true, single: true });
        assert_eq!(b.delivered(&id(1)), Some(Some(7)));
        assert_eq!(b.delivered(&id(1)), None, "not tracked any more");
        assert_eq!(b.want(id(2), Some(&7), true, t0), BatchWant { batch: true, single: false });
        assert_eq!(b.delivered(&id(2)), Some(Some(7)));
        // one peer's walks after batches: two batch asks, the rest single headers
        let mut batch = 0;
        for i in 0..1_000u32 {
            let w = b.want(id(10 + i), Some(&66), true, t0 + 1);
            assert!(w.batch != w.single, "exactly one of the two after a batch");
            batch += w.batch as usize;
        }
        assert_eq!(batch, MAX_BATCH_ASKS_PER_PEER);
        assert_eq!(b.held_by(Some(&66)), MAX_BATCH_ASKS_PER_PEER);
        // probes: one at a time per peer, however many walks it starts
        let mut probes = 0;
        for i in 0..1_000u32 {
            probes += b.want(id(2_000 + i), Some(&67), false, t0 + 1).batch as usize;
        }
        assert_eq!(probes, 1);
        // asking again for a tracked one starts nothing
        assert_eq!(b.want(id(10), Some(&66), true, t0 + 2), BatchWant::default());
        // many peers, and walks no peer started: the total stops at the cap
        for p in 100..200u8 {
            b.want(id(5_000 + p as u32), Some(&p), true, t0 + 3);
            b.want(id(6_000 + p as u32), Some(&p), true, t0 + 3);
            b.want(id(7_000 + p as u32), None, true, t0 + 3);
        }
        assert_eq!(b.len(), MAX_BATCH_ASKS);
        assert_eq!(b.held_by(None), MAX_BATCH_ASKS_PER_PEER);
        assert_eq!(b.want(id(9_999), Some(&250), true, t0 + 4), BatchWant { batch: false, single: true });
        // nobody answers: every entry is gone at the timeout and the shares are free
        assert!(b.expire(t0 + BATCH_ASK_TIMEOUT_MS).is_empty(), "the oldest is 1 ms short of the timeout");
        assert_eq!(b.expire(t0 + 3 + BATCH_ASK_TIMEOUT_MS).len(), MAX_BATCH_ASKS);
        assert!(b.is_empty() && b.per_peer.is_empty());
        assert!(b.paused.len() <= MAX_BATCH_PAUSES);
        // the peers whose asks went unanswered are paused; a peer that had none is not, and
        // gets its probe
        let t1 = t0 + 3 + BATCH_ASK_TIMEOUT_MS;
        assert!(b.is_paused(Some(&66), t1) && b.is_paused(Some(&67), t1) && b.is_paused(None, t1));
        assert_eq!(b.want(id(20_000), Some(&66), false, t1), BatchWant { batch: false, single: true });
        assert!(!b.is_paused(Some(&7), t1));
        assert_eq!(b.want(id(20_001), Some(&7), false, t1), BatchWant { batch: true, single: true });
        // a step after a batch is not held back by the pause (a batch was just served)
        assert_eq!(b.want(id(20_002), Some(&66), true, t1), BatchWant { batch: true, single: false });
        // the pause ends
        assert!(!b.is_paused(Some(&67), t1 + BATCH_PROBE_INTERVAL_MS));
        assert!(b.want(id(20_003), Some(&67), false, t1 + BATCH_PROBE_INTERVAL_MS).batch);
    }

    /// The serve budget: a burst, then one batch per refill interval, however many are asked.
    #[test]
    fn batch_serves_are_budgeted() {
        let mut s = ServeBudget::default();
        let t0 = 1_000_000;
        let mut served = 0;
        for _ in 0..10_000 {
            served += s.take(t0) as u32;
        }
        assert_eq!(served, BATCH_SERVE_BURST);
        // 10 s of requests every millisecond
        let mut served = 0;
        for ms in 1..=10_000u64 {
            served += s.take(t0 + ms) as u64;
        }
        assert_eq!(served, 10_000 / BATCH_SERVE_REFILL_MS);
        // idle: the burst is back, and no more than the burst
        assert!(s.available(t0 + 10_000 + 3_600_000));
        let mut served = 0;
        for _ in 0..100 {
            served += s.take(t0 + 10_000 + 3_600_000) as u32;
        }
        assert_eq!(served, BATCH_SERVE_BURST);
    }
}
