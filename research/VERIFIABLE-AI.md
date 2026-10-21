# VERIFIABLE-AI: making Cadre's AI answers checkable (2026-10-21)

Question: how can an independent evaluator decide that a paid response is exactly what the
registered model produces for the request? It must use common hardware (CPU or another GPU),
trust nothing the server says, and be fast enough to run on every ticket or on a staked sample.

Starting point (SPEC 4.5, 5.4; HANDOFF items 17, 20): every weight GEMM of a request is a
standing job with an exact int32 `C = A B` and a committed `c_hash`, and the evaluator rechecks
every segment. Nothing tied one segment's C to the next segment's A. The glue in between
(RMSNorm, RoPE, attention, softmax, SiLU, residuals, greedy pick) was fp32. Measured on an A40
on 2026-10-20: C is bit-identical between GPU and CPU, but A differs in 0.002% of elements at
layer 1 and 79% at lm_head, and 1 of 4 prompts changed wording. So "verifiable AI" was not true:
a server could post correct GEMMs over made-up activations.

**Decision: (a), deterministic glue (cadre-glue-v1, SPEC 4.7), built and shipped.** (b) and
(c) are kept as layers on top: (c)'s bonds are designed, and (b) is not needed once (a) holds.

## 1. State of the art (web research 2026-10-21; numbers are the sources', with their conditions)

| Approach | Mechanism | Published numbers | Fit for Cadre |
|---|---|---|---|
| TOPLOC (Prime Intellect) [1][2] | Every 32 tokens, commit the top-128 values of the last hidden state as a polynomial. The verifier replays as one prefill and accepts within thresholds (bf16: exponent mismatches <= 38, mean mantissa diff <= 10, median <= 8). | 258 bytes per 32 tokens; commit 0.26 ms/token; validation 81 ms (Llama-2-13B); "100x faster" validation than inference (blog); 0% FPR/FNR in their tests. INTELLECT-2 / SYNTHETIC-2: FPR 0.000925% (37 slashes) over a 4M-sample run; median verification 25x cheaper than re-inference [3][4]. Weak spot: fp8 vs bf16 "margin is small". | A probabilistic tolerance check. It cannot name an element and cannot be a fraud proof a third party verifies bit for bit. Our GEMMs are already exact, so we can do better than tolerance. |
| Thinking Machines, batch invariance [5][6] | Nondeterminism comes from batch-dependent reduction order (RMSNorm, matmul, split-KV attention), not from atomics. Kernels with a fixed reduction order make temperature-0 results identical. | Qwen3-235B: default 80 unique completions of 1000, with batch-invariant kernels 1000 identical. Qwen3-8B: 26 s default vs 42-55 s deterministic (1.6-2.1x). | The right diagnosis, but only for one GPU type: same kernels, same hardware. |
| vLLM / SGLang deterministic modes [7][8] | Batch-invariant kernels; seeded Gumbel sampling. | vLLM `VLLM_BATCH_INVARIANT=1` needs SM 9.0+ (H100+); Llama-3.2-1B-Instruct is on its tested list. SGLang: +34.35% average slowdown (+24.4% to +55.1%, Qwen3-8B, H200). | Same-SKU determinism only. |
| cuBLAS reproducibility [9] | | Bitwise identical only for the same toolkit version, architecture and SM count. | Confirms that float GEMM glue cannot cross devices. |
| EigenAI (EigenCloud) [10][11] | Bit-exact inference on a pinned GPU SKU, driver and kernels; logs on EigenDA; a challenger triggers a TEE re-run, a 2/3 committee votes, and a mismatch slashes. | 10,000 runs all hashes matched (two H100 nodes); **A100 vs H100: 0.0% match**; deterministic overhead +1.8% end to end. | Shows the cost of float determinism: it holds within one SKU and fails across SKUs. Our evaluator must run on any CPU. |
| Gensyn Verde + RepOps [12] | Refereed delegation: bisect checkpoint hashes to the first diverging step, then a Merkle tree of graph nodes to one operator, re-run by a referee. RepOps fixes float order and reimplements exp/tanh. | RepOps inference overhead: Llama-3.1-1B **+218% on T4, +58% on A100**; 8B +98% on A100 80GB. | Closest to us (cross-hardware bit identity through fixed-order ops). Their cost is float reimplementation; ours is integer glue, measured below. |
| RepDL (Microsoft) [13] | Correctly rounded math functions and fixed-order reductions give bitwise identity across platforms. | "Degrades performance mildly", no number. | Same idea as our binary64 rules. |
| opML (ORA / Hyper Oracle) [14] | Optimistic posting with a bisection fraud proof down to one MIPS instruction arbitrated on chain; determinism from fixed point and softfloat. | No measured gas or latency; analytical estimate ~$0.002 per request. | The fraud-proof shape we want, but a VM-level dispute is heavy. We dispute at one segment of one request. |
| zkML: zkLLM [15], DeepProve [16][17] | SNARKs over the whole inference, with lookups for nonlinear ops. | zkLLM LLaMA-2-13B: prover 803 s, verifier 3.95 s, proof 188 kB (A100, seq 2048). DeepProve: 174 tokens/min GPT-2, 86 tokens/min Gemma 3; perplexity +4% on Gemma-3. | Orders of magnitude too slow for every paid ticket today. |
| Hyperbolic Proof of Sampling [18] | With probability p, re-run the request on another server and compare hashes exactly; on a mismatch a 2/3 vote slashes. Honest play is the Nash equilibrium if p > C / ((1 - r)(R + S)). | Worked example: p ~ 0.735% (r = 10%, R = 1.2C, S = 150C); analytical only. A 2026 repeated-game paper argues one-round slashing analyses overstate security [19]. | Needs an exact equality check underneath: (a) gives one. |
| I-BERT [20], I-LLM [21] | Integer-only transformers (integer GELU/softmax/norm). | I-BERT: GLUE accuracy similar to FP. I-LLM: big PPL losses at W4A4 (LLaMA3-8B 6.14 -> 21.19), W6A6 small; the loss is mostly 4-bit activations. | Evidence that integer nonlinearities cost little when activations stay wide. Our glue keeps Q12-Q20 intermediates; only the GEMM inputs are int7. |
| Others | SVIP [22] (a proxy model on hidden states: FNR < 5%, FPR < 3%); VeriLLM [23] (prefill-only replay, ~1% of inference cost claimed). | | Statistical, not exact. |

## 2. The three options for Cadre

**(a) Deterministic glue.** Specify every non-GEMM step so every honest machine computes the same
A. A float spec is not enough across devices ([9][10]: even A100 vs H100 fails). An integer one
is, and it is cheap here because the GEMM inputs are already int7, so every glue output is
re-quantized anyway. Mismatches become provable bit for bit, down to one element of one
segment. Then a fraud proof is "replay this one request up to this segment", which anyone can
check with the registered model.

**(b) TOPLOC-style tolerance replay.** Keep fp32, commit activation fingerprints, accept within
thresholds. It is cheap and field-tested [3][4], but probabilistic. A server can choose any
output inside the tolerance (a tolerance is a budget for steering), and nothing is provable to
a third party. Once (a) holds, (b) adds nothing.

**(c) Spot-check replay with bonds and slashing.** This is an economic layer. It needs an exact
check underneath to decide what counts as a mismatch [18][19], which (a) supplies. With (a), a
mismatch is a public, self-contained fraud proof, so a slash is checkable by anyone, including
the evaluators' own honesty. Designed in SPEC 4.7, not built: the chain executor is under other
agents' rollouts. Replaying every ticket is affordable (below), so sampling is not needed yet.

## 3. What was built (2026-10-21)

- cadre-glue-v1 (SPEC 4.7). Integer fixed point wherever a value crosses a reduction: residual
  Q16, q/k Q12, v Q16, softmax weights Q20, exact integer sums. Single binary64 operations for
  elementwise scaling, in a stated order. exp/cos/sin from integer tables (Taylor in Q60,
  rotation recurrence for RoPE). Integer dot products run as binary64 matmuls only under a 2^53
  bound.
- Two independent implementations. tools/cadre/glue.py is the worker (torch, CPU and CUDA).
  crates/cadre-glue is the evaluator (Rust, plus the `cadre-replay` CLI). They agree on every
  table (sha256 757b2c06...) and on every A of every request tested.
- The evaluator's full ticket replay (services/attester replay.rs) and fraud proofs
  (`cadre-fraud-v1`); the gateway publishes the replay bundle in the response blob.

## 4. Measured cost (2026-10-21; raw lines in docs/measurements/verify-ai-2026-10-21/)

Accuracy (RunPod A40, same model files, the INT7-ACCURACY section 7 eval: WikiText-2 test 40 x 1024
windows, 40,920 tokens; 200 items per task):

| glue | WikiText-2 PPL | ARC-E acc | ARC-C acc | HellaSwag acc_norm |
|---|---|---|---|---|
| cadre-glue-v1 | 16.1770 (+0.05%) | 0.625 | 0.320 | 0.575 |
| fp32 glue | 16.1694 | 0.640 | 0.330 | 0.585 |

So the accuracy cost is 0.05% PPL, and every task gap is within the ~3-point standard error of
200 items. For scale: int7 itself costs +2.51% PPL against fp16 on this model (INT7-ACCURACY 7).

Speed (A40 and the same pod's Xeon Gold 6342 at 16 threads, decode tokens/s on the 4 test
prompts): cadre-glue-v1 GPU 7.2-8.4, CPU 3.9-4.7; fp32 glue GPU 19.2-24.7, CPU 5.2-7.2. So
deterministic glue decodes about 2.8x slower on the GPU and about 1.5x slower on the CPU in this
implementation. The GPU path is launch-bound: many small int64 / binary64 kernels per token, and
a Python loop over the 8 KV heads. Fused kernels are the obvious next step; nothing here says the
integer arithmetic itself is slow. For comparison, RepOps reports +58% on an A100 for a 1B model
[12].

Bit identity: all 144 matrices' A and C are equal between the A40 and the Xeon on every prompt.
The full binary64 logits of three 1024-token WikiText-2 windows are also equal (131,334,144
values each). Under fp32 glue, 126-144 matrices differed and 2 of 4 answers changed.

Found on the way: torch's CPU float64 `sqrt` is not correctly rounded. It goes through SLEEF and
was 1 ulp off in 5 of 1024 rows, against CUDA, `math.sqrt` and Rust, which agree. That never
changed an A (int7 rounding hides it), but it changed logits and the residual stream. The worker
now takes CPU sqrt from numpy, and the spec requires correctly rounded sqrt. Any reimplementation
must check its math library the same way (tools/cadre/test_cadre.py `test_sqrt_is_correctly_rounded`).

Replay cost (Rust evaluator, test box at load ~20): one 48-row request replays in 6.1-6.3 s
(GEMM 5.1-5.3 s, glue 1.0 s); a full ticket of 2 requests (96 rows, 144 segments) is accepted in
19.2 s through the evaluator. The M1 check already ran those GEMMs, so the replay itself costs
the glue, about 15% of the check. Verifying a fraud proof costs the request's forward up to the
named segment: 0.005 s at layers.0.qkv, 1.8 s at layer 4, 6.3 s at the last lm_head shard.

GPU spend for all of this: one A40 at $0.59/hr, 15:49-16:55 UTC 2026-10-21, $0.66 (account
balance before and after), pod terminated and confirmed through the API.

## 5. Sources

[1] https://arxiv.org/abs/2501.16007 (TOPLOC, ICML 2025; html https://arxiv.org/html/2501.16007v2)
[2] https://www.primeintellect.ai/blog/toploc
[3] https://arxiv.org/abs/2505.07291 (INTELLECT-2)
[4] https://www.primeintellect.ai/blog/synthetic-2 and https://www.primeintellect.ai/blog/synthetic-2-release
[5] https://thinkingmachines.ai/blog/defeating-nondeterminism-in-llm-inference/
[6] https://github.com/thinking-machines-lab/batch_invariant_ops
[7] https://docs.vllm.ai/en/v0.16.0/features/batch_invariance/
[8] https://lmsys.org/blog/2025-09-22-sglang-deterministic/
[9] https://docs.nvidia.com/cuda/cublas/index.html#results-reproducibility
[10] https://arxiv.org/abs/2602.00182 (EigenAI)
[11] https://blog.eigencloud.xyz/deterministic-ai-inference-eigenai/
[12] https://arxiv.org/abs/2502.19405 (Verde, RepOps)
[13] https://arxiv.org/abs/2510.09180 and https://github.com/microsoft/RepDL
[14] https://arxiv.org/abs/2401.17555 (opML)
[15] https://arxiv.org/abs/2404.16109 (zkLLM)
[16] https://eprint.iacr.org/2026/1112 (DeepProve)
[17] https://lagrange.dev/engineering-updates/lagrange-engineering-update-q4-2025
[18] https://arxiv.org/abs/2405.00295 (Proof of Sampling) and https://hyperbolic.xyz/blog/hyperbolic-introduces-novel-practical-solution-to-the-problem-of-verification-in-decentralized-ai
[19] https://arxiv.org/abs/2608.09055 (repeated-game slashing analysis)
[20] https://arxiv.org/abs/2101.01321 (I-BERT)
[21] https://arxiv.org/html/2405.17849v2 (I-LLM)
[22] https://arxiv.org/abs/2410.22307 (SVIP)
[23] https://arxiv.org/abs/2509.24257 (VeriLLM)
Ritual (optimistic / TEE / zk modes, no published numbers): https://ritual.net/blog/faq, https://ritual.net/blog/eigenlayer
