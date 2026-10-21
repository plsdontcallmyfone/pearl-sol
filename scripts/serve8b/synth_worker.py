#!/usr/bin/env python3
"""A worker for a synthetic model (scripts/serve8b/synth_model.py) with tools/cadre/worker.py's
HTTP API (/health, /tokenize, /generate with NDJSON streaming): it emits fixed tokens and, for
every registered matrix, random int7 activations A (rows x k) and the exact C = A B from the
model's own B^T files, so the gateway's segments, the attester's C check and the miners' blocks
are all real. No language model runs: this only drives the settlement path (no GPU).

    python3 scripts/serve8b/synth_worker.py --model DIR --port 10665
"""
import argparse, json, os, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import numpy as np

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=10665)
    a = ap.parse_args()
    man = json.load(open(os.path.join(a.model, "manifest.json")))
    mats = man["matrices"]
    bts = [np.fromfile(os.path.join(a.model, m["weights"]), dtype=np.int8).reshape(m["n"], m["k"]).astype(np.int32) for m in mats]
    period = len(man["rows_pattern"])
    words = ["Paris", " is", " the", " capital", " of", " France", "."]

    def generate(ids, n, out_dir, pad, emit):
        t0 = time.time()
        out = [1000 + i for i in range(n)]
        text = "".join(words[i % len(words)] for i in range(n))
        rows = len(ids) + n - 1
        padded = -(-rows // period) * period if pad else rows
        rng = np.random.default_rng(sum(ids) * 7919 + n)
        if out_dir:
            os.makedirs(os.path.join(out_dir, "a"), exist_ok=True)
            os.makedirs(os.path.join(out_dir, "c"), exist_ok=True)
        for m, bt in zip(mats, bts):
            A = rng.integers(-64, 65, size=(padded, m["k"]), dtype=np.int8)
            if padded > rows:
                A[rows:] = 0
            C = A.astype(np.int32) @ bt.T
            if out_dir:
                A.tofile(os.path.join(out_dir, "a", f"{m['index']}.i8"))
                C.astype("<i4").tofile(os.path.join(out_dir, "c", f"{m['index']}.i32"))
        for i in range(n):
            if emit:
                emit(out[i], words[i % len(words)])
        dt = time.time() - t0
        return {"output_ids": out, "text": text, "rows": rows, "rows_padded": padded, "prompt_tokens": len(ids), "completion_tokens": n,
                "finish_reason": "length", "timing": {"total_s": round(dt, 3), "decode_tokens_per_s": round(n / max(dt, 1e-6), 2), "synthetic": True}}

    class H(BaseHTTPRequestHandler):
        def log_message(self, *x):
            pass

        def reply(self, code, obj):
            b = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            if self.path == "/health":
                return self.reply(200, {"ok": True, "name": man["name"], "model_id": man.get("model_id"), "matrices": len(mats), "pid": os.getpid(), "device": "synthetic"})
            self.reply(404, {"error": "not found"})

        def do_POST(self):
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
                if self.path == "/tokenize":
                    if "messages" in body:
                        s = "".join(m.get("content") or "" for m in body["messages"])
                    else:
                        s = body["text"]
                    return self.reply(200, {"ids": [b for b in s.encode()][:64] or [1]})
                if self.path == "/generate":
                    ids = [int(x) for x in body["prompt_ids"]]
                    n = int(body.get("max_new_tokens", 16))
                    pad = bool(body.get("pad", True))
                    if not body.get("stream"):
                        return self.reply(200, generate(ids, n, body.get("out_dir"), pad, None))
                    self.send_response(200)
                    self.send_header("content-type", "application/x-ndjson")
                    self.end_headers()
                    def emit(tok, delta):
                        self.wfile.write((json.dumps({"token": tok, "text": delta}) + "\n").encode())
                        self.wfile.flush()
                    r = generate(ids, n, body.get("out_dir"), pad, emit)
                    self.wfile.write((json.dumps({"done": True, **r}) + "\n").encode())
                    self.wfile.flush()
                    return
                self.reply(404, {"error": "not found"})
            except Exception as e:  # noqa: BLE001
                self.reply(400, {"error": f"{type(e).__name__}: {e}"})

    srv = ThreadingHTTPServer((a.host, a.port), H)
    print(json.dumps({"msg": "synthetic worker started", "url": f"http://{a.host}:{a.port}", "matrices": len(mats), "pid": os.getpid()}), flush=True)
    srv.serve_forever()

if __name__ == "__main__":
    main()
