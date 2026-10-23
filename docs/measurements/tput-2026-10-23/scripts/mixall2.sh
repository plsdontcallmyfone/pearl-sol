#!/bin/bash
until grep -q DONE /home/pearl/tput/mix.jsonl; do sleep 5; done
R=/home/pearl/tput/mixrun.sh; N=/home/pearl/tput/cadre
B0="--max-batch 8 --max-step-rows 64 --max-live-rows 2048 --flush-rows 32 --max-queue 64"
T="--max-batch 8 --max-step-rows 32 --max-live-rows 4096 --flush-rows 32 --max-queue 64"
bash $R new-idle $N "$B0"
bash $R tuned-idle $N "$T"
bash $R tuned-fifo-idle $N "$T --prefill-order fifo"
echo DONE2 >> /home/pearl/tput/mix.jsonl
