#!/usr/bin/env python3
"""Records every tx that touches the light-client program while a run goes on (a test
validator keeps only a few minutes of history), one JSON line each: slot, fee, CU, the
light-client instructions its logs name, error. `summary FILE [FROM_SLOT]` totals them.

    python3 -I scripts/serve8b/lc_watch.py watch RPC_URL LIGHTCLIENT_ID OUT.jsonl [POLL_S]
    python3 -I scripts/serve8b/lc_watch.py summary OUT.jsonl [FROM_SLOT] [TO_SLOT]
"""
import collections, json, sys, time, urllib.request

def rpc_call(rpc, m, p):
    for _ in range(6):
        try:
            r = urllib.request.Request(rpc, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).encode(), headers={"content-type": "application/json"})
            j = json.load(urllib.request.urlopen(r, timeout=60))
            if "result" in j:
                return j["result"]
        except Exception:
            pass
        time.sleep(1)
    return None

def watch(rpc, prog, out, poll):
    last = None
    while True:
        new, before = [], None
        while True:
            p = {"limit": 1000}
            if before:
                p["before"] = before
            if last:
                p["until"] = last
            page = rpc_call(rpc, "getSignaturesForAddress", [prog, p]) or []
            new += page
            if len(page) < 1000:
                break
            before = page[-1]["signature"]
        if new:
            last = new[0]["signature"]
            with open(out, "a") as f:
                for s in reversed(new):
                    t = rpc_call(rpc, "getTransaction", [s["signature"], {"maxSupportedTransactionVersion": 0, "encoding": "json"}])
                    if not t:
                        f.write(json.dumps({"slot": s["slot"], "sig": s["signature"], "missing": True}) + "\n")
                        continue
                    logs = t["meta"].get("logMessages") or []
                    names = [l.split("Instruction: ")[1] for l in logs if "Instruction: " in l]
                    f.write(json.dumps({"slot": t["slot"], "sig": s["signature"], "fee": t["meta"]["fee"], "cu": t["meta"].get("computeUnitsConsumed"),
                                        "ixs": names, "err": t["meta"].get("err") is not None}) + "\n")
        time.sleep(poll)

def summary(path, lo, hi):
    g = collections.defaultdict(lambda: {"txs": 0, "fee": 0, "cu": 0, "failed": 0})
    missing = 0
    for line in open(path):
        r = json.loads(line)
        if not (lo <= r["slot"] <= hi):
            continue
        if r.get("missing"):
            missing += 1
            continue
        k = r["ixs"][0] if r["ixs"] else "?"
        g[k]["txs"] += 1
        g[k]["fee"] += r["fee"]
        g[k]["cu"] += r["cu"] or 0
        g[k]["failed"] += r["err"]
    print(json.dumps({"txs": sum(x["txs"] for x in g.values()), "fee_lamports": sum(x["fee"] for x in g.values()), "cu": sum(x["cu"] for x in g.values()),
                      "missing": missing, "by_first_instruction": dict(sorted(g.items()))}, indent=1))

if __name__ == "__main__":
    if sys.argv[1] == "watch":
        watch(sys.argv[2], sys.argv[3], sys.argv[4], float(sys.argv[5]) if len(sys.argv) > 5 else 20)
    else:
        summary(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 0, int(sys.argv[4]) if len(sys.argv) > 4 else 1 << 62)
