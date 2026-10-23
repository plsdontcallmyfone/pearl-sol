"""Golden digests of what the worker computes, to compare two versions of the worker code.

A speed change to the worker must not change one bit of what it produces. This file serves a
fixed set of requests (chat prompts and random id prompts of several lengths, one request alone
and all of them batched together, prefill chunks that split prompts) and records per request:
output ids, rows, finish reason, sha256 of every A and C file, and sha256 of every binary64
logits row behind a token (batched mode). Run it with the old code, then with the new code, and
compare the two JSON files: they must be equal.

  python golden.py --model DIR --out old.json            (in a checkout of the old code)
  python golden.py --model DIR --out new.json --compare old.json
"""
import argparse, hashlib, json, os, random, shutil, sys, tempfile

import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import batch  # noqa: E402
import worker  # noqa: E402

CHATS = [
    ([{"role": "user", "content": "What is the capital of France? Answer in one word."}], 8, []),
    ([{"role": "user", "content": "Write one sentence about the sea."}], 24, ["."]),
    ([{"role": "system", "content": "You are a careful trading agent. Answer TAKE or SKIP with one reason."},
      {"role": "user", "content": "Pool SOL/XYZ: liquidity 120 SOL, 15 min volume 40 SOL, top10 share 0.31, price up 12% in 5 min. TAKE or SKIP?"}], 20, []),
]


def files_digest(out_dir, mats):
    h = {}
    for kind in ("a", "c"):
        for m in mats:
            p = os.path.join(out_dir, kind, f"{m.index}.{'i8' if kind == 'a' else 'i32'}")
            h[f"{kind}{m.index}"] = hashlib.sha256(open(p, "rb").read()).hexdigest()
    return hashlib.sha256(json.dumps(h, sort_keys=True).encode()).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--compare")
    ap.add_argument("--threads", type=int, default=2)
    ap.add_argument("--work", default=None)
    a = ap.parse_args()
    torch.set_num_threads(a.threads)
    m = worker.Model(a.model)
    tok = m.tokenizer()
    rng = random.Random(7)
    reqs = [(list(m.chat_ids(msgs)), n, stop) for msgs, n, stop in CHATS]
    for plen, n in ((1, 6), (5, 4), (63, 3), (130, 5), (300, 2)):
        reqs.append(([rng.randrange(1000, 30000) for _ in range(plen)], n, []))
    work = a.work or tempfile.mkdtemp(prefix="golden-")
    res = {"solo": [], "batched": []}
    for i, (ids, n, stop) in enumerate(reqs):
        d = os.path.join(work, f"solo{i}")
        r = m.generate(ids, n, d, stop=stop, pad=(i % 2 == 0))
        res["solo"].append({"ids": r["output_ids"], "rows": r["rows"], "rows_padded": r["rows_padded"], "finish": r["finish_reason"],
                            "files": files_digest(d, m.mats)})
        shutil.rmtree(d)
    # batched: every request at once, 4 slots, prefill chunks of 48 rows (prompts split, joiners)
    b = m.start_batching(max_batch=4, max_queue=64, max_step_rows=48, max_live_rows=4096, flush_rows=40)
    b.logits_log = {}
    jobs = []
    for i, (ids, n, stop) in enumerate(reqs):
        jobs.append(b.submit(ids, n, os.path.join(work, f"b{i}"), stop=stop, pad=(i % 2 == 0)))
    for i, j in enumerate(jobs):
        r = batch.wait(m, j, timeout=3600)
        res["batched"].append({"ids": r["output_ids"], "rows": r["rows"], "rows_padded": r["rows_padded"], "finish": r["finish_reason"],
                               "files": files_digest(os.path.join(work, f"b{i}"), m.mats),
                               "logits": hashlib.sha256(b"".join(b.logits_log.get(id(j), []))).hexdigest()})
    res["texts"] = [tok.decode(x["ids"], skip_special_tokens=True) for x in res["solo"]]
    same = all(s["files"] == q["files"] and s["ids"] == q["ids"] for s, q in zip(res["solo"], res["batched"]))
    res["solo_eq_batched"] = same
    json.dump(res, open(a.out, "w"), indent=1)
    shutil.rmtree(work, ignore_errors=True)
    print(json.dumps({"solo_eq_batched": same, "requests": len(reqs), "texts": res["texts"][:3]}))
    if a.compare:
        old = json.load(open(a.compare))
        diff = [k for k in ("solo", "batched") if old[k] != res[k]]
        print(json.dumps({"check": "golden_equal", "ok": not diff, "differs": diff}))
        sys.exit(1 if diff or not same else 0)
    sys.exit(0 if same else 1)


if __name__ == "__main__":
    main()
