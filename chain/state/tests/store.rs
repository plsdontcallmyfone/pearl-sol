use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use state::{Store, View, WriteSet, StateRead};

fn ws(pairs: &[(u8, Option<&[u8]>)], withdrawals: &[&[u8]]) -> WriteSet {
    let mut w = WriteSet::default();
    for (k, v) in pairs {
        w.writes.insert([*k; 32], v.map(|b| b.to_vec()));
    }
    w.withdrawals = withdrawals.iter().map(|b| b.to_vec()).collect();
    w
}

#[test]
fn fork_finalize_reopen() {
    deterministic::Runner::default().start(|context| async move {
        let mut store = Store::open(context.child("a"), "t").await.unwrap();
        assert_eq!(store.committed_height().await.unwrap(), None);
        let base = store.committed().clone();
        let a1 = store.build(&base, &ws(&[(1, Some(b"x"))], &[]), 1).await.unwrap();
        let b1 = store.build(&base, &ws(&[(1, Some(b"y"))], &[b"w0"]), 1).await.unwrap();
        assert_ne!(a1.state_root, b1.state_root);
        assert_eq!(a1.withdraw_root, base.withdraw_root);
        assert_ne!(b1.withdraw_root, base.withdraw_root);
        let a2 = store.build(&a1, &ws(&[(2, Some(b"z"))], &[b"w1"]), 2).await.unwrap();
        // reads through the branch
        let v = View { store: &store, snap: &a2 };
        assert_eq!(v.get(&[1; 32]).await.unwrap(), Some(b"x".to_vec()));
        assert_eq!(v.get(&[2; 32]).await.unwrap(), Some(b"z".to_vec()));
        let v = View { store: &store, snap: &b1 };
        assert_eq!(v.get(&[1; 32]).await.unwrap(), Some(b"y".to_vec()));
        assert_eq!(v.get(&[2; 32]).await.unwrap(), None);
        // same writes give the same root (determinism)
        let a1b = store.build(&base, &ws(&[(1, Some(b"x"))], &[]), 1).await.unwrap();
        assert_eq!(a1b.state_root, a1.state_root);
        // finalize a1, then extend and finalize a2
        store.finalize(&a1).await.unwrap();
        assert_eq!(store.committed_height().await.unwrap(), Some(1));
        let a3 = store.build(&a2, &ws(&[(1, None)], &[]), 3).await.unwrap();
        store.finalize(&a2).await.unwrap();
        let root2 = store.committed().state_root;
        let wroot2 = store.committed().withdraw_root;
        assert_eq!(root2, a2.state_root);
        assert_eq!(wroot2, a2.withdraw_root);
        let v = View { store: &store, snap: &a3 };
        assert_eq!(v.get(&[1; 32]).await.unwrap(), None);
        // the withdrawal is in the committed log with a proof
        let size = store.log_size();
        let mut found = None;
        for loc in 0..size {
            if let Some(e) = store.log_get(loc).await.unwrap() {
                found = Some((loc, e));
            }
        }
        let (loc, e) = found.unwrap();
        assert_eq!(e, b"w1".to_vec());
        let (proof, root) = store.withdrawal_proof(loc).await.unwrap();
        assert_eq!(root, wroot2);
        assert!(!proof.is_empty());
        drop(store);
        let store = Store::open(context.child("b"), "t").await.unwrap();
        assert_eq!(store.committed().state_root, root2);
        assert_eq!(store.committed().withdraw_root, wroot2);
        assert_eq!(store.committed_height().await.unwrap(), Some(2));
    });
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn verifies(proof: &[u8], entry: &[u8], loc: u64, root: &[u8; 32]) -> bool {
    let mut el = vec![0u8; 11 + entry.len()];
    let n = pearl_verify_sbf::mmr::append_element(entry, &mut el);
    pearl_verify_sbf::mmr::verify_inclusion(sha, proof, &el[..n], loc, root).is_ok()
}

/// getWithdrawalProof at_height: a proof against an OLDER final withdraw_root verifies
/// with the light client's verifier, at every commit boundary, including blocks that
/// were finalized together in one commit; the current proof does not verify against an
/// old root and vice versa.
#[test]
fn historical_withdrawal_proofs_verify_against_older_roots() {
    deterministic::Runner::default().start(|context| async move {
        let mut store = Store::open(context.child("h"), "t").await.unwrap();
        let mut snap = store.committed().clone();
        // heights 1..=6; withdrawals in 1 (two), 2 (none), 3 (one), 4 (one), 5 (none), 6 (one)
        let plan: [&[&[u8]]; 6] = [&[b"w0", b"w1"], &[], &[b"w2"], &[b"w3"], &[], &[b"w4"]];
        let mut per_height = vec![]; // (height, log_size, withdraw_root)
        let mut locs = vec![]; // (entry, loc, block height)
        let mut snaps = vec![];
        for (i, ws_) in plan.iter().enumerate() {
            let h = i as u64 + 1;
            let base = snap.log_size;
            let next = store.build(&snap, &ws(&[(h as u8, Some(b"v"))], ws_), h).await.unwrap();
            for (j, e) in ws_.iter().enumerate() {
                locs.push((e.to_vec(), base + j as u64, h));
            }
            per_height.push((h, next.log_size, next.withdraw_root));
            snaps.push(next.clone());
            snap = next;
            // Finality lags the tip (as in the chain, children are built before their
            // parent is final): finalize 1 after 2 exists, then 2..=4 together and
            // 5..=6 together once 6 exists.
            if h == 2 {
                store.finalize(&snaps[0]).await.unwrap();
            }
            if h == 6 {
                store.finalize(&snaps[3]).await.unwrap();
                store.finalize(&snaps[5]).await.unwrap();
            }
        }
        for (entry, loc, wh) in &locs {
            assert_eq!(store.log_get(*loc).await.unwrap().as_deref(), Some(entry.as_slice()));
            for &(h, size, root) in &per_height {
                if h < *wh {
                    continue;
                }
                let proof = store.withdrawal_proof_at(*loc, size).await.unwrap();
                assert!(verifies(&proof, entry, *loc, &root), "entry at {loc} (block {wh}) against the root of height {h}");
                if root != store.committed().withdraw_root {
                    let (cur, _) = store.withdrawal_proof(*loc).await.unwrap();
                    assert!(!verifies(&cur, entry, *loc, &root), "the current proof must not verify against an older root");
                    assert!(!verifies(&proof, entry, *loc, &store.committed().withdraw_root));
                }
            }
        }
        // a location beyond the historical size is refused
        let (_, size1, _) = per_height[0];
        assert!(store.withdrawal_proof_at(size1, size1).await.is_err());
    });
}

/// Regression (2026-10-17, reported by the refund agent as "journal error: item out of
/// range" after finalizing several blocks together): a caller that keeps only the tip
/// snapshot used to lose the uncommitted ancestors' batches (Commonware batches hold a
/// Weak parent), so the next build panicked ("left child missing") or a read failed.
/// Snapshots now keep their uncommitted ancestors alive; this holds only the tip.
#[test]
fn tip_only_caller_builds_reads_and_finalizes() {
    deterministic::Runner::default().start(|context| async move {
        let mut store = Store::open(context.child("p"), "t").await.unwrap();
        let mut snap = store.committed().clone();
        let plan: [&[&[u8]]; 8] = [&[b"w0", b"w1"], &[], &[b"w2"], &[b"w3"], &[], &[b"w4"], &[], &[b"w5"]];
        for (i, w) in plan.iter().enumerate() {
            let h = i as u64 + 1;
            snap = store.build(&snap, &ws(&[(h as u8, Some(b"v"))], w), h).await.unwrap();
            let v = View { store: &store, snap: &snap };
            for k in 1..=h as u8 {
                assert_eq!(v.get(&[k; 32]).await.unwrap(), Some(b"v".to_vec()), "key {k} at {h}");
            }
            if h == 5 {
                // 1..=5 final together; keep building on the tip only
                let t = snap.clone();
                store.finalize(&t).await.unwrap();
            }
        }
        let t = snap.clone();
        store.finalize(&t).await.unwrap();
        assert_eq!(store.committed_height().await.unwrap(), Some(8));
        let mut entries = 0;
        for loc in 0..store.log_size() {
            entries += store.log_get(loc).await.unwrap().is_some() as usize;
        }
        assert_eq!(entries, 6);
    });
}

/// Random schedules: build up to 12 blocks with 0-3 withdrawals each, finalize a random
/// canonical non-final block (0-4 behind the tip) at random points, build a stale
/// sibling sometimes; every root must match a store that never finalizes, every read,
/// log entry and historical proof must work.
#[test]
fn random_finalize_schedules_match_an_unfinalized_store() {
    let mut rng: u64 = 0x9e3779b97f4a7c15;
    let mut next = move |m: u64| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng % m };
    let mut fails = 0;
    for iter in 0..400 {
        let sched: Vec<(usize, Option<usize>, bool)> = (0..12).map(|_| {
            let nw = next(4) as usize;
            let fin = if next(3) == 0 { Some(next(5) as usize) } else { None };
            (nw, fin, next(4) == 0)
        }).collect();
        let s2 = sched.clone();
        let res = std::panic::catch_unwind(move || {
            deterministic::Runner::default().start(|context| async move {
                let mut store = Store::open(context.child("p"), "t").await.unwrap();
                let reference = Store::open(context.child("r"), "r").await.unwrap();
                let mut snaps = vec![store.committed().clone()];
                let mut rsnaps = vec![reference.committed().clone()];
                let mut fin = 0usize;
                let mut per_height = vec![];
                let mut locs = vec![];
                let mut stale = vec![];
                for (i, (nw, f, sib)) in s2.iter().enumerate() {
                    let h = i + 1;
                    let entries: Vec<Vec<u8>> = (0..*nw).map(|j| format!("w{h}-{j}").into_bytes()).collect();
                    let refs: Vec<&[u8]> = entries.iter().map(|e| e.as_slice()).collect();
                    let w = ws(&[(h as u8, Some(b"v")), (100, Some(format!("{h}").as_bytes()))], &refs);
                    let parent = snaps[h - 1].clone();
                    for (j, e) in entries.iter().enumerate() {
                        locs.push((e.clone(), parent.log_size + j as u64, h));
                    }
                    let s = store.build(&parent, &w, h as u64).await.map_err(|e| format!("build {h}: {e}")).unwrap();
                    let r = reference.build(&rsnaps[h - 1], &w, h as u64).await.unwrap();
                    assert_eq!((s.state_root, s.withdraw_root), (r.state_root, r.withdraw_root), "roots {h}");
                    if *sib && h - 1 >= fin {
                        let alt = store.build(&parent, &ws(&[(200, Some(b"alt"))], &[b"alt"]), h as u64).await.map_err(|e| format!("sibling {h}: {e}")).unwrap();
                        stale.push(alt);
                    }
                    per_height.push((h, s.log_size, s.withdraw_root));
                    snaps.push(s);
                    rsnaps.push(r);
                    for k in 1..=h as u8 {
                        let v = View { store: &store, snap: &snaps[h] };
                        assert_eq!(v.get(&[k; 32]).await.map_err(|e| format!("read {k} at {h}: {e}")).unwrap(), Some(b"v".to_vec()));
                    }
                    if let Some(back) = f {
                        let target = h.saturating_sub(*back);
                        if target > fin {
                            let t = snaps[target].clone();
                            store.finalize(&t).await.map_err(|e| format!("finalize {target} at {h}: {e}")).unwrap();
                            fin = target;
                            stale.clear();
                        }
                    }
                }
                let last = snaps.len() - 1;
                if fin < last {
                    let t = snaps[last].clone();
                    store.finalize(&t).await.map_err(|e| format!("final finalize: {e}")).unwrap();
                }
                for (entry, loc, wh) in &locs {
                    assert_eq!(store.log_get(*loc).await.map_err(|e| format!("log_get {loc}: {e}")).unwrap().as_deref(), Some(entry.as_slice()));
                    for &(h, size, root) in &per_height {
                        if h < *wh { continue; }
                        let proof = store.withdrawal_proof_at(*loc, size).await.map_err(|e| format!("hist {loc} at {h}: {e}")).unwrap();
                        assert!(verifies(&proof, entry, *loc, &root), "hist verify {loc} at {h}");
                    }
                }
            });
        });
        if res.is_err() {
            fails += 1;
            eprintln!("FAIL iter {iter}: {sched:?}");
        }
    }
    assert_eq!(fails, 0);
}
