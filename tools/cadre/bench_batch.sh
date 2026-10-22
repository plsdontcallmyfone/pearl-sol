#!/usr/bin/env bash
# One-command throughput run of the batching worker (CPU or GPU), for docs/TEST-REPORT and the
# price model. Starts its own worker on a free port, runs bench_batch.py at each concurrency,
# stops the worker by PID. Every number lands in $OUT (JSON lines).
#   MODEL=<model dir> PY=<python> PORT=10750 DEVICE=cpu THREADS=0 MAX_BATCH=16 CONC=1,2,4,8,16
#   PROMPT=64 NEW=64 ROUNDS=2 SEG=<scratch dir for segment files> OUT=<results.jsonl> LABEL=...
#   POOL_POS=2048 positions per KV pool slot (CUDA: max_batch x POOL_POS x 196 KB on the 1B)
#   NOSEG=1 skips the segment files (compute only; the gateway always needs them).
#   SERIAL=1 also runs the one-at-a-time worker (--max-batch 1) at concurrency 1 and 4 first.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MODEL="${MODEL:?MODEL=<model dir>}"
PY="${PY:-python3}"; PORT="${PORT:-10750}"; DEVICE="${DEVICE:-cpu}"; THREADS="${THREADS:-0}"
MAX_BATCH="${MAX_BATCH:-16}"; CONC="${CONC:-1,2,4,8,16}"; PROMPT="${PROMPT:-64}"; NEW="${NEW:-64}"; ROUNDS="${ROUNDS:-2}"
SEG="${SEG:-$(mktemp -d)}"; OUT="${OUT:-$HERE/bench-batch.jsonl}"; LABEL="${LABEL:-$DEVICE}"
STEP_ROWS="${STEP_ROWS:-256}"; LIVE_ROWS="${LIVE_ROWS:-8192}"; POOL_POS="${POOL_POS:-2048}"
export HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1
mkdir -p "$SEG"

run() { # run <max_batch> <concurrency list> <label>
  if [ -n "$(lsof -ti :"$PORT" 2>/dev/null || true)" ]; then echo "port $PORT is in use"; exit 1; fi
  "$PY" "$HERE/worker.py" --model "$MODEL" --port "$PORT" --device "$DEVICE" --threads "$THREADS" --max-batch "$1" \
     --max-step-rows "$STEP_ROWS" --max-live-rows "$LIVE_ROWS" --pool-positions "$POOL_POS" > "$SEG/worker-$3.log" 2>&1 &
  local pid=$!
  trap 'kill $pid 2>/dev/null || true' EXIT
  for _ in $(seq 1 300); do curl -sf "http://127.0.0.1:$PORT/health" > /dev/null && break; sleep 1; done
  curl -sf "http://127.0.0.1:$PORT/health" > /dev/null || { echo "worker did not start"; tail -20 "$SEG/worker-$3.log"; kill $pid; exit 1; }
  # warm-up (CUDA kernels, tokenizer)
  "$PY" "$HERE/bench_batch.py" --url "http://127.0.0.1:$PORT" --concurrency 1 --prompt 16 --new 4 --rounds 1 > /dev/null
  "$PY" "$HERE/bench_batch.py" --url "http://127.0.0.1:$PORT" --concurrency "$2" --prompt "$PROMPT" --new "$NEW" --rounds "$ROUNDS" \
     $( [ "${NOSEG:-0}" = 1 ] || echo --segments "$SEG/seg" ) --label "$3" --out "$OUT"
  kill "$pid"; wait "$pid" 2>/dev/null || true
  trap - EXIT
}

[ "${SERIAL:-0}" = 1 ] && run 1 "1,4" "$LABEL-serial"
run "$MAX_BATCH" "$CONC" "$LABEL-batched"
echo "results: $OUT"
