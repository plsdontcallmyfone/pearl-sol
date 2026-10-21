"""Cadre model worker: serves a registered model directory in int7 (SPEC 4.5 recipe 1).

Every registered matrix's GEMM runs as the exact int32 product C = A B that its standing
job proves (torch._int_mm, int8 x int8 -> int32, operands in [-64, 64]); scales, the
zero-point correction, attention (QK^T, softmax, PV), RoPE, RMSNorm and SiLU run in fp32.
For each request the worker keeps every matrix's A (int7 activations, all rows of the
request stacked: prompt, then each generated token) and C, and writes them for the gateway,
which posts them as the ticket's segments.

HTTP (127.0.0.1 only):
  GET  /health
  POST /tokenize {"text"} | {"messages", "tools"?}   -> {"ids"} (messages: the model's chat
       template, or the Llama 3 header format when the tokenizer has none)
  POST /generate {"prompt_ids", "max_new_tokens", "out_dir", "stop"?, "stream"?, "pad"?}
       stream: NDJSON lines {"token", "text"} per token, then the final object below;
       pad false: A/C keep exactly `rows` rows (the gateway stacks requests into rounds)
       -> {"output_ids", "text", "rows", "rows_padded", "matrices": [{"index", "m", "k", "n", "a", "c"}], "timing"}
          writes out_dir/a/<i>.i8 (rows_padded x k int8) and out_dir/c/<i>.i32 (rows_padded x n int32 LE)
CLI checks:
  python worker.py --model DIR --ppl N      WikiText-2 test PPL over N windows of 1024 tokens

Greedy decoding only (deterministic on one platform).

Devices: --device cpu (default) or cuda. The int32 product C = A B is exact integer
arithmetic on both (torch._int_mm: int8 x int8 with int32 accumulation, |C| <= 64 * 64 * 2048),
so for a given A every C is bit-identical on CPU and GPU, and a segment verifies the same
everywhere (tools/cadre/gpu_check.py checks it). The fp32 glue (norms, attention, softmax,
scales) is not bit-identical across devices, so A itself can differ in a few elements
between a CPU and a GPU run of the same prompt; a segment is (A, C) as served, and the proof
only needs C = A B.
"""
import argparse, json, math, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import numpy as np
import torch

QMAX = 64


def load_f32(path, shape=None):
    a = torch.from_numpy(np.fromfile(path, dtype="<f4").copy())
    return a.reshape(shape) if shape else a


class Matrix:
    def __init__(self, d, m, dev):
        self.index, self.name, self.k, self.n = m["index"], m["name"], m["k"], m["n"]
        self.cols = tuple(m.get("cols", [0, m["n"]]))
        self.klo, self.khi = m.get("k_range", [0, m["k"]])
        raw = np.fromfile(os.path.join(d, m["weights"]), dtype=np.int8)
        assert raw.size == self.n * self.k, f"{m['weights']}: {raw.size} bytes, expected n x k"
        self.qw = torch.from_numpy(raw).reshape(self.n, self.k).to(dev)
        with open(os.path.join(d, m["aux"]), "rb") as f:
            aux = f.read()
        assert aux[:4] == b"LQA1"
        n, k, flags = np.frombuffer(aux[4:16], dtype="<u4")
        assert (n, k) == (self.n, self.k)
        o = 16
        self.sw = torch.from_numpy(np.frombuffer(aux[o:o + 4 * n], dtype="<f4").copy()).to(dev)
        o += 4 * n
        self.colsum = torch.from_numpy(np.frombuffer(aux[o:o + 4 * n], dtype="<i4").astype(np.float32)).to(dev)
        o += 4 * n
        self.smooth = torch.from_numpy(np.frombuffer(aux[o:o + 4 * k], dtype="<f4").copy()).to(dev) if flags & 1 else None


def int_mm(v, qw):
    """Exact int32 C = v @ qw^T (v: r x k int8, qw: n x k int8, same device). _int_mm wants
    > 16 rows (and on CUDA k, n multiples of 8, which every registered matrix is)."""
    r = v.shape[0]
    pad = max(32, -(-r // 8) * 8)
    if pad == r:
        return torch._int_mm(v, qw.t())
    v = torch.cat([v, torch.zeros(pad - r, v.shape[1], dtype=torch.int8, device=v.device)])
    # clone: a [:r] view keeps the whole padded product alive, and generate() keeps every
    # step's C for the receipt (32x the memory per decoded token: the test-box worker grew
    # about 50 MB per token and was OOM-killed at 7 GB)
    return torch._int_mm(v, qw.t())[:r].clone()


def quant_act(x):
    """Per-token asymmetric int7 onto [-64, 64]: x ~ s * (v - z)."""
    mn = x.amin(-1, keepdim=True)
    mx = x.amax(-1, keepdim=True)
    s = ((mx - mn) / (2 * QMAX)).clamp(min=1e-8)
    z = torch.round(-QMAX - mn / s)
    v = torch.clamp(torch.round(x / s) + z, -QMAX, QMAX)
    return v.to(torch.int8), s, z


class Model:
    def __init__(self, d, device="cpu", glue="cadre-glue-v1"):
        self.dir = d
        self.dev = torch.device(device)
        if self.dev.type == "cuda":
            # full fp32 glue (no TF32), so the GPU's fp32 is as close to the CPU's as it gets
            torch.backends.cuda.matmul.allow_tf32 = False
            torch.backends.cudnn.allow_tf32 = False
        self.man = json.load(open(os.path.join(d, "manifest.json")))
        a = self.man["arch"]
        assert a["type"] == "llama"
        self.a = a
        self.L, self.H, self.KV, self.hd = a["layers"], a["heads"], a["kv_heads"], a["head_dim"]
        self.eps = a["rms_norm_eps"]
        self.mats = [Matrix(d, m, self.dev) for m in self.man["matrices"]]
        # base name -> column shards (concatenated) -> k shards (int32 partial products summed)
        self.by_name = {}
        for mm, m in zip(self.mats, self.man["matrices"]):
            base = m["name"].split("[")[0].split("{")[0]
            self.by_name.setdefault(base, {}).setdefault(mm.cols, []).append(mm)
        for groups in self.by_name.values():
            for ks in groups.values():
                ks.sort(key=lambda x: x.klo)
        assert not self.man.get("unmined"), "unmined fp layers are not supported by this worker yet"
        t = {x["name"]: x for x in self.man["tensors"]}
        e = t["embed"]
        self.embed = torch.from_numpy(np.fromfile(os.path.join(d, e["file"]), dtype=np.int16)).view(torch.bfloat16).reshape(e["shape"]).to(self.dev)
        self.norms = {k: load_f32(os.path.join(d, v["file"])).to(self.dev) for k, v in t.items() if k.startswith("norm.")}
        self.inv_freq = load_f32(os.path.join(d, t["rope_inv_freq"]["file"])).to(self.dev)
        self.attn_scale = t["rope_inv_freq"].get("attention_scaling", 1.0)
        self.tok = None
        self.lock = threading.Lock()
        # cadre-glue-v1 (SPEC 4.7, tools/cadre/glue.py): deterministic glue, the same A on every
        # device and in every batch shape, so an evaluator can replay it; "fp32": the old glue
        assert glue in ("cadre-glue-v1", "fp32"), glue
        self.glue = glue
        self.det = None
        if glue == "cadre-glue-v1":
            import glue as gluemod
            self.det = gluemod.DetGlue(self)

    def tokenizer(self):
        if self.tok is None:
            from transformers import AutoTokenizer
            self.tok = AutoTokenizer.from_pretrained(self.dir)
        return self.tok

    # ------------------------------------------------------------------ forward

    def lin(self, name, x, cap):
        """y = x W^T through every registered shard of `name`: column shards concatenated,
        k shards summed as exact int32 partial products before the scales."""
        outs = []
        for cols in sorted(self.by_name[name]):
            ks = self.by_name[name][cols]
            smooth = None if ks[0].smooth is None else torch.cat([m.smooth for m in ks])
            xs = x / smooth if smooth is not None else x
            v, s, z = quant_act(xs)
            c = None
            for m in ks:
                vs = v[:, m.klo:m.khi].contiguous()
                part = int_mm(vs, m.qw)
                if cap is not None:
                    cap[m.index][0].append(vs)
                    cap[m.index][1].append(part)
                c = part if c is None else c + part
            colsum = ks[0].colsum if len(ks) == 1 else sum(m.colsum for m in ks)
            outs.append(s * (c.float() - z * colsum) * ks[0].sw)
        return outs[0] if len(outs) == 1 else torch.cat(outs, -1)

    def rms(self, x, w):
        return x * torch.rsqrt(x.pow(2).mean(-1, keepdim=True) + self.eps) * w

    def rope(self, x, pos):
        f = pos[:, None].float() * self.inv_freq[None, :]
        emb = torch.cat([f, f], -1)
        cos, sin = emb.cos() * self.attn_scale, emb.sin() * self.attn_scale
        h = x.shape[-1] // 2
        rot = torch.cat([-x[..., h:], x[..., :h]], -1)
        return x * cos[:, None, :] + rot * sin[:, None, :]

    def forward(self, ids, cache, cap, logits_rows=None):
        """ids: new token ids (r); cache: per-layer [K, V] (T x KV x hd) extended in place.
        Returns logits (r x vocab) for every new row (all rows go through lm_head)."""
        if self.det is not None:
            return self.det.forward(ids, cache, cap)
        r = len(ids)
        start = cache[0][0].shape[0] if cache[0][0] is not None else 0
        pos = torch.arange(start, start + r, device=self.dev)
        h = self.embed[torch.tensor(ids, device=self.dev)].float()
        H, KV, hd = self.H, self.KV, self.hd
        for li in range(self.L):
            x = self.rms(h, self.norms[f"norm.{li}.attn"])
            qkv = self.lin(f"layers.{li}.qkv", x, cap)
            q = qkv[:, : H * hd].reshape(r, H, hd)
            k = qkv[:, H * hd: (H + KV) * hd].reshape(r, KV, hd)
            v = qkv[:, (H + KV) * hd:].reshape(r, KV, hd)
            q, k = self.rope(q, pos), self.rope(k, pos)
            K = k if cache[li][0] is None else torch.cat([cache[li][0], k])
            V = v if cache[li][1] is None else torch.cat([cache[li][1], v])
            cache[li] = [K, V]
            T = K.shape[0]
            g = H // KV
            Kr = K.repeat_interleave(g, dim=1).transpose(0, 1)  # H x T x hd
            Vr = V.repeat_interleave(g, dim=1).transpose(0, 1)
            att = torch.matmul(q.transpose(0, 1), Kr.transpose(1, 2)) / math.sqrt(hd)  # H x r x T
            mask = torch.arange(T, device=self.dev)[None, :] > pos[:, None]
            att = att.masked_fill(mask[None], float("-inf")).softmax(-1)
            o = torch.matmul(att, Vr).transpose(0, 1).reshape(r, H * hd)
            h = h + self.lin(f"layers.{li}.o", o, cap)
            x = self.rms(h, self.norms[f"norm.{li}.mlp"])
            gte = self.lin(f"layers.{li}.gate", x, cap)
            up = self.lin(f"layers.{li}.up", x, cap)
            h = h + self.lin(f"layers.{li}.down", torch.nn.functional.silu(gte) * up, cap)
        h = self.rms(h, self.norms["norm.final"])
        return self.lin("lm_head", h, cap)

    def pick(self, row):
        """Greedy token (cadre-glue-v1: largest binary64 logit, lowest index on ties)."""
        if self.det is not None:
            import glue as gluemod
            return gluemod.pick(row)
        return int(row.argmax())

    def eos_ids(self):
        """Stop tokens: config eos plus generation_config.json's (an Instruct model ends a turn
        with <|eot_id|>, which a base config may not list)."""
        ids = set()
        srcs = [self.a.get("eos_token_id")]
        g = os.path.join(self.dir, "generation_config.json")
        if os.path.exists(g):
            srcs.append(json.load(open(g)).get("eos_token_id"))
        for e in srcs:
            if e is not None:
                ids.update(e if isinstance(e, list) else [e])
        return ids

    def new_cache(self):
        return [[None, None] for _ in range(self.L)]

    def chat_ids(self, messages, tools=None):
        """Prompt ids for chat messages: the tokenizer's chat template, else the Llama 3 format."""
        tok = self.tokenizer()
        if getattr(tok, "chat_template", None):
            return tok.apply_chat_template(messages, tools=tools, add_generation_prompt=True)
        parts = ["<|begin_of_text|>"]
        if tools:
            parts.append("<|start_header_id|>system<|end_header_id|>\n\nYou can call these tools. To call one, answer with only a JSON object "
                         "{\"name\": ..., \"parameters\": {...}}.\n" + json.dumps(tools) + "<|eot_id|>")
        for m in messages:
            c = m.get("content") or ""
            if isinstance(c, list):
                c = "".join(x.get("text", "") for x in c if isinstance(x, dict))
            parts.append(f"<|start_header_id|>{m.get('role', 'user')}<|end_header_id|>\n\n{c}<|eot_id|>")
        parts.append("<|start_header_id|>assistant<|end_header_id|>\n\n")
        return tok("".join(parts), add_special_tokens=False).input_ids

    @torch.no_grad()
    def generate(self, prompt_ids, max_new, out_dir=None, stop=None, on_token=None, pad=True):
        with self.lock:
            cap = {m.index: ([], []) for m in self.mats}
            cache = self.new_cache()
            eos = self.eos_ids()
            t0 = time.time()
            logits = self.forward(list(prompt_ids), cache, cap)
            t_prefill = time.time() - t0
            out = []
            nxt = self.pick(logits[-1])
            t1 = time.time()
            tok = self.tokenizer()
            stop = [x for x in (stop or []) if x]
            finish = "length"
            text_so_far = ""
            while True:
                out.append(nxt)
                if on_token is not None or stop:
                    full = tok.decode(out, skip_special_tokens=True)
                    if on_token is not None:
                        on_token(nxt, full[len(text_so_far):])
                    text_so_far = full
                    if any(x in full for x in stop):
                        finish = "stop"
                        break
                if nxt in eos:
                    finish = "stop"
                    break
                if len(out) >= max_new:
                    break
                logits = self.forward([nxt], cache, cap)
                nxt = self.pick(logits[-1])
            t_decode = time.time() - t1
            rows = len(prompt_ids) + len(out) - 1
            period = len(self.man["rows_pattern"])
            padded = -(-rows // period) * period if pad else rows
            mats = []
            if out_dir:
                os.makedirs(os.path.join(out_dir, "a"), exist_ok=True)
                os.makedirs(os.path.join(out_dir, "c"), exist_ok=True)
            t2 = time.time()
            for m in self.mats:
                A = torch.cat(cap[m.index][0]).cpu()
                C = torch.cat(cap[m.index][1]).cpu()
                assert A.shape[0] == rows, (m.name, A.shape, rows)
                if padded > rows:
                    A = torch.cat([A, torch.zeros(padded - rows, m.k, dtype=torch.int8)])
                    C = torch.cat([C, torch.zeros(padded - rows, m.n, dtype=torch.int32)])
                rec = {"index": m.index, "name": m.name, "m": padded, "k": m.k, "n": m.n}
                if out_dir:
                    rec["a"] = os.path.join(out_dir, "a", f"{m.index}.i8")
                    rec["c"] = os.path.join(out_dir, "c", f"{m.index}.i32")
                    A.numpy().tofile(rec["a"])
                    C.numpy().astype("<i4").tofile(rec["c"])
                mats.append(rec)
            t_segments = time.time() - t2
            text = tok.decode(out, skip_special_tokens=True)
            for x in stop:
                if x in text:
                    text = text[: text.index(x)]
            macs = sum(m.k * m.n for m in self.mats)
            return {
                "output_ids": out, "text": text, "rows": rows, "rows_padded": padded, "prompt_tokens": len(prompt_ids),
                "completion_tokens": len(out), "finish_reason": finish, "glue": self.glue,
                "matrices": mats,
                "timing": {"prefill_s": round(t_prefill, 3), "decode_s": round(t_decode, 3), "first_token_s": round(t_prefill, 3),
                           "decode_tokens_per_s": round((len(out) - 1) / t_decode, 3) if len(out) > 1 and t_decode > 0 else None,
                           "segments_s": round(t_segments, 3), "mined_macs": macs * rows, "threads": torch.get_num_threads(),
                           "device": str(self.dev)},
            }

    @torch.no_grad()
    def nll(self, ids):
        """Sum of -log p over ids[1:] (one prefill, no capture)."""
        logits = self.forward(list(ids), self.new_cache(), None)
        lp = torch.log_softmax(logits[:-1].float(), -1)
        t = torch.tensor(ids[1:], device=self.dev)
        return float(-lp[torch.arange(len(t), device=self.dev), t].sum()), len(t)


def serve(model, host, port):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
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
                return self.reply(200, {"ok": True, "name": model.man["name"], "model_id": model.man.get("model_id"), "matrices": len(model.mats), "pid": os.getpid(),
                                        "device": str(model.dev), "glue": model.glue})
            self.reply(404, {"error": "not found"})

        def do_POST(self):
            try:
                body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
                if self.path == "/tokenize":
                    if "messages" in body:
                        return self.reply(200, {"ids": list(model.chat_ids(body["messages"], body.get("tools")))})
                    return self.reply(200, {"ids": model.tokenizer()(body["text"]).input_ids})
                if self.path == "/generate":
                    ids = [int(x) for x in body["prompt_ids"]]
                    n = int(body.get("max_new_tokens", 16))
                    assert ids and 1 <= n <= 4096, "prompt_ids and max_new_tokens required"
                    stop = body.get("stop") or []
                    stop = [stop] if isinstance(stop, str) else list(stop)
                    pad = bool(body.get("pad", True))
                    if not body.get("stream"):
                        return self.reply(200, model.generate(ids, n, body.get("out_dir"), stop=stop, pad=pad))
                    self.send_response(200)
                    self.send_header("content-type", "application/x-ndjson")
                    self.end_headers()
                    def emit(token, delta):
                        self.wfile.write((json.dumps({"token": token, "text": delta}) + "\n").encode())
                        self.wfile.flush()
                    r = model.generate(ids, n, body.get("out_dir"), stop=stop, on_token=emit, pad=pad)
                    self.wfile.write((json.dumps({"done": True, **r}) + "\n").encode())
                    self.wfile.flush()
                    return
                self.reply(404, {"error": "not found"})
            except Exception as e:  # noqa: BLE001
                self.reply(400, {"error": f"{type(e).__name__}: {e}"})

    srv = ThreadingHTTPServer((host, port), H)
    print(json.dumps({"msg": "cadre worker started", "url": f"http://{host}:{port}", "model": model.man["name"], "pid": os.getpid()}), flush=True)
    srv.serve_forever()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=10665)
    ap.add_argument("--threads", type=int, default=0)
    ap.add_argument("--device", default=os.environ.get("CADRE_DEVICE", "cpu"), help="cpu (default) or cuda")
    ap.add_argument("--glue", default=os.environ.get("CADRE_GLUE", "cadre-glue-v1"), help="cadre-glue-v1 (default, replayable) or fp32")
    ap.add_argument("--ppl", type=int, default=0, help="WikiText-2 test PPL over N windows of 1024 tokens, then exit")
    ap.add_argument("--seqlen", type=int, default=1024)
    a = ap.parse_args()
    if a.threads:
        torch.set_num_threads(a.threads)
    t0 = time.time()
    model = Model(a.model, a.device, a.glue)
    print(json.dumps({"msg": "loaded", "device": a.device, "s": round(time.time() - t0, 1), "matrices": len(model.mats)}), file=sys.stderr, flush=True)
    if a.ppl:
        from datasets import load_dataset
        text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="test")["text"])
        ids = model.tokenizer()(text, return_tensors="pt").input_ids[0].tolist()
        nll = cnt = 0
        for i in range(a.ppl):
            s, c = model.nll(ids[i * a.seqlen:(i + 1) * a.seqlen])
            nll += s
            cnt += c
            print(json.dumps({"window": i, "ppl_so_far": math.exp(nll / cnt)}), file=sys.stderr, flush=True)
        print(json.dumps({"ppl": math.exp(nll / cnt), "tokens": cnt, "windows": a.ppl, "s": round(time.time() - t0, 1)}))
        return
    serve(model, a.host, a.port)


if __name__ == "__main__":
    main()
