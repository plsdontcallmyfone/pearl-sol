//! Cross-checks of the no_std verifier against the native references: the `blake3`
//! crate, crates/pearl-job (mining + verify_block) and commonware-storage's MMR.

use pearl_job::pow::{self, BlockHeader, BlockWork, FallbackParams};
use pearl_verify_sbf::{blake3 as b3, chaintx, config::MiningConfig, difficulty, fallback, header::Header, mmr, proof, verify};
use rand::{Rng, SeedableRng};

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

#[test]
fn blake3_matches_reference() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(7);
    let key: [u8; 32] = rng.random();
    for len in [0usize, 1, 31, 63, 64, 65, 127, 128, 1023, 1024, 1025, 2047, 2048, 2049, 3000, 4096, 5000, 12612, 65536] {
        let data: Vec<u8> = (0..len).map(|_| rng.random()).collect();
        assert_eq!(b3::hash(&data), *blake3::hash(&data).as_bytes(), "hash len {len}");
        assert_eq!(b3::keyed_hash(&key, &data), *blake3::keyed_hash(&key, &data).as_bytes(), "keyed len {len}");
    }
    // derive_key + XOF blocks at arbitrary positions
    let ctx = "pearl-sol 2026-10-16 fallback matrix A v1";
    let o = fallback::xof(ctx.as_bytes(), &[9u8; 32]);
    let mut h = blake3::Hasher::new_derive_key(ctx);
    h.update(&[9u8; 32]);
    let mut want = vec![0u8; 64 * 40];
    h.finalize_xof().fill(&mut want);
    for blk in 0..40u64 {
        assert_eq!(o.root_block(blk)[..], want[blk as usize * 64..blk as usize * 64 + 64], "xof block {blk}");
    }
}

#[test]
fn difficulty_port() {
    use difficulty::*;
    for bits in [0x1d00ffffu32, 0x207fffff, 0x1e040000, 0x1d020000, 0x03123456] {
        assert_eq!(target_to_bits(nbits_to_target(bits)), bits);
        let a = nbits_to_target(bits);
        let b = pearl_job::job::nbits_to_target(bits);
        let mut ab = [0u8; 32];
        let mut bb = [0u8; 32];
        a.to_little_endian(&mut ab);
        b.to_little_endian(&mut bb);
        assert_eq!(ab, bb);
    }
    assert_eq!(work(0x1d00ffff), U256::from(0x100010001u64));
    let r = |t: u64| Retarget {
        parent_bits: 0x1e040000,
        parent_ts_ms: 1_000_000 + t,
        grandparent_ts_ms: Some(1_000_000),
        target_block_time_ms: 60_000,
        half_life_ms: 600_000,
        pow_limit_bits: 0x207fffff,
        min_difficulty_bits: 0,
    };
    let base = nbits_to_target(0x1e040000);
    assert_eq!(next_bits(&r(60_000)), 0x1e040000);
    assert!(nbits_to_target(next_bits(&r(120_000))) > base);
    assert!(nbits_to_target(next_bits(&r(1_000))) < base);
    let up = nbits_to_target(next_bits(&r(120_000)));
    assert_eq!(up, nbits_to_target(target_to_bits(base + base / 10)));
    let mut x = r(1_000_000_000);
    x.pow_limit_bits = 0x1e080000;
    assert_eq!(next_bits(&x), 0x1e080000);
    x.grandparent_ts_ms = None;
    assert_eq!(next_bits(&x), 0x1e040000);
}
use pearl_verify_sbf::U256;

fn template(height: u64, job_id: [u8; 32], bits: u32) -> BlockHeader {
    BlockHeader {
        version: 1,
        height,
        prev_hash: [0xab; 32],
        timestamp_ms: 1_760_000_000_000 + height,
        target_bits: bits,
        job_id,
        miner: [0x4d; 32],
        tx_root: [1; 32],
        state_root: [2; 32],
        withdraw_root: [3; 32],
        proof_hash: [0; 32],
        nonce: 0,
    }
}

fn fallback_params(pd: &[u8; 32], fp: &FallbackParams) -> verify::JobParams {
    let cfg = fp.config.to_bytes();
    let jk = verify::job_key(&fallback::job_header(pd), &cfg);
    let (k, m, n) = (fp.config.common_dim, fp.m, fp.n);
    let mut scratch = vec![0u8; (k * n) as usize];
    let xb = fallback::xof(fallback::CTX_B, pd);
    for st in (0..(k * n / 64) as u64).step_by(100) {
        fallback::gen_b(&xb, &mut scratch, st, 100, k, n);
    }
    // resumable, in uneven ranges
    let mut st = fallback::Progress::default();
    let mut hash_b = None;
    while hash_b.is_none() {
        hash_b = fallback::hash_bt(&jk, &scratch, k, n, &mut st, 7);
    }
    let xa = fallback::xof(fallback::CTX_A, pd);
    let mut st = fallback::Progress::default();
    let mut hash_a = None;
    while hash_a.is_none() {
        hash_a = fallback::hash_a(&jk, &xa, m, k, &mut st, 5);
    }
    verify::JobParams { job_key: jk, config: MiningConfig::from_bytes(&cfg).unwrap(), m, n, hash_a: hash_a.unwrap(), hash_b: hash_b.unwrap() }
}

#[test]
fn fallback_block_verifies_and_matches_pearl_job() {
    let fp = FallbackParams::default();
    // pinned vector (PROOF-FORMAT 7.5)
    let p = fallback_params(&[9u8; 32], &fp);
    assert_eq!(hex::encode(p.hash_a), "f3f1b82d6f82b46c16435bdb2cb5b97bcfb02f22ad30ca3f50bd3360b6e1596a");
    assert_eq!(hex::encode(p.hash_b), "5ead9e6b5c2c9da12cf8dd666c7592afb7e7a64377e86741c2cec91fff84d347");
    let bits = 0x1e100000; // about 1 win in 32 nonces
    let t = template(5, [0; 32], bits);
    let mined = pow::mine_header(&t, &BlockWork::Fallback(fp), 0..10_000).unwrap().expect("mined");
    let hb = mined.header.to_bytes();
    let h = Header(&hb);
    assert_eq!(h.pow_digest(), mined.header.pow_digest());
    assert_eq!(h.block_hash(), mined.header.block_hash());
    assert_eq!(b3::hash(&mined.proof), h.proof_hash());
    pow::verify_block(&mined.header, None, &mined.proof, &fp).unwrap();
    let pd = h.pow_digest();
    let jp = fallback_params(&pd, &fp);
    verify::verify_block_proof(&mined.proof, &jp, &pd, bits, h.nonce()).unwrap();
    // wrong nonce, wrong digest, harder target, flipped leaf byte
    assert!(verify::verify_block_proof(&mined.proof, &jp, &pd, bits, h.nonce() + 1).is_err());
    let mut pd2 = pd;
    pd2[0] ^= 1;
    assert!(verify::verify_block_proof(&mined.proof, &jp, &pd2, bits, h.nonce()).is_err());
    assert_eq!(verify::verify_block_proof(&mined.proof, &jp, &pd, 0x1d00ffff, h.nonce()), Err(pearl_verify_sbf::Error::Target));
    let mut bad = mined.proof.clone();
    let v = proof::parse(&bad).unwrap();
    bad[v.a.data_off + 5] ^= 1;
    assert_eq!(verify::verify_block_proof(&bad, &jp, &pd, bits, h.nonce()), Err(pearl_verify_sbf::Error::Commitment));
    let mut bad = mined.proof.clone();
    bad.push(0);
    assert!(proof::parse(&bad).is_err());
}

#[test]
fn job_blocks_verify_many() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    let cfg = pearl_job::job::MiningConfig::localnet_default();
    for (m, n) in [(64u32, 64u32), (128, 64), (64, 256)] {
        let k = cfg.k();
        let a: Vec<i8> = (0..m as usize * k).map(|_| rng.random_range(-64i8..=64)).collect();
        let b: Vec<i8> = (0..k * n as usize).map(|_| rng.random_range(-64i8..=64)).collect();
        let header = pearl_job::job::JobHeader { job_pubkey: rng.random(), terms_hash: rng.random(), created_slot: 0, target_bits: 0 };
        let job = pearl_job::Job { header, config: cfg, m, n, hash_a: [0; 32], hash_b: [0; 32] };
        let prep = pearl_job::mine::PreparedJob::with_computed_commitments(job, &a, &b).unwrap();
        let job_id: [u8; 32] = rng.random();
        let bits = 0x207fffff;
        for round in 0..3u64 {
            let t = template(10 + round, job_id, bits);
            let mined = pow::mine_header(&t, &BlockWork::Job(&prep), round * 1000..round * 1000 + 1000).unwrap().expect("mined");
            pow::verify_block(&mined.header, Some(&prep.job), &mined.proof, &FallbackParams::default()).unwrap();
            let hb = mined.header.to_bytes();
            let h = Header(&hb);
            let jp = verify::JobParams {
                job_key: verify::job_key(&prep.job.header.to_bytes(), &cfg.to_bytes()),
                config: MiningConfig::from_bytes(&cfg.to_bytes()).unwrap(),
                m,
                n,
                hash_a: prep.job.hash_a,
                hash_b: prep.job.hash_b,
            };
            assert_eq!(jp.job_key, prep.job.job_key());
            verify::verify_block_proof(&mined.proof, &jp, &h.pow_digest(), bits, h.nonce()).unwrap();
            let mut wrong = jp;
            wrong.hash_b[3] ^= 1;
            assert!(verify::verify_block_proof(&mined.proof, &wrong, &h.pow_digest(), bits, h.nonce()).is_err());
        }
    }
}

#[test]
fn mmr_matches_commonware() {
    use commonware_cryptography::Sha256;
    use commonware_storage::merkle::{hasher::Standard, Bagging};
    use commonware_storage::mmr::{mem::Mmr, Location};
    use commonware_codec::Encode;
    let hasher: Standard<Sha256> = Standard::new(Bagging::BackwardFold);
    for n in 1u64..=70 {
        let mut mem = Mmr::new();
        let batch = {
            let mut batch = mem.new_batch();
            for i in 0..n {
                batch = batch.add(&hasher, &i.to_be_bytes());
            }
            batch.merkleize(&mem, &hasher)
        };
        mem.apply_batch(&batch).unwrap();
        let peaks = mem.peak_iterator().count();
        for inactive in [0usize, 1, peaks / 2, peaks] {
            let root = mem.root(&hasher, inactive).unwrap();
            let root: [u8; 32] = root.as_ref().try_into().unwrap();
            for loc in 0..n {
                let p = mem.proof(&hasher, Location::new(loc), inactive).unwrap();
                let bytes = p.encode().to_vec();
                mmr::verify_inclusion(sha, &bytes, &loc.to_be_bytes(), loc, &root)
                    .unwrap_or_else(|e| panic!("n={n} inactive={inactive} loc={loc}: {e:?}"));
                assert!(mmr::verify_inclusion(sha, &bytes, &(loc + 1).to_be_bytes(), loc, &root).is_err());
                if n > 1 {
                    let other = (loc + 1) % n;
                    assert!(mmr::verify_inclusion(sha, &bytes, &loc.to_be_bytes(), other, &root).is_err());
                }
            }
        }
    }
}

#[test]
fn tx_root_paths() {
    // Same construction as chain/agent-types/src/merkle.rs.
    fn leaf(h: &[u8; 32]) -> [u8; 32] {
        *blake3::hash(&[&[0u8][..], h].concat()).as_bytes()
    }
    fn node(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
        *blake3::hash(&[&[1u8][..], l, r].concat()).as_bytes()
    }
    for n in 1..12usize {
        let hs: Vec<[u8; 32]> = (0..n).map(|i| *blake3::hash(&[i as u8]).as_bytes()).collect();
        let mut level: Vec<[u8; 32]> = hs.iter().map(leaf).collect();
        let mut paths: Vec<Vec<u8>> = vec![Vec::new(); n];
        let mut idx: Vec<usize> = (0..n).collect();
        while level.len() > 1 {
            for (t, i) in idx.iter_mut().enumerate() {
                let sib = *i ^ 1;
                if sib < level.len() {
                    paths[t].extend_from_slice(&level[sib]);
                    paths[t].push((sib < *i) as u8);
                }
                *i /= 2;
            }
            level = level.chunks(2).map(|c| if c.len() == 2 { node(&c[0], &c[1]) } else { c[0] }).collect();
        }
        for i in 0..n {
            chaintx::verify_tx_path(&hs[i], &paths[i], &level[0]).unwrap();
            if n > 1 {
                assert!(chaintx::verify_tx_path(&hs[(i + 1) % n], &paths[i], &level[0]).is_err());
            }
        }
    }
}

/// The U256-only WTEMA equals the chain's U512 formula (reimplemented here with U512).
#[test]
fn wtema_matches_u512_reference() {
    use primitive_types::{U256, U512};
    use difficulty::*;
    fn reference(r: &Retarget) -> u32 {
        let Some(gp) = r.grandparent_ts_ms else { return r.parent_bits };
        let old = nbits_to_target(r.parent_bits);
        let limit = nbits_to_target(r.pow_limit_bits);
        let d = (r.parent_ts_ms as i128 - gp as i128) - r.target_block_time_ms as i128;
        let hl = U512::from(r.half_life_ms.max(1));
        let old5 = U512::from(old);
        let mag = U512::from(d.unsigned_abs()) * old5;
        let new5 = if d >= 0 { old5 + mag / hl } else {
            let q = (mag + hl - 1) / hl;
            if q >= old5 { U512::zero() } else { old5 - q }
        };
        let mut new = if new5 > U512::from(limit) { limit } else { U256::try_from(new5).unwrap_or(limit) };
        if new.is_zero() { new = U256::one(); }
        if r.min_difficulty_bits != 0 {
            let h = nbits_to_target(r.min_difficulty_bits);
            if new < h { new = h; }
        }
        target_to_bits(new)
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(99);
    for i in 0..200_000u32 {
        let bits = [0x207fffffu32, 0x1e040000, 0x1d00ffff, 0x1f00ffff, 0x03123456, 0x1e100000][(i % 6) as usize];
        let gp: u64 = rng.random_range(0..1u64 << 50);
        let dt: i64 = match i % 4 { 0 => rng.random_range(-100_000..100_000), 1 => rng.random_range(0..10_000_000), 2 => rng.random_range(-(1i64 << 40)..(1i64 << 40)), _ => rng.random_range(0..120_000) };
        let pts = (gp as i64 + dt).max(0) as u64;
        let r = Retarget {
            parent_bits: bits,
            parent_ts_ms: pts,
            grandparent_ts_ms: if i % 50 == 0 { None } else { Some(gp) },
            target_block_time_ms: [60_000u64, 10_000, 1_500, 1][(i % 4) as usize],
            half_life_ms: [3_600_000u64, 600_000, 1, 7][(i / 4 % 4) as usize],
            pow_limit_bits: [0x207fffffu32, 0x1e080000, 0x1f00ffff][(i % 3) as usize],
            min_difficulty_bits: if i % 7 == 0 { 0x1d00ffff } else { 0 },
        };
        assert_eq!(next_bits(&r), reference(&r), "{r:?}");
    }
}
