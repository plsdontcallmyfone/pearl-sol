# NOTES-proof: Pearl cuPOW proofs and verifying them on our SVM chain

Source: `research/pearlrepo` (pearl-research-labs/pearl @ 837c69c). Paths below are
relative to `research/pearlrepo/` unless absolute. Measurements: Apple M4 (10 cores),
macOS arm64, Rust 1.94.1, release build, done 2026-10-16 in the session scratchpad
(no ports, no validators). The measurement crate is a throwaway in the scratchpad, not
in this repo.

## TL;DR

- A Pearl proof says: "for committed int7 matrices A and B (BLAKE3 keyed roots), the
  noised h x w tile (A+E)(B+F) at public offsets hashes under the target". Noise E, F
  is low rank and derived from the commitments, so it cannot be chosen.
- The ZK certificate is a 3-layer STARK + plonky2 recursion, **59,560 bytes** proof +
  164 bytes public data. CPU verify measured at **~10-15 ms single thread**
  (6-37 ms with rayon). That is roughly 100x to 1000x over a 1.4M CU SBF budget. It
  does not fit in an SBF program, even split over several transactions.
- **The best fit for us is to skip ZK and verify Pearl's *plain proof* on chain.**
  Pools already do this with `verify_plain_proof`. It is the same statement, without
  the zero-knowledge. In the minimum config it is **12,977 bytes** and **0.36 ms** native
  single thread, about 1.3-1.9M CU in SBF by my bottom-up estimate. So: 2 instructions,
  a raised CU cap in our fork, or a BLAKE3-compress syscall. Job matrices are already
  public to miners, so ZK privacy buys us nothing.
- **Binding to job-committed matrices needs no circuit change.** Fix `job_key` from the
  on-chain job, so the poster can commit Pearl-format `hash_a`/`hash_b`. Then put the
  miner pubkey (and an optional nonce) into the *native* noise-seed derivation. Pearl
  did exactly this kind of change in its V3 "salted seed" fork, with circuits left
  untouched.

---

## 1. What a mining proof is

Statement (layer-0 STARK constraints, `zk-pow/src/circuit/pearl_circuit.rs:1-22`):
a. matrix slices are int7, b. the slices Merkle-hash to HASH_A / HASH_B under key
JOB_KEY, c. the noised tile matmul reduces to JACKPOT_MSG, d.
`Blake3(JACKPOT_MSG, key=COMMITMENT_HASH) = HASH_JACKPOT`. PIP-2 states the v1
protocol in one line: commit two matrices, derive low-rank noise from the commitments
and chain state, run a noised tiled matmul, open a winning tile, prove it
(`research/pip2.md`, Abstract).

**Inputs**
- Header `IncompleteBlockHeader` = version, prev_block, merkle_root, timestamp, nbits
  (76 bytes LE) (`zk-pow/src/api/proof.rs:10-16`; size in
  `zk-pow/src/api/proof_utils.rs:506-526`).
- `MiningConfiguration` (52 bytes): `common_dim` k, `rank` r, `mma_type` (only
  Int7xInt7ToInt32), `rows_pattern` / `cols_pattern` (periodic index sets that define
  the h rows and w columns of one hashed tile), and an MoE trailer
  (`proof.rs:61-72`).
- Matrices: A is m x k and B is k x n, values int7 in [-64, 64]
  (`zk-pow/src/api/verify.rs:113-118`). A is hashed row-major and B as B^T row-major,
  each zero-padded to a 1024-byte BLAKE3 chunk boundary
  (`zk-pow/src/ffi/mine.rs:42-48`, `pearl-blake3/src/merkle.rs:15-27`).

**What is hashed (the seed chain)**
- `job_key = blake3(header76 || config52)` (`proof_utils.rs:348-353`).
- `hash_a = BLAKE3_keyed(pad1024(A), key=job_key)`, `hash_b = BLAKE3_keyed(pad1024(B^T),
  key=job_key)`. This is standard BLAKE3 keyed hashing: the Merkle root equals
  `blake3_digest(padded, key)` (`zk-pow/src/ffi/mine.rs:556-570`). Any BLAKE3 library
  in keyed mode reproduces it.
- V3 (Salted, live on mainnet since height 99000) first salts the roots:
  `bind_root_a = blake3(hash_a || m_le || 0^28, key=SEED_SALT_A)`, and likewise for B
  with n (`zk-pow/src/api/seed.rs:41-59`).
- `b_noise_seed = blake3(job_key || hash_b')`,
  `a_noise_seed = blake3(b_noise_seed || hash_a')` (`proof_utils.rs:362-372`).
- Noise: `E_A = E_AL * E_AR`, where E_AL is uniform (h x r, from blake3 of the seed)
  and E_AR is a sparse +1/-1 permutation (k x r). E_B is built the same way
  (`zk-pow/src/circuit/pearl_noise.rs:46-160`). The rank r must be a power of two in
  32..1024, and since the rank-penalty fork it must be >= 128.
- Tile: for each length-r block of k, accumulate the h x w partial inner products of
  (A+E_A)(B+E_B), XOR-fold the tile into one of 16 u32 words with a rotation
  (`zk-pow/src/circuit/chip/jackpot/helper.rs:9-35`). Then
  `hash_jackpot = blake3(jackpot_msg64, key=a_noise_seed)` (`proof_utils.rs:1510-1514`).

**Difficulty target**
- Bitcoin-compact `nbits` is turned into a target, then multiplied by
  `h*w*dot_product_length`, saturating at U256::MAX
  (`zk-pow/src/api/sanity_checks.rs:184-235`). Test:
  `U256::from_le(hash_jackpot) <= bound` (`sanity_checks.rs:150-159`).
- Rank penalty (mainnet since height 96251): the factor is
  `h*w*(dlen/r)*128` and r must be >= 128 (`sanity_checks.rs:161-200`).
- The scaling means expected work per winning ticket is about `2^256 / target` MACs,
  whatever the tile shape. One ticket costs h*w*k MACs and succeeds with probability
  target*h*w*k/2^256.
- Parameter limits: k in [1024, 65536] and a multiple of 64; 16r <= k <= 4r^2;
  32 <= h*w <= 256; h and w multiples of TILE_H=2; m, n <= 2^24
  (`sanity_checks.rs:15-60`, `zk-pow/src/circuit/pearl_program.rs:21-22`).

**How the useful work relates.** The miner runs the real GEMM on noised operands,
(A+E)(B+F). Because E and F are low rank, the useful product AB is recovered cheaply
("denoising" in the CUDA kernels, `miner/pearl-gemm`). Every h x w tile of the noised
product is one lottery ticket, so tickets are proportional to matmul work done. The
proof opens **only the winning tile**: it does not prove the whole product was
computed, and it does not prove AB was delivered to anyone.

**What the miner submits.** The miner sends a `PlainProof` to the gateway: m, n, k,
noise_rank, the h rows of A and the w rows of B^T, plus multi-leaf BLAKE3 Merkle proofs
(`zk-pow/src/ffi/plain_proof.rs:20-84`). The gateway turns it into a ZK proof
(`miner/pearl-gateway/src/pearl_gateway/proof_generator.py`). The block carries
`ZKCertificate | header | txs` (`py-pearl-mining/README.md`, "Wire Format").

Public data on the wire (164 bytes, non-MoE): `mining_config(52) | hash_a | hash_b |
hash_jackpot | m | n | t_rows | t_cols` (`proof.rs:76-105`). Here t_rows and t_cols are
the tile's offsets. Production miner defaults: rank 128, rows_pattern [0,8] (h=2), a
64-entry cols_pattern (w=64), so h*w=128 (`miner/miner-base/src/miner_base/settings.py:10-26`).

## 2. How the node verifies

Path:
1. `blockchain/validate.go:333`: `checkProofOfWork` calls `zkpow.VerifyCertificate`.
   `validate.go:530-557` checks the certificate version for the height and calls
   `CheckRankPenalty`.
2. `node/zkpow/verify.go:41-160`: checks that the cert hash matches the header hash and
   that `header.ProofCommitment` matches the certificate, then calls Rust over cgo:
   `verify_zk_proof_v2/v3`.
3. `zk-pow/bindings/go/src/verify.rs:26-90`: size checks
   (`MAX_ZK_PROOF_SIZE = 60000`, `bindings/go/src/common.rs:25`), deserialize, then
   `verify_block_cached_circuits_only` against an embedded verifier circuit cache
   (`zk-pow/src/circuit/embedded_cache.rs`; the cache file `v2_cache.bin` is not in the
   clone and is built by the `build_cache` bin).
4. `zk-pow/src/api/verify.rs:48-94` (`prepare_verification`): public-param sanity
   checks, `hash_jackpot <= bound`, then **natively** recompute `job_key` and
   `commitment_hash` and regenerate the STARK's preprocessed columns. Those are CONTROL,
   NOISE, CV_OR_TWEAK, AB_ID and OUTER_INDICES, 5 columns of 2^degree_bits rows
   (`zk-pow/src/circuit/pearl_stark.rs:228-285`).
5. `pearl_circuit.rs:609-681`: evaluate the preprocessed columns at zeta and g*zeta,
   build the layer-1 public inputs, and run `CompactProofWithPublicInputs::verify`. That
   function also evaluates every constants/sigmas polynomial at zeta and at each FRI
   query point, because the compact format omits those Merkle openings
   (`plonky2/plonky2/src/plonk/proof.rs:447-588`).

What is proven, layer by layer (`pearl_circuit.rs:1-77`):
- Layer 0 is a Starky STARK over Goldilocks with BLAKE3 + matmul + jackpot chips
  (`zk-pow/src/circuit/pearl_air.rs:1-37`).
- Layer 1 is a plonky2 circuit that verifies the STARK.
- Layer 2 is a plonky2 circuit with ZK on, verifying layer 1.
- Configs: inner is PoseidonGoldilocks; the outer (final) proof is
  **Blake3GoldilocksConfig** (`pearl_circuit.rs:241-244`). 120 security bits. The final
  stage has rate_bits 7 and pow_bits 22, so 14 FRI queries
  (`pearl_circuit.rs:119-133`, `circuit_utils.rs:117-141`).

**Proof size:** 59,560 bytes (measured; the fixtures in `zk-pow/fixures/*.bin` are
59,724-59,795 bytes). Public data is 164 bytes. The intermediate layer-1 proof is
171,078 bytes and is not shipped.

**Measured** (my scratchpad crate links `zk_pow` directly; the `prove_verify bench`
bin gave matching numbers):

| config (m x n x k, r, h x w) | STARK rows | ZK verify, 1 thread | ZK verify, rayon | preprocessed regen | plain proof bytes | plain verify, 1 thread | CPU prove (incl. circuit build) |
|---|---|---|---|---|---|---|---|
| 64x64x1024, r32, 4x8 (minimum) | 2^13 | 9.8-11.9 ms | 6.2-6.9 ms | 0.5-0.8 ms | 12,977 | 0.36 ms | 13.5 s |
| 512x512x2048, r128, 2x64 (prod tile) | 2^15 | 13.3-14.7 ms | 11-37 ms (noisy) | 1.9-2.3 ms | 140,417 | 1.75 ms | 29 s (122 s single thread) |
| 512x512x8192, r128, 2x64 | 2^17 | n/a | 22-28 ms | 8-9 ms | 552,257 | 10.5 ms (rayon) | 52 s |
| bench bin, k=3584, 64x2 | 2^15 | n/a | 14 ms | n/a | n/a | n/a | 19.4 s (cache warm) |

Verify time grows with STARK size, because the native preprocessed regeneration and
evaluation are O(rows). The plonky2 part is roughly constant at about 9 ms single thread.

## 3. Can it run in an SBF program within 1.4M CU?

**ZK proof in SBF: no.** About 10 ms of single-thread native arm64 work, using
Goldilocks 64x64->128 multiplies and SIMD BLAKE3. Order-of-magnitude estimate: 10^8 to
10^9 CU, which is 100x to 1000x the 1.4M budget. Other blockers:
- The proof alone is 59.5 KB, against a 32 KB default heap and 256 KB maximum.
- The verifier needs the layer-2 constants/sigmas polynomial coefficients and must
  regenerate 5 x 2^13..2^19 preprocessed columns.
- Neither Poseidon nor Goldilocks has a syscall.

Options:

**(d) Recommended first: verify the plain proof (no ZK) in SBF.** This is the same
statement, checked directly (`zk-pow/src/api/verify.rs:96-134`; pools use it for
shares, with `nbits_override` as the share target). The work is: Merkle-verify h rows
of A and w rows of B^T, regenerate noise for those rows, run an h*w*k matmul, then one
jackpot hash.

Bottom-up estimate for the minimum config (h=4, w=8, k=1024, r=32):
- About 190 BLAKE3 compressions for 12 KB of row data, plus about 60 Merkle parents.
- About 270 single-block keyed hashes for noise: E_AL 4, E_AR 128, E_BL 128, E_BR 8
  (`pearl_noise.rs:141-150`).
- In total about 520 compressions at roughly 2-3k CU each in scalar SBF, which is
  1.0-1.6M CU.
- Plus about 32k MACs at roughly 5 CU, about 160k CU, and noise matvec at about 100k CU.
- **Total about 1.3-1.9M CU.**

Ways to fit it:
- Split it into 2 instructions with state in a PDA (Merkle checks, then noise + matmul
  + jackpot).
- Raise `MAX_COMPUTE_UNIT_LIMIT` in our Agave fork.
- Add a `blake3_compress` syscall, which would cut the cost to roughly 300-400k CU.
  Agave's existing `sol_blake3` syscall is unkeyed whole-message hashing only, so it
  cannot do keyed chunk/parent CVs directly; this needs checking against our Agave
  version.

The data (13 KB) is over the 1232-byte transaction limit, so upload it to a buffer
account first (about 12 write transactions, or fewer with a larger tile account). The
production tile shape (2x64, 140 KB per proof at k=2048) is about 10x more data and
CU, so for on-chain plain verification pick a small tile: h*w=32, small h+w, k=1024.
Since difficulty is priced in MACs (section 1), tile shape does not change fairness.

**(a) Groth16 wrap, verified with alt_bn128 syscalls.** On-chain cost is about
200-300k CU (groth16-solana style: 4 pairings plus a few G1 MSMs for the public inputs).
This is cheap on chain and expensive to build:
- The final Pearl layer uses BLAKE3 Merkle hashing (`pearl_circuit.rs:244`), which
  costs too much inside a BN254 circuit. We would need a new layer 3: plonky2 verifies
  layer 2 and outputs with PoseidonBN128. Then a gnark plonky2-verifier circuit with
  Groth16 on top (the Succinct gnark-plonky2-verifier pattern).
- The native preprocessed evaluation (noise depends on the commitment hash) must move
  inside layer 3, or the on-chain program still has O(rows) work.
- The compact-proof constants/sigmas trick must be undone.
- It needs a circuit-specific trusted setup.
- My estimate (not measured): weeks of circuit work, plus tens of seconds to minutes of
  extra CPU proving per proof and tens of GB of RAM for the gnark step.

**(b) Optimistic acceptance + fraud proof.**
- The miner posts public data (164 bytes) and the plain proof to an account (or its
  hash, with the data elsewhere), plus a bond.
- Anyone verifies it off chain in under 2 ms. Challenges must be one-transaction
  checks, which works because the plain-proof statement decomposes: one Merkle path,
  one noise row, one rank-block partial sum, or the final jackpot hash. Each is about
  10-300k CU.
- Happy path: about 10-20k CU plus a challenge window (for example 150-1000 slots,
  about 1-7 minutes).
- Fraud proofs over the ZK proof itself are not practical; this needs the plain proof
  to be available.
- Cost: payout latency, plus bond and watcher infrastructure.

**(c) Native builtin or syscall in our Agave fork.**
- Link `zk_pow::api::verify` (ZK) or `verify_plain_proof` into a syscall or builtin
  that reads the proof from account data.
- Precompile-style verification (like ed25519) does not work, because the data does not
  fit in a transaction.
- Cost is wall time. ZK is about 10-15 ms per verify; with a 400 ms slot and a few
  banking threads, that is a few dozen per block at most. Plain verify is 0.36-1.75 ms.
  If 1.4M CU is about 2-4 ms of SBF (my rough calibration), price ZK at about 4-7M CU
  and plain verify at about 0.2-1M CU.
- Notes: it must be deterministic (turn rayon off, which also matches my single-thread
  numbers), and the verifier circuit cache must be built and embedded. Every validator
  runs our fork anyway.

**Ranking for our chain:**
- M1 (localnet): (d) as an SBF program, or (c) with plain verify. Both are simple.
- Later: (c) plain or ZK as a builtin.
- (a) only if proofs must ever be verified on Solana mainnet.
- (b) only if (c) or (d) prove too costly.

## 4. Binding a proof to externally committed matrices (agent job)

The pieces already in Pearl:
- `hash_a` / `hash_b` are public inputs and are constrained in-circuit as the Merkle
  roots of the opened rows, keyed by the public input JOB_KEY
  (`zk-pow/src/circuit/chip/blake3/constraints.rs:78-87`, `pearl_air.rs:34-37`).
- `job_key`, `commitment_hash` and the noise are computed **natively by the verifier**
  from public params (`verify.rs:56-92`, `proof_utils.rs:670-700`).

The obstacle is that `job_key` hashes the whole header, so in Pearl the matrix roots
change per block and per miner. A poster cannot commit `hash(A,B)` ahead of time if
the key depends on the miner.

The change, with no circuit change:
1. **Job-fixed job_key.** Build the 76-byte "header" from the on-chain job:
   - prev_block = job account pubkey
   - merkle_root = blake3(job terms)
   - timestamp = job creation time
   - nbits = job difficulty
   - version = protocol tag

   The config is fixed by the job (k, r=128, patterns). Then
   `job_key = blake3(header || config)` is known at post time. The agent computes
   `hash_a = BLAKE3_keyed(pad1024(A), job_key)` and the same for B^T, and stores both
   in the job account. The verifier program recomputes `job_key` from the job account,
   so the proof is forced to open rows of exactly those matrices.
2. **Miner binding in the native seed derivation.** Add a `SeedDerivation` variant
   (next to Legacy and Salted, `seed.rs:11-26`):
   `b_noise_seed = blake3(job_key || miner_pubkey || nonce_le || hash_b')`. The chain
   then continues unchanged. The program takes miner_pubkey from the transaction signer.
   A copied proof under another key gives a different noise seed and jackpot key, so it
   fails.
   - This is a native-only change, like Pearl's V3 fork, which changed seed derivation
     and kept "the ZK circuits, wire formats, and share formats unchanged"
     (`docs/salted-seed-fork-upgrade-guide.md:14-16`).
   - With the plain-proof route (option d) there is no circuit at all; only the
     verifier code changes.
3. **Grinding policy.** With fixed A and B, each (job, miner, nonce) gives exactly
   (#row groups x #col groups) tickets. Re-grinding a new nonce redoes the full noised
   GEMM on the same data, which is no longer useful. So either:
   - allow no nonce (one ticket set per miner per job), or
   - use a small capped nonce, and set the job difficulty so the expected number of wins
     per full pass is about 1.

   Better still, pay by **shares**: tickets under a lower share target (Pearl's
   `nbits_override`, `verify.rs:96-106`). Expected shares are proportional to tiles
   computed, which is a statistical proof of how much of the job was done.
4. **Not covered by Pearl's proof:** delivery and correctness of C = AB to the agent.
   That needs a result commitment and a separate check: Freivalds O(n^2) by the agent,
   or an optimistic dispute.
5. Alternative, which does need circuit changes: add a separate MATRIX_KEY public input
   so the Merkle key differs from `job_key`. This is not needed if step 1 is done.

Note: giving the poster the choice of A and B adds nothing beyond what Pearl already
allows. Pearl miners choose their matrices freely, and the reference CPU miner uses
random ones. The noise is still unpredictable until A and B are committed, because it
is seeded from their roots.

## 5. CPU mining path (no GPU) for a localnet demo

Yes, all in Rust:
- `zk-pow/src/ffi/mine.rs:19-129` `try_mine_one` and `:320-349` `mine`: random int7
  A and B, full noise, a naive triple loop per tile, and returns a `PlainProof`.
  `mine_moe` is the MoE variant.
- Exposed in Python as `pearl_mining.mine` (`py-pearl-mining/README.md`, "naive ...
  for completeness and testing").
- Exposed to Go via `zk-pow/bindings/go/src/mine.rs`. The node's regtest/simnet
  generate path is `node/blockchain/solve.go:40` calling `node/zkpow/miner.go:60-71`
  (DefaultM 256, DefaultN 512, DefaultNBits 0x1E01FFFF, `miner.go:26-28`).
- For job-bound mining, adapt `try_mine_one` to take a given A and B instead of
  generating them (lines 33-43). The rest is reusable as is.
- Measured: one attempt (noise + hashing; the first tile wins at saturated difficulty)
  takes 4-5 ms at 64x64x1024 and 38-46 ms at 512x512x2048. ZK proving on CPU takes
  13-52 s (table above). For a demo we skip ZK if we use plain verify.
- Demo difficulty: nbits 0x207fffff saturates the bound, so every tile wins
  (`sanity_checks.rs:212-221`). For "about 2^31 MACs per win" (about 1 s of naive CPU)
  use about 0x1d020000, which is target 2^225. Regtest PowLimitBits is 0x1e010000,
  which is 2^232, or about 2^24 MACs.
- Build: `cargo build --release --bin prove_verify` in `zk-pow`, about 3 minutes cold
  on the M4. `RUST_LOG=info ./prove_verify correctness | bench | invalid`
  (`zk-pow/src/bin/prove_verify.rs:283-309`).

## 6. Emission and difficulty (reference)

- Supply cap: 2.1B PRL at 1e8 grains each (`node/blockchain/validate.go:44`,
  `node/btcutil/const.go:12`).
- Subsidy: `S(h) = Total*C / ((h+C)(h-1+C))` with `C = 4y / T` = 650,226 blocks
  (`validate.go:160-190`, `:49`).
  - Cumulative emission is `Total * H/(H+C)`, so half is emitted at 4 years and 3/4 at
    12 years. There are no halvings.
  - The first block pays about 3,229.6 PRL.
- Block time T = 3 min 14 s. Coinbase maturity is 100 (`node/chaincfg/params.go:354-358`).
- Difficulty is WTEMA, retargeted every block:
  `new = old + (t - T)*old/half_life`, with half_life = 168 h (1 week), capped at
  PowLimit (`node/blockchain/difficulty.go:73-140`, `params.go:358`).
- Mainnet PowLimit is 2^208 - 1 (PowLimitBits 0x1b00ffff). Regtest is 2^233 - 1
  (PowLimitBits 0x1e010000) (`params.go:27-41`, `:355`, `:475`).
- Forks (mainnet heights): MoE 71935, DenseOnly 91630, RankPenalty 96251,
  SaltedSeed 99000 (`params.go:364-371`).
- PIP-3 (draft) proposes floating-point PoUW certificates (`research/pip3.md`). It is
  not in the code at 837c69c.
