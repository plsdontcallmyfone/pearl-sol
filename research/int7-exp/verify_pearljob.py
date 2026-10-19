"""Check the simulator's integer product against pearl-job on real model matrices.

For one linear layer of a real model: capture real input activations (m tokens),
quantize exactly as quant.py does (A = per-token int7, B = W^T per-channel int7),
write the pearl-job data dir (A.i8 m x k, B.i8 k x n, manifest.json), then run
`pearl-job commit`, `pearl-job matmul` and `pearl-job freivalds` and compare C with the
simulator's fp32 product of the integer tensors and an exact int64 product.

Usage: python verify_pearljob.py --model PATH --layer N --proj mlp.down_proj --m 64 --out DIR
"""
import argparse, json, os, subprocess, sys
import numpy as np
import torch
from transformers import AutoModelForCausalLM, AutoTokenizer
from datasets import load_dataset
from quant import quant_sym
from run import build

PJ = os.environ.get("PEARL_JOB", "/home/pearl/pearl-sol/crates/target/release/pearl-job")
JOB = ["--job", "11111111111111111111111111111111", "--terms-hash", "00" * 32, "--slot", "0", "--target-bits", "0x1d010000"]


def pj(*args):
    r = subprocess.run([PJ, *args], capture_output=True, text=True)
    return json.loads(r.stdout.strip().splitlines()[-1])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--layer", type=int, default=0)
    ap.add_argument("--proj", default="mlp.down_proj")
    ap.add_argument("--m", type=int, default=64)
    ap.add_argument("--out", required=True)
    ap.add_argument("--mine-nonces", type=int, default=0)
    a = ap.parse_args()
    model, tok = build(a.model, "base", 0.5, os.path.dirname(os.path.abspath(__file__)))
    mod = model.model.layers[a.layer]
    for p in a.proj.split("."):
        mod = getattr(mod, p)
    cap = {}
    def grab(m, i, o):
        cap.setdefault("x", i[0].detach())  # returns None: output unchanged
    h = mod.register_forward_hook(grab)
    text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="validation")["text"][:200])
    ids = tok(text, return_tensors="pt").input_ids[:, : a.m]
    with torch.no_grad():
        model.model(ids)
    h.remove()
    x = cap["x"][0].float()  # m x k
    w = mod.w.float()  # n x k (bf16 checkpoint weights)
    m, k = x.shape
    n = w.shape[0]
    qa, sa = quant_sym(x, 7, -1)
    qw, sw = quant_sym(w, 7, 1)
    A = qa.to(torch.int8).numpy()  # m x k
    B = qw.t().contiguous().to(torch.int8).numpy()  # k x n
    os.makedirs(a.out, exist_ok=True)
    A.tofile(os.path.join(a.out, "A.i8"))
    B.tofile(os.path.join(a.out, "B.i8"))
    json.dump({"m": m, "n": n, "k": k, "a": "A.i8", "b": "B.i8"}, open(os.path.join(a.out, "manifest.json"), "w"))
    c_sim = (qa @ qw.t())  # simulator path (fp32 on integer values)
    c_exact = torch.from_numpy(A.astype(np.int64) @ B.astype(np.int64))
    cfg = pj("config", "--k", str(k), "--rank", "128")
    rec = dict(model=a.model.split("models--")[-1].split("/")[0], layer=a.layer, proj=a.proj, m=m, k=k, n=n,
               a_range=[int(A.min()), int(A.max())], b_range=[int(B.min()), int(B.max())], config_ok=cfg.get("ok"), config_err=cfg.get("error"))
    if cfg.get("ok"):
        com = pj("commit", *JOB, "--config", cfg["config"], "--data", a.out)
        rec["commit_ok"] = com.get("ok")
        rec["commit_err"] = com.get("error")
        cpath = os.path.join(a.out, "C.i32")
        mm = pj("matmul", *JOB, "--config", cfg["config"], "--data", a.out, "--out", cpath)
        rec["matmul_ok"] = mm.get("ok")
        if mm.get("ok"):
            c_pj = torch.from_numpy(np.fromfile(cpath, dtype="<i4").astype(np.int64).reshape(m, n))
            rec["pj_eq_exact"] = bool(torch.equal(c_pj, c_exact))
            rec["sim_eq_exact"] = bool(torch.equal(c_sim.to(torch.int64), c_exact)) and bool((c_sim == c_sim.round()).all())
            rec["max_abs_C"] = int(c_exact.abs().max())
            # Write the simulator's C and have pearl-job Freivalds-check it.
            simpath = os.path.join(a.out, "C_sim.i32")
            c_sim.to(torch.int32).numpy().astype("<i4").tofile(simpath)
            fv = pj("freivalds", *JOB, "--config", cfg["config"], "--data", a.out, "--c", simpath)
            rec["freivalds_sim_valid"] = fv.get("valid")
            # Dequantized output vs the fp32 layer output.
            y_q = c_sim * sa * sw.t()
            y = x @ w.t()
            rec["rel_err_int7_layer"] = float((y_q - y).norm() / y.norm())
            qa8, sa8 = quant_sym(x, 8, -1)
            qw8, sw8 = quant_sym(w, 8, 1)
            rec["rel_err_int8_layer"] = float(((qa8 @ qw8.t()) * sa8 * sw8.t() - y).norm() / y.norm())
        if a.mine_nonces:
            mn = pj("mine", *JOB, "--config", cfg["config"], "--data", a.out, "--miner", "11111111111111111111111111111112",
                    "--nonce-end", str(a.mine_nonces), "--share-target", "0x1e040000", "--out", os.path.join(a.out, "proof.bin"))
            rec["mine"] = {kk: mn.get(kk) for kk in ("ok", "found", "nonces_tried", "ms_per_nonce", "error")}
            if mn.get("found"):
                vf = pj("verify", *JOB, "--config", cfg["config"], "--m", str(m), "--n", str(n), "--hash-a", mn["hash_a"], "--hash-b", mn["hash_b"],
                        "--miner", "11111111111111111111111111111112", "--proof", os.path.join(a.out, "proof.bin"), "--share-target", "0x1e040000")
                rec["verify"] = {kk: vf.get(kk) for kk in ("ok", "valid", "reason", "proof_len")}
    print(json.dumps(rec))


if __name__ == "__main__":
    main()
