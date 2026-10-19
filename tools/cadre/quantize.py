"""Quantize a Hugging Face Llama checkpoint into a Cadre model directory (SPEC 4.5, recipe 1).

Recipe `cadre-w7a7-sq-asym-v1` (research/INT7-ACCURACY.md, config w7a7_sq_asym_lmhead):
  * weights: SmoothQuant (alpha 0.5) folded in, per-output-channel symmetric RTN onto [-64, 64];
  * per registered matrix, outside the proof: s_w (n f32), colsum(B) (n i32), smooth s (k f32);
  * activations (at serving time, tools/cadre/worker.py): per-token asymmetric int7.
q/k/v are fused into one `qkv` matrix (they share their input and their smooth factor);
gate and up stay separate; any output dimension above --max-n is split into equal column
shards (n a multiple of 8). Matrices that cannot be a job (k < 16 r etc.) are recorded as
unmined and kept in fp (none on Llama-3.2-1B).

Output directory (the model's files; `pearl model register` chunks them into blobs and adds
`files`, `hash_b`, `model_id` to the manifest):
  manifest.json, config.json, tokenizer files,
  w/<i>.i8   B^T (n x k int8, row = output channel),  w/<i>.aux  "LQA1" aux,
  t/embed.bf16, t/norm.<layer>.{attn,mlp}.f32, t/norm.final.f32, t/rope_inv_freq.f32

Usage: python quantize.py --model HF_DIR --out DIR [--name NAME] [--calib calib.pt] [--max-n 8192]
Weights are read zero-copy (mmap) and written one matrix at a time, so memory stays near
one layer.
"""
import argparse, glob, json, mmap, os, shutil, struct, sys

import numpy as np
import torch

RANK = 128
ROWS = [0, 1, 2, 3]
COLS = [0, 1, 2, 3, 4, 5, 6, 7]
QMAX = 64


def mmap_safetensors(path):
    f = open(path, "rb")
    mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    hlen = struct.unpack("<Q", mm[:8])[0]
    hdr = json.loads(mm[8:8 + hlen])
    base = 8 + hlen
    dt = {"BF16": torch.bfloat16, "F16": torch.float16, "F32": torch.float32}
    out = {}
    import warnings
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        for name, meta in hdr.items():
            if name == "__metadata__":
                continue
            a, b = meta["data_offsets"]
            d = dt[meta["dtype"]]
            out[name] = torch.frombuffer(mm, dtype=d, count=(b - a) // d.itemsize, offset=base + a).reshape(meta["shape"])
    return out, (f, mm)


def job_shaped(k, n, r=RANK):
    """pearl-job check_shape for B = W^T (k x n), r = 128, 4x8 tile."""
    return k >= 16 * r and k % 64 == 0 and k <= min(4 * r * r, 1 << 16) and n % 8 == 0 and k * n <= (1 << 26)


def shards(n, max_n):
    """Equal column shards, each a multiple of 8 and at most max_n."""
    for s in range(1, n + 1):
        if n % s == 0 and (n // s) % 8 == 0 and n // s <= max_n:
            return [(i * (n // s), (i + 1) * (n // s)) for i in range(s)]
    return [(0, n)]


def quant_weight(w):
    """w (n x k, fp32, smoothing already applied): q int8 in [-64, 64], s_w (n)."""
    amax = w.abs().amax(dim=1, keepdim=True)
    s = (amax / QMAX).clamp(min=1e-8)
    q = torch.clamp(torch.round(w / s), -QMAX, QMAX).to(torch.int8)
    return q, s[:, 0]


def write_aux(path, q, s_w, smooth):
    n, k = q.shape
    with open(path, "wb") as f:
        f.write(b"LQA1")
        f.write(struct.pack("<III", n, k, 1 if smooth is not None else 0))
        f.write(s_w.float().numpy().astype("<f4").tobytes())
        f.write(q.to(torch.int32).sum(dim=1).numpy().astype("<i4").tobytes())
        if smooth is not None:
            f.write(smooth.float().numpy().astype("<f4").tobytes())


def calibrate(model_dir, n_seq=16, seqlen=512):
    """Input absmax per linear (16 x 512 WikiText-2 train tokens), as research/int7-exp/run.py."""
    from datasets import load_dataset
    from transformers import AutoModelForCausalLM, AutoTokenizer
    tok = AutoTokenizer.from_pretrained(model_dir)
    model = AutoModelForCausalLM.from_pretrained(model_dir, torch_dtype=torch.float32)
    model.eval()
    text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="train")["text"][:6000])
    ids = tok(text, return_tensors="pt").input_ids[0]
    stats, hooks = {}, []
    for name, mod in model.named_modules():
        if isinstance(mod, torch.nn.Linear) and name != "lm_head":
            def hook(m, inp, out, name=name):
                a = inp[0].detach().float().reshape(-1, inp[0].shape[-1]).abs().amax(0)
                stats[name] = torch.maximum(stats[name], a) if name in stats else a
            hooks.append(mod.register_forward_hook(hook))
    with torch.no_grad():
        for i in range(n_seq):
            model.model(ids[i * seqlen:(i + 1) * seqlen][None])
    for h in hooks:
        h.remove()
    del model
    return stats


def rope_inv_freq(cfg):
    from transformers import AutoConfig
    from transformers.modeling_rope_utils import ROPE_INIT_FUNCTIONS
    rope_type = (cfg.get("rope_scaling") or {}).get("rope_type", "default")
    inv, scale = ROPE_INIT_FUNCTIONS[rope_type](AutoConfig.for_model(**cfg), "cpu")
    return inv.float(), float(scale)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True, help="HF checkpoint dir (safetensors + config + tokenizer)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--name", default=None)
    ap.add_argument("--base-model", default="unsloth/Llama-3.2-1B")
    ap.add_argument("--calib", default=None, help="saved input absmax stats (torch file); computed if absent")
    ap.add_argument("--alpha", type=float, default=0.5)
    ap.add_argument("--max-n", type=int, default=8192)
    ap.add_argument("--layers", type=int, default=None, help="quantize only the first N layers (tests)")
    a = ap.parse_args()

    cfg = json.load(open(os.path.join(a.model, "config.json")))
    assert cfg.get("model_type") == "llama", "only Llama-architecture checkpoints are supported"
    L = cfg["num_hidden_layers"] if a.layers is None else a.layers
    H, KV = cfg["num_attention_heads"], cfg["num_key_value_heads"]
    hd = cfg.get("head_dim") or cfg["hidden_size"] // H
    group = H // KV

    if a.calib and os.path.exists(a.calib):
        stats = torch.load(a.calib, weights_only=False)
    else:
        print("calibrating (16 x 512 WikiText-2 train tokens)...", file=sys.stderr)
        stats = calibrate(a.model)
        if a.calib:
            torch.save(stats, a.calib)

    sd, keep = {}, []
    for f in sorted(glob.glob(os.path.join(a.model, "*.safetensors"))):
        t, k = mmap_safetensors(f)
        sd.update(t)
        keep.append(k)
    W = lambda name: sd[name].float()

    os.makedirs(os.path.join(a.out, "w"), exist_ok=True)
    os.makedirs(os.path.join(a.out, "t"), exist_ok=True)
    matrices, unmined, tensors = [], [], []

    def smooth_for(names, stat_name, kv_share=False):
        ax = stats[stat_name].float()
        wmax = torch.stack([W(n).abs().amax(0) for n in names]).amax(0)
        if kv_share:
            ax = ax.view(KV, group, hd).amax(1, keepdim=True).expand(-1, group, -1).reshape(-1)
            wmax = wmax.view(KV, group, hd).amax(1, keepdim=True).expand(-1, group, -1).reshape(-1)
        return (ax.clamp(min=1e-5) ** a.alpha / wmax.clamp(min=1e-5) ** (1 - a.alpha)).clamp(min=1e-5)

    def emit(name, role, layer, w, smooth, input_name):
        """w: n x k fp32 source weight (unsmoothed)."""
        n, k = w.shape
        if not job_shaped(k, min(n, a.max_n)):
            np.asarray(w.to(torch.bfloat16).view(torch.int16).numpy()).tofile(os.path.join(a.out, "t", f"{name}.bf16"))
            unmined.append({"name": name, "role": role, "layer": layer, "k": k, "n": n, "reason": "not job-shaped (k < 16r, k % 64, or n % 8)",
                            "file": f"t/{name}.bf16", "input": input_name})
            return
        ws = w * smooth[None, :] if smooth is not None else w
        for lo, hi in shards(n, a.max_n):
            i = len(matrices)
            q, s_w = quant_weight(ws[lo:hi])
            q.numpy().tofile(os.path.join(a.out, "w", f"{i}.i8"))
            write_aux(os.path.join(a.out, "w", f"{i}.aux"), q, s_w, smooth)
            matrices.append({"index": i, "name": name if (lo, hi) == (0, n) else f"{name}[{lo}:{hi}]", "layer": layer, "role": role,
                             "k": k, "n": hi - lo, "cols": [lo, hi], "weights": f"w/{i}.i8", "aux": f"w/{i}.aux", "hash_b": "",
                             "input": input_name})
        print(f"{name}: {k} x {n} -> {len(shards(n, a.max_n))} matrix(es)", file=sys.stderr)

    for li in range(L):
        p = f"model.layers.{li}."
        att, mlp = p + "self_attn.", p + "mlp."
        qkv = [att + "q_proj.weight", att + "k_proj.weight", att + "v_proj.weight"]
        s = smooth_for(qkv, att + "q_proj")
        emit(f"layers.{li}.qkv", "qkv", li, torch.cat([W(n) for n in qkv]), s, "attn_norm")
        s = smooth_for([att + "o_proj.weight"], att + "o_proj", kv_share=True)
        emit(f"layers.{li}.o", "o", li, W(att + "o_proj.weight"), s, "attn_out")
        s = smooth_for([mlp + "gate_proj.weight", mlp + "up_proj.weight"], mlp + "gate_proj")
        emit(f"layers.{li}.gate", "gate", li, W(mlp + "gate_proj.weight"), s, "mlp_norm")
        emit(f"layers.{li}.up", "up", li, W(mlp + "up_proj.weight"), s, "mlp_norm")
        s = smooth_for([mlp + "down_proj.weight"], mlp + "down_proj")
        emit(f"layers.{li}.down", "down", li, W(mlp + "down_proj.weight"), s, "mlp_act")
        for nm, key in (("attn", "input_layernorm"), ("mlp", "post_attention_layernorm")):
            fn = f"t/norm.{li}.{nm}.f32"
            W(p + key + ".weight").numpy().astype("<f4").tofile(os.path.join(a.out, fn))
            tensors.append({"name": f"norm.{li}.{nm}", "file": fn, "dtype": "f32", "shape": [cfg["hidden_size"]]})
        del s
    head = "lm_head.weight" if "lm_head.weight" in sd else "model.embed_tokens.weight"
    emit("lm_head", "lm_head", None, W(head), None, "final_norm")
    W("model.norm.weight").numpy().astype("<f4").tofile(os.path.join(a.out, "t/norm.final.f32"))
    tensors.append({"name": "norm.final", "file": "t/norm.final.f32", "dtype": "f32", "shape": [cfg["hidden_size"]]})
    emb = sd["model.embed_tokens.weight"]
    emb.contiguous().view(torch.int16).numpy().tofile(os.path.join(a.out, "t/embed.bf16"))
    tensors.append({"name": "embed", "file": "t/embed.bf16", "dtype": "bf16", "shape": list(emb.shape)})
    inv, scale = rope_inv_freq(cfg)
    inv.numpy().astype("<f4").tofile(os.path.join(a.out, "t/rope_inv_freq.f32"))
    tensors.append({"name": "rope_inv_freq", "file": "t/rope_inv_freq.f32", "dtype": "f32", "shape": [inv.numel()], "attention_scaling": scale})

    for f in ("config.json", "tokenizer.json", "tokenizer_config.json", "special_tokens_map.json", "generation_config.json"):
        src = os.path.join(a.model, f)
        if os.path.exists(src):
            shutil.copyfile(src, os.path.join(a.out, f))

    macs = sum(m["k"] * m["n"] for m in matrices)
    manifest = {
        "format": "cadre-model-v1",
        "name": a.name or os.path.basename(a.out.rstrip("/")),
        "base_model": a.base_model,
        "recipe": {"id": 1, "name": "cadre-w7a7-sq-asym-v1", "weights": "per-output-channel symmetric RTN onto [-64, 64], SmoothQuant folded",
                   "smoothquant_alpha": a.alpha, "calibration": "16 x 512 WikiText-2 train tokens, input absmax",
                   "activations": "per-token asymmetric int7 onto [-64, 64]: s_a = (max - min) / 128, z = round(-64 - min / s_a)",
                   "output": "y = s_a * s_w * (C - z * colsum)", "attention": "fp32 (QK^T, softmax, PV never jobs)", "lm_head_smoothed": False},
        "arch": {"type": "llama", "hidden": cfg["hidden_size"], "layers": L, "heads": H, "kv_heads": KV, "head_dim": hd,
                 "intermediate": cfg["intermediate_size"], "vocab": cfg["vocab_size"], "rms_norm_eps": cfg["rms_norm_eps"],
                 "rope_theta": cfg.get("rope_theta"), "rope_scaling": cfg.get("rope_scaling"),
                 "bos_token_id": cfg.get("bos_token_id"), "eos_token_id": cfg.get("eos_token_id"), "tie_word_embeddings": cfg.get("tie_word_embeddings")},
        "rank": RANK, "rows_pattern": ROWS, "cols_pattern": COLS,
        "macs_per_row": macs,
        "matrices": matrices, "unmined": unmined, "tensors": tensors,
        "files": {}, "model_id": "", "registrant": "",
    }
    json.dump(manifest, open(os.path.join(a.out, "manifest.json"), "w"), indent=1)
    print(json.dumps({"out": a.out, "matrices": len(matrices), "unmined": len(unmined), "macs_per_row": macs}))


if __name__ == "__main__":
    main()
