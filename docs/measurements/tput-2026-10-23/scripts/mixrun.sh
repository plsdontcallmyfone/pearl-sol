#!/bin/bash
# bench_mix against worker configs on CPUs 0-3, 2 threads (box 0 has 4 vCPU of the same Xeon, WORKER_THREADS=2)
# usage: mixrun.sh LABEL CODE_DIR "worker args" [burn]
export HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1
M=/home/pearl/cadre-models/llama-3.2-1b-instruct-w7a7-k2048
P=/home/pearl/int7-exp/venv/bin/python
PORT=11870
LABEL=$1; CODE=$2; ARGS=$3; BURN=$4; ENVV=${5:-X=1}
OUT=/home/pearl/tput/mix.jsonl
for i in $(seq 30); do lsof -ti :$PORT >/dev/null || break; sleep 1; done
if lsof -ti :$PORT >/dev/null; then echo "port $PORT busy"; exit 1; fi
cleanup() { [ -n "$W" ] && kill $W 2>/dev/null && wait $W 2>/dev/null; for p in $(cat /home/pearl/tput/burn.pids 2>/dev/null); do kill $p 2>/dev/null; done; rm -f /home/pearl/tput/burn.pids; rm -rf /home/pearl/tput/seg; }
trap cleanup EXIT
cd $CODE
taskset -c 0-3 env $ENVV $P worker.py --model $M --port $PORT --threads 2 $ARGS > /home/pearl/tput/worker-$LABEL.log 2>&1 & W=$!
echo $W > /home/pearl/tput/worker.pid
until curl -sf http://127.0.0.1:$PORT/health >/dev/null; do sleep 1; kill -0 $W || exit 1; done
if [ -n "$BURN" ]; then
  N0=0; N1=10; [ "$BURN" = niced ] && N0=10 && N1=15
  for i in 1 2 3; do taskset -c 0-3 nice -n $N0 python3 -c "while 1: pass" & echo $! >> /home/pearl/tput/burn.pids; done
  for i in 1 2; do taskset -c 0-3 nice -n $N1 python3 -c "while 1: pass" & echo $! >> /home/pearl/tput/burn.pids; done
fi
mkdir -p /home/pearl/tput/seg
$P /home/pearl/tput/cadre/bench_mix.py --url http://127.0.0.1:$PORT --segments /home/pearl/tput/seg --label $LABEL --out $OUT | tail -1
