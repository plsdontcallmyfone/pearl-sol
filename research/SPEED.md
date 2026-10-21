# SPEED: weak-block pre-confirmations and a shorter block target for Cadre

Research note, 2026-10-21. Scope: the two owner-approved steps, (1) soft pre-confirmations
from weak blocks and (2) a target near 30 s with uncle-style credit for orphaned useful work.
Not a signing committee, not a BlockDAG. Every figure has a source [Sn] or a calculation with
its inputs. Cadre-internal inputs: header is 256 bytes [R1]; plain Pearl proof verify is
0.36-1.75 ms on CPU [R2]; proof size P is 12.6-24.7 KB and light-client cost is 7-10M CU and
34-37 txs per header (from the brief). KB here means 1000 bytes.

## Summary

- Weak blocks (also called near blocks, sub-blocks, input blocks, weak headers) have been
  proposed for Bitcoin since about 2013 [S2], formalized as subchains [S1], prototyped in
  Bitcoin Unlimited [S3], extended to Storm [S4] and Tailstorm [S5], and given a rigorous
  reward and fork-choice treatment in StrongChain [S12]. None shipped on Bitcoin or BCH
  mainnet. Ergo's input blocks are the closest PoW deployment; as of mid 2026 they are in
  devnet with 60 s ordering blocks and 60 input blocks per ordering block [S30].
- A weak-block "confirmation" is not binding. Its value is information (who has adopted the
  tx, measured in work) plus whatever economic penalty the protocol attaches to a strong
  block that ignores it. Without a reward link, the guarantee is just "the next strong block
  is probably from someone who saw the tx".
- For Cadre, a weak block carries a full Pearl proof, so bandwidth scales with P. At weak
  ratio 1/16 and a 30 s target: 0.533 weak blocks/s, 6.9-13.3 KB/s per peer link. That is
  fine. CPU is negligible (under 1 ms per verify).
- Stale rate at 30 s with an effective propagation delay of 1-3 s is 3.3-9.5% by the
  Poisson model; measured chains with small blocks did much better (Dogecoin 0.619% at
  1 min [S24]).
- Recommendations: weak ratio 1/16; weak blocks reference only the current strong tip
  (StrongChain-style flat set, not a subchain); fixed independent weak rewards plus a
  nephew bonus; uncles depth at most 2 with decaying reward; fork choice stays heaviest
  main-chain work without uncles or weak work (the light client is unchanged); K = 60 at 30 s.

## 1. Bitcoin weak blocks and subchains

**Origins.** In September 2015 Gavin Andresen opened the bitcoin-dev thread "Weak block
thoughts": miners pre-announce the blocks they are working on before solving the PoW, and
since concurrent weak blocks mostly share transactions, the extra bandwidth and CPU should be
small [S2]. Rusty Russell (who coined the term) proposed a first cut that validates only the
PoW and uses weak blocks purely as a bandwidth optimization, and credited Greg Maxwell with
the idea and Peter Todd with "near blocks" about two years earlier [S2]. Maxwell's caveats in
that thread: unless the weak block's tx list can be a superset of the final block's,
size-proportional propagation cost is not fully removed, and another miner can reuse a weak
block by swapping the coinbase [S2].

**Subchains (Rizun, Ledger 2016).** Weak blocks chain into a subchain between two real
blocks; the real block is propagated layer by layer over the whole interval instead of all at
once, which lowers orphan risk, and miners are motivated to cooperate because they can carry
more fee-paying txs without more orphan risk. No Bitcoin consensus change is required [S1].
StrongChain's critique: subchains skip a security analysis at the subchain level and, because
they use a Bitcoin-like selection rule within the subchain, are open to selfish mining on the
subchain itself [S12].

**Bitcoin Unlimited.** awemany posted a pre-alpha weak-blocks proof of concept for BU in
December 2017, following Rizun's paper; weak blocks were handled like strong blocks but marked
and kept off the active chain [S3]. BU also deployed Xthin and later Graphene (Bloom filter
plus IBLT set reconciliation, which never sends the full tx-id list) as a dev-only protocol in
2018 [S8].

**Storm (awemany, 2019).** Merges weak blocks into "delta blocks" aiming at instant
confirmations on BCH. Its own motivation names the two classic objections: with no
consensus-level validity rules miners have little incentive to follow weak blocks, and weak
block emission every few dozen seconds is too slow for instant payments [S4].

**Tailstorm (Keller, Glickenhaus, Bissias, Griffith, AFT 2023).** Each summary block needs
k subblocks (each with PoW) confirming the same parent summary; subblocks may point at another
subblock, so they form a tree. All subblocks in a tree get the same reward
`discount = (c / k) * max depth`, so withholding (which branches the tree and reduces depth)
is penalized [S5]. Fork preference: higher summary height, then larger subblock tree, then
reward [S5]. Results with k = 8: the minimal attacker share at which dishonest behaviour pays
(trained RL policy) is 41.6% for Tailstorm vs 33.6% for Bitcoin at gamma = 5%, and 35.8% vs
26.5% at gamma = 50% [S5, Table 2]. Their worked example: k = 60 with a 10 min summary gives
subblocks every 10 s; a 6-subblock confirmation takes about 1 min, and invalidating it forks
the tree by 6 or more, capping depth at 54 of 60, which destroys one tenth of the minted
rewards [S5, sec. 7]. The prototype is a fork of BU's BCH node with k = 3 (200 s subblocks)
and uses Graphene and compact blocks [S5]. On the BCH research forum, the thread on weak
blocks notes that Storm and Bobtail ideas ended up in Tailstorm, that weak-block attempts
without consensus changes have failed before, and that Tailstorm would change issuance [S6].
I found no BCH activation of Tailstorm.

**BIP152 compact blocks and how weak blocks pre-propagate tx sets.** A compact block is the
header plus 6-byte short tx IDs (SipHash, truncated) and a few prefilled txs [S7]. In
high-bandwidth mode a peer pushes the `cmpctblock` before full validation, needing only a
valid header; if all txs are in the mempool the block reconstructs with no extra round trip
(0.5 RTT from the fastest peers), otherwise a `getblocktxn` round trip costs 1.5 RTT [S7].
Weak blocks remove that extra round trip: the receiver already fetched any missing txs when
it saw the weak block. That is exactly the 2024 revival by Greg Sanders ("Second Look at
Weak Blocks", Delving Bitcoin), aimed at compact-block relay when mining and relay policies
diverge, with a proof of concept at a 2x PoW multiplier "for testing" and a suggestion of
about 16x (50-100 weak blocks per block) from ajtowns [S9, S10]. That thread explicitly does
not claim confirmation security: weak blocks there are not consensus, and alone they give no
ordering or conflict resolution [S9].

**Security of weak-block confirmations.** Three things are true at once:
1. Weak blocks are free in work: every attempt that hits the strong target also hits the weak
   one, and the weak hits are byproducts of the same search.
2. Because PoW is memoryless, seeing n weak blocks does not change who finds the next strong
   block. A double spend of a weakly confirmed tx only needs the next strong block to omit it.
3. The double-spend cost is therefore only what the protocol attaches: rewards that the
   strong miner forgoes by not referencing the weak blocks (StrongChain, Tailstorm), or
   chain weight it loses (StrongChain counts weak work in fork choice) [S12, S5]. With
   neither, the cost is zero for the attacker who mines the strong block.

**Selfish mining.** Subchains are vulnerable to selfish mining within the subchain [S12].
StrongChain names new strategies: reclusive (withhold own weak headers), spiteful (refuse
others' weak headers), and delaying a strong block to waste others' weak headers; in its
model reclusive mining does not pay for any alpha < 0.5 [S12]. Tailstorm's depth discount
exists precisely to punish withholding [S5].

## 2. Bobtail (Bissias and Levine, arXiv 2017, NDSS 2020)

A block is valid when the mean of the k lowest hash values (order statistics) found for it is
below a target; k = 1 is Bitcoin [S11]. Variance of inter-block time falls as O(1/k) relative
to k = 1 (so the coefficient of variation falls as about 1/sqrt(k)) [S11]. Attacks:
- Double spend: with k = 1, a 40% attacker succeeds with 30% probability against an 8-block
  embargo; with k >= 20 it falls below 1% [S11, NDSS version]. (The arXiv draft's abstract
  states this differently; the two versions differ [S11].)
- Selfish mining: a 45% selfish miner takes about 95% of blocks at k = 1; at k >= 20 selfish
  mining is less successful than honest mining [S11].
- New intra-block attacks: proof withholding (hide some of your proofs to assemble packages
  with more than your share) and zero-cost zero-confirmation double spends (reusing proofs on
  two branches), mitigated by the reward design [S11].
- Cost: larger blocks and more traffic [S11]. Simulated orphan rate stays at or below
  `1 - e^(-tau/T)` (tau = 10 s, T = 600 s) [S11, Fig. 3].

Relevance: Bobtail changes block validity (k proofs per block). For Cadre that means k Pearl
proofs per header in the Solana light client, i.e. k times 7-10M CU. Not compatible with the
"light client unchanged" constraint. Its lesson (rewarding partial work reduces variance and
selfish-mining gain) carries over to weak-block rewards.

## 3. Kaspa / GHOSTDAG / DAGKNIGHT (why we are not going DAG)

GHOST (Sompolinsky and Zohar, 2013/2015) picks the heaviest subtree rather than the longest
chain and argues the block rate can safely exceed one per second [S13]. PHANTOM / GHOSTDAG
generalizes to a DAG ordered by a k-cluster rule [S14]; DAGKNIGHT (Sompolinsky and Sutton,
2022) removes the latency parameter k [S15]. Kaspa moved from 1 to 10 blocks per second at
the Crescendo hard fork on 5 May 2025, cutting reward per block by 10x so emission per second
is unchanged [S16, S17]. As of a September 2026 report, DAGKNIGHT was still a proposed
replacement for GHOSTDAG, not activated; I found no primary source confirming activation
[S18].

Why not for Cadre:
- Every Cadre block carries a 12.6-24.7 KB proof. At 10 blocks/s that is 126-247 KB/s of
  proofs per peer link before any txs (10 x P).
- The Solana light client verifies every header at 34-37 txs each; at 10 blocks/s that is
  340-370 Solana txs per second, plus a DAG ordering rule on chain. Infeasible.
- Weak blocks plus a 30 s chain get most of the latency win (seconds to a soft
  pre-confirmation) while the canonical object stays a chain.

## 4. Ethereum uncles (PoW era)

**Theory.** GHOST counts stale blocks when choosing the heaviest subtree [S13]. Ethereum
adopted uncle *rewards* but not GHOST fork choice.

**Actual rules (go-ethereum ethash, v1.10.26) [S19]:**
- `maxUncles = 2` per block.
- Validation gathers 7 ancestors; an uncle's parent must be one of them but not the block's
  own parent, so uncle depth `d = N - uncle.Number` is 1..6.
- Uncle miner gets `(uncle.Number + 8 - N) * R / 8 = (8 - d)/8 * R`, i.e. 7/8 down to 2/8.
  The including (nephew) miner gets `R/32` per uncle.
- R was 5 ETH (Frontier), 3 ETH (Byzantium), 2 ETH (Constantinople) [S19].

**Fork choice.** geth stores total difficulty as `externTd = block.Difficulty() + ptd`
(parent TD), and reorgs when the external TD exceeds the local TD [S20]. Uncle difficulty is
not added. So Ethereum PoW fork choice was heaviest chain by summed block difficulty,
excluding uncles. Ritz and Zugenmaier state the same and note that ECIP-1029 proposed
weighting uncles in chain selection, which they argue would weaken security [S26]. Gervais
also notes the "Ethereum uses GHOST" claim is a misconception [S42].

**Difficulty.** Since Byzantium (EIP-100), the adjustment factor uses 2 instead of 1 when the
parent has uncles, so the DAA targets a mean rate that includes uncles and miners cannot raise
issuance by raising the uncle rate [S21, S26].

**Measured uncle rates.**
- First 280,000 blocks (post of 25 Sep 2015): 7.41% (20,750 uncles), 6.89% measured over all
  blocks; small miners (<= 10 blocks) 12.7%, largest (>= 10,000) 5.5% [S22].
- October 2016 regression: base uncle rate about 6.7% for a 0-gas block, plus about 1.86%
  per million gas (3-5% during Frontier) [S23].
- Gervais et al. (last 10,000 blocks, 2016): Ethereum stale (uncle) rate 6.8% at 10-20 s
  blocks, vs Bitcoin 0.41%, Litecoin 0.273%, Dogecoin 0.619% [S24, Table 1].
- 2017: ratio of blocks not extending the main chain 0.06 to 0.24, peaking in January 2018
  [S26].
- Rootstock (merge-mined, uncles in the DAA) as of a May 2025 draft: main blocks every 24 s
  with an uncle rate of 0.5 against a 14 s parameter [S40]. This is the cautionary case for
  short targets with uncle-driven difficulty.

**Uncle rate vs block time vs size (studies).**
- Decker and Wattenhofer (2013, blocks 180,000-190,000): median time for a node to receive a
  block 6.5 s, mean 12.6 s, 5% of nodes still without it after 40 s; observed fork rate
  1.69%; their model predicts 1.78% and an effective 11.37 s lost per block [S25].
- Gervais et al. simulated block intervals from 25 min to 0.5 s (Table 7, current Bitcoin
  block sizes). Stale rate r_s by case (1 = standard relay, 2 = plus unsolicited push,
  4 = sendheaders + push + relay network) [S24]:

  | interval | case 1 | case 2 | case 4 |
  |---|---|---|---|
  | 10 min | 1.51% | 0.13% | 0.13% |
  | 1 min | 2.15% | 0.35% | 0.29% |
  | 30 s | 2.54% | 0.45% | 0.52% |
  | 20 s | 3.20% | 0.86% | 0.82% |
  | 10 s | 4.77% | 1.73% | 1.59% |
  | 5 s | 8.64% | 2.94% | 3.05% |

  With 30% adversarial power, shorter intervals raise selfish-mining relative revenue (0.33 to
  0.42 from 25 min to 0.5 s, case 4) and halve the double-spend value [S24]. Block size: at
  10 min, case 1 stale rate goes 0.32% (0.1 MB), 3.17% (1 MB), 6.24% (2 MB), 29.97% (8 MB)
  [S24, Table 8]. The main lever is relay mechanism (push before validation), not interval.

**Selfish mining with uncles.**
- Ritz and Zugenmaier (2018): uncle rewards lower the profitability threshold from 0.25 (no
  uncle rewards, gamma = 0.5) to 0.245 at uncle ratio delta = 0.06, 0.225 at 0.12, and
  0.185 +/- 0.012 at 0.24 (the December 2017 level) [S26]. Mechanism: the selfish miner's
  lost races are partly refunded as uncles.
- Niu and Feng (ICDCS 2019): with gamma = 0.5 and uncle reward 4/8, selfish mining pays above
  alpha = 0.163, lower than Bitcoin's 0.25 [S27].

## 5. StrongChain (Szalachowski, Reijsbergen, Homoliak, Sun; USENIX Security 2019)

- Weak header: `Ts <= H(hdr) < Tw` with weak target Tw > strong target Ts [S12].
- Weak headers must point to the previous *strong* header (a flat set, not a subchain), are
  collected by miners and committed in the next strong block via an OP_RETURN binding hash;
  redundant fields are stripped so a weak header is about 60 bytes [S12].
- Rewards: strong finder gets R plus all fees; each weak header gets `gamma * c * R * Ts/Tw`,
  independently of the others, so rewards are not zero-sum and dropping others' weak headers
  cannot raise your own reward [S12]. Supply is not capped as a result [S12].
- Fork choice counts weak work: `chainPoW` adds `Tmax/Ts` per strong block and `Tmax/Tw` per
  weak header, so a secret strong block is beaten by a public block with more weak headers
  [S12].
- Numbers: number of weak headers per block is geometric with mean `Tw/Ts - 1`; at
  `Tw/Ts = 2^10` that is 1023 headers, about 6% load on a 1 MB block [S12]. With
  `Tw/Ts = 1024` a 0.1% miner has the same reward coefficient of variation as a 9% miner in
  Bitcoin [S12, Fig. 3]. Selfish mining pays only near a 45% share, vs about 33% for Bitcoin
  [S12]. Reclusive mining does not pay for any alpha < 0.5 [S12].
- Requires a hard fork; SPV clients receive the weak headers too [S12].

For Cadre: copy the flat structure and independent rewards. Do not copy weak work in fork
choice, because then the Solana light client must verify every weak header's Pearl proof
(1/rho times the per-header cost).

## 6. Ergo, other PoW, and the 2026 state of pre-confirmations

**Ergo.** kushti's sub-blocks paper (31 Oct 2023, status "Proposed", soft fork) proposes
exchanging tx deltas between full blocks with weak (optional) commitments [S28]. The docs
rename them input blocks (lower target `t = T/64`) and ordering blocks (2 min, final
settlement); ordering blocks validate the preceding input blocks; input blocks earn the fees
of "first-class" txs; activation after 90% of hashpower upgrades [S29]. The docs' prose says
about one input block per second, which does not match T/64 at 2 min (that is one per
1.875 s, computed 120/64) [S29]. 2026 roadmap: devnet relaunch in early February with 60 s
ordering blocks and 60 input blocks per ordering block; still in devnet testing in June [S30].
Guarantee: "fast, provisional feedback"; no collateral [S29].

**Ethereum based-rollup and L1 pre-confirmations (economic collateral).**
- Based preconfirmations (Drake, 8 Nov 2023): an opted-in L1 proposer signs a promise and is
  paid a tip; safety faults (promise conflicts with what was included while the slot was not
  missed) are fully slashable; liveness faults are slashed by a negotiated amount; promise
  latency on the order of 100 ms; a preconf can still be undone by an L1 reorg [S31].
- Bolt (Chainbound): live on Symbiotic mainnet from 5 Feb 2025; validators commit to include
  txs, enforced by Symbiotic slashing, collateral needed only during the proposal window plus
  a challenge period [S32].
- mev-commit (Primev): providers (builders) and validators stake or restake; a broken
  commitment slashes the provider's stake; 2% of the bid goes to the treasury; current docs
  put slashing liability mainly on providers [S33].
- ETHGas: validators pre-sell blockspace, collateral in ETH or restaked ETH via EigenLayer,
  slashed in proportion to blockspace sold; the CEO's 99.96% honoured figure was not verified
  by the reporter, and execution guarantees were slated for broad rollout in Jan-Feb 2026
  [S34].
- Commit-Boost: a validator sidecar for opting into commitment protocols (preconfs, inclusion
  lists) [S35]. Taiko runs based preconfirmations with whitelisted launch-partner preconfers
  [S36]. Luban's LingLong is a delegated pre-settlement layer [S37]. I could not find a
  primary source for "Taiyi" or preconf.wtf, so they are omitted.

**Solana shreds.** Jito ShredStream forwards leader shreds directly to subscribers ahead of
Turbine fan-out [S38]. That is early *data*, not a guarantee: a slot can still be skipped or
forked.

**Bitcoin revivals.** The 2024 weak-blocks work is about relay latency, not confirmations
[S9, S10]. I found no 2025-2026 PoW chain on mainnet with sub-block pre-confirmations; Ergo is
the nearest, in devnet [S30].

**What each guarantees.** Ethereum preconfs: a signed promise backed by slashable collateral
(economic, can be large). Ergo input blocks, Tailstorm subblocks, StrongChain weak headers:
pure work plus reward forfeiture (small, proportional to the weak reward). Shreds: nothing
beyond early visibility. Cadre's weak blocks will be in the second class.

## 7. Useful-PoW interaction for Cadre

Weak blocks are free byproducts: a miner searching matmul tiles for the strong target simply
publishes any tile whose hash also meets the easier weak target. No extra useful work is done
or wasted; the job being served is the same.

Definitions: T = strong target (s); rho = weak/strong difficulty ratio (1/16 means weak
target is 16x easier); P = proof bytes; H = 256-byte header [R1]; n_new = tx ids added since
the previous weak block (6 bytes each, BIP152-style short ids [S7]); V = verify time per proof;
D = peer links.

- Weak solutions per strong interval: `1/rho` in expectation (including the strong one);
  weak-only count is geometric with mean `1/rho - 1` [S12].
- Weak rate: `lambda_w = 1 / (rho * T)` per second.
- Bandwidth per link: `B = lambda_w * (P + H + 6 * n_new)`. Uploading node: `D * B`.
- CPU per node: `lambda_w * V` seconds of CPU per second.

Table (tx-id list excluded; add `6 * n_new * lambda_w` bytes/s). Inputs: H = 256 B,
P = 12,600 B or 24,700 B.

| T | rho | weak/s | spacing | B (P = 12.6 KB) | per day | B (P = 24.7 KB) | per day |
|---|---|---|---|---|---|---|---|
| 60 s | 1/8 | 0.133 | 7.50 s | 1.7 KB/s | 148 MB | 3.3 KB/s | 287 MB |
| 60 s | 1/16 | 0.267 | 3.75 s | 3.4 KB/s | 296 MB | 6.7 KB/s | 575 MB |
| 60 s | 1/32 | 0.533 | 1.88 s | 6.9 KB/s | 592 MB | 13.3 KB/s | 1150 MB |
| 60 s | 1/64 | 1.067 | 0.94 s | 13.7 KB/s | 1185 MB | 26.6 KB/s | 2300 MB |
| 30 s | 1/8 | 0.267 | 3.75 s | 3.4 KB/s | 296 MB | 6.7 KB/s | 575 MB |
| 30 s | 1/16 | 0.533 | 1.88 s | 6.9 KB/s | 592 MB | 13.3 KB/s | 1150 MB |
| 30 s | 1/32 | 1.067 | 0.94 s | 13.7 KB/s | 1185 MB | 26.6 KB/s | 2300 MB |
| 30 s | 1/64 | 2.133 | 0.47 s | 27.4 KB/s | 2370 MB | 53.2 KB/s | 4600 MB |

Example: 30 s, rho = 1/16, P = 24.7 KB: `(16/30) * (24,700 + 256) = 13,310 B/s`; per day
`* 86,400 = 1.15 GB`. With D = 8 links, upload is about 106 KB/s.

CPU: at the worst row (2.133/s) and V = 1.75 ms [R2], `2.133 * 1.75 ms = 3.7 ms` of CPU per
second (0.37% of one core). Not a constraint. The cost that matters is the DoS surface: a
peer can send junk weak blocks that cost a verify each, so check header target and parent
before proof verification and rate-limit per peer.

Only one thing scales badly: if the proof does not depend on the tx set, a weak block that
only adds txs still has to carry a fresh proof (it is a new PoW solution). Delta tx lists
(n_new, pointer to the previous weak block on the same tip) keep the tx part small.

## 8. Security model of pre-confirmations

Setup: tx X is first seen at time t0. Strong tip S. Attacker hashrate share q; share h of
hashrate follows the policy "include txs that weak blocks on my tip preconfirmed"; the rest
(1 - h - q) are neutral fee maximizers.

**Honest statement.** Weak blocks are not binding. The next strong block's miner may exclude X,
and nothing in consensus stops it (fork choice and finality are unchanged). The guarantee is
probabilistic and rests on (a) honest miners following the inclusion policy and (b) an
incentive that makes ignoring preconfirmed txs cost something.

**Next strong block.** PoW is memoryless, so whoever finds the next strong block is drawn by
hashrate regardless of the weak blocks already seen:
- `P(next strong block from a policy-following miner) = h`
- `P(next strong block excludes X) <= 1 - h`, and `>= q` if the attacker is trying.
  An attacker with q = 0.25 excludes X at the next strong block with probability 0.25.

**What n weak blocks tell you.** If n weak blocks on S since t0 include X, then at least the
hashrate that produced them has X. A useful confidence measure is the included fraction of
weak work since t0: `h_hat = n_incl / n_total` (weak blocks on S found after t0 that include X,
over all weak blocks on S found after t0). Under a binomial model its standard error is
`sqrt(h_hat (1 - h_hat) / n_total)`. Expected wait to n weak blocks: `n * rho * T`; at
rho = 1/16 and T = 30 s, 1, 3 and 6 weak blocks take 1.88, 5.62 and 11.25 s.

**Faked preconfirmation.** If the attacker shows X only to its own weak blocks (sends the
honest network a conflicting X'), the chance that n consecutive weak blocks on the tip are all
the attacker's is `q^n`: q = 0.25 gives 0.25, 0.0156, 0.000244 at n = 1, 3, 6. So require
n_total weak blocks *and* h_hat near 1 before showing "preconfirmed"; a mix of weak blocks
with X and with X' is a visible conflict and should show as "contested".

**Cost to the attacker.** Make it explicit in consensus:
- Rule: a strong block that references weak block W must include all of W's txs (a
  conflict-free superset). The strong miner earns a nephew bonus b per referenced weak block.
- Then excluding X means not referencing the n weak blocks that carry it: cost `n * b`
  forgone, plus the attacker's own weak blocks carrying X lose their weak reward w each.
- That cost is small next to Ethereum preconf collateral (for scale, a collateral proposal
  discussed 1000 ETH vs 1 ETH for solo stakers [S31 thread notes]). So preconfirmation is for
  low-value or time-sensitive flows, as in Tailstorm's coffee example [S5].

**With k strong confirmations (Rosenfeld 2014).** Merchant waits for n strong confirmations;
the attacker pre-mined one block; attacker blocks before n honest ones are negative binomial
[S39]:

`r(q, n) = 1 - sum_{m=0..n} C(m+n-1, m) * (p^n q^m - p^m q^n)` for q < p = 1 - q, else 1.

(Nakamoto's whitepaper uses a Poisson approximation for m instead [S39].) Values computed from
the formula; they match Rosenfeld's Table 1 at the checked points (q = 0.1, n = 6: 0.059%;
q = 0.3, n = 6: 15.645%) [S39]:

| q | n=1 | n=2 | n=3 | n=6 | n=12 | n=30 | n=60 |
|---|---|---|---|---|---|---|---|
| 0.10 | 20% | 5.6% | 1.71% | 0.059% | 9.4e-7 | 5.6e-15 | < 1e-14 |
| 0.25 | 50% | 31.2% | 20.7% | 6.87% | 0.93% | 3.5e-5 | 4.5e-9 |
| 0.30 | 60% | 43.2% | 32.6% | 15.6% | 4.29% | 0.128% | 5.0e-6 |
| 0.40 | 80% | 70.4% | 63.5% | 49.3% | 32.7% | 11.9% | 2.72% |

This ignores stale blocks; a higher stale rate effectively strengthens the attacker [S24].

**Status ladder for wallets:** seen -> preconfirmed (n weak, h_hat high, no conflict) ->
1..K-1 strong confirmations (show r(q, n) for an assumed q) -> final at K (reorgs deeper are
refused by consensus).

## 9. Orphan rate model

For effective propagation delay tau and interval T, Poisson block arrivals give
`stale ~ 1 - exp(-tau / T)` [S11, Fig. 3; S25 for the delay model].

| tau | T = 60 s | T = 30 s | T = 15 s |
|---|---|---|---|
| 1 s | 1.65% | 3.28% | 6.45% |
| 2 s | 3.28% | 6.45% | 12.48% |
| 3 s | 4.88% | 9.52% | 18.13% |
| 6.5 s | 10.27% | 19.48% | 35.17% |

Implied tau from measured stale rates via `tau = -T ln(1 - s)`: Dogecoin (60 s, 0.619%)
0.37 s; Litecoin (150 s, 0.273%) 0.41 s; Ethereum (15 s taken as the midpoint of 10-20 s,
6.8%) 1.06 s; Bitcoin 2016 (600 s, 0.41%) 2.47 s [S24]; Bitcoin 2012 (600 s, 1.69%) 10.2 s,
consistent with Decker-Wattenhofer's 11.37 s [S25]. tau = 6.5 s (the 2012 median) is a
pessimistic bound; Cadre blocks are small (proof plus a few KB of txs), comparable to the
Litecoin/Dogecoin rows, so tau of 1-2 s is the planning range if headers and proofs are
relayed before full tx validation (BIP152 high-bandwidth style [S7]). That gives 3.3-6.5% at
30 s by the model; Gervais' simulation with push relay gives 0.45% at 30 s [S24]. Measure on
testnet before fixing uncle parameters.

## 10. Recommendations for Cadre

1. **Weak ratio.** rho = 1/16 at both 60 s and 30 s. At 30 s that is a weak block every
   1.88 s, comfortably above a 1 s propagation delay, at 6.9-13.3 KB/s per link. 1/32 at 30 s
   (0.94 s spacing) is near tau and makes weak-level forks common; 1/8 at 60 s (7.5 s) is too
   slow to feel instant. For comparison, Ergo's devnet runs 1/60 at 60 s [S30].
2. **Weak-block structure.** Flat, StrongChain-style: every weak block points to the current
   strong tip (plus an optional pointer to the previous weak block for tx-list deltas). No
   subchain selection rule, which avoids subchain selfish mining [S12].
3. **Weak rewards.** Fixed and independent per weak block (w), not a split of a fixed pot, so
   spiteful exclusion gains the excluder nothing [S12]. Strong miner gets a nephew bonus b
   per referenced weak block, and referencing W requires including W's txs, so honoring
   preconfirmations is strictly profitable. Budget: expected extra issuance per strong block
   is `w * (1/rho - 1)` [S12]; fund it by lowering the strong subsidy so the daily emission
   stays about 1%. Weak blocks are not counted in fork choice and are not verified by the
   Solana light client.
4. **30 s target and emission.** emission_ppm = 7 at 60 s gives `1 - (1 - 7e-6)^1440 = 1.003%`
   per day; keeping 7 at 30 s would give `1 - (1 - 7e-6)^2880 = 1.996%`. Halve it (3.5 ppm,
   so the unit needs to become ppb or similar) to keep 1.003% per day.
5. **Uncles.** Depth at most 2 (60 s of wall clock at 30 s, far beyond tau), at most 2 per
   block, uncle reward 1/2 of the subsidy at depth 1 and 1/4 at depth 2, nephew bonus 1/32
   (Ethereum's nephew value [S19]). This is deliberately below Ethereum's 7/8 to 2/8 over
   depth 1-6, because uncle refunds are what lower the selfish-mining threshold (0.25 down to
   0.185 at delta = 0.24 [S26]; 0.163 at uncle reward 4/8 [S27]); that lower rewards reduce
   the effect is my inference from that mechanism, not a measured result. Either count uncles
   in the WTEMA rate (EIP-100 style [S21]) or, simpler, pay uncles from the same per-day
   budget as in item 4 and leave the DAA on main-chain blocks only. Avoid Rootstock's
   outcome (uncle rate 0.5 with uncle-driven difficulty [S40]) by monitoring the uncle rate and
   lengthening T if it exceeds about 10%.
6. **Fork choice.** Keep heaviest chain by summed main-chain work, uncles and weak blocks not
   counted, as Ethereum PoW did [S20]. Counting uncle or weak work would force the Solana
   light client to verify those proofs too (each about 7-10M CU and 34-37 txs per header), and
   ECIP-1029-style uncle weighting was argued to weaken security [S26]. The light client and
   bridge stay unchanged.
7. **Finality.** K = 60 at 30 s keeps 30 min of wall clock and the coinbase maturity. Per
   Rosenfeld, r(0.3, 60) = 5.0e-6 vs r(0.3, 30) = 0.128% [S39], so K = 30 at 30 s would be a
   real weakening. Light-client load doubles either way: 2880 headers/day x 34-37 txs =
   97,920-106,560 Solana txs/day, up from 48,960-53,280 at 60 s. Budget for that before
   switching.
8. **Selfish-mining caveats.** Shorter intervals raise selfish-mining revenue in Gervais' MDP
   (relative revenue 0.33 to 0.42 from 25 min to 0.5 s at alpha = 0.3) [S24]; uncles lower
   the threshold [S26, S27]; weak blocks add reclusive, spiteful and delayed-publication
   strategies [S12]. Mitigations: independent weak rewards, nephew bonus, push relay before
   full validation, uncle depth 2, and a testnet measurement of stale and uncle rates at
   30 s before mainnet.
9. **Order.** Ship step 1 (weak blocks at 60 s, rho = 1/16, 3.75 s spacing) first; it needs no
   change to T, K, emission, or the light client. Step 2 (30 s, K = 60, emission halved,
   uncles depth 2) after measuring tau and the stale rate on testnet.

## Sources

Internal:
- [R1] Cadre header length 256 bytes: `crates/pearl-verify-sbf/src/header.rs` (`LEN = 256`).
- [R2] Plain Pearl proof CPU verify 0.36-1.75 ms (ZK verify 10-15 ms): `research/NOTES-proof.md`.

External:
- [S1] Rizun, "Subchains: A Technique to Scale Bitcoin and Improve the User Experience", Ledger 1 (2016). https://ledger.pitt.edu/ojs/ledger/article/view/40
- [S2] bitcoin-dev thread "Weak block thoughts..." (Andresen, Russell, Maxwell), Sept 2015. https://lists.linuxfoundation.org/pipermail/bitcoin-dev/2015-September/011191.html and https://mirror.b10c.me/lists/bitcoindev/CADJgMzt9xYdaVnvj96YicrcNGj_RWObKTFUsCeWUaP7Apnzwjw@mail.gmail.com
- [S3] awemany, "Weakblocks / Subchains" BU proof of concept, bitcoin-ml, Dec 2017. https://lists.linuxfoundation.org/pipermail/bitcoin-ml/2017-December/000570.html
- [S4] awemany, "Storm: Using weak proof-of-work for instant confirmations on Bitcoin Cash", 30 Aug 2019. https://bitcoincashresearch.org/uploads/short-url/wC3oPnGRujHGwpGNQVXE15oHvza.pdf
- [S5] Keller, Glickenhaus, Bissias, Griffith, "Tailstorm: A Secure and Fair Blockchain for Cash Transactions", AFT 2023. https://arxiv.org/abs/2306.12206
- [S6] Bitcoin Cash Research, "Spearheading the future implementation of Weak Blocks (Intermediate Blocks) for BCH proof of work". https://bitcoincashresearch.org/t/spearheading-the-future-implementation-of-weak-blocks-intermediate-blocks-for-bch-proof-of-work/1333
- [S7] BIP152, Compact Block Relay. https://github.com/bitcoin/bips/blob/master/bip-0152.mediawiki
- [S8] Ozisik et al., "Graphene", SIGCOMM 2019. https://people.cs.umass.edu/~gbiss/graphene.pdf and project page https://cryptoeconomics.cs.umass.edu/graphene
- [S9] Sanders (instagibbs), "Second Look at Weak Blocks", Delving Bitcoin, Apr-Jun 2024. https://delvingbitcoin.org/t/second-look-at-weak-blocks/805
- [S10] Bitcoin Optech Newsletter #299, 24 Apr 2024. https://bitcoinops.org/en/newsletters/2024/04/24/
- [S11] Bissias and Levine, "Bobtail: Improved Blockchain Security with Low-Variance Mining", NDSS 2020. https://www.ndss-symposium.org/wp-content/uploads/2020/02/23095.pdf ; draft https://arxiv.org/abs/1709.08750
- [S12] Szalachowski, Reijsbergen, Homoliak, Sun, "StrongChain: Transparent and Collaborative Proof-of-Work Consensus", USENIX Security 2019. https://www.usenix.org/conference/usenixsecurity19/presentation/szalachowski ; https://arxiv.org/abs/1905.09655
- [S13] Sompolinsky and Zohar, "Accelerating Bitcoin's Transaction Processing. Fast Money Grows on Trees, Not Chains" (GHOST). https://eprint.iacr.org/2013/881
- [S14] Sompolinsky and Zohar, "PHANTOM: A Scalable BlockDAG Protocol". https://eprint.iacr.org/2018/104
- [S15] Sompolinsky and Sutton, "The DAG KNIGHT Protocol: A Parameterless Generalization of Nakamoto Consensus". https://eprint.iacr.org/2022/1494
- [S16] Kaspa, Crescendo hard fork (1 to 10 BPS, emission per second unchanged). https://kaspa.org/?p=51474
- [S17] coderofstuff, "Post-Crescendo mining observations" (fork on 5 May 2025). https://medium.com/@coderofstuff/post-crescendo-mining-observations-50b99f36721a
- [S18] "Kaspa DAGKnight GHOSTDAG upgrade", 14 Sep 2026 (secondary). https://ourcryptotalk.com/news/kaspa-dagknight-ghostdag-upgrade
- [S19] go-ethereum v1.10.26 `consensus/ethash/consensus.go`. https://github.com/ethereum/go-ethereum/blob/v1.10.26/consensus/ethash/consensus.go
- [S20] go-ethereum v1.10.26 `core/blockchain.go` and `core/forkchoice.go`. https://github.com/ethereum/go-ethereum/blob/v1.10.26/core/blockchain.go ; https://github.com/ethereum/go-ethereum/blob/v1.10.26/core/forkchoice.go
- [S21] EIP-100, "Change difficulty adjustment to target mean block time including uncles". https://eips.ethereum.org/EIPS/eip-100
- [S22] Buterin, "More uncle statistics", 25 Sep 2015. https://blog.ethereum.org/2015/09/25/more-uncle-statistics
- [S23] Buterin, "Uncle rate and transaction fee analysis", 31 Oct 2016. https://blog.ethereum.org/2016/10/31/uncle-rate-transaction-fee-analysis
- [S24] Gervais, Karame, Wust, Glykantzis, Ritzdorf, Capkun, "On the Security and Performance of Proof of Work Blockchains", CCS 2016 (Tables 1, 7, 8). https://eprint.iacr.org/2016/555
- [S25] Decker and Wattenhofer, "Information Propagation in the Bitcoin Network", IEEE P2P 2013. https://www.gsd.inesc-id.pt/~ler/docencia/rcs1314/papers/P2P2013_041.pdf
- [S26] Ritz and Zugenmaier, "The Impact of Uncle Rewards on Selfish Mining in Ethereum", 2018. https://arxiv.org/abs/1805.08832
- [S27] Niu and Feng, "Selfish Mining in Ethereum", ICDCS 2019. https://arxiv.org/abs/1901.04620
- [S28] kushti, Ergo sub-blocks paper, 31 Oct 2023. https://github.com/ergoplatform/ergo/blob/a248ccfe49f45e2e88dbd23312a7e811eb4ef58a/papers/subblocks.md
- [S29] Ergo docs, input blocks. https://docs.ergoplatform.com/uses/sidechains/input-blocks/
- [S30] Ergo docs, roadmap (2026 Matrix devnet: 60 s ordering blocks, 60 input blocks each). https://docs.ergoplatform.com/roadmap/
- [S31] Drake, "Based preconfirmations", ethresear.ch, 8 Nov 2023. https://ethresear.ch/t/based-preconfirmations/17353 ; collateral-size notes https://hackmd.io/@Ladislaus/rJfRYJoB0
- [S32] Symbiotic, "Network Highlight: bolt by Chainbound" (live 5 Feb 2025). https://blog.symbiotic.fi/network-highlight-bolt-by-chainbound
- [S33] Primev mev-commit docs, rewards and slashing. https://docs.primev.xyz/v1.2.x/concepts/rewards-and-slashing
- [S34] Cointelegraph, "ETHGas raised $12M as it launches Buterin-proposed gas futures". https://cointelegraph.com/news/ethgas-raised-12m-as-it-launches-buterin-proposed-gas-futures
- [S35] "Commit-Boost: Proposer Platform to Safely Make Commitments", ethresear.ch. https://ethresear.ch/t/commit-boost-proposer-platform-to-safely-make-commitments/20107
- [S36] Taiko docs, based preconfirmation. https://docs.taiko.xyz/taiko-alethia-protocol/protocol-design/based-preconfirmation
- [S37] "Introducing Luban and its Pre-Settlement Layer LingLong", Lido research forum. https://research.lido.fi/t/introducing-luban-and-its-pre-settlement-layer-linglong/10162
- [S38] Jito docs, Low Latency Block Updates (ShredStream). https://docs.jito.wtf/lowlatencytxnfeed/
- [S39] Rosenfeld, "Analysis of hashrate-based double-spending", 2014. https://arxiv.org/abs/1402.2009
- [S40] RSKIP517, "Block time-centric difficulty adjustment with uncle threshold" (draft, 29 May 2025). https://ips.rootstock.io/IPs/RSKIP517.html
- [S42] Gervais, Scaling Bitcoin Milan 2016 talk transcript. https://scalingbitcoin.org/transcript/milan2016/on-the-security-and-performance-of-proof-of-work-blockchains
