"""Where a batched worker step spends its time (CPU or CUDA), measured on a real model directory.

Runs forward_multi exactly as the batcher does (glue.KVCache per request, A/C captured through a
callable sink) for a few step shapes and prints, per shape, the wall time of one step and, with
--ops, the top torch ops by self time (torch.profiler). Nothing here changes what the worker
computes; it is a measuring tool (docs/measurements/tput-2026-10-23).

  python profile_step.py --model DIR --threads 2 [--ops] [--ctx 300] [--out FILE.jsonl]

Shapes: prefill chunk of N rows at position 0, decode of B rows (B requests each at position ctx),
and a mixed step (B-1 decode rows + a prefill chunk), the step the batcher runs when a new
request arrives while others decode.
"""
import argparse, json, os, sys, time

import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import glue  # noqa: E402
import worker  # noqa: E402


def make_req(m, ctx, need, seed):
    """A request whose cache already holds ctx positions (prefilled through the model)."""
    kv = glue.KVCache(m.det, need)
    g = torch.Generator().manual_seed(seed)
    ids = torch.randint(1000, 30000, (ctx,), generator=g).tolist()
    for i in range(0, ctx, 256):
        m.det.forward_multi([(ids[i:i + 256], kv)], None)
    return kv


def run(m, parts, keep):
    rec = {}

    def cap(index, vs, part):
        if keep:
            rec[index] = (vs, part)
    t0 = time.perf_counter()
    with torch.no_grad():
        m.det.forward_multi(parts, cap)
    return time.perf_counter() - t0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--threads", type=int, default=2)
    ap.add_argument("--ctx", type=int, default=300)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--ops", action="store_true")
    ap.add_argument("--shapes", default="p64,p256,d1,d2,d4,d8,m8")
    ap.add_argument("--label", default="")
    ap.add_argument("--out")
    a = ap.parse_args()
    torch.set_num_threads(a.threads)
    m = worker.Model(a.model)
    rows_out = []
    for shape in a.shapes.split(","):
        kind, n = shape[0], int(shape[1:])
        if kind == "p":
            def mk():
                return [(list(range(1000, 1000 + n)), glue.KVCache(m.det, n + 8))]
        elif kind == "d":
            kvs = [make_req(m, a.ctx, a.ctx + 64, s) for s in range(n)]
            def mk(kvs=kvs):
                for kv in kvs:
                    kv.t = a.ctx
                return [([1234], kv) for kv in kvs]
        else:  # m: n-1 decode rows + a 64-row prefill chunk
            kvs = [make_req(m, a.ctx, a.ctx + 64, s) for s in range(n - 1)]
            def mk(kvs=kvs):
                for kv in kvs:
                    kv.t = a.ctx
                return [([1234], kv) for kv in kvs] + [(list(range(2000, 2064)), glue.KVCache(m.det, 72))]
        run(m, mk(), True)  # warm
        ts = sorted(run(m, mk(), True) for _ in range(a.reps))
        rows = sum(len(i) for i, _ in mk())
        r = {"label": a.label, "shape": shape, "rows": rows, "threads": a.threads, "ctx": a.ctx, "step_s": round(ts[len(ts) // 2], 4),
             "min_s": round(ts[0], 4), "rows_per_s": round(rows / ts[len(ts) // 2], 2), "cpus": os.cpu_count(),
             "loadavg_1m": os.getloadavg()[0]}
        if a.ops:
            from torch.profiler import profile, ProfilerActivity
            parts = mk()
            with profile(activities=[ProfilerActivity.CPU]) as prof:
                run(m, parts, True)
            tot = sum(e.self_cpu_time_total for e in prof.key_averages())
            r["ops"] = [{"op": e.key, "self_ms": round(e.self_cpu_time_total / 1000, 1), "share": round(e.self_cpu_time_total / tot, 3), "calls": e.count}
                        for e in sorted(prof.key_averages(), key=lambda e: -e.self_cpu_time_total)[:14]]
        print(json.dumps(r), flush=True)
        rows_out.append(r)
    if a.out:
        with open(a.out, "a") as f:
            for r in rows_out:
                f.write(json.dumps(r) + "\n")


if __name__ == "__main__":
    main()
