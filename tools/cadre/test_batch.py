"""Continuous batching is bit-identical to serving each request alone (SPEC 4.7 "Batching").

  python test_batch.py                                  tiny synthetic Llama (quantize.py on a random 1-layer model)
  python test_batch.py --model DIR [--replay BIN]       a real registered model; with --replay, the evaluator's
                                                        `cadre-replay local` must accept every batched request

A mixed batch: prompts of different lengths (one longer than a prefill chunk, so its prompt is
split over steps), different max_new_tokens, a stop string, pad on and off, more requests than
slots (so some wait and join while others decode), and late joiners submitted mid-decode. For
every request the batched run must equal Model.generate of that request alone in: output ids,
text, rows, finish reason, every A file and every C file (byte for byte), and the binary64
logits row behind every generated token. Prints one JSON line per check; exits 1 on a failure.
"""
import argparse, hashlib, json, os, shutil, subprocess, sys, tempfile, time

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import batch  # noqa: E402
import worker  # noqa: E402

FAIL = []


def emit(check, ok, **kw):
    print(json.dumps({"check": check, "ok": bool(ok), **kw}), flush=True)
    if not ok:
        FAIL.append(check)


def digest(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest() if os.path.exists(path) else None


def tiny_model(tmp):
    import test_cadre
    hf, out = os.path.join(tmp, "hf"), os.path.join(tmp, "model")
    test_cadre.tiny_checkpoint(hf)
    r = subprocess.run([sys.executable, os.path.join(HERE, "quantize.py"), "--model", hf, "--out", out, "--name", "tiny", "--calib", "none"], capture_output=True, text=True)
    assert r.returncode == 0, r.stderr[-2000:]
    return out


def requests(model, real):
    """(name, prompt ids, max_new, stop, pad, delay_s): delay = submitted that long after the first batch."""
    tok = model.tokenizer()
    if real:
        msgs = [
            ("france", "What is the capital of France? Answer in one word.", 8, [], False),
            ("colors", "Name three primary colors, comma separated.", 16, [], False),
            ("sea", "Write one sentence about the sea.", 24, ["."], False),
            ("count", "Count from 1 to 20 separated by spaces.", 12, [], True),
            ("long", "Summarize this text in one line: " + "The quick brown fox jumps over the lazy dog. " * 12, 10, [], False),
            ("haiku", "Write a haiku about mining blocks.", 20, [], False),
            ("math", "What is 17 times 23? Show the steps briefly.", 18, [], True),
            ("late1", "Give one word that rhymes with cat.", 6, [], False),
            ("late2", "List the days of the week.", 14, [","], False),
        ]
        out = []
        for i, (name, text, n, stop, pad) in enumerate(msgs):
            ids = list(model.chat_ids([{"role": "user", "content": text}]))
            out.append((name, ids, n, stop, pad, 0.0 if not name.startswith("late") else 1.0))
        return out
    base = list(range(1, 400))
    return [
        ("p5", base[:5], 6, [], False, 0.0),
        ("p17", base[3:20], 9, [], True, 0.0),
        ("p70_chunked", base[10:80], 5, [], False, 0.0),   # longer than max_step_rows: prefill over several steps
        ("p1", base[7:8], 12, [], False, 0.0),
        ("p33_stop", base[40:73], 10, [tok.decode([base[2]])], False, 0.0),
        ("late_p9", base[100:109], 7, [], False, 0.3),
        ("late_p40", base[200:240], 4, [], True, 0.3),
    ]


def solo(model, reqs, work):
    """Each request alone through Model.generate, recording every token's logits row."""
    res = {}
    orig = model.pick
    for name, ids, n, stop, pad, _ in reqs:
        rows = []

        def rec(row):
            rows.append(row.cpu().numpy().tobytes())
            return orig(row)

        model.pick = rec
        t = time.time()
        r = model.generate(ids, n, os.path.join(work, "solo", name), stop=stop, pad=pad)
        model.pick = orig
        res[name] = (r, rows, time.time() - t)
    return res


def batched(model, reqs, work, max_batch, max_step_rows, kv_pool):
    b = model.start_batching(max_batch=max_batch, max_queue=64, max_step_rows=max_step_rows, max_live_rows=1 << 20, flush_rows=7, kv_pool=kv_pool, pool_positions=256)
    b.logits_log = {}
    jobs = {}
    t0 = time.time()
    for name, ids, n, stop, pad, delay in sorted(reqs, key=lambda r: r[5]):
        while time.time() - t0 < delay:
            time.sleep(0.01)
        jobs[name] = b.submit(ids, n, os.path.join(work, "batch", name), stop=stop, pad=pad)
    res = {name: batch.wait(model, j, timeout=3600) for name, j in jobs.items()}
    logs = {name: b.logits_log.get(id(j), []) for name, j in jobs.items()}
    return res, logs, time.time() - t0, b.health()


def compare(model, reqs, s, bres, blogs, work):
    for name, ids, n, stop, pad, _ in reqs:
        r1, lg1, _ = s[name]
        r2 = bres[name]
        same = {k: r1[k] == r2[k] for k in ("output_ids", "text", "rows", "rows_padded", "finish_reason", "completion_tokens")}
        files_bad = []
        for m in model.mats:
            for kind, ext in (("a", "i8"), ("c", "i32")):
                p1 = os.path.join(work, "solo", name, kind, f"{m.index}.{ext}")
                p2 = os.path.join(work, "batch", name, kind, f"{m.index}.{ext}")
                if digest(p1) is None or digest(p1) != digest(p2):
                    files_bad.append(f"{kind}/{m.index}")
        logits_same = lg1 == blogs[name]
        emit("batched_eq_solo", all(same.values()) and not files_bad and logits_same, request=name, prompt=len(ids), max_new=n, rows=r2["rows"],
             tokens=len(r2["output_ids"]), fields={k: v for k, v in same.items() if not v}, files_differ=files_bad[:8], n_files=2 * len(model.mats),
             logits_rows=len(lg1), logits_same=logits_same, batch_mean=r2["timing"]["batch_mean"], queue_s=r2["timing"]["queue_s"], text=r2["text"][:60])


def replay_all(binary, model_dir, reqs, bres, work):
    for name, ids, n, stop, pad, _ in reqs:
        r = bres[name]
        run = os.path.join(work, "batch", name)
        if r["rows_padded"] != r["rows"]:
            continue  # cadre-replay reads unpadded runs (the gateway always asks for pad false)
        req = {"request_id": name, "row_offset": 0, "prompt_ids": ids, "output_ids": r["output_ids"], "max_new_tokens": n, "stop": stop,
               "output_sha256": hashlib.sha256(json.dumps(r["output_ids"], separators=(",", ":")).encode()).hexdigest(),
               "text_sha256": hashlib.sha256(r["text"].encode()).hexdigest(),
               "prompt_sha256": hashlib.sha256(json.dumps(ids, separators=(",", ":")).encode()).hexdigest()}
        json.dump(req, open(os.path.join(run, "request.json"), "w"))
        t = time.time()
        p = subprocess.run([binary, "local", "--model", model_dir, "--run", run], capture_output=True, text=True)
        try:
            v = json.loads(p.stdout)
        except Exception:  # noqa: BLE001
            v = {"raw": p.stdout[-400:], "stderr": p.stderr[-400:]}
        emit("evaluator_replay_accepts_batched", v.get("verdict") == "honest" and v.get("text") == r["text"], request=name, verdict=v.get("verdict"),
             reason=v.get("reason"), rows=r["rows"], replay_s=round(time.time() - t, 2))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="")
    ap.add_argument("--replay", default="")
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--work", default="")
    ap.add_argument("--max-batch", type=int, default=4)
    ap.add_argument("--max-step-rows", type=int, default=32)
    ap.add_argument("--kv-pool", default="both", help="on, off or both (two batched runs, both checked against the solo runs)")
    a = ap.parse_args()
    tmp = tempfile.mkdtemp(prefix="cadre-batch-test-")
    work = a.work or tmp
    shutil.rmtree(os.path.join(work, "solo"), ignore_errors=True)
    shutil.rmtree(os.path.join(work, "batch"), ignore_errors=True)
    mdir = a.model or tiny_model(tmp)
    model = worker.Model(mdir, a.device)
    reqs = requests(model, bool(a.model))
    s = solo(model, reqs, work)
    for pool in ([False, True] if a.kv_pool == "both" else [a.kv_pool == "on"]):
        shutil.rmtree(os.path.join(work, "batch"), ignore_errors=True)
        bres, blogs, wall, h = batched(model, reqs, work, a.max_batch, a.max_step_rows, pool)
        emit("batcher_ran_mixed_batch", h["max_active"] == min(a.max_batch, len(reqs)) and h["served"] == len(reqs) and h["kv_pool"] == pool, kv_pool=pool, health=h,
             wall_s=round(wall, 2), solo_sum_s=round(sum(x[2] for x in s.values()), 2))
        compare(model, reqs, s, bres, blogs, work)
    if a.replay:
        replay_all(a.replay, mdir, reqs, bres, work)
    # backpressure: a full queue answers Busy at once
    b = model.batcher
    b.max_queue = 0
    try:
        b.submit(reqs[0][1], 2)
        emit("full_queue_is_busy", False)
    except batch.Busy as e:
        emit("full_queue_is_busy", True, error=str(e))
    print(json.dumps({"summary": "fail" if FAIL else "pass", "failed": FAIL}))
    if not a.work:
        shutil.rmtree(tmp, ignore_errors=True)
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
