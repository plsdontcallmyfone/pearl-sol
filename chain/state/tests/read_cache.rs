//! The state read cache (store.rs module doc) never changes what a read returns: random block
//! trees with forks, deletes and finalization, every key read at every live snapshot (and the
//! committed one) against a plain model of each branch's state, with the cache on and off.

use std::collections::BTreeMap;

use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use state::{Snapshot, StateRead, Store, View, WriteSet};

const KEYS: u8 = 24;

struct Node {
    snap: Snapshot,
    model: BTreeMap<u8, Vec<u8>>,
    height: u64,
    parent: Option<usize>,
}

fn run(seed: u64, cache_bytes: usize) {
    deterministic::Runner::seeded(seed).start(|context| async move {
        let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut next = move |m: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % m
        };
        let mut store = Store::open(context.child("s"), "t").await.unwrap();
        store.set_cache_bytes(cache_bytes);
        let mut nodes = vec![Node { snap: store.committed().clone(), model: BTreeMap::new(), height: 0, parent: None }];
        let mut committed = 0usize;
        for step in 0..60u64 {
            // extend a random live node that descends from the committed one
            let live: Vec<usize> = (0..nodes.len()).filter(|&i| descends(&nodes, i, committed)).collect();
            let p = live[next(live.len() as u64) as usize];
            let mut w = WriteSet::default();
            let mut model = nodes[p].model.clone();
            for _ in 0..1 + next(5) {
                let k = next(KEYS as u64) as u8;
                if next(4) == 0 {
                    w.writes.insert([k; 32], None);
                    model.remove(&k);
                } else {
                    let v = format!("{step}-{k}").into_bytes();
                    w.writes.insert([k; 32], Some(v.clone()));
                    model.insert(k, v);
                }
            }
            let h = nodes[p].height + 1;
            let snap = store.build(&nodes[p].snap, &w, h).await.unwrap();
            nodes.push(Node { snap, model, height: h, parent: Some(p) });
            // reads at every live snapshot and at the committed one, twice (miss, then hit)
            for _ in 0..2 {
                for i in (0..nodes.len()).filter(|&i| descends(&nodes, i, committed)) {
                    let v = View { store: &store, snap: &nodes[i].snap };
                    for k in 0..KEYS {
                        let got = v.get(&[k; 32]).await.unwrap();
                        assert_eq!(got.as_ref(), nodes[i].model.get(&k), "seed {seed} step {step} node {i} key {k}");
                    }
                }
                let v = View { store: &store, snap: store.committed() };
                for k in 0..KEYS {
                    assert_eq!(v.get(&[k; 32]).await.unwrap().as_ref(), nodes[committed].model.get(&k), "committed, key {k}");
                }
            }
            // sometimes finalize a descendant of the committed node, at most 3 deep
            if next(3) == 0 {
                let cands: Vec<usize> = (0..nodes.len())
                    .filter(|&i| i != committed && descends(&nodes, i, committed) && nodes[i].height <= nodes[committed].height + 3)
                    .collect();
                if !cands.is_empty() {
                    let t = cands[next(cands.len() as u64) as usize];
                    let s = nodes[t].snap.clone();
                    store.finalize(&s).await.unwrap();
                    committed = t;
                }
            }
        }
        if cache_bytes > 0 {
            let hits = store.cache().hits.load(std::sync::atomic::Ordering::Relaxed);
            assert!(hits > 0, "the cache answered nothing");
        }
    });
}

fn descends(nodes: &[Node], mut i: usize, anc: usize) -> bool {
    loop {
        if i == anc {
            return true;
        }
        match nodes[i].parent {
            Some(p) => i = p,
            None => return false,
        }
    }
}

#[test]
fn cached_reads_equal_the_branch_state() {
    for seed in 1..=12 {
        run(seed, state::store::DEFAULT_CACHE_BYTES);
    }
}

#[test]
fn tiny_cache_budget_rotates_and_stays_correct() {
    for seed in 13..=18 {
        run(seed, 2048);
    }
}

#[test]
fn cache_off_matches_too() {
    run(99, 0);
}
