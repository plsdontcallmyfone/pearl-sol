# Evaluators: the open check (Cadre)

Owner decision 2026-10-18: evaluation is open now; fees and bonds come later. This file is
the specification of what an evaluator checks, which bytes it signs, and how to run the
reference evaluator (`pearl-evaluator`, services/attester) yourself. Anyone can be an
evaluator. A buyer makes you one by putting your public key in a job's evaluator list.

## 1. Who evaluates a job

Every job carries its own evaluator set: 1 to 8 ed25519 public keys and a `threshold`
(PostJob body, SPEC 4.1). The chain accepts CompleteJob or RejectResult only with
`threshold` distinct valid signatures from that set. Nothing else gives a key authority:
there is no registry, stake, or allowlist. The buyer chooses:

| Where the buyer posts | How to name evaluators |
|---|---|
| Directly on the chain (PostJob) | `evaluators` and `threshold` in the body |
| `pearl job post` | `--evaluator <base58>` (repeatable) and `--threshold n` |
| storefront-x402 `POST /jobs` | `spec.evaluators: ["base58", ...]`, `spec.threshold: n`. Omitted: the storefront's default set (`GET /evaluators`) |

Evaluators are unpaid in this version. A buyer-paid evaluator fee and bonded evaluators
are later work (research/agents/REPUTATION.md 5.3); no emissions or pool money ever pays
evaluators.

## 2. What is signed

Raw ed25519 (no framing) over these exact bytes; `job_id`, `deliverer`, `result_hash` are
the job's current delivery as the node reports it (`getJob`):

| Verdict | Message | Tx that carries it |
|---|---|---|
| accept | `"pearl-sol:result:v1" \|\| job_id [32] \|\| deliverer [32] \|\| result_hash [32]` | CompleteJob (kind 4): W goes to the deliverer |
| reject | `"pearl-sol:reject:v1" \|\| job_id [32] \|\| deliverer [32] \|\| result_hash [32]` | RejectResult (kind 13): the job reopens, the failure is recorded against the deliverer |

The two domains differ, so an accept signature can never be replayed as a reject or the
reverse. Byte vectors: `chain/agent-types/vectors/messages.json` (result) and
`chain/agent-types/vectors/reject.json` (reject message and a RejectResult tx). Anyone may
send the tx carrying the signatures; it pays the fee.

## 3. The check for a plain job (matmul)

Inputs from the chain: the job (`getJob`: m, n, config with k, hash_a, hash_b, data_uri,
deliverer, result_hash, result_uri, evaluators, threshold), the poster's data at
`data_uri`, the delivered C at `result_uri`. `job_key` is the Pearl job key of the job's
header and config (pearl-job, the node reports it as `job_key`).

1. Load A (m x k) and B (k x n) from `data_uri`. Recompute the commitments
   `pearl_job::commit(job_key, A, B)` and require them equal to `hash_a`, `hash_b`.
   If the data is missing or does not match, the fault is the poster's: **hold** (sign
   nothing). A deliverer is never rejected for the poster's data.
2. Fetch C from `result_uri` (`blob:` from the node's blob store, or `file://` in tests).
   Not fetchable: **hold**, and after `unavailable_blocks` blocks (default 20) since the
   evaluator first saw the delivery, **reject** ("result data unavailable"): a deliverer
   that never publishes C has not delivered.
3. C must be exactly `m x n` i32 little-endian row-major (`m*n*4` bytes); else **reject**.
4. `blake3_keyed(C, job_key)` must equal the job's `result_hash`; else **reject**.
5. Freivalds: for `r` random vectors x, require `A (B x) == C x` (default r = 2). On a
   mismatch **reject**: a correct C always passes, so a failure proves C != A B. A pass
   on a wrong C has a small probability per round that shrinks with every extra round;
   raise `--freivalds-rounds` if you want more.
6. All passed: **accept**.

Rejects come only from steps 2 (after the grace), 3, 4 and 5, each a fact about the
deliverer's own bytes that anyone can recheck.

## 4. The check for an inference ticket

Exactly the ticket rule of SPEC 5.4 (services/attester `check_ticket`): the response blob
hashes to the ticket's `result_hash` and names the ticket's model, request and rounds; every
round has one segment per registered matrix with the right `m`; for every segment, A (blob)
times the registered B (checked against `hash_b`) gives a C whose keyed hash is the
segment's committed `c_hash`.

The reference evaluator **accepts** a ticket when this passes and **holds** otherwise: it
never rejects a ticket automatically yet. Two reasons: the M1 ticket check does not cover
the fp32 glue between segments (norms, attention, sampling), so a pass is not a full
replay of the forward pass; and its failure cases are not yet split into provable ones
and transient ones (missing blobs). An operator who has checked a failing ticket by hand
can still sign a reject with the message above. Segments (the standing jobs of a ticket)
have no delivery and are never evaluated.

What this does NOT claim: that an accepted ticket's output text is the model's correct
output. It claims the committed GEMMs are exact. Replay with a mismatch budget is the
follow-up (REPUTATION 5.3).

## 5. Running the reference evaluator

```
cargo build --release -p pearl-attester          # in crates/ (builds pearl-attester and pearl-evaluator)
pearl-evaluator --keypair my-evaluator.json --rpc https://<node>   # your key; never share the file
```

- It polls `listJobs {status: "delivered"}` every `--interval` seconds (default 5), keeps
  jobs whose evaluator list contains its key, checks each delivery once and keeps the
  verdict.
- When its own signature meets the job's threshold (threshold 1), it sends CompleteJob or
  RejectResult itself. The key then needs a balance for the tx fee (`min_fee`, getParams);
  without one it only signs and publishes. `--no-submit` never sends.
- Threshold > 1: list the other evaluators with `--peer <url>` (repeatable). It reads their
  `GET /signatures/<job_id>`, keeps only valid signatures from the job's set over the same
  message and verdict, and sends the tx once there are enough.
- HTTP (default 127.0.0.1:10667, `--port 0` turns it off): `GET /health`, `GET /verdicts`
  (all signed verdicts with reason, message, signature, tx hash), `GET /signatures/<job_id
  hex>`, and the attester's `POST /result` (sign on request).
- `--once` runs one pass and prints the verdicts as JSON.
- Flags: `--unavailable-blocks n` (0 = never reject for missing C), `--freivalds-rounds r`,
  `--models <dir>` (model store for tickets).

Holding the key file is holding the authority: keep it 0600 and off shared machines.

## 6. What profiles show

The explorer's agent profile (`/agent/<addr>`, `pearl agent <addr>`, docs/EXPLORER.md)
reads the signer keys from each CompleteJob and RejectResult tx body and shows:
- for a seller: the share of its completed work (by P) signed by evaluators that are
  neither the buyer nor the seller, and who signed each job;
- for an evaluator: jobs signed, P of those jobs, distinct sellers and buyers, rejects
  signed, and the share where it was also the deliverer or poster.

No rating, stake, or count-only ranking is shown (REPUTATION 5.6).

## 7. Not yet

- Evaluator fee E paid on CompleteJob (buyer-paid; default is an owner decision).
- Equivocation reports (one key signing accept and reject, or two result hashes, for one
  job): detectable from published verdicts and txs today, no tx acts on them.
- Bonds, random second checks, Merkle-structured commitments for entry-level fraud proofs.
- Gateway inference tickets: the buyer naming evaluators goes through the gateway's API,
  which is not part of this change.
