"""CPU vs GPU under cadre-glue-v1 (SPEC 4.7), on one machine with a CUDA GPU.

  python gpu_replay_check.py --model DIR --replay PATH/TO/cadre-replay [--work DIR]

For each prompt the worker serves it on cuda and on cpu with the same model files; every
matrix's A and C and the output tokens must be bit-identical, and the Rust replay (cadre-replay,
CPU) must accept the GPU-served request. The same is run with the old fp32 glue to show the
difference it removes. Decode speed of both glues on both devices is reported. One JSON line per
result; exit 1 on any mismatch.
"""
import argparse, hashlib, json, os, shutil, subprocess, sys, time

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import worker  # noqa: E402
from test_replay import PROMPTS, serve  # noqa: E402

FAIL = []


def emit(check, ok, **kw):
    print(json.dumps({"check": check, "ok": bool(ok), **kw}), flush=True)
    if not ok:
        FAIL.append(check)


def compare(man, a_dir, b_dir, rows):
    diff_a = diff_c = 0
    first = None
    for m in man["matrices"]:
        i, k, n = m["index"], m["k"], m["n"]
        a1 = np.fromfile(os.path.join(a_dir, f"a/{i}.i8"), dtype=np.int8)[: rows * k]
        a2 = np.fromfile(os.path.join(b_dir, f"a/{i}.i8"), dtype=np.int8)[: rows * k]
        c1 = np.fromfile(os.path.join(a_dir, f"c/{i}.i32"), dtype="<i4")[: rows * n]
        c2 = np.fromfile(os.path.join(b_dir, f"c/{i}.i32"), dtype="<i4")[: rows * n]
        if a1.shape != a2.shape or not np.array_equal(a1, a2):
            diff_a += 1
            if first is None:
                first = (i, m["name"], int((a1 != a2).sum()) if a1.shape == a2.shape else -1)
        if c1.shape != c2.shape or not np.array_equal(c1, c2):
            diff_c += 1
    return diff_a, diff_c, first


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--replay", required=True)
    ap.add_argument("--work", default="/root/gpu-replay")
    a = ap.parse_args()
    shutil.rmtree(a.work, ignore_errors=True)
    os.makedirs(a.work)
    gpu_name = torch.cuda.get_device_name(0)
    cpu_name = open("/proc/cpuinfo").read().split("model name")[1].split("\n")[0].strip(" :\t")
    print(json.dumps({"gpu": gpu_name, "cpu": cpu_name, "torch": torch.__version__, "threads": torch.get_num_threads(), "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}), flush=True)
    for glue in ("cadre-glue-v1", "fp32"):
        models = {dev: worker.Model(a.model, dev, glue) for dev in ("cuda", "cpu")}
        for i, (msgs, n, stop) in enumerate(PROMPTS):
            runs = {}
            for dev, model in models.items():
                run = os.path.join(a.work, f"{glue}-{dev}-req{i}")
                r, req = serve(model, msgs, n, stop, run)
                runs[dev] = (run, r, req)
            (gr, g, greq), (cr, c, creq) = runs["cuda"], runs["cpu"]
            same_tokens = g["output_ids"] == c["output_ids"]
            rows = min(g["rows"], c["rows"])
            da, dc, first = compare(models["cpu"].man, gr, cr, rows)
            rec = dict(glue=glue, request=i, tokens_equal=same_tokens, matrices_a_differ=da, matrices_c_differ=dc, first_a_diff=first,
                       gpu_text=g["text"], cpu_text=c["text"], gpu_decode_tok_s=g["timing"]["decode_tokens_per_s"], cpu_decode_tok_s=c["timing"]["decode_tokens_per_s"],
                       gpu_prefill_s=g["timing"]["prefill_s"], cpu_prefill_s=c["timing"]["prefill_s"], rows=g["rows"])
            if glue == "cadre-glue-v1":
                v = subprocess.run([a.replay, "local", "--model", a.model, "--run", gr], capture_output=True, text=True)
                out = json.loads(v.stdout) if v.stdout.startswith("{") else {"raw": v.stdout[-500:], "err": v.stderr[-500:]}
                rec["rust_replay_of_gpu_run"] = out.get("verdict")
                rec["rust_replay_timing"] = out.get("timing")
                emit("cpu_gpu_bit_identical", same_tokens and da == 0 and dc == 0 and out.get("verdict") == "honest", **rec)
            else:
                emit("fp32_glue_reference", True, **rec)
        del models
        torch.cuda.empty_cache()
    print(json.dumps({"summary": "fail" if FAIL else "pass", "failed": FAIL}), flush=True)
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
