[![Alea Research](https://substackcdn.com/image/fetch/$s_!P-QX!,w_40,h_40,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F76694762-23a4-4d13-a34f-3bfdc01a553e_1232x1232.png)](https://alearesearch.substack.com/)

# [Alea Research](https://alearesearch.substack.com/)

SubscribeSign in

![User's avatar](https://substackcdn.com/image/fetch/$s_!H9TY!,w_64,h_64,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F82df30a9-077f-4002-8d42-aec5b778456d_400x400.jpeg)

Discover more from Alea Research

Objective and actionable research to help you make informed decisions and give you a decisive edge in crypto.

Over 13,000 subscribers

Subscribe

By subscribing, you agree to Substack's [Terms of Use](https://substack.com/tos), and acknowledge its [Information Collection Notice](https://substack.com/ccpa#personal-data-collected) and [Privacy Policy](https://substack.com/privacy).

Already have an account? Sign in

# Pearl - Proof of Useful Work for AI Compute

### PRL Rewards, Matrix Multiplication, Token Emissions, and Inference Economics

Jun 10, 2026

2

Share

AI compute markets have a simple coordination problem: GPU providers spend real energy producing useful work, while proof-of-work blockchains spend real energy producing security. Pearl Research tries to merge the two.

Pearl is a Bitcoin-style Layer 1 where mining comes from AI computation, not hash puzzles. Its chain is live, PRL has a fixed 2.1B supply, and blocks target 194 seconds.

In this edition, we look at Proof of Useful Work, how PRL rewards are earned, and whether the token design actually maps to AI compute demand.

[![](https://substackcdn.com/image/fetch/$s_!afcW!,w_1456,c_limit,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2Fef39d050-3715-46ee-b844-9b96690e372c_1817x620.png)](https://substackcdn.com/image/fetch/$s_!afcW!,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2Fef39d050-3715-46ee-b844-9b96690e372c_1817x620.png)

#### What Pearl Is Building

Pearl is a Bitcoin-style L1 that replaces SHA-256 hashing with matrix multiplication as its proof of work function, using block rewards, transaction fees, and a longest-chain rule. Bitcoin miners race to find a hash below a target value while Pearl miners race to produce valid matrix multiply proofs against committed input matrices. The chain has a fixed 2.1B PRL supply and targets 194-second blocks.

[![X avatar for @prlnet](https://substackcdn.com/image/fetch/$s_!3BeW!,w_40,h_40,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fpbs.substack.com%2Fprofile_images%2F2029218139599257600%2FIuyT0pyZ.jpg)\\
\\
Pearl Research@prlnet\\
\\
It is becoming increasingly clear that the future economy will be denominated in compute cycles more than in human labor. \\
\\
In a world where AI drives the majority of electricity consumption and GDP, compute is the natural collateral for money: an open, auditable, AI-native\\
\\
12:12 PM · Mar 17, 2026 · 20K Views\\
\\
* * *\\
\\
16 Replies · 12 Reposts · 93 Likes](https://x.com/prlnet/status/2033939431980159308)

Matrix multiplication is the core arithmetic behind AI inference and training. When a neural network generates a response, it repeatedly multiplies large arrays of numbers through its layers, and every forward pass is a sequence of matrix multiplies. Pearl’s whitepaper argues that a GPU running those operations for a live AI workload can simultaneously produce valid PoW submissions as a side effect of computation already underway.

[![X avatar for @WeinsteinOmri](https://substackcdn.com/image/fetch/$s_!pbrC!,w_40,h_40,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fpbs.substack.com%2Fprofile_images%2F1473895631525756929%2FOfL5OKI5.jpg)\\
\\
Omri Weinstein@WeinsteinOmri\\
\\
Will Fast Matrix Multiplication ever be practical?\\
\\
Strassen’s 1986 discovery of fast matrix multiplication (FMM) – asserting that the product of two 𝑛×𝑛 matrices can be computed in sub-cubic time 𝑛^𝜔 ∼ 𝑛²·⁸⁷ ≪ 𝑛³ – had a profound impact on theoretical computer science \\
\\
![](https://pbs.substack.com/media/HI9zlMBbwAAKqsp.png)\\
\\
9:37 PM · May 22, 2026 · 42.7K Views\\
\\
* * *\\
\\
9 Replies · 19 Reposts · 108 Likes](https://x.com/WeinsteinOmri/status/2057999211631931842)

A miner commits to specific input matrices before computing, which prevents using trivial or pre-computed inputs that would give an unfair advantage. The commitment forces the miner to run the actual computation, where the output is then verifiable by other nodes without those nodes re-running the full workload.

#### How PRL Rewards Are Earned

PRL rewards work like Bitcoin block rewards. Miners receive newly minted PRL and transaction fees when they produce valid blocks. A miner producing a valid block means committing to input matrices, running the computation, and generating a verifiable proof. Rewards are paid per block, not per inference request served.

The cost argument favors providers already running inference. A data center serving real AI requests pays lower incremental mining costs because the matrix multiplications are happening anyway. Pearl mining adds only the overhead of proof generation on top of the existing workload. A miner with no AI customers pays the full GPU cost to mine, making that miner more expensive than one with paying inference clients.

Pearl’s emission curve models Bitcoin’s scarcity design without copying the halving structure. The 2.1B supply decays on a smooth polynomial curve that allocates roughly half over four years, avoiding the abrupt reward cliffs that create short-term instability around Bitcoin halvings. Gradual decay keeps miner incentives more predictable than discrete step-downs, at the cost of slightly less aggressive near-term scarcity.

#### The Together AI Integration and the Open Loop

Together AI launched an exclusive Pearl-powered inference endpoint for Gemma-4-31B-it-Pearl in May 2026, priced at a 25%+ discount to standard rates and offset by expected PRL emissions. Together’s announcement noted that future Pearl-powered products may let inference customers receive part of those emissions directly. This is the first integration where real compute buyers receive a price reduction because their provider’s GPU work generates PRL.

[![X avatar for @togethercompute](https://substackcdn.com/image/fetch/$s_!xqj9!,w_40,h_40,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fpbs.substack.com%2Fprofile_images%2F2028879663221882886%2FsMzEKnKc.jpg)\\
\\
Together AI@togethercompute\\
\\
Introducing Gemma-4-31B-it-Pearl on Together AI, Pearl Research Labs’ instruction-tuned checkpoint of Gemma 4 31B powered by @prlnet Proof of Useful Work protocol.\\
\\
AI natives can now use this Pearl model as a serverless inference endpoint on Together AI, at a 25%+ discounted \\
\\
![](https://pbs.substack.com/media/HIYD2YpWUAAlcw9.jpg)\\
\\
1:10 PM · May 15, 2026 · 139K Views\\
\\
* * *\\
\\
12 Replies · 19 Reposts · 113 Likes](https://x.com/togethercompute/status/2055334918255702242)

Pearl does not yet have a marketplace where buyers pay PRL and receive inference. Rewards flow to miners for valid blocks, the mechanism does not distinguish between a miner serving real inference clients and one running matrix multiplications with no AI buyers. Pearl’s documentation frames the marketplace layer as a future direction, with compute contracts eventually settling natively onchain.

Together AI’s 25%+ discount is sustained by PRL emissions expected from its GPU workloads. If that discount holds as the emission curve decays and more providers integrate Pearl mining for similar economics, PRL drastically subsidizes compute. If adoption stays concentrated on one commercial partner while the broader mining base operates without real inference clients, PRL remains a speculative token.

#### Important Links

- **[Pearl X](https://x.com/prlnet)**

- **[Pearl App](https://compute.pearlresearch.ai/)**

- **[Pearl Docs](https://pearlresearch.ai/#blockchain-overview)**

- **[Pearl Website](https://pearlresearch.ai/)**


[![](https://substackcdn.com/image/fetch/$s_!zdy0!,w_1456,c_limit,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F654bf1ac-be32-4c9d-9bef-38add98316e7_1354x118.jpeg)](https://alearesearch.io/research/)

Become a [Premium member](https://alearesearch.io/plans/) and get the full Alea platform.

Join thousands of sharp crypto investors & traders using Alea to evaluate opportunities and deploy with conviction. For just $149/month, Premium unlocks:

- **Full report suite**: Deep Dives, Perspectives, Blueprints, Theses, Benchmarks, Memos & Pulse.

- **Protocol Data Rooms**: Standardized due diligence portals with live analytics, risk matrices, and governance tracking across 24+ protocols.

- **Podcast Digest & Summit Digest**: Key takeaways from the conversations shaping markets, so you don’t have to listen to every pod.

- **Governance Tracker**: Live proposal feeds and voting activity across major protocols.

- Full access to the historical research archive.


[Join Today](https://alearesearch.io/plans/)

[![defisurfer's avatar](https://substackcdn.com/image/fetch/$s_!FtyZ!,w_32,h_32,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F0b5ee195-61c9-470a-bf53-3b3962e25ce4_96x96.jpeg)](https://substack.com/profile/105618175-defisurfer)[![napkin's avatar](https://substackcdn.com/image/fetch/$s_!khgO!,w_32,h_32,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F36ad1bdb-017c-4cba-815c-afd027968d4f_640x640.jpeg)](https://substack.com/profile/133914091-napkin)

2 Likes

2

Share

PreviousNext

#### Discussion about this post

CommentsRestacks

![User's avatar](https://substackcdn.com/image/fetch/$s_!TnFC!,w_32,h_32,c_fill,f_auto,q_auto:good,fl_progressive:steep/https%3A%2F%2Fsubstack.com%2Fimg%2Favatars%2Fdefault-light.png)

TopLatestDiscussions

[Macro Dose - Tariff Blowback, Market Fear, & More](https://alearesearch.substack.com/p/macro-dose-tariff-blowback-market)

[$BTC Bottom?, Market Outlook, & More](https://alearesearch.substack.com/p/macro-dose-tariff-blowback-market)

Apr 8, 2025

6

1

![](https://substackcdn.com/image/fetch/$s_!31AW!,w_320,h_213,c_fill,f_auto,q_auto:good,fl_progressive:steep,g_center/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2Fe09248ab-ee9f-472d-80bc-31e1ac31fce3_626x440.png)

[$SUI: What You Need to Know](https://alearesearch.substack.com/p/sui-what-you-need-to-know)

[Sui vs Aptos & Advantages of Move](https://alearesearch.substack.com/p/sui-what-you-need-to-know)

Jan 14, 2024

17

1

1

![](https://substackcdn.com/image/fetch/$s_!2qGe!,w_320,h_213,c_fill,f_auto,q_auto:good,fl_progressive:steep,g_center/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2F4010e842-bfc4-436f-a807-f9a4266d7f7d_1200x858.png)

[$DYM: What You Need to Know](https://alearesearch.substack.com/p/dym-what-you-need-to-know)

[Dymension Rollapps & Plans Ahead](https://alearesearch.substack.com/p/dym-what-you-need-to-know)

Jan 11, 2024

14

1

![](https://substackcdn.com/image/fetch/$s_!oNq5!,w_320,h_213,c_fill,f_auto,q_auto:good,fl_progressive:steep,g_center/https%3A%2F%2Fsubstack-post-media.s3.amazonaws.com%2Fpublic%2Fimages%2Fe3db9f84-05f4-4870-ad4f-9c3486ab991a_1200x858.png)

See all

### Ready for more?

Subscribe