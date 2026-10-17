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
