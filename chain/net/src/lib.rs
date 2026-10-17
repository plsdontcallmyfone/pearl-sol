//! Agent-chain networking.
//!
//! M1 (this crate today): `commonware_p2p::authenticated::discovery` with a static peer
//! set (the devnet's node keys) tracked at index 0. Every channel the node uses is
//! registered here, so the node code only sees `commonware_p2p::{Sender, Receiver,
//! Provider, Blocker}` and runs unchanged on the simulated network in tests.
//!
//! Seam for open membership (SPEC 6, required before a public net): a crate built on
//! `commonware-stream` with an accept-any bouncer, an address book with gossip, and
//! connection caps, that implements the same four traits and returns the same
//! [`Channels`] shape. Swap [`start_discovery`] for it; nothing above changes.

use std::net::SocketAddr;

use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::authenticated::discovery;
use commonware_p2p::{Ingress, Manager as _};
use commonware_runtime::{Quota, Spawner};
use commonware_utils::ordered::Set;

/// Channel ids.
pub const CH_BLOCKS: u64 = 0;
pub const CH_TXS: u64 = 1;
pub const CH_TIPS: u64 = 2;
pub const CH_HEADERS: u64 = 3;
pub const CH_BODIES: u64 = 4;

pub const NAMESPACE: &[u8] = b"pearl-sol:p2p:v1";
pub const MAX_MESSAGE: u32 = 8 << 20;

/// One (sender, receiver) pair per channel.
pub struct Channels<S, R> {
    pub blocks: (S, R),
    pub txs: (S, R),
    pub tips: (S, R),
    pub headers: (S, R),
    pub bodies: (S, R),
}

pub fn quota() -> Quota {
    Quota::per_second(std::num::NonZeroU32::new(10_000).unwrap())
}

pub type DiscoverySender<E> = commonware_p2p::authenticated::discovery::Sender<ed25519::PublicKey, E>;
pub type DiscoveryReceiver = commonware_p2p::authenticated::discovery::Receiver<ed25519::PublicKey>;

pub struct Started<E: Spawner + commonware_runtime::Clock + commonware_runtime::Metrics> {
    pub oracle: discovery::Oracle<ed25519::PublicKey>,
    pub channels: Channels<DiscoverySender<E>, DiscoveryReceiver>,
    pub handle: commonware_runtime::Handle<()>,
}

/// Start discovery on `listen` with `bootstrappers` and the static `peers` set
/// (should include our own key).
pub fn start_discovery<E>(
    context: E,
    signer: ed25519::PrivateKey,
    listen: SocketAddr,
    dialable: SocketAddr,
    bootstrappers: Vec<(ed25519::PublicKey, SocketAddr)>,
    peers: Vec<ed25519::PublicKey>,
) -> Started<E>
where
    E: Spawner
        + commonware_runtime::Clock
        + commonware_runtime::Metrics
        + commonware_runtime::Network
        + commonware_runtime::Resolver
        + commonware_runtime::BufferPooler
        + rand_core::CryptoRng,
{
    let me = signer.public_key();
    let max = std::num::NonZeroUsize::new(peers.len().max(2) + 1).unwrap();
    let boots = bootstrappers.into_iter().filter(|(k, _)| *k != me).map(|(k, a)| (k, Ingress::Socket(a))).collect();
    let cfg = discovery::Config::local(signer, NAMESPACE, listen, dialable, boots, max, MAX_MESSAGE);
    let (mut network, mut oracle) = discovery::Network::new(context, cfg);
    let mut set = peers;
    set.sort();
    set.dedup();
    let _ = oracle.track(0, Set::try_from(set).expect("distinct peers"));
    let channels = Channels {
        blocks: network.register(CH_BLOCKS, quota()),
        txs: network.register(CH_TXS, quota()),
        tips: network.register(CH_TIPS, quota()),
        headers: network.register(CH_HEADERS, quota()),
        bodies: network.register(CH_BODIES, quota()),
    };
    let handle = network.start();
    Started { oracle, channels, handle }
}
