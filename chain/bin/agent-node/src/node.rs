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
//!
//! Header walks (a peer is ahead: its tip's header, then that header's parent, and so on down
//! to this node's chain) ask for batches where a peer serves them: one request for a header
//! and up to 511 ancestors (`consensus_pow::chain::headers`, `sync::batch_key`). Peers from
//! before batches answer the single-header requests as always.

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
use consensus_pow::{ChainError, HeaderAccept, PowVerifier, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES};
use futures::channel::mpsc;
use futures::StreamExt;
use net::Channels;
use rpc::{ApiRequest, Core, Outbound, Subscriber};
use sync::{Kind, SyncEvent, SyncMailbox};
use tracing::{debug, info, warn};

use crate::fetches::{BatchAsks, HeaderFetches, ServeBudget, BATCH_MIN_GAP};

#[derive(Clone)]
pub struct ActorConfig {
    pub tip_interval: Duration,
    /// Live connected-peer count for getChainInfo `peers` (open net). None: count the
    /// distinct peers heard from (static net).
    pub peer_count: Option<PeerCount>,
    /// getPeers result (open net). None: list the peers heard from (static net).
    pub peer_info: Option<rpc::core::PeerInfoFn>,
    /// Ask for and serve header batches. Off: the node asks for single headers only and
    /// answers a batch ask with "do not have it", as a node from before batches does.
    pub header_batches: bool,
}

impl std::fmt::Debug for ActorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorConfig").field("tip_interval", &self.tip_interval).finish_non_exhaustive()
    }
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self { tip_interval: Duration::from_secs(2), peer_count: None, peer_info: None, header_batches: true }
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
    sync: SyncState,
}

/// What the node tracks about its own header asks and serves.
struct SyncState {
    /// Outstanding header fetches (C-U2): capped, per-peer limited, expiring.
    header_fetches: HeaderFetches<PublicKey>,
    /// Outstanding batch asks of the header walks: capped, per-peer limited, expiring.
    batch_asks: BatchAsks<PublicKey>,
    serve_budget: ServeBudget,
    /// Serve batches ([`ActorConfig::header_batches`]).
    batches: bool,
}

impl<S: Sender<PublicKey = PublicKey>> Net<S> {
    /// Start the fetch of single header `h` unless it is tracked or over a cap.
    fn fetch_header(&mut self, h: [u8; 32], from: Option<&PublicKey>, now_ms: u64, drop_headers: &mut HashSet<sync::Key>, stats: &mut rpc::core::SyncStats) {
        let w = self.sync.header_fetches.want(h, from, now_ms);
        if let Some(old) = w.evicted {
            drop_headers.insert(sync::key(&old));
        }
        if w.fetch {
            drop_headers.remove(&sync::key(&h));
            stats.header_asks += 1;
            let fb = self.headers.fetch(sync::key(&h));
            debug!(key = %hex::encode(&h[..6]), ?fb, "fetch header");
        }
    }

    /// Send what the core queued. `from` is the peer whose gossip this loop turn handled (the
    /// header fetches it caused count against that peer); `None` for sync deliveries, RPC
    /// and the timer. `walk` is the peer whose gossip started the header walk a sync delivery
    /// continued (its batch asks count against that peer). `known` says whether the chain
    /// holds a header.
    fn send(&mut self, outs: Vec<Outbound>, from: Option<&PublicKey>, walk: Option<&PublicKey>, now_ms: u64, known: &dyn Fn(&[u8; 32]) -> bool, stats: &mut rpc::core::SyncStats) {
        // expired asks are cancelled before anything is (re)fetched
        let mut drop_headers: HashSet<sync::Key> = self.sync.header_fetches.expire(now_ms).iter().map(sync::key).collect();
        // a batch ask nobody answered: cancelled, and the walk goes on with the single header
        for (h, origin) in self.sync.batch_asks.expire(now_ms) {
            debug!(key = %hex::encode(&h[..6]), "header batch ask unanswered: asking for the single header");
            stats.batch_asks_expired += 1;
            drop_headers.insert(sync::key(&sync::batch_key(&h)));
            if !known(&h) {
                self.fetch_header(h, None, now_ms, &mut drop_headers, stats);
                self.sync.header_fetches.set_walk(&h, origin.as_ref());
            }
        }
        let drop_bodies: HashSet<sync::Key> = outs.iter().filter_map(|o| if let Outbound::CancelBody(h) = o { Some(sync::key(h)) } else { None }).collect();
        if !drop_bodies.is_empty() {
            debug!(count = drop_bodies.len(), "body asks expired: fetches cancelled");
            let _ = self.bodies.retain(move |key, _| !drop_bodies.contains(key));
        }
        for o in outs {
            match o {
                Outbound::Block(b) => {
                    let _ = self.blocks.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::Tx(b) => {
                    let _ = self.txs.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::FetchHeader(h) => self.fetch_header(h, from, now_ms, &mut drop_headers, stats),
                Outbound::FetchHeaders { hash: h, after_batch } => {
                    let w = self.sync.batch_asks.want(h, walk, after_batch, now_ms);
                    if w.batch {
                        let k = sync::key(&sync::batch_key(&h));
                        drop_headers.remove(&k);
                        stats.batch_asks += 1;
                        let fb = self.headers.fetch(k);
                        debug!(key = %hex::encode(&h[..6]), ?fb, "fetch header batch");
                    }
                    if w.single {
                        // as before batches: the parent of a served header is the chain's own ask
                        self.fetch_header(h, None, now_ms, &mut drop_headers, stats);
                        self.sync.header_fetches.set_walk(&h, walk);
                    }
                }
                Outbound::CancelBody(_) => {}
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
                Outbound::Weak(b) => {
                    let _ = self.tips.send(Recipients::All, IoBuf::from(b), false);
                }
                Outbound::CancelBlob(h) => {
                    let k = sync::key(&h);
                    let _ = self.blob_fetch.retain(move |key, _| *key != k);
                }
            }
        }
        if !drop_headers.is_empty() {
            debug!(count = drop_headers.len(), tracked = self.sync.header_fetches.len(), "header fetches expired or over the cap: cancelled");
            let _ = self.headers.retain(move |key, _| !drop_headers.contains(key));
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
    let sync_state = SyncState { header_fetches: HeaderFetches::default(), batch_asks: BatchAsks::new(cfg.header_batches), serve_budget: ServeBudget::default(), batches: cfg.header_batches };
    let mut net = Net { blocks: blocks_s, txs: txs_s, tips: tips_s, blobs: blobs_s, headers, bodies, blob_fetch, sync: sync_state };
    core.peer_info = cfg.peer_info.clone();
    core.started_ms = now_ms(&context);
    let mut seen_peers: HashSet<PublicKey> = HashSet::new();
    let mut next_tip = context.current() + cfg.tip_interval;
    // events handled per select branch (api, blocks, txs, blobs, tips, sync, timer), logged every 15 tip ticks
    let mut seen = [0u64; 7];

    loop {
        // the peer whose gossip this turn handles: header fetches it causes count against it
        let mut from: Option<PublicKey> = None;
        // the peer whose gossip started the header walk this turn's sync delivery continued
        let mut walk: Option<PublicKey> = None;
        // biased select: the tip timer comes first so a busy branch below cannot starve it
        // (no tip announcements, a stale peer count; seen on devnet box 1, 2026-10-21); then
        // block gossip, sync deliveries and tips/weak blocks BEFORE the RPC mailbox: a node whose
        // RPC is saturated by local clients (devnet box 0, 2026-10-22: getChainInfo at 0.6-1.4 s)
        // otherwise read gossiped blocks late and fell off the tip (orphan stalls at 5762, 5768, 5777)
        select! {
            _ = context.sleep_until(next_tip) => {
                next_tip = context.current() + cfg.tip_interval;
                seen[6] += 1;
                if seen[6] % 15 == 0 {
                    debug!(api = seen[0], blocks = seen[1], txs = seen[2], blobs = seen[3], tips = seen[4], sync = seen[5], timer = seen[6], height = core.chain.tip().header.height, "node loop events");
                    // a syncing node says so (2026-11-10: a fresh node logged nothing for an hour)
                    let s = core.sync_info(now_ms(&context));
                    if s.phase != "synced" {
                        info!(phase = s.phase, height = core.chain.tip_height(), best_header = s.best_header_height, header_walk_low = ?s.walk_low,
                              headers_held = core.chain.headers_only_count(), bodies_outstanding = core.chain.bodies_outstanding(), "syncing");
                    }
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
            msg = blocks_r.recv() => {
                seen[1] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer.clone());
                from = Some(peer.clone());
                let t_dec = std::time::Instant::now();
                match Block::decode(buf.as_ref()) {
                    Ok(b) => {
                        let decode_us = t_dec.elapsed().as_micros() as u64;
                        let now = now_ms(&context);
                        let (bh, bheight, bsize) = (b.hash(), b.header.height, buf.as_ref().len());
                        let t_add = std::time::Instant::now();
                        let r = core.add_block(b, now, true).await;
                        debug!(block = %hex::encode(&bh[..6]), height = bheight, size = bsize, decode_us, add_us = t_add.elapsed().as_micros() as u64,
                               received_ms = now, accepted = r.accepted, "gossiped block timing");
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
            ev = sync_rx.recv() => {
                seen[5] += 1;
                let Some(ev) = ev else { break };
                walk = handle_sync(&mut core, &mut net.sync, ev, now_ms(&context)).await;
            },
            msg = tips_r.recv() => {
                seen[4] += 1;
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer.clone());
                from = Some(peer.clone());
                let b = buf.as_ref();
                if consensus_pow::is_weak_msg(b) {
                    // weak block (docs/SPEED.md 2.2): invalid ones get the peer blocked like a bad block
                    if let rpc::preconf::WeakOutcome::Invalid(reason) = core.add_weak(b, now_ms(&context)).await {
                        warn!(?peer, %reason, "invalid weak block from peer: blocking it");
                        let _ = gossip_blocker.block(peer.clone());
                    }
                } else if b.len() == 40 {
                    let h: [u8; 32] = b[..32].try_into().unwrap();
                    let height = u64::from_le_bytes(b[32..40].try_into().unwrap());
                    if height > core.chain.finalized().header.height && !core.chain.is_known(&h) {
                        core.outbox.push(Outbound::FetchHeader(h));
                    } else if height > core.chain.tip_height() {
                        // a tip we hold only as an orphan or header: ask again for whatever its
                        // ancestry is missing (consensus_pow::chain::repair, the 5762 stall)
                        match core.chain.repair_for(&h) {
                            Some(consensus_pow::Repair::Header(m)) => core.outbox.push(Outbound::FetchHeader(m)),
                            Some(consensus_pow::Repair::Body(m)) => core.chain.reask_body(&m),
                            None => {}
                        }
                    }
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
        }
        let now = now_ms(&context);
        core.schedule_fetches(now);
        // every turn, also with nothing to send: header fetches expire here
        let outs = core.take_outbox();
        let chain = &core.chain;
        net.send(outs, from.as_ref(), walk.as_ref(), now, &|h| chain.is_known(h), &mut core.sync_stats);
    }
}

/// Handle one resolver event. Returns the peer whose gossip started the header walk a
/// header delivery continued, when known.
async fn handle_sync<CE: commonware_storage::Context + Spawner, V: PowVerifier>(
    core: &mut Core<CE, V>,
    st: &mut SyncState,
    ev: SyncEvent,
    now: u64,
) -> Option<PublicKey> {
    let mut walk = None;
    match ev {
        SyncEvent::Delivered { kind: Kind::Header, key, value, verdict } => {
            let anchor = sync::batch_key(&key);
            let ok = if st.batch_asks.contains(&anchor) {
                // the answer to a batch ask: only a batch that is one chain down from the
                // header asked for is looked at, and each header is checked as a single one is
                debug!(key = %hex::encode(&anchor[..6]), bytes = value.len(), "header batch delivered");
                let r = match consensus_pow::decode_header_batch(&value) {
                    Ok(batch) => core.chain.insert_header_batch(&anchor, batch).await,
                    Err(e) => Err(e),
                };
                match r {
                    Ok(acc) => {
                        walk = st.batch_asks.delivered(&anchor).flatten();
                        core.sync_stats.batches_delivered += 1;
                        core.sync_stats.batch_headers += acc.stored as u64;
                        if acc.stored > 0 {
                            core.note_headers(acc.top_height, acc.low_height, now);
                        }
                        if let Some((p, height)) = acc.need_parent {
                            core.outbox.push(walk_step(p, height, core.chain.tip_height(), true));
                        }
                        true
                    }
                    Err(ChainError::Invalid(e)) => {
                        // the resolver asks other peers; the ask stays tracked and expires
                        warn!("invalid header batch from peer: {e}");
                        core.sync_stats.batches_refused += 1;
                        false
                    }
                    Err(_) => {
                        st.batch_asks.delivered(&anchor);
                        true
                    }
                }
            } else if consensus_pow::is_header_batch(&value) {
                // No batch is asked for under this key. An answer to an ask that expired a
                // moment ago is dropped unread and is not the peer's fault; it is that only if
                // it is one chain down from the block the key names. Anything else is junk.
                consensus_pow::decode_header_batch(&value).is_ok_and(|b| consensus_pow::check_header_links(&anchor, &b).is_ok())
            } else {
                debug!(key = %hex::encode(&key[..6]), "header delivered");
                let ok = match consensus_pow::Chain::<CE, V>::decode_header_payload(&value) {
                    Ok((h, proof)) if h.hash() == key => match core.chain.insert_header(h, proof).await {
                        Ok(HeaderAccept::Stored { need_parent }) => {
                            core.note_headers(h.height, h.height, now);
                            // the body is asked for by Core::schedule_fetches, in height order
                            if let Some(p) = need_parent {
                                core.outbox.push(walk_step(p, h.height.saturating_sub(1), core.chain.tip_height(), false));
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
                // on a refused answer the resolver keeps asking other peers: the entry stays
                // tracked and expires like any other
                if ok {
                    core.sync_stats.headers_delivered += 1;
                    walk = st.header_fetches.walk_of(&key).cloned();
                    st.header_fetches.done(&key);
                }
                ok
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
                Kind::Header => match core.chain.header_with_proof(&key).await.ok().flatten() {
                    Some(d) => Some(d),
                    // not a header this node has: a batch ask, if the key names a block it has
                    None if st.batches => {
                        let anchor = sync::batch_key(&key);
                        if !st.serve_budget.available(now) {
                            if core.chain.is_known(&anchor) {
                                core.sync_stats.batch_serves_skipped += 1;
                            }
                            None
                        } else {
                            let b = core.chain.header_batch(&anchor, MAX_HEADER_BATCH, MAX_HEADER_BATCH_BYTES).await.ok().flatten();
                            if b.is_some() {
                                st.serve_budget.take(now);
                                core.sync_stats.batches_served += 1;
                            }
                            b
                        }
                    }
                    None => None,
                },
                Kind::Body => core.chain.get_block(&key).await.ok().flatten().map(|b| b.encode()),
                Kind::Blob => core.serve_blob(&key).await,
            };
            if let Some(d) = data {
                let _ = reply.send(Bytes::from(d));
            }
        }
    }
    walk
}

/// The next ask of a header walk that needs header `h` at `height`: a batch while the walk is
/// far above the node's tip, the single header near it and below it ([`BATCH_MIN_GAP`]).
pub fn walk_step(h: [u8; 32], height: u64, tip_height: u64, after_batch: bool) -> Outbound {
    if height > tip_height.saturating_add(BATCH_MIN_GAP) {
        Outbound::FetchHeaders { hash: h, after_batch }
    } else {
        Outbound::FetchHeader(h)
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
