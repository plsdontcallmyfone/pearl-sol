//! Blob fetch scheduling: which wanted blobs are asked from peers right now.
//!
//! A node can want thousands of blobs at once (a fresh node, or one that comes back after
//! a while, wants every model segment and job input still referenced on chain). Asking for
//! all of them at once (up to 1024 before 2026-10-21) kept so many multi-MB answers in
//! flight that none arrived within the resolver's timeout; every key was re-asked about
//! every 2 s, the link to each peer stayed full, and the peers' block and tip gossip to the
//! node was dropped behind it: a synced node stopped following the tip while every peer
//! still listed it as connected (HANDOFF item 24). Now at most `window` fetches are in
//! flight, the rest wait in FIFO order, and a fetch with no answer after
//! [`BLOB_FETCH_STALE_BLOCKS`] blocks is cancelled and queued again at the back (no peer
//! may have that blob yet, and it must not hold a slot forever).

use std::collections::{HashMap, HashSet, VecDeque};

use blobs::BlobId;

/// Blob fetches in flight at once.
pub const BLOB_FETCH_WINDOW: usize = 16;
/// A fetch still unanswered after this many tip blocks is cancelled and queued again.
pub const BLOB_FETCH_STALE_BLOCKS: u64 = 5;

#[derive(Debug, Default)]
pub struct BlobFetches {
    queue: VecDeque<BlobId>,
    queued: HashSet<BlobId>,
    /// In flight, with the tip height when asked.
    inflight: HashMap<BlobId, u64>,
}

impl BlobFetches {
    /// Queue `id` (no-op if queued or in flight already).
    pub fn push(&mut self, id: BlobId) {
        if !self.inflight.contains_key(&id) && self.queued.insert(id) {
            self.queue.push_back(id);
        }
    }

    /// Forget `id` (arrived, stored by other means, or no longer wanted). Returns true if a
    /// fetch for it was in flight (the caller cancels it in the resolver).
    pub fn remove(&mut self, id: &BlobId) -> bool {
        self.queued.remove(id);
        self.inflight.remove(id).is_some()
    }

    /// At tip `height`: `(start, cancel)`. `cancel` are stale fetches (requeued at the back),
    /// `start` the fetches to issue now, at most `window` in flight. Cancels come first: a
    /// stale fetch handed out again in the same call is cancelled, then asked for afresh.
    pub fn pump(&mut self, height: u64, window: usize) -> (Vec<BlobId>, Vec<BlobId>) {
        let mut cancel: Vec<BlobId> = self
            .inflight
            .iter()
            .filter(|(_, asked)| height.saturating_sub(**asked) >= BLOB_FETCH_STALE_BLOCKS)
            .map(|(id, _)| *id)
            .collect();
        cancel.sort();
        for id in &cancel {
            self.inflight.remove(id);
            if self.queued.insert(*id) {
                self.queue.push_back(*id);
            }
        }
        let mut start = Vec::new();
        while self.inflight.len() < window {
            let Some(id) = self.queue.pop_front() else { break };
            if !self.queued.remove(&id) {
                continue; // removed while queued
            }
            self.inflight.insert(id, height);
            start.push(id);
        }
        (start, cancel)
    }

    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }

    pub fn queued(&self) -> usize {
        self.queued.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u32) -> BlobId {
        let mut b = [0u8; 32];
        b[..4].copy_from_slice(&i.to_be_bytes());
        b
    }

    #[test]
    fn thousands_wanted_never_more_than_the_window_in_flight() {
        let mut f = BlobFetches::default();
        for i in 0..2000 {
            f.push(id(i));
            f.push(id(i)); // duplicate wants are one fetch
        }
        let (start, cancel) = f.pump(100, BLOB_FETCH_WINDOW);
        assert_eq!(start.len(), BLOB_FETCH_WINDOW);
        assert!(cancel.is_empty());
        assert_eq!(start, (0..BLOB_FETCH_WINDOW as u32).map(id).collect::<Vec<_>>(), "FIFO order");
        // nothing more until one arrives
        assert!(f.pump(100, BLOB_FETCH_WINDOW).0.is_empty());
        let mut asked = start.len();
        let mut height = 100;
        while asked < 2000 {
            // every in-flight fetch answers before it goes stale
            for b in f.inflight.keys().copied().collect::<Vec<_>>() {
                assert!(f.remove(&b));
            }
            height += 1;
            let (s, c) = f.pump(height, BLOB_FETCH_WINDOW);
            assert!(c.is_empty());
            assert!(f.in_flight() <= BLOB_FETCH_WINDOW);
            asked += s.len();
        }
        assert_eq!(f.queued(), 0);
    }

    #[test]
    fn a_fetch_nobody_answers_gives_its_slot_back() {
        let mut f = BlobFetches::default();
        for i in 0..20 {
            f.push(id(i));
        }
        let (s, _) = f.pump(10, 4);
        assert_eq!(s, vec![id(0), id(1), id(2), id(3)]);
        // 1..3 answer, 0 never does
        for i in 1..4 {
            f.remove(&id(i));
        }
        let (s, c) = f.pump(11, 4);
        assert!(c.is_empty());
        assert_eq!(s, vec![id(4), id(5), id(6)]);
        // id(0) goes stale: cancelled and queued again behind the others
        let (s, c) = f.pump(10 + BLOB_FETCH_STALE_BLOCKS, 4);
        assert_eq!(c, vec![id(0)]);
        assert_eq!(s, vec![id(7)]);
        assert!(!f.inflight.contains_key(&id(0)));
        assert_eq!(*f.queue.back().unwrap(), id(0));
    }

    #[test]
    fn removed_while_queued_is_skipped() {
        let mut f = BlobFetches::default();
        f.push(id(1));
        f.push(id(2));
        assert!(!f.remove(&id(1)), "not in flight");
        let (s, _) = f.pump(0, 8);
        assert_eq!(s, vec![id(2)]);
        f.push(id(1));
        let (s, _) = f.pump(0, 8);
        assert_eq!(s, vec![id(1)]);
    }
}
