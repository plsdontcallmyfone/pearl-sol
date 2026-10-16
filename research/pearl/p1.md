Gain access to our exclusive data sets and research with [Hashrate Index Premium](https://hashrateindex.com/blog/introducing-hashrate-index-premium/)

Search Hashrate Index

![Banner reading "Mining Crypto With AI Compute" in white and gold text on a dark background with abstract flows of gold/iridescent green-blue and the Hashrate Index logo](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w1384/2026/06/hashrate-index-banner-pearl-crypto-gpu-ai-compute.jpg)

A five-week-old cryptocurrency set off a GPU mining rush in late May 2026, but the hardware isn't grinding out SHA-256 hashes. It's running matrix multiplication (the same arithmetic behind every modern AI model) and a top-tier inference provider has already signed on. Pearl (ticker PRL) is the first AI-compute cryptocurrency to push **proof-of-useful-work** into production at any real scale, and it sits squarely on the mining-to-AI bridge we've spent the past year mapping at Hashrate Index.

**TLDR**

- Pearl (PRL) is a Layer-1 blockchain that secures itself with proof-of-useful-work (PoUW): miners run large-scale matrix multiplication on Nvidia GPUs instead of hashing.
- Launched mainnet on April 27, 2026, built by Pearl Research Labs and grounded in a peer-reviewed cryptography paper, not a meme coin.
- An exclusive Together AI partnership put a discounted, Pearl-powered inference endpoint into production, the first real-world validation of the model.
- The gating variable is paid demand. Today most of the "useful work" is inference nobody bought, which makes the bulk of current mining AI-shaped proof-of-work.
- Returns are compressing fast: the RTX 5090's estimated daily revenue roughly halved within weeks as network difficulty climbed.

## **What Pearl Is: Proof-of-Useful-Work and the MatMul Breakthrough**

Proof-of-useful-work is an old idea with a bad track record: replace Bitcoin's "wasteful" hashing with computation that produces something valuable on the side. Pearl is the most serious attempt yet. Instead of SHA-256, the work that secures the network is **matrix multiplication (matmul)**—the dense linear algebra that dominates AI training and inference. Pearl calls its consensus mechanism proof-of-useful-work and implements it through a routine it terms NoisyGEMM.

Mechanically, a miner runs noisy matrix multiplications on a GPU, hashes the result into a commitment, and wraps the work in a zero-knowledge proof for cheap on-chain verification. The base layer itself is deliberately conservative—forked from Bitcoin's battle-tested btcd codebase, open-source under a permissive license, shipping a full node, wallet, light client, and a GPU miner. The novelty is the work function, not the plumbing.

The Bitcoin homage runs deep, right down to a 2.1 billion PRL max supply (exactly 100x Bitcoin's 21 million) with a declining block reward and roughly two-minute blocks. What separates Pearl from years of failed PoUW concepts is that the math is real: it rests on a 2025 cryptography paper, "Proofs of Useful Work from Arbitrary Matrix Multiplication" (Komargodski, Schen, and Weinstein), which claims a scheme with only ~1+o(1) overhead versus a naive matrix multiply. In plain terms, securing the chain costs almost nothing on top of the underlying compute. The pitch is "2-for-1": one GPU pass, two outputs.

|     |     |     |
| --- | --- | --- |
| Dimension | Bitcoin (PoW) | Pearl (PoUW) |
| Work performed | SHA-256 hashing | Matrix multiplication |
| Hardware | Purpose-built ASICs | Nvidia GPUs in servers |
| Byproduct | Discarded hashes | Potential AI inference/training output |
| Overhead vs. the useful task | n/a | ~1+o(1) over naive matmul |
| Max supply | 21 million BTC | 2.1 billion PRL |

## **Who Built Pearl and How It Launched**

Pearl Research Labs is led by co-founder and CEO Omri Weinstein, a complexity theorist with a Princeton PhD, a stint in Columbia's theoretical computer science group, and a current post at the Hebrew University, plus industry time he lists as ex-Nvidia and ex-Vast Data. The academic pedigree matters here: the project's credibility rests on a proof, and the proof's authors are the team.

The launch narrative leans hard on a fair-launch claim (no pre-mine, no founder allocation, no venture capital), which is central to how early miners talk about it. Treat that as positioning worth verifying rather than gospel, but the structure does mirror Bitcoin's permissionless start.

The timeline is compressed. The paper went up in April 2025, mainnet went live on April 27, 2026, and the Together AI partnership landed on May 15. Exchange listings and a mining rush followed in late May, PRL printed an all-time high near $1.65 on May 29, and mainstream hardware press picked it up by month's end. One caveat for anyone pulling data: PRL trades only on minor venues with thin liquidity, and the top "PRL" result on the major price aggregators is a _different_ coin entirely. Use Pearl's own block explorer for accurate network figures.

## **The Together AI Partnership and the Unit-Economics Pitch**

The reason Pearl jumped from crypto-Twitter to hardware headlines is the Together AI deal. Together is a top-tier inference and neocloud provider, so its involvement reads as genuine validation rather than another listing. On May 15, the two launched a discounted inference endpoint for an instruction-tuned Gemma-4 31B checkpoint, priced more than 25% below the usual rate, with the discount offset by the future value of PRL emissions—and the stated intent to widen the discount as PRL appreciates.

This is the whole thesis in one product. Weinstein's framing is that Pearl "changes the unit economics of AI" by letting every GPU cycle spent on training or inference simultaneously mint the native asset at no additional cost. The flywheel: a higher PRL price subsidizes cheaper inference, cheaper inference pulls in more compute, and more compute mines more PRL. It's a clean story for **decentralized AI compute**, and Together called it the first of a planned portfolio of Pearl-powered products.

The question is whether the flywheel spins when the compute isn't actually being bought.

## **Is the "Useful Work" Actually Useful?**

Here is the gating variable, the constraint that decides whether Pearl is a breakthrough or a rebranded GPU lottery: paid demand for the compute. The protocol _can_ extract a mining proof from genuine inference, exactly as the Together endpoint does. But the rigs driving the rush are running inference nobody requested and nobody pays for. The output goes nowhere. By Pearl's own logic, computation is only "useful" if someone pays for the result, which makes the overwhelming majority of current mining AI-shaped proof-of-work, not useful work.

This isn't a niche objection. The PoUW literature has long flagged the same failure mode: earlier schemes were criticized precisely because the work was randomly generated rather than tied to real, user-submitted demand, which hollowed out the "useful" part. Together's endpoint is the answer to that critique: a real buyer for real output But today it's a single endpoint set against a flood of speculative rented GPUs.

The sustainability claim deserves the same scrutiny. Pearl is marketed as greener than Bitcoin because the energy does double duty, but that only holds if the second job exists. Burn a megawatt on matmuls that produce inference no one consumes, and you've spent the same energy as hashing while telling a better story about it.

To be fair to the design: the mechanism is sound, the overhead is genuinely low, and the paid-inference path is real and demonstrated. The critique is about the current mix, not the ceiling. If paying demand scales faster than speculative mining, the useful-work claim becomes true. Right now, it mostly isn't.

## **Inside the Mining Rush: Economics and Hardware**

The rush is a cloud-arbitrage trade. Miners rent RTX 4090 and 5090 instances on GPU marketplaces for well under a dollar an hour, point them at a mining pool, and pocket the spread between rental cost and PRL payout. When the math works, it works briefly—because everyone runs the same trade. As capacity flooded in, network difficulty climbed steeply, and one widely cited profitability tracker cut its RTX 5090 estimate from ~$33.80 to ~$17.19 per day, a ~49% drop in a matter of weeks. The block reward only declines from here.

The pool structure splits along hardware lines. An official Pearl Research pool charges a steep 20% fee and accepts only H100 and H200 datacenter cards; lower-fee community pools have since added support for consumer GPUs down to older Volta-class silicon. Mining is Nvidia-only. The network snapshot, as of early June 2026, shows a market cap near ~$146 million against a fully diluted valuation around ~$1.6 billion, with PRL trading roughly 35–40% below its peak.

The more interesting signal comes from the demand side. Luxor's GPU sales expert Mike San Miguel, speaking on the [Blockspace podcast](https://www.youtube.com/live/f31kHaRS-tc?si=8NiqNfvNRK76Zxfc&t=1406&ref=hashrateindex.com), said the **inbound interest isn't coming from degens—it's coming from VCs, private financial firms, and miners trying to get in early**. He also described the demand curve as front-loaded: the first two-to-three months is where the action concentrates, with some interest stretching out to roughly 12–18 months. That shape lines up exactly with a declining-reward chain where being early is the entire edge.

San Miguel's sharpest point was about hardware fit. He noted that the natural home for Pearl mining is a class of GPUs that lack the back-end networking needed for at-scale AI training and therefore have little place in the AI rental market. Pearl gives them a job. The mining profile is unusually light—low RAM, low storage, low CPU, and none of the expensive multi-fiber networking a real training cluster demands. So, in his words, it's "a great way to use up equipment." The catch is deployment: these arrive as servers, not mining ASICs. The whole stack has to be engineered and managed as server infrastructure, which is a different operational discipline than racking an ASIC fleet.

## **What Pearl Means for Miners (and Whether It Lasts)**

Pearl inverts the story Hashrate Index usually tells. We cover miners pivoting _to_ AI/HPC; Pearl pulls AI compute _into_ mining. Whether or not this specific chain survives, that inversion is the takeaway: proof-of-useful-work is a live experiment in monetizing GPU cycles that would otherwise sit idle or underpriced. That is the clearest version yet of revenue-stacking on compute that the market has mispriced.

The bull case is structural. If paid demand (more Together-style endpoints) scales faster than speculative mining, the useful-work claim turns real and the flywheel earns its narrative. The bear case is mechanical, and it's the one most miners chasing the rush are living: a falling block reward, rising difficulty, and thin liquidity is the textbook setup for return compression, and most of today's "useful" work isn't.

San Miguel framed the open question well: **the real unknown is staying power—whether anyone commits to buying hardware _specifically_ for Pearl rather than just renting opportunistically while the spread lasts**. That's the line to watch.

For operators, the near-term play is obvious and time-boxed; the long-term thesis depends on three things to track: 1) the ratio of paid to speculative compute, 2) the cadence of new neocloud partnerships, and 3) whether PRL ever earns liquidity beyond minor exchanges. Pearl is the most credible attempt yet to make proof-of-work useful. For now, "useful" is still mostly a promise, and the miners piling in are mining a falling reward against rising difficulty.

[AI/HPC](https://hashrateindex.com/blog/tag/ai-hpc/) [AI Hardware](https://hashrateindex.com/blog/tag/ai-hardware/)

[![Ian Philpot](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w320/2026/01/Ian-Philpot-Fall-2024-3.jpg)](https://hashrateindex.com/blog/author/ian/)

### [Ian Philpot](https://hashrateindex.com/blog/author/ian/)

Marketing Director at Luxor Technology

### Hashrate Index Newsletter

Join the newsletter to receive the latest updates in your inbox.

Your email addressSubscribe

Please check your inbox and click the link to confirm your subscription.

Please enter a valid email address!

An error occurred, please try again later.

Disqus Comments

We were unable to load Disqus. If you are a moderator please see our [troubleshooting guide](https://docs.disqus.com/help/83/).

G

Start the discussion…

﻿

Comment

###### Log in with

###### or sign up with Disqus  or pick a name

### Disqus is a discussion network

- Don't be a jerk or do anything illegal. Everything is easier that way.

[Read full terms and conditions](https://docs.disqus.com/kb/terms-and-policies/)

This comment platform is hosted by Disqus, Inc. I authorize Disqus and its affiliates to:

- Use, sell, and share my information to enable me to use its comment services and for marketing purposes, including cross-context behavioral advertising, as described in our [Terms of Service](https://help.disqus.com/customer/portal/articles/466260-terms-of-service) and [Privacy Policy](https://disqus.com/privacy-policy), including supplementing that information with other data about me, such as my browsing and location data.
- Contact me or enable others to contact me by email with offers for goods or services
- Process any sensitive personal information that I submit in a comment. See our [Privacy Policy](https://disqus.com/privacy-policy) for more information

Acknowledge I am 18 or older

- [Favorite this discussion](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default# "Favorite this discussion")

  - ## Discussion Favorited!



    Favoriting means this is a discussion worth sharing. It gets shared to your followers' Disqus feeds, and gives the creator kudos!


     [Find More Discussions](https://disqus.com/home/?utm_source=disqus_embed&utm_content=recommend_btn)

[Share](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default#)

  - Tweet this discussion
  - Share this discussion on Facebook
  - Share this discussion via email
  - Copy link to discussion

  - [Best](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default#)
  - [Newest](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default#)
  - [Oldest](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default#)

Be the first to comment.

[Load more comments](https://disqus.com/embed/comments/?base=default&f=blog-hashrateindex-com&t_i=ghost-6a1f22cac491a50001264908&t_u=https%3A%2F%2Fhashrateindex.com%2Fblog%2Fpearl-prl-ai-compute-cryptocurrency%2F&t_d=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&t_t=Pearl%20(PRL)%3A%20The%20AI-Compute%20Cryptocurrency%2C%20Explained&s_o=default#)

* * *

## You might also like

![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w1384/2026/10/compute-competition-thumbnail-v3-2.png)

## [The Compute Competition](https://hashrateindex.com/blog/the-compute-competition/)

Zcash ASICs, Bitcoin ASICs and H100s are bidding for the same megawatts. What each earns per MWh, per machine and per dollar of capex in October 2026, and why the answer flips with the denominator.

[![Kaan Farahani](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w128/2025/07/p1_4x4.jpg)](https://hashrateindex.com/blog/author/kaan/)

[Kaan Farahani](https://hashrateindex.com/blog/author/kaan/)

13 Oct 2026

[AI/HPC](https://hashrateindex.com/blog/tag/ai-hpc/)

![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w1384/2026/10/Yotta-2026-Recap-----Blog-Banner-1200x600.jpg)

## [Compute Derivatives, Token Factories, Modular Builds: 5 Takeaways from Yotta 2026](https://hashrateindex.com/blog/yotta-2026-takeaways-compute-derivatives-token-factories/)

We were on the floor at Yotta 2026. Here are the 5 takeaways that matter for infrastructure operators and capital allocators.

[![Hashrate Index](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w128/2025/07/Vector-2-1.png)](https://hashrateindex.com/blog/author/hashrate_index/)

[Hashrate Index](https://hashrateindex.com/blog/author/hashrate_index/)

1 Oct 2026

[AI/HPC](https://hashrateindex.com/blog/tag/ai-hpc/)

## Hashrate Index Newsletter

Join the newsletter to receive the latest updates in your inbox.

Your email addressSubscribe

Please check your inbox and click the link to confirm your subscription.

Please enter a valid email address!

An error occurred, please try again later.

- [Twitter](https://x.com/hashrateindex)
- [LinkedIn](https://www.linkedin.com/company/luxor-tech)
- [Corporate Website](https://www.luxor.tech/)
- [YouTube](https://www.youtube.com/channel/UCtPffT43JHCVf827z2dcw2Q)
- [RSS](https://hashrateindex.com/rss/)

## Featured Posts

[**The Compute Competition** 13 Oct 2026\\
\\
![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w346/2026/10/compute-competition-thumbnail-v3-2.png)](https://hashrateindex.com/blog/the-compute-competition/) [**October 12, 2026** 12 Oct 2026\\
\\
![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w346/2025/07/IMG_6469-1.png)](https://hashrateindex.com/blog/latest-weekly-asic-closing-prices/) [**Luxor Hashrate Lookback Series — September 2026** 9 Oct 2026\\
\\
![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w346/2026/10/Thumbnail--35-.png)](https://hashrateindex.com/blog/luxor-hashrate-lookback-series-september-2026/)

## [Authors →](https://hashrateindex.com/blog/authors/)

[![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w256/2021/12/colin.jpg)\\
\\
**Colin Harper** \\
\\
Contributor](https://hashrateindex.com/blog/author/colin/) [![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w256/2025/07/p1_4x4.jpg)\\
\\
**Kaan Farahani** \\
\\
Research Associate at Luxor Technology](https://hashrateindex.com/blog/author/kaan/) [![](https://storage.ghost.io/c/5f/b9/5fb98cf7-67da-41b5-a184-b1b39752525f/content/images/size/w256/2025/07/unnamed-1.jpg)\\
\\
**Ben Harper** \\
\\
Director, Financial Services at Luxor Technology](https://hashrateindex.com/blog/author/ben/)

Premium