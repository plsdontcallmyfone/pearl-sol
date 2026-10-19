//! The network actor (accept, dial, gossip timers) and per-connection tasks.
//!
//! Task tree (Commonware supervision: a finished or aborted parent aborts children):
//! `open_p2p` (this actor) -> `listener`, `handshake` (one per attempt), `peer` (one per
//! connection, the writer) -> `reader`. Aborting a `peer` task drops the connection.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use commonware_cryptography::ed25519::{self, PublicKey};
use commonware_macros::select;
use commonware_runtime::{
    BufferPooler, Clock, IoBufs, Listener as _, Metrics, Network as RNetwork, SinkOf, Spawner, Stream as _,
    StreamOf,
};
use commonware_stream::encrypted;
use futures::{SinkExt as _, StreamExt as _};
use tracing::{debug, error, info};

use super::handle::lock;
use super::limits;
use super::state::{ms, PeerEntry, Shared};
use super::wire::{self, AddrRecord, Msg};
use super::{ChannelIn, Config};

enum Dir {
    Inbound,
    Outbound(SocketAddr),
}

enum Outcome<Si, St> {
    Ok { key: PublicKey, sender: encrypted::Sender<Si>, receiver: encrypted::Receiver<St> },
    SelfConnect,
    Refused(&'static str),
    Failed(String),
}

struct Hs<Si, St> {
    dir: Dir,
    remote: SocketAddr,
    outcome: Outcome<Si, St>,
}

type Channels = Arc<HashMap<u64, ChannelIn>>;

fn stream_cfg(cfg: &Config) -> encrypted::Config<ed25519::PrivateKey> {
    encrypted::Config {
        signing_key: cfg.signer.clone(),
        namespace: cfg.namespace.clone(),
        max_message_size: cfg.max_message + 64,
        synchrony_bound: cfg.synchrony_bound,
        max_handshake_age: cfg.max_handshake_age,
        handshake_timeout: cfg.handshake_timeout,
    }
}

pub(crate) async fn run<E, N>(ctx: E, net: Arc<N>, cfg: Arc<Config>, shared: Arc<Mutex<Shared>>, channels: Channels)
where
    E: Spawner + Clock + Metrics + BufferPooler + rand_core::CryptoRng + Send + Sync + 'static,
    N: RNetwork + Send + Sync + 'static,
{
    let mut listener = match net.bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            error!(listen = %cfg.listen, "open p2p: bind failed: {e}");
            return;
        }
    };
    info!(listen = %cfg.listen, advertised = ?cfg.advertised(), seeds = cfg.seeds.len(), "open p2p listening");
    let (mut acc_tx, mut acc_rx) = futures::channel::mpsc::channel::<(SocketAddr, SinkOf<N>, StreamOf<N>)>(64);
    ctx.child("listener").spawn(move |c| async move {
        loop {
            match listener.accept().await {
                Ok(x) => {
                    if acc_tx.send(x).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("accept failed: {e}");
                    c.sleep(Duration::from_millis(50)).await;
                }
            }
        }
    });
    let (hs_tx, mut hs_rx) = futures::channel::mpsc::channel::<Hs<SinkOf<N>, StreamOf<N>>>(64);
    let mut next_dial = ctx.current();
    let mut next_gossip = ctx.current() + cfg.gossip_interval;
    loop {
        select! {
            a = acc_rx.next() => {
                let Some((remote, sink, stream)) = a else { break };
                on_accept(&ctx, &cfg, &shared, hs_tx.clone(), remote, sink, stream);
            },
            h = hs_rx.next() => {
                let Some(h) = h else { break };
                on_handshake(&ctx, &cfg, &shared, &channels, h);
            },
            _ = ctx.sleep_until(next_dial) => {
                dial_tick(&ctx, &net, &cfg, &shared, &hs_tx);
                next_dial = ctx.current() + cfg.dial_interval;
            },
            _ = ctx.sleep_until(next_gossip) => {
                gossip_tick(&ctx, &cfg, &shared);
                next_gossip = ctx.current() + cfg.gossip_interval;
            },
        }
    }
}

fn on_accept<E, Si, St>(
    ctx: &E,
    cfg: &Arc<Config>,
    shared: &Arc<Mutex<Shared>>,
    mut hs_tx: futures::channel::mpsc::Sender<Hs<Si, St>>,
    remote: SocketAddr,
    mut sink: Si,
    stream: St,
) where
    E: Spawner + Clock + BufferPooler + rand_core::CryptoRng + Send + Sync + 'static,
    Si: commonware_runtime::Sink,
    St: commonware_runtime::Stream,
{
    let now = ctx.current();
    {
        let mut s = lock(shared);
        if let Err(why) = s.precheck_inbound(remote, now) {
            s.counters.inbound_refused += 1;
            debug!(%remote, why, "inbound refused before handshake");
            return;
        }
    }
    let scfg = stream_cfg(cfg);
    let sh = shared.clone();
    ctx.child("handshake").spawn(move |c| async move {
        let me = lock(&sh).me.clone();
        let outcome = if sink.send(IoBufs::from(wire::banner(&me))).await.is_err() {
            Outcome::Failed("banner send".into())
        } else {
            let sh2 = sh.clone();
            let c2 = c.child("bouncer");
            let bouncer = move |k: PublicKey| {
                let ok = lock(&sh2).admit_key(&k, c2.current());
                async move { ok }
            };
            match encrypted::listen(c, bouncer, scfg, stream, sink).await {
                Ok((key, sender, receiver)) => Outcome::Ok { key, sender, receiver },
                Err(e) => Outcome::Failed(format!("inbound handshake: {e}")),
            }
        };
        let _ = hs_tx.send(Hs { dir: Dir::Inbound, remote, outcome }).await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn outbound_hs<E, N>(
    c: E,
    net: Arc<N>,
    scfg: encrypted::Config<ed25519::PrivateKey>,
    shared: Arc<Mutex<Shared>>,
    addr: SocketAddr,
    expect: Option<PublicKey>,
    pinned: bool,
    timeout: Duration,
) -> Outcome<SinkOf<N>, StreamOf<N>>
where
    E: Clock + BufferPooler + rand_core::CryptoRng + Send + Sync + 'static,
    N: RNetwork,
{
    let dialed = select! {
        r = net.dial(addr) => r,
        _ = c.sleep(timeout) => return Outcome::Failed("dial timeout".into()),
    };
    let (sink, mut stream) = match dialed {
        Ok(x) => x,
        Err(e) => return Outcome::Failed(format!("dial: {e}")),
    };
    let banner = select! {
        r = stream.recv(wire::BANNER_LEN) => r,
        _ = c.sleep(timeout) => return Outcome::Failed("banner timeout".into()),
    };
    let key = match banner {
        Ok(b) => match wire::parse_banner(b.coalesce().as_ref()) {
            Some(k) => k,
            None => return Outcome::Failed("bad banner".into()),
        },
        Err(e) => return Outcome::Failed(format!("banner: {e}")),
    };
    if pinned && expect.as_ref() != Some(&key) {
        return Outcome::Refused("key differs from the pinned seed key");
    }
    {
        let now = c.current();
        let mut s = lock(&shared);
        if key == s.me {
            return Outcome::SelfConnect;
        }
        if s.is_banned(&key, now) {
            return Outcome::Refused("banned");
        }
        if s.peers.contains_key(&key) {
            return Outcome::Refused("already connected");
        }
    }
    match encrypted::dial(c, scfg, key.clone(), stream, sink).await {
        Ok((sender, receiver)) => Outcome::Ok { key, sender, receiver },
        Err(e) => Outcome::Failed(format!("outbound handshake: {e}")),
    }
}

fn dial_tick<E, N>(
    ctx: &E,
    net: &Arc<N>,
    cfg: &Arc<Config>,
    shared: &Arc<Mutex<Shared>>,
    hs_tx: &futures::channel::mpsc::Sender<Hs<SinkOf<N>, StreamOf<N>>>,
) where
    E: Spawner + Clock + BufferPooler + rand_core::CryptoRng + Send + Sync + 'static,
    N: RNetwork + Send + Sync + 'static,
{
    let now = ctx.current();
    let mut picks: Vec<(SocketAddr, Option<PublicKey>, bool)> = Vec::new();
    {
        let mut g = lock(shared);
        let s = &mut *g;
        let out = s.outbound_count() + s.pending_out;
        if out >= cfg.max_outbound {
            return;
        }
        let room = cfg.max_pending_handshakes.saturating_sub(s.pending_in + s.pending_out);
        let mut want = (cfg.max_outbound - out).min(room).min(4);
        let allow = cfg.allow_private;
        while want > 0 {
            let mut used: HashSet<limits::Subnet> = s
                .peers
                .values()
                .filter(|p| !p.inbound && !limits::exempt(p.remote.ip(), allow))
                .map(|p| limits::subnet(p.remote.ip()))
                .collect();
            for (a, _, _) in &picks {
                if !limits::exempt(a.ip(), allow) {
                    used.insert(limits::subnet(a.ip()));
                }
            }
            let (peers, bans, ip_bans, me) = (&s.peers, &s.bans, &s.ip_bans, &s.me);
            let pick = s.book.select(now, &mut s.rng, |e| {
                if let Some(k) = &e.key {
                    if k == me || peers.contains_key(k) || bans.get(k).is_some_and(|u| *u > now) {
                        return false;
                    }
                }
                let ip = limits::canonical(e.addr.ip());
                if ip_bans.get(&ip).is_some_and(|u| *u > now) {
                    return false;
                }
                // outbound diversity (one per /16) does not apply to key-pinned seeds: the
                // operator chose them (a devnet's own boxes often share a /16)
                if !e.pinned && !limits::exempt(ip, allow) && used.contains(&limits::subnet(ip)) {
                    return false;
                }
                limits::dialable(&e.addr, allow)
            });
            let Some(a) = pick else { break };
            s.book.mark_attempt(&a);
            let e = s.book.get(&a).unwrap();
            picks.push((a, e.key.clone(), e.pinned));
            want -= 1;
        }
        s.pending_out += picks.len();
        s.counters.dials += picks.len() as u64;
    }
    for (addr, expect, pinned) in picks {
        let (n, scfg, sh, mut tx, t) = (net.clone(), stream_cfg(cfg), shared.clone(), hs_tx.clone(), cfg.dial_timeout);
        ctx.child("handshake").spawn(move |c| async move {
            let outcome = outbound_hs(c, n, scfg, sh, addr, expect, pinned, t).await;
            let _ = tx.send(Hs { dir: Dir::Outbound(addr), remote: addr, outcome }).await;
        });
    }
}

fn gossip_tick<E: Clock>(ctx: &E, cfg: &Arc<Config>, shared: &Arc<Mutex<Shared>>) {
    let now = ctx.current();
    let mut s = lock(shared);
    s.scores.retain(|_, v| {
        *v = v.saturating_sub(1);
        *v > 0
    });
    s.bans.retain(|_, u| *u > now);
    s.ip_bans.retain(|_, u| *u > now);
    let keys = s.connected();
    if let Some(a) = cfg.advertised() {
        let rec = AddrRecord { key: s.me.clone(), addr: a, last_seen_ms: ms(now) };
        for k in &keys {
            s.send_ctrl(k, &Msg::Addr(vec![rec.clone()]));
        }
    }
    if s.outbound_count() < cfg.max_outbound && !keys.is_empty() {
        let k = keys[(s.rng.next() % keys.len() as u64) as usize].clone();
        s.send_ctrl(&k, &Msg::GetAddr);
    }
}

fn on_handshake<E, Si, St>(ctx: &E, cfg: &Arc<Config>, shared: &Arc<Mutex<Shared>>, channels: &Channels, h: Hs<Si, St>)
where
    E: Spawner + Clock + Send + Sync + 'static,
    Si: commonware_runtime::Sink,
    St: commonware_runtime::Stream,
{
    let now = ctx.current();
    let mut s = lock(shared);
    match h.dir {
        Dir::Inbound => s.release_inbound(h.remote),
        Dir::Outbound(_) => s.pending_out = s.pending_out.saturating_sub(1),
    }
    let dialed = match h.dir {
        Dir::Outbound(a) => Some(a),
        Dir::Inbound => None,
    };
    let (key, sender, receiver) = match h.outcome {
        Outcome::Ok { key, sender, receiver } => (key, sender, receiver),
        Outcome::SelfConnect => {
            if let Some(a) = dialed {
                s.book.mark_self(&a);
            }
            return;
        }
        Outcome::Refused(why) => {
            debug!(remote = %h.remote, why, "connection refused");
            if let Some(a) = dialed {
                if why == "already connected" {
                    s.book.mark_closed(&a, now, cfg.gossip_interval);
                } else {
                    let mut rng = s.rng.clone();
                    s.book.mark_failed(&a, now, &mut rng);
                    s.rng = rng;
                }
            }
            return;
        }
        Outcome::Failed(e) => {
            debug!(remote = %h.remote, "handshake failed: {e}");
            if let Some(a) = dialed {
                s.counters.dial_failures += 1;
                let mut rng = s.rng.clone();
                s.book.mark_failed(&a, now, &mut rng);
                s.rng = rng;
            }
            return;
        }
    };
    let inbound = dialed.is_none();
    if s.is_banned(&key, now) {
        if let Some(a) = dialed {
            s.book.mark_closed(&a, now, cfg.ban_duration);
        }
        return;
    }
    // Duplicate link: keep the one dialed by the smaller key (both ends agree).
    if let Some(existing) = s.peers.get(&key) {
        let me = s.me.clone();
        let dialer_new = if inbound { key.clone() } else { me.clone() };
        let dialer_old = if existing.inbound { key.clone() } else { me.clone() };
        let keep_new = dialer_new != dialer_old && dialer_new == std::cmp::min(me, key.clone());
        if !keep_new {
            if let Some(a) = dialed {
                s.book.mark_closed(&a, now, cfg.gossip_interval);
            }
            return;
        }
        s.remove_peer(&key, None, now);
    }
    if inbound && s.inbound_count() >= cfg.max_inbound {
        // Full: hand the dialer an address sample, say why, hang up.
        s.counters.inbound_full += 1;
        let sample = s.addr_sample(&key, cfg.addr_reply, now);
        drop(s);
        let mut sender = sender;
        ctx.child("full").spawn(move |_| async move {
            let _ = sender.send(IoBufs::from(wire::encode(&Msg::Addr(sample)))).await;
            let _ = sender.send(IoBufs::from(wire::encode(&Msg::Bye(wire::BYE_FULL)))).await;
            drop(receiver);
        });
        return;
    }
    if !inbound && s.outbound_count() >= cfg.max_outbound {
        if let Some(a) = dialed {
            s.book.mark_closed(&a, now, cfg.backoff_base);
        }
        return;
    }
    let conn_id = s.next_conn;
    s.next_conn += 1;
    let (mut qtx, qrx) = futures::channel::mpsc::channel::<IoBufs>(cfg.queue_size);
    let _ = qtx.try_send(IoBufs::from(wire::encode(&Msg::Hello { version: wire::VERSION, listen: cfg.advertised() })));
    if !inbound {
        let _ = qtx.try_send(IoBufs::from(wire::encode(&Msg::GetAddr)));
        let a = dialed.unwrap();
        s.book.mark_success(&a, &key, now, ms(now));
    }
    s.peers.insert(
        key.clone(),
        PeerEntry {
            conn_id,
            inbound,
            remote: h.remote,
            listen: dialed,
            queue: qtx,
            handle: None,
            last_getaddr: None,
            addr_tokens: cfg.addr_burst,
            addr_refill: now,
        },
    );
    debug!(peer = ?key, remote = %h.remote, inbound, "peer connected");
    s.changed();
    drop(s);
    let (sh, ch, c2, k2) = (shared.clone(), channels.clone(), cfg.clone(), key.clone());
    let handle = ctx.child("peer").spawn(move |c| conn_task(c, sh, ch, c2, k2, conn_id, sender, receiver, qrx));
    let mut s = lock(shared);
    match s.peers.get_mut(&key) {
        Some(p) if p.conn_id == conn_id => p.handle = Some(handle),
        _ => handle.abort(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn conn_task<E, Si, St>(
    ctx: E,
    shared: Arc<Mutex<Shared>>,
    channels: Channels,
    cfg: Arc<Config>,
    key: PublicKey,
    conn_id: u64,
    mut sender: encrypted::Sender<Si>,
    receiver: encrypted::Receiver<St>,
    mut queue: futures::channel::mpsc::Receiver<IoBufs>,
) where
    E: Spawner + Clock + Send + Sync + 'static,
    Si: commonware_runtime::Sink,
    St: commonware_runtime::Stream,
{
    let last_recv = Arc::new(AtomicU64::new(ms(ctx.current())));
    let reader = {
        let (sh, lr, k, c2) = (shared.clone(), last_recv.clone(), key.clone(), cfg.clone());
        ctx.child("reader").spawn(move |c| reader_task(c, sh, channels, c2, k, conn_id, receiver, lr))
    };
    let mut next_ping = ctx.current() + cfg.ping_interval;
    let mut nonce = 0u64;
    loop {
        select! {
            m = queue.next() => {
                match m {
                    Some(b) => {
                        if sender.send(b).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            },
            _ = ctx.sleep_until(next_ping) => {
                let now = ctx.current();
                if ms(now).saturating_sub(last_recv.load(Ordering::Relaxed)) > cfg.idle_timeout.as_millis() as u64 {
                    debug!(peer = ?key, "idle timeout");
                    break;
                }
                nonce += 1;
                if sender.send(IoBufs::from(wire::encode(&Msg::Ping(nonce)))).await.is_err() {
                    break;
                }
                next_ping = now + cfg.ping_interval;
            },
        }
    }
    reader.abort();
    let now = ctx.current();
    lock(&shared).remove_peer(&key, Some(conn_id), now);
}

#[allow(clippy::too_many_arguments)]
async fn reader_task<E, St>(
    ctx: E,
    shared: Arc<Mutex<Shared>>,
    channels: Channels,
    cfg: Arc<Config>,
    key: PublicKey,
    conn_id: u64,
    mut rx: encrypted::Receiver<St>,
    last_recv: Arc<AtomicU64>,
) where
    E: Clock,
    St: commonware_runtime::Stream,
{
    let mut buckets: HashMap<u64, (f64, std::time::SystemTime)> = HashMap::new();
    let mut senders: HashMap<u64, _> = channels.iter().map(|(c, ch)| (*c, ch.tx.clone())).collect();
    loop {
        let bufs = match rx.recv().await {
            Ok(b) => b,
            Err(e) => {
                debug!(peer = ?key, "recv: {e}");
                break;
            }
        };
        let now = ctx.current();
        last_recv.store(ms(now), Ordering::Relaxed);
        let msg = match wire::decode(bufs.coalesce()) {
            Ok(m) => m,
            Err(e) => {
                lock(&shared).misbehave(&key, 20, e.0, now);
                break;
            }
        };
        match msg {
            Msg::Data { channel, payload } => {
                let Some(ch) = channels.get(&channel) else {
                    if lock(&shared).misbehave(&key, 10, "unknown channel", now) {
                        return;
                    }
                    continue;
                };
                let b = buckets.entry(channel).or_insert((ch.burst, now));
                let dt = now.duration_since(b.1).unwrap_or(Duration::ZERO).as_secs_f64();
                b.0 = (b.0 + dt * ch.rate).min(ch.burst);
                b.1 = now;
                if b.0 < 1.0 {
                    let mut s = lock(&shared);
                    s.counters.rate_dropped += 1;
                    if s.misbehave(&key, 1, "message rate", now) {
                        return;
                    }
                    continue;
                }
                b.0 -= 1.0;
                let tx = senders.get_mut(&channel).unwrap();
                let _ = tx.send((key.clone(), payload)).await;
            }
            Msg::Hello { version, listen } => {
                if !lock(&shared).on_hello(&key, conn_id, version, listen, now) {
                    break;
                }
            }
            Msg::Ping(n) => {
                lock(&shared).send_ctrl(&key, &Msg::Pong(n));
            }
            Msg::Pong(_) => {}
            Msg::GetAddr => lock(&shared).on_getaddr(&key, now),
            Msg::Addr(list) => {
                let mut s = lock(&shared);
                let fresh = s.on_addr(&key, list, now);
                s.relay(&key, fresh);
            }
            Msg::Bye(reason) => {
                let mut s = lock(&shared);
                let listen = s.peers.get(&key).filter(|p| p.conn_id == conn_id && !p.inbound).and_then(|p| p.listen);
                s.remove_peer(&key, Some(conn_id), now);
                if reason == wire::BYE_FULL {
                    if let Some(a) = listen {
                        s.book.mark_closed(&a, now, cfg.full_backoff);
                    }
                }
                return;
            }
        }
    }
    let now = ctx.current();
    lock(&shared).remove_peer(&key, Some(conn_id), now);
}
