"""Measure the int7 worker on this machine: time to first token and decode tokens/s.

Usage: python bench.py --model DIR [--prompts 8,64,256] [--new 32] [--threads N] [--out FILE.jsonl]
Runs greedy generation in process (no HTTP), one line of JSON per prompt length, plus the
machine's load average and CPU count, so every figure in the docs traces to a run.
"""
import argparse, json, os, time

import torch

import worker


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--prompts", default="8,64,256")
    ap.add_argument("--new", type=int, default=32)
    ap.add_argument("--threads", type=int, default=0)
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    if a.threads:
        torch.set_num_threads(a.threads)
    t0 = time.time()
    m = worker.Model(a.model)
    load_s = time.time() - t0
    base = m.tokenizer()("The quick brown fox jumps over the lazy dog. " * 64).input_ids
    for p in [int(x) for x in a.prompts.split(",")]:
        ids = base[:p]
        r = m.generate(ids, a.new, None)
        rec = {
            "model": m.man["name"], "prompt_tokens": len(ids), "completion_tokens": r["completion_tokens"],
            "ttft_s": r["timing"]["prefill_s"], "decode_s": r["timing"]["decode_s"],
            "decode_tokens_per_s": r["timing"]["decode_tokens_per_s"], "rows": r["rows"],
            "mined_macs": r["timing"]["mined_macs"], "threads": torch.get_num_threads(), "cpus": os.cpu_count(),
            "loadavg_1m": os.getloadavg()[0], "load_s": round(load_s, 1), "torch": torch.__version__,
        }
        print(json.dumps(rec), flush=True)
        if a.out:
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")


if __name__ == "__main__":
    main()
