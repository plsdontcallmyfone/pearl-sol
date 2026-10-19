# Agent-economy tokenomics: what makes value reach the token, and what Cadre should change (2026-10-18)

Scope: which token mechanics made an agent or compute economy accrue real value to its
token in 2025-2026, and what CADRE should add, change or avoid. Builds on
`research/narratives/SYNTHESIS.md`, `CRYPTO-COMPUTE.md` and `FLOWS-AND-PUMPS.md` and does
not repeat their tables; where a figure below already appears there it is cited as such.
Cadre rules are from `docs/SPEC.md` 3.4, 4.1, 4.4, 4.5, 5.1 (as of commit 50ffee5).

How the numbers were gathered:
- Prices: CoinGecko free API `/coins/markets`, `last_updated` 2026-10-19 02:42 UTC
  (2026-10-18 evening local). "200d" stands in for "about six months".
- Fees and holder revenue: DefiLlama free API `api.llama.fi/summary/fees/<slug>?dataType=`
  `dailyFees` / `dailyHoldersRevenue`, pulled 2026-10-18. October 2026 months are partial (17-18 days).
- Everything marked **(computed)** is my arithmetic on the cited inputs. Nothing is modelled
  or filled in; where a figure could not be found it says so.
- Firecrawl was down; pages were read with WebFetch. Several sources are third-party
  analysts (ownyourmind.ai, Pine Analytics, chainward.ai); they are named so they can be weighed.

## 1. The short answer

Value reached the token in 2026 only when **all three** held:
1. **Payers outside the incentive loop.** Revenue that exists because emissions or prize pools
   pay for it disappears with them (Virtuals ACP fees -98.6% the month the subsidy ended;
   Morpheus served 63.2B tokens of compute of which 48,138 were paid).
2. **A program rule that turns each payment into token demand or locked supply**, readable on
   chain, not a team decision (Hyperliquid 97-99% of fees; Venice 5% of every credit purchase;
   Akash BME burns on every lease). Discretionary or webpage policies did not hold.
3. **Supply issuance smaller than that demand, and shrinking.** Venice cut emissions 86% from
   launch while its buyback line grew 13x; Render and Bittensor kept emissions far above
   burns/revenue and tracked beta or fell.

Holding the token for a **consumptive claim** (Venice DIEM: $1/day of API credit) beat a pure
fee share: Venice's buyback covered only about 8-22% of its emissions in 2026, yet VVV is
+1,504% over 1y, because most of its token demand is VVV locked to mint credit.

For Cadre the most important finding is internal (section 3): **under the current rules the
mining pool is already a soft burn.** Tokens that enter it can leave only as a rebate of at
most `alpha x P` on paid work, so the "reserve vs burn" choice is really "how big a pool do
paid jobs need before more tokens are dead weight".

## 2. Case studies

### 2.1 Venice (VVV / DIEM): the one that worked

Price: $24.57, mcap $1.19B, FDV $1.99B, ATH $34.53 (2026-09-21), -28.8% from ATH, 200d
+243.9%, 1y +1,504.2% (CoinGecko).

Mechanics (Venice blog https://venice.ai/blog/tokenomics-update-credit-burns-and-diem-supply-expansion,
2026-07-17, and ownyourmind.ai https://ownyourmind.ai/projects/venice/, read 2026-10-18):
- **Credit burn:** since 2026-07-17, $5 of every $100 of API credit purchases buys and burns
  VVV. Subscription burns ($2 Pro, $5 Pro+, $10 Max per new subscription) since 2026-04-26.
- **Emissions:** 14M -> 10M (2025-08-20) -> 8M -> 6M -> 5M -> 4M -> 3M (2026-07-01) ->
  2.5M (2026-09-01) -> 2M VVV/yr (2026-10-01), 86% below launch. Stakers receive all
  emissions; sVVV locked behind DIEM keeps 80% of yield, 20% goes to Venice.
- **DIEM:** lock sVVV to mint DIEM; 1 DIEM = $1/day of API credit, refreshing daily. Mint rate
  (sVVV per DIEM) rises as supply nears a target: launch base rate 90, adjustment power 2,
  target 38,000 (https://veniceai.featurebase.app/help/articles/5517796-what-is-the-mint-rate-and-where-can-i-check);
  target raised in four 500-DIEM steps to 40,000 by 2026-09-14 "spread across six weeks on
  purpose".
- State on 2026-10-18 (ownyourmind tracker): DIEM supply 36,729 (91.7% of target), 8.26M VVV
  escrowed behind DIEM (about 10% of effective supply), marginal mint rate 438 sVVV per DIEM,
  34.4M VVV staked (29.9%), 7-day unstake cooldown.
- Buyback flow (DefiLlama `venice` holders revenue = the buyback line, $k/month): 2025-12 64,
  2026-03 112, 2026-05 236, 2026-07 448, 2026-08 697, 2026-09 833, 2026-10 (18 days) 768.
  30d $963,721.
- Buyback coverage of emissions (ownyourmind): 2.7-10.7% from buybacks alone in 2026 months,
  15.0% / 21.8% / 17.7% (Jul / Aug / Sep) including per-event burns.

Sizing, **(computed)** from the figures above at $24.57:
- Credit capacity outstanding: 36,729 DIEM = $36,729/day = about **$13.4M/yr** of API credit.
  Against third-party revenue estimates of about $60M ARR (Delphi, May 2026, via ownyourmind)
  to the founder's $100M+ run-rate claim (CRYPTO-COMPUTE), that is **roughly 13-22% of revenue**.
- Lock per $1/day of credit: average 225 VVV (about $5,526), so credit worth about **6.6% a year**
  of the locked value; at the marginal mint rate 438 sVVV (about $10,762), about **3.4% a year**.
  On top, the locker keeps 80% of staking yield.
- Annualised buyback (30d x 365/30) about $11.7M = **about 1.0% of mcap a year**.

What held and why: the token is the unit for a product people pay for anyway; the credit
supply is capped and stepped slowly; issuance was cut every month or two. Weaknesses on the
record: every emissions cut and the DIEM launch were unilateral team decisions; a 4-of-6 Safe
holds mint and upgrade authority with no timelock; top-10 holders 63.37% raw (ownyourmind).

### 2.2 Hyperliquid (HYPE): buyback held, labelled burned later

Price $79.19, mcap $17.62B, FDV $75.67B, ATH $97.96 (2026-09-23), -19.2%, 200d +118.9%.
- About 97-99% of fees go to the Assistance Fund, which buys HYPE on the open market
  (VanEck https://www.vaneck.com/es/en/blog/digital-assets/exploring-hyperliquid-redefining-derivatives-trading/;
  AMINA https://aminagroup.com/research/hyperliquid-hype-etf-buyback-staking-yield-institutional-access-2026/).
- The fund **held** what it bought for over a year. In December 2025 the Foundation proposed
  recognising the fund's roughly 37M HYPE (13% of the then-circulating 270M) as burned; the
  tokens sit at a system address with no key and the vote was "binding social consensus to
  never authorize a protocol upgrade to access this address"
  (The Block https://www.theblock.co/post/383091/hyper-foundation-proposes-removing-1-billion-hype-tokens-from-circulating-supply-by-zeroing-out-hyperliquids-assistance-fund).
  The reported tally (85% for) comes from a Foundation X mirror only; unconfirmed.
- DefiLlama holders revenue 30d $68.4M; monthly 2025-09 $85.2M, 2026-04 $42.4M, 2026-09 $74.2M.
  Annualised **about 4.7% of mcap** (computed).
- Operator bonding: HIP-3 market deployers must stake 500,000 HYPE, slashable by stake-weighted
  validator vote, slashed HYPE is burned, 7-day unbonding during which it stays slashable
  (Nansen https://nansen.ai/post/what-is-hip-3-hyperliquid; Blockworks https://blockworks-research.beehiiv.com/p/hip-3-bottleneck).
- Staking fee discounts 5% (10 HYPE) to 40% (500,000 HYPE), multiplied on volume tiers
  (secondary sources citing the docs; e.g. https://www.dwellir.com/blog/hyperliquid-fees-explained).

Lesson: "buy and hold in a fund the team cannot touch" and "burn" were treated the same by the
market once the fund was credibly locked. The credibility came from a key-less address plus a
public commitment, not from the word burn.

### 2.3 pump.fun (PUMP): big buyback, falling revenue, no trend

Price $0.00436, mcap $2.01B, -50.5% from ATH (2025-09-14), 200d +152.5%, 1y +9.7%.
- 100% of revenue to buybacks from mid-July 2025; on 2026-04-28 it burned the stockpile (about
  $370M, about 36% of circulating) and moved to 50% of net revenue for 12 months, the other 50%
  to growth (CoinDesk https://www.coindesk.com/markets/2026/04/29/pump-fun-burns-36-of-pump-supply-in-usd370-million-wipe-locks-50-revenue-into-ongoing-buybacks).
  Gross revenue $971.37M in 2025, annualising to about $320M in 2026 (same source).
- DefiLlama holders revenue: 2025-09 $42.8M, 2026-01 $31.8M, 2026-06 $12.2M, 2026-09 $23.6M;
  30d $30.4M, annualised **about 18% of mcap** (computed), the highest ratio in this set.
- Revenue figures were cross-checked against on-chain buys and matched DefiLlama within about
  0.05% (4pillars https://research.4pillars.io/en/research/debunking-pumpfun-revenue-manipulation-fud).

Lesson: buyback yield alone does not make a trend when revenue is flat to falling and a large
insider float sells into it. The buy was a floor (FLOWS-AND-PUMPS has the same verdict).
Also relevant: pump.fun itself, like Hyperliquid, accumulated first and burned later.

Locked liquidity on our own launch: graduation to PumpSwap has locked the pool's liquidity
permanently; since BOOST (The Block, 2026-07-29,
https://theblock.co/post/409815/pump-fun-token-graduation-rate-jumps-boost-changes-launch-incentives)
the roughly 20% of migration liquidity that used to sit dead in the pool is instead spent on
automatic buys in the first five minutes after migration, and every bought token is burned.
So CADRE gets locked liquidity and a post-graduation burn for free; no extra lock is needed.

### 2.4 Akash (AKT): burn-mint with a USD credit

Price $0.715, mcap $214M, -91.1% from 2021 ATH, 200d +46.0%, 1y +8.6%.
- BME live 2026-03-23. Tenants send AKT (or pay by card; the console buys AKT) into a vault and
  receive ACT, a **non-transferable**, soulbound, non-expiring ~$1 compute credit usable only
  for leases. At settlement ACT is burned and providers are paid AKT at the settlement price,
  first from the vault's "remint credits", minting only the shortfall. Circuit breaker on
  vault collateral ratio (warn 0.95, halt new mints 0.90), 30-minute TWAP oracle
  (AEP-76 https://akash.network/roadmap/aep-76/).
- Stated reason: adding stablecoin payments (AEP-23) grew revenue but cut AKT demand, and the
  stablecoin fee share paid to stakers was too small to matter.
- DefiLlama fees ($k/month): 2026-02 64, 2026-05 164, 2026-07 231, 2026-09 288 (4.5x from
  February). Burns still below inflation (CRYPTO-COMPUTE).

Lesson: a non-transferable credit that every user must acquire with the token is the cleanest
"usage becomes token demand" design for a compute market, and it cannot be farmed on a secondary
market. Its weakness is size: $3-4M/yr of spend does not offset inflation.

### 2.5 Render (RENDER): burn-mint where emissions win

Price $1.82, mcap $944M, -86.6% from ATH, 200d +1.5%, 1y -26.3%.
- Job payers burn RENDER at the USD value of the job; node operators are paid from a scheduled
  emission pool (https://know.rendernetwork.com/basics).
- DefiLlama holders revenue (= burns): 2026-05 $87k, 2026-09 $221k, 2026-10 (18 days) $423k;
  30d $423k, annualised **about 0.55% of mcap** (computed). Monthly emissions about 492k RENDER
  vs May burns 45k RENDER (CRYPTO-COMPUTE).

Lesson: a burn on usage that is an order of magnitude smaller than scheduled issuance is a
narrative, not a sink.

### 2.6 Bittensor (TAO) and dTAO subnet tokens: revenue at 9-12% of emissions

TAO $272, mcap $3.09B, -64.1% from 2024 ATH, 200d -13.5%, 1y -32.9%. Chutes SN64 alpha
$18.16, mcap $113M, -82.6% from ATH (2025-06-10), 1y -44.0%.
- First halving 2025-12: 7,200 -> 3,600 TAO/day; next at 15.75M issued, estimated around
  2029 (https://guides.learnbittensor.org/concepts/halving). Each subnet alpha has its own 21M
  cap and halvings. Subnet emission shares were set by net TAO staking flows (Taoflow, from
  November 2025); one June 2026 guide reports a return to price-based shares (unconfirmed).
- 24 revenue-generating subnets at an estimated $28-35M annualised external revenue, about
  **9-12% of over $300M/yr of emissions**; 14 of the 24 use revenue to buy back their own alpha
  (SubConnect index via cryptobriefing, 2026-09-21,
  https://cryptobriefing.com/bittensor-subnets-annualized-revenue/). Top: Lium SN51 $8-10M,
  Targon SN4 $5.5-6M, Chutes SN64 $4-5M. No source measures the buybacks' price effect.
- Chutes took about 14.4% of emissions (about $52M/yr) against $1.3-2.4M of external revenue
  in March: 22-40:1 (Pine Analytics, 2026-03-23, https://pineanalytics.substack.com/p/the-bear-case-for-bittensor-tao).
  DefiLlama `chutes` fees ran $376-495k/month Jan-Aug 2026.
- Pine's structural point: the demand signal used to route emissions (staking flows or alpha
  price) is not the demand for the service, which is off chain and unmeasured.

Lesson: routing emissions by token price or stake flow pays whoever is best at attracting
stake, not whoever has customers. Cadre's routing (subsidy only on paid jobs, capped by the fee
the buyer paid) is the opposite and is the right base.

### 2.7 Virtuals (VIRTUAL, agent tokens, ACP): subsidy in, subsidy out

VIRTUAL $0.663, mcap $436M, -86.9% from ATH, 1y -12.0%.
- Revenue Network: five weekly epochs 2026-02-11 to 2026-03-23 paid $1,022,634 (half USDC, half
  VIRTUAL used to buy the winning agents' tokens); prize pools exceeded platform fees in every
  epoch; top 10 agents took 63-86% of each epoch. Virtuals flagged "actors attempting to game
  the system through artificial service activity" on 2026-02-23. ACP platform fees: January
  about $119k, February $315k, March $345k, April $4,935 (-98.6%), May $28, June-September
  under $6/month. On ACP v2 on Base (June-August), $1,618 of $2,527 of stablecoin inflows came
  from two wallets paying each other (chainward.ai https://chainward.ai/decodes/virtuals-top-dog).
- DefiLlama `virtuals-protocol` fees 30d $396k (mostly launchpad trading tax), holders revenue 0.

Lesson: **an incentive per dollar of activity that exceeds the cost of faking that dollar buys
fake activity, exactly up to the incentive.** Section 3.3 computes Cadre's cost of faking.

### 2.8 ai16z / elizaOS: agent-held treasury, supply expansion, zero

ai16z $0.000372, -100% from ATH, 1y -99.3%. Migration 1:6 to ELIZAOS (November 2025) with
total supply expanding toward 11B (https://docs.elizaos.ai/tokenomics.md). A class action filed
2026-04-22; on 2026-08-04 the founder declared the token dead after a settlement transferred the
remaining treasury to a group of holders, ruled out buybacks or a replacement token
(coin360 https://coin360.com/news/elizaos-token-dead-treasury-settlement; single outlet).

Lesson: a treasury is a liability that can be taken; a token whose value is "the DAO's
holdings plus a framework" has no flow to protect it. The framework kept going; the token did not.

### 2.9 Olas (OLAS): staking paid for activity

OLAS $0.0532, mcap $15.9M, -99.4% from ATH, 1y -66.1%. Operators stake OLAS and earn
emissions when an on-chain activity checker sees their agent meet a target number of actions
("Proof of Active Agent"); developers earn emissions when their registered code runs; bonding
sells discounted OLAS for LP tokens to build protocol-owned liquidity
(https://immunefi.com/bug-bounty/autonolas/information/; https://coinranking.com/coin/e793ximHn+autonolas-olas/about).
I found no 2026 revenue, emission-rate or PoL figure; DefiLlama has no `olas` fees listing.

Lesson (mechanism, not measured): paying emissions for a count of agent actions rewards
actions, which are free to produce; the price outcome is consistent with that.

### 2.10 Morpheus (MOR): stake-for-compute with no fee

MOR $0.855, mcap $7.7M, -99.4% from ATH, 1y -72.5%. Users stake MOR to draw compute sessions;
providers are paid from emissions under a stipend formula. Since launch about **63.2B tokens of
inference were served, of which 48,138 were paid**; paying a provider buys no more session time
than staking, so nobody pays; the buyback takes about 3.4% of daily issuance (50% burned, 50%
locked 16 years) (ownyourmind newsletter, 2026-08-26,
https://buttondown.com/ownyourmind/archive/we-built-morpheus-a-balance-sheet/).

Lesson: **stake-for-credit with no paid path behind it is a free-compute faucet.** Credit must
be a slice of a paid market, not a replacement for it.

### 2.11 io.net (IO), Fetch/ASI (FET), Nous, Pearl (PRL)

- **io.net:** Incentive Dynamic Engine (June 2026) emits only what supplier rewards need at the
  live price and sends at least 50% of surplus revenue to buyback and burn; target at least 12M
  IO burned in year one (press release via
  https://lifestyle.kbew98country.com/story/271004/io-net-to-burn-up-to-12m-tokens-as-network-closes-8m-deal-and-hits-4-billion-daily-ai-tokens/).
  An independent read says the burn is live but its funding is not clearly tied to revenue
  (https://buttondown.com/ownyourmind/archive/what-ionets-new-token-burn-actually-runs-on/).
  IO about 96% below peak (https://solanacompass.com/projects/ionet); flat YTD (CRYPTO-COMPUTE).
- **Fetch / ASI:** FET $0.189, mcap $438M, -94.5% from ATH, 200d -21.8%. Ocean left the
  alliance in October 2025 amid disputes (https://www.datawallet.com/zh/%E9%9A%90%E8%94%BD%E6%80%A7/artificial-superintelligence-alliance-asi-explained);
  I found no reliable Agentverse revenue or value-accrual figure. A payment/staking/governance
  token with no measured sink.
- **Nous:** no token. Psyche runs on Solana; Polymarket priced a token by 2026-12-31 at about 3%
  (https://polymarket.com/event/will-nous-research-launch-a-token-by). Nothing to copy yet.
- **Pearl (PRL):** $1.93, mcap $679M, FDV $4.08B, ATH $2.10 (2026-10-14), -7.9%. Emission from
  the node source (`research/pearlrepo/node/blockchain/validate.go`, `CalcBlockSubsidy`):
  `subsidy(h) = S x E / ((h + E)(h - 1 + E))`, S = 2.1B PRL, E = 4 years of 194-second blocks
  = 650,226. This telescopes to **cumulative mined = S x h / (h + E)** (computed): half the supply
  at four years, hyperbolic tail, no halvings, and **paid to every block whether or not anyone
  bought the work**. At h = 122,062 it gives 331.9M PRL (computed), matching the 331.9M Datawallet
  reported for 2026-10-02 (CRYPTO-COMPUTE). The Together AI Gemma-4-31B-it-pearl endpoint is
  "discounted by over 25%", the discount "offset by the expected future value of Pearl
  emissions" (https://www.together.ai/blog/together-ai-partners-with-pearl-research-labs,
  2026-05-15). DefiLlama `pearl` fees: $823 over 30d, $2,923 all time.

## 3. What Cadre's own rules already imply

### 3.1 The mining pool is a soft burn

From SPEC 3.4 and 4.5: a job's P goes to the mining pool at post time; the job's total subsidy
is capped at `alpha x P` (alpha 0.8); per block `subsidy = min(pool x emission_ppm / 1e6,
cap_remaining)` with emission_ppm 7; fallback blocks earn 0; nothing is minted. So
**(computed from the rules)**:
- Every paid job leaves at least `(1 - alpha) x P` = 0.2 P in the pool for good. The pool can
  never be drained by subsidies, and paid jobs alone fund their own subsidy cap.
- Tokens sent into the pool from outside (dev buy, creator-fee buybacks, storefront buybacks
  via `fund_mining`) can leave **only** as part of some paid job's `alpha x P` rebate. With no
  paid work, they never leave.
- What the outside tokens buy is **speed**: with a small pool, `pool x 7 ppm` binds and a job
  with a short block budget never collects its cap. For a segment's full cap to be paid in one
  block (SPEC default: one block per segment), the pool must be at least
  `cap / 7e-6 = about 142,857 x cap`. Below that, miners get less than `alpha x P` and the
  inference rebate is smaller than advertised.
- So the realised rebate is `min(alpha, pool x 7e-6 x blocks / P)`. Once the pool is large
  enough for the trailing job flow, extra pool tokens raise nothing; they are supply removed
  from circulation with a governance overhang (they could be released only by a rule change to
  alpha or emission_ppm).

This is the Hyperliquid situation before December 2025: bought tokens held where only a rule
change could spend them. The market priced that as burned once it was credible.

### 3.2 The reserve's exit is discretionary

`fund_mining` (token/programs/reserve) is callable only by the admin or the `funder` key.
Buying is a program rule (good, per FLOWS-AND-PUMPS); spending is a person. Section 5 item 1
fixes this.

### 3.3 The cost of faking demand on Cadre

A self-dealer buys a job through the storefront and mines or serves it themselves. Per 1 SOL of
`sol_in` on the curve (computed from SPEC 5.1 defaults: buyback 10%, pump curve fee 1.25%, P
20%, alpha 0.8; ignoring overhead, price impact and block fees): 0.10 goes to the buyback; 0.9
buys 0.889 of tokens; W 0.711 comes back as worker pay; at most 0.142 (`0.8 x P`) comes back as
subsidy, and only if the pool is large enough; selling it back pays another 1.25%. Recovered at
most about 0.843, so **faking a dollar of demand costs at least about 15.7%**, more when the pool
is small (less subsidy) or on overhead. This is the number every incentive has to stay under:
any reward per dollar of storefront volume above roughly 15% pays for itself in fake volume, as
Virtuals' epochs did.

### 3.4 Token sinks per order today

Per order (computed, same defaults): 10% of `sol_in` buys CADRE into the reserve after the
order finalises, and 0.2 x P (about 3.6% of the tokens bought) stays in the pool permanently.
Both are soft burns under 3.1. Plus pump's creator fee (0.30% on the curve) feeds the reserve
buyback. Compare Venice's 5% of credit purchases and pump.fun's 50% of net revenue: Cadre's
10% of gross is high for a compute price that must compete with centralised open-model hosts.

## 4. Mechanisms: evidence summary

| Mechanism | Worked when | Failed when | Evidence |
|---|---|---|---|
| Buyback from fees | revenue organic and growing, rule on chain, bought tokens credibly locked | revenue flat/falling, insider float selling into it | HYPE, VVV vs PUMP, LIT, ASTER (FLOWS) |
| Burn on usage (BME) | burn comparable to issuance | burn about 10x below issuance | AKT modest; RENDER 0.55% of mcap/yr |
| Buy and hold in a locked fund | address keyless, public commitment | discretionary spending | HYPE AF; MOR 50/50 burn/lock (tiny) |
| Stake for credit | credit is a slice of a paid market, supply stepped slowly, mint cost rises with supply | credit replaces paying (no fee) | DIEM vs MOR (48,138 paid of 63.2B) |
| Non-transferable credit bought with the token | every user must buy the token | spend too small vs inflation | Akash ACT |
| Operator bond | slashable, burned, sized to what the operator can break | n/a in sample | HIP-3 500k HYPE |
| Fee discounts for stakers | real fee volume | n/a | HYPE 5-40% |
| Emissions routed by stake/price | never shown to track customers | incentive budget 8-11x revenue | Bittensor, Chutes |
| Emissions for activity counts | n/a | activity is free to fake | Olas |
| Incentive epochs | n/a | prize > fee, fees -98.6% after | Virtuals ACP |
| Agent/DAO treasury | n/a | taken by settlement, supply x10 | elizaOS |
| Emissions-funded discount | narrative run | demand not measurable ($823/30d fees) | Pearl/Together |

Wash activity in agent payments: TRM Labs (2026-09-09,
https://www.trmlabs.com/trm-tech-blog/whos-actually-paying-measuring-ai-agent-payments-onchain)
took $52.7M across 198.9M x402 settlements, removed self-payment, bulk flows from one or two
payers and sellers with fewer than ten distinct buyers (leaving $25.62M), and found 0.6-7.5% of
that agentic, about $5,000-11,000 a month. Artemis classed 63.94% of x402 transactions as gamed
over 2025-11-09 to 2026-02-07 (peak day 90.21%; via 4pillars
https://research.4pillars.io/en/research/x402-on-polygon-real-or-gamed). Any "agent payments"
number Cadre publishes must pass the same screens or it will be read as wash.

## 5. Ranked changes and additions for Cadre

Ordered by value per effort. "Editable" = admin-editable parameter (owner rule: every fee,
split and limit stays editable).

**1. Pool sufficiency rule: buyback to the reserve up to a cap, burn above it.**
- Mechanism: the storefront crank sends bought CADRE to the reserve while the reserve's token
  balance is below `bb_reserve_cap`, and burns the rest (`buyback_dest` 0 and 1 already exist; this
  makes the choice per buy, by rule). Optionally the cap is expressed as N days of trailing
  segment caps (3.1 gives the size needed to pay caps in their block budget).
- Evidence: 3.1 (pool tokens beyond what paid jobs need are dead supply with an overhang);
  Hyperliquid and pump.fun both accumulated then burned once accumulation stopped serving a purpose.
- Value to CADRE: the reserve keeps the inference rebate at full `alpha x P` (cheaper inference,
  more demand), and every token beyond that is a visible, irreversible supply cut instead of an
  overhang.
- Effort: low (storefront crank branch + one config field + event field).
- Editable: `bb_reserve_cap`, destination override, existing buyback knobs.

**2. Make the reserve's exit a rule, not a key.**
- Mechanism: `fund_mining` becomes permissionless, rate limited (`max_fund_per_call`,
  `min_interval_secs`), always to the fixed `mining_pool`; the admin keeps the limits, not the
  trigger. Publish the 3.1 property ("pool tokens can leave only as at most alpha x P of paid
  work") on the explorer next to the pool balance.
- Evidence: FLOWS-AND-PUMPS (discretionary buybacks did not hold); Hyperliquid's AF became
  "burned" once access was given up by a public commitment.
- Value: holders can price reserve and pool as locked supply. Effort: low.
- Editable: per-call cap, interval, an optional funder allowlist kept as an emergency switch.

**3. Publish value accrual where buyers look: DefiLlama adapter, circulating supply endpoint,
screened revenue.**
- Mechanism: (a) a DefiLlama fees adapter: storefront `sol_in` as fees, released buyback shares
  (`BuybackAccrued`) as holders revenue, burns separate; (b) a circulating-supply endpoint that
  excludes the reserve ATA, bridge-vault tokens backing the chain's mining pool and escrow, and
  burned supply; (c) the explorer reports revenue after TRM's screens (no self-pay, no single-
  payer bulk, sellers with at least 10 distinct buyers) and unique payers, never tx counts.
- Evidence: Venice's rise is legible as one rising DefiLlama line; x402 counts are read as wash
  (TRM, Artemis).
- Value: turns the "every block was paid for" claim into numbers third parties reproduce.
- Effort: low. Editable: n/a (reporting).

**4. Server and evaluator bond in CADRE, slashed to burn.**
- Mechanism: an inference ticket server (SPEC 4.5) or evaluator posts a CADRE bond on the chain;
  a server may hold open tickets whose total W is at most `bond / bond_ratio`; a proven wrong
  result (C mismatch, or evaluator fraud shown by the data availability path) slashes the bond,
  slashed tokens burned (or sent to the pool, which 3.1 makes equivalent). Unbonding delay at
  least the longest ticket deadline plus K.
- Evidence: HIP-3's 500k HYPE slashable, burned; Olas shows the bond must secure correctness,
  not pay for activity. Coordinate with research/agents/REPUTATION.md (opening evaluators).
- Value: locked supply that grows with paid job flow, plus a real cost to cheating that makes
  opening the evaluator set safe. Effort: medium (chain tx kinds + state).
- Editable: `bond_ratio`, minimum bond, unbonding delay, slash share.

**5. Stake-for-credit, sized as a slice of paid inference (only after paid inference exists).**
- Mechanism: lock CADRE on Solana to mint a **non-transferable** daily inference credit (in
  lamports of storefront spend) redeemable only at the inference gateway. Mint cost rises
  with outstanding credit (Venice-style curve). Credit capacity is capped at a share of
  trailing paid spend: `total daily credit <= credit_share x (trailing 30d paid inference SOL / 30)`,
  default **10%**, never above **20%** without new evidence. Credits are funded from the
  released buyback share (SOL), so a redeemed credit is an ordinary paid order (W to the server,
  P to the pool, the light-client path finalises it), not an emissions-funded job. Credit
  orders pay no buyback share and earn no incentives. Stepped capacity increases (Venice moved
  its target in 500-DIEM steps two weeks apart).
- Sizing evidence: Venice runs credit at roughly 13-22% of estimated revenue and charges lockers
  about 3.4-6.6% a year in credit per locked value at today's price (computed, 2.1); Morpheus
  shows what happens with no paid path (48,138 paid of 63.2B served); Akash shows non-
  transferable credit cannot leak into a secondary market for free compute.
- Value: inference demand becomes demand to buy and lock CADRE, which in Venice's case mattered
  more than the burn. Effort: medium-high (Solana staking program, gateway metering).
- Editable: `credit_share`, mint curve base/power/target, cooldown, minimum lock.
- Trade-off the owner should decide: credits funded from the buyback share mean less buyback.
  My view: at launch keep the whole 10% as buyback; once paid inference has a few months of
  history, move part of it to credit funding, because locked supply held up Venice better
  than its burn did.

**6. Hold the buyback share at 10% of gross for now; do not raise it.**
- Evidence: Venice 5% of credit purchases; Cadre already removes about 10% plus 0.2 P per order
  (3.4). Inference has to be cheaper than centralised open-model hosts to get organic buyers;
  pump.fun shows a large buyback on flat revenue does not trend. If the price comparison against
  centralised hosts is lost, lower it (editable) rather than chase it up.
- Effort: none.

**7. Agent Credit liens sized from proven income, never from CADRE price.**
- Mechanism: lenders size a lien's `cap` and `route_bps` from trailing matured coinbase receipts
  and W credits (SPEC 3.4 R1, 4.4), in CADRE units, with a haircut; no LTV on the token's market
  price, no auto-liquidation. Already the design; make it the documented policy in ac_pearl.
- Evidence: SYNTHESIS item 6 (uncollateralised lending is tiny; borrowed capacity punished);
  token-price collateral is the reflexive loop that unwinds on drawdowns.
- Effort: low (policy and defaults). Editable: haircut, max route_bps, max cap multiple.

**8. Staker fee discount at the storefront (later, with item 5's program).**
- Mechanism: wallets with locked CADRE pay a smaller buyback markup (tiers like Hyperliquid's
  5-40%). Low value until volume exists; reuses item 5's lock. Effort: low once 5 exists.

## 6. Opinions the request asked for

**Buyback destination: reserve, with a cap, then burn.** Under the current rules the reserve is
not "emissions waiting to dump": it can only rebate paid work, at most `alpha x P`, and that is
also the only thing that makes Cadre inference cheaper than a plain host. So the reserve should
be funded until the pool can pay full caps for the trailing job flow (3.1), and burned above
that. Pure burn from day one would leave miners collecting a fraction of `alpha x P` while the
pool is small, weakening the product the token depends on; pure reserve forever builds an
overhang the market will discount. The owner's 2026-10-18 decision (reserve) stays correct
until the cap is reached; item 1 adds the cap.

**Stake-for-credit sizing: 10% of trailing paid inference spend, funded from revenue,
non-transferable, rising mint cost, stepped increases.** Never let credit exceed what paying
users spend (Morpheus), never fund it from the pool or reserve (that is the Chutes/Pearl
emissions-for-discount pattern), and do not ship it before paid inference exists (SYNTHESIS
item 4 says the same).

## 7. What not to do

- Do not add inflation, halvings or a block reward independent of paid work. Pearl's hyperbolic
  emission is paid regardless of demand and its fees are $823 a month; Bittensor pays 8-11x its
  revenue. Cadre's fixed supply plus `alpha x P` cap is its edge.
- Do not run incentive epochs, leaderboard prizes, points or airdrops worth more than about 15%
  of the volume they reward (3.3). Virtuals paid more than its fees and kept nothing.
- Do not pay staking rewards for activity counts (Olas) or route anything by stake flow or token
  price (dTAO).
- Do not sell discounted inference funded by the reserve or "expected future token value"
  (Pearl/Together, Chutes 22-40:1). The rebate must stay bounded by what the buyer paid.
- Do not make credit transferable at launch, and do not let credit replace a paid path (Morpheus).
- Do not create an agent- or DAO-held treasury of CADRE, or expand supply (elizaOS).
- Do not escalate the buyback share to chase price (Aster 40-80-99%), or publish a buyback that
  is not a program rule (StonkFun).
- Do not report raw x402 or tx counts as demand (TRM 0.6-7.5% agentic; Artemis 63.94% gamed).
- Do not add an extra liquidity lock: PumpSwap graduation already locks liquidity and BOOST
  burns what used to be dead liquidity.

## 8. Risks

- **Reflexivity of `alpha x P` in token terms.** The rebate is paid in CADRE: a falling price
  shrinks the miners' rebate in USD and raises the effective inference price, which can cut
  demand. Mitigation: price jobs in SOL at the storefront (already) and keep the rebate a bonus,
  not the margin a server needs to break even.
- **Subsidised fake usage.** Under the rules a self-dealer loses at least about 15.7% per round
  trip, so the protocol itself is not farmable; the risk is any future incentive that pays more
  than that (3.3).
- **Wash in agent payments.** Third parties will apply TRM-style screens; publish screened numbers
  first (item 3).
- **Overhang perception.** Reserve and pool balances look like future sell pressure until the
  3.1 property is shown and the exit is rule-based (items 1, 2).
- **Admin keys.** Every parameter is admin-editable by owner rule; Venice's record shows the
  market tolerates that while changes go one way (issuance down) and are announced ahead. Emit
  every change as an event (the storefront already does) and stage them.
- **Framing.** No geofences, no jurisdiction gates (owner rule). Describe CADRE by what the
  programs do: the buyback is a rule inside `buy_job`, the credit is consumptive compute, there
  is no insider allocation and no promise of returns. That keeps the story about paid work,
  which is also the only story the data says holds.

## Sources (beyond those inline)

- CoinGecko markets API https://api.coingecko.com/api/v3/coins/markets (pulled 2026-10-18).
- DefiLlama fees API https://api.llama.fi/summary/fees/{venice,hyperliquid,pump.fun,virtuals-protocol,chutes,akash,render-network-bme,pearl} (pulled 2026-10-18).
- Pearl node source, `research/pearlrepo` @ commit dated 2026-10-13 (`node/blockchain/validate.go`, `node/chaincfg/params.go`).
- research/narratives/CRYPTO-COMPUTE.md, FLOWS-AND-PUMPS.md, SYNTHESIS.md (2026-10-17/18).
