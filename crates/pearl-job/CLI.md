# `pearl-job` CLI

Build: `cargo build --release -p pearl-job` in `crates/` (binary at
`crates/target/release/pearl-job`). services/agent-cli finds it via `$PEARL_JOB`,
then `crates/target/{release,debug}/pearl-job`, then `PATH`.

## Contract (stable)

- Every subcommand prints exactly **one JSON object on one line** on stdout.
- Success: `"ok": true`, exit 0. Failure: `{"ok": false, "error": "..."}`, exit 1.
- `verify` and `freivalds` report a negative result as `"ok": true, "valid": false`
  (the command worked, the proof or C did not).
- Encodings: pubkeys base58; hashes, header and config lowercase hex (no `0x`);
  compact targets decimal or `0x`-prefixed hex on input, decimal on output; proofs
  binary files or standard base64.
- New output fields may be added; existing ones keep their names and meaning.

## Data layout

A job's data directory holds:

| file | content |
|---|---|
| `A.i8` | A, m x k int8 row-major (values in [-64, 64]) |
| `B.i8` | B, k x n int8 row-major (natural layout, not transposed) |
| `manifest.json` | `{"m", "n", "k", "a": "A.i8", "b": "B.i8"}` (paths relative to the manifest) |

`--data` and the job's `data_uri` accept `file:///abs/dir`,
`file:///abs/dir/manifest.json` or a plain path. Only `file://` in M1.

C (the result) is `m x n` i32 little-endian row-major (`.i32`).
`result_hash = blake3_keyed(C bytes, key = job_key)`.

## Job arguments (shared by header, commit, mine, verify, matmul, freivalds)

Either `--job-file job.json` (a JobSpec, below) or flags; flags override the file.

| flag | meaning |
|---|---|
| `--job <base58>` | job account pubkey (header bytes 0..32) |
| `--terms-hash <hex32>` | header bytes 32..64 |
| `--slot <u64>` | created_slot, header bytes 64..72 |
| `--target-bits <u32>` | share target (compact nbits), header bytes 72..76 |
| `--config <hex52>` | mining config; default = localnet (k=1024, r=32, 4x8 tile) |
| `--m`, `--n` | dims; taken from the data manifest when `--data` is given |
| `--hash-a`, `--hash-b` | commitments; computed from `--data` when omitted, and checked against the data when given |

JobSpec JSON (also what the miner and attester use; `jobs::Job` maps onto it):

```json
{"job": "<base58>", "terms_hash": "<hex>", "created_slot": 1234, "target_bits": 503578624,
 "config": "<hex52>", "m": 64, "n": 64, "hash_a": "<hex>", "hash_b": "<hex>",
 "data_uri": "file:///...", "max_nonce": 256}
```

`hash_a`, `hash_b`, `data_uri`, `max_nonce` are optional.

## Subcommands

### `gen --m M --n N [--k 1024] --out-dir DIR [--seed S]`
Random int7 A, B (deterministic with `--seed`) into DIR.
`{ok, m, n, k, manifest, a, b, data_uri}` (absolute paths; `data_uri` = `file://<manifest>`).

### `config [--k 1024] [--rank 32] [--rows 0,1,2,3] [--cols 0,1,2,3,4,5,6,7]`
`{ok, config, k, rank, h, w, row_period, col_period}`. m must be a multiple of
`row_period`, n of `col_period`.

### `header <job args>`
`{ok, header, config, job_key}`.

### `commit <job args> --data URI`
`{ok, hash_a, hash_b, job_key, header, config, m, n, k}`. Ignores any given
`--hash-a/--hash-b`. Fails if the data does not fit the config (shape rules in
PROOF-FORMAT.md section 2).

### `mine <job args> --data URI --miner <base58> [--nonce-start 0] [--nonce-end 1] [--share-target T] [--all] [--out PATH]`
Mines nonces in `[start, end)`. Per nonce: the full noised GEMM, tiles in order,
the first tile under the target is that nonce's share (at most one share per nonce,
matching the on-chain replay bitmap). Without `--all` stops at the first share.
`--out`: file for the single proof, or directory (`proof-<nonce>.bin`) with `--all`;
without `--out` each share carries `proof_b64`.
`{ok, found, shares: [{nonce, t_rows, t_cols, proof_len, proof_path | proof_b64}], nonces_tried, elapsed_ms, ms_per_nonce, share_target, hash_a, hash_b}`.

### `verify <job args, needs --m --n --hash-a --hash-b> --miner <base58> (--proof FILE | --proof-b64 B64) [--share-target T]`
`{ok, valid: true, nonce, t_rows, t_cols, jackpot_hash, proof_len, verify_us}` or
`{ok, valid: false, reason, proof_len, verify_us}`.

### `matmul <job args> --data URI --out C.i32`
`{ok, c, m, n, result_hash}`.

### `freivalds <job args> --data URI --c C.i32 [--rounds 2] [--seed S]`
Freivalds over GF(2^61 - 1); a wrong C passes a round with probability <= 2^-61.
OS randomness unless `--seed` (tests only). Also checks the data against the given
hashes. `{ok, valid, rounds, elapsed_ms, result_hash, hash_a, hash_b}`.

## v2 block subcommands (SPEC v2 section 3, PROOF-FORMAT.md section 7)

32-byte header fields take hex (64 chars) or base58. `--job` also accepts hex now.
Header flags (shared): `--header <hex256>` as the base, then overrides
`--version` (default 1), `--height`, `--prev-hash`, `--timestamp-ms`,
`--block-target` (chain target, compact), `--job-id` (zero/omitted = fallback),
`--payout` (miner field), `--tx-root`, `--state-root`, `--withdraw-root`,
`--proof-hash`, `--header-nonce`. Fallback shape: `--fallback-m 64 --fallback-n 64`.

### `block-header <header flags>`
`{ok, header, pow_digest, block_hash, height, nonce, target_bits, fallback}`.

### `block-mine <header flags> [<job args> --data URI] [--nonce-start 0] [--nonce-end 1] [--out PATH]`
Iterates the header nonce in `[start, end)` (each nonce is a new `pow_digest`, a full
noised GEMM). With job args and `--data` it mines a job block (a zero `--job-id` is
filled from the job); without them, a fallback block. On a win the header carries
the nonce and `proof_hash`.
`{ok, found, nonces_tried, elapsed_ms, ms_per_nonce[, header, pow_digest, block_hash, height, nonce, target_bits, fallback, proof_len, proof_path | proof_b64]}`.

### `block-verify <header flags> [<job args with --m --n --hash-a --hash-b>] (--proof FILE | --proof-b64 B64)`
Full block-work check (`pow::verify_block`). Job args are ignored for fallback headers.
`{ok, valid: true, nonce, t_rows, t_cols, jackpot_hash, block_hash, proof_len, verify_us}` or
`{ok, valid: false, reason, proof_len, verify_us}`.

## Choosing a share target

Expected MACs per share is `2^256 / target`; on one Apple M4 core the noised GEMM
runs at about 4.7 GMAC/s. One share per nonce at most, so expected shares per job
pass is about `min(1, m*n*k*target/2^256)` per nonce times `max_nonce`.

| target_bits | target | MACs per share | M4 single core |
|---|---|---|---|
| `0x1e040000` | 2^234 | 2^22 | tests: about 0.6 shares per nonce on 64 x 64 |
| `0x1d020000` | 2^225 | 2^31 | about 0.5 s |
| `0x1d010000` | 2^224 | 2^32 | about 1 s (measured 3 shares / 4.5 s on 256 x 256, 256 nonces) |
