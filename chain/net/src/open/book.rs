//! Address book: a scored table of known listen addresses, split into `new`
//! (heard about, never connected) and `tried` (we completed an outbound handshake).
//!
//! Simpler than Bitcoin's hashed buckets, but keeps the properties that matter here:
//! - bounded: `max_new` / `max_tried`, and one gossiping peer may own at most
//!   `max_per_source` new entries (so a single peer cannot flush the table);
//! - selection alternates tried / new, weights by failure count and skips entries
//!   in backoff, already in use, banned, or in a subnet we already dial out to;
//! - exponential backoff with jitter per address; entries that keep failing are
//!   dropped (seeds never are).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use commonware_cryptography::ed25519::PublicKey;

use super::wire::AddrRecord;
use super::Rand;

#[derive(Debug, Clone)]
pub struct Entry {
    pub addr: SocketAddr,
    pub key: Option<PublicKey>,
    /// From the seed list (never evicted).
    pub seed: bool,
    /// Key given in config (`key@host:port`): any other key is refused.
    pub pinned: bool,
    pub tried: bool,
    /// Peer that told us about it (None = config or self-reported by an inbound peer).
    pub source: Option<PublicKey>,
    pub last_seen_ms: u64,
    pub last_success: Option<SystemTime>,
    pub failures: u32,
    pub next_attempt: SystemTime,
    /// A dial or connection to it is in progress / open.
    pub in_use: bool,
}

#[derive(Debug, Clone)]
pub struct BookLimits {
    pub max_new: usize,
    pub max_tried: usize,
    pub max_per_source: usize,
    pub max_failures: u32,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

pub struct Book {
    entries: HashMap<SocketAddr, Entry>,
    selfs: HashSet<SocketAddr>,
    lim: BookLimits,
}

impl Book {
    pub fn new(lim: BookLimits) -> Self {
        Self { entries: HashMap::new(), selfs: HashSet::new(), lim }
    }

    pub fn get(&self, a: &SocketAddr) -> Option<&Entry> {
        self.entries.get(a)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn counts(&self) -> (usize, usize) {
        let tried = self.entries.values().filter(|e| e.tried).count();
        (self.entries.len() - tried, tried)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    pub fn is_self(&self, a: &SocketAddr) -> bool {
        self.selfs.contains(a)
    }

    pub fn add_seed(&mut self, addr: SocketAddr, key: Option<PublicKey>, now: SystemTime) {
        let pinned = key.is_some();
        self.entries.insert(
            addr,
            Entry {
                addr,
                key,
                seed: true,
                pinned,
                tried: false,
                source: None,
                last_seen_ms: 0,
                last_success: None,
                failures: 0,
                next_attempt: now,
                in_use: false,
            },
        );
    }

    /// Learn an address. Returns true if it was new to us.
    pub fn add(&mut self, addr: SocketAddr, key: PublicKey, source: Option<PublicKey>, last_seen_ms: u64, now: SystemTime) -> bool {
        if self.selfs.contains(&addr) {
            return false;
        }
        if let Some(e) = self.entries.get_mut(&addr) {
            e.last_seen_ms = e.last_seen_ms.max(last_seen_ms);
            if e.key.is_none() || (!e.pinned && !e.tried && e.key.as_ref() != Some(&key)) {
                e.key = Some(key);
            }
            return false;
        }
        let new_count = self.entries.values().filter(|e| !e.tried).count();
        if let Some(src) = &source {
            let owned = self.entries.values().filter(|e| !e.tried && e.source.as_ref() == Some(src)).count();
            if owned >= self.lim.max_per_source {
                // This source is at its cap: replace its own worst entry, never someone else's.
                let Some(victim) = self.worst_new(|e| e.source.as_ref() == Some(src)) else { return false };
                self.entries.remove(&victim);
            } else if new_count >= self.lim.max_new {
                let Some(victim) = self.worst_new(|_| true) else { return false };
                self.entries.remove(&victim);
            }
        } else if new_count >= self.lim.max_new {
            let Some(victim) = self.worst_new(|_| true) else { return false };
            self.entries.remove(&victim);
        }
        self.entries.insert(
            addr,
            Entry {
                addr,
                key: Some(key),
                seed: false,
                pinned: false,
                tried: false,
                source,
                last_seen_ms,
                last_success: None,
                failures: 0,
                next_attempt: now,
                in_use: false,
            },
        );
        true
    }

    fn worst_new(&self, f: impl Fn(&Entry) -> bool) -> Option<SocketAddr> {
        self.entries
            .values()
            .filter(|e| !e.tried && !e.seed && !e.in_use && f(e))
            .max_by_key(|e| (e.failures, u64::MAX - e.last_seen_ms))
            .map(|e| e.addr)
    }

    pub fn mark_attempt(&mut self, a: &SocketAddr) {
        if let Some(e) = self.entries.get_mut(a) {
            e.in_use = true;
        }
    }

    pub fn mark_success(&mut self, a: &SocketAddr, key: &PublicKey, now: SystemTime, now_ms: u64) {
        let promote = match self.entries.get(a) {
            Some(e) => !e.tried,
            None => false,
        };
        if promote && self.entries.values().filter(|e| e.tried).count() >= self.lim.max_tried {
            // Demote the tried entry that succeeded longest ago.
            if let Some(old) = self
                .entries
                .values()
                .filter(|e| e.tried && !e.seed && !e.in_use)
                .min_by_key(|e| e.last_success)
                .map(|e| e.addr)
            {
                if let Some(e) = self.entries.get_mut(&old) {
                    e.tried = false;
                }
            }
        }
        if let Some(e) = self.entries.get_mut(a) {
            e.tried = true;
            e.failures = 0;
            e.in_use = true;
            e.key = Some(key.clone());
            e.last_success = Some(now);
            e.last_seen_ms = now_ms;
        }
    }

    /// A dial failed: back off exponentially (with jitter), drop hopeless entries.
    pub fn mark_failed(&mut self, a: &SocketAddr, now: SystemTime, rng: &mut Rand) {
        let Some(e) = self.entries.get_mut(a) else { return };
        e.in_use = false;
        e.failures = e.failures.saturating_add(1);
        let limit = if e.tried { self.lim.max_failures * 2 } else { self.lim.max_failures };
        if !e.seed && e.failures >= limit {
            self.entries.remove(a);
            return;
        }
        let exp = self.lim.backoff_base.saturating_mul(1u32 << (e.failures - 1).min(20));
        let d = exp.min(self.lim.backoff_max);
        e.next_attempt = now + jitter(d, rng);
    }

    /// An open connection ended: retry after `delay`.
    pub fn mark_closed(&mut self, a: &SocketAddr, now: SystemTime, delay: Duration) {
        if let Some(e) = self.entries.get_mut(a) {
            e.in_use = false;
            e.next_attempt = now + delay;
        }
    }

    pub fn mark_self(&mut self, a: &SocketAddr) {
        self.entries.remove(a);
        self.selfs.insert(*a);
    }

    pub fn remove_key(&mut self, key: &PublicKey) {
        self.entries.retain(|_, e| e.seed || e.key.as_ref() != Some(key));
    }

    /// Pick an address to dial. `ok` filters by key / IP / subnet.
    pub fn select(&self, now: SystemTime, rng: &mut Rand, ok: impl Fn(&Entry) -> bool) -> Option<SocketAddr> {
        let usable: Vec<&Entry> = self.entries.values().filter(|e| !e.in_use && e.next_attempt <= now && ok(e)).collect();
        if usable.is_empty() {
            return None;
        }
        let tried: Vec<&Entry> = usable.iter().copied().filter(|e| e.tried || e.seed).collect();
        let new: Vec<&Entry> = usable.iter().copied().filter(|e| !(e.tried || e.seed)).collect();
        let class = match (tried.is_empty(), new.is_empty()) {
            (false, true) => &tried,
            (true, false) => &new,
            _ => {
                if rng.next() & 1 == 0 { &tried } else { &new }
            }
        };
        // Weight 1 / (1 + failures), in fixed point.
        let w: Vec<u64> = class.iter().map(|e| 1024 / (1 + e.failures as u64)).collect();
        let total: u64 = w.iter().sum();
        let mut pick = rng.next() % total.max(1);
        for (e, w) in class.iter().zip(w) {
            if pick < w {
                return Some(e.addr);
            }
            pick -= w;
        }
        class.last().map(|e| e.addr)
    }

    /// Up to `n` records for an Addr reply: tried first, then recently seen new
    /// entries that have not failed much. Shuffled.
    pub fn sample(&self, n: usize, rng: &mut Rand, exclude: impl Fn(&Entry) -> bool) -> Vec<AddrRecord> {
        let mut v: Vec<&Entry> =
            self.entries.values().filter(|e| e.key.is_some() && (e.tried || e.failures < 3) && !exclude(e)).collect();
        v.sort_by_key(|e| (!e.tried, u64::MAX - e.last_seen_ms, e.addr));
        v.truncate(n.saturating_mul(2));
        for i in (1..v.len()).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
        v.into_iter()
            .take(n)
            .map(|e| AddrRecord { key: e.key.clone().unwrap(), addr: e.addr, last_seen_ms: e.last_seen_ms })
            .collect()
    }
}

fn jitter(d: Duration, rng: &mut Rand) -> Duration {
    // 75% .. 125%
    let f = 750 + rng.next() % 501;
    d * f as u32 / 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::{ed25519, Signer as _};
    use std::time::UNIX_EPOCH;

    fn k(i: u64) -> PublicKey {
        ed25519::PrivateKey::from_seed(i).public_key()
    }
    fn a(i: u16) -> SocketAddr {
        SocketAddr::new([10, (i >> 8) as u8, i as u8, 1].into(), 1000 + i)
    }
    fn lim() -> BookLimits {
        BookLimits {
            max_new: 50,
            max_tried: 5,
            max_per_source: 10,
            max_failures: 3,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(8),
        }
    }

    #[test]
    fn per_source_cap_and_size_cap() {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let mut b = Book::new(lim());
        for i in 0..30 {
            b.add(a(i), k(i as u64), Some(k(999)), i as u64, now);
        }
        // one source owns at most 10 entries
        assert_eq!(b.len(), 10);
        for s in 0..10u64 {
            for i in 0..10u16 {
                b.add(a(1000 + s as u16 * 10 + i), k(5000 + i as u64), Some(k(s)), 1, now);
            }
        }
        assert!(b.len() <= 50, "{}", b.len());
    }

    #[test]
    fn backoff_and_drop() {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let mut r = Rand::new(1);
        let mut b = Book::new(lim());
        b.add(a(1), k(1), None, 0, now);
        b.add_seed(a(2), None, now);
        assert!(b.select(now, &mut r, |_| true).is_some());
        b.mark_attempt(&a(1));
        b.mark_failed(&a(1), now, &mut r);
        let e = b.get(&a(1)).unwrap();
        assert!(e.next_attempt > now && e.next_attempt <= now + Duration::from_millis(1250));
        b.mark_failed(&a(1), now, &mut r);
        b.mark_failed(&a(1), now, &mut r);
        assert!(b.get(&a(1)).is_none(), "dropped after max_failures");
        for _ in 0..10 {
            b.mark_failed(&a(2), now, &mut r);
        }
        let e = b.get(&a(2)).unwrap();
        assert!(e.next_attempt <= now + Duration::from_secs(10), "capped at backoff_max + jitter");
    }

    #[test]
    fn tried_promotion_and_cap() {
        let now = UNIX_EPOCH + Duration::from_secs(1000);
        let mut b = Book::new(lim());
        for i in 0..8 {
            b.add(a(i), k(i as u64), None, 0, now);
            b.mark_success(&a(i), &k(i as u64), now + Duration::from_secs(i as u64), 0);
            b.mark_closed(&a(i), now, Duration::ZERO);
        }
        assert!(b.counts().1 <= 6, "{:?}", b.counts());
        let mut r = Rand::new(2);
        let s = b.sample(3, &mut r, |_| false);
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn self_address_never_stored() {
        let now = UNIX_EPOCH;
        let mut b = Book::new(lim());
        b.add(a(1), k(1), None, 0, now);
        b.mark_self(&a(1));
        assert!(!b.add(a(1), k(1), None, 0, now));
        assert!(b.is_empty());
    }
}
