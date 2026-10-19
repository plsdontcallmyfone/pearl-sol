# BUYERS: what AI agents pay for, how, how much, and what would make them buy Cadre inference

Research subagent, 2026-10-18. Web sources read 2026-10-18 unless a date is given. Firecrawl was
down, so pages were read with WebFetch and, for OpenRouter, with plain `curl` of the public
pages and the public `/api/v1/models` API (the data is embedded in the HTML). Numbers marked
[derived] are my arithmetic on the cited figures. Where data does not exist I say so.

Already covered elsewhere and not repeated here (see those files for sources): x402 cumulative
and 30-day totals, TRM/Artemis/Visa real-vs-wash splits, Solana's ~70% volume share vs ~1.6K of
43.1K monthly senders, ERC-8004 payer share (FLOWS-AND-PUMPS.md section 5); PING round trip
(VIRALITY.md 1.2); OpenRouter 128.4T weekly tokens and 72% open-weight token share, Epoch price
declines (COMPUTE-TECH.md 2); Venice DIEM and Chutes subsidy ratios (CRYPTO-COMPUTE.md).

Our constraints, from docs/SPEC.md 4.5, 5.1, 5.2, 5.4 and research/INT7-ACCURACY.md: weight
GEMMs in int7 (W7A7, +2.6-2.9% PPL with SmoothQuant + asymmetric activations); job shape needs
k >= 2048; a request may produce at most R = 4096 rows (prompt plus generated tokens); greedy
sampling; the gateway exposes `POST /v1/completions` (raw prompt) only; every request is one
storefront order (open_order + buy_job, a curve or PumpSwap buy, a bridge deposit); miners are
CPU or GPU; demo model Llama-3.2-1B, target 7-8B and up.

---

## 1. Findings

### 1.1 What agents buy over x402: mostly data, very little inference

- **Coinbase x402 Bazaar, top 100 listings by 30-day calls** (Turnkey, published 2026-09-17):
  208,415 calls. Web search and retrieval 42.0%, crypto market data 23.4%, people/company
  enrichment 11.3%, social search 7.5%, on-chain RPC 7.5%, travel 2.5%, loyalty 1.7%,
  **AI inference 1.6% (3,311 calls, three listings: aubr.ai $0.02, token4u.ai $0.009 for
  GPT-4o mini, farouter.tech $0.001)**. Top listing stableenrich.dev, $0.01 a call, 46,567
  calls. Same article: 44,930,157 stablecoin transfers on Base from **440,608 payers**,
  **median payment $0.0061** (2026-08-13 to 2026-09-13).
  https://www.turnkey.com/blog/agents-buying-coinbases-x402-bazaar-discovery-layer
- **Chainalysis** (via Blockhead, 2026-10-15): model inference is "among the largest" x402
  categories but no share is published; 87%+ of payments are $0.0001 to $0.10, 1 to 10 cents
  most common. US-linked wallets made 88.1% of geolocated inference payments, Chinese-linked
  wallets 0.1% (Chainalysis' reading: Chinese open-weight models are run for free, US models
  are metered). Median wallet age at first x402 payment fell from 73 days (late 2025) to
  **18 minutes** (Q3 2026); wallets that spend within an hour of funding rose from 4% to
  **67%**. https://www.blockhead.co/2026/10/15/ai-agents-have-made-138-million-payments-over-x402-most-of-them-under-10-cents/
  (primary: https://www.chainalysis.com/blog/north-america-crypto-adoption-2026/)
- **The largest x402 merchant by transaction count is an LLM gateway.** x402gle server list,
  last 24 h (read 2026-10-18, no absolute date on the page): sol.blockrun.ai (Solana) 42.5K txs,
  **$99.20**, average $0.0023; blockrun.ai (Base) 2.5K txs, $41.21, average $0.02. Three more
  blockrun hosts show identical Solana figures, so they are likely the same traffic counted
  per host. https://x402gle.com/servers. So the biggest inference seller on x402 settles about
  $100 to $140 a day [derived]: real, but tiny.
- x402scan.com itself renders client-side and gave no data to fetch. A 2026-04-20 snapshot of
  its crypto category (1,688 servers) had the top revenue server at **$113.1 in 7 days**.
  https://github.com/blockparty-global/1s-mcp/issues/1
- Split of x402 by "real vs wash" by category, or a per-category dollar value for inference:
  **does not exist publicly**. Nobody publishes payer counts for inference sellers.

### 1.2 Other agent payment rails: only Stripe MPP has a usage number

| Rail | Real usage found | Source |
|---|---|---|
| Stripe/Tempo **MPP** (HTTP 402, card or USDC wallet, settles to Stripe balance) | >20M transactions in the last 30 days by Sept 2026, **average $0.37**; min $0.50 by card, 0.01 USDC by stablecoin. That is about $7.4M per 30 days [derived], comparable to x402's real volume. `mppx` npm: 1,023,593 downloads (2026-09-17..10-16) | https://belitsoft.com/financial-technology-trends-news/agentic-payments/stripe (citing Stripe's Jillian Chi, https://www.youtube.com/watch?v=ud_Q_YfPlvQ); npm API |
| Stripe/OpenAI **ACP** (checkout) | Partner counts only (25+ endorsers); reach claims (Copilot checkout, 500K+ US merchants). No volume. One secondary source says ChatGPT dropped Instant Checkout in March 2026 | https://stripe.com/blog/three-agentic-commerce-trends-nrf-2026, https://www.xpay.sh/blog/article/stripe-sessions-2026-builders-guide/ |
| Google **AP2** | Launch partners (60+, Sept 2025). **No transaction data found** | https://techcrunch.com/2025/09/16/google-launches-new-protocol-for-agent-driven-purchases |
| **Visa Intelligent Commerce** | "Hundreds of controlled, agent-initiated transactions", 30+ sandbox partners (secondary) | https://fintech.global/?p=226340 |
| **Mastercard Agent Pay** | First live authenticated transactions per country (HK, Thailand, US, AU, NZ; Santander Europe 2026-03-02). No volume | https://ai2.work/blog/santander-and-mastercard-complete-europe-s-first-live-ai-agent-payment |
| **Skyfire, Nevermined, Payman** | **No usage numbers found** (only self-published comparison posts) | https://nevermined.ai/blog/stripe-vs-skyfire-vs-nevermined |
| **MCP paid tools** | 11K-17K+ MCP servers listed, under 5% monetized (March 2026, no methodology) | https://chatforest.com/guides/mcp-marketplace-monetization/ |
| **Lightning L402** | Toolkit launched 2026-02-11 (lnget, Aperture). **No agent usage numbers found** | https://lightning.engineering/posts/2026-02-11-ln-agent-tools/ |

Card rails (ACP, AP2, Visa, Mastercard) are built for agents buying goods from merchants for a
human. None of them is a fit for sub-cent machine-to-machine inference.

### 1.3 Where agent inference actually goes (OpenRouter, the only public meter)

**Apps, last month** (openrouter.ai/apps, embedded data read 2026-10-18; ranks 1-2 are not
shown publicly):

| App | Tokens (month) | Requests | Tokens per request [derived] |
|---|---|---|---|
| Hermes Agent (Nous) | 52.04T | 499.8M | ~104K |
| Claude Code | 35.31T | 328.8M | ~107K |
| Kilo Code | 23.86T | 270.1M | ~88K |
| Cline | 20.12T | 162.6M | ~124K |
| pi | 12.75T | 119.4M | |
| Codex | 11.45T | 145.3M | |
| Freebuff | 10.40T | 76.3M | |
| omp | 10.26T | 68.3M | |
| OpenClaw | 7.38T | 82.9M | ~89K |
| Roo Code | 0.22T | 1.4M | |

Week view: Hermes 11.70T, Claude Code 9.92T, Kilo 5.30T, Cline 3.47T, pi 2.93T, OpenClaw 2.45T.
Short-request apps exist further down: Context7 0.21T over 96.3M requests (~2.2K tokens per
request), Portkey 0.95T / 145.7M (~6.5K), "Scammer Get Scammed" 0.36T / 109.7M (~3.3K)
[derived]. Names like "judge", "classifier", "enricher", "labeler", "OCR pipeline",
"datagen" appear across ranks 100-230.

**Which models those agents use** (each app's page, read 2026-10-18): Hermes Agent's top
models are DeepSeek V4.1 Flash (12.0T), a stealth model (5.7T), GLM 5.3 Flash (4.5T), DeepSeek
V4 Flash (3.5T), Step 5 Preview (3.2T); OpenClaw: Step 5 Preview, DeepSeek V4.1 Flash, GLM 5.3
Flash; Kilo and Cline: a stealth model, Nemotron 3 Ultra 550B, Laguna, MiMo Flash. **No model
under about 20B parameters appears in any of these lists.**

**Models leaderboard, current bucket** (openrouter.ai/rankings, top 20, dated 2026-10-18):
125.1T prompt vs 2.78T completion tokens, **prompt:completion about 45:1**; on average
**28.7K prompt tokens and 636 completion tokens per request** [derived]. Leaders: DeepSeek V4.1
Flash, GLM 5.3 Flash, Step 5 Preview, MiMo V2.6 Flash, Hy4 preview, GPT-6 Luna, Claude Opus 5.5.

**Small dense models are a rounding error.** Last 7 daily buckets on each model page
(2026-10-13..19): Llama 3.1 8B Instruct **56.1B tokens**, Qwen3.5-9B **40.8B**, Llama 3.3 70B
**32.5B**. Against ~128T a week, Llama 3.1 8B is about 0.04% of OpenRouter tokens [derived].

### 1.4 Price per million tokens (OpenRouter `/api/v1/models` and `/endpoints`, 2026-10-18)

Input / output, USD per M tokens; quantization as each provider declares it.

| Model | Cheapest | Others |
|---|---|---|
| Llama 3.1 8B | DeepInfra fp8 **$0.02 / $0.04**; Novita $0.02 / $0.05 | Groq $0.05 / $0.08; Cloudflare $0.152 / $0.287; CoreWeave bf16 $0.22 / $0.22 |
| Qwen3.5-9B | Darkbloom fp4 $0.08 / $0.13 | DeepInfra, SiliconFlow, Venice $0.10 / $0.15; Together $0.17 / $0.25 |
| Mistral Nemo 12B | $0.029 / $0.03 | |
| Ministral 8B | $0.15 / $0.15 (`:batch` $0.075 / $0.075) | |
| gpt-oss-20b (MoE) | $0.018 / $0.09 | |
| Gemma 4 26B A4B (MoE) | $0.0675 / $0.225; **a free variant exists** | |
| Gemma 4 31B | CoreWeave fp4 $0.10 / $0.34 | Venice fp4 $0.12 / $0.36, **Chutes fp4 $0.12 / $0.37**, DeepInfra $0.135-0.30 in, **io.net $0.361 / $1.09** |
| Qwen3 32B | DeepInfra fp8 $0.08 / $0.28 | |
| Llama 3.3 70B | DeepInfra fp8 $0.10 / $0.32 | Novita $0.135 / $0.40, **AkashML fp8 $0.20 / $0.52**, SambaNova $0.45 / $0.90, Groq $0.59 / $0.79, **Together $1.04 / $1.04** |
| DeepSeek V4.1 Flash (the agent default) | $0.048 / $0.12, **cached input $0.0016** | 33 endpoints, Together and Fireworks $0.30 / $1.20 |
| GLM 5.3 Flash | $0.048 / $0.20, cached input $0.006 | |

- **Pearl's Together deal:** Together lists `pearl-ai/gemma-4-31b-it` at **$0.28 / $0.86** with a
  32K context (TypingMind, read 2026-10-18; computeprices dated 2026-08-04), vs Together's
  standard Gemma 4 31B at $0.39 / $0.97 (computeprices, updated 2026-10-18). So about 28% off
  input and 11% off output [derived], but still 2-3x the cheapest Gemma 4 31B on OpenRouter
  (CoreWeave $0.10 / $0.34, 262K context). An emissions-funded discount on a premium provider
  does not reach the market floor.
  https://www.typingmind.com/guide/togetherai/pearl-ai-gemma-4-31b-it,
  https://computeprices.com/providers/together-ai/models/gemma-4-31b-it
- **Quantization is not a selling problem:** many endpoints already run fp4 (CoreWeave, Venice,
  Chutes, Darkbloom). OpenRouter's provider `quantization` field accepts int4, int8, fp4,
  mxfp4, nvfp4, fp6, fp8, mxfp8, fp16, bf16, fp32; **there is no int7 value**.
  https://openrouter.ai/docs/guides/community/for-providers
- **Batch is priced at about half:** Ministral 8B $0.15 to $0.075, Mistral Small $0.15 / $0.60
  to $0.075 / $0.30 on the `:batch` variants.
- Chutes' own price list and subscription tiers: not found (DefiLlama shows pay-as-you-go
  revenue of roughly $636K-977K and subscription about $153K-260K per quarter in 2026; secondary,
  https://cryptobriefing.com/chutes-surplus-onchain-inference-growth/).

### 1.5 Developer experience that won

- **OpenAI-compatible base URL is the integration point.** Hermes Agent takes `provider: custom`
  + `base_url` in `~/.hermes/config.yaml` (https://docs.novita.ai/guides/hermes); OpenClaw takes
  `models.providers.<id>` with `baseUrl` and `api: openai-completions`, the default for any
  baseUrl (https://docs.openclaw.ai/gateway/config-tools/custom-providers). Neither speaks x402
  natively; the only x402-for-inference Hermes plugin found is a hackathon project (Clawmes,
  https://hackquest.io/projects/Clawmes).
- **npm downloads, 2026-09-17..10-16** (api.npmjs.org): `openai` 175,958,798;
  `@modelcontextprotocol/sdk` 261,880,885; `@ai-sdk/openai-compatible` 36,666,932;
  `@openrouter/ai-sdk-provider` 13,232,898; `openclaw` 14,961,588; `x402` 1,910,242;
  `@x402/core` 1,437,614; `@x402/evm` 945,555; `@x402/fetch` 527,811; `@x402/svm` 331,955;
  `mppx` 1,023,593; `@coinbase/agentkit` 19,910; `@blockrun/clawrouter` 12,495;
  `solana-agent-kit` 5,894; `venice-x402-client` 589. PyPI `x402` 204,765 last month. Download
  counts include CI and are not users.
- **The pattern that wins on x402 is a local proxy with its own wallet: BlockRun ClawRouter**
  (MIT, 6.6k GitHub stars). It generates a wallet on first run (Solana default, Base optional),
  you fund it with USDC, it runs an OpenAI-compatible proxy on localhost:8402, signs x402
  payments per call ($0.001 settlement fee), routes each request to the cheapest capable model
  (seven free open-weight models included), and ships a one-command OpenClaw setup. A card
  path with an API key exists for people without USDC. https://github.com/BlockRunAI/ClawRouter
- **Venice: prepaid balance plus wallet auth, not pay per request.** Agents sign an
  `X-Sign-In-With-X` header (SIWE on Base or ed25519 on Solana), top up once via a 402 flow in
  USDC on Base or Solana, and each call draws down the balance (ledger shows TOP_UP, CHARGE,
  REFUND); DIEM balance is spent first. https://docs.venice.ai/overview/guides/x402-venice-api
- **x402 now has schemes for metered and repeated calls, both on Solana:** `upto` (buyer signs
  a maximum, server settles the actual amount; LLM generation is the named use case; on SVM via
  a payment-channels program, facilitator pays fees and rent), and `batch-settlement` (one
  escrow deposit, a signed cumulative voucher per request, the server answers **without waiting
  for an on-chain transfer**, claims in batches; a server-signed mode suits usage-based
  billing). https://docs.x402.org/schemes/upto, https://docs.x402.org/schemes/batch-settlement
- **Distribution precedent for a decentralized network: Darkbloom** (Eigen Labs, idle Apple
  Silicon Macs) became a paid OpenRouter provider in April 2026 (secondary) and today lists
  Qwen3.5-9B at the lowest price on OpenRouter. OpenRouter provider rules: a list-models
  endpoint with pricing and quantization, automatic payment to the provider, uptime >= 95% for
  normal routing (80-94% deprioritized, < 80% fallback only), TTFT and throughput shown publicly.
  https://openrouter.ai/provider/darkbloom, https://openrouter.ai/docs/guides/community/for-providers
- **Latency:** no measured, cross-vendor requirement exists. Practitioner rules of thumb: TTFT
  under about 500 ms for interactive steps; cached TTFT matters most in loops; Claude Code
  streams 93% of calls (Requesty data). https://www.kunalganglani.com/blog/ai-agent-latency-optimization-budget,
  https://www.requesty.ai/data. Our int7 worker's TTFT and tokens/s on a CPU or GPU miner for a
  7-8B model: **not measured yet**.

---

## 2. Top 5 findings

1. **Agents pay for data, not inference, on x402.** Inference is 1.6% of Bazaar top-100 calls;
   the median x402 payment is $0.0061. The single biggest inference seller (BlockRun) settles
   about $100-140 a day. Agent inference money flows through prepaid accounts (OpenRouter,
   Venice), not per-call crypto payments.
2. **Agent token demand is huge-context MoE models, not 7-8B dense.** Coding/personal agents
   average 88K-124K tokens per request, 45:1 prompt to completion, on DeepSeek V4.1 Flash, GLM
   5.3 Flash and Step 5. Llama 3.1 8B is about 0.04% of OpenRouter tokens. Our R <= 4096 cap and
   7-8B models do not fit agent coding loops at all.
3. **The price floor is already near zero.** 8B: $0.02 / $0.04. The agent default model costs
   $0.048 in and $0.0016 per M for cached input. Pearl's Together rebate ($0.28 / $0.86) is still
   2-3x the floor for the same model. A rebate only matters if it lands below these numbers.
4. **The winning integration is "change the base URL".** Every top agent takes an
   OpenAI-compatible endpoint; none speaks x402. What works for crypto payment is a local proxy
   with an auto-created USDC wallet (ClawRouter, Solana default) or wallet-auth plus a prepaid
   balance (Venice). `openai` gets 176M npm downloads a month; `@x402/svm` 332K.
5. **Beyond x402, only Stripe MPP has real usage** (>20M txs in 30 days, $0.37 average). AP2,
   Visa, Mastercard, Skyfire, Nevermined, Payman and L402 publish no usage worth building for.

---

## 3. Ranked features for a Cadre buyer SDK / agent template

Effort: L = days, M = one to two weeks, H = more, for one agent.

1. **Take the chain off the request path: one deposit, many requests.** What: a prepaid
   balance per buyer (Solana USDC or SOL deposit into the storefront), wallet-signed requests
   (SIWX-style ed25519 header) or x402 `batch-settlement` vouchers on SVM, and the gateway
   rolling many requests into one ticket/order per balance per period. Why: median payment is
   $0.0061 and 87% are under $0.10; Venice and batch-settlement both exist because per-call
   settlement is too slow and too costly; today every Cadre request is two Solana transactions,
   a curve buy and a bridge deposit. Effort: M-H (storefront balance account, gateway metering,
   batched ticket posting; receipts stay per request in the response blob).
2. **OpenAI-compatible `/v1/chat/completions` with streaming, tool calls and `/v1/models`
   pricing fields.** Why: Hermes, OpenClaw, Kilo and Cline all integrate only through a base URL;
   the gateway now has only raw `/v1/completions`. Effort: M (chat template, tool-call format,
   SSE streaming, stop sequences, usage block; keep greedy decoding but accept and document
   `temperature`).
3. **`cadre-proxy`: a local OpenAI endpoint with its own Solana wallet (the ClawRouter
   pattern).** What: `npx cadre-proxy` generates a keypair, prints a funding address, takes USDC
   or SOL, signs payments, enforces per-day and per-request spend caps, and writes ready configs
   for Hermes (`provider: custom`), OpenClaw (`models.providers` + allowlist), Kilo/Cline/Roo.
   Why: ClawRouter (6.6k stars) shows this is how x402 inference gets used; 67% of x402 wallets
   now spend within an hour of funding, so the funding step must be one screen. Effort: L-M.
4. **Aim at the workloads that fit our shape: short-context and batch.** What: market Cadre for
   classification, extraction, judging, enrichment, tagging and dataset generation (inputs well
   under 4K tokens), and add an OpenAI-style batch endpoint (submit many, results later). Why:
   R <= 4096 rules out coding loops (~100K per request) but such apps exist on OpenRouter
   (Context7 ~2.2K, Portkey ~6.5K tokens per request, judge/classifier/labeler apps); batch
   variants are priced at about 50% and tolerate our settlement latency. Effort: M.
5. **Quote a price that beats the floor, in USD per M tokens, and show the rebate.** What:
   `/v1/models` returns prompt/completion prices like OpenRouter; the price shown must be at or
   under $0.02 / $0.04 for 8B class and under $0.08 / $0.13 for 9B class, or there is no reason
   to switch. Why: section 1.4. Effort: L (pricing logic), but the number itself needs the
   miner cost measurement below.
6. **Pick models with demand that fit k >= 2048 and CPU/GPU miners.** Candidates, by measured
   demand: Llama 3.1 8B (56.1B tokens/week, k = 4096), Qwen3.5-9B (40.8B/week); next, MoE
   models with small active size (gpt-oss-20b, Gemma 4 26B A4B, Qwen3.6-35B-A3B) once expert
   GEMMs are mapped to jobs. Measure TTFT and tokens/s of the int7 worker per miner class first;
   no figure exists yet. Effort: M (registration + recipe per model) plus measurement.
7. **Response receipts.** What: every response carries headers with ticket id, order id, block
   heights, fee burned and an explorer link; `GET /v1/requests/<id>` already exists. Why: ties
   buyer traffic to SYNTHESIS item 1 ("every block was paid for") and lets agent operators audit
   spend. Effort: L.
8. **List where agents discover services.** x402 Bazaar and x402scan listing, `upto` scheme on
   SVM for no-account single calls (buyer signs a max, we settle actual tokens). Why: BlockRun
   is the top x402 merchant by tx count because it is listed and cheap; `upto` is the documented
   LLM path. Effort: L.
9. **OpenRouter provider application, after 1-6 and a measured uptime.** Why: it is where the
   agent tokens are, and Darkbloom shows a decentralized network can be listed. Blockers: >= 95%
   uptime, public TTFT/throughput, no int7 value in the quantization enum (we must not label it
   int8), OpenRouter pays us in fiat or USDC, so the storefront needs a path that turns that into
   SOL orders to keep receipts and buybacks honest. Effort: H.
10. **USDC on Base as a second funding source**, bridged to SOL orders by the gateway. Why:
   about 32K of 43.1K monthly x402 senders are on Base vs about 1.6K on Solana (FLOWS-AND-PUMPS).
   Effort: M. Only after 1-3.

## 4. What NOT to build

- **Per-request on-chain orders with a token buy in the hot path.** Keep it for settlement, not
  for each call.
- **A Cadre-specific payment protocol or a framework SDK that agents must adopt.** Frameworks
  take a base URL; x402 already has `upto` and `batch-settlement` on Solana.
- **Integrations with AP2, Visa Intelligent Commerce, Mastercard Agent Pay, ACP, Skyfire,
  Nevermined, Payman or L402** now: no published usage, and the card rails target shopping.
  Stripe MPP is the only one with volume; revisit it after features 1-3 if fiat-funded buyers
  ask.
- **Selling 7-8B models to coding agents** or claiming agent-loop parity: their requests are
  ~25x our 4096-row cap and they run 100B+ MoE models.
- **A free tier funded by block rewards as the main draw.** Free open models already exist
  (ClawRouter, Gemma 4 26B A4B free on OpenRouter), and emissions-funded discounts faded
  elsewhere (CRYPTO-COMPUTE.md).
- **Marketing x402 transaction counts.** Report settled USD and unique paying wallets, as the
  narratives already say.
- **An MCP paid-tool server first.** Under 5% of MCP servers are monetized and there is no
  usage data; an MCP wrapper is cheap later once the endpoint exists.

## 5. Gaps (data that does not exist or I could not get)

- Per-category dollar value and payer counts for x402 inference; wash share by category.
- Usage of AP2, Visa, Mastercard, Skyfire, Nevermined, Payman, L402.
- OpenRouter per-provider token volumes and TTFT/throughput (fields were null in the API).
- Chutes and Akash/io.net own price lists (only their OpenRouter endpoints are cited above).
- The ranks 1-2 apps on OpenRouter's monthly app list are not shown publicly.
- Cadre's own TTFT, tokens/s and cost per M tokens on a CPU or GPU miner: to be measured.
