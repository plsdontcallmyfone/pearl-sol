//! Property test over proof-of-work verification (security pass 2026-10-21).
//!
//! Every peer can send a header with arbitrary proof bytes; `insert_header` and
//! `insert_block` run the verifier on them before any other work. The verifier must
//! return an error, never panic, for any proof bytes, and must not accept a proof that
//! differs from the one mined for the header.

use agent_types::{block, Header};
use consensus_pow::{PearlVerifier, PowVerifier};
use pearl_job::pow::{BlockHeader, FallbackParams};
use proptest::prelude::*;
use std::sync::OnceLock;

/// One real fallback block at the easiest target: (header, proof bytes).
fn mined() -> &'static (Header, Vec<u8>) {
    static M: OnceLock<(Header, Vec<u8>)> = OnceLock::new();
    M.get_or_init(|| {
        let h = Header { version: 1, height: 1, timestamp_ms: 1_000, target_bits: 0x207fffff, miner: [9; 32], ..Default::default() };
        let tpl = BlockHeader::from_bytes(&h.encode()).unwrap();
        let fb = FallbackParams::default();
        let m = pearl_job::pow::mine_header(&tpl, &pearl_job::BlockWork::Fallback(fb), 0..4096).unwrap().expect("easy target wins");
        (Header::decode(&m.header.to_bytes()).unwrap(), m.proof)
    })
}

fn check(proof: Vec<u8>) {
    let (h, good) = mined();
    let mut hh = *h;
    // bind the header to these bytes so the verifier gets past proof_hash and parses them
    hh.proof_hash = block::proof_hash(&proof);
    let r = PearlVerifier.verify(&hh, None, &proof, &FallbackParams::default());
    if r.is_ok() {
        assert_eq!(&proof, good, "a proof other than the mined one verified");
    }
}

#[test]
fn mined_proof_verifies() {
    let (h, p) = mined();
    PearlVerifier.verify(h, None, p, &FallbackParams::default()).unwrap();
    assert!(PearlVerifier.verify(h, None, &p[..p.len() - 1], &FallbackParams::default()).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, .. ProptestConfig::default() })]

    #[test]
    fn random_proofs_never_panic(p in prop::collection::vec(any::<u8>(), 0..4096)) {
        check(p);
    }

    #[test]
    fn mutated_proofs_never_panic(flips in prop::collection::vec((any::<usize>(), 1u8..=255), 1..4), cut in any::<Option<usize>>()) {
        let mut p = mined().1.clone();
        for (i, x) in flips {
            let n = p.len();
            p[i % n] ^= x;
        }
        if let Some(c) = cut {
            let n = p.len();
            p.truncate(c % n);
        }
        check(p);
    }

    /// Length fields inside the proof set to large values.
    #[test]
    fn large_inner_lengths(at in 0usize..256, v in any::<u32>()) {
        let mut p = mined().1.clone();
        let n = p.len();
        let at = at.min(n - 4);
        p[at..at + 4].copy_from_slice(&v.to_le_bytes());
        check(p);
    }
}
