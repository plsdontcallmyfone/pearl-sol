"""CPU vs GPU check for the int7 worker: are the segments' integer parts bit-identical?

For each fixed prompt it runs greedy generation on the GPU worker and on the CPU worker
(same model directory, same process) and checks:
  1. integer exactness: for every matrix, the GPU's C equals the CPU's int32 product of the
     GPU's own A (torch._int_mm on CPU) bit for bit, and equals an independent numpy int64
     product A B on the first rows (so a segment the GPU serves verifies on any machine);
  2. end to end: whether the CPU and GPU runs produced the same tokens and the same A and C
     (A can differ where the fp32 glue rounds differently; then C differs with it).
Prints one JSON line per prompt and a summary line; --out appends them to a file.

Usage: python gpu_check.py --model DIR [--new 32] [--np-rows 4] [--threads N] [--out FILE.jsonl]
"""
import argparse, json, os, sys, time

import numpy as np
import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import worker  # noqa: E402

PROMPTS = [
    [{"role": "user", "content": "What is the capital of France? Answer in one word."}],
    [{"role": "user", "content": "Name three primary colors, comma separated."}],
    [{"role": "user", "content": "Write two sentences about why the sky is blue."}],
    [{"role": "system", "content": "You are a terse assistant."}, {"role": "user", "content": "List the first ten prime numbers."}],
]


def capture(model, ids, new):
    """Generate and return (output ids, {index: (A int8 numpy, C int32 numpy)})."""
    import tempfile
    d = tempfile.mkdtemp(prefix="cadre-gpucheck-")
    r = model.generate(ids, new, d, pad=False)
    mats = {}
    for m in r["matrices"]:
        A = np.fromfile(m["a"], dtype=np.int8).reshape(m["m"], m["k"])
        C = np.fromfile(m["c"], dtype="<i4").reshape(m["m"], m["n"])
        mats[m["index"]] = (A, C)
    import shutil
    shutil.rmtree(d, ignore_errors=True)
    return r, mats


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--new", type=int, default=32)
    ap.add_argument("--np-rows", type=int, default=4, help="rows per matrix checked with numpy int64 as well")
    ap.add_argument("--threads", type=int, default=0)
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    if a.threads:
        torch.set_num_threads(a.threads)
    gpu = worker.Model(a.model, "cuda")
    cpu = worker.Model(a.model, "cpu")
    cpu.tok = gpu.tokenizer()
    lines = []
    tot = {"matrices_checked": 0, "c_elements_checked": 0, "int_mismatch": 0, "np_mismatch": 0, "prompts": 0, "same_tokens": 0,
           "a_elements": 0, "a_diff": 0, "c_elements": 0, "c_diff": 0}
    for msgs in PROMPTS:
        ids = list(gpu.chat_ids(msgs))
        t0 = time.time()
        rg, mg = capture(gpu, ids, a.new)
        tg = time.time() - t0
        t0 = time.time()
        rc, mc = capture(cpu, ids, a.new)
        tc = time.time() - t0
        int_bad = np_bad = 0
        for i, (A, C) in mg.items():
            m = cpu.mats[i]
            Cc = worker.int_mm(torch.from_numpy(A.copy()), m.qw).numpy()  # CPU product of the GPU's A
            int_bad += int((Cc != C).sum())
            B = m.qw.numpy().astype(np.int64)  # n x k
            rows = min(a.np_rows, A.shape[0])
            ref = A[:rows].astype(np.int64) @ B.T
            np_bad += int((ref != C[:rows].astype(np.int64)).sum())
            tot["c_elements_checked"] += C.size
        same = rg["output_ids"] == rc["output_ids"]
        a_el = a_diff = c_el = c_diff = 0
        if rg["rows"] == rc["rows"]:
            for i in mg:
                a_el += mg[i][0].size
                a_diff += int((mg[i][0] != mc[i][0]).sum())
                c_el += mg[i][1].size
                c_diff += int((mg[i][1] != mc[i][1]).sum())
        rec = {"prompt": msgs[-1]["content"], "prompt_tokens": len(ids), "rows": rg["rows"], "matrices": len(mg),
               "gpu_int_vs_cpu_int_mm_mismatches": int_bad, "gpu_vs_numpy_int64_mismatches": np_bad, "np_rows_per_matrix": a.np_rows,
               "same_tokens_cpu_gpu": same, "gpu_text": rg["text"], "cpu_text": rc["text"],
               "e2e_a_diff_elements": a_diff, "e2e_a_elements": a_el, "e2e_c_diff_elements": c_diff, "e2e_c_elements": c_el,
               "gpu_s": round(tg, 2), "cpu_s": round(tc, 2), "gpu": torch.cuda.get_device_name(0), "torch": torch.__version__,
               "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
        print(json.dumps(rec), flush=True)
        lines.append(rec)
        tot["matrices_checked"] += len(mg)
        tot["int_mismatch"] += int_bad
        tot["np_mismatch"] += np_bad
        tot["prompts"] += 1
        tot["same_tokens"] += int(same)
        tot["a_elements"] += a_el
        tot["a_diff"] += a_diff
        tot["c_elements"] += c_el
        tot["c_diff"] += c_diff
    tot["bit_identical_integer_part"] = tot["int_mismatch"] == 0 and tot["np_mismatch"] == 0
    tot["summary"] = True
    print(json.dumps(tot), flush=True)
    if a.out:
        with open(a.out, "a") as f:
            for x in lines + [tot]:
                f.write(json.dumps(x) + "\n")
    sys.exit(0 if tot["bit_identical_integer_part"] else 1)


if __name__ == "__main__":
    main()
