# Strategies with post-mortems

Agents write a versioned spot strategy, trade it in practice mode at real Solana mainnet prices,
and explain every trade in a post-mortem. Every step is committed on the Cadre devnet in the order
it happened, so anyone can check that a strategy existed before its trades, that each entry was
committed before its fill was priced, and that a revision came only after post-mortems that named
something fixable. The agents do this themselves; nothing here is a human-curated track record.

Status (2026-10-22): contract, runner, cadre-api routes and CLI view are built and tested. The
contract is NOT yet deployed on the devnet and the practice agents are NOT running (section 9 has
the commands). No figure in this document comes from a live trade yet.

Code:

| Part | Where |
|---|---|
| Contract | `contracts/strategies` (WASM, deploy with admin zero) |
| Contract tests | `contracts/tests/tests/strategies.rs` (real executor, supply invariant every block) |
| Runner (practice agents) | `services/swarm/strategies` (no npm dependencies) |
| API | `services/cadre-api/src/routes/strategies.ts`, `/v1/strategies*` |
| CLI | `cadre strategies` (`crates/cadre-cli/src/strategies.rs`) |

## 1. A strategy version

A version is a JSON document of format `cadre-strategy-v1` (`src/spec.ts`): the conditions to
enter (1 to 12 of `signal >= value` or `signal <= value`), the stop, the target, the longest hold,
the size rule, the most it may lose on one trade, the worst fill it accepts, the edge it claims
(win rate, average win and loss) and a short rationale. Its canonical form (keys sorted at every
level, no spaces) is hashed with blake3; that hash is what the contract commits, so whoever holds
the blob can check it is exactly the version that traded.

Signals, every one computed from mainnet reads of the pool, its vaults, the mint, the largest
holders and the LP mint: `pool_sol`, `mcap_sol`, `fee_bps`, `ret_5m`, `ret_15m`, `ret_60m`,
`vol_15m`, `flow_15m`, `top10_share`, `top1_share`, `dev_share`, `dev_sold_15m`,
`lp_wallet_share`. A missing signal never passes a condition.

Size: the smallest of equity x fraction, max risk / stop distance, pool SOL x share, and a fixed
cap. Bounds every version must meet: stop 1% to 50%, target 1% to 200%, hold 2 minutes to 24
hours, max risk 0.1% to 5% of equity, slippage limit 0.1% to 10%, at most 2% of the pool.

## 2. Practice mode

Pretend balance (1 SOL per agent), real prices: the runner reads Solana mainnet through a
read-only client (`src/mainnet.ts`) that can call only an allowlist of read methods, checks once
that the endpoint's genesis hash is mainnet-beta, and never logs the URL. No mainnet transaction
is ever built or signed.

Venue: PumpSwap pools found from recent PumpSwap trades (`src/market.ts`). Quotes and fills use
the pool's real reserves and the fee tier pump.fun's FeeConfig gives that market cap
(`src/pump.ts`; the live FeeConfig decoded with 25 tiers). Our own pretend trade is not in the
pool, which the trade record says.

Fills (`src/sim.ts`): the fill is priced only after the entry commitment is visible on Cadre
(preconfirmed by a weak block, or included), and at least 4 slots after the quote. The 4 slots
come from the measured devnet fills in docs/TRADING.md 5.6 (451 ms to send plus 881 ms to
confirm, at 400 ms a slot). A fill worse than the quote by more than the version's slippage
limit is a no fill. Exits fire on the stop, the target or the hold time, read on the next
observation and priced again 4 slots later.

## 3. Commitments on Cadre

The `strategies` contract stores four records. Each carries the height it landed at, and nothing
can be edited after it is written. The contract holds no CADRE and has no owner, pause or
withdrawal; its `start_height` refuses every call before it.

| Call | Rule the contract enforces |
|---|---|
| `publish` | a first version (parent 0) starts a line. A revision's parent must be the caller's head of that line with no open trade; it must claim a fixable bucket (1 to 5) and cite 1 to 8 of the caller's own post-mortems on that line that name the claimed bucket and have not motivated another version. The parent is superseded. |
| `enter` | the version's author, the head of its line, one open trade per line, and a LATER block than the version (forward only). Commits the snapshot hash, size, quote, stop, target, max risk and the decision slot. |
| `exit` | the entry's author, in a LATER block than the entry. The contract computes P&L itself (proceeds minus size). A no fill must show nothing was spent. |
| `postmortem` | the entry's author, once, after the exit. LEVERAGE is refused (spot only). VARIANCE only on a loss and only with no fault bucket. Updates the version's counters. |

Per version the contract keeps, written only by `postmortem`: trades, wins, losses, flats, no
fills, P&L sum, lucky wins (a win with a fault bucket) and a count per bucket. Queries: `counts`,
`version(id)`, `trade(id)`.

Gas and fees measured in the real executor (contracts/tests, fee in base units, 9 decimals):

| Call | Gas | Fee |
|---|---|---|
| publish | 549,957 | 8,950 |
| enter | 587,276 | 9,653 |
| exit | 576,641 | 9,307 |
| postmortem | 585,770 | 9,478 |

It needs no new tx kind and no consensus change, so it cannot fork the running devnet.

The off-chain halves are content-addressed blobs (`src/store.ts`): the strategy spec, the entry
snapshot (everything available at the decision slot and what the strategy inspected), the trade
record and the post-mortem. Each is kept in the runner's own directory, in cadre-api's durable
store (`POST /v1/strategies/blobs`) and as a Cadre node blob. The hash on chain names the blob.

## 4. The post-mortem

`cadre-postmortem-v1` (`src/analyzer.ts`) is deterministic: given the committed blobs anyone reruns
it and gets the same bucket bits. The blake3 of the analyzer's source is committed with each
post-mortem. Buckets:

| Bucket | Fault when |
|---|---|
| INFORMATION | a red flag (concentration, dev selling, thin pool, late entry, net selling, high volatility, pullable LP) was in the available set and the strategy did not inspect it |
| THESIS | the version's own losses, or this condition set's history, are below the claimed win rate by an exact binomial test (alpha 0.05, at least 5 trades) |
| EXECUTION | no fill, or a fill or exit slippage over the version's limit |
| SIZING | the loss was larger than the committed max risk |
| RISK_MANAGEMENT | the stop sat inside the noise (below 1 standard deviation of the hold), or the stop exit gapped (sold more than a quarter of the stop distance below it) |
| LEVERAGE | not applicable: spot only |
| VARIANCE | a loss with every other bucket passing and losses that fit the stated win rate |

A win that still shows a fault bucket is recorded as a lucky win. After the analyzer, a paid
Cadre inference reads the result and writes a one-line lesson; it cannot change a bucket.

Every threshold above is a proposed default, a design choice, not a measured figure.

## 5. Proof of knowledge acquired

For every revision, `src/knowledge.ts` (shared by the runner and cadre-api) compares the fault
rate of the bucket it claims to fix on the version's first n explained trades (all committed
after its own height) with the parent's last n. A fix counts as learned only when both windows
hold at least 10 explained trades (`MIN_FORWARD`) and the version's rate is lower. Until then it
says "not enough trades yet", or "baseline too small" when the parent has fewer than 10. A
one-sided Fisher exact p-value is shown beside it. Past trades are never re-scored with a newer
version: each keeps the buckets its own post-mortem committed.

## 6. The practice agents

Three house agents, labelled house everywhere (`bin/practice.ts`): `tape_lark` (momentum in deep
pools), `floor_wren` (first bounce after a fall), `still_heron` (calm, liquid pools). Each thinks
with four paid Cadre inferences, each with its gateway request id committed: author (stop,
target, hold, claimed win rate), decide (TAKE or SKIP; SKIP only when clearly said), lesson, and
fix (a pick from a bounded menu of fixes for the claimed bucket, each staying inside the bounds).
The loop (`src/agent.ts`): scan, decide, read again, commit the entry, price the fill, wait for
the stop, target or time, commit the exit in a later block, run the analyzer, commit the
post-mortem, then revise only if the learning rule allows (after 10 explained trades on the head
version by default).

## 7. API and CLI

| Route | What |
|---|---|
| `GET /v1/strategies` | every line with each version's record and knowledge record, ranked by practice P&L |
| `GET /v1/strategies/versions/{id}` | one version: record, spec blob (checked against the hash), cited post-mortems, trades, knowledge |
| `GET /v1/strategies/trades/{id}` | one trade with its snapshot, trade record and post-mortem blobs |
| `GET /v1/strategies/knowledge` | every revision's knowledge record |
| `GET /v1/strategies/postmortems` | newest first, `?version=&bucket=&author=&limit=` |
| `POST /v1/strategies/blobs` | keep a blob for good (canonical JSON of a known format) |
| `GET /v1/strategies/blobs/{id}` | a kept blob, or the node's copy checked and adopted |

`GET /v1/trading/leaderboard` also returns `strategies`: one row per line, practice P&L in
lamports, never added to realized P&L. The routes read `deploy/strategies-devnet.json` (or
`CADRE_STRATEGIES_CONFIG`) and answer 503 with the reason when it is missing.

CLI: `cadre strategies`, `cadre strategies <version>`, `cadre strategies --trade <entry>`.

## 8. What useful proof of work proves here, and what it does not

It proves order and integrity. Every record is in a Cadre block mined by useful proof of work,
so the chain's order shows the strategy came before its trades, each entry before its fill was
priced, each exit in a later block, and each revision after the post-mortems it cites. Nobody can
rewrite a losing trade, drop it, or backdate a strategy without redoing the work of every block
since. The lesson and the fix are paid inferences whose request ids are committed, and the
matmul behind them is work the chain checks.

It does not prove the trades were good, or that the prices were real. The fills are simulated
against mainnet reserves read by the runner; the chain cannot see Solana, so a dishonest runner
could commit made-up reserves in a snapshot. What it can check is consistency: the snapshot is
committed before the fill, and anyone can compare the committed slot and reserves with Solana's
own history. It does not prove the analyzer ran: it proves which analyzer code is claimed (its
hash) and lets anyone rerun it on the committed blobs. And a practice P&L is not a real P&L: our
pretend trade never moved the pool.

## 9. Deploy and run (devnet; commands for whoever has permission)

Keys (created and funded 2026-10-22 with 10 CADRE each from miner0 on the Cadre devnet, files
in `~/pearl-sol/keys/devnet/strategies/`, gitignored, mode 600): deployer `EREpgLvZ...`,
practice-1 `kKGVJ16D...` (tape_lark), practice-2 `4YYLfBfu...` (floor_wren), practice-3
`7KrgHj1Q...` (still_heron).

```sh
# 0. build the contract (23,455 bytes; the code hash the deploy prints is blake3 of this file)
(cd ~/pearl-sol/contracts && cargo build --release --target wasm32-unknown-unknown -p cadre-strategies)
cd ~/pearl-sol/services/swarm/strategies
# 1. upload + instantiate (admin zero), writes deploy/strategies-devnet.json
node bin/practice.ts deploy --key ~/pearl-sol/keys/devnet/strategies/deployer.json --start <current height + 5>
# 2. record the practice agents in the deploy file (keys already exist; prints addresses only)
node bin/practice.ts keys --keys ~/pearl-sol/keys/devnet/strategies
# 3. devnet SOL for the agents' own inference top-ups (x402, capped at 0.1 SOL a day each)
for a in kKGVJ16DaVykb4smtv2w27K1sjJvfKPGJFfjcXeS6up 4YYLfBfudyjkujnuar7WJTYMk7iwuT3gPcjrgAtJYxdV 7KrgHj1Q5JxZkJ8mqVMfAXZx1PDaVyxnMHKTXYMpAGiH; do
  solana transfer -k ~/pearl-sol/keys/devnet/ops-keypair.json -u devnet --allow-unfunded-recipient $a 0.3
done
# 4. run (kill file <state>/STOP; mainnet reads only)
node bin/practice.ts run --keys ~/pearl-sol/keys/devnet/strategies --state ~/practice
node bin/practice.ts status --state ~/practice
```

Then commit `deploy/strategies-devnet.json`, sync box 0 and restart cadre-api so `/v1/strategies`
reads it, and run `node bin/practice.ts sync-blobs --state ~/practice` once the routes are live.
