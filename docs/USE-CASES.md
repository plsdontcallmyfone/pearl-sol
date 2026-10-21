# Cadre: the full use case (2026-10-19)

Every rule below is from docs/SPEC.md, docs/MINING.md, docs/EVALUATORS.md and docs/DEVNET.md.
Measured figures are from docs/measurements and HANDOFF item 17. Nothing here is a forecast.

## One paragraph

Cadre sells cheap open-model AI inference and pays for it with its own token. A buyer pays in
SOL on Solana. Part of every payment buys CADRE back. The GPUs that serve the model mine the
Cadre chain while they do it, so the block rewards act as a rebate on the inference price. A
block earns a subsidy only when it was mined on paid work. So the network can say "every
block was paid for" and anyone can check that on chain.

## Two chains, one token

| | Solana (token chain) | Cadre chain (agent chain) |
|---|---|---|
| What it is | The CADRE pump.fun token (fixed 1B supply), and our programs: reserve, bridge, storefront, light client | Our own chain (Rust node on Commonware, not a fork) |
| What happens there | Trading, payments in SOL, buybacks, the final record of jobs and withdrawals | The work: AI jobs, mining, agents, evaluators, liens |
| Money | SOL, CADRE (SPL) | CADRE (native coin, the bridged token) |

The bridge moves CADRE between them. Deposits to the chain are attested. Withdrawals back to
Solana are proved: a light client on Solana follows the chain's proof-of-work headers and
verifies them.

## Who uses Cadre, and what each one does

### 1. AI buyer (a developer, an app, or an AI agent)
What they want: cheap inference on open models, through an OpenAI-style API.
1. Top up a prepaid balance once: pay SOL over x402 (`POST /v1/balance/topup`). That one
   payment becomes one Solana order and one balance ticket on the chain.
2. Call `/v1/chat/completions` or `/v1/completions` like OpenAI, by changing the base URL.
   Sign in with a wallet signature (`cadre-auth-v1`) or an API key (`cadre_sk_...`, made with
   `POST /v1/keys`). Streaming works.
3. Every response carries a receipt: ticket, order, rows, amount charged, and explorer links.
   `GET /v1/receipts` lists them.
4. Close the balance at any time (`POST /v1/balance/close`). The unused part comes back on
   the chain.
- There is no Solana or chain transaction on each request. Requests are settled in batches.
- Agents can run `cadre-proxy` on their own machine: a local OpenAI endpoint with its own
  wallet and spend caps (per request, per day, total). Config snippets exist for Hermes,
  OpenClaw, Kilo and Cline.
- Pay-per-request over x402 also works, but it is not the main path.
- Where: the Cadre site's AI page, the wallet's AI screen, or the API directly.

### 2. Miner (anyone with a computer or GPU)
What they want: earn CADRE.
1. Install with one command (`install-miner.sh`, docs/MINING.md), solo or through a pool.
2. The miner does useful proof of work: matrix multiplications from paid AI jobs.
3. Earnings per block are tx fees plus a subsidy from the mining pool. The subsidy is paid
   only on blocks mined on a paid job, and it is capped per job (at most alpha x P, alpha
   0.8 by default, P = the job's protocol fee). Blocks with no paid job earn fees only.
4. Rewards mature after the finality depth and are provable on Solana (every block carries a
   reward record).
- Where: the site's Mine page, the wallet's Mining rewards screen, `pearl` CLI.

### 3. Inference server (a GPU operator who serves a model)
What they want: get paid for serving AI, and mine at the same time.
1. A model's weight matrices are registered once on the chain (quantized to int7 with the
   fixed recipe in SPEC 4.5). The weights stay on the server.
2. The server runs the gateway and the model worker. Each paid request becomes standing jobs
   whose big matrix is a registered weight. The server mines those same multiplications.
3. It earns the worker pay W (pro rata to rows actually used) plus the block rewards on its
   own segments. This is why Cadre inference can be priced low.
- Today: one gateway on the test box (CPU only, Llama-3.2-1B-Instruct). No GPU worker yet. Not on
  the public devnet yet.

### 4. Agent with an on-chain identity
What they want: a public track record that buyers can trust.
1. `RegisterAgent` with a name and a link to an agent card (A2A-style JSON).
2. The agent sells work through jobs and gets paid in CADRE.
3. Its profile is built by an indexer from chain data only: paid volume behind completed
   work, distinct payers, top payer share, self-dealing excluded, rejected and expired
   deliveries, acceptance weighted by value, share checked by outside evaluators, and its
   mining record. Never raw job counts.
- Where: explorer `/agent/<addr>` and `/agents`, the site's Agents page, `pearl agent`.

### 5. Evaluator
What they want: check work and be named by buyers as a judge.
1. A job names its evaluators and a threshold.
2. Evaluators sign `CompleteJob` (pays the worker) or `RejectResult` (sends the job back to
   open, records the failure on the worker's record, and that worker cannot deliver to the
   job again).
- Open to anyone now (docs/EVALUATORS.md, `pearl-evaluator`). Evaluator fees and bonds
  come later.

### 6. CADRE holder or trader
What they want: a token with real demand behind it.
- Buy on pump.fun or PumpSwap (Solana).
- Demand that comes from use, all on chain:
  - 10% of every storefront payment buys CADRE. It goes to the mining reserve until the
    reserve cap (provisional 100M), and above the cap it is burned.
  - Creator fees buy CADRE back into the reserve.
  - The reserve can only flow into the mining pool, through `fund_mining`. Anyone can call
    it, within rate limits (provisional: 10M per call, once an hour, 20M per day). The admin
    can pause it but cannot redirect it.
  - Pool tokens leave only as subsidy on paid jobs.
- The public numbers (explorer `/revenue`, `/api/supply`, the DefiLlama adapter) show
  revenue after wash screens and unique payers.

### 7. Bridge user
- Move CADRE from Solana to the Cadre chain (deposit) and back (withdraw, proved by the
  light client).
- Where: the site's Buy and bridge page, the wallet's Swap screen.

### 8. Borrower and lender (Agent Credit)
What they want: credit for a miner or agent, repaid from its own on-chain income.
- The borrower sets a payout lien (`SetRoute`): a share of every income credit (block
  rewards, worker pay, refunds) goes straight to the lender on Solana until a cap or end
  height.
- The borrower cannot shorten, weaken, redirect or cancel an active lien. Only the lender's
  release key can clear it early.
- Where: the wallet's Payout lien and Agent credit screens.

### 9. Solana apps and other programs
- The storefront's `buy_job` and the `Order` account are open to any Solana program (CPI),
  so a Solana app can buy AI work and read the result on chain.
- The light client lets Solana programs verify Cadre chain blocks, rewards and job results
  without trusting a committee.

### 10. The public and auditors
- Explorer: blocks, jobs, orders, transactions, accounts, agents, and the paid-block ratio.
- Anyone can check that the subsidy only went to paid work, and how much SOL was settled.

## How the money moves (one paid request)

1. The buyer pays SOL to the storefront (x402).
2. The storefront moves 10% of it into a CADRE buyback (reserve up to the cap, burn above).
3. The rest buys CADRE and splits it into worker pay W and protocol fee P.
4. The order crosses the bridge and becomes a balance ticket on the chain. P goes to the
   mining pool right away. W stays escrowed.
5. The server answers requests, posts the work as standing jobs and mines them. Blocks on
   that work earn fees plus subsidy (capped at alpha x P per job).
6. When the balance closes, the ticket completes: the server gets W for the rows used. The
   unused W and P go back to the buyer on the chain.
7. The result is recorded on Solana against a light-client header.

A fake round trip (paying yourself to mine your own job) loses at least (1 - alpha) x P plus
the 10% buyback share, about 15.7% (research/agents/TOKENOMICS.md), so wash volume costs
money.

## What runs where today

| Piece | Status |
|---|---|
| Cadre chain | Live on the public devnet (3 nodes) |
| Solana programs (reserve, bridge, storefront, light client) | Deployed on Solana devnet |
| cadre-api, explorer | Live on devnet box 0 |
| Inference gateway, prepaid, OpenAI API | Live on devnet box 0 since 2026-10-20 (CPU, Llama-3.2-1B-Instruct, 2 worker threads) through https://api.64-95-12-116.sslip.io/v1/openai; also on the test box |
| Wallet extension | Release "Cadre Wallet 6.73.4 (public devnet)" |
| Website | Live on devnet box 0: https://cadre.64-95-12-116.sslip.io (installer at /install-miner.sh; the installer's source repo is still private) |
| Mainnet | Prepared, not launched (docs/MAINNET.md); needs the owner's go-ahead |

Measured speed on the 4-core CPU test box: about 1.5 tokens/s when the worker runs on its
own, and 0.4 to 0.6 tokens/s through the full paid flow (docs/measurements). On devnet box 0
(4 vCPU, worker on 2 threads, the box's miner on the other 2): 2.0-2.1 tokens/s decode, and the
paid devnet chat answered at 2.1 tokens/s (docs/measurements/cadre-worker-devnet-box0-2026-10-20.jsonl,
docs/DEVNET.md "Inference"). A GPU worker would be needed for a real product.

## Verifiable answers: what is proven (2026-10-21)

Since 2026-10-21 a Cadre answer is replayable: the worker runs the deterministic glue
cadre-glue-v1 (SPEC 4.7), so every honest CPU or GPU computes the same activations bit for bit.
The reference evaluator replays every paid ticket whose response carries the replay bundle. It
rebuilds every segment's activations from the request's token ids, rechecks every GEMM, and
requires the output tokens, the stopping point and the text hash. Measured on an A40 against a
Xeon CPU: 0 of 144 matrices differed on any test prompt (the old fp32 glue: 126-144 differed,
and 2 of 4 answers changed). A server that posts correct GEMMs over altered activations is
rejected on chain (RejectResult), with a fraud proof naming the segment and element that anyone
can check (docs/PROOFS.md row 11).

What you can claim: "every answer is exactly what the registered int7 model produces for that
input under greedy decoding, and anyone can check it on a CPU". What you cannot claim yet:
that cheating costs a bond (slashing is designed, not built), that the int7 model equals the
original fp16 model (it is measured close, research/VERIFIABLE-AI.md), privacy (none), or
sampled (temperature) output. Workers started before this change (devnet box 0, the test box)
still run the fp32 glue until they restart, and their tickets get only the GEMM check.

## What Cadre deliberately does not do

From research/agents/SYNTHESIS.md and research/narratives/SYNTHESIS.md: no token purchase
per request, no free tier paid for by block rewards, no star ratings or count leaderboards,
no slashing for anything the chain cannot prove, no "verifiable AI" claim beyond what the
replay check covers, no transferable credit, no treasury or supply expansion, and no
matrix multiplication sold on data the buyer uploads.

## Later (after paid inference has real users)

Stake CADRE for daily inference credit (Venice DIEM model, capped by real paid spend),
evaluator fees and bonds (server and evaluator slashing on replay fraud proofs, designed in
SPEC 4.7), x402 Bazaar and OpenRouter listings, USDC
on Base, and 7-8B models on GPUs.

## Why CADRE: utility and incentives

Sources: SPEC 3.4, 4.1, 5.1 (Buyback, Reserve exit), research/agents/TOKENOMICS.md,
research/narratives/SYNTHESIS.md. No price claims; every mechanism below is a program rule.

### Where demand for the token comes from
1. **Every AI payment buys CADRE on the market.** A buyer pays SOL, and the storefront spends
   all of the job's SOL buying CADRE on pump.fun or PumpSwap inside the same transaction.
   Those tokens become the worker pay W (80%) and the protocol fee P (20%). Buyers never
   have to touch the token; their usage is the buy.
2. **On top of that, a 10% buyback.** It is a markup on each order, paid only when the work
   is delivered (refunded orders get it back). It buys CADRE into the mining reserve up to
   the cap (provisional 100M) and burns everything above it. On devnet the reserve already
   holds more than the cap, so every buyback there is burned.
3. **Creator fees come back too.** pump.fun's creator fee is cranked into CADRE buys for the
   mining reserve.

### Where tokens get locked or removed
- **Burn:** buybacks above the reserve cap lower the mint supply for good.
- **Mining pool:** P goes to the pool at once and is not refundable. Tokens leave the pool
  only as subsidy on paid jobs, at most 80% of each job's P, so at least 20% of every P stays
  locked. The pool works as a soft burn.
- **Mining reserve:** it can only flow into the mining pool, through `fund_mining`, rate
  limited. There is no admin withdrawal, no team treasury, and no new supply: 1B fixed, the
  chain mints nothing.

### Why each participant shows up
| Who | What they get | Why it ties to the token |
|---|---|---|
| AI buyer or agent | OpenAI-compatible inference, prepaid, receipts, refunds of unused balance | Their SOL becomes CADRE buys |
| Inference server | Worker pay W plus block rewards on the same work, so it can charge less | Paid in CADRE |
| Miner | Fees plus subsidy, only on paid work | Paid in CADRE; more paid usage means more subsidy, so miners want real demand |
| Agent | A public record built from paid work from distinct payers | Paid in CADRE, profile worth more as volume grows |
| Evaluator | Named by buyers to sign or reject work (fees later) | Track record on chain |
| Lender (Agent Credit) | Repaid automatically from a borrower's on-chain income | Income is in CADRE |
| Holder | Buy pressure and burns that only exist when people pay for real work | All of the above |

### Why it is hard to fake
A block earns subsidy only if it was mined on a paid job, and the subsidy per job is capped
below what the payer put in. Paying yourself to mine your own job loses at least about 15.7%
per round trip (the 20% of P that stays in the pool plus the buyback markup;
research/agents/TOKENOMICS.md). So fake volume costs money, and the public numbers count
distinct payers, never raw transaction counts.

### Why it stands out
- Pearl (PRL), the leading "useful proof of work" token, is criticised for rewarding work
  nobody bought and for very low chain fees (research/narratives/CRYPTO-COMPUTE.md). Cadre's
  rule is the direct answer: only paid work earns subsidy, and anyone can check it on the
  explorer ("every block was paid for").
- The models that held value in 2026 tied the token to real, growing, visible revenue
  (Venice, Hyperliquid). Activity rewards and subsidised usage faded
  (research/narratives/FLOWS-AND-PUMPS.md). Cadre's demand comes only from paid usage.

### Honest limits (say these too)
- Buyers pay in SOL and do not need to hold CADRE; servers and miners receive CADRE and may
  sell it. Net buy pressure depends on real paid volume growing.
- Today's volume is tiny (devnet: 2 storefront orders), and the only inference server is a
  CPU box at about 0.5 tokens/s. A GPU worker is needed before this is a product.
- The holder perk that ties holding to usage, staking CADRE for daily inference credit
  (the Venice DIEM model), is planned, not built.
- Program upgrade authority is still trusted, and the cap and rate limits are provisional.
