# pearl-sol spec

Status: v1, 2026-10-16. Sources: research/NOTES-proof.md, research/NOTES-pump.md,
research/NOTES-chain-bridge.md.

## 1. Shape

Two chains run side by side.

| | Token chain | Agent chain |
|---|---|---|
| What | Solana mainnet | Our own SVM cluster (Agave fork, own genesis) |
| Token | pump.fun Token-2022 mint, fixed 1B, 6 dp | Native lamports = the bridged token, 9 dp |
| Holds | Mining reserve, bridge vault | AI job market, mining, agent identity |
| Localnet | solana-test-validator :10600 with pump cloned | solana-test-validator :10630 |

Supply rule: 1 SPL base unit = 1000 lamports. At genesis the agent chain's entire
supply (1B x 1e9 = 1e18 lamports, fits u64) sits in the bridge escrow. Bridging in
releases lamports from the escrow and bridging out locks them back. Nothing is
minted on either chain. Genesis sets `--inflation none` and
`--fee-burn-percentage 0`, so lamport supply never changes.

Invariant (checked by the relayer and the indexer):
`vault SPL on Solana x 1000 == 1e18 - escrow lamports on agent chain`.

## 2. Token chain (Solana)

### 2.1 Launch
- `create_v2` + dev buy in one transaction. `creator` = the `reserve` program's
  data-less, system-owned PDA `["creator"]`, so creator fees accrue to the program.
- Dev buy size is the owner's call. Reference: 10% of supply is about 3.1 SOL,
  20% about 7 SOL. Tokens go to the reserve ATA.

### 2.2 `reserve` program (Anchor)
- `crank_buyback(max_sol_in, min_tokens_out)`, permissionless:
  pump `sweep_creator_fee` -> `collect_creator_fee_v2` (curve) or
  `collect_coin_creator_fee` (PumpSwap, wSOL) -> `buy_exact_quote_in_v3` (curve)
  or PumpSwap `buy_exact_quote_in_v2` -> tokens into reserve ATA.
  - Per-call SOL cap (`config.max_buy_per_call`) and a minimum interval between
    calls, both admin-editable.
  - `min_tokens_out` is enforced on chain, and the program also rejects the
    buy if the realised price is worse than `config.max_slippage_bps` from the
    pre-trade quote it reads from the curve or pool itself.
- `fund_mining(amount)`, admin or scheduled: moves reserve tokens into the
  bridge vault with the agent-chain `mining_pool` PDA as recipient. This is the
  only way reserve tokens reach miners.
- Risk to state in docs: pump's admin can reassign a coin's creator (community
  takeover), which would redirect future fees. Tokens already in the reserve are
  unaffected.

### 2.3 `bridge` program, Solana side (Anchor)
- `deposit(amount, agent_recipient: Pubkey)`: transfers SPL into the vault ATA,
  writes a `Deposit { nonce, sender, recipient, amount }` account (nonce from a
  global counter), emits an event.
- `release(msg, sigs)`: pays SPL out of the vault for a `Withdrawal` that happened
  on the agent chain. Needs M-of-N ed25519 signatures from `config.signers`
  (verified through the ed25519 precompile instruction in the same transaction).
  A `Processed` PDA per `(source_chain, nonce)` blocks replay.
- Signer set and threshold are admin-editable. M1: 1-of-1. Later 2-of-3.
  Production: self-hosted Hyperlane Sealevel (to confirm: the route type and
  the 6-to-9 decimal scaling).

## 3. Agent chain (own SVM)

### 3.1 `escrow` program (agent-side bridge)
Mirror of 2.3 with lamports:
- `unlock(msg, sigs)`: moves `amount x 1000` lamports from the escrow PDA to the
  recipient for a Solana `Deposit`. Same M-of-N + replay PDA pattern.
- `lock(amount, solana_recipient)`: user sends lamports into the escrow PDA,
  writes `Withdrawal { nonce, ... }`. Amount must be a multiple of 1000.

Localnet: solana-test-validator cannot write custom genesis, so the escrow PDA
is preloaded with `--account` (1e18 lamports) and the faucet balance is
ignored by the invariant check. Production uses `solana-genesis` from our fork
with `--primordial-accounts-file`.

### 3.2 `agents` program (identity)
- `register(name, uri, payout)`: an agent's profile PDA `["agent", authority]`.
- M2: replace with or bridge to the Solana Agent Registry (ERC-8004 port).

### 3.3 `jobs` program (AI job market, ERC-8183 style)
A job is a matrix multiply AB that an agent pays for.

`post_job(params)`:
- `hash_a`, `hash_b`: Pearl-format keyed BLAKE3 commitments to the int7 matrices.
- `config` (52 bytes, Pearl mining config: k, rank, row/column patterns), `m`,
  `n`, `k`, data URI (where miners fetch A and B).
- `worker_pay` W (lamports, escrowed in the job PDA) and `protocol_fee` P
  (moved to the `mining_pool` PDA at post time, non-refundable).
- `deadline`, `share_target` (difficulty for shares on this job).
- The job's 76-byte Pearl header is fixed at post time:
  `job_pubkey (32) || terms_hash (32) || created_slot (8) || target_bits (4)`.
  `job_key = blake3(header || config)` is therefore computable by anyone.

Lifecycle: `Open -> Delivered -> Completed | Disputed`, or `Open -> Expired`.
- `submit_result(result_hash, result_uri)` by the miner who will deliver AB.
- Evaluator releases W to the deliverer. M1 evaluator = the attester service
  (runs a Freivalds check off chain on the delivered AB and signs). Poster can also
  accept directly. If nobody delivers by `deadline`, W returns to the poster.

### 3.4 `mining` program (useful PoW)
- `submit_share(job, miner, proof)`: a share is a Pearl tile proof on the job's
  committed matrices at `share_target`. Seed derivation is the miner-bound variant
  (NOTES-proof Q4): `b_noise_seed = blake3(job_key || miner_pubkey || nonce || hash_b')`,
  so a proof cannot be stolen and resubmitted by someone else.
- Nonce range is capped per job (`config.max_nonce`), since repeated passes over
  the same job redo the multiply but are not new useful work.
- Accepted shares increment `Shares { job, miner, count }`.

Verification path by milestone:
- **M1, attested:** the attester service runs Pearl's plain (non-ZK) verifier
  natively (about 0.36 ms) and signs `(job, miner, nonce, tile)`. `submit_share`
  checks the attester signature through the ed25519 precompile. 1-of-1 attester.
- **M2, on chain:** port the plain verifier (no ZK, about 13 KB proof at the
  smallest tile, about 1.3-1.9M CU) to SBF. Proof uploaded to a buffer account,
  verify split across 2 instructions with state in the buffer.
- **M3, native:** a BLAKE3-compression syscall in our Agave fork (estimate
  300-400k CU per verify), or the whole plain verify as a builtin. Our own chain,
  so no feature gate is needed. Groth16 wrap only if a proof must ever be checked
  on Solana mainnet.

### 3.5 Rewards and emission
The pool is finite (fed only by `fund_mining` and protocol fees), so emission is a
share of what is there, not a fixed schedule.

- Epoch = `config.epoch_slots` (default 9000 slots, about 1 hour).
- Epoch budget `E = pool_balance x config.emission_bps / 10_000`
  (default 42 bps per epoch, about 1% per day, about 63% emitted in 100 days of
  a fixed pool; fees and buybacks keep topping it up).
- Per job, the subsidy cap is `S_job = min(E_share, alpha x P)` with
  `alpha = config.alpha_bps / 10_000 < 1` (default 8000). This is the anti
  self-dealing rule: posting a fake job and mining it yourself returns W and at
  most `alpha x P`, so it always loses `(1 - alpha) x P`.
- `E_share`: E is split across jobs active this epoch in proportion to their P.
  Within a job, the subsidy is split across miners by share count.
- `claim(epoch)`: miner pulls their lamports after the epoch closes.
- All of `epoch_slots`, `emission_bps`, `alpha_bps`, `max_nonce`, caps and
  signer sets are admin-editable on chain.

### 3.6 x402 and agent payments
Agents pay each other in native lamports with plain transfers. For x402 (an
SPL-only scheme on Solana), a wrapped-native mint like wSOL. M2.

## 4. Off-chain services (all on ports 10660-10669)

| Service | Lang | Job |
|---|---|---|
| relayer | TS | Watches Deposit/Withdrawal on both chains, signs, submits unlock/release, checks the supply invariant, refuses to sign if it breaks |
| attester | Rust | Links Pearl's zk-pow plain verifier (forked for the miner-bound seed), signs shares and Freivalds results |
| miner | Rust | CPU miner: Pearl `try_mine_one` changed to take the job's A and B and the miner-bound seed. Localnet difficulty about 1 s per share |
| crank | TS | Calls `crank_buyback` and `fund_mining` on a schedule |
| agent-cli | TS | Register an agent, post a job (generates A, B, commits hashes, uploads data), watch it, accept results |

## 5. Milestones

**M1: localnet end to end (attested verify)**
1. `scripts/localnet.sh` brings up token :10600 with pump programs and accounts
   cloned (from NOTES-pump), agent :10630 with the escrow preloaded.
2. Programs: `reserve`, `bridge` (token side), `escrow`, `agents`, `jobs`,
   `mining` (agent side), Anchor 0.31.1.
3. Services: relayer, attester, miner, crank, agent-cli.
4. Demo script `scripts/demo.sh`, which must pass:
   launch coin with reserve PDA as creator + dev buy -> trades generate creator
   fees -> crank buys back into the reserve -> `fund_mining` bridges to the pool
   -> an agent bridges tokens in, registers, posts a job -> miner mines shares,
   attester signs, shares land on chain -> epoch closes, miner claims lamports
   -> miner delivers AB, attester Freivalds-checks, W released -> miner bridges
   lamports back out and receives SPL on the token chain -> invariant holds
   at every step.

**M2:** on-chain plain verify (split instruction), agent registry (ERC-8004),
x402 wrapped native, 2-of-3 signers, indexer + simple web UI (ports 10670+).

**M3:** Agave fork with the BLAKE3 syscall or a verify builtin, real
multi-validator cluster from our genesis, Hyperlane Sealevel bridge, devnet
then mainnet.

## 6. Signed message formats (ed25519, M1)

All little-endian. Signers sign these exact bytes; programs rebuild them and
check them against the ed25519 precompile instruction in the same transaction.

- Deposit (Solana -> agent): `b"pearl-sol:deposit:v1" || nonce u64 || recipient [32] || amount_spl u64`
- Withdrawal (agent -> Solana): `b"pearl-sol:withdraw:v1" || nonce u64 || recipient [32] || amount_spl u64`
- Share: `b"pearl-sol:share:v1" || job [32] || miner [32] || nonce u64`
- Result: `b"pearl-sol:result:v1" || job [32] || deliverer [32] || result_hash [32]`

## 7. Open questions for the owner
- Dev buy size (sets the starting mining reserve).
- Defaults for `emission_bps` and `alpha_bps`.
- Name and ticker.
