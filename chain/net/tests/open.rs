//! Open network on Commonware's deterministic runtime (in-memory sockets, simulated
//! time). Nodes join knowing only a seed and must find each other through gossip.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use commonware_cryptography::{ed25519, Signer as _};
use commonware_p2p::{Blocker as _, Receiver as _, Recipients, Sender as _};
use commonware_runtime::{deterministic, Clock, IoBuf, Quota, Runner as _, Spawner as _, Supervisor as _};
use net::open::testing::GatedNetwork;
use net::open::{Config, Network, Oracle, Receiver, Seed, Sender};

const CH: u64 = 7;

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn key(i: u64) -> ed25519::PublicKey {
    ed25519::PrivateKey::from_seed(i).public_key()
}

fn cfg(i: u64, port: u16, seeds: &[u16]) -> Config {
    let mut c = Config::local(ed25519::PrivateKey::from_seed(i), b"pearl-sol:test", addr(port));
    c.seeds = seeds.iter().map(|p| Seed { addr: addr(*p), key: None }).collect();
    c.dial_interval = Duration::from_millis(200);
    c.gossip_interval = Duration::from_secs(1);
    c.ping_interval = Duration::from_secs(1);
    c.idle_timeout = Duration::from_secs(5);
    c.backoff_base = Duration::from_millis(200);
    c.backoff_max = Duration::from_secs(2);
    c.full_backoff = Duration::from_secs(2);
    c.max_outbound = 4;
    c
}

struct Node {
    oracle: Oracle,
    sender: Sender<deterministic::Context>,
    receiver: Receiver,
    down: Arc<AtomicBool>,
}

fn start(ctx: &deterministic::Context, i: u64, c: Config, quota: u32) -> Node {
    let (gated, down) = GatedNetwork::new(ctx.child("net"));
    let (mut n, oracle) = Network::new(ctx.child("node").with_attribute("i", i), gated, c);
    let (sender, receiver) = n.register(CH, Quota::per_second(std::num::NonZeroU32::new(quota).unwrap()), 1024);
    n.start();
    Node { oracle, sender, receiver, down }
}

fn runner(seed: u64) -> deterministic::Runner {
    // NET_TEST_SEED shifts every seed (to soak the tests over many schedules).
    let seed = seed + std::env::var("NET_TEST_SEED").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1000;
    deterministic::Runner::new(deterministic::Config::new().with_seed(seed).with_timeout(Some(Duration::from_secs(3600))))
}

/// Connected components of the "is connected to" graph.
fn components(nodes: &[&Node]) -> usize {
    let keys: Vec<_> = nodes.iter().map(|n| n.oracle.me()).collect();
    let mut comp: Vec<usize> = (0..nodes.len()).collect();
    fn find(c: &mut Vec<usize>, i: usize) -> usize {
        if c[i] != i {
            let r = find(c, c[i]);
            c[i] = r;
        }
        c[i]
    }
    for (i, n) in nodes.iter().enumerate() {
        for p in n.oracle.peers() {
            if let Some(j) = keys.iter().position(|k| *k == p.key) {
                let (a, b) = (find(&mut comp, i), find(&mut comp, j));
                comp[a] = b;
            }
        }
    }
    (0..nodes.len()).filter(|i| find(&mut comp, *i) == *i).count()
}

#[test]
fn nodes_join_via_one_seed_and_discover_each_other() {
    runner(1).start(|ctx| async move {
        let mut nodes = vec![start(&ctx, 0, cfg(0, 3000, &[]), 1000)];
        for i in 1..7u64 {
            nodes.push(start(&ctx, i, cfg(i, 3000 + i as u16, &[3000]), 1000));
            ctx.sleep(Duration::from_millis(300)).await;
        }
        ctx.sleep(Duration::from_secs(20)).await;
        let refs: Vec<&Node> = nodes.iter().collect();
        assert_eq!(components(&refs), 1);
        let seed = key(0);
        for (i, n) in nodes.iter().enumerate().skip(1) {
            let peers = n.oracle.peers();
            assert!(peers.len() >= 2, "node {i} has {} peers", peers.len());
            assert!(peers.iter().any(|p| p.key != seed), "node {i} only knows the seed");
            assert!(n.oracle.known_addrs().len() >= 4, "node {i} address book {:?}", n.oracle.known_addrs());
            let st = n.oracle.stats();
            assert!(st.outbound >= 1 && st.outbound <= 4, "{st:?}");
        }
        // a message from the last joiner reaches every peer it is connected to
        let last = nodes.len() - 1;
        let to: Vec<_> = nodes[last].oracle.peers().into_iter().map(|p| p.key).collect();
        let sent = nodes[last].sender.send(Recipients::All, IoBuf::from(b"hello".to_vec()), false);
        assert_eq!(sent.len(), to.len());
        for n in nodes.iter_mut() {
            if to.contains(&n.oracle.me()) {
                let (from, msg) = n.receiver.recv().await.unwrap();
                assert_eq!(from, key(last as u64));
                assert_eq!(msg.as_ref(), b"hello");
            }
        }
    });
}

#[test]
fn inbound_cap_is_enforced_and_joiners_still_mesh() {
    runner(2).start(|ctx| async move {
        let mut c0 = cfg(0, 3100, &[]);
        c0.max_inbound = 2;
        let mut nodes = vec![start(&ctx, 0, c0, 1000)];
        for i in 1..6u64 {
            let mut c = cfg(i, 3100 + i as u16, &[3100]);
            c.max_outbound = 3;
            nodes.push(start(&ctx, i, c, 1000));
            ctx.sleep(Duration::from_millis(500)).await;
        }
        let mut max_seen = 0;
        for _ in 0..300 {
            let s = nodes[0].oracle.stats();
            max_seen = max_seen.max(s.inbound);
            assert!(s.inbound <= 2, "seed has {} inbound", s.inbound);
            ctx.sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(max_seen, 2);
        let st = nodes[0].oracle.stats();
        assert!(st.inbound_full > 0, "{st:?}");
        let refs: Vec<&Node> = nodes.iter().collect();
        assert_eq!(components(&refs), 1, "joiners turned away by the seed still found each other");
    });
}

#[test]
fn spammer_is_scored_and_banned() {
    runner(3).start(|ctx| async move {
        // victim allows 5 msgs/s on the channel (10/s with slack); the spammer registers
        // a huge quota and floods.
        let victim = start(&ctx, 0, cfg(0, 3200, &[]), 5);
        let spam = start(&ctx, 1, cfg(1, 3201, &[3200]), 1_000_000);
        ctx.sleep(Duration::from_secs(3)).await;
        assert!(victim.oracle.is_connected(&key(1)));
        let mut s = spam.sender.clone();
        for _ in 0..400 {
            s.send(Recipients::All, IoBuf::from(vec![0u8; 16]), false);
        }
        ctx.sleep(Duration::from_secs(2)).await;
        assert!(victim.oracle.is_banned(&key(1)));
        assert!(!victim.oracle.is_connected(&key(1)));
        assert!(victim.oracle.stats().rate_dropped >= 90);
        // the spammer keeps redialing (it is our seed) and is refused every time
        ctx.sleep(Duration::from_secs(10)).await;
        assert_eq!(spam.oracle.connected_count(), 0);
        assert!(!victim.oracle.is_connected(&key(1)));
        assert!(spam.oracle.stats().dial_failures > 0);
    });
}

#[test]
fn blocker_bans_and_refuses_reconnect() {
    runner(4).start(|ctx| async move {
        let mut a = start(&ctx, 0, cfg(0, 3300, &[]), 1000);
        let b = start(&ctx, 1, cfg(1, 3301, &[3300]), 1000);
        ctx.sleep(Duration::from_secs(3)).await;
        assert!(a.oracle.is_connected(&key(1)) && b.oracle.is_connected(&key(0)));
        let mut blocked = a.oracle.blocked();
        a.oracle.block(key(1));
        let set = blocked.recv().await.unwrap();
        assert!(set.position(&key(1)).is_some());
        ctx.sleep(Duration::from_secs(10)).await;
        assert!(!a.oracle.is_connected(&key(1)));
        assert_eq!(b.oracle.connected_count(), 0);
        // an unknown channel id costs score but does not ban at once
        let c = start(&ctx, 2, cfg(2, 3302, &[3300]), 1000);
        ctx.sleep(Duration::from_secs(3)).await;
        assert!(a.oracle.is_connected(&key(2)));
        assert!(a.oracle.misbehave(&key(2), 50, "test") == false);
        assert_eq!(a.oracle.score(&key(2)), 50);
        assert!(a.oracle.misbehave(&key(2), 50, "test"));
        ctx.sleep(Duration::from_millis(500)).await;
        assert!(!a.oracle.is_connected(&key(2)));
        drop(c);
        let _ = &mut a;
    });
}

#[test]
fn partitioned_node_rejoins_through_gossiped_addresses() {
    runner(5).start(|ctx| async move {
        let seed = start(&ctx, 0, cfg(0, 3400, &[]), 1000);
        let n1 = start(&ctx, 1, cfg(1, 3401, &[3400]), 1000);
        let n2 = start(&ctx, 2, cfg(2, 3402, &[3400]), 1000);
        let n3 = start(&ctx, 3, cfg(3, 3403, &[3400]), 1000);
        ctx.sleep(Duration::from_secs(15)).await;
        assert!(n3.oracle.known_addrs().iter().any(|(a, _, _)| *a == addr(3401) || *a == addr(3402)));
        // cut node 3 off, and take the seed down for good
        n3.down.store(true, Ordering::SeqCst);
        seed.down.store(true, Ordering::SeqCst);
        ctx.sleep(Duration::from_secs(10)).await;
        assert_eq!(n3.oracle.connected_count(), 0);
        assert!(!n1.oracle.is_connected(&key(3)) && !n2.oracle.is_connected(&key(3)));
        // heal node 3: it can only come back through addresses it learned by gossip
        n3.down.store(false, Ordering::SeqCst);
        ctx.sleep(Duration::from_secs(10)).await;
        let peers: Vec<_> = n3.oracle.peers().into_iter().map(|p| p.key).collect();
        assert!(peers.contains(&key(1)) || peers.contains(&key(2)), "node 3 peers {peers:?}");
        assert!(!peers.contains(&key(0)));
        let refs = [&n1, &n2, &n3];
        assert_eq!(components(&refs), 1);
    });
}

#[test]
fn pinned_seed_key_self_dial_and_foreign_namespace() {
    runner(6).start(|ctx| async move {
        let a = start(&ctx, 0, cfg(0, 3500, &[3500]), 1000); // own address as a seed
        // wrong pinned key for a's address
        let mut cb = cfg(1, 3501, &[]);
        cb.seeds = vec![Seed { addr: addr(3500), key: Some(key(99)) }];
        let b = start(&ctx, 1, cb, 1000);
        // other chain (namespace)
        let mut cc = cfg(2, 3502, &[3500]);
        cc.namespace = b"another chain".to_vec();
        let c = start(&ctx, 2, cc, 1000);
        // right pinned key
        let mut cd = cfg(3, 3503, &[]);
        cd.seeds = vec![Seed { addr: addr(3500), key: Some(key(0)) }];
        let d = start(&ctx, 3, cd, 1000);
        ctx.sleep(Duration::from_secs(8)).await;
        assert!(!a.oracle.known_addrs().iter().any(|(x, _, _)| *x == addr(3500)), "own address dropped from the book");
        assert_eq!(b.oracle.connected_count(), 0);
        assert!(b.oracle.stats().dial_failures == 0 && b.oracle.stats().dials > 0);
        assert_eq!(c.oracle.connected_count(), 0);
        assert!(c.oracle.stats().dial_failures > 0);
        assert!(d.oracle.is_connected(&key(0)));
        assert!(!a.oracle.is_connected(&key(1)) && !a.oracle.is_connected(&key(2)));
    });
}

#[test]
fn pinned_seeds_in_one_slash16_all_connect() {
    // the public devnet's three boxes share 64.95.0.0/16; outbound diversity (one per /16)
    // left boxes 1 and 2 connected only to box 0. Key-pinned seeds are exempt.
    runner(11).start(|ctx| async move {
        let ips = [[64, 95, 12, 116], [64, 95, 13, 213], [64, 95, 12, 22]];
        let a = |i: usize| SocketAddr::from((ips[i], 10640));
        let mut nodes = Vec::new();
        for i in 0..3usize {
            let mut c = cfg(i as u64, 0, &[]);
            c.listen = a(i);
            c.external = Some(a(i));
            c.allow_private = false;
            c.seeds = (0..3).filter(|j| *j != i).map(|j| Seed { addr: a(j), key: Some(key(j as u64)) }).collect();
            nodes.push(start(&ctx, i as u64, c, 1000));
        }
        ctx.sleep(Duration::from_secs(20)).await;
        for (i, n) in nodes.iter().enumerate() {
            let peers = n.oracle.peers();
            assert_eq!(peers.len(), 2, "node {i} peers {:?}", peers.iter().map(|p| p.remote).collect::<Vec<_>>());
        }
    });
}

/// A follower that is pulling many large blobs from a peer still gets that peer's block and
/// tip gossip promptly: bulk frames (blob channels, `register_bulk`) wait behind the normal
/// lane and overflow on their own queue, never the normal one. Before 2026-10-21 both shared
/// one 1024-frame queue per peer: on a busy link, blocks and tips queued behind megabytes of
/// blob answers or were dropped when the queue was full, and a synced node stopped following
/// the tip while its peers still listed it as connected (HANDOFF item 24).
#[test]
fn normal_lane_is_not_held_behind_bulk_frames() {
    const BULK: u64 = 8;
    runner(41).start(|ctx| async move {
        // 1 MB/s link in each direction (sends take simulated time)
        let mk = |i: u64, port: u16, seeds: &[u16]| {
            let net = net::open::testing::ThrottledNetwork::new(ctx.child("net"), ctx.child("clock"), 1_000_000.0);
            let (mut n, oracle) = Network::new(ctx.child("node").with_attribute("i", i), net, cfg(i, port, seeds));
            let q = Quota::per_second(std::num::NonZeroU32::new(100_000).unwrap());
            let normal = n.register(CH, q, 4096);
            let bulk = n.register_bulk(BULK, q, 4096);
            n.start();
            (oracle, normal, bulk)
        };
        let (oa, (mut sa, _ra), (mut ba, _bra)) = mk(0, 3400, &[]);
        let (ob, (_sb, mut rb), (_bb, mut brb)) = mk(1, 3401, &[3400]);
        while oa.peers().is_empty() || ob.peers().is_empty() {
            ctx.sleep(Duration::from_millis(100)).await;
        }
        // drain B's bulk channel as it arrives (as the node's blob consumers do)
        let bulk_got = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let bg = bulk_got.clone();
        ctx.child("bulk_rx").spawn(move |_| async move {
            while brb.recv().await.is_ok() {
                bg.fetch_add(1, Ordering::SeqCst);
            }
        });
        // 200 bulk frames of 256 KiB: 52 MB, about 52 s on this link, far past the bulk queue
        let frame = vec![7u8; 256 * 1024];
        for _ in 0..200 {
            let _ = ba.send(Recipients::All, IoBuf::from(frame.clone()), false);
        }
        // then a burst of normal messages (blocks, tips): every one arrives, within a few
        // seconds (at most one bulk frame, 0.26 s, ahead of each)
        let t0 = ctx.current();
        for i in 0..100u32 {
            let _ = sa.send(Recipients::All, IoBuf::from(i.to_be_bytes().to_vec()), false);
        }
        for i in 0..100u32 {
            let (from, msg) = rb.recv().await.unwrap();
            assert_eq!(from, oa.me());
            assert_eq!(msg.as_ref(), i.to_be_bytes(), "normal messages arrive in order, none dropped");
        }
        let took = ctx.current().duration_since(t0).unwrap();
        assert!(took < Duration::from_secs(3), "normal lane waited {took:?} behind bulk frames");
        let st = oa.stats();
        assert_eq!(st.queue_dropped, 0, "{st:?}");
        assert!(st.bulk_dropped > 0, "the bulk queue overflowed on its own: {st:?}");
        // the bulk frames that were queued still arrive, after the normal lane is empty
        ctx.sleep(Duration::from_secs(30)).await;
        assert!(bulk_got.load(Ordering::SeqCst) >= 10, "bulk frames still flow");
    });
}
