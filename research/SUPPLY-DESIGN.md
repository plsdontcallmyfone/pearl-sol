# CADRE supply design: stay fixed, switch to a levpad-style split mint, or a hybrid? (2026-10-21)

Decision support for the owner. No code was changed. Scope: whether CADRE should leave its
fixed 1B pump.fun supply for the levpad (TILT) shape: 600M launched, 400M mintable only by an
immutable program as mining rewards proven on Solana, minted in proportion to paid AI revenue,
under a hard daily ceiling.

Rules followed: every figure is cited, or computed from cited inputs with the arithmetic shown
(marked **(computed)**). Inputs chosen to span a range (CADRE price, volume) are labelled
**input** and are not forecasts. Nothing here predicts a price.

## Executive summary

**Recommendation: stay with the fixed 1B pump.fun supply.** Do not switch to 600M + 400M, and do
not switch to the 800M + 200M variant either.

The owner set the bar on 2026-10-21: switch only if the evidence shows the current design clearly
fails miners or holders. It does not:
- **Miners.** At the decided price, a miner gets 93.6% of what the buyer's job spend buys. One A40
  breaks even at 53.4% utilization (section 4.3).
- **Holders.** Holders face no dilution, and about 13.4-13.6% of every SOL ordered leaves the
  float (section 6).
- **What does fail.** The price is 19-469x the market for the same models. Supply design does not
  cause that, and no farm-safe mint can fix it (sections 4-5).

**Minimum change that is enough (section 6.1).** None to supply, code or venue. Two owner
decisions already on the MAINNET.md section 11 list cover it:
1. Pick the dev buy at 1 to 3.12 SOL (34.2M to 100M into the reserve). 100M already keeps every
   miner rebate full for about ten fully busy A40s at the curve's graduation price.
2. Keep `bb_reserve_cap` at 100M. Raising it, an admin parameter, is the no-code way to hold more
   supply back: the reserve refills from buybacks and creator fees instead of burning.

Only if third-party miners fail to show up after a live GPU test should the small hybrid
(option C) be built: one reserve instruction paying a bonus capped in the program below the farm
line. Neither a 20% nor a 40% held-back mint does anything a reserve cannot, except add an
overhang and force the venue off pump.fun.

Why, in five points:
1. **The mint cannot fix the real gap without paying for fake work.** The measured GPU cost of
   the 1B ($1.93 per 1M tokens on an A40) is 9.6x the cheapest market price for the same model
   ($0.201 per 1M on OpenRouter). (Update 2026-10-21, section 4.5: the batched worker measures
   $1.17, 5.8x; recommended D2 $2.34 per 1M rows.) Closing that gap with minted tokens needs a mint multiple of
   about k = 8.9. Self-dealing pays from k = 0.18 for anyone with spare hardware, and from about
   0.69 for a farmer paying the honest cost (sections 4.4, 5).
2. **Below the farm line the mint is small.** A farm-safe mint adds at most about 15.7% of gross
   order flow to miner pay. It moves one A40's break-even from 53.4% to about 46% utilization.
   The reserve could pay the same bonus from tokens it already bought, with no new supply and
   without leaving pump.fun (option C).
3. **At k_b = 1, "mint tracks burn" just routes the buyback to miners by a longer path.** Net
   supply is flat, which is the same as sending the buyback to the pool instead of burning it.
   Above k_b = 1 it is inflation. Multicoin's own BME essay warns that a mint that depends on
   burns is circular, and that self-burners can farm it (section 2.2).
4. **The record of revenue-linked emission is poor.** It shows dilution and capture far more
   often than success:
   - Helium's HIP-149 added about 141M HNT (+77% of effective max supply), and HNT fell about 45%
     in a week.
   - Render's burns covered about 21% of emissions in July 2026.
   - Filecoin's 10x verified-deal multiplier drew fabricated datasets.
   - Akash and io.net mint more as the price falls.
   - The tokens that held value are fixed or shrinking supply with buybacks from real fees.
     Hyperliquid's holders revenue runs at about 4.6% of market cap a year. Venice cut emissions
     86% (section 2).
5. **Leaving pump.fun costs reach, and inflation costs the story.**
   - pump.fun earned 60.9% of Solana launchpad fees over the last 30 days; Meteora DBC earned
     3.2%. A standard DBC pool cannot even keep a mint authority.
   - A 400M overhang is 66.7% on top of a 600M float. Keyrock found about 90% of unlock events
     price-negative.
   - CADRE's public edge over Pearl is "fixed supply, every block was paid for" (section 3).

**When the answer flips** (any one of these):
- **Toward the hybrid (C).** A GPU worker is live, the price is set, and after about 30 days of
  screened paid volume no third-party miner keeps a card up. Then fund a bonus b of at most about
  0.10 of gross (hard cap 0.157) from the reserve.
- **Toward a switch (B), burn base only.**
  - The owner decides to launch on Meteora DBC for other reasons. The pump.fun audience then no
    longer counts.
  - The owner wants a miner bonus that lasts years, longer than any affordable dev buy can fund.
  - The mint is the levpad rule with k_b of 1 or less (net non-inflationary), no seed, a ceiling
    of 1M a day or less, and an immutable cap.
- **Never toward a T-base mint with k above about 0.18.** At any volume or price in this model,
  it pays wash volume first.

The numbers to watch before revisiting: screened paid volume from distinct payers, GPU cost per
1M tokens after the worker is batched, and third-party miner count.

## 1. The two designs, exactly

### 1.1 CADRE today (docs/SPEC.md 3.4, 5, 5.1; docs/USE-CASES.md "Why CADRE")

- Supply: pump.fun `create_v2` mints a Token-2022 CADRE with exactly 1,000,000,000 supply and
  sets mint and freeze authority to null (research/NOTES-pump.md section 4, verified). Nothing
  can ever mint more. The agent chain mints nothing: its coin is the bridged token, the whole
  supply preallocated to the bridge escrow.
- Mining reserve: the dev buy (create and buy in one tx) plus pump creator-fee buybacks plus the
  storefront's 10% buyback land in the reserve. Its only exit is `fund_mining`, permissionless,
  rate limited (provisional 10M per call, 20M per 24 h), always into the fixed chain mining pool.
  Storefront buybacks above `bb_reserve_cap` (provisional 100M) are burned.
- Rewards (SPEC 3.4): only job blocks earn subsidy, `subsidy = min(pool x 7 ppm, cap_remaining)`,
  per job capped at `alpha x P` with alpha 0.8 and P = 20% of the tokens the order bought. Pool
  tokens leave only as these rebates.
- Per order: buyer pays `sol_in + overhead`; 10% of `sol_in` is the buyback markup (held until
  delivery); the rest buys CADRE on the curve or PumpSwap (1.25% fee at the launch tier, 0.30%
  of it the creator fee), split W 80% to the server and P 20% to the pool.
- Mainnet pump curve (docs/MAINNET.md section 6, read from chain 2026-10-20): 1 SOL dev buy gets
  34,199,203 CADRE; 3.1218 SOL gets 100M (the reserve cap); the full 793.1M curve costs 86.0679 SOL.

### 1.2 levpad / TILT (~/levpad/docs/PLAN.md "Platform token supply"; ~/levpad/SPEC.md "Platform token: levcap", "Mint budget")

- 1B lifetime cap. 600M minted at a Meteora DBC launch (curve plus graduation liquidity; DBC
  `token_update_authority = 3`, CreatorUpdateAndMintAuthority, then the authority is handed to
  levcap in the launch transaction).
- 400M only through `levcap.mint_to`, an immutable program with two instructions, a fixed minter
  (the engine PDA), a lifetime cap that burns never free, and a fixed `daily_ceiling` (PLAN.md,
  owner 2026-10-20: 10M TILT per day, so at least 40 days for the 400M; SPEC.md's earlier
  recommendation was 1M per day, at least 400 days).
- The working limit lives in the upgradeable engine: **mint tracks burn**. Over a trailing 7-day
  window, grants take at most k (default 1x, hard bound 10x) of the TILT burned by levfee in that
  window, plus a lifetime 10M seed allowance spent before burns exist. At k = 1 net supply holds
  or falls once the seed is spent.
- What levcap guarantees: never more than 1B in total, never more than the ceiling per UTC day.
  What it does not: the engine's upgrade authority decides the terms of minting below those two
  bounds (SPEC.md "What the engine's upgrade authority can still do").

### 1.3 What the switch would mean for CADRE

"Minted in proportion to paid AI revenue" can be wired to two bases, and they behave very
differently, so both are modelled:
- **T base:** mint `k x` the CADRE that paid orders bought (the job's tokens).
- **Burn base (the levpad rule):** mint `k_b x` the CADRE the 10% buyback burned.

Either way the proof would be the existing coinbase receipt: each block's reward record is
provable on Solana by tx_root inclusion under a final light-client header
(`pearl_verify_sbf::chaintx::verify_coinbase`, SPEC 3.4), so an immutable Solana program could
mint against proven paid-work receipts.

## 2. Evidence: comparable tokens and burn-and-mint records

### 2.1 Comparables (market data 2026-10-21)

Market data: CoinGecko `/coins/markets`, last_updated 2026-10-21 20:50 UTC. Fees: DefiLlama
`api.llama.fi/summary/fees/<slug>`, 30-day totals, pulled 2026-10-21. MC/FDV is computed.
Unknown means not found in a citable source.

| token | supply design | MC | FDV | MC/FDV | 30d fees / holder revenue | what happened |
|---|---|---|---|---|---|---|
| Pearl PRL | 2.1B cap, hyperbolic PoW emission paid whether or not work was bought | $819.9M | $4.876B | 0.168 | $944.83 chain fees | narrative-driven; about 1.01M PRL a day issued (Parameter, 2026-10-15), about $2.36M a day at $2.34 (computed), against under $1k of fees in 30 days |
| Hyperliquid HYPE | 1B fixed; 97-99% of fees buy HYPE into a keyless fund | $18.09B | $77.70B | 0.233 | fees $85.36M, holders revenue $68.50M | holders revenue x 365/30 = $833M a year, 4.6% of MC (computed); held near highs |
| Venice VVV | no cap, emissions cut 86% since launch; 5% of credit purchases burned; DIEM stake-for-credit | $1.314B | $2.195B | 0.598 | $979.8K holders revenue | +1,504% over 1y (TOKENOMICS 2.1); buybacks covered only 8-22% of emissions, so lock-for-credit carried it |
| Bittensor TAO | 21M cap, halvings; dTAO routes emissions by stake flow | $3.122B | $5.781B | 0.540 | unknown | subnet external revenue $28-35M a year against over $300M of emissions, 9-12% (cryptobriefing 2026-09-21, via TOKENOMICS 2.6) |
| Render RENDER | BME: burns on jobs, fixed emission schedule | $1.000B | $1.028B | 0.972 | $480.4K | July 2026 burns 105,025 RENDER against 492,132 emitted, 21.3% (computed; @NEWRenderBurn via KuCoin, unverified); YTD net +2.88M RENDER |
| Helium HNT | BME with net-emission re-mint, halvings; HIP-149 supplemental mint | $83.9M | $83.9M | 1.000 | unknown (DefiLlama shows $0) | -99.2% from the $54.88 ATH (computed); HIP-149 (July 2026) about +141M HNT into a team-run multisig, about -45% in a week |
| Akash AKT | BME (AEP-76): burn AKT for ACT credit, mint the shortfall at payout | $220.2M | $220.2M | 1.000 | fees $215.6K | fees 4.5x Feb to Sep 2026, burns still below inflation (CRYPTO-COMPUTE) |
| io.net IO | 800M cap; Incentive Dynamic Engine emits USD-targeted supplier pay | $68.8M | $132.8M | 0.518 | unknown | burn "funded by emissions, not by revenue"; client purchases about 3% of supplier payouts (ownyourmind, 2026-06-22) |
| Grass GRASS | 1B cap; no committed token sink | $496.5M | $709.8M | 0.699 | unknown | about $33M a year of claimed revenue, none to holders yet; still near highs |
| Nosana NOS | 100M, fully circulating | $55.6M | $55.6M | 1.000 | unknown | revenue not found; narrative beta (CRYPTO-COMPUTE) |
| Filecoin FIL | no cap; block rewards plus vesting; 10x verified-deal multiplier | $898.9M | $2.104B | 0.427 | $83.0K | vesting about 66.7M FIL a year plus about 21.7M from block rewards; Filecoin Pay run-rate $59,327 (Aug 2026, 119 payers) |
| Virtuals VIRTUAL | 1B; incentive epochs | $460.0M | $698.6M | 0.658 | fees $392.1K, holders $0 | ACP fees -98.6% the month the subsidy ended (TOKENOMICS 2.7) |
| elizaOS (ex ai16z) | migrated 1:6, supply 6.6B toward 11B | $2.98M | $3.70M | 0.804 | unknown | the closest case of a meme-born AI token adding supply; now about $3M MC (confounded by a lawsuit and the founder declaring it dead) |
| pump.fun PUMP | fixed; 50% of net revenue to buybacks; burned 36% of circulating in April 2026 | $2.365B | $4.234B | 0.559 | holders revenue $30.8M | buyback a floor, not a trend, while revenue fell (TOKENOMICS 2.3) |

Pump.fun-born AI tokens: GOAT peaked near $920M (The Block) and is $0.01515 now, about -98.9%
from its $1.35 ATH. Fartcoin is $0.157 against a $2.48 ATH. No pump.fun- or DBC-launched AI or
compute token was found that attached material verified revenue.

Read-across:
- Every token in the set whose supply grows faster than paid demand trades on narrative or has
  fallen: Pearl, TAO, RENDER, FIL, HNT, IO.
- The two with durable value accrual have fixed or falling issuance and a program-rule flow from
  real fees: HYPE fixed, VVV with emissions cut.
- Pearl is the counterexample on price: high emission, almost no fees, a 0.168 MC/FDV, still
  near highs. That price rides a "fair launch PoW" story, not revenue. CADRE's stated edge is
  being the opposite of it.

### 2.2 Burn-and-mint and revenue-tracked emission: failure modes on record

- **Circularity, named by the inventor of the term.** Kyle Samani's BME essay (Multicoin,
  2018-02-13, https://multicoin.capital/2018/02/13/new-models-utility-tokens/) specifies a fixed
  mint per period. It says the mint "cannot depend on the number of tokens burned", because that
  is circular.
  - Its end notes describe the self-burn attack: when organic burns are low, arbitrageurs burn to
    capture the mint at a guaranteed profit.
  - Factom, the original BME, paid its authority nodes a fixed amount of FCT whatever the usage.
    The company filed Chapter 11 in June 2020 owing up to $7.5M against $640k of assets.
  - "Mint tracks burn" is the variable-mint design the essay warns against. Section 5 computes
    where the self-burn attack starts for CADRE.
- **Wash or fabricated demand to farm multiplied rewards.**
  - Filecoin's 10x verified-deal multiplier took verified deals from under 1% to about 90% of
    onboarding (hackmd cryptoecon, 2023). FIP-0078 says miners "fabricate datasets" and "we don't
    know how much of the data stored via Fil+ deals has an actual user behind it".
  - Virtuals: incentive epochs above fee value, then fees -98.6%.
  - x402: Artemis classed 63.94% of transactions as gamed (TOKENOMICS 4).
- **Supply-side spoofing.** About 25,000 of 524,000 Helium hotspots (about 5%) were confirmed as
  fake-location or gaming in late 2021 (altfins, secondary).
- **Burns far below emissions.**
  - Render: about 21% in July 2026.
  - Helium 2022: about $6.5k a month of data revenue, about $1,150 of it actual data transfer
    (Cointelegraph 2022-07-28; Fortune 2022-09-27).
  - Filecoin: paid demand tiny against issuance.
- **Burn that is really recycled.** Helium re-mints burned HNT up to 1,643.84 HNT a day (Helium
  docs). Below that cap a burn is not removal. At k_b = 1, "mint tracks burn" has the same
  property.
- **Revenue that is a policy, not demand.**
  - Helium Mobile's burn share jumped only after 100% of subscriber revenue was routed to burns
    (Aug 2025). Discretionary buybacks were paused in January 2026 "citing a lack of market
    response" (FalconX 2026-01-14).
  - io.net's burn is funded by emissions.
- **Death spiral when the price falls.** A mint sized from USD revenue issues more tokens as the
  price falls:
  - io.net IDE: $10k of payouts needs 5,000 tokens at $2 and 10,000 at $1 (The Block,
    2026-06-11, sponsored).
  - Akash AEP-76: settling 1,000 ACT at $0.90 pays 1,111.11 AKT, with no numeric mint cap.
  - The extreme case is Terra: LUNA went from about 1B to about 6T in about 3 days (Harvard Law
    corpgov, 2023-05-22).
  - CADRE's T-base mint has the same 1/price property (section 4.4). A token-denominated daily
    ceiling is the only bound.
- **Governance capture of the mint.**
  - Helium HIP-149 passed a supplemental mint of about 141M HNT over 36 months into a
    Nova-administered multisig, raising effective max supply about 77%. Curtailing it needs only
    a simple majority (solanacompass 2026-07-14). HNT fell about 45% in the week after it was
    published.
  - Bittensor's emission routing by stake flow "cannot fully distinguish" real demand from
    coordinated flows (tokenomist).
  - levcap removes the cap and the ceiling from governance, but not k, the base or the window,
    which live in the upgradeable minter.
- **Subsidised demand that collapses.** Helium Mobile's free plan was ended in 2026 as "not
  sustainable long term". Morpheus served 63.2B tokens of compute, of which 48,138 were paid
  (TOKENOMICS 2.10).

## 3. Evidence: launch venue and holder perception

### 3.1 pump.fun vs Meteora DBC vs a custom curve (2026)

- **Mintable supply on pump.fun is impossible.**
  - `create_v2` sets mint and freeze authority to null and mints exactly 1B (research/NOTES-pump.md
    section 4, verified in LiteSVM).
  - pump's help center says "No further coins can be minted"
    (https://intercom.help/pumpfun-web/en/articles/11002404-total-token-supply). BloFin's on-chain
    check of 2026-10-03 agrees.
  - The only exception is Mayhem/agent mode, where the platform's agent mints a second billion and
    burns its unsold share after 24 h (https://pump.fun/docs/mayhem-mode). The pump platform, not
    the creator, controls it.
- **Audience.**
  - 30-day Solana launchpad fees (DefiLlama `/overview/fees`, 2026-10-21): pump.fun $62.6M
    (60.9%), StonkFun $19.1M, LaunchLab $7.86M, BONK.fun $6.28M, **Meteora DBC $3.33M (3.2%)**,
    Bags $358K.
  - 30-day volume: pump.fun curve $4.74B plus PumpSwap $10.36B, against Meteora DBC $477.5M.
  - Bitquery: pump.fun made 78% of about 11.5M new Solana tokens from Nov 2025 to 2026-10-07; about
    1 in 58 graduated (cryptonews.net).
  - The flip side of the audience is noise: tens of thousands of launches a day (about 42,000 on
    one June 2026 day, solanacompass).
- **Fees and liquidity.**
  - Curve: 0.30% creator plus 0.95% protocol. PumpSwap fees are tiered by market cap: 0.30%
    creator below 420 SOL MC, 0.95% creator at 420-1,470 SOL, then stepping down
    (https://pump.fun/docs/fees, updated 2026-10-17). Graduation costs 0.015 SOL.
  - Graduated liquidity is locked, and since BOOST the dead migration liquidity is spent on
    buy-and-burn in the first five minutes (The Block 2026-07-29, via TOKENOMICS 2.3).
  - Creator fees can be split across up to 10 wallets (The Block, Jan 2026).
- **Meteora DBC.**
  - Five token authority options. **On a standard pool the mint-authority options are rejected and
    the mint authority is revoked at initialization.** Only transfer-hook pools may keep it
    (https://docs.meteora.ag/core-products/dbc/token-2022-support). levpad uses exactly this
    (`token_update_authority = 3` plus SetAuthority to levcap).
  - Supply is configurable (pre/post migration, leftover receiver) and migration is to DAMM v2. LP
    can be locked, but at least 10% must stay locked one day after migration.
- **Holding back supply on pump.fun (what CADRE already does).** The dev buy in the create
  transaction goes to the reserve PDA, which can only pay the chain mining pool, rate limited, with
  a frozen destination (SPEC 5 "Reserve exit").
  - That is a program-controlled reserve, stronger than a Streamflow-style time lock, and it is the
    only pump.fun-compatible way to hold supply back.
  - Size versus perception: 3.1218 SOL buys 100M (10%); 400M would take about 18.06 SOL
    (computed: 400M = 1,073M x s / (30 + s) gives s = 17.83 SOL net, / 0.9875).
  - Screening guides fail a coin when a bundle takes 10% or more of supply
    (https://madeonsol.com/blog/how-to-score-pumpfun-token-60-seconds). A dev buy near 10% or
    above will be flagged unless the reserve PDA and its rules are shown next to it. A 40% dev buy
    would look like a 40% insider position whatever the program says.
- **A native buyback rail.** pump.fun's "Tokenized Agent" setting buys back and burns the coin
  every hour from deposited SOL or stablecoins, at a creator-set percentage
  (https://pump.fun/docs/tokenized-agent-disclaimer). Its authority is "centrally controlled by the
  Pump Platform". CADRE's storefront buyback is already a program rule under CADRE's own programs,
  so this adds nothing needed. It is evidence that revenue buy-and-burn on a fixed supply is the
  shape pump.fun itself now supports.

### 3.2 Holder perception in 2026

- **Buybacks.**
  - Spend reached about $638M in 2026 through August, mostly HYPE and PUMP. Bitwise, 1inch and
    Spark warn that buybacks do not fix a weak business (KuCoin flash).
  - CryptoRank's 2025 study of 9 buyback programs found only HYPE and AAVE held gains. The
    condition that recurs across sources is that buybacks must outpace emissions and unlocks.
- **Unlocks and overhang.** Keyrock's study of over 16,000 unlock events found about 90%
  price-negative, with declines starting up to 30 days before. Unlocks of 5-10% of supply hit
  about 2.4x harder (cryptoslate; beincrypto).
  - A revenue-tracked mint is not a scheduled unlock, but holders see the same thing on a
    screener: MC/FDV of 0.6 at launch, with 400M "to come".
- **CADRE's own narrative research.**
  - research/narratives/SYNTHESIS.md and TOKENOMICS.md section 7: "Do not add inflation, halvings
    or a block reward independent of paid work".
  - CRYPTO-COMPUTE.md finding 5: low float, high FDV and unlocks destroyed value regardless of
    tech; tokens with no team unlock schedule (Pearl) did not suffer this.
  - The fixed-supply, paid-work story is what separates CADRE from Pearl in public.
- **Not found:** a 2025-2026 holder survey, or a primary Messari, Galaxy or Delphi piece on fixed
  versus inflationary supply.


## 4. Miner economics model

### 4.1 Inputs (all cited)

| input | value | source |
|---|---|---|
| SOL/USD | 108.09 | CoinGecko `simple/price`, last_updated 2026-10-21 20:51 UTC |
| pump fee on a new curve | 1.25% (95 bps protocol + 30 bps creator) | docs/MAINNET.md section 6 |
| buyback markup | 10% of `sol_in` | SPEC 5.1 (owner decision 2026-10-18) |
| P, alpha | 20% of tokens bought, 0.8 | SPEC 3.4, 5.1 |
| emission_ppm, block time | 7 per block, 60 s (1,440 blocks a day) | SPEC 3.4; MAINNET.md params |
| inference price (1B) | $3.86 per 1M rows, one row = one token (2x the A40 cost) | docs/ECOSYSTEM.md 5.1 D2 (owner, 2026-10-20) |
| fixed per-order knobs | base 10,000,000 lamports (buys tokens), overhead 14,000,000 lamports (rent and fees) | ECOSYSTEM.md 5.1; SPEC 5.1 |
| GPU cost, 1B | $1.93 per 1M (8 processes on one A40, 84.9 tok/s summed); $6.52-6.68 (one process) | MAINNET.md section 7; HANDOFF item 20 |
| GPU cost, 8B | $19.16-20.45 per 1M (one process, 8.0-8.6 tok/s) | same |
| A40 rental | $0.59/hr on demand (RunPod secure cloud) | same |
| MACs per row | 1B: 1,235,746,816; 8B: 7,504,658,432 | SPEC 4.5; MAINNET.md section 11 item 11 |
| market price, Llama-3.2-1B-Instruct | $0.027 prompt / $0.201 completion per 1M | OpenRouter `api/v1/models`, read 2026-10-21 20:52 UTC |
| market price, Llama-3.1-8B-Instruct | $0.05 prompt / $0.08 completion per 1M | same |
| CADRE price | **input**: FDV of 411 SOL (the curve's price at graduation, computed below), 4,110 SOL and 41,100 SOL (10x and 100x, chosen only to span a range) | not launched; no market price exists |

Curve reference (computed from MAINNET.md section 6 Global fields, constant product on virtual
reserves): start price = 30 / 1,073M = 2.796e-8 SOL (FDV 27.96 SOL). Buying the 793.1M real
tokens leaves vT = 279.9M, vS = 30 x 1,073M / 279.9M = 115.01 SOL (85.0 SOL net in, 86.08 with
the 1.25% fee, matching MAINNET's 86.0679), so the graduation price is 115.01 / 279.9M =
4.109e-7 SOL, an FDV of 410.9 SOL (about $44,400 at $108.09).

### 4.2 Where one SOL of an order goes (computed)

Per 1 SOL of gross `sol_in` S (ignoring price impact and the fixed overhead):

    buyback markup                      0.10000
    job part J                          0.90000
    tokens bought  0.9 x (1 - 0.0125) = 0.88875
    W  = 0.8 x 0.88875                = 0.71100   (server)
    alpha x P = 0.8 x 0.2 x 0.88875   = 0.14220   (subsidy, only if the pool is big enough)
    miner, after selling: (0.711 + 0.1422) x 0.9875 = 0.84254
    round-trip loss for a self-dealer:  1 - 0.84254  = 0.15746   (the 15.7% in USE-CASES)

So under the current design **the miner receives at most 93.6% of the job part J** (0.84254 /
0.9), or 84.3% of what the buyer paid. The subsidy is a rebate of the buyer's own P: in aggregate
the pool pays out at most 0.8 of the P it takes in, so the reserve adds **zero net pay** to miners.
What the reserve buys is sufficiency: per day the pool can pay at most `pool x 7e-6 x 1,440` =
1.008% of itself, so paying every cap in full needs a pool of at least
`0.1422 x S_daily / (price x 0.01008)` tokens, about 15.9 days of the tokens orders buy (0.1422 / 0.01008 / 0.88875, computed). When the pool
is short, the miner falls toward W alone (0.711 x 0.9875 = 0.702 of S).

### 4.3 Does the current design pay a GPU miner? (computed)

Miner revenue per 1M rows of 1B at the decided price: 0.93615 x $3.86 = **$3.61**.
- One A40 at $0.59/hr is $14.16 a day and serves at most 84.9 x 86,400 = 7.335M rows a day
  (8 processes). Full-utilization revenue $26.51/day against $14.16: **break-even at 53.4%
  utilization** ($1.93 / $3.61).
- 8B: a row has 7,504,658,432 / 1,235,746,816 = 6.073x the MACs, so it is priced 6.073 x $3.86 =
  $23.44 per 1M; the miner gets $21.94 against a cost of $19.16-20.45: break-even at **87.3-93.2%
  utilization** on the one-process measurement. Not viable until the 8B worker is batched.
- Per-order fixed fees dominate small orders: a 256-row top-up costs the buyer 25,121,471
  lamports ($2.72, or $10,607 per 1M rows); a 1,048,576-row balance costs 67,543,487 lamports
  ($7.30, $6.96 per 1M rows). (computed: J = 10,000,000 + rows x 36.42, gross = J / 0.9 + 14,000,000)

Against the market (computed from the OpenRouter prices): the decided 1B price is 19x the
cheapest listed completion price and 143x the prompt price; the 8B price is 293x / 469x. The
measured GPU cost alone ($1.93 per 1M for the 1B) is 9.6x the market completion price.

| paid volume (1B rows/day) | job part J | miner revenue (current) | GPU cost at that load | covers it? |
|---|---|---|---|---|
| 1M | 0.0357 SOL ($3.86) | $3.61/day | one A40 day $14.16 (14% busy) | no |
| 7.335M (one A40 full) | 0.2620 SOL ($28.31) | $26.51/day | $14.16 | yes, $12.35 margin |
| 73.35M (10 A40s full) | 2.6195 SOL ($283.14) | $265.07/day | $141.60 | yes |
| 733.5M (100 A40s full) | 26.195 SOL ($2,831) | $2,650.66/day | $1,416.00 | yes |

The volumes are **inputs**. Devnet has served 2 storefront orders (USE-CASES "Honest limits").

### 4.4 The split-supply mint on top (computed)

**T base, multiple k.** The miner also gets `k x tokens bought`, worth (at a constant price, sold
once) `k x 0.88875 x 0.9875 = 0.87764 k` per SOL of S, i.e. `0.97515 k` per unit of J.
- 1B revenue per 1M rows: k = 0.15: $4.18 (break-even utilization 46.2%); k = 0.5: $5.50 (35.1%);
  k = 1: $7.38 (26.2%). Current: $3.61 (53.4%).
- To pay the miner's GPU cost while charging the market price, k must be
  `(G/S - 0.84254) / 0.87764`: at J = $0.201 per 1M (market completion) k = **8.89**; at J = $0.027
  (market prompt) k = **72.3**.

**Burn base, multiple k_b (levpad).** The 10% buyback buys `0.1 x 0.9875` tokens per SOL; minting
k_b times that and selling it is worth `0.1 x 0.9875 x 0.9875 k_b = 0.09752 k_b` per SOL of S. At
the levpad default k_b = 1 that is **+9.75% of gross** to miners, and net supply is flat (mint =
burn). In Cadre the buyback burns only above the 100M reserve cap; below it the burn base is zero.

**Dilution (tokens minted per day).** `minted = k x 0.88875 x S / price`, and as a share of 1B,
`k x 0.88875 x S / FDV` (FDV in SOL):

| paid volume | FDV (input) | k = 0.15 | k = 1 | k_b = 1 (equals the buyback tokens) | 10M ceiling binds? |
|---|---|---|---|---|---|
| 7.335M rows/day (S 0.2911 SOL) | 411 SOL | 0.094M/day (0.009%) | 0.629M/day (0.063%) | 0.070M/day | no |
| 73.35M rows/day (S 2.911 SOL) | 411 SOL | 0.944M/day (0.094%) | 6.29M/day (0.63%) | 0.699M/day | no |
| 733.5M rows/day (S 29.11 SOL) | 411 SOL | 9.44M/day (0.94%) | 62.9M/day, cut to 10M (400M gone in 40 days) | 6.99M/day | yes at k = 1 |
| 733.5M rows/day | 4,110 SOL | 0.944M/day | 6.29M/day | 0.699M/day | no |

Two properties follow directly from the formula:
- **Dilution is inverse to price.** The same SOL revenue mints 10x more tokens at a 10x lower
  price, so a falling price raises issuance exactly when holders are weakest (the death-spiral
  shape, section 2).
- **At early volumes the mint is economically small.** At one busy A40 (7.3M rows a day) a k = 1
  T-base mint is worth about $27.6 a day (0.97515 x $28.31) to all miners combined.

### 4.5 After batching (2026-10-21, measured; docs/MAINNET.md section 7)

The 9.6x cost gap above was measured with a worker serving one request at a time. The batched
worker (tools/cadre/batch.py, continuous batching, bit-identical per request, SPEC 4.7) on the
same A40 at $0.59/hr, cadre-glue-v1, 64 prompt + 64 output tokens per request, segment files
written: 140.0 tok/s at 32 concurrent requests (139.4 in a second run), so

| figure | before (one request at a time) | after (batched, measured) |
|---|---|---|
| GPU cost per 1M output tokens, 1B | $1.93 (fp32 glue, 8 processes); $21.65 for cadre-glue-v1 one at a time | **$1.17** (cadre-glue-v1, 32 concurrent) |
| same, compute only (no segment files) | n/a | $0.51-0.52 (318-321 tok/s at 64-128) |
| cost / cheapest market completion price ($0.201) | 9.6x | **5.8x** (2.6x compute only) |
| cost per 1M chain rows at this mix (127 rows per 64 output tokens) | n/a | $0.59 |

Recommended new D2 (owner decision; same rule as 2026-10-20, 2x the measured cost on the
output-token basis, which is conservative because prompt rows cost less than decode rows):
**$2.34 per 1M rows** (was $3.86, -39%). At SOL/USD 108.09 (the 4.1 input): 2.342e-6 / 108.09 x
1e9 = 21.67 lamports per row; one 1B row is 2.4715 GFLOP, so config key 25
`price_mlamports_per_gflop` = **8,767** (was 14,736). Against the market it is 11.6x the cheapest
completion price (was 19x) and 87x the prompt price (was 143x). Server revenue per 1M rows at the
new price: 0.93615 x $2.34 = $2.19; break-even utilization stays 53.4% on the output-token basis
(the price is 2x the cost by construction) and is 27% on the per-row basis of this mix ($0.59 /
$2.19). What would close the rest of the gap, from the same run: the segment files are half the
cost (compute only is $0.52), so a gateway that takes A/C from the worker in memory and keeps C
only as its commitment would bring the cost to about 2.6x the market completion price; that is not
built. The 8B was not measured batched, so the 8B rows of 4.3 are unchanged.

## 5. Wash trading and farming (computed)

A self-dealer pays for a job, serves and mines it, and sells everything back. Per SOL of gross S:

    loss without any mint            0.15746 x S   (4.2)
    plus the real GPU work           G              (the evaluators replay the work, SPEC 4.7, so it cannot be skipped)
    plus the fixed overhead          O = 14,000,000 lamports per order (negligible on large orders)
    gain from the mint               0.87764 x k x S   (T base)  or  0.09752 x k_b x S (burn base)

Wash is profitable when the gain exceeds `0.15746 S + G + O`:

| farmer's marginal GPU cost | break-even k (T base) | break-even k_b (burn base) |
|---|---|---|
| near zero (idle or already-paid hardware; lower bound) | **0.179** | **1.61** |
| the honest cost at the decided price (G = 0.5 J = 0.45 S) | 0.692 | 6.23 |
| full cost while selling at the market completion price (G/S = 8.69) | 10.03 | 90.7 |

(computed: k* = (0.15746 + G/S) / 0.87764; k_b* = (0.15746 + G/S) / 0.09752; O ignored)

What this shows:
1. **The window between "pays honest miners" and "pays farmers" is always the same width:**
   `k_farm - k_honest = 1 / 0.87764 = 1.139` for a farmer with the same cost as the honest miner,
   because the farmer's only extra cost is the buyer's price S. A farmer whose GPU is cheaper
   than the marginal honest miner's by more than S (per job) profits from self-dealing at any k
   high enough to keep that honest miner in business. When S is small relative to the GPU cost
   (any market-price scenario, S is 10% of G), the paid-work gate costs almost nothing to pass.
2. **The safe zone is k below about 0.18 (T base) or k_b below about 1.6 (burn base).** That adds
   at most about 15.7% of gross to miner pay (0.87764 x 0.179). It cannot close a 9.6x gap
   between GPU cost and market price.
3. **The daily ceiling does not stop farming; it prices it.** Once farming pays, wash volume grows
   until the ceiling binds and profit is competed to zero. The ceiling's value is
   10M x price = 1% of FDV a day: 4.11 SOL ($444) at an FDV of 411 SOL, 41.1 SOL at 4,110 SOL.
   A zero-cost farmer would run up to 4.11 / 0.15746 = 26.1 SOL a day of fake orders to collect it
   at an FDV of 411 SOL (computed). The ceiling then works as a fixed emission paid to whoever
   self-deals cheapest: Pearl's emission with an entry fee.
4. **Under the current design no k exists**, so every round trip loses at least 15.7% plus the GPU
   cost. The only farming risk today is any future incentive above that line
   (research/agents/TOKENOMICS.md 3.3), and Agent Credit liens sized from self-paid income.

## 6. Options compared

Options (the hybrid is option C):

| | A. Stay (fixed 1B on pump.fun) | B. Switch (600M on DBC + 400M revenue-tracked mint) | C. Hybrid: stay fixed, add a farm-safe bootstrap bonus from the existing reserve |
|---|---|---|---|
| Venue | pump.fun (60.9% of Solana launchpad fees over 30 days; curve + PumpSwap $15.1B of 30-day volume) | Meteora DBC transfer-hook pool (3.2% of launchpad fees, $477.5M of 30-day volume) or a custom curve | pump.fun |
| Supply | 1B, mint authority null at create; only falls (burns) | 1B cap; 600M at launch; up to 400M more, at most 10M a day | 1B, only falls |
| Miner pay per SOL the buyer pays | 0.843 (0.936 of J) when the pool is big enough | 0.843 + 0.878k (T base) or + 0.0975k_b (burn base) | 0.843 + bonus b, with b at most about 0.15, paid from reserve tokens |
| Pays GPU miners at the decided $3.86/1M price? | yes above 53.4% utilization of an A40 (1B) | yes at lower utilization (46.2% at k = 0.15) | same as B at the same bonus |
| Pays GPU miners at market prices ($0.20/1M for 1B)? | no (cost 9.6x price) | only at k of about 8.9, which is farmable | no |
| Farm-safe bound | no reward to farm (loss at least 15.7%) | k below 0.18 (T base) or k_b below 1.6 (burn base) | b below 0.157 (same arithmetic) |
| Holder dilution | none; net removal about 13.6% of gross order flow (computed below) | up to 400M (40% of the launch supply, 66.7% on top of the 600M); inverse to price | none; the bonus spends reserve tokens already bought |
| Bootstrap before revenue | the dev buy, paid in SOL, visible (1 SOL: 34.2M; 3.12 SOL: 100M) | a seed allowance minted for nothing (levpad 10M) | the dev buy |
| Trust surface | reserve, storefront, bridge upgrade authority (already listed) | plus an immutable mint program and a trusted, upgradeable "revenue oracle" (the receipt verifier and the k rule) | plus one rate-limited instruction in the reserve |
| Narrative | "fixed supply, every block was paid for, buy and burn from real fees" | "inflation tied to revenue" (has to be explained; Pearl and Bittensor are the reference points) | same as A |
| Engineering | done | new launch path, new mint program, receipt-to-mint verifier, bridge and supply accounting changes, docs | small |

Net removal under A (computed, per SOL of gross order flow): the 10% buyback (0.0988 SOL of
tokens after the fee, burned above the reserve cap, otherwise held in the reserve) plus 0.2 x P
left in the pool for good (0.2 x 0.2 x 0.88875 = 0.0356): about 0.134-0.136 of every SOL ordered
leaves the float.

### 6.1 The owner's 80/20 variant, and getting the same effect without a mint

Owner, 2026-10-21: "i dont want to implement this if its not SUPER necessary, maybe we mint 80% or
so". Compared here: an 800M launch with 200M mintable, the 60/40 split, and ways to hold supply
back on pump.fun with no mint (all figures computed from MAINNET.md section 6 and the rules above).

| | status quo (1B fixed, dev buy reserve) | 80/20 (800M + 200M mint) | 60/40 (600M + 400M mint) |
|---|---|---|---|
| Venue | pump.fun | DBC transfer-hook pool or custom curve (pump.fun impossible) | same as 80/20 |
| Held back | the dev buy: 1 SOL = 34.2M (3.4%), 3.1218 SOL = 100M (10%), 6.96 SOL = 200M (20%) | 200M | 400M |
| Overhang on the launched float | 0 (reserve tokens can only rebate paid work, at most alpha x P each) | 200 / 800 = 25% | 400 / 600 = 66.7% |
| MC/FDV a screener shows at launch | 1.0 (circulating excludes the reserve via `/api/supply`) | 0.8 | 0.6 |
| Lifetime of the held-back pool at the 10M/day ceiling | n/a (fund_mining moves at most 20M/day into the pool) | 20 days minimum | 40 days minimum |
| Farm-safe extra miner pay | same bound as a mint if a bonus rule is added (option C): at most about 15.7% of gross | at most about 15.7% of gross (k below 0.179) | same |
| Who pays for the held-back tokens | the dev, in SOL, at launch, visible on chain | nobody: minted later, paid for by holders through dilution | same |
| Engineering | none | the whole of section 8 | the whole of section 8 |

The size of the split does not change the economics. The farm line (section 5) caps what any
mint can safely pay per SOL of revenue, whether the pool behind it is 200M or 400M. The split
only sets how long the pool lasts and how big the overhang looks. A smaller split means less
overhang, but it still costs the venue, the program and the "fixed supply" claim.

**The same effect with no mintable supply (keeps pump.fun):**
1. **A larger dev-buy reserve.** The reserve is already program-locked, its only exit is the
   chain mining pool, and its destination is frozen (SPEC 5 "Reserve exit"). Approximate costs:
   - 100M: 3.12 SOL.
   - 200M (the 80/20 equivalent): 6.96 SOL.
   - 400M: about 18.06 SOL.
   Above about 10% the dev buy will be flagged by bundle screens (section 3.1), so 100M is the
   comfortable top unless the reserve is explained on the site and explorer.
2. **Buyback and creator-fee refill instead of burning.** Storefront buybacks go to the reserve
   while it holds less than `bb_reserve_cap`, and pump creator fees are cranked into the reserve
   unconditionally. Raising the cap from 100M to, say, 200M grows the held-back supply from paid
   volume alone, with no code change. Every token in it was bought with real SOL, which is the
   point a mint cannot match.
3. **A per-receipt bonus from the reserve (option C).** This is needed only if a bonus is ever
   needed. It gives exactly what the 80/20 mint would give miners, from tokens already bought.

**Is a bigger reserve even useful today?** Under the current rules the reserve adds no net pay.
It keeps the alpha x P rebate full, which needs about 16 days of the tokens orders buy in the pool
(section 4.2):
- 1.36M tokens at one A40-day of 1M rows and an FDV of 411 SOL.
- 9.99M at one fully busy A40.
- 99.9M at ten fully busy A40s.
- About 1B at a hundred, at that price. At a 10x higher price, a tenth of each.
So 34M-100M is enough for any early volume in this model. More held-back supply matters only if
option C is turned on.

## 7. Risks

- **Switch (B):**
  - Wash farming is the dominant risk. Any k that matters to miners is above the farm line for an
    efficient operator (section 5), and the 10M ceiling then becomes a fixed emission captured by
    the cheapest self-dealer. This is exactly what the narrative research says killed value
    elsewhere: Virtuals' epochs (fees -98.6% when the prize stopped), Filecoin's farmed verified
    deals, Helium's spoofed hotspots (section 2).
  - Reflexive dilution: tokens minted per SOL of revenue scale with 1 / price, so issuance rises
    when the price falls (section 4.4).
  - The mint's terms live in upgradeable code. levcap makes the cap and ceiling permanent, but
    the rule below them (k, the base, the window) is whatever the minter program says; the same is
    true here. Governance capture of k is the failure the burn-and-mint record shows (section 2).
  - Venue: leaving pump.fun gives up the venue with about 61% of launchpad fees and about 10x
    Meteora DBC's curve volume; a DBC standard pool cannot keep a mint authority at all, only a
    transfer-hook pool can (Meteora docs).
  - Perception: 400M of future supply is a 66.7% overhang on the 600M float. Keyrock found about 90%
    of unlock events negative, with 5-10% unlocks hitting about 2.4x harder; MC/FDV is the number
    screeners show first (Pearl 0.168).
  - Agent Credit: liens are sized from proven income (TOKENOMICS item 7); self-paid income that
    earns minted rewards inflates the credit base too.
- **Stay (A):**
  - Miners are paid only what buyers pay. At early volumes and at a price 19-469x the market, there
    may be too few buyers for even one A40 to reach 53.4% utilization. That is a product cost
    problem (the worker is launch-bound, HANDOFF item 20) that no supply design fixes.
  - A small dev buy leaves the pool short: below about 16 days of the tokens orders buy the miner's
    rebate falls toward W alone (section 4.2).
  - Program upgrade authority over the reserve and storefront stays trusted until the multisig
    decision (MAINNET section 11 item 4).
- **Hybrid (C):** the bonus is a subsidy from reserve tokens, so it must stay below the farm line
  (b < 0.157 of gross, set lower to leave margin), be rate limited, and stop when the reserve is
  spent; otherwise it has A's risks.

## 8. If the owner switches: what changes (high level)

Only if the owner picks B. High level; nothing here is built.

1. **Launch path.** pump.fun cannot be used (mint authority null at `create_v2`). Either a Meteora
   DBC transfer-hook config with CreatorUpdateAndMintAuthority, a SetAuthority to the mint
   program's PDA and `register` in the launch transaction (levpad's sequence), or a custom curve.
   The reserve's `launch` (pump create_v2 + dev buy), the storefront's `buy_job` / `buy_job_amm`
   (pump curve and PumpSwap CPIs), the crank's creator-fee path (pump creator vault) and every
   pump fixture would be rewritten for DBC and DAMM v2. Creator fees become DBC partner/creator
   fees.
2. **Mint program.** A levcap-like immutable program (lifetime cap 1B, ceiling per UTC day, one
   minter) plus a minter that mints only against proven coinbase receipts:
   `verify_coinbase(header, record, path)` under a final light-client header, a per-receipt replay
   guard, and the k rule over a trailing window of paid order flow (or of buyback burns).
3. **Chain and bridge.** The chain premints the whole supply into the bridge escrow (1e18 base
   units) and the bound is `1e18 - escrow <= 1000 x mint supply`; it still holds with a smaller
   Solana supply, but the reserve, pool and "supply stays 1B" text, `/api/supply`, the DefiLlama
   adapter and the explorer's circulating numbers change. Rewards minted on Solana either stay on
   Solana (paid to the miner's Solana address) or are deposited through the bridge.
4. **Subsidy rule.** SPEC 3.4's `alpha x P` cap stays the farming guard for the pool; the mint is
   a second reward stream with its own cap per receipt. The reserve's role shrinks to the seed.
5. **Docs and claims.** USE-CASES "no new supply: 1B fixed, the chain mints nothing", CLAUDE.md's
   shape paragraph, the site and cadre-docs.
6. **Mainnet runbook.** MAINNET.md sections 3-6 and 11 (dev buy size becomes seed size; a DBC curve
   threshold and range replace the pump Global figures).

If the owner picks C instead: one new reserve instruction (or a `fund_mining` variant) that pays a
per-receipt bonus `b x` the order's job tokens from the reserve, proven by the same coinbase
receipt, with `b` admin-editable but hard-capped below 0.157 in the program, plus a daily ceiling
and a lifetime budget equal to what the reserve holds. No venue, supply or bridge change.

## Sources

Primary and data sources (retrieved 2026-10-21 unless dated):
- CoinGecko API `/coins/markets` and `/simple/price` (SOL $108.09); DefiLlama `/summary/fees/<slug>`, `/overview/fees`, `/overview/dexs`.
- OpenRouter `https://openrouter.ai/api/v1/models` (Llama-3.2-1B-Instruct, Llama-3.1-8B-Instruct prices).
- pump.fun: https://pump.fun/docs/fees, https://pump.fun/docs/tokenized-agent-disclaimer, https://pump.fun/docs/mayhem-mode, https://intercom.help/pumpfun-web/en/articles/11002404-total-token-supply; BloFin https://blofin.com/academy/education/pumpfun/pump-fun-coin-supply-is-not-fixed.
- Meteora: https://docs.meteora.ag/core-products/dbc/token-2022-support, https://docs.meteora.ag/core-products/dbc/launch-configurations.
- Bitquery via https://cryptonews.net/news/market/33623012/; https://solanacompass.com/news/pumpfun-launched-42000-tokens-in-one-day-fewer-than-2-will-ever-reach-a-dex; https://madeonsol.com/blog/how-to-score-pumpfun-token-60-seconds; The Block https://www.theblock.co/post/384975/pump-fun-overhauls-creator-fees-token-launches-highest-daily-september.
- Pearl: https://parameter.io/prl-token-surges-sevenfold-since-august-on-ai-mining-model-despite-code-controversy/ (2026-10-15); node source research/pearlrepo (research/NOTES-proof.md section 6).
- Helium: https://docs.helium.com/tokens/; https://solanacompass.com/news/heliums-hip-149-passes-advisory-council-election-now-open-to-vehnt-holders (2026-07-14); https://crypto-economy.com/es/helium-hnt-se-desploma-cerca-del-45-en-una-semana/; https://www.falconx.io/newsroom/the-3-depin-protocols-seeing-record-activity (2026-01-14); https://cointelegraph.com/news/critique-on-helium-s-6-5k-monthly-revenue-causes-a-stir (2022-07-28); https://fortune.com/2022/09/27/embattled-helium-attempting-crypto-powered-mobile-network; https://altfins.com/crypto-screener/hnt-helium; https://www.androidauthority.com/helium-mobile-ceo-comments-3670637/.
- Render: https://know.rendernetwork.com/basics/burn-mint-equilibrium; https://www.kucoin.com/news/insight/RENDER/6a6de0789bd794000857627b (2026-08-01, reposting @NEWRenderBurn).
- Multicoin BME essay: https://multicoin.capital/2018/02/13/new-models-utility-tokens/. Factom: https://www.theblock.co/linked/60885/blockchain-firm-factom-begins-dissolution-process-after-failing-to-source-additional-funding; CoinDesk 2020-06-19 Chapter 11 report.
- Filecoin: https://docs.filecoin.io/storage-providers/filecoin-deals/verified-deals; https://hackmd.io/@cryptoecon/rknAqoOAh; https://github.com/filecoin-project/FIPs/blob/master/FIPS/fip-0078.md; https://www.kucoin.com/blog/ae-fil-issuance-drops-75-percent-in-october-2026-as-filecoin-paid-onchain-demand-grows (2026-09-15).
- io.net: https://www.theblock.co/research/blockchains/the-incentive-dynamic-engine-io-nets-shift-to-sustainable-tokenomics-402943 (sponsored); https://buttondown.com/ownyourmind/archive/what-ionets-new-token-burn-actually-runs-on/ (2026-06-22).
- Akash: https://akash.network/roadmap/aep-76/. Bittensor: https://tokenomist.ai/research/bittensor-and-subnets-how-the-emission-engine-works; https://pineanalytics.substack.com/p/the-bear-case-for-bittensor-tao.
- Grass: https://www.kucoin.com/news/flash/grass-generates-33m-annually-from-ai-customers-governance-vote-on-revenue-capture-set-for-july-7.
- Terra: https://corpgov.law.harvard.edu/2023/05/22/anatomy-of-a-run-the-terra-luna-crash/.
- Unlocks and buybacks: https://cryptoslate.com/90-of-token-unlocks-drive-prices-down-declines-begin-a-month-ahead/; https://cryptorank.io/insights/analytics/do-buybacks-work-in-2025; https://www.kucoin.com/news/flash/crypto-token-buybacks-surge-to-640m-in-2026-spark-debate-on-value-creation; https://www.coindesk.com/markets/2026/04/29/pump-fun-burns-36-of-pump-supply-in-usd370-million-wipe-locks-50-revenue-into-ongoing-buybacks.

In-repo: docs/SPEC.md 3.4, 4.5, 5, 5.1; docs/MAINNET.md 6, 7, 11; docs/HANDOFF.md item 20;
docs/ECOSYSTEM.md 5.1 (D2 price); docs/USE-CASES.md "Why CADRE"; research/agents/TOKENOMICS.md;
research/narratives/{SYNTHESIS,CRYPTO-COMPUTE,FLOWS-AND-PUMPS}.md; research/NOTES-pump.md;
~/levpad/docs/PLAN.md and ~/levpad/SPEC.md (read only).
Model script (not committed): the arithmetic in sections 4 and 5 is reproducible from the
inputs table with the formulas shown.
