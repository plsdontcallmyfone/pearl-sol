//! State shared by the network actor, connection tasks and the [`super::Oracle`].

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use commonware_cryptography::ed25519::PublicKey;
use commonware_p2p::{PeerSetUpdate, TrackedPeers};
use commonware_runtime::{Handle, IoBuf, IoBufs};
use commonware_utils::channel::{mpsc, ring};
use commonware_utils::ordered::Set;
use tracing::{debug, info, warn};

use super::book::{Book, BookLimits};
use super::limits;
use super::wire::{self, AddrRecord, Msg};
use super::{Config, Rand};

pub(crate) struct PeerEntry {
    pub conn_id: u64,
    pub inbound: bool,
    pub remote: SocketAddr,
    /// Address it listens on (dialed address for outbound, Hello for inbound).
    pub listen: Option<SocketAddr>,
    pub queue: futures::channel::mpsc::Sender<IoBufs>,
    /// Bulk lane (see `Config::bulk_queue_size`).
    pub bulk: futures::channel::mpsc::Sender<IoBufs>,
    /// Bytes queued on this peer's bulk lane (see `Config::bulk_peer_bytes`).
    pub bulk_bytes: Arc<AtomicUsize>,
    pub handle: Option<Handle<()>>,
    pub last_getaddr: Option<SystemTime>,
    pub addr_tokens: f64,
    pub addr_refill: SystemTime,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Counters {
    pub bans: u64,
    pub inbound_full: u64,
    pub inbound_refused: u64,
    pub dials: u64,
    pub dial_failures: u64,
    pub rate_dropped: u64,
    pub queue_dropped: u64,
    pub bulk_dropped: u64,
    /// Inbound bulk frames dropped because the local consumer's channel was full.
    pub bulk_in_dropped: u64,
}

/// Subtract up to `n` from `a` (never below zero).
pub(crate) fn release(a: &AtomicUsize, n: usize) -> usize {
    let mut took = 0;
    let _ = a.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
        took = v.min(n);
        Some(v - took)
    });
    took
}

pub(crate) struct Shared {
    pub me: PublicKey,
    pub cfg: Arc<Config>,
    pub peers: BTreeMap<PublicKey, PeerEntry>,
    pub pending_in: usize,
    pub pending_in_ip: HashMap<IpAddr, usize>,
    pub pending_out: usize,
    pub book: Book,
    pub bans: HashMap<PublicKey, SystemTime>,
    pub ip_bans: HashMap<IpAddr, SystemTime>,
    pub scores: HashMap<PublicKey, u32>,
    pub set_index: u64,
    pub set_subs: Vec<mpsc::UnboundedSender<PeerSetUpdate<PublicKey>>>,
    pub conn_rings: Vec<ring::Sender<Vec<PublicKey>>>,
    pub blocked_rings: Vec<ring::Sender<Set<PublicKey>>>,
    pub next_conn: u64,
    pub rng: Rand,
    pub counters: Counters,
    /// Bytes queued on every peer's bulk lane together (see `Config::bulk_total_bytes`).
    pub bulk_total: Arc<AtomicUsize>,
}

pub(crate) fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Shared {
    pub fn new(cfg: Arc<Config>, seed: u64, now: SystemTime) -> Self {
        let mut book = Book::new(BookLimits {
            max_new: cfg.max_new,
            max_tried: cfg.max_tried,
            max_per_source: cfg.max_per_source,
            max_failures: cfg.max_failures,
            backoff_base: cfg.backoff_base,
            backoff_max: cfg.backoff_max,
        });
        for s in &cfg.seeds {
            book.add_seed(s.addr, s.key.clone(), now);
        }
        for (a, k) in &cfg.known {
            if limits::dialable(a, cfg.allow_private) {
                book.add(*a, k.clone(), None, 0, now);
            }
        }
        if let Some(a) = cfg.advertised() {
            book.mark_self(&a);
        }
        Self {
            me: cfg.public_key(),
            cfg,
            peers: BTreeMap::new(),
            pending_in: 0,
            pending_in_ip: HashMap::new(),
            pending_out: 0,
            book,
            bans: HashMap::new(),
            ip_bans: HashMap::new(),
            scores: HashMap::new(),
            set_index: 0,
            set_subs: Vec::new(),
            conn_rings: Vec::new(),
            blocked_rings: Vec::new(),
            next_conn: 0,
            rng: Rand::new(seed),
            counters: Counters::default(),
            bulk_total: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn is_banned(&mut self, k: &PublicKey, now: SystemTime) -> bool {
        match self.bans.get(k) {
            Some(until) if *until > now => true,
            Some(_) => {
                self.bans.remove(k);
                self.scores.remove(k);
                false
            }
            None => false,
        }
    }

    pub fn ip_banned(&mut self, ip: IpAddr, now: SystemTime) -> bool {
        let ip = limits::canonical(ip);
        match self.ip_bans.get(&ip) {
            Some(until) if *until > now => true,
            Some(_) => {
                self.ip_bans.remove(&ip);
                false
            }
            None => false,
        }
    }

    /// Bouncer: may this key finish a handshake with us?
    pub fn admit_key(&mut self, k: &PublicKey, now: SystemTime) -> bool {
        *k != self.me && !self.is_banned(k, now)
    }

    pub fn inbound_count(&self) -> usize {
        self.peers.values().filter(|p| p.inbound).count()
    }

    pub fn outbound_count(&self) -> usize {
        self.peers.values().filter(|p| !p.inbound).count()
    }

    pub fn exempt(&self, ip: IpAddr) -> bool {
        limits::exempt(ip, self.cfg.allow_private)
    }

    /// Pre-handshake check for an accepted TCP connection. Reserves a pending slot.
    pub fn precheck_inbound(&mut self, remote: SocketAddr, now: SystemTime) -> Result<(), &'static str> {
        let ip = limits::canonical(remote.ip());
        if self.ip_banned(ip, now) {
            return Err("ip banned");
        }
        if self.pending_in + self.pending_out >= self.cfg.max_pending_handshakes {
            return Err("too many pending handshakes");
        }
        if !self.exempt(ip) {
            let same_ip = self.peers.values().filter(|p| p.inbound && limits::canonical(p.remote.ip()) == ip).count()
                + self.pending_in_ip.get(&ip).copied().unwrap_or(0);
            if same_ip >= self.cfg.max_inbound_per_ip {
                return Err("per-ip limit");
            }
            let sn = limits::subnet(ip);
            let same_net = self.peers.values().filter(|p| p.inbound && limits::subnet(p.remote.ip()) == sn).count()
                + self.pending_in_ip.iter().filter(|(i, _)| limits::subnet(**i) == sn).map(|(_, n)| *n).sum::<usize>();
            if same_net >= self.cfg.max_inbound_per_subnet {
                return Err("per-subnet limit");
            }
        }
        self.pending_in += 1;
        *self.pending_in_ip.entry(ip).or_default() += 1;
        Ok(())
    }

    pub fn release_inbound(&mut self, remote: SocketAddr) {
        let ip = limits::canonical(remote.ip());
        self.pending_in = self.pending_in.saturating_sub(1);
        if let Some(n) = self.pending_in_ip.get_mut(&ip) {
            *n -= 1;
            if *n == 0 {
                self.pending_in_ip.remove(&ip);
            }
        }
    }

    pub fn connected(&self) -> Vec<PublicKey> {
        self.peers.keys().cloned().collect()
    }

    /// Publish the connected set to Provider subscribers and limited senders.
    pub fn changed(&mut self) {
        self.set_index += 1;
        let set = Set::from_iter_dedup(self.peers.keys().cloned());
        let tracked = TrackedPeers::primary(set);
        let upd = PeerSetUpdate { index: self.set_index, latest: tracked.clone(), all: tracked };
        self.set_subs.retain(|s| s.send(upd.clone()).is_ok());
        let v = self.connected();
        self.conn_rings.retain(|r| !r.is_closed());
        for r in &self.conn_rings {
            let _ = r.send_lossy(v.clone());
        }
    }

    pub fn current_update(&self) -> PeerSetUpdate<PublicKey> {
        let tracked = TrackedPeers::primary(Set::from_iter_dedup(self.peers.keys().cloned()));
        PeerSetUpdate { index: self.set_index, latest: tracked.clone(), all: tracked }
    }

    pub fn banned_set(&self, now: SystemTime) -> Set<PublicKey> {
        Set::from_iter_dedup(self.bans.iter().filter(|(_, u)| **u > now).map(|(k, _)| k.clone()))
    }

    /// Drop the connection (if `conn_id` matches, or any if None). Returns true if removed.
    pub fn remove_peer(&mut self, k: &PublicKey, conn_id: Option<u64>, now: SystemTime) -> bool {
        let matches = match (self.peers.get(k), conn_id) {
            (Some(p), Some(id)) => p.conn_id == id,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !matches {
            return false;
        }
        let p = self.peers.remove(k).unwrap();
        if let Some(h) = &p.handle {
            h.abort();
        }
        // its queued bulk frames go with the connection
        let left = p.bulk_bytes.swap(0, Ordering::SeqCst);
        release(&self.bulk_total, left);
        if !p.inbound {
            if let Some(a) = p.listen {
                let d = self.cfg.backoff_base;
                self.book.mark_closed(&a, now, d);
            }
        }
        debug!(peer = ?k, inbound = p.inbound, "peer disconnected");
        self.changed();
        true
    }

    /// Add `score` to a peer's ban score. Returns true if it is now banned.
    pub fn misbehave(&mut self, k: &PublicKey, score: u32, reason: &str, now: SystemTime) -> bool {
        if self.is_banned(k, now) {
            return true;
        }
        let s = self.scores.entry(k.clone()).or_default();
        *s = s.saturating_add(score);
        let total = *s;
        if total < self.cfg.ban_threshold {
            debug!(peer = ?k, score = total, reason, "misbehaviour");
            return false;
        }
        self.ban(k, reason, now);
        true
    }

    pub fn ban(&mut self, k: &PublicKey, reason: &str, now: SystemTime) {
        if *k == self.me {
            return;
        }
        let until = now + self.cfg.ban_duration;
        self.bans.insert(k.clone(), until);
        self.counters.bans += 1;
        let remote = self.peers.get(k).map(|p| p.remote);
        if let Some(r) = remote {
            let ip = limits::canonical(r.ip());
            if !self.exempt(ip) {
                self.ip_bans.insert(ip, until);
            }
        }
        warn!(peer = ?k, reason, "peer banned");
        self.remove_peer(k, None, now);
        self.book.remove_key(k);
        let set = self.banned_set(now);
        self.blocked_rings.retain(|r| !r.is_closed());
        for r in &self.blocked_rings {
            let _ = r.send_lossy(set.clone());
        }
    }

    /// Queue a control message to a peer (dropped if its queue is full).
    pub fn send_ctrl(&mut self, k: &PublicKey, m: &Msg) -> bool {
        match self.peers.get_mut(k) {
            Some(p) => p.queue.try_send(IoBufs::from(IoBuf::from(wire::encode(m)))).is_ok(),
            None => false,
        }
    }

    /// Records for an Addr reply to `to`: the book plus connected peers' listen
    /// addresses, never `to` itself.
    pub fn addr_sample(&mut self, to: &PublicKey, n: usize, now: SystemTime) -> Vec<AddrRecord> {
        let now_ms = ms(now);
        let mut out: Vec<AddrRecord> = Vec::new();
        for (k, p) in &self.peers {
            if k == to {
                continue;
            }
            if let Some(a) = p.listen {
                out.push(AddrRecord { key: k.clone(), addr: a, last_seen_ms: now_ms });
            }
        }
        if let Some(a) = self.cfg.advertised() {
            out.push(AddrRecord { key: self.me.clone(), addr: a, last_seen_ms: now_ms });
        }
        let mut rng = self.rng.clone();
        let more = self.book.sample(n, &mut rng, |e| e.key.as_ref() == Some(to));
        self.rng = rng;
        for r in more {
            if !out.iter().any(|o| o.addr == r.addr) {
                out.push(r);
            }
        }
        out.truncate(n.min(wire::MAX_ADDRS));
        out
    }

    pub fn on_hello(&mut self, k: &PublicKey, conn_id: u64, version: u32, listen: Option<SocketAddr>, now: SystemTime) -> bool {
        if version != wire::VERSION {
            info!(peer = ?k, version, "peer speaks another protocol version");
            return false;
        }
        let allow = self.cfg.allow_private;
        let Some(p) = self.peers.get_mut(k) else { return true };
        if p.conn_id != conn_id || !p.inbound {
            return true;
        }
        if let Some(a) = listen {
            // Only trust a self-reported address on the IP we see the peer on.
            let same_ip = limits::canonical(a.ip()) == limits::canonical(p.remote.ip());
            if limits::dialable(&a, allow) && (same_ip || limits::exempt(p.remote.ip(), allow)) {
                p.listen = Some(a);
                self.book.add(a, k.clone(), None, ms(now), now);
            }
        }
        true
    }

    pub fn on_getaddr(&mut self, k: &PublicKey, now: SystemTime) {
        let every = self.cfg.gossip_interval;
        let n = self.cfg.addr_reply;
        let Some(p) = self.peers.get_mut(k) else { return };
        if let Some(t) = p.last_getaddr {
            if now < t + every {
                return; // answer at most once per gossip interval per connection
            }
        }
        p.last_getaddr = Some(now);
        let list = self.addr_sample(k, n, now);
        self.send_ctrl(k, &Msg::Addr(list));
    }

    /// Learn addresses from a peer. Returns the records worth relaying.
    pub fn on_addr(&mut self, from: &PublicKey, list: Vec<AddrRecord>, now: SystemTime) -> Vec<AddrRecord> {
        let (burst, refill) = (self.cfg.addr_burst, self.cfg.addr_refill_per_s);
        let allow = self.cfg.allow_private;
        let Some(p) = self.peers.get_mut(from) else { return Vec::new() };
        let dt = now.duration_since(p.addr_refill).unwrap_or(Duration::ZERO).as_secs_f64();
        p.addr_tokens = (p.addr_tokens + dt * refill).min(burst);
        p.addr_refill = now;
        let small = list.len() <= 10;
        let mut fresh = Vec::new();
        let now_ms = ms(now);
        for r in list {
            let p = self.peers.get_mut(from).unwrap();
            if p.addr_tokens < 1.0 {
                break; // over the address rate: ignore the rest (Bitcoin does the same)
            }
            p.addr_tokens -= 1.0;
            if r.key == self.me || !limits::dialable(&r.addr, allow) || self.book.is_self(&r.addr) {
                continue;
            }
            if self.bans.contains_key(&r.key) {
                continue;
            }
            let seen = r.last_seen_ms.min(now_ms);
            let src = if r.key == *from { None } else { Some(from.clone()) };
            let is_new = self.book.add(r.addr, r.key.clone(), src, seen, now);
            if small && is_new && now_ms.saturating_sub(seen) < 10 * 60 * 1000 {
                fresh.push(r);
            }
        }
        fresh
    }

    /// Relay fresh records to up to two random peers other than `from`.
    pub fn relay(&mut self, from: &PublicKey, recs: Vec<AddrRecord>) {
        if recs.is_empty() {
            return;
        }
        let mut others: Vec<PublicKey> = self.peers.keys().filter(|k| *k != from).cloned().collect();
        for _ in 0..2 {
            if others.is_empty() {
                break;
            }
            let i = (self.rng.next() % others.len() as u64) as usize;
            let k = others.swap_remove(i);
            let list: Vec<AddrRecord> = recs.iter().filter(|r| r.key != k).cloned().collect();
            if !list.is_empty() {
                self.send_ctrl(&k, &Msg::Addr(list));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::{ed25519, Signer as _};

    fn shared(allow_private: bool) -> Shared {
        let mut c = Config::recommended(ed25519::PrivateKey::from_seed(1), b"t", "1.1.1.1:1".parse().unwrap());
        c.allow_private = allow_private;
        c.max_inbound_per_ip = 2;
        c.max_inbound_per_subnet = 3;
        c.max_pending_handshakes = 10;
        Shared::new(Arc::new(c), 7, UNIX_EPOCH)
    }

    #[test]
    fn per_ip_and_subnet_limits() {
        let mut s = shared(false);
        let now = UNIX_EPOCH;
        let a = |x: &str| -> SocketAddr { x.parse().unwrap() };
        assert!(s.precheck_inbound(a("5.6.7.8:1"), now).is_ok());
        assert!(s.precheck_inbound(a("5.6.7.8:2"), now).is_ok());
        assert_eq!(s.precheck_inbound(a("5.6.7.8:3"), now), Err("per-ip limit"));
        assert!(s.precheck_inbound(a("5.6.9.9:1"), now).is_ok());
        assert_eq!(s.precheck_inbound(a("5.6.10.10:1"), now), Err("per-subnet limit"));
        assert!(s.precheck_inbound(a("9.9.9.9:1"), now).is_ok());
        s.release_inbound(a("5.6.7.8:1"));
        assert!(s.precheck_inbound(a("5.6.7.8:4"), now).is_ok());
        // pending handshake cap
        let mut s = shared(false);
        for i in 0..10u8 {
            assert!(s.precheck_inbound(SocketAddr::new([20 + i, 1, 1, 1].into(), 1), now).is_ok());
        }
        assert_eq!(s.precheck_inbound(a("99.1.1.1:1"), now), Err("too many pending handshakes"));
        // local addresses are exempt only with allow_private
        let mut s = shared(true);
        for p in 0..5 {
            assert!(s.precheck_inbound(SocketAddr::new([127, 0, 0, 1].into(), 100 + p), now).is_ok());
        }
        let mut s = shared(false);
        assert!(s.precheck_inbound(a("127.0.0.1:1"), now).is_ok());
        assert!(s.precheck_inbound(a("127.0.0.1:2"), now).is_ok());
        assert!(s.precheck_inbound(a("127.0.0.1:3"), now).is_err());
    }

    #[test]
    fn ip_ban_blocks_precheck_and_expires() {
        let mut s = shared(false);
        let ip: IpAddr = "5.6.7.8".parse().unwrap();
        s.ip_bans.insert(ip, UNIX_EPOCH + Duration::from_secs(10));
        assert_eq!(s.precheck_inbound("5.6.7.8:1".parse().unwrap(), UNIX_EPOCH), Err("ip banned"));
        assert!(s.precheck_inbound("5.6.7.8:1".parse().unwrap(), UNIX_EPOCH + Duration::from_secs(11)).is_ok());
    }

    #[test]
    fn scores_accumulate_to_a_ban() {
        let mut s = shared(false);
        let k = ed25519::PrivateKey::from_seed(9).public_key();
        let now = UNIX_EPOCH;
        for _ in 0..9 {
            assert!(!s.misbehave(&k, 10, "x", now));
        }
        assert!(s.misbehave(&k, 10, "x", now));
        assert!(s.is_banned(&k, now));
        assert!(!s.admit_key(&k, now));
        let later = now + s.cfg.ban_duration + Duration::from_secs(1);
        assert!(!s.is_banned(&k, later));
        assert!(s.admit_key(&k, later));
        let me = s.me.clone();
        assert!(!s.admit_key(&me, now));
    }
}
