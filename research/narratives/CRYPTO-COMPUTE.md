# Crypto compute and infra narratives, 2026 (read 2026-10-18)

Scope: decentralized GPU/compute, decentralized training/inference (Bittensor and subnets),
useful-work PoW (Pearl), ZK prover markets, AI agents and agent payments (x402), TEEs and
confidential compute, storage/data, robotics DePIN, BTC miners pivoting to AI hosting, and
the new narratives that ran in the last 3 to 6 months.

## How the numbers were gathered (read this before using any figure)

- **Prices, market caps, YTD/90d/30d**: CoinPaprika public API, read 2026-10-18 02:46 UTC.
  Current: `https://api.coinpaprika.com/v1/tickers/<id>`. History (daily 00:00 UTC):
  `https://api.coinpaprika.com/v1/tickers/<id>/historical?start=2026-01-01&interval=1d`.
  "Jan 1" = 2026-01-01 snapshot, "90d" = 2026-07-19, "30d" = 2026-09-17. I computed the
  % changes and "USD added YTD" (mcap now minus mcap on 2026-01-01) myself from those rows.
  CoinGecko's API rate-limited me (HTTP 429) for most of the session, so CoinPaprika is the
  one consistent source. CoinPaprika circulating-supply figures differ from CoinGecko for some
  tokens (TAO: Paprika mcap $2.57B vs CoinGecko $3.05B, see per-project notes).
- **FDV**: price x max supply from the same CoinPaprika ticker; where a token has no max supply
  I used total supply and marked it `*` (inflationary, so the FDV is a floor, not a cap).
- **Revenue/fees**: DefiLlama fees API, read 2026-10-18:
  `https://api.llama.fi/summary/fees/<slug>?dataType=dailyFees` (monthly sums computed by me;
  October 2026 is a partial month, 17 days). Human page: `https://defillama.com/protocol/<slug>`.
- Catalysts: news and research pages, each linked inline with the date I read it (all read
  2026-10-17 or 2026-10-18 unless noted).
- Where I could not find a figure it says **not found**. Nothing here is estimated silently;
  anything I derived is labelled "computed".

## Summary table

Source for every number in the table: CoinPaprika API as above, read 2026-10-18 02:46 UTC,
except where a cell says otherwise. "2026 high" is the highest daily close since 2026-01-01.

| Token | Price $ | YTD | 90d | 30d | Mcap now | Mcap Jan 1 | USD added YTD | FDV | 2026 high (date) | Demand read |
|---|---|---|---|---|---|---|---|---|---|---|
| **Compute / GPU** |
| Render RENDER | 1.851 | +41% | +25% | +32% | $958M | $679M | +$279M | $1.19B | 2.47 (01-12) | some real, net inflationary |
| Akash AKT | 0.673 | +88% | +25% | +32% | $201M | $102M | +$99M | $261M | 0.899 (05-26) | small real, recovering |
| io.net IO | 0.1534 | +0% | +5% | +19% | $63M | $41M | +$23M | $123M | 0.199 (06-21) | disputed revenue |
| Aethir ATH | 0.00549 | -37% | +20% | +26% | $111M | $138M | -$27M | $231M | 0.0117 (01-06) | largest real revenue, falling |
| Nosana NOS | 0.497 | +160% | +83% | +84% | $41M | $16M | +$26M | $50M* | 0.69 (10-05) | thin, narrative |
| Golem GLM | 0.114 | -49% | +15% | +3% | $114M | $223M | -$109M | $114M | 0.335 (01-13) | legacy |
| Livepeer LPT | 1.688 | -43% | +16% | +25% | $84M | $143M | -$59M | n/a (no cap) | 3.45 (01-24) | small real, falling |
| Theta THETA | 0.192 | -29% | +40% | +5% | $192M | $270M | -$77M | $192M | 0.335 (01-18) | legacy |
| AIOZ | 0.0977 | +9% | +97% | +41% | $126M | $110M | +$16M | n/a (no cap) | 0.131 (09-22) | narrative |
| **Training / inference networks** |
| Bittensor TAO | 268 | +20% | +36% | +18% | $2.57B | $2.14B | +$429M | $5.63B | 349.9 (03-25) | small external revenue vs emissions |
| Chutes SN64 (alpha) | 18.13 | not found | not found | +8.8% (CG) | $112M (CG) | not found | not found | $112M (CG) | ATH $104.42 (2025-06-10, CG) | some real, heavily subsidised |
| Gensyn AI | 0.0175 | n/a (TGE 04-29) | -26% | -6% | $23M | n/a | n/a | $175M | 0.046 (04-30) | narrative, dumped from TGE |
| Venice VVV | 22.32 | **+1261%** | +91% | -10% | $1.09B | $70M | **+$1.02B** | $2.24B* | 32.22 (09-21) | **real revenue wired to burns** |
| NEAR (AI chain + intents) | 4.962 | **+220%** | +158% | +75% | $6.50B | $1.84B | **+$4.66B** | n/a (no cap) | 5.64 (10-12) | mixed: intents volume real, AI mostly narrative |
| ASI/Fetch FET | 0.191 | -6% | +22% | +18% | $499M | $533M | -$34M | $519M | 0.297 (01-06) | narrative |
| **Useful-work PoW** |
| Pearl PRL | 1.799 | n/a (mainnet 04-27) | +458% | +169% | $630M (CG) | did not exist | ~+$630M since launch | $3.80B (CG) | ATH 2.06 (10-14 Paprika; $2.10 elsewhere) | **pure narrative so far** |
| **ZK provers** |
| Succinct PROVE | 0.183 | -55% | -4% | -3% | $36M | $79M | -$43M | $183M | 0.468 (01-10) | narrative, unlock overhang |
| Boundless ZKC | 0.0342 | -70% | -18% | -10% | $7M | $23M | -$16M | $34M* | 0.152 (01-25) | near-zero fees |
| Brevis BREV | 0.0738 | n/a (listed 01-06) | 0% | 0% | $18M | n/a | n/a | $74M | 0.469 (01-07) | narrative |
| Lagrange LA | 0.0548 | -83% | +8% | -18% | $11M | $61M | -$51M | $55M | 0.338 (01-06) | narrative |
| Cysic CYS | 0.124 | -62% | -59% | -6% | $20M | $53M | -$33M | $124M | 1.42 (08-12) | pump and dump |
| zkVerify VFY | 0.00609 | -79% | -3% | -15% | $2M | $10M | -$8M | $6M | 0.0322 (01-16) | dead |
| **Agents / agent payments** |
| Virtuals VIRTUAL | 0.653 | -1% | +7% | +8% | $423M | $429M | -$6M | $653M | 1.117 (01-06) | revenue collapsed after subsidy |
| Kite KITE | 0.115 | +26% | +4% | +9% | $208M | $165M | +$43M | $1.15B | 0.296 (03-08) | narrative |
| elizaOS ELIZAOS | 0.000494 | -90% | +14% | +217% | $4M | $36M | -$32M | $5M | 0.00527 (01-04) | dead cat |
| AIXBT | 0.0185 | -49% | +2% | -5% | $16M | $31M | -$15M | $19M | 0.042 (01-05) | narrative |
| Olas OLAS | 0.0532 | -41% | +104% | +95% | $9M | $16M | -$7M | $25M* | 0.098 (01-06) | narrative bounce |
| **TEE / confidential compute / privacy** |
| Phala PHA | 0.0499 | +37% | +109% | +73% | $43M | $30M | +$13M | $50M | 0.0818 (09-26) | small real (Phala Cloud), weak token link |
| Nillion NIL | 0.0809 | +9% | +131% | +98% | $41M | $22M | +$19M | $81M* | 0.124 (09-24) | narrative |
| Oasis ROSE | 0.00747 | -30% | +38% | +10% | $50M | $71M | -$21M | $75M | 0.0211 (01-30) | narrative |
| Zama ZAMA | 0.0715 (CG) | not found | not found | +43.0% (CG) | $182M (CG) | not found (TGE 02-02) | not found | $810M (CG) | ATH 0.107 (2026-09-23, CG) | narrative (FHE) |
| Aleo ALEO | 0.0518 | -57% | +174% | +237% | $78M | $93M | -$15M | $83M* | 0.132 (01-05) | privacy rotation |
| **Storage / data** |
| Filecoin FIL | 1.059 | -23% | +45% | +31% | $884M | $1.01B | -$130M | $2.08B | 1.58 (01-14) | small fees |
| Walrus WAL | 0.0404 | -66% | +38% | +50% | $50M | $149M | -$98M | $202M | 0.16 (01-18) | tiny fees |
| Arweave AR | 3.795 | +6% | +100% | +47% | $249M | $235M | +$14M | $250M | 4.81 (09-27) | small fees |
| 0G | 0.233 | -76% | +36% | +17% | $50M | $207M | -$157M | $233M* | 1.118 (01-24) | unlock-driven decline |
| Grass GRASS | 0.625 | **+115%** | +74% | +89% | $437M | $132M | **+$305M** | $625M | 0.801 (10-12) | **real revenue, no committed token sink** |
| OpenLedger OPEN | 0.0926 | -42% | -40% | -32% | $20M | $34M | -$14M | $93M | 0.279 (04-29) | narrative, bleeding |
| **Robotics / physical AI** |
| peaq PEAQ | 0.0369 | +19% | +114% | +36% | $95M | $52M | +$43M | $157M* | 0.0444 (10-04) | narrative |
| Geodnet GEOD | 0.253 (CG) | not found | not found | -24.1% (CG) | $117M (CG) | not found | not found | $236M (CG) | ATH 0.374 (2025-01-25, CG) | **real, steadily rising fees** |
| Robotics category (CG) | | | | | $797M total | | | | | |

CG = CoinGecko web page, read 2026-10-18: Pearl https://www.coingecko.com/en/coins/pearl-research,
Chutes https://www.coingecko.com/en/coins/chutes, robotics https://www.coingecko.com/en/categories/robotics.
CoinPaprika returned HTTP 402 (free quota exhausted) for Geodnet, Codec Flow, XMAQUINA, Auki,
Hivemapper and GAIB before I could pull their history, so their YTD is **not found** (current
CoinGecko values for them are in the cross-check below).

### CoinGecko cross-check (supply definitions differ)

CoinGecko markets API (`https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&ids=...`),
one successful call at 2026-10-18 03:01 UTC. Price agrees with CoinPaprika within about 1% for
every token; market cap differs where the two sites count circulating supply differently:
TAO $3.05B (CG) vs $2.57B (Paprika); Walrus $106M vs $50M; 0G $140M vs $50M; Kite $277M vs
$208M; Nosana $50M vs $41M; FET $444M vs $499M. "USD added YTD" in the table uses Paprika on
both ends so it is internally consistent; do not mix it with CG mcaps. CG 30d moves on the same
call: NEAR +54.9%, Venice -12.6%, Pearl +115.5%, Grass +83.8%, Aleo +235.9%, Nillion +91.6%,
Phala +63.8%, Akash +28.9%, Render +23.4%, TAO +13.5%, templar SN3 -11.0%, Targon SN4 +6.1%,
Ridges SN62 -11.1% (1y -91.1%), Acurast -14.5%.

### Biggest USD value added YTD (computed from the table)

1. NEAR +$4.66B (+220%)
2. Venice VVV +$1.02B (+1261%)
3. Pearl PRL about +$630M of market cap created from zero since its 2026-04-27 mainnet
4. Bittensor TAO +$429M (+20%) (on CoinPaprika supply; on CoinGecko supply it is larger)
5. Grass +$305M (+115%)
6. Render +$279M (+41%)

### Biggest value destroyed YTD

0G -$157M (-76%), Filecoin -$130M, Golem -$109M, Walrus -$98M (-66%), Theta -$77M,
Livepeer -$59M, Lagrange -$51M (-83%), Succinct -$43M (-55%). Every ZK prover token is
down 55 to 83% YTD.

### Market backdrop that colours every 90d number

Most tokens in this table printed their 2026 low between 2026-08-02 and 2026-08-18
(CoinPaprika history, column "low" in my pull: Render 08-11, Aethir 08-14, Golem 08-18,
Livepeer 08-18, FET 08-18, FIL 08-18, Walrus 08-14, Aleo 08-16, Olas 08-16, Pearl 08-08).
So the +30 to +100% 90d moves are partly a market-wide bounce from an August bottom, not
project news. Artemis's factor reports had the crypto market factor at -14.2% YTD in May
(https://research.artemis.ai/p/artemis-crypto-factor-model-analysis-caf, read 2026-10-17).
CoinGecko DePIN category: $10.2B total mcap on 2026-10-18
(https://www.coingecko.com/en/categories/depin).

---

## Per-project notes

### Useful-work PoW

**Pearl (PRL)**: the single most relevant comp for pearl-sol and the hottest compute
narrative of the last 60 days.
- Mainnet 2026-04-27; fair launch, no pre-mine, no team/VC allocation; 2.1B cap; about
  331.9M PRL mined by block 122,062 (2026-10-02); about 1.02M PRL/day issuance; about 121.7M
  (37% of supply at the time) minted in the first six days (Datawallet,
  https://www.datawallet.com/crypto/pearl-explained, read 2026-10-17).
- Price path: consolidated $0.25 to $0.34 in August to early September, broke $0.50 on
  2026-09-11, low $0.80 on 2026-09-21, ATH above $1.60 with +67% in 24h on 2026-09-23
  (Cryptonomist, https://en.cryptonomist.ch/2026/09/23/pearl-prl-token-rally/). $0.59 mid-September
  to $2.10 on 2026-10-14, "sevenfold since August" (Parameter,
  https://parameter.io/prl-token-surges-sevenfold-since-august-on-ai-mining-model-despite-code-controversy/).
  CoinPaprika daily low 2026-08-08 $0.2505, ATH $2.06 on 2026-10-14 (API, read 2026-10-18).
  Now $1.80 to $1.81, +30% 7d, +113% 30d (CG) / +169% 30d (Paprika, from 09-17 close).
- Mcap $630M, FDV $3.80B, circ 348.7M, 24h volume $5.5M (CG page, read 2026-10-18). Earlier
  readings: $466.9M on 2026-10-12 (Bybit, per search), $312M to $385M on 2026-10-01 (Datawallet).
- Venues: thin. BigONE, CoinEx, SafeTrade, Ourbit; wrapped WPRL on Uniswap v3. No Binance or
  Upbit listing found (CG page; Parameter).
- Catalysts: Together AI's Gemma-4-31B-it-pearl endpoint on 2026-05-15, priced more than 25%
  below Together's standard rate, the discount funded by expected PRL mining revenue
  (Datawallet). Token2049 Singapore (2026-10-07/08) buzz: Ansem called it the "AI version of
  Bitcoin", No Limit Holdings sponsored a Pearl event on 2026-10-07, Dragonfly's Tom Schmidt
  praised it (KuCoin flash, https://www.kucoin.com/news/flash/pearl-gains-attention-at-token2049-as-ai-version-of-bitcoin;
  CoinStats summary, https://coinstats.app/ai/a/latest-news-for-pearl-2). NVIDIA Inception
  membership cited around July (CoinGabbar, per search).
- Real demand: **almost none measurable on chain.** DefiLlama "Pearl" chain fees: $768.67 over
  the last 30 days, $2,854 over the last year (https://defillama.com/protocol/pearl, slug
  `pearl`, gecko_id pearl-2, read 2026-10-18). Basu's June 2026 preprint "The Usefulness Gap in
  Proof-of-Useful-Work" found most June mining produced no AI output, mined random matrices and
  got 44 shares accepted by a pool, and attributed a 38% rise in budget Vast.ai GPU rental prices
  to Pearl mining (Datawallet; Parameter). Weinstein acknowledged that letting miners choose
  inputs "will always allow spam mining" (Parameter). Four consensus changes in four months
  (MoE hard fork June, dense-only soft fork July, rank-penalty soft fork August, salted
  noise-seed hard fork August) (Datawallet). Miner economics: RTX 5090 daily revenue fell from
  about $33.80 to $17.19 within weeks in May; official pool fee 20% (Hashrate Index via Datawallet).
- Verdict: **pure narrative plus a fair-launch PoW scarcity story.** It held most of its gains
  so far (2026-10-18 price is 12% under ATH) despite about $1.8M/day of new supply at current
  price (computed: 1.02M PRL x $1.80).

**Others in PoUW**: Nockchain claims a roadmap to merge-mine with Pearl (single commentary
source, unconfirmed: https://lordofpearls.xyz/blog/top-pouw-chains-2026). Qubic integrated
Dogecoin mining 2026-04-01 (per search summary). I found no current price data for either.

### Decentralized GPU / compute

**Render (RENDER)**: +41% YTD (+$279M); 2026 high 2026-01-12, low 2026-08-11 ($1.25), back
to $1.85. CG mcap $960.5M, 30d +23.4% (DePIN category page, read 2026-10-18).
- Fees, DefiLlama `render-network-bme` (monthly, $k): 2025-09 477, 2025-12 178, 2026-01 127,
  2026-02 98, 2026-04 232, 2026-05 91, 2026-06 85, 2026-07 164, 2026-08 120, 2026-09 232,
  2026-10 (17 days) 345. 1y total $2.04M. Trend: flat to up in Q3/Q4 2026, still tiny vs mcap.
- Burns vs emissions: May 2026 burns $87,194 (45,150 RENDER), June $106,650 (63,281 RENDER);
  monthly emissions about 492,132 RENDER; YTD net flow +2.49M RENDER, i.e. inflationary;
  emissions step down to 380,284/month in December 2026 (@NEWRenderBurn,
  https://x.com/NEWRenderBurn/status/2072264132645064949).
- Catalysts: Octane 2026 production on the network; "negative GPU supply" claim (weak sources,
  https://hoge.gg/render-paradox-booming-gpu-grid-sinking-token-2026/). No single 2026 catalyst
  found for the January high.
- Verdict: real but small usage; price follows sector beta.

**Akash (AKT)**: +88% YTD (+$99M), high 2026-05-26.
- AEP-76 Burn-Mint Equilibrium went live with Mainnet 17 on 2026-03-23 (Proposal 318, 99.7%
  yes): tenants burn AKT to mint ACT, a USD-pegged compute credit (https://ownyourmind.ai/projects/akash/;
  proposal https://github.com/orgs/akash-network/discussions/1034). Burns still below inflation.
- Fees, DefiLlama `akash` ($k/month): 2025-07 334, 2025-12 157, 2026-01 104, **2026-02 64
  (bottom)**, 2026-04 104, 2026-05 164, 2026-06 220, 2026-07 231, 2026-08 288, 2026-09 288.
  1y $2.02M. Trend: **4.5x recovery Feb to Sep**, one of the few compute tokens with rising
  on-chain fees. Messari Q1 2026: lease revenue $253,250, GPU utilisation 33.7% (via search
  summary of https://ownyourmind.ai/tokenomics/render-vs-akash-vs-ionet/).
- May rally attributed to AI-model usage surge and AI rotation, BME is a supporting story
  (https://letsdatascience.com/news/akash-networks-ai-model-spurs-akt-price-rally-88f87e0c).
  Price $0.78 on 2026-10-05, +17% week (https://hoge.gg/akash-vs-io-net-render-depin-gpu-race-2026/).

**io.net (IO)**: flat YTD. Incentive Dynamic Engine started 2026-06-11: at least 50% of
post-payout revenue received in IO is burned, target 12M IO burned in a year; $8M enterprise
contract; 4B inference tokens/day claimed (CoinDesk press release,
https://www.coindesk.com/press-release/2026/06/11/io-net-to-burn-up-to-12m-tokens-as-network-closes-usd8m-deal-and-hits-4-billion-daily-ai-tokens).
Independent review puts revenue near $12.5M annualised and says the burn is emission-funded
(https://ownyourmind.ai/projects/io-net/). Not on DefiLlama fees. Burn announcement did not
move the price durably (high 2026-06-21, then fell).

**Aethir (ATH)**: -37% YTD despite the largest verifiable revenue in the sector.
- Fees, DefiLlama `aethir` ($M/month): 2025-09 13.9, 2025-10 11.2, 2025-12 8.0, 2026-01 7.2,
  2026-03 6.4, 2026-05 3.8, 2026-07 3.1, 2026-08 5.2, 2026-09 4.4. 1y $63.6M. Trend: **down
  about 70% from the Q3 2025 peak**.
- 2025 buybacks: $45M DAT buyback via Nasdaq-listed POAI (Oct 2025); Q3 2025 revenue $39.8M
  and a $140M buyback plan (https://www.kucoin.com/news/flash/aethir-reveals-39-8m-q3-revenue-and-140m-ath-token-buyback-plan).
  42B supply. Lesson: revenue falling + big supply beats buyback headlines.

**Nosana (NOS)**: +160% YTD from a tiny base ($16M to $41M), high 2026-10-05. Solana GPU
inference marketplace; September 2026 blog says 4090/5090 demand exceeds supply
(https://nosana.com/blog/nosana-monthly-september-2026/). No fee data on DefiLlama;
revenue **not found**. Treat as narrative beta.

**Golem, Livepeer, Theta**: all -29 to -49% YTD. Livepeer fees ($k/month, DefiLlama `livepeer`)
fell from 101 (2026-01) to 23 (2026-07), 63 in 2026-09. Legacy compute tokens did not
participate.

### Decentralized training / inference

**Bittensor (TAO)**: +20% YTD on CoinPaprika (+$429M); CoinGecko shows $3.05B mcap,
11.34M circulating, FDV $5.65B, 30d +14.9% (CoinGecko API call that succeeded at the start of
the session, 2026-10-18 02:30 UTC).
- Catalysts: first halving December 2025 (1 to 0.5 TAO/block)
  (https://www.dlnews.com/articles/defi/bittensor-halving-seen-to-boost-tao-price-and-starve-zombie-subnets/).
  Templar (SN3) finished Covenant-72B on 2026-03-10 (72B params, about 1.1T tokens, 70+
  permissionless nodes); TAO up about 46% by 2026-03-18 and nearly doubled in March to about
  $317, plus an Upbit listing (https://cryptonews.com/news/bittensor-tao-surges-46-covenant-72b-subnet-explosion/;
  https://blockonomi.com/bittensor-tao-price-surges-100-in-march-following-major-network-developments/).
  2026 high $349.9 on 2026-03-25. Then on 2026-04-10 Covenant AI publicly exited Bittensor
  accusing co-founder Jacob Steeves of centralised control
  (https://ownyourmind.ai/tokenomics/templar-bittensor-covenant-exit/). Grayscale filed an amended
  S-1 for a TAO ETF on 2026-04-03; no approval found; trust sold 253,100 shares on 2026-10-13
  (per search summary). TAO +37% in September per Grayscale Research (per search summary).
- dTAO: live since early 2025, each subnet has an alpha token whose AMM price sets its emission
  share. A June 2026 change reportedly gated emissions to the top ~30 subnets by price (single
  source, unconfirmed: https://ownyourmind.ai/tokenomics/bittensor-halving-what-tao-holders-need-to-know/).
- Subnet alphas (CoinGecko category page, read 2026-10-18, https://www.coingecko.com/en/categories/bittensor-subnets):
  category $1.24B. Top: lium SN51 (GPU rental) $144M, 30d 40.1%; Chutes SN64 $112M;
  Targon SN4 $84M; EfficientFrontier SN53 $60M (30d 65.2%); templar SN3 $37.8M. Note the top
  alpha is a GPU-rental subnet: compute-for-pay is what the market values inside Bittensor too.
- Real demand: Chutes external revenue estimated $1.3M to $2.4M annualised vs about $52M/year
  of TAO emissions, a 22:1 to 40:1 subsidy ratio (Pine Analytics via
  https://ownyourmind.ai/tokenomics/chutes-bittensor-revenue-machine/). DefiLlama `chutes`
  fees: about $440k to $495k/month Jan to May 2026, $376k to $388k Jun to Aug, then only $74k
  in September (series may have stopped updating). 1y $3.69M.
- Verdict: the one decentralized-AI token with large mcap that held up; demand is mostly
  emissions-subsidised; catalysts were technical milestones (Covenant-72B) and listings/ETF filings.

**Gensyn (AI)**: TGE 2026-04-29, listed on five exchanges at once, ATH about $0.10 on TGE day,
down about 68% in three days (https://ownyourmind.ai/projects/gensyn/;
https://www.coingabbar.com/en/price-prediction/gensyn-ai-listed-45percent-crashed-in-hours).
Now $0.0175, $23M mcap vs $175M FDV; 54.6% insiders; training mainnet still not live (Aug 2026
review). Classic low-float/high-FDV dump.

**Prime Intellect, Nous Psyche, Pluralis**: no liquid tokens found. Prime Intellect is
RL post-training focused (INTELLECT-3); Nous Psyche coordinates on Solana; Pluralis runs
"protocol learning" and launched Agora in 2026 (secondary source:
https://www.spheron.network/blog/decentralized-llm-training-pluralis-prime-intellect-nous-psyche/).
No price or revenue figures exist to report.

**Venice (VVV)**: the best performer in this whole set, **+1261% YTD, +$1.02B**.
- From a December 2025 low of $0.92 to an ATH $34.53 on 2026-09-21, now $22.32
  (https://cryptobriefing.com/venice-ai-vvv-token-surges-record-high/; CoinPaprika).
- Mechanism: stake VVV and lock to mint DIEM (each DIEM = $1/day of Venice API credit);
  from July, $5 of every $100 of API credit purchases buys back and burns VVV; subscription
  burns raised ($2 Pro, $5 Pro+, $10 Max); emissions cut sharply
  (https://www.kucoin.com/news/flash/venice-s-vvv-surges-on-near-integration-but-can-buybacks-outpace-emissions).
  Founder claimed $100M annualised revenue run rate on 2026-08-17 (per search summary,
  https://www.digitalassetradar.com/guides/vvv-token-explained/).
- DefiLlama `venice` fees ($k/month): 2025-12 64, 2026-01 93, 2026-03 112, 2026-05 236,
  2026-07 448, 2026-08 697, **2026-09 833**, 2026-10 (17 days) 768. **13x growth Dec to Sep**,
  the steepest rising on-chain fee line in this report. Venice runs private TEE models on Phala
  (Phala note below).
- Verdict: **real paying users + a direct revenue-to-token sink + supply cut**. The clearest
  "real demand" winner of 2026. Concentration risk: one source says top 100 wallets hold 98%.

**NEAR**: +220% YTD, +$4.66B, the largest dollar gain in AI/infra.
- Catalysts: Bitwise spot NEAR ETF launched 2026-09-29; confidential perps launched mid-September
  (NEAR +30% from about $2); NEAR Intents recovered a $3.8M exploit on 2026-10-02; proposal on
  2026-10-02 to cut max issuance from 2.5% to 1.6% over 24 months; Intents cumulative $33B+,
  confidential TVL $160M+; Q3 gain 198%
  (https://www.cryptotimes.io/2026/10/06/near-price-jumps-128-in-a-month-as-ai-intents-and-hack-recovery-align/;
  https://www.thecoinrepublic.com/2026/10/08/near-crypto-price-surged-198-in-q3-is-ai-the-next-catalyst/;
  https://cryptodaily.co.uk/2026/10/near-price-prediction-6-rally-issuance-cut).
- Fees: DefiLlama `near-intents` 30d $7.56M, 1y $52.6M; chain fees `near` only $224k 30d
  (https://api.llama.fi/overview/fees, read 2026-10-18). Futures volume 5.5x spot, OI $1.59B
  on 2026-10-06 (CoinGlass via cryptotimes): leverage-driven.
- Verdict: real intents/bridge volume plus ETF flows plus leverage; the "AI chain" label is
  narrative (Bitwise did not claim AI drove it).

**ASI/Fetch (FET)**: -6% YTD; no catalyst of note found.

### ZK prover markets

All down hard. **Succinct PROVE** -55% YTD: Optimism chose Succinct for OP Stack ZK proofs on
2026-02-12 (per search summary), but a large unlock on 2026-08-05 (sources disagree: 100M,
208.33M or 312.49M PROVE;
https://cryptoslate.com/a-massive-100-million-prove-tokens-unlock-today-but-razor-thin-exchange-liquidity-reveals-a-market-completely-unprepared-for-a-51-supply-shock/)
sent it to an ATL $0.143 on 2026-08-14; more unlocks of 19.45M PROVE on 2026-11-05 and
2026-12-05 (https://dropstab.com/coins/succinct). **Boundless ZKC** -70%: most proof auctions
clear at zero fee, provers are paid in points redeemable for ZKC
(https://medium.com/@CFrontier_Labs/the-prover-network-landscape-a-comparative-analysis-of-succint-and-boundless-networks-b1c362b4e67d);
pitched repurposing its about 4,000 GPUs to AI inference (CMC AI page, unverified). **Brevis,
Lagrange, Cysic, zkVerify** -62% to -83% YTD or since listing; Cysic pumped to $1.42 on
2026-08-12 then fell 91% (CoinPaprika). Revenue: not found on DefiLlama for any prover network.
Verdict: **narrative with no paying demand at token level, low float + unlocks.** The worst
category of 2026.

### AI agents and agent payments

**x402 (protocol, no token)**: Chainalysis (2026-10-14 report) counts 138M+ x402 payments on
Base through September 2026; 165M+ and over $50M including Solana; 87%+ of payments between
$0.0001 and $0.10; TRM Labs estimates only 0.6% to 7.5% of screened x402 commerce is genuinely
agent-driven (https://www.cryptopolitan.com/base-logs-138-million-x402-payments-as-ai-agent-revenue-draws-scrutiny/;
https://www.kucoin.com/news/flash/coinbase-s-x402-protocol-logs-138-5m-transactions-as-chainalysis-tracks-ai-agent-payments).
The x402 tokens (PING, PAYAI) ran 20x and 1,900% in late 2025 and then fell 80% to 98% from
ATH; daily x402 transactions fell from about 3.8M (Nov 2025) to tens of thousands (Odaily,
https://www.odaily.news/en/post/5210138; Cointelegraph,
https://cointelegraph.com/news/why-the-x402-protocol-didn-t-fade-after-the-ping-hype-and-what-s-driving-the-second-wave).
Verdict: protocol adoption is real (Coinbase, Cloudflare) but **value did not accrue to
"x402 tokens"**.

**Virtuals (VIRTUAL)**: -1% YTD. ACP platform fees were $315k to $345k/month during the
Revenue Network subsidy (five epochs 2026-02-11 to 2026-03-23, $1.02M paid out), then fell
98.6% to $4,935 in April and $28 in May (https://chainward.ai/decodes/virtuals-top-dog).
DefiLlama `virtuals-protocol` fees ($k/month): 2025-10 4,798, 2026-01 1,104, 2026-04 258,
2026-07 2,234 (spike), 2026-09 624. aGDP self-reported $479M to $482M, counts trading turnover.
Verdict: **subsidised usage evaporated when the subsidy stopped.**

**elizaOS, AIXBT, Olas**: -90%, -49%, -41% YTD; 30d bounces (+217%, -5%, +95%) are off
August lows. **Kite (KITE)**: +26% YTD, ATH 2026-03-08, $33M from PayPal Ventures and Coinbase
Ventures, listed on Binance/Coinbase/Upbit (https://cryptoslate.com/coins/kite/); FDV $1.15B vs
$208M mcap; no revenue found.

### TEE / verifiable inference / confidential compute

A clear rotation in the last 90 days (all computed from CoinPaprika): Phala +109% 90d,
Nillion +131%, Aleo +174% (+237% 30d), Oasis +38%, plus NEAR's confidential perps and
Zcash above $1,300 (https://coindcx.com/blog/crypto-highlights/top-privacy-tokens). 21Shares
(2026-09-22) said privacy tokens grew nearly 5x in a year
(https://www.21shares.com/en-us/insights/crypto-privacy-tokens-sector-outlook-2026).
- **Phala (PHA)**: +37% YTD; TEE GPUs (H100/H200/B200 + Intel TDX), H200 at $3.50/hr on Phala
  Cloud; Venice runs private TEE models on Phala; partially verified $2M+ ARR and 398 paid
  users, mostly in the centralised Phala Cloud, token link unclear
  (https://ownyourmind.ai/projects/phala/). Spike to $0.078 on 2026-09-26 tied to rotation into
  AI/confidential tokens (OKX/CMC via search).
- **Nillion (NIL)**: Covenants on Ethereum launched in September (encrypted orders/strategies)
  (https://mudrex.com/learn/best-privacy-tokens-in-2026/). No revenue found.
- **Zama (ZAMA)**: FHE; sealed-bid auction 2026-01-21 to 01-24 at $0.05, TGE 2026-02-02, fell
  to $0.01676 on 2026-02-12, about $0.06 by end of July
  (https://phemex.com/academy/what-is-zama-zama-and-why-fully-homomorphic-encryption-token-surged).
  Now $0.0715, mcap $182M, FDV $810M, 30d +43.0%, ATH $0.107 on 2026-09-23 (CoinGecko markets
  API, read 2026-10-18 03:01 UTC), i.e. it ran with the September confidential-compute rotation.
- Verdict: narrative rotation; Phala is the only one with a disclosed (small) paying customer base.

### Storage / data

- **Filecoin (FIL)** -23% YTD (-$130M); DefiLlama `filecoin` fees $43k to $340k/month,
  falling, 1y $1.96M.
- **Walrus (WAL)** -66% YTD; DefiLlama `walrus-protocol` fees $2k to $24k/month, 1y $116k.
  Fixed USD pricing since May 2026 ($0.023/GB-month), Walrus Console launched early October
  2026, Quilt batching reduced revenue (https://decrypt.co/362558/all-to-play-for-walrus-hits-450tb-of-data-stored-amid-renewed-ai-push).
- **Arweave (AR)** +6% YTD but +100% 90d; fees 1y $323k.
- **0G** -76% YTD, worst of the AI-L1s.
- **Grass (GRASS)**: +115% YTD (+$305M), ATH 2026-10-12. Sells scraped web data to AI labs:
  $33M revenue claimed for 2025 (another source says $17M), guidance $65M to $75M for 2026
  (https://cryptobriefing.com/grass-33m-revenue-governance-vote/;
  https://milkroad.com/news/grass-revenue-is-real-but-token-sink-is-broken/). July 7 governance
  vote on revenue capture; sources disagree whether holders got a claim; no committed buyback;
  Multicoin bought in. Not on DefiLlama fees. Verdict: **real off-chain revenue, weak token
  link, and the market paid for it anyway.**

### Robotics / physical AI DePIN

CoinGecko robotics category $797M total (read 2026-10-18), led by VIRTUAL (tagged robotics),
GEOD $117M, PEAQ $95M, Auki $33M, OpenRoboto SN80 $16M, Fabric ROBO $15M, XMAQUINA $9.7M,
Codec Flow $6.2M (https://www.coingecko.com/en/categories/robotics). The category page shows
30d moves without signs; the CoinGecko markets API (signed, read 2026-10-18 03:01 UTC) gives
30d: PEAQ +29.5%, Auki +12.0%, XMAQUINA +29.3%, GEOD -24.1%, Codec Flow -14.9%, GAIB -47.6%,
Hivemapper +103.9%.
peaq +19% YTD, +114% 90d. **Geodnet** is the standout on fundamentals: DefiLlama `geodnet` fees
rose every month from $342k (2025-07) to $944k (2026-08), $914k (2026-09), 1y $9.51M. GEOD YTD
not found (Paprika quota). Verdict: small sector, mostly narrative except Geodnet.

### BTC miners pivoting to AI hosting (equity bridge)

- Contracts: Core Scientific / CoreWeave about $10.2B over 12 years; IREN / Microsoft about
  $9.7B over 5 years; TeraWulf / Fluidstack (Google-backed) about $9.5B over 25 years; IREN /
  Nvidia $3.4B 5-year managed cloud (May 2026); Cipher / AWS about $5.5B, 300 MW (Nov 2025);
  TeraWulf / Anthropic 401 MW 20-year lease (date and value disputed)
  (https://oneminers.com/blogs/news/bitcoin-miners-ai-deals-ranked-2026). CoinShares: over $70B
  of cumulative AI/HPC contracts across public miners; about $1.1B/yr actually billed vs $100B+
  backlog (per search summary of the same and https://www.bbae.com/blog/the-2026-bitcoin-miner-to-ai-data-center-boom/).
- Price: as of 2026-07-30 Cipher +53% YTD and TeraWulf +56% YTD
  (https://247wallst.com/investing/2026/07/30/terawulf-and-cipher-digital-are-up-50-in-2026-while-iren-lags-behind-is-it-time-to-buy-iren-for-a-catch-up-trade/);
  IREN +22.19% YTD on 2026-09-25; then IREN closed $26.99 on 2026-10-16, -11.19% on the day,
  65% below ATH; Riot $13.72, -55% from its 2026 high; Core Scientific, TeraWulf, Cipher, HIVE
  also slumped in October (https://www.benzinga.com/markets/equities/26/10/62428557/riot-platforms-iren-and-mara-drop-further-as-bitcoin-miners-wrestle-with-the-ai-headwinds).
  Current YTD for the group **not found**. Debt is heavy (IREN $3.7B converts, TeraWulf $5.7B).
- Related new market: GPU-hour price indices and futures. CME planned H100/B200 rental futures
  (listing 2026-10-05, under regulatory review), Bitget launched H100USDT/B200USDT perps,
  Ornn OCPI-H100 $3.13/GPU-hour on 2026-10-15
  (https://financefeeds.com/cme-is-about-to-list-futures-on-the-price-of-renting-a-gpu/;
  https://data.ornn.com/markets/bloomberg; https://www.bitget.com/blog/articles/gpu-compute-perpetuals-ai-rental-prices).
  Compute is becoming a priced commodity; this is the benchmark a pearl-sol job price will be
  compared to.

### New narratives that ran recently (last 3 to 6 months)

1. **Proof of useful work / "AI Bitcoin"** (Pearl): 7 to 8x Aug to Oct, $630M mcap from a fair launch.
2. **Revenue-burn AI apps** (Venice): +1261% YTD.
3. **Confidential compute / privacy** (NEAR confidential perps, Aleo, Phala, Nillion, Zcash): Sep to Oct.
4. **AI data sales** (Grass): +115% YTD.
5. **GPU compute as a financial commodity** (CME/ICE futures, Bitget perps, Ornn indices; Jump
   Trading planning market making, https://tokenpost.com/news/business/33946).
6. **Rising GPU-rental subnets inside Bittensor** (lium SN51 is now the largest alpha).

---

## Patterns: what actually drove value accrual to the token

1. **A direct, mechanical revenue-to-token sink beat everything else.** Venice wired API sales
   into buy-and-burn plus stake-to-get-credit (DIEM), with fees up 13x Dec to Sep on DefiLlama,
   and was the best performer (+1261%). Akash added burn-mint (March) and is one of the few
   compute tokens up YTD (+88%) with fees rising 4.5x from February. Render has burn-mint too
   but emissions still exceed burns, so it only tracked beta (+41%).
2. **Revenue alone, with no sink, did not protect the token when revenue fell.** Aethir had the
   biggest verifiable revenue ($63.6M over 1y) but fees fell about 70% from the 2025 peak and the
   token fell 37% YTD despite buyback headlines. Grass is the exception: real revenue and no
   committed sink, but revenue was growing and the story was simple.
3. **Subsidised usage vanishes when the subsidy stops.** Virtuals ACP fees fell 98.6% the month
   the Revenue Network ended. Chutes runs at a 22:1 to 40:1 emissions-to-revenue ratio. Pearl's
   Together AI discount is funded by mining emissions. Anything pearl-sol sells that is priced
   under cost by emissions will look like Virtuals the day emissions drop, unless real buyers
   stay.
4. **Fair launch + PoW scarcity + a one-line story was enough for a narrative run, without
   demand.** Pearl has about $770/month in chain fees and a published "usefulness gap" critique,
   yet went from $0.25 (2026-08-08) to $2.06 (2026-10-14) and held most of it. Drivers were
   KOLs (Ansem "AI Bitcoin"), Token2049 timing, Dragonfly praise, no VC unlock overhang, and
   thin CEX float. This is the narrative pearl-sol would ride; it is also the critique pearl-sol
   must answer (bind mining to paid jobs, so "useful" is checkable).
5. **Low float, high FDV and unlocks destroyed value regardless of tech quality.** Every ZK
   prover token is down 55 to 83% YTD (Succinct unlock 2026-08-05, Boundless zero-fee auctions);
   Gensyn fell about 68% within three days of TGE and about 83% from ATH; 0G -76%. A token with
   no team unlock schedule (Pearl) did not suffer this.
6. **Protocol adoption did not accrue to "ecosystem tokens".** x402 grew to 138M+ payments while
   PING/PAYAI fell 80 to 98%. The value went to the stablecoin rails and the payment-taking apps.
   For pearl-sol, an x402 endpoint is a distribution channel, not a token thesis, unless each
   payment buys the token (spec section 1, job 3, already does this).
7. **Technical milestones produced sharp, short pumps.** Covenant-72B took TAO up about 100% in
   March; the team exiting Bittensor in April gave much of it back. A milestone needs follow-on
   usage to hold.
8. **Listings, ETFs and leverage amplified the biggest dollar moves.** NEAR's +$4.66B came with
   a Bitwise ETF (2026-09-29), an issuance cut proposal and futures volume 5.5x spot. TAO's
   March run included an Upbit listing and Grayscale filings.
9. **The August 2026 bottom flatters every 90d number.** Most names bottomed 2026-08-02 to
   2026-08-18; +30 to +100% 90d moves across unrelated tokens are partly beta. Use YTD and fee
   trends, not 90d price, to separate winners from the bounce.
10. **What held vs what dumped.** Held: Venice (still 13x YTD after -35% from ATH), Grass (at
    ATH), Pearl (12% off ATH), NEAR (12% off 2026 high), Akash (25% off May high). Dumped: Cysic
    (-91% from 2026-08-12 high), Gensyn (TGE), x402 tokens, elizaOS, OpenLedger, Kite (-61%
    from March ATH), Render (-25% from January high).

### What this suggests for pearl-sol (brief)

- The market is currently paying for exactly the Pearl story (useful PoW, fair launch). The
  open flank is the "usefulness gap": pearl-sol's design (blocks bound to posted, paid jobs;
  results committed back to Solana) is the direct answer and should be the headline.
- Token value accrual should be mechanical and visible on DefiLlama-style dashboards from day
  one: storefront SOL/USDC payments buying the token (Venice/Akash pattern), with the monthly
  fee line rising. Avoid emissions-funded discounts as the main demand source (Virtuals/Chutes).
- Keep insider unlocks at zero or tiny (pump.fun launch already helps), since unlock overhang
  is what killed the ZK prover and TGE-launched compute tokens this year.
- Agent Credit and x402 are distribution, not the token's reason to exist; value goes where the
  payments settle.
