# Cadre chain speed: weak-block pre-confirmations and a 30 s target

Owner decision 2026-10-21 ("yeah proceed on speed then pls"), the plan in docs/CONTRACTS.md
section 10: first soft pre-confirmations from weak blocks (no change to finality or the Solana
light client), then a moderately shorter block target (30 s) that still credits orphaned useful
work. No signing committee, no BlockDAG. Research with sources: research/SPEED.md.

Status and measured figures: section 7 (filled in as they are measured; nothing here is
modelled unless it says so and shows its inputs).

## 1. What changes, in one table

| | Today | Phase 1 (p2p only) | Phase 2 (consensus, devnet height `S`) |
|---|---|---|---|
| block target | 60 s | 60 s | 30 s from block `S + 1` |
| first signal for a tx | inclusion, about 30 s mean wait + propagation | **preconfirmed**: a weak block on the tip lists it, about 1.9 s mean | same, about 1.9 s |
| finality K | 30 blocks (30 min) | 30 | 60 blocks (30 min) |
| coinbase maturity | K blocks | K | K (60 = 30 min) |
| subsidy per block | pool x 7 ppm | same | pool x 3,500 ppb (half; same per day) |
| stale job blocks | lost | lost | uncle credit: 1/2 at depth 1, 1/4 at depth 2, nephew 1/32 |
| fork choice | heaviest chain by own header work | same | same (uncles do not count) |
| light client | WTEMA at 60 s, K 30 | unchanged | WTEMA target time and K by height (program upgrade) |

## 2. Phase 1: weak blocks

### 2.1 Definition

A weak block is a block header with every field of a valid child of its parent (height, prev_hash,
target_bits = the chain's required target, timestamp rules, job binding, tx_root, state_root,
withdraw_root as the template built them) whose Pearl proof meets the **weak target** instead of
the header's target:

```
weak_target = min(target(header.target_bits) << weak_shift, target(pow_limit_bits))
weak_shift  = clamp(floor(log2(target_block_time_ms / 1875)), 0, 10)   (node policy)
```

`weak_shift` is 5 at 60 s (1 in 32 weak solutions is strong; a weak block about every 1.9 s)
and 4 at 30 s (1 in 16; also about every 1.9 s). Shift 0 (targets under 3.75 s, the demo's 1.5 s)
turns weak blocks off. The proof format is unchanged (crates/pearl-job PROOF-FORMAT.md): the
jackpot bound is linear in the target, so the same tile check runs with the weak target. The compact encoding keeps a 23-bit
mantissa, so the weak target can sit up to 2^-15 below the exact `target << shift`. A proof that
meets the strong target is a block and is submitted as one (it is not also counted as a weak
block: the tip moves anyway). A full-strength proof sent to `submitWeakBlock` is accepted as a
weak block.

Weak blocks are free: they are the miner's own nonce attempts that land between the weak and the
strong bound, found by the same matmul search (no extra useful work, research/SPEED.md 7).

### 2.2 Wire message

Weak blocks travel on the existing **tips** channel (CH_TIPS = 2). A tip announcement is exactly
40 bytes and nodes from before Phase 1 ignore every other length on that channel without scoring
the peer, so a mixed-version network needs no handshake or channel change (a new channel id would
cost the sender 10 ban points per message on an old node, chain/net/src/open/actor.rs).

```
weak msg = "CWB1" (4) || header [256] || proof_len u32 || proof || item_count u32
           || item hash [32] * item_count || flags u8 (bit 0: the items include a contracts record)
items    = the tx_root leaf items in block order: tx hashes, the contracts record hash (if flag),
           the coinbase receipt hash (always last)
```

Checks on receipt, in order (cheap first):
1. decodes, `item_count >= 1`, `item_count <= max_block_txs + 2`, total size <= 4 MiB;
2. not seen before (by header hash); the parent is a live block on this node, at most 2 below
   the tip (older parents are dropped silently);
3. `merkle::root(items) == header.tx_root` (a node cannot tell from hashes alone that the last
   item is the coinbase receipt; carrying the 97-byte receipt would allow it and was not needed);
4. version, height = parent + 1, `target_bits = required_bits(parent)`, timestamp > median time
   past and <= now + max_future_ms;
5. the job named by `job_id` exists in the parent's state (fallback blocks: none needed);
6. `proof_hash` matches and the proof meets the weak target (`pearl_job::pow::verify_block_at`).

A message failing 3-6 is invalid: the peer is blocked like a peer sending an invalid block
(`ban_worthy`). A valid weak block is relayed to all peers once. Per parent a node keeps at most
256 weak blocks (more are dropped, oldest first); weak state for parents more than 2 below the
tip is pruned.

### 2.3 Miner side

`getBlockTemplate` adds `weak_target_bits` (compact) and `weak_shift`. A miner that knows them
mines each nonce against the weak bound; a hit that also meets `target_bits` goes to
`submitBlock`, any other hit to `submitWeakBlock {header, proof, template_id}` (same parameters
as submitBlock). The node builds the weak message from the template it handed out, checks it
like a gossiped one, stores it and relays it. Mining continues on the same template after a weak
hit. Miners that ignore the new fields keep working unchanged. A pool may submit weak blocks for
its workers the same way (not built: services/pool is unchanged in this step).

### 2.4 Pre-confirmation status

For a tx hash X and the node's current tip P, over the weak blocks stored with parent P:

- `weak_count` = weak blocks listing X, `weak_miners` = distinct miner keys among them;
- `weak_share` = weak_count / (weak blocks with parent P seen at or after the first one that
  listed X): the share of the hashrate seen on P since X first appeared that committed to X;
- `first_seen_ms`, `latest_age_ms` (now minus the newest weak block listing X);
- `confidence`: `low` (one weak block or one miner), `medium` (2+ weak blocks from 2+ miners),
  `high` (4+ weak blocks, 2+ miners, weak_share >= 0.9). These are labels for the evidence, not
  probabilities (2.5).

Tx status, everywhere (RPC getTx and getTxStatus, cadre-api, explorer, CLI, wallet, trading):

| status | meaning |
|---|---|
| `pending` | in the mempool, no weak block on the tip lists it |
| `preconfirmed` | in the mempool (or not seen there) and listed by at least one weak block on the current tip; carries `preconf {weak_count, weak_miners, weak_share, first_seen_ms, latest_age_ms, confidence}` |
| `included` | in a canonical block, fewer than K deep (`confirmations` 1..K) |
| `final` | in a canonical block at or below the finalized height (cannot reorg) |
| `unknown` | getTxStatus only: the node knows nothing about the hash |

`getTx` keeps every old field (for a tx a weak block lists but this node has not pooled it
returns the status object without the tx body); `status` gains the values `preconfirmed` and `final` (a client that
only knew `included` must treat `final` as included). New `getTxStatus {tx_hash}` returns
`{status, preconf?, block_hash?, height?, confirmations?, final}` without the tx body. WebSocket
topic `weak` notifies `{hash, parent, height, miner, job_id, tx_count, received_ms}` per new weak
block. `getWeakBlocks {parent?}` lists the stored weak blocks of a parent (default the tip).

### 2.5 What a pre-confirmation does and does not guarantee

It guarantees: a miner who controls the header's payout key spent real useful work (1/32 of a
block's work at 60 s) on a block on the current tip that contains X, and every node can check it.
Several weak blocks from several keys mean that much observed hashrate is mining X now.

It does NOT guarantee inclusion. Proof of work has no memory: the next block's miner is drawn by
hashrate whatever weak blocks exist, and nothing forces that miner to keep X. Weak blocks carry no
reward and no penalty in this phase (no StrongChain-style weak rewards; that would be a consensus
change, research/SPEED.md 5 and 10). The honest risk statement, with q = the hashrate share of
whoever wants X out (an attacker mining a conflicting tx with the same nonce, or a censor):

- **Next block excludes X**: at least q (that miner wins the next block and leaves X out), plus
  the share of honest hashrate that has not refreshed its template yet (templates refresh every
  2 s, so at about 1.9 s after X entered the mempool most honest miners have it).
- **X never confirms (double spend before inclusion)**: the attacker must win the next block
  with a conflicting tx; each later block it must win again unless honest miners also accept the
  conflict. Nodes refuse a same-nonce replacement of a preconfirmed tx (2.6), so honest miners
  keep X once they have it, and the attacker's chance per block stays about q.
- **Fake evidence**: weak blocks all from the attacker's own keys look like support. n weak blocks
  in a row being all the attacker's has probability about q^n (q = 0.25, n = 3: 1.6%), which is
  why `weak_miners` and `weak_share` are shown, not only `weak_count`.
- **After inclusion**: the usual PoW double-spend math over confirmations (Rosenfeld 2014;
  research/SPEED.md 8): with q = 0.3 the reorg chance is 15.6% at 6 blocks, 0.128% at 30 and
  5.0e-6 at 60; `final` (K deep) is a protocol rule, not a probability.

Recommended use, by value at risk (stated in every client that shows the status):

| confidence | use it for |
|---|---|
| preconfirmed `low` | UI feedback only ("seen by a miner") |
| preconfirmed `medium` / `high` | small, reversible actions: replies, follows, a quote refresh, a trade under the trading runtime's preconf cap (services/swarm/trading) |
| included | anything the counterparty could refuse to honour later |
| final | bridge, withdrawals, large trades, settlement |

### 2.6 Mempool rule

A pooled tx listed by any stored weak block on the tip cannot be replaced by a same-nonce tx
(the 10% fee bump rule, chain/mempool, does not apply to it) until it is included or the tip
moves to a branch whose weak blocks do not list it. Node policy, not consensus.

### 2.7 Block propagation

Measured first (7.3): the time from a block's arrival to its relay is dominated by proof
verification and execution, not by tx bytes. A strong block whose template a peer has already
seen as a weak block is still sent in full (the proof dominates its size: 24,724 bytes measured, txs are
usually a few hundred bytes). Compact blocks (BIP152 style) are therefore not built; section 7.3
gives the measured split.

## 3. Phase 2: 30 s target with uncle credit

### 3.1 Activation

`speed_upgrade_height(chain_id)`: the Cadre public devnet (chain id 1,128,350,802) at
`DEVNET_SPEED_HEIGHT = S` (S = 6000, about 18:40 UTC 2026-10-22 at 60 s blocks); every other chain 0 (uncle rules from genesis; their target time, K
and emission stay whatever their genesis config says, so existing local chains replay
unchanged because they carry no uncles and no `emission_ppb`).

On the devnet, block `S` runs a one-time config migration before its txs (in
`BlockExecutor::begin`, after maturing coinbase):

```
target_block_time_ms := target_block_time_ms / 2     (60,000 -> 30,000)
finality_k           := finality_k x 2               (30 -> 60)
emission_ppb         := emission_ppm x 1000 / 2      (7 ppm -> 3,500 ppb)
```

So block `S` itself is still validated against the 60 s rule (its parent's config), it matures
at `S + 60` (K in force at the end of the block), and its child `S + 1` is the first block whose
target comes from the 30 s WTEMA. WTEMA keeps its wall-clock half-life (1 h,
`wtema_half_life_ms` unchanged): the time constant of the filter is the half-life in ms whatever
the target, so per block it now moves half as far. No one-time difficulty step: blocks drift from
about 60 s to 30 s as the filter converges; at a 60 s solve time against a 30 s target each block
raises the target by 30/3600 = 0.83%, so the target doubles after about 84 blocks (ln 2 / ln
1.00833), roughly an hour at the shrinking block times. (The faster alternative, doubling the
target at `S + 1`, would add a special case to the light client for a one-hour saving.)

### 3.2 Emission

New config key 45 `emission_ppb` (u64, parts per billion of the pool per block; 0 = unset). When
set it replaces `emission_ppm`: `subsidy = min(pool x emission_ppb / 1e9, job.cap_remaining)`.
3,500 ppb at 2,880 blocks a day gives 1 - (1 - 3.5e-6)^2880 = 1.003% of the pool a day, the same
as 7 ppm at 1,440 blocks (1 - (1 - 7e-6)^1440 = 1.003%). Uncle and nephew rewards (3.4) come on
top, bounded by the uncle rate times their share (7.4 has the measured figure).

### 3.3 Uncles

An uncle is a stale **job block** (not a fallback block) that a later canonical block includes so
its useful work is credited. Constants (consensus):

```
MAX_UNCLES      = 2          per block
MAX_UNCLE_DEPTH = 2          depth d = including height - uncle height, 1..=2
UNCLE_SHARE     = d=1: 1/2, d=2: 1/4   of the uncle's own subsidy
NEPHEW_SHARE    = 1/32       of each uncle's reward, to the including miner
```

A block B at height H on branch b may carry uncles U, each valid when:
1. `U.version` is the header version, `U.job_id` is not zero;
2. `1 <= H - U.height <= MAX_UNCLE_DEPTH`;
3. `U.prev_hash` = the hash of b's block at height `U.height - 1` (U's parent is on the branch);
4. `U.hash` != b's block at height `U.height` (U is not on the branch), uncles in one block are
   distinct, and U was not included by an earlier block of b (state key `uncle(U.hash)` absent in
   B's parent state);
5. `U.target_bits` = the target_bits of b's block at `U.height` (siblings share a parent, so they
   share the required target), and `U.timestamp_ms` > median time past of U's parent;
6. the job record `U.job_id` exists in B's parent state with `created_height < U.height`, and
   `pearl_job::pow::verify_block(U, job, proof)` passes at U's own target.
7. from `uncle_job_rule_height` (devnet 10,000, every other chain from the uncle activation):
   the job was mineable at `U.height` by the canonical job-block rule (`executor::check_mineable`
   on B's parent state: live status, `U.height <= deadline_height`, `blocks_mined < max_blocks`,
   cap left, not a ticket). Audit 2026-10-23: without it an uncle could name a Completed or
   Expired job with cap left, or one whose `max_blocks` its sibling used up, and be paid from the
   pool. A stale block that loses the last block of a job, or whose job completes before the
   uncle is included, earns nothing.

U's txs, state_root and withdraw_root are not checked and not executed (as in Ethereum): an uncle
pays for its work, nothing else. Fork choice is unchanged: the heaviest chain by the work of its
own headers; uncle work is not counted (Ethereum did the same; counting it would make the Solana
light client verify uncle proofs at about 7M CU each, research/SPEED.md 4 and 10).

Rewards, executed in `finish` after the block's own subsidy, uncles in body order, each:

```
base   = min(pool x rate, job.cap_remaining)       rate = emission_ppb / 1e9 (or emission_ppm / 1e6)
reward = base x share(d)                            (floor)
nephew = reward / 32                                (floor)
both are debited from the pool and from job.cap_remaining (the job's alpha x P bound holds);
reward matures to U.miner, nephew to B's miner, at B.height + K (the Immature entries, so payout
liens split them like any coinbase); job.blocks_mined is NOT incremented (max_blocks counts
canonical job blocks); state key uncle(U.hash) := H.
```

Commitment: each uncle adds one tx_root leaf item, after the txs and the contracts record and
before the coinbase receipt:

```
uncle record (137 bytes) = 0x03 (version) || height u64 (H) || uncle_hash [32] || uncle_height u64
                           || miner [32] (U.miner) || job_id [32] || reward u64 || nephew u64
                           || mature_height u64
item hash = blake3(record)
```

Version 0x03 starts no tx (0x01), no coinbase receipt (0x00) and no contracts record (0x02), so
the leaf-type argument of SPEC 3.4 still holds; a record is provable on Solana by tx_root
inclusion under a final header, like the coinbase. Body encoding, after the coinbase and the
optional contracts section: `0x02 || count u8 || (uncle header [256] || proof_len u32 || proof ||
record [137]) * count`. A block without uncles encodes exactly as before. The node requires the
body's records to equal the ones execution produced ("uncle record mismatch").

Templates offer uncles automatically: live, non-canonical job blocks this node connected, eligible
under the rules above for the template's parent, highest reward first. A miner needs no change.

As built (317e61a, 7610305, 49c6700): an uncle on an inference segment job moves only the job's
own totals (cap_remaining, subsidy_paid), not its ticket's or model's paid-subsidy rollups; weak
messages carry the template's uncle item count in flag bits 1-2 (the uncle record items sit
between the contracts record and the coinbase, so the tx hashes are found by counting back);
test-only chain ids `0x7E570000 + h` migrate at height h; `getChainInfo.speed_height` is the
migration height (null without one), `getParams` has `speed_height` (uncle rules, 0 off devnet)
and `speed_migration_height`.

### 3.4 Parameters at 30 s, with reasons

| parameter | 60 s | 30 s | why |
|---|---|---|---|
| target_block_time_ms | 60,000 | 30,000 | owner plan |
| wtema_half_life_ms | 3,600,000 | 3,600,000 | wall-clock filter constant (3.1) |
| finality_k | 30 | 60 | same 30 min wall clock; the bridge and proof readers wait the same time |
| coinbase maturity | K | K | follows K (30 min) |
| emission | 7 ppm | 3,500 ppb | same per day (3.2) |
| max_job_blocks | 100,000 | 100,000 | an upper bound only; 100,000 blocks is 34.7 days at 30 s |
| max_future_ms | 120,000 | 120,000 | clock tolerance, not a block count |
| weak_shift (policy) | 5 | 4 | weak block about every 1.9 s either way |
| block_gas_limit | 150M | 150M | 1.1 s worst case (CONTRACTS 10) is 3.7% of 30 s |

Job and inference economics: a job's cap `alpha x P` now drains at half the subsidy per block, so
the same cap lasts twice the blocks (the same wall time at the same emission). Posters who set
`max_blocks` or `deadline_height` in blocks get half the wall time for the same number; the
gateway already converts with `target_block_time_ms` from getParams (SPEC 5.4 ticket budget), so
new tickets keep their wall-clock life; tickets posted before `S` keep their block counts. The
chain mines 2,880 job blocks a day instead of 1,440, so a 1B inference round (144 segments) takes
72 min of the chain's whole capacity instead of 144, an 8B round (640 segments) 5.3 h instead of
10.7 h.

### 3.5 Light client (Solana)

Header validity changes only in two numbers by height, so the program gets a schedule, not a new
rule: `Config` gains `speed_height u64, speed_target_block_time_ms u64, speed_finality_k u32`
(admin `set_speed`, which reallocates the account once). `submit_header` uses
`speed_target_block_time_ms` for headers with `height > speed_height`; the final floor uses
`speed_finality_k` once the tip height is `>= speed_height`. Uncles and weak blocks are not seen
by the light client. Relay cost per day doubles with the header rate (7.5 has the measured SOL/day
before and after).

## 4. Rollout order

1. Phase 1 node + miner binaries (no consensus change): any time, box by box; weak gossip is
   inert toward old nodes.
2. Light-client upgrade with `set_speed(S, 30000, 60)`, then header-relay if it changed, before
   the chain reaches `S`.
3. Phase 2 node binary on every box before `S` (old nodes fork off at `S`: they reject block `S`'s
   config migration result, state_root mismatch).

The exact commands: docs/HANDOFF.md item 31.

## 5. Tests

Listed with results in docs/TEST-REPORT.md "Chain speed".

## 6. Not built

Weak-block rewards (StrongChain/Tailstorm style), compact blocks, weak blocks from pools, a
finality committee, a DAG.

## 7. Measurements

All on 2026-10-21/22. Raw logs: test box `~/speed/p1net` (7.1-7.2), `~/speed/soak` (7.4 soak); the
propagation runs were collected to the orchestrator's scratchpad (`speed/prop/`).

### 7.1 Proof and weak-block sizes, verify time

Real Pearl fallback proofs (k 2048, r 128, 4x8 tile), test box: proof 24,724 bytes (SPEC 3.1's
"about 12.6 KB" predates the rank-penalty port); weak message 25,025 bytes median; `check_weak`
427 us median, 1,128 us p90. At one weak block per 1.9 s that is about 13 KB/s per peer link and
under 0.1% of a core.

### 7.2 Pre-confirmation latency (Phase 1)

3 nodes on the test box, real verifier, 60 s target (66.2 s mean over the window), 4 one-thread
miners, 36 transfers 60 s apart, each read on a node other than the one it was sent to
(`agent-node preconf-probe`):

| | median | p90 | max |
|---|---|---|---|
| sent -> preconfirmed (other node) | 1,052 ms | 3,744 ms | 9,328 ms |
| sent -> included | 41,234 ms | 234,080 ms | 414,124 ms |
| weak blocks per block | 31 | 97 | 184 |

All 36 were preconfirmed before inclusion. With only two payout keys among the miners, 35 of 36 were
`low` at first sight (the confidence label needs 2+ miners).

### 7.3 Propagation split

Per gossiped block on loopback: decode 6-18 us, PoW verify 423 us median (1,040 p90), execution
152 us (406 p90), whole add_block 7.2 ms median (12.5 p90, mostly state build and the archive
write); origin submit to first peer receive 7 ms median. Tx bytes are not the cost, so compact
blocks were not built (2.7).

### 7.4 Orphan (stale) rates at 60 s and 30 s

Cross-box test (4 nodes: test box + devnet boxes 0-2, own chain, full mesh over ssh tunnels,
fallback blocks with real proofs, one miner thread per box; devnet services untouched):

| run | target | blocks | mean interval | stale | per-peer delay median / p90 / max |
|---|---|---|---|---|---|
| p60 | 60 s | 32 | 120.5 s (under-calibrated) | 0 | 12 / 20 / 156 ms |
| p30 | 30 s | 79 | 40.5 s | 0 | 12 / 21 / 79 ms |
| p3 (amplifier) | 3 s | 1,313 | 3.9 s | 7 (0.53%) | 11 / 17 / 25,835 ms (one ssh stall) |

Live devnet (heads on all three nodes from the test box, 2026-10-21 21:16 to 2026-10-22 12:20 UTC,
heights 4733-5624, 892 blocks, 60.8 s mean): 4 depth-1 reorgs (4951, 5000, 5119, 5408), a stale
rate of about 0.45% (a lower bound: the monitor reconnected a few times). Inverting
stale = 1 - exp(-tau/T) gives an effective tau of 60 x -ln(1 - 0.0045) = 0.27 s on the live
devnet (job blocks, loaded box 0), against 0.012 s for empty fallback blocks between idle nodes.
At 30 s the same tau gives 1 - exp(-0.27/30) = 0.9%: those stale job blocks are what uncles credit
(1/2 at depth 1). The three boxes share one region; miners elsewhere add their link latency to tau
(research/SPEED.md 9: 2 s of delay is 6.45% stale at 30 s).

Phase 2 soak (test box, 4 nodes, migration at height 15 on a test chain id, about 8.8 h): 973
blocks, 31.5 s mean after the migration (28.9 s over the last 30), finality lag exactly 60, tips
equal on all four nodes in 524 of 524 one-minute samples, 1 stale block (a fallback block, which
the uncle rule excludes), 0 uncles (fallback-only mining; uncles are proven by the network tests
with 300 ms links: 6 of 6 racing rounds credited the stale block as an uncle on every node).

### 7.5 Light-client cost (Solana devnet, measured)

Relay txs of boxes 1 and 2 over the 24 h before slot time 1792618550: 62,251 txs (31,011 + 31,240),
all one signature at 5,000 lamports and no priority fee, for 1,537 headers (56.2 s mean blocks):
0.3113 SOL/day, 40.5 txs and 202,509 lamports per header. Per day, (202,509 + priority x 12.2M CU)
x headers:

| priority (micro-lamports/CU) | 60 s (1,440 headers) | 30 s (2,880 headers) |
|---|---|---|
| 0 | 0.2916 SOL | 0.5832 SOL |
| 1,000 | 0.3092 SOL | 0.6184 SOL |
| 10,000 | 0.4673 SOL | 0.9346 SOL |

CU per header after the schedule change: submit_header 31,746 -> 31,818, completing jackpot
640,831 -> 640,893 (about +130 CU of about 10.2M; every tx under 1.4M; tx count unchanged).
Refundable rent: +167,040 lamports once (schedule bytes) and about +0.125 SOL locked while K = 60
keeps 30 more header records (4,176,000 lamports each).
