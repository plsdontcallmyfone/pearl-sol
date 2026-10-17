# Rank penalty and low-rank job matrices (2026-10-17)

Owner decision 2026-10-17: port Pearl mainnet's rank-penalty rule so job blocks on
zero or low-rank matrices cannot be mined cheaply (SPEC section 8). This note is what
the rule is, what it does and does not protect against, whether it closes our hole,
and what was built. Pearl source: research/pearlrepo @ 837c69c.

## 1. The rule, as Pearl has it

Code (all paths in research/pearlrepo):

- `zk-pow/src/api/sanity_checks.rs:13`: `PENALTY_BASE_RANK = 128`, "minimum accepted
  noise rank and normalization point for the rank penalty".
- `sanity_checks.rs:166-182` `check_rank_penalty(config, hash_jackpot, nbits)`:
  1. `config.rank >= 128`, else reject;
  2. `factor = penalized_adjustment_factor(config)`, reject if 0;
  3. accept iff `U256::from_le(hash_jackpot) <= scale_target(nbits, factor)`.
- `sanity_checks.rs:196-200` `penalized_adjustment_factor = h*w * (dlen / r) * 128`,
  against the ordinary `difficulty_adjustment_factor = h*w * dlen` (`:192-194`, used by
  `extract_difficulty_bound`, `:233`). `dlen = k - k % r` (`zk-pow/src/v1/api/proof_utils.rs:383-388`).
- `sanity_checks.rs:211-223` `scale`: `target * factor`, saturating at U256::MAX
  (consensus path). `:243-251` `penalized_target_bound`: same product but `None` on
  overflow, for pools scaling a share target.
- Pearl's own tests: `sanity_checks.rs:252-345` (neutral at r = 128, bound divided by
  r/128, ranks below 128 and zero refused, inclusive bound, degenerate config).
- Activation: a softfork at mainnet height 96251 (`node/chaincfg/params.go:266-274,369`,
  `IsRankPenaltyForkActive` `:316-318`), applied in `CheckCertificateRules`
  (`node/blockchain/validate.go:508-555`, call at `:549-555`) through the cgo binding
  `node/zkpow/verify.go:190-215` -> `zk-pow/bindings/go/src/verify.rs:192-217`. It runs
  next to, not instead of, the unpenalized jackpot check inside proof verification.
  Pools apply it to shares too (verify.go:190-193 comment).
- Miners: the default config sits on the floor, `DEFAULT_RANK = MIN_NOISE_RANK`,
  `common_dim = 16 * rank` = 2048, 4 x 8 tile (`zk-pow/bindings/go/src/mine.rs:16-26`);
  `miner/miner-base/src/miner_base/settings.py:40-48` and the gateway
  (`miner/pearl-gateway/src/pearl_gateway/comm/dataclasses.py:214`) refuse r < 128.

In words: the noise rank r (a miner-chosen field of `MiningConfiguration`) must be at
least 128, and one jackpot attempt is credited `h*w*128` multiply-accumulates per rank
block (one transcript checkpoint) instead of `h*w*r`. At r = 128 nothing changes; at
r = 128 * 2^j the jackpot bound is 2^j times harder.

## 2. What it protects against (and what it does not)

The clone has no written rationale (no PIP, no doc comment beyond the above, shallow
history). What the code fixes is the **noise rank**, never the matrices: the rule reads
only `rank`, `k`, the tile patterns and the jackpot hash. In Pearl the miner chooses A
and B (and r), so a rule about A or B would be meaningless there; Pearl's defence against
zero or low-rank A, B is the cuPOW construction itself, not this rule.

My reading of why (an inference, not stated anywhere in the repo):
- The per-checkpoint hardness argument is about rank-r noise, so Pearl sets a floor on
  r below which it does not rely on it (r = 32 and 64 were allowed before the fork).
- Above 128 the credit is capped at 128 per checkpoint per tile element. That matches
  the noise alphabet: with A = 0 the noised entries are differences of two 6-bit
  uniforms, at most 127 distinct values (`NOISE_RANGE = 128`,
  `zk-pow/src/circuit/pearl_noise.rs:13-17`), so each length-r dot product needs at most
  about 127 distinct multiplications plus r additions. Capping the credit at the base
  rank makes a large r worthless to anyone who could exploit that, and costs honest
  miners nothing because they stay at r = 128.

What it does not do: it does not make zero or low-rank A, B more expensive. Our SPEC
section 8 hoped it would.

## 3. Our hole, measured

SPEC section 8 says: zero or low-rank job matrices make the noised product low rank, so
it can be computed far cheaper than a real multiply. That is the failure of cuPOW's
"Idea 2" (hash only the output of (A+E)(B+F): with A = B = 0 that is a rank-r product,
O(n^2 r) instead of O(n^3); research/pearl_blog.md "Idea 2"). Pearl's fix is **Idea 3**,
which our verifier already implements: the jackpot hashes the whole transcript, the h x w
running partial sums after **every** length-r block of k
(`crates/pearl-job/src/noise.rs` `jackpot_noised`, Pearl `chip/jackpot/helper.rs`). A
shortcut must reproduce every checkpoint, not just the final product.

Best attack on zero matrices I could build (crates/pearl-job/tests/rank_penalty.rs):
with A = B = 0 the noised rows are pure noise, `A'[i, t] = x_i[p_t] - x_i[q_t]` (x_i
uniform, `(p_t, q_t)` the sparse factor), so each block's increment is the bilinear form
`x_i^T U_l V_l^T z_j` with `U_l`, `V_l` signed incidence matrices of a random graph on r
vertices with r edges. Cycles in that graph are linear dependencies: factoring through a
spanning forest of the sparser side cuts the inner dimension from r to r - c (c = number
of cycles). The test runs this attack, checks it reproduces the real jackpot word for
word, and counts multiplies; it also computes the exact rank of every block's form over
GF(2^31 - 1), which bounds any attack that factors the per-block form.

| config | forest attack, multiplies vs honest | extra additions | rank bound |
|---|---|---|---|
| k 1024, r 32 (old localnet) | 0.788 | 0.18 | |
| **k 2048, r 128 (new default)** | **0.820** | 0.33 | 0.818 |
| k 4096, r 256 | 0.825 | 0.42 | |

So a zero-matrix job is about 1.22x cheaper to mine than a job with real data, not the
k/r = 16x of the feared shortcut. Rank-rho (nonzero) A or B adds about rho to each
block's inner dimension, so it lands between 0.82 and 1.0. The same 1.22x is open to every
Pearl mainnet miner today (they choose their own matrices and may choose zero); Pearl
accepts it.

### Does Pearl's rule close our hole?

- The rule does not touch A or B, so it does nothing specific to zero or low-rank job
  matrices, whether the poster or the miner chooses them.
- The hole as SPEC 8 describes it (cheap low-rank PoW) is not there: the per-rank-block
  transcript already closes it. What remains is a bounded constant, about 1.22x, the same
  as on Pearl mainnet.
- What the port does close: our localnet default was r = 32, below the floor Pearl relies
  on, and the poster chooses r for each job, so any r from 32 to 1024 was accepted at full
  credit. Both now follow Pearl exactly.

### Additional check: none at consensus, one optional policy

What is left, and whether it needs a rule:
- PoW soundness: no. 1.22x on the cost of one job block does not let anyone produce
  blocks meaningfully cheaper than the network, and fork choice uses the chain target.
- Economics: job blocks earn at most `alpha x P` in total per job (SPEC 3.4), so a
  self-dealer still loses `(1 - alpha) x P` per job. Mining his zero job is 1.22x cheaper,
  which moves the alpha-cap equilibrium only slightly. `max_blocks` and the cap bound it.
- Miner preference: at equal subsidy a miner would rather mine a zero-matrix job than a
  real one (1.22x) or a fallback block (fallback matrices come from `pow_digest`, so they
  are full rank). That is a mild bias towards useless jobs, paid for by their posters.

The verifier sees only h + w opened rows (4 + 8 by default). Any rank or entropy bound it
could check on them, such as "rows not all zero" or "the h x r block has full rank",
is beaten by rank-h or rank-1 matrices, which keep almost the whole 1.22x. An on-chain
check that actually removes it would have to certify about r - c (roughly 20 at r = 128)
dimensions per rank block, and the opened tile cannot do that. The options, from least to
most intrusive:
1. Accept parity with Pearl (recommended, and what is built).
2. Policy, not consensus: evaluators (who already fetch the job data, DA) refuse to
   accept, or attesters refuse to sign results for, jobs whose A or B fails a cheap
   randomized rank test (for example, rank over GF(p) of each r-column block of a random
   sketch, at least r/4). It costs O((m+n) k r) once per job, off chain. It stops useless
   jobs from completing, but not from being mined.
3. Credit job blocks at about 0.8 of fallback blocks (a fixed extra factor on the bound).
   This removes the edge for zero jobs but taxes every honest job by 20%. Not
   recommended.

## 4. What was built (commits 82e002f..)

- `crates/pearl-job`: `PENALTY_BASE_RANK`, r >= 128 in `check_shape` (so PostJob,
  AdminConfig fallback_config, `PreparedJob`, every verify), `difficulty_bound` =
  Pearl's penalized bound (saturating), `penalized_target_bound` (non-saturating),
  `unpenalized_difficulty_bound` (reference only). Localnet default config
  k = 2048, r = 128, 4 x 8 contiguous tile (Pearl's default shape, mine.rs:16-26, with
  contiguous rows and columns). `noise::noise_factors` exposed for the analysis.
  Tests: tests/rank_penalty.rs (Pearl's rule tests ported, old r = 32 refused at
  prepare, mine and verify, honest default mines and verifies, and the zero-matrix work
  ratio above). CLI defaults k = 2048, r = 128. PROOF-FORMAT sections 2, 4.9, 7.4, 7.5
  and the new 7.7.
- `crates/pearl-verify-sbf`: floor in `check_shape`, `MiningConfig::penalized_factor`,
  bound uses it; resumable `fallback::hash_plain` (proof hash), `noised_side_range`
  (noise in 8-column blocks), `JackpotState` + `jackpot_step` / `jackpot_finish`
  (jackpot in rank blocks). Crosscheck tests: penalty equals pearl-job on 14 configs x 3
  targets; split steps equal one-shot.
- `token/programs/lightclient`: k = 2048 no longer fits single steps (proof hash
  1.3M, noise B^T about 1.7M, jackpot about 1.1M CU), so `verify_proof_hash(count)`,
  `verify_noise(side, count)` and `verify_jackpot(count)` are resumable (state in the
  work account; the jackpot state reuses the A progress slot after hash_a is done).
  `client::plan(&Shape, budget)` sizes steps from the block's config and proof length.
  services/header-relay follows (job config from the JobCommit account).
- `chain/consensus-pow`: no source change (it calls `pearl_job::pow::verify_block`).
  Tests: k = 2048 PostJob fixture, r = 32 fallback_config refused by AdminConfig.
- services/miner, services/attester test fixtures: k = 2048.

CU (LiteSVM, measured): job block verify 7.25M CU in 34 txs incl. 26 proof writes
(was about 3.5M / 19 txs); 4 x 8 fallback 10.2M / 37 txs (was 5.1M / 20); 64 x 64
fallback 37.9M / 60 (was 18.9M / 32). Largest tx 1.23M CU. The 2x is k doubling
(k >= 16r with r >= 128), not the penalty itself.
