import json, sys
for l in open(sys.argv[1]):
    if l.startswith("{"):
        r = json.loads(l)
        print(r.get("label"), r["shape"], r["rows"], r["threads"], r["step_s"], r["rows_per_s"],
              [(o["op"][6:], o["self_ms"]) for o in r.get("ops", [])[:6]])
