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
| Inference gateway, prepaid, OpenAI API | Test box only (CPU, Llama-3.2-1B-Instruct), not on devnet yet |
| Wallet extension | Release "Cadre Wallet 6.73.4 (public devnet)" |
| Website | Built (private repo cadre-web), not deployed publicly |
| Mainnet | Not started; needs the owner's go-ahead |

Measured speed on the 4-core CPU test box: about 1.5 tokens/s when the worker runs on its
own, and 0.4 to 0.6 tokens/s through the full paid flow (docs/measurements). A GPU worker
would be needed for a real product.

## What Cadre deliberately does not do

From research/agents/SYNTHESIS.md and research/narratives/SYNTHESIS.md: no token purchase
per request, no free tier paid for by block rewards, no star ratings or count leaderboards,
no slashing for anything the chain cannot prove, no "verifiable AI" claims before the int7
replay check exists, no transferable credit, no treasury or supply expansion, and no
matrix multiplication sold on data the buyer uploads.

## Later (after paid inference has real users)

Stake CADRE for daily inference credit (Venice DIEM model, capped by real paid spend),
evaluator fees and bonds, the int7 replay check, x402 Bazaar and OpenRouter listings, USDC
on Base, and 7-8B models on GPUs.
