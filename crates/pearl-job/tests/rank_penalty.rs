//! Rank penalty (Pearl mainnet rule, ported 2026-10-17) and the low-rank / zero
//! matrix work ratio. Analysis: research/NOTES-rank-penalty.md.
//!
//! Part 1 ports Pearl's own `rank_penalty_tests` (zk-pow/src/api/sanity_checks.rs:252-345).
//! Part 2 measures the best attack we know on zero (or low-rank) job matrices: the
//! noised product is then pure noise, whose per-rank-block increments factor through a
//! spanning forest of the sparse noise factor. It reproduces the real jackpot exactly,
//! so its multiply count is a real upper bound on what a zero-matrix miner pays.

use pearl_job::data::random_matrices;
use pearl_job::job::{
    check_shape, difficulty_bound, meets_target, nbits_to_target, penalized_adjustment_factor, penalized_target_bound,
    unpenalized_difficulty_bound, PENALTY_BASE_RANK,
};
use pearl_job::noise::{jackpot, jackpot_hash, noise_factors, noise_for_indices, JACKPOT_SIZE, LROT_PER_TILE};
use pearl_job::pattern::PeriodicPattern;
use pearl_job::pow::{self, BlockHeader, BlockWork, FallbackParams};
use pearl_job::{commit_a, commit_b, job_key, Job, JobHeader, MiningConfig, PreparedJob};
use primitive_types::U256;

const TEST_NBITS: u32 = 0x1d00ffff;

/// Pearl's `test_config`: rows [0,8,64,72], cols [0,1,8,9,32,33,40,41] (h*w = 32).
fn pearl_cfg(rank: u16, k: u32) -> MiningConfig {
    MiningConfig {
        common_dim: k,
        rank,
        rows_pattern: PeriodicPattern::from_list(&[0, 8, 64, 72]).unwrap(),
        cols_pattern: PeriodicPattern::from_list(&[0, 1, 8, 9, 32, 33, 40, 41]).unwrap(),
    }
}

fn le(v: U256) -> [u8; 32] {
    let mut b = [0u8; 32];
    v.to_little_endian(&mut b);
    b
}

// ------------------------------------------------------------ part 1: the rule

#[test]
fn penalty_is_neutral_at_base_rank() {
    let c = pearl_cfg(128, 4096);
    assert_eq!(difficulty_bound(TEST_NBITS, &c), unpenalized_difficulty_bound(TEST_NBITS, &c));
    // and on the localnet default
    let d = MiningConfig::localnet_default();
    assert_eq!((d.k(), d.r()), (2048, PENALTY_BASE_RANK));
    assert_eq!(difficulty_bound(TEST_NBITS, &d), nbits_to_target(TEST_NBITS) * U256::from(32 * 2048));
}

#[test]
fn penalty_divides_bound_in_proportion_to_rank() {
    for multiple in [2u16, 4, 8] {
        let c = pearl_cfg(128 * multiple, 65536);
        assert_eq!(
            difficulty_bound(TEST_NBITS, &c),
            unpenalized_difficulty_bound(TEST_NBITS, &c) / U256::from(multiple),
            "rank {} should be penalized {multiple}x",
            c.rank
        );
        assert_eq!(penalized_adjustment_factor(&c), 32 * (65536 / c.r()) * 128);
    }
}

#[test]
fn rejects_rank_below_base() {
    for (r, k) in [(64u16, 4096u32), (32, 1024), (32, 4096)] {
        let err = check_shape(&pearl_cfg(r, k), 128, 128).unwrap_err().to_string();
        assert!(err.contains("rank must be >= 128"), "r={r}: {err}");
    }
    check_shape(&pearl_cfg(128, 2048), 128, 128).unwrap();
    // k >= 16r still holds: r=128 needs k >= 2048
    assert!(check_shape(&pearl_cfg(128, 1024), 128, 128).is_err());
}

#[test]
fn accepts_jackpot_at_the_bound_and_rejects_above_it() {
    for r in [128u16, 256] {
        let c = pearl_cfg(r, 65536);
        let bound = penalized_target_bound(nbits_to_target(TEST_NBITS), &c).unwrap();
        assert_eq!(bound, difficulty_bound(TEST_NBITS, &c));
        assert!(meets_target(&le(bound), &difficulty_bound(TEST_NBITS, &c)));
        assert!(!meets_target(&le(bound + 1), &difficulty_bound(TEST_NBITS, &c)));
        if r > 128 {
            // an unpenalized comparison would accept bound + 1
            assert!(meets_target(&le(bound + 1), &unpenalized_difficulty_bound(TEST_NBITS, &c)));
        }
    }
}

#[test]
fn penalized_target_bound_refuses_overflow() {
    let c = pearl_cfg(128, 4096);
    assert_eq!(penalized_target_bound(U256::one(), &c), Some(U256::from(32 * 4096)));
    assert_eq!(penalized_target_bound(U256::MAX, &c), None);
    // consensus path saturates instead
    assert_eq!(difficulty_bound(0x2100ffff, &c), U256::MAX);
}

fn job_on(config: MiningConfig, m: u32, n: u32) -> (Job, Vec<i8>, Vec<i8>) {
    let header = JobHeader { job_pubkey: [7; 32], terms_hash: [9; 32], created_slot: 5, target_bits: 0x1e040000 };
    let (a, b) = random_matrices(m as usize, n as usize, config.k(), Some(3));
    let key = job_key(&header, &config);
    let job = Job { header, config, m, n, hash_a: commit_a(&key, &a), hash_b: commit_b(&key, &b, config.k(), n as usize) };
    (job, a, b)
}

/// The pre-port localnet config (k=1024, r=32) can no longer be prepared, mined or
/// verified, and a block on it is refused by `verify_block` even with a valid proof.
#[test]
fn old_r32_config_is_refused_everywhere() {
    let old = MiningConfig { common_dim: 1024, rank: 32, ..MiningConfig::localnet_default() };
    let (job, a, b) = job_on(old, 8, 16);
    assert!(job.sanity_check().is_err());
    assert!(PreparedJob::new(job.clone(), &a, &b).is_err());
    let fb = FallbackParams { config: old, ..FallbackParams::default() };
    assert!(pow::fallback_job(&[1; 32], &fb).is_err());
    let t = BlockHeader { version: 1, height: 1, target_bits: 0x2000ffff, ..Default::default() };
    assert!(pow::mine_header(&t, &BlockWork::Fallback(fb), 0..1).is_err());
}

/// Honest mining on the new default: a 4x8 fallback block still verifies, the proof is
/// 2 leaves per row (k=2048), and the per-attempt bound is twice the old default's
/// (32*2048 vs 32*1024) for twice the multiply work, so expected work per block in
/// MACs is unchanged at equal target_bits.
#[test]
fn honest_default_mines_and_verifies() {
    let fb = FallbackParams::default();
    let t = BlockHeader { version: 1, height: 1, target_bits: 0x2000ffff, ..Default::default() };
    let blk = pow::mine_header(&t, &BlockWork::Fallback(fb), 0..64).unwrap().expect("easy target");
    pow::verify_block(&blk.header, None, &blk.proof, &fb).unwrap();
    assert!(blk.proof.len() > 24 * 1024 && blk.proof.len() < 25 * 1024 + 512, "{}", blk.proof.len());
    let old = MiningConfig { common_dim: 1024, rank: 32, ..MiningConfig::localnet_default() };
    assert_eq!(penalized_adjustment_factor(&fb.config), 2 * old.h() * old.w() * old.dot_product_length());
}

// ------------------------------------------------------------ part 2: work ratio

/// Spanning forest of the r-vertex multigraph whose edges are the block's sparse
/// noise pairs `[p, q]` (column t of the factor is `e_p - e_q`).
struct Forest {
    /// Block-local indices of the tree edges.
    tree: Vec<usize>,
    /// For each non-tree edge t: the tree edges (as positions in `tree`) and signs
    /// with `e_p - e_q = sum sign * (e_p' - e_q')`.
    cycles: Vec<(usize, Vec<(usize, i32)>)>,
}

fn find(parent: &mut [usize], x: usize) -> usize {
    let mut x = x;
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn forest(edges: &[[u32; 2]], r: usize) -> Forest {
    let mut parent: Vec<usize> = (0..r).collect();
    let mut tree = Vec::new();
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); r]; // (neighbour, tree position)
    let mut non_tree = Vec::new();
    for (t, &[p, q]) in edges.iter().enumerate() {
        let (p, q) = (p as usize, q as usize);
        let (rp, rq) = (find(&mut parent, p), find(&mut parent, q));
        if rp == rq {
            non_tree.push(t);
        } else {
            parent[rp] = rq;
            adj[p].push((q, tree.len()));
            adj[q].push((p, tree.len()));
            tree.push(t);
        }
    }
    let mut cycles = Vec::new();
    for t in non_tree {
        let [p, q] = edges[t];
        // path q -> p in the forest (BFS), each step a -> b over edge e adds e_b - e_a
        let (p, q) = (p as usize, q as usize);
        let mut prev: Vec<Option<(usize, usize)>> = vec![None; r];
        let mut seen = vec![false; r];
        let mut queue = std::collections::VecDeque::from([q]);
        seen[q] = true;
        while let Some(a) = queue.pop_front() {
            if a == p {
                break;
            }
            for &(b, pos) in &adj[a] {
                if !seen[b] {
                    seen[b] = true;
                    prev[b] = Some((a, pos));
                    queue.push_back(b);
                }
            }
        }
        let mut path = Vec::new();
        let mut b = p;
        while b != q {
            let (a, pos) = prev[b].expect("p and q are in one tree");
            let [ep, _] = edges[tree[pos]];
            // tree edge column is e_ep - e_eq; step a -> b contributes e_b - e_a
            path.push((pos, if ep as usize == b { 1 } else { -1 }));
            b = a;
        }
        cycles.push((t, path));
    }
    Forest { tree, cycles }
}

/// Rank of a small integer matrix over GF(2^31 - 1) (equals the rational rank with
/// overwhelming probability for these 0/±1/±2 entries).
fn rank_mod_p(mut m: Vec<Vec<i64>>) -> usize {
    const P: i64 = (1 << 31) - 1;
    let pow = |mut b: i64, mut e: i64| {
        let mut r = 1i64;
        b = b.rem_euclid(P);
        while e > 0 {
            if e & 1 == 1 {
                r = r * b % P;
            }
            b = b * b % P;
            e >>= 1;
        }
        r
    };
    for row in m.iter_mut() {
        for x in row.iter_mut() {
            *x = x.rem_euclid(P);
        }
    }
    let (rows, cols) = (m.len(), m[0].len());
    let mut rank = 0;
    for c in 0..cols {
        let Some(piv) = (rank..rows).find(|&i| m[i][c] != 0) else { continue };
        m.swap(rank, piv);
        let inv = pow(m[rank][c], P - 2);
        for i in rank + 1..rows {
            if m[i][c] != 0 {
                let f = m[i][c] * inv % P;
                for j in c..cols {
                    m[i][j] = (m[i][j] - f * m[rank][j]).rem_euclid(P);
                }
            }
        }
        rank += 1;
    }
    rank
}

struct Ratio {
    honest_macs: u64,
    attack_macs: u64,
    attack_adds: u64,
    rank_bound_macs: u64,
}

/// Jackpot of one h x w tile of a ZERO job (A = 0, B = 0) by the forest attack.
/// Asserts it equals the honest jackpot and returns the multiply counts.
fn zero_tile_attack(k: usize, r: usize, h: usize, w: usize, seed: u8, with_rank: bool) -> Ratio {
    let (b_seed, a_seed) = ([seed; 32], [seed ^ 0x5a; 32]);
    let rows: Vec<usize> = (0..h).collect();
    let cols: Vec<usize> = (0..w).collect();
    let f = noise_factors(k, r, (&b_seed, &a_seed), &rows, &cols);
    // honest reference: zero strips plus noise through the real jackpot
    let zeros_a = vec![vec![0i8; k]; h];
    let zeros_b = vec![vec![0i8; k]; w];
    let honest = jackpot(k, r, &zeros_a, &zeros_b, &noise_for_indices(k, r, (&b_seed, &a_seed), &rows, &cols));

    let x: Vec<Vec<i32>> = f.e_al.iter().map(|v| v.iter().map(|&e| e as i32).collect()).collect();
    let z: Vec<Vec<i32>> = f.e_br_t.iter().map(|v| v.iter().map(|&e| e as i32).collect()).collect();
    let dlen = k - k % r;
    let mut acc = vec![0i32; h * w];
    let mut msg = [0u32; JACKPOT_SIZE];
    let mut out = Ratio { honest_macs: 0, attack_macs: 0, attack_adds: 0, rank_bound_macs: 0 };
    for blk in 0..dlen / r {
        let ea = &f.e_ar_t[blk * r..(blk + 1) * r];
        let eb = &f.e_bl[blk * r..(blk + 1) * r];
        // a[i][t] = x_i[p_t] - x_i[q_t]  (A' = noise), b[j][t] likewise for B'^T
        let a: Vec<Vec<i32>> = x.iter().map(|xi| ea.iter().map(|&[p, q]| xi[p as usize] - xi[q as usize]).collect()).collect();
        let b: Vec<Vec<i32>> = z.iter().map(|zj| eb.iter().map(|&[p, q]| zj[p as usize] - zj[q as usize]).collect()).collect();
        // factor through the side with the smaller forest (more cycles)
        let (fa, fb) = (forest(ea, r), forest(eb, r));
        let use_a = fa.tree.len() <= fb.tree.len();
        let (fr, keep, fold) = if use_a { (&fa, &a, &b) } else { (&fb, &b, &a) };
        // fold[.][t] for non-tree t into the tree positions: adds only
        let folded: Vec<Vec<i32>> = fold
            .iter()
            .map(|v| {
                let mut g: Vec<i32> = fr.tree.iter().map(|&t| v[t]).collect();
                for (t, path) in &fr.cycles {
                    for &(pos, s) in path {
                        g[pos] = g[pos].wrapping_add(s * v[*t]);
                    }
                }
                g
            })
            .collect();
        out.attack_adds += (fold.len() * fr.cycles.iter().map(|(_, p)| p.len()).sum::<usize>()) as u64;
        for i in 0..h {
            for j in 0..w {
                let (ki, fj) = if use_a { (&keep[i], &folded[j]) } else { (&keep[j], &folded[i]) };
                let mut s = 0i32;
                for (pos, &t) in fr.tree.iter().enumerate() {
                    s = s.wrapping_add(ki[t].wrapping_mul(fj[pos]));
                }
                acc[i * w + j] = acc[i * w + j].wrapping_add(s);
            }
        }
        out.attack_macs += (h * w * fr.tree.len()) as u64;
        out.honest_macs += (h * w * r) as u64;
        if with_rank {
            // G = U_A U_B^T (r x r), the exact bilinear form of this block's increment
            let mut g = vec![vec![0i64; r]; r];
            for t in 0..r {
                let ([pa, qa], [pb, qb]) = (ea[t], eb[t]);
                for (u, su) in [(pa, 1i64), (qa, -1)] {
                    for (v, sv) in [(pb, 1i64), (qb, -1)] {
                        g[u as usize][v as usize] += su * sv;
                    }
                }
            }
            out.rank_bound_macs += (h * w * rank_mod_p(g)) as u64;
        }
        let xr = acc.iter().fold(0u32, |a, &v| a ^ v as u32);
        let tid = blk % JACKPOT_SIZE;
        msg[tid] = msg[tid].rotate_left(LROT_PER_TILE) ^ xr;
    }
    assert_eq!(msg, honest, "forest attack must reproduce the real jackpot (k={k} r={r} seed={seed})");
    assert_eq!(jackpot_hash(&msg, &a_seed), jackpot_hash(&honest, &a_seed));
    out
}

fn ratios(k: usize, r: usize, seeds: u8, with_rank: bool) -> (f64, f64, f64) {
    let mut t = Ratio { honest_macs: 0, attack_macs: 0, attack_adds: 0, rank_bound_macs: 0 };
    for s in 0..seeds {
        let o = zero_tile_attack(k, r, 4, 8, s.wrapping_mul(37).wrapping_add(1), with_rank);
        t.honest_macs += o.honest_macs;
        t.attack_macs += o.attack_macs;
        t.attack_adds += o.attack_adds;
        t.rank_bound_macs += o.rank_bound_macs;
    }
    let h = t.honest_macs as f64;
    (t.attack_macs as f64 / h, t.rank_bound_macs as f64 / h, t.attack_adds as f64 / h)
}

/// Zero matrices on the new default (k=2048, r=128): the best attack we know pays
/// about 0.8 of the honest multiplies, nowhere near the r/k = 1/16 of the
/// "noised product is low rank" shortcut (which only hashes the final product, not
/// Pearl's per-rank-block transcript, and so produces the wrong jackpot).
#[test]
fn zero_matrices_work_ratio_default() {
    let (attack, bound, adds) = ratios(2048, 128, 6, true);
    eprintln!("k=2048 r=128 zero A,B: forest attack {attack:.3} x honest MACs (+{adds:.3} adds), rank bound {bound:.3}");
    assert!((0.70..0.92).contains(&attack), "attack ratio {attack}");
    assert!(bound <= attack + 1e-9 && bound > 0.70, "rank bound {bound}");
}

/// Same measurement at the old r=32 default and above the base rank. The ratio is
/// about the same at every r; what the penalty changes above r=128 is the credit.
#[test]
fn zero_matrices_work_ratio_across_ranks() {
    for (k, r) in [(1024usize, 32usize), (2048, 128), (4096, 256)] {
        let (attack, _, adds) = ratios(k, r, 3, false);
        eprintln!("k={k} r={r} zero A,B: forest attack {attack:.3} x honest MACs (+{adds:.3} adds)");
        assert!((0.65..0.95).contains(&attack), "k={k} r={r}: {attack}");
    }
}
