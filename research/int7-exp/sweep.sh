#!/usr/bin/env bash
# Usage: sweep.sh MODEL_DIR_GLOB OUT.jsonl "ARGS" cfg1 cfg2 ...   (runs on the test VPS)
# Runs each config in a fresh process at nice 10 so the demo stack keeps priority.
set -u
cd /home/pearl/int7-exp
export HF_HOME=/home/pearl/int7-exp/hf HF_HUB_OFFLINE=1 HF_DATASETS_OFFLINE=1 TOKENIZERS_PARALLELISM=false
# The demo miner holds 1.5-3 cores; 2 threads with passive OpenMP waits avoid spin waste.
export OMP_NUM_THREADS=${OMP_NUM_THREADS:-2} OMP_WAIT_POLICY=PASSIVE
M=$(ls -d $1 | head -1); OUT=$2; ARGS=$3; shift 3
for c in "$@"; do
  /usr/bin/time -f "maxrss_kb=%M cfg=$c" nice -n 10 venv/bin/python run.py --model "$M" --cfg "$c" $ARGS --out "$OUT" 2>&1 \
    | grep -E '^\{|maxrss|Error|error' | cut -c1-600
done
