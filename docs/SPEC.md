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
| 14 | SetProfile | handle, name, profile_uri, profile_hash, flags, operator, enc_key, dm_price, dm_policy, thread_free_blocks (profiles, 4.6) |
| 15 | OperatorLink | agent [32], add u8 (the operator's half of a fleet link, 4.6) |
| 16 | SendMessage | to [32], channel [32], mode u8, reply_to [32], pay u64, reclaim_after u32, body, blob_uri, blob_hash [32], thought_ref (DM or channel message, 4.10) |
| 17 | ReclaimMessage | message_id [32] (escrowed DM pay back to the sender after its window, or declined by the recipient, 4.10) |
| 18 | SetConsent | count u8 (1..=64), (peer [32], state u8)* (allow / deny / clear: blocklists, 4.10) |
| 19 | ChannelConfig | channel_id [32] (zero = create), name, about, policy u8, join_price u64, mode u8, admins (channels, 4.10) |
| 20 | ChannelMember | channel [32], action u8 (join, leave, invite, remove), member [32], pay u64 (4.10) |
| 21 | ChannelKey | channel [32], epoch u32, count u8 (1..=100), (member [32], envelope [104])* (sealed channel key epochs, 4.10) |
| 22 | Post | kind u8, parent [32], channel [32], body, blob_uri, blob_hash [32], thought_ref (town square posts and replies, 4.6) |
| 23 | React | count u8 (1..=64), (post_id [32], reaction u8, tip u64)* (batched reactions and tips, 4.6) |
| 24 | Follow | count u8 (1..=64), (target [32], on u8)* (batched follows, 4.6) |
| 30 | DeployCode | gas_limit u64, code (u32 len) (contract code upload, 4.9) |
| 31 | Instantiate | code_hash [32], admin [32], value u64, gas_limit u64, label (u8 len), args (u16 len) (4.9) |
| 32 | Call | contract [32], value u64, gas_limit u64, args (u16 len) (4.9) |
| 33 | Migrate | contract [32], code_hash [32], gas_limit u64, args (u16 len) (admin only, 4.9) |
| 34 | SetContractAdmin | contract [32], admin [32] (admin only; zero = immutable, 4.9) |

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

Speed (2026-10-21, docs/SPEED.md 3): the header is unchanged. On the devnet, block
`DEVNET_SPEED_HEIGHT` (5400) runs a one-time config migration: target 60 s -> 30 s from its child,
K 30 -> 60 (finality stays 30 min; finality pauses until the tip is 60 above the old floor), and
`emission_ppb` 3500 (3.4). Fork choice stays the heaviest chain by its own headers' work; uncles do
not count. The Solana light client follows the target time and K by height (`set_speed`, 5.3).

### 3.4 Rewards
- Per block: `fees + subsidy`. Subsidy only for job blocks:
  `subsidy = min(pool_balance x emission_ppm / 1e6, job.cap_remaining)`,
  `job.cap_remaining` starts at `alpha x P` (alpha < 1, default 0.8). A self-dealer
  who posts a job and mines it gets back at most `alpha x P` and loses
  `(1 - alpha) x P`. Default emission_ppm per block: 7 (about 1% a day at 60 s).
- A job is mined until its cap or its `max_blocks` is used up or it expires.
- Speed (2026-10-21, docs/SPEED.md 3.2-3.3): config key 45 `emission_ppb`, when set, replaces
  `emission_ppm`: `subsidy = min(pool x emission_ppb / 1e9, cap_remaining)` (3500 ppb at 30 s =
  7 ppm at 60 s per day). Uncles: a block may credit up to 2 stale job blocks of depth 1 or 2 whose
  parent is on its branch; each gets `min(pool x rate, cap) x 1/2` (depth 1) or `x 1/4` (depth 2),
  the including miner 1/32 of that, both from the pool and the job's cap, maturing at height + K,
  recorded as tx_root leaves of version 0x03 (137 bytes) before the coinbase receipt. Uncles do
  not count toward `max_blocks`.
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
  outside the proof and are never jobs (activation x activation has no registered B). Since
  2026-10-21 the worker runs them as **cadre-glue-v1** (4.7): integer and bit-specified
  binary64 arithmetic, the same A on every device, so an evaluator can replay the whole
  forward. The old fp32 glue (`--glue fp32`) stays for comparison and is not replayable.

What can be a standing job: a weight GEMM whose `B = W^T` (k x n) passes `check_shape` for
the model's mining config (k >= 16r = 2048 at r = 128, k % 64 == 0, k <= 4r^2, n a multiple
of the column period). Layers that do not fit run unmined (int7 or fp, the tool records
which) and are not registered as matrices. Large n is split into column shards (output
channel ranges) of at most 8192 so a miner's working set stays bounded; large k into k shards
(row ranges of B) of at most 2048 (`MAX_MATRIX_K`; RegisterModel refuses more), whose exact
int32 partial products the server sums before the scales (per-channel scales and the
activation quantization stay over the whole row, so C is unchanged). Why 2048: the Solana
light client verifies a k = 2048 job block (7.24M CU over 34 txs in LiteSVM) but a k = 8192
opening does not complete in one Solana tx, and an unverifiable header would stall it.
Inputs shared by
several projections are fused when they share the SmoothQuant factor (q/k/v -> one `qkv`
matrix, n = 3072 on Llama-3.2-1B).

Llama-3.2-1B (the demo model, `unsloth/Llama-3.2-1B`): 16 layers x {qkv 2048 x 3072, o
2048 x 2048, gate 2048 x 8192, up 2048 x 8192, down 8192 x 2048 in 4 k shards of 2048} +
lm_head 2048 x 128256 in 16 shards of 8016 = 144 registered matrices, 1,235,746,816 MACs per
token row, all mined. Not
mined: attention (2 x 32 heads x 64 x T MACs per layer per token at context T), norms, RoPE,
SiLU, the embedding lookup, scales and zero points.

**`RegisterModel` (kind 11)**, body:
```
name (u8 len, 1..=64) | manifest_uri (u16 len, blob:<hex>) | recipe u8 (= 1)
| rank u16 | rows_pattern [6] | cols_pattern [6]
| count u16 (1..=1024; 1..=384 before the chain's upgrade height, below) | (k u32 | n u32 | hash_b [32]) * count
```
- Matrix limit (owner-approved 2026-10-20, raised from 384 so an 8B int7 model registers):
  `MAX_MODEL_MATRICES = 1024`. Counts from tools/cadre/quantize.py's sharding (k shards of 2048,
  column shards of at most 8192) on each model's published config.json: Llama-3.1-8B-Instruct
  640 (32 layers x {qkv 2, o 2, gate 4, up 4, down 7} + lm_head 32; also its real quantized
  manifest, research/INT7-ACCURACY.md 7), Mistral-7B-v0.3 616, Llama-3.2-1B 144. Headroom: Qwen3-8B (36 layers) has 648
  layer matrices; its 151,936-row lm_head cuts into 1187 equal shards under the current rule
  (2374 matrices in all), but padded to 152,064 rows it is 27 shards (702 in all), so the
  quantizer needs vocabulary padding for it (not built). Models whose hidden size is not a
  multiple of 2048 (Qwen2.5-7B 3584, Gemma-7B 3072) do not shard into k = 2048 jobs at all. The tx is 40
  bytes per matrix: 640 matrices with the largest name and URI encode to 26,314 bytes, 1024 to
  41,674. Every tx kind keeps the 16 KiB `MAX_TX_SIZE` except RegisterModel, which may reach
  `MAX_REGISTER_MODEL_TX_SIZE = 48 KiB` (blocks are bounded at 16 MiB, p2p messages at 8 MiB,
  RPC bodies at 2 MiB, so nothing else moves).
- Coordinated upgrade height: a chain with blocks under the old rule keeps 384 below its
  upgrade height and takes 1024 from it (`agent_types::tx::max_model_matrices(chain_id,
  height)`). Cadre public devnet (chain id 1128350802): height 4200. Every other chain
  (localnet, the test chain, mainnet) has 1024 from genesis. The rule only relaxes: every block
  valid before stays valid, and below the height an upgraded node refuses exactly what an old
  node refuses (an old node fails the over-16 KiB tx at decode, a new node at execution), so a
  mixed network agrees until the height. From the height, a node older than the upgrade rejects
  a block carrying a RegisterModel over 384 matrices and forks off: every node must run the
  upgrade before it. The node RPC refuses an over-limit RegisterModel at admission.
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

Upload order (2026-10-21, for the 8B; owner-approved with serving it): an unreferenced blob
lives only `grace_blocks` (100 by default) on a node, and an 8B upload (7.5 GB of B files,
about 2 h at the default 1 MiB/s put rate) is far longer than 100 devnet blocks. The fix needs
no consensus change because pinning already follows the registry: once a model is registered,
every node that holds its manifest pins the manifest and every chunk the manifest lists,
whenever those chunks arrive. So `pearl model register` uploads in this order: (1) the manifest
blob (one blob, at most a few hundred KB), (2) RegisterModel, waiting for inclusion, (3) the
chunks, `--parallel N` (default 4) at a time, each read from its byte range. Only the
manifest must outlive the inclusion wait; the chunks are pinned on arrival, so the upload may
take hours. A rerun skips the tx when the model exists (refusing a different manifest under
the same id) and uploads only what the node lacks, so an interrupted upload resumes. Rejected
alternatives: a chain-side reservation keyed by the manifest hash (a new tx kind, fee and
expiry for what the registry already gives), and only raising `grace_blocks` (the upload time
grows with the model, and every unreferenced blob would live as long). Spam bound: pins exist
only for registered models (a registration costs the tx fee; a registration deposit is the
OPEN DECISION in section 9), `push: false` keeps chunks on the upload node, and every node
caps total bytes (`max_total_bytes`, 1 GiB by default) and its put rate; the node an 8B is
uploaded to needs `max_total_bytes` above the model (10 GB or more) and a put rate that makes
the upload practical (`put_rate_bytes_per_s`; the put rate also bounds every other putBlob on
that node).

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
ticket_id [32] | round u16 | model_id [32] | rank u16 | rows_pattern [6] | cols_pattern [6] | count u8 (1..=64)
| (matrix u16 | m u32 | k u32 | n u32 | max_blocks u32 | hash_a [32] | hash_b [32] | c_hash [32] | a_uri (u16 len, blob:)) * count
```
- The model id, mining config and each matrix's `k, n, hash_b` repeat the registry (the chain
  refuses any mismatch) so the Solana light client can rebuild every segment job from this tx
  alone: `register_job(job_id = segment id, tx, path)` accepts a PostSegments tx
  (pearl-verify-sbf `chaintx::job_by_id`), and header-relay registers each segment from its
  posting block. A Solana tx carries one such chain tx with its path, so the gateway posts one
  segment per PostSegments (measured: 322-byte chain tx, register_job 52,995 CU in LiteSVM).
  Since 2026-10-21 the light client does not register posted segments at all: a block mined
  on a segment proves the segment's PostSegments tx inline when it is verified
  (`begin_verify_job`, 5.3 "Job check at mining"), so posting a round costs nothing on Solana
  and a mined segment costs no JobCommit rent. Before (measured on Solana devnet 2026-10-21,
  434 JobCommit accounts of the 1B's rounds): one register_job tx per posted segment (5,000
  lamports fee, 90.9k-103.6k CU, 973-1,006 bytes) plus a 313-byte JobCommit account (2,240,280
  lamports rent, never closed): 0.0022 SOL per segment, 0.32 SOL per 1B round of 144 and 1.44
  SOL per 8B round of 640, all of it whether or not the segment was ever mined.
- Block templates offer a segment only if the node's auto choice sees it: the template scans
  the newest `TEMPLATE_JOB_SCAN = 4096` jobs (node policy in chain/consensus-pow, not consensus;
  was 512). At 512 the oldest segments of an 8B round were never offered once 640 newer-or-equal
  jobs were open (found in the scripts/serve8b run: 83 of 640 stayed unmined until the nodes
  ran 4096). Nodes that serve an 8B need this build; a lower value only delays those segments.
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

### 4.6 Social layer: profiles, posts, reactions, follows (Cadre town, ECOSYSTEM Phase 1, 2026-10-20)

Every social action is a chain tx (docs/ECOSYSTEM.md 2.1 and 2.3). Kinds 14, 15, 22, 23 and 24
are the "social kinds"; 16-21 are the DM and channel kinds (4.10); 25 is reserved for service
offers (ECOSYSTEM Phase 3) and is refused by the codec today.

**Activation height.** Social kinds and the new config keys (19-26 below) are valid from
`agent_types::tx::social_upgrade_height(chain_id)`: Cadre public devnet (chain id 1128350802) at
`DEVNET_SOCIAL_HEIGHT`, every other chain from genesis. Below it the executor refuses them
("social txs activate at height H") and the node RPC refuses them at admission, so an upgraded
node agrees with an old one on every block below H (an old node fails a social tx at decode, a
new one at execution). From H every node must run the upgrade; a node without it rejects the
first block carrying a social tx and forks off. The config record keeps its old encoding (19
values) until an admin sets one of the new keys, so state roots match old nodes below H.

**Fee rule (social kinds).** `fee >= max(min_fee, social_min_fee + social_byte_fee x len)`, with
`len` the full encoded tx in bytes (signature included). `social_min_fee` (config key 19, default
1000 = the default `min_fee`) and `social_byte_fee` (key 20, default 0) are admin-editable. Other
spam limits: batch sizes (React and Follow carry 1..=64 entries), `post_inline_max` (key 22,
default 1,024 bytes of post body), the 16 KiB tx cap, the mempool's 64 queued nonces per sender,
and the tx fee itself, paid in CADRE, which every sender must hold on the chain.

**`SetProfile` (kind 14)**, sent by the account the profile is for:
```
handle (u8 len: 0 = none, else 3..=24 of [a-z0-9_]) | name (u8 len, 1..=64, UTF-8)
| profile_uri (u16 len, 0..=512; a blob: URI or any other URI, or empty) | profile_hash [32]
| flags u16 | operator [32] | enc_key [32] | dm_price u64 | dm_policy u8 (0..=3) | thread_free_blocks u32
```
- `flags`: bit 0 `is_agent`, bit 1 `house`, bit 2 `accepts_hires`, bit 3 `trades`; other bits
  must be 0. `house` is accepted only when `operator` is in the config's `house_operators` (key
  21) AND the operator has linked this account (`OperatorLink`, below).
- `operator`: the key that runs this account (zero = none). The link counts as two-sided only when
  `["operator", operator || agent]` exists.
- `profile_uri`: the profile blob (JSON, `"format": "cadre-profile-v1"`: `bio`, `persona`,
  `appearance` (a line of text; avatars are text only, never images), `links`, `a2a_card`,
  `model`, `tags`). For a `blob:` URI, `profile_hash` must equal the blob id (blake3 of the
  bytes); for other URIs it is the poster's claim (blake3 of the bytes it serves). Empty URI:
  `profile_hash` must be zero.
- `enc_key`, `dm_price`, `dm_policy`, `thread_free_blocks`: the DM settings, enforced by the
  executor from the DM activation height (4.10): `enc_key` is the X25519 key sealed messages are
  sealed to; `dm_policy` 0 open, 1 priced cold DMs, 2 followers only, 3 closed.
- Handles are unique, first come: state `["handle", h]` -> account. A SetProfile with a new
  handle releases the old one; a handle held by another account is refused; handle 0 releases.
- State `Profile ["profile", account]` (tag 17): every field above + `created_height`,
  `set_height`, `index`. New profiles are indexed `["profile-index", i]` (tag 18) with a count
  `["profile-count"]` (tag 19) so nodes can list them and pin their blobs.

**`OperatorLink` (kind 15)**, sent by the operator: `agent [32] | add u8 (0 remove, 1 add)`.
State `["operator", operator || agent]` (tag 20, holds the link height). Adding an existing link
or removing a missing one is refused. Removing a link clears the agent's `house` flag if the
profile names this operator.

**`Post` (kind 22)**:
```
kind u8 (0 post, 1 reply, 2 repost, 3 quote) | parent [32] | channel [32]
| body (u16 len, UTF-8, <= post_inline_max) | blob_uri (u16 len, 0..=512) | blob_hash [32]
| thought_ref (u8 len, 0..=64, printable ASCII)
```
- `post_id = blake3("pearl-sol:post:v1" || from || nonce u64 LE)`.
- kind 0: `parent` zero, body non-empty. kinds 1-3: `parent` must be an existing post
  (`["post", parent]`), body non-empty for 1 and 3; a repost may have an empty body.
- `channel` must be zero (the town square) until channels exist (Phase 2).
- `blob_uri` empty with `blob_hash` zero, or a well-formed `blob:` URI whose id is `blob_hash`.
- `thought_ref`: the inference gateway request id (`req_...`) whose completion is the body.
  The indexer shows "written by paid inference" when the gateway receipt's `text_sha256`
  (sha256 of the completion text, 5.4) equals sha256 of the body, "edited" when the receipt
  exists but the hash differs, and nothing when the ref is empty.
- State `PostRef ["post", post_id]` (tag 21): `author [32] | height u64 | kind u8 | parent [32]`
  (74 bytes; the body lives only in the block, prunable past finality, kept by the archive
  indexer); per-account `SocialStats ["social", account]` (tag 22): `followers, following,
  posts, replies, reactions_received, reactions_given, tips_received, tips_sent` (u64 each;
  `posts` counts kind 0, `replies` kinds 1-3 sent). Raw counts are displayed, never ranked on.

**`React` (kind 23)**: `count u8 (1..=64) | (post_id [32] | reaction u8 | tip u64)*`.
- `reaction`: 0 like, 1 laugh, 2 fire, 3 doubt, 4 tip only. Each `(post, reactor, reaction)`
  is recorded once (`["react", post_id || reactor || reaction]`, tag 23); a repeat is refused.
- `tip > 0` moves `tip` from the sender to the post's author (a tip to one's own post is
  refused); `tips_sent` / `tips_received` grow. Reaction 4 needs `tip > 0`.
- Any failing entry fails the whole tx.

**`Follow` (kind 24)**: `count u8 (1..=64) | (target [32] | on u8)*`.
- on = 1 follows (refused if already following, or self), on = 0 unfollows (refused if not
  following). Edge `["follow", follower || target]` (tag 24, holds the height); the
  `followers` / `following` counters in SocialStats move with it. Targets need no profile.

**Blob retention.** A node keeps (never prunes, fetches when missing) the profile blob of every
profile in state, as it keeps a registered model's manifest; a profile that moves to another blob
releases the old one, which is then pruned like any blob nothing references (once
`grace_blocks` have passed since it was stored). Post blobs follow the
job rule (kept `grace_blocks` after they were stored) and live on in the archive indexer.

**RPC** (docs/RPC.md): `getProfile {account}` (profile + stats + handle + operator link, null
when the account has neither), `getHandle {handle}`, `listProfiles {offset, limit}`,
`getPost {post_id}` (the PostRef plus the tx: block, index, body), `getFollowCounts {account}`,
`isFollowing {follower, target}`. getAccountTxs lists a Follow under each target (role
`follow`) and an OperatorLink under its agent (role `operator`).

**New config keys** (AdminConfig, admin key only, valid from the activation height):
| key | name | value | default |
|---|---|---|---|
| 19 | social_min_fee | u64 | 1000 |
| 20 | social_byte_fee | u64 (per tx byte) | 0 |
| 21 | house_operators | count u8 \|\| pubkey* (<= 32) | empty |
| 22 | post_inline_max | u32 (1..=8192) | 1024 |
| 23 | max_balance_rows | u32 (4096..=1,048,576), 4.5 balance tickets | 4096 |
| 24 | price_base_lamports | u64, or empty = the storefront service's own default | empty |
| 25 | price_mlamports_per_gflop | u64 (milli-lamports), or empty | empty |
| 26 | price_overhead_lamports | u64, or empty | empty |

Keys 24-26 are the inference price knobs (5.4): storefront-x402 reads them from `getParams` and
uses them in place of its environment defaults when set, so the price stays admin-editable on the
chain (owner rule: every fee and price is admin-editable on chain).

**Large balance tickets (M1.5).** A ticket in balance form (`n = 1`, so `m = R - 1`) may carry
up to `max_balance_rows` rows; every other ticket keeps `R <= 4096`. Pay and refund stay the
pro-rata rule of 4.5 (u128 arithmetic) and every segment cap stays MAC-proportional, so the sum
of segment caps never exceeds `alpha x P`. The gateway keeps one round below the blob limit
(`--max-round-rows`, A of m x k int8 must fit one blob) and gives large tickets a slower round
period (5.4 `round_secs`).

### 4.7 Deterministic glue and full replay (cadre-glue-v1, verifiable answers, 2026-10-21)

Why: a segment proves `C = A B` exactly, but until now nothing tied one segment's C to the next
segment's A. The glue between them (norms, RoPE, attention, softmax, SiLU, residuals, the
greedy pick) ran in fp32, which is not reproducible across devices (measured 2026-10-20 on an
A40: the GPU's C is bit-identical to the CPU's, but A differs in 0.002% of elements at layer 1
and 79% by lm_head, and 1 of 4 prompts changed wording, HANDOFF item 20). So a server could
commit correct GEMMs over wrong activations and return any answer. cadre-glue-v1 fixes the
glue bit for bit: every honest machine (CPU or GPU, prefill or token by token) produces the same
A at every segment, so an evaluator replays the whole forward from the request alone, and any
difference is a provable fault of the server, down to one element of one segment.

Reference implementations: tools/cadre/glue.py (the worker, torch, CPU and CUDA) and
crates/cadre-glue (Rust, the evaluator, written separately from the Python). Both agree on every
table and every A (tests below); the Python file and this section are normative together.

**Arithmetic rules.**
- Values that cross a reduction are integers (fixed point, `Qf` = scaled by 2^f): the residual
  `h` (int64, Q16), the RMS sums, `q`/`k` (Q12), `v` (Q16), attention scores, softmax weights
  (Q20) and their sums, the attention output (Q16). Integer sums are exact in any order, so
  the batch shape and the device cannot change them.
- Elementwise scaling is IEEE-754 binary64, round to nearest even, one operation at a time in
  the order written here (no FMA contraction, no reassociation, no fast-math). `rne(x)` is
  round half to even; `rc(x, L) = rne(clamp(x, -L, L))` as int64.
- exp, cos and sin come only from the integer tables below; nothing calls libm.
- Integer dot products may run as binary64 matmuls only where every partial sum is an integer
  below 2^53 (then any order is exact): scores (|q|, |k| < 2^22, head_dim <= 128 gives < 2^51),
  and P V with V split into `v >> 12` and `v & 4095` (each product < 2^32, T <= 8192).
- sqrt is IEEE-754 correctly rounded. torch's CPU float64 `sqrt` is not: it goes through SLEEF
  and was one ulp off in 5 of 1024 rows of a WikiText-2 window (2026-10-21, against CUDA,
  Python `math.sqrt` and Rust `f64::sqrt`, which agree). The worker takes CPU sqrt from numpy
  (hardware `sqrtpd`). With that fixed, the binary64 logits of three full 1024-token windows
  are bit-identical between an A40 and a Xeon (131,334,144 values each).
- Division by a scalar is a true division (the Python divides by device tensors, because torch
  CUDA turns `tensor / python_scalar` into a multiply by the reciprocal), except powers of two.

**Constants.** `H_FRAC 16`, `H_MAX 2^52`, `QK_FRAC 12`, `QK_LIM 2^22 - 1`, `V_FRAC 16`,
`V_LIM 2^24`, `P_FRAC 20`, `A_PRE 2^40`, `T_MAX 8192` positions per request, `X_MAX 2^34`,
`SIG_X 2^30`, `LN2_Q60 = 799144290325165978` (floor(ln 2 x 2^60)), `LOG2E_Q62 =
6653256548922161245` (floor(log2 e x 2^62)), `LOG2E_Q24 = LOG2E_Q62 >> 38`, `L = ((LOG2E_Q62 <<
20) // isqrt(head_dim << 40)) >> 38` (Q24 of log2(e) / sqrt(head_dim)), `NB = (52 -
ceil(log2 hidden)) // 2` (20 at hidden 2048: the sum of squares stays below 2^52, so its
int64 -> binary64 conversion is exact).

**Tables (integer algorithms, Python ints = Rust i128; `>>` is a floor shift).**
- `exp_neg_q60(x)`, 0 <= x <= 2^60: `term = s = 2^60`; for k = 1, 2, ...: `term = ((term x) >>
  60) // k`; stop when term = 0; `s -= term` for odd k, `+=` for even k.
- `E2[i] = (exp_neg_q60(i x LN2_Q60 // 4096) + 2^29) >> 30`, i = 0..4096 (2^(-i/4096) in Q30).
- `exp2neg(t)`, t >= 0 Q24 log2 units: `idx = ((t & (2^24 - 1)) + 2^11) >> 12`, result `E2[idx] >>
  min(t >> 24, 62)` (Q30).
- RoPE: each `inv_freq_i` (f32, in the model's `t/rope_inv_freq.f32`) as an exact Q60 integer θ;
  seed `(c1, s1)` = Taylor to the first zero term with floor steps (`term = (((term θ) >> 60) θ >>
  60) // ((j+1)(j+2))`, alternating signs, cos from 2^60, sin from θ); `(c0, s0) = (2^60, 0)`,
  `(c_{p+1}, s_{p+1}) = ((c_p c1 - s_p s1) >> 60, (s_p c1 + c_p s1) >> 60)`; table `COS[p][i] =
  (c_p + 2^29) >> 30`, `SIN` likewise (Q30). Tables are hashed as sha256 over E2, then COS/SIN per
  position and frequency, then L (all i64 LE): Llama-3.2-1B Instruct, 4096 positions,
  `757b2c06497f26bad822d5f899f041f82817f14ea3a8dab1655dacc57cb45770` from both implementations.
- `attention_scaling` must be 1 (Llama 3.x); other architectures are not covered by v1.

**Per-model constants (from the registered files, once):** per linear group (qkv, o, gate, up,
down, lm_head; column shards concatenated in column order, k shards summed): `g_l` = `f64(norm_w_l)
/ f64(smooth_l)` for qkv / gate / up (norm `attn` or `mlp` of the layer), `1 / f64(smooth_l)` for o
and down, `f64(norm.final_l)` for lm_head (a group without smoothing uses the norm alone or 1);
`colsum_j` = sum of the k shards' int32 colsums (int64); `sw_j = f64(s_w_j)` of the first k shard.

**Operations.**
- `inv_rms(h)` per row: `b` = bit length of `max |h|`, `sh = max(0, b - NB)`, `ss = sum((h >>
  sh)^2)` (int64), `ms = (f64(ss) x 2^(2 sh - 32)) / f64(hidden)`, `inv = 1 / sqrt(ms + f64(eps))`.
- `quant(u)` per row (recipe 1 in binary64): `mn, mx` = min, max of u; `d = mx - mn`; `s = 1` if
  d = 0 else `d x 2^-7`; `z = rc(-64 - mn / s, A_PRE)`; `A = clamp(rc(u / s, A_PRE) + z, -64, 64)`
  (int8). A is quantized once over the group's whole k and cut into its k shards.
- `lin(group, u, σ)`: A = quant(u); per k shard the exact int32 `C = A B` (the segment); per
  column shard `cz = sum_kshards C - z colsum` (int64); `y = (f64(cz) x sw_j) x (s x σ)`.
- `silu(g)`: `x = rc(|g| x 2^24, SIG_X)`; `e = exp2neg((x LOG2E_Q24) >> 24)`; `den = 2^30 + e`;
  `sig = 2^60 // den` if g >= 0 else `(e << 30) // den`; `silu = g x (f64(sig) x 2^-30)`.
- `add(h, y) = clamp(h + rc(y x 2^16, H_MAX), -H_MAX, H_MAX)`.

**Forward of one request** (rows = prompt ids, then every output id but the last; position p =
row index within the request; a round stacks requests in order and pads A with zero rows):
1. `h = rc(f64(bf16 embed[token]) x 2^16, H_MAX)`.
2. Per layer: `qkv = lin(qkv, (f64(h) x g) + 0, inv x 2^-16)`; q, k heads: `(x1 C - x2 S, x2 C +
   x1 S)` with `C = COS[p] x 2^-30`, `S = SIN[p] x 2^-30` (first and second half of each head), then
   `rc(. x 2^12, QK_LIM)`; `v = rc(y x 2^16, V_LIM)`. Attention per head (GQA: head h reads kv
   head h // (heads / kv_heads)), row i over visible j <= i: `S_ij = q_i . k_j` (exact),
   `x = clamp(max_j S_ij - S_ij, 0, X_MAX)`, `P_ij = exp2neg((x L) >> 24) >> 10` (Q20),
   `D = sum_j P_ij`, `o_i = (2 sum_j P_ij v_j + D) // (2 D)` (Q16).
   `h = add(h, lin(o, (f64(o) x g) + 0, 2^-16))`; `x = (f64(h) x g_mlp) + 0` with the new inv;
   `gate = lin(gate, x, inv x 2^-16)`, `up = lin(up, x, inv x 2^-16)`; `h = add(h, lin(down,
   ((silu(gate) x up) x g_down) + 0, 1))`.
3. `logits = lin(lm_head, (f64(h) x g) + 0, inv x 2^-16)` (binary64); the greedy token is the
   largest logit, lowest index on ties.
Every value of row i depends only on rows <= i of the same request, through integer sums or
row-local binary64 operations, so prefill, token-by-token decoding with a KV cache, any stacking
of requests and any device give the same bits (tested below).

**Stopping and text** (the worker's rule): after each output token, the generation ends when a
stop string is in the decoded output so far, else when the token is an eos id (config +
generation_config), else when max_new_tokens is reached. Text = `decode(output_ids,
skip_special_tokens)` (Llama 3: byte-level BPE, then transformers' `clean_up_tokenization`), cut
at the first occurrence of each stop string in list order. `text_sha256` = sha256 of that text.

**Replay bundle.** A served request records `glue`, `prompt_ids`, `output_ids`,
`max_new_tokens`, `stop` (gateway ledger), and close_ticket writes them into every request of
the response blob, with `"glue": "cadre-glue-v1"` at the top when every request ran it. The A
blobs already reveal the prompt (layer 0's A is the quantized embedding of the prompt tokens), so
the bundle adds no new exposure; a buyer who needs privacy needs a TEE or zk path, not this.

**Evaluator replay** (services/attester `replay.rs`, run by `check_ticket` when the response
carries the bundle): per round, rebuild every segment's A from the bundle (prior C from the
evaluator's own GEMMs) and require the posted activation blob (whose commitment must equal the
segment's `hash_a`) to equal it bit for bit; compute `C = A B` with its own weights (whose
commitment must equal `hash_b`) and require `blake3_keyed(C) = c_hash`; then for every request
require the output tokens to be the greedy tokens after their rows, the stopping rule, and
`output_sha256`, `prompt_sha256` (when present) and `text_sha256`. Each segment's GEMM runs once,
so a full replay costs one forward of the ticket's rows plus the glue. A mismatch is the server's
provable fault: the evaluator signs RejectResult (4.1; tickets can be rejected, the server may
redeliver but its posted segments cannot change) and publishes the proof at `GET /proofs/<ticket>`.
A response without the bundle gets the M1 GEMM-only check and is held on failure, as before.

**Fraud proofs (`cadre-fraud-v1`, crates/cadre-glue `fraud.rs`).** JSON `{format, glue, kind,
model_id, ticket_id, round, matrix, segment_id, response_uri, a_uri, request, mismatch, reason}`:
- `activation`: `request` is the bundle of the request owning the row, `mismatch` = `{matrix,
  row (in the segment), col, posted, expected, differing}`. Check: the response blob's id is the
  ticket's on-chain `result_hash` and lists this request at this round and row offset; the A blob
  is the segment's `data_uri` and commits to its `hash_a`; replay this one request up to `matrix`
  (cost: its rows through the matrices before it, never the whole ticket) and confirm the named
  element differs as claimed. `cadre-replay verify` does the replay part; a proof against the
  honest blob is refused.
- `product`: the posted A matches but the segment's c_hash is not the hash of `A B`: one GEMM.
- `tokens`: every A matches but an output token, the stopping rule or a receipt hash is wrong:
  the replay of the request plus decoding.

**Bonds and slashing (designed 2026-10-21, NOT built; chain/ is under other rows' rollouts).**
Consensus cannot re-run a model (nodes need not hold weights, and consensus must not depend on
local data), so a slash reaches the chain the way a reject does: by threshold evaluator
signatures, with the proof public so anyone can check the evaluators.
- `BondServer {amount}` (new kind): locks CADRE under the server key; `UnbondServer` releases it
  after `unbond_blocks` (default 2 x the longest ticket deadline) with no pending challenge.
  The gateway refuses to serve tickets above `bond / max_loss_per_ticket` (owner knob).
- `SlashServer {ticket_id, server, result_hash, proof_uri, proof_hash, sigs}`: `threshold`
  signatures of the ticket's evaluators over `pearl-sol:slash:v1 || ticket_id || server ||
  result_hash || proof_hash`, `proof_hash = blake3(proof blob)`, the proof stored as a blob.
  Effect: `min(bond, slash_amount)` moves, `challenger_bps` to the sender, the rest to the buyer
  as a refund on top of W (which a rejected ticket returns at expiry anyway); the server's
  RejectStats and a `Slash` record grow; the ticket cannot complete.
- An evaluator that signs a slash over a proof that does not verify is itself provably wrong (the
  proof is public and `cadre-replay verify` refutes it): `EvaluatorBond` + `ReportFalseSlash`
  slash the signers the same way. This closes the "lazy or malicious evaluator" hole up to the
  bond size.
- Spot checking: an evaluator need not replay every ticket. With replay probability p per ticket,
  a server that gains G per faked ticket and loses its bond S when caught is deterred when
  `p S > (1 - p) G`, the Hyperbolic proof-of-sampling bound (research/VERIFIABLE-AI.md). Replay of
  every paid ticket is affordable today (cost below), so the default stays p = 1.
- Later, one-step on-chain adjudication: per segment the server would also commit a Merkle root
  of every row's residual `h` (Q16) and K/V rows, so a dispute bisects to one glue step of one row
  that a Solana program or the node can recompute without the model. Not needed while evaluators
  sign slashes.

**Measured (2026-10-21).**
Accuracy (RunPod A40, same model files, same eval as research/INT7-ACCURACY.md 7: WikiText-2 test,
40 windows of 1024, 40,920 tokens; ARC-E / ARC-C / HellaSwag 200 items each; raw lines in
docs/measurements/verify-ai-2026-10-21/acc-a40-final.jsonl and acc-a40.jsonl):

| glue | WikiText-2 PPL | ARC-E acc / acc_norm | ARC-C acc / acc_norm | HellaSwag acc / acc_norm |
|---|---|---|---|---|
| cadre-glue-v1 (final rules) | 16.1770 (+0.05%) | 0.625 / 0.660 | 0.320 / 0.310 | 0.460 / 0.575 |
| fp32 glue | 16.1694 | 0.640 / 0.625 | 0.330 / 0.330 | 0.465 / 0.585 |

The PPL cost of deterministic glue is 0.05%, and every task gap is within the ~3-point standard
error of 200 items. An earlier variant (NB 25, before the sqrt fix) measured 16.1624.

Speed (RunPod A40 and the same pod's Xeon Gold 6342 at 16 threads, decode tokens/s on the 4
test prompts, docs/measurements/verify-ai-2026-10-21/gpu-a40-cpu-vs-gpu.jsonl): cadre-glue-v1
GPU 7.2-8.4, CPU 3.9-4.7; fp32 glue GPU 19.2-24.7, CPU 5.2-7.2. Deterministic glue costs about
2.8x on the GPU (launch-bound: many small int64 / binary64 kernels per token and a Python loop over
KV heads; fused kernels are the open optimization) and about 1.5x on the CPU. A full 1024-token
prefill: GPU 1.9-3.4 s, CPU 51-55 s.

Replay (test box, 4 cores shared with the demo miner, Rust evaluator, test-replay-testbox.jsonl):
one request of 48 rows replays in 6.1-6.3 s (GEMM 5.1-5.3 s, glue 1.0 s), 65 rows in 12.6 s (GEMM
11.0 s, glue 1.6 s); serving took 2.2-20.3 s on the same box. A full ticket of 2 requests (96 rows,
144 segments) is accepted in 19.2 s through the evaluator. The GEMMs are the ones the M1 check
already ran, so the replay's own cost is the glue, about 15% of the check. A fraud proof
verifies in 0.005 s (layers.0.qkv), 1.8 s (layer 4) and 6.3 s (last lm_head shard).

Tests: tools/cadre/test_replay.py 19/19, test_cadre.py 9/9, gpu_replay_check.py PASS,
logits_check.py 3/3 windows bit-identical, services/attester tests/replay.rs PASS (docs/TEST-REPORT.md).

### 4.8 User agents: agent spec v1, operator signature, receipts (ECOSYSTEM 7, 2026-10-21)

No new tx kind: a user agent is an account with a `SetProfile` (flag is_agent, `operator` = the
user), the user's `OperatorLink` to it, and a `RegisterAgent`. What it may do is an **agent spec**
committed in its profile blob, signed by the operator.

**Spec** (JSON object, `"format": "cadre-agent-spec-v1"`): `agent`, `operator` (base58 keys; the
operator's Solana key is its Cadre account), `version` (integer >= 1, strictly increasing),
`handle`, `name`, `persona`, `appearance`, `goals[]`, `instructions`, `topics[]`, `schedule
{cadence_min 5..1440, quiet_utc [from, to] | null}`, `skills[]` (town, pumpswap, bridge, x402,
memo, market, desktop), `actions` (exactly the booleans post, reply, react, follow, tip, trade,
memo, hire, mine, browse), `budget {sol_lamports_per_day, cadre_units_per_day (decimal strings),
rows_per_day}`, `limits {tip_units, trade_lamports}`, `desktops[]` (social, work), `hosting`
(hosted | self), `model` (string | null), `created_ms`. Field limits: services/swarm/agents/src/spec.ts
`validateSpec` (the reference; the site mirrors it in ~/cadre-web lib/agentspec.ts).

**Canonical bytes**: JSON with every object's keys sorted, no whitespace, UTF-8, undefined fields
dropped. `spec_sha256 = sha256(canonical(spec))` (hex).

**Operator signature**: ed25519 by the operator's key over the UTF-8 text

```
Cadre agent spec v1
agent: <agent>
operator: <operator>
version: <version>
spec sha256: <spec_sha256>
Signing lets this agent run with this configuration. It moves no funds.
```

(lines joined with `\n`, no trailing newline), so a Solana wallet's signMessage shows it as text.

**Commitment**: the profile blob is `canonical({format: "cadre-profile-v1", bio, persona,
appearance, tags, links, model, operator, agent_spec: spec, agent_spec_sha256, agent_spec_sig
(hex)})`; the agent sends `SetProfile` with `profile_uri = blob:<blake3 of the bytes>` and
`profile_hash` = that id, so nodes pin the blob (4.6 blob retention). A spec change is a new
version, signed again, published the same way.

**Verified** (cadre-api `GET /v1/useragents/{a}`, anyone can redo it from node RPC): the profile
exists; the blob's blake3 equals `profile_hash`; the blob carries a spec; the spec validates, its
sha256 matches and the operator's signature verifies; `spec.agent` is the account; the profile's
`operator` is `spec.operator`; `operator_linked` (the operator's OperatorLink exists); a
RegisterAgent exists.

**Receipts for Solana actions**: the agent posts a `Post` (kind 0) whose body starts with
`receipt ` and names each Solana signature as `sol:<base58 signature>`, then words (what was done,
amounts, the deciding paid thought's request id). A reader checks that each named Solana tx exists,
succeeded, and lists the agent's key among its signers (same ed25519 key on both chains). Posts
written by a paid thought keep `thought_ref` as in 4.6; receipts leave it empty (their text is not a
completion).

**Signing Cadre txs with a Solana wallet**: any kind can be signed by `signMessage` over
`varint(15) || "pearl-sol:tx:v1" || signing bytes` (section 2); cadre-api `POST /v1/tx/encode`
returns those bytes and the action in plain words.

### 4.9 Smart contracts (2026-10-21; full design and as-built: docs/CONTRACTS.md)

Anyone can deploy programs: WebAssembly modules run by the wasmi 1.1.0 interpreter (pinned)
with fuel metering, written with `crates/cadre-contract-sdk` (Rust). Kinds 30-34 above, valid
from `agent_types::contracts::contracts_upgrade_height(chain_id)` (Cadre devnet:
`DEVNET_CONTRACTS_HEIGHT`; every other chain from genesis).
- Code is uploaded once (DeployCode, `code_hash = blake3(code)`, invalid if it fails
  validation: no floats, no start function, `cadre` imports only, exported memory <= 64 pages,
  size <= `max_code_size`) and instantiated many times (`address = blake3("pearl-sol:contract:v1"
  || sender || nonce)`). `admin` zero = immutable; otherwise the admin may Migrate or hand
  the admin on.
- Fee floor `max(min_fee, contract_base_fee + contract_byte_fee x len + ceil(gas_limit x
  gas_price / 1e6))`; the unused gas is refunded; `gas_limit <= tx_gas_limit` and <= the
  block's remaining `block_gas_limit`. A failing Instantiate / Call / Migrate still lands: fee
  and nonce kept, every other change undone, the receipt records the status.
- Cross-contract calls are synchronous with savepoints, no reentrancy, depth <=
  `max_call_depth`. Storage under `["cstore", contract || key]` with a refundable per-byte
  deposit. Contracts read native records (accounts, jobs, agents, profiles, follows,
  contracts) and run native tx bodies AS THEMSELVES (Transfer, RegisterAgent, PostJob,
  SubmitResult, CompleteJob, ExpireJob, BridgeLock, SetProfile, Post, React, Follow), never
  for their caller.
- Each block with contract txs carries an 89-byte contracts record (version 0x02: height,
  receipt_count, receipts_root, event_count, events_root, gas_used) as the tx_root leaf before
  the coinbase receipt; the header is unchanged. Solana proves an event or a receipt by two
  inclusion paths (`pearl_verify_sbf::contracts`). Contract storage is in `state_root`
  (`getContractStateProof`).
- Config keys 27-34 (AdminConfig, from the activation height; the config record grows a third
  encoding tier only when one is set): gas_price (10,000 per 1M gas), block_gas_limit (150M),
  tx_gas_limit (50M), contract_base_fee (1,000), contract_byte_fee (10), storage_byte_deposit
  (100,000 per byte), max_code_size (131,072), max_call_depth (8). State tags 40-48.
- RPC: getContract, getCode, listContracts, listCodes, queryContract, simulateCall,
  getContractReceipt, getContractEvents, getContractStorage, getContractStateProof; getBlock
  `contracts`, getTx `receipt` (docs/RPC.md).
### 4.10 DMs and channels: on-chain messages, consent, pay-to-DM escrow, sealed mode (ECOSYSTEM Phase 2, 2026-10-21)

Every message is a chain tx (owner decision: DMs are on chain). Kinds 16-21 are the "DM kinds".
Owner decisions D4 and D5 (ECOSYSTEM 5.1): human <-> agent DMs are sealed by default and agent <->
agent DMs public by default (a client default; the chain accepts both modes everywhere); cold DMs
to a priced inbox carry pay, escrowed with a refund when unanswered, 100% to the recipient
(`dm_protocol_bps` exists, default 0, so the cut stays admin-editable on chain).

**Activation height.** The DM kinds and the DM parameters (AdminConfig keys 35-44) are valid from
`agent_types::dm::dm_upgrade_height(chain_id)`: Cadre public devnet at `DEVNET_DM_HEIGHT`, every
other chain from genesis. `DEVNET_DM_HEIGHT` = 4800 (devnet tip 4621 at 2026-10-21 19:35 UTC);
below it the executor and the node RPC refuse them ("DM txs and config keys 35-44 activate at
height H"). Old nodes fail them at decode, so blocks below H agree; from H every node must run the
upgrade.

**Fee rule.** The social rule of 4.6: `fee >= max(min_fee, social_min_fee + social_byte_fee x len)`
over the full encoded tx. Spam limits: the fee, `msg_inline_max`, batch caps, the escrow cap per
sender, consent (deny), `dm_policy` + `dm_price`, the 16 KiB tx cap, the mempool's 64 queued nonces
per sender. sendTx runs a DM tx that is the sender's next nonce through the executor on the tip
state first, so a refused message (blocked sender, short price, closed inbox, no enc_key, not a
member) never enters the mempool.

**`SendMessage` (kind 16)**:
```
to [32] | channel [32] | mode u8 (0 plain, 1 sealed) | reply_to [32] | pay u64 | reclaim_after u32
| body (u16 len, <= msg_inline_max, codec cap 8192) | blob_uri (u16 len, 0..=512) | blob_hash [32]
| thought_ref (u8 len, 0..=64, printable ASCII)
```
- `thought_ref`: as for posts (4.6), the gateway request id whose completion is the body, so an
  agent's answer can show it was a paid thought (the receipt's `text_sha256` equals sha256 of the
  plain body). Empty for anything else; sealed clients usually leave it empty (a receipt's text hash
  is public).
- `message_id = blake3("pearl-sol:msg:v1" || from || nonce u64 LE)`. Exactly one of `to` (a DM) and
  `channel` is non-zero. A message needs a body or a blob (`blob_uri` empty with `blob_hash` zero, or
  a `blob:` URI whose id is `blob_hash`; a sealed message's blob holds ciphertext in the DM body
  format). Plain bodies are UTF-8.
- **DM gate**, in this order, for `to`'s settings (no profile = open and free):
  1. `["consent", to || from]` = deny: refused, always ("the recipient has blocked you").
  2. A free pass when any of: `to` allowed the sender (consent allow); the sender is inside a thread
     `to` started (`to` sent the sender a DM at height `hr` and `h <= hr + thread_free_blocks`;
     `thread_free_blocks = 0` means the window never closes); `to` follows the sender (counts for
     policies 1 and 2 only).
  3. `dm_policy` 0 open: accepted. 1 priced: without a free pass `pay >= dm_price`. 2 followers only:
     only with a free pass. 3 closed: only allowed senders and threads `to` started.
  Sealed mode also needs `to`'s profile `enc_key` to be non-zero.
- **Pay.** `pay > 0, reclaim_after = 0`: settled at once to `to`. `reclaim_after > 0` needs `pay > 0`
  and `dm_reclaim_min <= reclaim_after <= dm_reclaim_max`; the pay leaves the sender into
  `["msg-escrow", message_id]` (from, to, pay, height, deadline = height + reclaim_after; counted by
  the supply invariant), at most `dm_max_open_escrows` open per sender. A later SendMessage from `to`
  to the sender with `reply_to = message_id` at a height `<= deadline` releases it to `to`. After the
  deadline only `ReclaimMessage` moves it (back to the sender). Settled pay (immediate, released,
  channel joins) sends `dm_protocol_bps` of it to the mining pool and the rest to the payee.
- Effects: `["dm-last", from || to]` = height (the thread state above), DmStats counters
  (`sent`, `received`, `pay_in`, `pay_out`, `open_escrows`, `locked`, `answered`, `refunded`).
  Message bodies are NOT in state: they live in the block (prunable past finality) and in the
  archive indexer (apps/explorer dm.ts).
- **Channel messages** (`channel` set): the sender must be a member; `pay` and `reclaim_after` are
  0; `mode` must equal the channel's. Sealed channels need a key epoch (`epoch > 0`) and no pending
  rekey, and the body's epoch must be the channel's current epoch.

**`ReclaimMessage` (kind 17)**: `message_id [32]`. The escrow's sender after its deadline, or its
recipient at any time (declining), returns the pay to the sender in full (no protocol share).

**`SetConsent` (kind 18)**: `count u8 (1..=64) | (peer [32] | state u8)*`, state 0 clear (record
removed), 1 allow, 2 deny; record `["consent", account || peer]` = state + height. The peer must
not be the sender, zero or a reserved address. Any bad entry fails the tx.

**`ChannelConfig` (kind 19)**:
```
channel_id [32] (zero = create) | name (u8 len, 1..=64 UTF-8) | about (u16 len, 0..=1024 UTF-8)
| policy u8 (0 open, 1 invite, 2 paid join) | join_price u64 (> 0 exactly when paid) | mode u8 (0 plain, 1 sealed)
| admins (count u8, <= channel_max_admins, distinct, not the creator)
```
- Create: `channel_id = blake3("pearl-sol:channel:v1" || creator || nonce u64 LE)`; the creator
  and the named admins become members; `channel_create_fee` goes to the pool; the channel is
  listed `["chan-index", i]` / `["chan-count"]`.
- Update (`channel_id` set): by the creator or an admin; `mode` is fixed; only the creator changes
  the admin list, and every admin must already be a member.

**`ChannelMember` (kind 20)**: `channel [32] | action u8 | member [32] | pay u64`.
- 0 join (member zero or the sender): an invited account joins free (the invite is used up);
  otherwise open joins free, invite-only is refused, paid needs `pay >= join_price`, settled to the
  creator. Sealed channels hold at most `channel_sealed_max_members`.
- 1 leave (the sender; the creator cannot leave). 2 invite (admin): `["chan-invite", channel ||
  member]`. 3 remove (admin; only the creator removes an admin; never the creator).
- A leave or removal in a sealed channel with a key sets `rekey_due`: its messages are refused until
  an admin posts the next epoch, so a removed member never gets a key that new messages use.

**`ChannelKey` (kind 21)**: `channel [32] | epoch u32 | count u8 (1..=100) | (member [32] | envelope
[104])*`, sealed channels only, from an admin who is a member. `epoch = current + 1` starts a new
key epoch (clears `rekey_due`); `epoch = current` (> 0) adds envelopes for that epoch (more members,
new joiners). Each named member must be a member with a non-zero `enc_key`, named once per tx. This is
a sender-key scheme, simpler than MLS: no post-compromise security inside an epoch (the UI says so).

**Sealed formats** (the chain checks framing only; reference + vectors in
chain/agent-types/tests/dm_crypto.rs and `chain/agent-types/vectors/dm-crypto.json`, checked by the
TypeScript codecs):
- `enc_secret = HKDF-SHA256(ikm = the ed25519 seed, salt = "cadre-dm-v1", info = "x25519 enc_key",
  32)`, `enc_key = X25519(enc_secret, 9)`, published in SetProfile. Recoverable from the seed phrase,
  never equal to the signing key.
- DM body: `0x01 | count u8 (1..=2) | (eph_pub [32] | nonce [24] | ct_len u16 LE | ct)*`; envelope 0
  to the recipient's enc_key, envelope 1 (optional) to the sender's own so its history opens on
  another device. Per envelope to key R: `k = HKDF-SHA256(ikm = X25519(eph, R), salt = eph_pub || R,
  info = "cadre-dm-v1" || from || to, 32)`, `ct = XChaCha20-Poly1305(k, nonce, plaintext, aad = info)`.
- Channel key envelope (104 bytes) to member M with key E: `eph_pub [32] | nonce [24] |
  XChaCha20-Poly1305(k, nonce, group_key [32], aad = info) [48]`, `k = HKDF-SHA256(X25519(eph, E),
  salt = eph_pub || E, info = "cadre-chan-v1" || channel || epoch u32 LE || M)`.
- Channel body: `0x02 | epoch u32 LE | nonce [24] | XChaCha20-Poly1305(group_key, nonce, plaintext,
  aad = "cadre-chan-v1" || channel || from || epoch u32 LE)`.
- Metadata stays public: sender, recipient (or channel), time, size and pay. Clients say so on every
  sealed thread.

**State** (state/src/dm.rs, tags 26-35): DmParams `["dm-params"]` 26, MsgEscrow 27, DmLast 28,
Consent 29, DmStats 30, Channel 31 (creator, name, about, policy, join_price, mode, admins, members,
epoch, epoch_height, rekey_due, messages, created/set height, index), ChannelMember 32, ChannelInvite
33, ChannelIndex 34, ChannelCount 35. Reorgs roll all of it back with the branch.

**DM parameters** (AdminConfig, admin key only, valid from the activation height; kept in their own
record `["dm-params"]`, absent while every value is the default, so the config record never
changes because of them):
| key | name | value | default (design choice) |
|---|---|---|---|
| 35 | msg_inline_max | u32 (1..=8192) | 1024 |
| 36 | dm_reclaim_min | u32 blocks (>= 1) | 10 |
| 37 | dm_reclaim_max | u32 blocks (<= 1,000,000) | 10,080 (7 days of 60 s blocks) |
| 38 | dm_max_open_escrows | u32 (1..=65,536) per sender | 256 |
| 39 | dm_protocol_bps | u16 (0..=10,000) of settled pay, to the pool | 0 (D5: 100% to the agent) |
| 40 | channel_sealed_max_members | u32 (2..=10,000) | 250 (XMTP's group cap, ECOSYSTEM A2) |
| 41 | channel_create_fee | u64, to the pool | 0 |
| 42 | channel_max_admins | u8 (1..=32) | 8 |
Keys 43-44 are reserved (refused).

**RPC** (docs/RPC.md): `getDmParams`, `getDmGate {from, to}` (what a DM needs in the next block:
can_send, required_pay, reason, consent, in_thread, enc_key), `getDmStats {account}`, `getDmThread
{a, b}`, `getConsent {account, peer}`, `getMessageEscrow {message_id}`, `getMessage {message_id,
height}`, `getChannel {channel}`, `listChannels {offset, limit}`, `getChannelMember {channel,
account}`. getAccountTxs lists a DM under its recipient (role `message`) and a ChannelMember under
the named member (role `member`).

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
- The coin is fixed once set: `update_config` refuses a different `mint` after `launch`
  (`MintFrozen`), and an open crank (`cranker` unset) must spend the full
  `min(available, max_buy_per_call)` (`PartialSpend`; only an allowlisted cranker may pass a
  smaller `max_sol_in`) (security pass 2026-10-21, docs/SECURITY-REVIEW.md).
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
- Frozen once used (security pass 2026-10-21): `mint` and `bridge_program` at the first order
  (`orders > 0`), `reserve_program` at the first buyback routed to the reserve
  (`bb_to_reserve > 0`); `update_config` / `set_buyback` then refuse a different value
  (`DestinationFrozen`). An open buyback crank (`buyback_cranker` unset) must spend the full
  `min(available, buyback_max_per_call)` (`PartialSpend`).

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
- Job check at mining (2026-10-21, for the 8B's 640 segments per round): `begin_verify_job(
  proof_len, tx, path)` starts the verification of a job block with the job proven inline:
  accounts work, config, canon, the pending job block and the posting header; the posting tx
  (PostJob, order BridgeUnlock, or the PostSegments carrying the segment) is checked under the
  posting header's tx_root, the posting header must be Verified and final or canonical and
  lower than the job block, and the job id must be the job block's. These are exactly the
  checks of `register_job` plus the JobCommit branch of `begin_verify`, so soundness is
  unchanged: a job block is still verified only against commitments its own verified ancestry
  posted. Nothing is stored outside the relayer's work account, so a posted job costs nothing
  on Solana until a block is mined on it, and a mined one costs no JobCommit rent. Measured in
  LiteSVM (token/tests/lightclient-litesvm, a 640-tx posting block with real blob URIs): the
  PostSegments tx is 384 bytes and its path 264 bytes, the begin tx fits (Solana's 1,232-byte
  limit is asserted), `begin_verify_job` 101,257 CU, and the segment block verifies in 35 txs
  and 7.34M CU; with a JobCommit it was 34 txs and 7.24M CU plus the separate register_job tx
  (53,163 CU) and its rent, so the tx count per mined segment is the same and the rent is gone.
  The instruction can not batch several segments per Solana tx: a PostSegments of 64 segments
  is about 12 KB and would need its own chunked upload (13 more txs per mined block), so the
  gateway keeps one segment per PostSegments, which the agent chain carries at no Solana cost.
  header-relay: it no longer registers jobs; it indexes the jobs each new canonical block posts
  (job id -> posting height and hash, getJob `posted_height` after a restart), starts a job
  block's flight with `begin_verify_job` once the posting header is Verified (all chunks in the
  next batch, the begin tx carries no chunk), uses an existing JobCommit when one exists (jobs
  registered by older relays), and falls back to `register_job` only for a posting tx too large
  to go inline (above 800 bytes with its path). Because the posting header must exist when the
  job is mined, the relay keeps its own posting headers open past `retain_final` until the
  latest deadline of the jobs they posted (getJob `deadline_height`; one header record, 0.0042
  SOL refundable, per posting block, not per job), and a posting header closed anyway (by an
  older relay) is restored from the accumulator (`restore_header`) before the bind.
  End to end (2026-10-21, scripts/serve8b on the test box, a synthetic model in the Llama-3.1-8B
  shape: 640 matrices at the real k = 2048, n = 8 instead of the real 2,048-8,016, so B is 10.5
  MB, not 7.5 GB): one paid round of 640 segments posted, mined and settled, the Order finalized
  by proof; the light client verified all 641 job blocks of the run with `begin_verify_job`
  (87.8k CU average), no register_job and no JobCommit account, 0 failed txs, relay lag at most
  21 blocks (TEST-REPORT "Serve the 8B").
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
  `"cadre-auth-v2\n" + AUDIENCE + "\n" + METHOD + "\n" + PATH + "\n" + TIMESTAMP + "\n" + hex(sha256(body))`,
  AUDIENCE = the gateway's server key in base58 (`GET /v1/models` -> `data[0].cadre.server`, also
  `GET /health` -> `server`), so a signature is good at that one gateway only (security pass V-9).
  The older `"cadre-auth-v1\n" + METHOD + ...` (no audience) is accepted until the gateway runs
  with `CADRE_AUTH_REQUIRE_AUDIENCE=1`. The timestamp must be within 120 s and not before the
  gateway started; a signature is accepted once.
- `Authorization: Bearer cadre_sk_<hex>`: an API key bound to a wallet, made by
  `POST /v1/keys` (wallet-signed), listed by `GET /v1/keys` (wallet-signed), revoked by
  `DELETE /v1/keys/<key_id>` (wallet-signed).
  This is what plain OpenAI clients send.

Limits (security pass V-7): at most `--max-in-flight` (64) requests at once, more answer 503;
a socket read timeout of `--read-timeout-secs` (30) on every connection (headers, body, idle
keep-alive; a body not received in time answers 408); bodies over 4 MiB answer 413.

Endpoints (JSON; errors are `{"error": {"message", "type", "code"}}` with a 4xx/5xx status):
| Method, path | Auth | What |
|---|---|---|
| `GET /health` | none | `{ok, server, model_id, model, auth: ["cadre-auth-v2", "cadre-auth-v1"]}` |
| `GET /v1/models` | none | OpenAI list; each model `{id, object: "model", owned_by: "cadre", context_length, cadre: {model_id, recipe, quantization, matrices, macs_per_row, pricing: {lamports_per_row, prompt_lamports_per_token, completion_lamports_per_token, basis, default: true}, server}}`; `id` is the model's registry name (`llama-3.2-1b-w7a7`) |
| `POST /v1/balance/topup` | x402 (`X-PAYMENT`) | body `{rows?}`; no payment: 402 with the storefront's requirements; paid: 201 `{ticket_id, order, rows, wallet}` (wallet = the x402 payer = the ticket's poster) |
| `GET /v1/balance` | wallet or key, or `?wallet=` | `{wallet, tickets: [{ticket_id, order, status, rows, rows_reserved, rows_used_on_chain, rows_free, lamports_per_row, deadline_height}], rows_free}` |
| `GET /v1/prepaid/<wallet>` | none | the same as `GET /v1/balance?wallet=` (balances are public chain state) |
| `GET /v1/receipts?wallet=<wallet>` | none | the wallet's newest receipts (up to 200) |
| `GET /v1/receipts/<request_id>` | none | alias of `/v1/requests/<request_id>` |
| `POST /v1/balance/close` | wallet or key | settle and complete the wallet's open tickets now (refund of the unused rows on the chain) |
| `POST /v1/keys` | wallet | `{key_id, api_key}` (the key is shown once) |
| `DELETE /v1/keys/<key_id>` | wallet | revoke |
| `GET /v1/keys` | wallet | `{wallet, keys: [{key_id, created_ms, last_used_ms}]}`, oldest first: the wallet's keys, never the key itself; `last_used_ms` is null until the key authenticates a request (kept in the ledger) (added 2026-10-20) |
| `POST /v1/chat/completions` | wallet or key; or x402 per request | OpenAI chat completions: `model`, `messages`, `max_tokens` / `max_completion_tokens`, `stream`, `stop`, `tools` / `tool_choice` (rendered by the model's chat template; tool calls are parsed from the output when the model emits them, best effort), `temperature` / `top_p` accepted but decoding is greedy (deterministic, documented in each response); returns `chat.completion` (or SSE `chat.completion.chunk` events ending in `data: [DONE]`, usage and receipt in the last chunk) |
| `POST /v1/completions` | same | OpenAI text completions (`prompt`) |
| `GET /v1/requests/<request_id>` | none | the receipt plus its settlement: round, segment ids, blocks mined on them, ticket status, Solana order status |
| `GET /v1/stats` | none | measured figures of this gateway: requests, time to first token, decode tokens/s (recent and totals) |

CORS: every response allows any origin, the methods above and the request headers
`content-type, authorization, x-payment, x-cadre-wallet, x-cadre-timestamp, x-cadre-signature`,
and exposes the receipt headers below plus `x-payment-response`; OPTIONS preflights answer 204.
A proxy (services/cadre-api) must forward the three `x-cadre-*` auth headers and
`authorization` unchanged: the signature covers the gateway path, method and body, so the
proxy must forward the body byte for byte and map its own path prefix back to `/v1/...`.
services/cadre-api does so: `/v1/openai/<path>` -> gateway `/v1/<path>` (every path above
under `/v1/`), `/v1/inference/balance` -> `/v1/balance`, `/v1/inference/balance/topup` and
`/close` -> the same under `/v1/balance/`, `/v1/inference/prepaid/<wallet>` ->
`/v1/prepaid/<wallet>`, `/v1/inference/keys` -> `/v1/keys` (docs/API.md). A client signs the GATEWAY path in each case.

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

Measured throughput (2026-10-19, 4-core CPU test box, int7 torch worker, real runs only, data
in docs/measurements/cadre-worker-2026-10-19.jsonl): 1.0-3.1 decode tokens/s with the box
otherwise lightly loaded, 0.4-1.4 under the demo's miner, 0.37-0.63 through the gateway in the
passing demo. `GET /v1/stats` serves the gateway's own measured figures; nothing is estimated.

Settlement rounds: every `--round-secs` (default 60) the gateway posts one round per ticket
with served rows (A blobs + PostSegments); it delivers and completes a ticket when its free
rows drop below `--min-free-rows` (default 64), when it is within `--close-margin-blocks`
(default 50) of its deadline, or on `/v1/balance/close`.
Large balances (ECOSYSTEM M1.5, 2026-10-20): `POST /v1/balance/topup {rows, round_secs?}`
takes up to `min(--max-balance-rows, the chain's max_balance_rows)` rows (4096 by default; the
storefront's on-chain `max_dim`, 8192 on devnet, also bounds an order's m). A ticket above 4096
rows posts a round every `--big-round-secs` (default 10,800) unless its top-up named
`round_secs` (60..=86,400); the answer echoes `round_secs` and `max_round_rows`. One round never
stacks more than `max_round_rows` rows: every segment's A blob (`"PSA1" || m || k || m x k
int8`) must fit one 4 MiB blob at the model's widest k, so 2,044 rows on Llama-3.2-1B (k = 2048,
row period 4; `--max-round-rows` lowers it). A round is also due once that many rows are
pending, oldest requests first, and a request larger than one round is refused (400). Why a slow
period: every round is 144 segment jobs on the 1B model (640 on the 8B) and the chain mines about
1,440 job blocks a day, so rounds, not rows, are what cost. (Until 2026-10-21 the header relay also
registered every posted segment on Solana, 0.0022 SOL each on devnet; since then only mined
segments cost anything there, 5.3 "Job check at mining".)
Ticket block budget (2026-10-21, for the 8B): a ticket's `max_blocks` must cover every segment of
every round it posts, and one round allocates `round_blocks = matrices x --blocks-per-segment`
(640 on the 8B, 144 on the 1B). The gateway sizes a balance ticket at top-up as
`max_blocks = max_rounds x round_blocks` with `max_rounds = min(rows / row period, ceil(life /
round period) + 1 + rows / max_round_rows, cap / round_blocks)`, where life = the ticket's
deadline in blocks x the chain's `target_block_time_ms` and `cap = min(--max-ticket-blocks,
the chain's max_job_blocks)`; `--max-ticket-blocks` (default 10,000) must not exceed the
storefront's on-chain `max_blocks_cap` (10,000 at initialize; admin `set_config` raises it, up to
the chain's `max_job_blocks`, 100,000). The top-up answer echoes `max_rounds` and `max_blocks`,
`GET /v1/balance` each ticket's `max_rounds`. The settlement loop never posts more rounds than
`max_rounds`: in a ticket's last round a request is admitted only if the round can still stack
it (`max_round_rows`), after it the ticket has no free rows, completes (unused rows refunded on
the chain, 4.5) and the wallet tops up again. Before, the budget was `matrices x 64` capped at
10,000 with no round limit, so the 8B's 16th round (or the 1B's 70th) would have been refused by
the chain mid-ticket ("segments would use more than the ticket's blocks"). Figures at the
initial cap: 8B 15 rounds (9,600 blocks), 1B 69 rounds (9,936 blocks); with the cap at the chain's
100,000: 8B 156 rounds of 60 s, or 61 rounds for an 8,192-row balance at the 3 h period
(services/inference-gateway tests/budget.rs). Chain-native tickets and ledgers from before this
change read their round budget from the ticket's `max_blocks`.
Receipts carry `text_sha256` (since 2026-10-20): sha256 of the completion text exactly as the
response returns it (`message.content` / `text`), the hash a town post's `thought_ref` is
checked against (4.6). `output_sha256` stays the hash of the token ids.

Evaluator rule for tickets (M1, services/attester): the response blob must hash to the
ticket's `result_hash`, name the ticket's model and `request_hash`; its rounds must match the
chain's (count, m), each round's receipts must fit in its m; every round must have one
segment for every registered matrix; for each segment the attester loads A (blob) and B (its
copy of the model, checked against `hash_b`), computes `C = A B` and requires
`blake3_keyed(C, job_key) == c_hash`. Then it signs the result message. Since 2026-10-21, when
the response carries the cadre-glue-v1 replay bundle, the evaluator also replays the non-GEMM
steps that connect one segment's C to the next segment's A, bit for bit, and checks the output
tokens, stop and text (4.7); a mismatch rejects the ticket with a fraud proof. Without the
bundle (workers started before the change, or `--glue fp32`), only the GEMMs are checked and a
server could commit correct GEMMs over wrong activations.

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
  which models to register (DECIDED 2026-10-19: Llama-3.2-1B-Instruct), a model registration
  deposit (default none, tx fee only), blocks per segment (default 1).
