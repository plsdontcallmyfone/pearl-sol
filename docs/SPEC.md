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
whole supply (1e18) to the bridge escrow. Nothing is minted or burned on the chain
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

### 4.2 Agents
`RegisterAgent`: profile `{name, uri, registered_height}` keyed by account.

### 4.3 Bridge (chain side)
- `BridgeUnlock`: M-of-N attester signatures over
  `b"pearl-sol:deposit:v1" || solana_nonce || recipient || amount_spl` (or the
  `job_order` variant, see 5). Replay set keyed by `solana_nonce`. Pays
  `amount_spl x 1000` from the escrow.
- `BridgeLock`: moves `amount` into the escrow and appends
  `{nonce, solana_recipient, amount_spl}` to the withdrawal log (`withdraw_root`).
- Inbound (Solana -> chain) stays attested in M1 and M2 (our chain verifying
  Solana state is out of scope). Outbound becomes proof-based in M2 (section 6).

## 5. Solana side

Existing: `reserve` (launch, buybacks, fund_mining) and `bridge` (deposit,
release with M-of-N ed25519 signatures). Changes and additions:

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
- Refund path: if the job expires, W comes back over the bridge to the buyer.
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
  fallback rebuild for fallback blocks, openings, noise, jackpot). Measured: job
  block about 3.5M CU / 19 txs; fallback block about 5.1M CU / 20 txs at the 4x8
  fallback default (18.9M at 64x64). About 0.0001 SOL in fees per header plus
  0.0042 SOL rent, refunded on close_header. Relayers can be paid from a pool.
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

## 8. Known security issue (open, 2026-10-17)

Low-rank job matrices: a poster can post zero or low-rank matrices for the price
of P. The noised product is then itself low rank and can be computed far cheaper
than a real multiply, so job blocks on such jobs are cheap PoW. Pearl mainnet has
a rank-penalty rule for this; we dropped it in M1. Must be fixed before any
public network (options: Pearl's rank penalty on the difficulty bound; requiring
job matrices to pass a randomized rank/entropy check by the evaluators before the
job is minable; or binding job blocks to a higher target than fallback blocks).

## 9. Open questions for the owner
- Dev buy size (sets the starting reserve).
- Block time (60 s default), K (30), alpha (0.8), emission (7 ppm per block).
- Name and ticker.
