//! The node actor: one task owns the [`rpc::Core`] (chain + mempool) and reacts to
//! RPC calls, block / tx / tip gossip, and resolver deliveries.
//!
//! Gossip channels:
//! - blocks: full encoded blocks (miners' blocks are broadcast by the node that accepted
//!   them; peers relay blocks that extend their tree).
//! - txs: encoded txs accepted into the mempool.
//! - tips: `tip hash [32] || height u64` every `tip_interval`, so nodes that missed
//!   blocks (partitions, restarts) start a headers-first backfill.
//! Headers and bodies are fetched through the two resolvers in the `sync` crate.

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

#[derive(Debug, Clone)]
pub struct ActorConfig {
    pub tip_interval: Duration,
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self { tip_interval: Duration::from_secs(2) }
    }
}

pub fn now_ms<C: Clock>(c: &C) -> u64 {
    c.current().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Net<S> {
    blocks: S,
    txs: S,
    tips: S,
    headers: SyncMailbox,
    bodies: SyncMailbox,
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
                    let _ = self.headers.fetch(sync::key(&h));
                }
                Outbound::FetchBody(h) => {
                    let _ = self.bodies.fetch(sync::key(&h));
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
    let headers = sync::start(context.child("headers"), Kind::Header, me.clone(), provider.clone(), blocker.clone(), channels.headers, sync_tx.clone());
    let bodies = sync::start(context.child("bodies"), Kind::Body, me.clone(), provider, blocker, channels.bodies, sync_tx);
    let (blocks_s, mut blocks_r) = channels.blocks;
    let (txs_s, mut txs_r) = channels.txs;
    let (tips_s, mut tips_r) = channels.tips;
    let mut net = Net { blocks: blocks_s, txs: txs_s, tips: tips_s, headers, bodies };
    let mut seen_peers: HashSet<PublicKey> = HashSet::new();
    let mut next_tip = context.current() + cfg.tip_interval;

    loop {
        select! {
            req = api.next() => {
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
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer);
                match Block::decode(buf.as_ref()) {
                    Ok(b) => {
                        let now = now_ms(&context);
                        let r = core.add_block(b, now, true).await;
                        if !r.accepted {
                            debug!(reason = ?r.reason, "gossiped block rejected");
                        }
                    }
                    Err(e) => debug!("undecodable block from peer: {e}"),
                }
            },
            msg = txs_r.recv() => {
                let Ok((peer, buf)) = msg else { break };
                seen_peers.insert(peer);
                if let Ok(t) = Tx::decode(buf.as_ref()) {
                    let _ = core.add_tx(t, false).await;
                }
            },
            msg = tips_r.recv() => {
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
                let Some(ev) = ev else { break };
                handle_sync(&mut core, ev, now_ms(&context)).await;
            },
            _ = context.sleep_until(next_tip) => {
                next_tip = context.current() + cfg.tip_interval;
                let tip = core.chain.tip();
                let mut m = tip.hash.to_vec();
                m.extend_from_slice(&tip.header.height.to_le_bytes());
                let _ = net.tips.send(Recipients::All, IoBuf::from(m), false);
                core.peers = seen_peers.len();
            },
        }
        let outs = core.take_outbox();
        if !outs.is_empty() {
            net.send(outs);
        }
    }
}

async fn handle_sync<CE: commonware_storage::Context + Spawner, V: PowVerifier>(core: &mut Core<CE, V>, ev: SyncEvent, now: u64) {
    match ev {
        SyncEvent::Delivered { kind: Kind::Header, key, value, verdict } => {
            let ok = match consensus_pow::Chain::<CE, V>::decode_header_payload(&value) {
                Ok((h, proof)) if h.hash() == key => match core.chain.insert_header(h, proof).await {
                    Ok(HeaderAccept::Stored { need_parent }) => {
                        core.outbox.push(Outbound::FetchBody(key));
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
                    r.accepted || !r.reason.as_deref().unwrap_or("").starts_with("invalid")
                }
                _ => false,
            };
            let _ = verdict.send(ok);
        }
        SyncEvent::Serve { kind, key, reply } => {
            let data = match kind {
                Kind::Header => core.chain.header_with_proof(&key).await.ok().flatten(),
                Kind::Body => core.chain.get_block(&key).await.ok().flatten().map(|b| b.encode()),
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
