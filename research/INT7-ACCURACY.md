# INT7-ACCURACY: does our int7 job arithmetic keep an open-weight LLM's accuracy?

Measurement for ranked item #3 in research/narratives/SYNTHESIS.md (standing jobs on loaded
open-weight models). Run 2026-10-18 on the test VPS (64.95.13.106, 4 vCPU, CPU only), next to
the running demo stack. Scripts: research/int7-exp/. Every number below comes from those runs
unless a source is named.

## Verdict

**Viable with these changes, not as-is.** Running a model's weight GEMMs through our exact
int7 job arithmetic keeps its accuracy close to standard int8 W8A8 if the recipe is:
per-output-channel weight scales, SmoothQuant factors folded into the registered weights,
**asymmetric** per-token int7 activations (zero-point correction outside the proof), and
attention (QK^T, PV, softmax) left in high precision. Measured cost vs the bf16 reference:
WikiText-2 PPL +2.9% on Llama-3.2-1B with every job-shaped GEMM in int7 (including lm_head),
+2.6% on Qwen2.5-0.5B with all block linears in int7; int8 W8A8 + SmoothQuant costs +1.5% and
+1.8% on the same tokens. No task loss was measurable (ARC-Easy, HellaSwag; SE 2-4 points).
The naive port (symmetric per-token int7) costs +5.8% / +9.9% and is not good enough.

FP8 is **not needed for accuracy** at this scale; it remains the hardware question already in
COMPUTE-TECH (B300 INT8 about 1/30 of FP8). No protocol change is needed: everything above is a
choice of what the poster registers and how the miner rounds activations. Limits: two small
models, CPU fake-quant, 16-33k PPL tokens and 150-500 task items, RTN weights (no GPTQ, which
Pearl itself pairs with SmoothQuant for W7A7, research/pearl_blog.md; Pearl publishes no W7A7
accuracy numbers in that post or in PIP-3).

## 1. What the job format forces (from crates/pearl-job)

- A job proves `C = A B` **exactly** in int32: A is m x k, B is k x n, every entry an integer
  in **[-64, 64]** (`mine.rs` `PreparedJob::build` refuses anything else; 129 levels, about
  7.01 bits). `mma_type` must be 0, Int7 x Int7 -> Int32 (`job.rs` `MiningConfig::from_bytes`).
  The miner computes `(A+E)(B+F)` with low-rank integer noise and recovers AB by integer
  denoising (NOTES-proof.md), so the useful product has no error beyond the operand rounding.
  Overflow cannot happen: |C| <= 64 * 64 * 2^16 = 2^28.
- **No scales live in the job.** So any scale that factors out of the integer product is free
  and stays outside the proof: per-token (per-row of A) and per-output-channel (per-column of
  B) scales, an activation zero point (correction `z * colsum(B)`, precomputed per registered
  B), and SmoothQuant per-input-channel factors folded into the weights or the preceding norm.
- **Not free:** group-wise scales along k (the usual g128 weight groups). Each k-group would
  be its own job, and every job needs k >= 16r with r >= 128, i.e. k >= 2048 (`check_shape`).
  The same rule rules out FP8 checkpoints' 128 x 128 block scales (DeepSeek-V3's
  `quantization_config`: `fp8`, `e4m3`, `weight_block_size [128, 128]`, read from its
  config.json): requantizing those to our format means per-channel scales over the whole k.
- Shape rules for a weight GEMM (`B = W^T` registered, `A` = the miner's activations):
  k >= 2048, k % 64 == 0, k <= 4 r^2 = 65536 at r = 128, n a multiple of the column
  pattern period (8 at the localnet 4 x 8 tile), m padded to the row period (4). r = 128 is
  the only sensible rank: the credited work is `h*w*(dlen/r)*128` (`penalized_adjustment_factor`),
  so any r > 128 earns r/128 times less, and k >= 16r is lowest at r = 128.
- For mining to be a rebate on inference, the served model must use the int7 product itself
  (otherwise the miner does the GEMM twice). So the accuracy that matters is the accuracy of
  the model run with int7 operands, which is what is measured here.

## 2. Simulator, and its check against pearl-job

`quant.py` replaces every `nn.Linear` with a fake-quant layer: weights quantized once offline
(symmetric, per-output-channel absmax / 64 unless stated), activations quantized per token at
run time, then `C = torch._int_mm(qa_int8, qw_int8^T)` (exact int32, the same arithmetic as the
job), then `y = s_a * s_w * C` (+ zero-point correction, bias). Everything else (norms, rotary,
attention softmax, QK^T and PV) runs in fp32. Weights come from the bf16 checkpoint.

`verify_pearljob.py` takes a real layer, captures 64 real input tokens (WikiText-2 validation),
quantizes them exactly as the simulator does, writes a pearl-job data dir (`A.i8`, `B.i8`,
`manifest.json`) and runs the box's `pearl-job` binary (`config`, `commit`, `matmul`,
`freivalds`, `mine`, `verify`):

| model | layer | k x n (m = 64) | job config | pearl-job C == exact int64 | simulator C == exact | pearl-job Freivalds on simulator C | max abs C | layer rel. error int7 / int8 | mined share verifies |
|---|---|---|---|---|---|---|---|---|---|
| Qwen2.5-0.5B | 0 mlp.down_proj | 4864 x 896 | ok (k 4864, r 128, 4x8) | True | True | valid=True | 42534 | 9.0% / 4.9% | True (proof 59256 B, 46 ms/nonce) |
| Qwen2.5-0.5B | 12 mlp.down_proj | 4864 x 896 | ok (k 4864, r 128, 4x8) | True | True | valid=True | 31142 | 7.3% / 3.7% | True (proof 59256 B, 58 ms/nonce) |
| Qwen2.5-0.5B | 23 mlp.down_proj | 4864 x 896 | ok (k 4864, r 128, 4x8) | True | True | valid=True | 21913 | 8.7% / 4.7% | True (proof 59256 B, 46 ms/nonce) |
| Qwen2.5-0.5B | 5 mlp.up_proj | 896 x 4864 | refused: "k must be >= 16r" | - | - | - | - | - | - |
| Llama-3.2-1B | 0 mlp.down_proj | 8192 x 2048 | ok (k 8192, r 128, 4x8) | True | True | valid=True | 31325 | 3.9% / 2.1% | True (proof 99124 B, 233 ms/nonce) |
| Llama-3.2-1B | 8 self_attn.q_proj | 2048 x 2048 | ok (k 2048, r 128, 4x8) | True | True | valid=True | 109788 | 2.4% / 1.2% | True (proof 25108 B, 41 ms/nonce) |
| Llama-3.2-1B | 15 mlp.up_proj | 2048 x 8192 | ok (k 2048, r 128, 4x8) | True | True | valid=True | 175818 | 1.8% / 0.9% | True (proof 25172 B, 156 ms/nonce) |
| Llama-3.2-1B | 15 self_attn.o_proj | 2048 x 2048 | ok (k 2048, r 128, 4x8) | True | True | valid=True | 21657 | 8.6% / 4.5% | True (proof 25108 B, 41 ms/nonce) |

pearl-job 0.1.0 built from the box's repo at 60bb4378; torch 2.5.1+cpu, transformers 4.46.3.

So the simulator's integer product is bit-identical to what a miner gets from a job, on real
model matrices, and those matrices are valid jobs end to end (a share mines and verifies, at
the easy test share target 0x1e040000; one CPU thread on the shared box).
The per-layer relative output error of int7 is about 2x that of int8 on every layer (129 vs 255
levels per operand).

## 3. Method

- Models: Qwen2.5-0.5B (`Qwen/Qwen2.5-0.5B`, hidden 896) and Llama-3.2-1B (the ungated
  `unsloth/Llama-3.2-1B` mirror of Meta's weights, hidden 2048; no HF token was available).
  Qwen2.5-1.5B was left out: at 3.1 GB bf16 it did not fit next to the demo stack (3.4 GB
  available at the start). The two chosen models bracket the job-shape question: in Qwen 0.5B
  only `down_proj` fits a job, in Llama-3.2-1B every projection and lm_head does.
- Reference: bf16 checkpoint weights, fp32 compute. Weights are read zero-copy from the
  safetensors files (mmap) to keep anonymous memory at 0.5-1.4 GB per run.
- Configs (all symmetric RTN unless stated; "SQ" = SmoothQuant alpha 0.5, calibrated on 16 x 512
  WikiText-2 train tokens, folded per layer group: q/k/v and gate/up into the norms, o_proj
  into v_proj rows shared per KV group, down_proj into up_proj rows; all foldable exactly):
  int8 W8A8 per-channel / per-token (the standard reference), int8 + SQ, and ours: int7 W7A7
  per-channel / per-token, + SQ, + asymmetric (zero-point) activations, + MSE-clipped weights,
  per-tensor scales, int7 weights only (W7A16, not a valid job, isolates the weight error),
  job-shaped GEMMs only, lm_head included or not.
- WikiText-2 (raw, test split, "\n\n"-joined): non-overlapping 1024-token windows from the
  start, 32 windows (32,736 scored tokens) for Qwen, 16 (16,368) for Llama. The same tokens
  for every config of a model, so PPL deltas are paired.
- Tasks, lm-eval-harness formatting re-implemented in a light loop (`run.py`): ARC-Easy test
  (first 500 items Qwen, 300 Llama; "Question: ...\nAnswer:" + " choice") and HellaSwag
  validation (first 300 Qwen, 150 Llama; lm-eval's preprocessing). Log-likelihood of each
  continuation; acc and acc_norm (divided by continuation characters). Paired flip counts
  vs the reference are given because one accuracy has a binomial SE of 2-4 points at these n.
- Sizes are small because the box is shared: the demo miner holds 1.5-3 of 4 cores and these
  runs ran at nice 10 (45-90 min per config with tasks). PPL differences are well resolved;
  task differences below about 3 points are not.

## 4. Results

"ours" = int7 W7A7 with per-channel weight / per-token activation scales (the `w7a7` row);
raw records in int7-exp/results/results.jsonl (task picks included), tables from
`analyze.py results/results.jsonl results/gold.json`.

### Qwen2.5-0.5B (only down_proj is job-shaped)

WikiText-2 test: 32 windows x 1024 tokens = 32736 scored tokens (of 292 windows). ARC-Easy test first 500 items, HellaSwag validation first 300 items.

| config | quantized linears | WikiText-2 PPL | dPPL vs base | dPPL vs int8 SQ | ARC-E acc | ARC-E acc_norm | HellaSwag acc | HellaSwag acc_norm |
|---|---|---|---|---|---|---|---|---|
| base: bf16 weights, fp32 compute (reference) | 0 | 14.373 | +0.000 (+0.0%) | -0.265 | 63.6 | 58.6 | 39.3 | 50.0 |
| w8a8: int8 W8A8 per-channel / per-token, RTN | 168 | 14.782 | +0.409 (+2.8%) | +0.144 | 60.6 | 58.2 | 40.0 | 50.3 |
| w8a8_sq: int8 W8A8 + SmoothQuant (a=0.5) | 168 | 14.638 | +0.265 (+1.8%) | +0.000 | 63.4 | 59.8 | 41.7 | 49.7 |
| w7a16: int7 W per-channel, fp activations (not a job: A not int7) | 168 | 14.512 | +0.140 (+1.0%) | -0.125 | - | - | - | - |
| w7a7: int7 W7A7 per-channel / per-token, RTN | 168 | 15.801 | +1.429 (+9.9%) | +1.163 | 62.0 | 58.8 | 40.3 | 48.7 |
| w7a7_sq: ours + SmoothQuant (a=0.5) | 168 | 15.032 | +0.659 (+4.6%) | +0.394 | 62.2 | 61.0 | 39.0 | 49.3 |
| w7a7_sq_asym: ours + SQ, asymmetric (zero-point) activations | 168 | 14.752 | +0.380 (+2.6%) | +0.114 | 62.4 | 58.8 | 39.3 | 51.7 |
| w7a7_sq_mineable: ours + SQ on job-shaped GEMMs only (k >= 2048), rest fp | 24 | 14.773 | +0.400 (+2.8%) | +0.135 | 63.4 | 61.0 | 40.3 | 52.3 |
| w7a7_sq_lmhead: ours + SQ, lm_head also int7 | 169 | 15.468 | +1.095 (+7.6%) | +0.830 | - | - | - | - |
| w7a7_sq_asym_mineable: ours + SQ + asym, job-shaped GEMMs only | 24 | 14.649 | +0.277 (+1.9%) | +0.012 | - | - | - | - |
| w7a7_sq_mse: ours + SQ, MSE-clipped weights | 168 | 15.004 | +0.631 (+4.4%) | +0.366 | - | - | - | - |
| w7a7_pertensor: ours, per-tensor W and A scales | 168 | 219.932 | +205.559 (+1430.2%) | +205.294 | - | - | - | - |

ARC n=500: binomial SE of one accuracy at p=0.59 is 2.2 points. Paired flips vs base (acc_norm; lost / won):
| config | lost | won | net |
|---|---|---|---|
| w8a8 | 14 | 12 | -2 |
| w8a8_sq | 8 | 14 | +6 |
| w7a7 | 22 | 23 | +1 |
| w7a7_sq | 14 | 26 | +12 |
| w7a7_sq_asym | 11 | 12 | +1 |
| w7a7_sq_mineable | 12 | 24 | +12 |

HS n=300: binomial SE of one accuracy at p=0.50 is 2.9 points. Paired flips vs base (acc_norm; lost / won):
| config | lost | won | net |
|---|---|---|---|
| w8a8 | 7 | 8 | +1 |
| w8a8_sq | 7 | 6 | -1 |
| w7a7 | 13 | 9 | -4 |
| w7a7_sq | 13 | 11 | -2 |
| w7a7_sq_asym | 8 | 13 | +5 |
| w7a7_sq_mineable | 5 | 12 | +7 |

### Llama-3.2-1B (every projection and lm_head is job-shaped)

WikiText-2 test: 16 windows x 1024 tokens = 16368 scored tokens (of 282 windows). ARC-Easy test first 300 items, HellaSwag validation first 150 items.

| config | quantized linears | WikiText-2 PPL | dPPL vs base | dPPL vs int8 SQ | ARC-E acc | ARC-E acc_norm | HellaSwag acc | HellaSwag acc_norm |
|---|---|---|---|---|---|---|---|---|
| base: bf16 weights, fp32 compute (reference) | 0 | 10.358 | +0.000 (+0.0%) | -0.151 | 63.7 | 61.0 | 44.0 | 58.7 |
| w8a8: int8 W8A8 per-channel / per-token, RTN | 112 | 10.515 | +0.157 (+1.5%) | +0.007 | - | - | - | - |
| w8a8_sq: int8 W8A8 + SmoothQuant (a=0.5) | 112 | 10.509 | +0.151 (+1.5%) | +0.000 | 64.3 | 62.0 | 44.7 | 58.0 |
| w7a16: int7 W per-channel, fp activations (not a job: A not int7) | 112 | 10.415 | +0.057 (+0.5%) | -0.094 | - | - | - | - |
| w7a7: int7 W7A7 per-channel / per-token, RTN | 112 | 10.957 | +0.599 (+5.8%) | +0.448 | - | - | - | - |
| w7a7_sq: ours + SmoothQuant (a=0.5) | 112 | 10.821 | +0.463 (+4.5%) | +0.312 | - | - | - | - |
| w7a7_sq_asym: ours + SQ, asymmetric (zero-point) activations | 112 | 10.656 | +0.298 (+2.9%) | +0.147 | - | - | - | - |
| w7a7_sq_lmhead: ours + SQ, lm_head also int7 | 113 | 10.832 | +0.474 (+4.6%) | +0.323 | - | - | - | - |
| w7a7_sq_asym_lmhead: ours + SQ + asym activations, lm_head also int7 | 113 | 10.662 | +0.304 (+2.9%) | +0.153 | 65.0 | 62.3 | 45.3 | 60.7 |
| w7a7_sq_mse_lmhead: ours + SQ, MSE-clipped weights, lm_head also int7 | 113 | 10.820 | +0.462 (+4.5%) | +0.311 | - | - | - | - |

ARC n=300: binomial SE of one accuracy at p=0.61 is 2.8 points. Paired flips vs base (acc_norm; lost / won):
| config | lost | won | net |
|---|---|---|---|
| w8a8_sq | 6 | 9 | +3 |
| w7a7_sq_asym_lmhead | 7 | 11 | +4 |

HS n=150: binomial SE of one accuracy at p=0.59 is 4.0 points. Paired flips vs base (acc_norm; lost / won):
| config | lost | won | net |
|---|---|---|---|
| w8a8_sq | 3 | 2 | -1 |
| w7a7_sq_asym_lmhead | 2 | 5 | +3 |

SmoothQuant alpha (int7 W7A7 + SQ, PPL only, results/alpha.jsonl): Qwen2.5-0.5B 15.034 at
alpha 0.8 vs 15.032 at 0.5; Llama-3.2-1B 11.279 at 0.8 vs 10.821 at 0.5. Alpha 0.5 is kept.

What the numbers say:

- **Plain int7 is not enough.** Symmetric per-token / per-channel RTN, the obvious port of
  W8A8: +9.9% PPL on Qwen 0.5B and +5.8% on Llama 1B, against +2.8% / +1.5% for the same
  recipe in int8. Per-tensor scales are unusable (Qwen PPL 219.9).
- **The loss is in the activations, not the weights.** int7 weights with fp activations cost
  only +1.0% (Qwen) and +0.5% (Llama), and MSE weight clipping moves almost nothing (15.032 to
  15.004; 10.832 to 10.820 with lm_head). The 129-level activation grid is the problem.
- **Two free fixes close most of the gap:** SmoothQuant folded into the registered weights,
  and asymmetric (zero-point) per-token activations whose correction `z * colsum(B)` is
  computed outside the proof. Together: Qwen 0.5B +2.6% (all 168 block linears int7), Llama 1B
  +2.9% (all 112 block linears + lm_head int7, i.e. every GEMM that fits a job). That is
  within 0.8 (Qwen) and 1.4 (Llama) PPL-percentage points of int8 W8A8 + SmoothQuant, and
  equal to or better than int8 RTN on Qwen (14.752 vs 14.782).
- **Tasks show no measurable loss** at these sample sizes: the best int7 configs score at or
  above the reference on ARC-Easy and HellaSwag (Llama acc_norm 62.3 vs 61.0 and 60.7 vs 58.7;
  Qwen 58.8 vs 58.6 and 51.7 vs 50.0), with paired flips roughly balanced. The binomial SE is
  2.2-4.0 points, so this rules out large drops (more than about 5 points), not small ones.
- **lm_head:** on Llama int7 lm_head with asymmetric activations is free (10.656 to 10.662).
  On Qwen (k = 896, not job-shaped anyway) int7 lm_head with symmetric activations costs a
  lot (15.032 to 15.468), so keep lm_head in high precision where it cannot be mined.
- **Mining only the job-shaped GEMMs** (Qwen: down_proj alone, 24 of 168 linears) costs +1.9%
  with asym activations: down_proj, the layer with the known activation outliers, carries most
  of Qwen's int7 error (+2.8% alone with symmetric activations vs +4.6% for all layers).

## 5. Which matmuls can be mined, and what share of FLOPs (flops.py)

Per token, from each model's config.json; attention QK^T + PV counted at an average past
length of 2048 (decode at 4K context). Attention is activation x activation, so it can never
be a standing job (no registered B); the embedding lookup is not a GEMM.

| model | hidden | GEMMs that fit (k >= 2048) | MACs/token (GEMM + attn at T=2048) | mineable share of all MACs | of GEMM MACs | of GEMM MACs excl. lm_head | fit only with k-padding |
|---|---|---|---|---|---|---|---|
| Qwen--Qwen2.5-0.5B | 896 | down_proj | 0.58 G | 18.0% | 21.2% | 29.2% | 78.8% |
| Qwen--Qwen2.5-1.5B | 1536 | down_proj | 1.72 G | 22.4% | 25.0% | 29.4% | 75.0% |
| Qwen--Qwen2.5-72B | 8192 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 74.14 G | 96.4% | 100.0% | 100.0% | 0.0% |
| Qwen--Qwen2.5-7B | 3584 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 7.48 G | 94.5% | 100.0% | 100.0% | 0.0% |
| Qwen--Qwen3-30B-A3B | 2048 | expert gate/up, k_proj, lm_head, o_proj, q_proj, v_proj | 3.83 G | 63.2% | 80.1% | 77.8% | 19.9% |
| Qwen--Qwen3-8B | 4096 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 8.17 G | 92.6% | 100.0% | 100.0% | 0.0% |
| unsloth--Llama-3.1-8B | 4096 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 8.04 G | 93.3% | 100.0% | 100.0% | 0.0% |
| unsloth--Llama-3.2-1B | 2048 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 1.37 G | 90.2% | 100.0% | 100.0% | 0.0% |
| unsloth--Llama-3.3-70B-Instruct | 8192 | down_proj, gate_proj+up_proj, k_proj, lm_head, o_proj, q_proj, v_proj | 72.19 G | 96.3% | 100.0% | 100.0% | 0.0% |
| deepseek-ai--DeepSeek-V3 | 7168 | expert down, expert gate/up, kv_a_proj, lm_head, mlp.down (dense layers), mlp.gate/up (dense layers), o_proj, q_a_proj | 42.66 G | 77.8% | 90.9% | 90.7% | 9.1% |

- Small models are mostly unmineable: Qwen2.5-0.5B and 1.5B (hidden 896 / 1536) only have
  `down_proj` with k >= 2048, 18-22% of their MACs. Every dense model with hidden >= 2048
  (Llama-3.2-1B and up, Qwen2.5-7B and up, Qwen3-8B/32B, Llama-3.1-8B, Llama-3.3-70B) fits on
  100% of its weight GEMMs, 90-96% of all MACs; the rest is attention.
- MoE: Qwen3-30B-A3B's expert `down` (k = 768) does not fit (63% of all MACs mineable);
  DeepSeek-V3's MLA up-projections `q_b_proj` (k = 1536) and `kv_b_proj` (k = 512) do not fit
  (78% mineable). Each routed expert would be its own registered B with small per-expert m,
  which the job allows (m only needs padding to 4).
- Zero-padding k up to 2048 makes a small layer job-shaped but wastes (2048 - k)/2048 of the
  mined MACs (56% for Qwen 0.5B's k = 896), which the chain would credit as work. Not worth it.
- [derived] Denoising overhead on top of the useful GEMM is about 2r/k extra MACs
  (`E_A (B+F)` and `A F` are each m x r x n after the sparse factors): 12.5% at k = 2048, 3% at
  k = 8192. Pearl reports +5.08% end to end on Llama 70B and +3.9% on DeepSeek V3.2 for its
  kernel (research/pearl_blog.md).

## 6. What it means for item #3

- **Go on item #3, with the quantization recipe fixed in the registry.** `RegisterModel`
  needs, per registered B: the int7 weights (per-channel absmax / 64, SmoothQuant factors
  already folded), the per-channel scale vector, `colsum(B)` for the zero-point correction,
  and the activation recipe ("per-token asymmetric int7 onto [-64, 64]"). None of it enters
  the proof; the job stays `C = A B` exactly. Group-wise (g128) weights and FP8 block-scaled
  checkpoints cannot be registered as-is: they must be requantized to per-channel scales.
- **Pick models with every GEMM dimension >= 2048.** Dense models from Llama-3.2-1B up and
  Qwen2.5-7B / Qwen3-8B up are 100% job-shaped on weight GEMMs (90-96% of all MACs). Qwen2.5
  0.5B / 1.5B (18-22%) are bad standing-job targets. MoE and MLA models lose 20-37% to small
  expert-down or latent dims (Qwen3-30B-A3B 63%, DeepSeek-V3 78% mineable).
- **The miner must serve the int7 model** (otherwise it does the GEMM twice), so the served
  quality is the int7 quality above: about 1-1.5 PPL-percentage points behind int8 serving.
  That is the honest disclosure for the storefront: "W7A7-quantized open model".
- **Before building, one more measurement is worth it** on a GPU box: a 7-8B model (the
  likely product), full WikiText-2 plus the standard lm-eval suites, GPTQ weights, and a real
  int8 kernel (clamped to [-64, 64]) to measure serving throughput. This run says the recipe
  works on small models; larger models have larger activation outliers, which is exactly what
  SmoothQuant and the zero point address, but it has not been measured here.
- **FP8 (G5) stays a hardware hedge, not an accuracy fix.**

## 7. Llama-3.1-8B-Instruct and Llama-3.2-1B-Instruct on a GPU (2026-10-20, measured)

Recipe 1 (`cadre-w7a7-sq-asym-v1`, tools/cadre/quantize.py, k shards <= 2048) applied to
unsloth/Llama-3.1-8B-Instruct (an ungated copy of meta-llama/Llama-3.1-8B-Instruct) and
unsloth/Llama-3.2-1B-Instruct, run through the real worker (tools/cadre/worker.py --device cuda,
every job-shaped GEMM as an exact int32 product) and compared with the fp16 Hugging Face model on
the same token ids (tools/cadre/eval_accuracy.py). One NVIDIA A40 on RunPod, torch 2.5.1+cu124,
transformers 4.46.3. Calibration as the recipe (16 x 512 WikiText-2 train tokens, fp32 forward; on
the GPU for the 8B, TF32 off). Eval: WikiText-2 raw test, first 40 windows of 1024 tokens (40,920
predicted tokens); ARC-Easy and ARC-Challenge test, HellaSwag validation, first 200 items each,
zero-shot, acc = highest summed log-prob, acc_norm = per byte. 200 items gives a standard error of
about 3 points, so the multiple-choice gaps below are within noise. Raw lines:
docs/measurements/gpu-a40-2026-10-20/acc-*.jsonl.

| model | WikiText-2 PPL fp16 | int7 | change | ARC-E acc fp16 / int7 | ARC-C acc | HellaSwag acc_norm |
|---|---|---|---|---|---|---|
| Llama-3.1-8B-Instruct (640 matrices) | 8.3131 | 8.5664 | +3.05% | 0.795 / 0.775 | 0.535 / 0.540 | 0.695 / 0.690 |
| Llama-3.2-1B-Instruct (144 matrices) | 15.7729 | 16.1694 | +2.51% | 0.625 / 0.640 | 0.330 / 0.330 | 0.610 / 0.585 |

Reading: the recipe holds at 8B (+3.05% PPL, against +2.6-2.9% on the 1B base models in section 4),
and the multiple-choice scores move by at most 2.5 points either way. Not done: a bigger
multiple-choice set, other 7-8B families (Qwen2.5-7B was not needed: the ungated Llama copy loaded).

Constraint found on the way: the 8B has 640 registered matrices (32 layers x {qkv 2, o 2, gate 4,
up 4, down 7 shards} + lm_head 16 column x 2 k shards), and `RegisterModel` takes at most 384
(SPEC 4.5), so an 8B model cannot be registered as is. Wider column shards do not get under the
limit (at k = 2048, k x n <= 2^26 allows n up to 32768, which still gives 488 matrices), so serving
an 8B needs a higher matrix count limit in RegisterModel (a chain change).
