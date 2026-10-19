# REPUTATION: a real track record for Cadre agents, and opening evaluators honestly

Research subagent, 2026-10-18. Question: how should agent profiles get a REAL track record,
and how could third-party evaluators be opened up without pretending to more verification
than we have? Web research by WebSearch/WebFetch (firecrawl was down); every outside number
below carries its source and date. Tags: [V] read on the primary page, [S] secondary or
self-reported, [U] unverified or sources disagree. Repo facts come from docs/SPEC.md
(2, 3.4, 4.1, 4.2, 4.5, 5.4), docs/RPC.md, docs/EXPLORER.md, chain/executor/src/lib.rs and
chain/state/src/schema.rs as of commit 50ffee5 plus the working tree. Nothing here is a
measurement of Cadre itself.

## 0. Answer in eight lines

1. Every agent reputation system with a public feedback write path is mostly sybil traffic.
   ERC-8004: 59-91% of reviewers coordinated sybils; a fake rating costs $0.0027-$0.055.
   x402: 48-90% of transactions gamed depending on tracker and window.
2. Registration counts are noise. Of 698,980 ERC-8004 agents, only 2-15% have a live
   endpoint.
3. Validation and evaluation layers are almost unused and unpaid. ERC-8004 Validation
   Registry: 0 requests on Ethereum. Virtuals ACP: the evaluator is optional and named by
   the client.
4. No AI verification network has a documented, named slash. Bittensor penalizes only
   through dividends, Prime Intellect only ejects, Psyche and Gensyn publish no slashing
   terms, Hyperbolic's slashing exists on paper. EigenLayer slashing has been live since
   April 2025, with no credible named event found.
5. Signals that survive come from three places: money from distinct counterparties
   (with self-dealing filtered out), checks against a reference (OpenRouter Exacto, the
   Moonshot vendor verifier), and liveness over time (Vast reliability, OpenRouter's
   30-second outage rule).
6. Cadre already has the one property the others lack. A fake job costs real money: the
   protocol fee P is non-refundable, and a self-dealer gets back at most alpha x P. So a
   track record weighted by P, and by SOL settled through the storefront, carries a cost
   to fake that a star rating does not.
7. Build first (no consensus change): indexer-derived profiles in the explorer, CLI and
   RPC, plus gateway server selection by record. Next, a Reject path, buyer-paid evaluator
   fees, and a tolerance-based replay check for the fp32 glue between int7 segments.
   Equivocation-only slashing comes last.
8. Do not build: star ratings or open feedback, stake-weighted curation, Bittensor-style
   weight consensus, subjective slashing, emissions-paid evaluators, or leaderboards by
   job or tx count.

## 1. What Cadre stores today (the raw material)

From the code, not the docs:
- `Agent` (schema.rs): `name, uri, registered_height, updated_height`. Nothing else.
  RegisterAgent is a pure self-claim.
- `JobRecord` holds `poster, worker_pay W, protocol_fee P, evaluators, threshold,
  created_height` (posted), `status, blocks_mined, cap_remaining, subsidy_paid, deliverer,
  result_hash, result_uri, closed_height`, plus a ticket or segment extension (model_id,
  server, rows, segments, blocks, subsidy).
- `Model` holds running totals: tickets, segments, segment blocks and subsidy.
- Every block has a 97-byte coinbase receipt: `height, miner, fees, subsidy, job_id,
  mature_height`. It is provable on Solana by tx_root inclusion (SPEC 3.4).
- `getAccountTxs` lists sent txs with height. The CompleteJob tx body names which evaluator
  keys signed. Neither is kept in state.
- Lifecycle facts that matter for reputation (executor lib.rs lines 502-552):
  - SubmitResult is first come: the first sender on an Open plain job becomes `deliverer`.
    Tickets accept only the ticket's server.
  - CompleteJob needs `threshold` valid signatures from the job's own evaluator list. W
    goes to the deliverer.
  - ExpireJob after the deadline refunds W to the poster, sets `closed_height`, and keeps
    `deliverer`.
  - So "Delivered, then Expired" is already readable from state as a delivery that no
    evaluator quorum accepted.
  - There is no Reject: an evaluator can only sign or stay silent.
- Evaluators are chosen per job by the poster: 1 to 8 keys plus a threshold. The
  storefront fills in its own attesters by default (services/storefront-x402 service.ts
  line 249). Evaluators are unpaid. Opening evaluators is therefore mostly a defaults and
  payment problem, not a consensus one.
- Cost to fake a completed job (SPEC 3.4, 5.1, 9, default parameters):
  - **Direct PostJob on chain**, by someone who also delivers and mines it: W comes back,
    and P goes to the mining pool, of which at most `alpha x P` (alpha 0.8) comes back as
    subsidy, and only if the same party mines all the blocks. Floor loss: `(1 - alpha) x P
    = 0.2 x P`, plus mining compute and slippage on the token.
  - **Through the storefront**, the buyback share (10% of `sol_in`, decided 2026-10-18) is
    also gone, and the payment is a Solana SOL transfer that the light client ties to the
    chain job.
  - In both cases the cost scales with P, not with the number of jobs. That is why every
    stat below is weighted by P or by SOL, never by count alone.

## 2. Findings: agent identity and reputation systems

### ERC-8004 Trustless Agents (identity, reputation, validation registries)

**What it is.**
- Spec at https://eips.ethereum.org/EIPS/eip-8004: a Draft ERC created 2025-08-13, in
  Review from 2025-10-08, revised 2026-01-07, 01-13 and 01-25 [V].
- Identity is an ERC-721. The `agentURI` points to an off-chain registration JSON listing
  name and endpoints.
- `giveFeedback(agentId, int128 value, valueDecimals, tag1, tag2, endpoint, feedbackURI,
  feedbackHash)` stores the value and tags on chain. The rest is event-only and
  aggregation is off chain [V].

**Who may rate.**
- Anyone except the agent's owner or operator [V].
- The Oct 2025 draft had `feedbackAuth`: an agent-signed pre-authorization of the client.
  It is gone from the current text, so rating is open [V on the old draft
  https://raw.githubusercontent.com/ethereum/ERCs/cb7ae28/ERCS/erc-8004.md; the removal
  commit is U].
- The spec itself says "Sybil attacks are possible, inflating the reputation of fake
  agents" and leaves filtering to downstream systems [V].

**Validation registry.**
- The owner names a validator, and only that address answers, with a score of 0-100.
- Incentives and slashing "are outside the scope of this registry" [V].

**Deployments.**
- Ethereum IdentityRegistry deployed 2026-01-29 (block 24,339,871), Base on 2026-02-03.
- The same 0x8004... addresses are used on every chain, and the upgrade owner is an EOA
  [S, https://llm4agents.com/blog/erc-8004-registries-onchain-audit, 2026-08-17].

**Usage.**
- 8004scan, fetched 2026-10-18 [S, https://8004scan.io/]: 698,980 registered agents,
  720,707 feedback entries, 754,889 unique wallets, 29 networks.
- Datawallet's 2026-09-18 snapshot [S, https://www.datawallet.com/crypto/erc-8004-explained]:
  - Base held 466,378 feedback entries, 79.1% of all.
  - About 2.24% of agents passed A2A/MCP health checks, and 89.2% declared neither protocol.
- llm4agents on-chain audit, 2026-08-17 [S]:
  - Ethereum had 3,215 NewFeedback events covering 1,668 rated agents (about 3.3%) from
    653 distinct clients.
  - Validation Registry: 0 requests on Ethereum, 13 requests and 8 responses on Base.
  - Datawallet instead found no Validation Registry address on any mainnet as of
    2026-09-18 [U, the two sources disagree].

**Sybil measurement.**
- Xiong et al., "Can Trustless Agents Be Trusted?", arXiv 2606.26028 v2 (2026-07-08, data
  to 2026-05-13) [V abstract, https://arxiv.org/abs/2606.26028]:
  - Only 3% (ETH), 4% (BSC) and 15% (Base) of identities have a valid registration file
    with a live endpoint.
  - 73.5%, 59.2% and 90.6% of reviewers show coordinated sybil behavior.
  - After filtering, 15.8%, 77.9% and 86.8% of rated agents have no valid feedback.
- Median cost to fake a rating: $0.055 (ETH), $0.0042 (BSC), $0.0027 (Base) [S, as
  summarized by Datawallet].
- Design gaps (llm4agents): a fresh address per review defeats the owner check, and
  `appendResponse` has no auth check [S].

**Lesson for Cadre:** a free write path for feedback becomes sybil data. ERC-8004 is useful
as an identity and endpoint card format, not as a source of reputation.

### Olas (Autonolas)

- Components, agents and services are NFTs. Operators stake OLAS, and staking contracts are
  chosen by veOLAS vote weighting [S, https://olas.network/data].
- https://olas.network/, read 2026-10-19 02:00 UTC [S, self-reported subgraph data]:
  - 20,889,621 agent txs, of which 14,819,821 agent-to-agent
  - 449 daily active agents (7-day average)
  - 3,774 operators
  - 3,210,494 OLAS staked
  - Mech Marketplace turnover $110,095.50 and protocol fees $878.66
- Mech page [S, https://olas.network/agent-economies/mech]: 12,110,320 requests,
  10,223,386 deliveries, $110,096.72 total task payments, 12 daily active mechs.
- No general reputation registry was found.

**Lesson:** a large tx count sits next to a small paid turnover. Requests and deliveries
are easy to show, but the dollars are the honest number. Cadre's explorer already leads
with paid figures, and profiles should too.

### Virtuals ACP (Agent Commerce Protocol)

**Design.**
- Request, Negotiation, Transaction, Evaluation, with escrow on Base.
- Live 2025-07-03, USDC settlement from 2025-08-12, v2.0 in April 2026 as the reference
  for draft ERC-8183 [S, a competitor's comparison,
  https://www.agirails.io/learn/agirails-vs-virtuals-acp/, verified 2026-07-02].

**Evaluator.**
- The client names the evaluator address at job creation, and its decision is final.
- A job with no evaluator pays out on submission.
- The provider/protocol split is 95/5, with a further 5% to a third-party evaluator when
  one is used [S, same source].
- Forkast, 2026-07-22, calls the evaluator "just an address on-chain" [S,
  https://forkast.news/the-machine-as-buyer-virtuals-acp-and-the-evaluator-problem/].

**Numbers (self-reported).**
- $479M aGDP, 1.77M+ completed jobs and $1.16M+ cumulative agent revenue in Feb/Mar 2026
  [S, https://bex.co/blog/2026/04/04/virtuals-protocol-ai-economic-operating-system-on-chain-agent-economy].
- An independent tracker counted 12,361,834 cumulative memo events on Base, but 7 memos
  from 3 senders on the latest day [S, https://agenteconomy.to/virtuals-acp, 2026-10-18].
- Forkast calls the split between real and simulated commerce a "black box".

**Lesson:** a buyer-named evaluator with a fee share is the right shape, and it matches
Cadre's per-job evaluator list. When the evaluator is optional and the headline metric is
gross volume, nobody checks quality. aGDP is the wrong headline.

### Fetch.ai Almanac

- A CosmWasm contract mapping an agent address to its endpoints, protocol digests and an
  expiry. Registration is signature-checked and expired entries drop out [V,
  https://network.fetch.ai/docs/introduction/almanac/introduction].
- Testnet fee 0.5 TESTFET, re-registration every 3600 s [V, uAgents config.py on GitHub].
- Agentverse claims "2 million+" agents (press release, 2025-11-19) [S].
- There is no reputation layer beyond search ranking.

**Lesson:** an expiring registration is a cheap liveness signal worth copying. A profile
nobody refreshes, and that answers no health check, should drop out of discovery.

### Google A2A Agent Cards

- `/.well-known/agent-card.json` with name, description, interfaces, version, capabilities
  and skills, plus optional JWS signatures over the JCS-canonicalized card. "Clients SHOULD
  verify at least one signature" [V, https://a2a-protocol.org/latest/specification/].
- No reputation of any kind: a signature proves origin, not quality.
- 150+ supporting organizations and 22,000+ GitHub stars [S, Linux Foundation release,
  2026-04-09,
  https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year].
- No count of live cards was found [U].

**Lesson:** make Cadre's `RegisterAgent.uri` point to an A2A-shaped card, so the identity
format is the standard one and the reputation comes from the chain.

### Recall Network

- AgentRank combines competition results with "certainty" from RECALL staked by curators,
  with graduated slashing of wrong curators [S, https://blog.recall.network/our-vision-for-agent-discovery, 2025-06-20].
- Competition payouts decay exponentially, and 1 staked RECALL = 1 boost [V,
  https://docs.recall.network/competitions/rewards].
- No sybil or identity rules are documented.
- No 2026 activity numbers were found [U].

**Lesson:** stake-weighted curation measures what curators bet on, not what buyers paid
for.

### Kite AI

- Mainnet and Agent Passport (on-chain identity plus spend limits) launched 2026-04-30
  [S, commissioned release,
  https://cryptobriefing.com/kite-launches-kite-chain-and-kite-agent-passport-enabling-autonomous-ai-agent-payments/].
- Testnet claims: 546M+ agent calls and about 4M users [S, 2025-03-29]; 17.8M passports
  [S, Whales Market].
- This was an airdrop-farmed testnet, and rate limits were added to restrict spam calls
  [S, Messari snippet].
- No post-mainnet usage was found [U].

**Lesson:** an incentivized testnet's counts carry no information about demand.

### x402 Bazaar and x402 volume

- Bazaar is a catalog built from the CDP facilitator's traffic. Its ranking blends
  relevance with 30-day call count, unique payers, last call time and metadata
  completeness. There are no reviews [V, https://docs.cdp.coinbase.com/x402/bazaar].
- The live discovery API reported 25,788 resources on 2026-10-18 [V,
  https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources?limit=1].
- How much of that traffic is gamed:
  - 4Pillars [S, https://research.4pillars.io/en/research/x402-on-polygon-real-or-gamed,
    2026-02-10]:
    - 63.94% of x402 txs were gamed on average over 2025-11-09 to 2026-02-07, with a peak
      day of 90.21%.
    - On Polygon, about 2.2M of 3.6M "real" txs came from 30 fresh wallets, 99.99% of them
      sending exactly 0.01 USDC to one recipient, and they stopped together around Feb 1.
  - Artemis filter, December 2025: about 48% of txs and 81% of volume linked to gamed
    activity [S, via Cryptopolitan; the primary post was unreachable].
  - Bloomberg's $24M 30-day headline adjusts to $1.6M [S, Artemis on X,
    https://x.com/artemis/status/2031768320081277029, about March 2026, date U].
  - Visa with Artemis, August 2026: adjusted volume about $19M, with about 4,000 wallets
    driving about 90% of adjusted spend [S,
    https://thedefiant.io/news/infrastructure/visa-s-sheffield-pegs-adjusted-x402-volume-at-19m].

**Lesson:** "unique payers" and "call count" are exactly the signals the wash traders
target. They mean something only when each payer carries a cost that cannot be recovered
and concentration is shown next to them.

## 3. Findings: verification and evaluation markets

### Bittensor validators

**Scoring.**
- Validators set weights on chain. Yuma Consensus clips weights above the stake-weighted
  consensus (kappa "typically 0.5"), and bonds held out of consensus are penalized [V/S,
  https://learnbittensor.org/concepts/scoring-rewards/yuma-consensus].

**Pay.**
- Validators earn emission dividends. Delegate take defaults to 18%, now settable from 0
  to 18% [V, https://docs.bittensor.com/staking-and-delegation/delegation].
- No stake slashing for bad scoring was found. The penalty is lower vtrust and lower
  dividends.

**Weight copying.**
- Copiers read the public weight matrix and keep vtrust without evaluating anything.
- Opentensor's working paper (2024-05-29) proposes random ground-truth tests as future
  work.
- Commit-reveal shipped in v7.3.0 in July 2024 [S]. It now uses Drand timelock, with a
  reveal period of 1 to 100 epochs, default 1 (about 72 min) [V,
  https://www.bittensor.com/docs/hyperparameters/commit-reveal-period].
- Known limit: it helps only if consensus moves within the reveal window. With stable
  scores, copying still works [V, docs snippet].

**Gaming and failures.**
- On 2026-04-16, 67 of 128 subnets burned 99%+ of miner incentives [S, Taostats data via
  FalconX,
  https://falconx.io/newsroom/state-of-bittensor-subnet-adoption-trends-network-mechanics-and-covenants-departure].
- Covenant's April 2026 exit (it ran 3 of the top 15 subnets): its subnet tokens fell up
  to 40% and TAO fell 24% within hours [S, same source].
- Collusion types described publicly: copying, validators boosting each other, validators
  favoring colluding miners [S,
  https://mattkotnik.medium.com/proposal-enhancing-dtao-with-subnet-blockchains-a-solution-for-validator-collusion-in-bittensor-4cf296aef2b0].

**Lesson:** when evaluators are paid from emissions for agreeing with each other, they
optimize agreement, not checking. Cadre evaluators should be paid by the buyer for
checks other parties can re-run, never for consensus with other evaluators.

### Gensyn Verde and RepOps

- Refereed delegation: the result is correct if at least one of the providers is honest.
- A dispute bisects by training step and then by operator, so the referee recomputes one
  operator [V, https://arxiv.org/html/2502.19405v1, Feb 2025].
- RepOps fixes floating-point operation order so results are bitwise identical across
  hardware [V, https://blog.gensyn.ai/verde-a-verification-system-for-machine-learning-over-untrusted-nodes/].
- "Verde and RepOps are already deployed in Judge" (2025-11-24). The post names no stake,
  slashing or referee, only that "the penalty for malicious behavior far exceeds the
  reward" [V, https://www.gensyn.ai/research/verde-verification-system-in-production].
- Mainnet launched 2026-04-22 [S].

**Lesson:** exact replay needs engineered determinism. Bisection down to one operator is
the template for a cheap on-chain dispute.

### Prime Intellect TOPLOC

**Method.**
- Commits the top-128 activation values every 32 tokens as a polynomial.
- Size: 258 bytes per 32 tokens in the abstract, "8 bytes per token" in section 5.8, a
  1000x reduction against storing embeddings (Llama-3.1-8B) [V,
  https://arxiv.org/html/2501.16007v2, ICML 2025].

**Results.**
- 0% false positives and 0% false negatives, deterministic and nondeterministic alike.
- Thresholds (bf16): exponent mismatches T_exp = 38, mean 10, median 8.
- Worst honest case across A100, 4090 and tensor-parallel 4090, and across FlashAttention
  2, SDPA and FlexAttention: 28 mismatches (21.88%).
- Altered prompts gave at least 67 mismatches, and a swapped model at least 125.
- bf16 generations validated only with a bf16 validator [V].
- Cost on Llama-2-13B: 0.26 ms/token to commit, 81 ms to validate [V].

**Enforcement.**
- INTELLECT-2 (2025-05-11): invalid rollouts "remove the submitting node from the pool".
  No stake or slashing is mentioned [V, https://www.primeintellect.ai/blog/intellect-2-release].

**Lesson:** tolerance thresholds calibrated on honest variance across real hardware work,
and ejection is the only penalty actually used.

### Ritual

- Infernet supports eager verification (on chain, in the same block) or lazy
  verification (later, with the proof expiring after a week). It is proof-system agnostic,
  so users choose and pay [S, https://docs.ritualfoundation.org/infernet/general/proofs].
- Symphony's design is "Execute Once, Verify Many Times", with a TEE variant.
- Today: "the builder only accepts results from registered executors with valid, unexpired
  attestations" [V, same docs].
- No stake or slashing terms were found. Testnet only, no mainnet date [S].

**Lesson:** in production, trust comes from attestation, not cryptoeconomics.

### EigenLayer AVS slashing

- Slashing went live on testnet 2024-12-20 [V, https://www.eigenlabs.org/blog/introducing-slashing/]
  and on mainnet 2025-04-17 (ELIP-002 to 004) [S,
  https://thedefiant.io/news/blockchains/eigenlayer-activates-mainnet-slashing-on-april-17-implementing-elips-002-004-ecb683e0].
- It is opt-in through Operator Sets, and withdrawals wait 14 days [V, EigenCloud docs].
- Blockdaemon reported "zero past slashing events" at launch [S].
- No count of AVSs with slashing enabled was found, and no credible, named slashing event.
  The reports that exist are unsourced or look like spam [U].
- To settle it, read `OperatorSlashed` events on the AllocationManager contract.

**Lesson:** shipping slashing does not mean it gets used. Do not market slashing until a
fault can be proven on chain.

### Hyperbolic Proof of Sampling (PoSP)

- Honest-only Nash equilibrium when `p > C / ((1 - r)(R + S))`. The paper's example: R =
  1.2C, S = 150C and r = 10% give p = 0.735% [V, https://arxiv.org/html/2405.00295v2,
  2024-05-01].
- spML protocol:
  1. With probability p, a second server recomputes the request.
  2. If the hashes differ, a 2/3 vote decides.
  3. The cheater loses S, and the user gets u + S/3.
- Determinism comes from fixed point or software floating point.
- Our own arithmetic on the paper's numbers: its "one undetected fraud every 22 days"
  (0.98% at 50 requests a day) works out to about one every 2 days.
- A deployed PoSP slash, or its own chain, was not found [U].

**Lesson:** random second checks at a low rate are cheap when the penalty is large and the
fault can be checked. The formula is useful for sizing a Cadre audit rate once a bond
exists.

### Nous Psyche

- A Solana coordinator program picks witnesses who check liveness and correctness. Bloom
  filters confirm DisTrO results were shared.
- Nous: "a practical implementation [of verification] is still an open problem." Today
  they duplicate work and compare outputs with similarity metrics they admit can be gamed
  [V, https://nousresearch.com/nous-psyche/, May 2025].
- No stake, slashing or reward terms on the page.

### Chutes (Bittensor SN64)

- Now "TEE-exclusive": workers must run Intel TDX with NVIDIA GPU evidence, and "the legacy
  GraVal-based GPU verification is no longer supported" [V,
  https://raw.githubusercontent.com/chutesai/chutes-miner/main/README.md].
- Validators are listed by hand in the miner config, and no miner stake is stated [V].
- On OpenRouter, roughly 42B tokens/day at the 2026-02-07 peak and about 7B/day averaged
  over 89 days, against a self-reported 160B/day [S,
  https://ownyourmind.ai/tokenomics/chutes-bittensor-revenue-machine/, updated
  2026-10-06].
- The Moonshot vendor verifier (below) scored Chutes' K2-thinking schema accuracy at
  83.05%, with a trigger similarity of 68.10%, below the 73% bar.

**Lesson:** hardware attestation proves which code ran, not that outputs match the
reference. Output checks still caught quality gaps.

### Determinism, the key to replay checks

**Thinking Machines** (2025-09-10) [V,
https://thinkingmachines.ai/blog/defeating-nondeterminism-in-llm-inference/]:
- The main cause is batch size changing with server load, which changes how reductions
  split.
- Experiment on Qwen3-235B at temperature 0:
  - 1,000 runs gave 80 unique completions, diverging at token 103.
  - With batch-invariant kernels, all 1,000 were identical.
- Cost on Qwen3-8B: 26 s by default, 42 s deterministic with an improved attention kernel.

**SGLang and vLLM.**
- SGLang's deterministic mode: average slowdown 34.35%, only at TP1/TP2, dense models only
  [V, https://lmsys.org/blog/2025-09-22-sglang-deterministic/].
- vLLM's `VLLM_BATCH_INVARIANT=1` needs compute capability 9.0 or higher [V,
  https://docs.vllm.ai/en/latest/features/batch_invariance/].

**What none of them show:** bitwise equality across different GPU models. That gap is why
RepOps exists and why TOPLOC uses tolerances.

**For Cadre:** the GEMMs are exact int32 by construction (INT7-ACCURACY section 1), so they
are reproducible on any hardware. The only nondeterministic part is the fp32 glue between
segments (attention, norms, RoPE, SiLU, sampling).

## 4. Findings: how marketplaces rank sellers

**OpenRouter** [V, https://openrouter.ai/docs/features/provider-routing, read 2026-10-18]
- Default routing first prefers providers "that have not seen significant outages in the
  last 30 seconds". Among those, it picks the lowest-cost candidates weighted by the
  inverse square of the price. Uptime is the only quality input.
- `sort` can be price, throughput or latency, over rolling 5-minute p50 to p99.
- Providers that miss min-throughput or max-latency thresholds are deprioritized, not
  excluded.

**OpenRouter Exacto** [V, https://openrouter.ai/blog/announcements/provider-variance-introducing-exacto/,
2025-10-21, updated 2026-06-24]
- Every tool call in live traffic is checked for valid JSON, a known tool name and
  arguments that match the schema.
- Providers qualify by tool-calling accuracy, a normal propensity to call tools, and not
  being often ignored or blacklisted by users. The per-provider data is not published.

**Moonshot K2 Vendor Verifier** [V, https://github.com/MoonshotAI/K2-Vendor-Verifier,
tested 2025-11-15]
- About 4,000 tool-call requests per vendor, compared against the official API.
- The same weights scored 100% schema accuracy at some vendors and 83-87% at others
  (K2-thinking).
- Two vendors fell below the 73% trigger-similarity bar.

**Vast.ai**
- "New machines start at 60% and climb as they demonstrate availability" [V,
  https://docs.vast.ai/guides/concepts].
- Verification needs reliability "Over 90%" plus hardware minimums. It is automated, and
  detected errors cause temporary deverification [V,
  https://docs.vast.ai/documentation/host/verification-stages].
- Buyers filter with `reliability > 0.99`.
- The formula is not published.

**Akash**
- The tenant picks among bids, and "lowest isn't always best".
- Reputation is informal tiers (above 99% uptime and 6+ months; above 95% and 1-6 months;
  unknown under 1 month) [V, https://akash.network/docs/learn/core-concepts/providers-leases/].
- Audited attributes are worth only as much as the signer behind them.
- AEP-86 (Draft, 2026 Q2) proposes:
  - Tiers L0 to L4, where L3 needs 90 days at 99%+ uptime and a 98%+ lease completion
    rate.
  - Auditors registered by governance who post bonds. If two auditors disagree by more
    than one tier, both attestations are voided and both bonds frozen [V,
    https://akash.network/roadmap/aep-86/].

**io.net, April 2024**
- About 1.8M fake GPUs tried to join to farm rewards [S, The Block, 2024-04-28,
  https://www.theblock.co/post/291315/solana-based-depin-io-net-ceo-claims-network-was-attacked-in-detailed-postmortem].
- Today: an hourly PoW puzzle plus a time-lock proof, not run on hired workers [V,
  https://docs.io.net/docs/proof-of-work].

**Research on fake reputation**
- Bought Amazon reviews give a short-lived lift, mostly to low-quality products [V
  abstract, He, Hollenbeck, Proserpio, Marketing Science 2022, https://papers.ssrn.com/abstract=3664992].
- Wash trading is estimated at more than 70% of reported volume on unregulated exchanges.
  It was detected with first-digit, round-size and tail tests [V abstract, Cong et al.,
  https://nber.org/papers/w30783].
- Symmetric reputation functions are manipulable by sybils. Asymmetric ones, relative to a
  trusted seed, can be sybil-proof [V, Cheng and Friedman 2005].
- Seuken and Parkes (2011) show no sybil-proof accounting mechanism exists under
  reasonable assumptions [S].
- No primary source states the principle that reputation weighted by money from distinct
  counterparties is harder to fake. That principle is our inference from the evidence
  above.

**Which signals are hard to fake (synthesis of sections 2-4)**

| Signal | Fakeable by | Cost to fake | Verdict |
|---|---|---|---|
| Registrations, agent cards, tx count, call count | anyone | ~0 (gas) | noise |
| Star ratings / open feedback | sybil wallets | $0.0027-$0.055 per rating (ERC-8004) | noise |
| Unique payers (raw) | fresh wallets | one tiny payment each (x402 Polygon case) | weak alone |
| Paid volume (raw) | self-dealing loops | fees only, if the payment comes back | weak alone |
| Paid volume net of recoverable flows, by distinct payers, with concentration shown | collusion with real money | the part that cannot be recovered (for Cadre: `(1-alpha) x P` + buyback share) | **strong** |
| Completion rate under an evaluator the seller does not control | evaluator collusion | depends on who chooses the evaluator | **strong if the buyer chooses** |
| Output checks against a reference (Exacto, vendor verifier, replay) | nothing cheap | redo the real work | **strongest** |
| Liveness and uptime over time (Vast, OpenRouter 30 s) | staying online | real uptime | strong for availability, says nothing about correctness |
| Stake at risk | anyone with capital | opportunity cost, unless faults are provable | weak until a slashable fault exists |

## 5. Recommended design for Cadre

Roles on one account: **buyer** (poster), **server or deliverer** (SubmitResult, ticket
server), **miner** (coinbase `miner`), **evaluator** (CompleteJob signer). A profile is
the account's record in every role it played. The registered name and uri are display
only.

### 5.1 Profile stats (show these; computable from existing state and txs unless flagged)

All are indexer reads over `listJobs` / `getJob`, `getBlock` coinbase receipts,
`getAccountTxs`, `getModel` and storefront Orders, the same sources `apps/explorer` already
indexes. Amounts are TOKEN and SOL, never USD (the explorer has no price source).

**As server or deliverer (the main record)**

| Stat | Source | Notes |
|---|---|---|
| Completed jobs and tickets, W earned | JobRecord where `deliverer = acct`, `status = Completed` | count always paired with W |
| **P behind completed work** | sum `protocol_fee` of those jobs | headline number: the non-refundable money behind the record |
| **SOL settled via storefront** for its jobs | Finalized storefront Orders whose job it completed | stronger than P (SOL paid on Solana, light-client proven) |
| Distinct payers, top-payer share of P | distinct `poster` over completed jobs, plus the max share | show concentration next to the count, always |
| Self-dealt share | jobs where `poster = deliverer`, and jobs where the poster key was funded by the deliverer (Transfer graph via getAccountTxs) | the funding link is a heuristic, labeled as one; never hidden, shown as "excluded" |
| **Delivered-then-expired** (rejected or unaccepted deliveries) | `deliverer = acct`, `status = Expired` | exists in state today; caveat: an offline evaluator also causes it |
| Acceptance rate | completed / (completed + delivered-then-expired), by P | per P, not per count |
| Independently evaluated share | CompleteJob txs whose signing keys are neither the deliverer nor the poster, and not keys from the same operator | signer keys are read from the CompleteJob tx body, not state |
| Time to completion | `closed_height - posted_height` | p50/p90 in blocks |
| Time to delivery | SubmitResult tx height from getAccountTxs | **optional new state**: `delivered_height` on JobRecord, if RPC-only clients need it |
| Per model: tickets served, rows, segments | ticket ext + Model totals | ticket `rows R` is in state |
| Active since, last delivery | first and last delivery heights | stale profiles sink in discovery (Almanac-style expiry) |

**As miner**
- Blocks, split into paid-job and fallback, with subsidy and fees: from coinbase receipts.
- Distinct jobs and posters it mined for.
- Segment blocks per model, which shows it holds the weights.
- Every figure is provable on Solana by receipt inclusion (SPEC 3.4).

**As buyer**
- P and W paid, jobs posted, the share that expired with no delivery, and the share
  completed. This protects sellers from buyers who never pay out (rare here, since W is
  escrowed).

**As evaluator**
- Jobs signed, P of those jobs, and the distinct sellers and buyers it signed for.
- Share of its signatures on jobs where it was also deliverer or poster.
- Equivocations: two signatures on different `result_hash` for one job. These are
  detectable off chain today, but no tx exists to act on them.

**Shown, not computed:** `RegisterAgent.uri` should point to an A2A-shaped agent card
(name, endpoints, skills, optional JWS). The chain stays the source for reputation and the
card for endpoints.

**Never shown as reputation:** registration age alone, tx count, raw job count without P,
raw unique wallets, any rating.

### 5.2 How a buyer or gateway picks a server by record

Today the gateway is the server, and segments are mineable by anyone holding the model,
so the "miner choice" is the protocol's, not the buyer's. The choice that matters is the
ticket's server (the `hash_b` server key; 0 means the first PostSegments sender).

A multi-server router (gateway mode) works off chain, so it is policy and needs no
consensus change. Thresholds are provisional knobs, not measured values:

1. **Eligible servers:**
   - It holds the model: segment blocks for that model_id, or a passed fetch probe.
   - It answered a health probe in the last 30 s, borrowing OpenRouter's outage window.
   - Its agent-card endpoint is live.
2. **Quality gate:**
   - Acceptance rate by P over a trailing window is above a bar.
   - Independently evaluated share is above a bar.
   - It has passed the gateway's own reference spot checks. Cadre can replay exactly
     (section 5.3), so those checks are exact rather than statistical like Exacto.
3. **Pick:**
   - Among eligible servers, weight by inverse square of the quoted price, as OpenRouter
     does, multiplied by a record factor (for example, capped log of independent P
     behind completed work).
   - New servers start in a probation lane, like Vast's 60% start: a small, capped share
     of traffic and a ticket-size cap until the record exists.
4. **Show the buyer why:** the gateway's reply names the server and links its profile.

On chain, a buyer who wants to choose by record still names the server in the ticket.
Nothing new is needed for that.

### 5.3 Opening evaluators honestly

What an evaluator can check today (SPEC 5.4):
- The response blob hashes to `result_hash`.
- Every registered matrix has a segment.
- Each `C = A B` matches `c_hash` exactly.

What it cannot check: the fp32 glue linking one segment's C to the next segment's A. A
server can commit correct GEMMs over wrong activations. The sections below close that gap
before claiming more.

**Replay check for int7 greedy decoding**

Proposal for services/attester. It needs a measurement first and makes no protocol change.
- The GEMMs are exact on any hardware. Only the glue (attention, RMSNorm, RoPE, SiLU,
  residuals, argmax) is fp32.
- The evaluator recomputes, layer by layer, the next A from the previous segments' C plus
  the model's non-GEMM tensors. It then compares this with the posted A (an int7 tensor
  per segment).
- **Exact match is the wrong rule across platforms.** Thinking Machines shows divergence
  from reduction order, and TOPLOC needed thresholds. The rule should be a mismatch
  budget: entries off by more than 1 quantization step are not allowed, and the count of
  entries off by exactly 1 step stays under a threshold calibrated on honest runs across
  CPU and GPU. This follows the TOPLOC pattern.
- Output tokens are checked by argmax replay. A token may differ only where the reference
  top-2 logit margin is below a small epsilon.
- Cost: the glue is much smaller than the GEMMs. SPEC 4.5 counts attention at `2 x 32 x 64
  x T` MACs per layer per token against 1,235,746,816 GEMM MACs per token row on
  Llama-3.2-1B.
- What to measure first (fits research/int7-exp): on the same requests, run CPU fp32 and
  one GPU, and count how often an int7 activation entry flips by one step, and how often
  greedy tokens differ. Set thresholds from that data. If the flip rate is high, the
  fallback is a batch-invariant, fixed-order glue implementation (the RepOps idea, applied
  only to the small fp32 part).
- Optional, for cheaper checks: if servers also publish C blobs, an evaluator can check
  each `C = A B` with Freivalds' randomized check, O(mk + kn + mn), instead of recomputing
  O(mkn). The data cost is m x n x 4 bytes per segment. Measure it for lm_head shards
  before choosing.

**Who evaluates.** Anyone already can: the evaluator list is per job (1 to 8 keys plus a
threshold). Opening up means:
- publishing the check as a spec plus the reference attester code, so a third party
  runs the identical check;
- letting buyers name evaluators through storefront-x402 (the spec field exists, and only
  the default is ours);
- showing in profiles which evaluator signed.

No consensus change is needed for any of this.

**Who pays evaluators.** The buyer pays, per job, as an explicit line in the price (the
ACP pattern: a share to a third-party evaluator). Never from emissions: SYNTHESIS "do not
build" says rewards for work nobody bought are out, and Bittensor shows what
emission-paid scoring optimizes for.
- **New state:** `evaluator_fee E` in the PostJob / ticket body and JobRecord. CompleteJob
  pays E split equally among the first `threshold` valid signers, and W to the deliverer.
  On expiry, E goes back to the poster.
- The storefront prices E into the order. Default E is an owner decision, not a number we
  have data for.

**Bonds and slashing (later, and only for provable faults).**

Faults the chain can prove:
- **Equivocation:** one key signing two different `result_hash` for one `job_id`. The two
  signatures are the proof, and checking them is cheap on chain.
- **Signing a delivery whose segment C does not equal A x B:** this needs entry-level
  fraud proofs. Today `hash_a`, `hash_b` and `c_hash` are flat blake3 commitments, so a
  challenger cannot open one row of A, one column of B and one entry of C. That requires
  Merkle-structured commitments, a format change to pearl-job, the registry and the
  attester. High effort. It mirrors Verde's idea of bisecting to one operator.
- **Signing a delivery with wrong glue:** not provable on chain at all, because
  consensus must not depend on blobs or floating point. This stays off-chain reputation
  (the evaluator's record), plus random second checks.

**New state for an evaluator registry (`RegisterEvaluator`):**
- a bond, and an unbonding delay longer than the longest job deadline window;
- `SlashEquivocation` (two signatures) burning or splitting the bond to the reporter;
- a profile flag "bonded evaluator".

Honest framing: the bond is a sybil cost and an exit penalty, not a correctness guarantee.
No AI network has a documented slash, so do not market it as one.

**Random second checks (PoSP style).**
- A fraction p of completed tickets is reassigned to a second evaluator, chosen
  deterministically from a later block hash so it cannot be predicted at signing time.
- If that evaluator disagrees, a public flag goes on both profiles.
- Paid from E (the buyer's evaluator fee), not from the pool.
- Size p with PoSP's `p > C / ((1 - r)(R + S))` once a bond S exists. Without a bond, the
  only penalty is reputation, as with Prime Intellect's ejection.

### 5.4 Fixes the record needs (small, from reading the executor)

- **First-come SubmitResult on plain jobs.** Any account can deliver garbage first and
  lock an Open job until its deadline. The record shows it as the griefer's own
  delivered-then-expired, but the buyer loses the job. Tickets are safe (server-only).
- **Proposed new tx: `RejectResult`.** It carries threshold evaluator signatures over
  `b"pearl-sol:reject:v1" || job_id || deliverer || result_hash`, returns the job to Open
  (or names a server or deliverer allowlist), and records the failure in state.
- With it, "rejected" becomes an explicit stat instead of being inferred from expiry, and
  griefing stops.

### 5.5 Ranked plan (value, then effort)

| # | What | Value | Effort | New state? |
|---|---|---|---|---|
| 1 | Profile stats of 5.1 in apps/explorer (`/agent/{acct}`, `/api/agent/{acct}`), `pearl agent <acct>`, ranked lists by independent P | high: the one honest record anyone in the agent space shows | low (indexer over existing reads) | no |
| 2 | RegisterAgent uri -> A2A-shaped agent card; health probe; stale profiles drop from lists | medium | low | no |
| 3 | Publish the evaluator check spec + reference attester; storefront-x402 lets buyers name evaluators; profiles show signers | high: opens evaluation without claiming more | low | no |
| 4 | Gateway server selection by record (5.2) once there is more than one server | high when servers exist | low to medium | no |
| 5 | Measure int7 glue flip rate CPU vs GPU, then tolerance replay in services/attester (5.3) | high: closes the M1 "wrong activations" gap | medium | no |
| 6 | `RejectResult` tx + reject count (5.4) | medium-high | medium | yes (tx kind, JobRecord field) |
| 7 | `evaluator_fee E` paid on CompleteJob | medium: needed for anyone but us to evaluate at scale | medium | yes (PostJob / ticket body, JobRecord) |
| 8 | Random second checks paid from E | medium | medium | small (selection rule) |
| 9 | Evaluator registry with bond + equivocation slashing | low-medium | medium | yes |
| 10 | Merkle-structured A/B/C commitments for entry-level fraud proofs, slashable wrong-C signing | medium, long term | high | yes (proof format) |
| 11 | On-chain `AgentStats` record (completed, W, P, rejects) so Solana programs can read a record via the light client | only if a Solana-side consumer needs it | medium | yes |
| - | `delivered_height` on JobRecord | low (convenience for RPC-only clients) | low | yes |

### 5.6 What NOT to build

- **Star ratings or open feedback** (ERC-8004-style giveFeedback). Measured: 59-91% sybil
  reviewers at $0.0027-$0.055 per rating.
- **Leaderboards by job count, tx count, call count or unique wallets.** These are the
  exact things x402 wash traders and Kite testnet farmers produced. Every ranking is by P
  or SOL behind independently evaluated work, with concentration shown.
- **Stake-weighted curation or voting on agents** (Recall boosts, veOLAS-style
  nomination). It ranks what holders bet on, not what buyers got.
- **Evaluator consensus scoring** (Yuma-style agreement rewards). It invites weight
  copying and collusion. Evaluators are judged by re-runnable checks and second-check
  disagreements, not by agreeing with each other.
- **Slashing for anything the chain cannot prove** (quality, wrong glue). EigenLayer
  shipped slashing and nobody uses it. Subjective slashing would be either unused or a
  governance weapon.
- **Evaluator pay from emissions or the mining pool.** It breaks "only paid work earns".
- **TEE attestation as a requirement** (Chutes, Ritual). It is a different trust model,
  and Chutes still scored below the reference bar on output checks. It could be an
  optional profile badge later, not the basis.
- **ERC-8004 mirroring as the source of truth.** It is fine later as a distribution
  card, but its reputation registry must not feed our ranking.
- **"Verifiable AI" claims for the whole inference** until the replay check (item 5) is
  measured and running. Today the evaluator verifies the GEMMs, not the forward pass.

## 6. Open points for the owner

- Default evaluator fee E and who sets it (buyer or storefront default).
- Whether plain jobs keep open first-come delivery (and need RejectResult) or move to a
  named-deliverer model like tickets.
- Whether "bonded evaluator" is wanted at all before entry-level fraud proofs exist.
