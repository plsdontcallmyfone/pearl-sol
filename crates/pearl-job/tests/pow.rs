//! v2 block proofs: job and fallback round trips, binding to the header, rejections.

use pearl_job::data::random_matrices;
use pearl_job::job::{commit_a, commit_b, derive_seeds};
use pearl_job::pow::{
    block_proof_from_bytes, block_proof_to_bytes, fallback_job, mine_block_proof, mine_header, proof_hash, verify_block,
    verify_block_proof, BlockHeader, BlockWork, FallbackParams,
};
use pearl_job::{job_key, Job, JobHeader, MiningConfig, PreparedJob, SeedDerivation};

/// About 0.6 wins per nonce on a 64 x 64 job (see CLI.md).
const EASY: u32 = 0x1e040000;
const IMPOSSIBLE: u32 = 0x03000001;
const JOB_ID: [u8; 32] = [7; 32];

fn job_with(seed: u64) -> (PreparedJob, Vec<i8>, Vec<i8>) {
    let config = MiningConfig::localnet_default();
    // The job's own share_target (header bytes 72..76) only feeds job_key here.
    let header = JobHeader { job_pubkey: JOB_ID, terms_hash: [9; 32], created_slot: 12, target_bits: 0x1d010000 };
    let (a, b) = random_matrices(64, 64, config.k(), Some(seed));
    let key = job_key(&header, &config);
    let job = Job { header, config, m: 64, n: 64, hash_a: commit_a(&key, &a), hash_b: commit_b(&key, &b, config.k(), 64) };
    (PreparedJob::new(job, &a, &b).unwrap(), a, b)
}

fn template(job_id: [u8; 32]) -> BlockHeader {
    BlockHeader {
        version: 1,
        height: 5,
        prev_hash: [0xab; 32],
        timestamp_ms: 1_760_000_000_000,
        target_bits: EASY,
        job_id,
        miner: [0x4d; 32],
        tx_root: [1; 32],
        state_root: [2; 32],
        withdraw_root: [3; 32],
        proof_hash: [0; 32],
        nonce: 0,
    }
}

fn fb() -> FallbackParams {
    FallbackParams::default()
}

#[test]
fn job_block_roundtrip() {
    let (prep, a, b) = job_with(42);
    let t = template(JOB_ID);
    let blk = mine_header(&t, &BlockWork::Job(&prep), 0..64).unwrap().expect("a block within 64 nonces");
    assert_eq!(blk.header.proof_hash, proof_hash(&blk.proof));
    assert_eq!(blk.proof[5], 3, "seed mode byte");
    let v = verify_block(&blk.header, Some(&prep.job), &blk.proof, &fb()).unwrap();
    assert_eq!(v.nonce, blk.header.nonce);
    // Header bytes round trip and still verify.
    let h2 = BlockHeader::from_bytes(&blk.header.to_bytes()).unwrap();
    verify_block(&h2, Some(&prep.job), &blk.proof, &fb()).unwrap();
    // The free function gives the same proof as the prepared path.
    let pd = blk.header.pow_digest();
    let p = mine_block_proof(&a, &b, &prep.job, &pd, EASY, blk.header.nonce).unwrap().unwrap();
    assert_eq!(block_proof_to_bytes(&p), blk.proof);
    verify_block_proof(&prep.job, &p, &pd, EASY, blk.header.nonce).unwrap();
    // 8 A leaves + 16 B^T leaves (k = 2048: two 1 KiB leaves per row).
    assert!(blk.proof.len() > 24 * 1024 && blk.proof.len() < 25 * 1024 + 512, "{}", blk.proof.len());
}

/// Re-point one header field, keep the proof (and its proof_hash): must fail.
#[test]
fn proof_bound_to_pow_digest() {
    let (prep, _, _) = job_with(42);
    let blk = mine_header(&template(JOB_ID), &BlockWork::Job(&prep), 0..64).unwrap().unwrap();
    let changes: Vec<(&str, Box<dyn Fn(&mut BlockHeader)>)> = vec![
        ("miner", Box::new(|h| h.miner = [0x99; 32])),
        ("prev_hash", Box::new(|h| h.prev_hash[0] ^= 1)),
        ("nonce", Box::new(|h| h.nonce += 1)),
        ("height", Box::new(|h| h.height += 1)),
        ("state_root", Box::new(|h| h.state_root[31] ^= 1)),
        ("timestamp", Box::new(|h| h.timestamp_ms += 1)),
    ];
    for (what, f) in changes {
        let mut h = blk.header;
        f(&mut h);
        assert!(verify_block(&h, Some(&prep.job), &blk.proof, &fb()).is_err(), "{what} change still verifies");
    }
    // A harder chain target on the same header fails too.
    let mut h = blk.header;
    h.target_bits = IMPOSSIBLE;
    assert!(verify_block(&h, Some(&prep.job), &blk.proof, &fb()).is_err());
    // Same proof checked against another pow_digest directly.
    let p = block_proof_from_bytes(&blk.proof).unwrap();
    let err = verify_block_proof(&prep.job, &p, &[0x55; 32], EASY, blk.header.nonce).unwrap_err();
    assert!(err.to_string().contains("target"), "{err}");
}

#[test]
fn wrong_job_or_matrices_rejected() {
    let (prep, _, _) = job_with(42);
    let blk = mine_header(&template(JOB_ID), &BlockWork::Job(&prep), 0..64).unwrap().unwrap();
    // Same job id, different matrices: commitments differ, Merkle opening fails.
    let (other, _, _) = job_with(43);
    let err = verify_block(&blk.header, Some(&other.job), &blk.proof, &fb()).unwrap_err();
    assert!(err.to_string().contains("Merkle"), "{err}");
    // A job under a different id.
    let mut j = prep.job.clone();
    j.header.job_pubkey = [8; 32];
    assert!(verify_block(&blk.header, Some(&j), &blk.proof, &fb()).is_err());
    // No job given for a job block.
    assert!(verify_block(&blk.header, None, &blk.proof, &fb()).is_err());
    // Mining with matrices that do not match the job is refused up front.
    let (_, a2, b2) = job_with(44);
    assert!(mine_block_proof(&a2, &b2, &prep.job, &[1; 32], EASY, 0).is_err());
    // Claiming the block is a fallback block fails (the matrices are the job's).
    let mut h = blk.header;
    h.job_id = [0; 32];
    assert!(verify_block(&h, Some(&prep.job), &blk.proof, &fb()).is_err());
    // (Which chain job id a prepared job is mined under is the caller's pairing: see
    // job_block_with_header_id_other_than_chain_job_id.)
}

#[test]
fn proof_bytes_checked() {
    let (prep, _, _) = job_with(42);
    let blk = mine_header(&template(JOB_ID), &BlockWork::Job(&prep), 0..64).unwrap().unwrap();
    // proof_hash must match the bytes.
    let mut h = blk.header;
    h.proof_hash[0] ^= 1;
    assert!(verify_block(&h, Some(&prep.job), &blk.proof, &fb()).unwrap_err().to_string().contains("proof_hash"));
    // A v1 (mode 2) encoding of the same proof is not a block proof.
    let p = block_proof_from_bytes(&blk.proof).unwrap();
    let v1 = p.to_bytes();
    let mut h = blk.header;
    h.proof_hash = proof_hash(&v1);
    assert!(verify_block(&h, Some(&prep.job), &v1, &fb()).unwrap_err().to_string().contains("seed mode"));
    assert!(pearl_job::PlainProof::from_bytes(&blk.proof).is_err());
    // Proof nonce field must equal the header nonce.
    let mut q = p.clone();
    q.nonce += 1;
    assert!(verify_block_proof(&prep.job, &q, &blk.header.pow_digest(), EASY, blk.header.nonce).is_err());
}

#[test]
fn fallback_block_roundtrip_and_rejections() {
    let t = template([0; 32]);
    let blk = mine_header(&t, &BlockWork::Fallback(fb()), 0..1024).unwrap().expect("a fallback block within 1024 nonces");
    verify_block(&blk.header, None, &blk.proof, &fb()).unwrap();
    // The job argument is ignored for fallback blocks.
    let (prep, _, _) = job_with(42);
    verify_block(&blk.header, Some(&prep.job), &blk.proof, &fb()).unwrap();
    for f in [
        (|h: &mut BlockHeader| h.miner = [0x98; 32]) as fn(&mut BlockHeader),
        |h| h.prev_hash[3] ^= 1,
        |h| h.nonce += 1,
    ] {
        let mut h = blk.header;
        f(&mut h);
        assert!(verify_block(&h, None, &blk.proof, &fb()).is_err());
    }
    // Other fallback dimensions: the proof's m, n no longer match.
    let other = FallbackParams { m: 128, ..fb() };
    assert!(verify_block(&blk.header, None, &blk.proof, &other).is_err());
    // Fallback work on a job header is refused.
    assert!(mine_header(&template(JOB_ID), &BlockWork::Fallback(fb()), 0..1).is_err());
}

#[test]
fn impossible_target_finds_nothing() {
    let (prep, _, _) = job_with(42);
    let mut t = template(JOB_ID);
    t.target_bits = IMPOSSIBLE;
    assert!(mine_header(&t, &BlockWork::Job(&prep), 0..3).unwrap().is_none());
}

/// Pinned vectors for other implementations (node, light client, wallet).
#[test]
fn pinned_vectors() {
    // PowBound seeds over fixed inputs.
    let (b, a) = derive_seeds(SeedDerivation::PowBound { pow_digest: [0x22; 32] }, &[0x11; 32], &[0xAA; 32], &[0xBB; 32], 192, 320);
    let mut bd = Vec::new();
    bd.extend_from_slice(&[0x11; 32]);
    bd.extend_from_slice(&[0x22; 32]);
    let hb_salted = *blake3::keyed_hash(&pearl_job::job::SEED_SALT_B, &{
        let mut m = [0u8; 64];
        m[..32].copy_from_slice(&[0xBB; 32]);
        m[32..36].copy_from_slice(&320u32.to_le_bytes());
        m
    })
    .as_bytes();
    bd.extend_from_slice(&hb_salted);
    assert_eq!(b, *blake3::hash(&bd).as_bytes(), "b = blake3(job_key || pow_digest || hash_b')");
    println!("powbound b={} a={}", hex::encode(b), hex::encode(a));
    // Fallback job for pow_digest = 0x09 * 32 on the default params (4 x 8) and on 64 x 64.
    assert_eq!((fb().m, fb().n), (4, 8));
    let fj = fallback_job(&[9; 32], &fb()).unwrap();
    println!("fallback 4x8 hash_a={} hash_b={}", hex::encode(fj.job.hash_a), hex::encode(fj.job.hash_b));
    let fj64 = fallback_job(&[9; 32], &FallbackParams { m: 64, n: 64, ..fb() }).unwrap();
    println!("fallback 64x64 hash_a={} hash_b={}", hex::encode(fj64.job.hash_a), hex::encode(fj64.job.hash_b));
    assert_eq!(hex::encode(fj64.job.hash_a), FB64_HASH_A);
    assert_eq!(hex::encode(fj64.job.hash_b), FB64_HASH_B);
    // Header digests of the test template.
    let h = template(JOB_ID);
    println!("template pow_digest={} block_hash={}", hex::encode(h.pow_digest()), hex::encode(h.block_hash()));
    assert_eq!(hex::encode(b), POW_B);
    assert_eq!(hex::encode(a), POW_A);
    assert_eq!(hex::encode(fj.job.hash_a), FB_HASH_A);
    assert_eq!(hex::encode(fj.job.hash_b), FB_HASH_B);
    assert_eq!(hex::encode(h.pow_digest()), TEMPLATE_POW_DIGEST);
}

const POW_B: &str = "e8f7d72728f2143cdb331ee04fd00792ebf1a07d9ef60367a8640ed8e78bbcac";
const POW_A: &str = "3eb3148589302d73b3e2e7dcd56f5cd4d7479187fe18fbfa761f0aea28c05fd2";
const FB_HASH_A: &str = "0e68c8d3bae3e18efe30f78e02f313921ce6dc7adcd6d496e3605fd7d445ee62";
const FB_HASH_B: &str = "33e22b577f66feb98851fcb1f4ff628dcd6c42857b8b7e24f37f08bd542f1ae6";
const FB64_HASH_A: &str = "f675c2fc3e528ca43cc87d78024ceace26202d5d71485c8cc75f7f3c847e4bc2";
const FB64_HASH_B: &str = "dbbaa3865d4f10ddcb5b3b337e42b05c739b6a89c64a0f7b24669136d2408c05";
const TEMPLATE_POW_DIGEST: &str = "de30bddaa1209116cffbf120dc3da81512c11e574fa098c1edb8cf3451bf3cee";

/// Storefront orders: the job's Pearl header id (the poster) is not the chain job id in the
/// block header. Mining and verifying pair them by the caller; the proof still binds both.
#[test]
fn job_block_with_header_id_other_than_chain_job_id() {
    let (prep, _, _) = job_with(42);
    let chain_job_id = [0x5e; 32];
    assert_ne!(chain_job_id, prep.job.header.job_pubkey);
    let blk = mine_header(&template(chain_job_id), &BlockWork::Job(&prep), 0..64).unwrap().expect("a block within 64 nonces");
    verify_block(&blk.header, Some(&prep.job), &blk.proof, &fb()).unwrap();
    // The same proof under another chain job id (another pow_digest) fails.
    let mut h = blk.header;
    h.job_id = [0x5f; 32];
    assert!(verify_block(&h, Some(&prep.job), &blk.proof, &fb()).is_err());
}
