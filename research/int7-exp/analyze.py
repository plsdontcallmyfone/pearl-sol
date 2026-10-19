"""results.jsonl -> markdown tables (deltas vs base and vs W8A8+SQ, paired task flips)."""
import json, math, sys

rows = [json.loads(l) for l in open(sys.argv[1]) if l.startswith("{")]
by = {}
for r in rows:
    by.setdefault(r["model"], {})[r["cfg"]] = r  # last record wins

LABEL = {
    "base": "bf16 weights, fp32 compute (reference)",
    "w8a16": "int8 W per-channel, fp activations",
    "w8a8": "int8 W8A8 per-channel / per-token, RTN",
    "w8a8_sq": "int8 W8A8 + SmoothQuant (a=0.5)",
    "w8a8_sq_lmhead": "int8 W8A8 + SQ, lm_head also int8",
    "w7a16": "int7 W per-channel, fp activations (not a job: A not int7)",
    "w7a7": "ours: int7 W7A7 per-channel / per-token, RTN",
    "w7a7_sq": "ours + SmoothQuant (a=0.5)",
    "w7a7_sq_asym": "ours + SQ, asymmetric (zero-point) activations",
    "w7a7_sq_mineable": "ours + SQ on job-shaped GEMMs only (k >= 2048), rest fp",
    "w7a7_sq_nodown": "ours + SQ, down_proj kept fp",
    "w7a7_sq_lmhead": "ours + SQ, lm_head also int7",
    "w7a7_sq_asym_lmhead": "ours + SQ + asym activations, lm_head also int7",
    "w7a7_sq_asym_mineable": "ours + SQ + asym, job-shaped GEMMs only",
    "w7a7_sq_mse": "ours + SQ, MSE-clipped weights",
    "w7a7_sq_mse_lmhead": "ours + SQ, MSE-clipped weights, lm_head also int7",
    "w7a7_pertensor": "ours, per-tensor W and A scales",
}


def mcnemar(a, b, gold):
    """Paired flips: items base right / cfg wrong and vice versa."""
    lost = sum(1 for x, y, g in zip(a, b, gold) if x == g and y != g)
    won = sum(1 for x, y, g in zip(a, b, gold) if x != g and y == g)
    return lost, won


for model, cfgs in by.items():
    base = cfgs.get("base")
    ref8 = cfgs.get("w8a8_sq")
    print(f"\n### {model}\n")
    b = base
    print(f"WikiText-2 test: {b['ppl_windows']} windows x {b['seqlen']} tokens = {b['ppl_tokens']} scored tokens (of {b['ppl_windows_total']} windows). "
          f"ARC-Easy test first {b.get('arc_n')} items, HellaSwag validation first {b.get('hs_n')} items.\n")
    print("| config | quantized linears | WikiText-2 PPL | dPPL vs base | dPPL vs int8 SQ | ARC-E acc | ARC-E acc_norm | HellaSwag acc | HellaSwag acc_norm |")
    print("|---|---|---|---|---|---|---|---|---|")
    order = [c for c in LABEL if c in cfgs]
    for c in order:
        r = cfgs[c]
        d0 = r["ppl"] - b["ppl"]
        d8 = r["ppl"] - ref8["ppl"] if ref8 else float("nan")
        f = lambda k: f"{100*r[k]:.1f}" if k in r else "-"
        print(f"| {c}: {LABEL[c]} | {r['quantized_linears']} | {r['ppl']:.3f} | {d0:+.3f} ({100*d0/b['ppl']:+.1f}%) | {d8:+.3f} | {f('arc_acc')} | {f('arc_accn')} | {f('hs_acc')} | {f('hs_accn')} |")
    # Standard errors and paired flips
    for task, n_key in (("arc", "arc_n"), ("hs", "hs_n")):
        if f"{task}_picks" not in b:
            continue
        n = b[n_key]
        p = b[f"{task}_accn"]
        print(f"\n{task.upper()} n={n}: binomial SE of one accuracy at p={p:.2f} is {100*math.sqrt(p*(1-p)/n):.1f} points. Paired flips vs base (acc_norm; lost / won):")
        print("| config | lost | won | net |\n|---|---|---|---|")
        # gold not stored; reconstruct correctness from base: use picks + acc only via gold file
        gold = json.load(open(sys.argv[2]))[task][:n] if len(sys.argv) > 2 else None
        for c in order:
            r = cfgs[c]
            if f"{task}_picks" not in r or c == "base" or gold is None:
                continue
            lost, won = mcnemar([x[1] for x in b[f"{task}_picks"]], [x[1] for x in r[f"{task}_picks"]], gold)
            print(f"| {c} | {lost} | {won} | {won-lost:+d} |")
