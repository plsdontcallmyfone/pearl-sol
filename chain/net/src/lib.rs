//! Agent-chain networking.
//!
//! M1 (this crate today): `commonware_p2p::authenticated::discovery` with a static peer
//! set (the devnet's node keys) tracked at index 0. Every channel the node uses is
//! registered here, so the node code only sees `commonware_p2p::{Sender, Receiver,
//! Provider, Blocker}` and runs unchanged on the simulated network in tests.
//!
//! M2: [`open`] is open-membership p2p on `commonware-stream` (any key admitted under
//! limits, address gossip, seeds, caps, ban scores). It implements the same four traits
//! and [`start_open`] returns the same [`Channels`] shape, so the node, broadcast and
//! resolver code run unchanged on either. The static mode stays for tests and the M1
//! devnet.

pub mod open;

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
/// Blob pushes (`id [32] || bytes`) from the node that took the putBlob (one hop).
pub const CH_BLOBS: u64 = 5;
/// Blob fetch on demand (resolver keyed by blob id).
pub const CH_BLOB_FETCH: u64 = 6;

pub const NAMESPACE: &[u8] = b"pearl-sol:p2p:v1";
pub const MAX_MESSAGE: u32 = 8 << 20;

/// One (sender, receiver) pair per channel.
pub struct Channels<S, R> {
    pub blocks: (S, R),
    pub txs: (S, R),
    pub tips: (S, R),
    pub headers: (S, R),
    pub bodies: (S, R),
    pub blobs: (S, R),
    pub blob_fetch: (S, R),
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
        blobs: network.register(CH_BLOBS, quota()),
        blob_fetch: network.register(CH_BLOB_FETCH, quota()),
    };
    let handle = network.start();
    Started { oracle, channels, handle }
}

pub type OpenSender<E> = open::Sender<E>;
pub type OpenReceiver = open::Receiver;

pub struct StartedOpen<E: commonware_runtime::Clock> {
    pub oracle: open::Oracle,
    pub channels: Channels<OpenSender<E>, OpenReceiver>,
    pub handle: commonware_runtime::Handle<()>,
}

/// Start the open network with the node's channels. `net` is the transport (the
/// runtime context in production).
pub fn start_open<E, N>(context: E, net: N, cfg: open::Config) -> StartedOpen<E>
where
    E: Spawner
        + commonware_runtime::Clock
        + commonware_runtime::Metrics
        + commonware_runtime::BufferPooler
        + rand_core::CryptoRng
        + Send
        + Sync
        + 'static,
    N: commonware_runtime::Network + Send + Sync + 'static,
{
    let (mut network, oracle) = open::Network::new(context, net, cfg);
    let backlog = 4096;
    let channels = Channels {
        blocks: network.register(CH_BLOCKS, quota(), backlog),
        txs: network.register(CH_TXS, quota(), backlog),
        tips: network.register(CH_TIPS, quota(), backlog),
        headers: network.register(CH_HEADERS, quota(), backlog),
        bodies: network.register(CH_BODIES, quota(), backlog),
        // bulk lane: multi-MB blob frames must not delay or crowd out block and tip gossip
        blobs: network.register_bulk(CH_BLOBS, quota(), backlog),
        blob_fetch: network.register_bulk(CH_BLOB_FETCH, quota(), backlog),
    };
    let handle = network.start();
    StartedOpen { oracle, channels, handle }
}

/// Handshake namespace for the open network of one chain (genesis hash mixed in, so
/// nodes of different chains fail the handshake instead of exchanging garbage).
pub fn open_namespace(genesis_hash: &[u8; 32]) -> Vec<u8> {
    let mut v = b"pearl-sol:p2p:open:v1:".to_vec();
    v.extend_from_slice(genesis_hash);
    v
}
