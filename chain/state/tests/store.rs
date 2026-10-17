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
