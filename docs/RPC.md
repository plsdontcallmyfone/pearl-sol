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
 "tx_count": 3, "fees": "3000", "subsidy": "700", "min_fee": "1000"}
```
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
`{"account": "base58", "balance": "0", "nonce": 0}` (always an object; empty accounts are zero).
`"address"` is accepted as an alias for `"account"`.

### getAgent `{"account": "base58"}`
`{"account", "name", "uri", "registered_height", "updated_height"}` or null.

### getJob `{"job_id": "hex"}` -> Job or null

### listJobs `{"status"?: "open|delivered|completed|expired|mineable", "offset"?: 0, "limit"?: 50}`
`{"total": 12, "jobs": [Job]}`, in posting order. `limit` is at most 200.

### getBlock `{"hash": "hex"}` or `{"height": n}`, optional `"raw": true`
```json
{"hash": "hex", "header": "hex", "header_fields": {...}, "proof_len": 12600,
 "txs": [{"tx_hash": "hex", "kind": "Transfer", "from": "base58", "nonce": 0, "fee": "1000", "tx": "base64"}],
 "cumulative_work": "hex", "canonical": true, "confirmations": 3, "final": false,
 "fees": "3000", "subsidy": "45000000", "reward": "45003000", "matured": "0",
 "block": "base64 (only with raw)"}
```
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

### getAccountTxs `{"account": "base58", "limit"?: 50, "before"?: "height:index"}`
Txs touching the account on the canonical chain, newest first: txs it sent, Transfers to it,
BridgeUnlocks to it (coinbase, job payouts and refunds are not txs and are not listed).
```json
{"account": "base58", "txs": [{"tx_hash", "kind", "from", "nonce", "fee", "tx",
  "block_hash": "hex", "height": 41, "index": 0, "role": "sender|recipient",
  "confirmations": 3, "final": false}], "next": "41:0" | null,
 "tip_height": 43, "finalized_height": 13}
```
`limit` is 1..200. Pass `next` back as `before` for the next page (strictly older entries);
`next` is null on the last page. The final part comes from an index the node extends when
blocks finalize (they cannot reorg); the part above `finalized_height` is read from the
current canonical branch at query time, so a reorg simply changes what is listed. A page
taken before a reorg may overlap or skip entries on the new branch when you continue it:
restart from `before` = null after a `heads` notification with `reorg_depth` > 0. The index
lives in memory and is rebuilt from stored blocks at start-up.

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

### putBlob `{"data": "base64"}` (or `"data_hex"`)
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
Solana; the expire/refund path is the way back). So the relayer should read `getParams`
and refuse to sign / submit an order unlock with `P < min_protocol_fee_effective` (report it
as needing a refund) instead of retrying it, and the storefront's minimum order size should
keep `amount x protocol_fee_bps / 10000 x 1000 >= min_protocol_fee_effective`. Raising
`min_protocol_fee` by AdminConfig can strand orders already paid on Solana but not yet
unlocked: raise it only with the storefront's minimum raised first.

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

Rules worth knowing:
- `fee >= min_fee`, except BridgeUnlock (fee may be 0, so a relayer needs no balance).
- PostJob / order P >= `min_protocol_fee_effective` (getParams).
- `blob:` URIs (data_uri, SubmitResult uri) must be `blob:` + 64 hex.
- Nonces are per account and strict (`tx.nonce == account.nonce`).
- Fees and job subsidies are paid to the block's miner after `finality_k` blocks.
- Job block subsidy = `min(pool x emission_ppm / 1e6, job.cap_remaining)`; a job's cap
  starts at `alpha x P`. Fallback blocks earn fees only.
