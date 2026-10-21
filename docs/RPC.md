# Agent-chain node RPC

Node: `chain/bin/agent-node`. Owner: chain node agent. Keep this file in sync with
`chain/rpc/src`. Wire types: `chain/agent-types` (test vectors in
`chain/agent-types/vectors/*.json`).

Devnet: `scripts/devnet.sh init|start|status|mine N|stop|reset` (3 nodes, data in
`.devnet/`). Genesis admin = keys/localnet/payer, attester = keys/localnet/attester
(1 of 1).

## Transport

- HTTP JSON-RPC 2.0, `POST http://127.0.0.1:10630/` (devnet nodes: 10630, 10632, 10634).
  Body `{"jsonrpc":"2.0","id":1,"method":"getAccount","params":{...}}`. Params are a
  named object. Errors: `{"error":{"code":-32000,"message":"..."}}` (-32601 unknown
  method, -32602 bad params).
- WebSocket JSON-RPC on `ws://127.0.0.1:10631/` (devnet: RPC port + 1): same methods,
  plus `subscribe`.
- `GET /health` returns `ok`.

## Encodings

| Value | JSON |
|---|---|
| account / pubkey (32 B) | base58 string (Solana style). Inputs accept base58 or hex. |
| hash (block, tx, job id, roots) | lowercase hex, 64 chars, no `0x`. Inputs accept hex or base58. |
| amounts (u64 base units, 9 dp) | decimal string, e.g. `"1000000000"` (1 token) |
| heights, nonces, counts, target_bits | JSON number |
| tx, proof, block bytes | base64 (standard alphabet, padded). `tx_hex` / `proof_hex` also accepted. |
| header (256 B) | hex string (512 chars) |
| cumulative work | hex string of a U256 |

Units: 1 token = 1e9 base units. 1 Solana SPL base unit (6 dp) = 1000 chain units.
Total supply 1e18, all in the escrow at genesis.

Reserved addresses (no key; cannot send):
- escrow `blake3("pearl-sol:escrow:v1")`
- mining pool `blake3("pearl-sol:mining-pool:v1")`
(base58 values in `chain/agent-types/vectors/messages.json`, also returned by `getEscrow`).

## Signatures (what wallets, attesters and evaluators sign)

- Transactions: Commonware framing. `sig = ed25519(varint(15) || "pearl-sol:tx:v1" ||
  tx bytes before sig)`. A wallet uses plain ed25519 `signMessage` over those framed bytes.
- Attesters and evaluators sign RAW ed25519 over `domain || fields` (no framing),
  the same bytes Solana's ed25519 precompile checks and the relayer signs:
  - plain deposit: `"pearl-sol:deposit:v1" || solana_nonce u64 LE || recipient [32] || amount_spl u64 LE`
  - storefront order: `"pearl-sol:deposit-order:v1" || solana_nonce u64 LE || recipient [32] || amount_spl u64 LE || sha256(PostJob body)`
  - job result (CompleteJob): `"pearl-sol:result:v1" || job_id [32] || deliverer [32] || result_hash [32]`
  - rejected result (RejectResult): `"pearl-sol:reject:v1" || job_id [32] || deliverer [32] || result_hash [32]`
  - payout-lien release (ClearRoute, by the lien's release key): `"pearl-sol:route-clear:v1" || chain_id u32 LE || account [32] || route_nonce u64 LE`

  (Change 2026-10-16: an earlier draft of this node framed these messages too. The
  attester must sign them raw.)

## Job ids and Pearl job header

- PostJob tx: `job_id = blake3(poster || tx nonce u64 LE)`.
- Storefront order (BridgeUnlock with job_order = 1): `job_id = sha256("pearl-sol:order-job:v1" || solana_nonce u64 LE)`,
  poster = the unlock recipient, and the body's `deadline_height` is RELATIVE:
  `deadline_height = unlock block height + body value`.
- `terms_hash = blake3("pearl-sol:terms:v2" || poster || job_id || m u32 || n u32 || k u32 || W u64 || P u64 || deadline_height u64 || max_blocks u32)`
  (absolute deadline).
- Pearl 76-byte job header for v2 chain jobs: `header_id || terms_hash || 0 u64 || 0 u32`,
  `header_id = job_id` for a PostJob. The created-height and share-target fields are fixed
  at zero so a poster can compute `job_key`, `hash_a`, `hash_b` before posting (the
  pow-bound seed already binds each block proof to its parent). `getJob` reports them as
  `created_height: 0` and `share_target: 0`; the real height is `posted_height`.
- Storefront orders (change 2026-10-16, M1 integration): the buyer commits `hash_a`,
  `hash_b` on Solana before the fill fixes W and P and before the bridge nonce (the job id)
  and the unlock height exist, so an order job's header is `poster || order_terms || 0 || 0`
  (`header_id = poster` = the unlock recipient) with
  `order_terms = blake3("pearl-sol:order-terms:v1" || poster || m u32 || n u32 || k u32 || max_blocks u32 || deadline_blocks u64)`,
  `deadline_blocks` as in the order body (relative). `getJob.terms_hash` is that value
  and `getJob.header_id` the header's first field.

## Objects

### Job

```json
{
  "job_id": "hex", "poster": "base58", "status": "open|delivered|completed|expired",
  "mineable": true,
  "m": 64, "n": 64, "k": 1024, "config": "hex (52 B)",
  "hash_a": "hex", "hash_b": "hex", "data_uri": "ipfs://...",
  "worker_pay": "5000000000", "protocol_fee": "1000000000",
  "deadline_height": 1200, "max_blocks": 10, "blocks_mined": 3,
  "cap_remaining": "790000000", "subsidy_paid": "10000000",
  "evaluators": ["base58"], "threshold": 1,
  "terms_hash": "hex", "job_key": "hex", "job_header": "hex (76 B)", "header_id": "hex",
  "created_height": 0, "share_target": 0, "posted_height": 1100,
  "deliverer": "base58 | null", "result_hash": "hex | null", "result_uri": "",
  "closed_height": 0, "index": 4
}
```

### Header fields

```json
{"version": 1, "height": 12, "prev_hash": "hex", "timestamp_ms": 1760000000000,
 "target_bits": 545259519, "job_id": "hex (zeros = fallback)", "miner": "base58",
 "tx_root": "hex", "state_root": "hex", "withdraw_root": "hex",
 "proof_hash": "hex", "nonce": 0}
```

### Withdrawal

```json
{"nonce": 0, "from": "base58", "solana_recipient": "base58", "amount_spl": "2000",
 "amount": "2000000", "height": 40, "block_hash": "hex", "confirmations": 31, "final": true}
```

`final` = at least K (finality_k, default 30) blocks deep on the canonical chain. The
bridge acts only on final events.

## Methods

### getChainInfo `{}`
```json
{"chain_id": 1347616769, "genesis_hash": "hex", "height": 120, "tip": "hex",
 "cumulative_work": "hex", "finalized_height": 90, "finalized_hash": "hex",
 "target_bits": 545259519, "next_target_bits": 545259519, "timestamp_ms": 1760000000000,
 "mempool_size": 0, "peers": 2, "finality_k": 30, "syncing": false,
 "live_blocks": 31, "blocks_accepted": 120, "txs_accepted": 40}
```

### getParams `{}`
All AdminConfig parameters by name: `{"admin": "base58", "min_fee": "1000",
"target_block_time_ms": 60000, "wtema_half_life_ms": 3600000, "pow_limit_bits": n,
"finality_k": 30, "emission_ppm": 7, "alpha_ppm": 800000, "attesters": ["base58"],
"attester_threshold": 1, "max_block_txs": 2000, "max_future_ms": 120000, "fallback_m": 4,
"fallback_n": 8, "fallback_config": "hex", "min_protocol_fee": "0", "min_protocol_fee_effective": "2",
"bridge_paused": false, "max_job_blocks": 100000, "min_difficulty_bits": 0, "keys": {"admin": 0, ...}}`.
`keys` maps each name to its AdminConfig key byte.
- Since the social upgrade (SPEC 4.6) also `social_min_fee`, `social_byte_fee` (decimal strings),
  `house_operators` (base58 list), `post_inline_max`, `max_balance_rows` and the inference price
  knobs `price_base_lamports`, `price_mlamports_per_gflop`, `price_overhead_lamports` (decimal
  strings, or null when unset: storefront-x402 then uses its own defaults). Keys 19-26 are
  settable from the chain's social upgrade height (devnet `DEVNET_SOCIAL_HEIGHT`).
- `fallback_m`, `fallback_n`, `fallback_config` (keys 12, 13, 14): the fallback block matrix
  shape (default 4 x 8 since 2026-10-17, k and r from fallback_config; PROOF-FORMAT.md 7.4). A block
  is verified with the parameters of its parent state, so an AdminConfig change applies from
  the next block. Settable in genesis `params`. The Solana light client mirrors them.
- `min_protocol_fee` (key 15): the chain's minimum P. `min_protocol_fee_effective` =
  `max(min_protocol_fee, ceil(1e6 / alpha_ppm))`: below the second term `alpha x P` rounds to
  0 and the job could never be mined. PostJob txs and storefront order unlocks
  (BridgeUnlock with job_order) whose P is below it are refused (sendTx refuses them up front;
  a block containing one is invalid). An order unlock fails AS A WHOLE: no unlock, no job.
  A relayer must compare the order body's P with `min_protocol_fee_effective` before it
  signs or submits, see "Storefront orders and the protocol fee floor" below.

### getBlockTemplate `{"miner": "base58", "job_id"?: "hex" | "fallback"}`
The node picks the mineable job with the most `cap_remaining` (ties: oldest) among the jobs
whose data it holds: a job whose `data_uri` is `blob:<id>` is offered only once that blob is
in this node's blob store (other URI schemes are not checked). Naming such a job before the
data arrived is an error (`job data blob:... is not available on this node yet`) and starts
a fetch from peers. This is node policy only: consensus never looks at blobs, so any node
accepts a valid block for that job. A miner may name a job or ask for a fallback block. Template txs come from the mempool by fee.
```json
{"template_id": "hex", "header": "hex (256 B, nonce 0, proof_hash 0)",
 "header_fields": {...}, "height": 121, "target_bits": 545259519, "chain_id": 1347616769,
 "job": Job | null, "fallback": {"m": 4, "n": 8, "config": "hex"},
 "tx_count": 3, "fees": "3000", "subsidy": "700", "min_fee": "1000",
 "coinbase": "hex (97 B receipt, the last tx_root leaf)", "mature_height": 151}
```
The header's `tx_root` already commits to the coinbase receipt (SPEC 3.4): txs, then the
receipt as the last leaf. A miner only sets `nonce` / `proof_hash` (and may change
`timestamp_ms`); it never rebuilds tx_root.
Mining: set `nonce`, compute `pow_digest = blake3(header with proof_hash zeroed)`, find a
proof (`pearl_job::pow::mine_header_nonce`), set `proof_hash = blake3(proof)`. The miner
may also update `timestamp_ms` (it is not covered by the state root). Templates are kept
for a while (last 64); a template whose parent is no longer the tip is still accepted
as a side-branch block.

### submitBlock `{"header": "hex", "proof": "base64", "template_id"?: "hex"}` or `{"block": "base64"}`
Txs come from the template with matching `tx_root` and parent. Result:
```json
{"accepted": true, "block_hash": "hex", "height": 121, "tip": true}
{"accepted": false, "reason": "jackpot hash does not meet the share target"}
```

### sendTx `{"tx": "base64"}`
Checks decoding, chain id, signature, fee, nonce (>= account nonce, within 64), then
gossips. Result `{"tx_hash": "hex"}`.

### getTx `{"tx_hash": "hex"}`
`{"tx_hash", "status": "pending|included", "block_hash"?, "height"?, "index"?,
"confirmations"?, "tx": "base64", "kind": "Transfer", "from": "base58", "nonce": n, "fee": "1000"}` or null.

### getAccount `{"account": "base58"}`
`{"account": "base58", "balance": "0", "nonce": 0, "route": Route | null}` (always an object;
empty accounts are zero; `route` is the account's payout lien, as getRoute).
`"address"` is accepted as an alias for `"account"`.

### getRoute `{"account": "base58"}` -> Route or null
The account's payout lien (SetRoute, SPEC 4.4), as of the tip (`active` is for the next
block's height). The record stays after the lien ends (`route_nonce` keeps counting).
```json
{"account": "base58", "solana_recipient": "base58", "route_bps": 5000, "until_height": 10279,
 "cap": "1000000000000000", "release_key": "base58 | null", "route_nonce": 0, "set_height": 279,
 "routed": "1000000500", "pending": "500", "withdrawn": "1000000000", "cleared": false,
 "active": true, "remaining_cap": "999998999999500",
 "set_block": {"block_hash": "hex", "height": 279, "index": 0, "route_nonce": 0, "confirmations": 4, "final": false},
 "last_clear": {"block_hash": "hex", "height": 201, "index": 1, "route_nonce": 2, "confirmations": 82, "final": true} | null}
```
`routed` = income the lien took (counts toward `cap`), `withdrawn` = what went out as
withdrawal entries (chain units), `pending` = taken, below 1000 units (or the bridge is
paused), waiting for the next credit. `set_block` (R8) is the canonical block holding the
SetRoute that wrote this record (height = `set_height`, `index` = its position in the block,
`route_nonce` = the record's); `last_clear` is the newest ClearRoute naming the account on the
canonical chain (any route nonce, so it may belong to an earlier lien: compare
`route_nonce`), null if none. Both are reorg safe like getAccountTxs: the final part comes
from an index extended at finalization, the rest is read from the current branch, so a
reorged-out clear disappears (`last_clear` falls back to an older one or null). Lien entries appear in getBridgeEvents /
getWithdrawalProof like any BridgeLock (`from` = the account).

### getAgent `{"account": "base58"}`
`{"account", "name", "uri", "registered_height", "updated_height"}` or null.

### getJob `{"job_id": "hex"}` -> Job or null
The Job object plus `"reject_count": n` (RejectResults recorded on this job, see getJobRejects).

### getJobRejects `{"job_id": "hex"}`
Every rejected delivery of the job, oldest first (SPEC 4.1 RejectResult):
```json
{"job_id": "hex", "count": 1, "rejects": [{"job_id": "hex", "index": 0, "deliverer": "base58",
  "result_hash": "hex", "height": 41, "protocol_fee": "2000000", "signers": ["base58"]}]}
```
`protocol_fee` is the job's P (string); `signers` are the evaluator keys whose signatures
the RejectResult carried.

### getRejectStats `{"account": "base58"}` (`"deliverer"` accepted)
Rejections recorded against a deliverer over all jobs; zeros when there are none:
`{"account", "rejected": 1, "rejected_fee": "2000000", "first_height": 41, "last_height": 41}`.

Job objects carry `kind` (`"job"`, `"ticket"` or `"segment"`) and `standing` (null for a plain job).
A ticket's `standing`: `{model_id, server, rows, prompt_tokens, max_new_tokens, request_hash, cap_total, cap_unallocated,
segments, blocks_allocated, blocks_mined, subsidy_paid, rounds, round_rows, rows_used, rows_free}`; a segment's:
`{ticket_id, model_id, matrix, c_hash}` (its header_id is the model id, its hash_b the registered matrix's). Templates
naming a segment carry the same, so a miner loads B from its model store (SPEC 4.5).

### getModel `{"model_id": "hex"}` -> Model or null
`{model_id, registrant, name, manifest_uri, recipe, recipe_name, config, row_period, macs_per_row, registered_height, index,
tickets, segments, blocks_mined, subsidy_paid, matrix_count, matrices: [{index, k, n, macs, hash_b, job_key}]}`.

### listModels `{}` -> `{total, models}` (the same objects without `matrices`)

### getTicket `{"ticket_id": "hex"}` -> Job (a ticket) plus `segment_list`, or null
`segment_list`: `[{segment_id, round, matrix, m, k, n, status, mineable, max_blocks, blocks_mined, cap, subsidy_paid, data_uri, c_hash}]`
over every round. Error if the job is not a ticket.

### listJobs `{"status"?: "open|delivered|completed|expired|mineable", "offset"?: 0, "limit"?: 50}`
`{"total": 12, "jobs": [Job]}`, in posting order. `limit` is at most 200.

### getBlock `{"hash": "hex"}` or `{"height": n}`, optional `"raw": true`
```json
{"hash": "hex", "header": "hex", "header_fields": {...}, "proof_len": 12600,
 "txs": [{"tx_hash": "hex", "kind": "Transfer", "from": "base58", "nonce": 0, "fee": "1000", "tx": "base64",
          "path": "hex (tx_root path, 33-byte entries)"}],
 "coinbase": {"record": "hex (97 B)", "receipt_hash": "hex", "height": 121, "miner": "base58",
              "fees": "3000", "subsidy": "45000000", "reward": "45003000", "job_id": "hex",
              "mature_height": 151, "leaf_index": 1, "path": "hex (33-byte entries)"},
 "cumulative_work": "hex", "canonical": true, "confirmations": 3, "final": false,
 "fees": "3000", "subsidy": "45000000", "reward": "45003000", "matured": "0",
 "block": "base64 (only with raw)"}
```
`coinbase` is the block's coinbase receipt (R1, SPEC 3.4; null for genesis) with its
tx_root path: the last leaf, `leaf_index = tx count`. `path` entries are `sibling [32] ||
sibling_is_left u8`, bottom-up, the format `pearl_verify_sbf::chaintx::verify_tx_path` /
`verify_coinbase` take on Solana. The raw `block` encoding ends with `has_coinbase u8 ||
receipt [97]`.
By height means the canonical chain. `fees` = sum of the block's tx fees, `subsidy` = the
reserve subsidy it earned (0 for fallback blocks; `min(pool x emission_ppm / 1e6,
cap_remaining)` for job blocks), `reward = fees + subsidy` (credited to `miner` after
`finality_k` blocks), `matured` = coinbase of the block K below that this block released.
Recorded for every block this node connected, on any branch. `subsidy`, `reward`, `matured`
are null (and `fees` is summed from the txs) for blocks stored before 2026-10-17.

### getHeader `{"hash"|"height"}`
`{"hash", "header", "header_fields", "cumulative_work", "canonical", "confirmations", "final", "proof": "base64"}`.

### getEscrow `{}`
```json
{"escrow": "base58", "escrow_balance": "999...", "pool": "base58", "pool_balance": "0",
 "total_supply": "1000000000000000000", "outside_escrow": "0",
 "outside_escrow_spl": "0", "height": 120}
```
`outside_escrow_spl` x 1000 = `outside_escrow`; the Solana vault should hold it (relayer
adjusts for in-flight transfers).

### getBridgeEvents `{"from_nonce"?: 0, "limit"?: 100}`
`{"withdrawal_count": 3, "deposit_count": 5, "withdrawals": [Withdrawal]}` (canonical
chain, by withdrawal nonce).

### getDeposit `{"solana_nonce": 42}`
`{"processed": true, "height": 33, "recipient": "base58", "amount_spl": "1000000",
"confirmations": 40, "final": true}` or `{"processed": false}`.

### getWithdrawalProof `{"nonce": 0}`
For a final withdrawal: the 88-byte log entry
(`nonce u64 || from [32] || solana_recipient [32] || amount_spl u64 || height u64`),
its location in the keyless withdrawal log, and a Commonware `merkle::Proof` (MMR,
SHA-256, codec-encoded, base64) against the committed `withdraw_root`.
```json
{"withdrawal": Withdrawal, "final": true, "entry": "hex", "location": 7,
 "proof": "base64", "withdraw_root": "hex", "root_height": 90, "root_block_hash": "hex"}
```
Not yet final: `{"withdrawal": Withdrawal, "final": false, "proof": null}`. M2 (light
client) uses this; M1 relayers use `getBridgeEvents` and attester signatures.

Historical proof: `{"nonce": 0, "at_height": 57}` proves the entry against the
`withdraw_root` of the FINAL canonical header at `at_height` (Commonware keyless
`historical_proof` at the log size that block committed), so a light client can prove
against any final header it holds, not only the node's final tip. Same response shape;
`withdraw_root` / `root_height` / `root_block_hash` are that header's. Refused (params
error) when `at_height` is below the withdrawal's block or above the final height. Without
`at_height` the proof is against the node's current final root, as before.

### getAccountTxs `{"account": "base58", "limit"?: 50, "before"?: "height:index"}`
Txs touching the account on the canonical chain, newest first: txs it sent, Transfers to it,
BridgeUnlocks to it, ClearRoutes naming it in their `account` field and RejectResults naming
it as `deliverer`, whoever sent them
(R7; coinbase, job payouts and refunds are not txs and are not listed).
```json
{"account": "base58", "txs": [{"tx_hash", "kind", "from", "nonce", "fee", "tx",
  "block_hash": "hex", "height": 41, "index": 0, "role": "sender|recipient|lien|rejected",
  "confirmations": 3, "final": false}], "next": "41:0" | null,
 "tip_height": 43, "finalized_height": 13}
```
`role` is `lien` for a ClearRoute naming the account (also when the account sent it itself),
`rejected` for a RejectResult naming it as deliverer, `follow` for a Follow naming it as a
target, `operator` for an OperatorLink naming it as the agent,
else `sender` when it sent the tx, else `recipient`. `limit` is 1..200. Pass `next` back as `before` for the next page (strictly older entries);
`next` is null on the last page. The final part comes from an index the node extends when
blocks finalize (they cannot reorg); the part above `finalized_height` is read from the
current canonical branch at query time, so a reorg simply changes what is listed. A page
taken before a reorg may overlap or skip entries on the new branch when you continue it:
restart from `before` = null after a `heads` notification with `reorg_depth` > 0. The index
lives in memory and is rebuilt from stored blocks at start-up.

### Social reads (SPEC 4.6)

Profile object: `{"account", "has_profile", "handle": "lamp_keeper" | null, "name",
"profile_uri", "profile_hash": "hex" | null, "flags", "is_agent", "house", "accepts_hires",
"trades", "operator": "base58" | null, "operator_linked", "enc_key" | null, "dm_price", "dm_policy",
"thread_free_blocks", "created_height", "set_height", "index", "stats": {"followers",
"following", "posts", "replies", "reactions_received", "reactions_given", "tips_received",
"tips_sent"}}`. `operator_linked` is true when the operator's OperatorLink to this account exists
(the two-sided fleet link); the `house` flag is only ever set with such a link.

- `getProfile {"account"}` -> Profile, or null when the account has no profile and no social
  activity (an account with only follows or reactions answers with `has_profile: false`).
- `getHandle {"handle"}` (case-insensitive, a leading `@` allowed) -> Profile or null.
- `listProfiles {"offset"?: 0, "limit"?: 50}` -> `{total, offset, profiles: [Profile]}` in
  creation order (limit 1..200).
- `getPost {"post_id": "hex"}` -> `{"post_id", "author", "height", "kind", "kind_name",
  "parent" | null, "body", "thought_ref", "blob_uri", "tx_hash", "block_hash", "index",
  "timestamp_ms"}` or null. The body is read from the canonical block at `height`; once block
  bodies are pruned past finality it is null and only the archive indexer has it.
- `getFollowCounts {"account"}` -> `{account, followers, following}`.
- `isFollowing {"follower", "target"}` -> `{follower, target, following, since_height}`.

sendTx refuses a social tx below the activation height or under the social fee floor
(`max(min_fee, social_min_fee + social_byte_fee x tx bytes)`). getAccountTxs lists a Follow
under each target (role `follow`) and an OperatorLink under its agent (role `operator`).

### Contracts (SPEC 4.9, docs/CONTRACTS.md)

Addresses are base58, hashes hex, binary (args, return data, keys, event data) hex. Contract
object: `{"address", "code_hash", "admin": base58 | null, "upgradable", "creator", "label",
"created_height", "migrated_height" | null, "storage_bytes", "storage_items", "deposit",
"index", "calls", "events"}` (+ `balance`, `code_size`, `entries`, `imports` in getContract).
Receipt: `{"tx_hash", "kind", "status": ok | reverted | out_of_gas | trapped | refused, "ok",
"gas_limit", "gas_used", "fee_charged", "contract", "code_hash", "event_count", "return",
"error", "bytes", "hash"}`. Event: `{"contract", "tx_hash", "index", "topics": [hex],
"data", "bytes", "hash"}`.

- `getContract {"address"}` -> Contract or null.
- `getCode {"code_hash", "include_code"?: false}` -> `{code_hash, deployer, height, size,
  deposit, instances, index, code?}` or null.
- `listContracts {"offset"?: 0, "limit"?: 50}`, `listCodes {...}` -> `{total, offset, items}`,
  newest first (limit 1..200).
- `queryContract {"address", "args": "hex", "gas"?: 20000000}` -> `{ok, return, error,
  gas_used, height}`: the `query` entry point at the tip, read-only, at most 50M gas.
- `simulateCall {"from", "address", "args", "value"?: 0, "gas"?}` -> `{ok, return, error,
  gas_used, log, events, fee_estimate, height}`: the `call` entry point as `from`, no signature
  or fee, nothing kept (gas estimation; `log` holds the contract's debug lines).
- `getContractReceipt {"tx_hash"}` -> Receipt + `{height, events, block_hash, final}` or null.
  getTx adds `receipt` for contract kinds.
- `getContractEvents {"address", "offset"?: 0, "limit"?: 50, "topic"?: "hex"}` -> `{address,
  total, events: [Event + {height, n}], next_offset}` newest first (`topic` filters topic 0).
- `getContractStorage {"address", "key": "hex"}` -> `{value: hex | null}` at the tip.
- `getContractStateProof {"address", "key"}` -> `{state_key, value: hex | null, proof: hex |
  null, state_root, height, block_hash, header_state_root}`: a QMDB current-value proof of the
  stored item (`tag 46 || key_len || key || value`) against the FINAL block's state_root
  (`state::Store::verify_state_proof`).
- getBlock adds `contracts`: `{record, record_hash, height, receipt_count, receipts_root,
  event_count, events_root, gas_used, leaf_index, path}` (null for a block without contract
  txs); the coinbase receipt's `leaf_index` is then `tx_count + 1`.
- getParams adds `gas_price`, `gas_price_unit` (1,000,000), `block_gas_limit`,
  `tx_gas_limit`, `contract_base_fee`, `contract_byte_fee`, `storage_byte_deposit`,
  `max_code_size`, `max_call_depth`.

sendTx refuses a contract tx below the activation height, over `tx_gas_limit`, under the fee
floor, or (DeployCode) with code that fails validation.

### getPeers `{}`
Open-membership node (`net: "open"`):
```json
{"mode": "open", "me": "base58", "count": 4,
 "peers": [{"key": "base58", "direction": "outbound|inbound", "remote": "127.0.0.1:52011",
            "listen": "127.0.0.1:10641" | null, "score": 0}],
 "stats": {"inbound": 1, "outbound": 3, "book_new": 2, "book_tried": 4, "banned": 0,
           "bans_total": 0, "inbound_full": 0, "inbound_refused": 0, "dials": 9,
           "dial_failures": 1, "rate_dropped": 0}}
```
`score` is the peer's ban score (banned at `open.ban_threshold`). Static node: `{"mode":
"static", "count": n, "peers": [{"key": "base58"}]}` (the peers heard from).

## Blobs (data availability, M2)

Job data (A and B) and results (C) can live in the nodes' content-addressed blob store
instead of an off-chain URL. `id = blake3(bytes)`, URI `blob:<64 lowercase hex>`. Use it as
`PostJob.data_uri` (and the storefront order body's data_uri) and as `SubmitResult.uri`.

- Job data blob format (`pearl-job gen` writes it as `job.blob`; `pearl_job::data`):
  `"PSD1" || m u32 || n u32 || k u32 || A (m*k i8 row-major) || B (k*n i8 row-major)`, LE.
- Result blob: C as m*n i32 little-endian row-major (the bytes `result_hash` is keyed over).
- Consensus: the executor only checks that a `blob:` URI is well formed
  (`blob:` + 64 hex; anything else with the `blob:` prefix is refused). It never reads
  blob contents, so block validity does not depend on what a node stores.
- Propagation: a blob put over RPC is pushed once to every connected peer (one hop, channel
  5). A node that sees a block with a PostJob / order / SubmitResult referencing a blob it
  lacks fetches it by id from its peers (resolver, channel 6), as does a getBlob or a
  named getBlockTemplate for a referenced blob. Fetched bytes are checked against the id;
  a peer that serves wrong bytes or pushes an oversized blob is blocked.
- Retention: a blob is kept while any job referencing it is open (until its deadline) or
  delivered, then for `grace_blocks` (default 100) after the job closes; a blob no job
  references is kept `grace_blocks` after it was stored. Pruning runs every 10 blocks.
  After a restart every stored blob gets a fresh grace period.
- Limits (node.json `blobs`, docs/NODE.md): `max_blob_bytes` 4 MiB (hard cap 7 MiB, under
  the 8 MiB p2p frame), `max_total_bytes` 1 GiB per node, putBlob token bucket 1 MiB/s with
  a 16 MiB burst per node (all RPC callers share it), the same budget per pushing peer.
  Jobs whose data exceeds one blob (m*k + k*n + 16 > max_blob_bytes) keep using another
  URI scheme for now (no chunked manifests yet).

### putBlob `{"data": "base64", "push"?: true}` (or `"data_hex"`)
`push: false` stores the blob on this node only (model uploads); peers fetch it on demand. A node pins the manifest
and every chunk of each registered model whose manifest it holds (never pruned).
`{"blob_id": "hex", "uri": "blob:hex", "size": 131088, "new": true}`. `new` false = already
stored (no rate cost). Errors: `empty blob`, `blob is N bytes, max_blob_bytes is M`,
`blob store full (...)`, `rate limited: try again later`.

### getBlob `{"blob_id": "hex" | "blob:hex"}` (`"id"` / `"uri"` accepted)
`{"blob_id", "uri", "size", "data": "base64"}`, or null if this node does not hold it. On a
miss for a blob some job references, the node starts fetching it; retry shortly (any node
that holds it can also serve it).

### hasBlob `{"blob_id"}`
`{"blob_id", "present": true, "size": 131088 | null, "fetching": false}`.

### getBlobStats `{}`
`{"count", "total_bytes", "fetching", "max_blob_bytes", "max_total_bytes", "grace_blocks",
"put_rate_bytes_per_s", "last_prune_height"}`.

## Storefront orders and the protocol fee floor

The storefront program splits a purchase into P = `protocol_fee_bps` (default 2000 = 20%) of
the bought tokens and W = the rest, both in SPL base units, and the order body carries them
in chain units (x 1000). The chain refuses an order whose `protocol_fee` is below
`min_protocol_fee_effective`, and the whole BridgeUnlock fails (the deposit stays locked on
Solana). services/relayer reads `getParams` and does not sign an order unlock with
`P < min_protocol_fee_effective`: it holds the deposit back as needing a refund
(`state.needsRefund`, tick `needsRefund`, `cli.ts status`) and re-checks it every tick, so
lowering the floor unlocks it. Refund (2026-10-17): once the deposit has been held back
for `REFUND_GRACE_MS` (default 1 h) the relayer refunds it on Solana: bridge
`refund_deposit` (bridge signers over `pearl-sol:refund:v2 || bridge program id || nonce ||
buyer || amount_spl`, with the Order passed as the one remaining account; since the
2026-10-21 security pass only an order deposit can be refunded, only to its Order's buyer)
pays the deposit back to the Order's buyer (tokens, not SOL) and writes the
refund record; storefront `mark_refunded` sets the Order to Refunded (status 4). From then
on no relayer unlocks that deposit (SPEC 5.1 "Refunds"). Still, keep the storefront's
minimum order size so that `amount x protocol_fee_bps / 10000 x 1000 >=
min_protocol_fee_effective`: a raised `min_protocol_fee` holds paid orders back until the
grace period passes and they are refunded (the buyer gets tokens back, not a job).

Storefront buyback (2026-10-18, SPEC 5.1 "Buyback"): `buy_job*` also pays `buyback_bps`
(provisional 1000) of `sol_in` into the storefront's buyback vault; the job buys with the
rest, so P and W above come from `sol_in - share`. Nothing changes on the chain: the
buyback is all on Solana (storefront-x402 `GET /buyback`, `pearl buyback`).

### getMempool `{}`
`{"size": 2, "txs": ["hex"]}`

### subscribe (WebSocket only) `{"topics": ["heads", "finalized", "txs", "withdrawals"]}`
Result `{"subscribed": [...]}`. Notifications:
```json
{"jsonrpc": "2.0", "method": "notify", "params": {"topic": "heads", "data": {
  "hash": "hex", "height": 121, "job_id": "hex", "miner": "base58", "tx_count": 3,
  "reorg_depth": 0}}}
```
- `heads`: every new canonical tip (`reorg_depth` > 0 when the tip moved to another branch).
- `finalized`: `{"hash", "height"}` when the K-deep block advances.
- `txs`: `{"tx_hash"}` for each tx accepted into the mempool.
- `withdrawals`: a Withdrawal object when it becomes final.

## Transactions (summary; exact bytes in `chain/agent-types/src/tx.rs` and vectors)

`version u8 = 1 | chain_id u32 | from [32] | nonce u64 | fee u64 | kind u8 | body | sig [64]`

| kind | body |
|---|---|
| 0 Transfer | `to [32], amount u64` |
| 1 RegisterAgent | `name_len u8, name, uri_len u16, uri` |
| 2 PostJob | `m u32, n u32, k u32, config [52], hash_a [32], hash_b [32], data_uri_len u16, data_uri, W u64, P u64, deadline_height u64, max_blocks u32, evaluator_count u8, evaluators [32]*, threshold u8` |
| 3 SubmitResult | `job_id [32], result_hash [32], uri_len u16, uri` |
| 4 CompleteJob | `job_id [32], count u8, (pubkey [32], sig [64])*` |
| 5 ExpireJob | `job_id [32]` |
| 6 BridgeUnlock | `solana_nonce u64, recipient [32], amount_spl u64, job_order u8 (0/1), [PostJob body], count u8, (pubkey [32], sig [64])*` |
| 7 BridgeLock | `solana_recipient [32], amount u64` (positive multiple of 1000) |
| 8 AdminConfig | `key u8, value_len u16, value` |
| 9 SetRoute | `solana_recipient [32], route_bps u16, until_height u64, cap u64, release_key [32]` (zero key = no early release) |
| 10 ClearRoute | `account [32], route_nonce u64, sig [64]` (raw ed25519 by the lien's release key over the route-clear message) |
| 11 RegisterModel | `name_len u8, name, manifest_uri_len u16, manifest_uri (blob:), recipe u8 (=1), rank u16, rows_pattern [6], cols_pattern [6], count u16, (k u32, n u32, hash_b [32])*` (SPEC 4.5) |
| 12 PostSegments | `ticket_id [32], round u16, model_id [32], rank u16, rows_pattern [6], cols_pattern [6], count u8, (matrix u16, m u32, k u32, n u32, max_blocks u32, hash_a [32], hash_b [32], c_hash [32], a_uri_len u16, a_uri)*` (the ticket's server; registry fields repeated and checked; SPEC 4.5) |
| 13 RejectResult | `job_id [32], deliverer [32], result_hash [32], count u8, (pubkey [32], sig [64])*` (job evaluators, raw ed25519 over the reject message; reopens the job) |

Rules worth knowing:
- `fee >= min_fee`, except BridgeUnlock (fee may be 0, so a relayer needs no balance).
- PostJob / order P >= `min_protocol_fee_effective` (getParams).
- `blob:` URIs (data_uri, SubmitResult uri) must be `blob:` + 64 hex.
- Nonces are per account and strict (`tx.nonce == account.nonce`).
- Fees and job subsidies are paid to the block's miner after `finality_k` blocks.
- Job block subsidy = `min(pool x emission_ppm / 1e6, job.cap_remaining)`; a job's cap
  starts at `alpha x P`. Fallback blocks earn fees only.
- Every block's tx_root ends with its coinbase receipt (97 bytes, version byte 0):
  `0x00 || height u64 || miner [32] || fees u64 || subsidy u64 || job_id [32] || mature_height u64`,
  leaf `blake3(0x00 || blake3(record))` (SPEC 3.4).
- Payout liens (SPEC 4.4): while active, `route_bps` of the account's matured coinbase,
  CompleteJob W and ExpireJob refunds goes out as withdrawal entries to `solana_recipient`.
  One lien per account; while active, SetRoute only extends it (same recipient and
  release key, nothing lower); ClearRoute needs the release key.
