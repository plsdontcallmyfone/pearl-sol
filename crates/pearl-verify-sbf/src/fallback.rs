//! Fallback blocks (`job_id` zero, PROOF-FORMAT.md section 7.4): the job and both
//! matrices come from `pow_digest`. The verifier must rebuild `hash_a` and `hash_b`,
//! which means hashing the whole of A and B^T. This is the expensive part, so it is
//! resumable: chunks are hashed in ranges and a [`Tree`] carries the CV stack.
//!
//! - A (m x k row-major) is the XOF stream of `derive_key(CTX_A, pow_digest)`, int7
//!   mapped; chunk i of A is XOF bytes `[1024 i, 1024 i + 1024)` (clipped to m*k, then
//!   zero padded), so A needs no scratch space.
//! - B (k x n row-major) is the CTX_B stream. The commitment is over B^T, whose chunk j
//!   gathers a column-strided slice of B, so B is generated into scratch space first
//!   ([`gen_b`]) and then hashed column-major ([`hash_bt`]).

use crate::blake3::{self, Cv, Output, Tree, KEYED_HASH};

pub const CTX_A: &[u8] = b"pearl-sol 2026-10-16 fallback matrix A v1";
pub const CTX_B: &[u8] = b"pearl-sol 2026-10-16 fallback matrix B v1";

#[inline(always)]
fn int7(x: u8) -> u8 {
    ((x & 0x7f) as i8 - 64) as u8
}

/// `0^32 || pow_digest || 0 u64 || 0 u32`.
pub fn job_header(pow_digest: &[u8; 32]) -> [u8; 76] {
    let mut h = [0u8; 76];
    h[32..64].copy_from_slice(pow_digest);
    h
}

pub fn xof(ctx: &[u8], pow_digest: &[u8; 32]) -> Output {
    let ck = blake3::derive_key_context(ctx);
    blake3::derive_key_output(&ck, pow_digest)
}

/// Write XOF blocks `[start, start+count)` of B (int7) into `scratch` (length k*n).
pub fn gen_b(out: &Output, scratch: &mut [u8], start: u64, count: u64) {
    let total = (scratch.len() / 64) as u64;
    let end = (start + count).min(total);
    for blk in start..end {
        let b = out.root_block(blk);
        let dst = &mut scratch[(blk * 64) as usize..(blk * 64 + 64) as usize];
        for i in 0..64 {
            dst[i] = int7(b[i]);
        }
    }
}

/// Number of 1024-byte chunks in a padded `len`-byte matrix.
pub fn chunks(len: u64) -> u64 {
    len.div_ceil(1024).max(1)
}

/// Resumable keyed-hash progress over one matrix commitment.
#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub tree: Tree,
    pub done: u64,
}

/// Hash chunks of B^T (`n` rows of `k`) from `b` (B row-major, k x n, int7 bytes).
/// Returns the root once the last chunk is in.
pub fn hash_bt(key: &[u8; 32], b: &[u8], k: u32, n: u32, st: &mut Progress, count: u64) -> Option<[u8; 32]> {
    let kw = blake3::key_words(key);
    let len = n as u64 * k as u64;
    let total = chunks(len);
    let end = (st.done + count).min(total);
    let mut root = None;
    while st.done < end {
        let j = st.done;
        let o = blake3::chunk_output_with(&kw, KEYED_HASH, 1024, j, |blk, buf| {
            for (t, slot) in buf.iter_mut().enumerate() {
                let gi = j * 1024 + blk as u64 * 64 + t as u64;
                if gi < len {
                    let c = gi / k as u64;
                    let l = gi % k as u64;
                    *slot = b[(l * n as u64 + c) as usize];
                }
            }
        });
        root = finish_or_push(&kw, st, o, j, total);
        st.done += 1;
    }
    root
}

/// Hash chunks of A (m x k) straight from the XOF stream.
pub fn hash_a(key: &[u8; 32], xa: &Output, m: u32, k: u32, st: &mut Progress, count: u64) -> Option<[u8; 32]> {
    let kw = blake3::key_words(key);
    let len = m as u64 * k as u64;
    let total = chunks(len);
    let end = (st.done + count).min(total);
    let mut root = None;
    while st.done < end {
        let i = st.done;
        let o = blake3::chunk_output_with(&kw, KEYED_HASH, 1024, i, |blk, buf| {
            let base = i * 1024 + blk as u64 * 64;
            if base >= len {
                return;
            }
            let x = xa.root_block(i * 16 + blk as u64);
            let n = ((len - base).min(64)) as usize;
            for t in 0..n {
                buf[t] = int7(x[t]);
            }
        });
        root = finish_or_push(&kw, st, o, i, total);
        st.done += 1;
    }
    root
}

fn finish_or_push(kw: &Cv, st: &mut Progress, o: Output, idx: u64, total: u64) -> Option<[u8; 32]> {
    if idx + 1 == total {
        Some(st.tree.finish(kw, KEYED_HASH, o).root_hash())
    } else {
        st.tree.push_chunk(kw, KEYED_HASH, o.chaining_value(), idx);
        None
    }
}
