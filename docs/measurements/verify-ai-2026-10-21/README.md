# verify-ai 2026-10-21 (cadre-glue-v1, SPEC 4.7)

- acc-a40-final.jsonl: cadre-glue-v1 with the final rules (NB 20, correctly rounded sqrt) on the A40: WikiText-2 PPL + ARC-E/ARC-C/HellaSwag.
- acc-a40.jsonl: the first pass. The cadre-glue-v1 lines (PPL 16.1624) predate the NB / sqrt fix; the fp32-glue lines (16.1694) are the comparison.
- gpu-a40-cpu-vs-gpu.jsonl: tools/cadre/gpu_replay_check.py, final rules (A40 vs Xeon Gold 6342 on one pod).
- logits-cpu-vs-gpu.jsonl: tools/cadre/logits_check.py, 3 full windows, final rules.
- test-replay-testbox.jsonl: tools/cadre/test_replay.py on the test box, final rules (19/19).
