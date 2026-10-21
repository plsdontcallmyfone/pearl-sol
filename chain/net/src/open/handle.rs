//! Public handles: [`Oracle`] (Provider + Blocker + stats) and channel Sender / Receiver.

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use commonware_actor::{Feedback, Unreliable};
use commonware_cryptography::ed25519::PublicKey;
use commonware_p2p::utils::limited::{self, Connected};
use commonware_p2p::{BlockedSubscription, Message, PeerSetSubscription, Recipients, TrackedPeers};
use commonware_runtime::{Clock, IoBuf, IoBufs, Quota};
use commonware_utils::channel::{mpsc, ring};
use futures::StreamExt as _;

use super::state::Shared;
use super::wire;

type Now = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// Handle to a running open network: implements `Provider` (the connected set) and
/// `Blocker` (instant ban), plus scoring and stats.
#[derive(Clone)]
pub struct Oracle {
    shared: Arc<Mutex<Shared>>,
    now: Now,
}

impl fmt::Debug for Oracle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("open::Oracle").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub key: PublicKey,
    pub inbound: bool,
    pub remote: SocketAddr,
    pub listen: Option<SocketAddr>,
}

#[derive(Debug, Clone, Default)]
pub struct NetStats {
    pub inbound: usize,
    pub outbound: usize,
    pub book_new: usize,
    pub book_tried: usize,
    pub banned: usize,
    pub bans_total: u64,
    pub inbound_full: u64,
    pub inbound_refused: u64,
    pub dials: u64,
    pub dial_failures: u64,
    pub rate_dropped: u64,
    /// Frames not sent because a peer's send queue was full (normal lane, bulk lane).
    pub queue_dropped: u64,
    pub bulk_dropped: u64,
}

pub(crate) fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Oracle {
    pub(crate) fn new(shared: Arc<Mutex<Shared>>, now: Now) -> Self {
        Self { shared, now }
    }

    pub(crate) fn shared(&self) -> &Arc<Mutex<Shared>> {
        &self.shared
    }

    pub fn me(&self) -> PublicKey {
        lock(&self.shared).me.clone()
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        lock(&self.shared)
            .peers
            .iter()
            .map(|(k, p)| PeerInfo { key: k.clone(), inbound: p.inbound, remote: p.remote, listen: p.listen })
            .collect()
    }

    pub fn connected_count(&self) -> usize {
        lock(&self.shared).peers.len()
    }

    pub fn is_connected(&self, k: &PublicKey) -> bool {
        lock(&self.shared).peers.contains_key(k)
    }

    pub fn is_banned(&self, k: &PublicKey) -> bool {
        let now = (self.now)();
        lock(&self.shared).is_banned(k, now)
    }

    pub fn score(&self, k: &PublicKey) -> u32 {
        lock(&self.shared).scores.get(k).copied().unwrap_or(0)
    }

    /// Add to a peer's ban score; returns true if that banned it.
    pub fn misbehave(&self, k: &PublicKey, score: u32, reason: &str) -> bool {
        let now = (self.now)();
        lock(&self.shared).misbehave(k, score, reason, now)
    }

    /// Known addresses (book), for diagnostics.
    pub fn known_addrs(&self) -> Vec<(SocketAddr, Option<PublicKey>, bool)> {
        lock(&self.shared).book.entries().map(|e| (e.addr, e.key.clone(), e.tried)).collect()
    }

    pub fn stats(&self) -> NetStats {
        let now = (self.now)();
        let s = lock(&self.shared);
        let (book_new, book_tried) = s.book.counts();
        NetStats {
            inbound: s.inbound_count(),
            outbound: s.outbound_count(),
            book_new,
            book_tried,
            banned: s.bans.values().filter(|u| **u > now).count(),
            bans_total: s.counters.bans,
            inbound_full: s.counters.inbound_full,
            inbound_refused: s.counters.inbound_refused,
            dials: s.counters.dials,
            dial_failures: s.counters.dial_failures,
            rate_dropped: s.counters.rate_dropped,
            queue_dropped: s.counters.queue_dropped,
            bulk_dropped: s.counters.bulk_dropped,
        }
    }
}

impl commonware_p2p::Provider for Oracle {
    type PublicKey = PublicKey;

    async fn peer_set(&mut self, id: u64) -> Option<TrackedPeers<PublicKey>> {
        let s = lock(&self.shared);
        (s.set_index == id).then(|| s.current_update().latest)
    }

    async fn subscribe(&mut self) -> PeerSetSubscription<PublicKey> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut s = lock(&self.shared);
        let _ = tx.send(s.current_update());
        s.set_subs.push(tx);
        rx
    }
}

impl commonware_p2p::Blocker for Oracle {
    type PublicKey = PublicKey;

    fn block(&mut self, peer: PublicKey) -> Feedback {
        let now = (self.now)();
        lock(&self.shared).ban(&peer, "blocked by application", now);
        Feedback::Ok
    }

    fn blocked(&mut self) -> BlockedSubscription<PublicKey> {
        let (tx, rx) = ring::channel(super::nz(1));
        let now = (self.now)();
        let mut s = lock(&self.shared);
        let _ = tx.send_lossy(s.banned_set(now));
        s.blocked_rings.push(tx);
        rx
    }
}

impl Connected for Oracle {
    type PublicKey = PublicKey;

    fn peers(&self) -> Vec<PublicKey> {
        lock(&self.shared).connected()
    }

    fn subscribe(&self) -> ring::Receiver<Vec<PublicKey>> {
        let (tx, rx) = ring::channel(super::nz(1));
        let mut s = lock(&self.shared);
        let _ = tx.send_lossy(s.connected());
        s.conn_rings.push(tx);
        rx
    }
}

/// Unlimited per-channel sender: frames the message once and queues it to each
/// connected recipient (a full queue drops it for that peer).
#[derive(Clone)]
pub struct ChannelSender {
    channel: u64,
    max: u32,
    bulk: bool,
    shared: Arc<Mutex<Shared>>,
}

impl fmt::Debug for ChannelSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChannelSender").field("channel", &self.channel).finish()
    }
}

impl commonware_p2p::UnlimitedSender for ChannelSender {
    type PublicKey = PublicKey;

    fn send(&mut self, recipients: Recipients<PublicKey>, message: impl Into<IoBufs> + Send, _priority: bool) -> Unreliable<Feedback> {
        let mut bufs: IoBufs = message.into();
        if bufs.len() > self.max as usize {
            return Unreliable::rejected();
        }
        bufs.prepend(IoBuf::from(wire::data_header(self.channel)));
        let mut s = lock(&self.shared);
        let targets: Vec<PublicKey> = match recipients {
            Recipients::All => s.connected(),
            Recipients::Some(v) => v,
            Recipients::One(p) => vec![p],
        };
        let mut dropped = 0u64;
        for k in targets {
            if let Some(p) = s.peers.get_mut(&k) {
                let q = if self.bulk { &mut p.bulk } else { &mut p.queue };
                if q.try_send(bufs.clone()).is_err() {
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            if self.bulk {
                s.counters.bulk_dropped += dropped;
            } else {
                s.counters.queue_dropped += dropped;
            }
        }
        Unreliable::new(Feedback::Ok)
    }
}

/// Rate-limited channel sender (same limiter as `discovery`).
pub struct Sender<E: Clock> {
    inner: limited::LimitedSender<E, ChannelSender, Oracle>,
}

impl<E: Clock> Clone for Sender<E> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<E: Clock> fmt::Debug for Sender<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("open::Sender").finish_non_exhaustive()
    }
}

impl<E: Clock> Sender<E> {
    pub(crate) fn new(channel: u64, max: u32, bulk: bool, oracle: Oracle, clock: E, quota: Quota) -> Self {
        let cs = ChannelSender { channel, max, bulk, shared: oracle.shared().clone() };
        Self { inner: limited::LimitedSender::new(cs, quota, clock, oracle) }
    }
}

impl<E: Clock + Send + Sync + 'static> commonware_p2p::LimitedSender for Sender<E> {
    type PublicKey = PublicKey;
    type Checked<'a>
        = limited::CheckedSender<'a, ChannelSender>
    where
        Self: 'a;

    fn check(&mut self, recipients: Recipients<PublicKey>) -> Result<Self::Checked<'_>, SystemTime> {
        self.inner.check(recipients)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network closed")]
    Closed,
}

pub struct Receiver {
    rx: futures::channel::mpsc::Receiver<(PublicKey, IoBuf)>,
}

impl fmt::Debug for Receiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("open::Receiver").finish_non_exhaustive()
    }
}

impl Receiver {
    pub(crate) fn new(rx: futures::channel::mpsc::Receiver<(PublicKey, IoBuf)>) -> Self {
        Self { rx }
    }
}

impl commonware_p2p::Receiver for Receiver {
    type Error = Error;
    type PublicKey = PublicKey;

    async fn recv(&mut self) -> Result<Message<PublicKey>, Error> {
        self.rx.next().await.ok_or(Error::Closed)
    }
}
