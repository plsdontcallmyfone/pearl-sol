//! Contract storage is provable against a header's state_root (docs/CONTRACTS.md "Proofs"):
//! a QMDB current-value proof of `["cstore", contract || key]` from the committed state.
//! Also an ignored measurement of merkleizing state writes (gas schedule input).

use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use state::contracts as sc;
use state::{Store, WriteSet};

#[test]
fn contract_storage_proof_verifies() {
    deterministic::Runner::default().start(|context| async move {
        let mut store = Store::open(context.child("a"), "t").await.unwrap();
        let base = store.committed().clone();
        let c = [7u8; 32];
        let k = sc::store_key(&c, b"n");
        let v = sc::encode_store(b"n", &42u64.to_le_bytes());
        let mut ws = WriteSet::default();
        ws.writes.insert(k, Some(v.clone()));
        ws.writes.insert(sc::store_key(&c, b"other"), Some(sc::encode_store(b"other", b"x")));
        let s1 = store.build(&base, &ws, 1).await.unwrap();
        store.finalize(&s1).await.unwrap();
        let root = store.committed().state_root;
        let proof = store.state_proof(&k).await.unwrap();
        assert!(Store::<deterministic::Context>::verify_state_proof(&k, &v, &proof, &root));
        // wrong value, wrong key, wrong root
        let bad = sc::encode_store(b"n", &43u64.to_le_bytes());
        assert!(!Store::<deterministic::Context>::verify_state_proof(&k, &bad, &proof, &root));
        assert!(!Store::<deterministic::Context>::verify_state_proof(&sc::store_key(&c, b"m"), &v, &proof, &root));
        assert!(!Store::<deterministic::Context>::verify_state_proof(&k, &v, &proof, &[0; 32]));
        // after an overwrite the old proof no longer verifies against the new root
        let mut ws = WriteSet::default();
        ws.writes.insert(k, Some(bad.clone()));
        let s2 = store.build(&s1, &ws, 2).await.unwrap();
        store.finalize(&s2).await.unwrap();
        let root2 = store.committed().state_root;
        assert!(!Store::<deterministic::Context>::verify_state_proof(&k, &v, &proof, &root2));
        let proof2 = store.state_proof(&k).await.unwrap();
        assert!(Store::<deterministic::Context>::verify_state_proof(&k, &bad, &proof2, &root2));
        println!("state proof: {} bytes", proof2.len());
    });
}

/// `cargo test --release -p state --test contract_proof -- --ignored --nocapture`
#[test]
#[ignore]
fn measure_merkleize_writes() {
    deterministic::Runner::default().start(|context| async move {
        let store = Store::open(context.child("a"), "m").await.unwrap();
        let mut snap = store.committed().clone();
        for round in 0..3u32 {
            let n = 10_000u32;
            let mut ws = WriteSet::default();
            for i in 0..n {
                let key = [&round.to_le_bytes()[..], &i.to_le_bytes()[..]].concat();
                ws.writes.insert(sc::store_key(&[1; 32], &key), Some(sc::encode_store(&key, &[9u8; 64])));
            }
            let t = std::time::Instant::now();
            snap = store.build(&snap, &ws, round as u64 + 1).await.unwrap();
            let el = t.elapsed();
            println!("merkleize {n} writes of 64 B: {el:?} = {:.0} ns/write", el.as_nanos() as f64 / n as f64);
        }
    });
}
