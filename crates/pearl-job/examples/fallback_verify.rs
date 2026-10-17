//! Fallback block verify time for a given matrix shape (PROOF-FORMAT.md 7.4).
//! `cargo run --release -p pearl-job --example fallback_verify -- [m n [iters [k r]]]`
//! (k, r override the default config's common dim and rank; tile patterns unchanged)
//! Mines one fallback block at the easiest target, then times `pow::verify_block`.

use std::time::Instant;

use pearl_job::pow::{self, BlockHeader, BlockWork, FallbackParams};

fn main() {
    let args: Vec<u64> = std::env::args().skip(1).map(|a| a.parse().expect("numbers")).collect();
    let fp = FallbackParams::default();
    let m = args.first().copied().unwrap_or(fp.m as u64) as u32;
    let n = args.get(1).copied().unwrap_or(fp.n as u64) as u32;
    let iters = args.get(2).copied().unwrap_or(500) as u32;
    let mut config = fp.config;
    if let (Some(k), Some(r)) = (args.get(3), args.get(4)) {
        config.common_dim = *k as u32;
        config.rank = *r as u16;
    }
    let p = FallbackParams { m, n, config };
    let template = BlockHeader { version: 1, height: 5, prev_hash: [0xab; 32], timestamp_ms: 1_760_000_000_000, target_bits: 0x207fffff, ..Default::default() };
    let t0 = Instant::now();
    let mined = pow::mine_header(&template, &BlockWork::Fallback(p), 0..256).unwrap().expect("easy target");
    let mine = t0.elapsed();
    for _ in 0..10 {
        pow::verify_block(&mined.header, None, &mined.proof, &p).unwrap();
    }
    let t = Instant::now();
    for _ in 0..iters {
        pow::verify_block(&mined.header, None, &mined.proof, &p).unwrap();
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
    println!(
        "fallback m={m} n={n} k={} r={}: verify {per:.1} us/block (mean of {iters}), proof {} B, mine {:.1} ms (nonce {})",
        p.config.k(),
        p.config.rank,
        mined.proof.len(),
        mine.as_secs_f64() * 1e3,
        mined.header.nonce
    );
}
