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
