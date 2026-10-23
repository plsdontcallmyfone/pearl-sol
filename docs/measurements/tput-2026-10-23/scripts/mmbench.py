import torch, time, sys
torch.set_num_threads(int(sys.argv[1]))
for n in (8192, 128256):
    w = torch.randint(-64, 65, (n, 2048), dtype=torch.int8)
    for m in (1, 2, 4, 8, 16, 17, 24, 32, 64):
        v = torch.randint(-64, 65, (m, 2048), dtype=torch.int8)
        try:
            c = torch._int_mm(v, w.t()); ok = True
        except Exception as e:
            print(n, m, "ERR", str(e)[:80]); continue
        ref = (v.double() @ w.double().t()).to(torch.int64)
        exact = bool((c.to(torch.int64) == ref).all())
        t0 = time.perf_counter(); k = 5 if n == 128256 else 30
        for _ in range(k): torch._int_mm(v, w.t())
        dt = (time.perf_counter() - t0) / k
        print(n, m, f"{dt*1000:.2f} ms", "exact", exact, flush=True)
