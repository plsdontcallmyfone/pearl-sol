"""Replay tests for cadre-glue-v1 on a real model (SPEC 4.7), run on a box with the model.

  python test_replay.py --model DIR --replay PATH/TO/cadre-replay [--work DIR] [--device cpu|cuda]

1. tables: Python and Rust print the same integer-table digest;
2. decode: the evaluator's Rust decode equals the worker's HF decode on real outputs and on
   random id sequences;
3. honest: the worker (this file's process) serves prompts; the Rust replay rebuilds every A of
   every segment bit for bit, checks C, tokens, stop rule and text -> "honest";
4. cheater: one segment's A altered by one step and its C recomputed (a correct GEMM over the
   wrong activations) -> "fraud", the exact matrix and element named; the fraud proof verifies
   against the altered A and fails against the honest A;
5. tokens: a changed output token -> "fraud" (kind tokens).
Prints one JSON line per check; exits 1 on any failure.
"""
import argparse, hashlib, json, os, random, shutil, subprocess, sys, time

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import glue  # noqa: E402
import worker  # noqa: E402

PROMPTS = [
    ([{"role": "user", "content": "What is the capital of France? Answer in one word."}], 8, []),
    ([{"role": "user", "content": "Name three primary colors, comma separated."}], 16, []),
    ([{"role": "user", "content": "Write one sentence about the sea."}], 24, ["."]),
    ([{"role": "user", "content": "Count from 1 to 20 separated by spaces."}], 12, []),
]

FAIL = []


def emit(check, ok, **kw):
    print(json.dumps({"check": check, "ok": bool(ok), **kw}), flush=True)
    if not ok:
        FAIL.append(check)


def replay(binary, *args):
    t = time.time()
    r = subprocess.run([binary, *args], capture_output=True, text=True)
    try:
        out = json.loads(r.stdout)
    except Exception:  # noqa: BLE001
        out = {"raw": r.stdout[-2000:], "stderr": r.stderr[-2000:]}
    out["_exit"] = r.returncode
    out["_wall_s"] = round(time.time() - t, 2)
    return out


def serve(model, msgs, max_new, stop, run):
    ids = model.chat_ids(msgs)
    r = model.generate(ids, max_new, out_dir=run, stop=stop, pad=False)
    req = {"request_id": os.path.basename(run), "row_offset": 0, "prompt_ids": list(ids), "output_ids": r["output_ids"], "max_new_tokens": max_new, "stop": stop,
           "output_sha256": hashlib.sha256(json.dumps(r["output_ids"], separators=(",", ":")).encode()).hexdigest(),
           "text_sha256": hashlib.sha256(r["text"].encode()).hexdigest(),
           "prompt_sha256": hashlib.sha256(json.dumps(list(ids), separators=(",", ":")).encode()).hexdigest()}
    json.dump(req, open(os.path.join(run, "request.json"), "w"))
    return r, req


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--replay", required=True)
    ap.add_argument("--work", default="/tmp/cadre-replay-test")
    ap.add_argument("--device", default="cpu")
    a = ap.parse_args()
    shutil.rmtree(a.work, ignore_errors=True)
    os.makedirs(a.work)
    model = worker.Model(a.model, a.device)
    tok = model.tokenizer()

    # 1. tables
    py = glue.table_digest(model.inv_freq.cpu().numpy(), model.hd, 4096)
    rs = replay(a.replay, "tables", "--model", a.model, "--positions", "4096")
    emit("tables_python_eq_rust", py == rs.get("digest"), python=py, rust=rs.get("digest"))

    # 2. decode on random sequences (special ids included)
    rng = random.Random(7)
    bad = 0
    for _ in range(60):
        ids = [rng.randrange(0, model.a["vocab"]) for _ in range(rng.randrange(1, 40))]
        if rng.random() < 0.3:
            ids.insert(rng.randrange(0, len(ids)), 128009)
        want = tok.decode(ids, skip_special_tokens=True)
        got = replay(a.replay, "decode", "--model", a.model, "--ids", json.dumps(ids)).get("text")
        bad += int(want != got)
    emit("decode_rust_eq_hf_random", bad == 0, sequences=60, differing=bad)

    # 3. honest runs
    runs = []
    for i, (msgs, n, stop) in enumerate(PROMPTS):
        run = os.path.join(a.work, f"req{i}")
        r, req = serve(model, msgs, n, stop, run)
        v = replay(a.replay, "local", "--model", a.model, "--run", run)
        # the final residual stream, all rows at once (bit-identical to the served incremental pass)
        with torch.no_grad():
            model.forward(req["prompt_ids"] + req["output_ids"][:-1], model.new_cache(), None)
        h_py = hashlib.sha256(model.det.last_h.cpu().contiguous().numpy().astype("<i8").tobytes()).hexdigest()
        emit("honest_replay", v.get("verdict") == "honest" and v.get("text") == r["text"] and v.get("h_sha256") == h_py, h_python=h_py, h_rust=v.get("h_sha256"), request=i, rows=r["rows"], text=r["text"], tokens=len(r["output_ids"]),
             serve_timing=r["timing"], replay=v.get("timing"), verdict=v.get("verdict"), reason=v.get("reason"))
        runs.append((run, r, req))

    # 4. cheater: correct GEMM over altered activations
    run, r, req = runs[1]
    for target in (0, 37, len(model.mats) - 1):  # layers.0.qkv, a layer-4 matrix, the last lm_head shard
        m = model.mats[target]
        bad_run = run + f"-cheat{target}"
        shutil.copytree(run, bad_run)
        A = np.fromfile(os.path.join(bad_run, f"a/{target}.i8"), dtype=np.int8).reshape(-1, m.k).copy()
        row, col = A.shape[0] // 2, 100
        A[row, col] = A[row, col] + 1 if A[row, col] < 64 else A[row, col] - 1
        A.tofile(os.path.join(bad_run, f"a/{target}.i8"))
        W = m.qw.cpu().numpy().astype(np.int64)  # n x k
        C = (A.astype(np.int64) @ W.T).astype("<i4")
        C.tofile(os.path.join(bad_run, f"c/{target}.i32"))  # the GEMM itself is correct
        proof = os.path.join(bad_run, "proof.json")
        v = replay(a.replay, "local", "--model", a.model, "--run", bad_run, "--proof-out", proof)
        mx = v.get("mismatch") or {}
        emit("cheater_caught", v.get("verdict") == "fraud" and v.get("matrix") == target and mx.get("row") == row and mx.get("col") == col,
             matrix=target, name=m.name, row=row, col=col, verdict=v.get("verdict"), named=v.get("matrix_name"), reason=v.get("reason"), replay=v.get("timing"))
        pv = replay(a.replay, "verify", "--model", a.model, "--proof", proof, "--a-file", os.path.join(bad_run, f"a/{target}.i8"), "--m", str(A.shape[0]))
        emit("fraud_proof_valid", pv.get("proof") == "valid", matrix=target, verify_s=pv.get("verify_s"))
        pv2 = replay(a.replay, "verify", "--model", a.model, "--proof", proof, "--a-file", os.path.join(run, f"a/{target}.i8"), "--m", str(A.shape[0]))
        emit("fraud_proof_rejected_on_honest_a", pv2.get("proof") == "invalid", matrix=target, reason=pv2.get("reason"))

    # 5. tokens: a different last output token (never fed back, so every A still matches and
    # only the token check can catch it), and a different first output token (fed back as a row,
    # so the activations no longer match: caught as an activation fraud)
    run, r, req = runs[1]
    for which, kind in ((-1, "tokens"), (0, "activation")):
        t_run = run + f"-tok{which}"
        shutil.copytree(run, t_run)
        q = json.load(open(os.path.join(t_run, "request.json")))
        q["output_ids"][which] = (q["output_ids"][which] + 1) % 128000
        q["output_sha256"] = ""
        q["text_sha256"] = ""
        json.dump(q, open(os.path.join(t_run, "request.json"), "w"))
        v = replay(a.replay, "local", "--model", a.model, "--run", t_run)
        emit("wrong_token_caught", v.get("verdict") == "fraud" and v.get("kind") == kind, token=which, kind=v.get("kind"), reason=v.get("reason"))
    # a correct answer whose receipt text hash does not match
    run, r, req = runs[0]
    t_run = run + "-text"
    shutil.copytree(run, t_run)
    q = dict(req)
    q["text_sha256"] = hashlib.sha256(b"London").hexdigest()
    json.dump(q, open(os.path.join(t_run, "request.json"), "w"))
    v = replay(a.replay, "local", "--model", a.model, "--run", t_run)
    emit("wrong_text_hash_caught", v.get("verdict") == "fraud" and "text_sha256" in (v.get("reason") or ""), reason=v.get("reason"))
    # truncated answer (stopped early)
    run, r, req = runs[1]
    if len(req["output_ids"]) > 2:
        t_run = run + "-trunc"
        shutil.copytree(run, t_run)
        q = dict(req)
        q["output_ids"] = req["output_ids"][:-1]
        q["output_sha256"] = q["text_sha256"] = ""
        json.dump(q, open(os.path.join(t_run, "request.json"), "w"))
        v = replay(a.replay, "local", "--model", a.model, "--run", t_run)
        emit("truncation_caught", v.get("verdict") == "fraud", reason=v.get("reason"))
    print(json.dumps({"summary": "fail" if FAIL else "pass", "failed": FAIL}))
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
