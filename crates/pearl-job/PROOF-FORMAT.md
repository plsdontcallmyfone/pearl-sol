# Plain share proof: byte format and verification (PSJ1)

This is the spec an SBF port of `verify_share` (M2) has to match. The reference is
`src/verify.rs`. All integers are little-endian. "blake3(x)" is unkeyed BLAKE3-256,
"blake3_k(x, key)" is BLAKE3 keyed mode.

## 1. Inputs the verifier takes from the chain, not from the proof

From the `jobs::Job` account (agent/programs/jobs):

| field | bytes | use |
|---|---|---|
| job pubkey | 32 | header[0..32] |
| terms_hash | 32 | header[32..64] |
| created_slot | u64 | header[64..72] |
| share_target (target_bits) | u32 | header[72..76], and the share difficulty |
| config | 52 | mining configuration (section 2) |
| m, n | u32 each | A is m x k, B is k x n |
| hash_a, hash_b | 32 each | Merkle roots the proof must open against |

From the transaction: the miner pubkey (the signer).

```
header  = job_pubkey || terms_hash || created_slot u64 || target_bits u32      (76 bytes)
job_key = blake3(header || config)                                              (128-byte input)
```

## 2. Mining config (52 bytes, Pearl `MiningConfiguration`, dense only)

```
0   u32  k (common_dim)
4   u16  rank r
6   u16  mma_type, must be 0 (int7 x int7 -> int32)
8   6    rows_pattern  (PeriodicPattern, see below)
14  6    cols_pattern
20  32   reserved (Pearl MoE trailer), must be all zero
```

PeriodicPattern: 3 x (factor-1 u8, length-1 u8). Decoding (Pearl `from_bytes`):
`min_stride = 1; for each pair: factor = 1+b0, length = 1+b1, stride = factor*min_stride,
min_stride = stride*length`. The pattern's index list is every
`a*stride0 + b*stride1 + c*stride2` with `a < length0`, `b < length1`, `c < length2`,
in that nesting order (dimension 0 fastest). `size = length0*length1*length2`,
`period = stride2*length2`. An offset t is a valid tile start iff, walking dimensions
from last to first, `t %= stride*length; t < stride` holds at each step.

Localnet default (`pearl-job config`): k=1024, r=32, rows 0..4, cols 0..8, h = 4, w = 8:
`00040000 2000 0000 000300000000 000700000000` followed by 32 zero bytes.

Shape rules (`job::check_shape`, Pearl `public_params_sanity_check`): r a power of
two in [32, 1024]; k multiple of 64, 1024 <= k <= 65536, 16r <= k <= 4r^2;
h, w even; 32 <= h*w <= 256; dlen = k - k % r is a multiple of 64; m, n <= 2^24;
m and n are multiples of the row and column pattern periods.

## 3. Proof bytes

```
off  size        field
0    4           magic "PSJ1"
4    1           version = 1
5    1           seed mode = 2 (miner-bound; nothing else is accepted)
6    4   u32     m
10   4   u32     n
14   4   u32     k
18   2   u16     rank
20   8   u64     nonce
28   4   u32     t_rows   first row of the tile (rows = rows_pattern + t_rows)
32   4   u32     t_cols   first column of the tile (cols = cols_pattern + t_cols)
36   ...         opening A    (over A, m x k int8, row-major)
...  ...         opening B^T  (over B^T, n x k int8, row-major)

opening:
     4   u32     total_leaves     = ceil(rows*k / 1024)
     2   u16     L                leaf count
     4*L u32[L]  leaf indices     strictly increasing
     1024*L      leaf data        the 1024-byte chunks, in index order
     2   u16     S                sibling count
     32*S        siblings         chaining values, in consumption order (section 4)
```

Nothing may follow the second opening. Row indices and Merkle roots are not on the
wire: rows come from the job config plus `t_rows` / `t_cols`, roots come from the job.

Measured size, localnet config (4 + 8 leaves of 1 KiB): **12,612 bytes** on a
64 x 64 job, 12,740 bytes on 256 x 256 (more siblings). Deeper trees add 32 bytes
per sibling.

## 4. Verification algorithm

1. Parse; reject on bad magic, version, mode, truncation, trailing bytes, size > 4 MiB.
2. `m, n` must equal the job's; `k, rank` must equal the config's. Shape rules hold.
3. `t_rows` is a valid row-pattern offset and `t_rows + max(rows_pattern) < m`;
   same for `t_cols` with n. `rows = rows_pattern + t_rows`, `cols = cols_pattern + t_cols`.
4. For each opening (A with `rows`, total m rows; B^T with `cols`, total n rows):
   - `total_leaves == ceil(total_rows*k/1024)`.
   - leaf indices == exactly the chunks covering those rows:
     `{ floor(r*k/1024) ..= floor(((r+1)*k - 1)/1024) : r in rows }`, sorted, unique.
   - Recompute the root (below) and compare with `hash_a` / `hash_b`.
   - Extract each row's first `dlen` bytes (offset `row*k`) as int8; every value
     must be in [-64, 64].
5. Seeds (miner-bound; SPEC 3.4, NOTES-proof Q4):
   ```
   SALT_A = blake3("pearl/cert-v3/noise-seed/A"), SALT_B = blake3("pearl/cert-v3/noise-seed/B")
   hash_a' = blake3_k(hash_a || m u32 || 0^28, SALT_A)          (Pearl V3 salting)
   hash_b' = blake3_k(hash_b || n u32 || 0^28, SALT_B)
   b_seed  = blake3(job_key || miner [32] || nonce u64 || hash_b')   (104-byte input)
   a_seed  = blake3(b_seed || hash_a')
   ```
6. Noise for the tile (Pearl `pearl_noise.rs`). Let
   `rh(i, seed, key, slot) = blake3_k(msg, key)` where `msg` is 64 bytes of zeros with
   `(1+i) as i32` written at byte `4*slot`, and `seed` written at bytes 32..64.
   `LABEL_A = "A_tensor"` and `LABEL_B = "B_tensor"`, each zero-padded to 32 bytes.
   - uniform(row, seed, key): r values; value j of row `row` is byte `(row*r + j) % 32`
     of `rh((row*r + j) / 32, seed, key, 0)`, mapped `x -> (x & 63) - 32`.
   - perm(seed, key): k pairs; for block `i` (8 pairs), `h = rh(i, seed, key, 1)`,
     pair `8i + j` uses `x = u32 LE h[4j..4j+4]`: `p = x & (r-1)`,
     `q = p ^ (1 + ((r-1) * x) >> 32)`.
   - `E_A[row][l] = U_A[row][p_l] - U_A[row][q_l]` with `U_A = uniform(row, LABEL_A, a_seed)`,
     `(p_l, q_l) = perm(LABEL_A, a_seed)[l]`.
   - `E_B^T[col][l] = U_B[col][p_l] - U_B[col][q_l]` with `U_B = uniform(col, LABEL_B, b_seed)`,
     `(p_l, q_l) = perm(LABEL_B, b_seed)[l]`.
   All as i8 (the difference fits in [-63, 63]).
7. Jackpot (Pearl `jackpot/helper.rs`): `acc[h][w] = 0`, `msg[16] = 0`. For
   `ll = r, 2r, ..., <= k`: `acc[u][v] += sum_{l in [ll-r, ll)} (A[u][l]+E_A[u][l]) * (Bt[v][l]+E_B^T[v][l])`
   (i32); `x = XOR of all acc[u][v] as u32`; `t = (ll/r - 1) % 16`;
   `msg[t] = rotl(msg[t], 13) ^ x`.
8. `jackpot_hash = blake3_k(msg as 16 x u32 LE, a_seed)`.
9. Difficulty: `target = nbits_to_target(share_target)` (Bitcoin compact; zero if the
   mantissa is 0, the exponent is 0 or bit 0x00800000 is set),
   `bound = min(U256::MAX, target * h*w*dlen)`. Accept iff
   `U256::from_le(jackpot_hash) <= bound`. No rank penalty (a Pearl-mainnet rule).

### Root recomputation (keyed BLAKE3 chunk tree)

The tree is BLAKE3's own tree with `job_key` as the key, so the root equals
`blake3_k(pad1024(data), job_key)`.
- Leaf CV: BLAKE3 chunk chaining value of the 1024-byte chunk with chunk counter =
  leaf index, keyed, not finalized as root (`blake3::hazmat` `set_input_offset` +
  `finalize_non_root`).
- `total_leaves == 1`: the root is `blake3_k(chunk, job_key)` and S must be 0.
- Otherwise, levels from the bottom while the level has more than 2 nodes: walk the
  known nodes in increasing index. Even index i: the right child is the known node
  i+1, else the next sibling if `i+1 < level_len`, else none (the node is promoted
  unchanged). Odd index i whose left neighbour is not known: the left child is the
  next sibling. Parent = keyed BLAKE3 parent CV (non-root). Next level length is
  `ceil(len/2)`.
- Last level (2 nodes): fill node 0 and 1 from known nodes or the next siblings,
  then root = keyed root merge of the two. All siblings must be consumed.

## 5. Costs (Apple M4, release, single thread)

| | |
|---|---|
| verify (64 x 64 or 256 x 256 job, localnet config) | about 95 us |
| one nonce, full noised GEMM, 64 x 64 x 1024 | 1.3 ms |
| one nonce, 256 x 256 x 1024 | 14.4 ms |

Work per verify: 12 chunks (about 192 compressions) + parents, about 2 + 2*k/8 + 4 +
8 = 270 single-block keyed hashes for noise (k=1024), 32 x 1024 MACs, 1 jackpot hash.

## 6. Cross-check against Pearl

`verify::verify_with_seed_mode` with `Legacy` / `Salted` reproduces Pearl's own
`verify_plain_proof`: 10 proofs mined by Pearl's `zk_pow::ffi::mine::mine`
(128 x 128 x 1024, r=32, rows [0,8,64,72], cols [0,1,8,9,32,33,40,41]) verified by
this crate in both modes, and harder targets rejected by both (checked 2026-10-16
with a scratch crate linking `research/pearlrepo/zk-pow`). Pearl's pinned seed vectors
are unit tests in `src/job.rs`.

## 7. v2 block proofs (SPEC v2 section 3, `src/pow.rs`)

On the v2 chain a block's work is one PSJ1 proof with **seed mode 3** (pow-bound).
The byte layout of section 3 is unchanged except byte 5 = 3. `nonce` (bytes 20..28)
must equal the header nonce. Mode 2 proofs are not block proofs and mode 3 proofs
are not v1 shares (each parser accepts exactly one mode).

### 7.1 Header (256 bytes, all integers little-endian)

```
off  size  field
0    4     version u32
4    8     height u64
12   32    prev_hash
44   8     timestamp_ms u64
52   4     target_bits u32   chain target (Bitcoin compact), the block's difficulty
56   32    job_id            zero = fallback block
88   32    miner             payout key
120  32    tx_root
152  32    state_root
184  32    withdraw_root
216  32    proof_hash        blake3(proof bytes)
248  8     nonce u64
```

```
block_hash = blake3(header)
pow_digest = blake3(header with bytes 216..248 zeroed)
proof_hash = blake3(proof bytes)
```

### 7.2 Seeds (replaces section 4 step 5 for block proofs)

```
hash_a', hash_b' as in section 4 step 5 (V3 salting with m, n)
b_seed = blake3(job_key || pow_digest || hash_b')     (96-byte input)
a_seed = blake3(b_seed || hash_a')
```
`pow_digest` commits to the parent, height, payout key, roots, timestamp, target and
nonce, so a proof cannot be moved to another header. Steps 6 to 9 are unchanged,
with `share_target` replaced by the header's `target_bits`.

### 7.3 Job blocks

`job_key` is the posted job's (section 1: `job_id || terms_hash || created_height u64
|| share_target u32`, then the config). `hash_a`, `hash_b`, `m`, `n`, config come from
the job named by `header.job_id`. The verifier checks `job_id` against that job.

### 7.4 Fallback blocks (`job_id` zero)

Everything is derived from `pow_digest`, with chain parameters `m`, `n` and a
config (default `FallbackParams`: m = n = 64, the localnet config of section 2):

```
job header76  = 0^32 || pow_digest || 0 u64 || 0 u32
job_key       = blake3(job header76 || config52)                   (as section 1)
A (m x k)     = int7(XOF(blake3_derive_key("pearl-sol 2026-10-16 fallback matrix A v1", pow_digest)), m*k bytes), row-major
B (k x n)     = int7(XOF(blake3_derive_key("pearl-sol 2026-10-16 fallback matrix B v1", pow_digest)), k*n bytes), row-major
int7(x)       = (x & 0x7f) - 64                                     (values in [-64, 63])
hash_a, hash_b = section 1 commitments of A and B^T under job_key
```
then sections 4 and 7.2 as for a job block. The verifier rebuilds A and B (128 KiB
of XOF and keyed hashing at the default size; fallback verify measured 0.55-0.7 ms vs about 0.1 ms for a job block, M4 under load) before the usual check.

### 7.5 Pinned vectors (`tests/pow.rs::pinned_vectors`)

- PowBound seeds, `job_key = 0x11*32`, `pow_digest = 0x22*32`, `hash_a = 0xAA*32`,
  `hash_b = 0xBB*32`, m = 192, n = 320:
  `b = e8f7d72728f2143cdb331ee04fd00792ebf1a07d9ef60367a8640ed8e78bbcac`,
  `a = 3eb3148589302d73b3e2e7dcd56f5cd4d7479187fe18fbfa761f0aea28c05fd2`.
- Fallback job for `pow_digest = 0x09*32`, default params:
  `hash_a = f3f1b82d6f82b46c16435bdb2cb5b97bcfb02f22ad30ca3f50bd3360b6e1596a`,
  `hash_b = 5ead9e6b5c2c9da12cf8dd666c7592afb7e7a64377e86741c2cec91fff84d347`.
- Header `version 1, height 5, prev 0xab*32, timestamp_ms 1760000000000,
  target 0x1e040000, job_id 0x07*32, miner 0x4d*32, tx_root 0x01*32,
  state_root 0x02*32, withdraw_root 0x03*32, proof_hash 0, nonce 0`:
  `pow_digest = de30bddaa1209116cffbf120dc3da81512c11e574fa098c1edb8cf3451bf3cee`.

### 7.6 API

`pow::BlockHeader` (`to_bytes`, `from_bytes`, `pow_digest`, `block_hash`),
`pow::proof_hash`, `PreparedJob::mine_block(pow_digest, target_bits, nonce)`,
`pow::mine_block_proof(a, b, job, pow_digest, target_bits, nonce)`,
`pow::verify_block_proof(job, proof, pow_digest, target_bits, nonce)`,
`pow::fallback_job(pow_digest, params)`, `pow::mine_header_nonce(template, work, nonce)`
and `pow::verify_block(header, job, proof_bytes, fallback_params)` (the full check a
node runs: proof_hash, mode, job binding, seed, target).
