"""Evaluate one quantization config on one model: WikiText-2 PPL, ARC-Easy, HellaSwag.

Usage: python run.py --model PATH --cfg NAME [--ppl-windows N] [--arc N] [--hs N] --out results.jsonl
Each call loads the model fresh (bf16 storage, fp32 compute) and applies one config.
"""
import argparse, json, math, os, re, time
import torch
import torch.nn as nn
from transformers import AutoModelForCausalLM, AutoTokenizer
from datasets import load_dataset
from quant import QLinear, Fp32Embedding, head_logprob
import glob, mmap, struct
from accelerate import init_empty_weights
from transformers import AutoConfig

BLOCK = ("q_proj", "k_proj", "v_proj", "o_proj", "gate_proj", "up_proj", "down_proj")
W8A8 = dict(w_bits=8, w_gran="channel", a_bits=8, a_gran="token")
W7A7 = dict(w_bits=7, w_gran="channel", a_bits=7, a_gran="token")
CONFIGS = {
    "base": dict(block=None),
    "w8a8": dict(block=W8A8),
    "w8a8_sq": dict(block=W8A8, smooth=True),
    "w7a7": dict(block=W7A7),
    "w7a7_sq": dict(block=W7A7, smooth=True),
    "w7a7_pertensor": dict(block=dict(w_bits=7, w_gran="tensor", a_bits=7, a_gran="tensor")),
    "w7a16": dict(block=dict(w_bits=7, w_gran="channel")),
    "w8a16": dict(block=dict(w_bits=8, w_gran="channel")),
    "w7a7_sq_asym": dict(block=dict(W7A7, a_asym=True), smooth=True),
    "w7a7_sq_mineable": dict(block=W7A7, smooth=True, only_mineable=True),
    "w7a7_sq_lmhead": dict(block=W7A7, smooth=True, lm_head=W7A7),
    "w8a8_sq_lmhead": dict(block=W8A8, smooth=True, lm_head=W8A8),
    "w7a7_sq_nodown": dict(block=W7A7, smooth=True, skip=("down_proj",)),
    "w7a7_sq_mse": dict(block=dict(W7A7, w_mse=True), smooth=True),
    "w7a7_sq_mse_lmhead": dict(block=dict(W7A7, w_mse=True), smooth=True, lm_head=dict(W7A7, w_mse=True)),
    "w8a8_sq_mse": dict(block=dict(W8A8, w_mse=True), smooth=True),
    "w7a7_sq_asym_lmhead": dict(block=dict(W7A7, a_asym=True), smooth=True, lm_head=dict(W7A7, a_asym=True)),
    "w7a7_sq_asym_mineable": dict(block=dict(W7A7, a_asym=True), smooth=True, only_mineable=True),
}


def mineable(k, n, r=128):
    """pearl-job check_shape for a weight GEMM with B = W^T (k x n), r = 128, 4x8 tile.
    m (tokens) is padded to a multiple of 4 by the miner."""
    return k >= 16 * r and k % 64 == 0 and k <= min(4 * r * r, 1 << 16) and n % 8 == 0


def calib_stats(model, tok, path, n_seq=16, seqlen=512):
    if os.path.exists(path):
        return torch.load(path)
    text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="train")["text"][:6000])
    ids = tok(text, return_tensors="pt").input_ids[0]
    stats, hooks = {}, []
    for name, mod in model.named_modules():
        if isinstance(mod, QLinear):
            def hook(m, inp, out, name=name):
                a = inp[0].detach().float().reshape(-1, inp[0].shape[-1]).abs().amax(0)
                stats[name] = torch.maximum(stats[name], a) if name in stats else a
            hooks.append(mod.register_forward_hook(hook))
    with torch.no_grad():
        for i in range(n_seq):
            model.model(ids[i * seqlen:(i + 1) * seqlen][None])
    for h in hooks:
        h.remove()
    torch.save(stats, path)
    return stats


def smooth_factors(model, stats, alpha):
    """SmoothQuant s_j = amax_x^alpha / amax_w^(1-alpha), shared by layers with one input.
    o_proj's factors are shared across the query heads of one KV group so they fold
    into v_proj rows; down_proj's fold into up_proj rows; q/k/v and gate/up into norms."""
    cfg = model.config
    hd = getattr(cfg, "head_dim", None) or cfg.hidden_size // cfg.num_attention_heads
    group = cfg.num_attention_heads // cfg.num_key_value_heads
    out = {}
    for li, layer in enumerate(model.model.layers):
        pre = f"model.layers.{li}."
        sets = [("self_attn", ("q_proj", "k_proj", "v_proj")), ("self_attn", ("o_proj",)),
                ("mlp", ("gate_proj", "up_proj")), ("mlp", ("down_proj",))]
        for part, names in sets:
            mods = [getattr(getattr(layer, part), n) for n in names]
            ax = stats[pre + part + "." + names[0]]
            wmax = torch.stack([m.w.float().abs().amax(0) for m in mods]).amax(0)
            if names == ("o_proj",):
                ax = ax.view(cfg.num_key_value_heads, group, hd).amax(1, keepdim=True).expand(-1, group, -1).reshape(-1)
                wmax = wmax.view(cfg.num_key_value_heads, group, hd).amax(1, keepdim=True).expand(-1, group, -1).reshape(-1)
            s = (ax.clamp(min=1e-5) ** alpha / wmax.clamp(min=1e-5) ** (1 - alpha)).clamp(min=1e-5)
            for n in names:
                out[pre + part + "." + n] = s
    return out


_MAPS = []


def mmap_safetensors(path):
    """Zero-copy tensors backed by the file's pages (evictable page cache, not anon RSS)."""
    f = open(path, "rb")
    mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    _MAPS.append((f, mm))
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
            t = torch.frombuffer(mm, dtype=d, count=(b - a) // d.itemsize, offset=base + a)
            out[name] = t.reshape(meta["shape"])
    return out


def load_mmap(model_path):
    cfg = AutoConfig.from_pretrained(model_path)
    with init_empty_weights():
        model = AutoModelForCausalLM.from_config(cfg, torch_dtype=torch.bfloat16)
    sd = {}
    for f in sorted(glob.glob(os.path.join(model_path, "*.safetensors"))):
        sd.update(mmap_safetensors(f))
    missing, unexpected = model.load_state_dict(sd, strict=False, assign=True)
    model.tie_weights()
    missing = [m for m in missing if not (m == "lm_head.weight" and cfg.tie_word_embeddings)]
    assert not missing and not unexpected, (missing, unexpected)
    for n, b in model.named_buffers():  # rotary inv_freq etc. were created on meta
        pass
    # Re-create non-persistent buffers (rotary inv_freq) that init_empty_weights left on meta.
    for mod in model.modules():
        if hasattr(mod, "inv_freq") and mod.inv_freq.is_meta:
            from transformers.modeling_rope_utils import ROPE_INIT_FUNCTIONS
            inv, sc = ROPE_INIT_FUNCTIONS[mod.rope_type](mod.config, "cpu")
            mod.inv_freq = inv
            mod.original_inv_freq = inv
            mod.attention_scaling = sc
    model.eval()
    return model


def build(model_path, cfg_name, alpha, cache_dir):
    cfg = CONFIGS[cfg_name]
    tok = AutoTokenizer.from_pretrained(model_path)
    model = load_mmap(model_path)
    # Step 1: every Linear -> fp QLinear (bf16 storage, fp32 compute), embeddings fp32 out.
    def wrap_all(root):
        for name, child in list(root.named_children()):
            if isinstance(child, nn.Linear):
                setattr(root, name, QLinear(child, {}, name=name))
            else:
                wrap_all(child)
    wrap_all(model)
    model.model.embed_tokens = Fp32Embedding(model.model.embed_tokens)
    for n, p in model.named_parameters():
        if p.dtype == torch.bfloat16 and "embed_tokens" not in n:
            p.data = p.data.float()
    for n, b in model.named_buffers():
        if b.dtype == torch.bfloat16:
            b.data = b.data.float()
    smooth = {}
    if cfg.get("smooth"):
        tag = os.path.basename(model_path.rstrip("/")) if "snapshots" not in model_path else model_path.split("models--")[1].split("/")[0]
        stats = calib_stats(model, tok, os.path.join(cache_dir, f"calib-{tag}.pt"))
        smooth = smooth_factors(model, stats, alpha)
    # Step 2: re-wrap quantized layers.
    for name, mod in list(model.named_modules()):
        for cname, child in list(mod.named_children()):
            if not isinstance(child, QLinear):
                continue
            full = f"{name}.{cname}" if name else cname
            k, n = child.in_features, child.out_features
            lin = nn.Linear(k, n, bias=child.bias is not None)
            lin.weight = nn.Parameter(child.w, requires_grad=False)
            if child.bias is not None:
                lin.bias = child.bias
            q = None
            if cname in BLOCK and cfg.get("block") and cname not in cfg.get("skip", ()):
                if not cfg.get("only_mineable") or mineable(k, n):
                    q = cfg["block"]
            if cname == "lm_head" and cfg.get("lm_head"):
                q = cfg["lm_head"]
            if q is None:
                continue
            s = smooth.get(full)
            if cname == "lm_head" and cfg.get("smooth"):
                s = None  # lm_head unsmoothed (would need folding into the final norm; kept simple)
            setattr(mod, cname, QLinear(lin, q, smooth=s, name=full))
    return model, tok




@torch.no_grad()
def wikitext_ppl(model, tok, n_windows, seqlen):
    text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="test")["text"])
    ids = tok(text, return_tensors="pt").input_ids[0]
    total = ids.numel() // seqlen
    n = min(n_windows, total)
    nll, cnt = 0.0, 0
    for i in range(n):
        x = ids[i * seqlen:(i + 1) * seqlen][None]
        h = model.model(x).last_hidden_state[0]
        lp = head_logprob(model.lm_head, h[:-1], x[0, 1:])
        nll -= lp.sum().item()
        cnt += lp.numel()
    return math.exp(nll / cnt), cnt, total


@torch.no_grad()
def loglik_choices(model, tok, ctx, conts):
    """lm-eval style: context tokens = tok(ctx), continuation = tok(ctx+cont)[len(ctx):].
    Runs the context once, then all continuations as one batch on the expanded KV cache.
    Returns sum log p(cont | ctx) per continuation."""
    c_ids = tok(ctx).input_ids
    conts_ids = [tok(ctx + c).input_ids[len(c_ids):] for c in conts]
    from transformers import DynamicCache
    out = model.model(torch.tensor([c_ids]), past_key_values=DynamicCache(), use_cache=True)
    cache = out.past_key_values
    h_last = out.last_hidden_state[0, -1]
    B = len(conts_ids)
    cache.batch_repeat_interleave(B)
    L = max(len(c) for c in conts_ids)
    pad = tok.pad_token_id if tok.pad_token_id is not None else 0
    ids = torch.full((B, L), pad, dtype=torch.long)
    att = torch.zeros((B, len(c_ids) + L), dtype=torch.long)
    att[:, :len(c_ids)] = 1
    for i, c in enumerate(conts_ids):
        ids[i, :len(c)] = torch.tensor(c)
        att[i, len(c_ids):len(c_ids) + len(c)] = 1
    h = model.model(ids, attention_mask=att, past_key_values=cache, use_cache=True).last_hidden_state
    res = []
    for i, c in enumerate(conts_ids):
        hs = torch.cat([h_last[None], h[i, :len(c) - 1]])
        res.append(head_logprob(model.lm_head, hs, torch.tensor(c)).sum().item())
    return res


def hs_pre(t):
    t = t.strip().replace(" [title]", ". ")
    t = re.sub("\\[.*?\\]", "", t)
    return t.replace("  ", " ")


def mc_eval(model, tok, items):
    """items: list of (ctx, [conts], gold). acc and acc_norm (by continuation chars)."""
    acc = accn = 0
    picks = []
    for ctx, conts, gold in items:
        ll = loglik_choices(model, tok, ctx, conts)
        p = max(range(len(ll)), key=lambda i: ll[i])
        pn = max(range(len(ll)), key=lambda i: ll[i] / len(conts[i]))
        acc += p == gold
        accn += pn == gold
        picks.append([p, pn])
    n = len(items)
    return acc / n, accn / n, picks


def arc_items(n):
    d = load_dataset("allenai/ai2_arc", "ARC-Easy", split="test")
    out = []
    for ex in d.select(range(min(n, len(d)))):
        labels = ex["choices"]["label"]
        out.append((f"Question: {ex['question']}\nAnswer:", [" " + t for t in ex["choices"]["text"]], labels.index(ex["answerKey"])))
    return out


def hs_items(n):
    d = load_dataset("Rowan/hellaswag", split="validation")
    out = []
    for ex in d.select(range(min(n, len(d)))):
        ctx = ex["ctx_a"] + " " + ex["ctx_b"].capitalize()
        out.append((hs_pre(ex["activity_label"] + ": " + ctx), [" " + hs_pre(e) for e in ex["endings"]], int(ex["label"])))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--cfg", required=True)
    ap.add_argument("--alpha", type=float, default=0.5)
    ap.add_argument("--ppl-windows", type=int, default=40)
    ap.add_argument("--seqlen", type=int, default=1024)
    ap.add_argument("--arc", type=int, default=0)
    ap.add_argument("--hs", type=int, default=0)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cache", default=os.path.dirname(os.path.abspath(__file__)))
    a = ap.parse_args()
    torch.manual_seed(0)
    t0 = time.time()
    model, tok = build(a.model, a.cfg, a.alpha, a.cache)
    nq = sum(1 for m in model.modules() if isinstance(m, QLinear) and m.w is None)
    rec = dict(model=a.model.split("models--")[-1].split("/")[0], cfg=a.cfg, alpha=a.alpha, quantized_linears=nq)
    if a.ppl_windows:
        t = time.time()
        ppl, ntok, total = wikitext_ppl(model, tok, a.ppl_windows, a.seqlen)
        rec.update(ppl=ppl, ppl_tokens=ntok, ppl_windows=a.ppl_windows, ppl_windows_total=total, seqlen=a.seqlen, ppl_s=round(time.time() - t, 1))
    if a.arc:
        acc, accn, picks = mc_eval(model, tok, arc_items(a.arc))
        rec.update(arc_n=a.arc, arc_acc=acc, arc_accn=accn, arc_picks=picks)
    if a.hs:
        acc, accn, picks = mc_eval(model, tok, hs_items(a.hs))
        rec.update(hs_n=a.hs, hs_acc=acc, hs_accn=accn, hs_picks=picks)
    rec["wall_s"] = round(time.time() - t0, 1)
    st = dict(l.split(":", 1) for l in open("/proc/self/status").read().splitlines() if ":" in l)
    rec["rss_anon_mb"] = int(st["RssAnon"].split()[0]) // 1024
    rec["rss_file_mb"] = int(st["RssFile"].split()[0]) // 1024
    with open(a.out, "a") as f:
        f.write(json.dumps(rec) + "\n")
    print(json.dumps({k: v for k, v in rec.items() if not k.endswith("picks")}))


if __name__ == "__main__":
    main()
