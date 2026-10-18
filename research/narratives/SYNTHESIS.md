# What to add: ranked synthesis (2026-10-18)

Sources, all read 2026-10-17/18 with URLs inside each file:
CRYPTO-COMPUTE.md, EQUITIES-COMPUTE.md, FLOWS-AND-PUMPS.md, COMPUTE-TECH.md, VIRALITY.md.
Every figure below is quoted from those files; nothing new is introduced here.

## The picture in five lines

1. Pearl (PRL) is the hottest compute narrative right now: about $630M mcap, ATH $2.10 on
   2026-10-14, fair launch, "AI Bitcoin" (Ansem, Token2049). Its weak point is public:
   random-matrix shares accepted by a pool (Basu, June), $769 of chain fees in 30 days, and
   its Together AI discount paid for by emissions (CRYPTO-COMPUTE, VIRALITY).
2. Tokens held value where real, growing, visible revenue flowed into the token: Venice
   (fees 13x Dec to Sep, buyback and burn, stake for daily API credit, +$1.02B YTD) and
   Hyperliquid (buybacks from fees). Revenue without a token sink did not (Aethir -37% YTD);
   subsidised usage vanished when subsidies stopped (Virtuals fees -98.6%) (FLOWS, CRYPTO-COMPUTE).
3. Raw matmul is the least scarce thing in compute. The stock market pays for memory
   (+$1.88T across seven names), a second source of accelerators, funded backlog and power
   now; it punishes borrowed capacity and pre-revenue bets (EQUITIES-COMPUTE).
4. Matmul on data the buyer posts never pays: moving A and B costs about 14,000x the
   compute. Useful work pays only when data already sits on the miner, e.g. model weights
   (COMPUTE-TECH).
5. Attention lasted only where it pointed at a number that kept growing. Paid posting,
   points, airdrops and "agent"/x402 labels with no payers faded (VIRALITY, FLOWS).

## Ranked additions

Each: what, why (evidence), how value reaches the token, what it takes.

1. **Paid-work receipts, made public ("every block was paid for").**
   Why: it is the exact answer to Pearl's public weakness, and the one number Pearl lacks.
   Our rule already says only proofs tied to a paid job earn, and the subsidy stays under
   the fee burned (alpha x P). Value: credibility that converts attention into holders;
   report settled USD and unique payers, never raw tx counts.
   Takes: a public feed/explorer page per block and job (payment, fee burned, proof, light
   client verification on Solana). Mostly reads of what exists. Low to medium.

2. **Storefront SOL into a verifiable buyback.**
   Why: Hyperliquid and Venice held; webpage-only or demand-less buybacks (Aster,
   StonkFun) did not. Value: every paid job buys the token on chain.
   Takes: route a share of storefront SOL through the reserve program into buys, visible
   on chain, with a live reserve view. Low to medium. Owner decision: the share.

3. **Standing jobs on loaded open-weight models (the real product).**
   Why: the only job shape where useful work pays (weights stay on the miner); open-weight
   models are 72% of OpenRouter tokens; Venice proves people pay for this. Value: block
   rewards act as a rebate on inference prices, so the chain sells cheaper open-model
   inference, paid in SOL through the storefront, which feeds items 1 and 2.
   Takes: a model registry in the blob store, a job kind where B is a registered weight
   matrix and the miner commits its activations, an inference gateway in front of the
   storefront. High. Blockers: measure int7 requantization accuracy on a real model first;
   new chips run INT8 at about 1/30 of FP8 and Pearl's own draft moves to FP8.

4. **Stake for compute credit (Venice's DIEM model).**
   Why: the clearest real-demand token design of 2026. Value: holding the token gives daily
   inference credit, so demand for inference becomes demand for the token.
   Takes: a staking program plus credit metering at the storefront. Medium. Only worth it
   once item 3 exists; without real inference to redeem, it is vanity.

5. **Evaluator-verified RL rollout and eval jobs.**
   Why: real buyers (RL and eval runs) and a verification path we already have (CompleteJob
   evaluators, TOPLOC-style checks at about 8 bytes per token). Medium.

6. **Agent Credit for GPU operators, kept small.**
   Why: the market rewards funded backlog and punished borrowed capacity; uncollateralised
   lending earns $0.17M a month across DefiLlama. Credit sized from proven mining income is
   the honest version. Mostly built (R1/R2/R5, ac_pearl); pool-level liens missing. Medium.

7. **One-command mining with an honest meter, and leaderboards by fees burned.**
   Why: proven spread mechanics that cannot be farmed if they count only paid work.
   Low. Comes after items 1 and 3, or the meter shows nothing.

## Do not build

Matmul as a service on posted data; ZK proving; FP64 science; training on our chain now;
"verifiable AI" claims from the PoW proof alone; paid posting, points or large airdrops;
rewards for work nobody bought; emissions-funded discounts as the main demand; agent-run
treasuries on pump.fun as the main hook; unlocks for insiders.

## What this changes in the current build

The storefront today sells posted-data matmul jobs. The mechanics (x402, orders, proofs,
refunds, light client) carry over to standing jobs, but the job kind it sells has no
natural buyer. Items 1 and 2 can start now. Item 3 has one open question that changes what
gets built: does int7 requantization keep an open-weight model's accuracy? Measure that
before any build agent starts on it.
