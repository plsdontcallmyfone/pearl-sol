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
- Pearl 76-byte job header for v2 chain jobs: `job_id || terms_hash || 0 u64 || 0 u32`.
  The created-height and share-target fields are fixed at zero so a poster can compute
  `job_key`, `hash_a`, `hash_b` before posting (the pow-bound seed already binds each
  block proof to its parent). `getJob` reports them as `created_height: 0` and
  `share_target: 0`; the real height is `posted_height`.

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
  "terms_hash": "hex", "job_key": "hex", "job_header": "hex (76 B)",
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
"attester_threshold": 1, "max_block_txs": 2000, "max_future_ms": 120000, "fallback_m": 64,
"fallback_n": 64, "fallback_config": "hex", "min_protocol_fee": "0", "bridge_paused": false,
"max_job_blocks": 100000, "min_difficulty_bits": 0, "keys": {"admin": 0, ...}}`.
`keys` maps each name to its AdminConfig key byte.

### getBlockTemplate `{"miner": "base58", "job_id"?: "hex" | "fallback"}`
The node picks the mineable job with the most `cap_remaining` (ties: oldest); a miner
may name a job or ask for a fallback block. Template txs come from the mempool by fee.
```json
{"template_id": "hex", "header": "hex (256 B, nonce 0, proof_hash 0)",
 "header_fields": {...}, "height": 121, "target_bits": 545259519, "chain_id": 1347616769,
 "job": Job | null, "fallback": {"m": 64, "n": 64, "config": "hex"},
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
 "block": "base64 (only with raw)"}
```
By height means the canonical chain.

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
- Nonces are per account and strict (`tx.nonce == account.nonce`).
- Fees and job subsidies are paid to the block's miner after `finality_k` blocks.
- Job block subsidy = `min(pool x emission_ppm / 1e6, job.cap_remaining)`; a job's cap
  starts at `alpha x P`. Fallback blocks earn fees only.
