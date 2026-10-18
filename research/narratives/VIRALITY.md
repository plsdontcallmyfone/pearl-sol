# VIRALITY: what spread in crypto compute/AI/agents (Apr to Oct 2026), and what fits pearl-sol

Research slice, 2026-10-17. Scope: attention and virality, not prices or flows (other files
in this folder cover those). No code changes. Firecrawl was down, so sources are WebSearch
(extended) and WebFetch. Every page below was read on 2026-10-17 unless noted. "[derived]"
marks my own arithmetic on sourced inputs. Where a number is only in a secondary
aggregator, it says so.

**Data gaps, stated up front:**
- Kaito mindshare leaderboards, Cookie.fun and LunarCrush live screeners are JS dashboards;
  WebFetch could not read current values. Kaito numbers below are from articles that quote
  Kaito. No current (Oct 2026) Kaito AI-sector share was found.
- Google Trends returned HTTP 429 to WebFetch. Only Google Trends figures quoted in
  articles are used. No Trends series for "Pearl", "x402" or "Bittensor" was obtained.
- X follower time series: not available. @prlnet shows 17.5K and 10.8K followers in two
  undated search snapshots ([x.com/prlnet](https://x.com/prlnet)); no growth can be claimed.
- Kaito Yaps ended 2026-01-15, so there is no Yaps leaderboard for the period in scope.

## 0. TL;DR

1. **The biggest compute virality of the period is Pearl itself, and it came in two waves.**
   Wave 1 (late May 2026): a GPU mining rush five weeks after the 2026-04-27 mainnet, PRL
   to ~$1.65 to $1.70 (May 28-29), then a 92% round trip to $0.1399 on 2026-07-23. Wave 2
   (Aug to Oct): KOL calls (Ansem, "AI version of Bitcoin"), a fund taking a position
   (No Limit Holdings), "most-mentioned" project at Token2049, ATH $2.10 on 2026-10-14,
   #1 to #2 on CoinGecko trending on 2026-10-17, market cap ~$588M to $631M.
2. **Pearl's loudest negative viral moment was a usefulness exposé**: an arXiv preprint
   (2026-06-03) found zero inference code in the dominant miner and got 44 pool shares
   accepted on random matrices; Tom's Hardware headlined "320,000 RTX 3090-class GPUs ...
   112 megawatts ... 'zero useful AI computation'". That critique is exactly the gap our
   design closes (only proofs bound to a paid job earn), which is the most defensible
   attention hook pearl-sol has.
3. **x402 is the canonical attention-to-round-trip case.** PING (pay 1 USDC over x402 to
   mint) drove a ~10,000% weekly jump in Oct 2025; daily x402 tx peaked ~3.8M (Nov 2025)
   and fell to tens of thousands; ecosystem tokens are down 80% to 98%; Artemis flagged
   ~48% of count and ~81% of volume non-organic. But value per payment matured (95% of
   value in payments >= $1 by early 2026) and Solana now carries ~70% to 76% of x402.
4. **Spectacle agents spread fast and decay fast**: Moltbook (Jan 2026, 1.54M "agents" in
   4 days, one dev admitted 500K fakes, MOLT ~$120M peak then retraced, Meta acquired the
   platform in March); Lobstar Wilde (Feb 2026, an agent sent its whole memecoin bag to a
   stranger, LOBSTAR +579% to >$15M); Agency on pump.fun (Oct 2026, every coin gets an AI
   "mind" and treasury, ~128x in two days).
5. **Posting-for-rewards is now off the table on X.** X banned apps that pay users to post
   (2026-01-15, "AI slop & reply spam"); Kaito shut Yaps, KAITO fell ~17% to 21%. Messari
   found Kaito leaderboards raised mindshare but net price was flat or negative.
6. **What converted attention into lasting value had a revenue or utility loop under it**:
   Venice (stake VVV to mint DIEM = $1/day of API credit; ~2M users; LunarCrush social
   dominance +424% w/w in Mar 2026, then ATH ~$34.51 later in the year, $1.08B cap today),
   Grass (Contents API launch, network +699% w/w, price +49% in a day, Oct 2026).
   Points/airdrop farms (ROBO: one cluster took 87%, price -79%) and pure agent coins did not.

## 1. What went viral, Apr to Oct 2026 (plus the Jan to Mar setup)

### 1.1 Pearl (PRL): useful-PoW, GPU mining, "AI Bitcoin"

| Date | Moment | Number | Source |
|---|---|---|---|
| 2026-04-27 | Mainnet launch, "fair launch, no premine, no VC" | 2.1B max supply | [techflowpost](https://www.techflowpost.com/en-US/article/34466) |
| 2026-05-15 | alpha-miner public release; Together AI Gemma-4-31B-it-pearl endpoint at >25% below standard | | [arXiv 2606.04819](https://arxiv.org/html/2606.04819v1), [techi](https://www.techi.com/pearl-prl-search-surge-ai-mining-together-api/) |
| late May | **GPU mining rush** (the viral moment of wave 1): vast.ai RTX 3090 median $0.145 to $0.276/hr (+90%) within two weeks; budget-GPU median +38%, utilization 57% to 94%; network ~24 EH/s, est. ~320,000 RTX 3090-equivalents | | [arXiv 2606.04819](https://arxiv.org/html/2606.04819v1) |
| 2026-05-28/29 | Wave-1 peak | $1.70 (arXiv) / ~$1.65 (Hashrate Index) | [arXiv](https://arxiv.org/html/2606.04819v1), [hashrateindex](https://hashrateindex.com/blog/pearl-prl-ai-compute-cryptocurrency/) |
| early Jun | Miner economics compress: RTX 5090 est. revenue $33.80/day to $17.19/day (-49%) within weeks; mcap ~$146M, FDV ~$1.6B | | [hashrateindex](https://hashrateindex.com/blog/pearl-prl-ai-compute-cryptocurrency/) (2026-06-02) |
| 2026-06-03 | **Usefulness exposé**: 8,012 workers inspected, 0 inference-framework strings in 4,803 identifiable strings; 44 shares from random matrices accepted. Tom's Hardware amplifies: "320,000 RTX 3090-class GPUs ... 112 megawatts ... zero useful AI computation" | | [arXiv](https://arxiv.org/html/2606.04819v1), [tomshardware](https://www.tomshardware.com/tech-industry/artificial-intelligence/ai-cryptomining-networks-320-000-rtx-3090-class-gpus-allegedly-burn-112-megawatts-of-power-on-zero-useful-ai-computation-pearls-gpus-are-doing-random-matrix-math-study-claims) (article body truncated; headline figures only) |
| 2026-07-23 | ATL $0.1399 (-24.2% that day) | **-92% from $1.70** [derived] | [simeononsecurity](https://simeononsecurity.com/articles/pearl-prl-mining-guide-2026/) (2026-09-22), [coingecko](https://www.coingecko.com/en/coins/pearl-research) |
| 2026-08-06 | Rank-penalty softfork (block ~96251) | | [simeononsecurity](https://simeononsecurity.com/articles/pearl-prl-mining-guide-2026/) |
| Aug to Oct | **Wave 2**: Ansem calls Pearl "AI version of Bitcoin" in an interview; No Limit Holdings announces a "long-term heavy position" (size not disclosed); ~$0.33 to $1.40 in a month | ~300% | [kucoin flash](https://www.kucoin.com/news/flash/prl-surges-300-as-pearl-gains-attention-as-ai-bitcoin), [coinstats](https://coinstats.app/ai/a/latest-news-for-pearl-2) (secondary; primary interview not found) |
| mid-Sep to mid-Oct | ~$0.59 to $1.93 (+228%); ~$0.28 (Aug) to $2.10 (~7x); Milk Road's Rohit: PRL "dominated" his feed for days | | [parameter.io](https://parameter.io/prl-token-surges-sevenfold-since-august-on-ai-mining-model-despite-code-controversy/) (2026-10-15), [ambcrypto](https://ambcrypto.com/pearl-crypto-surges-228-in-a-month-but-what-exactly-is-prl/) |
| 2026-10-07/10 | Token2049: "abnormally high frequency" of mentions, no counts given; Pearl Connect side event sponsored by No Limit | no mention metric | [chaincatcher](https://www.chaincatcher.com/en/article/2295358), [techflowpost](https://www.techflowpost.com/en-US/article/34466) |
| 2026-10-14 | ATH $2.10 | ~15x from ATL [derived] | [coingecko](https://www.coingecko.com/en/coins/pearl-research) |
| 2026-10-17 | $1.81, mcap $631M, 24h vol $5.52M, 7d +30.8%, 30d +113.1%, 348.7M circulating, 7.09K portfolio adds, 4 exchanges (Ourbit, BigONE, SafeTrade, Margin Trade); #11 on CoinGecko trending at my read, #1 to #2 in TECHi's checks | | [coingecko PRL](https://www.coingecko.com/en/coins/pearl-research), [coingecko trending](https://www.coingecko.com/en/highlights/trending-crypto), [techi](https://www.techi.com/pearl-prl-search-surge-ai-mining-together-api/) |

Caveats: liquidity is thin (in Sept, 95.17% of reported volume on SafeTrade with $0.00 book
depth per [simeononsecurity](https://simeononsecurity.com/articles/pearl-prl-mining-guide-2026/));
ticker collision with a different "PRL" on Ink; TECHi notes the Together model page now says
"not available on Together's Serverless API" (checked 2026-10-17). A widely shared ApeX
post: "$PRL has 13x'd since July. Mining it is still barely breakeven, and @prlnet issues
more PRL every year than the entire token is worth" ([x.com/OfficialApeXdex](https://x.com/OfficialApeXdex/status/2110295798613258314)).

**Read:** wave 1 was participation-led (anyone with a GPU, rentable on vast.ai) and round-
tripped once emissions met falling miner revenue and the exposé. Wave 2 is narrative + KOL +
conference led and is holding for now (3 days off ATH). Neither wave was driven by proof of
useful output; the strongest criticism (job-binding) is still open.

### 1.2 x402 and PING: "pay-to-mint" over HTTP 402

- PING: pay 1 USDC over x402 to mint; ~31,000 new buyers in week 1 (secondary), >150,000
  tx in its first month (Chainalysis). x402 weekly tx ~26K to ~488K (Oct 14 to 20, 2025),
  reported +10,000% to +10,780%. [chainalysis](https://www.chainalysis.com/blog/x402-agentic-payments-adoption/) (2026-06-03), [panews](https://panews.io/articles/7c3be5b3-b0de-4405-adbd-af9a16d4ae09)
- x402 ecosystem tokens passed ~$800M combined cap in late Oct 2025, +366% in a day
  (secondary). [search summary of mexc/odaily coverage](https://www.odaily.news/en/post/5210138)
- Round trip: daily x402 tx peaked ~3.8M (Nov 2025), now "tens of thousands"; daily volume
  >$2M to ~$10K to $20K; PING, Daydreams, PAYAI down 80% to 98% from ATH; Artemis (end
  2025): ~48% of tx count and ~81% of volume non-organic. [odaily](https://www.odaily.news/en/post/5210138) (2026-04-07)
- Quality improved after the meme: payments >= $1 were 95% of value in early 2026 (49% a
  year earlier). [chainalysis](https://www.chainalysis.com/blog/x402-agentic-payments-adoption/)
- Cumulative: >138M x402 payments on Base to 2026-07-01; 87% between $0.0001 and $0.10;
  median funding-to-first-payment fell from 73 days to 18 minutes (Q3 2026 cohort); 67% of
  Q3 2026 wallets spend within an hour. Categories include AI inference, code review,
  data APIs; no wash share quantified. [crypto.news](https://crypto.news/ai-agents-made-138-million-crypto-payments-what-did-they-buy) (2026-10-15)
- Solana share: ~70% of monthly x402 volume (~$24.24M / 30d, mid-Aug 2026, x402scan via
  [cryptobriefing](https://cryptobriefing.com/solana-x402-market-share-dominance/)); Solana's
  own claim 76% of tx, 23.2M in four weeks ([x.com/solana](https://x.com/solana/status/2102285997304148088), self-reported).
  Solana facilitators: Dexter ~68.7%, PayAI ~30.5% (search summary of the same coverage).

### 1.3 Agents as spectacle

| Case | Viral moment | Numbers | Outcome | Source |
|---|---|---|---|---|
| Moltbook / OpenClaw (Jan to Mar 2026) | Reddit for AI agents, humans only watch | 1.54M agents, 100K posts, 360K comments, 1M human spectators by 2026-02-02; one dev admitted 500K fake accounts (~1/3); Columbia count 6,000+ agents | MOLT (unofficial, Base) ~$120M peak "before sharply retracing"; CLAWNCH $43M peak; Meta acquired Moltbook (Mar 2026) | [odaily](https://www.odaily.news/en/post/5209159) (2026-02-02), [forbes headline](https://www.forbes.com/sites/tylerroush/2026/03/10/meta-acquires-moltbook-social-media-platform-for-ai-agents/) (403 on fetch; via search) |
| Clanker on Base (Feb 2026) | agent-coin launch wave riding Moltbook | record >$11M protocol fees in a week (Odaily) / $8.02M week (other secondary); 21,870 tokens in a day (secondary) | not tracked | [odaily](https://www.odaily.news/en/post/5209159), [panews](https://panews.io/articles/mc5mb15p) |
| Lobstar Wilde (2026-02-23) | autonomous Solana agent funded with $50K SOL, told "make no mistakes", sends its whole memecoin bag to an X user who sells for ~$40K | LOBSTAR >$15M cap, +579% in 24h, $28.4M volume | no post-incident price found | [spendnode](https://www.spendnode.io/blog/lobstar-wilde-ai-agent-openai-developer-accidental-memecoin-transfer-40k-profit/) |
| pump.fun Tokenized Agent mode | pump.fun's own agent-coin feature | n/a | **removed 2026-07-02**: "excessive griefing, confusion ... unnecessary PvP" (Alon Cohen) | [coinspot](https://coinspot.io/en/analysis/pump-fun-abandons-tokenized-agent-mode/) |
| Agency (AGENCY), pump.fun, Oct 2026 | launchpad where every coin gets an AI "mind" (OpenRouter model) that runs its treasury from creator fees; mind hard-stops if fees < 0.1 SOL/hour | graduated 2026-10-01; ~128x over first two full days; $16.06M PumpSwap volume on 2026-10-03; #7 GeckoTerminal trending 2026-10-04; ~$13.8M cap at 10-03 close | too new | [phemex](https://phemex.com/academy/what-is-agency-solana-ai-launchpad-token) (2026-10-05) |
| Nof1 Alpha Arena (Oct to Dec 2025) | six frontier models trade real money live on Hyperliquid, public leaderboard + reasoning | S1: Qwen3-Max +22.3%, DeepSeek +4.9%, GPT-5 -62.7%, Gemini -56.7% | benchmark, no token; template for "watch the AI do X" | [yahoo/decrypt](https://finance.yahoo.com/news/ai-crypto-trading-showdown-deepseek-202512124.html), [forklog](https://forklog.com/en/new-season-of-ai-trading-tournament-kicks-off/) (outside 6 months; included as the mechanic reference) |

### 1.4 Other compute / AI cases

- **Bittensor (TAO)**: the period's big viral event was negative. Covenant AI (Templar SN3,
  Basilica SN39, Grail SN81) exited 2026-04-09/10 calling it "decentralization theatre";
  TAO -15% (The Block), reportedly ~37,000 TAO of alpha sold on the way out. This followed
  Covenant-72B (Mar 2026, >70 permissionless nodes, largest decentralized pretrain run,
  self-reported MMLU 67.1). [theblock](https://www.theblock.co/post/396959/covenant-ai-exits-bittensor-tao), [tao.media](https://www.tao.media/covenant-ais-bittensor-exit-what-happened-how-bittensor-responded-and-whats-next-for-the-network/).
  Today: $267.93, mcap ~$3.04B, ATH $757.60 ([coingecko](https://www.coingecko.com/en/coins/bittensor)).
  Subnet revenue $28M to $35M annualized across 24 subnets = 9% to 12% of incentive payouts
  ([cryptobriefing](https://cryptobriefing.com/bittensor-subnets-annualized-revenue/), Sep 2026);
  Chutes took ~14.4% of TAO emissions with est. $1.3M to $2.4M external revenue (22:1 to 40:1
  subsidy) and OpenRouter throughput fell from ~42B to ~6.8B tokens/day after 2026-02-07
  ([startupfortune](https://startupfortune.com/bittensor-subnets-are-generating-real-revenue-but-the-cheap-prices-hide-a-subsidy/)).
- **Venice (VVV)**: 2026-03-02, +20% intraday to $6.78, 7.5x in three months, top-300 top
  gainer while market fell; LunarCrush: social engagement 255% above average, social
  dominance +424% w/w, AltRank 8; ~2M users; 7.56M VVV (~17% of circ.) locked for DIEM
  ([yahoo/beincrypto](https://finance.yahoo.com/news/why-venice-token-vvv-rallying-083746675.html)).
  Later reported annualized revenue >$100M ([invezz](https://invezz.com/news/2026/08/18/will-vvv-token-rally-as-venice-ai-revenue-tops-100-million/), 2026-08-18) and ATH ~$34.51 (search summary, date unclear).
  Today $22.32, $1.08B cap, largest coin in CoinGecko's AI Agents category ([coingecko](https://www.coingecko.com/en/categories/ai-agents)).
- **Grass**: Contents API launch (~Oct 8-9, 2026) turning residential nodes into scrapers;
  network +699% w/w and GRASS +49.41% on 2026-10-11 ([GitHub dispatch](https://github.com/Ricosworks1/depin-research-reports/releases/tag/depin-dispatch-grass-grows-699-percent-20261013), unreviewed single source).
- **DePIN compute broadly**: RENDER, IO, AKT 94% to 99% below ATH while network fees went
  $1.3M (Jul) to $2.3M (Sep) ([GitHub dispatch](https://github.com/Ricosworks1/depin-research-reports/releases/tag/depin-dispatch-compute-won-tokens-lost-20260928), unreviewed).
- **Pump.fun livestreams / GO bounties**: the 2025 livestream comeback (39 live-section tokens
  >$1M, Bagwork ~$50M) is the template ([bitget](https://www.bitget.com/news/detail/12560604969296));
  in June 2026 the GO bounty feature drew "hundreds of paid tasks within hours" and abuse
  criticism ([thecoinrepublic](https://www.thecoinrepublic.com/2026/06/06/pump-fun-draws-viral-stunts-with-new-crypto-incentive-platform/),
  [cryptorank](https://cryptorank.io/news/feed/b1006-pump-fun-under-fire-over-new-bounty-feature)).
  I found no verified 2026 compute/tech coin that went viral via a pump.fun livestream.
- **KOL coin as attention engine**: ANSEM (pump.fun, created ~Jun 17): ~19,878% in 7 days,
  ATH ~$0.121 on Jun 29, $6.7M airdropped to 704 wallets of which ~74% went to 7 wallets
  (Lookonchain) ([thedefiant](https://thedefiant.io/news/defi/ansem-airdrops-usd7m-of-usdansem-memecoin-in-bid-to-reach-1m-holders), [kucoin](https://www.kucoin.com/news/flash/solana-memecoin-ansem-surges-19-878-in-7-days-drives-80-day-high-in-token-launches)).
  Same KOL later amplified Pearl.

### 1.5 Macro attention backdrop

- Google Trends "crypto" (global): 100 in Aug 2025, 26 to 32 through May 2026
  ([crypto.news](https://crypto.news/why-crypto-search-interest-hit-a-one-year-low-in-2026/), 2026-06-02).
  "bitcoin" searches at multi-year lows in Oct 2026, "buy cryptocurrency" at a yearly high
  ([kucoin flash](https://www.kucoin.com/news/flash/bitcoin-searches-hit-multi-year-lows-in-october-2026-amid-market-correction)).
  Retail search attention is low; attention is concentrated in crypto-native X and CoinGecko.
- AI tokens took 35.7% of crypto "attention" in Q1 2026 (memes 27.1%) vs ~$22.6B of ~$3.5T
  market cap, about 0.65% [derived] (attributed to a CoinGecko narrative report, method not
  stated) ([blockeden](https://blockeden.xyz/blog/2026/04/29/ai-tokens-narrative-revenue-gap-q1-2026-bittensor-render/)).
  Kaito's last AI-share figure I found is 42% (Nov 2024, [x.com/KaitoAI](https://x.com/KaitoAI/status/1862809786539987418)).
- Meme mindshare (Kaito): ~20% late 2024 to 2.5% Oct 2025 ([yahoo](https://finance.yahoo.com/news/meme-coins-dead-mindshare-drops-072524526.html), via search summary).
- AI Agents category today: $3.44B, -1.7% 24h ([coingecko](https://www.coingecko.com/en/categories/ai-agents));
  sector topped >$15B in early Jan 2025 ([theblock](https://www.theblock.co/post/334213/ai-agent-tokens-reel-from-a-steep-market-correction)), ~-77% [derived].
- pump.fun: 38,526 launches on 2026-09-17 (MemeFees, via search summary); graduation ~0.26%
  mid-June 2026 ([dextools](https://www.dextools.io/news/pump-fun-graduation-collapse-solana-fees-2026)); monthly revenue ~$25.6M early 2026 vs $137M peak Jan 2025 ([tokenomics.com](https://tokenomics.com/articles/pumpfun-tokenomics-how-pump-distributes-45m-monthly-to-holders)).
  A plain launch is a needle in ~40K/day; it needs an outside attention source.

## 2. Mechanics that made things spread

| Mechanic | Where it worked | Evidence |
|---|---|---|
| Anyone can participate with rentable hardware | Pearl wave 1 | vast.ai RTX 3090 +90% in two weeks, utilization 57% to 94% (arXiv). The rush itself became the story. |
| A one-line meme frame | Pearl "AI version of Bitcoin"; "fair launch, no VC" | repeated in every Token2049 writeup (techflowpost, chaincatcher, kucoin) |
| KOL + fund endorsement | Pearl wave 2 (Ansem, No Limit), ANSEM | ~300% in a month after the interview (kucoin flash) |
| Pay-to-participate over a new protocol | PING on x402 | 10,000% weekly tx jump; became "proof" of x402 traction |
| Watch-only spectacle of agents | Moltbook (1M human spectators), Alpha Arena | numbers in 1.3 |
| Agent failure as content | Lobstar Wilde | +579% in 24h after the mistake |
| Agent with its own treasury per coin | Agency | ~128x in two days |
| Live leaderboard with real money | Alpha Arena | per-model P&L became the headline |
| Revenue-backed utility that is easy to explain | Venice DIEM ($1/day API credit) | LunarCrush social dominance +424% w/w, then sustained cap |
| Product launch with a measurable jump | Grass Contents API | network +699% w/w |
| Paid posting (yaps) | Kaito 2025 | **banned by X 2026-01-15**; ~157K-member Yapper community (ZachXBT via [coindesk](https://www.coindesk.com/business/2026/01/15/kaito-to-sunset-yaps-as-x-cracks-down-on-infofi-apps-token-falls-17)) |
| Negative virality (exposé) | Pearl usefulness paper, Covenant exit | both moved price down (Pearl ATL in July; TAO -15%) |

## 3. Attention vs value

| Case | Attention | Converted to lasting value? | Real users / revenue vs farming |
|---|---|---|---|
| Pearl wave 1 | GPU rush, rental spike | **No.** $1.70 to $0.14 (-92%) in ~8 weeks | Real GPUs, but mostly inference nobody bought ("AI-shaped proof-of-work", [hashrateindex](https://hashrateindex.com/blog/pearl-prl-ai-compute-cryptocurrency/)); one paying integration (Together) |
| Pearl wave 2 | KOL, fund, Token2049, CoinGecko #1 | **Open.** 15x off ATL, -14% off ATH 3 days later [derived from $1.81 vs $2.10] | Same usefulness gap; thin venues |
| PING / x402 tokens | 10,000% tx jump | **No.** tokens -80% to -98%; tx -99% from peak | ~48% count / ~81% volume non-organic (Artemis); underlying rail kept growing in value per payment |
| Moltbook / MOLT | 1.5M agents, Meta exit | Platform yes (acquired); **token no** (unofficial, retraced) | ~1/3 accounts fake by one admission |
| Kaito Yaps campaigns | mindshare up | **No.** Messari: post-TGE leaderboards raised mindshare, "net price action remained largely flat or negative" ([messari](https://messari.io/report/an-analysis-of-post-tge-kaito-campaigns), ~2025) | Pure farming; X banned it |
| Points / airdrops | | **Mostly no.** ROBO: one cluster took 87%, -79% from first week by late Sep ([bitquery](https://bitquery.io/investigations/robo-airdrop-sybil-attack)); "~88% of airdropped tokens lose value within 3 months" (CoinLaw, unverified compilation) | Sybil filters removing 40% to 59% of addresses (secondary) |
| Bittensor | sector top-3 mindshare (Bitget, unverified) | Partially: TAO ~$3B, but 64.6% below ATH | Subnet revenue only 9% to 12% of emissions |
| Venice | LunarCrush spike Mar 2026 | **Yes so far.** $290M (Mar) to $1.08B today | ~2M users, >$100M ARR claimed |
| Grass | +699% network week | Too early (one day +49%) | Paying API product |
| Agency, Lobstar | trending | Too early / unknown | Treasury mechanic is real; demand is speculative |

**Mindshare-to-price correlation:** I found no rigorous, dated regression. The closest
measured result is Messari's: manufactured mindshare (leaderboards) rose without price. A
"92% correlation" Medium claim has no method and is not used. The macro split (AI ~36% of
attention, <1% of market cap) says attention is cheap to get in this sector and does not
price in by itself.

**Pattern:** attention lasted where it pointed at something measurable that kept growing
(Venice usage, Grass API, x402 value per payment). It round-tripped where the measurable
thing was emissions or self-referential activity (Pearl wave 1 mining rewards, PING mints,
Moltbook posts, yaps).

## 4. Compute-specific virality

- **"Mine with your GPU"**: Pearl is the only 2026 case at scale (est. ~320K 3090-equivalents,
  arXiv). The window narrowed fast: RTX 5090 revenue halved in weeks; SRBMiner notes a
  coming upgrade will make consumer GPUs "pretty much useless" for Pearl
  ([simeononsecurity](https://simeononsecurity.com/articles/pearl-prl-mining-guide-2026/)).
  Third-party funnels exist ("free GPU credit" guide at [lordofpearls.xyz/mine](https://lordofpearls.xyz/mine)).
- **Browser / phone mining**: no viral 2026 case found. Earlier references: Nexus prover
  testnet (browser tabs, phones; points; early 2025, [x.com/0xMoei](https://x.com/0xMoei/status/1891970634944581906)),
  Mina Web Node (4GB WASM memory limit, [minaprotocol](https://minaprotocol.com/blog/retro-mina-web-node-testing)).
  These were points farms, not revenue.
- **Bittensor subnet cycles**: Covenant-72B (Mar) was the high, Covenant exit (Apr) the low.
  Hype tracks emissions, and the revenue/emission ratio is the standing critique.
- **Pearl reception**: enthusiasm for the fair launch and "AI Bitcoin" frame; the core
  critique, conceded by co-founder Omri Weinstein, is that letting miners choose inputs
  "will always allow spam mining" ([parameter.io](https://parameter.io/prl-token-surges-sevenfold-since-august-on-ai-mining-model-despite-code-controversy/)).
  Core devs said there are no on-chain metrics for the share of useful mining ([chaincatcher](https://www.chaincatcher.com/en/article/2295358)).
- **x402 hype**: covered in 1.2. Solana is the main x402 chain now, which favors a
  Solana-side storefront.
- **Agent-pays-for-compute demos**: I found no single viral demo of an agent paying for
  GPU compute in this period. The aggregate signal is Chainalysis: "AI model inference" is
  a named x402 category and 88.1% of geolocated inference payments came from US-funded
  wallets, but no per-category value is published.

## 5. Viral mechanics that fit pearl-sol without being vanity

Ground truth to stay honest about (from [COMPUTE-TECH.md](COMPUTE-TECH.md)): a
stand-alone posted matmul job has no natural buyer once data movement is priced in. So
every mechanic below must show **paid jobs and their proofs**, not imply that we sell
cheap frontier inference. What our system really does (SPEC/HANDOFF): only proofs bound to
a paid job earn from the reserve, and each job's mining reward stays below the fee it burns;
blocks carry a coinbase receipt; the light client on Solana verifies our PoW headers; the
storefront sells jobs over x402 for SOL/USDC; payout liens (SetRoute) route a share of a
miner/agent's income to a lender; pools (PPLNS) and a `pearl` CLI exist.

1. **"Every block was paid for" live feed.**
   - Mechanic: Pearl's largest negative viral moment was "zero useful AI computation" and
     44 shares on random matrices. The counter-artifact is a public feed where each block
     shows: job id, buyer (Solana tx link of the x402 payment), fee burned, reward paid
     (always < fee), proof hash, and the Solana light-client verification tx.
   - Evidence: the exposé drove Pearl's July low; Weinstein conceded spam mining. A feed
     that answers it directly is the most credible differentiator we have.
   - Build: a read-only indexer over agent-chain RPC (`getBlock`, coinbase receipt,
     job/CompleteJob) joined with Solana storefront orders and light-client txs; one web
     page + `pearl feed` in the CLI. Ports 10670-10679.
   - Farming risk: low by construction (reward < fee). Risk is self-dealing jobs looking
     like demand: show buyer concentration (top-10 buyer share) on the same page.
2. **Laptop/one-command mining with an honest earnings meter.**
   - Mechanic: Pearl wave 1 spread because anyone could rent a 3090 and join.
   - Evidence: vast.ai +90% in two weeks (arXiv). Counter-evidence: revenue halved in weeks
     and price round-tripped 92%; browser/phone mining had no 2026 traction.
   - Build: `pearl mine --pool` already exists; add a single install script and a
     dashboard that shows earnings in SOL terms from paid jobs only, plus the current
     backlog of paid jobs. Skip browser mining unless WASM matmul is measured to be worth it.
   - Farming risk: high if the reserve pays for empty work; our job binding blocks that, so
     the risk becomes "no jobs, no earnings", which the meter must show plainly.
3. **Agent leaderboard by verified work, not by followers.**
   - Mechanic: live leaderboards with real money (Alpha Arena) spread; follower/mindshare
     leaderboards (Yaps) became farms and got banned.
   - Evidence: Alpha Arena S1 headlines were per-model P&L; Messari found leaderboard
     mindshare did not move price.
   - Build: rank agents and miners by settled paid jobs, fees paid, fees earned and
     on-time completion, every row clickable to on-chain receipts. No posting points.
   - Farming risk: wash jobs between two accounts. Mitigation: rank on fees burned
     (costly), show counterparty diversity, exclude jobs where buyer and miner share funding.
4. **Agents visibly paying for compute over x402.**
   - Mechanic: PING proved "pay over x402" is itself shareable; Chainalysis shows the rail
     matured toward real >= $1 payments, and Solana carries ~70% to 76%.
   - Build: a public demo agent (CLI `agent-cli` + storefront-x402) that buys a job, shows
     the 402 challenge, the Solana payment, the bridged job and the result commitment
     posted back, as a single receipt page.
   - Farming risk: one operator making many tiny calls inflates counts (Chainalysis caveat;
     Artemis 48% non-organic). Report dollar value and unique paying wallets, not tx count.
5. **Agent Credit: borrow against work history, in public.**
   - Mechanic: agents with their own treasuries (Agency) and agents moving money in public
     (Lobstar) draw attention; nobody in the period showed credit underwritten by verified
     work, so it is novel.
   - Build: a page per credit line showing the coinbase receipts it was underwritten on,
     the SetRoute lien (route_bps, cap, until_height), each routed repayment in the
     withdrawal log, and ClearRoute at the end (getRoute / getAccountTxs role lien exist).
   - Farming risk: borrowers inflating history with self-paid jobs. Underwrite only on fee-
     burning jobs from diverse buyers; show it.
6. **Launch mechanic tied to the reserve, not a points program.**
   - Mechanic: pump.fun is ~38K launches/day with ~0.26% graduation; launches that broke
     out had an outside hook (KOL, livestream, agent). Fair launch "no VC" was half of
     Pearl's frame.
   - Build: show the mining reserve balance and creator-fee buybacks into it live from
     day one, so the "fair, no premine" claim is checkable.
   - Farming risk: low; it is a display of real flows.

### Mechanics to avoid

- **Paid posting / yaps / "post to earn"**: banned by X since 2026-01-15; Kaito's own
  verdict was that permissionless distribution is "no longer viable"
  ([coindesk](https://www.coindesk.com/business/2026/01/15/kaito-to-sunset-yaps-as-x-cracks-down-on-infofi-apps-token-falls-17)).
- **Points programs and big airdrops**: ROBO (87% to one cluster, -79%), sybil filters
  removing 40% to 59% of claimants; ANSEM airdrop (~74% to 7 wallets).
- **Mining rewards for work nobody bought**: this is Pearl's exact weak point and the
  reason for its 92% round trip; our rule (reward < burned fee, job-bound) must not be
  relaxed for a launch boost.
- **Fake agent activity / inflated agent counts**: Moltbook's 500K self-admitted fakes
  became part of the story; PING-style tx-count inflation was flagged as ~48% non-organic.
  Never show raw tx or agent counts without unique-payer and dollar-value counterparts.
- **Agent-run treasuries on pump.fun as the main hook**: pump.fun itself removed Tokenized
  Agent mode for "griefing ... PvP"; Lobstar shows the failure mode is a public loss.
- **Bounty/stunt farming (pump.fun GO)**: drew immediate abuse criticism in June 2026.
- **Overclaiming usefulness** ("cheaper AI inference"): Together AI's Pearl endpoint now
  shows "not available on Together's Serverless API" (TECHi, 2026-10-17); critics framed
  the integration as subsidy. Claim only what the feed proves.

## Sources (all read 2026-10-17)

Primary or near-primary: arxiv.org/html/2606.04819v1; chainalysis.com/blog/x402-agentic-payments-adoption;
coingecko.com (PRL, TAO, AI Agents category, trending); coindesk.com (Kaito Yaps, 2026-01-15);
hashrateindex.com (Pearl, 2026-06-02); messari.io (post-TGE Kaito campaigns); bitquery.io (ROBO).
Secondary: techflowpost, chaincatcher, kucoin flash, parameter.io, ambcrypto, techi, simeononsecurity,
tomshardware (headline only), crypto.news (x2), odaily (x2), panews, cryptobriefing (x2), theblock,
tao.media, startupfortune, yahoo/beincrypto, invezz, phemex, coinspot, spendnode, thedefiant,
dextools, tokenomics.com, blockeden, bitget, thecoinrepublic, cryptorank, GitHub DePIN dispatches
(unreviewed), x.com posts as indexed by search (undated snapshots).
