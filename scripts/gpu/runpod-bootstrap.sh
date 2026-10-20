#!/usr/bin/env bash
# Bootstrap the pod: clone the PUBLIC repo, install the worker's Python deps, fetch the public
# HF checkpoints, and quantize them with recipe cadre-w7a7-sq-asym-v1 (tools/cadre/quantize.py).
# Nothing from the Mac's ~/.config or ~/.ssh is copied; the pod only gets the public repo.
#   MODELS   space-separated HF repos (default the 1B Instruct the chain serves)
#   BRANCH   repo branch (default main)
source "$(dirname "$0")/runpod-lib.sh"
MODELS="${MODELS:-unsloth/Llama-3.2-1B-Instruct}"
BRANCH="${BRANCH:-main}"
ssh_pod bash -s -- "$BRANCH" "$MODELS" <<'POD'
set -euo pipefail
BRANCH="$1"; MODELS="$2"
cd /root
[ -d pearl-sol ] || git clone -q --depth 1 -b "$BRANCH" https://github.com/plsdontcallmyfone/pearl-sol
cd pearl-sol && git fetch -q --depth 1 origin "$BRANCH" && git reset -q --hard FETCH_HEAD && git log --oneline -1
pip install -q --no-input "torch==2.5.1" --index-url https://download.pytorch.org/whl/cu124 2>&1 | tail -2
pip install -q --no-input "transformers==4.46.3" "datasets==3.1.0" "numpy>=2" safetensors huggingface_hub accelerate 2>&1 | tail -2
python -c "import torch;print('torch', torch.__version__, torch.cuda.get_device_name(0))"
mkdir -p /root/hf /root/cadre-models
for m in $MODELS; do
  d=/root/hf/$(echo "$m" | tr / -)
  [ -f "$d/config.json" ] || python -c "from huggingface_hub import snapshot_download as s; s('$m', local_dir='$d', allow_patterns=['*.json','*.safetensors','tokenizer*'])"
  name=$(basename "$m" | tr 'A-Z' 'a-z')-w7a7
  out=/root/cadre-models/$name-k2048
  if [ ! -f "$out/manifest.json" ]; then
    dev=cpu; [ "$(du -sm "$d" | cut -f1)" -gt 6000 ] && dev=cuda
    ( time python tools/cadre/quantize.py --model "$d" --out "$out" --name "$name" --base-model "$m" --calib "/root/cadre-models/calib-$name.pt" --calib-device $dev ) 2>&1 | tail -4
  fi
  du -sh "$out"
done
POD
