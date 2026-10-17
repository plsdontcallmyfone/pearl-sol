//! Commit / mine / verify round trips at localnet difficulty, plus rejection cases.

use pearl_job::data::random_matrices;
use pearl_job::freivalds::{freivalds, matmul};
use pearl_job::job::{commit_a, commit_b};
use pearl_job::{job_key, verify_share, Job, JobHeader, MiningConfig, PlainProof, PreparedJob};

/// About 0.7% of tiles win; 128 tiles per nonce on a 64x64 job, so most nonces win.
const EASY: u32 = 0x1e040000;
/// target = 1: never met.
const IMPOSSIBLE: u32 = 0x03000001;

const MINER: [u8; 32] = [0x4d; 32];
const OTHER: [u8; 32] = [0x99; 32];

fn setup(m: u32, n: u32, target: u32) -> (PreparedJob, Vec<i8>, Vec<i8>) {
    let config = MiningConfig::localnet_default();
    let header = JobHeader { job_pubkey: [7; 32], terms_hash: [9; 32], created_slot: 1234, target_bits: target };
    let (a, b) = random_matrices(m as usize, n as usize, config.k(), Some(42));
    let key = job_key(&header, &config);
    let job = Job {
        header,
        config,
        m,
        n,
        hash_a: commit_a(&key, &a),
        hash_b: commit_b(&key, &b, config.k(), n as usize),
    };
    (PreparedJob::new(job, &a, &b).unwrap(), a, b)
}

fn mined(prep: &PreparedJob) -> PlainProof {
    prep.mine_range(&MINER, 0..64, prep.job.header.target_bits).expect("a share within 64 nonces")
}

#[test]
fn commit_mine_verify_roundtrip() {
    let (prep, _, _) = setup(64, 64, EASY);
    let p = mined(&prep);
    let v = verify_share(&prep.job, &p, &MINER).unwrap();
    assert_eq!(v.nonce, p.nonce);
    // serialisation round trip
    let bytes = p.to_bytes();
    let q = PlainProof::from_bytes(&bytes).unwrap();
    assert_eq!(p, q);
    verify_share(&prep.job, &q, &MINER).unwrap();
    // 8 A leaves + 16 B^T leaves of 1 KiB (k = 2048), plus header and siblings
    assert!(bytes.len() > 24 * 1024 && bytes.len() < 25 * 1024 + 1024, "proof size {}", bytes.len());
    // Many shares: every share found in a range verifies.
    let mut count = 0;
    for nonce in 0..16 {
        if let Some(p) = prep.mine_nonce(&MINER, nonce, EASY) {
            verify_share(&prep.job, &p, &MINER).unwrap();
            count += 1;
        }
    }
    assert!(count > 0);
}

#[test]
fn non_square_job() {
    let (prep, _, _) = setup(32, 128, EASY);
    let p = mined(&prep);
    verify_share(&prep.job, &p, &MINER).unwrap();
}

#[test]
fn wrong_miner_rejected() {
    let (prep, _, _) = setup(64, 64, EASY);
    let p = mined(&prep);
    let err = verify_share(&prep.job, &p, &OTHER).unwrap_err();
    assert!(err.to_string().contains("share target"), "{err}");
}

#[test]
fn tampered_proofs_rejected() {
    let (prep, _, _) = setup(64, 64, EASY);
    let p = mined(&prep);
    let job = &prep.job;

    let mut t = p.clone();
    t.a.leaf_data[0][5] ^= 1;
    assert!(verify_share(job, &t, &MINER).unwrap_err().to_string().contains("Merkle"));

    let mut t = p.clone();
    t.bt.leaf_data[3][100] = t.bt.leaf_data[3][100].wrapping_add(1);
    assert!(verify_share(job, &t, &MINER).is_err());

    let mut t = p.clone();
    t.a.siblings[0][0] ^= 0x80;
    assert!(verify_share(job, &t, &MINER).is_err());

    let mut t = p.clone();
    t.nonce += 1_000_000;
    assert!(verify_share(job, &t, &MINER).is_err());

    // different tile than the one opened
    let mut t = p.clone();
    t.t_rows = (t.t_rows + 4) % job.m;
    assert!(verify_share(job, &t, &MINER).is_err());

    // misaligned tile start
    let mut t = p.clone();
    t.t_cols += 1;
    assert!(verify_share(job, &t, &MINER).unwrap_err().to_string().contains("valid tile start"));

    // tile past the end
    let mut t = p.clone();
    t.t_rows = job.m;
    assert!(verify_share(job, &t, &MINER).is_err());

    // dims lie
    let mut t = p.clone();
    t.m *= 2;
    assert!(verify_share(job, &t, &MINER).is_err());

    // extra leaf
    let mut t = p.clone();
    t.a.leaf_indices.push(15);
    t.a.leaf_data.push([0; 1024]);
    assert!(verify_share(job, &t, &MINER).is_err());

    // byte-level: truncation and trailing garbage
    let bytes = p.to_bytes();
    assert!(PlainProof::from_bytes(&bytes[..bytes.len() - 1]).is_err());
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(PlainProof::from_bytes(&longer).is_err());
    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(PlainProof::from_bytes(&bad).is_err());
}

#[test]
fn proof_bound_to_job() {
    let (prep, a, b) = setup(64, 64, EASY);
    let p = mined(&prep);
    // Same matrices, a different job (other pubkey): commitments differ, proof fails.
    let mut job2 = prep.job.clone();
    job2.header.job_pubkey = [8; 32];
    let key2 = job2.job_key();
    job2.hash_a = commit_a(&key2, &a);
    job2.hash_b = commit_b(&key2, &b, job2.config.k(), 64);
    assert!(verify_share(&job2, &p, &MINER).is_err());
    // Harder share target on the same job: the easy share fails it.
    assert!(pearl_job::verify_share_with_target(&prep.job, &p, &MINER, IMPOSSIBLE).is_err());
}

#[test]
fn impossible_target_finds_nothing() {
    let (prep, _, _) = setup(64, 64, IMPOSSIBLE);
    assert!(prep.mine_range(&MINER, 0..4, IMPOSSIBLE).is_none());
}

#[test]
fn prepared_job_rejects_wrong_matrices() {
    let (prep, a, mut b) = setup(64, 64, EASY);
    b[0] = if b[0] == 1 { 2 } else { 1 };
    assert!(PreparedJob::new(prep.job.clone(), &a, &b).is_err());
    let mut a2 = a.clone();
    a2[3] = 65;
    assert!(PreparedJob::new(prep.job.clone(), &a2, &b).is_err());
}

#[test]
fn freivalds_true_and_false() {
    let (m, n, k) = (48usize, 40usize, 1024usize);
    let (a, b) = random_matrices(m, n, k, Some(7));
    let c = matmul(&a, &b, m, n, k);
    let mut rng = rand::rng();
    assert!(freivalds(&a, &b, &c, m, n, k, 3, &mut rng));
    for (i, delta) in [(0usize, 1i32), (m * n - 1, -1), (17 * n + 3, 1 << 20)] {
        let mut bad = c.clone();
        bad[i] = bad[i].wrapping_add(delta);
        assert!(!freivalds(&a, &b, &bad, m, n, k, 1, &mut rng), "entry {i} off by {delta} passed");
    }
    // transposed-ish garbage and wrong length
    let zeros = vec![0i32; m * n];
    assert!(!freivalds(&a, &b, &zeros, m, n, k, 1, &mut rng));
    assert!(!freivalds(&a, &b, &c[1..], m, n, k, 1, &mut rng));
}
