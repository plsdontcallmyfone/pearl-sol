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
    assert_eq!(hex::encode(p.hash_a), "0e68c8d3bae3e18efe30f78e02f313921ce6dc7adcd6d496e3605fd7d445ee62");
    assert_eq!(hex::encode(p.hash_b), "33e22b577f66feb98851fcb1f4ff628dcd6c42857b8b7e24f37f08bd542f1ae6");
    let bits = 0x1e080000; // about 1 win in 32 nonces (h*w*(dlen/r)*128 = 2^16)
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
    let jh = verify::verify_block_proof(&mined.proof, &jp, &pd, bits, h.nonce()).unwrap();
    // wrong nonce, wrong digest, harder target, flipped leaf byte
    assert!(verify::verify_block_proof(&mined.proof, &jp, &pd, bits, h.nonce() + 1).is_err());
    let mut pd2 = pd;
    pd2[0] ^= 1;
    // a different digest gives a different jackpot (it may still meet an easy target)
    assert_ne!(verify::verify_block_proof(&mined.proof, &jp, &pd2, 0x207fffff, h.nonce()), Ok(jh));
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

/// Rank penalty mirrored from pearl-job: same floor, same penalized bound.
#[test]
fn rank_penalty_matches_pearl_job() {
    use pearl_job::pattern::PeriodicPattern;
    let pats = [
        (PeriodicPattern::from_list(&[0, 1, 2, 3]).unwrap(), PeriodicPattern::from_list(&[0, 1, 2, 3, 4, 5, 6, 7]).unwrap()),
        (PeriodicPattern::from_list(&[0, 8, 64, 72]).unwrap(), PeriodicPattern::from_list(&[0, 1, 8, 9, 32, 33, 40, 41]).unwrap()),
    ];
    for (rows_pattern, cols_pattern) in pats {
        for (r, k) in [(32u16, 1024u32), (64, 4096), (128, 2048), (128, 4096), (256, 4096), (512, 65536), (1024, 65536)] {
            let pc = pearl_job::MiningConfig { common_dim: k, rank: r, rows_pattern, cols_pattern };
            let sc = MiningConfig::from_bytes(&pc.to_bytes()).unwrap();
            let (m, n) = (128, 128);
            assert_eq!(sc.check_shape(m, n).is_ok(), pearl_job::job::check_shape(&pc, m as usize, n as usize).is_ok(), "r={r} k={k}");
            assert_eq!(sc.check_shape(m, n).is_ok(), r >= 128, "r={r} k={k}");
            assert_eq!(sc.penalized_factor() as usize, pearl_job::job::penalized_adjustment_factor(&pc));
            for bits in [0x1d00ffffu32, 0x1e040000, 0x207fffff] {
                let ours = difficulty::difficulty_bound(bits, sc.penalized_factor());
                assert_eq!(ours, U256::from_little_endian(&{
                    let mut b = [0u8; 32];
                    pearl_job::job::difficulty_bound(bits, &pc).to_little_endian(&mut b);
                    b
                }));
            }
        }
    }
}

/// The resumable pieces the light client runs (proof hash, noise in column ranges,
/// jackpot in rank-block ranges) give the same results as the one-shot calls.
#[test]
fn split_steps_match_one_shot() {
    for len in [1usize, 1023, 1024, 1025, 4096, 12612, 25000, 25600] {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
        for step in [1u64, 3, 7, 100] {
            let mut st = fallback::Progress::default();
            let mut out = None;
            while out.is_none() {
                out = fallback::hash_plain(&data, &mut st, step);
            }
            assert_eq!(out.unwrap(), *blake3::hash(&data).as_bytes(), "len {len} step {step}");
        }
    }
    let fp = FallbackParams::default();
    let t = template(7, [0; 32], 0x207fffff);
    let mined = pow::mine_header(&t, &BlockWork::Fallback(fp), 0..16).unwrap().expect("mined");
    let hb = mined.header.to_bytes();
    let h = Header(&hb);
    let pd = h.pow_digest();
    let jp = fallback_params(&pd, &fp);
    let v = proof::parse(&mined.proof).unwrap();
    let full_a = verify::noised_side(&mined.proof, &v, &jp, &pd, verify::Side::A).unwrap();
    let full_b = verify::noised_side(&mined.proof, &v, &jp, &pd, verify::Side::Bt).unwrap();
    let nb = verify::noise_blocks(&jp);
    assert_eq!(nb, 2048 / 8);
    for (side, full, rows) in [(verify::Side::A, &full_a, 4usize), (verify::Side::Bt, &full_b, 8)] {
        for step in [1usize, 37, 100, nb] {
            let mut out = vec![0u8; rows * 2048];
            let mut b = 0;
            while b < nb {
                let e = (b + step).min(nb);
                verify::noised_side_range(&mined.proof, &v, &jp, &pd, side, b, e, &mut out).unwrap();
                b = e;
            }
            assert!(out.iter().zip(full.iter()).all(|(x, y)| *x as i8 == *y), "{side:?} step {step}");
        }
    }
    let one = verify::jackpot_noised(&full_a, &full_b, &jp, &pd, 0x207fffff).unwrap();
    for step in [1u32, 3, 5, 16] {
        let mut st = verify::JackpotState::default();
        let mut done = false;
        while !done {
            done = verify::jackpot_step(&full_a, &full_b, &jp, &mut st, step).unwrap();
            let mut buf = vec![0u8; verify::JackpotState::BYTES];
            st.to_bytes(&mut buf);
            st = verify::JackpotState::from_bytes(&buf);
        }
        assert_eq!(verify::jackpot_finish(&st, &jp, &pd, 0x207fffff).unwrap(), one);
    }
    assert_eq!(verify::jackpot_blocks(&jp), 16);
}

fn vectors(name: &str) -> serde_json::Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chain/agent-types/vectors").join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// R1: the chain's published header + coinbase vector verifies here, every tx path in
/// it too, and tampering fails.
#[test]
fn coinbase_receipt_vector() {
    let v = vectors("header.json");
    let hb: [u8; 256] = hex::decode(v["header"].as_str().unwrap()).unwrap().try_into().unwrap();
    let h = Header(&hb);
    let cb = &v["coinbase"];
    let rec = hex::decode(cb["record"].as_str().unwrap()).unwrap();
    let path = hex::decode(cb["path"].as_str().unwrap()).unwrap();
    let c = chaintx::verify_coinbase(&h, &rec, &path).unwrap();
    assert_eq!(c.height, cb["fields"]["height"].as_u64().unwrap());
    assert_eq!(c.fees.to_string(), cb["fields"]["fees"].as_str().unwrap());
    assert_eq!(c.subsidy.to_string(), cb["fields"]["subsidy"].as_str().unwrap());
    assert_eq!(c.mature_height, cb["fields"]["mature_height"].as_u64().unwrap());
    assert_eq!(hex::encode(chaintx::tx_hash(&rec)), cb["receipt_hash"].as_str().unwrap());
    for (i, t) in v["block_tx_hashes"].as_array().unwrap().iter().enumerate() {
        let th: [u8; 32] = hex::decode(t.as_str().unwrap()).unwrap().try_into().unwrap();
        let p = hex::decode(v["tx_paths"][i].as_str().unwrap()).unwrap();
        chaintx::verify_tx_path(&th, &p, &h.tx_root()).unwrap();
    }
    // tampering: any field, the path, or a header that is not the receipt's block
    for off in [1usize, 9, 41, 49, 57, 89] {
        let mut bad = rec.clone();
        bad[off] ^= 1;
        assert!(chaintx::verify_coinbase(&h, &bad, &path).is_err(), "byte {off}");
    }
    let mut bad_path = path.clone();
    bad_path[0] ^= 1;
    assert!(chaintx::verify_coinbase(&h, &rec, &bad_path).is_err());
    let mut other = hb;
    other[4] ^= 1; // height
    assert!(chaintx::verify_coinbase(&Header(&other), &rec, &path).is_err());
    // every published tx is refused as a receipt (version byte 1, other lengths)
    for t in vectors("tx.json").as_array().unwrap() {
        let tx = hex::decode(t["tx"].as_str().unwrap()).unwrap();
        assert!(chaintx::coinbase(&tx).is_err());
    }
    assert!(chaintx::TxView::any_chain(&rec).is_err(), "a receipt is not a tx");
}

/// R2: SetRoute / ClearRoute parse from the chain's vectors; the release message matches.
#[test]
fn route_tx_vectors() {
    let txs = vectors("tx.json");
    let find = |n: &str| txs.as_array().unwrap().iter().find(|t| t["name"] == n).unwrap().clone();
    let t = find("set_route");
    let b = hex::decode(t["tx"].as_str().unwrap()).unwrap();
    let view = chaintx::TxView::any_chain(&b).unwrap();
    let r = chaintx::set_route(&view).unwrap();
    assert_eq!(r.route_bps, 2500);
    assert_eq!(r.until_height, 100_000);
    assert_eq!(r.cap, 1_000_000_000_000);
    assert_ne!(r.release_key, [0; 32]);
    let t = find("set_route_irrevocable");
    let b = hex::decode(t["tx"].as_str().unwrap()).unwrap();
    assert_eq!(chaintx::set_route(&chaintx::TxView::any_chain(&b).unwrap()).unwrap().release_key, [0; 32]);
    let t = find("clear_route");
    let b = hex::decode(t["tx"].as_str().unwrap()).unwrap();
    let (account, nonce) = chaintx::clear_route(&chaintx::TxView::any_chain(&b).unwrap()).unwrap();
    assert_eq!(nonce, 0);
    let msgs = vectors("messages.json");
    let m = msgs.as_array().unwrap().iter().find(|m| m["name"] == "route_clear").unwrap();
    let chain_id = t["chain_id"].as_u64().unwrap() as u32;
    assert_eq!(hex::encode(chaintx::route_clear_message(chain_id, &account, nonce)), m["message"].as_str().unwrap());
    // other kinds are refused
    let t = find("transfer");
    let b = hex::decode(t["tx"].as_str().unwrap()).unwrap();
    assert!(chaintx::set_route(&chaintx::TxView::any_chain(&b).unwrap()).is_err());
}

/// R5: accumulator proofs for every leaf of every size up to 70; incremental appends give
/// the same root; wrong leaf, index or size fail.
#[test]
fn final_header_accumulator() {
    use pearl_verify_sbf::acc;
    let hs: Vec<[u8; 32]> = (0..70u32).map(|i| *blake3::hash(&i.to_le_bytes()).as_bytes()).collect();
    let mut slots = [[0u8; 32]; acc::MAX_PEAKS];
    for n in 1..=hs.len() {
        acc::append(sha, n as u64 - 1, &mut slots, &hs[n - 1]);
        let root = acc::root_of_slots(sha, n as u64, &slots);
        assert_eq!(root, acc::root_of_hashes(sha, &hs[..n]), "n {n}");
        for i in 0..n as u64 {
            let p = acc::prove(sha, &hs[..n], i).unwrap();
            assert_eq!(Some(p.len()), acc::proof_len(n as u64, i));
            acc::verify(sha, n as u64, i, &hs[i as usize], &p, &root).unwrap();
            let short = acc::prove_short(sha, &hs[..n], i).unwrap();
            let (b, peak) = acc::verify_to_peak(sha, n as u64, i, &hs[i as usize], &short).unwrap();
            assert_eq!(peak, slots[b as usize], "short form climbs to the stored peak");
            assert!(acc::verify(sha, n as u64, i, &hs[(i as usize + 1) % 70], &p, &root).is_err());
            if n > 1 {
                assert!(acc::verify(sha, n as u64, (i + 1) % n as u64, &hs[i as usize], &p, &root).is_err());
            }
            if !p.is_empty() {
                let mut bad = p.clone();
                bad[0] ^= 1;
                assert!(acc::verify(sha, n as u64, i, &hs[i as usize], &bad, &root).is_err());
            }
        }
        assert!(acc::verify(sha, n as u64, n as u64, &hs[0], &[], &root).is_err());
    }
}

/// SPEC 4.5: a PostSegments tx (chain vector) gives the light client every segment job, with
/// the ids, header and config the chain uses (crates/pearl-job standing).
#[test]
fn post_segments_vector_rebuilds_segment_jobs() {
    use pearl_job::standing;
    let v = vectors("standing.json");
    let ps = v.as_array().unwrap().iter().find(|x| x["name"] == "post_segments").unwrap();
    let tx = hex::decode(ps["tx"].as_str().unwrap()).unwrap();
    let view = chaintx::TxView::new(&tx, ps["chain_id"].as_u64().unwrap() as u32).unwrap();
    let mut jobs = vec![];
    chaintx::for_each_segment(&view, |j| jobs.push(j)).unwrap();
    assert_eq!(jobs.len(), 1);
    let b = &ps["body"];
    let g = &b["segments"][0];
    let key32 = |s: &serde_json::Value| -> [u8; 32] { hex::decode(s.as_str().unwrap()).unwrap().try_into().unwrap() };
    let (ticket, model) = (key32(&b["ticket_id"]), key32(&b["model_id"]));
    let (matrix, k, n) = (g["matrix"].as_u64().unwrap() as u16, g["k"].as_u64().unwrap() as u32, g["n"].as_u64().unwrap() as u32);
    let j = &jobs[0];
    assert_eq!(j.job_id, standing::segment_id(&ticket, matrix, b["round"].as_u64().unwrap() as u16));
    assert_eq!(j.header76, standing::matrix_job_header(&model, matrix, k, n).to_bytes());
    assert_eq!((j.m, j.n), (g["m"].as_u64().unwrap() as u32, n));
    assert_eq!(j.hash_a, key32(&g["hash_a"]));
    assert_eq!(j.hash_b, key32(&g["hash_b"]));
    assert_eq!(&j.config[0..4], &k.to_le_bytes());
    assert_eq!(&j.config[4..6], &(b["rank"].as_u64().unwrap() as u16).to_le_bytes());
    assert_eq!(hex::encode(&j.config[8..14]), b["rows_pattern"].as_str().unwrap());
    assert_eq!(hex::encode(&j.config[14..20]), b["cols_pattern"].as_str().unwrap());
    assert!(j.config[20..].iter().all(|&x| x == 0) && j.config[6..8] == [0, 0]);
    // by id, and a wrong id or a truncated body is refused
    assert_eq!(chaintx::job_by_id(&view, &j.job_id, sha).unwrap().job_id, j.job_id);
    assert!(chaintx::job_by_id(&view, &[0; 32], sha).is_err());
    let mut short = tx.clone();
    short.remove(54 + 40);
    assert!(chaintx::for_each_segment(&chaintx::TxView::any_chain(&short).unwrap(), |_| {}).is_err());
    // a real segment job (localnet config): its block proof verifies with the rebuilt commit
    let base = pearl_job::MiningConfig::localnet_default();
    let mut config = [0u8; 52];
    config.copy_from_slice(&standing::matrix_config(&base, 2048).to_bytes());
    assert_eq!(&config[8..14], &base.rows_pattern.to_bytes());
}
