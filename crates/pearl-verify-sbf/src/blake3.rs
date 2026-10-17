//! Minimal BLAKE3 (hash, keyed hash, derive key, XOF blocks, chunk and parent chaining
//! values) with no allocation and resumable tree state, for SBF. Follows the BLAKE3
//! reference implementation (CC0 / Apache-2.0). Cross-checked against the `blake3`
//! crate in tests/blake3.rs.

pub const OUT_LEN: usize = 32;
pub const BLOCK_LEN: usize = 64;
pub const CHUNK_LEN: usize = 1024;

pub const CHUNK_START: u32 = 1 << 0;
pub const CHUNK_END: u32 = 1 << 1;
pub const PARENT: u32 = 1 << 2;
pub const ROOT: u32 = 1 << 3;
pub const KEYED_HASH: u32 = 1 << 4;
pub const DERIVE_KEY_CONTEXT: u32 = 1 << 5;
pub const DERIVE_KEY_MATERIAL: u32 = 1 << 6;

pub const IV: [u32; 8] = [
    0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19,
];

pub type Cv = [u32; 8];

macro_rules! g {
    ($a:ident, $b:ident, $c:ident, $d:ident, $x:expr, $y:expr) => {
        $a = $a.wrapping_add($b).wrapping_add($x);
        $d = ($d ^ $a).rotate_right(16);
        $c = $c.wrapping_add($d);
        $b = ($b ^ $c).rotate_right(12);
        $a = $a.wrapping_add($b).wrapping_add($y);
        $d = ($d ^ $a).rotate_right(8);
        $c = $c.wrapping_add($d);
        $b = ($b ^ $c).rotate_right(7);
    };
}

/// The BLAKE3 compression function: full 16-word output. State in locals (one
/// round per macro call, message schedule from the reference MSG_PERMUTATION).
#[inline(never)]
pub fn compress(cv: &Cv, m: &[u32; 16], counter: u64, block_len: u32, flags: u32) -> [u32; 16] {
    let (mut v0, mut v1, mut v2, mut v3, mut v4, mut v5, mut v6, mut v7) = (cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7]);
    let (mut v8, mut v9, mut v10, mut v11) = (IV[0], IV[1], IV[2], IV[3]);
    let (mut v12, mut v13, mut v14, mut v15) = (counter as u32, (counter >> 32) as u32, block_len, flags);
    macro_rules! round {
        ($i0:literal, $i1:literal, $i2:literal, $i3:literal, $i4:literal, $i5:literal, $i6:literal, $i7:literal,
         $i8:literal, $i9:literal, $i10:literal, $i11:literal, $i12:literal, $i13:literal, $i14:literal, $i15:literal) => {
            g!(v0, v4, v8, v12, m[$i0], m[$i1]);
            g!(v1, v5, v9, v13, m[$i2], m[$i3]);
            g!(v2, v6, v10, v14, m[$i4], m[$i5]);
            g!(v3, v7, v11, v15, m[$i6], m[$i7]);
            g!(v0, v5, v10, v15, m[$i8], m[$i9]);
            g!(v1, v6, v11, v12, m[$i10], m[$i11]);
            g!(v2, v7, v8, v13, m[$i12], m[$i13]);
            g!(v3, v4, v9, v14, m[$i14], m[$i15]);
        };
    }
    round!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15);
    round!(2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8);
    round!(3, 4, 10, 12, 13, 2, 7, 14, 6, 5, 9, 0, 11, 15, 8, 1);
    round!(10, 7, 12, 9, 14, 3, 13, 15, 4, 0, 11, 2, 5, 8, 1, 6);
    round!(12, 13, 9, 11, 15, 10, 14, 8, 7, 2, 5, 3, 0, 1, 6, 4);
    round!(9, 14, 11, 5, 8, 12, 15, 1, 13, 3, 0, 10, 2, 6, 4, 7);
    round!(11, 15, 5, 0, 1, 9, 8, 6, 14, 10, 2, 12, 3, 4, 7, 13);
    [
        v0 ^ v8, v1 ^ v9, v2 ^ v10, v3 ^ v11, v4 ^ v12, v5 ^ v13, v6 ^ v14, v7 ^ v15,
        v8 ^ cv[0], v9 ^ cv[1], v10 ^ cv[2], v11 ^ cv[3], v12 ^ cv[4], v13 ^ cv[5], v14 ^ cv[6], v15 ^ cv[7],
    ]
}

#[inline(always)]
fn first8(s: &[u32; 16]) -> Cv {
    [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]
}

/// Little-endian words of up to 64 bytes, zero padded.
#[inline(always)]
pub fn block_words(b: &[u8]) -> [u32; 16] {
    let mut m = [0u32; 16];
    if b.len() == BLOCK_LEN {
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
    } else {
        let mut tmp = [0u8; BLOCK_LEN];
        tmp[..b.len()].copy_from_slice(b);
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([tmp[4 * i], tmp[4 * i + 1], tmp[4 * i + 2], tmp[4 * i + 3]]);
        }
    }
    m
}

pub fn key_words(key: &[u8; 32]) -> Cv {
    let mut k = [0u32; 8];
    for (i, w) in k.iter_mut().enumerate() {
        *w = u32::from_le_bytes([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]);
    }
    k
}

pub fn cv_bytes(cv: &Cv) -> [u8; 32] {
    let mut o = [0u8; 32];
    for (i, w) in cv.iter().enumerate() {
        o[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    o
}

/// A node whose final compression has not run yet (so it can become a root or a CV).
#[derive(Clone, Copy)]
pub struct Output {
    pub cv: Cv,
    pub block: [u32; 16],
    pub counter: u64,
    pub block_len: u32,
    pub flags: u32,
}

impl Output {
    pub fn chaining_value(&self) -> Cv {
        first8(&compress(&self.cv, &self.block, self.counter, self.block_len, self.flags))
    }
    pub fn root_hash(&self) -> [u8; 32] {
        cv_bytes(&first8(&compress(&self.cv, &self.block, 0, self.block_len, self.flags | ROOT)))
    }
    /// 64 bytes of root output at XOF block `out_counter`.
    pub fn root_block(&self, out_counter: u64) -> [u8; 64] {
        let w = compress(&self.cv, &self.block, out_counter, self.block_len, self.flags | ROOT);
        let mut o = [0u8; 64];
        for (i, x) in w.iter().enumerate() {
            o[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
        }
        o
    }
}

/// Output of one chunk (`data.len() <= 1024`), chunk counter `counter`.
pub fn chunk_output(key: &Cv, flags: u32, data: &[u8], counter: u64) -> Output {
    let mut cv = *key;
    let nblocks = if data.is_empty() { 1 } else { data.len().div_ceil(BLOCK_LEN) };
    for b in 0..nblocks - 1 {
        let m = block_words(&data[b * BLOCK_LEN..(b + 1) * BLOCK_LEN]);
        let f = flags | if b == 0 { CHUNK_START } else { 0 };
        cv = first8(&compress(&cv, &m, counter, BLOCK_LEN as u32, f));
    }
    let last = &data[(nblocks - 1) * BLOCK_LEN..];
    let f = flags | CHUNK_END | if nblocks == 1 { CHUNK_START } else { 0 };
    Output { cv, block: block_words(last), counter, block_len: last.len() as u32, flags: f }
}

/// Chunk built from a closure filling each 64-byte block (`len` bytes in total, <= 1024).
pub fn chunk_output_with(key: &Cv, flags: u32, len: usize, counter: u64, mut fill: impl FnMut(usize, &mut [u8; 64])) -> Output {
    let mut cv = *key;
    let nblocks = if len == 0 { 1 } else { len.div_ceil(BLOCK_LEN) };
    let mut buf = [0u8; 64];
    for b in 0..nblocks {
        let blen = if b + 1 == nblocks { len - b * BLOCK_LEN } else { BLOCK_LEN };
        buf.fill(0);
        fill(b, &mut buf);
        let m = block_words(&buf);
        let f = flags | if b == 0 { CHUNK_START } else { 0 };
        if b + 1 == nblocks {
            return Output { cv, block: m, counter, block_len: blen as u32, flags: f | CHUNK_END };
        }
        cv = first8(&compress(&cv, &m, counter, BLOCK_LEN as u32, f));
    }
    unreachable!()
}

pub fn parent_output(key: &Cv, flags: u32, l: &Cv, r: &Cv) -> Output {
    let mut m = [0u32; 16];
    m[..8].copy_from_slice(l);
    m[8..].copy_from_slice(r);
    Output { cv: *key, block: m, counter: 0, block_len: BLOCK_LEN as u32, flags: flags | PARENT }
}

pub fn parent_cv(key: &Cv, flags: u32, l: &Cv, r: &Cv) -> Cv {
    parent_output(key, flags, l, r).chaining_value()
}

/// Resumable chunk tree: push chunk CVs in order, then finish with the last chunk's output.
/// The stack is plain bytes so it can live in an account.
#[derive(Clone, Copy)]
pub struct Tree {
    pub stack: [Cv; 54],
    pub len: usize,
}

impl Default for Tree {
    fn default() -> Self {
        Self { stack: [[0; 8]; 54], len: 0 }
    }
}

impl Tree {
    /// Add the CV of chunk number `chunk_index` (0-based) that is NOT the last chunk.
    pub fn push_chunk(&mut self, key: &Cv, flags: u32, mut cv: Cv, chunk_index: u64) {
        let mut total = chunk_index + 1;
        while total & 1 == 0 {
            self.len -= 1;
            cv = parent_cv(key, flags, &self.stack[self.len], &cv);
            total >>= 1;
        }
        self.stack[self.len] = cv;
        self.len += 1;
    }
    /// Root output given the last chunk's output.
    pub fn finish(&self, key: &Cv, flags: u32, last: Output) -> Output {
        let mut out = last;
        let mut i = self.len;
        while i > 0 {
            i -= 1;
            out = parent_output(key, flags, &self.stack[i], &out.chaining_value());
        }
        out
    }
}

/// Root output of a whole input under `key` / `flags` (KEYED_HASH or 0 or DERIVE_KEY_MATERIAL).
pub fn hash_output(key: &Cv, flags: u32, input: &[u8]) -> Output {
    let n = if input.is_empty() { 1 } else { input.len().div_ceil(CHUNK_LEN) };
    let mut t = Tree::default();
    for c in 0..n - 1 {
        let cv = chunk_output(key, flags, &input[c * CHUNK_LEN..(c + 1) * CHUNK_LEN], c as u64).chaining_value();
        t.push_chunk(key, flags, cv, c as u64);
    }
    let last = chunk_output(key, flags, &input[(n - 1) * CHUNK_LEN..], (n - 1) as u64);
    t.finish(key, flags, last)
}

pub fn hash(input: &[u8]) -> [u8; 32] {
    hash_output(&IV, 0, input).root_hash()
}

pub fn keyed_hash(key: &[u8; 32], input: &[u8]) -> [u8; 32] {
    hash_output(&key_words(key), KEYED_HASH, input).root_hash()
}

/// Hash of several parts concatenated (total <= 1024 bytes, one chunk).
pub fn hash_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = [0u8; CHUNK_LEN];
    let mut n = 0;
    for p in parts {
        buf[n..n + p.len()].copy_from_slice(p);
        n += p.len();
    }
    hash(&buf[..n])
}

/// `blake3_derive_key(context)` context key words.
pub fn derive_key_context(context: &[u8]) -> Cv {
    let k = hash_output(&IV, DERIVE_KEY_CONTEXT, context).root_hash();
    key_words(&k)
}

/// Root output of `derive_key(context, material)`; its XOF stream is `root_block(i)`.
pub fn derive_key_output(context_key: &Cv, material: &[u8]) -> Output {
    hash_output(context_key, DERIVE_KEY_MATERIAL, material)
}
