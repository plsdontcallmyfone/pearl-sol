#!/bin/bash
R=/home/pearl/tput/mixrun.sh; O=/home/pearl/tput/orig; N=/home/pearl/tput/cadre
B0="--max-batch 8 --max-step-rows 64 --max-live-rows 2048 --flush-rows 32 --max-queue 64"
T="--max-batch 8 --max-step-rows 32 --max-live-rows 4096 --flush-rows 32 --max-queue 64"
bash $R old-idle $O "$B0"
bash $R new-idle $N "$B0"
bash $R tuned-idle $N "$T"
bash $R tuned-burn-passive $N "$T" burn OMP_WAIT_POLICY=PASSIVE
bash $R tuned-niced-passive $N "$T" niced OMP_WAIT_POLICY=PASSIVE
bash $R old-burn $O "$B0" burn
echo DONE >> /home/pearl/tput/mix.jsonl
