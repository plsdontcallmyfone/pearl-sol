#!/usr/bin/env bash
# Experiment driver on the test VPS (nohup ./pipeline.sh > pipeline.log 2>&1 &).
# Results append to results.jsonl (SmoothQuant alpha sweep to alpha.jsonl).
#
# The box is shared with the pearl-sol demo stack: its miner holds 1.5-3 of 4 cores and
# these runs use nice 10, so one config with tasks took 45-90 min. Sizes and the config
# list were cut in stages as results came in; this file records what ran.
#   Qwen2.5-0.5B, PPL 32 x 1024 + ARC-Easy 500 + HellaSwag 300:
#     base w8a8 w8a8_sq w7a7 w7a7_sq w7a7_sq_mineable w7a7_sq_asym
#   Qwen2.5-0.5B, PPL only: w7a7_sq_mse w7a16 w7a7_sq_lmhead w7a7_pertensor
#     w7a7_sq_asym_mineable; alpha 0.8 for w7a7_sq
#   Llama-3.2-1B, PPL 16 x 1024 + ARC-Easy 300 + HellaSwag 150:
#     base w8a8_sq w7a7_sq_asym_lmhead
#   Llama-3.2-1B, PPL only: w7a7_sq_lmhead w7a7_sq w7a7 w8a8 w7a7_sq_asym w7a16
#     w7a7_sq_mse_lmhead; alpha 0.8 for w7a7_sq
cd /home/pearl/int7-exp
Q="hf/hub/models--Qwen--Qwen2.5-0.5B/snapshots/*"
L="hf/hub/models--unsloth--Llama-3.2-1B/snapshots/*"
if [ "${ALL:-0}" = 1 ]; then
  ./sweep.sh "$Q" results.jsonl "--ppl-windows 32 --arc 500 --hs 300" base w8a8 w8a8_sq w7a7 w7a7_sq w7a7_sq_mineable
  ./sweep.sh "$Q" results.jsonl "--ppl-windows 32" w7a7_sq_mse w7a16 w7a7_sq_lmhead w7a7_pertensor
  ./sweep.sh "$L" results.jsonl "--ppl-windows 16 --arc 300 --hs 150" base
fi
./sweep.sh "$L" results.jsonl "--ppl-windows 16 --arc 300 --hs 150" w7a7_sq_asym_lmhead w8a8_sq
./sweep.sh "$Q" results.jsonl "--ppl-windows 32 --arc 500 --hs 300" w7a7_sq_asym
./sweep.sh "$Q" results.jsonl "--ppl-windows 32" w7a7_sq_asym_mineable
./sweep.sh "$L" results.jsonl "--ppl-windows 16" w7a7_sq_lmhead w7a7_sq w7a7 w8a8 w7a7_sq_asym w7a16 w7a7_sq_mse_lmhead
./sweep.sh "$Q" alpha.jsonl "--ppl-windows 32 --alpha 0.8" w7a7_sq
./sweep.sh "$L" alpha.jsonl "--ppl-windows 16 --alpha 0.8" w7a7_sq
echo PIPELINE_DONE
