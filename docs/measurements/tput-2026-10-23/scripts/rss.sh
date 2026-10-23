#!/bin/bash
export HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1
M=/home/pearl/cadre-models/llama-3.2-1b-instruct-w7a7-k2048; P=/home/pearl/int7-exp/venv/bin/python; PORT=11870
one() { # label code args
  for i in $(seq 30); do lsof -ti :$PORT >/dev/null || break; sleep 1; done
  cd $2; taskset -c 0-3 $P worker.py --model $M --port $PORT --threads 2 $3 > /dev/null 2>&1 & W=$!
  until curl -sf http://127.0.0.1:$PORT/health >/dev/null; do sleep 1; done
  mkdir -p /home/pearl/tput/seg
  $P /home/pearl/tput/cadre/bench_mix.py --url http://127.0.0.1:$PORT --segments /home/pearl/tput/seg --label rss-$1 --trace "0:1400:64,1:300:40,2:300:40,3:300:40,4:300:40,5:300:40,6:300:40,7:300:40" > /dev/null
  echo "$1 $(grep VmHWM /proc/$W/status)" >> /home/pearl/tput/rss.txt
  kill $W; wait $W 2>/dev/null; rm -rf /home/pearl/tput/seg
}
one old /home/pearl/tput/orig "--max-batch 8 --max-step-rows 64 --max-live-rows 2048 --flush-rows 32"
one tuned /home/pearl/tput/cadre "--max-batch 8 --max-step-rows 32 --max-live-rows 4096 --flush-rows 32"
echo DONE >> /home/pearl/tput/rss.txt
