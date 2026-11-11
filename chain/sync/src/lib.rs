//! Headers-first sync on `commonware_resolver::p2p` (SPEC 6).
//!
//! Two resolver engines, each on its own p2p channel and keyed by block hash:
//! - **headers**: value = `header (256 B) || proof`. The node checks it (proof_hash,
//!   target limit, PoW where the job is known) before asking for the body.
//! - **bodies**: value = the full encoded block. The node checks `tx_root` and
//!   `proof_hash` against the header it already holds, then executes it.
//!
//! A header key can also ask for a batch ([`batch_key`]): the header of a block and its
//! ancestors in one value. It travels as an ordinary header request, so a node that does not
//! know batches answers "do not have it" like for any unknown hash.
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

/// XOR mask that turns a block hash into the key asking for a header batch that starts at
/// that block. A batch ask is an ordinary request on the headers channel whose key is no
/// block's hash: a node from before batches looks the key up as a hash, finds nothing and
/// answers "do not have it", the answer it gives for any header it lacks. No new channel or
/// message kind is involved (the open network scores frames on a channel it does not know).
pub const BATCH_KEY_MASK: [u8; 32] = *b"cadre:header-batch:v1:::::::::::";

/// The headers-channel key that asks for the batch starting at block `anchor`. The same
/// function maps the key back to the block hash.
pub fn batch_key(anchor: &[u8; 32]) -> [u8; 32] {
    let mut k = *anchor;
    for (b, m) in k.iter_mut().zip(BATCH_KEY_MASK) {
        *b ^= m;
    }
    k
}
