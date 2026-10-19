# Agents on Cadre: combined plan (2026-10-18)

Sources: BUYERS.md (76a3065), TOKENOMICS.md (aa08430), REPUTATION.md (cc1f13b). Every
figure is quoted from those files, with URLs inside them; nothing new is introduced here.

## What the three agree on

1. Agents buy inference through prepaid balances and OpenAI-compatible endpoints, not per-call
   chain payments. x402 traffic is mostly data (inference 1.6% of top-100 Bazaar calls) and
   largely gamed (63.94% of x402 txs per 4Pillars; TRM finds 0.6-7.5% agentic).
2. Counts are worthless as signals, for demand (x402 tx counts), for reputation (ERC-8004:
   59-91% of reviewers sybil) and for rewards (Virtuals ACP fees -98.6% after prize epochs).
   Paid volume from distinct payers, self-dealing excluded, is the signal that holds.
3. Cadre's own rules already make faking expensive: a completed job loses at least
   (1 - alpha) x P = 0.2 P to the pool, plus the 10% buyback share via the storefront, so a
   fake round trip costs at least about 15.7%. Any incentive above that buys fake volume.
4. Value held where real spend was locked into the token (Venice DIEM: about 10% of VVV locked
   for credit) or bought it by rule (Hyperliquid). Activity rewards and subsidised usage died.

## Build, in order

| # | What | Why | Effort | Source |
|---|------|-----|--------|--------|
| 1 | Prepaid balance + signed requests (or x402 batch-settlement), many requests settled as one order per period | Today each request costs 2 Solana txs; buyers use prepaid | M-H | BUYERS 1 |
| 2 | OpenAI-compatible `/v1/chat/completions` (streaming, tools, `/v1/models` with prices) on the gateway | "Change the base URL" is what frameworks adopt | M | BUYERS 2 |
| 3 | `cadre-proxy`: local OpenAI endpoint with its own Solana wallet and spend caps; configs for Hermes, OpenClaw, Kilo, Cline | ClawRouter pattern (6.6k stars) | L-M | BUYERS 3 |
| 4 | Agent profile from an indexer: paid P and SOL, distinct payers, top payer share, self-dealt share excluded, delivered-then-expired, acceptance rate weighted by P, third-party evaluator share, mining record. Explorer page + `pearl agent` | Only unfakeable signals; no new chain state | L | REPUTATION 1 |
| 5 | Receipts on every response: ticket, order, fee, explorer link | Ties usage to "every block was paid for" | L | BUYERS 7 |
| 6 | Buyback to reserve up to `bb_reserve_cap`, burn above; `fund_mining` permissionless and rate limited | Keeps the reserve decision, removes the admin-key exit, avoids an overhang | L | TOKENOMICS 1-2 |
| 7 | Public numbers: DefiLlama adapter, circulating-supply endpoint, revenue after wash screens with unique payers | Others can check us | L | TOKENOMICS 3 |
| 8 | Product shape: short-context and batch work (classify, extract, judge, generate), priced at or under the floor ($0.02 / $0.04 per M for 8B); Llama 3.1 8B and Qwen3.5-9B first; measure our tokens/s first | Coding agents need 88K-124K contexts on big MoE models; we cap at 4096 | M | BUYERS 4-6 |
| 9 | Agent card at `RegisterAgent.uri`, health probe, stale profiles hidden; gateway picks servers by record (liveness, acceptance, inverse price squared, probation lane) | OpenRouter / Vast patterns | L-M | REPUTATION 2, 4 |
| 10 | Open evaluation: publish the check spec + reference attester, buyers name evaluators, then the int7 replay check with a mismatch budget | Needed before any "verified" claim | M | REPUTATION 3, 5 |

Later, after paid inference exists: stake-for-credit (daily credit at most 10% of trailing paid
spend, non-transferable, funded from the released buyback share); `RejectResult`;
buyer-paid evaluator fee; bonds in CADRE with burn on provable fault only; x402 Bazaar listing;
USDC on Base; OpenRouter provider application.

## Do not build

Token buy per request; our own payment protocol or an SDK frameworks must adopt; card/AP2
integrations now; 7-8B pitched at coding agents; free tier paid by block rewards; star
ratings or open feedback; leaderboards by counts; stake-weighted curation; evaluator pay from
emissions or the pool; slashing for anything the chain cannot prove; "verifiable AI" claims
before the replay check; raising the buyback above 10%; transferable credit; a CADRE treasury
or supply expansion; reporting raw x402 or tx counts.

## Owner decisions

1. Payment model for inference: prepaid balance (recommended) vs keep per-request storefront.
2. Buyback reserve cap (then burn above it), and making `fund_mining` permissionless.
3. Plain jobs: keep first-come delivery (griefable until deadline) or add `RejectResult` now.
4. Evaluator fee default and whether bonded evaluators come before fraud proofs (recommend: later).
