"""Which GEMMs of a forward pass fit a pearl-job job, and what share of FLOPs that is.

Job rules (crates/pearl-job/src/job.rs check_shape) for a weight GEMM C = X W^T with
A = X (m x k activations, miner-committed) and B = W^T (k x n, registered):
  r >= 128 (rank penalty floor), r power of two, k >= 16r, k <= 4r^2, k <= 2^16,
  k % 64 == 0, n % (col period) == 0, m % (row period) == 0 (padding rows is free).
r = 128 is the only sensible choice: credited work is h*w*(dlen/r)*128, so any r > 128
pays r/128 times less, and the floor k >= 16r is lowest at r = 128 (k >= 2048).
Attention QK^T and PV are activation x activation: no registered B, so never a
standing job. Embedding lookup is not a GEMM.

Per-token MACs; attention at an average past length T (decode at context 2T).
Usage: python flops.py CONFIG.json [...]   (prints a markdown table)
"""
import json, sys

R = 128
KMIN = 16 * R


def fits(k, n):
    return k >= KMIN and k % 64 == 0 and k <= min(4 * R * R, 1 << 16) and n % 8 == 0


def gemms(c):
    """List of (name, k, n, count_per_token) for one token's forward pass."""
    H = c["hidden_size"]
    L = c["num_hidden_layers"]
    nh = c["num_attention_heads"]
    out = []
    if "kv_lora_rank" in c:  # DeepSeek-V3 MLA
        qr, kvr = c["q_lora_rank"], c["kv_lora_rank"]
        dn, dr, dv = c["qk_nope_head_dim"], c["qk_rope_head_dim"], c["v_head_dim"]
        out += [("q_a_proj", H, qr, L), ("q_b_proj", qr, nh * (dn + dr), L),
                ("kv_a_proj", H, kvr + dr, L), ("kv_b_proj", kvr, nh * (dn + dv), L), ("o_proj", nh * dv, H, L)]
        dense = c.get("first_k_dense_replace", 0)
        I = c["intermediate_size"]
        out += [("mlp.gate/up (dense layers)", H, I, 2 * dense), ("mlp.down (dense layers)", I, H, dense)]
        mi = c["moe_intermediate_size"]
        act = c["num_experts_per_tok"] + c.get("n_shared_experts", 0)
        out += [("expert gate/up", H, mi, 2 * act * (L - dense)), ("expert down", mi, H, act * (L - dense))]
        hd_attn = dn + dr
    else:
        hd = c.get("head_dim") or H // nh
        nkv = c.get("num_key_value_heads", nh)
        out += [("q_proj", H, nh * hd, L), ("k_proj", H, nkv * hd, L), ("v_proj", H, nkv * hd, L), ("o_proj", nh * hd, H, L)]
        if c.get("num_experts"):
            mi = c["moe_intermediate_size"]
            act = c["num_experts_per_tok"]
            out += [("expert gate/up", H, mi, 2 * act * L), ("expert down", mi, H, act * L)]
        else:
            I = c["intermediate_size"]
            out += [("gate_proj+up_proj", H, I, 2 * L), ("down_proj", I, H, L)]
        hd_attn = hd
    out.append(("lm_head", H, c["vocab_size"], 1))
    return out, nh * hd_attn * L


def report(path, T=2048):
    c = json.load(open(path))
    name = path.split("models--")[-1].split("/")[0] if "models--" in path else path
    gs, attn_per_pos = gemms(c)
    rows, tot, mine, pad_mine = [], 0, 0, 0
    for n_, k, n, cnt in gs:
        macs = k * n * cnt
        tot += macs
        f = fits(k, n)
        mine += macs if f else 0
        if not f and k < KMIN and k % 64 == 0 and n % 8 == 0:
            pad_mine += macs  # mineable only after zero-padding k to 2048 (useful share k/2048)
        rows.append((n_, k, n, cnt, macs, f))
    attn = 2 * attn_per_pos * T  # QK^T + PV per token at average past length T
    tot_all = tot + attn
    return name, c, rows, tot, attn, tot_all, mine, pad_mine


if __name__ == "__main__":
    print("| model | hidden | GEMMs that fit (k >= 2048) | MACs/token (GEMM + attn at T=2048) | mineable share of all MACs | of GEMM MACs | of GEMM MACs excl. lm_head | fit only with k-padding |")
    print("|---|---|---|---|---|---|---|---|")
    for p in sys.argv[1:]:
        name, c, rows, tot, attn, tot_all, mine, pad = report(p)
        fitnames = sorted({r[0] for r in rows if r[5]})
        lm = [r for r in rows if r[0] == "lm_head"][0]
        mine_nolm = mine - (lm[4] if lm[5] else 0)
        print(f"| {name} | {c['hidden_size']} | {', '.join(fitnames) or 'none'} | {tot_all/1e9:.2f} G | {100*mine/tot_all:.1f}% | {100*mine/tot:.1f}% | {100*mine_nolm/(tot-lm[4]):.1f}% | {100*pad/tot:.1f}% |")
