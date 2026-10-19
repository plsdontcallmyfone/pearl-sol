# pearl-sol spec v2

Status: v2, 2026-10-16. Replaces v1 (SVM agent chain; see git history and branch
wip/svm-agent-chain). Sources: research/NOTES-proof.md, NOTES-pump.md,
NOTES-chain-bridge.md, NOTES-commonware.md.

## 1. Shape

| | Token chain | Agent chain ("the chain") |
|---|---|---|
| What | Solana | Our own chain: Rust node on Commonware primitives, not a fork |
| Blocks | Solana | Useful proof of work: miners do Pearl matmul on posted AI jobs |
| Token | pump.fun Token-2022 mint, fixed 1B, 6 dp | Native coin = the bridged token, 9 dp |
| Role | Money, buyers, final record | Work |

Solana's four jobs:
1. Liquidity and distribution (pump.fun curve, then PumpSwap).
2. Funding: creator fees buy the token back into the mining reserve (built: token/programs/reserve).
3. Compute storefront (M1): any Solana app or agent buys AI jobs in SOL or USDC; the payment becomes token buy pressure and job pay on the chain; the result commitment lands back on Solana.
4. Light client (M2): a Solana program follows our PoW headers and verifies the work, so withdrawals and job results are proved, not signed by a committee.

Supply rule: 1 SPL base unit = 1000 base units on the chain. Genesis allocates the
whole supply (1e18) to the bridge escrow (a storefront buyback set to burn lowers the Solana
supply; see 5.1 "Buyback"). Nothing is minted or burned on the chain
(tx fees go to miners). Invariant: `vault SPL x 1000 == 1e18 - escrow balance`
(relayer adjusts for in-flight transfers).

## 2. Accounts, keys, transactions

- Accounts are ed25519 public keys (32 bytes): the same keypair works on Solana
  and on the chain (verified byte for byte, NOTES-commonware Q4). State per
  account: `balance u64, nonce u64`, plus typed records (agents, jobs).
- Transaction encoding (fixed little-endian binary, so the wallet extension and
  the Solana light client can build and parse it without a codec library):

```
Tx = version u8 (=1) || chain_id u32 || from [32] || nonce u64 || fee u64
     || kind u8 || body (kind-specific, below) || sig [64]
sig = ed25519 over varint(len NS) || NS || all preceding bytes, NS = b"pearl-sol:tx:v1"
```
  (this is Commonware's `sign(ns, msg)` framing; a wallet can produce it with a
  plain ed25519 signMessage over the framed bytes.)

| kind | Name | Body |
|---|---|---|
| 0 | Transfer | to [32], amount u64 |
| 1 | RegisterAgent | name_len u8, name, uri_len u16, uri |
| 2 | PostJob | see 4.1 |
| 3 | SubmitResult | job_id [32], result_hash [32], uri_len u16, uri |
| 4 | CompleteJob | job_id [32], evaluator sigs (count u8, (pubkey [32], sig [64])*) |
| 5 | ExpireJob | job_id [32] |
| 6 | BridgeUnlock | solana_nonce u64, recipient [32], amount_spl u64, job_order (0 or 1, then PostJob body), attester sigs |
| 7 | BridgeLock | solana_recipient [32], amount u64 (multiple of 1000) |
| 8 | AdminConfig | key u8, value bytes (admin key only; every parameter is editable) |
| 9 | SetRoute | solana_recipient [32], route_bps u16, until_height u64, cap u64, release_key [32] (payout lien on the sender, 4.4) |
| 10 | ClearRoute | account [32], route_nonce u64, sig [64] (ends a lien early; sig by its release key, 4.4) |
| 11 | RegisterModel | name, manifest_uri, recipe, mining config, matrices (k, n, hash_b)* (model registry, 4.5) |
| 12 | PostSegments | ticket_id [32], segments (matrix, m, max_blocks, hash_a, c_hash, a_uri)* (standing jobs of a paid inference ticket, 4.5) |
| 13 | RejectResult | job_id [32], deliverer [32], result_hash [32], evaluator sigs (count u8, (pubkey [32], sig [64])*) (job evaluators reject the current delivery; the job reopens, 4.1) |

- Tx fee: `fee` is paid to the block's miner. Minimum fee is a chain parameter.

## 3. Blocks and useful PoW

### 3.1 Header (fixed 256 bytes)

```
version u32 | height u64 | prev_hash [32] | timestamp_ms u64 | target_bits u32
| job_id [32] (zero = fallback block) | miner [32] (payout key)
| tx_root [32] | state_root [32] | withdraw_root [32] | proof_hash [32] | nonce u64
```
- `block_hash = blake3(header)`.
- `pow_digest = blake3(header with proof_hash zeroed)`. This is what the proof
  binds to, so the proof cannot be reused for another block, another parent or
  another payout key.
- `proof_hash = blake3(proof bytes)`; the proof (about 12.6 KB, crates/pearl-job
  PROOF-FORMAT.md) travels in the block body. Headers stay fixed-size for the
  Solana light client.
- `state_root` = QMDB `current` root after this block; `withdraw_root` = root of
  the append-only `keyless` withdrawal log (light client proofs for BridgeLock).

### 3.2 Binding the proof to a job and a block
- A job's Pearl `job_key` comes from its fixed 76-byte job header (built at post
  time, as in v1: `job_id || terms_hash || created_height u64 || share_target u32`),
  so the poster precomputes `hash_a`, `hash_b` once.
- Seed derivation changes one input versus M1 pearl-job: the miner pubkey is
  replaced by the block's `pow_digest`:
  `b_noise_seed = blake3(job_key || pow_digest || hash_b')`, `a = blake3(b || hash_a')`.
- The block's difficulty `target_bits` is the chain target, not the job's.
- Rank penalty (Pearl mainnet rule, from genesis): every mining config has noise
  rank r >= 128; the jackpot bound is `target * h*w*(dlen/r)*128` (=
  `target*h*w*dlen` at r = 128). Localnet/fallback config: k = 2048, r = 128,
  4x8 tile.
- Fallback block (`job_id` zero, no open job or miner choice): matrices are derived
  from `pow_digest` (Pearl style). Valid, earns fees only, no reserve subsidy.

### 3.3 Fork choice and finality
- Heaviest chain by cumulative work, work per block = 2^256 / target.
- Difficulty: WTEMA per block (Pearl's rule), target block time 60 s
  (parameter). Proof storage about 6.6 GB per year at 60 s; bodies prunable past
  finality.
- Finality depth K = 30 blocks (parameter). Reorgs deeper than K are refused.
  Coinbase and reserve subsidies mature after K. The bridge only acts on events at
  least K deep.

### 3.4 Rewards
- Per block: `fees + subsidy`. Subsidy only for job blocks:
  `subsidy = min(pool_balance x emission_ppm / 1e6, job.cap_remaining)`,
  `job.cap_remaining` starts at `alpha x P` (alpha < 1, default 0.8). A self-dealer
  who posts a job and mines it gets back at most `alpha x P` and loses
  `(1 - alpha) x P`. Default emission_ppm per block: 7 (about 1% a day at 60 s).
- A job is mined until its cap or its `max_blocks` is used up or it expires.
- The pool is funded from outside only by reserve `fund_mining` (permissionless and rate
  limited since 2026-10-19, 5.1 "Reserve exit"): pool tokens leave only as subsidy on paid
  jobs (at most alpha x P each), reserve tokens leave only into the pool.

Coinbase receipt (Agent Credit R1, 2026-10-17): the LAST leaf of every non-genesis
block's `tx_root` is the block's reward record, so a reward is provable on Solana by
tx_root inclusion under a final light-client header (no state proof):

```
record (97 bytes) = 0x00 (version) || height u64 || miner [32] || fees u64 || subsidy u64
                    || job_id [32] || mature_height u64
leaf              = blake3(0x00 || blake3(record))      (the tx leaf rule, merkle.rs)
tx_root           = merkle root over (tx hashes in block order, then blake3(record))
```
- `fees` = the block's tx fees, `subsidy` = its reserve subsidy (0 for a fallback block),
  `mature_height = height + finality_k` (K in force at the end of the block): the block
  executing at that height credits `fees + subsidy` to `miner` (less a payout lien's
  share, 4.4). `height`, `miner`, `job_id` repeat the header so a verifier ties the
  record to the header it holds. Every block has one receipt, even with zero reward.
- 97 bytes, not the 99 the request's field list adds up to: the 2 reserved bytes were
  dropped (a later layout would be a new length and version, not reserved space).
  Version 0 can never parse as a tx (txs start with 0x01 and are
  longer than 97 bytes), and a tx can never parse as a receipt, so any receipt-shaped
  leaf under a tx_root IS the block's receipt: a proof needs no leaf position.
- The receipt travels in the block body (`has_coinbase u8 || record`), so tx_root stays a
  context-free check (`Block::check_commitments`: receipt height/miner/job_id = header,
  tx_root over txs + receipt); after execution the node requires the body's receipt to
  equal the one execution produced ("coinbase receipt mismatch" rejects the block). The
  genesis block carries none (its tx_root commits to the genesis spec).
- Miners and pools are unaffected: getBlockTemplate's header already commits to it.
- Solana: `pearl_verify_sbf::chaintx::verify_coinbase(header, record, path)` (path = the
  33-byte tx_root path entries; getBlock returns it) parses the record, checks the header
  fields and the inclusion.

## 4. Native modules

### 4.1 Jobs (AI job market)
`PostJob` body: `m u32, n u32, k u32, config [52], hash_a [32], hash_b [32],
data_uri, worker_pay W u64, protocol_fee P u64, deadline_height u64,
max_blocks u32, evaluators (count u8, pubkey*), threshold u8`.
- W is escrowed in the job; P goes to the mining pool at post time
  (non-refundable). `job_id = blake3(poster || poster_nonce)`.
- Lifecycle: Open -> Delivered (SubmitResult) -> Completed (CompleteJob with
  evaluator sigs over `b"pearl-sol:result:v1" || job_id || deliverer || result_hash`;
  pays W) or Expired (deadline passes; W back to poster).
- `result_hash = blake3_keyed(C as i32 LE row-major, key = job_key)` (pearl-job).
- M1 evaluator = the attester service (Freivalds check off chain). M2: data
  availability for A, B, C so any node or the light client can check.
- **RejectResult** (kind 13, owner decision 2026-10-18): the job's evaluators reject its
  current delivery with `threshold` raw ed25519 signatures over
  `b"pearl-sol:reject:v1" || job_id || deliverer || result_hash` (the CompleteJob rule under
  another domain, so neither signature serves as the other). The tx names `deliverer` and
  `result_hash`, which must be the job's current ones; anyone may send it. Effect: Delivered ->
  Open (same deadline, W stays escrowed, mining continues), deliverer / result cleared, and the
  failure is recorded: `Reject ["reject", job_id || index]` (tag 14: deliverer, result_hash,
  height, P, signer keys), `RejectCount ["reject-count", job_id]` (tag 15) and
  `RejectStats ["reject-stats", deliverer]` (tag 16: rejected, rejected P, first / last
  height). On a plain job the rejected deliverer may not deliver to that job again, so the
  first-deliverer griefing (deliver garbage first, hold the job until its deadline) costs a
  fresh key and fee per attempt and each attempt is on the griefer's record. A ticket's server
  may redeliver (only it can). Segments have no delivery to reject. RPC: getJob
  `reject_count`, getJobRejects, getRejectStats; getAccountTxs lists the reject under the
  deliverer with role `rejected` (docs/RPC.md). Open evaluation: docs/EVALUATORS.md.

### 4.2 Agents
`RegisterAgent`: profile `{name, uri, registered_height}` keyed by account.
- Agent card (owner decision 2026-10-18): `uri` may point at an A2A-style agent card JSON
  (`/.well-known/agent-card.json` shape: name, description, url / interfaces, version,
  capabilities, skills). The card is a self-claim for display and endpoints only; the
  explorer probes it for health and hides profiles with a stale card and no recent delivery
  from its lists (docs/EXPLORER.md).
- Agent profile (owner decision 2026-10-18): derived by an indexer, no chain state beyond
  RejectResult's records (4.1). apps/explorer `/agent/<addr>` + `/api/agent/<addr>` and
  `pearl agent <addr> [--json]` lead with P and storefront SOL behind completed work (never
  job counts), then distinct payers, top payer share, self-dealt share (excluded),
  delivered-then-expired and rejected deliveries, acceptance rate weighted by P, the share
  signed by evaluators other than buyer and seller (signer keys from the CompleteJob tx
  bodies), time to completion, and the mining record from coinbase receipts. Every figure
  is traced to chain state (research/agents/REPUTATION.md 5.1). Open evaluation:
  docs/EVALUATORS.md and `pearl-evaluator`.

### 4.3 Bridge (chain side)
- `BridgeUnlock`: M-of-N attester signatures over
  `b"pearl-sol:deposit:v1" || solana_nonce || recipient || amount_spl` (or the
  `job_order` variant, see 5). Replay set keyed by `solana_nonce`. Pays
  `amount_spl x 1000` from the escrow.
- `BridgeLock`: moves `amount` into the escrow and appends
  `{nonce, solana_recipient, amount_spl}` to the withdrawal log (`withdraw_root`).
- Inbound (Solana -> chain) stays attested in M1 and M2 (our chain verifying
  Solana state is out of scope). Outbound becomes proof-based in M2 (section 6).

### 4.4 Payout liens (Agent Credit R2, 2026-10-17)
A lien makes the chain itself repay a loan from an account's on-chain income: while it is
active, the executor splits a share of every income credit to the account into
withdrawal-log entries to a Solana recipient (Agent Credit's repay authority), which the
light-client path (bridge `release_proof`, header-relay `--settle`) releases on Solana.

`SetRoute` (kind 9), sent and signed by the account:
`solana_recipient [32], route_bps u16, until_height u64, cap u64, release_key [32]`.
- `route_bps` 1..=10000, `until_height` > current height, `cap` > 0 (chain base units,
  lifetime total the lien may take), recipient non-zero. `release_key` all zero = the
  lien cannot be cleared early (it ends only at `until_height` or `cap`).
- ONE lien per account. While one is active, SetRoute is accepted only as an extension:
  same recipient and release key, and `route_bps`, `until_height`, `cap` each >= the
  current value (`routed`, `pending` carry over). So the account can never shorten,
  weaken, redirect or cancel an active lien, and a second lender cannot dilute the first.
  After a lien ended (expired, cap reached, cleared), a new SetRoute starts a fresh one.
- Every SetRoute (new or extension) gets `route_nonce` = previous + 1 (first 0), so an
  old release signature cannot clear a later lien.

Income that is split ("credit" = what the chain credits to the account), and only this:
1. matured coinbase (fees + subsidy credited at `mature_height`, 3.4), for the payout key;
2. CompleteJob worker pay W, for the deliverer;
3. ExpireJob refunds of W, for the poster.
Transfers, BridgeUnlock deposits and pool payouts (off chain, R4) are not split.

Split of a credit `amt` at height h, when `!cleared && h <= until_height && routed < cap`:
- `share = min(floor(amt x route_bps / 10000), cap - routed)`; the account is credited
  `amt - share`; `routed += share`; `pending += share`.
- Unless the bridge is paused: `out = floor(pending / 1000) x 1000` moves into the escrow
  and appends the withdrawal entry `{nonce, from = account, solana_recipient, amount_spl =
  out / 1000, height}` to the existing log (same nonce counter and record as BridgeLock;
  `withdrawn += out`). The remainder (< 1000 units, i.e. < 1 SPL base unit) stays in
  `pending` for the next credit, so the routed fraction is exact over time.
- While the bridge is paused, `pending` just accumulates and goes out at the first
  credit after the unpause.
- When the lien ends (cap reached in this credit, or cleared), its dust (< 1000) goes
  back to the account at once; after it ended by height, at the account's next income
  credit, or at the next SetRoute (which also first sends out any whole units left).
- Supply: `pending` counts in the supply invariant (executor `supply_of`); the escrow
  receives exactly the SPL the entries promise.

`ClearRoute` (kind 10), sent by anyone (the sender pays the fee):
`account [32], route_nonce u64, sig [64]`, `sig` = RAW ed25519 by the lien's `release_key`
over `"pearl-sol:route-clear:v1" || chain_id u32 LE || account [32] || route_nonce u64 LE`
(chain id added to the request's draft so a signature cannot be replayed on another
network). Refused when the lien is not active, `route_nonce` is not the current one, or
the lien has no release key. Effect: `cleared`, pending whole units go out, dust back.

Interaction with BridgeLock: none beyond sharing the log. The account can still
BridgeLock or Transfer its own (unrouted) balance anywhere; the lien takes its share of
income before the account ever holds it. Lien entries and BridgeLock entries are
indistinguishable on Solana (`from` = the account), which is what Agent Credit's F5
proof reads.

State: `Route ["route", account]` (schema tag 10): recipient, bps, until, cap,
release_key, route_nonce, set_height, routed, pending, withdrawn, cleared; kept after
the lien ends. RPC `getRoute`, `getAccount.route` (docs/RPC.md).

### 4.5 Model registry and standing jobs (Cadre inference, SYNTHESIS item 3, 2026-10-18)

Why: posted-data matmul has no natural buyer (moving A and B costs far more than the
compute, research/narratives/COMPUTE-TECH.md). Useful work pays when the data already sits
on the miner: model weights. So a model's weight matrices are registered ONCE (B, the big
operand, stays on the miners), and a paid inference request becomes a set of jobs whose B
is a registered weight matrix and whose A is the activations the server computed. The
miner serving the model runs exactly those GEMMs, so block rewards act as a rebate on the
inference price. Only paid work earns: every standing job hangs off a paid ticket and the
ticket's whole subsidy stays under `alpha x P` (3.4). The proof itself is unchanged: a
standing job proves `C = A B` exactly in int32 with operands in [-64, 64].

Quantization recipe (fixed by research/INT7-ACCURACY.md; measured +2.9% WikiText-2 PPL on
Llama-3.2-1B with every job-shaped GEMM in int7, int8 W8A8 + SmoothQuant +1.5%). Recipe 1,
`cadre-w7a7-sq-asym-v1`:
- weights: per output channel j, symmetric round-to-nearest onto [-64, 64] with scale
  `s_w[j] = max_l |W'[j,l]| / 64`, where `W' = W diag(s)` has the SmoothQuant factors folded
  in (`s_l = amax_x[l]^0.5 / amax_w[l]^0.5`, calibrated on 16 x 512 WikiText-2 train tokens;
  q/k/v share one s, gate/up share one s, o_proj's s is shared per KV group). lm_head is
  not smoothed. The registered B is `round(W'^T / s_w)` (k x n).
- per matrix, outside the proof: `s_w` (n f32), `colsum(B)` (n i32, sum over k), `s` (k f32,
  or none).
- activations: per token (row of A), asymmetric onto [-64, 64]: `x' = x / s`,
  `s_a = (max x' - min x') / 128`, `z = round(-64 - min x' / s_a)`,
  `A = clamp(round(x' / s_a) + z, -64, 64)`.
- output: `y[i,j] = s_a[i] * s_w[j] * (C[i,j] - z[i] * colsum[j])` (+ bias). The zero-point
  correction and every scale are computed outside the proof.
- attention (QK^T, softmax, PV), RoPE, RMSNorm, SiLU, residuals and the embedding lookup run
  in fp32 and are never jobs (activation x activation has no registered B).

What can be a standing job: a weight GEMM whose `B = W^T` (k x n) passes `check_shape` for
the model's mining config (k >= 16r = 2048 at r = 128, k % 64 == 0, k <= 4r^2, n a multiple
of the column period). Layers that do not fit run unmined (int7 or fp, the tool records
which) and are not registered as matrices. Large n is split into column shards (output
channel ranges) of at most 8192 so a miner's working set stays bounded; inputs shared by
several projections are fused when they share the SmoothQuant factor (q/k/v -> one `qkv`
matrix, n = 3072 on Llama-3.2-1B).

Llama-3.2-1B (the demo model, `unsloth/Llama-3.2-1B`): 16 layers x {qkv 2048 x 3072, o
2048 x 2048, gate 2048 x 8192, up 2048 x 8192, down 8192 x 2048} + lm_head 2048 x 128256 in 16
shards of 8016 = 96 registered matrices, 1,235,746,816 MACs per token row, all mined. Not
mined: attention (2 x 32 heads x 64 x T MACs per layer per token at context T), norms, RoPE,
SiLU, the embedding lookup, scales and zero points.

**`RegisterModel` (kind 11)**, body:
```
name (u8 len, 1..=64) | manifest_uri (u16 len, blob:<hex>) | recipe u8 (= 1)
| rank u16 | rows_pattern [6] | cols_pattern [6]
| count u16 (1..=384) | (k u32 | n u32 | hash_b [32]) * count
```
- `model_id = blake3("pearl-sol:model:v1" || registrant || name)`; registering an existing
  id is refused (a new version is a new name).
- Per matrix the chain checks `check_shape` of `(k, rank, patterns)` with m = the row
  period, and `k x n <= 2^26`. It never reads the blobs (consensus must not depend on
  local data): `hash_b` is the registrant's claim, which anyone holding the blobs can
  recompute; miners and the attester refuse a matrix whose files do not match.
- Matrix job header (fixed per matrix, so `hash_b` is computed once at registration):
  `job_pubkey = model_id`, `terms_hash = blake3("pearl-sol:matrix-terms:v1" || model_id ||
  index u16 || k u32 || n u32)`, `created_slot = 0`, `target_bits = 0`; config = `(k, rank,
  rows_pattern, cols_pattern)`; `job_key_j = blake3(header76 || config52)` as for every
  job, `hash_b_j = commit_b(job_key_j, B_j)`. The pow-bound seed (3.2) binds every block
  proof to its header, so sharing one job key across many jobs allows no pre-mining.
- Fee: the tx fee only. Registration deposit: OPEN DECISION (section 9).
- State `Model ["model", model_id]` (schema tag 11): registrant, name, manifest_uri, recipe,
  rank, patterns, matrices (k, n, hash_b), `macs_per_row = sum k x n`, registered_height,
  and running totals (tickets, segments, segment blocks, subsidy). Index `["model_index",
  i]` (tag 12) and a model counter (tag 13). RPC `getModel`, `listModels`.

Blob-store layout (M2 blobs, node-local, not consensus): the model is a directory of files;
every file is cut into raw chunks of at most 3 MiB, each chunk one blob. The manifest blob
(`manifest_uri`, JSON, `"format": "cadre-model-v1"`) lists the recipe, the architecture,
every matrix (`index, name, layer, role, k, n, cols [lo, hi), hash_b`, its weight file and
aux file) and every file (`size`, blake3 of the whole file, chunk blob ids in order).
- weight file `w/<index>.i8`: `B^T` (n x k int8, row = output channel), the layout
  commit_b hashes, so miners mmap it as is;
- aux file `w/<index>.aux`: `"LQA1" || n u32 || k u32 || flags u32 (bit 0: has smooth) ||
  s_w f32[n] || colsum i32[n] || [s f32[k]]` (little-endian);
- non-GEMM tensors `t/<name>.<dtype>` (embedding bf16, norms f32), `config.json` and the
  tokenizer files, raw.
A node pins (never prunes) the manifest and every chunk it lists for each registered model
whose manifest it holds. `putBlob` takes `"push": false` so a model upload is not pushed to
every peer; others fetch on demand. `pearl model fetch` rebuilds the directory from blobs
and checks every file hash and every `hash_b`.

**Ticket** (one paid inference request): a `PostJob` body (4.1) whose config is the ticket
form, so the existing storefront Order (5.1) carries it unchanged:
```
config [52] = "CADRE-T1" || model_id [32] || 0 [12]
m = prompt tokens, n = max new tokens, k = 1
hash_a = request_hash = blake3(request blob)        hash_b = server key (0 = first PostSegments sender)
data_uri = request blob (blob:)                      max_blocks = block budget of all its segments
W, P, deadline, evaluators, threshold as for any job
```
- The model must be registered; the row budget is `R = round_up(m + n, row period) <= 4096`.
  A one-request ticket uses m = prompt tokens, n = max new tokens; a prepaid balance ticket
  (5.4) uses m = R - 1, n = 1.
- A ticket is a JobRecord with a ticket extension (`model_id, server, rows R, cap_total =
  alpha x P, segments, blocks allocated, blocks mined, subsidy`). It is never mineable. W is
  escrowed, P goes to the mining pool at post time (non-refundable), `cap_remaining` starts
  at `alpha x P` and is the not-yet-allocated part of the ticket's subsidy cap.
- Lifecycle as a job: Open -> Delivered (SubmitResult by the server only, after its last
  round; `result_hash` = blob id of the response blob, uri = that blob) -> Completed
  (CompleteJob, evaluator signatures over the usual result message; W pro rata to the server,
  see PostSegments) or Expired (after the deadline; W back to the poster, the unallocated cap
  stays in the pool).
- Request blob: JSON `{"format": "cadre-request-v1", "model_id", "prompt_ids": [...],
  "max_new_tokens", "sampling": "greedy"}`. Response blob: JSON `{"format":
  "cadre-response-v1", "model_id", "ticket_id", "request_hash", "rows", "rounds": [{"round",
  "m", "requests": [{"request_id", "body_sha256", "row_offset", "rows", "prompt_tokens",
  "completion_tokens", "output_sha256"}]}], "segments": [{"round", "matrix", "segment_id", "m"}]}`
  (one-request tickets also carry `prompt_ids`, `output_ids`, `text`). Per-request receipts
  live here: the chain sees one ticket per balance, the buyer can check every request's rows.

**`PostSegments` (kind 12)**, sent by the ticket's server:
```
ticket_id [32] | round u16 | count u8 (1..=64)
| (matrix u16 | m u32 | max_blocks u32 | hash_a [32] | c_hash [32] | a_uri (u16 len, blob:)) * count
```
- Rounds (prepaid balances, owner decision 2026-10-18): one ticket can carry many requests.
  The server serves requests off chain against the ticket's row budget R and posts their GEMMs
  in rounds: round r stacks the rows of every request served in that period (each request's
  rows contiguous, zero-padded to the row period). The ticket records `rounds` (started),
  `round_rows` (m of the current round) and `rows_used` (sum of every round's m). A tx names
  either the current round (`round = rounds - 1`, every segment's `m = round_rows`) or starts
  the next one (`round = rounds`, sets `round_rows = m`, `rows_used += m`); `rows_used <= R`.
  A one-request ticket is a single round 0.
- Ticket must be Open and `height <= deadline`; the sender must be the server (or claims an
  unset server). `matrix < count`; `m` a positive multiple of the row period; `max_blocks >= 1`
  and all segments together use at most the ticket's `max_blocks`; `a_uri` a well-formed blob
  URI.
- Each segment is a standing job: `segment_id = blake3("pearl-sol:segment:v1" || ticket_id
  || matrix u16 || round u16)` (one segment per matrix per round), a JobRecord with poster =
  server, `(m, n, k)` and config from the registry, `hash_a` from the tx, `hash_b` = the
  registered one, header = the matrix job header above, `data_uri = a_uri`, W = P = 0,
  deadline = the ticket's, `result_hash = c_hash = blake3_keyed(C as i32 LE row-major,
  job_key)` (the output the server used, committed at post time; status stays Open),
  evaluators = the ticket's.
- Subsidy cap, MAC-proportional: `seg_cap = floor(cap_total x m k n / (R x macs_per_row))`,
  moved from the ticket's `cap_remaining` into the segment's. Per matrix the rounds' m sum to
  `rows_used <= R`, so `sum seg_cap <= cap_total = alpha x P`: the paid-work rule holds for the
  whole ticket however its segments are mined, and unused rows earn nothing. Mining a segment
  is an ordinary job block (3.4): `subsidy = min(pool x emission_ppm, seg cap_remaining)`; the
  ticket's and the model's totals are updated in the same block.
- Pay for rows used: CompleteJob on a ticket pays the server `floor(W x rows_used / R)` and
  returns the rest of W plus `floor(P x (R - rows_used) / R)` (from the mining pool) to the
  poster, so an unused balance or an early stop is refunded on the chain (in CADRE, to the
  buyer's chain account). The subsidy bound still holds: the segments can earn at most
  `alpha x P x rows_used / R`, alpha times the P the pool keeps. ExpireJob refunds all of W
  (P stays in the pool, as for every job).
- Segments are mineable until the ticket's deadline whether or not the ticket completed.
  SubmitResult and CompleteJob are refused on a segment (its output is committed at post
  time); ExpireJob closes it after the deadline (nothing to refund).
- A block on a segment needs `A` (the `a_uri` blob, `"PSA1" || m u32 || k u32 || A i8[m x
  k]`) and `B_j` (the miner's copy of the registered model). The template's job object
  carries `kind: "segment"` and the standing fields so a miner knows to load B from its
  model store.

How a paid inference request maps to jobs, pay and fees:
1. Buyer -> inference gateway (5.4) -> storefront-x402 -> storefront `buy_job` with the ticket
   as the Order spec. Price (storefront-x402, provisional knobs shared with matmul jobs):
   `job = PRICE_BASE_LAMPORTS + 2 x macs_per_row x R x PRICE_LAMPORTS_PER_GFLOP / 1e9`, then
   grossed up by the buyback share (5.1) plus the overhead. OPEN DECISION: inference price.
2. The tokens bought split into W (80%) and P (`protocol_fee_bps`, 20%), bridge to the chain
   and the relayer's order `BridgeUnlock` posts the ticket (job id = the order job id).
3. The server runs the request in int7 (recipe 1). Every registered matrix's GEMM over all
   rows of the request (prompt, then each generated token, stacked; padded to the row
   period) is one segment; PostSegments posts them with the A blobs and C commitments.
4. Miners mine the segments (one block per segment by default); each block's subsidy comes
   from the pool under the segment's cap, its fees as usual.
5. The server delivers the response blob (SubmitResult); the evaluators check it (5.4) and
   sign; CompleteJob pays W to the server; the order is finalized on Solana by the light
   client path (`finalize_order_proof` proves the ticket's CompleteJob), so `Order.result_hash`
   is the response blob id and `result_uri` its blob URI.
Totals per request: buyer pays `sol_in + overhead`; the server earns W plus, when it mines
its own segments, up to `alpha x P` of subsidy and the blocks' fees.

## 5. Solana side

Existing: `reserve` (launch, buybacks, fund_mining) and `bridge` (deposit,
release with M-of-N ed25519 signatures). Changes and additions:

Reserve exit (owner decision 2026-10-18, built 2026-10-19; token/programs/reserve): the
reserve has no admin-key exit.
- `fund_mining(amount)` is PERMISSIONLESS: any signer (it pays the bridge Deposit rent)
  moves reserve tokens into the bridge vault with `config.mining_pool` as the chain
  recipient. `config.funder` is no longer read (kept for the layout).
- Rate limits, all admin-editable with `set_fund_limits(FundLimitParams { paused,
  max_per_call, min_interval_secs, window_secs, max_per_window })` (event
  `FundLimitsUpdated` with every value): `max_fund_per_call` (0 = no cap),
  `fund_min_interval_secs` between calls, at most `fund_max_per_window` per
  `fund_window_secs` window (a window starts at the first funding after the previous one
  ended; 0 = no window cap). Defaults PROVISIONAL: 10M tokens per call, 3600 s, 20M tokens
  per 86,400 s (2% of supply a day). The admin can pause (`paused`, error `FundPaused`)
  but cannot redirect: `mining_pool` and `bridge_program` freeze once `total_funded > 0`
  (`update_config` then refuses a different value, `DestinationFrozen`).
- `MiningFunded` gains `caller, reserve_left, window_start, window_used, calls` (appended;
  older decoders stop after `total_funded`).
- Config layout 1 appends `layout u8, fund_paused bool, fund_min_interval_secs i64,
  fund_window_secs i64, fund_max_per_window u64, fund_last_ts i64, fund_window_start i64,
  fund_window_used u64, fund_calls u64` (58 bytes; account 334 bytes). A config written
  before (276 bytes) does not deserialize until the admin runs `migrate_config()` (realloc,
  rent from the admin, fund defaults set; no-op on a current config).
- What remains trusted: the program upgrade authority (as for every program here).
- Creator-fee buybacks (`crank_buyback*`) still land in the reserve unconditionally; the
  reserve cap below applies to the storefront's buybacks.
- Readers: `node services/crank/src/main.ts status | fund | fund-limits | migrate`, explorer
  `/api/supply` (`reserve`).

### 5.1 `storefront` program (M1)
- `buy_job(order)`: buyer pays SOL (localnet and mainnet) or USDC (mainnet, via
  Jupiter CPI to SOL first). The program buys the token on the curve or PumpSwap
  (reuse reserve's buy code), splits it into W and P, and deposits it into the
  bridge with an `Order` account holding the job spec (dims, config, hash_a,
  hash_b, data_uri, deadline, evaluators) and the buyer.
- The relayer turns the deposit into one `BridgeUnlock` with `job_order`, which
  unlocks and posts the job in one tx on behalf of the buyer's chain address.
- `finalize_order(order, result_hash, result_uri)`: records the result on Solana.
  M1: M-of-N attester signatures. M2: a proof against a light-client header.
  The `Order` account is readable by any Solana program (CPI-composable result).
- Refund path: if the job expires, W comes back over the bridge to the buyer. If the
  chain never unlocks the order, the deposit is refunded on Solana (see "Refunds" below).
- Slippage bound and per-order caps are admin-editable.

As built (18f3cd0, token/programs/storefront, ID DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr):
- Two steps, since the spec plus about 27 accounts exceeds 1232 bytes:
  `open_order(draft_id, recipient, spec)` writes an `OrderDraft`, then
  `buy_job` (curve) or `buy_job_amm` (PumpSwap) buys via a system-owned trader
  PDA `["trader"]`, splits P (`protocol_fee_bps`, default 2000) and W, deposits
  through the bridge with the trader PDA as sender, and creates
  `Order ["order", deposit_nonce]` (fixed 1070 bytes, offsets in
  `storefront::layout`).
- A bridge Deposit whose sender is the trader PDA is an order. The relayer gets
  attested: `b"pearl-sol:deposit-order:v1" || nonce || recipient || amount_spl ||
  sha256(PostJob body)` (stored as `order.body_hash`).
- Order job id on the chain: `sha256(b"pearl-sol:order-job:v1" || nonce LE)`
  (Solana has no usable blake3). Normal PostJob keeps `blake3(poster || nonce)`.
- The order's deadline is relative: `deadline_height = unlock height + deadline`.
- Order job header (fixed after the M1 demo, 29f4783..): the buyer commits hash_a /
  hash_b before W, P, job_id and the absolute deadline exist, so order jobs use the
  Pearl job header `poster || order_terms_hash`, where order_terms hashes poster,
  m, n, k, max_blocks and the relative deadline (chain/state/src/schema.rs).
  `JobRecord.header_id` holds it and getJob returns it; miners build the job header
  from header_id, not job_id. Normal PostJob jobs keep `job_id || terms_hash || 0 || 0`.
- `finalize_order` callers: buyer, admin or an attester (result_uri is unsigned).
  `expire_order` takes attester sigs over `b"pearl-sol:expired:v1" || job_id`.
- One-time setup: the trader PDA needs pump and PumpSwap
  `init_user_volume_accumulator` before the first order.
- The x402 service wallet is the on-chain buyer; the payer's key is the chain
  recipient.

Refunds (owner decision 2026-10-17; 6743aa0, e9a3dfa): an order deposit the chain will
never unlock (the relayer holds it back: P below `min_protocol_fee_effective`, or the
Order closed on Solana before any unlock) is refunded ON SOLANA, in tokens:
- bridge `refund_deposit(nonce, buyer)`: M-of-N bridge signers sign
  `b"pearl-sol:refund:v1" || nonce u64 LE || buyer [32] || amount_spl u64 LE`
  (ed25519 precompile, as `release`); the vault pays the Deposit's full amount to the
  buyer's token account and writes the refund record `Processed ["processed", [0],
  nonce]` (source chain 0 = a Solana deposit; withdrawals use [1]). The record is the
  replay guard (a deposit is refunded once) and is open in every `release_modes`
  (inbound stays attested in M2). For orders, buyer = `Order.buyer`.
- storefront `mark_refunded()`: permissionless and CPI-free; reads the refund record
  (owner = `config.bridge_program`, address, nonce, recipient = `order.buyer`, amount =
  `order.tokens`) and moves a Pending or Expired Order to Refunded (status 4). Finalize /
  expire then refuse it (NotPending).
- Mutual exclusion with the chain's BridgeUnlock: the chain cannot read Solana, so
  "a deposit is never both unlocked on the chain and refunded on Solana" rests on the
  signers signing at most ONE of the two messages per nonce. Requirements: the bridge
  signer set is the chain's attester set (same operators), each operator's relayer keeps
  the choice in persisted state, and threshold > N/2 (two disjoint signer subsets cannot
  both reach the threshold). services/relayer enforces it for its keys: it saves
  `signedUnlock[nonce]` before sending an unlock and never refunds those; it saves
  `refunds[nonce]` before signing a refund and never unlocks those; it reads the refund
  records every tick and never unlocks a refunded deposit (whoever refunded it); it
  refunds only a deposit the chain reports as unprocessed, and only after
  `REFUND_GRACE_MS` (default 1 h; lowering the floor within it unlocks normally).
  A refunded deposit left the vault, so the invariant counts it as settled
  (`vault x 1000 == 1e18 - escrow` holds exactly again).
- The relayer refunds in one Solana tx: buyer ATA (idempotent), ed25519 precompile,
  `refund_deposit`, `mark_refunded`. Plain (non-order) deposits are not auto-refunded.
- Measured (LiteSVM): refund_deposit about 23k CU, mark_refunded about 10.5k CU.
- An expired job (unlocked, posted, never delivered) still refunds W the normal way:
  ExpireJob on the chain returns W to the poster, then BridgeLock + release.

Buyback (as built 2026-10-18, SYNTHESIS item 2; token/programs/storefront "Buyback"):
Why: research/narratives/FLOWS-AND-PUMPS.md: buybacks held where they were continuous,
on chain and fed by real fees (Hyperliquid, Venice); webpage-only or discretionary ones did
not (StonkFun, Aster). So the share is a program rule inside `buy_job`, and every buy is an
event anyone can read.
- Where storefront SOL went before: the x402 price is `sol_in + overhead`. `overhead`
  (default 12M lamports) stays with the service wallet and pays the Order and Deposit rent
  and tx fees, so it is a cost, not margin. All of `sol_in` bought the token (pump's 1.25%
  fee, of which 0.30% creator fee comes back to the reserve's creator-fee buyback); the
  tokens became W (job pay, 80%) and P (to the mining pool, 20%). There was no margin.
- Now: `buy_job` / `buy_job_amm` first move `share = sol_in x config.buyback_bps / 10000`
  (floor) from the buyer to the buyback vault, a system-owned data-less PDA `["buyback"]`,
  and the job buys with `sol_in - share`; `Order.sol_spent` is the job's part. The first
  share also tops the vault up to rent exemption (890,880 lamports, not counted as a share).
  Both instructions take two more accounts at the end: the vault, then the order's
  `ShareRecord ["share", deposit nonce]` (57 bytes: disc, nonce u64, buyer [32], lamports
  u64, bump u8; created in the handler, rent paid by the buyer).
- Held until the order closes (owner decision 2026-10-18: a refunded order also refunds
  its share). A new share is HELD (`config.bb_locked`, event `BuybackShareHeld`) and no crank
  can spend it. `settle_share()` (permissionless, accounts config, order, share record,
  buyer = order.buyer, vault, system program) settles it once the Order is closed:
  Finalized -> released to the buyback (`BuybackAccrued`, now crankable, counted in the
  next buy's order range); Refunded or Expired -> paid back from the vault to the buyer
  (`BuybackShareRefunded`, `config.bb_refunded`). Pending -> refused. The record is closed
  (rent back to the buyer), so a share settles exactly once. Why this option and not a
  refundable reserve: the buyer pays for a buyback only when the job was delivered, the
  vault always holds every unsettled share (cranks are bounded by `vault - rent -
  bb_locked` and re-check that floor after the trade), so a refund can never find the SOL
  already spent; the cost is a delay (shares buy back after finalize, not at order time).
  Expired orders refund the share too (W comes back as well; P does not).
  services/relayer adds `settle_share` to its refund tx; the crank (`sf-settle`, and
  `sf-buyback` / `run` first) settles every closed order before buying. The x402 service grosses
  `sol_in` up so that `sol_in - share` still covers the job's own price: the buyback is a
  markup on top of the job (quote fields `job_lamports`, `buyback_lamports`).
- Crank: `crank_buyback(max_sol_in, min_tokens_out)` (curve, `buy_exact_quote_in_v3`) and
  `crank_buyback_amm` (PumpSwap after graduation, `buy_exact_quote_in_v2`, wSOL ATA
  wrapped and closed, its rent back to the cranker). The vault PDA is the pump `user`,
  exactly as the reserve's creator PDA is for creator-fee buybacks. Permissionless unless
  `buyback_cranker` is set. Spend = `min(vault - rent - bb_locked (released shares only),
  buyback_max_per_call (0 = no cap), max_sol_in)`, at least `buyback_min_buy`, at most once per `buyback_min_interval_secs`;
  fill checked like `buy_job` (caller's `min_tokens_out`, and `max_slippage_bps` against
  the program's fee-free quote). Then the vault ATA's whole balance goes to
  `buyback_dest`: 0 = the mining reserve (TransferChecked to
  ATA(PDA(["creator"], reserve_program), mint, Token-2022), the reserve's own token
  account, so `fund_mining` can pay it out), 1 = burn (Token-2022 BurnChecked).
- Admin: `set_buyback(BuybackParams { buyback_bps, destination, reserve_program, cranker,
  max_per_call, min_buy, min_interval_secs })` (each optional; bps < 10000; destination
  reserve needs a reserve program); emits `BuybackConfigUpdated` with every value.
  Defaults at `initialize`: share 1000 bps (10%) and destination mining reserve are
  DECIDED (owner, 2026-10-18); reserve program `4FkQcoCM...LY6p`; cranker anyone, 1 SOL per
  call, min 0.001 SOL and 600 s stay PROVISIONAL.
- Config fields appended after `light_client`: `buyback_bps u16, buyback_dest u8,
  reserve_program [32], buyback_cranker [32], buyback_max_per_call u64, buyback_min_buy u64,
  buyback_min_interval_secs i64, bb_last_ts i64, bb_accrued, bb_spent, bb_tokens,
  bb_to_reserve, bb_burned, bb_buys, bb_pending_orders, bb_pending_first_nonce,
  bb_pending_last_nonce, bb_locked, bb_refunded (u64)` (187 bytes, starting 59 bytes after
  the attester vector; readers that stop at bb_pending_last_nonce still parse).
  The Order layout is unchanged (1070 bytes). A config written before this reads as zeros
  (share 0) as long as the attester vector leaves 171 bytes of slack (4 attesters or fewer);
  a fuller one needs a fresh initialize (localnet only so far).
  `bb_accrued` now counts RELEASED shares (finalized orders).
- Events (Anchor `Program data:` logs; discriminator `sha256("event:<Name>")[..8]`, Borsh):
  `BuybackShareHeld { order, nonce, lamports, total_locked, vault_lamports }` per order
  (buy_job*); `BuybackAccrued { order, nonce, lamports, vault_available, total_accrued }`
  when a finalized order's share is released; `BuybackShareRefunded { order, nonce, buyer,
  lamports, status, total_refunded }` when a refunded / expired order's share goes back;
  `BuybackExecuted { buy, venue, sol_in, tokens_out, routed, destination,
  destination_account, quote, first_nonce, last_nonce, orders, available_before, carry,
  total_spent, total_tokens, total_to_reserve, total_burned }` per buy. `first_nonce ..
  last_nonce` / `orders` = the Orders (bridge deposit nonces) whose share was released since
  the previous buy; `carry` = what a per-call cap left for the next buy (a later buy can then
  show `orders = 0`). Vault = rent + bb_locked + released-not-yet-spent (plus any donation
  sent straight to the vault, which is bought back too); sum of released shares = sum of
  buys' `sol_in` + what is crankable now.
- Supply: the bridge invariant `vault SPL x 1000 == 1e18 - escrow` is unaffected (the
  buyback never touches the bridge vault or the escrow; tested). A burn lowers the Solana
  mint supply below 1B; the escrow lamports matching burned tokens can never be unlocked, so
  the chain-side bound is `1e18 - escrow <= 1000 x mint supply` (demo step 6 checks it).
  "Supply stays 1B across both chains" now reads: 1B minus what the buyback burned.
- Refunds: a refunded order (bridge `refund_deposit` + `mark_refunded`) gets its tokens
  back and, by `settle_share`, its buyback share in SOL (owner decision 2026-10-18).
- Measured (LiteSVM): buy_job 180.8k CU (tx 1133 bytes), buy_job_amm 208.7k, crank_buyback
  134.8k (894 bytes), crank_buyback_amm 184.1k. x402 `overhead` default 14M lamports (12M +
  the ShareRecord rent, which comes back to the service wallet at settle).
- Sandwich risk is the reserve's: the on-chain quote is read inside the tx, so the caller's
  `min_tokens_out`, the per-call cap and the optional cranker allowlist are the protection.
- Reserve cap (owner decision 2026-10-18, built 2026-10-19): with `buyback_dest` 0 a buy's
  tokens go to the mining reserve only while the reserve ATA holds less than
  `bb_reserve_cap`; the part that would lift it above the cap is burned (Token-2022
  BurnChecked, lowers the mint supply) in the same instruction: `to_reserve =
  min(routed, cap - reserve_balance)` (`storefront::reserve_share`), the rest burned.
  `buyback_dest` 1 still burns everything (override). Cap 0 = no cap (all to the reserve);
  a config written before the field reads 0, so an upgraded program keeps the old
  behaviour until the admin sets it. Default at `initialize` PROVISIONAL: 100M tokens (1e14
  base units, 10% of supply). Admin: `set_buyback_cap(reserve_cap)` (event
  `BuybackCapUpdated`). Config: `bb_reserve_cap u64` appended after `bb_refunded` (fits the
  attester-vector slack of a config with 9 attesters or fewer).
  Events per buy, before `BuybackExecuted`: `BuybackToReserve { buy, tokens,
  reserve_before, reserve_after, reserve_cap }` and `BuybackBurned { buy, tokens, reason
  (0 above cap, 1 destination burn), reserve_before, reserve_cap, supply_after,
  total_burned }`. `BuybackExecuted.destination` is 0 when any part reached the reserve
  (`destination_account` the reserve ATA), else 1 (the mint); `bb_to_reserve` /
  `bb_burned` add the parts. Measured (LiteSVM): crank_buyback with the split 127.5k CU.
  Bridge bound after burns: `1e18 - escrow <= 1000 x mint supply` (tested in LiteSVM and
  demo step 6).
- Public numbers (SYNTHESIS item 7): `tools/defillama` (fees = OrderPlaced.sol_spent +
  BuybackShareHeld, holders revenue = BuybackAccrued), explorer `/api/supply`
  (`/circulating`, `/total`: mint supply - reserve - chain pool - transit) and `/revenue`
  (self-dealing excluded, unique payers); docs/EXPLORER.md.
- Readers: `GET /buyback` (storefront-x402), `node services/crank/src/main.ts sf-status`,
  `pearl buyback [--json]` (config, vault, totals, latest `BuybackExecuted`).

### 5.2 x402 endpoint (M1)
`services/storefront-x402` (port 10662): an HTTP service that prices a job,
accepts x402 payment on Solana, calls `buy_job`, and serves the result once
finalized.

### 5.3 Light client program (M2)
- `submit_header(header)` + `verify_work(header, proof buffer)`: plain Pearl verify
  split over 2 transactions (about 1.3-1.9M CU total), checks `prev_hash`,
  `target_bits` against our WTEMA rule, and stores the header with cumulative work.
  It keeps the heaviest tip and treats headers K deep as final.
- `bridge.release` gains a proof mode: a Merkle proof of a withdrawal against
  `withdraw_root` of a final header. The M-of-N signer path stays as a fallback,
  admin-switchable.
- `storefront.finalize_order` likewise proves `Completed` against `state_root`.
- Anyone can relay headers (permissionless crank, reimbursed per header from a fee
  pool; cost roughly a few thousandths of a SOL per header).


As built (f5caf0f..50bf84c, token/programs/lightclient, ID
BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5; verifier crates/pearl-verify-sbf;
relay services/header-relay):
- Verification is split into many steps (proof upload in chunks, then proof hash,
  fallback rebuild for fallback blocks, openings, noise, jackpot). Measured after the
  rank-penalty port (k 2048, r 128, 4x8 tile, proof 24,724 bytes): fallback block
  10.24M CU / 37 txs (submit 31.7k, begin + first chunk 24.9k, 26 chunks of 950
  bytes at 1.5k each, 9 verify-step txs of up to 1.23M); job block about 7.25M CU /
  34 txs. Fees: see "Relay cost on real Solana" below; 0.0042 SOL rent per header
  record, refunded on close_header. Relayers can be paid from a pool.
- Parallel relay (2026-10-18). Problem: one header is a chain of about 12 dependent,
  each-confirmed transactions on one work account, and `submit_header` required a
  Verified parent, so a relay could verify only one header at a time: about 12
  confirmation rounds per header. On the loaded test VPS (366 ms slots instead of 50
  ms) that was 7.5 s per header against 1.5 s demo blocks; it fell 12,000 blocks
  behind and never released proof-mode withdrawals. Fix (lightclient + header-relay):
  `submit_header` accepts any submitted parent (Pending, WorkDone or Verified; WTEMA
  target, MTP and future-time checks still use the parent's stored fields); the
  completing `verify_jackpot` marks the header Verified only if its parent is Verified
  (the parent is the tip, or the parent's record is passed as the one remaining
  account), else `WorkDone` (status 3: own work checked, not part of the verified
  chain, no tip change, no reward); `connect(parent, header)` promotes a WorkDone
  header once its parent is Verified (tip extension and reward as before; several per
  tx, parent first). Invariant unchanged: Verified = own work verified AND parent
  Verified, so by induction from the checkpoint every Verified header (and therefore
  the tip, finality, `mark_final`, `register_job`, `accumulate`, every proof reader)
  has a fully verified ancestry, exactly as before. What the relaxation allows: anyone
  may create Pending / WorkDone records on unverified parents; each costs them rent
  (refundable once at or below the final floor) and touches no shared state; a forged
  ancestor (fake timestamps lowering the WTEMA target) can make an easy child WorkDone
  but never Verified, because the forged ancestor's own work never verifies. Rewards
  are paid only at Verified, so easy WorkDone headers earn nothing. Also
  `close_work` (rent of a work account back to its authority). header-relay keeps
  `--parallel N` (default 8) headers in flight, one work account each, and advances all
  of them every confirmation round: submit frontier (two headers per tx), begin +
  first chunk, the other chunks as one batch, the planned steps; status polls and
  account reads are batched; connect / reorg / mark_final / accumulate / close run as
  fire-and-forget batches; txs ask for the planned CU, not 1.4M. Throughput is about N
  headers per 12 rounds instead of 1. LiteSVM catch-up test (services/header-relay
  tests/catchup.rs, one slot per round, one block per 4 slots): the sequential relay
  is 15 blocks behind after 96 slots and growing; parallel 8 drains a 30-block backlog
  in 67 slots and then stays within 3 blocks. A `lag {...}` JSON line (node tip, light
  client tip and final, `lag_blocks`, in flight, headers/min) is logged every 10 s;
  `header-relay status` prints `lag_blocks` too.
- Solana clock and `max_future_ms` (found 2026-10-18): `submit_header` refuses a
  header more than `max_future_ms` (chain value 120 s) ahead of the Solana Clock
  sysvar. That is right on real Solana, whose clock tracks wall time. A
  `solana-test-validator --ticks-per-slot 8` (50 ms nominal slots) running 256-366 ms
  slots under load caps its clock at about 125 ms per slot and falls behind wall time
  by about 30 s per minute (measured on the test VPS: 224 s, then 255 s a minute
  later), so the light client can only follow as fast as that clock (about 19 of the
  demo's 40 headers/min): the second reason the VPS relay fell behind once it was
  fast. Not a protocol change: on a localnet raise the limit (`header-relay set-params
  --max-future-ms`, admin; the VPS stack runs 24 h since 2026-10-18 16:48 UTC) or run
  the validator at its default ticks per slot. The relay logs each wait and counts
  `clock_waits`.
- Relay cost on real Solana (per header, measured CU, 5,000 lamports per signature):
  a fallback header is 37 signatures = 185,000 lamports base (0.000185 SOL) plus
  priority fees on about 12.2M CU of requested limit (was 20.6M when every tx asked
  for 1.4M): +12,200 lamports at 1,000 micro-lamports/CU, +122,000 at 10,000. At the
  60 s target that is 1,440 headers/day: 0.27 SOL/day base, about 0.44 SOL/day at
  10,000 micro-lamports/CU, roughly 100-160 SOL a year for one relay, all paid by the
  relayer (reimbursable from the pool via `relay_reward`). Locked but refundable: 0.49
  SOL per work account (69,632 bytes; `--parallel 8` = 3.9 SOL, `close-works` returns
  it) and 0.0042 SOL per live header record (window + K + retain_final). Throughput is
  not the constraint at 60 s: 37 txs/min = 0.6 tx/s and 10M CU/min on a chain with
  60M CU per 400 ms block; latency is: at about 1 s per confirmed round on mainnet one
  header takes about 12 s, so a sequential relay keeps up at 60 s (5x headroom) but
  drains a backlog at only about 4 headers/min net, a day of backlog in about 6 hours;
  `--parallel 4` (about 2 SOL refundable rent) drains it in about 1.3 hours. Biggest
  remaining saving, not built: for fallback blocks A and B are derived from the header
  (pow_digest), so the program could rebuild the opened rows itself and skip the
  24.7 KB proof upload and the openings (about 27 of 37 txs and 70% of the fees); job
  blocks still need the upload. The demo's 1.5 s blocks (40x the spec rate) need
  40 headers/min sustained; that is what the test VPS stack exercises.
- Job commitments: `register_job` proves the job's posting tx (PostJob or order
  BridgeUnlock) by tx_root inclusion in a verified canonical header below the job
  block. No state proof needed (blocks are all-or-nothing).
- `bridge.release_proof`: Commonware MMR proof of the withdrawal entry against
  withdraw_root of a final header; `set_release_modes` = signers, proof or both.
- `storefront.finalize_order_proof` / `expire_order_proof`: inclusion of the
  CompleteJob / ExpireJob tx under a final header's tx_root, plus one ed25519
  precompile re-verify of an (evaluator, signature) pair from that tx. Replaces
  the QMDB state proof.
- Light-client params (incl. fallback m/n, K, WTEMA) mirror the chain's
  AdminConfig at init and via `set_params`; any chain AdminConfig change must be
  mirrored. Reorgs are limited to about 30 headers per tx (path accounts).
- Final-header accumulator (Agent Credit R5, 2026-10-17): `Accumulator ["acc"]`, a
  SHA-256 Merkle mountain range over the hashes of FINAL headers in height order
  (`pearl_verify_sbf::acc`: leaf `sha256(0x00 || block_hash)`, node `sha256(0x01 || l ||
  r)`, root `sha256(0x02 || n u64 || peaks left to right)`; stored: start height, count,
  last hash, the 64 peak slots, the last 128 roots). `init_accumulator` (admin, once, at
  the current final header = leaf 0; `header-relay init` does it); `accumulate`
  (permissionless; header records in height order, each verified, final or canonical at
  or below the floor, child of the last one; about 2.7k CU per header); `close_header`
  refuses a final header not accumulated yet, so every final header from the start stays
  provable for good. `prove_header(header, count, proof)`: short proof (siblings to the
  peak, checked against the stored peak, `count` = current; 608 bytes at 2^20 leaves, so
  it fits a tx at any realistic size) or full proof (siblings + other peaks, against one
  of the last 128 roots); return data `block_hash || height`; 17.0k-20.8k CU at 2^20-1
  leaves. `restore_header` re-creates a closed final `HeaderRecord` from such a proof
  (25.9k CU; `_pad[0] = 1` marks it restored), so `read_final_header` readers (bridge,
  storefront, ac_pearl) work on it unchanged. `verify_accumulated(acc_data, header,
  count, proof)` is the raw reader for other programs (owner + PDA checked by the
  caller). header-relay: `run` accumulates before closing; `acc-proof HEIGHT [--restore]`.
- Historical withdrawal proofs (16a8842, fe1f88f): node RPC `getWithdrawalProof
  {nonce, at_height}` proves the entry against the `withdraw_root` of the FINAL header at
  `at_height` (Commonware keyless `historical_proof` at the log size that block committed;
  the node keeps a per-height log-size index, rebuilt from the log on restart). A light
  client therefore need not hold a header carrying the node's CURRENT root: header-relay
  `--settle` proves each withdrawal at the light client's own final height (block hash
  checked against the node's), falling back to the current-root proof on an older node.
  Refused below the withdrawal's block or above the node's final height.
### 5.4 Inference gateway and its public API (Cadre, 2026-10-18; API v1, keep stable)

`services/inference-gateway` (`cadre-gateway`, port 10664) sits in front of storefront-x402
and next to a model worker (`tools/cadre/worker.py`, port 10665, the int7 model runner). The
gateway's key is the server of its tickets (it posts their segments, delivers, receives W).
Other services (cadre-proxy, services/cadre-api) build against the API below; changes are
additive only.
services/cadre-proxy (127.0.0.1:10695) is such a client: a local OpenAI-compatible proxy with
its own wallet (key in a 0600 file, never printed), wallet-signature or API-key auth to this
API, per request / day / total spend caps booked from the receipts, SSE pass-through, and
config snippets for Hermes, OpenClaw, Kilo and Cline (services/cadre-proxy/README.md).

Payment model (owner decision 2026-10-18): PREPAID BALANCE. A buyer tops up once (SOL over
x402 through the storefront: one Order = one balance ticket of R rows, default 4096), then
sends wallet-authenticated requests that draw rows from it with no Solana or chain tx on the
request path. The gateway posts the served rows as segment rounds every period, and when the
balance is used up, near its deadline or closed by the buyer, delivers the receipts and
completes the ticket: one order and one CompleteJob per balance, W paid pro rata to rows
used, the rest of W and P refunded on the chain (4.5). Any open ticket the wallet posted on
the chain itself (PostJob, ticket form, server = this gateway) counts as balance too. A
per-request x402 payment (one ticket per request) stays available but is not the main path.

Authentication (requests that draw on a balance), either:
- wallet signature headers: `X-Cadre-Wallet: <base58 ed25519 pubkey>`, `X-Cadre-Timestamp:
  <unix ms>`, `X-Cadre-Signature: <hex 64 bytes>`, a raw ed25519 signature over
  `"cadre-auth-v1\n" + METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + hex(sha256(body))`.
  The timestamp must be within 120 s; a signature is accepted once.
- `Authorization: Bearer cadre_sk_<hex>`: an API key bound to a wallet, made by
  `POST /v1/keys` (wallet-signed), revoked by `DELETE /v1/keys/<key_id>` (wallet-signed).
  This is what plain OpenAI clients send.

Endpoints (JSON; errors are `{"error": {"message", "type", "code"}}` with a 4xx/5xx status):
| Method, path | Auth | What |
|---|---|---|
| `GET /health` | none | `{ok, server, model_id, model}` |
| `GET /v1/models` | none | OpenAI list; each model `{id, object: "model", owned_by: "cadre", context_length, cadre: {model_id, recipe, quantization, matrices, macs_per_row, pricing: {lamports_per_row, prompt_lamports_per_token, completion_lamports_per_token, basis, default: true}}}`; `id` is the model's registry name (`llama-3.2-1b-w7a7`) |
| `POST /v1/balance/topup` | x402 (`X-PAYMENT`) | body `{rows?}`; no payment: 402 with the storefront's requirements; paid: 201 `{ticket_id, order, rows, wallet}` (wallet = the x402 payer = the ticket's poster) |
| `GET /v1/balance` | wallet or key, or `?wallet=` | `{wallet, tickets: [{ticket_id, order, status, rows, rows_reserved, rows_used_on_chain, rows_free, lamports_per_row, deadline_height}], rows_free}` |
| `GET /v1/prepaid/<wallet>` | none | the same as `GET /v1/balance?wallet=` (balances are public chain state) |
| `GET /v1/receipts?wallet=<wallet>` | none | the wallet's newest receipts (up to 200) |
| `GET /v1/receipts/<request_id>` | none | alias of `/v1/requests/<request_id>` |
| `POST /v1/balance/close` | wallet or key | settle and complete the wallet's open tickets now (refund of the unused rows on the chain) |
| `POST /v1/keys` | wallet | `{key_id, api_key}` (the key is shown once) |
| `DELETE /v1/keys/<key_id>` | wallet | revoke |
| `POST /v1/chat/completions` | wallet or key; or x402 per request | OpenAI chat completions: `model`, `messages`, `max_tokens` / `max_completion_tokens`, `stream`, `stop`, `tools` / `tool_choice` (rendered by the model's chat template; tool calls are parsed from the output when the model emits them, best effort), `temperature` / `top_p` accepted but decoding is greedy (deterministic, documented in each response); returns `chat.completion` (or SSE `chat.completion.chunk` events ending in `data: [DONE]`, usage and receipt in the last chunk) |
| `POST /v1/completions` | same | OpenAI text completions (`prompt`) |
| `GET /v1/requests/<request_id>` | none | the receipt plus its settlement: round, segment ids, blocks mined on them, ticket status, Solana order status |
| `GET /v1/stats` | none | measured figures of this gateway: requests, time to first token, decode tokens/s (recent and totals) |

Receipt (every inference response): headers `X-Cadre-Request` (request id), `X-Cadre-Ticket`,
`X-Cadre-Order` (storefront order nonce, or empty for a chain-native ticket),
`X-Cadre-Rows`, `X-Cadre-Charged-Lamports`, `X-Cadre-Receipt` (status URL) and
`X-Cadre-Explorer` (explorer link to the ticket); the body carries the same as
`cadre: {request_id, ticket_id, order, rows, charged_lamports, lamports_per_row, round: null
until posted, status_url, explorer_url, decoding: "greedy"}`. `usage` is OpenAI's
(`prompt_tokens`, `completion_tokens`, `total_tokens`).

Pricing: one row = one token through every registered matrix. `lamports_per_row` = the
balance's x402 price / R (the storefront knobs over `2 x macs_per_row x R`, 5.1/4.5), so
prompt and completion tokens cost the same. A DEFAULT, not a market price (owner decision
open); no USD figure is shown because the stack has no price source.

Settlement rounds: every `--round-secs` (default 60) the gateway posts one round per ticket
with served rows (A blobs + PostSegments); it delivers and completes a ticket when its free
rows drop below `--min-free-rows` (default 64), when it is within `--close-margin-blocks`
(default 50) of its deadline, or on `/v1/balance/close`.

Evaluator rule for tickets (M1, services/attester): the response blob must hash to the
ticket's `result_hash`, name the ticket's model and `request_hash`; its rounds must match the
chain's (count, m), each round's receipts must fit in its m; every round must have one
segment for every registered matrix; for each segment the attester loads A (blob) and B (its
copy of the model, checked against `hash_b`), computes `C = A B` and requires
`blake3_keyed(C, job_key) == c_hash`. Then it signs the result message. Not checked in M1: the
non-GEMM steps (norms, attention, sampling) that connect one segment's C to the next segment's
A, so a server could commit correct GEMMs over wrong activations. Follow-up: replay by the
evaluator (greedy decoding is deterministic on one platform) or a TOPLOC-style activation
check (research/agents/REPUTATION.md).

## 6. Node architecture (Rust, Commonware 2026.9.0 pinned, Rust 1.95)

| Crate | Uses (commonware-*) | Job |
|---|---|---|
| agent-types | codec, cryptography | Header, Block, Tx, encodings above |
| pearl-job (exists) | | Proofs; add the `pow_digest` seed variant |
| state | storage (qmdb current, keyless) | Accounts, jobs, agents, escrow, withdrawal log; batch per branch for shallow reorgs |
| executor | | Deterministic all-or-nothing block application (Constantinople pattern) |
| consensus-pow | runtime | Block tree, cumulative work, WTEMA, finality depth, reorgs |
| sync | resolver, archive (prunable) | Headers-first backfill keyed by digest; PoW check before download |
| net | stream | Open-membership p2p (bouncer admits any key), address gossip, connection caps; implements the p2p traits so broadcast/resolver work |
| mempool | | Fee-priority pool, nonce ordering |
| rpc | | HTTP JSON + websocket: getBlockTemplate, submitBlock, sendTx, getAccount, getJob, getBlock, subscribe |
| bin/agent-node | all | The node |

Miners talk to the node over RPC (getBlockTemplate / submitBlock) and do not need
to be p2p peers. M1 may use Commonware `discovery` with a static peer list for the
local multi-node devnet; `net` (open membership) is required before a public net.

Ports (localnet): node RPC 10630 (+ws 10631), local devnet nodes p2p 10640-10649
and RPC 10630-10639, relayer 10660, attester 10661, storefront-x402 10662.

## 7. Milestones

**M1: our chain runs, Solana storefront works (localnet)**
1. Node: types, state, executor, consensus-pow, sync, mempool, rpc; 3-node local
   devnet; genesis with the escrow allocation; reorg tests in the deterministic
   runtime.
2. Miner as block producer (CPU, low difficulty) on job and fallback blocks.
3. Bridge: relayer rewired to the node RPC (attested both ways, K-deep).
4. Solana: `storefront` program + x402 service; buy a job from Solana, mined on our
   chain, result finalized back on Solana.
5. Wallet extension (Chrome, MV3) for both chains: balances, send, bridge, agents,
   jobs, rewards, and a dApp provider.
6. `scripts/demo.sh`: launch, buyback, fund the pool, storefront order, mining,
   delivery, finalize on Solana, bridge out; invariant at every step.

**M2:** Solana light client + proof-based release and finalize; open-membership
`net`; data availability for job matrices and results; mining pools (shares).

**M3:** public testnet, audits, mainnet.

## 8. Low-rank job matrices (analysed 2026-10-17)

See research/NOTES-rank-penalty.md. Because the jackpot hashes every rank-block
checkpoint, zero or low-rank A, B do not make job blocks cheap: the best known
attack costs 0.82x an honest job (rank bound 0.818), the same edge Pearl mainnet
miners have. Pearl's rank penalty is ported (noise rank >= 128, penalized bound).
No consensus check on A, B. Optional evaluator policy: refuse jobs that fail a
randomized rank test. Self-dealing still loses (1 - alpha) x P per job.

## 9. Open questions for the owner
- Dev buy size (sets the starting reserve).
- Storefront buyback: share 10% and destination mining reserve DECIDED 2026-10-18; still
  open: per-call cap (1 SOL), min buy (0.001 SOL), interval (600 s), cranker allowlist
  (5.1 "Buyback").
- Block time (60 s default), K (30), alpha (0.8), emission (7 ppm per block).
- Name and ticker: Cadre / CADRE (owner, 2026-10-18; was Lode).
- Inference (4.5, 5.4): price per request (default = the matmul knobs over 2 x macs_per_row x R),
  which models to register (default: Llama-3.2-1B for the demo), a model registration
  deposit (default none, tx fee only), blocks per segment (default 1).
