# COMPUTE-TECH: what compute is popping, and what our chain can honestly serve

Research slice, 2026-10-17. Scope: the technical side of compute demand and supply, and
where pearl-sol (SPEC v2: int7 matmul jobs of given shapes, data via the blob store,
evaluator-signed CompleteJob, Solana storefront over x402, pools, payout liens) could add
real value. No code changes. Firecrawl was down; sources are WebSearch (extended) and
WebFetch. "Read" = the date I read the page (all 2026-10-17 unless noted). Where a number
comes from a secondary aggregator, it says so. Numbers marked **[derived]** are my own
arithmetic on sourced inputs, shown so they can be checked.

## 0. TL;DR

1. Demand is inference, and inference is agents and code. OpenRouter went from 0.5T to
   128.4T tokens per week (Jan 2025 to Sep 2026); open-weight models are 72% of its tokens
   and Chinese labs 69%. Open-weight is now the volume layer, closed-weight still takes
   73% of spend.
2. What is scarce is memory, power and Blackwell capacity, not multiply-adds. HBM is
   sold out through 2026 (SK hynix) and HBM4 stack prices are expected to roughly double
   next year. Raw INT8 throughput is the most commoditized compute there is: an H100's
   dense INT8 peak rents for about $0.78 per 10^18 MACs **[derived]**.
3. The decisive fact for us: **a stand-alone matmul job is worth far less than the cost
   of moving its data.** For a 16384^3 int7 job, the H100 compute is worth about $0.0000034
   while shipping A and B (537 MB) at cloud egress rates costs about $0.048 **[derived]**,
   about 14,000x more. Within Pearl's parameter limits (k <= 65536, m, n <= 2^24) no shape
   breaks even. So "post a matrix, get the product" has no real buyer at any price that
   covers bandwidth. Useful work only pays when the data already lives on the miner
   (model weights resident, activations generated locally), which is exactly how Pearl's
   one commercial integration (Together AI) works.
4. Hardware is drifting away from integer matmul. Blackwell Ultra (B300) cut dense INT8 to
   about 150 TOPS, about 1/30 of its FP8, versus 1:1 on H200/B200; vLLM has no Blackwell
   INT8 GEMM. Pearl itself drafted PIP-3 (2026-07-30) to move PoUW to FP8 because "the
   overwhelming majority of AI models are natively floating-point". Our int7 protocol is
   on a shrinking hardware base (H100/H200/B200 and consumer cards).
5. Usefulness is the open wound of the whole category. A June 2026 study measured one
   Pearl pool (about 21% of hashrate, 8,012 workers) and found no inference code in the
   dominant miner; an IACR SoK of 50+ PoUW designs concludes "PoUW is actually not as
   useful as expected". Pass (June 2026) gives the defensible framing: block rewards act
   as a **rebate on inference prices** ("Duplexia"). That, not "verifiable matmul", is the
   honest pitch.
6. Best gaps for us (section 6): (G1) a model-resident "standing job" so miners mine on
   the GEMMs of an open-weight model they serve, sold through the storefront at a
   subsidized price; (G2) generic evaluator-verified jobs (RL rollouts, evals) checked by
   TOPLOC-style proofs, using the existing CompleteJob path; (G3) Agent Credit for GPU
   operators against provable coinbase income. Vanity: matmul-as-a-service on posted data,
   ZK proving, scientific/FP64 compute, and "decentralized training on our chain" now.

---

## 1. What our chain can and cannot compute today (from SPEC v2 + NOTES-proof)

- **Can:** an int7 x int7 -> int32 GEMM, A m x k, B k x n, values in [-64, 64],
  k in [1024, 65536] and a multiple of 64, m, n <= 2^24, posted as a job with Pearl
  commitments `hash_a`, `hash_b` (BLAKE3 keyed under a job-fixed `job_key`), data in the
  blob store. Miners mine noised tiles of (A+E)(B+F); blocks carry a plain proof of one
  winning tile (12.6 KB at the localnet config). `result_hash` commits to the full C.
  Completion pays W after evaluator signatures (M1: off-chain Freivalds by the attester).
- **Proof covers:** that the miner did matmul work proportional to the target on exactly
  the committed A and B (shares give a statistical measure of how much of the job was
  done). **It does not cover:** that C was delivered or correct (that is the evaluator's
  Freivalds check), or anything beyond one GEMM (no nonlinearities, no model, no chain of
  layers).
- **Latency:** 60 s target blocks, finality K = 30 blocks (about 30 minutes). Any job that
  needs an answer in seconds cannot wait for the chain; it can only settle on it.
- **Data path:** both operands and the result travel through the blob store. C is
  m x n int32, so a 65536 x 65536 result is 17 GB **[derived]**; there is no reduction
  (top-k, sum, argmax) in the job spec today.

## 2. AI workload demand

### 2.1 Inference vs training
- Deloitte (via Computerworld): inference was half of AI compute in 2025, projected
  two-thirds in 2026. https://www.computerworld.com/article/4114579/ces-2026-ai-compute-sees-a-shift-from-training-to-inference.html (read 2026-10-17)
- Gartner (secondary summary): 55% of the $42B AI-optimized IaaS market in 2026 supports
  inference. Same page as above / https://telnyx.com/resources/ai-training-vs-inference (read 2026-10-17)
- Epoch AI, 2024 OpenAI split (the only lab-level split with a method): about $3B training,
  $1.8B inference, $1B amortized research; only about $500M (about 10%) went to the final
  training runs of released models. https://epoch.ai/data-insights/openai-compute-spend and https://epoch.ai/gradient-updates/r-and-d-vs-training-compute (read 2026-10-17)
- OpenAI inference on Azure, first three quarters of 2025: $8.7B (leaked Microsoft
  financials). https://www.theregister.com/2025/11/12/openai_spending_report/ (read 2026-10-17)
- OpenAI expects to spend $50B on compute in 2026 (Brockman, secondary report).
  https://startupfortune.com/openai-plans-to-spend-50-billion-on-computing-in-2026-and-that-number-reframes-what-kind-of-company-openai-actually-is/ (read 2026-10-17)
- I found no current Epoch estimate of the global inference share. Not invented here.

### 2.2 Token volumes (OpenRouter is the only public, cross-provider meter)
OpenRouter, "The Token State of Play", Inferred 01, published 2026-09-22.
https://openrouter.ai/data/newsletter/the-token-state-of-play (read 2026-10-17)
- Weekly tokens: 0.5T (week of 2025-01-13) to **128.4T** (week of 2026-09-14), about 250x
  in 20 months.
- Estimated last day humans used more tokens than agents: **2026-02-06**.
- Open-weight share of tokens crossed 50% in Feb/Apr 2026, peaked at 79% in early July,
  **72%** in the week before publication. Closed-weight share of **spend** fell from 89%
  (week of 2026-01-05) to 73% (Sep 2026). Volume is open; money is still mostly closed.
- Chinese labs: **69%** of tokens. Labs outside the US and China: 2% (from about 12% in
  early 2025). DeepSeek is used in production by over 40% of US startups on OpenRouter
  (from about 13% in January).
- Top single models by week (secondary): DeepSeek V4 Flash 0731 at 12.1T tokens for the week
  ending 2026-09-01; a stealth model at 38.7T and DeepSeek V4.1 Flash at 25.6T for
  Sep 28 to Oct 4. https://www.teamday.ai/blog/top-ai-models-openrouter-2026 (secondary, read 2026-10-17)
- Programming rose from about 11% of OpenRouter tokens in early 2025 to over 50% by year
  end (OpenRouter + a16z, 100T-token study). https://openrouter.ai/state-of-ai and https://arxiv.org/html/2601.10088v1 (read 2026-10-17). The arXiv version reports 37% programming + 26% technology at sample end; the share depends on category definitions.
- The live rankings page (https://openrouter.ai/rankings, data through 2026-10-17) did not
  render chart values in fetch; per-app token counts this week: unavailable.

### 2.3 Open-weight adoption
- Qwen: about 2.05B downloads this year and 151,448 derivatives on the Hub, 2.6x Meta's
  footprint (Hugging Face "State of Open Models: Summer 2026").
  https://huggingface.co/blog/state-of-open-models-summer-2026 and https://thenextweb.com/news/alibaba-qwen-downloads-hugging-face-open-models (read 2026-10-17). Alibaba's own claim (3B downloads, 300k derivatives) is higher.
- The single most downloaded model is still a tiny embedding model:
  sentence-transformers/all-MiniLM-L6-v2 at 254,324,263 downloads in 30 days (Sep 2026
  snapshot, secondary). https://www.digitalapplied.com/blog/most-downloaded-open-ai-models-hugging-face-september-2026 (read 2026-10-17)
- Moonshot, Z.ai and DeepSeek grew monthly spend 10x or more from Jan to Aug 2026 but
  remain high single digits of spend share (OpenRouter newsletter, above).

### 2.4 Price per million tokens
- Epoch (2026-09-22, "The plunging price of thought"): the cost of a fixed performance
  level fell about 47% per quarter, about **13x per year** since 2023; about 75x per year
  right after a level becomes SOTA, about 4.7x per year two years later. Example: GPQA
  level that cost about $0.30 per question with o3 (Jan 2025) cost $0.0004 with GPT-5.6
  Luna, 725x cheaper. https://epoch.ai/publications/the-plunging-price-of-thought (read 2026-10-17)
- Epoch (2025-03-12): 9x to 900x per year across milestones. https://epoch.ai/data-insights/llm-inference-price-trends (read 2026-10-17)
- MIT FutureTech (Gundlach et al., March 2026): 5x to 10x per year (as cited by Epoch above).
- Spot prices (secondary aggregators, change weekly): DeepSeek V4 Flash on OpenRouter about
  $0.14 input / $0.28 output per M tokens (2026-08-04 snapshot).
  https://computeprices.com/providers/openrouter/models/deepseek-v4-flash (read 2026-10-17)
- SemiAnalysis cites an average of about $5 per million tokens for agent tooling
  (2026-04-02). https://newsletter.semianalysis.com/p/the-great-gpu-shortage-rental-capacity (read 2026-10-17)
- Embeddings: OpenAI text-embedding-3-small $0.02/M ($0.01/M batch), 3-large $0.13/M
  ($0.065 batch); Voyage 4-lite $0.02/M; Gemini Embedding 2 $0.20/M ($0.10 batch)
  (aggregators; one April 2026 post disputes the OpenAI embedding batch discount).
  https://embeddingcost.com/ and https://aiapiprices.com/embeddings-api-pricing/ (read 2026-10-17)
- Batch APIs at 50% off for 24 h turnaround are standard at OpenAI (above); i.e. the
  incumbents already sell exactly the "slow, cheap" tier a PoW chain would offer.

### 2.5 Agent workloads and their bill
- Anthropic annualized revenue: $9B at end of 2025 to over $30B (SemiAnalysis, 2026-04-02,
  link above). SemiAnalysis expects Claude Code to make 20%+ of daily commits by end 2026.
- Claude Code enterprise average about $13 per developer per active day, about $150 to
  $250 per month; Gartner: 23% of tech leaders spend $200 to $500 per developer per month on
  tokens, 6% over $2,000. https://www.computerweekly.com/news/366645054/Gartner-AI-coding-agents-will-cost-more-than-real-developers and https://www.morphllm.com/ai-coding-costs (read 2026-10-17; the per-day figure is repeated by several secondary sites)
- Agentic sessions of 400k to 2M tokens; heavy users $500 to $2,000/month (secondary,
  https://www.morphllm.com/ai-coding-costs, read 2026-10-17).
- Computer-use agents: I found no sourced 2026 figure for their share of tokens or spend.
  Unavailable.
- Agent payments rail (x402): about 75M payments, about $24M, about $0.32 average, 94k
  buyers and 22k sellers in the 30 days to mid-July 2026.
  https://www.coindesk.com/tech/2026/07/15/visa-mastercard-and-ripple-join-the-standard-letting-ai-agents-pay-in-stablecoins (read 2026-10-17).
  Tracker: 191,555,284 cumulative payments, $41,911,435 settled, 24,953 in the latest day
  (as of 2026-10-10). https://agenteconomy.to/x402 (read 2026-10-17). Visa's estimate of
  real (non-wash) volume: about $19M (secondary, https://startupfortune.com/ai-agents-have-moved-over-200-million-payments-on-x402-but-half-may-be-fake/). x402 is real but small: tens of
  millions of dollars total versus tens of billions of inference spend.

## 3. What is scarce, what is commoditized

### 3.1 GPU rental prices (USD per GPU-hour)
| GPU | Price | Source, date of data |
|---|---|---|
| H100 SXM | $2.78 | Ornn OCPI, settled 2026-10-17, https://data.ornn.com/ |
| H100 SXM | $3.13 (Oct 15), $2.79 (Oct 2) | Ornn OCPI history (secondary snippet), https://data.ornn.com/markets/h100-sxm |
| H100 | $2.82 | Silicon Data index, 2026-10-17, https://www.silicondata.com/products/silicon-index/h100 |
| H100 1-yr contract | $1.70 (Oct 2025 low) to $2.35 (Mar 2026), about +40% | SemiAnalysis, 2026-04-02, link above |
| H100 composite | $2.80 (Mar 2026) | https://gpu-index.semianalysis.com/ |
| H200 | $6.39 | Ornn, 2026-10-17 |
| B200 | $8.48 | Ornn, 2026-10-17 |
| B200 | $3.68 (Apr 2026) | SemiAnalysis composite; methodology differs from Ornn, so the two disagree by 2.3x |
| B200 spot, AWS p6-b200 | about $14 | SemiAnalysis, early 2026 |
| A100 SXM4 | $0.97 | Ornn, 2026-10-17 |
| RTX 4090 | $0.16 to $0.48 (Vast median $0.48) | https://www.beam.cloud/blog/nvidia-rtx-4090-pricing, https://computeprices.com/providers/vast/gpus/rtx4090 (Aug 2026) |
| RTX 5090 | $0.25 to $0.65 (Vast median $0.65) | https://gpus.io/en/gpus/rtx5090, https://computeprices.com/providers/vast/gpus/rtx5090 (Aug 2026) |
| Hyperscaler on-demand | AWS P5 $6.88, GCP about $11.06 per H100-hour; 16-provider median $4.20 | https://uvik.net/blog/gpu-price-index/ (secondary) |

Trend: H100 rentals rose from the late-2025 trough through spring 2026 (SemiAnalysis:
"on-demand capacity is sold out across all GPU types", capacity through Aug/Sep 2026
pre-booked) and sit around $2.8 to $3.1 now. Basu (below) measured budget GPU rental prices
up 38% and utilization 57% to 94% after Pearl's miner was released, i.e. PoUW itself is now
a measurable buyer of consumer-GPU hours.

### 3.2 Memory and power (the real scarcity)
- SK hynix: HBM, DRAM and NAND "essentially sold out" for 2026; three years of HBM sold out
  (April). 12-high 36 GB HBM4 expected to go from about $600 to about $1,300 next year.
  https://en.sedaily.com/finance/2026/10/01/hbm-prices-to-more-than-double-next-year-as-shortage and https://en.sedaily.com/finance/2026/04/23/sk-hynixs-hbm-sells-out-for-3-years-dram-supply-runs-short (read 2026-10-17)
- SemiAnalysis: LPDDR5 / DDR5 contract prices tracking about 4x and 5x year on year in 1Q26
  (2026-04-02, link above).
- Power: LBNL "Queued Up: 2026 Edition" counts about 2,060 GW of generation and storage in
  interconnection queues (secondary summary); PJM/ERCOT waits 36 to 48 months.
  https://www.globaldatacenterhub.com/p/us-data-center-infrastructure-the (read 2026-10-17; primary LBNL report not opened)

### 3.3 Low-precision matmul: where the silicon is going
- H100 SXM dense INT8: 1,979 TOPS (3,958 with 2:4 sparsity). H200 and B200: FP8 dense equals
  INT8 dense (1:1); B200 INT8 dense 4,500 TOPS. **B300 (HGX) INT8 dense about 150 TOPS,
  FP8:INT8 about 30:1**; PTX omits `.kind::i8` for sm_103a; CUTLASS skips INT8 UMMA for
  103a; vLLM has no Blackwell INT8 GEMM. arXiv 2608.11693v2 (Chen, 2026-08-14),
  https://arxiv.org/html/2608.11693v2 (read 2026-10-17). H100 datasheet reproductions:
  https://www.spheron.network/blog/nvidia-h100-specs/ (read 2026-10-17).
- B300 NVFP4: 15 PFLOPS dense per chip, 1.5x B200.
  https://www.tomshardware.com/pc-components/gpus/nvidia-shares-blackwell-ultras-secrets-nvfp4-boost-detailed-and-pcie-6-0-support (read 2026-10-17)
- Pearl PIP-3 (draft, 2026-07-30, local copy `research/pip3.md`): FP protocol, operands
  quantized to FP8 E4M3 with FP32 accumulation, noise rank r = 16, lossy (not bit-exact),
  miner must declare its hardware in advance because outputs differ bitwise per GPU.

**Commoditized:** INT8 multiply-adds, A100-class and consumer GPUs, small-model tokens,
embeddings (cents per million), batch tiers. **Scarce/expensive:** HBM capacity (memory to
hold big MoE weights and long KV caches), GB300 NVL72-class scale-up domains (SemiAnalysis:
large MoE inference runs best there), power and grid interconnects.

**[derived] What a MAC is worth.** H100 dense INT8 = 1.979e15 ops/s = about 0.99e15 MAC/s.
At $2.78/h (Ornn), 10^18 MACs take about 1,010 s = 0.28 h = **about $0.78** at 100%
utilization (about $1.56 at 50%). This is the ceiling on what any buyer would pay for raw
int7 matmul.

**[derived] Why posted-data matmul does not pay.** For m = n = k = N with int7 stored as
bytes: data in = 2N^2 bytes, compute = N^3 MACs. AWS internet egress is about $0.09/GB for
the first 10 TB (https://egresscost.com/aws/data-transfer-pricing, verified 2026-06, read
2026-10-17).
- N = 16384: 537 MB in, 4.4e12 MACs = $0.0000034 of H100 time; moving the inputs once
  costs about $0.048, about 14,000x the compute. C (int32) adds 1.07 GB more.
- N = 65536 (k at Pearl's maximum): 8.6 GB in, 2.8e14 MACs = $0.00022; egress about $0.77.
- Break-even needs about N/2 MACs per byte at $7.8e-19 per MAC = $9e-11 per byte, i.e.
  N about 2.3e8. That is far outside k <= 65536. **No legal job shape makes the work worth
  more than the transfer.** The only economic matmul is one whose operands are already on
  the miner.

## 4. Decentralized and verifiable compute: what is maturing

### 4.1 Useful PoW (Pearl and its critics)
- Komargodski, Schen, Weinstein, "Proofs of Useful Work from Arbitrary Matrix
  Multiplication", 1 + o(1) overhead. https://arxiv.org/abs/2504.09971 (Apr 2025, read 2026-10-17)
- Pearl mainnet 2026-04-27; block 128,999 on 2026-10-17: 347.65M PRL issued (16.6% of 2.1B),
  2,248.84 PRL per block, about 1M PRL per day; 194 s target.
  https://www.tokenpost.com/news/technology/33759 (2026-10-17, read 2026-10-17)
- Hashrate about 60.13 EH/s on 2026-10-14 per PearlTrack (secondary snippet); 24 EH/s in early
  June (Basu). https://www.kucoin.com/blog/pearl-blockchain-ai-mining-prl-token (read 2026-10-17)
- Together AI "Gemma-4-31B-it-pearl", announced 2026-05-15, "funded by the PRL mined on the
  same forward pass": $0.28 / $0.86 per M input/output vs $0.39 / $0.97 standard (about 28%
  and 11% cheaper); the model page says it is not on Together's serverless API.
  https://www.together.ai/models/gemma-4-31b-it-pearl and https://pearlresearch.ai/network (read 2026-10-17). No usage figure published anywhere I found.
- Basu, "The Usefulness Gap in Proof-of-Useful-Work" (arXiv 2606.04819, 2026-06-03/05):
  24 EH/s, about 320k GPU-equivalents, about 112 MW; 8,012 workers of one pool (about 21% of
  hashrate) over May 30-31, no inference code in the dominant miner; 44 accepted shares from
  a random-matrix miner on NVIDIA, AMD, CPU and Apple Silicon; distribution checks defeated
  by adversarial Gaussian sampling; ROI -1% to +67% at PRL $0.76.
  https://arxiv.org/abs/2606.04819 (read 2026-10-17)
- Pass, "The Economics of Proof-of-Useful-Work" (arXiv 2606.06700, 2026-06-04): equilibria
  "Bitconia" (pure PoW), "Fortessia" (duplex replaces mining, same useful output),
  "Duplexia" (rewards are rebates on inference prices, extra useful compute); majority-attack
  cost stays tied to the block reward. https://arxiv.org/abs/2606.06700 (read 2026-10-17)
- Dikshit et al., "SoK: Is Proof-of-Useful-Work Really Useful?" (ePrint 2025/1814, Oct 2025):
  50+ constructions; "PoUW is actually not as useful as expected"; economic utility does not
  add to the security budget. https://eprint.iacr.org/2025/1814 (read 2026-10-17)

Implication for us: our job binding (job-fixed `job_key`, poster-committed `hash_a`/`hash_b`,
evaluator-paid W, `alpha < 1` self-dealing loss) already answers Basu's narrow critique
better than Pearl mainnet does (miners on job blocks cannot use their own random matrices).
It does not answer the broader one: a committed matrix is not a customer's useful job unless
a real buyer posted it, and section 3.3 shows buyers will not ship matrices.

### 4.2 Verifiable inference
- TOPLOC (Prime Intellect, arXiv 2501.16007, Jan/May 2025): locality-sensitive hashes of
  activations, 258 bytes per 32 tokens (about 8 bytes/token), 100% detection in their tests,
  robust to GPU nondeterminism; used to verify permissionless rollout workers in
  INTELLECT-2. https://arxiv.org/abs/2501.16007, https://www.primeintellect.ai/blog/intellect-2 (read 2026-10-17)
- GPU TEEs: H100 Confidential Computing throughput penalty 4-8%, shrinking with batch size
  (arXiv 2509.18886, Sep 2025). https://arxiv.org/abs/2509.18886 (read 2026-10-17). Trust
  rests on NVIDIA attestation and side-channel resistance.
- Ambient: SVM-compatible PoW L1 with "Proof of Logits" (validator recomputes one random
  token position), $7.2M raise; status conflicting (live API vs "announced"); sources are
  mostly promotional. https://blockeden.xyz/blog/2026/04/01/ambient-ai-l1-proof-of-logits-pow-blockchain-decentralized-inference/ (read 2026-10-17)
- Hollow-LLM attack (arXiv 2607.28884, 2026-07-30, IEEE S&P): a ZK proof of correct
  inference does not prove large-model execution; "ghost weights" keep the declared shape
  but collapse effective compute and still pass the circuit. https://arxiv.org/abs/2607.28884 (read 2026-10-17).
  Relevant to us: Pearl's rank-penalty/jackpot design (our NOTES-rank-penalty: zero or
  low-rank matrices give only a 0.82x edge) is one of the few mechanisms that prices
  **effort** rather than correctness.

### 4.3 zkML
- Lagrange DeepProve: self-reported 54x to 158x faster proving than EZKL; proves GPT-2 and
  Gemma-3 end to end; open-sourced June 2026; proofs several MB even for small nets.
  https://lagrange.dev/blog/deepprove-is-now-open-source (read 2026-10-17)
- Overhead commonly cited at 1,000x to 10,000x the inference compute (practitioner blog,
  https://blog.prompt20.com/posts/verifiable-inference/, read 2026-10-17). No independent
  2026 LLM proving benchmark found. Not a fit for a 60 s-block chain's job market today.

### 4.4 Distributed training over the internet
- INTELLECT-2 (32B, globally distributed RL, May 2025) and INTELLECT-3 (106B MoE, 12B active,
  arXiv 2512.16144); I could not confirm INTELLECT-3 used permissionless compute or TOPLOC.
  https://arxiv.org/pdf/2512.16144 (read 2026-10-17)
- Nous Psyche: coordinator on Solana; Hermes 4.3 (36B) trained fully on Psyche at about
  144k tokens/s across 24 nodes (secondary); Consilience 40B over 20T tokens.
  https://nousresearch.com/nous-psyche, https://ownyourmind.ai/projects/nous-research/ (read 2026-10-17)
- Pluralis-8B (Agora paper, arXiv 2607.13332): 2026-05-14 to 06-23, 40 days, 500B tokens,
  opened to public workers after step 5,000. https://arxiv.org/pdf/2607.13332 (read 2026-10-17)
- DiLoCoX (2025): 107B pretraining over 1 Gbps, 357x vs AllReduce (emulated);
  Factored Gossip DiLoCo (arXiv 2606.22768): up to 2x wall clock (unverified summary).
  https://arxiv.org/pdf/2506.21263 (read 2026-10-17)
- Gensyn: mainnet reported live 2026-04-22 by several sources, "training mainnet pending"
  by a later review; RL Swarm had about 12,000 testnet nodes. Conflicting; unverified.
  https://ownyourmind.ai/projects/gensyn/ (read 2026-10-17)
All of these run BF16/FP8 training and need gradients and optimizer state; none of them is
an int7 exact GEMM.

### 4.5 Decentralized compute markets (who actually earns)
- Akash: lease revenue $253,250 in Q1 2026 (about $1M annualized), $3.15M in 2025 (Messari via
  secondary). https://blockeden.xyz/blog/2026/03/12/depin-compute-revenue-pivot-akash-ionet-aethir/ (read 2026-10-17)
- io.net: $12.5M to $20M annualized, self-reported, sources conflict (same link).
- Bittensor: subnets about $28M to $35M annualized external revenue.
  https://cryptobriefing.com/bittensor-subnets-annualized-revenue/ (read 2026-10-17).
  Chutes (SN64): OpenRouter throughput peaked about 42B tokens/day (Feb 2026), about 6.8B
  later; self-reported $280k revenue per trillion tokens (June 2026); emissions (about
  $52M/yr) far above external revenue in March. https://ownyourmind.ai/tokenomics/chutes-bittensor-revenue-machine/, https://www.tao.media/inside-chutes-next-phase-revenue-efficiency-ai-training-and-secure-infrastructure/ (read 2026-10-17)
- Lesson: the decentralized-compute sector's real revenue is in the tens of millions per
  year against a market of tens of billions, and the winners (Chutes, Together's Pearl
  endpoint) sell **model tokens**, not raw FLOPs.

## 5. Who would actually pay for cheap, verifiable, permissionless matmul or inference

| Buyer | What they buy | Evidence of spend | Fit with our chain today |
|---|---|---|---|
| Agents and coding tools on open-weight models | tokens from DeepSeek, Qwen, GLM, Kimi | 128.4T tokens/week on OpenRouter, 72% open-weight; DeepSeek V4 Flash about $0.14/$0.28 per M | Needs a served model; our chain proves matmul, not tokens. Fit only via G1 |
| Price-insensitive-to-latency batch users | batch inference, embeddings | Batch tiers at 50% off are standard; embeddings $0.01 to $0.13/M | Incumbent batch is already cheap; we would need to beat it via the mining rebate |
| RL post-training labs and fine-tuners | rollouts (generation) | rollouts dominate on-policy RL cost (arXiv 2605.26606); INTELLECT-2 used permissionless rollout workers; RL ablations 10k to 100k GPU-hours (arXiv 2504.12501) | Good fit for evaluator-verified jobs (G2), not for raw matmul |
| Eval runners | benchmark sweeps | no sourced spend figure found | Same as RL rollouts; small |
| ZK rollups and provers | MSM/NTT/hash proving | Succinct SP1 Hypercube: 99.7% of Ethereum blocks under 12 s on 16 RTX 5090 (vendor, Nov 2025); Boundless 399K orders (aggregator); "$97M prover market" (blog) | Wrong kernel: MSM/NTT, not int7 GEMM. No fit |
| Scientific HPC | FP64 dense linear algebra | not researched in depth; B300 also cut FP64 (Tom's Hardware above) | int7 exact is useless for FP64 science. No fit |
| GPU operators / miners | working capital | Pearl ROI -1% to +67% (Basu); rental spikes when PoUW launches | Direct fit for Agent Credit liens (G3); spend figure for miner lending not researched in this slice |
| Agents paying per call | x402 micro-payments | about $24M over 30 days, $0.32 average (CoinDesk, July 2026) | Storefront already speaks x402; volume is small |

## 6. Gaps where our chain could add real value

Ranked by (real demand x what we already have) / difficulty.

### G1. Model-resident "standing jobs": subsidized open-weight inference (the Duplexia play)
- **What:** miners serve a registered open-weight model and mine on the GEMMs of the forward
  passes they run anyway. The chain pays them subsidy for those blocks; the storefront sells
  tokens of that model below market, the discount funded by mining income (Together/Pearl's
  model, Pass's "rebate on inference prices").
- **Who pays:** agents and coding tools buying open-weight tokens; routers (OpenRouter
  providers compete on price; Chinese open models win on price).
- **Evidence:** 128.4T tokens/week, 72% open-weight, 69% Chinese labs (OpenRouter
  2026-09-22); Together's Pearl endpoint at about 28% off input; Chutes' $280k per trillion
  tokens shows the revenue per token a decentralized provider actually earns.
- **What to build on SPEC v2:**
  1. New job kind `StandingJob` (or a `PostJob` flag): fixes `hash_b` = commitment to a
     registered weight matrix (one per GEMM shape of the model, e.g. each layer's projection,
     transposed and int7-quantized), leaves `hash_a` **free**: the miner commits its own
     activation batch per block (Pearl-native behaviour) inside the proof. The block header's
     `job_id` names the standing job. Today PostJob requires both hashes at post time; the
     verifier (crates/pearl-job, pearl-verify-sbf) would accept a miner-supplied `hash_a`
     for this kind only.
  2. A model registry: `RegisterModel {model_id, layer shapes, hash_b per layer, quant
     scheme, uri}`; weights in the blob store once, not per job (removes the section 3.3
     bandwidth problem: weights move once, activations never leave the miner).
  3. Subsidy policy: standing jobs draw from a separate emission budget (they have no
     protocol fee P, so `alpha x P` capping does not apply); cap per model per epoch, admin
     editable.
  4. Storefront: a `buy_tokens` path that does not post a job per request; x402 meter for
     tokens, served by registered miner endpoints, with TOPLOC-style commitments returned
     per response and spot-checked by evaluators (G2 machinery).
  5. Quantization: int7 operands mean requantizing int8 or FP8 models to 7 bits, with an
     accuracy cost we have not measured; long term follow PIP-3 (FP8) or this stays a
     niche on H100/H200/B200 and consumer cards (B300 INT8 is 1/30 of FP8).
- **What the proof then says:** "this miner did target-priced matmul work against the
  registered weights" (an effort proof, which is exactly what the Hollow-LLM paper says ZK
  inference proofs lack). It still does not prove that a specific customer's response used
  those weights; that needs TOPLOC or TEE attestation on top.
- **Difficulty:** high (new job kind, verifier change, registry, model-serving miner,
  quantization study). Highest real value.

### G2. Evaluator-verified non-matmul jobs: RL rollouts and evals
- **What:** jobs whose payload is "run model M on prompts P with sampling S, return outputs
  plus TOPLOC proofs"; evaluators recompute a sample (TOPLOC validation is far cheaper than
  the generation) and sign CompleteJob. Mining stays on matmul (fallback or standing jobs);
  W and the market do the paying.
- **Who pays:** RL post-training teams, open-model fine-tuners, eval runners. INTELLECT-2
  already ran permissionless rollout workers verified by TOPLOC.
- **Evidence:** rollouts are the main cost of on-policy RL (arXiv 2605.26606); RL ablation
  runs of 10k to 100k GPU-hours (arXiv 2504.12501), i.e. $28k to $280k at $2.8/H100-hour
  **[derived]**. No aggregate market figure found.
- **What to build:** `PostJob` already carries `data_uri`, `evaluators`, `threshold`, and
  CompleteJob is evaluator-signed, so the chain side is small: a job `kind` field so
  m/n/k/config/hash_a/hash_b may be zero for non-matmul jobs (they then never become block
  jobs, only escrow + settlement), a result-format spec (outputs + TOPLOC proofs in the blob
  store), and an evaluator service that runs TOPLOC validation instead of Freivalds. The
  storefront `Order` already records results on Solana.
- **Difficulty:** medium. Honest framing: this is an escrow-and-settlement market with
  verifiable results; the PoW is not what verifies it.

### G3. Agent Credit for GPU operators, underwritten by provable coinbase income
- **What:** extend the existing payout liens (SPEC 4.4) and coinbase receipts (3.4) into
  working-capital loans for miners and pools: a lender on Solana advances SOL/USDC, the lien
  routes a share of future coinbase and W to the lender, and the light client proves both
  income and repayment.
- **Who pays:** GPU operators renting capacity for PoUW (Basu: rental prices of budget GPUs
  up 38% when Pearl's miner shipped; ROI -1% to +67%), and agents that deliver jobs (W
  income).
- **Evidence:** demand for the GPUs is visible (section 3.1); I did not research the
  size of crypto-miner lending in this slice, so no number is claimed.
- **What to build:** mostly done (R1 receipts, R2 liens, R5 accumulator). Missing: a
  Solana lending program that reads `verify_coinbase` proofs to size credit lines on
  trailing proven income, pool-level liens (pool payouts are off chain today, SPEC 4.4
  excludes them), and a hashrate-to-income oracle from header difficulty.
- **Difficulty:** medium. Unique to us: no other PoUW chain makes miner income provable on
  Solana.

### G4. Reduced-output batch similarity jobs (embeddings kNN, dedup)
- **What:** int7-quantized embedding matrices, job returns per-row top-k instead of the full
  C. Pearl matmul fits well (k = embedding dim padded to 1024+).
- **Who pays:** RAG pipelines, dataset dedup.
- **Evidence:** embedding models are the most downloaded on Hugging Face (all-MiniLM at
  254M per 30 days), but embedding APIs cost $0.01 to $0.13 per M tokens and the matmul itself
  is worth fractions of a cent (section 3.3).
- **What to build:** a reduction spec in the job (`result = top-k per row`), a result
  commitment over the reduced output, Freivalds-style spot checks of sampled rows by
  evaluators.
- **Difficulty:** medium. **Marginal**: bandwidth still dominates unless the corpus is
  resident at the miner; only worth it as a demo of a real useful job, not as revenue.

### G5. Hardware-path hedge: an FP8 job mode
- **What:** follow Pearl PIP-3 (FP8 E4M3 operands, FP32 accumulate, declared hardware).
- **Why:** B300 INT8 is about 1/30 of FP8; serving engines lack Blackwell INT8 GEMM; most
  models are FP natively. Without this, G1 is stuck on 7-bit requantized models.
- **What to build:** new certificate/proof version in crates/pearl-job and the SBF verifier,
  a hardware declaration in the job or miner registration, lossy result checks (tolerance
  instead of bit-exact `result_hash`).
- **Difficulty:** very high (PIP-3 is itself a draft; bit-exactness and our light-client
  verifier both get harder). Track Pearl rather than lead.

### Vanity (do not build, or say clearly that they are not the value)
- **Matmul-as-a-service on posted data.** Section 3.3: transfer costs 10^3 to 10^4 times the
  compute for every legal shape. Our current PostJob/storefront flow is a correct demo of the
  mechanism, not a product anyone would buy for its output.
- **ZK proving on our miners.** Different kernels (MSM/NTT/hashing). Succinct and Boundless
  already run GPU prover markets.
- **Scientific / FP64 compute.** int7 exact is the wrong arithmetic.
- **Decentralized training on our chain now.** Needs BF16/FP8 gradients and fast
  interconnect; Psyche (also on Solana), Pluralis and Prime Intellect are years ahead. A
  later option at most, after G5.
- **"Verifiable AI" marketing on the PoW proof alone.** The proof shows effort on committed
  matrices, not correct inference; Basu and the SoK will be quoted against any stronger
  claim.

## 7. Open data gaps (not found, not invented)
- Current Epoch estimate of global inference vs training share.
- OpenRouter per-app and per-category token counts for this week (page did not render).
- Computer-use agents' share of tokens or spend.
- Usage of Together's Gemma-4-31B-it-pearl endpoint (no figure published).
- Size of crypto-miner lending markets (for G3).
- Accuracy cost of int7 (7-bit) requantization of current open-weight models (needs our own
  measurement before G1).
