[Skip to content](https://pearlresearch.ai/research/blog/proof-of-useful-work#main-content)

[![Pearl](https://pearlresearch.ai/brand/logos/logotype-black.svg)](https://pearlresearch.ai/)

[Research](https://pearlresearch.ai/research) [Network](https://pearlresearch.ai/network) [Pricing](https://pearlresearch.ai/pricing) [Docs](https://platform.pearlresearch.ai/docs)

### Table of Contents

- Introduction
- What is matrix multiplication?
- Why MatMul is fundamental
- Why GPUs?
- Neural net inference
- The verification problem
- Freivalds' algorithm
- Zero-Knowledge Proofs for Inference
- How cuPOW solves verification
- Putting it all together
- Why this matters
- The 2-for-1 property
- Store of value

Contents

# Proof of Useful Work  from the Ground Up

> “If we can find some useful computation which is easy to verify, then cryptocurrency mining could actually become a huge boon to society… but it is probably infeasible.”

Vitalik Buterin, 2019

Llama 3.3 70B is a 70 billion parameter model. At bfloat16 precision, that's 140GB of weight data. Running inference, i.e. turning a prompt into a response, means multiplying your input through 80 transformer layers, each of which performs multiple matrix multiplications involving matrices with thousands of rows and columns. A single token of output costs roughly 2×70B = 140 billion multiply-add operations. A full response might cost several hundreds of trillion.

Meanwhile, the Bitcoin network is estimated to run on roughly 4.18 million ASICs worldwide, based on a total hashrate of 980,000,000 TH/s and a representative machine like the Antminer S21 Pro at 234 TH/s. Together they consume roughly 150 TWh of electricity per year, comparable to a mid-sized country. What do they compute? SHA-256 hashes. The output of every single hash was designed to prove that energy was spent, making it provably hard to fake.

What if that computation were the matrix multiplications that the AI economy is already paying for?

That's what Pearl does. Pearl is a Layer-1 proof-of-useful-work blockchain where the mining operation is matrix multiplication, which happens to be the native operation of GPUs. Miners earn block rewards and inference fees for the same work.

Measured in production

The proof-of-useful-work kernel adds only a few percent of end-to-end overhead versus stock serving engines:+5.08% on Llama 70B (DP=4 on 4×H200) and +3.9% on DeepSeek V3.2 (DP=8 + EP on 8×H200). It runs today on integer hardware viaW7A7 quantization (GPTQ + SmoothQuant), with native floating-point support in development.

In this post, we will cover:

- What matrix multiplication is and how it works at the level of individual operations
- Why every neural network, every LLM, every image model, every AI agent, is nothing more than a chain of matrix multiplications
- Why “just check the work” doesn't work, and why this single problem blocked useful proof-of-work for 15 years
- How Pearl's protocol (cuPOW) solves verification with 1+o(1) overhead, using a technique you can understand with high school math
- Why a token backed by useful computation is a natural store of value for the AI era

If you already understand matrix multiplication and neural network inference, feel free to skip to [the verification problem](https://pearlresearch.ai/research/blog/proof-of-useful-work#verification-problem).

## What is matrix multiplication?

Let's start at the very bottom. A matrix is a grid of numbers. Matrix multiplication takes two grids and produces a third.

The rule for computing each output cell is: take a row from the first matrix and a column from the second, multiply the corresponding elements together, and add up the results. This is called a _dot product_.

Here's the simplest case. One row, one column:

u = \[a  b\]

2

3

v = \[c  d\]ᵀ

4

1

a = 2

c = 4

b = 3

d = 1

* * *

2×4=8+3×1=3=

11

u · v = a×c + b×d

Two multiplications and one addition. This single operation, multiply pairs and sum, is called a **multiply-accumulate (MAC)**. It's the atom of AI computation. Everything in modern AI is built from this.

Now scale up. Below is a 2×3 matrix multiplied by a 3×2 matrix, producing a 2×2 output. Each output cell is one dot product.

A  (2×3)

2

1

3

0

4

1

×

B  (3×2)

1

2

3

0

1

4

=

C  (2×2)

?

?

?

?

hover or tap a cell in C to compute it

Notice what happened: each dot product required 3 MACs (because the shared dimension is 3), and we needed 4 dot products (because the output is 2×2). That's 12 multiplications total.

The pattern is general. For matrices with dimensions M×K and K×N, the output is M×N, and each of the M×N output cells requires K multiply-accumulates. Total:

total MACs = M × N × K

That formula is the reason AI is expensive.

This operation, multiplying two matrices, is called **GEMM: General Matrix Multiplication**. When people say “AI runs on matrix multiplication,” they mean GEMM. When people say there's a global GPU shortage, what they mean is that the world's demand for GEMM exceeds supply.

## Why is MatMul so fundamental in ML?

Matrix multiplication is fundamental to machine learning because it encodes the transformation of data from one representation into another: a (learned) change of basis: mapping an input X into a new space Y where useful patterns become easier to detect (e.g., sparsity, invariances, spectral decay etc.).

Indeed, a core underlying assumption in ML is that some structure exists in data, albeit not necessarily in its raw form. Training statistical models can be viewed as the process of finding representations that uncover patterns in the input layer's signal. Matrix multiplication encodes a linear transformation that enables this process. For example, MLP layers project activations into new feature spaces, while attention layers use learned projections to form queries, keys, and values. In both cases, matrix multiplication is what turns raw inputs into useful representations.

## Why GPUs?

A CPU computes one (or a few) operations at a time. It has a small number of powerful cores. A GPU has thousands of weaker small cores that each compute one multiply-accumulate simultaneously.

Look at our 2×3 times 3×2 example again. There are 4 output cells, each independent of the others. A CPU computes them one after another, 4 sequential steps. A GPU computes all 4 in parallel, 1 step.

2×3 · 3×2 = 2×2, 4 independent dot products

A (2×3)

2

1

3

0

4

1

×

B (3×2)

1

2

3

0

1

4

=

C (2×2)

?

?

?

?

▶ run↺ reset

t=1

t=2

t=3

t=4

CPU1 core, sequential

C\[1,1\]

C\[1,2\]

C\[2,1\]

C\[2,2\]

waiting

GPU4 cores, parallel

C\[1,1\]

C\[1,2\]

C\[2,1\]

C\[2,2\]

waiting

Swipe horizontally to see the full race.

This is why NVIDIA's market cap has exceeded $4 trillion. This is why there are multi-billion-dollar data center buildouts. The world needs an unimaginable number of GEMM, and GPUs remain the dominant hardware for this workload.

It's also why Pearl mining runs on GPUs, not ASICs (Application-Specific Integrated Circuit). GEMM is the native operation of GPU silicon. Bitcoin miners need custom chips because GPUs aren't efficient at hashing. Pearl miners just need the GPUs they'd already want for AI inference.

## Neural network inference is mostly a chain of GEMMs

Now that you know what a matrix multiplication is, let's see why AI is built entirely out of them.

A neural network layer takes an input, multiplies it by a matrix of learned “weights,” adds a bias, and applies a nonlinear function. Let's build this up piece by piece.

inputs x

x12.0

x21.0

x3-0.5

weights W

W\[1,1\]0.5

W\[1,2\]-0.3

W\[1,3\]0.8

W\[2,1\]0.1

W\[2,2\]-0.6

W\[2,3\]0.4

bias b

b10.5

b2-0.2

activation σ

relusigmoidtanh

y =σ(W·x+b) =\[0.8, 0.0\]

That's the whole thing. A neural network layer is a matrix multiplication, plus one small addition (bias) that is computationally trivial by comparison. The next step is activation functions that are non-linear but usually represent less than 5 percent of the compute used by a neural network.

Now, a transformer (the architecture behind GPT, Claude, Llama, Gemini, and every modern LLM) is a specific arrangement of these layers. Each “transformer block” contains two major sections, and each section is built from GEMMs.

Self-attention uses 4 GEMMs:

```
Q = input × W_Q    // "Query"  what am I looking for?
K = input × W_K    // "Key"    what do I contain?
V = input × W_V    // "Value"  what information do I carry?
output = attn(Q,K) × V × W_O   // combine and project
```

Feed-forward network uses 2–3 GEMMs:

```
hidden = input × W_up      // expand: 4096 → 14336 dimensions
gate   = input × W_gate    // gating (in SwiGLU architectures)
output = (hidden ⊙ gate) × W_down   // contract: 14336 → 4096
```

That's ~7 GEMMs per transformer block. A model like Llama 3 8B has 32 blocks. Llama 3 70B has 80.

Every one of those MACs is a multiplication and an addition inside a GEMM. This is what Pearl miners compute. The exact same GEMMs that every AI inference provider on earth (OpenAI, Anthropic, Google, every startup hosting a model) is running right now.

## The verification problem

Here is the problem that blocked “useful” proof of work for 15 years. It's subtle, and it's worth understanding precisely.

In Bitcoin, mining is hard and verification is trivial:

```
// Mining: try ~10²¹ nonces to find one that works
for (let nonce = 0; ; nonce++) {
    const hash = sha256(blockData + nonce);
    if (hash < difficultyTarget) {
        return { nonce, hash }; // Found it!
    }
}

// Verification: compute ONE hash and check
function verify(blockData, nonce, target) {
    return sha256(blockData + nonce) < target;
}
// Cost: 1 hash. Takes nanoseconds.
```

The asymmetry is enormous. Mining costs ~1021 hash operations. Verification costs 1 hash operation. Any node in the network can verify a block on a Raspberry Pi.

Now imagine a miner says “I multiplied matrix A by matrix B and got C.” How does a verifier check?

The naive way: redo the multiplication.

```
function verifyNaive(A, B, claimedC) {
    const actualC = matmul(A, B);  // Full GEMM: O(n³)
    return matricesEqual(actualC, claimedC);
}
// Mining cost:       O(n³)
// Verification cost: O(n³)
// Ratio: 1. Useless.
```

If verification costs as much as the work itself, you haven't accomplished anything. Every verifier would need the same GPU as the miner. You can't build a decentralized network where every node needs an H100.

This is the fundamental challenge. Every previous attempt at useful PoW (protein folding, optimization problems, training runs) failed here. Either verification was as expensive as the work, or the verification required trusting a third party, which defeats the purpose of a decentralized system.

## The Freivalds' algorithm

In 1977, Rūsiņš Freivalds published a beautifully simple result. You can probabilistically verify a matrix multiplication in O(n²) instead of O(n³).

Here's how it works. Given matrices A, B, and a claimed product C = A × B:

1. Pick a random vector r ∈ {0,1}n
2. Compute y₁ = C × r (one matrix-vector multiply: O(n²))
3. Compute y₂ = A × (B × r) (two matrix-vector multiplies: O(n²))
4. If y₁ = y₂, accept. Otherwise, reject.

Why does this work?

**If C = A·B**: Then C·r = A·(B·r) always. It follows from the associativity of matrix multiplication. The check passes every time.

**If C ≠ A·B**: Meaning at least one element is different, then C·r ≠ A·(B·r) with probability ≥ 1/2 for a randomly chosen r.

The intuition: r is a random projection that “samples” the disagreement between C and the true product. Any single projection has at least a coin-flip chance of landing on the error. Repeat with k independent random vectors and the probability of missing a cheater drops to 2−k. At k = 10, that's a 99.9% catch rate.

But Freivalds gives you a **proof of correctness**, not a **proof of work**. It confirms the miner got the right answer but it says nothing about _how much effort_ was spent getting there. There's no difficulty parameter, so no way to make the problem harder when more miners join. A miner who precomputed A·B last week passes Freivalds just as easily as one who computed it right now. Freivalds tells you _what_ was computed. A proof-of-work protocol needs to tell you _how much_ was computed, and make that amount tunable.

## Zero-Knowledge Proofs for Inference

Another natural idea is to use zero-knowledge proofs to certify that an inference pass was executed correctly on a specified model. The thinking is that if the proof attests to a model whose execution in a timely manner requires significant computational resources, it could serve as a proof of useful work. In principle, that is very appealing: the prover could show that a model really produced an output without revealing internal weights or every intermediate activation. In practice, though, state-of-the-art ZK systems still impose too much overhead to serve as a practical proof layer for the frontier models that matter economically today. Modern LLMs live in the high tens of billions to trillions of parameters, which means enormous matrix multiplies, long activation traces, and many nonlinear operations that must be encoded into proof-friendly arithmetic. The result is that proof generation is still far more expensive than the lightweight verification a decentralized proof-of-work protocol needs.

One recent example is [NANOZK](https://arxiv.org/abs/2603.18046v1), which proposes layerwise zero-knowledge proofs for verifiable LLM inference and reports meaningful improvements over earlier ZKML approaches. That is real progress. But translated into the latency budget of autoregressive generation, these numbers are still nowhere near production inference. A proved forward pass for GPT-2-scale models is measured in tens of seconds, not milliseconds: [zkGPT](https://www.usenix.org/conference/usenixsecurity25/presentation/qu-zkgpt) reports under 25 seconds for GPT-2 inference, which is at best about 0.04 proved tokens per second if each next-token step is proved. NANOZK reports minutes for full GPT-2 verification under parallel proving, and [zkLLM](https://arxiv.org/abs/2404.16109) reports under 15 minutes for a 13B-parameter model. These are impressive cryptographic results, but they are many orders of magnitude away from the tens to hundreds of tokens per second expected from ordinary LLM serving. It is also a useful reality check: the paper's proof benchmarks are still at GPT-2 scale and below, on the order of roughly 124 million parameters rather than the tens of billions to trillions of parameters used by production frontier models. So while ZK inference is an active and promising research direction, it is not yet the mechanism that makes useful proof-of-work viable for most LLMs used in production today. And perhaps more importantly, the question becomes whether it will _ever_ have practically equivalent or lower overhead for proof generation compared to cuPOW.

## How cuPOW solves verification

We've established the problem: verification of a matrix multiplication costs O(n³), the same as doing the work itself. Every previous attempt at useful proof of work died here. Let's see how cuPOW, the protocol introduced by Ilan Komargodski and Omri Weinstein in the PoUW paper, actually solves it.

The solution doesn't arrive in one step. It arrives through three ideas, each one fixing a flaw in the previous.

### Idea 1: Add random noise

Start with two matrices A and B that a miner wants to multiply. Let's say, a weight matrix and an activation matrix from a real AI inference request. The product C = A·B is the useful result.

Now, here's our first idea on how to turn this into a proof of work. Instead of computing A·B directly, force the miner to compute:

C' = (A + E) · (B + F)

where E and F are random matrices derived from a public seed σ (generated by hashing the block header, so the miner can't predict them in advance).

**Why does this work as a proof of work?** Because C' is the product of two random matrices. No matter what A and B the miner chose, (A+E) and (B+F) are random, which means C' is random too. The miner hashes C', and if the hash falls below a difficulty target, they win a block. Just like Bitcoin, except instead of hashing nonces, the miner is multiplying matrices.

**But there's a fatal flaw.** The miner needs to output the useful result C = A·B, not the noisy C'. To recover C from C', they need to “peel off” the noise:

C = C' − A·F − E·(B + F)

That subtraction requires computing A·F and E·(B+F), two additional full n×n matrix multiplications. So the honest miner does 3 GEMMs total (one for C', two for denoising), while a cheating miner who doesn't care about the useful result only needs 1 GEMM to produce C' and check if its hash wins.

The honest miner works 3× harder than a cheater. That's not a proof of useful work; it's a proof of wasted work, just with matrices instead of hashes.

### Idea 2: Make the noise low-rank

The problem was that peeling off the noise cost two full GEMMs. What if the noise were structured so that removing it is cheap?

Instead of making E and F fully random n×n matrices, construct them as low-rank matrices. Specifically, sample four smaller matrices, EL and FLof size n×r, and ER and FR of size r×n, and set:

E = EL·ER     F = FL·FR

where r &ll; n (think r = n0.3, so for n = 4096, r ≈ 20).

E and F are still n×n matrices, but they each have only rank r. They look random (every r×r subblock is marginally uniform) but they have far less independent information than a truly random matrix.

The honest miner now does:

- 1 full GEMM: C' = (A+E)·(B+F), cost O(n³)
- Denoising: cost O(n²·r), negligible

Total overhead: O(n²·r)/O(n³) = O(r/n) = o(1). As matrix dimensions grow, the overhead vanishes.

**But we've introduced a new problem.** A cheating miner who picks A = B = 0 (the all-zeros matrix) reduces their task to computing E·F, the product of two rank-r matrices. And multiplying two rank-r matrices can be done in O(n²·r) time, massively faster than the honest O(n³). The cheater has an enormous advantage.

### Idea 3: Prove the transcript, not the output

Here is the key insight of the paper. The two previous attempts both used the _output_ of the computation, the final matrix C', as the source of hardness. Hash C', check if it's below a target. The problem is that clever attackers can compute the output without doing the full work.

cuPOW does something different. Instead of hashing the output, it hashes the **transcript**, the full sequence of intermediate results that accumulate during the block-by-block execution of the matrix multiplication.

Recall how the canonical matrix multiplication algorithm works on blocks. You partition A' = (A+E) and B' = (B+F) into r×r tiles, and compute the product iteratively:

C'(ℓ)i,j = C'(ℓ-1)i,j \+ A'i,ℓ · B'ℓ,j

where i, j, ℓ range over the n/r block indices. Each step multiplies one r×r tile from A' by one r×r tile from B' and adds it to a running sum. The full computation produces (n/r)³ intermediate r×r matrices. This sequence (every intermediate partial sum at every step) is the **transcript**.

Block matrix multiplication: 4×4 matrices, 2×2 tiles

Each output tile C'\[i,j\] accumulates partial products A'\[i,ℓ\] × B'\[ℓ,j\] across all ℓ. The full sequence of 8 intermediate partial sums is the **transcript**.

▶ play← prevnext →↺ resetStep 0 / 8

A'

2

1

-2

-1

1

0

0

0

0

-1

0

0

1

-2

-1

-1

×

B'

0

1

0

2

0

2

1

0

-2

-1

2

-1

1

2

2

-1

=

C' (running sums)

C'\[0,0\]

·

·

·

·

C'\[0,1\]

·

·

·

·

C'\[1,0\]

·

·

·

·

C'\[1,1\]

·

·

·

·

Use the controls above to step through or play the animation

Transcript0 / 8 entries

Each step records one intermediate 2×2 partial sum. This growing list is the transcript.

Now the miner hashes this entire transcript and checks if the hash falls below the difficulty target. The honest prover produces the transcript naturally; it's just the standard computation, recorded step by step.

**But can a cheater produce the transcript without doing the full work?**

This is where the low-rank noise pays off. Because E and F are random rank-r matrices, the noised matrices A' = A+E and B' = B+F have a crucial property: every r×r tile of A' and B' is marginally uniform; it looks completely random on its own. This means each intermediate product A'i,ℓ · B'ℓ,j is the product of two random r×r matrices, which cannot be computed faster than O(r³).

There are (n/r)³ such intermediates, each costing O(r³), giving a total of O(n³), the full cost of the GEMM.

“But wait,” you might think, “the intermediates are correlated; they share rows and columns from A' and B'. Can't the attacker exploit those correlations to compute multiple intermediates at once?”

The paper argues no. Recovering those correlations amounts to solving a batch of low-rank random linear equations, a problem for which no known algorithm is faster than performing the honest computation. This is the core security assumption of cuPOW.

## Putting it all together

The full cuPOW protocol:

The miner receives an inference request (matrices A, B) and a public seed σ (from the block header):

1. **Encode:** Derive low-rank noise matrices E = EL·ER and F = FL·FR from σ. Set A' = A+E, B' = B+F. Cost: O(n²·r), negligible.
2. **Compute:** Run the block matrix multiplication MatMulr(A', B'), recording the full transcript of (n/r)³ intermediate r×r matrices. Cost: O(n³). This is the real work.
3. **Hash:** Hash the transcript. If the hash falls below the difficulty target, a valid proof is found.
4. **Decode:** Peel off the noise to recover C = A·B. Cost: O(n²·r), negligible.
5. **Output:** Return C to the AI client (the useful inference result) and submit the transcript hash as the proof of work.

The verifier receives (σ, A, B, transcript hash) and recomputes to check correctness.

Total overhead for the honest miner: O(n²·r) / O(n³) = O(r/n) = o(1). For the matrix sizes in modern LLMs, this is a fraction of a percent.

Advantage for a cheating miner: none. Computing the transcript honestly is the fastest known method. No shortcut exists.

The full cuPOW pipeline

1

ENCODE

Add low-rank noise to inference matrices

O(n²r), negligible+

2

COMPUTE

Block matrix multiplication with transcript

O(n³), the real work+

3

HASH

Hash transcript and check difficulty

Negligible+

4

DECODE

Peel off noise to recover useful result

O(n²r), negligible+

5

OUTPUT

Deliver inference + submit proof of work

Two revenue streams+

One GPU. One computation. Two revenue streams. Zero waste.

## Why this matters

Let's step back and appreciate what cuPOW achieves.

For almost 20 years, since Bitcoin launched in 2008, the question of useful proof of work was considered open, and by many, impossible. Vitalik Buterin called it “probably not feasible” in 2019. Ethereum's entire migration to proof of stake was partly motivated by the belief that proof of work could never be made useful.

cuPOW resolves this for matrix multiplication, the single most economically valuable computation in the world. The construction is not a brute-force trick or a clever hack. It follows from a structural observation about matrix multiplication: that the transcript of a block computation over randomly-perturbed matrices is inherently hard to produce without doing the actual work, even when the noise is structured enough to be peeled off cheaply.

The overhead is 1+o(1) and vanishes as matrices grow. The security reduces to a concrete algebraic problem (low-rank random linear equations). And because matrix multiplication _is_ AI inference, the useful work has an immediate, enormous market of paying customers.

## The 2-for-1 property

A single computation simultaneously serves an AI client and secures the blockchain. The energy spent mining Pearl is not wasted; it's the energy cost of AI inference that would have been spent anyway. Pearl just lets the miner get paid twice for it.

## Why Pearl is the store of value of the AI era

Every PRL in existence was mined through verified GPU computation: the actual matrix multiplications that served a real inference request. Token mining is physically bounded by the amount of GPU capacity dedicated to the network. You cannot mine PRL faster without deploying more hardware and doing more useful work.

That makes PRL supply a direct function of the scarcest resource in the global technology economy. GPU demand is growing exponentially. Supply trails by years. Every hyperscaler, every startup, every sovereign fund is competing for the same chips. NVIDIA's lead times stretch past a year. New fab capacity takes three to five years to come online. The bottleneck that constrains PRL issuance is the same bottleneck the entire AI industry is throwing hundreds of billions of dollars at.

No other digital asset has this property. Bitcoin's scarcity is enforced by an arbitrary difficulty adjustment over computation. The energy is spent, the hash is checked, the result is discarded. PRL's scarcity is enforced by the physical availability of GPUs doing work the market is already desperate to buy. The computation isn't discarded; it's delivered to a customer.

This distinction matters for how you think about the asset long-term. Bitcoin's value rests on a social consensus that scarce-because-the-protocol-says-so is valuable. That consensus has proven remarkably durable, but the scarcity is ultimately circular. PRL's scarcity is grounded in something external to the protocol: the supply-demand imbalance of AI compute. As long as the world needs more inference than it can supply, and every credible forecast says that gap is widening, the resource that backs PRL issuance becomes more constrained, not less.

The equilibrium is self-correcting. If PRL trades below the cost of the GPU-hours required to mine it, rational miners redirect hardware to selling inference directly. If PRL trades above that cost, new miners join, expanding both supply and the network's total inference throughput. The token price gravitates toward the real economic cost of AI computation. And as demand for inference grows faster than GPU supply, that floor rises.

Bitcoin proved that a decentralized network can produce trustless digital scarcity. But Bitcoin's scarcity is self-referential; tokens are scarce because the protocol says so, secured by energy that vanishes after use.

Pearl's scarcity is inherited. It comes from a bottleneck that exists independent of the protocol: the global shortage of AI compute. Every token is a receipt for verified useful work that the market was already willing to pay for. The constraint isn't designed. It's physical.

Read the PoUW paper at [arXiv:2504.09971](https://arxiv.org/abs/2504.09971).

[![Pearl](https://pearlresearch.ai/brand/logos/logotype-oyster.svg)](https://pearlresearch.ai/)

- [Twitter / X](https://twitter.com/prlnet "Twitter / X")
- [GitHub](https://github.com/pearl-research-labs "GitHub")
- [Hugging Face](https://huggingface.co/pearl-ai "Hugging Face")
- [YouTube](https://www.youtube.com/@PearlResearchLabs "YouTube")

### Products

- [Pearl Inference Platform](https://platform.pearlresearch.ai/sign-in)

### Research

- [Articles](https://pearlresearch.ai/research)
- [Open Problems](https://pearlpolymath.com/)

### Legal

- [Privacy Policy](https://pearlresearch.ai/legal/privacy)
- [Terms of Service](https://pearlresearch.ai/legal/terms)
- [Cookie Policy](https://pearlresearch.ai/legal/cookies)
- [Accessibility](https://pearlresearch.ai/legal/accessibility)

![AICPA SOC for Service Organizations](https://pearlresearch.ai/_next/image?url=%2Fpartners%2Fsoc2-aicpa.png&w=640&q=90)NVIDIAInception Program

© 2026 Pearl Research Labs

© 2026 NVIDIA, the NVIDIA logo, and all other trademarks mentioned are trademarks and/or registered trademarks of NVIDIA Corporation in the U.S. and other countries.

Proof of Useful Work · Pearl Research Labs