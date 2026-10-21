"""Accuracy of a Cadre int7 model directory vs the fp16 Hugging Face checkpoint it came from.

Fixed eval, the same token ids for both modes:
  * perplexity: WikiText-2 (raw) test split, the first --windows windows of --seqlen tokens;
  * multiple choice: ARC-Easy and ARC-Challenge test (first --mc items each, in file order) and
    HellaSwag validation (first --mc items): each choice scored by the sum of its token
    log-probs given the question (acc) and by that sum over the choice's byte length (acc_norm,
    the lm-eval-harness convention). Zero-shot, no chat template.

Usage:
  python eval_accuracy.py --mode int7 --model CADRE_DIR [--device cuda] --out FILE.jsonl
  python eval_accuracy.py --mode fp16 --model HF_DIR --out FILE.jsonl
Prints one JSON line per metric and appends them to --out.
"""
import argparse, json, math, os, sys, time

import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


class Int7:
    def __init__(self, d, device, glue="cadre-glue-v1"):
        import worker
        self.m = worker.Model(d, device, glue)
        self.tok = self.m.tokenizer()
        self.name = self.m.man["name"]

    @torch.no_grad()
    def logits(self, ids):
        return self.m.forward(list(ids), self.m.new_cache(), None).float()


class FP16:
    def __init__(self, d, device):
        from transformers import AutoModelForCausalLM, AutoTokenizer
        self.tok = AutoTokenizer.from_pretrained(d)
        self.m = AutoModelForCausalLM.from_pretrained(d, torch_dtype=torch.float16).to(device).eval()
        self.dev = device
        self.name = os.path.basename(d.rstrip("/"))

    @torch.no_grad()
    def logits(self, ids):
        return self.m(torch.tensor([list(ids)], device=self.dev)).logits[0].float()


def ppl(model, windows, seqlen):
    from datasets import load_dataset
    text = "\n\n".join(load_dataset("wikitext", "wikitext-2-raw-v1", split="test")["text"])
    ids = model.tok(text, return_tensors="pt").input_ids[0].tolist()
    nll = cnt = 0.0
    for i in range(windows):
        w = ids[i * seqlen:(i + 1) * seqlen]
        lg = model.logits(w)
        lp = torch.log_softmax(lg[:-1], -1)
        t = torch.tensor(w[1:], device=lp.device)
        nll += float(-lp[torch.arange(len(t), device=lp.device), t].sum())
        cnt += len(t)
    return math.exp(nll / cnt), int(cnt)


def score(model, ctx, conts):
    """Sum of log p(continuation tokens | ctx) per continuation (one forward each)."""
    bos = [model.tok.bos_token_id] if model.tok.bos_token_id is not None else []
    c_ids = bos + model.tok(ctx, add_special_tokens=False).input_ids
    out = []
    for c in conts:
        x_ids = model.tok(c, add_special_tokens=False).input_ids
        ids = c_ids + x_ids
        lg = model.logits(ids)
        lp = torch.log_softmax(lg[len(c_ids) - 1:len(ids) - 1], -1)
        t = torch.tensor(x_ids, device=lp.device)
        out.append(float(lp[torch.arange(len(t), device=lp.device), t].sum()))
    return out


def mc_items(task, n):
    from datasets import load_dataset
    items = []
    if task.startswith("arc"):
        cfg = "ARC-Easy" if task == "arc_easy" else "ARC-Challenge"
        for r in load_dataset("allenai/ai2_arc", cfg, split="test").select(range(n)):
            items.append(("Question: " + r["question"] + "\nAnswer:", [" " + t for t in r["choices"]["text"]], r["choices"]["label"].index(r["answerKey"])))
    else:
        for r in load_dataset("Rowan/hellaswag", split="validation").select(range(n)):
            ctx = r["activity_label"] + ": " + r["ctx_a"] + " " + r["ctx_b"].capitalize()
            items.append((ctx, [" " + e for e in r["endings"]], int(r["label"])))
    return items


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["int7", "fp16"], required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--device", default="cuda")
    ap.add_argument("--windows", type=int, default=40)
    ap.add_argument("--seqlen", type=int, default=1024)
    ap.add_argument("--mc", type=int, default=200)
    ap.add_argument("--tasks", default="arc_easy,arc_challenge,hellaswag")
    ap.add_argument("--out", default="")
    ap.add_argument("--glue", default="cadre-glue-v1", help="int7 mode: cadre-glue-v1 (deterministic) or fp32")
    a = ap.parse_args()
    t0 = time.time()
    model = Int7(a.model, a.device, a.glue) if a.mode == "int7" else FP16(a.model, a.device)
    gpu = torch.cuda.get_device_name(0) if torch.cuda.is_available() and a.device != "cpu" else None
    base = {"mode": a.mode, "model": model.name, "gpu": gpu, "torch": torch.__version__, "glue": a.glue if a.mode == "int7" else None}

    def emit(rec):
        rec = {**base, **rec, "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "elapsed_s": round(time.time() - t0, 1)}
        print(json.dumps(rec), flush=True)
        if a.out:
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")

    p, n = ppl(model, a.windows, a.seqlen)
    emit({"metric": "wikitext2_ppl", "value": round(p, 4), "tokens": n, "windows": a.windows, "seqlen": a.seqlen})
    for task in [t for t in a.tasks.split(",") if t]:
        items = mc_items(task, a.mc)
        acc = accn = 0
        for ctx, conts, gold in items:
            s = score(model, ctx, conts)
            acc += int(max(range(len(s)), key=lambda i: s[i]) == gold)
            sn = [x / max(1, len(c.encode())) for x, c in zip(s, conts)]
            accn += int(max(range(len(sn)), key=lambda i: sn[i]) == gold)
        emit({"metric": task, "acc": round(acc / len(items), 4), "acc_norm": round(accn / len(items), 4), "items": len(items),
              "correct": acc, "correct_norm": accn})


if __name__ == "__main__":
    main()
