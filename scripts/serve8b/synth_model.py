#!/usr/bin/env python3
"""Synthetic model directory in the Llama-3.1-8B-Instruct w7a7 SHAPE (SPEC 4.5): the same 640
registered matrices in the same order (32 layers x {qkv 2, o 2, gate 4, up 4, down 7} + lm_head
32, every k shard 2048), with random int7 weights. Used by scripts/serve8b (no GPU, no real
weights). Scaling: every matrix keeps its real k (2048) but has n = --n columns (default 8)
instead of its real 2,048-8,016, so B is 10.5 MB instead of 7.5 GB; the chain txs, the segment
count, the Solana light-client work per segment block (k and the miner's tile set the proof)
and the round structure are those of the real model. --real-n writes the real column counts
(7.5 GB, needs the disk). --filler-mb adds a raw file of that size (an upload-rate probe).

    python3 scripts/serve8b/synth_model.py --out DIR [--n 8] [--real-n] [--filler-mb 0] [--seed 8]
"""
import argparse, json, os, struct
import numpy as np

K = 2048

def shapes():
    """(name, layer, role, k_total, n_total, cols) of every registered matrix, in order."""
    hidden, inter, kv, vocab = 4096, 14336, 1024, 128256
    out = []
    def add(name, layer, role, k_total, n_total, col_shard):
        cols = []
        lo = 0
        while lo < n_total:
            hi = min(n_total, lo + col_shard)
            cols.append((lo, hi))
            lo = hi
        for (lo, hi) in cols:
            for ks in range(0, k_total, K):
                out.append((name, layer, role, (ks, ks + K), k_total, (lo, hi)))
    for l in range(32):
        add(f"layers.{l}.qkv", l, "qkv", hidden, hidden + 2 * kv, 8192)
        add(f"layers.{l}.o", l, "o", hidden, hidden, 8192)
        add(f"layers.{l}.gate", l, "gate", hidden, inter, 7168)
        add(f"layers.{l}.up", l, "up", hidden, inter, 7168)
        add(f"layers.{l}.down", l, "down", inter, hidden, 8192)
    add("lm_head", None, "lm_head", hidden, vocab, 8016)
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--n", type=int, default=8)
    ap.add_argument("--real-n", action="store_true")
    ap.add_argument("--filler-mb", type=int, default=0)
    ap.add_argument("--seed", type=int, default=8)
    ap.add_argument("--name", default="llama-3.1-8b-shape-w7a7")
    a = ap.parse_args()
    os.makedirs(os.path.join(a.out, "w"), exist_ok=True)
    rng = np.random.default_rng(a.seed)
    mats = []
    macs = 0
    for i, (name, layer, role, kr, k_total, cols) in enumerate(shapes()):
        n = (cols[1] - cols[0]) if a.real_n else a.n
        bt = rng.integers(-64, 65, size=(n, K), dtype=np.int8)  # B^T, n x k
        bt.tofile(os.path.join(a.out, f"w/{i}.i8"))
        colsum = bt.astype(np.int32).sum(axis=1)
        with open(os.path.join(a.out, f"w/{i}.aux"), "wb") as f:
            f.write(b"LQA1" + struct.pack("<III", n, K, 0))
            f.write(np.full(n, 1.0 / 64, dtype=np.float32).tobytes())
            f.write(colsum.astype(np.int32).tobytes())
        mats.append({"index": i, "name": f"{name}.k{kr[0] // K}.c{cols[0]}", "layer": layer, "role": role, "k": K, "n": n,
                     "cols": [cols[0], cols[0] + n], "k_range": list(kr), "k_total": k_total, "weights": f"w/{i}.i8", "aux": f"w/{i}.aux", "hash_b": ""})
        macs += K * n
    assert len(mats) == 640, len(mats)
    if a.filler_mb:
        os.makedirs(os.path.join(a.out, "t"), exist_ok=True)
        with open(os.path.join(a.out, "t/filler.bin"), "wb") as f:
            for _ in range(a.filler_mb):
                f.write(rng.integers(0, 256, size=1 << 20, dtype=np.uint8).tobytes())
    man = {"format": "cadre-model-v1", "name": a.name, "base_model": "synthetic (Llama-3.1-8B-Instruct shape)",
           "recipe": {"id": 1, "name": "cadre-w7a7-sq-asym-v1", "synthetic": True},
           "arch": {"type": "llama", "hidden": 4096, "layers": 32, "synthetic_n": None if a.real_n else a.n},
           "rank": 128, "rows_pattern": [0, 1, 2, 3], "cols_pattern": [0, 1, 2, 3, 4, 5, 6, 7], "macs_per_row": macs,
           "matrices": mats, "unmined": [], "files": {}, "model_id": "", "registrant": ""}
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(man, f, indent=1)
    print(json.dumps({"out": a.out, "matrices": len(mats), "macs_per_row": macs, "weights_bytes": macs}))

if __name__ == "__main__":
    main()
