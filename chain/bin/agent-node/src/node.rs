//! The node actor: one task owns the [`rpc::Core`] (chain + mempool) and reacts to
//! RPC calls, block / tx / tip gossip, and resolver deliveries.
//!
//! Gossip channels:
//! - blocks: full encoded blocks (miners' blocks are broadcast by the node that accepted
//!   them; peers relay blocks that extend their tree).
//! - txs: encoded txs accepted into the mempool.
//! - tips: `tip hash [32] || height u64` every `tip_interval`, so nodes that missed
//!   blocks (partitions, restarts) start a headers-first backfill.
//! - blobs: raw blob bytes, pushed one hop by the node that took the putBlob.
//! Headers, bodies and blobs (by id) are fetched through resolvers in the `sync` crate.

use std::collections::HashSet;
use std::time::{Duration, UNIX_EPOCH};

use agent_types::block::encode_header_with_proof;
use agent_types::{Block, Tx};
use bytes::Bytes;
use commonware_cryptography::ed25519::PublicKey;
use commonware_macros::select;
use commonware_p2p::{Blocker, Provider, Receiver, Recipients, Sender};
use commonware_resolver::Resolver as _;
use commonware_runtime::{BufferPooler, Clock, IoBuf, Metrics, Spawner};
use consensus_pow::{ChainError, HeaderAccept, PowVerifier};
use futures::channel::mpsc;
use futures::StreamExt;
use net::Channels;
use rpc::{ApiRequest, Core, Outbound, Subscriber};
use sync::{Kind, SyncEvent, SyncMailbox};
use tracing::{debug, warn};

#[derive(Clone)]
pub struct ActorConfig {
    pub tip_interval: Duration,
    /// Live connected-peer count for getChainInfo `peers` (open net). None: count the
    /// distinct peers heard from (static net).
    pub peer_count: Option<PeerCount>,
    /// getPeers result (open net). None: list the peers heard from (static net).
    pub peer_info: Option<rpc::core::PeerInfoFn>,
}

impl std::fmt::Debug for ActorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorConfig").field("tip_interval", &self.tip_interval).finish_non_exhaustive()
    }
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self { tip_interval: Duration::from_secs(2), peer_count: None, peer_info: None }
    }
}

#[derive(Clone)]
pub struct PeerCount(pub std::sync::Arc<dyn Fn() -> usize + Send + Sync>);

impl std::fmt::Debug for PeerCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PeerCount")
    }
}

/// Should a gossiped block rejected with `reason` get its sender banned? Invalid
/// blocks (bad PoW, bad roots, bad target, ...) yes; context-dependent rejections
/// (orphans, future timestamps, reorgs deeper than K, internal errors) no.
pub fn ban_worthy(reason: &str) -> bool {
    reason.starts_with("invalid:") && !consensus_pow::is_time_too_new(reason)
}

pub fn now_ms<C: Clock>(c: &C) -> u64 {
    c.current().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Net<S> {
    blocks: S,
    txs: S,
    tips: S,
    blobs: S,
    headers: SyncMailbox,
    bodies: SyncMailbox,
    blob_fetch: SyncMailbox,
}

impl<S: Sender<PublicKey = PublicKey>> Net<S> {
    fn send(&mut self, outs: Vec<Outbound>) {
        for o in outs {
            match o {
                Outbound::Block(b) => {
                    let _ = self.blocks.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::Tx(b) => {
                    let _ = self.txs.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::FetchHeader(h) => {
                    let fb = self.headers.fetch(sync::key(&h));
                    debug!(key = %hex::encode(&h[..6]), ?fb, "fetch header");
                }
                Outbound::FetchBody(h) => {
                    let fb = self.bodies.fetch(sync::key(&h));
                    debug!(key = %hex::encode(&h[..6]), ?fb, "fetch body");
                }
                Outbound::Blob(b) => {
                    let _ = self.blobs.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::FetchBlob(h) => {
                    let _ = self.blob_fetch.fetch(sync::key(&h));
                }
                Outbound::CancelBlob(h) => {
                    let k = sync::key(&h);
                    let _ = self.blob_fetch.retain(move |key, _| *key != k);
                }
            }
        }
    }
}

/// Run the node until the API mailbox closes.
#[allow(clippy::too_many_arguments)]
pub async fn run<E, CE, V, S, R, D, B>(
    context: E,
    mut core: Core<CE, V>,
    me: PublicKey,
    provider: D,
    blocker: B,
    channels: Channels<S, R>,
    mut api: mpsc::Receiver<ApiRequest>,
    cfg: ActorConfig,
) where
    E: BufferPooler + Clock + Spawner + rand_core::Rng + Metrics,
    CE: commonware_storage::Context + Spawner,
    V: PowVerifier,
    S: Sender<PublicKey = PublicKey>,
    R: Receiver<PublicKey = PublicKey>,
    D: Provider<PublicKey = PublicKey> + Clone,
    B: Blocker<PublicKey = PublicKey> + Clone,
{
    let (sync_tx, mut sync_rx) = commonware_utils::channel::mpsc::unbounded_channel::<SyncEvent>();
    let mut gossip_blocker = blocker.clone();
    let headers = sync::start(context.child("headers"), Kind::Header, me.clone(), provider.clone(), blocker.clone(), channels.headers, sync_tx.clone());
    let bodies = sync::start(context.child("bodies"), Kind::Body, me.clone(), provider.clone(), blocker.clone(), channels.bodies, sync_tx.clone());
    let blob_fetch = sync::start(context.child("blob_fetch"), Kind::Blob, me.clone(), provider, blocker, channels.blob_fetch, sync_tx);
    let (blocks_s, mut blocks_r) = channels.blocks;
    let (txs_s, mut txs_r) = channels.txs;
    let (tips_s, mut tips_r) = channels.tips;
    let (blobs_s, mut blobs_r) = channels.blobs;
    let mut net = Net { blocks: blocks_s, txs: txs_s, tips: tips_s, blobs: blobs_s, headers, bodies, blob_fetch };
    core.peer_info = cfg.peer_info.clone();
    let mut seen_peers: HashSet<PublicKey> = HashSet::new();
    let mut next_tip = context.current() + cfg.tip_interval;
    // events handled per select branch (api, blocks, txs, blobs, tips, sync, timer), logged every 15 tip ticks
    let mut seen = [0u64; 7];

    loop {
        // biased select: the tip timer comes first so a busy branch below cannot starve it
        // (no tip announcements, a stale peer count; seen on devnet box 1, 2026-10-21)
        select! {
            _ = context.sleep_until(next_tip) => {
                next_tip = context.current() + cfg.tip_interval;
                seen[6] += 1;
                if seen[6] % 15 == 0 {
                    debug!(api = seen[0], blocks = seen[1], txs = seen[2], blobs = seen[3], tips = seen[4], sync = seen[5], timer = seen[6], height = core.chain.tip().header.height, "node loop events");
                }
                let tip = core.chain.tip();
                let mut m = tip.hash.to_vec();
                m.extend_from_slice(&tip.header.height.to_le_bytes());
                let _ = net.tips.send(Recipients::All, IoBuf::from(m), false);
                core.peers = match &cfg.peer_count {
                    Some(f) => (f.0)(),
                    None => seen_peers.len(),
                };
                if cfg.peer_info.is_none() {
                    core.static_peers = seen_peers.iter().map(|k| bs58::encode(commonware_codec::Encode::encode(k)).into_string()).collect();
                    core.static_peers.sort();
                }
            },
            req = api.next() => {
                seen[0] += 1;
                let Some(req) = req else { break };
                match req {
                    ApiRequest::Call { method, params, reply } => {
                        let now = now_ms(&context);
                        let r = core.dispatch(&method, &params, now).await;
                        let _ = reply.send(r);
                    }
                    ApiRequest::Subscribe { topics, tx } => core.subscribers.push(Subscriber { topics, tx }),
                }
            },
            msg = blocks_r.recv() => {
                seen[1] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer.clone());
                match Block::decode(buf.as_ref()) {
                    Ok(b) => {
                        let now = now_ms(&context);
                        let r = core.add_block(b, now, true).await;
                        if !r.accepted {
                            let reason = r.reason.unwrap_or_default();
                            if ban_worthy(&reason) {
                                warn!(?peer, %reason, "invalid block from peer: blocking it");
                                let _ = gossip_blocker.block(peer);
                            } else {
                                debug!(%reason, "gossiped block rejected");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(?peer, "undecodable block from peer: {e}: blocking it");
                        let _ = gossip_blocker.block(peer);
                    }
                }
            },
            msg = txs_r.recv() => {
                seen[2] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer);
                if let Ok(t) = Tx::decode(buf.as_ref()) {
                    let _ = core.add_tx(t, false).await;
                }
            },
            msg = blobs_r.recv() => {
                seen[3] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer.clone());
                let key = commonware_codec::Encode::encode(&peer);
                if !core.receive_blob(key.as_ref(), buf.as_ref(), now_ms(&context)).await {
                    warn!(?peer, "oversized blob push from peer: blocking it");
                    let _ = gossip_blocker.block(peer);
                }
            },
            msg = tips_r.recv() => {
                seen[4] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer);
                let b = buf.as_ref();
                if b.len() == 40 {
                    let h: [u8; 32] = b[..32].try_into().unwrap();
                    let height = u64::from_le_bytes(b[32..40].try_into().unwrap());
                    if height > core.chain.finalized().header.height && !core.chain.is_known(&h) {
                        core.outbox.push(Outbound::FetchHeader(h));
                    }
                }
            },
            ev = sync_rx.recv() => {
                seen[5] += 1;
                let Some(ev) = ev else { break };
                handle_sync(&mut core, ev, now_ms(&context)).await;
            },
        }
        core.schedule_fetches();
        let outs = core.take_outbox();
        if !outs.is_empty() {
            net.send(outs);
        }
    }
}

async fn handle_sync<CE: commonware_storage::Context + Spawner, V: PowVerifier>(core: &mut Core<CE, V>, ev: SyncEvent, now: u64) {
    match ev {
        SyncEvent::Delivered { kind: Kind::Header, key, value, verdict } => {
            debug!(key = %hex::encode(&key[..6]), "header delivered");
            let ok = match consensus_pow::Chain::<CE, V>::decode_header_payload(&value) {
                Ok((h, proof)) if h.hash() == key => match core.chain.insert_header(h, proof).await {
                    Ok(HeaderAccept::Stored { need_parent }) => {
                        // the body is asked for by Core::schedule_fetches, in height order
                        if let Some(p) = need_parent {
                            core.outbox.push(Outbound::FetchHeader(p));
                        }
                        true
                    }
                    Ok(HeaderAccept::Known) => true,
                    Err(ChainError::Invalid(e)) => {
                        warn!("invalid header from peer: {e}");
                        false
                    }
                    Err(_) => true,
                },
                _ => false,
            };
            let _ = verdict.send(ok);
        }
        SyncEvent::Delivered { kind: Kind::Body, key, value, verdict } => {
            let ok = match Block::decode(&value) {
                Ok(b) if b.hash() == key => {
                    let r = core.add_block(b, now, true).await;
                    debug!(key = %hex::encode(&key[..6]), accepted = r.accepted, reason = ?r.reason, "body delivered");
                    // same rule as gossip: a future timestamp is not the serving peer's fault
                    r.accepted || !ban_worthy(r.reason.as_deref().unwrap_or(""))
                }
                _ => false,
            };
            let _ = verdict.send(ok);
        }
        SyncEvent::Delivered { kind: Kind::Blob, key, value, verdict } => {
            let ok = core.deliver_blob(key, &value).await;
            let _ = verdict.send(ok);
        }
        SyncEvent::Serve { kind, key, reply } => {
            let data = match kind {
                Kind::Header => core.chain.header_with_proof(&key).await.ok().flatten(),
                Kind::Body => core.chain.get_block(&key).await.ok().flatten().map(|b| b.encode()),
                Kind::Blob => core.serve_blob(&key).await,
            };
            if let Some(d) = data {
                let _ = reply.send(Bytes::from(d));
            }
        }
    }
}

/// Header + proof bytes for a block (sync payload).
pub fn header_payload(b: &Block) -> Vec<u8> {
    encode_header_with_proof(&b.header, &b.proof)
}

/// getPeers result for an open-membership node: one entry per connected peer, plus
/// the net counters (`net::open::Oracle::stats`).
pub fn open_peers_json(o: &net::open::Oracle) -> serde_json::Value {
    use commonware_codec::Encode as _;
    let mut peers: Vec<serde_json::Value> = o
        .peers()
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "key": bs58::encode(p.key.encode()).into_string(),
                "direction": if p.inbound { "inbound" } else { "outbound" },
                "remote": p.remote.to_string(),
                "listen": p.listen.map(|a| a.to_string()),
                "score": o.score(&p.key),
            })
        })
        .collect();
    peers.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    let s = o.stats();
    serde_json::json!({
        "mode": "open",
        "me": bs58::encode(o.me().encode()).into_string(),
        "count": peers.len(),
        "peers": peers,
        "stats": {
            "inbound": s.inbound, "outbound": s.outbound, "book_new": s.book_new, "book_tried": s.book_tried,
            "banned": s.banned, "bans_total": s.bans_total, "inbound_full": s.inbound_full,
            "inbound_refused": s.inbound_refused, "dials": s.dials, "dial_failures": s.dial_failures,
            "rate_dropped": s.rate_dropped, "queue_dropped": s.queue_dropped, "bulk_dropped": s.bulk_dropped,
        },
    })
}
