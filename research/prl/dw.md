We use cookies to give user the best experience.

Read our [Privacy Policy](https://www.datawallet.com/privacy-policy) for more details.

[Preferences](https://www.datawallet.com/crypto/pearl-explained#)

[Deny](https://www.datawallet.com/crypto/pearl-explained#) [Accept](https://www.datawallet.com/crypto/pearl-explained#)

Privacy Preference Center

When you visit our website, data might be stored or retrieved in your browser. This storage is often essential for the core functionality of the site. Additionally, storage can be utilized for purposes such as marketing, analytics, and personalizing your experience by saving your preferences. We value your privacy and offer you the ability to disable certain types of storage that are not critical for the basic operation of the site. Please note that blocking these categories may affect your website experience.

[Reject all cookies](https://www.datawallet.com/crypto/pearl-explained#) [Allow all cookies](https://www.datawallet.com/crypto/pearl-explained#)

Manage Consent Preferences by Category

Essential

**Always Active**

These items need to be active and are required to enable basic website functionality.

Marketing

Essential

These items help deliver advertising that is more relevant to your interests. They can also limit the number of times you see an ad and measure the effectiveness of advertising campaigns. Typically, advertising networks place these items with the website operator’s permission.

Personalization

Essential

These items allow the website to remember choices you make, such as your username, language, or region, and provide enhanced, more personalized features. For example, storing data about your current location enables the site to offer local weather reports or traffic news.

Analytics

Essential

These items help the website operator understand how the site performs, how visitors interact with it, and whether there are any technical issues. This type of storage typically doesn't collect information that identifies a visitor.

[Confirm my preferences](https://www.datawallet.com/crypto/pearl-explained#)

# Pearl Explained: Proof of Useful Work, AI Mining & PRL Tokenomics

![Datawallet Team](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/69c5187fb98a5743c20090ae_69990de6192b4155c0133f60_Datawallet%2520Team.webp)

Written by

[Datawallet Team](https://www.datawallet.com/authors/datawallet-team)

Last updated

October 3, 2026

This date marks a full audit, not a minor edit. Our editing team reviews every claim, figure, and platform detail in line with our [editorial guidelines](https://www.datawallet.com/editorial-methodology) before republishing.

Fact checked

Editorially Verified

Editorial fact-check process

This article has been reviewed and verified for accuracy by our editorial team. All claims, data points and platform details are cross-referenced against primary sources.

Data accuracy verified

Sources cross-referenced

Platform details confirmed

[View our fact-checking process](https://www.datawallet.com/editorial-methodology)

Disclaimer

Affiliate Disclosure

How Datawallet is funded

Some links on this page are affiliate links. Datawallet may earn a commission when you sign up through them, at no extra cost to you. Ratings and rankings reflect our own testing and assessment criteria.

[Read our full disclosure](https://www.datawallet.com/affiliate-disclosure)

**Summary:** Pearl is a Bitcoin-derived blockchain that rewards miners with PRL for running matrix multiplication on graphics cards instead of hashing. This arithmetic underlies every AI model. Supply is capped at 2.1 billion PRL, with about 332 million mined by October 2, 2026.

Published cryptography research supports the design, and Together AI is its sole production partner. An independent study, however, found that most mining produced no AI output. More than a third of all existing PRL was also minted in the first six days.

## What is Pearl?

[Pearl](https://pearlresearch.ai/network) is a Layer 1 blockchain, an independent network with its own coin, that replaces Bitcoin's hashing with matrix multiplication. AI training and inference rely on this arithmetic. A graphics processing unit (GPU) serving a language model can therefore compete for block rewards at the same time.

Known as Proof of Useful Work, the approach extracts a mining proof from a computation the miner wanted to run anyway. Standard proof of work instead spends electricity on puzzles with no outside value. Pearl's method comes from a [2025 paper by Ilan Komargodski and Omri Weinstein](https://arxiv.org/abs/2504.09971), which claims that producing the proof adds almost no cost to the multiplication.

Pearl Research Labs launched mainnet on April 27, 2026. Its node software forks btcd, a Bitcoin implementation written in Go, retaining Bitcoin's transaction model and the rule that the chain with the most accumulated work wins. The native coin, PRL, has a hard cap of 2.1 billion. The project writes it as ¶PRL.

Most [AI crypto coins](https://www.datawallet.com/crypto/best-ai-crypto-coins) use team-issued tokens to sell compute or coordinate models. PRL has neither a token sale nor a contract address. All coins enter circulation through block rewards, making the mining rules and launch history central to its distribution.

![What is Pearl?](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3ebfd8c41f53c3109955_01_what-is-pearl%201.webp)

## How Does Pearl Work?

Pearl retains Bitcoin's ledger but replaces its mining function. The main differences concern how miners win blocks, prove their work and adopt upgrades.

### 1\. Mining with Matrix Multiplication

Neural networks largely consist of grids of numbers, called matrices, multiplied together billions of times. To mine Pearl, an operator chooses two matrices and computes their product on a GPU. Typically, these are a model's weights and the data passing through it.

The protocol divides the output into small tiles, each acting as a lottery ticket. A tile wins if its hash, a fixed-length fingerprint of the data, falls below the network target. More arithmetic produces more tiles, increasing the miner's chance of finding a block.

Letting miners choose their inputs creates an obvious shortcut. A dishonest miner could submit easy matrices, such as ones filled with zeros. Pearl prevents this by adding random noise to both inputs. In the [original Pearl whitepaper](https://pearlresearch.ai/research/int-whitepaper), that noise comes from low-rank matrices, a compact form that miners can subtract afterwards to recover the true result at little cost.

Before computing, the miner must commit to both matrices and recent block data through BLAKE3, a fast hash function. This commitment generates the noise, preventing miners from choosing inputs that cancel it or reusing old work on a new block.

Difficulty adjusts after every block using a weighted moving average with a one-week half-life. This setting controls how hard blocks are to find, targeting one every 194 seconds, a little over three minutes.

Official mining uses a plugin for vLLM, an open-source engine for serving language models. The plugin replaces the model's multiplication routine with Pearl's version, NoisyGEMM.

![](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3ecce332dcaa9ffa759b_02_mining-with-matrix-multiplication%201.webp)

### 2\. Zero-Knowledge Block Certificates

A winning tile exposes rows from the miner's matrices, potentially revealing proprietary model weights or user data. Raw proofs would also be bulky. The whitepaper estimates that a proof for two 16,384 by 16,384 matrices would occupy 0.5 to 2 megabytes.

To avoid publishing those inputs, Pearl wraps each winning proof in a zk-SNARK. This zero-knowledge proof confirms that the computation was performed correctly without revealing its inputs. [Zcash](https://www.datawallet.com/crypto/zcash-explained) uses the same family of cryptography to hide payment details. The technique also overlaps with [zkML](https://www.datawallet.com/crypto/what-is-zkml), which focuses on proving AI computations.

Pearl uses Plonky2, a proof system built only on hash functions. Three layers of recursion compress the certificate below 60 kilobytes. Each block carries this certificate, subject to a 65-kilobyte cap, instead of Bitcoin's nonce, the number Bitcoin miners vary to find a valid hash.

![](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3ed87087df0ec02818bd_03_zero-knowledge-block-certificates%201.webp)

### 3\. Bitcoin-Derived Base Layer

Outside mining, Pearl follows Bitcoin closely. Coins are tracked as unspent transaction outputs (UTXOs), discrete chunks of value consumed and recreated with each payment. Transaction fees go to miners through the same highest-bidder auction. One hundred-millionth of a PRL, the smallest unit, is called a grain.

**Where Pearl departs from Bitcoin:**

- **Addresses:** Pearl supports only Taproot, Bitcoin's newest address format. Addresses begin with prl1.
- **Quantum preparation:** Wallets embed an inactive backup key for XMSS, a hash-based signature scheme designed to resist quantum computers. The key stays dormant until needed. Nodes also implement the Pay-to-Merkle-Root outputs proposed in [BIP-360](https://www.datawallet.com/crypto/bip-360-explained).
- **Scripting:** OP\_CAT, an opcode that joins two pieces of data, is enabled with a 520-byte output limit. It remains disabled on Bitcoin.
- **Timestamps:** Pearl rejects blocks dated more than five minutes into the future. Bitcoin allows two hours.

![](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3ee9214c97825115e9ff_04_bitcoin-derived-base-layer%201.webp)

### 4\. Protocol Upgrades and the FP8 Scheme

The [node's mainnet parameters](https://github.com/pearl-research-labs/pearl/blob/master/node/chaincfg/params.go) record four mining-rule changes in Pearl's first four months. Hard forks require every node to upgrade. Soft forks only tighten existing rules.

**The four forks to date:**

- **MoE hard fork (block 71,935, June):** Added version 2 certificates and support for mixture-of-experts (MoE) models. These models route each input through a subset of specialist sub-networks.
- **Dense-only soft fork (block 91,630, July):** Ended acceptance of MoE proofs, restricting mining to standard multiplications again. Pearl's code comments do not explain the reversal.
- **Rank-penalty soft fork (block 96,251, August):** Established a minimum amount of noise for each multiplication and imposed a stricter winning threshold on proofs that use less.
- **Salted noise-seed hard fork (block 99,000, August):** Introduced version 3 certificates and changed how the noise seed is derived.

Pearl launched with integer-based mining, while production AI models increasingly use FP8, an 8-bit floating-point number format. A [floating-point specification](https://pearlresearch.ai/Pearl_Whitepaper.pdf) published in September 2026 describes mining directly on FP8 multiplications. It relies on quantisation, the rounding of numbers into that format, to eliminate shortcuts.

The verification method draws on [Hawkeye](https://arxiv.org/abs/2603.20421), research co-authored with Stanford cryptographer Dan Boneh that reproduces a GPU's exact arithmetic on a CPU. Our October 2 check of the node's mainnet parameters found no activation height for FP8. It remains a published design until a fork is scheduled.

## How to Mine Pearl

To mine Pearl, you need a supported GPU, the Pearl node and wallet, plus either a community miner or the vLLM miner from the [official Pearl repository](https://github.com/pearl-research-labs/pearl).

**From hardware to first payout:**

01. **Check hardware:** Official miner documentation supports only Nvidia H100 and H200 data-centre cards. Community miners also support consumer RTX cards.
02. **Install the software:** Use the Pearl repository's installer to obtain pearld, the full node, and Oyster, the wallet.
03. **Create a wallet:** Open oystercli, set a passphrase and record the seed phrase offline.
04. **Generate an address:** Request a new Taproot address from the wallet for block rewards.
05. **Start the node:** Run pearld with your mining address set, then let it sync with the network.
06. **Start the gateway:** Launch pearl-gateway to pass work between the node and GPU.
07. **Load a model:** Start the vLLM miner using a Pearl-converted model from the project's Hugging Face page, such as its Llama 3.3 70B build.
08. **Or join a pool:** Connect a community miner to a pool for smaller, steadier payouts instead of rare full blocks.
09. **Wait for maturity:** Newly mined coins become spendable after 100 further blocks, roughly five and a half hours.
10. **Track costs:** Regularly compare payouts with electricity or rental costs. The reward per block falls continuously.

As more miners join, mining income falls quickly. [Hashrate Index reported](https://hashrateindex.com/blog/pearl-prl-ai-compute-cryptocurrency/) that estimated daily revenue for an RTX 5090 dropped from about $33.80 to $17.19 within weeks of the May mining rush as difficulty rose. The report also put Pearl's official pool fee at 20%.

![](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3efa6c2d73c95f723f67_05_how-to-mine-pearl%201.webp)

## PRL Tokenomics & Supply

Mining is the only source of PRL. The code allocates nothing to founders or investors, and a single formula in the node software determines issuance.

### Supply and Emission

Pearl adopts Bitcoin's scarcity model at 100 times the unit count, but replaces halvings, the four-yearly events when [Bitcoin's block reward is cut in half](https://www.datawallet.com/bitcoin-halving-countdown). Its [block subsidy function](https://github.com/pearl-research-labs/pearl/blob/master/node/blockchain/validate.go) instead reduces the reward slightly with each block.

**PRL supply and market figures in early October 2026:**

- **Maximum supply:** 2.1 billion PRL. The node code enforces this cap and provides no mechanism to create more.
- **Mined supply:** About 331.9 million PRL at block 122,062 on October 2, equal to 15.8% of the cap. We recalculated the total using the subsidy formula and the block height displayed on Pearl's explorer.
- **Current block reward:** Roughly 2,289 PRL, compared with about 3,230 PRL at block 1.
- **Daily issuance:** Close to 1.02 million PRL at the target interval of 194 seconds per block.
- **Halfway point:** Cumulative supply reaches 1.05 billion PRL at block 650,226. At the target pace, this falls around the end of 2029.
- **Reported circulating supply:** CoinDesk's [Pearl price page](https://www.coindesk.com/price/pearl-research) lists 269.2 million PRL, about 63 million below the code-derived total. The gap remains unexplained.
- **Market value:** CoinDesk data put PRL near $1.16 on October 1. This implies about $312 million using CoinDesk's circulating supply, or roughly $385 million using the code-derived total.
- **Fully diluted value:** Around $2.43 billion, calculated by multiplying the price by the full 2.1 billion PRL supply.
- **Trading venues:** CoinDesk tracks BigONE and CoinEx markets. Daily volume of about $2 million represents less than 1% of market capitalisation.

The launch had more influence on ownership than the emission curve. Pearl began with difficulty near its minimum. Its [block explorer](https://explorer.pearlresearch.ai/block/40000) dates block 40,000 to May 3, just six days after the first block. Those blocks would have taken about 90 days at the 194-second target.

Roughly 121.7 million PRL were minted across those 40,000 blocks, close to 37% of all coins mined to date. According to the whitepaper, low starting difficulty was intended to give early miners a fair opportunity and ensure the core team had resources for research and development. This wording implies that the team mined during the period, though its holdings have not been disclosed.

### Utility

PRL currently serves a narrow set of functions.

**Current uses:**

- **Mining rewards:** The miner who finds a block receives newly issued PRL and its transaction fees.
- **Payments:** Users transfer PRL between Taproot addresses and use it to pay transfer fees.
- **Inference subsidies:** Operators can sell PRL earned while serving AI models to offset costs. Together AI uses this approach to fund its discounted inference service.
- **Bug bounties:** Pearl Research Labs offers discretionary security-report rewards ranging from 1,000 PRL to 50,000 PRL or more.

There are no governance rights or staking attached to PRL. Pearl's script system also lacks general smart contracts. The whitepaper proposes a future marketplace in which the chain settles compute contracts, but that extension is not live.

![](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3f070220514fd7f22f9f_06_prl-tokenomics-supply%201.webp)

## Is Pearl's Useful Work Useful?

Pearl proves that a matrix multiplication took place. It cannot establish that anyone wanted the result, which is the central criticism of its design.

Researcher Abhinaba Basu published [a preprint](https://arxiv.org/abs/2606.04819), The Usefulness Gap in Proof-of-Useful-Work, in June 2026. As [Tom's Hardware reported](https://www.tomshardware.com/tech-industry/artificial-intelligence/ai-cryptomining-networks-320-000-rtx-3090-class-gpus-allegedly-burn-112-megawatts-of-power-on-zero-useful-ai-computation-pearls-gpus-are-doing-random-matrix-math-study-claims), Basu mined with random matrices unrelated to any AI model. A pool accepted 44 of his shares, the partial proofs counted towards payouts.

Basu also examined 8,012 machines in one pool, representing about 21% of the network. The dominant mining program contained no machine-learning code. His study estimated Pearl's computing power at the equivalent of 320,000 RTX 3090-class GPUs drawing 112 megawatts, and attributed a 38% increase in budget GPU rental prices on Vast.ai to Pearl mining. He argued that miners skip AI computation whenever doing so is cheaper because the protocol permits useful work without requiring it.

Commercial activity provides Pearl's counter-evidence. On May 15, 2026, [Together AI](https://www.together.ai/blog/together-ai-partners-with-pearl-research-labs) launched a Gemma-4-31B-it-pearl endpoint, a paid access point to a hosted model, at more than 25% below its standard rate. Expected PRL mining revenue from its GPUs funds the discount. Together called the partnership exclusive and said more Pearl products would follow.

Pearl Research Labs now sells inference through its own [Pearl Inference Platform](https://pearlresearch.ai/pricing). The service lists eight open models from DeepSeek, Qwen, Z.AI and Google with per-token pricing. What proportion of this traffic is mined has not been stated.

A separate objection concerns security. If mining work has outside value, an attacker is partly paid to attack. Cryptographer Rafael Pass examined this in [The Economics of Proof-of-Useful-Work](https://arxiv.org/abs/2606.06700). His model concludes that, after prices adjust, the cost of a majority attack remains tied to the block reward. Such an attack occurs when one party controls most mining power. The model also finds that rewards can serve as rebates, reducing inference prices.

The planned FP8 upgrade would let operators mine on standard FP8 inference using the number format production models employ. Its specification still does not verify customer demand for the output. Basu's usefulness gap would narrow only if paid workloads became the cheapest way to mine.

## Pearl vs Bitcoin

Pearl borrows Bitcoin's monetary design while changing the work that secures it.

| Feature | Pearl | Bitcoin |
| --- | --- | --- |
| Mining work | Matrix multiplication on GPUs | SHA-256 hashing on ASICs |
| Maximum supply | 2.1 billion PRL | 21 million BTC |
| Issuance | Reward falls with every block | Reward halves about every four years |
| Target block time | 194 seconds | 10 minutes |
| Difficulty adjustment | Every block | Every 2,016 blocks |
| Block proof | Zero-knowledge certificate up to 65 KB | Nonce inside an 80-byte header |
| Address types | Taproot only | Legacy, SegWit and Taproot |
| Launch date | April 27, 2026 | January 3, 2009 |

Running since 2009, [Bitcoin](https://www.datawallet.com/price/bitcoin) is mined with application-specific integrated circuits (ASICs), chips built for a single task. Pearl's whitepaper argues that dedicated PRL mining chips would sacrifice the ability to sell AI compute. That trade-off, it suggests, should keep mining on general-purpose GPUs.

Other AI networks use different reward systems. [Bittensor](https://www.datawallet.com/crypto/what-is-bittensor) pays participants according to validator-assigned scores. Most [DePIN projects](https://www.datawallet.com/crypto/best-depin-projects), which pay people to supply physical hardware, rent GPU time to customers. Pearl instead proves the arithmetic and leaves operators to sell the output.

![Pearl vs Bitcoin](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf3f1343ffd8320231b953_07_pearl-vs-bitcoin%201.webp)

## Is Pearl Safe?

Established Bitcoin software underpins Pearl's node, wallet and light client, a wallet backend that avoids storing the full chain. The code is open source under the ISC licence, and published research supports the mining scheme. A [discretionary bug bounty](https://github.com/pearl-research-labs/pearl/blob/master/SECURITY.md) pays security researchers in PRL.

Mining remains the untested component. Security in the original paper depends on a conjecture, while the floating-point design introduces another assumption about the difficulty of multiplying quantised matrices. Four consensus changes in four months show that Pearl is still adjusting its rules on a live network. We found no published third-party audit of either the node or the proof circuits.

### Risks

Pearl's launch and governance account for several risks specific to PRL.

**The main risks to weigh:**

- **Unproven usefulness:** Most mining measured in June generated no AI output. The energy argument therefore depends on paid workloads arriving.
- **Concentrated early supply:** The first six days produced more than a third of all mined PRL. These coins remain in unidentified addresses without a lock-up schedule.
- **Continuous dilution:** Daily issuance adds about 1.02 million PRL, worth roughly $1.2 million at early October prices.
- **Thin trading:** A few smaller exchanges account for PRL trading, leaving prices vulnerable to sharp moves from single orders.
- **Consensus changes:** Every node must upgrade for a hard fork. Pearl Research Labs currently writes and schedules these changes.
- **Hardware dependence:** The official miner targets Nvidia GPUs. FP8 verification also works only on approved GPU architectures.
- **Ticker confusion:** Other, unrelated tokens use PRL. Confirm the project name before trading.
- **Regulatory uncertainty:** Rules governing mined digital assets vary by country and may change.

## Pearl Founders and Team

Co-founder and chief executive Omri Weinstein leads Pearl Research Labs. He is a theoretical computer scientist with a Princeton doctorate and a faculty position at the Hebrew University of Jerusalem. According to Hashrate Index, he lists previous roles at Nvidia and Vast Data.

The founding paper and Hawkeye research were co-written by Ilan Komargodski, also of the Hebrew University. Rafael Pass wrote the network's economic analysis.

The project describes its launch as having no pre-mine or investor allocation, and no venture round has been publicly announced. Its website shows membership in the Nvidia Inception startup programme alongside an AICPA SOC compliance badge. The company has not disclosed its legal structure, headcount or PRL holdings.

## Final Thoughts

Pearl is the first Proof of Useful Work chain to operate at scale with serious cryptography behind it. It has a named commercial partner and a working product, with credible engineering that includes zero-knowledge block certificates and support for quantum-resistant signatures.

Its economics remain less settled. About a million new coins enter circulation daily, creating supply that PRL's price must absorb, while early miners hold a large share. Pearl has yet to demonstrate that paid AI workloads account for a meaningful portion of mining.

Watch for FP8 activation, inference partners beyond Together AI and listings on larger exchanges. Until then, PRL remains a high-risk bet on an unproven idea. Any allocation should be small enough to lose.

## Frequently asked questions

Where can I buy PRL?

Smaller centralised exchanges, including BigONE, CoinEx and SafeTrade, offer PRL against USDT. PRL runs on its own blockchain and has no contract address. Tokens using that name on other networks are unrelated.

Can I mine Pearl with a gaming GPU?

Consumer Nvidia RTX cards are supported by community mining software. The official miner targets H100 and H200 data-centre GPUs. To keep earning full rewards, miners must also update their software before each fork.

Was Pearl a fair launch?

All PRL comes from mining, with no coins allocated in advance. Low starting difficulty still made the launch uneven. About 121.7 million PRL were mined in the first six days.

Does Pearl support smart contracts?

No. Its Bitcoin-style script system lacks general-purpose programmability. Taproot scripts and the enabled OP\_CAT opcode allow limited conditional spending.

Which wallet holds PRL?

Oyster is the official wallet, available as a desktop application or command-line client. It uses a light client by default, and PRL addresses begin with prl1.

![Datawallet Team](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/69c5187fb98a5743c20090ae_69990de6192b4155c0133f60_Datawallet%2520Team.webp)

### Written by     Datawallet Team

Research

Datawallet is an independent crypto research platform covering digital assets, blockchain data and on-chain analytics since 2022. Our research is cited by Binance, CoinMarketCap, Messari and leading academic publications.

[Visit Author on X](https://twitter.com/Datawalletcom)[Visit Author's Linkedin](https://www.linkedin.com/company/datawallet-com)

### Post tags

[Decentralized Finance](https://www.datawallet.com/tag/decentralized-finance)

[Guides](https://www.datawallet.com/tag/guides)

[Artificial Intelligence](https://www.datawallet.com/tag/artificial-intelligence)

[Bitcoin](https://www.datawallet.com/tag/bitcoin)

Share this Post

![Pearl Explained: Proof of Useful Work, AI Mining & PRL Tokenomics](https://www.datawallet.com/cdn-cgi/image/format=auto,quality=80/https://cdn.prod.website-files.com/69b5b6e1d5b9feee55a73d35/6abf40ff174e7779a8d127cc_Pearl%20Explained%20%26%20PRL%20Tokenomics.webp)