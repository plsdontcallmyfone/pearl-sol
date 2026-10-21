//! Headers-first sync on `commonware_resolver::p2p` (SPEC 6).
//!
//! Two resolver engines, each on its own p2p channel and keyed by block hash:
//! - **headers**: value = `header (256 B) || proof`. The node checks it (proof_hash,
//!   target limit, PoW where the job is known) before asking for the body.
//! - **bodies**: value = the full encoded block. The node checks `tx_root` and
//!   `proof_hash` against the header it already holds, then executes it.
//!
//! Deliveries and serve requests are forwarded to the node actor as [`SyncEvent`]s; the
//! actor answers with a verdict (invalid data blocks the serving peer) or the bytes.

use bytes::Bytes;
use commonware_cryptography::{blake3::Digest, ed25519::PublicKey};
use commonware_p2p::{Blocker, Provider, Receiver, Sender};
use commonware_resolver::p2p::{Config, Engine, Mailbox, Producer};
use commonware_resolver::{Consumer, Delivery};
use commonware_runtime::{BufferPooler, Clock, Metrics, Spawner};
use commonware_utils::channel::{mpsc, oneshot};
use std::time::Duration;

pub type Key = Digest;
pub type SyncMailbox = Mailbox<Key, PublicKey, ()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Header,
    Body,
    /// Blob store content keyed by blob id (blake3 of the bytes).
    Blob,
}

pub enum SyncEvent {
    /// A peer delivered data we fetched: reply `true` if valid.
    Delivered { kind: Kind, key: [u8; 32], value: Bytes, verdict: oneshot::Sender<bool> },
    /// A peer asks for data: reply with the bytes (drop the sender if we have none).
    Serve { kind: Kind, key: [u8; 32], reply: oneshot::Sender<Bytes> },
}

#[derive(Clone)]
pub struct Handler {
    kind: Kind,
    tx: mpsc::UnboundedSender<SyncEvent>,
}

impl Consumer for Handler {
    type Key = Key;
    type Value = Bytes;
    type Subscriber = ();
    type Outcome = bool;

    fn deliver(&mut self, d: Delivery<Key, ()>, value: Bytes) -> oneshot::Receiver<bool> {
        let (verdict, rx) = oneshot::channel();
        let _ = self.tx.send(SyncEvent::Delivered { kind: self.kind, key: d.key.0, value, verdict });
        rx
    }
}

impl Producer for Handler {
    type Key = Key;

    fn produce(&mut self, key: Key) -> oneshot::Receiver<Bytes> {
        let (reply, rx) = oneshot::channel();
        let _ = self.tx.send(SyncEvent::Serve { kind: self.kind, key: key.0, reply });
        rx
    }
}

/// Start one resolver engine of `kind` on `net`.
pub fn start<E, D, B, S, R>(
    context: E,
    kind: Kind,
    me: PublicKey,
    provider: D,
    blocker: B,
    net: (S, R),
    events: mpsc::UnboundedSender<SyncEvent>,
) -> SyncMailbox
where
    E: BufferPooler + Clock + Spawner + rand_core::Rng + Metrics,
    D: Provider<PublicKey = PublicKey>,
    B: Blocker<PublicKey = PublicKey>,
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
{
    let h = Handler { kind, tx: events };
    let (engine, mailbox) = Engine::new(
        context,
        Config {
            peer_provider: provider,
            blocker,
            consumer: h.clone(),
            producer: h,
            mailbox_size: std::num::NonZeroUsize::new(4096).unwrap(),
            me: Some(me),
            timeout: request_timeout(kind),
            fetch_retry_timeout: Duration::from_millis(250),
            priority_requests: false,
            priority_responses: false,
        },
    );
    engine.start(net);
    mailbox
}

/// How long one request waits for a peer's answer before the resolver asks again. Headers
/// and bodies are small and latency matters (fork repair, tip follow). A blob is up to
/// 4 MiB and shares the link with everything else: at 2 s a node missing many blobs timed
/// out and re-asked for each one about every 2 s, and the re-asks kept the link full, so its
/// peers' block and tip gossip to it was dropped and the node stopped following the tip
/// (2026-10-21, HANDOFF item 24).
pub fn request_timeout(kind: Kind) -> Duration {
    match kind {
        Kind::Header | Kind::Body => Duration::from_secs(2),
        Kind::Blob => Duration::from_secs(30),
    }
}

pub fn key(h: &[u8; 32]) -> Key {
    Digest(*h)
}
