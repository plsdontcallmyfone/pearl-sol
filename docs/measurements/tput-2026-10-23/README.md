# Paid inference throughput, 2026-10-23 ("tput" row; docs/SPEED.md 8)

Box 0 read only; experiments on the test box 64.95.13.106 (same CPU model as box 0: Xeon Gold
6142, avx512bw/dq/vl, no VNNI; torch 2.5.1+cpu, numpy 2.5.4, transformers 4.46.3, the same
versions as box 0's venv), model ~/cadre-models/llama-3.2-1b-instruct-w7a7-k2048.

| file | what |
|---|---|
| box0-snapshot.json | box 0: CPU consumers, worker env, worker /health over 428 s, gateway /v1/stats, served rows / ttft / decode per UTC hour (24 h) |
| testbox-prof-t2.jsonl | profile_step.py, old code, 2 threads: step time + top torch ops for prefill 64/256, decode 1/2/4/8, mixed |
| testbox-prof-t2-new.jsonl | same, new code (prefill 16/32/64/128) |
| testbox-prof-threads.jsonl | new code at 1, 3, 4 threads |
| testbox-prof-contend.jsonl | 4 CPUs, idle vs five busy loops (3 nice 0, 2 nice 10), default vs OMP_WAIT_POLICY=PASSIVE vs niced loops (scripts/contend.sh) |
| testbox-int_mm-rows.txt | CPU `_int_mm` time by row count (exactness checked) |
| testbox-mix.jsonl | bench_mix.py trace, per request and summary, 7 worker configs (scripts/mixall.sh, mixall2.sh, mixrun.sh) plus the rss- runs |
| testbox-rss.txt | peak RSS (VmHWM) old vs new config on an 8-request trace (scripts/rss.sh) |
| testbox-golden-orig.json, testbox-golden-new.json | golden.py digests, old code and new code: equal |
| testbox-test_batch-new.jsonl | test_batch.py on the new code with the Rust cadre-replay: 28/28 |
| testbox-test_replay-new.jsonl | test_replay.py on the new code: 19/19 |
| testbox-c-compression.json | C files of one real 394-row request: zstd / zlib ratios and speed (scripts/czbench.py, gen_c.py) |

Scripts in scripts/ ran from /home/pearl/tput on the test box (removed after the run).
