"""Latency of a running worker under a fixed open-loop trace of mixed requests, over HTTP.

The devnet load is a few long reports among many short thoughts (box 0 gateway, 2026-10-23:
median request 236 rows, p90 389, max 1,512). This sends a fixed trace (arrival offset, prompt
tokens, max_new_tokens) of streamed /generate requests with segment files written like the gateway
asks (pad false), and prints one JSON line per request (time to first token, total, decode
tokens/s, the worker's queue/prefill split) and one summary line. Greedy decoding is
deterministic, so two worker versions given the same trace do the same work token for token;
only the timings differ. Nothing is estimated.

  python bench_mix.py --url http://127.0.0.1:10751 --segments /path/scratch --label X --out mix.jsonl
  trace (default): "0:1400:64,1:300:40,6:300:40,11:300:40,16:300:40,21:300:40,26:300:40"
"""
import argparse, json, os, shutil, statistics, threading, time, urllib.request

from bench_batch import get, post_stream

TRACE = "0:1400:64,1:300:40,6:300:40,11:300:40,16:300:40,21:300:40,26:300:40"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--trace", default=TRACE, help="comma list of at_s:prompt_tokens:max_new")
    ap.add_argument("--segments", default="")
    ap.add_argument("--label", default="")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    text = "The quick brown fox jumps over the lazy dog while the miners hash the blocks of the chain. " * 200
    ids = json.load(urllib.request.urlopen(urllib.request.Request(a.url + "/tokenize", data=json.dumps({"text": text}).encode(),
                                                                  headers={"content-type": "application/json"}), timeout=60))["ids"]
    trace = [tuple(int(x) for x in t.split(":")) for t in a.trace.split(",")]
    before = get(a.url, "/health")
    recs, lock = [None] * len(trace), threading.Lock()
    t0 = time.time()

    def client(i, at, plen, new):
        time.sleep(max(0.0, t0 + at - time.time()))
        off = 1 + i * 11
        body = {"prompt_ids": ids[:1] + ids[off:off + plen - 1], "max_new_tokens": new, "stream": True, "pad": False}
        d = os.path.join(a.segments, f"mix-{i}") if a.segments else None
        if d:
            body["out_dir"] = d
        ttft, total, fin = post_stream(a.url, body)
        if d:
            shutil.rmtree(d, ignore_errors=True)
        t = fin["timing"]
        with lock:
            recs[i] = {"label": a.label, "i": i, "at_s": at, "prompt": plen, "max_new": new, "tokens": fin["completion_tokens"], "rows": fin["rows"],
                       "ttft_s": round(ttft, 2), "total_s": round(total, 2), "queue_s": t.get("queue_s"), "prefill_s": t.get("prefill_s"),
                       "decode_tokens_per_s": t.get("decode_tokens_per_s"), "out_ids_head": fin["output_ids"][:4]}
    th = [threading.Thread(target=client, args=(i, *t)) for i, t in enumerate(trace)]
    for t in th:
        t.start()
    for t in th:
        t.join()
    wall = time.time() - t0
    after = get(a.url, "/health")
    short = [r for r in recs if r["prompt"] < 1000]
    summ = {"label": a.label, "summary": True, "requests": len(recs), "wall_s": round(wall, 2), "rows": sum(r["rows"] for r in recs),
            "rows_per_s": round(sum(r["rows"] for r in recs) / wall, 2),
            "short_ttft_mean_s": round(statistics.mean(r["ttft_s"] for r in short), 2) if short else None,
            "short_ttft_max_s": round(max(r["ttft_s"] for r in short), 2) if short else None,
            "short_decode_tps_mean": round(statistics.mean(r["decode_tokens_per_s"] or 0 for r in short), 3) if short else None,
            "long_ttft_s": [r["ttft_s"] for r in recs if r["prompt"] >= 1000], "long_total_s": [r["total_s"] for r in recs if r["prompt"] >= 1000],
            "worker": {k: after.get("batching", {}).get(k) for k in ("max_batch", "max_step_rows", "max_live_rows", "prefill_order", "threads")},
            "worker_busy_s": round(after["batching"]["busy_s"] - before["batching"]["busy_s"], 2), "cpus": os.cpu_count(),
            "loadavg_1m": round(os.getloadavg()[0], 2), "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    for r in recs + [summ]:
        print(json.dumps(r), flush=True)
    if a.out:
        with open(a.out, "a") as f:
            for r in recs + [summ]:
                f.write(json.dumps(r) + "\n")


if __name__ == "__main__":
    main()
