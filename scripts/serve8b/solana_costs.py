#!/usr/bin/env python3
"""Light-client cost of a slot window on a Solana RPC (the local test validator in the serve8b
run, or devnet): every successful tx that touched the light-client program, grouped by the first
light-client instruction its logs name, with fees and CU; plus the JobCommit accounts (313 bytes)
and their rent. Measured figures only.

    python3 -I scripts/serve8b/solana_costs.py RPC_URL LIGHTCLIENT_ID [FROM_SLOT] [TO_SLOT]
"""
import json, sys, time, urllib.request, collections

def main():
    rpc, prog = sys.argv[1], sys.argv[2]
    lo = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    hi = int(sys.argv[4]) if len(sys.argv) > 4 else 1 << 62
    def call(m, p):
        for _ in range(6):
            try:
                r = urllib.request.Request(rpc, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).encode(), headers={"content-type": "application/json"})
                j = json.load(urllib.request.urlopen(r, timeout=60))
                if "result" in j:
                    return j["result"]
            except Exception:
                pass
            time.sleep(2)
        raise SystemExit(f"rpc {m} failed")
    sigs, before = [], None
    while True:
        p = {"limit": 1000}
        if before:
            p["before"] = before
        page = call("getSignaturesForAddress", [prog, p])
        if not page:
            break
        before = page[-1]["signature"]
        sigs += [s for s in page if lo <= s["slot"] <= hi]
        if page[-1]["slot"] < lo:
            break
    groups = collections.defaultdict(lambda: {"txs": 0, "fee": 0, "cu": 0, "failed": 0})
    for s in sigs:
        t = call("getTransaction", [s["signature"], {"maxSupportedTransactionVersion": 0, "encoding": "json"}])
        if not t:
            continue
        logs = t["meta"].get("logMessages") or []
        names = [l.split("Instruction: ")[1] for l in logs if "Instruction: " in l]
        g = groups[names[0] if names else "?"]
        g["txs"] += 1
        g["fee"] += t["meta"]["fee"]
        g["cu"] += t["meta"].get("computeUnitsConsumed") or 0
        if t["meta"].get("err"):
            g["failed"] += 1
    jc = call("getProgramAccounts", [prog, {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}, "filters": [{"dataSize": 313}]}])
    out = {"window_slots": [lo, hi], "txs": sum(g["txs"] for g in groups.values()), "fee_lamports": sum(g["fee"] for g in groups.values()),
           "by_first_instruction": dict(sorted(groups.items())), "jobcommit_accounts": len(jc), "jobcommit_rent_lamports": sum(a["account"]["lamports"] for a in jc)}
    print(json.dumps(out, indent=1))

if __name__ == "__main__":
    main()
