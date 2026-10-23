#!/bin/bash
# contention experiment: worker steps on CPUs 0-3 with burners standing in for attester/node (nice 0) and miner (nice 10)
cd /home/pearl/tput/cadre
export HF_HUB_OFFLINE=1
M=/home/pearl/cadre-models/llama-3.2-1b-instruct-w7a7-k2048
P=/home/pearl/int7-exp/venv/bin/python
OUT=/home/pearl/tput/prof-contend.jsonl
burn() { # $1 = count, $2 = nice
  for i in $(seq $1); do taskset -c 0-3 nice -n $2 python3 -c "while 1: pass" & echo $! >> /home/pearl/tput/burn.pids; done
}
stopburn() { for p in $(cat /home/pearl/tput/burn.pids 2>/dev/null); do kill $p 2>/dev/null; done; rm -f /home/pearl/tput/burn.pids; }
trap stopburn EXIT
run() { taskset -c 0-3 env $2 $P profile_step.py --model $M --threads 2 --reps 3 --ctx 300 --label "$1" --shapes p32,d1,d4 --out $OUT > /dev/null 2>&1; }
run "4cpu-idle" "X=1"
burn 3 0; burn 2 10
run "4cpu-burn" "X=1"
run "4cpu-burn-passive" "OMP_WAIT_POLICY=PASSIVE"
stopburn
burn 3 10; burn 2 15
run "4cpu-burn-niced" "X=1"
stopburn
echo DONE >> $OUT
