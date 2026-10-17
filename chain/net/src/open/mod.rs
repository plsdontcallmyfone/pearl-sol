//! Open-membership p2p on `commonware-stream` (SPEC 6 `net`).
//!
//! Anyone can run a node and join knowing one seed address:
//! - every link is a commonware-stream encrypted connection; its handshake proves the
//!   peer's ed25519 identity. The listener's bouncer admits ANY key that is not banned
//!   (no registered peer set, unlike `discovery` / `lookup`).
//! - Bitcoin-style address gossip: `GetAddr` on every outbound connect, `Addr` replies
//!   sampled from the address book, self-announcement every `gossip_interval`, small
//!   `Addr` messages relayed to two random peers, an address token bucket per peer.
//! - an address book with `new` / `tried` tables ([`book`]), seeds from config,
//!   reconnect with exponential backoff and jitter.
//! - `max_outbound` outbound target (one per subnet), `max_inbound` cap (a full node
//!   still answers with an address sample before it hangs up, so joiners can move on),
//!   per-IP and per-subnet inbound limits, a pending-handshake cap.
//! - ban scores: misbehaviour adds points, `ban_threshold` bans the key (and its IP,
//!   unless the IP is local and `allow_private` is set) for `ban_duration`.
//!   [`commonware_p2p::Blocker::block`] is an instant ban (used by the resolver and the
//!   node for invalid headers / blocks / PoW).
//!
//! The [`Oracle`] implements `commonware_p2p::{Provider, Blocker}`; channels give
//! `commonware_p2p::{Sender, Receiver}`. The peer set the Provider reports is the set of
//! currently connected peers, re-published (with a new index) on every change, so the
//! resolver-based sync runs unchanged.

pub mod book;
mod actor;
mod handle;
pub mod limits;
mod state;
pub mod testing;
pub mod wire;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use commonware_cryptography::{ed25519, Signer as _};
use commonware_runtime::{BufferPooler, Clock, Handle, Metrics, Network as RNetwork, Quota, Spawner};

pub use handle::{Error, NetStats, Oracle, PeerInfo, Receiver, Sender};

/// A seed: `host:port`, optionally with the expected key (`key@host:port`).
#[derive(Debug, Clone)]
pub struct Seed {
    pub addr: SocketAddr,
    pub key: Option<ed25519::PublicKey>,
}

#[derive(Clone)]
pub struct Config {
    pub signer: ed25519::PrivateKey,
    /// Handshake namespace; nodes with different namespaces cannot connect (the node
    /// mixes in its genesis hash).
    pub namespace: Vec<u8>,
    pub listen: SocketAddr,
    /// Address we advertise. None: the listen address if it is dialable.
    pub external: Option<SocketAddr>,
    pub seeds: Vec<Seed>,
    pub max_outbound: usize,
    pub max_inbound: usize,
    pub max_inbound_per_ip: usize,
    pub max_inbound_per_subnet: usize,
    pub max_pending_handshakes: usize,
    /// Accept, gossip and dial loopback / private addresses, and exempt them from
    /// per-IP / per-subnet limits and IP bans (devnets and tests only).
    pub allow_private: bool,
    pub ban_threshold: u32,
    pub ban_duration: Duration,
    pub handshake_timeout: Duration,
    pub dial_timeout: Duration,
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
    pub dial_interval: Duration,
    pub gossip_interval: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    /// Retry delay for a peer that said its inbound slots are full.
    pub full_backoff: Duration,
    pub max_message: u32,
    pub queue_size: usize,
    pub max_new: usize,
    pub max_tried: usize,
    pub max_per_source: usize,
    pub max_failures: u32,
    /// Addresses per Addr reply.
    pub addr_reply: usize,
    /// Address token bucket per peer: burst and refill per second (Bitcoin: 1000, 0.1).
    pub addr_burst: f64,
    pub addr_refill_per_s: f64,
    pub synchrony_bound: Duration,
    pub max_handshake_age: Duration,
}

impl Config {
    /// Production defaults for a public node.
    pub fn recommended(signer: ed25519::PrivateKey, namespace: &[u8], listen: SocketAddr) -> Self {
        Self {
            signer,
            namespace: namespace.to_vec(),
            listen,
            external: None,
            seeds: Vec::new(),
            max_outbound: 8,
            max_inbound: 64,
            max_inbound_per_ip: 4,
            max_inbound_per_subnet: 16,
            max_pending_handshakes: 32,
            allow_private: false,
            ban_threshold: 100,
            ban_duration: Duration::from_secs(24 * 3600),
            handshake_timeout: Duration::from_secs(5),
            dial_timeout: Duration::from_secs(5),
            ping_interval: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(90),
            dial_interval: Duration::from_secs(1),
            gossip_interval: Duration::from_secs(60),
            backoff_base: Duration::from_secs(2),
            backoff_max: Duration::from_secs(600),
            full_backoff: Duration::from_secs(300),
            max_message: crate::MAX_MESSAGE,
            queue_size: 1024,
            max_new: 4096,
            max_tried: 1024,
            max_per_source: 256,
            max_failures: 8,
            addr_reply: 250,
            addr_burst: 1000.0,
            addr_refill_per_s: 0.1,
            synchrony_bound: Duration::from_secs(5),
            max_handshake_age: Duration::from_secs(10),
        }
    }

    /// Local devnet / tests: private addresses allowed and faster timers.
    pub fn local(signer: ed25519::PrivateKey, namespace: &[u8], listen: SocketAddr) -> Self {
        Self {
            allow_private: true,
            ping_interval: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(30),
            dial_interval: Duration::from_millis(500),
            gossip_interval: Duration::from_secs(5),
            backoff_base: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
            full_backoff: Duration::from_secs(30),
            addr_refill_per_s: 1.0,
            ..Self::recommended(signer, namespace, listen)
        }
    }

    pub fn public_key(&self) -> ed25519::PublicKey {
        self.signer.public_key()
    }

    pub(crate) fn advertised(&self) -> Option<SocketAddr> {
        let a = self.external.unwrap_or(self.listen);
        limits::dialable(&a, self.allow_private).then_some(a)
    }
}

/// Small deterministic PRNG (splitmix64) for address selection and sampling.
#[derive(Debug, Clone)]
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

pub(crate) struct ChannelIn {
    pub tx: futures::channel::mpsc::Sender<(ed25519::PublicKey, commonware_runtime::IoBuf)>,
    /// Inbound messages per second allowed per peer, and burst.
    pub rate: f64,
    pub burst: f64,
}

/// Builder: register channels, then [`Network::start`].
pub struct Network<E, N> {
    context: E,
    net: N,
    cfg: Arc<Config>,
    shared: Arc<Mutex<state::Shared>>,
    oracle: Oracle,
    channels: HashMap<u64, ChannelIn>,
}

impl<E, N> Network<E, N>
where
    E: Spawner + Clock + Metrics + BufferPooler + rand_core::CryptoRng + Send + Sync + 'static,
    N: RNetwork + Send + Sync + 'static,
{
    /// `net` is the transport (normally a child of the runtime context; tests can wrap it).
    pub fn new(mut context: E, net: N, cfg: Config) -> (Self, Oracle) {
        let seed = rand_core::Rng::next_u64(&mut context);
        let cfg = Arc::new(cfg);
        let shared = Arc::new(Mutex::new(state::Shared::new(cfg.clone(), seed, context.current())));
        let c = context.child("clock");
        let oracle = Oracle::new(shared.clone(), Arc::new(move || c.current()));
        (Self { context, net, cfg, shared, oracle: oracle.clone(), channels: HashMap::new() }, oracle)
    }

    /// Register a channel. `quota` limits what we send to each peer (as in discovery)
    /// and, with 2x slack, what each peer may send us (excess is dropped and scored).
    pub fn register(&mut self, channel: u64, quota: Quota, backlog: usize) -> (Sender<E>, Receiver) {
        assert!(!self.channels.contains_key(&channel), "channel {channel} registered twice");
        let (tx, rx) = futures::channel::mpsc::channel(backlog);
        let per_cell = quota.replenish_interval().as_secs_f64().max(1e-9);
        let rate = 2.0 / per_cell;
        let burst = 2.0 * quota.burst_size().get() as f64;
        self.channels.insert(channel, ChannelIn { tx, rate, burst });
        let sender = Sender::new(channel, self.cfg.max_message, self.oracle.clone(), self.context.child("sender"), quota);
        (sender, Receiver::new(rx))
    }

    pub fn start(self) -> Handle<()> {
        let Self { context, net, cfg, shared, channels, .. } = self;
        let channels = Arc::new(channels);
        let net = Arc::new(net);
        context.child("open_p2p").spawn(move |ctx| actor::run(ctx, net, cfg, shared, channels))
    }
}

pub(crate) fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n.max(1)).unwrap()
}
