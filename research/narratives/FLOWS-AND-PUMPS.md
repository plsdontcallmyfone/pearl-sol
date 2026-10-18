# Where the money is flowing (and what actually pumped), as of 2026-10-17

Research slice for ~/pearl-sol. Read date for every number below is 2026-10-17 (DefiLlama and CoinGecko API pulls were served with 2026-10-18 UTC timestamps) unless a different date is given. No numbers were modelled or filled in. Where data could not be reached, it says so.

Market backdrop, so the percentages make sense: BTC is $81.7K, -35% from its $126,198 ATH (2025-10-06) and -24% over 1y (CoinGecko markets API, https://api.coingecko.com/api/v3/coins/markets). H1 2026 was a drawdown. Q3 2026 was a relief rally: total market cap +39.6% QoQ to about $3.0T but still -4.0% YTD, ETH +71% in Q3, perp OI back above $100B (CoinGecko Q3 2026 report, https://www.coingecko.com/research/publications/2026-q3-crypto-industry-report, updated 2026-10-15). Total DeFi TVL is down YTD but up over the last 90 days.

Method notes:
- DefiLlama free API: chain TVL history (`api.llama.fi/v2/historicalChainTvl/{chain}`), stablecoins (`stablecoins.llama.fi/stablecoincharts/{chain}`), revenue (`api.llama.fi/overview/fees?dataType=dailyRevenue`), DEX volume (`api.llama.fi/overview/dexs/solana`). The bridges, derivatives-volume and categories endpoints now return "Upgrade to the paid API plan", and defillama.com pages are behind Cloudflare, so **bridge net flows by chain could not be pulled**. Chain TVL and stablecoin deltas are used as a stand-in.
- CoinGecko free API: category market caps come from `/coins/categories`. That endpoint has no history, so **category % changes are my computation**: the market-cap-weighted price change of each category's top 100 constituents by current market cap (`/coins/markets?category=...&price_change_percentage=7d,30d,200d,1y`). This has survivorship bias and is not an official index. A true YTD and 90d window is not available on the free API. "200d" (about 2026-04-01 to now) stands in for "last 6 months", and "30d" is the recent window.
- Kaito, Cookie, Artemis, Token Terminal and Dune dashboards were not reachable as structured data (Firecrawl is down, and those pages are paywalled or JS-only). Where a news article cites them, the article URL is given.

---

## Summary tables

### A. Chain TVL (DefiLlama, DeFi TVL excluding CEX)

Source: https://api.llama.fi/v2/historicalChainTvl/{chain}, read 2026-10-17.

| Chain | TVL now | TVL 2026-01-01 | YTD | 90d | 180d |
|---|---|---|---|---|---|
| All chains | $88.5B | $114.4B | -23% | +19% (from $74.4B) | +5% |
| Ethereum | $49.5B | $68.3B | -27.5% | +20.0% | +6.4% |
| Base | $6.12B | $4.45B | **+37.5%** | **+33.6%** | +44.3% |
| Solana | $5.89B | $8.16B | -27.8% | +21.3% | +6.6% |
| Tron | $5.58B | $4.39B | +26.9% | +16.3% | +10.6% |
| BSC | $5.38B | $6.60B | -18.4% | +9.5% | -1.2% |
| Bitcoin | $4.36B | $6.79B | -35.8% | +3.1% | -14.7% |
| Arbitrum | $1.40B | $2.89B | -51.6% | +14.1% | -17.8% |
| Robinhood Chain | $1.05B | n/a (launched 2026-07-01) | n/a | **+298%** | n/a |
| Hyperliquid L1 | $1.01B | $1.42B | -28.8% | -21.6% | -28.5% |
| Monad | $0.98B | $0.25B | **+297%** | +39.8% | +193% |
| Polygon | $0.68B | $1.17B | -41.9% | -24.6% | -45.2% |
| Avalanche | $0.59B | $1.15B | -48.7% | +37.6% | -9.6% |
| Plasma | $0.56B | $1.29B | -57.0% | -17.2% | -55.7% |
| Arc (Circle) | $0.53B | n/a (TVL from 2026-09-17) | n/a | new | n/a |
| Sui | $0.49B | $0.90B | -45.3% | +10.4% | -16.5% |

YTD gainers: Monad, Base, Tron, plus the two new chains (Robinhood Chain, Arc). Everything else is down YTD and recovering over 90d.

### B. Stablecoin supply by chain (DefiLlama, USD-pegged)

Source: https://stablecoins.llama.fi/stablecoincharts/{chain} and /stablecoincharts/all, read 2026-10-17.

| Chain | Now | YTD | 90d |
|---|---|---|---|
| **All** | **$304.6B** | **+0.3%** (from $303.7B) | +1.2% |
| Ethereum | $144.8B | -12% | -3% |
| Tron | $94.5B | +16% | +5% |
| Solana | $15.8B | +11% | +3% |
| BSC | $14.3B | +3% | -3% |
| Hyperliquid L1 | $7.02B | **+49%** | +12% |
| Base | $5.21B | +14% | +8% |
| Arbitrum | $3.86B | -14% | +3% |
| Polygon | $3.32B | +14% | -15% |
| X Layer | $1.50B | from ~$0.03B | -23% |
| Plasma | $1.34B | -31% | +36% |
| Robinhood Chain | $1.09B | new | +175% |
| Monad | $0.69B | +74% | +15% |
| Arc | $0.66B | new | new |

Stablecoin supply has been flat all year: the "new money" story is rotation between chains, not net issuance. Money is leaving Ethereum L1 and Arbitrum and going to Tron, Hyperliquid, Solana, Base, Monad and the new chains. **Bridge net flows: not available** (DefiLlama bridges API is paywalled; https://bridges.llama.fi returns "Upgrade to the paid API plan").

### C. CoinGecko category market cap and performance

Category market cap: https://api.coingecko.com/api/v3/coins/categories (updated 2026-10-18T02:31Z). The % columns are my mcap-weighted computation over each category's top 100 constituents (see method notes). "YTD" and "90d" are not available on the free API.

| Category | Mkt cap (USD) | 30d | ~6mo (200d) | 1y | Largest constituents (200d) |
|---|---|---|---|---|---|
| Privacy | $55.04B | -1.4% | **+99.2%** | +43.8% | ZEC $19.4B +358%, XMR $11.2B +77% |
| Privacy Coins | $31.10B | -12.3% | **+175.9%** | +196.5% | ZEC, XMR |
| Meme | $28.84B | -8.4% | -17.2% | -54.1% | DOGE -14%, PUMP +159%, M -68% |
| Artificial Intelligence | $24.31B | +25.5% | +41.4% | -25.8% | NEAR +315%, TAO -12%, VVV +239% |
| Zero Knowledge | $23.65B | not fetched (rate limited) | n/a | n/a | n/a |
| Perpetuals | $23.61B | -7.4% | **+91.0%** | +37.8% | HYPE $17.5B +115%, ASTER +5%, LIT +257% |
| Oracle | $10.37B | +6.9% | +36.9% | -30.6% | LINK +36%, PYTH +99% |
| DePIN | $10.16B | +17.8% | -0.1% | -41.6% | TAO -12%, RENDER +4%, FIL +27%, GRASS +122% |
| x402 Ecosystem | $10.13B | +4.9% | +37.3% | -33.1% | dominated by LINK; UB +407%, PEAQ +174% |
| Data Availability | $7.66B | +51.8% | +253.5% | +44.9% | NEAR, TIA +114% |
| Prediction Markets | $6.65B | -26.8% | +11.1% | +114.9% | RAIN $6.56B is about 99% of the category |
| Real World Assets | $76.16B | +5.6% | +10.7% | -12.8% | FIGR_HELOC, LINK, QNT +180% |
| AI Agents | $3.44B | -2.2% | -1.6% | -20.5% | VVV, FET -18%, VIRTUAL -1%, KITE -25% |
| AI Applications | $3.38B | +14.2% | +120.2% | -31.0% | VVV, AKE, Q (Quack AI) +1173% |
| Solana Meme | $3.04B | -11.6% | -17.5% | -66.6% | TRUMP -42%, BONK -50% |
| Tokenized Stocks | $2.47B | -5.3% | +6.8% | -24.8% | xStocks, Backed |
| Storage | $1.98B | +21.7% | +9.8% | -41.9% | FIL, AR +118% |
| AI Framework | $1.87B | -2.7% | -1.0% | -74.4% | VIRTUAL, UB, EIGEN |
| AI Agent Launchpad | $1.24B | n/a | n/a | n/a | n/a |
| Pump.fun Ecosystem | $1.20B | -7.3% | -11.5% | -59.0% | FARTCOIN -14% |
| Robinhood Chain Meme | $0.47B | **-67.8%** | +38.6% | n/a | CASHCAT, AI, HOODINU |
| DeSci | $0.35B | +10.2% | +21.0% | -54.3% | TRAC, BIO |
| Layer 1 (reference) | $2,333.97B | +4.4% | +17.9% | -27.4% | BTC +20%, ETH +14%, SOL +24% |

Raw data: https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&category={id}&per_page=100&price_change_percentage=7d,30d,200d,1y, read 2026-10-17.

### D. Fastest pumps, last ~6 months, tokens with market cap of at least $100M today

CoinGecko markets API, top 750 by market cap, read 2026-10-17 (https://api.coingecko.com/api/v3/coins/markets?order=market_cap_desc&per_page=250&page=1..3&price_change_percentage=7d,30d,200d,1y). A 200d change for a token that listed less than 200 days ago is measured from its listing.

| Token | Mcap now | 200d | Peak (date) | Now vs peak | Catalyst (source) | Verdict |
|---|---|---|---|---|---|---|
| BTW Bitway | $3.52B | +11,436% | $2.77 (2026-10-14) | -16% | Binance Wallet booster campaigns, Bitway Earn lockups, only 1.5B of 10B tokens circulating, short squeeze; one wallet reportedly holds 78% of supply (https://beincrypto.com/bitway-btw-record-high-short-analysis/, read 2026-10-17) | Fresh, low float, not tested yet |
| AKE Akedo | $0.67B | +14,784% | $0.1467 (2026-09-20) | **-80%** | Leveraged whale squeeze, momentum, "AI multi-agent on BNB" story, ahead of an unlock; FDV $2.8B vs $641M mcap (https://coinstats.app/ai/a/latest-news-for-akedo.md, Sept 2026) | Round-tripped |
| Q Quack AI | $0.66B | +1,173% (30d +376%) | ATH today (2026-10-18) | -5% | x402-branded "Q402" settlement on BNB; mcap was about $16M on 2026-07-24 per Phantom (https://phantom.com/tokens/ethereum/0xc07e1300dc138601fa6b0b59f8d0fa477e690589). No confirmed catalyst for the October move | Fresh, unverified |
| ZEC Zcash | $19.40B | +358% | $1,249 (about 2026-09-07) | about -9% (now $1,142; -22% 30d) | Under $200 on 2026-03-08, then Grayscale ZCSH ETF trading from 2026-08-25 ($463M AUM by 2026-09-04) and a privacy plus AI-surveillance narrative (https://cointelegraph.com/news/zcash-highest-price-market-cap-20-billion, 2026-09-07) | **Held** |
| NEAR | $6.49B | +315% | about $5.16 (late Sept) | about -4% (now $4.97) | Spot ETF anticipation (Bitwise), AI rotation and intents, short squeeze (https://whale-alert.io/stories/d4b801f6f8d413/NEAR-Protocol-Rises-54-in-a-Week-on-Short-Squeeze-AI-Rotation-and-Growing-Institutional-Interest) | **Held** |
| VVV Venice | $1.09B | +239% (1y +1,362%) | $34.53 (2026-09-21) | -35% | AI inference revenue at a **$100M+ annualized run rate**, revenue funds buyback and burn ($5 of every $100 of API credits), DIEM gives $1/day of API credit per locked staked VVV, emissions cut 3M to 2.5M to 2M (https://cryptobriefing.com/venice-ai-vvv-token-surges-record-high/, 2026-09-21) | Mostly held (+2,300% from Dec low) |
| HYPE Hyperliquid | $17.51B | +115% | $97.96 (2026-09-23) | -20% | Assistance Fund buybacks over $1.26B (https://247wallst.com/investing/cryptocurrency/2026/10/14/hyperliquid-has-bought-back-1-26-billion-of-its-own-token-ethereum-burns-its-fees-instead-which-one-holds-its-price-better/, 2026-10-14); +40.5% in 4 days after the 2026-08-19 announcement (CoinGecko Q3 report); HIP-3 equity perps | **Held** |
| PRL Pearl | $0.63B | listed recently (30d +113%) | $2.10 (2026-10-14) | -14% | Useful-PoW L1 (matrix-multiplication mining), mainnet 2026-04-27; about $0.28 in August to $2.10 (about 7.5x); Together AI discounted inference endpoint (https://ambcrypto.com/pearl-crypto-surges-228-in-a-month-but-what-exactly-is-prl, 2026-10-14; https://www.techi.com/pearl-prl-search-surge-ai-mining-together-api/, 2026-10-17) | Running now. **Direct comp for pearl-sol** |
| QNT Quant | $2.84B | +180% (30d +220%) | about $372 (Sept 2026) | about -48% (now $195) | Institutional and banking-rail story (The Clearing House on-chain money), tight supply (https://www.kucoin.com/news/flash/former-blackrock-vp-identifies-factors-behind-quant-qnt-472-price-surge) | Half round-trip |
| LIT Lighter | $0.79B | +257% | $7.86 (2025-12-30) | -60% (30d -34%) | Revenue-funded buybacks, 15.5M LIT burn in July (https://beincrypto.com/lighter-lit-token-burn-rally/) | Faded |
| PUMP pump.fun | $2.00B | +159% | $0.008819 (2025-09-14) | -51% | 50% of revenue to buybacks: $487.78M cumulative, 17.23% of supply burned (https://pump.fun/pump-token, read 2026-10-17) | Recovered off lows, below ATH |
| CARDS Collector Crypt | $0.24B | +601% | $0.3814 (2025-09-14) | -29% | Physical TCG gacha with real revenue: $16.7M in 30d, up from $9.6M (DefiLlama revenue API) | Held, revenue-backed |
| INJ Injective | $0.86B | +201% (30d +51%) | 2024 ATH | -84% | 21Shares staking ETF filing, Canary product (https://www.fxstreet.com/cryptocurrencies/news/celestia-jumps-57-to-11-month-high-as-7-altcoins-beat-the-market-202610170556, 2026-10-17) | Fresh |
| TIA Celestia | $0.62B | +114% (30d +76%, 7d +57% per FXStreet) | 2024 ATH | -97% | Launch of "Eden", an EVM chain for AI agents that uses TIA as its native token (FXStreet, same URL) | Fresh. Note: an "AI agent chain" relaunch moved a dead L1 token |
| UB Unibase | $0.30B | +407% | $0.2436 (2026-05-15) | -50% | Agent memory, "Agent Market" launch in May (https://www.mexc.com/news/1087007) | Half round-trip |
| M MemeCore | $1.70B | -68% | $5.64 (2026-07-02) | **-87%** | Hard fork in late March 2026 (https://financefeeds.com/top-crypto-gainers-2026-real-revenue/) | Round-tripped |
| RAIN | $6.56B | +13% (1y +155%) | $0.01946 (2026-08-25) | -52% | Prediction markets on Arbitrum; a treasury company's liquidity commitment (https://www.redchip.com/news/8774720158159749/) | Round-tripping |
| Robinhood Chain memes: PONS, CASHCAT, AI (Artificial Inu), STONK (Solana) | $0.08-0.19B each | n/a | early to mid Sept 2026 | -67% to -79% | Robinhood Chain launch (2026-07-01) memecoin mania, Pons launchpad, gas subsidy ending 2026-09-29 (https://www.theblock.co/news/markets/2026-08-31-robinhood-chain-activity-surges-in-august-as-dex-volume-near-1-billion-413136) | **Round-tripped** |

### E. Protocol revenue leaders, last 30 days (DefiLlama dailyRevenue)

Source: https://api.llama.fi/overview/fees?dataType=dailyRevenue, read 2026-10-17. All-protocol revenue: $1,275.7M over 30d (previous 30d $1,242.5M, +2.7%); $13.47B over 1y.

| # | Protocol | Category | 30d rev | Prev 30d | 1y rev | Token value accrual |
|---|---|---|---|---|---|---|
| 1 | Tether | Stablecoin issuer | $522.1M | n/a | $5,949.8M | No token |
| 2 | Circle | Stablecoin issuer | $212.0M | n/a | $2,380.2M | Equity |
| 3 | Hyperliquid Perps | Perps | $68.0M | $82.8M | $664.8M | About 97-99% of fees to buyback and burn |
| 4 | **Papertrade** (launched 2026-10-10) | Perps (house pool, 1000x) | $56.1M in 8 days | 0 | $56.1M | Over 90% to PAPER stakers in USDC |
| 5 | pump.fun | Launchpad | $44.4M | $34.2M | $325.2M | 50% buyback and burn |
| 6 | Tron | Chain | $23.7M | n/a | $340.4M | |
| 7 | **StonkFun** (launched about 2026-08-03) | Launchpad (stock-paired) | $19.9M | $14.5M | $35.2M | About 60% buyback and burn of STONK |
| 8 | fomo | Trading app | $18.9M | $29.4M | $80.2M | |
| 9 | Grayscale | RWA/ETF | $18.1M | $17.2M | $188.2M | |
| 10 | GMGN | Trading app | $17.5M | $43.1M | $162.4M | |
| 11 | Collector Crypt | Physical TCG | $16.7M | $9.6M | $97.2M | CARDS |
| 12 | Axiom | Trading app | $16.4M | $18.2M | $158.5M | |
| 13 | PumpSwap | DEX | $15.9M | $15.2M | $123.4M | (pump.fun) |
| 14 | Polymarket | Prediction market | $14.7M | $18.4M | $131.5M | |

---

## 1. On-chain capital flows

**TVL.** Total DeFi TVL is $88.5B, down 23% YTD (from $114.4B on 2026-01-01) and up 19% from the $74.4B 90-day low (https://api.llama.fi/v2/historicalChainTvl). By category, the largest current buckets among non-CEX protocols (https://api.llama.fi/protocols, read 2026-10-17) are bridges $57.6B, liquid staking $54.8B, lending $51.4B, staking pools $14.4B, DEXs $13.7B, restaking $9.8B, risk curators $9.5B, basis trading $7.7B, RWA $4.0B (DefiLlama's protocol-level RWA bucket, much smaller than CoinGecko's RWA token mcap), privacy $0.94B, prediction markets $0.41B, and payments $0.32B. The free API no longer serves the 1-month category change (`change_1m` is absent), so **category TVL change for YTD and 90d is not available**.

**Who is gaining.** Over 90 days, Robinhood Chain (+298%), Monad (+40%), Avalanche (+38%), Base (+34%), Solana (+21%) and Ethereum (+20%) gained. Hyperliquid L1 DeFi TVL fell 22% over 90 days even though its stablecoin float rose 12%: the capital sits in the perp venue, not in HyperEVM DeFi. Two chains launched by large incumbents (Robinhood Chain on 2026-07-01, Circle's Arc with TVL from 2026-09-17) each reached about $0.5-1.1B within weeks. A distribution partner bootstrapping a chain moves more TVL than any incentive program this year.

**Stablecoins.** Total supply is flat at $304.6B YTD. Ethereum L1 lost about $19B (-12%) YTD while Tron gained about $13B (+16%), Hyperliquid gained $2.3B (+49%), Solana gained $1.6B (+11%), and Base gained $0.65B (+14%) (https://stablecoins.llama.fi/stablecoincharts/{chain}).

**Bridges.** Not available, as explained in the method notes. The one data point found is a live Blockscan feed showing many Across transfers into Robinhood Chain on 2026-10-17 (https://blockscan.com/cross-chain), which is anecdotal.

**Solana specifics.**
- Solana DeFi TVL is $5.89B, -27.8% YTD and +21.3% over 90d. The largest protocols are Sanctum LSTs $1.80B, Kamino Lend $1.31B, Raydium AMM $1.16B, Jupiter Lend $1.09B, Jito $1.09B, Binance staked SOL $1.09B, and Jupiter Perps $0.74B (https://api.llama.fi/protocols).
- Solana app revenue: $163.8M over 30d vs $139.1M the previous 30d, +18% (https://api.llama.fi/overview/fees/solana?dataType=dailyRevenue).
- Solana DEX volume: $69.8B over 30d vs $80.5B previously (-13%), $1,055B over 1y. PumpSwap is still #1 at $10.9B but fell from $19.0B. The pump.fun bonding curve rose from $2.6B to $4.4B (https://api.llama.fi/overview/dexs/solana).
- pump.fun monthly revenue (https://api.llama.fi/summary/fees/pump.fun?dataType=dailyRevenue): Sept 2025 $42.9M, then a steady $18-32M/mo through July 2026 (low: June $18.1M), then a rebound to Aug 2026 $37.2M, Sept $33.5M, and Oct $25.1M with only 17 days counted. That puts October on pace for its best month in a year. pump.fun still takes 64.2% of three-way launchpad revenue (9 Aug to 7 Sep per https://cryptobriefing.com/pump-fun-fee-share-rebounds-50-percent/).
- Launchpad competitors, 30d revenue (DefiLlama): StonkFun $19.95M (prev $14.46M), BONK.fun $4.43M (prev $1.80M), Graphite $3.0M, LaunchLab $1.79M. Pons V2 (Robinhood Chain) $5.71M, down from $21.97M (-74%). Flap.sh (BSC) $8.2M. **Launchpad revenue is moving toward "pair against something other than SOL" designs** (StonkFun pairs against tokenized SPY; Pons and Long on Robinhood Chain pair against stock tokens), but those coins round-trip just like plain memes.
- Mindshare: Solana is at 16.74% of crypto mindshare (+9.88% m/m), Bitcoin 6.48%, Ethereum 3.93%, per a @SolanaFloor report cited at https://cryptobriefing.com/solana-tops-crypto-mindshare-outpacing-bitcoin-and-ethereum/ (2026-10-13). Kaito's own monthly report was not found.

## 2. Category performance

See table C. The patterns:
- **Over 6 months, the categories that worked were privacy (ZEC, XMR), perps (HYPE, LIT), and AI tokens with a real revenue line or an ETF story (VVV, NEAR).** Generic "AI agents" (+/-0% over 200d, -20.5% 1y), AI frameworks (-74% 1y), Solana memes (-66.6% 1y), the pump.fun ecosystem (-59% 1y), and DePIN (-41.6% 1y) did not.
- Over the last 30 days, AI (+25.5%), Data Availability (+51.8%, NEAR and TIA), Storage (+21.7%) and DePIN (+17.8%) bounced, while prediction markets (-26.8%), Robinhood Chain memes (-67.8%) and perps (-7.4%) cooled.
- The x402 category is $10.1B, but about 91% of that is LINK. Pure x402 names are small (UB $0.30B, PEAQ $0.10B), so treat the category label as marketing.
- Prediction markets: CoinGecko's $6.65B category is almost all RAIN. Polymarket earned $14.7M in 30d and Kalshi did $14.9B in 30d volume (https://api.llama.fi/overview/dexs), but the leaders have no liquid token. Revenue is real, the token exposure is thin.

## 3. Fastest pumps (detail beyond table D)

Patterns among the more-than-3x movers with at least $100M mcap:
1. **ETF or regulated-wrapper catalysts held best**: ZEC (Grayscale ETF), NEAR (Bitwise ETF anticipation), HYPE (21Shares and Bitwise ETFs per https://financefeeds.com/top-crypto-gainers-2026-real-revenue/), INJ (staking ETF filing).
2. **Revenue-funded supply sinks held reasonably well when revenue kept growing**: VVV (inference ARR from about $70M to $100M+), HYPE, CARDS. Where revenue fell, so did the price: LIT -60% from ATH, ASTER -71% from ATH despite a 99%-of-fees buyback from 2026-06-17 (https://www.lcx.com/en/cryptonews/aster-popped-over-10-on-radical-buyback-and-burn-upgrade-but-gains-were-short-li).
3. **Low-float BNB-chain AI tokens squeezed by Binance campaigns and leverage round-tripped** (AKE -80%, UB -50%, M -87%). BTW and Q look like the same setup, still near their highs.
4. **Launch-mania memes on a new chain round-tripped within weeks**: the Robinhood Chain memes fell 67-79% from early-September peaks, and STONK fell 74% from 2026-09-21.
5. **Compute and useful-work tokens that moved**: VVV (inference revenue), PRL (useful-PoW mining), NOS (Nosana, +84% 30d and +104% 200d but only $0.05B mcap), AKT (+33% 30d, $0.21B). Tokens with no sticky demand stayed down: TAO -12% over 200d at $3.04B, RENDER +4%, FET -18%, PROVE -19% at $0.04B, Gensyn AI -84% from its 2026-04-29 launch high, and ZKC (Boundless) is ranked about #1216 on CoinGecko (search API, read 2026-10-17).

## 4. Revenue leaders and buybacks: what moved price vs what did not

| Program | Size | Price result |
|---|---|---|
| Hyperliquid Assistance Fund (about 97-99% of fees, open-market buyback, burn) | Over $1.26B cumulative (247wallst, 2026-10-14); 47.5M HYPE bought for $1.32B by 2026-09-26 (https://x.com/HyperliquidNews/status/2103796259853095393) | 200d +115%, ATH 2026-09-23, -20% now. **Worked**: big, continuous, on-chain, verifiable, from organic fees |
| pump.fun (50% of net revenue, buyback and burn until April 2027) | $487.78M cumulative, 17.23% of supply burned (https://pump.fun/pump-token) | 200d +159% off the lows but still -51% from ATH. **Partial**: put a floor in, did not create a trend, because revenue only kept pace and did not grow |
| Venice VVV (revenue buyback and burn plus DIEM compute credit) | Burns reported at 42% of circulating supply (https://coinmarketcap.com/top-stories/69fec02f6713e005f857fc71/, figure disputed) | **Worked**: about 37x from the December low, mostly held. The token is a claim on compute (DIEM), not only a fee share |
| Lighter LIT (revenue-funded buyback, 15.5M burn) | 6.3% of circulating supply (beincrypto, July) | Faded (-60% from ATH): perp DEX revenue is small next to HYPE ($2.75M in 30d) |
| Aster (40%, then 80%, then 99% of fees) | Escalated Jan to Jun 2026 | **Did not work**: -71% from ATH; each escalation pumped for a day. The volume was believed to be incentive-farmed |
| StonkFun (about 60% of revenue to STONK buyback) | $19.9M of 30d revenue | STONK -74% from ATH 2026-09-21. **Did not hold**: the buyback policy is a webpage commitment, not an on-chain rule (https://www.datawallet.com/crypto/stonk-fun-explained) |
| Papertrade PAPER (USDC to stakers; PAPER minted to losing traders) | $56.1M "revenue" in 8 days, about 86% of which was LP excess yield, i.e. trader losses (https://www.techflowpost.com/en-US/article/34664) | Too new; staker payouts per hour fell about 46x after the first 8 hours (same source). CoinGecko had no PAPER listing on 2026-10-17 |
| Gensyn AI (revenue buyback, 70% burned, since May 2026) | Not disclosed | -84% from launch. **No demand means no effect** |
| Akash BME (burn-mint, live 2026-03-23) | Q1 2026 compute spend $5M (https://akash.network/blog/akash-network-q1-2026-report/); DefiLlama fees $0.22M in 30d | AKT +39% over 200d at $0.21B mcap: modest |

## 5. Agent economy metrics

- **x402, cumulative**: 192,071,381 transactions and $41.93M settled across 12 chains and 18 facilitators; Base has 85.06M transactions (https://agenteconomy.to/x402, read 2026-10-17, built from public Dune dashboards).
- **x402, last 30 days** (x402.org via Crypto Briefing, 2026-10-15): 75.41M transactions, $24.24M settled, 43,100 monthly active senders (+152%), about 94,060 buyers and 22,000 sellers. Active senders by chain: Base about 32,000, Algorand about 9.1K, Solana about 1.6K (https://cryptobriefing.com/x402-stablecoin-payments-active-senders-surge/).
- **How much of it is real**: TRM Labs counts 198.9M settlements worth $52.7M since May 2025. Only $25.62M of that is "plausible trading", and only 0.6-7.5% was completed by agents (https://cryptonews.net/news/finance/33594885/, 2026-10-15). Chainalysis counts more than 138M x402 transactions on Base by Q3 2026, over 87% of them under $0.10 (https://bravenewcoin.com/insights/coinbases-x402-records-138-5m-transactions-chainalysis-shows-ai-agents-reshaping-crypto-payments). Visa put real volume at about $19M after removing wash trading (https://startupfortune.com/ai-agents-have-moved-over-200-million-payments-on-x402-but-half-may-be-fake/).
- **Solana share**: about 70% of monthly x402 volume as of mid-August 2026, over 35M cumulative Solana transactions, and Solana passed Base in daily transaction count on 2026-08-25 (https://cryptobriefing.com/solana-x402-market-share-dominance/, 2026-09-15). This conflicts with the sender count (Solana about 1.6K of 43.1K): Solana has fewer, higher-frequency senders.
- **What agents pay for**: no tracker breaks x402 spending out by compute or inference. Of 37,292 AI-accessible services in the Agent Bazaar snapshot, only 246 had 20 or more unique paying agents (cryptonews, same URL).
- **Agent wallets and identities**: at least 583,667 on-chain agent identities as of 2026-08-29, including 526,620 ERC-8004 agents on 24 chains and 57,047 Virtuals agents (https://agenteconomy.to/stats/how-many-ai-agents-are-onchain). On Base, 95,882 ERC-8004 agents are registered but only 1,198 (1.2%) have ever paid anyone, for $8.8M in total. About 60% of that money went to lending and vaults, 18% to bridges, 4.5% to escrowed work, and only about 5% of transfers were x402 API purchases (https://dev.to/agenticfinancegraph/we-read-every-erc-8004-ai-agent-on-base-and-joined-it-to-its-money-12-have-ever-paid-anyone-3bjf, 2026-09-27).
- **Agent token launches**: Virtuals' dashboard shows 81,908 agents launched and $481.79M "aGDP", which is self-reported and includes trading turnover. Virtuals' TTM revenue is about $4.8M (https://ownyourmind.ai/projects/virtuals/); DefiLlama shows Virtuals at $0.12M revenue and $0.41M fees over 30d. VIRTUAL is at $0.65, -87% from ATH.
- **On-chain revenue of the AI agents and DePIN categories (DefiLlama, 30d)**: AI Agents $0.7M in total, DePIN $0.4M, Payments $0.4M, Developer Tools $0.66M (Aethir $0.62M). For comparison, ORE (gamified mining on Solana) earned $3.47M in 30d and $42.7M over 1y.
- **Compute payments and useful work specifically**:
  - Pearl (PRL): useful-PoW L1, mainnet 2026-04-27, 2.1B max supply with about 332-340M mined by early October (https://hashrateindex.com/blog/pearl-prl-ai-compute-cryptocurrency/). Market cap is $0.63B and it is #1 on CoinGecko trending (techi, 2026-10-17). No public figure exists for how much mining is customer inference. A June preprint showed random-matrix shares getting accepted, and the Together AI Pearl model is no longer on the Serverless API as of 2026-10-17 (techi).
  - Venice: the strongest compute-revenue token, with $100M+ ARR and DIEM (tokenized daily API credit).
  - io.net: claimed run rate of $12.11M (June 2026) and $20M+ lifetime on-chain revenue (https://blockeden.xyz/blog/2026/04/03/depin-revenue-inflection-enterprise-cloud-overflow-akash-ionet/, secondary source). IO mcap is $0.06B.
  - Messari, cited secondhand: about $72M of DePIN on-chain revenue in FY2025 and about $100M projected for 2026 (https://blockeden.xyz/blog/2026/03/21/depin-march-2026-reality-check-650-projects-19b-market-cap-revenue/).
  - Bittensor: estimated ecosystem ARR of $28-35M against far larger emissions. Its largest subnet receives about $52M/yr in emissions and earns about $2.4M in revenue (https://x.com/PineAnalytics/status/2036156766736601117). TAO is -12% over 200d.
  - Proof markets: Succinct generated 6M+ proofs in 2025 (https://blog.succinct.xyz/succinct-2025-recap/). **No 2026 revenue figures found** for Succinct or Boundless. PROVE has a $0.04B mcap (-89% from ATH); ZKC is ranked about #1216.
  - Kite AI (agent payments L1, mainnet 2026-04-30): KITE $0.28B, -64% from its 2026-03-06 ATH.

---

## Mechanisms that produced durable value accrual vs pump-and-fade

**Durable (held most of their gains through Q3 and into October):**
1. **Continuous open-market buyback from organic, growing fee revenue, verifiable on-chain** (HYPE). The buyback has to be large relative to float and unlocks, and fed by non-incentivized volume. The same mechanism on flat or farmed revenue (PUMP, LIT, ASTER) only sets a floor.
2. **The token is the unit of account for a real service**: VVV's DIEM makes staked VVV a claim on $1/day of inference. Demand for the token scales with API demand, and the burn comes on top. This is the closest existing analog to "mine or stake, then get compute credit, then sell it for SOL", and it is the one AI-compute token that did more than 30x and mostly held.
3. **Regulated-wrapper access** (ETF or ETP) bringing in new holders who are not crypto-natives: ZEC, NEAR, HYPE, INJ.
4. **A distribution partner bootstrapping a venue**: Robinhood Chain ($1.05B TVL in about 3.5 months) and Arc ($0.53B in a month). The chains held TVL; their memecoins did not.
5. **Physical or real-world goods revenue in a crypto wrapper**: Collector Crypt's revenue grew 74% m/m and CARDS held.

**Pump-and-fade (round-tripped 50-90%):**
1. **Low float plus exchange campaign plus leverage** with no revenue line (AKE, M, UB; BTW and Q are the live candidates for the same outcome).
2. **New-chain or new-launchpad meme waves** (Robinhood Chain memes, PONS, STONK): revenue moves to the next venue within weeks (Pons V2 revenue -74% m/m).
3. **A buyback promise that is off-chain or discretionary, or that escalates without fresh demand** (Aster's 40-80-99%, StonkFun's webpage policy).
4. **Revenue that is really participant losses or emissions** (Papertrade's LP excess yield; Bittensor subnet emissions about 20x subnet revenue). The headline revenue is huge for a week, then falls about 46x per hour.
5. **"Agent" or "x402" labels with no paying demand**: about 192M x402 transactions but only $25-42M settled in total, 1.2% of registered agents have ever paid, and the AI Agents category earns $0.7M/month on-chain. VIRTUAL, KITE, FET, PROVE and Gensyn are all down 25-89% from their peaks.

**What this implies for pearl-sol:**
- **Pearl (PRL) is the live comp and is running right now** (about 7.5x since August, $0.63B). The market will pay for useful-PoW. The open attack surface is "show me the paying customers": PRL has no public figure for the customer-inference share. If pearl-sol publishes on-chain receipts of x402 compute sales (SOL in, verified job out, per block), it gets the one metric the PRL bull case lacks.
- **Copy the VVV pattern, not the PUMP pattern**: make the token (or a staked derivative) redeemable for compute credit, and route storefront SOL revenue into an on-chain, verifiable, continuous buyback. A discretionary or webpage buyback does not hold.
- **Agent Credit has no measured market yet.** Uncollateralized lending is $0.17M of revenue per month across the whole of DefiLlama (Accountable, Wildcat), and agents mostly park cash in vaults. That is a reason to make it the differentiator and keep it small at first, not to lead with it.
- **x402 on Solana is the right rail** (about 70% of volume). Expect skeptics to discount transaction counts: report settled USD and unique paying agents, not transaction count.
