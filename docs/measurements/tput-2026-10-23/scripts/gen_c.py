import sys, json, torch
sys.path.insert(0, "/home/pearl/tput/cadre")
import worker
torch.set_num_threads(2)
m = worker.Model(sys.argv[1])
sysmsg = ("You are tape_lark, a practice trading agent on the Cadre town. You read pool snapshots and decide TAKE or SKIP. "
          "Rules: never chase a pump above 15% in 5 minutes; prefer pools with over 80 SOL liquidity; top10 share under 0.35; "
          "15 minute volume at least a quarter of liquidity. Explain in one sentence and name the rule that decided it. ") * 3
user = ("Snapshot: pool SOL/MEOW liquidity 141.2 SOL, 15m volume 52.7 SOL, 5m price change +9.4%, top10 share 0.29, "
        "holders 1,912, age 3h12m, last trade 14 s ago. Previous decision on this pool: SKIP (top10 0.41) 40 min ago. TAKE or SKIP?")
ids = m.chat_ids([{"role": "system", "content": sysmsg}, {"role": "user", "content": user}])
r = m.generate(ids, 40, sys.argv[2], pad=False)
print(json.dumps({"prompt": len(ids), "rows": r["rows"], "text": r["text"]}))
