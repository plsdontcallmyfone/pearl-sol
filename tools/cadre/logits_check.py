"""CPU vs GPU binary64 logits and every A/C under cadre-glue-v1 on full WikiText-2 windows.

  python logits_check.py MODEL_DIR [WINDOWS]   (needs a CUDA GPU; one JSON line per window)
"""
import sys, os, json, hashlib, time, torch
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import worker
from datasets import load_dataset
M = sys.argv[1]
W = int(sys.argv[2]) if len(sys.argv) > 2 else 3
text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="test")["text"])
h = lambda t: hashlib.sha256(t.contiguous().numpy().tobytes()).hexdigest()
models = {d: worker.Model(M, d) for d in ("cuda", "cpu")}
ids_all = models["cpu"].tokenizer()(text).input_ids
for w in range(W):
    ids = ids_all[w * 1024:(w + 1) * 1024]
    out = {}
    for d, m in models.items():
        cap = {x.index: ([], []) for x in m.mats}
        t = time.time()
        with torch.no_grad():
            lg = m.forward(ids, m.new_cache(), cap).cpu()
        out[d] = (lg, {i: (torch.cat(a).cpu(), torch.cat(c).cpu()) for i, (a, c) in cap.items()}, time.time() - t)
    (gl, ga, gt), (cl, ca, ct) = out["cuda"], out["cpu"]
    diff_mats = sum(1 for i in ga if not (torch.equal(ga[i][0], ca[i][0]) and torch.equal(ga[i][1], ca[i][1])))
    print(json.dumps({"window": w, "rows": len(ids), "logits_bit_identical": bool(torch.equal(gl, cl)), "logits_differing": int((gl != cl).sum()),
                      "matrices_differ": diff_mats, "sha256_logits_gpu": h(gl), "sha256_logits_cpu": h(cl), "gpu_forward_s": round(gt, 2), "cpu_forward_s": round(ct, 2),
                      "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}), flush=True)
