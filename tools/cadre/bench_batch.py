"""Throughput of a running worker under concurrent requests (continuous batching), over HTTP.

  python bench_batch.py --url http://127.0.0.1:10750 --concurrency 1,2,4,8,16 --prompt 64 --new 64 \\
      --rounds 2 --segments /path/scratch --out results.jsonl

For each concurrency C, C clients each send `rounds` streamed /generate requests back to back
(closed loop), with segment files written like the gateway asks (pad false, under --segments,
removed after each request). Every prompt is distinct (an offset into the same text) so no two
requests are the same. Per C it prints one JSON line: summed output tokens/s over the wall time,
per-request latency (total), time to first token (client side: from sending the request to the
first streamed token), per-request decode tokens/s, the worker's own counters, and the machine's
load average, so every figure traces to a run. Nothing is estimated.
"""
import argparse, json, os, shutil, statistics, threading, time, urllib.request


def post_stream(url, body):
    req = urllib.request.Request(url + "/generate", data=json.dumps(body).encode(), headers={"content-type": "application/json"})
    t0 = time.time()
    first = None
    final = None
    with urllib.request.urlopen(req, timeout=7200) as r:
        for line in r:
            if not line.strip():
                continue
            v = json.loads(line)
            if v.get("done"):
                final = v
            elif "error" in v:
                raise RuntimeError(v["error"])
            elif first is None:
                first = time.time() - t0
    return first, time.time() - t0, final


def get(url, path):
    return json.load(urllib.request.urlopen(url + path, timeout=60))


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p * (len(xs) - 1))))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--concurrency", default="1,2,4,8,16")
    ap.add_argument("--prompt", type=int, default=64, help="prompt tokens per request")
    ap.add_argument("--new", type=int, default=64, help="max_new_tokens per request")
    ap.add_argument("--rounds", type=int, default=2, help="requests per client")
    ap.add_argument("--segments", default="", help="write segment files under this dir (removed after each request)")
    ap.add_argument("--label", default="")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    text = "The quick brown fox jumps over the lazy dog while the miners hash the blocks of the chain. " * 64
    base = get(a.url, "/health")
    ids = urllib.request.urlopen(urllib.request.Request(a.url + "/tokenize", data=json.dumps({"text": text}).encode(),
                                                        headers={"content-type": "application/json"}), timeout=60)
    ids = json.load(ids)["ids"]
    for C in [int(x) for x in a.concurrency.split(",")]:
        recs, errs = [], []
        lock = threading.Lock()
        before = get(a.url, "/health").get("batching", {})
        load0 = os.getloadavg()[0]

        def client(ci):
            for k in range(a.rounds):
                off = 1 + (ci * a.rounds + k) * 7  # distinct prompts
                body = {"prompt_ids": ids[:1] + ids[off:off + a.prompt - 1], "max_new_tokens": a.new, "stream": True, "pad": False}
                d = None
                if a.segments:
                    d = os.path.join(a.segments, f"c{C}-{ci}-{k}")
                    body["out_dir"] = d
                try:
                    ttft, total, fin = post_stream(a.url, body)
                    with lock:
                        recs.append({"ttft_s": ttft, "total_s": total, "tokens": fin["completion_tokens"], "rows": fin["rows"], "timing": fin["timing"]})
                except Exception as e:  # noqa: BLE001
                    with lock:
                        errs.append(f"{type(e).__name__}: {e}")
                if d:
                    shutil.rmtree(d, ignore_errors=True)

        t0 = time.time()
        th = [threading.Thread(target=client, args=(i,)) for i in range(C)]
        for t in th:
            t.start()
        for t in th:
            t.join()
        wall = time.time() - t0
        after = get(a.url, "/health").get("batching", {})
        toks = sum(r["tokens"] for r in recs)
        dec = [r["timing"]["decode_tokens_per_s"] for r in recs if r["timing"].get("decode_tokens_per_s")]
        rec = {
            "label": a.label, "concurrency": C, "requests": len(recs), "errors": errs[:3], "prompt_tokens": a.prompt, "max_new": a.new,
            "output_tokens": toks, "wall_s": round(wall, 2), "tokens_per_s_summed": round(toks / wall, 3),
            "latency_s": {"mean": round(statistics.mean(r["total_s"] for r in recs), 2), "p50": round(pct([r["total_s"] for r in recs], 0.5), 2),
                          "max": round(max(r["total_s"] for r in recs), 2)} if recs else None,
            "ttft_s": {"mean": round(statistics.mean(r["ttft_s"] for r in recs), 2), "p50": round(pct([r["ttft_s"] for r in recs], 0.5), 2),
                       "max": round(max(r["ttft_s"] for r in recs), 2)} if recs else None,
            "decode_tokens_per_s_per_request": {"mean": round(statistics.mean(dec), 3), "min": round(min(dec), 3)} if dec else None,
            "batch_mean": round(statistics.mean(r["timing"].get("batch_mean") or 1 for r in recs), 2) if recs else None,
            "worker": {"mode": base.get("batching", {}).get("mode"), "max_batch": base.get("batching", {}).get("max_batch"), "device": base.get("device"),
                       "steps": (after.get("steps") or 0) - (before.get("steps") or 0), "busy_s": round((after.get("busy_s") or 0) - (before.get("busy_s") or 0), 2)},
            "segments": bool(a.segments), "cpus": os.cpu_count(), "loadavg_1m": [round(load0, 2), round(os.getloadavg()[0], 2)],
            "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        print(json.dumps(rec), flush=True)
        if a.out:
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")


if __name__ == "__main__":
    main()
