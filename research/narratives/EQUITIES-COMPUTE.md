# Equities: where compute value was created in 2026 (YTD to 2026-10-16 close)

Research slice for ~/pearl-sol (useful-PoW chain selling AI compute jobs + agent credit). Written 2026-10-17 (Saturday). All prices are closes of Friday 2026-10-16 unless noted. Nothing here is invented: every figure is either computed from the Yahoo Finance data pulled below or quoted from a linked source with the date read. Where a figure was not found it says so.

## Data method and primary sources (read 2026-10-17)

- **Prices:** Yahoo Finance chart API, daily closes, `https://query1.finance.yahoo.com/v8/finance/chart/<TICKER>?period1=...&interval=1d` (read 2026-10-17). YTD = close 2026-10-16 vs last close on or before 2025-12-31 (Korea and Japan last traded 2025-12-30). "Last 90d" = close 2026-10-16 vs close 2026-07-17 (91 calendar days; the 90-day mark fell on a weekend).
- **Market cap now:** Yahoo Finance quote API, `https://query1.finance.yahoo.com/v7/finance/quote?symbols=...` field `marketCap` (read 2026-10-17). Cross-check: NVDA $5.17T on Yahoo matches stockanalysis.com ($5.17T, quote Oct 16 2026 4:00 PM EDT) at https://stockanalysis.com/stocks/nvda/ (read 2026-10-17).
- **Cap added YTD** = cap now minus (cap now / (1 + YTD price return)). This assumes a constant share count, so buybacks, issuance and converts make it approximate (it overstates added value for heavy issuers like miners/neoclouds and understates it for heavy buyback names like AAPL). Non-USD names (Korea, Taiwan, Japan) are converted at the KRW/TWD/JPY rate on each end date from Yahoo FX series, so their USD figure includes currency moves.
- **Caveats on specific caps:** Alphabet uses the GOOGL line only (Yahoo reports company-level cap on both GOOGL and GOOG, so GOOG is excluded to avoid double counting). Samsung (005930.KS) cap of $1,055B is as Yahoo reports it and may include preferred shares; treat as an upper bound. Cerebras (CBRS) IPO'd 2026-05-14, so its "YTD" is measured from the $185 IPO price.
- **Index levels:** S&P 500 (^GSPC) 6,845.50 on 2025-12-31 to 7,659.44 on 2026-10-16 = **+11.9% YTD**, +2.7% over 90d. Nasdaq-100 (^NDX) 25,249.85 to 29,187.55 = **+15.6% YTD**, +2.1% over 90d. QQQ 614.31 to 709.80 = **+15.5%**. SPY 681.92 to 763.25 = **+11.9%**. Semis ETFs: **SMH +49.9% YTD (-3.0% 90d)**, **SOXX +63.8% YTD (-5.4% 90d)**. Cross-check: Macrotrends puts the S&P 500 at +11.9% YTD as of Oct 16 after a record high of 7,818.93 on Oct 6 (https://www.macrotrends.net/2324/sp-500-historical-chart-data, read 2026-10-17). Price returns, not total return.
- **Context for the latest week:** the week ending Oct 16 was the Nasdaq's worst since June; the Philadelphia Semiconductor Index fell 11.4% on the week (https://rallies.ai/news/semiconductor-selloff-drags-nasdaq-sp-500-to-weekly-losses-0d535b6862869f31, read 2026-10-17). Drivers cited: AI disruption fears for software, debt-funded AI infra and CoreWeave's debt load, Chinese chip competition (https://www.tradingkey.com/analysis/stocks/us-stocks/262221811-us-stock-market-close-nasdaq-drop-chip-stocks-fall-coreweave-micron-tradingkey), Brent near $110 (https://money.whatfinger.com/2026/10/16/nasdaq-drops-2-as-chip-and-software-stocks-sell-off/), a 3.7% September CPI print and higher yields that hit leveraged AI-infra builders on Oct 14 (https://www.altcoinbuzz.io/bitcoin-miners-drop-12-as-ai-data-center-debt-reprices), all read 2026-10-17. So the YTD numbers below are measured right after a sharp drawdown; 90d columns show it.
- **Top contributors to index gains:** no source found that publishes an official 2026 points-contribution table for the S&P 500 or Nasdaq-100 (slickcharts YTD pages returned HTTP 403). Table 3 is the proxy: USD cap added, computed from the data above. By that measure the S&P 500's largest adders are AAPL, MU, NVDA, AMD, GOOGL, MSFT, INTC, XOM, DELL, AMZN; TSM, Samsung, SK hynix, ASML and MediaTek are not S&P members but are top global adders.

## Headline findings

1. **The money in 2026 went to the scarce physical inputs of AI, not to the GPU leader or the clouds.** Memory and storage (MU, Samsung, SK hynix, SanDisk, Seagate, WDC, Kioxia) added roughly **$1.88T** YTD in USD terms, more than all of AI chips ex-AMD/Intel and far more than the four hyperscalers' stock gains (MSFT+GOOGL+AMZN+META about **$1.0T** combined, all single-digit % YTD).
2. **Nvidia grew revenue ~106% YoY but the stock is only +14.8% YTD**; AMD (+147%), Intel (+144%), Marvell (+168%), MediaTek (+197%) and GUC (+252%) outran it. The market paid for a second source of accelerators and for custom silicon, while Broadcom (-5.0%) was sold on margin and customer concentration.
3. **Leverage-funded compute lost.** Oracle -34% (-$202B), CoreWeave -14%, IREN -29%, Applied Digital -26%, Cipher -39%, Cerebras -31% from its IPO price. Pure option-value names lost harder: quantum (IONQ -30%, RGTI -47%, QBTS -55%) and SMR nuclear (OKLO -61%, SMR -59%).
4. **Systems integrators who hold the backlog won:** Dell +259% ($95B AI server backlog), HPE +130%.
5. **Power: pay for deliverable-now, behind-the-meter power, not for future reactors.** Bloom Energy +155%, GE Vernova +41%, Quanta +50%, Eaton +23%; independent power producers fell (VST -6%, CEG -23%, NRG -35%).

## Ranked tables

Source for every number in Tables 1-3 and the per-theme price tables: Yahoo Finance chart and quote APIs (see method), read 2026-10-17; prices are Oct 16 2026 closes.

### Table 1. Compute/tech universe ranked by USD market cap added YTD

| # | Ticker | Company | YTD % | Mkt cap now ($B) | Cap added YTD ($B) | Last 90d % |
|---|---|---|---|---|---|---|
| 1 | AAPL | Apple Inc. | +26.3% | 5,013 | 1,045 | +2.9% |
| 2 | MU | Micron Technology, Inc. | +207.9% | 994 | 671 | +3.5% |
| 3 | NVDA | NVIDIA Corporation | +14.8% | 5,171 | 667 | +5.6% |
| 4 | TSM | Taiwan Semiconductor Manufactur | +35.8% | 2,140 | 564 | +3.6% |
| 5 | AMD | Advanced Micro Devices, Inc. | +146.9% | 863 | 514 | +6.7% |
| 6 | 005930.KS | SamsungElec | +79.7% | 1,055 | 512 | -15.5% |
| 7 | 000660.KS | SK hynix | +95.4% | 697 | 367 | -30.9% |
| 8 | GOOGL | Alphabet Inc. | +8.8% | 4,165 | 337 | -1.8% |
| 9 | MSFT | Microsoft Corporation | +9.4% | 3,928 | 337 | +34.3% |
| 10 | INTC | Intel Corporation | +143.5% | 475 | 280 | -5.5% |
| 11 | ASML | ASML Holding N.V. - New York Re | +53.4% | 633 | 220 | -6.1% |
| 12 | DELL | Dell Technologies Inc. | +258.8% | 287 | 207 | +14.0% |
| 13 | AMZN | Amazon.com, Inc. | +8.3% | 2,697 | 207 | +1.1% |
| 14 | SNDK | Sandisk Corporation | +447.4% | 188 | 153 | -4.1% |
| 15 | 2454.TW | MEDIATEK INC | +197.2% | 209 | 136 | +26.1% |
| 16 | PANW | Palo Alto Networks, Inc. | +85.7% | 280 | 129 | -4.7% |
| 17 | MRVL | Marvell Technology, Inc. | +168.1% | 205 | 128 | +20.7% |
| 18 | ARM | Arm Holdings plc | +108.4% | 243 | 127 | -14.7% |
| 19 | META | Meta Platforms, Inc. | +7.5% | 1,807 | 125 | +9.8% |
| 20 | AMAT | Applied Materials, Inc. | +59.3% | 325 | 121 | -22.7% |
| 21 | CSCO | Cisco Systems, Inc. | +39.4% | 423 | 120 | -4.1% |
| 22 | LRCX | Lam Research Corporation | +54.2% | 330 | 116 | -15.8% |
| 23 | CRWD | CrowdStrike Holdings, Inc. | +94.0% | 233 | 113 | +12.0% |
| 24 | TXN | Texas Instruments Incorporated | +59.4% | 252 | 94 | -2.7% |
| 25 | STX | Seagate Technology Holdings PLC | +132.0% | 145 | 83 | -18.9% |
| 26 | GEV | GE Vernova Inc. | +40.9% | 245 | 71 | -12.9% |
| 27 | ADI | Analog Devices, Inc. | +45.8% | 192 | 60 | +5.3% |
| 28 | WDC | Western Digital Corporation | +92.7% | 124 | 60 | -30.4% |
| 29 | ANET | Arista Networks, Inc. | +35.1% | 223 | 58 | +5.0% |
| 30 | KLAC | KLA Corporation | +36.4% | 216 | 58 | -22.1% |
| 31 | HPE | Hewlett Packard Enterprise Comp | +129.5% | 73 | 41 | +20.3% |
| 32 | LITE | Lumentum Holdings Inc. | +123.6% | 75 | 41 | +12.5% |
| 33 | BE | Bloom Energy Corporation | +154.7% | 65 | 40 | +3.0% |
| 34 | 285A.T | KIOXIA HOLDINGS CORPORATION | +284.5% | 45 | 33 | -23.0% |
| 35 | PWR | Quanta Services, Inc. | +49.7% | 95 | 32 | +0.5% |
| 36 | SNOW | Snowflake Inc. | +39.9% | 108 | 31 | +14.1% |
| 37 | ETN | Eaton Corporation, PLC | +22.8% | 152 | 28 | -2.2% |
| 38 | NBIS | Nebius Group N.V. | +97.7% | 45 | 22 | -6.9% |
| 39 | 3443.TW | GLOBAL UNICHIP CORP. | +252.0% | 31 | 22 | +95.0% |
| 40 | ALAB | Astera Labs, Inc. | +71.8% | 50 | 21 | -5.9% |
| 41 | TER | Teradyne, Inc. | +67.9% | 51 | 21 | +0.8% |
| 42 | PLTR | Palantir Technologies Inc. | +4.7% | 447 | 20 | +40.5% |
| 43 | VRT | Vertiv Holdings, LLC | +27.4% | 79 | 17 | -28.7% |
| 44 | MPWR | Monolithic Power Systems, Inc. | +33.6% | 60 | 15 | -7.7% |
| 45 | CIEN | Ciena Corporation | +42.3% | 47 | 14 | -11.1% |
| 46 | COHR | Coherent Corp. | +26.2% | 46 | 9.5 | -16.1% |
| 47 | ON | ON Semiconductor Corporation | +33.0% | 28 | 7.0 | -17.6% |
| 48 | CRDO | Credo Technology Group Holding  | +17.9% | 32 | 4.9 | -16.3% |
| 49 | AAOI | Applied Optoelectronics, Inc. | +116.8% | 6.4 | 3.5 | -26.2% |
| 50 | NXPI | NXP Semiconductors N.V. | +5.7% | 58 | 3.1 | -14.0% |
| 51 | HUT | Hut 8 Corp. | +47.5% | 8.4 | 2.7 | -25.9% |
| 52 | CDNS | Cadence Design Systems, Inc. | +2.2% | 88 | 1.9 | -3.2% |
| 53 | SMCI | Super Micro Computer, Inc. | +8.1% | 21 | 1.6 | +30.8% |
| 54 | QCOM | QUALCOMM Incorporated | +0.6% | 184 | 1.1 | +0.2% |
| 55 | CLS | Celestica, Inc. | +2.8% | 38 | 1.0 | +0.8% |
| 56 | RIOT | Riot Platforms, Inc. | +8.3% | 5.1 | 0.4 | -24.9% |
| 57 | 3661.TW | ALCHIP TECHNOLOGIES LIMITD | +5.0% | 9.8 | 0.1 | +5.9% |
| 58 | WULF | TeraWulf Inc. | +1.7% | 5.8 | 0.1 | -35.7% |
| 59 | CORZ | Core Scientific, Inc. | -7.4% | 4.3 | -0.3 | -35.6% |
| 60 | CLSK | CleanSpark, Inc. | -17.0% | 2.2 | -0.4 | -35.5% |
| 61 | MARA | MARA Holdings, Inc. | -16.1% | 2.9 | -0.6 | -29.6% |
| 62 | NNE | Nano Nuclear Energy Inc. | -46.4% | 0.7 | -0.6 | -19.7% |
| 63 | QUBT | Quantum Computing Inc. | -38.0% | 1.4 | -0.9 | -18.5% |
| 64 | TLN | Talen Energy Corporation | -5.3% | 17 | -0.9 | -4.7% |
| 65 | BTDR | Bitdeer Technologies Group | -31.1% | 2.1 | -0.9 | -28.1% |
| 66 | NEE | NextEra Energy, Inc. | -0.6% | 166 | -1.0 | -10.1% |
| 67 | APLD | Applied Digital Corporation | -26.4% | 5.5 | -2.0 | -30.0% |
| 68 | CIFR | Cipher Digital Inc. | -39.2% | 3.7 | -2.4 | -48.9% |
| 69 | LEU | Centrus Energy Corp. | -52.7% | 2.4 | -2.6 | -26.3% |
| 70 | SNPS | Synopsys, Inc. | -3.2% | 87 | -2.9 | +18.3% |
| 71 | VST | Vistra Corp. | -5.8% | 51 | -3.1 | -2.2% |
| 72 | FN | Fabrinet | -20.2% | 13 | -3.3 | -24.0% |
| 73 | SMR | NuScale Power Corporation | -58.9% | 2.4 | -3.4 | -24.5% |
| 74 | RGTI | Rigetti Computing, Inc. | -46.7% | 3.9 | -3.5 | -16.4% |
| 75 | GLXY | Galaxy Digital Inc. | -27.8% | 9.5 | -3.7 | -25.4% |
| 76 | IREN | IREN LIMITED | -28.5% | 11 | -4.2 | -19.7% |
| 77 | CCJ | Cameco Corporation | -10.9% | 36 | -4.3 | -4.8% |
| 78 | QBTS | D-Wave Quantum Inc. | -55.0% | 4.5 | -5.5 | -29.6% |
| 79 | IONQ | IonQ, Inc. | -30.3% | 13 | -5.5 | -10.1% |
| 80 | CRWV | CoreWeave, Inc. | -14.3% | 34 | -5.6 | -16.1% |
| 81 | MDB | MongoDB, Inc. | -19.6% | 27 | -6.6 | +8.1% |
| 82 | OKLO | Oklo Inc. | -61.0% | 5.2 | -8.1 | -32.0% |
| 83 | NOW | ServiceNow, Inc. | -7.3% | 147 | -12 | +37.5% |
| 84 | NRG | NRG Energy, Inc. | -34.8% | 22 | -12 | -19.6% |
| 85 | CRM | Salesforce, Inc. | -9.1% | 198 | -20 | +40.9% |
| 86 | CEG | Constellation Energy Corporatio | -23.2% | 96 | -29 | +7.5% |
| 87 | ADBE | Adobe Inc. | -29.9% | 95 | -41 | +3.4% |
| 88 | IBM | International Business Machines | -21.6% | 219 | -60 | +9.2% |
| 89 | AVGO | Broadcom Inc. | -5.0% | 1,569 | -83 | -11.3% |
| 90 | INTU | Intuit Inc. | -50.9% | 87 | -90 | +11.7% |
| 91 | NFLX | Netflix, Inc. | -24.8% | 294 | -97 | +2.3% |
| 92 | APP | Applovin Corporation | -60.5% | 89 | -137 | -37.3% |
| 93 | ORCL | Oracle Corporation | -34.2% | 389 | -202 | +1.4% |
| 94 | TSLA | Tesla, Inc. | -21.1% | 1,401 | -375 | -6.9% |

### Table 2. Compute/tech universe ranked by YTD % price change

| # | Ticker | Company | YTD % | Last 90d % | Mkt cap now ($B) |
|---|---|---|---|---|---|
| 1 | SNDK | Sandisk Corporation | +447.4% | -4.1% | 188 |
| 2 | 285A.T | KIOXIA HOLDINGS CORPORATION | +284.5% | -23.0% | 45 |
| 3 | DELL | Dell Technologies Inc. | +258.8% | +14.0% | 287 |
| 4 | 3443.TW | GLOBAL UNICHIP CORP. | +252.0% | +95.0% | 31 |
| 5 | MU | Micron Technology, Inc. | +207.9% | +3.5% | 994 |
| 6 | 2454.TW | MEDIATEK INC | +197.2% | +26.1% | 209 |
| 7 | MRVL | Marvell Technology, Inc. | +168.1% | +20.7% | 205 |
| 8 | BE | Bloom Energy Corporation | +154.7% | +3.0% | 65 |
| 9 | AMD | Advanced Micro Devices, Inc. | +146.9% | +6.7% | 863 |
| 10 | INTC | Intel Corporation | +143.5% | -5.5% | 475 |
| 11 | STX | Seagate Technology Holdings PLC | +132.0% | -18.9% | 145 |
| 12 | HPE | Hewlett Packard Enterprise Comp | +129.5% | +20.3% | 73 |
| 13 | LITE | Lumentum Holdings Inc. | +123.6% | +12.5% | 75 |
| 14 | AAOI | Applied Optoelectronics, Inc. | +116.8% | -26.2% | 6.4 |
| 15 | ARM | Arm Holdings plc | +108.4% | -14.7% | 243 |
| 16 | NBIS | Nebius Group N.V. | +97.7% | -6.9% | 45 |
| 17 | 000660.KS | SK hynix | +95.4% | -30.9% | 697 |
| 18 | CRWD | CrowdStrike Holdings, Inc. | +94.0% | +12.0% | 233 |
| 19 | WDC | Western Digital Corporation | +92.7% | -30.4% | 124 |
| 20 | PANW | Palo Alto Networks, Inc. | +85.7% | -4.7% | 280 |
| 21 | 005930.KS | SamsungElec | +79.7% | -15.5% | 1,055 |
| 22 | ALAB | Astera Labs, Inc. | +71.8% | -5.9% | 50 |
| 23 | TER | Teradyne, Inc. | +67.9% | +0.8% | 51 |
| 24 | TXN | Texas Instruments Incorporated | +59.4% | -2.7% | 252 |
| 25 | AMAT | Applied Materials, Inc. | +59.3% | -22.7% | 325 |
| 26 | LRCX | Lam Research Corporation | +54.2% | -15.8% | 330 |
| 27 | ASML | ASML Holding N.V. - New York Re | +53.4% | -6.1% | 633 |
| 28 | PWR | Quanta Services, Inc. | +49.7% | +0.5% | 95 |
| 29 | HUT | Hut 8 Corp. | +47.5% | -25.9% | 8.4 |
| 30 | ADI | Analog Devices, Inc. | +45.8% | +5.3% | 192 |
| 31 | CIEN | Ciena Corporation | +42.3% | -11.1% | 47 |
| 32 | GEV | GE Vernova Inc. | +40.9% | -12.9% | 245 |
| 33 | SNOW | Snowflake Inc. | +39.9% | +14.1% | 108 |
| 34 | CSCO | Cisco Systems, Inc. | +39.4% | -4.1% | 423 |
| 35 | KLAC | KLA Corporation | +36.4% | -22.1% | 216 |
| 36 | TSM | Taiwan Semiconductor Manufactur | +35.8% | +3.6% | 2,140 |
| 37 | ANET | Arista Networks, Inc. | +35.1% | +5.0% | 223 |
| 38 | MPWR | Monolithic Power Systems, Inc. | +33.6% | -7.7% | 60 |
| 39 | ON | ON Semiconductor Corporation | +33.0% | -17.6% | 28 |
| 40 | VRT | Vertiv Holdings, LLC | +27.4% | -28.7% | 79 |
| 41 | AAPL | Apple Inc. | +26.3% | +2.9% | 5,013 |
| 42 | COHR | Coherent Corp. | +26.2% | -16.1% | 46 |
| 43 | ETN | Eaton Corporation, PLC | +22.8% | -2.2% | 152 |
| 44 | CRDO | Credo Technology Group Holding  | +17.9% | -16.3% | 32 |
| 45 | NVDA | NVIDIA Corporation | +14.8% | +5.6% | 5,171 |
| 46 | MSFT | Microsoft Corporation | +9.4% | +34.3% | 3,928 |
| 47 | GOOGL | Alphabet Inc. | +8.8% | -1.8% | 4,165 |
| 48 | AMZN | Amazon.com, Inc. | +8.3% | +1.1% | 2,697 |
| 49 | RIOT | Riot Platforms, Inc. | +8.3% | -24.9% | 5.1 |
| 50 | SMCI | Super Micro Computer, Inc. | +8.1% | +30.8% | 21 |
| 51 | META | Meta Platforms, Inc. | +7.5% | +9.8% | 1,807 |
| 52 | NXPI | NXP Semiconductors N.V. | +5.7% | -14.0% | 58 |
| 53 | 3661.TW | ALCHIP TECHNOLOGIES LIMITD | +5.0% | +5.9% | 9.8 |
| 54 | PLTR | Palantir Technologies Inc. | +4.7% | +40.5% | 447 |
| 55 | CLS | Celestica, Inc. | +2.8% | +0.8% | 38 |
| 56 | CDNS | Cadence Design Systems, Inc. | +2.2% | -3.2% | 88 |
| 57 | WULF | TeraWulf Inc. | +1.7% | -35.7% | 5.8 |
| 58 | QCOM | QUALCOMM Incorporated | +0.6% | +0.2% | 184 |
| 59 | NEE | NextEra Energy, Inc. | -0.6% | -10.1% | 166 |
| 60 | SNPS | Synopsys, Inc. | -3.2% | +18.3% | 87 |
| 61 | AVGO | Broadcom Inc. | -5.0% | -11.3% | 1,569 |
| 62 | TLN | Talen Energy Corporation | -5.3% | -4.7% | 17 |
| 63 | VST | Vistra Corp. | -5.8% | -2.2% | 51 |
| 64 | NOW | ServiceNow, Inc. | -7.3% | +37.5% | 147 |
| 65 | CORZ | Core Scientific, Inc. | -7.4% | -35.6% | 4.3 |
| 66 | CRM | Salesforce, Inc. | -9.1% | +40.9% | 198 |
| 67 | CCJ | Cameco Corporation | -10.9% | -4.8% | 36 |
| 68 | CRWV | CoreWeave, Inc. | -14.3% | -16.1% | 34 |
| 69 | MARA | MARA Holdings, Inc. | -16.1% | -29.6% | 2.9 |
| 70 | CLSK | CleanSpark, Inc. | -17.0% | -35.5% | 2.2 |
| 71 | MDB | MongoDB, Inc. | -19.6% | +8.1% | 27 |
| 72 | FN | Fabrinet | -20.2% | -24.0% | 13 |
| 73 | TSLA | Tesla, Inc. | -21.1% | -6.9% | 1,401 |
| 74 | IBM | International Business Machines | -21.6% | +9.2% | 219 |
| 75 | CEG | Constellation Energy Corporatio | -23.2% | +7.5% | 96 |
| 76 | NFLX | Netflix, Inc. | -24.8% | +2.3% | 294 |
| 77 | APLD | Applied Digital Corporation | -26.4% | -30.0% | 5.5 |
| 78 | GLXY | Galaxy Digital Inc. | -27.8% | -25.4% | 9.5 |
| 79 | IREN | IREN LIMITED | -28.5% | -19.7% | 11 |
| 80 | ADBE | Adobe Inc. | -29.9% | +3.4% | 95 |
| 81 | IONQ | IonQ, Inc. | -30.3% | -10.1% | 13 |
| 82 | BTDR | Bitdeer Technologies Group | -31.1% | -28.1% | 2.1 |
| 83 | CBRS | Cerebras Systems | -31.2% | -26.4% | 30 |
| 84 | ORCL | Oracle Corporation | -34.2% | +1.4% | 389 |
| 85 | NRG | NRG Energy, Inc. | -34.8% | -19.6% | 22 |
| 86 | QUBT | Quantum Computing Inc. | -38.0% | -18.5% | 1.4 |
| 87 | CIFR | Cipher Digital Inc. | -39.2% | -48.9% | 3.7 |
| 88 | NNE | Nano Nuclear Energy Inc. | -46.4% | -19.7% | 0.7 |
| 89 | RGTI | Rigetti Computing, Inc. | -46.7% | -16.4% | 3.9 |
| 90 | INTU | Intuit Inc. | -50.9% | +11.7% | 87 |
| 91 | LEU | Centrus Energy Corp. | -52.7% | -26.3% | 2.4 |
| 92 | QBTS | D-Wave Quantum Inc. | -55.0% | -29.6% | 4.5 |
| 93 | SMR | NuScale Power Corporation | -58.9% | -24.5% | 2.4 |
| 94 | APP | Applovin Corporation | -60.5% | -37.3% | 89 |
| 95 | OKLO | Oklo Inc. | -61.0% | -32.0% | 5.2 |

### Table 3. Biggest USD value creators and destroyers across every name pulled (tech and non-tech, US and Asia)

| # | Ticker | Company | YTD % | Cap added YTD ($B) | In compute universe? |
|---|---|---|---|---|---|
| 1 | AAPL | Apple Inc. | +26.3% | 1,045 | yes |
| 2 | MU | Micron Technology, Inc. | +207.9% | 671 | yes |
| 3 | NVDA | NVIDIA Corporation | +14.8% | 667 | yes |
| 4 | TSM | Taiwan Semiconductor Manufactur | +35.8% | 564 | yes |
| 5 | AMD | Advanced Micro Devices, Inc. | +146.9% | 514 | yes |
| 6 | 005930.KS | SamsungElec | +79.7% | 512 | yes |
| 7 | 000660.KS | SK hynix | +95.4% | 367 | yes |
| 8 | GOOGL | Alphabet Inc. | +8.8% | 337 | yes |
| 9 | MSFT | Microsoft Corporation | +9.4% | 337 | yes |
| 10 | INTC | Intel Corporation | +143.5% | 280 | yes |
| 11 | XOM | ExxonMobil Holdings Corporation | +48.7% | 241 | no |
| 12 | ASML | ASML Holding N.V. - New York Re | +53.4% | 220 | yes |
| 13 | DELL | Dell Technologies Inc. | +258.8% | 207 | yes |
| 14 | AMZN | Amazon.com, Inc. | +8.3% | 207 | yes |
| 15 | LLY | Eli Lilly and Company | +17.0% | 163 | no |
| 16 | SNDK | Sandisk Corporation | +447.4% | 153 | yes |
| 17 | CVX | Chevron Corporation | +47.6% | 142 | no |
| 18 | 2454.TW | MEDIATEK INC | +197.2% | 136 | yes |
| 19 | PANW | Palo Alto Networks, Inc. | +85.7% | 129 | yes |
| 20 | MRVL | Marvell Technology, Inc. | +168.1% | 128 | yes |
| 21 | ARM | Arm Holdings plc | +108.4% | 127 | yes |
| 22 | META | Meta Platforms, Inc. | +7.5% | 125 | yes |
| 23 | AMAT | Applied Materials, Inc. | +59.3% | 121 | yes |
| 24 | CSCO | Cisco Systems, Inc. | +39.4% | 120 | yes |
| 25 | JNJ | Johnson & Johnson | +23.5% | 117 | no |
| ... | | | | | |
| bottom | BKNG | Booking Holdings Inc. Common St | -23.7% | -38 | no |
| bottom | ADBE | Adobe Inc. | -29.9% | -41 | yes |
| bottom | MCD | McDonald's Corporation | -20.5% | -44 | no |
| bottom | TMUS | T-Mobile US, Inc. | -21.6% | -47 | no |
| bottom | ISRG | Intuitive Surgical, Inc. | -24.7% | -49 | no |
| bottom | IBM | International Business Machines | -21.6% | -60 | yes |
| bottom | AVGO | Broadcom Inc. | -5.0% | -83 | yes |
| bottom | INTU | Intuit Inc. | -50.9% | -90 | yes |
| bottom | NFLX | Netflix, Inc. | -24.8% | -97 | yes |
| bottom | APP | Applovin Corporation | -60.5% | -137 | yes |
| bottom | ORCL | Oracle Corporation | -34.2% | -202 | yes |
| bottom | TSLA | Tesla, Inc. | -21.1% | -375 | yes |

## Per-theme price tables

IREN appears under both Neoclouds and Miners, so it is counted in both theme totals. Cerebras (CBRS) is measured from its $185 IPO price and has no cap-added figure.

#### AI chips

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| NVDA | NVIDIA Corporation | 186.50 | 214.13 | +14.8% | +5.6% | 5,171 | 667 |
| AVGO | Broadcom Inc. | 346.10 | 328.75 | -5.0% | -11.3% | 1,569 | -83 |
| AMD | Advanced Micro Devices, Inc. | 214.16 | 528.75 | +146.9% | +6.7% | 863 | 514 |
| MRVL | Marvell Technology, Inc. | 84.98 | 227.83 | +168.1% | +20.7% | 205 | 128 |
| ARM | Arm Holdings plc | 109.31 | 227.80 | +108.4% | -14.7% | 243 | 127 |
| QCOM | QUALCOMM Incorporated | 171.05 | 172.11 | +0.6% | +0.2% | 184 | 1.1 |
| INTC | Intel Corporation | 36.90 | 89.85 | +143.5% | -5.5% | 475 | 280 |
| CBRS | Cerebras Systems | 185.00 | 127.21 | -31.2% | -26.4% | 30 |  |

Theme total cap added YTD: $1,634B

#### Custom silicon / ASIC design (Taiwan)

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| 2454.TW | MEDIATEK INC | 1,430.00 | 4,250.00 | +197.2% | +26.1% | 209 | 136 |
| 3443.TW | GLOBAL UNICHIP CORP. | 2,125.00 | 7,480.00 | +252.0% | +95.0% | 31 | 22 |
| 3661.TW | ALCHIP TECHNOLOGIES LIMITD | 3,510.00 | 3,685.00 | +5.0% | +5.9% | 9.8 | 0.1 |

Theme total cap added YTD: $158B

#### Foundry and equipment

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| TSM | Taiwan Semiconductor Manufactur | 303.89 | 412.61 | +35.8% | +3.6% | 2,140 | 564 |
| ASML | ASML Holding N.V. - New York Re | 1,069.86 | 1,641.52 | +53.4% | -6.1% | 633 | 220 |
| AMAT | Applied Materials, Inc. | 256.99 | 409.29 | +59.3% | -22.7% | 325 | 121 |
| LRCX | Lam Research Corporation | 171.18 | 263.94 | +54.2% | -15.8% | 330 | 116 |
| KLAC | KLA Corporation | 121.51 | 165.74 | +36.4% | -22.1% | 216 | 58 |
| TER | Teradyne, Inc. | 193.56 | 325.00 | +67.9% | +0.8% | 51 | 21 |

Theme total cap added YTD: $1,099B

#### Memory and storage

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| MU | Micron Technology, Inc. | 285.41 | 878.84 | +207.9% | +3.5% | 994 | 671 |
| 000660.KS | SK hynix | 651,000.00 | 1,272,000.00 | +95.4% | -30.9% | 697 | 367 |
| 005930.KS | SamsungElec | 119,900.00 | 215,500.00 | +79.7% | -15.5% | 1,055 | 512 |
| SNDK | Sandisk Corporation | 237.38 | 1,299.30 | +447.4% | -4.1% | 188 | 153 |
| WDC | Western Digital Corporation | 172.27 | 332.00 | +92.7% | -30.4% | 124 | 60 |
| STX | Seagate Technology Holdings PLC | 275.39 | 639.01 | +132.0% | -18.9% | 145 | 83 |
| 285A.T | KIOXIA HOLDINGS CORPORATION | 3,478.33 | 13,375.00 | +284.5% | -23.0% | 45 | 33 |

Theme total cap added YTD: $1,880B

#### Networking and optics

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| ANET | Arista Networks, Inc. | 131.03 | 176.98 | +35.1% | +5.0% | 223 | 58 |
| CSCO | Cisco Systems, Inc. | 77.03 | 107.39 | +39.4% | -4.1% | 423 | 120 |
| COHR | Coherent Corp. | 184.57 | 232.90 | +26.2% | -16.1% | 46 | 9.5 |
| CRDO | Credo Technology Group Holding  | 143.89 | 169.70 | +17.9% | -16.3% | 32 | 4.9 |
| ALAB | Astera Labs, Inc. | 166.36 | 285.82 | +71.8% | -5.9% | 50 | 21 |
| LITE | Lumentum Holdings Inc. | 368.59 | 824.13 | +123.6% | +12.5% | 75 | 41 |
| CIEN | Ciena Corporation | 233.87 | 332.73 | +42.3% | -11.1% | 47 | 14 |
| FN | Fabrinet | 455.28 | 363.51 | -20.2% | -24.0% | 13 | -3.3 |
| AAOI | Applied Optoelectronics, Inc. | 34.86 | 75.57 | +116.8% | -26.2% | 6.4 | 3.5 |
| CLS | Celestica, Inc. | 295.61 | 303.81 | +2.8% | +0.8% | 38 | 1.0 |

Theme total cap added YTD: $269B

#### Servers / systems

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| DELL | Dell Technologies Inc. | 125.88 | 451.71 | +258.8% | +14.0% | 287 | 207 |
| HPE | Hewlett Packard Enterprise Comp | 24.02 | 55.13 | +129.5% | +20.3% | 73 | 41 |
| SMCI | Super Micro Computer, Inc. | 29.27 | 31.63 | +8.1% | +30.8% | 21 | 1.6 |

Theme total cap added YTD: $250B

#### Hyperscalers and mega-cap platforms

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| MSFT | Microsoft Corporation | 483.62 | 529.05 | +9.4% | +34.3% | 3,928 | 337 |
| GOOGL | Alphabet Inc. | 313.00 | 340.59 | +8.8% | -1.8% | 4,165 | 337 |
| AMZN | Amazon.com, Inc. | 230.82 | 250.00 | +8.3% | +1.1% | 2,697 | 207 |
| META | Meta Platforms, Inc. | 660.09 | 709.33 | +7.5% | +9.8% | 1,807 | 125 |
| ORCL | Oracle Corporation | 194.91 | 128.23 | -34.2% | +1.4% | 389 | -202 |
| AAPL | Apple Inc. | 271.86 | 343.46 | +26.3% | +2.9% | 5,013 | 1,045 |
| TSLA | Tesla, Inc. | 449.72 | 354.72 | -21.1% | -6.9% | 1,401 | -375 |

Theme total cap added YTD: $1,475B

#### Neoclouds

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| CRWV | CoreWeave, Inc. | 71.61 | 61.39 | -14.3% | -16.1% | 34 | -5.6 |
| NBIS | Nebius Group N.V. | 83.71 | 165.47 | +97.7% | -6.9% | 45 | 22 |
| IREN | IREN LIMITED | 37.77 | 26.99 | -28.5% | -19.7% | 11 | -4.2 |

Theme total cap added YTD: $12B

#### Power and datacenter infra

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| GEV | GE Vernova Inc. | 653.57 | 921.20 | +40.9% | -12.9% | 245 | 71 |
| VRT | Vertiv Holdings, LLC | 162.01 | 206.42 | +27.4% | -28.7% | 79 | 17 |
| ETN | Eaton Corporation, PLC | 318.51 | 391.25 | +22.8% | -2.2% | 152 | 28 |
| PWR | Quanta Services, Inc. | 422.06 | 631.86 | +49.7% | +0.5% | 95 | 32 |
| BE | Bloom Energy Corporation | 86.89 | 221.31 | +154.7% | +3.0% | 65 | 40 |
| VST | Vistra Corp. | 161.33 | 152.02 | -5.8% | -2.2% | 51 | -3.1 |
| CEG | Constellation Energy Corporatio | 353.27 | 271.37 | -23.2% | +7.5% | 96 | -29 |
| NRG | NRG Energy, Inc. | 159.24 | 103.75 | -34.8% | -19.6% | 22 | -12 |
| TLN | Talen Energy Corporation | 374.84 | 354.81 | -5.3% | -4.7% | 17 | -0.9 |
| NEE | NextEra Energy, Inc. | 80.28 | 79.80 | -0.6% | -10.1% | 166 | -1.0 |

Theme total cap added YTD: $142B

#### Nuclear / SMR / fuel

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| OKLO | Oklo Inc. | 71.76 | 27.97 | -61.0% | -32.0% | 5.2 | -8.1 |
| SMR | NuScale Power Corporation | 14.17 | 5.83 | -58.9% | -24.5% | 2.4 | -3.4 |
| CCJ | Cameco Corporation | 91.49 | 81.52 | -10.9% | -4.8% | 36 | -4.3 |
| LEU | Centrus Energy Corp. | 242.76 | 114.94 | -52.7% | -26.3% | 2.4 | -2.6 |
| NNE | Nano Nuclear Energy Inc. | 24.01 | 12.88 | -46.4% | -19.7% | 0.7 | -0.6 |

Theme total cap added YTD: $-19B

#### Bitcoin miners pivoting to AI/HPC

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| IREN | IREN LIMITED | 37.77 | 26.99 | -28.5% | -19.7% | 11 | -4.2 |
| CIFR | Cipher Digital Inc. | 14.76 | 8.98 | -39.2% | -48.9% | 3.7 | -2.4 |
| WULF | TeraWulf Inc. | 11.49 | 11.68 | +1.7% | -35.7% | 5.8 | 0.1 |
| CORZ | Core Scientific, Inc. | 14.56 | 13.48 | -7.4% | -35.6% | 4.3 | -0.3 |
| HUT | Hut 8 Corp. | 45.94 | 67.75 | +47.5% | -25.9% | 8.4 | 2.7 |
| APLD | Applied Digital Corporation | 24.52 | 18.05 | -26.4% | -30.0% | 5.5 | -2.0 |
| RIOT | Riot Platforms, Inc. | 12.67 | 13.72 | +8.3% | -24.9% | 5.1 | 0.4 |
| MARA | MARA Holdings, Inc. | 8.98 | 7.53 | -16.1% | -29.6% | 2.9 | -0.6 |
| CLSK | CleanSpark, Inc. | 10.12 | 8.40 | -17.0% | -35.5% | 2.2 | -0.4 |
| BTDR | Bitdeer Technologies Group | 11.21 | 7.72 | -31.1% | -28.1% | 2.1 | -0.9 |
| GLXY | Galaxy Digital Inc. | 22.36 | 16.14 | -27.8% | -25.4% | 9.5 | -3.7 |

Theme total cap added YTD: $-11B

#### Quantum

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| IONQ | IonQ, Inc. | 44.87 | 31.28 | -30.3% | -10.1% | 13 | -5.5 |
| RGTI | Rigetti Computing, Inc. | 22.15 | 11.80 | -46.7% | -16.4% | 3.9 | -3.5 |
| QBTS | D-Wave Quantum Inc. | 26.15 | 11.78 | -55.0% | -29.6% | 4.5 | -5.5 |
| QUBT | Quantum Computing Inc. | 10.26 | 6.36 | -38.0% | -18.5% | 1.4 | -0.9 |

Theme total cap added YTD: $-15B

#### Analog / power semis

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| TXN | Texas Instruments Incorporated | 173.49 | 276.48 | +59.4% | -2.7% | 252 | 94 |
| ADI | Analog Devices, Inc. | 271.20 | 395.44 | +45.8% | +5.3% | 192 | 60 |
| MPWR | Monolithic Power Systems, Inc. | 906.36 | 1,210.86 | +33.6% | -7.7% | 60 | 15 |
| NXPI | NXP Semiconductors N.V. | 217.06 | 229.33 | +5.7% | -14.0% | 58 | 3.1 |
| ON | ON Semiconductor Corporation | 54.15 | 72.02 | +33.0% | -17.6% | 28 | 7.0 |

Theme total cap added YTD: $179B

#### EDA

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| SNPS | Synopsys, Inc. | 469.72 | 454.63 | -3.2% | +18.3% | 87 | -2.9 |
| CDNS | Cadence Design Systems, Inc. | 312.58 | 319.51 | +2.2% | -3.2% | 88 | 1.9 |

Theme total cap added YTD: $-1B

#### Software / other tech

| Ticker | Company | Dec 31 2025 close | Oct 16 2026 close | YTD % | Last 90d % | Mkt cap ($B) | Added YTD ($B) |
|---|---|---|---|---|---|---|---|
| PLTR | Palantir Technologies Inc. | 177.75 | 186.03 | +4.7% | +40.5% | 447 | 20 |
| NOW | ServiceNow, Inc. | 153.19 | 141.97 | -7.3% | +37.5% | 147 | -12 |
| CRM | Salesforce, Inc. | 264.91 | 240.70 | -9.1% | +40.9% | 198 | -20 |
| ADBE | Adobe Inc. | 349.99 | 245.20 | -29.9% | +3.4% | 95 | -41 |
| INTU | Intuit Inc. | 662.42 | 325.15 | -50.9% | +11.7% | 87 | -90 |
| APP | Applovin Corporation | 673.82 | 266.13 | -60.5% | -37.3% | 89 | -137 |
| PANW | Palo Alto Networks, Inc. | 184.20 | 342.00 | +85.7% | -4.7% | 280 | 129 |
| CRWD | CrowdStrike Holdings, Inc. | 117.19 | 227.39 | +94.0% | +12.0% | 233 | 113 |
| SNOW | Snowflake Inc. | 219.36 | 306.84 | +39.9% | +14.1% | 108 | 31 |
| MDB | MongoDB, Inc. | 419.69 | 337.57 | -19.6% | +8.1% | 27 | -6.6 |
| IBM | International Business Machines | 296.21 | 232.14 | -21.6% | +9.2% | 219 | -60 |
| NFLX | Netflix, Inc. | 93.76 | 70.53 | -24.8% | +2.3% | 294 | -97 |

Theme total cap added YTD: $-170B

## Per-theme notes and catalysts

All sources below read 2026-10-17. Secondary-source figures are labelled as such; where outlets disagreed, the disagreement is stated.

### AI chips (NVDA, AVGO, AMD, MRVL, ARM, INTC, Cerebras)

- **NVDA (+14.8% YTD, $5.17T, +$667B).** Q2 FY27 (reported 2026-08-26): revenue $96.2B (+106% YoY), Data Center $89.0B (+117%), of which hyperscale $48.7B and "AI Clouds, Industrial and Enterprise" $40.3B; Q3 guide $108.0B +/-2% assuming no China data center compute; Vera Rubin ramping at CoreWeave, Google Cloud, Azure, OCI and Nebius. Source: https://nvidianews.nvidia.com/news/nvidia-announces-financial-results-for-second-quarter-fiscal-2027 (via search summary of the press release; the 75.0% GM and EPS figures are from the same release). Takeaway: fundamentals doubled, multiple compressed; the stock is a laggard relative to its suppliers.
- **AMD (+146.9%, $863B, +$514B).** Catalysts: the OpenAI 6 GW Instinct deal (announced Oct 2025, first 1 GW of MI450 in 2H26, warrants for up to 160M AMD shares) https://finance.yahoo.com/news/amd-signs-ai-chip-supply-105442489.html ; Q2 2026 (2026-08-04) revenue $11.5B, Data Center $6.7B (+107% YoY), Q3 guide ~$13B vs $12.52B consensus, Helios racks shipping to Meta, OpenAI, Oracle; **Anthropic to deploy up to 2 GW of MI450 in Helios, first GW from 1H27**; management guided data center revenue to more than double in 2027 https://www.cnbc.com/2026/08/04/amd-earnings-report-q2-2026.html and AMD 8-K https://www.sec.gov/Archives/edgar/data/0000002488/000000248826000121/q22026991.htm . Oracle commitment to 50,000 MI450 and a Raymond James upgrade cited for a September record high (Benzinga reported +183% YTD at the peak; date of that article not confirmed) https://www.benzinga.com/markets/tech/26/09/61900130/amd-stock-record-high-openai-oracle-mi450-raymond-james .
- **INTC (+143.5%, $475B, +$280B).** US government ~10% stake (433.3M shares at $20.47, $8.9B, Aug 2025), Nvidia $5B and SoftBank $2B investments (2025), reported Apple foundry talks then a WSJ-reported preliminary Apple manufacturing agreement (May 2026, record high ~$130 intraday on May 8), 18A-P in risk production mid-June https://www.cnbc.com/2026/05/05/intel-intc-stock-apple-talks-report.html , https://decrypt.co/367320/intel-all-time-high-chip-deal-apple . No major external 14A customer publicly confirmed as of the sources read; CFO said in late August Intel has "conviction" on 14A customers https://bits-chips.com/article/intel-signals-external-customer-wins-for-14a-foundry-process/ .
- **MRVL (+168.1%, $205B, +$128B, +20.7% 90d).** Fiscal Q4 (2026-03-05) record revenue $2.219B, stock +18% that day; expanded Alphabet custom-silicon agreement tied to the TPU ecosystem with a warrant for Alphabet to buy up to ~$12.2B of Marvell stock; Investor Day 2026-10-06 targeted fiscal 2031 revenue of ~$70-90B with custom ~$30B; optical interconnect guided >70% growth in FY27 (secondary sources: https://www.tikr.com/blog/marvell-stock-has-doubled-in-2026-analysts-are-still-playing-catch-up , https://247wallst.com/investing/2026/10/06/marvell-rallies-6-as-investor-day-presses-the-custom-ai-silicon-case-broadcom-advances-4/ , https://www.techtimes.com/articles/324740/20260817/marvell-interconnect-outpacing-custom-silicon-ahead-aug-27-earnings-call.htm ). Note: stock was $287.59 on Oct 6 vs $227.83 on Oct 16, so the last week's selloff took ~21% off.
- **AVGO (-5.0%, $1.57T, -$83B).** Fiscal Q3 (2026-09-02): AI semi revenue $16.7B (+221% YoY), Q4 AI guide $21.7B, FY26 AI ~ $58B, outlined ~$115B FY27 and ~$230B FY28 AI revenue https://seekingalpha.com/news/4639799-broadcom-forecasts-58b-fiscal-2026-ai-revenue-and-outlines-115b-in-2027-230b-in-2028 ; stock fell on a disappointing current-quarter total revenue forecast, gross margin pressure (~73% Q4, memory content and XPU mix) and BofA flagging reliance on Anthropic and OpenAI https://www.bloomberg.com/news/articles/2026-09-02/broadcom-forecast-misses-estimates-disappointing-investors , https://www.cnbc.com/2026/09/02/broadcom-avgo-q3-earnings-report-2026.html . Lesson: the market discounted a custom-silicon vendor whose margin is being eaten by memory costs.
- **ARM (+108.4%, -14.7% 90d), QCOM (+0.6%).** Both reported disappointing results in the Oct 16 week per https://congress.net/tech-selloff-wipes-billions-from-nasdaq-as-chip-and-software-stocks-crumble-together/ (thin source; specifics not verified).
- **Cerebras (CBRS).** IPO 2026-05-14 at $185 (30M shares, $5.5B raised), first close $311 (+68%) https://techcrunch.com/2026/05/14/cerebras-raises-5-5b-kicking-off-2026s-ipo-season-with-a-bang/ ; lockup expired Oct 1, post-IPO low and Nvidia pressure https://www.cnbc.com/2026/10/02/cerebras-stock-hits-post-ipo-low-on-nvidia-pressure-lockup-expiration.html . Yahoo: $127.21 on Oct 16, cap $30.2B, -31% vs IPO price, -26% over 90d.
- **Lambda:** no listing found (Yahoo has no LMDA quote; searches returned no IPO). Treat as private.

### Custom silicon design houses (Taiwan)

- **MediaTek (2454.TW, +197.2%, ~$209B, +$136B) and Global Unichip (3443.TW, +252.0%, +95.0% in 90d, ~$31B).** Driver: Google TPU ASIC work; MediaTek +19% in two days in late January on the Google tie-up https://www.taipeitimes.com/News/biz/archives/2026/01/27/2003851260 ; TrendForce: CSPs accelerating ASIC push in 2H26, MediaTek, GUC, Alchip benefit https://www.trendforce.com/news/2026/03/20/news-csps-accelerate-asic-push-in-2h26-challenging-nvidia-as-mediatek-guc-alchip-benefit/ ; DigiTimes: MediaTek ASIC revenue may overtake smartphone chips https://www.digitimes.com/news/a20260427PD208/mediatek-asic-revenue-google-tpu.html . Risks reported: Google order delays, rumors of a future TPU shifting to Marvell. Alchip (3661.TW) only +5.0%.

### Foundry and equipment (TSM, ASML, AMAT, LRCX, KLAC, TER)

- **TSM (+35.8%, $2.14T, +$564B).** Q3 2026 (2026-10-15): revenue ~$46.7B vs guide $44.6-45.8B, HPC 68% of sales, GM 66.4% (2nm ramp dilution), Q4 guide $52.3-54.3B, full-year 2026 USD growth raised to ~44%, 2026 capex held at $60-64B, CEO says AI demand still exceeds supply https://www.crnasia.com/news/2026/components-and-peripherals/tsmc-third-quarter-revenue-tops-guidance-as-ceo-says-ai-dema , https://xenospectrum.com/en/tsmc-q3-2026-gross-margin-capex/ .
- **ASML +53.4%, AMAT +59.3%, LRCX +54.2%, KLAC +36.4%, TER +67.9%.** All strong YTD, but the 90d column is ugly (AMAT -22.7%, KLAC -22.1%, LRCX -15.8%), consistent with the October chip selloff. Specific 2026 equipment catalysts (orders, guidance) were not researched in depth in this slice; Teradyne's gain lines up with HBM/AI test demand but that link was not sourced.

### Memory and storage (MU, SK hynix, Samsung, SanDisk, Seagate, WDC, Kioxia)

The single biggest value pool of 2026: about $1.88T of USD cap added across the seven names in Table 1.
- **MU (+207.9%, $994B, +$671B).** FQ4 2026 (2026-09-30): beat and strong guide, data center revenue up ~11-fold, DRAM revenue $39.8B (+343% YoY, 73% of sales) https://www.cnbc.com/2026/09/30/micron-mu-q4-earnings-report-2026.html (via search summary; CNBC page returned 403 to direct fetch). Secondary: Micron's 2026 HBM output is sold out under contract; DRAM spot +52% since January (https://www.investing.com/analysis/micron-earnings-preview-ai-memory-rally-faces-reality-check-200682720). Morgan Stanley bear case $675 if memory turns in 2027 (same search summary, not independently fetched).
- **SK hynix (+95.4%, ~$697B, +$367B, -30.9% 90d) and Samsung (+79.7%, ~$1,055B per Yahoo, +$512B, -15.5% 90d).** Both reported as the sole HBM4 suppliers for Nvidia Vera Rubin; Counterpoint-cited share shift in HBM with Samsung rising from 21% (Q1) to 33% (Q2) and SK hynix 58% to 50%, while another source has SK hynix at 62%; sources conflict https://www.astutegroup.com/news/general/sk-hynix-holds-62-of-hbm-micron-overtakes-samsung-2026-battle-pivots-to-hbm4/ , https://siliconanalysts.com/analysis/hbm4-market-share-race-2026 . A reported 117% one-week HBM4 contract price jump is unconfirmed https://tech-insider.org/hbm4-memory-shortage-amd-samsung-sk-hynix-price-spike-2026/ .
- **SanDisk (+447.4%, $188B; best % performer in the universe).** Enterprise SSD/NAND for AI inference: SanDisk doubled enterprise 3D NAND pricing in Q1 2026 https://www.tomshardware.com/pc-components/ssds/sandisk-to-double-price-of-3d-nand-for-enterprise-ssds-in-q1-2026-hyperscalers-to-pay-top-dollar-for-storage-as-ai-continues-to-roll ; TrendForce sees enterprise SSD prices up ~235% in 2026 and NAND contract +15-20% QoQ in 4Q26 (secondary). Stock closed $1,394.83 on Oct 15, ~41% below its June 22 high of $2,354.39; earnings Oct 29 https://admiralmarkets.com/education/articles/shares/sandisk-stock-sndk-earnings-preview .
- **Seagate +132%, WDC +92.7%, Kioxia +284.5%**: same storage-pricing tailwind (not sourced individually beyond prices).
- Second-order effect: memory cost inflation is now raising everyone else's capex. Amazon attributed its capex raise to ~$220B mainly to higher memory costs; Microsoft's figure was reported to include ~$25B tied to component pricing; Broadcom cited memory content for its margin squeeze (sources in capex section and AVGO above).

### Networking and optics (ANET, CSCO, COHR, CRDO, ALAB, LITE, CIEN, FN, AAOI, CLS)

- **Lumentum (+123.6%, $75B).** FQ4 FY26: revenue ~$1.01B (+109% YoY), adj EPS $3.23 vs $2.95, FQ1 guide $1.225-1.275B, supply-constrained on high-power lasers, incremental multi-hundred-million-dollar co-packaged optics order for 1H CY27, optical circuit switch backlog > $400M https://investor.lumentum.com/financial-news-releases/news-details/2026/Lumentum-Announces-Fourth-Quarter-and-Full-Fiscal-Year-2026-Results/default.aspx .
- **Astera Labs +71.8%, Cisco +39.4%, Ciena +42.3%, Arista +35.1%, AAOI +116.8%.** Coherent +26.2%, Credo +17.9%, Celestica +2.8%; Fabrinet -20.2% is the notable loser. Catalysts for these individually were not found in this pass (searches on Astera returned nothing usable); figures are price data only.

### Servers and systems (DELL, HPE, SMCI)

- **Dell (+258.8%, $287B, +$207B).** FQ2 FY27 (reported 2026-09-01): revenue $47B (+58%), AI server revenue $16.4B (doubled), record $60.9B AI orders, record **$95B AI backlog**, FY27 revenue guide raised by $25B to $192B, AI server FY guide to $74B from $60B (secondary summaries of Dell's 8-K) https://www.fool.com/investing/2026/09/28/dells-ai-server-backlog-has-ballooned-to-95-billio/ , https://finance.yahoo.com/markets/article/dell-stock-surges-on-record-orders-for-ai-servers-205629050.html .
- **HPE +129.5%.** Catalyst not researched. **SMCI +8.1%** (lagged peers).

### Hyperscalers, platforms and capex (MSFT, GOOGL, AMZN, META, ORCL, AAPL)

- **2026 capex guidance (as of late-July Q2 reports, tracker updated 2026-10-06):** Microsoft ~$175B calendar 2026 (from ~$190B; the cut reflects a lease-accounting reclassification, plan unchanged), Alphabet $195-205B (raised twice from $175-185B), Amazon ~$220B (from ~$200B, mainly memory costs), Meta $130-145B. **Big four total $720-745B (midpoint ~$732.5B).** Also Oracle fiscal 2027 capex $90-95B and CoreWeave 2026 capex $35-39B (not in that total) https://www.yieldtheory.app/research/hyperscaler-ai-capex-tracker-2026 . FactSet puts calendar 2026 near **$800B** including finance leases and customer prepayments and notes capex is outrunning cash flow, forcing external financing https://insight.factset.com/hyperscalers-tap-external-financing-as-ai-capex-outruns-cash-flow . Add TSMC's $60-64B and the tracked AI-infra capex pool is roughly $900B+ for 2026 (sum of the cited ranges; definitions differ, so this is a scale indicator, not an audited total).
- **MSFT (+9.4% YTD but +34.3% in 90d).** Fell to $352.83 on June 25 (lowest close in a year), then FQ4 (late July): Azure +43%, Azure > $100B annual revenue, stock +15.5% next day, commercial backlog $678B (+84%; +25% ex-OpenAI), next-quarter Azure guide ~45%; best quarter since 1998 https://www.geekwire.com/2026/microsofts-wild-ride-stock-hits-new-record-after-a-year-of-ups-and-downs/ , https://finance.yahoo.com/markets/stocks/articles/msft-stock-posts-best-quarter-084616308.html . Next report Oct 28.
- **GOOGL +8.8%, AMZN +8.3%, META +7.5%.** All below the S&P. META: Bloomberg (2026-07-01) reported Meta is building "Meta Compute" to sell excess AI capacity, either as hosted models or raw compute like CoreWeave; CoreWeave and Nebius fell sharply on it https://thenextweb.com/news/meta-cloud-business-excess-ai-compute , https://247wallst.com/investing/2026/07/01/nebius-coreweave-and-iren-tumble-on-metas-cloud-ambitions-is-this-the-end-of-the-neocloud-boom/ . Unconfirmed by Meta.
- **ORCL (-34.2%, -$202B: the largest compute-sector value destroyer after Tesla).** RPO $664B in FQ1 FY27 with the ~$300B OpenAI contract estimated (by analysts, not Oracle) at ~45% of it; quarterly capex $28.5B vs $2.3B two years earlier; long-term debt reported > $160B; negative FCF; plans to raise $45-50B; Oct 8 drop on reports OpenAI annualized revenue is ~$20B short of prior estimates https://finance.yahoo.com/technology/article/oracle-stock-falls-on-more-debt-to-fund-ai-chip-buying-openai-revenue-disclosure-180233070.html , https://www.useluminix.com/reports/company-overviews/oracle-ai-debt-openai-contract .
- **AAPL (+26.3%, +$1,045B: the single largest USD adder of 2026).** Not a compute-capex story: Siri AI upgrade landing, iPhone Duo foldable, iPhone 17 demand, $100B buyback, and investors rewarding Apple for not levering up for AI; record intraday $348.15 on Oct 13 https://appleinsider.com/articles/26/09/21/apple-shares-hit-record-high-after-iphone-duo-and-siri-ai-success , https://www.tradingkey.com/analysis/stocks/us-stocks/262214559-apple-stock-breaks-all-time-high-348-smart-home-iphone-share-tradingkey . The Gemini-powers-Siri angle appears in commentary only and is unconfirmed.
- **TSLA -21.1% (-$375B)** is the largest destroyer in the pulled set; not researched further (outside compute scope).

### Neoclouds (CoreWeave, Nebius, IREN, Cerebras cloud)

- **CoreWeave (-14.3% YTD, $33.9B per Yahoo, -16.1% 90d).** Q2 2026 (2026-08-11): revenue ~$2.58B (+112%), backlog ~$104B at June 30 (one source: ~$129B including early Q3 commitments), FY26 revenue guide $12.4-13.2B, ~$35B debt, ~$640M quarterly net interest with Q3 interest guided $860-940M; stock +20% on Aug 12 https://www.nasdaq.com/press-release/coreweave-reports-strong-second-quarter-2026-results-2026-08-11 , https://khancapitals.com/coreweave-q2-2026-earnings/ . Then fell 15.5% on Oct 14 (rates/CPI) and ~12% on Oct 16 on debt-load worries (sources in method section). Meta deals: $14.2B plus a $21B expansion through Dec 2032 (secondary) https://www.fool.com/investing/2026/08/21/coreweave-vs-nebius-group-which-artificial-intelli/ .
- **Nebius (+97.7%, $45B).** Meta deal up to $27B over five years (March 2026), Microsoft ~$17.4B expandable to $19.4B, Q2 revenue $582.3M (+454% YoY) (secondary: same Motley Fool link; https://finance.yahoo.com/markets/stocks/articles/nebius-vs-coreweave-better-ai-125951416.html ). Running Vera Rubin racks per Nvidia's Q2 release.
- Read-across: the market paid for the neocloud that grew revenue fastest with the least debt, and punished the one with the biggest interest bill, even with a $100B+ backlog.

### Power and datacenter infrastructure (GEV, VRT, ETN, PWR, BE, VST, CEG, NRG, TLN)

- **Bloom Energy (+154.7%, $65B, +$40B).** FY25 revenue $2.02B, ~$20B total backlog; Oracle Project Jupiter (New Mexico) to be fully powered by Bloom fuel cells, up to 2.45 GW; $25B Brookfield financing deal cited for a +34% September https://www.fool.com/coverage/better-buy/2026/09/15/bloom-energy-vs-ge-vernova-which-stock-is-a-better-buy-in-2026/ . Short-seller Hunterbrook disputes the backlog composition https://hntrbrk.com/investigations/bloom (non-neutral source).
- **GE Vernova (+40.9%, $245B, +$71B).** Q1 2026 orders +71% organic to $18.3B, backlog $163B, gas turbine backlog + slot reservations 83 to 100 GW in one quarter, target >= 110 GW by end-2026; Q2 backlog reported $176B and >$5B data center orders YTD (secondary) https://www.fool.com/coverage/better-buy/2026/09/15/bloom-energy-vs-ge-vernova-which-stock-is-a-better-buy-in-2026/ .
- **Vertiv (+27.4% YTD but -28.7% 90d).** More than doubled in 1H26 (+106.7%), 52-week high $379.93 in mid-May, backlog $15B (doubled), 2026 guide $13.5B sales +28% organic https://www.fool.com/investing/2026/07/22/why-vertiv-stock-zoomed-107-in-just-six-months-of/ ; now $206. **Quanta +49.7%, Eaton +22.8%.**
- **IPPs lagged:** Vistra -5.8%, Constellation -23.2%, NRG -34.8%, Talen -5.3%, NextEra -0.6%. Specific causes were not researched; plausible drivers (rates, PPA pricing) are unverified, so not asserted.

### Nuclear, SMR and fuel (OKLO, SMR, CCJ, LEU, NNE)

- Every name down: OKLO -61.0%, SMR -58.9%, LEU -52.7%, NNE -46.4%, CCJ -10.9%. Oklo: equity raises with no revenue, Aurora not yet NRC-approved, fading 2025 hype; NuScale: TD Cowen downgrade on a possible Romania delay to 2034, $507.4M ENTRA1 milestone charge, Q1 revenue $565K, Fluor fully exited (~$2.4B) https://www.nasdaq.com/articles/why-oklo-stock-sank-27-first-half-2026 , https://www.nasdaq.com/articles/smr-down-58-6-months-nuclear-stock-buy-dip .

### Bitcoin miners pivoting to AI/HPC (IREN, CIFR, WULF, CORZ, HUT, APLD, plus RIOT, MARA, CLSK, BTDR, GLXY)

- Only HUT (+47.5%), RIOT (+8.3%) and WULF (+1.7%) are up YTD; everything else is down, and **every name is down 20-49% over 90d**. Cipher -39.2% (-48.9% 90d), IREN -28.5%, APLD -26.4%, CORZ -7.4%, GLXY -27.8%.
- This happened despite contract wins: TeraWulf reports > $27B long-term contracted revenue across 839 MW including a 20-year 401 MW agreement with Anthropic; Cipher's Barber Lake contracted revenue rose from ~$3.8B to > $9B with a 20-year extension (stock still fell 3% that day) https://247wallst.com/investing/2026/09/30/cipher-digital-slips-3-despite-20-year-barber-lake-lease-extension-terawulf-retreats-2-iren-limited-treads-water/ . IREN carries $3.7B of converts across five series; TeraWulf $5.7B total debt; Oct 14 the miner/digital-power ETF fell 10.7% on a hot CPI and higher yields https://www.altcoinbuzz.io/bitcoin-miners-drop-12-as-ai-data-center-debt-reprices . Valuation split cited: miners with HPC contracts ~12.3x NTM sales vs pure miners ~5.9x (secondary, same search; verify before reuse).
- Read-across: the market values the HPC conversion but prices the financing risk of building it; signed MW is not enough when funded with converts at 5%+ yields.

### Quantum (IONQ, RGTI, QBTS, QUBT)

- All down: IONQ -30.3%, RGTI -46.7%, QBTS -55.0%, QUBT -38.0%. Drivers: risk-off on pre-profit high-multiple names, IonQ dilution (a $2B equity offering plus a $1.3B all-stock Elisity acquisition in October), D-Wave CFO retirement, yields above 5.3% (secondary) https://247wallst.com/investing/2026/10/14/ionq-falls-6-as-2b-equity-offering-follows-1-3b-elisity-stock-deal-d-wave-and-rigetti-retreat-4/ , https://247wallst.com/investing/2026/07/29/ionq-rigetti-and-d-wave-quantum-are-down-30-in-a-month-is-more-pain-coming-for-quantum-computing-stocks/ .

### Everything else that ran (or broke)

- **Security software ran:** CrowdStrike +94.0%, Palo Alto +85.7%. **App-layer and SaaS broke:** Intuit -50.9%, AppLovin -60.5%, Adobe -29.9%, ServiceNow -7.3%, Salesforce -9.1%, IBM -21.6%, consistent with the "AI erodes software" fear cited in the Oct 16 selloff coverage. Catalysts for the security names were not researched.
- **Analog/power semis:** TXN +59.4%, ADI +45.8%, MPWR +33.6%.
- **Outside tech, energy was the other big winner** (XOM +48.7% / +$241B, CVX +47.6%, COP +56.5%) against a Brent ~$110 backdrop; a Strait of Hormuz conflict is referenced as a July risk-off driver https://247wallst.com/investing/2026/07/13/ionq-tumbles-8-d-wave-rigetti-quantum-computing-drop-6-as-risk-off-ripple-effects-hit-speculative-quantum-stocks/ .

## What the market pays for in compute right now

Ranked by evidence in the price data above, strongest first.

1. **Memory bandwidth and capacity (HBM, DRAM, enterprise SSD).** The largest value pool of 2026 (~$1.88T added across seven memory/storage names; SanDisk +447%, Micron +208%). Supply is sold out under contract, prices are rising fast enough to inflate hyperscaler capex and squeeze Broadcom's margin. The market treats memory, not FLOPs, as the binding constraint for training and especially inference (KV cache, long context, retrieval storage).
2. **A second source of accelerators and custom ASICs.** AMD (+147%), Marvell (+168%), MediaTek (+197%), GUC (+252%), Intel (+144%) all beat Nvidia (+15%). Buyers (OpenAI, Anthropic, Google, Meta) are paying to avoid single-vendor dependence; the multi-GW contracts (OpenAI 6 GW and Anthropic 2 GW with AMD; Alphabet with Marvell; Google TPU via MediaTek) are the catalysts.
3. **Leading-edge wafer capacity.** TSMC +$564B; demand "still exceeds supply" per its CEO; 2026 growth raised to ~44%.
4. **Whoever holds a funded, deliverable backlog.** Dell's $95B AI backlog (+259%), GE Vernova's 100+ GW turbine queue, Lumentum's laser-constrained order book. Backlog that depends on a single levered customer (Oracle/OpenAI, CoreWeave's debt stack) was punished.
5. **Power that can be delivered now, behind the meter.** Bloom (+155%) and turbines (GEV) won; nuclear SMRs and IPPs lost. The market pays for megawatts in 2026-27, not in 2030+.
6. **Interconnect and optics.** Lumentum (+124%), Astera (+72%), AAOI (+117%), Marvell's optical guide (>70% growth). Scale-out and co-packaged optics are a paid bottleneck, though the group sold off hard in the last 90 days.

What the market is **not** paying for right now: leverage-funded GPU rental (CoreWeave, Oracle, most miner-to-HPC conversions), pre-revenue optionality (quantum, SMRs), and the hyperscalers' own equity (all single-digit YTD despite $720-745B of capex). Money flows **through** the hyperscalers to suppliers of scarce inputs.

**Where the money is flowing (2026 capex):** big four $720-745B (yieldtheory, updated 2026-10-06), ~ $800B including leases and prepayments (FactSet), plus Oracle FY27 $90-95B, CoreWeave $35-39B, TSMC $60-64B. Inference is the explicit growth driver cited for enterprise SSD pricing (TrendForce via search) and for Nvidia's "AI Clouds, Industrial and Enterprise" bucket ($40.3B of Q2 data center revenue).

### Read-across for pearl-sol (interpretation, not data)

- A useful-PoW chain doing matmul is selling **FLOPs**, the one input the market is currently pricing as least scarce (Nvidia +15% on doubled revenue). Narratives that line up with what is actually being paid for: memory-heavy inference jobs (bandwidth and KV-cache, not raw matmul), a "second source" of compute outside a single vendor or cloud, and capacity that is deliverable now on existing power.
- Meta's reported plan to sell excess capacity (July 2026) and the neocloud selloff say spare capacity is getting cheaper to buy; a job marketplace that aggregates idle capacity matches that trend, while one that needs to finance new GPUs mirrors the CoreWeave/miner trade the market just punished.
- For agent credit lines: the 2026 tape punished debt-funded compute (Oracle, CoreWeave interest bill, miner converts) when rates rose. Credit underwritten against contracted, short-duration job revenue is the defensible framing; credit against future capacity build is the framing the market just sold.

