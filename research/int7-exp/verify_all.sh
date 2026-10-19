#!/usr/bin/env bash
cd /home/pearl/int7-exp
export HF_HOME=$PWD/hf HF_HUB_OFFLINE=1 HF_DATASETS_OFFLINE=1
Q=$(ls -d hf/hub/models--Qwen--Qwen2.5-0.5B/snapshots/*); L=$(ls -d hf/hub/models--unsloth--Llama-3.2-1B/snapshots/*)
rm -f verify.jsonl
for spec in "$Q 0 mlp.down_proj" "$Q 12 mlp.down_proj" "$Q 23 mlp.down_proj" "$Q 5 mlp.up_proj" "$L 0 mlp.down_proj" "$L 8 self_attn.q_proj" "$L 15 mlp.up_proj" "$L 15 self_attn.o_proj"; do
  set -- $spec
  nice -n 10 venv/bin/python verify_pearljob.py --model $1 --layer $2 --proj $3 --m 64 --mine-nonces 4 --out vdata/v-$2-$3-$(echo $1 | cut -c18-22) 2>&1 | grep -E "^\{|Error" >> verify.jsonl
done
echo VERIFY_DONE >> verify.jsonl
